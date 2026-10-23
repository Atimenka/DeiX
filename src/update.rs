//! update — центр обновлений DeiX.
//!
//! ОС берёт у сервера подписанный манифест, скачивает сжатую нагрузку,
//! перезагружается в режим обновления и целиком перезаписывает `/system`
//! вместе с загрузочным регионом; `/userdata` остаётся зашифрованным, его
//! данные преобразуются операциями из манифеста. Формат пакета, порядок
//! фаз и границы надёжности — docs/UPDATE.md.
//!
//! Подпись манифеста — ECDSA P-256 над sha256("DEIX-UPDATE-1\0" || манифест),
//! публичный ключ вшит из build/update_pubkey.bin (нулевой — обновления
//! выключены). Сервер и канал не считаются доверенными.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::println;

const GATEWAY_URL: &str = "https://deix-atimenkas-projects.vercel.app/api/update";
const LANG_HEADER: (&str, &str) = ("X-Deix-Lang", "en");

static UPDATE_PUBKEY: &[u8; 65] = include_bytes!("../build/update_pubkey.bin");
const BUILD_ID: Option<&str> = option_env!("DEIX_BUILD_ID");

const DOMAIN: &[u8] = b"DEIX-UPDATE-1\0";
const SECTOR_SIZE: usize = 512;
const MBR_CODE_BYTES: usize = 446;
const MAX_PAYLOAD: usize = 6 * 1024 * 1024;
const MAX_PARTS: usize = 128;
const PART_RETRIES: u32 = 3;
const WRITE_BATCH_SECTORS: usize = 128;

/// Версия схемы данных `/userdata`, которую понимает это ядро.
pub const USERDATA_SCHEMA: u32 = 1;

const UPDATE_DIR: &str = "/userdata/update";
const STAGED_MANIFEST: &str = "/userdata/update/manifest";
const STAGED_PAYLOAD: &str = "/userdata/update/payload.bin";
const SCHEMA_FILE: &str = "/userdata/.deix-schema";

/// Управляющий сектор лежит вне разделов, в свободной области LBA 2112..4093,
/// и не зашифрован: его читает ядро до открытия тома.
const CONTROL_LBA: u32 = 4000;
const CONTROL_MAGIC: [u8; 8] = *b"DEIXUPDC";
const CONTROL_VERSION: u32 = 1;
const STATE_IDLE: u32 = 0;
const STATE_REQUESTED: u32 = 1;
const STATE_APPLYING: u32 = 2;

const PHASE_NONE: u32 = 0;
const PHASE_SYSTEM: u32 = 1;
const PHASE_BOOT: u32 = 2;
const PHASE_DATA: u32 = 3;

// ==================== разбор манифеста ====================

struct Segment {
    lba: u32,
    sectors: u32,
    mbr_only: bool,
}

struct Chunk {
    size: usize,
    hash: [u8; 32],
}

enum Op {
    Mkdir(String),
    Remove(String),
    Write(String, Vec<u8>),
}

struct Manifest {
    version: String,
    channel: String,
    notes: String,
    build: u64,
    min_build: u64,
    schema: u32,
    chunk_sectors: usize,
    payload_size: usize,
    payload_sha256: [u8; 32],
    segments: Vec<Segment>,
    chunks: Vec<Chunk>,
    ops: Vec<Op>,
}

/// Текст после `"key":` в плоском JSON без вложенных строк со скобками.
fn field_rest<'a>(obj: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("\"{}\"", key);
    let start = obj.find(&pat)? + pat.len();
    obj[start..].trim_start().strip_prefix(':').map(|r| r.trim_start())
}

fn str_field<'a>(obj: &'a str, key: &str) -> Option<&'a str> {
    let inner = field_rest(obj, key)?.strip_prefix('"')?;
    let end = inner.find('"')?;
    Some(&inner[..end])
}

fn num_field(obj: &str, key: &str) -> Option<u64> {
    let rest = field_rest(obj, key)?;
    let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// Объекты `{...}` верхнего уровня массива `"key":[...]`.
fn array_objects<'a>(text: &'a str, key: &str) -> Option<Vec<&'a str>> {
    let rest = field_rest(text, key)?.strip_prefix('[')?;
    let mut out = Vec::new();
    let mut depth = 0u32;
    let mut begin = 0usize;
    for (i, c) in rest.char_indices() {
        match c {
            '{' => {
                if depth == 0 {
                    begin = i;
                }
                depth += 1;
            }
            '}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    out.push(&rest[begin..=i]);
                }
            }
            ']' if depth == 0 => return Some(out),
            _ => {}
        }
    }
    None
}

