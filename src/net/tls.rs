//! TLS 1.3 клиент (RFC 8446) поверх net::tcp.
//!
//! Реализовано: рукопожатие X25519 + HKDF-SHA256, шифросьют
//! TLS_AES_128_GCM_SHA256 (обязательный для TLS 1.3), ПРОВЕРКА
//! СЕРТИФИКАТА СЕРВЕРА (цепочка X.509 до встроенных корней Mozilla,
//! срок действия, имя хоста — см. net/x509.rs) и подписи
//! CertificateVerify, проверка server Finished, record layer с AEAD,
//! алерты, close_notify.
//!
//! ЧЕСТНЫЕ ОГРАНИЧЕНИЯ (не скрываем):
//! - Отзыв сертификатов не проверяется (нет OCSP/CRL), нет Certificate
//!   Transparency; набор корней — 23 CA (см. x509.rs).
//! - Только TLS 1.3: серверы без его поддержки получают честный отказ.
//! - Только X25519; HelloRetryRequest с другой группой не обслуживается.
//! - ГСЧ ядра (rng.rs) — RDRAND с jitter-фолбэком, не сертифицированный
//!   CSPRNG.

use crate::crypto::gcm::{AesGcm128, TAG_LEN};
use crate::crypto::hkdf::{hkdf_expand, hkdf_extract, hmac_sha256};
use crate::crypto::sha256::{sha256, Sha256};
use crate::crypto::x25519::{x25519, BASEPOINT};
use crate::net::tcp;
use crate::net::x509;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

const CONTENT_CCS: u8 = 20;
const CONTENT_ALERT: u8 = 21;
const CONTENT_HANDSHAKE: u8 = 22;
const CONTENT_APPDATA: u8 = 23;

const HS_CLIENT_HELLO: u8 = 1;
const HS_NEW_SESSION_TICKET: u8 = 4;
const HS_ENCRYPTED_EXTENSIONS: u8 = 8;
const HS_CERTIFICATE: u8 = 11;
const HS_CERT_VERIFY: u8 = 15;
const HS_FINISHED: u8 = 20;
const HS_SERVER_HELLO: u8 = 2;

const CIPHER_AES128_GCM_SHA256: u16 = 0x1301;
const GROUP_X25519: u16 = 0x001d;
const HANDSHAKE_TIMEOUT_MS: u64 = 10_000;

/// Специальный random из RFC 8446 §4.1.3, которым сервер сигналит
/// HelloRetryRequest.
const HRR_RANDOM: [u8; 32] = [
    0xCF, 0x21, 0xAD, 0x74, 0xE5, 0x9A, 0x61, 0x11, 0xBE, 0x1D, 0x8C, 0x02,
    0x1E, 0x65, 0xB8, 0x91, 0xC2, 0xA2, 0x11, 0x16, 0x7A, 0xBB, 0x8C, 0x5E,
    0x07, 0x9E, 0x09, 0xE2, 0xC8, 0xA8, 0x33, 0x9C,
];

/// Стадия рукопожатия и время её начала — для сообщения об ошибке:
/// видно, где именно оно встало и сколько ушло на вычисления.
static STAGE: crate::spinlock::SpinLock<&'static str> = crate::spinlock::SpinLock::new("не начато");
static STAGE_START_MS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static HANDSHAKE_START_MS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

fn mark(stage: &'static str) {
    *STAGE.lock() = stage;
    STAGE_START_MS.store(crate::timer::uptime_ms(), core::sync::atomic::Ordering::Relaxed);
}

fn stage_report() -> String {
    let now = crate::timer::uptime_ms();
    format!(
        "стадия «{}», {} мс в ней, {} мс с начала рукопожатия; {}",
        *STAGE.lock(),
        now.saturating_sub(STAGE_START_MS.load(core::sync::atomic::Ordering::Relaxed)),
        now.saturating_sub(HANDSHAKE_START_MS.load(core::sync::atomic::Ordering::Relaxed)),
        tcp::link_stats()
    )
}

