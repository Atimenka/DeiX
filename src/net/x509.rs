//! Разбор сертификатов X.509 (DER, RFC 5280) и проверка цепочки доверия
//! для TLS 1.3.
//!
//! Что проверяется по-настоящему:
//! - подпись каждого сертификата ключом следующего (RSA PKCS#1 v1.5
//!   c SHA-256/384/512, ECDSA P-256/P-384 c SHA-256/384);
//! - цепочка до корня из встроенного набора (23 актуальных корневых CA
//!   из бандла Mozilla: ISRG/Let's Encrypt, Google Trust Services,
//!   DigiCert, GlobalSign, Amazon, Sectigo/USERTrust, Microsoft);
//! - срок действия по часам RTC;
//! - имя хоста по subjectAltName dNSName (включая *.wildcard);
//! - CA=true (basicConstraints) у промежуточных сертификатов.
//!
//! ЧЕСТНЫЕ ОГРАНИЧЕНИЯ: нет OCSP/CRL (отзыв не проверяется), нет
//! Certificate Transparency, name constraints и политик; цепочка
//! обрабатывается линейно (без перебора кросс-подписей); iPAddress в
//! SAN не поддержан (https по IP-литералу даёт честную ошибку).

use crate::crypto::ecdsa;
use crate::crypto::rsa;
use crate::crypto::sha256::sha256;
use crate::crypto::sha512::{sha384, sha512};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Встроенные корневые CA (DER из актуального бандла Mozilla/certifi).
static ROOT_DERS: [&[u8]; 23] = [
    include_bytes!("../data/roots/amazon_1.der"),
    include_bytes!("../data/roots/amazon_3.der"),
    include_bytes!("../data/roots/digicert_g2.der"),
    include_bytes!("../data/roots/digicert_g3.der"),
    include_bytes!("../data/roots/digicert_g4.der"),
    include_bytes!("../data/roots/digicert_g5_ecc.der"),
    include_bytes!("../data/roots/digicert_g5_rsa.der"),
    include_bytes!("../data/roots/globalsign_e4.der"),
    include_bytes!("../data/roots/globalsign_e46.der"),
    include_bytes!("../data/roots/globalsign_e5.der"),
    include_bytes!("../data/roots/globalsign_r3.der"),
    include_bytes!("../data/roots/globalsign_r46.der"),
    include_bytes!("../data/roots/globalsign_r6.der"),
    include_bytes!("../data/roots/gts_r1.der"),
    include_bytes!("../data/roots/gts_r4.der"),
    include_bytes!("../data/roots/isrg_x1.der"),
    include_bytes!("../data/roots/isrg_x2.der"),
    include_bytes!("../data/roots/microsoft_ecc.der"),
    include_bytes!("../data/roots/microsoft_rsa.der"),
    include_bytes!("../data/roots/sectigo_e46.der"),
    include_bytes!("../data/roots/sectigo_r46.der"),
    include_bytes!("../data/roots/usertrust_ecc.der"),
    include_bytes!("../data/roots/usertrust_rsa.der"),
];

// --- OID (значения без заголовка 06 xx) ---
const OID_RSA_ENCRYPTION: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
const OID_SHA256_RSA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b];
const OID_SHA384_RSA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0c];
const OID_SHA512_RSA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0d];
const OID_EC_PUBLIC_KEY: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
const OID_P256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];
const OID_P384: &[u8] = &[0x2b, 0x81, 0x04, 0x00, 0x22];
const OID_ECDSA_SHA256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02];
const OID_ECDSA_SHA384: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x03];
const OID_EXT_SAN: &[u8] = &[0x55, 0x1d, 0x11];
const OID_EXT_BASIC_CONSTRAINTS: &[u8] = &[0x55, 0x1d, 0x13];

#[derive(Clone, Copy, PartialEq)]
pub enum SigAlg {
    RsaSha256,
    RsaSha384,
    RsaSha512,
    EcdsaSha256,
    EcdsaSha384,
}

#[derive(Clone, Copy, PartialEq)]
pub enum EcCurve {
    P256,
    P384,
}

pub enum PubKey {
    Rsa { n: Vec<u8>, e: Vec<u8> },
    Ec { curve: EcCurve, point: Vec<u8> },
}