/// Объект `"key":{...}` целиком, как текст.
fn object_text<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let rest = field_rest(text, key)?;
    if !rest.starts_with('{') {
        return None;
    }
    let mut depth = 0u32;
    for (i, c) in rest.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(&rest[..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

fn hex_nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    }
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    if b.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(b.len() / 2);
    for pair in b.chunks(2) {
        out.push((hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?);
    }
    Some(out)
}

fn hash_field(obj: &str, key: &str) -> Option<[u8; 32]> {
    let bytes = hex_decode(str_field(obj, key)?)?;
    let mut out = [0u8; 32];
    if bytes.len() != 32 {
        return None;
    }
    out.copy_from_slice(&bytes);
    Some(out)
}

fn userdata_path_ok(path: &str) -> bool {
    path.starts_with("/userdata/")
        && path.len() <= 200
        && path.split('/').all(|part| part != "..")
        && path.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'/' | b'-'))
}

fn parse_manifest(text: &str) -> Result<Manifest, String> {
    let bad = |what: &str| format!("manifest: invalid or missing field {}", what);
    if str_field(text, "product") != Some("deix") {
        return Err(String::from("manifest: not a DeiX update"));
    }
    let version = str_field(text, "version").ok_or_else(|| bad("version"))?.to_string();
    let channel = str_field(text, "channel").ok_or_else(|| bad("channel"))?.to_string();
    let notes = str_field(text, "notes").unwrap_or("").to_string();
    let build = num_field(text, "build").ok_or_else(|| bad("build"))?;
    let min_build = num_field(text, "min_build").ok_or_else(|| bad("min_build"))?;
    let schema = num_field(text, "schema").ok_or_else(|| bad("schema"))? as u32;
    let chunk_sectors = num_field(text, "chunk_sectors").ok_or_else(|| bad("chunk_sectors"))? as usize;
    let payload_size = num_field(text, "payload_size").ok_or_else(|| bad("payload_size"))? as usize;
    let payload_sha256 = hash_field(text, "payload_sha256").ok_or_else(|| bad("payload_sha256"))?;
    if chunk_sectors == 0 || chunk_sectors > 1024 {
        return Err(bad("chunk_sectors"));
    }
    if payload_size == 0 || payload_size > MAX_PAYLOAD {
        return Err(String::from("manifest: payload size is outside the supported range"));
    }

    let mut segments = Vec::new();
    for obj in array_objects(text, "segments").ok_or_else(|| bad("segments"))? {
        segments.push(Segment {
            lba: num_field(obj, "lba").ok_or_else(|| bad("segments.lba"))? as u32,
            sectors: num_field(obj, "sectors").ok_or_else(|| bad("segments.sectors"))? as u32,
            mbr_only: match str_field(obj, "mode") {
                Some("mbr") => true,
                Some("raw") => false,
                _ => return Err(bad("segments.mode")),
            },
        });
    }
    let mut chunks = Vec::new();
    for obj in array_objects(text, "chunks").ok_or_else(|| bad("chunks"))? {
        chunks.push(Chunk {
            size: num_field(obj, "c").ok_or_else(|| bad("chunks.c"))? as usize,
            hash: hash_field(obj, "h").ok_or_else(|| bad("chunks.h"))?,
        });
    }
    let mut ops = Vec::new();
    for obj in array_objects(text, "userdata_ops").ok_or_else(|| bad("userdata_ops"))? {
        let path = str_field(obj, "path").ok_or_else(|| bad("userdata_ops.path"))?;
        if !userdata_path_ok(path) {
            return Err(String::from("manifest: an operation path is outside /userdata"));
        }
        let path = path.to_string();
        ops.push(match str_field(obj, "op") {
            Some("mkdir") => Op::Mkdir(path),
            Some("remove") => Op::Remove(path),
            Some("write") => {
                let data = hex_decode(str_field(obj, "hex").ok_or_else(|| bad("userdata_ops.hex"))?)
                    .ok_or_else(|| bad("userdata_ops.hex"))?;
                Op::Write(path, data)
            }
            _ => return Err(bad("userdata_ops.op")),
        });
    }

    let manifest = Manifest {
        version,
        channel,
        notes,
        build,
        min_build,
        schema,
        chunk_sectors,
        payload_size,
        payload_sha256,
        segments,
        chunks,
        ops,
    };
    check_layout(&manifest)?;
    Ok(manifest)
}

/// Известная раскладка диска: код MBR, загрузочный регион и `/system`.
/// Всё остальное пакет трогать не вправе, даже подписанный.
fn expected_segments() -> Result<[(u32, u32, bool); 3], String> {
    let system = crate::partition_map::lookup_layout("/system")
        .ok_or_else(|| String::from("the /system partition is not in the partition map"))?;
    let boot_sectors = crate::partition_map::KERNEL_IMAGE_INFO_LBA - crate::partition_map::BOOTLOADER_LBA + 1;
    Ok([
        (0, 1, true),
        (crate::partition_map::BOOTLOADER_LBA, boot_sectors, false),
        (system.start_lba, system.sectors, false),
    ])
}