/// Односторонний набор ключей записи (клиентский или серверный).
struct Keys {
    gcm: AesGcm128,
    iv: [u8; 12],
    seq: u64,
}

impl Keys {
    fn from_secret(secret: &[u8; 32]) -> Keys {
        let key_v = hkdf_expand_label(secret, b"key", &[], 16);
        let iv_v = hkdf_expand_label(secret, b"iv", &[], 12);
        let mut key = [0u8; 16];
        key.copy_from_slice(&key_v);
        let mut iv = [0u8; 12];
        iv.copy_from_slice(&iv_v);
        Keys { gcm: AesGcm128::new(&key), iv, seq: 0 }
    }

    /// Nonce по RFC 8446 §5.3: IV XOR порядковый номер записи.
    fn nonce(&self) -> [u8; 12] {
        let mut n = self.iv;
        let seq_be = self.seq.to_be_bytes();
        for k in 0..8 {
            n[4 + k] ^= seq_be[k];
        }
        n
    }
}

/// Поток TLS 1.3 поверх текущего TCP-соединения.
pub struct TlsStream {
    client: Keys,
    server: Keys,
    rx: Vec<u8>,
    closed: bool,
    /// Причина, по которой `recv` последний раз вернул пустой результат.
    last_error: Option<String>,
    /// Принято TLS-записей после рукопожатия.
    records: u32,
}

/// HKDF-Expand-Label (RFC 8446 §7.1).
fn hkdf_expand_label(secret: &[u8; 32], label: &[u8], context: &[u8], len: usize) -> Vec<u8> {
    let mut info = Vec::with_capacity(10 + label.len() + context.len());
    info.extend_from_slice(&(len as u16).to_be_bytes());
    info.push((6 + label.len()) as u8);
    info.extend_from_slice(b"tls13 ");
    info.extend_from_slice(label);
    info.push(context.len() as u8);
    info.extend_from_slice(context);
    hkdf_expand(secret, &info, len)
}

fn derive_secret(secret: &[u8; 32], label: &[u8], transcript_hash: &[u8; 32]) -> [u8; 32] {
    let v = hkdf_expand_label(secret, label, transcript_hash, 32);
    let mut out = [0u8; 32];
    out.copy_from_slice(&v);
    out
}

fn len24(b: &[u8]) -> usize {
    ((b[1] as usize) << 16) | ((b[2] as usize) << 8) | (b[3] as usize)
}

fn alert_text(p: &[u8]) -> String {
    let desc = if p.len() >= 2 { p[1] } else { 255 };
    let name = match desc {
        0 => "close_notify",
        40 => "handshake_failure",
        42 => "bad_certificate",
        47 => "illegal_parameter",
        70 => "protocol_version",
        80 => "internal_error",
        109 => "missing_extension",
        110 => "unsupported_extension",
        112 => "unrecognized_name",
        116 => "certificate_required",
        120 => "no_application_protocol",
        _ => "alert",
    };
    format!("TLS alert от сервера: {} (код {})", name, desc)
}

/// Дочитывает rx из TCP до need байт или до дедлайна.
fn tcp_fill(rx: &mut Vec<u8>, need: usize, deadline_ms: u64) -> bool {
    while rx.len() < need {
        let now = crate::timer::uptime_ms();
        if now >= deadline_ms {
            return false;
        }
        let chunk = tcp::recv((deadline_ms - now).min(1000));
        if !chunk.is_empty() {
            rx.extend_from_slice(&chunk);
        } else if crate::timer::uptime_ms() >= deadline_ms {
            return false;
        }
    }
    true
}

/// Читает один TLS record: (content_type, payload).
fn read_record(rx: &mut Vec<u8>, deadline_ms: u64) -> Result<(u8, Vec<u8>), String> {
    if !tcp_fill(rx, 5, deadline_ms) {
        return Err("TLS: таймаут чтения record".into());
    }
    let rtype = rx[0];
    let len = ((rx[3] as usize) << 8) | rx[4] as usize;
    if len > 16384 + 256 {
        return Err("TLS: record превышает допустимый размер".into());
    }
    if !tcp_fill(rx, 5 + len, deadline_ms) {
        return Err("TLS: record оборван (таймаут)".into());
    }
    let payload = rx[5..5 + len].to_vec();
    rx.drain(..5 + len);
    Ok((rtype, payload))
}

