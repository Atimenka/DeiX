//! Журналирование событий: точки входа, фильтрация и немедленный вывод.
//!
//! Критический путь (сборка записи → кольцевой буфер → serial) не обращается
//! к куче. Запись на диск выполняется отдельно и отложенно, поэтому из
//! обработчика прерывания диск не опрашивается.

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use super::code::ErrorCode;
use super::record::{Action, ErrorRecord};
use super::severity::Severity;
use super::ring;

/// Порог журналирования. События ниже этого уровня не регистрируются.
/// По умолчанию `Info`; в диагностическом режиме снижается до `Trace`.
static THRESHOLD: AtomicU8 = AtomicU8::new(Severity::Info as u8);

/// Диагностический режим: включены `Trace` и `Debug`.
static DIAGNOSTIC_MODE: AtomicBool = AtomicBool::new(false);

/// Идёт ли сейчас загрузка (до запуска оболочки).
///
/// Пока флаг взведён, события уровня Notice и выше дублируются на текстовую
/// консоль. После запуска оболочки консольный вывод прекращается: CLI и GUI
/// сами решают, что показывать, а печать из ядра ломала бы их вёрстку.
static BOOTING: AtomicBool = AtomicBool::new(true);

/// Текущий уровень журналирования.
pub fn threshold() -> Severity {
    Severity::from_level(THRESHOLD.load(Ordering::Relaxed))
}

/// Устанавливает уровень журналирования.
pub fn set_threshold(sev: Severity) {
    THRESHOLD.store(sev as u8, Ordering::Relaxed);
}

/// Включён ли диагностический режим.
pub fn diagnostic_mode() -> bool {
    DIAGNOSTIC_MODE.load(Ordering::Relaxed)
}

/// Переключает диагностический режим.
///
/// В диагностическом режиме порог опускается до `Trace`, в обычном
/// возвращается к `Info`.
pub fn set_diagnostic_mode(on: bool) {
    DIAGNOSTIC_MODE.store(on, Ordering::Relaxed);
    set_threshold(if on { Severity::Trace } else { Severity::Info });
}

/// Находится ли система в фазе загрузки.
pub fn booting() -> bool {
    BOOTING.load(Ordering::Relaxed)
}

/// Отмечает завершение фазы загрузки.
pub fn finish_boot() {
    BOOTING.store(false, Ordering::Relaxed);
}

/// Проходит ли событие текущий фильтр.
///
/// События уровня `Error` и выше регистрируются всегда: фильтр режимов
/// относится к подробностям, а не к ошибкам.
pub fn passes(sev: Severity) -> bool {
    sev.is_error() || sev >= threshold()
}

/// Основная точка регистрации события.
///
/// Возвращает идентификатор записи или `None`, если событие отфильтровано.
pub fn log(sev: Severity, code: ErrorCode, action: Action, message: &str) -> Option<u32> {
    log_ctx(sev, code, action, message, 0, 0, 0, 0)
}

/// Регистрация события с контекстом процесса и адресом.
pub fn log_ctx(
    sev: Severity,
    code: ErrorCode,
    action: Action,
    message: &str,
    pid: u32,
    tid: u32,
    cpu: u8,
    address: u64,
) -> Option<u32> {
    if !passes(sev) {
        return None;
    }

    let mut record = ErrorRecord::empty();
    record.severity = sev;
    record.code = code;
    record.pid = pid;
    record.tid = tid;
    record.cpu = cpu;
    record.address = address;
    record.action = action;
    record.set_message(message);

    let now = crate::timer::uptime_ms();
    let id = ring::push(&mut record, now);

    emit(&record);
    Some(id)
}

/// Регистрация события со ссылкой на причину (цепочка связанных сбоев).
pub fn log_related(
    sev: Severity,
    code: ErrorCode,
    action: Action,
    message: &str,
    cause_id: u32,
) -> Option<u32> {
    if !passes(sev) {
        return None;
    }
    let mut record = ErrorRecord::empty();
    record.severity = sev;
    record.code = code;
    record.action = action;
    record.related_id = cause_id;
    record.set_message(message);

    let now = crate::timer::uptime_ms();
    let id = ring::push(&mut record, now);
    emit(&record);
    Some(id)
}

/// Немедленный вывод в serial и, при включённой консоли, на экран.
///
/// Serial доступен с самых ранних этапов загрузки и не требует ни кучи, ни
/// файловой системы, поэтому именно он является основным каналом.
fn emit(record: &ErrorRecord) {
    // Serial: компактная машиночитаемая строка.
    let mut line = [0u8; 160];
    let n = format_compact(record, &mut line);
    for &b in &line[..n] {
        crate::serial::write_byte(b);
    }
    crate::serial::write_byte(b'\r');
    crate::serial::write_byte(b'\n');

    // Текстовая консоль: только до запуска оболочки, чтобы не перерисовывать окна.
    if record.severity >= Severity::Notice && booting() {
        crate::println!("{}", core::str::from_utf8(&line[..n]).unwrap_or(""));
    }
}