fn check_layout(m: &Manifest) -> Result<(), String> {
    let expected = expected_segments()?;
    let same = m.segments.len() == expected.len()
        && m.segments
            .iter()
            .zip(expected.iter())
            .all(|(s, e)| s.lba == e.0 && s.sectors == e.1 && s.mbr_only == e.2);
    if !same {
        return Err(String::from(
            "the package disk layout differs from this system: a reinstall is required",
        ));
    }
    let raw_total = raw_total_bytes(m);
    let chunk_bytes = m.chunk_sectors * SECTOR_SIZE;
    if m.chunks.len() != raw_total.div_ceil(chunk_bytes) {
        return Err(String::from("manifest: the chunk list does not match the segments"));
    }
    let framed: usize = m.chunks.iter().map(|c| c.size + 4).sum();
    if framed != m.payload_size {
        return Err(String::from("manifest: chunk sizes do not add up to the payload size"));
    }
    Ok(())
}

fn raw_total_bytes(m: &Manifest) -> usize {
    m.segments.iter().map(|s| s.sectors as usize * SECTOR_SIZE).sum()
}

// ==================== подпись ====================

fn is_enabled() -> bool {
    UPDATE_PUBKEY.iter().any(|&b| b != 0)
}

fn installed_build() -> u64 {
    BUILD_ID.and_then(|s| s.parse::<u64>().ok()).unwrap_or(0)
}

fn verify_signature(signature_hex: &str, manifest_text: &str) -> Result<(), String> {
    let sig = hex_decode(signature_hex)
        .filter(|s| s.len() == 64)
        .ok_or_else(|| String::from("the signature is not 64 bytes of hex"))?;
    let mut message: Vec<u8> = Vec::with_capacity(DOMAIN.len() + manifest_text.len());
    message.extend_from_slice(DOMAIN);
    message.extend_from_slice(manifest_text.as_bytes());
    let digest = crate::crypto::sha256::sha256(&message);
    let curve = crate::crypto::ecdsa::p256();
    if crate::crypto::ecdsa::verify(&curve, &UPDATE_PUBKEY[..], &digest, &sig[..32], &sig[32..]) {
        Ok(())
    } else {
        Err(String::from("the manifest signature is not valid for this system's update key"))
    }
}

/// Манифест и подпись, проверенные ключом ядра.
struct Signed {
    manifest: Manifest,
    text: String,
    signature: String,
}

fn verify_signed(signature_hex: &str, manifest_text: &str) -> Result<Signed, String> {
    verify_signature(signature_hex, manifest_text)?;
    let manifest = parse_manifest(manifest_text)?;
    Ok(Signed { manifest, text: manifest_text.to_string(), signature: signature_hex.to_string() })
}

// ==================== сервер ====================

fn clip(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn http_error(what: &str, resp: &crate::net::http::HttpBytes) -> String {
    let text = String::from_utf8_lossy(&resp.body).into_owned();
    match str_field(&text, "error") {
        Some(e) => format!("{}: HTTP {}: {}", what, resp.status, e),
        None => format!("{}: HTTP {}", what, resp.status),
    }
}

struct Offer {
    signed: Signed,
    parts: usize,
    part_size: usize,
}

/// Манифест последней сборки канала; `None` — сервер ничего не публиковал.
fn fetch_offer() -> Result<Option<Offer>, String> {
    let url = format!("{}?channel=stable", GATEWAY_URL);
    let resp = crate::net::http::request_bytes("GET", &url, &[LANG_HEADER], &[])?;
    if resp.status != 200 {
        return Err(http_error("update server", &resp));
    }
    let text = String::from_utf8_lossy(&resp.body).into_owned();
    if text.contains("\"none\"") && !text.contains("\"manifest\"") {
        return Ok(None);
    }
    let manifest_text = object_text(&text, "manifest")
        .ok_or_else(|| format!("unexpected server response: {}", clip(&text, 120)))?;
    let signature = text
        .rfind("\"signature\"")
        .and_then(|i| str_field(&text[i..], "signature"))
        .ok_or_else(|| String::from("the server response has no signature"))?;
    let signed = verify_signed(signature, manifest_text)?;
    let parts = num_field(&text[text.rfind("\"parts\"").unwrap_or(0)..], "parts").unwrap_or(0) as usize;
    let part_size = num_field(&text[text.rfind("\"part_size\"").unwrap_or(0)..], "part_size").unwrap_or(0) as usize;
    if part_size == 0 || parts == 0 || parts > MAX_PARTS || parts != signed.manifest.payload_size.div_ceil(part_size) {
        return Err(String::from("the server announced a part layout that does not match the signed manifest"));
    }
    Ok(Some(Offer { signed, parts, part_size }))
}

fn download_part(build: u64, index: usize, expected: usize) -> Result<Vec<u8>, String> {
    let url = format!("{}?build={}&part={}", GATEWAY_URL, build, index);
    let mut last = String::new();
    for attempt in 1..=PART_RETRIES {
        match crate::net::http::request_bytes("GET", &url, &[LANG_HEADER], &[]) {
            Ok(resp) if resp.status == 200 && resp.body.len() == expected => return Ok(resp.body),
            Ok(resp) if resp.status == 200 => {
                last = format!("part {} has {} bytes, expected {}", index, resp.body.len(), expected);
            }
            Ok(resp) => {
                last = http_error("part download", &resp);
                if resp.status == 404 || resp.status == 429 {
                    break;
                }
            }
            Err(e) => last = e,
        }
        println!("  [update] part {} failed (attempt {}/{}): {}", index, attempt, PART_RETRIES, last);
    }
    Err(last)
}

// ==================== управляющий сектор ====================

#[derive(Clone, Copy)]
struct Control {
    state: u32,
    phase: u32,
    build: u64,
    manifest_hash: [u8; 32],
}

fn control_checksum(sector: &[u8]) -> [u8; 4] {
    let d = crate::crypto::sha256::sha256(&sector[..60]);
    [d[0], d[1], d[2], d[3]]
}

fn read_control() -> Option<Control> {
    let mut buf = [0u8; SECTOR_SIZE];
    crate::ata::read_sectors(CONTROL_LBA, 1, &mut buf).ok()?;
    if buf[..8] != CONTROL_MAGIC || buf[60..64] != control_checksum(&buf) {
        return None;
    }
    if u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]) != CONTROL_VERSION {
        return None;
    }
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&buf[28..60]);
    Some(Control {
        state: u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]),
        phase: u32::from_le_bytes([buf[16], buf[17], buf[18], buf[19]]),
        build: u64::from_le_bytes([buf[20], buf[21], buf[22], buf[23], buf[24], buf[25], buf[26], buf[27]]),
        manifest_hash: hash,
    })
}