/// Дата-время как кортеж для прямого сравнения.
pub type DateTime = (u16, u8, u8, u8, u8, u8);

pub struct ParsedCert {
    /// Сырые байты tbsCertificate (включая заголовок TLV) — то, что
    /// хэшируется при проверке подписи.
    pub tbs_raw: Vec<u8>,
    pub sig_alg: SigAlg,
    pub signature: Vec<u8>,
    /// Сырые TLV имён — сравниваются побайтово.
    pub issuer: Vec<u8>,
    pub subject: Vec<u8>,
    pub not_before: DateTime,
    pub not_after: DateTime,
    pub key: PubKey,
    pub san_dns: Vec<String>,
    pub is_ca: bool,
}

/// Минимальный DER-ридер: тег в один байт, длина коротким или длинным
/// (до 4 байт) видом.
struct Der<'a> {
    d: &'a [u8],
    pos: usize,
}

impl<'a> Der<'a> {
    fn new(d: &'a [u8]) -> Self {
        Der { d, pos: 0 }
    }

    fn done(&self) -> bool {
        self.pos >= self.d.len()
    }

    fn peek_tag(&self) -> Option<u8> {
        self.d.get(self.pos).copied()
    }

    /// Читает TLV: (tag, value, raw TLV целиком).
    fn tlv(&mut self) -> Result<(u8, &'a [u8], &'a [u8]), String> {
        let start = self.pos;
        if self.pos + 2 > self.d.len() {
            return Err("DER: обрыв на заголовке TLV".into());
        }
        let tag = self.d[self.pos];
        self.pos += 1;
        let first = self.d[self.pos];
        self.pos += 1;
        let len = if first < 0x80 {
            first as usize
        } else {
            let n = (first & 0x7f) as usize;
            if n == 0 || n > 4 || self.pos + n > self.d.len() {
                return Err("DER: некорректная длина".into());
            }
            let mut l = 0usize;
            for _ in 0..n {
                l = (l << 8) | self.d[self.pos] as usize;
                self.pos += 1;
            }
            l
        };
        if self.pos + len > self.d.len() {
            return Err("DER: длина выходит за границы данных".into());
        }
        let value = &self.d[self.pos..self.pos + len];
        self.pos += len;
        Ok((tag, value, &self.d[start..self.pos]))
    }
}

/// UTCTime (YYMMDDHHMMSSZ) или GeneralizedTime (YYYYMMDDHHMMSSZ).
fn parse_time(tag: u8, v: &[u8]) -> Result<DateTime, String> {
    let digits = |b: &[u8]| -> Result<u16, String> {
        let mut acc: u16 = 0;
        for &c in b {
            if !(b'0'..=b'9').contains(&c) {
                return Err("DER: нецифровой символ в дате".into());
            }
            acc = acc * 10 + (c - b'0') as u16;
        }
        Ok(acc)
    };
    match tag {
        0x17 => {
            if v.len() != 13 || v[12] != b'Z' {
                return Err("DER: неподдерживаемый формат UTCTime".into());
            }
            let yy = digits(&v[0..2])?;
            let year = if yy < 50 { 2000 + yy } else { 1900 + yy };
            Ok((
                year,
                digits(&v[2..4])? as u8,
                digits(&v[4..6])? as u8,
                digits(&v[6..8])? as u8,
                digits(&v[8..10])? as u8,
                digits(&v[10..12])? as u8,
            ))
        }
        0x18 => {
            if v.len() != 15 || v[14] != b'Z' {
                return Err("DER: неподдерживаемый формат GeneralizedTime".into());
            }
            Ok((
                digits(&v[0..4])?,
                digits(&v[4..6])? as u8,
                digits(&v[6..8])? as u8,
                digits(&v[8..10])? as u8,
                digits(&v[10..12])? as u8,
                digits(&v[12..14])? as u8,
            ))
        }
        _ => Err("DER: неожиданный тег времени".into()),
    }
}

/// INTEGER: убирает ведущий нулевой байт (знаковый бит DER).
fn int_bytes(v: &[u8]) -> &[u8] {
    if v.len() > 1 && v[0] == 0 {
        &v[1..]
    } else {
        v
    }
}