/// Отправляет нешифрованный record.
fn send_record(rtype: u8, ver_minor: u8, payload: &[u8]) -> Result<(), String> {
    let mut rec = Vec::with_capacity(5 + payload.len());
    rec.push(rtype);
    rec.push(3);
    rec.push(ver_minor);
    rec.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    rec.extend_from_slice(payload);
    if !tcp::send(&rec) {
        return Err("TCP: отправка TLS record не удалась".into());
    }
    Ok(())
}

/// Отправляет ClientHello двумя TCP-сегментами, разрезая запись внутри
/// имени хоста (SNI). Приёмнику это не мешает (поток тот же), а DPI,
/// который ищет имя в первом сегменте, имени целиком не видит.
fn send_client_hello(ch: &[u8], host: &str) -> Result<(), String> {
    let mut rec = Vec::with_capacity(5 + ch.len());
    rec.push(CONTENT_HANDSHAKE);
    rec.push(3);
    rec.push(1);
    rec.extend_from_slice(&(ch.len() as u16).to_be_bytes());
    rec.extend_from_slice(ch);
    let name = host.as_bytes();
    let at = if name.len() >= 2 {
        ch.windows(name.len()).position(|w| w == name)
    } else {
        None
    };
    match at {
        Some(p) => {
            let split = 5 + p + name.len() / 2;
            if !tcp::send(&rec[..split]) || !tcp::send(&rec[split..]) {
                return Err("TCP: отправка ClientHello не удалась".into());
            }
        }
        None => {
            if !tcp::send(&rec) {
                return Err("TCP: отправка ClientHello не удалась".into());
            }
        }
    }
    Ok(())
}

/// Шифрует record (RFC 8446 §5.2): inner plaintext = данные || тип.
fn seal_record(keys: &mut Keys, inner_type: u8, payload: &[u8]) -> Vec<u8> {
    let mut inner = Vec::with_capacity(payload.len() + 1);
    inner.extend_from_slice(payload);
    inner.push(inner_type);
    let total = inner.len() + TAG_LEN;
    let aad = [CONTENT_APPDATA, 3, 3, (total >> 8) as u8, (total & 0xff) as u8];
    let nonce = keys.nonce();
    let ct = keys.gcm.seal(&nonce, &aad, &inner);
    keys.seq += 1;
    let mut rec = Vec::with_capacity(5 + ct.len());
    rec.extend_from_slice(&aad);
    rec.extend_from_slice(&ct);
    rec
}

/// Расшифровывает record: (настоящий content type, данные).
fn open_record(keys: &mut Keys, payload: &[u8]) -> Result<(u8, Vec<u8>), String> {
    let aad = [
        CONTENT_APPDATA,
        3,
        3,
        (payload.len() >> 8) as u8,
        (payload.len() & 0xff) as u8,
    ];
    let nonce = keys.nonce();
    let mut plain = keys
        .gcm
        .open(&nonce, &aad, payload)
        .ok_or_else(|| String::from("TLS: AEAD-тег записи не сошёлся"))?;
    keys.seq += 1;
    while plain.last() == Some(&0) {
        plain.pop();
    }
    let itype = plain
        .pop()
        .ok_or_else(|| String::from("TLS: пустой inner plaintext"))?;
    Ok((itype, plain))
}

/// Добавляет расширение ClientHello: тип + длина + тело.
fn push_ext(out: &mut Vec<u8>, etype: u16, body: &[u8]) {
    out.extend_from_slice(&etype.to_be_bytes());
    out.extend_from_slice(&(body.len() as u16).to_be_bytes());
    out.extend_from_slice(body);
}