fn write_control(c: &Control) -> Result<(), String> {
    let mut buf = [0u8; SECTOR_SIZE];
    buf[..8].copy_from_slice(&CONTROL_MAGIC);
    buf[8..12].copy_from_slice(&CONTROL_VERSION.to_le_bytes());
    buf[12..16].copy_from_slice(&c.state.to_le_bytes());
    buf[16..20].copy_from_slice(&c.phase.to_le_bytes());
    buf[20..28].copy_from_slice(&c.build.to_le_bytes());
    buf[28..60].copy_from_slice(&c.manifest_hash);
    let sum = control_checksum(&buf);
    buf[60..64].copy_from_slice(&sum);
    crate::ata::write_sectors(CONTROL_LBA, 1, &buf).map_err(|_| String::from("cannot write the update control sector"))
}

fn idle_control() -> Control {
    Control { state: STATE_IDLE, phase: PHASE_NONE, build: 0, manifest_hash: [0u8; 32] }
}

/// Ждёт ли обновление применения (запрошено или прервано посреди работы).
pub fn pending() -> bool {
    matches!(read_control(), Some(c) if c.state != STATE_IDLE)
}

// ==================== пакет на диске ====================

fn staged_text() -> Result<(String, String), String> {
    let bytes = crate::vfs::read_file(STAGED_MANIFEST).map_err(|e| e.message())?;
    let text = String::from_utf8(bytes).map_err(|_| String::from("the staged manifest is not text"))?;
    let mut lines = text.split('\n');
    if lines.next() != Some("DEIXMANIFEST1") {
        return Err(String::from("the staged manifest has no DEIXMANIFEST1 header"));
    }
    let signature = lines.next().ok_or_else(|| String::from("the staged manifest has no signature"))?;
    let manifest = lines.next().ok_or_else(|| String::from("the staged manifest has no body"))?;
    Ok((signature.to_string(), manifest.to_string()))
}

fn manifest_file_bytes(signed: &Signed) -> Vec<u8> {
    format!("DEIXMANIFEST1\n{}\n{}\n", signed.signature, signed.text).into_bytes()
}

fn load_staged() -> Result<Signed, String> {
    let (signature, manifest) = staged_text()?;
    verify_signed(&signature, &manifest)
}

fn read_payload(m: &Manifest) -> Result<Vec<u8>, String> {
    let payload = crate::vfs::read_file(STAGED_PAYLOAD).map_err(|e| e.message())?;
    if payload.len() != m.payload_size {
        return Err(String::from("the staged payload has the wrong size"));
    }
    if crate::crypto::sha256::sha256(&payload) != m.payload_sha256 {
        return Err(String::from("the staged payload does not match the signed sha256"));
    }
    Ok(payload)
}

fn clear_staged() {
    for path in [STAGED_PAYLOAD, STAGED_MANIFEST] {
        if crate::vfs::exists(path) {
            let _ = crate::vfs::remove(path);
        }
    }
}

// ==================== предусловия ====================

fn check_target() -> Result<(), String> {
    if crate::ramdisk::is_active() {
        return Err(String::from("the system runs from a RAM disk (live mode): there is no installed disk to update"));
    }
    if crate::ext2::is_formatted_vol(crate::ext2::VolId::System) {
        return Err(String::from("/system was converted to ext2: the update replaces an EROFS image"));
    }
    let end = expected_segments()?[2].0 + expected_segments()?[2].1;
    match crate::ata::sector_count(crate::ata::Drive::Master) {
        Some(n) if n >= end => Ok(()),
        Some(_) => Err(String::from("the disk is smaller than the system layout")),
        None => Err(String::from("cannot read the disk size")),
    }
}