/// Собирает машиночитаемую строку `key=value` без обращения к куче.
///
/// Формат совпадает с тем, что пишется в файлы `/userdata/log/*.log`,
/// поэтому одну и ту же функцию используют и serial, и диск.
pub fn format_compact(record: &ErrorRecord, out: &mut [u8]) -> usize {
    let mut w = Writer::new(out);

    w.str("ts=");
    w.u64(record.timestamp_ms);
    w.str(" sev=");
    w.str(record.severity.tag());
    w.str(" code=");
    w.code(record.code);
    w.str(" pid=");
    w.u64(record.pid as u64);
    w.str(" tid=");
    w.u64(record.tid as u64);
    w.str(" cpu=");
    w.u64(record.cpu as u64);
    if record.address != 0 {
        w.str(" addr=0x");
        w.hex(record.address);
    }
    if record.related_id != 0 {
        w.str(" cause=");
        w.u64(record.related_id as u64);
    }
    if record.occurrences > 1 {
        w.str(" count=");
        w.u64(record.occurrences as u64);
    }
    w.str(" action=");
    w.str(record.action.tag());
    w.str(" msg=\"");
    w.escaped(record.message());
    w.str("\"");

    w.len
}

/// Человекочитаемая строка для консоли и Центра ошибок.
pub fn format_human(record: &ErrorRecord) -> alloc::string::String {
    let mut out = alloc::string::String::new();
    out.push_str(&format_time(record.timestamp_ms));
    out.push(' ');
    out.push_str(&padded(record.severity.tag(), 6));
    out.push(' ');
    out.push_str(&record.code.as_string());
    out.push(' ');
    out.push_str(record.message());
    if record.occurrences > 1 {
        use core::fmt::Write;
        let _ = write!(out, "  (×{})", record.occurrences);
    }
    out
}

/// Время события в виде `mm:ss.mmm`.
pub fn format_time(ms: u64) -> alloc::string::String {
    let total_sec = ms / 1000;
    let millis = ms % 1000;
    let min = (total_sec / 60) % 60;
    let sec = total_sec % 60;
    alloc::format!("[{:02}:{:02}.{:03}]", min, sec, millis)
}

/// Дополняет строку пробелами до указанной ширины.
fn padded(text: &str, width: usize) -> alloc::string::String {
    let mut s = alloc::string::String::from(text);
    while s.len() < width {
        s.push(' ');
    }
    s
}

/// Писатель в фиксированный буфер: позволяет собирать строки без кучи.
struct Writer<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl<'a> Writer<'a> {
    fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, len: 0 }
    }

    fn byte(&mut self, b: u8) {
        if self.len < self.buf.len() {
            self.buf[self.len] = b;
            self.len += 1;
        }
    }

    fn str(&mut self, s: &str) {
        for &b in s.as_bytes() {
            self.byte(b);
        }
    }

    /// Текст с экранированием кавычек и управляющих символов — чтобы одна
    /// строка журнала не могла разбить формат `key=value`.
    fn escaped(&mut self, s: &str) {
        for &b in s.as_bytes() {
            match b {
                b'"' => self.str("\\\""),
                b'\\' => self.str("\\\\"),
                b'\n' => self.str("\\n"),
                b'\r' => self.str("\\r"),
                b'\t' => self.str("\\t"),
                0x00..=0x1F => self.str("?"),
                _ => self.byte(b),
            }
        }
    }

    fn code(&mut self, code: ErrorCode) {
        if code == super::NONE {
            self.str("DX-LOG-0000");
        } else {
            let mut tmp = [0u8; 16];
            let n = code.write_to(&mut tmp);
            self.str(core::str::from_utf8(&tmp[..n]).unwrap_or("DX-???-0000"));
        }
    }

    fn u64(&mut self, mut v: u64) {
        let mut digits = [0u8; 20];
        let mut n = 0;
        if v == 0 {
            self.byte(b'0');
            return;
        }
        while v > 0 {
            digits[n] = b'0' + (v % 10) as u8;
            n += 1;
            v /= 10;
        }
        for i in (0..n).rev() {
            self.byte(digits[i]);
        }
    }

    fn hex(&mut self, v: u64) {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        self.str("0000000000000000");
        let base = self.len - 16;
        for i in 0..16 {
            let nibble = ((v >> ((15 - i) * 4)) & 0xF) as usize;
            if base + i < self.buf.len() {
                self.buf[base + i] = HEX[nibble];
            }
        }
    }
}