fn build_client_hello(
    host: &str,
    random: &[u8; 32],
    session_id: &[u8; 32],
    pubkey: &[u8; 32],
) -> Vec<u8> {
    let mut ext = Vec::new();

    // server_name (SNI)
    let hb = host.as_bytes();
    let mut sni = Vec::with_capacity(hb.len() + 5);
    sni.extend_from_slice(&((hb.len() + 3) as u16).to_be_bytes());
    sni.push(0); // host_name
    sni.extend_from_slice(&(hb.len() as u16).to_be_bytes());
    sni.extend_from_slice(hb);
    push_ext(&mut ext, 0x0000, &sni);

    // supported_groups: только X25519
    let mut groups = Vec::with_capacity(4);
    groups.extend_from_slice(&2u16.to_be_bytes());
    groups.extend_from_slice(&GROUP_X25519.to_be_bytes());
    push_ext(&mut ext, 0x000a, &groups);

    // signature_algorithms: предлагаем ТОЛЬКО схемы, подписи которых
    // мы реально проверяем (x509::verify_tls_signature):
    // ecdsa_secp256r1_sha256, ecdsa_secp384r1_sha384, rsa_pss_rsae_sha256.
    let algs: [u16; 3] = [0x0403, 0x0503, 0x0804];
    let mut sig = Vec::with_capacity(2 + algs.len() * 2);
    sig.extend_from_slice(&((algs.len() * 2) as u16).to_be_bytes());
    for a in algs.iter() {
        sig.extend_from_slice(&a.to_be_bytes());
    }
    push_ext(&mut ext, 0x000d, &sig);

    // supported_versions: только 1.3
    push_ext(&mut ext, 0x002b, &[2, 0x03, 0x04]);

    // psk_key_exchange_modes: psk_dhe_ke
    push_ext(&mut ext, 0x002d, &[1, 1]);

    // key_share: X25519
    let mut ks = Vec::with_capacity(38);
    ks.extend_from_slice(&36u16.to_be_bytes());
    ks.extend_from_slice(&GROUP_X25519.to_be_bytes());
    ks.extend_from_slice(&32u16.to_be_bytes());
    ks.extend_from_slice(pubkey);
    push_ext(&mut ext, 0x0033, &ks);

    let mut body = Vec::with_capacity(ext.len() + 80);
    body.extend_from_slice(&[3, 3]); // legacy_version
    body.extend_from_slice(random);
    body.push(32);
    body.extend_from_slice(session_id);
    body.extend_from_slice(&2u16.to_be_bytes());
    body.extend_from_slice(&CIPHER_AES128_GCM_SHA256.to_be_bytes());
    body.push(1); // legacy_compression_methods
    body.push(0);
    body.extend_from_slice(&(ext.len() as u16).to_be_bytes());
    body.extend_from_slice(&ext);

    let mut msg = Vec::with_capacity(4 + body.len());
    msg.push(HS_CLIENT_HELLO);
    let blen = body.len() as u32;
    msg.extend_from_slice(&blen.to_be_bytes()[1..]);
    msg.extend_from_slice(&body);
    msg
}