fn ensure_newer(m: &Manifest) -> Result<(), String> {
    let installed = installed_build();
    if installed != 0 && m.build <= installed {
        return Err(format!("build {} is already installed (server has {})", installed, m.build));
    }
    if installed != 0 && m.min_build > installed {
        return Err(format!(
            "this update needs build {} or newer first (installed: {}): install the intermediate release",
            m.min_build, installed
        ));
    }
    Ok(())
}

// ==================== команды ====================

pub fn cmd_update(arg: &str) {
    let arg = arg.trim();
    let (word, rest) = match arg.find(char::is_whitespace) {
        Some(i) => (&arg[..i], arg[i..].trim()),
        None => (arg, ""),
    };
    match word {
        "check" => cmd_check(),
        "download" => cmd_download(),
        "apply" => cmd_apply(rest),
        "cancel" => cmd_cancel(),
        _ => cmd_status(),
    }
}

fn require_enabled() -> bool {
    if is_enabled() {
        return true;
    }
    println!("  [update] this build has no update key, updates are disabled.");
    println!("  [update] generate one with tools/make_update.py keygen and rebuild (docs/UPDATE.md).");
    false
}

fn cmd_status() {
    println!("  [update] installed: {} (build {})", crate::OS_RELEASE, match installed_build() {
        0 => String::from("unknown"),
        n => n.to_string(),
    });
    println!("  [update] data schema: kernel {}, /userdata {}", USERDATA_SCHEMA, data_schema());
    println!("  [update] update key: {}", if is_enabled() { "present" } else { "absent (updates disabled)" });
    match read_control() {
        Some(c) if c.state == STATE_REQUESTED => println!("  [update] state: apply requested for build {}, runs after the next login", c.build),
        Some(c) if c.state == STATE_APPLYING => println!("  [update] state: applying build {} (phase {} of 3 done), continues after the next login", c.build, c.phase),
        _ => println!("  [update] state: idle"),
    }
    match load_staged() {
        Ok(s) => println!(
            "  [update] downloaded: {} (build {}, {} KiB), run 'update apply yes'",
            s.manifest.version,
            s.manifest.build,
            s.manifest.payload_size / 1024
        ),
        Err(_) => println!("  [update] downloaded: nothing"),
    }
    println!("  [update] commands: update check | download | apply yes | cancel");
}

fn cmd_check() {
    if !require_enabled() {
        return;
    }
    match fetch_offer() {
        Ok(None) => println!("  [update] the server has no published update."),
        Ok(Some(offer)) => {
            let m = &offer.signed.manifest;
            println!("  [update] server: {} (build {}, channel {}), signature valid", m.version, m.build, m.channel);
            if !m.notes.is_empty() {
                println!("  [update] notes: {}", m.notes);
            }
            println!("  [update] payload: {} KiB in {} parts", m.payload_size / 1024, offer.parts);
            match ensure_newer(m) {
                Ok(()) => println!("  [update] a newer build is available: run 'update download'."),
                Err(e) => println!("  [update] {}", e),
            }
        }
        Err(e) => println!("  [update] check failed: {}", e),
    }
}

fn cmd_download() {
    if !require_enabled() {
        return;
    }
    if pending() {
        println!("  [update] an update is already waiting to be applied; finish it or run 'update cancel'.");
        return;
    }
    if let Err(e) = check_target() {
        println!("  [update] cannot update this system: {}", e);
        return;
    }
    let offer = match fetch_offer() {
        Ok(Some(o)) => o,
        Ok(None) => {
            println!("  [update] the server has no published update.");
            return;
        }
        Err(e) => {
            println!("  [update] check failed: {}", e);
            return;
        }
    };
    if let Err(e) = ensure_newer(&offer.signed.manifest) {
        println!("  [update] {}", e);
        return;
    }
    if let Ok(staged) = load_staged() {
        if staged.manifest.build == offer.signed.manifest.build && read_payload(&staged.manifest).is_ok() {
            println!("  [update] build {} is already downloaded and verified: run 'update apply yes'.", staged.manifest.build);
            return;
        }
    }
    match download_all(&offer) {
        Ok(()) => {
            println!("  [update] downloaded and verified: {} (build {}).", offer.signed.manifest.version, offer.signed.manifest.build);
            println!("  [update] run 'update apply yes': the system reboots into update mode.");
            println!("  [update] you will enter the same password; /system is replaced, your data is kept.");
        }
        Err(e) => {
            println!("  [update] download failed: {}", e);
            clear_staged();
        }
    }
}

