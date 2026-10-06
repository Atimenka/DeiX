//! Единая система ошибок и диагностики DeiX OS.
//!
//! Подсистема отвечает за всё, что связано с регистрацией и объяснением
//! сбоев: от трассировочного сообщения в кольцевой буфер до отчёта об отказе
//! ядра на диске.
//!
//! ## Слои
//!
//! - [`code`] — реестр кодов `DX-<SUBSYSTEM>-<CODE>` и их описаний.
//! - [`severity`] — шкала критичности от `Trace` до `Fatal`.
//! - [`record`] — структура события и рекомендованные реакции.
//! - [`ring`] — предвыделенный кольцевой буфер событий.
//! - [`log`] — точки входа, фильтрация режимов, немедленный вывод.
//! - [`persist`] — сохранение журналов в `/userdata/log/`.
//! - [`panic`] — отчёт об отказе ядра и экран паники.
//! - [`cli`] — команды `log`, `error`, `panic`, `diagnostics`.
//!
//! ## Соглашения
//!
//! Каждому конкретному сценарию сбоя соответствует ровно один код. Один и
//! тот же сбой не должен регистрироваться под двумя кодами, иначе цепочка
//! «первопричина → вторичные сбои» теряет смысл.
//!
//! Смерть пользовательского процесса или сбой модуля ядра — это
//! регистрируемое событие, а не паника. Паника означает, что само ядро
//! больше не может продолжать работу.

pub mod boot;
pub mod cli;
pub mod code;
pub mod log;
pub mod panic;
pub mod persist;
pub mod record;
pub mod ring;
pub mod severity;

pub use code::{ErrorCode, Subsystem, CODE_COUNT};
pub use log::{format_human, format_time, passes};
pub use record::{Action, ErrorRecord};
pub use severity::Severity;

/// Псевдокод для событий, у которых нет собственного кода в реестре
/// (обычные информационные сообщения журнала).
pub const NONE: ErrorCode = ErrorCode(0xFFFF_FFFF);

/// Инициализирует систему диагностики.
///
/// Вызывается сразу после кучи — до файловой системы и до графической
/// оболочки. Кольцевой буфер находится в `.bss` и принимает события ещё
/// раньше; эта функция лишь выставляет режим и фиксирует факт готовности.
pub fn init() {
    log::set_threshold(Severity::Info);
    info(NONE, "система диагностики готова (буфер, serial)");
}

/// Вторая фаза: постоянное хранилище журналов.
///
/// Вызывается, когда `/userdata` доступен на запись. До этого момента все
/// события живут в кольцевом буфере и ничего не теряется — они будут
/// сброшены на диск первым же `persist::flush()`.
pub fn storage_ready() {
    persist::init();
    let session = persist::next_boot_session();
    ring::set_boot_session(session);
    info(NONE, &alloc::format!("журналы на диске готовы (сессия загрузки #{})", session));
    persist::flush();
}

// ==================== Точки входа ====================
//
// Имена выбраны по уровню критичности, а не по подсистеме: вызывающий код
// читается как намерение («это предупреждение»), а подсистема уже зашита
// в переданный код ошибки.

/// Трассировочное сообщение — видно только в диагностическом режиме.
pub fn trace(code: ErrorCode, message: &str) -> Option<u32> {
    log::log(Severity::Trace, code, Action::None, message)
}

/// Отладочное сообщение — видно только в диагностическом режиме.
pub fn debug(code: ErrorCode, message: &str) -> Option<u32> {
    log::log(Severity::Debug, code, Action::None, message)
}

/// Информационное событие жизненного цикла.
pub fn info(code: ErrorCode, message: &str) -> Option<u32> {
    log::log(Severity::Info, code, Action::None, message)
}

/// Событие, требующее внимания, но не являющееся ошибкой.
pub fn notice(code: ErrorCode, message: &str) -> Option<u32> {
    log::log(Severity::Notice, code, Action::None, message)
}