/// Разбирает ServerHello: возвращает серверный X25519-ключ.
fn parse_server_hello(msg: &[u8]) -> Result<[u8; 32], String> {
    if msg.len() < 4 || msg[0] != HS_SERVER_HELLO {
        return Err("TLS: ожидался ServerHello".into());
    }
    let body = &msg[4..];
    if body.len() < 35 {
        return Err("TLS: ServerHello слишком короткий".into());
    }
    if body[2..34] == HRR_RANDOM {
        return Err(
            "TLS: сервер требует HelloRetryRequest (другая группа ключей) — поддерживается только X25519"
                .into(),
        );
    }
    let sid_len = body[34] as usize;
    let mut off = 35 + sid_len;
    if body.len() < off + 3 {
        return Err("TLS: ServerHello обрезан".into());
    }
    let cipher = u16::from_be_bytes([body[off], body[off + 1]]);
    if cipher != CIPHER_AES128_GCM_SHA256 {
        return Err(format!(
            "TLS: сервер выбрал неподдерживаемый шифронабор 0x{:04x}",
            cipher
        ));
    }
    off += 3; // cipher(2) + compression(1)
    if body.len() < off + 2 {
        return Err("TLS: нет расширений в ServerHello".into());
    }
    let ext_len = u16::from_be_bytes([body[off], body[off + 1]]) as usize;
    off += 2;
    let end = (off + ext_len).min(body.len());

    let mut is_tls13 = false;
    let mut server_pub: Option<[u8; 32]> = None;
    while off + 4 <= end {
        let etype = u16::from_be_bytes([body[off], body[off + 1]]);
        let elen = u16::from_be_bytes([body[off + 2], body[off + 3]]) as usize;
        off += 4;
        if off + elen > end {
            break;
        }
        match etype {
            0x002b if elen >= 2 => {
                if body[off] == 0x03 && body[off + 1] == 0x04 {
                    is_tls13 = true;
                }
            }
            0x0033 if elen >= 36 => {
                let group = u16::from_be_bytes([body[off], body[off + 1]]);
                let klen = u16::from_be_bytes([body[off + 2], body[off + 3]]) as usize;
                if group == GROUP_X25519 && klen == 32 {
                    let mut pk = [0u8; 32];
                    pk.copy_from_slice(&body[off + 4..off + 36]);
                    server_pub = Some(pk);
                }
            }
            _ => {}
        }
        off += elen;
    }

    if !is_tls13 {
        return Err("TLS: сервер не поддерживает TLS 1.3 (выбрал старую версию протокола)".into());
    }
    server_pub.ok_or_else(|| String::from("TLS: в ServerHello нет key_share X25519"))
}

/// Устанавливает TLS 1.3 соединение (TCP + рукопожатие).
pub fn connect(ip: [u8; 4], port: u16, host: &str) -> Result<TlsStream, String> {
    if !tcp::connect(ip, port) {
        return Err(format!(
            "Не удалось установить TCP-соединение с {} ({}.{}.{}.{}:{}): {}",
            host, ip[0], ip[1], ip[2], ip[3], port, tcp::connect_failure()
        ));
    }
    match handshake(host) {
        Ok(stream) => {
            crate::diag::notice(
                crate::diag::NONE,
                &format!(
                    "TLS {}: рукопожатие заняло {} мс",
                    host,
                    crate::timer::uptime_ms()
                        .saturating_sub(HANDSHAKE_START_MS.load(core::sync::atomic::Ordering::Relaxed))
                ),
            );
            Ok(stream)
        }
        Err(e) => {
            let report = stage_report();
            tcp::close();
            Err(format!("{} [{}]", e, report))
        }
    }
}

/// Разбирает Certificate (RFC 8446 §4.4.2): DER-цепочка, лист первым.
fn parse_certificate_msg(msg: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    if msg.len() < 8 {
        return Err("TLS: сообщение Certificate обрезано".into());
    }
    let ctx_len = msg[4] as usize;
    let mut off = 5 + ctx_len;
    if msg.len() < off + 3 {
        return Err("TLS: сообщение Certificate обрезано".into());
    }
    let list_len =
        ((msg[off] as usize) << 16) | ((msg[off + 1] as usize) << 8) | msg[off + 2] as usize;
    off += 3;
    let end = (off + list_len).min(msg.len());
    let mut chain = Vec::new();
    while off + 3 <= end {
        let cert_len =
            ((msg[off] as usize) << 16) | ((msg[off + 1] as usize) << 8) | msg[off + 2] as usize;
        off += 3;
        if off + cert_len > end {
            return Err("TLS: сертификат в списке обрезан".into());
        }
        chain.push(msg[off..off + cert_len].to_vec());
        off += cert_len;
        if off + 2 > end {
            break;
        }
        let ext_len = u16::from_be_bytes([msg[off], msg[off + 1]]) as usize;
        off += 2 + ext_len;
    }
    if chain.is_empty() {
        return Err("TLS: сервер прислал пустой список сертификатов".into());
    }
    Ok(chain)
}