fn download_all(offer: &Offer) -> Result<(), String> {
    let m = &offer.signed.manifest;
    let mut payload: Vec<u8> = Vec::with_capacity(m.payload_size);
    for index in 0..offer.parts {
        let expected = core::cmp::min(offer.part_size, m.payload_size - index * offer.part_size);
        let part = download_part(m.build, index, expected)?;
        payload.extend_from_slice(&part);
        if index % 8 == 7 || index + 1 == offer.parts {
            println!("  [update] downloaded {} of {} KiB", payload.len() / 1024, m.payload_size / 1024);
        }
    }
    if crate::crypto::sha256::sha256(&payload) != m.payload_sha256 {
        return Err(String::from("the payload does not match the signed sha256"));
    }
    println!("  [update] sha256 matches the signed manifest, saving to {}", UPDATE_DIR);
    crate::vfs::mkdir(UPDATE_DIR).map_err(|e| e.message())?;
    crate::vfs::write_file(STAGED_PAYLOAD, &payload).map_err(|e| {
        format!("{} (is there enough free space on /userdata for {} KiB?)", e.message(), m.payload_size / 1024)
    })?;
    crate::vfs::write_file(STAGED_MANIFEST, &manifest_file_bytes(&offer.signed)).map_err(|e| e.message())
}

fn cmd_apply(arg: &str) {
    if !require_enabled() {
        return;
    }
    if arg != "yes" {
        println!("  [update] this reboots into update mode and replaces /system and the boot region.");
        println!("  [update] a power loss during the kernel write can leave the disk unbootable (docs/UPDATE.md).");
        println!("  [update] if you accept that, run 'update apply yes'.");
        return;
    }
    if pending() {
        println!("  [update] an update is already waiting; run 'update status'.");
        return;
    }
    if let Err(e) = check_target() {
        println!("  [update] cannot update this system: {}", e);
        return;
    }
    let staged = match load_staged() {
        Ok(s) => s,
        Err(_) => {
            println!("  [update] nothing is downloaded: run 'update download' first.");
            return;
        }
    };
    if let Err(e) = ensure_newer(&staged.manifest) {
        println!("  [update] {}", e);
        return;
    }
    if let Err(e) = read_payload(&staged.manifest) {
        println!("  [update] the downloaded payload is damaged: {}", e);
        return;
    }
    let control = Control {
        state: STATE_REQUESTED,
        phase: PHASE_NONE,
        build: staged.manifest.build,
        manifest_hash: crate::crypto::sha256::sha256(&manifest_file_bytes(&staged)),
    };
    if let Err(e) = write_control(&control) {
        println!("  [update] {}", e);
        return;
    }
    println!("  [update] update mode requested for build {}. Rebooting...", staged.manifest.build);
    crate::cli::cmd_reboot();
}

fn cmd_cancel() {
    match read_control() {
        Some(c) if c.state == STATE_APPLYING => {
            println!("  [update] the update is partially applied: cancelling would leave /system and the kernel mismatched.");
            println!("  [update] reboot and sign in to finish it.");
            return;
        }
        Some(c) if c.state == STATE_REQUESTED => {
            if let Err(e) = write_control(&idle_control()) {
                println!("  [update] {}", e);
                return;
            }
        }
        _ => {}
    }
    clear_staged();
    println!("  [update] cancelled, the downloaded package was removed.");
}

// ==================== схема /userdata ====================

fn data_schema() -> u32 {
    match crate::vfs::read_file(SCHEMA_FILE) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).trim().parse().unwrap_or(1),
        Err(_) => 1,
    }
}

// ==================== режим обновления ====================

/// Вызывается после входа, когда том уже открыт. Возвращает управление только
/// если обновление отменено до первой записи на диск и загрузку можно
/// продолжать; иначе завершается перезагрузкой.
pub fn run_apply_mode() {
    let control = match read_control() {
        Some(c) if c.state != STATE_IDLE => c,
        _ => return,
    };
    println!();
    println!("  ==== DeiX update mode ====");
    println!("  Do not turn the power off until the system restarts.");
    match apply(control) {
        Ok(()) => {
            println!("  [update] done. Rebooting into the new system...");
            crate::cli::cmd_reboot();
        }
        Err(e) => {
            println!("  [update] FAILED: {}", e);
            match read_control() {
                Some(c) if c.state == STATE_APPLYING => {
                    println!("  [update] the update is partially applied. Press any key to reboot and resume.");
                    let _ = crate::keyboard::read_char();
                    crate::cli::cmd_reboot();
                }
                _ => {
                    let _ = write_control(&idle_control());
                    println!("  [update] nothing was changed on the disk, continuing normal boot.");
                }
            }
        }
    }
}