/// Предупреждение: работа продолжается, но ситуация требует внимания.
pub fn warn(code: ErrorCode, message: &str) -> Option<u32> {
    log::log(Severity::Warning, code, Action::default_for(Severity::Warning), message)
}

/// Ошибка операции. Подсистема продолжает работу.
pub fn error(code: ErrorCode, message: &str) -> Option<u32> {
    log::log(Severity::Error, code, Action::default_for(Severity::Error), message)
}

/// Ошибка с явным указанием реакции.
pub fn error_with(code: ErrorCode, action: Action, message: &str) -> Option<u32> {
    log::log(Severity::Error, code, action, message)
}

/// Критический сбой с явным указанием реакции.
pub fn critical_with(code: ErrorCode, action: Action, message: &str) -> Option<u32> {
    log::log(Severity::Critical, code, action, message)
}

/// Предупреждение с явным указанием реакции.
pub fn warn_with(code: ErrorCode, action: Action, message: &str) -> Option<u32> {
    log::log(Severity::Warning, code, action, message)
}

/// Событие, связанное с уже зарегистрированной причиной.
///
/// Строит цепочку «первопричина → вторичные сбои»: например, отказ диска
/// (первопричина) и последовавшая за ним ошибка чтения EXT2 (вторичная).
pub fn caused_by(cause_id: u32, code: ErrorCode, message: &str) -> Option<u32> {
    log::log_related(Severity::Error, code, Action::default_for(Severity::Error), message, cause_id)
}

/// Расширенный идентификатор события: `DX-GFX-0008-20261006-083154`.
///
/// Дата и время берутся из аппаратных часов, чтобы идентификатор оставался
/// уникальным между перезагрузками и его можно было передать в отчёте.
/// При неисправном RTC используется номер загрузки и время от старта —
/// идентификатор всё равно остаётся различимым.
pub fn extended_id(record: &ErrorRecord) -> alloc::string::String {
    let clock = crate::rtc::now();
    let suffix = if crate::rtc::is_plausible() {
        clock.compact()
    } else {
        alloc::format!("boot{}-{:06}", ring::boot_session(), record.timestamp_ms)
    };
    alloc::format!("{}-{}", record.code.as_string(), suffix)
}

/// Сводка состояния системы диагностики — для `diagnostics summary`.
pub struct DiagSummary {
    /// Всего событий зарегистрировано за текущую загрузку.
    pub total_events: usize,
    /// Сколько событий сейчас в кольцевом буфере.
    pub buffered: usize,
    /// Сколько событий не поместилось в буфер.
    pub dropped: usize,
    /// Счётчики по уровням критичности, индекс = уровень.
    pub by_severity: [usize; 9],
    /// Счётчики по подсистемам, индекс = `Subsystem as usize`.
    pub by_subsystem: [usize; 14],
    /// Идентификатор текущей загрузки.
    pub boot_session: u32,
    /// Порог журналирования.
    pub threshold: Severity,
    /// Включён ли диагностический режим.
    pub diagnostic_mode: bool,
}

/// Собирает сводку по текущему содержимому буфера.
pub fn summary() -> DiagSummary {
    let records = ring::all();
    let mut by_severity = [0usize; 9];
    let mut by_subsystem = [0usize; 14];

    for r in &records {
        let s = r.severity.level() as usize;
        if s < by_severity.len() {
            by_severity[s] += r.occurrences as usize;
        }
        let sub = r.code.subsystem() as usize;
        if r.code != NONE && sub < by_subsystem.len() {
            by_subsystem[sub] += r.occurrences as usize;
        }
    }

    DiagSummary {
        total_events: ring::total_events(),
        buffered: records.len(),
        dropped: ring::dropped_events(),
        by_severity,
        by_subsystem,
        boot_session: ring::boot_session(),
        threshold: log::threshold(),
        diagnostic_mode: log::diagnostic_mode(),
    }
}