fn handshake(host: &str) -> Result<TlsStream, String> {
    HANDSHAKE_START_MS.store(crate::timer::uptime_ms(), core::sync::atomic::Ordering::Relaxed);
    mark("ключи клиента");
    // Дедлайн ожидания данных от сети. Вычисления (X25519, проверка
    // цепочки и подписей) могут занимать секунды и не должны съедать
    // время, отведённое серверу, поэтому после каждого из них он
    // отсчитывается заново.
    let mut deadline: u64;

    let mut priv_key = [0u8; 32];
    crate::rng::fill_random(&mut priv_key);
    let pubkey = x25519(&priv_key, &BASEPOINT);
    let mut random = [0u8; 32];
    crate::rng::fill_random(&mut random);
    let mut session_id = [0u8; 32];
    crate::rng::fill_random(&mut session_id);

    let ch = build_client_hello(host, &random, &session_id, &pubkey);
    let mut transcript = Sha256::new();
    transcript.update(&ch);
    send_client_hello(&ch, host)?;
    mark("ClientHello отправлен, ждём ServerHello");
    deadline = crate::timer::uptime_ms() + HANDSHAKE_TIMEOUT_MS;

    let mut rx: Vec<u8> = Vec::new();

    // --- ServerHello (единственное нешифрованное handshake-сообщение) ---
    let mut sh_stream: Vec<u8> = Vec::new();
    loop {
        if sh_stream.len() >= 4 && sh_stream.len() >= 4 + len24(&sh_stream) {
            break;
        }
        let (rtype, payload) = read_record(&mut rx, deadline)?;
        match rtype {
            CONTENT_HANDSHAKE => sh_stream.extend_from_slice(&payload),
            CONTENT_ALERT => return Err(alert_text(&payload)),
            CONTENT_CCS => {}
            other => return Err(format!("TLS: неожиданный record {} до ServerHello", other)),
        }
    }
    let sh_len = 4 + len24(&sh_stream);
    let sh: Vec<u8> = sh_stream.drain(..sh_len).collect();
    if !sh_stream.is_empty() {
        return Err("TLS: лишние нешифрованные данные после ServerHello".into());
    }
    let server_pub = parse_server_hello(&sh)?;
    transcript.update(&sh);

    // --- ключевое расписание (RFC 8446 §7.1) ---
    mark("X25519 с ключом сервера");
    let shared = x25519(&priv_key, &server_pub);
    deadline = crate::timer::uptime_ms() + HANDSHAKE_TIMEOUT_MS;
    mark("ждём серверный flight (EncryptedExtensions..Finished)");
    if shared.iter().all(|&b| b == 0) {
        return Err("TLS: X25519 дал нулевой общий секрет".into());
    }
    let zeros = [0u8; 32];
    let empty_hash = sha256(&[]);
    let early = hkdf_extract(&zeros, &zeros);
    let derived = derive_secret(&early, b"derived", &empty_hash);
    let hs_secret = hkdf_extract(&derived, &shared);
    let th_sh = transcript.clone().finalize();
    let c_hs = derive_secret(&hs_secret, b"c hs traffic", &th_sh);
    let s_hs = derive_secret(&hs_secret, b"s hs traffic", &th_sh);
    let mut ckeys = Keys::from_secret(&c_hs);
    let mut skeys = Keys::from_secret(&s_hs);

    // --- серверный flight: EncryptedExtensions..Finished ---
    let mut hs_stream: Vec<u8> = Vec::new();
    let mut server_finished = false;
    let mut leaf_key: Option<x509::PubKey> = None;
    let mut cert_verified = false;
    while !server_finished {
        let (rtype, payload) = read_record(&mut rx, deadline)?;
        match rtype {
            CONTENT_CCS => continue,
            CONTENT_ALERT => return Err(alert_text(&payload)),
            CONTENT_APPDATA => {
                let (itype, plain) = open_record(&mut skeys, &payload)?;
                match itype {
                    CONTENT_HANDSHAKE => hs_stream.extend_from_slice(&plain),
                    CONTENT_ALERT => return Err(alert_text(&plain)),
                    other => {
                        return Err(format!("TLS: неожиданный inner type {} в рукопожатии", other))
                    }
                }
            }
            other => return Err(format!("TLS: неожиданный record {} в рукопожатии", other)),
        }

        while hs_stream.len() >= 4 && hs_stream.len() >= 4 + len24(&hs_stream) {
            let mlen = 4 + len24(&hs_stream);
            let msg: Vec<u8> = hs_stream.drain(..mlen).collect();
            match msg[0] {
                HS_FINISHED => {
                    if !cert_verified {
                        return Err(
                            "TLS: сервер не предъявил сертификат с подписью (PSK-режим не поддерживается)"
                                .into(),
                        );
                    }
                    let th = transcript.clone().finalize();
                    let fk_v = hkdf_expand_label(&s_hs, b"finished", &[], 32);
                    let mut fk = [0u8; 32];
                    fk.copy_from_slice(&fk_v);
                    let expect = hmac_sha256(&fk, &th);
                    if msg.len() != 36 || msg[4..] != expect[..] {
                        return Err("TLS: server Finished не прошёл проверку".into());
                    }
                    transcript.update(&msg);
                    server_finished = true;
                }
                HS_ENCRYPTED_EXTENSIONS => {
                    transcript.update(&msg);
                }
                HS_CERTIFICATE => {
                    let chain = parse_certificate_msg(&msg)?;
                    mark("проверка цепочки сертификатов");
                    x509::verify_chain(&chain, host)
                        .map_err(|e| format!("сертификат {}: {}", host, e))?;
                    leaf_key = Some(x509::leaf_public_key(&chain[0])?);
                    transcript.update(&msg);
                    deadline = crate::timer::uptime_ms() + HANDSHAKE_TIMEOUT_MS;
                    mark("ждём серверный flight (CertificateVerify, Finished)");
                }
                HS_CERT_VERIFY => {
                    // Подпись покрывает транскрипт ПО Certificate
                    // включительно (RFC 8446 §4.4.3) — снимок до update.
                    let th = transcript.clone().finalize();
                    let key = leaf_key
                        .as_ref()
                        .ok_or_else(|| String::from("TLS: CertificateVerify раньше Certificate"))?;
                    if msg.len() < 8 {
                        return Err("TLS: CertificateVerify обрезан".into());
                    }
                    let scheme = u16::from_be_bytes([msg[4], msg[5]]);
                    let sig_len = u16::from_be_bytes([msg[6], msg[7]]) as usize;
                    if msg.len() != 8 + sig_len {
                        return Err("TLS: длина подписи CertificateVerify не сходится".into());
                    }
                    let mut content = Vec::with_capacity(64 + 34 + 32);
                    content.extend_from_slice(&[0x20u8; 64]);
                    content.extend_from_slice(b"TLS 1.3, server CertificateVerify");
                    content.push(0);
                    content.extend_from_slice(&th);
                    mark("проверка подписи CertificateVerify");
                    x509::verify_tls_signature(key, scheme, &content, &msg[8..])
                        .map_err(|e| format!("TLS: {}", e))?;
                    cert_verified = true;
                    transcript.update(&msg);
                    deadline = crate::timer::uptime_ms() + HANDSHAKE_TIMEOUT_MS;
                    mark("ждём серверный Finished");
                }
                other => {
                    return Err(format!("TLS: неожиданное handshake-сообщение {}", other))
                }
            }
        }
    }
    let th_server_fin = transcript.clone().finalize();

    // --- application secrets ---
    let derived2 = derive_secret(&hs_secret, b"derived", &empty_hash);
    let master = hkdf_extract(&derived2, &zeros);
    let c_ap = derive_secret(&master, b"c ap traffic", &th_server_fin);
    let s_ap = derive_secret(&master, b"s ap traffic", &th_server_fin);

    mark("отправка client Finished");
    // --- CCS (совместимость с middlebox) + client Finished ---
    send_record(CONTENT_CCS, 3, &[1])?;
    let fk_v = hkdf_expand_label(&c_hs, b"finished", &[], 32);
    let mut fk = [0u8; 32];
    fk.copy_from_slice(&fk_v);
    let verify = hmac_sha256(&fk, &th_server_fin);
    let mut fin = Vec::with_capacity(36);
    fin.push(HS_FINISHED);
    fin.extend_from_slice(&[0, 0, 32]);
    fin.extend_from_slice(&verify);
    let rec = seal_record(&mut ckeys, CONTENT_HANDSHAKE, &fin);
    if !tcp::send(&rec) {
        return Err("TCP: не удалось отправить client Finished".into());
    }

    Ok(TlsStream {
        client: Keys::from_secret(&c_ap),
        server: Keys::from_secret(&s_ap),
        rx,
        closed: false,
        last_error: None,
        records: 0,
    })
}