fn apply(control: Control) -> Result<(), String> {
    let signed = load_staged()?;
    let m = &signed.manifest;
    if crate::crypto::sha256::sha256(&manifest_file_bytes(&signed)) != control.manifest_hash || m.build != control.build {
        return Err(String::from("the staged package is not the one that was requested"));
    }
    if control.state == STATE_REQUESTED {
        ensure_newer(m)?;
    }
    check_target()?;
    println!("  [update] package: {} (build {}), checking the payload", m.version, m.build);
    let payload = read_payload(m)?;
    println!("  [update] checking every chunk before the first write");
    walk_chunks(m, &payload, |_, _| Ok(()))?;

    let mut state = control;
    state.state = STATE_APPLYING;
    if state.phase < PHASE_SYSTEM {
        write_control(&state)?;
        println!("  [update] writing /system");
        {
            let _batch = crate::ata::FlushBatch::begin();
            walk_chunks(m, &payload, |offset, data| write_range(m, offset, data, Target::System))?;
        }
        crate::vfs::remount_system();
        state.phase = PHASE_SYSTEM;
        write_control(&state)?;
    }
    if state.phase < PHASE_BOOT {
        println!("  [update] writing the boot region and the kernel");
        let mut mbr: Vec<u8> = Vec::new();
        {
            let _batch = crate::ata::FlushBatch::begin();
            walk_chunks(m, &payload, |offset, data| {
                write_range(m, offset, data, Target::Boot)?;
                collect_mbr(m, offset, data, &mut mbr);
                Ok(())
            })?;
        }
        write_mbr_code(&mbr)?;
        state.phase = PHASE_BOOT;
        write_control(&state)?;
    }
    drop(payload);
    if state.phase < PHASE_DATA {
        println!("  [update] converting /userdata ({} operations)", m.ops.len());
        run_ops(m)?;
        state.phase = PHASE_DATA;
        write_control(&state)?;
    }
    clear_staged();
    write_control(&idle_control())?;
    Ok(())
}

/// Распаковывает кусок за куском, сверяет sha256 каждого и отдаёт
/// `(смещение в сыром потоке, данные)` обработчику.
fn walk_chunks<F>(m: &Manifest, payload: &[u8], mut each: F) -> Result<(), String>
where
    F: FnMut(usize, &[u8]) -> Result<(), String>,
{
    let raw_total = raw_total_bytes(m);
    let chunk_bytes = m.chunk_sectors * SECTOR_SIZE;
    let mut pos = 0usize;
    let mut offset = 0usize;
    for (index, chunk) in m.chunks.iter().enumerate() {
        if pos + 4 > payload.len() {
            return Err(String::from("the payload ends inside a chunk header"));
        }
        let len = u32::from_le_bytes([payload[pos], payload[pos + 1], payload[pos + 2], payload[pos + 3]]) as usize;
        if len != chunk.size || pos + 4 + len > payload.len() {
            return Err(format!("chunk {} has an unexpected size", index));
        }
        let (raw, truncated) = crate::net::inflate::gunzip(&payload[pos + 4..pos + 4 + len])
            .map_err(|e| format!("chunk {}: {}", index, e))?;
        let want = core::cmp::min(chunk_bytes, raw_total - offset);
        if truncated || raw.len() != want {
            return Err(format!("chunk {} unpacked to {} bytes, expected {}", index, raw.len(), want));
        }
        if crate::crypto::sha256::sha256(&raw) != chunk.hash {
            return Err(format!("chunk {} does not match its sha256", index));
        }
        each(offset, &raw)?;
        pos += 4 + len;
        offset += raw.len();
    }
    if pos != payload.len() || offset != raw_total {
        return Err(String::from("the payload has trailing data"));
    }
    Ok(())
}

enum Target {
    System,
    Boot,
}

/// Части данных, попадающие в сегмент выбранной фазы, пишутся на диск блоками,
/// секторы, уже совпадающие с диском, пропускаются.
fn write_range(m: &Manifest, offset: usize, data: &[u8], target: Target) -> Result<(), String> {
    let mut seg_start = 0usize;
    for seg in &m.segments {
        let seg_bytes = seg.sectors as usize * SECTOR_SIZE;
        let seg_end = seg_start + seg_bytes;
        let wanted = !seg.mbr_only
            && match target {
                Target::System => seg.lba == crate::partition_map::lookup_layout("/system").map(|l| l.start_lba).unwrap_or(0),
                Target::Boot => seg.lba == crate::partition_map::BOOTLOADER_LBA,
            };
        let lo = core::cmp::max(offset, seg_start);
        let hi = core::cmp::min(offset + data.len(), seg_end);
        if wanted && lo < hi {
            let piece = &data[lo - offset..hi - offset];
            let first_sector = seg.lba as usize + (lo - seg_start) / SECTOR_SIZE;
            write_sectors_skipping_equal(first_sector as u32, piece)?;
        }
        seg_start = seg_end;
    }
    Ok(())
}