fn parse_sig_alg(seq_value: &[u8]) -> Result<SigAlg, String> {
    let mut d = Der::new(seq_value);
    let (tag, oid, _) = d.tlv()?;
    if tag != 0x06 {
        return Err("DER: в AlgorithmIdentifier нет OID".into());
    }
    if oid == OID_SHA256_RSA {
        Ok(SigAlg::RsaSha256)
    } else if oid == OID_SHA384_RSA {
        Ok(SigAlg::RsaSha384)
    } else if oid == OID_SHA512_RSA {
        Ok(SigAlg::RsaSha512)
    } else if oid == OID_ECDSA_SHA256 {
        Ok(SigAlg::EcdsaSha256)
    } else if oid == OID_ECDSA_SHA384 {
        Ok(SigAlg::EcdsaSha384)
    } else {
        Err("сертификат подписан неподдерживаемым алгоритмом (поддержаны: RSA-SHA256/384/512, ECDSA-SHA256/384)".into())
    }
}

fn parse_spki(spki_value: &[u8]) -> Result<PubKey, String> {
    let mut d = Der::new(spki_value);
    let (_, alg_value, _) = d.tlv()?; // AlgorithmIdentifier
    let (tag, key_bits, _) = d.tlv()?; // BIT STRING
    if tag != 0x03 || key_bits.is_empty() || key_bits[0] != 0 {
        return Err("DER: некорректный BIT STRING ключа".into());
    }
    let key_data = &key_bits[1..];

    let mut a = Der::new(alg_value);
    let (tag, oid, _) = a.tlv()?;
    if tag != 0x06 {
        return Err("DER: в SPKI нет OID алгоритма".into());
    }
    if oid == OID_RSA_ENCRYPTION {
        let mut k = Der::new(key_data);
        let (t, rsa_seq, _) = k.tlv()?;
        if t != 0x30 {
            return Err("DER: ключ RSA не SEQUENCE".into());
        }
        let mut r = Der::new(rsa_seq);
        let (tn, n, _) = r.tlv()?;
        let (te, e, _) = r.tlv()?;
        if tn != 0x02 || te != 0x02 {
            return Err("DER: модуль/экспонента RSA не INTEGER".into());
        }
        Ok(PubKey::Rsa {
            n: int_bytes(n).to_vec(),
            e: int_bytes(e).to_vec(),
        })
    } else if oid == OID_EC_PUBLIC_KEY {
        let (t, curve_oid, _) = a.tlv()?;
        if t != 0x06 {
            return Err("DER: нет OID кривой EC-ключа".into());
        }
        let curve = if curve_oid == OID_P256 {
            EcCurve::P256
        } else if curve_oid == OID_P384 {
            EcCurve::P384
        } else {
            return Err("EC-ключ на неподдерживаемой кривой (поддержаны P-256, P-384)".into());
        };
        Ok(PubKey::Ec {
            curve,
            point: key_data.to_vec(),
        })
    } else {
        Err("неподдерживаемый тип открытого ключа (поддержаны RSA и EC)".into())
    }
}

fn parse_extensions(ext_seq: &[u8], san_dns: &mut Vec<String>, is_ca: &mut bool) -> Result<(), String> {
    let mut d = Der::new(ext_seq);
    while !d.done() {
        let (tag, ext, _) = d.tlv()?;
        if tag != 0x30 {
            continue;
        }
        let mut e = Der::new(ext);
        let (t, oid, _) = e.tlv()?;
        if t != 0x06 {
            continue;
        }
        // critical BOOLEAN опционален
        let mut next = e.tlv()?;
        if next.0 == 0x01 {
            next = e.tlv()?;
        }
        if next.0 != 0x04 {
            continue; // extnValue обязан быть OCTET STRING
        }
        let value = next.1;
        if oid == OID_EXT_SAN {
            let mut s = Der::new(value);
            let (t, names, _) = s.tlv()?;
            if t != 0x30 {
                continue;
            }
            let mut n = Der::new(names);
            while !n.done() {
                let (ntag, nval, _) = n.tlv()?;
                if ntag == 0x82 {
                    // dNSName (IA5String)
                    let name = String::from_utf8_lossy(nval).to_ascii_lowercase();
                    san_dns.push(name);
                }
            }
        } else if oid == OID_EXT_BASIC_CONSTRAINTS {
            let mut s = Der::new(value);
            let (t, bc, _) = s.tlv()?;
            if t == 0x30 {
                let mut b = Der::new(bc);
                if b.peek_tag() == Some(0x01) {
                    let (_, v, _) = b.tlv()?;
                    *is_ca = v.first() == Some(&0xff);
                }
            }
        }
    }
    Ok(())
}