impl TlsStream {
    /// Шифрует и отправляет данные приложения.
    pub fn send(&mut self, data: &[u8]) -> Result<(), String> {
        for chunk in data.chunks(1300) {
            let rec = seal_record(&mut self.client, CONTENT_APPDATA, chunk);
            if !tcp::send(&rec) {
                return Err("TCP: отправка данных не удалась".into());
            }
        }
        Ok(())
    }

    /// Возвращает очередную порцию расшифрованных данных приложения.
    /// Пустой Vec — таймаут либо соединение закрыто (как у tcp::recv);
    /// причина сохраняется и доступна через `diagnosis`.
    pub fn recv(&mut self, timeout_ms: u64) -> Vec<u8> {
        if self.closed {
            return Vec::new();
        }
        let deadline = crate::timer::uptime_ms() + timeout_ms;
        loop {
            let (rtype, payload) = match read_record(&mut self.rx, deadline) {
                Ok(r) => r,
                Err(e) => {
                    self.last_error = Some(format!("{} ({})", e, tcp::last_end()));
                    return Vec::new();
                }
            };
            self.records += 1;
            match rtype {
                CONTENT_APPDATA => match open_record(&mut self.server, &payload) {
                    Ok((CONTENT_APPDATA, plain)) => {
                        if !plain.is_empty() {
                            return plain;
                        }
                    }
                    Ok((CONTENT_HANDSHAKE, msg)) => {
                        // Пост-рукопожатные сообщения (NewSessionTicket и
                        // KeyUpdate без запроса) — тикеты игнорируем.
                        if msg.first() == Some(&HS_NEW_SESSION_TICKET) {
                            continue;
                        }
                    }
                    Ok((CONTENT_ALERT, plain)) => {
                        self.last_error = Some(alert_text(&plain));
                        self.closed = true;
                        return Vec::new();
                    }
                    Ok((other, _)) => {
                        self.last_error = Some(format!("неожиданный inner type {}", other));
                        self.closed = true;
                        return Vec::new();
                    }
                    Err(e) => {
                        self.last_error = Some(e);
                        self.closed = true;
                        return Vec::new();
                    }
                },
                CONTENT_ALERT => {
                    self.last_error = Some(alert_text(&payload));
                    self.closed = true;
                    return Vec::new();
                }
                CONTENT_CCS => {}
                other => {
                    self.last_error = Some(format!("неожиданный record {}", other));
                    self.closed = true;
                    return Vec::new();
                }
            }
        }
    }

    /// Человекочитаемая причина отсутствия данных.
    pub fn diagnosis(&self) -> String {
        let why = match &self.last_error {
            Some(e) => e.clone(),
            None => String::from(tcp::last_end()),
        };
        format!(
            "{}; TCP принято {} байт, TLS-записей после рукопожатия {}; {}",
            why,
            tcp::rx_bytes(),
            self.records,
            tcp::link_stats()
        )
    }

    /// Отправляет close_notify и закрывает TCP.
    pub fn close(&mut self) {
        if !self.closed {
            let rec = seal_record(&mut self.client, CONTENT_ALERT, &[1, 0]);
            let _ = tcp::send(&rec);
            self.closed = true;
        }
        tcp::close();
    }
}