fn write_sectors_skipping_equal(first_lba: u32, data: &[u8]) -> Result<(), String> {
    let mut disk = alloc::vec![0u8; WRITE_BATCH_SECTORS * SECTOR_SIZE];
    let mut done = 0usize;
    while done < data.len() {
        let bytes = core::cmp::min(WRITE_BATCH_SECTORS * SECTOR_SIZE, data.len() - done);
        let sectors = (bytes / SECTOR_SIZE) as u8;
        let lba = first_lba + (done / SECTOR_SIZE) as u32;
        let block = &data[done..done + bytes];
        let same = crate::ata::read_sectors(lba, sectors, &mut disk).is_ok() && disk[..bytes] == *block;
        if !same {
            crate::ata::write_sectors(lba, sectors, block).map_err(|_| format!("disk write failed at LBA {}", lba))?;
        }
        done += bytes;
    }
    Ok(())
}

fn collect_mbr(m: &Manifest, offset: usize, data: &[u8], mbr: &mut Vec<u8>) {
    let mut seg_start = 0usize;
    for seg in &m.segments {
        let seg_end = seg_start + seg.sectors as usize * SECTOR_SIZE;
        if seg.mbr_only && offset <= seg_start && offset + data.len() >= seg_start + MBR_CODE_BYTES {
            mbr.clear();
            mbr.extend_from_slice(&data[seg_start - offset..seg_start - offset + MBR_CODE_BYTES]);
        }
        seg_start = seg_end;
    }
}

/// Код MBR заменяется последним: таблица разделов и подпись 0x55AA остаются
/// теми, что у этого диска.
fn write_mbr_code(code: &[u8]) -> Result<(), String> {
    if code.len() != MBR_CODE_BYTES {
        return Err(String::from("the package has no MBR code segment"));
    }
    let mut sector = [0u8; SECTOR_SIZE];
    crate::ata::read_sectors(0, 1, &mut sector).map_err(|_| String::from("cannot read the MBR"))?;
    if sector[SECTOR_SIZE - 2] != 0x55 || sector[SECTOR_SIZE - 1] != 0xAA {
        return Err(String::from("the disk MBR has no 0x55AA signature: refusing to touch it"));
    }
    if sector[..MBR_CODE_BYTES] == *code {
        return Ok(());
    }
    sector[..MBR_CODE_BYTES].copy_from_slice(code);
    crate::ata::write_sectors(0, 1, &sector).map_err(|_| String::from("cannot write the MBR"))
}

fn run_ops(m: &Manifest) -> Result<(), String> {
    for op in &m.ops {
        match op {
            Op::Mkdir(path) => crate::vfs::mkdir(path).map_err(|e| e.message())?,
            Op::Remove(path) => {
                if crate::vfs::exists(path) {
                    crate::vfs::remove(path).map_err(|e| e.message())?;
                }
            }
            Op::Write(path, data) => crate::vfs::write_file(path, data).map_err(|e| e.message())?,
        }
    }
    crate::vfs::write_file(SCHEMA_FILE, m.schema.to_string().as_bytes()).map_err(|e| e.message())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "{\"build\":1790000000,\"channel\":\"stable\",\"chunk_sectors\":1024,\"chunks\":[{\"c\":10,\"h\":\"00\"},{\"c\":20,\"h\":\"11\"}],\"min_build\":5,\"notes\":\"a b\",\"product\":\"deix\",\"segments\":[{\"lba\":0,\"mode\":\"mbr\",\"sectors\":1},{\"lba\":1,\"mode\":\"raw\",\"sectors\":2047}],\"userdata_ops\":[],\"version\":\"0.4.0\"}";

    #[test]
    fn reads_flat_fields_without_confusing_similar_keys() {
        assert_eq!(num_field(SAMPLE, "build"), Some(1790000000));
        assert_eq!(num_field(SAMPLE, "min_build"), Some(5));
        assert_eq!(str_field(SAMPLE, "channel"), Some("stable"));
        assert_eq!(str_field(SAMPLE, "notes"), Some("a b"));
        assert_eq!(str_field(SAMPLE, "missing"), None);
    }

    #[test]
    fn splits_arrays_into_objects() {
        let chunks = array_objects(SAMPLE, "chunks").unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(num_field(chunks[1], "c"), Some(20));
        let segments = array_objects(SAMPLE, "segments").unwrap();
        assert_eq!(str_field(segments[0], "mode"), Some("mbr"));
        assert_eq!(array_objects(SAMPLE, "userdata_ops").unwrap().len(), 0);
    }

    #[test]
    fn extracts_the_embedded_manifest_object() {
        let envelope = format!("{{\"manifest\":{},\"parts\":3,\"signature\":\"ab\"}}", SAMPLE);
        assert_eq!(object_text(&envelope, "manifest"), Some(SAMPLE));
    }

    #[test]
    fn hex_and_paths() {
        assert_eq!(hex_decode("00ff10"), Some(alloc::vec![0, 255, 16]));
        assert_eq!(hex_decode("0"), None);
        assert_eq!(hex_decode("zz"), None);
        assert!(userdata_path_ok("/userdata/home/a-b_c.txt"));
        assert!(!userdata_path_ok("/userdata/../etc"));
        assert!(!userdata_path_ok("/system/x"));
        assert!(!userdata_path_ok("/userdata/a b"));
    }
}