/// Разбирает сертификат X.509 v3 из DER.
pub fn parse_cert(der: &[u8]) -> Result<ParsedCert, String> {
    let mut top = Der::new(der);
    let (tag, cert_body, _) = top.tlv()?;
    if tag != 0x30 {
        return Err("DER: сертификат не SEQUENCE".into());
    }
    let mut c = Der::new(cert_body);
    let (t, tbs_value, tbs_raw) = c.tlv()?;
    if t != 0x30 {
        return Err("DER: tbsCertificate не SEQUENCE".into());
    }
    let (t, outer_alg, _) = c.tlv()?;
    if t != 0x30 {
        return Err("DER: нет signatureAlgorithm".into());
    }
    let sig_alg = parse_sig_alg(outer_alg)?;
    let (t, sig_bits, _) = c.tlv()?;
    if t != 0x03 || sig_bits.is_empty() || sig_bits[0] != 0 {
        return Err("DER: подпись не BIT STRING".into());
    }
    let signature = sig_bits[1..].to_vec();

    let mut tbs = Der::new(tbs_value);
    if tbs.peek_tag() == Some(0xa0) {
        tbs.tlv()?; // version [0]
    }
    tbs.tlv()?; // serialNumber
    tbs.tlv()?; // signature (внутренний AlgorithmIdentifier)
    let (_, _, issuer_raw) = tbs.tlv()?;
    let (t, validity, _) = tbs.tlv()?;
    if t != 0x30 {
        return Err("DER: Validity не SEQUENCE".into());
    }
    let mut v = Der::new(validity);
    let (t1, nb, _) = v.tlv()?;
    let (t2, na, _) = v.tlv()?;
    let not_before = parse_time(t1, nb)?;
    let not_after = parse_time(t2, na)?;
    let (_, _, subject_raw) = tbs.tlv()?;
    let (t, spki_value, _) = tbs.tlv()?;
    if t != 0x30 {
        return Err("DER: нет SubjectPublicKeyInfo".into());
    }
    let key = parse_spki(spki_value)?;

    let mut san_dns = Vec::new();
    let mut is_ca = false;
    while !tbs.done() {
        let (tag, ext_value, _) = tbs.tlv()?;
        if tag == 0xa3 {
            // extensions [3]: внутри SEQUENCE OF Extension
            let mut x = Der::new(ext_value);
            let (t, ext_seq, _) = x.tlv()?;
            if t == 0x30 {
                parse_extensions(ext_seq, &mut san_dns, &mut is_ca)?;
            }
        }
    }

    Ok(ParsedCert {
        tbs_raw: tbs_raw.to_vec(),
        sig_alg,
        signature,
        issuer: issuer_raw.to_vec(),
        subject: subject_raw.to_vec(),
        not_before,
        not_after,
        key,
        san_dns,
        is_ca,
    })
}

/// Подпись ECDSA: DER SEQUENCE { r INTEGER, s INTEGER }.
fn parse_ecdsa_sig(sig: &[u8]) -> Result<(Vec<u8>, Vec<u8>), String> {
    let mut d = Der::new(sig);
    let (t, seq, _) = d.tlv()?;
    if t != 0x30 {
        return Err("DER: подпись ECDSA не SEQUENCE".into());
    }
    let mut s = Der::new(seq);
    let (tr, r, _) = s.tlv()?;
    let (ts, sv, _) = s.tlv()?;
    if tr != 0x02 || ts != 0x02 {
        return Err("DER: r/s не INTEGER".into());
    }
    Ok((int_bytes(r).to_vec(), int_bytes(sv).to_vec()))
}

