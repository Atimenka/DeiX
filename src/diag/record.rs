//! Структурированная запись события диагностики.
//!
//! Запись намеренно не содержит владеемых строк: она копируется в кольцевой
//! буфер из контекста прерывания и из обработчика паники, где выделение
//! памяти недопустимо. Сообщение хранится в фиксированном буфере и
//! усекается, если не помещается.

use super::code::ErrorCode;
use super::severity::Severity;

/// Ёмкость буфера сообщения внутри записи.
pub const MESSAGE_LEN: usize = 96;

/// Рекомендованная реакция на событие.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Action {
    /// Действий не требуется — информационное событие.
    None = 0,
    /// Продолжить работу, событие уже обработано.
    Continue = 1,
    /// Повторить операцию позже.
    Retry = 2,
    /// Остановить конкретный процесс, сохранив работу ядра.
    TerminateProcess = 3,
    /// Изолировать модуль, сохранив работу ядра.
    QuarantineModule = 4,
    /// Отключить подсистему и продолжить в деградированном режиме.
    DegradeSubsystem = 5,
    /// Сохранить отчёт об отказе и остановить систему.
    SaveReportAndHalt = 6,
    /// Выполнить аппаратный сброс.
    HardwareReset = 7,
}

impl Action {
    /// Короткая метка для журнала.
    pub const fn tag(self) -> &'static str {
        match self {
            Action::None => "none",
            Action::Continue => "continue",
            Action::Retry => "retry",
            Action::TerminateProcess => "terminate-process",
            Action::QuarantineModule => "quarantine-module",
            Action::DegradeSubsystem => "degrade-subsystem",
            Action::SaveReportAndHalt => "save-report-halt",
            Action::HardwareReset => "hardware-reset",
        }
    }

    /// Человекочитаемое описание реакции.
    pub const fn name(self) -> &'static str {
        match self {
            Action::None => "Действий не требуется",
            Action::Continue => "Работа продолжается",
            Action::Retry => "Операция будет повторена",
            Action::TerminateProcess => "Процесс будет остановлен",
            Action::QuarantineModule => "Модуль будет помещён в карантин",
            Action::DegradeSubsystem => "Подсистема будет отключена",
            Action::SaveReportAndHalt => "Отчёт будет сохранён, система остановится",
            Action::HardwareReset => "Будет выполнен аппаратный сброс",
        }
    }

    /// Действие по умолчанию для уровня критичности, если код его не уточняет.
    pub const fn default_for(sev: Severity) -> Self {
        match sev {
            Severity::Trace | Severity::Debug | Severity::Info | Severity::Notice => Action::None,
            Severity::Warning => Action::Continue,
            Severity::Error => Action::Retry,
            Severity::Critical => Action::DegradeSubsystem,
            Severity::Panic => Action::SaveReportAndHalt,
            Severity::Fatal => Action::HardwareReset,
        }
    }
}

/// Одно событие диагностики.
#[derive(Clone, Copy)]
pub struct ErrorRecord {
    /// Монотонный идентификатор записи; на него ссылаются `related_id`
    /// вторичных сбоев. 0 — запись ещё не помещена в буфер.
    pub id: u32,
    /// Момент события в миллисекундах от старта системы.
    pub timestamp_ms: u64,
    /// Уровень критичности.
    pub severity: Severity,
    /// Код ошибки из реестра; `NONE` для событий без конкретного кода.
    pub code: ErrorCode,
    /// Идентификатор процесса; 0 означает событие ядра.
    pub pid: u32,
    /// Идентификатор потока.
    pub tid: u32,
    /// Номер процессора.
    pub cpu: u8,
    /// Адрес, с которым связано событие; 0, если адреса нет.
    pub address: u64,
    /// Рекомендованная реакция.
    pub action: Action,
    /// Идентификатор записи-причины в цепочке связанных событий; 0 — корень.
    pub related_id: u32,
    /// Сколько раз событие повторилось (агрегация одинаковых записей).
    pub occurrences: u32,
    /// Момент первого появления агрегированной серии.
    pub first_seen_ms: u64,
    /// Момент последнего появления агрегированной серии.
    pub last_seen_ms: u64,
    /// Текст сообщения (без завершающего нуля).
    pub message: [u8; MESSAGE_LEN],
    /// Реальная длина `message`.
    pub message_len: u8,
}

impl ErrorRecord {
    /// Пустая запись — используется для константной инициализации буфера.
    pub const fn empty() -> Self {
        Self {
            id: 0,
            timestamp_ms: 0,
            severity: Severity::Info,
            code: super::NONE,
            pid: 0,
            tid: 0,
            cpu: 0,
            address: 0,
            action: Action::None,
            related_id: 0,
            occurrences: 0,
            first_seen_ms: 0,
            last_seen_ms: 0,
            message: [0; MESSAGE_LEN],
            message_len: 0,
        }
    }

    /// Сообщение записи как срез.
    pub fn message(&self) -> &str {
        let len = (self.message_len as usize).min(MESSAGE_LEN);
        core::str::from_utf8(&self.message[..len]).unwrap_or("<не-utf8>")
    }

    /// Заменяет сообщение, усекая до размера буфера.
    ///
    /// Обрезка выполняется по границе UTF-8-символа, чтобы в журнал не попал
    /// разорванный многобайтовый символ.
    pub fn set_message(&mut self, text: &str) {
        let bytes = text.as_bytes();
        let mut len = bytes.len().min(MESSAGE_LEN);
        // При усечении откатываемся до границы символа: байт продолжения
        // UTF-8 имеет вид 10xxxxxx.
        while len < bytes.len() && len > 0 && (bytes[len] & 0xC0) == 0x80 {
            len -= 1;
        }
        self.message[..len].copy_from_slice(&bytes[..len]);
        self.message_len = len as u8;
    }

    /// Краткая подпись серии: код + сообщение. Используется для агрегации —
    /// одинаковые сбои не должны порождать тысячи строк в журнале.
    pub fn identity(&self) -> (u32, [u8; 8]) {
        // Простой хеш сообщения (FNV-1a), достаточный для группировки.
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for &b in self.message() .as_bytes() {
            hash ^= b as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        let mut key = [0u8; 8];
        key.copy_from_slice(&hash.to_le_bytes());
        (self.code.0, key)
    }
}
