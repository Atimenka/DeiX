//! Шкала критичности событий DeiX.
//!
//! Порядок вариантов значим: сравнение через `PartialOrd` определяет, проходит
//! ли событие через фильтр текущего режима журналирования.

/// Уровень критичности события.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[repr(u8)]
pub enum Severity {
    /// Пошаговая трассировка; включается только в диагностическом режиме.
    Trace = 0,
    /// Отладочная информация; включается только в диагностическом режиме.
    Debug = 1,
    /// Обычное событие жизненного цикла.
    Info = 2,
    /// Событие, требующее внимания, но не являющееся ошибкой.
    Notice = 3,
    /// Потенциальная проблема; работа продолжается.
    Warning = 4,
    /// Ошибка операции; подсистема продолжает работу.
    Error = 5,
    /// Серьёзный сбой; подсистема деградировала или отключена.
    Critical = 6,
    /// Отказ ядра; система останавливается и сохраняет отчёт.
    Panic = 7,
    /// Неисправимый отказ; выполняется аппаратный сброс.
    Fatal = 8,
}

impl Severity {
    /// Короткая метка для журнала (`ERR`).
    pub const fn tag(self) -> &'static str {
        match self {
            Severity::Trace => "TRACE",
            Severity::Debug => "DEBUG",
            Severity::Info => "INFO",
            Severity::Notice => "NOTICE",
            Severity::Warning => "WARN",
            Severity::Error => "ERROR",
            Severity::Critical => "CRIT",
            Severity::Panic => "PANIC",
            Severity::Fatal => "FATAL",
        }
    }

    /// Человекочитаемое название.
    pub const fn name(self) -> &'static str {
        match self {
            Severity::Trace => "Трассировка",
            Severity::Debug => "Отладка",
            Severity::Info => "Информация",
            Severity::Notice => "Уведомление",
            Severity::Warning => "Предупреждение",
            Severity::Error => "Ошибка",
            Severity::Critical => "Критическая ошибка",
            Severity::Panic => "Отказ ядра",
            Severity::Fatal => "Неисправимый отказ",
        }
    }

    /// Все уровни в порядке возрастания.
    pub const ALL: [Severity; 9] = [
        Severity::Trace,
        Severity::Debug,
        Severity::Info,
        Severity::Notice,
        Severity::Warning,
        Severity::Error,
        Severity::Critical,
        Severity::Panic,
        Severity::Fatal,
    ];

    /// Числовой уровень — для сериализации и фильтрации.
    pub const fn level(self) -> u8 {
        self as u8
    }

    /// Восстанавливает уровень из числа.
    pub const fn from_level(level: u8) -> Self {
        match level {
            0 => Severity::Trace,
            1 => Severity::Debug,
            3 => Severity::Notice,
            4 => Severity::Warning,
            5 => Severity::Error,
            6 => Severity::Critical,
            7 => Severity::Panic,
            8 => Severity::Fatal,
            _ => Severity::Info,
        }
    }

    /// Является ли уровень ошибкой (попадает в `error.log` и в счётчик ошибок).
    pub const fn is_error(self) -> bool {
        matches!(self, Severity::Error | Severity::Critical | Severity::Panic | Severity::Fatal)
    }
}