/// Проверяет подпись child ключом signer_key.
fn verify_signed_by(child: &ParsedCert, signer_key: &PubKey) -> Result<(), String> {
    let ok = match (child.sig_alg, signer_key) {
        (SigAlg::RsaSha256, PubKey::Rsa { n, e }) => {
            rsa::verify_pkcs1_sha256(n, e, &child.signature, &sha256(&child.tbs_raw))
        }
        (SigAlg::RsaSha384, PubKey::Rsa { n, e }) => {
            rsa::verify_pkcs1_sha384(n, e, &child.signature, &sha384(&child.tbs_raw))
        }
        (SigAlg::RsaSha512, PubKey::Rsa { n, e }) => {
            rsa::verify_pkcs1_sha512(n, e, &child.signature, &sha512(&child.tbs_raw))
        }
        (SigAlg::EcdsaSha256, PubKey::Ec { curve, point }) => {
            let (r, s) = parse_ecdsa_sig(&child.signature)?;
            let c = match curve {
                EcCurve::P256 => ecdsa::p256(),
                EcCurve::P384 => ecdsa::p384(),
            };
            ecdsa::verify(&c, point, &sha256(&child.tbs_raw), &r, &s)
        }
        (SigAlg::EcdsaSha384, PubKey::Ec { curve, point }) => {
            let (r, s) = parse_ecdsa_sig(&child.signature)?;
            let c = match curve {
                EcCurve::P256 => ecdsa::p256(),
                EcCurve::P384 => ecdsa::p384(),
            };
            ecdsa::verify(&c, point, &sha384(&child.tbs_raw), &r, &s)
        }
        _ => return Err("тип подписи не соответствует типу ключа издателя".into()),
    };
    if ok {
        Ok(())
    } else {
        Err("криптографическая подпись сертификата не сошлась".into())
    }
}

/// Ищет издателя в встроенном наборе корневых CA (по subject DN).
fn find_root(issuer: &[u8]) -> Option<ParsedCert> {
    for der in ROOT_DERS.iter() {
        if let Ok(root) = parse_cert(der) {
            if root.subject == issuer {
                return Some(root);
            }
        }
    }
    None
}

/// Сопоставление имени хоста с dNSName из SAN (включая *.wildcard,
/// который покрывает ровно одну левую метку — RFC 6125 §6.4.3).
fn hostname_matches(host: &str, names: &[String]) -> bool {
    for name in names {
        if let Some(suffix) = name.strip_prefix("*.") {
            if let Some((label, rest)) = host.split_once('.') {
                if !label.is_empty() && rest == suffix {
                    return true;
                }
            }
        } else if name == host {
            return true;
        }
    }
    false
}

fn fmt_date(d: &DateTime) -> String {
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
        d.0, d.1, d.2, d.3, d.4, d.5
    )
}

/// Проверяет цепочку сертификатов сервера (лист первым, как в TLS).
pub fn verify_chain(chain_der: &[Vec<u8>], host: &str) -> Result<(), String> {
    if chain_der.is_empty() {
        return Err("сервер не прислал сертификатов".into());
    }

    let host = host.trim().to_ascii_lowercase();
    let is_ip = !host.is_empty()
        && host.split('.').count() == 4
        && host.chars().all(|c| c.is_ascii_digit() || c == '.');
    if is_ip {
        return Err(
            "проверка сертификата по IP-адресу не поддерживается — откройте сайт по имени хоста"
                .into(),
        );
    }

    let mut certs = Vec::with_capacity(chain_der.len());
    for (i, der) in chain_der.iter().enumerate() {
        certs.push(
            parse_cert(der).map_err(|e| format!("сертификат #{} в цепочке: {}", i + 1, e))?,
        );
    }

    // Часы: без правдоподобного RTC проверить срок действия нельзя.
    if !crate::rtc::is_plausible() {
        return Err(
            "часы RTC не установлены — срок действия сертификата проверить невозможно".into(),
        );
    }
    let wc = crate::rtc::now();
    let now: DateTime = (wc.year, wc.month, wc.day, wc.hour, wc.minute, wc.second);

    // Имя хоста — по листовому сертификату.
    let leaf = &certs[0];
    if leaf.san_dns.is_empty() {
        return Err("в сертификате сервера нет subjectAltName dNSName".into());
    }
    if !hostname_matches(&host, &leaf.san_dns) {
        return Err(format!(
            "сертификат выдан не для этого хоста (в SAN: {})",
            leaf.san_dns.join(", ")
        ));
    }

    for (i, cert) in certs.iter().enumerate() {
        if now < cert.not_before {
            return Err(format!(
                "сертификат #{} ещё не действителен (notBefore {})",
                i + 1,
                fmt_date(&cert.not_before)
            ));
        }
        if now > cert.not_after {
            return Err(format!(
                "сертификат #{} истёк ({})",
                i + 1,
                fmt_date(&cert.not_after)
            ));
        }

        // Издатель в корневом хранилище? Тогда цепочка замкнулась.
        if let Some(root) = find_root(&cert.issuer) {
            if now > root.not_after {
                return Err("корневой сертификат из встроенного набора истёк".into());
            }
            verify_signed_by(cert, &root.key)
                .map_err(|e| format!("подпись сертификата #{} корнем: {}", i + 1, e))?;
            return Ok(());
        }

        // Иначе — следующий в цепочке обязан быть его издателем.
        let next = certs.get(i + 1).ok_or_else(|| {
            String::from(
                "цепочка не доходит до доверенного корневого CA (издатель не во встроенном наборе из 23 корней Mozilla)",
            )
        })?;
        if next.subject != cert.issuer {
            return Err(format!(
                "разрыв цепочки: issuer сертификата #{} не совпадает с subject #{}",
                i + 1,
                i + 2
            ));
        }
        if !next.is_ca {
            return Err(format!(
                "сертификат #{} не является CA, но подписывает другие",
                i + 2
            ));
        }
        verify_signed_by(cert, &next.key)
            .map_err(|e| format!("подпись сертификата #{}: {}", i + 1, e))?;
    }
    Err("цепочка не доходит до доверенного корневого CA".into())
}

/// Разбирает лист и возвращает его открытый ключ — для проверки
/// CertificateVerify в TLS 1.3.
pub fn leaf_public_key(leaf_der: &[u8]) -> Result<PubKey, String> {
    Ok(parse_cert(leaf_der)?.key)
}

/// Проверка подписи CertificateVerify (RFC 8446 §4.4.3).
/// scheme — SignatureScheme из TLS; content — подписанные байты.
pub fn verify_tls_signature(
    key: &PubKey,
    scheme: u16,
    content: &[u8],
    sig: &[u8],
) -> Result<(), String> {
    let ok = match (scheme, key) {
        // rsa_pss_rsae_sha256
        (0x0804, PubKey::Rsa { n, e }) => rsa::verify_pss_sha256(n, e, sig, &sha256(content)),
        // ecdsa_secp256r1_sha256
        (0x0403, PubKey::Ec { curve: EcCurve::P256, point }) => {
            let (r, s) = parse_ecdsa_sig(sig)?;
            ecdsa::verify(&ecdsa::p256(), point, &sha256(content), &r, &s)
        }
        // ecdsa_secp384r1_sha384
        (0x0503, PubKey::Ec { curve: EcCurve::P384, point }) => {
            let (r, s) = parse_ecdsa_sig(sig)?;
            ecdsa::verify(&ecdsa::p384(), point, &sha384(content), &r, &s)
        }
        _ => {
            return Err(format!(
                "сервер выбрал неподдерживаемую схему подписи 0x{:04x}",
                scheme
            ))
        }
    };
    if ok {
        Ok(())
    } else {
        Err("подпись CertificateVerify не сошлась".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_and_selfverify_all_roots() {
        // Каждый встроенный корень разбирается и его самоподпись
        // проверяется его же ключом (корни самоподписаны).
        for (i, der) in ROOT_DERS.iter().enumerate() {
            let c = parse_cert(der).unwrap_or_else(|e| panic!("root #{}: {}", i, e));
            assert_eq!(c.subject, c.issuer, "root #{} не самоподписан", i);
            assert!(c.is_ca, "root #{} без CA=true", i);
            verify_signed_by(&c, &c.key)
                .unwrap_or_else(|e| panic!("root #{} самоподпись: {}", i, e));
        }
    }

    #[test]
    fn test_hostname_matching() {
        let names = alloc::vec![
            String::from("example.com"),
            String::from("*.example.org"),
        ];
        assert!(hostname_matches("example.com", &names));
        assert!(hostname_matches("www.example.org", &names));
        assert!(!hostname_matches("example.org", &names));
        assert!(!hostname_matches("a.b.example.org", &names));
        assert!(!hostname_matches("badexample.com", &names));
    }
}
