// ❗ЗАВИСИМОТИ: инит скрипт pid 1 который будет ограничивать пользовательский
// ЯДЕРНЫЙ МОДУЛЬ DeiX OS (src/lib.rs, Ring 0). Интеграция в существующий код
// init_parser — парсер init.deix (стадия init_boot, PID 1): карта разделов,
// команды mount/service, реестр BTreeMap<BootStage, Vec<Command>>, Vault-проверка.
// no_std-совместимо (ядро DeiX OS): только core/alloc (BTreeMap, String, Vec),
// вывод — через crate::println!/crate::print! (стиль dxinit.rs).


use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::format;

use crate::vault::{VaultRejection, evaluate as vault_evaluate};

/// Максимальная длина содержательной строки конфигурации (байт UTF-8).
/// Защита кучи ядра от деструктивного ввода на этапе init_boot.
pub const MAX_CONFIG_LINE_LEN: usize = 256;

/// Максимальное количество команд в одной стадии загрузки (DoS-защита).
pub const MAX_STAGE_COMMANDS: usize = 512;

/// Фазы загрузки DeiX OS. Ключ реестра команд парсера.
///
/// no_std-заметка: enum без данных — размер 1 байт, Copy, не требует
/// динамической памяти. Hash позволяет использовать его ключом HashMap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
    pub enum BootStage {
    /// Ранняя загрузка Ring 0: микроядро, структуры PID 1, init.deix.
    InitBoot,
    /// Загрузка раздела оборудования/драйверов (HAL, прошивка вендора).
    VendorBoot,
    /// Штатная загрузка основной ОС.
    Boot,
    /// Изолированная среда восстановления (TWRP/OrangeFox).
    Recovery,
    /// Демон прошивки пользовательского пространства (fastboot flash).
    Fastbootd,
    /// Аварийный режим прошивки Ring 0 (Emergency Download / unbrick).
    Edl,
}

impl BootStage {
    /// Строгий разбор токена стадии из конфигурационного файла.
    /// Допустимы ровно шесть литералов; любой иной — типизированная
    /// ошибка ParseError::UnknownStage (отказоустойчивость по ТЗ).
    pub fn from_token(token: &str) -> Result<BootStage, ParseError> {
        match token {
            "init_boot" => Ok(BootStage::InitBoot),
            "vendor_boot" => Ok(BootStage::VendorBoot),
            "boot" => Ok(BootStage::Boot),
            "recovery" => Ok(BootStage::Recovery),
            "fastbootd" => Ok(BootStage::Fastbootd),
            "edl" => Ok(BootStage::Edl),
            other => Err(ParseError::UnknownStage(other.to_string())),
        }
    }

    /// Обратное представление стадии в строковый литерал (диагностика).
    pub fn as_token(&self) -> &'static str {
        match self {
            BootStage::InitBoot => "init_boot",
            BootStage::VendorBoot => "vendor_boot",
            BootStage::Boot => "boot",
            BootStage::Recovery => "recovery",
            BootStage::Fastbootd => "fastbootd",
            BootStage::Edl => "edl",
        }
    }

    /// Является ли стадия штатным контекстом прошивки, в котором РАЗРЕШЕНА
    /// запись (rw) в системные разделы: Fastbootd, Edl, Recovery.
    /// Развёрнутый match исключает обход логики через манипуляции
    /// с условиями (требование ТЗ по защите Vault).
    pub fn is_flash_authorized(&self) -> bool {
        match self {
            BootStage::Fastbootd => true,
            BootStage::Edl => true,
            BootStage::Recovery => true,
            BootStage::InitBoot => false,
            BootStage::VendorBoot => false,
            BootStage::Boot => false,
        }
    }

    /// Разрешено ли на данной стадии монтирование /userdata в режиме rw.
    /// Строго по ТЗ: Boot и Recovery; прошивочные контексты (Fastbootd,
    /// Edl) также могут сбрасывать пользовательские данные, ранние стадии
    /// (InitBoot, VendorBoot) — нет.
    pub fn allows_userdata_rw(&self) -> bool {
        match self {
            BootStage::Boot => true,
            BootStage::Recovery => true,
            BootStage::Fastbootd => true,
            BootStage::Edl => true,
            BootStage::InitBoot => false,
            BootStage::VendorBoot => false,
        }
    }
}

/// Модификатор доступа к узлам дерева виртуальной файловой системы (VFS).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountMode {
    /// Жёсткая блокировка записи на уровне дисковых секторов (EROFS).
    ReadOnly,
    /// Разрешена модификация данных на целевом устройстве.
    ReadWrite,
}

impl MountMode {
    /// Строгий разбор токена режима: допустимы только "ro" и "rw".
    pub fn from_token(token: &str) -> Result<MountMode, ParseError> {
        match token {
            "ro" => Ok(MountMode::ReadOnly),
            "rw" => Ok(MountMode::ReadWrite),
            other => Err(ParseError::InvalidMountMode(other.to_string())),
        }
    }

    pub fn as_token(&self) -> &'static str {
        match self {
            MountMode::ReadOnly => "ro",
            MountMode::ReadWrite => "rw",
        }
    }

}

/// Полное описание команды привязки блочного устройства к VFS.
///
/// Поля (owned-строки; в no_std — alloc::String из кучи ядра):
/// * `fs_type` — драйвер файловой системы: "erofs", "ext4", "sysfs".
/// * `src`     — путь к источнику/блочному устройству.
/// * `dst`     — точка монтирования в иерархии VFS.
/// * `mode`    — атрибут прав доступа (MountMode).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountCmd {
    pub fs_type: String,
    pub src: String,
    pub dst: String,
    pub mode: MountMode,
}

impl MountCmd {
    pub fn new(fs_type: String, src: String, dst: String, mode: MountMode) -> MountCmd {
        MountCmd {
            fs_type,
            src,
            dst,
            mode,
        }
    }
}

/// Конфигурационные параметры службы для fork/exec в кольце исполнения.
///
/// Поля:
/// * `name`           — уникальное имя службы (глобальная таблица процессов).
/// * `path`           — абсолютный путь к исполняемому файлу в EROFS-образе.
/// * `execution_ring` — кольцо защиты исполнения: 0 (Ring 0 / Kernel) или
///   3 (Ring 3 / User). Валидируется в диапазоне 0..=3.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceCmd {
    pub name: String,
    pub path: String,
    pub execution_ring: u8,
}

impl ServiceCmd {
    pub fn new(name: String, path: String, execution_ring: u8) -> ServiceCmd {
        ServiceCmd {
            name,
            path,
            execution_ring,
        }
    }
}

/// Токенизированный элемент операции — атомарная инструкция AST.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Mount(MountCmd),
    Service(ServiceCmd),
}

impl Command {
    /// Диагностическое строковое представление команды.
    pub fn describe(&self) -> String {
        match self {
            Command::Mount(m) => format!(
                "mount {} {} {} {}",
                m.fs_type,
                m.src,
                m.dst,
                m.mode.as_token()
            ),
            Command::Service(s) => {
                format!("service {} {} ring={}", s.name, s.path, s.execution_ring)
            }
        }
    }
}

/// Строго типизированное перечисление синтаксических ошибок парсера.
/// Ни одна из них НЕ останавливает парсинг: каждая превращается в
/// предупреждение, строка отбрасывается, разбор продолжается. Единственное
/// легитимное условие остановки системы — нарушение Vault (panic!).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// Команда встречена до первого объявления стадии `on <stage>`.
    MissingStageContext,
    /// Неизвестный токен стадии (не из шести штатных).
    UnknownStage(String),
    /// Недостаточно аргументов у ключевого слова.
    TooFewArguments {
        keyword: String,
        expected: usize,
        got: usize,
    },
    /// Лишние аргументы у ключевого слова.
    TooManyArguments {
        keyword: String,
        expected: usize,
        got: usize,
    },
    /// Четвёртый параметр mount не равен "ro" или "rw".
    InvalidMountMode(String),
    /// execution_ring службы не является числом 0..=3.
    InvalidExecutionRing(String),
    /// Строка без единого токена после очистки.
    EmptyCommand,
    /// Длина строки превышает MAX_CONFIG_LINE_LEN.
    LineTooLong { len: usize, limit: usize },
    /// Некорректный заголовок стадии `on`.
    MalformedStageHeader { detail: String },
}

impl ParseError {
    /// Человекочитаемое описание ошибки (уходит в поле `detail`).
    pub fn message(&self) -> String {
        match self {
            ParseError::MissingStageContext => {
                "команда встречена до первого объявления стадии 'on'".to_string()
            }
            ParseError::UnknownStage(tok) => {
                format!("неизвестная стадия загрузки '{}'", tok)
            }
            ParseError::TooFewArguments {
                keyword,
                expected,
                got,
            } => format!(
                "команда '{}': ожидалось {} аргумента, получено {}",
                keyword, expected, got
            ),
            ParseError::TooManyArguments {
                keyword,
                expected,
                got,
            } => format!(
                "команда '{}': ожидалось {} аргумента, получено {}",
                keyword, expected, got
            ),
            ParseError::InvalidMountMode(tok) => format!(
                "недопустимый режим монтирования '{}' (допустимы только 'ro' и 'rw')",
                tok
            ),
            ParseError::InvalidExecutionRing(tok) => format!(
                "недопустимое кольцо исполнения '{}' (допустимы 0..=3)",
                tok
            ),
            ParseError::EmptyCommand => "пустая команда (нет токенов)".to_string(),
            ParseError::LineTooLong { len, limit } => {
                format!("длина строки {} байт превышает лимит {} байт", len, limit)
            }
            ParseError::MalformedStageHeader { detail } => {
                format!("некорректный заголовок стадии: {}", detail)
            }
        }
    }
}

/// Классификатор предупреждения для лога ядра.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseWarningKind {
    EmptyCommand,
    LineTooLong,
    MalformedStageHeader,
    UnknownKeyword,
    UnknownStage,
    TooFewArguments,
    TooManyArguments,
    InvalidMountMode,
    InvalidExecutionRing,
    MissingStageContext,
    TooManyStageCommands,
    UserdataPolicyViolation,
    NonExt4Userdata,
    TpmPartitionDenied,
}

impl ParseWarningKind {
    pub fn describe(&self) -> &'static str {
        match self {
            ParseWarningKind::EmptyCommand => "пустая команда",
            ParseWarningKind::LineTooLong => "строка превышает лимит длины",
            ParseWarningKind::MalformedStageHeader => "некорректный заголовок стадии 'on'",
            ParseWarningKind::UnknownKeyword => "неизвестное ключевое слово",
            ParseWarningKind::UnknownStage => "неизвестная стадия загрузки",
            ParseWarningKind::TooFewArguments => "недостаточно аргументов",
            ParseWarningKind::TooManyArguments => "лишние аргументы",
            ParseWarningKind::InvalidMountMode => "неверный флаг режима монтирования",
            ParseWarningKind::InvalidExecutionRing => "недопустимое кольцо исполнения",
            ParseWarningKind::MissingStageContext => "команда вне контекста стадии",
            ParseWarningKind::TooManyStageCommands => "переполнение вектора команд стадии",
            ParseWarningKind::UserdataPolicyViolation => {
                "rw-монтирование /userdata вне разрешённых стадий"
            }
            ParseWarningKind::NonExt4Userdata => "файловая система /userdata отличается от ext4",
            ParseWarningKind::TpmPartitionDenied => "доступ к скрытому разделу /TPM запрещён",
        }
    }
}

/// Запись о дефектной строке: номер, исходный текст, категория, детали.
#[derive(Debug, Clone)]
pub struct ParseWarning {
    pub line_no: usize,
    pub raw_line: String,
    pub kind: ParseWarningKind,
    pub detail: String,
}

/// Итоговая статистика прогона: accepted + rejected + skipped == total.
#[derive(Debug, Clone, Default)]
pub struct ParseReport {
    pub total_lines: usize,
    pub accepted_lines: usize,
    pub rejected_lines: usize,
    pub skipped_lines: usize,
}

/// Центральное хранилище состояния парсера.
///
/// Поля (публичны — стиль ветки):
/// * `registry`    — BTreeMap<BootStage, Vec<Command>>: карта стадия -> команды.
/// * `stage_order` — порядок объявления стадий (детерминированный вывод).
/// * `warnings`    — журнал синтаксических инцидентов.
/// * `report`      — счётчики прогона.
pub struct InitParser {
    pub registry: BTreeMap<BootStage, Vec<Command>>,
    pub stage_order: Vec<BootStage>,
    pub warnings: Vec<ParseWarning>,
    pub report: ParseReport,
}

impl InitParser {
    /// Конструирует пустой парсер. Коллекции создаются пустыми; память
    /// выделяется лениво при первом использовании.
    pub fn new() -> InitParser {
        InitParser {
            registry: BTreeMap::new(),
            stage_order: Vec::new(),
            warnings: Vec::new(),
            report: ParseReport::default(),
        }
    }

    /// Главный цикл построчного разбора текстового стрима. Алгоритм
    /// оперирует срезами строк, минимизируя избыточные аллокации:
    /// токены — заимствованные фрагменты исходного буфера; owned-копии
    /// создаются только для полей принимаемых команд.
    ///
    /// Возвращает клон реестра. Паника возможна ТОЛЬКО при нарушении Vault.
    pub fn parse(&mut self, source: &str) -> BTreeMap<BootStage, Vec<Command>> {
        // Сброс состояния (парсер переиспользуем).
        self.registry.clear();
        self.stage_order.clear();
        self.warnings.clear();
        self.report = ParseReport::default();

        // Активный контекст сборки: стадия, объявленная последним `on`.
        let mut active_stage: Option<BootStage> = None;

        for (idx, raw) in source.lines().enumerate() {
            let line_no: usize = idx + 1;
            self.report.total_lines += 1;

            // ЭТАП 1 — очистка пробелов (\r \n \t ')' и пробел), UTF-8-safe.
            let stripped: &str = Self::trim_control_chars(raw);
            if stripped.is_empty() {
                self.report.skipped_lines += 1;
                continue;
            }

            // ЭТАП 2 — изоляция комментариев: усечение по '#' вне кавычек.
            let meaningful: &str = Self::slice_comment(stripped);
            let cleaned: &str = Self::trim_control_chars(meaningful);
            if cleaned.is_empty() {
                self.report.skipped_lines += 1;
                continue;
            }

            // ЭТАП 3 — контроль длины строки.
            if cleaned.len() > MAX_CONFIG_LINE_LEN {
                self.push_warning(
                    line_no,
                    cleaned,
                    ParseWarningKind::LineTooLong,
                    ParseError::LineTooLong {
                        len: cleaned.len(),
                        limit: MAX_CONFIG_LINE_LEN,
                    }
                    .message(),
                );
                continue;
            }

            // ЭТАП 4 — токенизация на срезах.
            let tokens: Vec<&str> = cleaned.split_whitespace().collect();
            if tokens.is_empty() {
                self.push_warning(
                    line_no,
                    cleaned,
                    ParseWarningKind::EmptyCommand,
                    ParseError::EmptyCommand.message(),
                );
                continue;
            }

            // ЭТАП 5 — диспетчеризация по первому токену.
            let keyword: &str = tokens[0];
            match keyword {
                "on" => self.handle_stage_header(line_no, cleaned, &tokens, &mut active_stage),
                "mount" => self.handle_mount_line(line_no, cleaned, &tokens, &active_stage),
                "service" => self.handle_service_line(line_no, cleaned, &tokens, &active_stage),
                other => self.push_warning(
                    line_no,
                    cleaned,
                    ParseWarningKind::UnknownKeyword,
                    format!("неизвестная команда '{}' — строка отброшена", other),
                ),
            }
        }

        self.registry.clone()
    }

    /// Очистка начальных и конечных символов форматирования. Работает на
    /// срезах — ноль аллокаций. Свободная функция (результат привязан ко
    /// входному срезу, а не к &self) — исключает конфликт заимствований.
    fn trim_control_chars(s: &str) -> &str {
        let is_trim_char = |c: char| -> bool {
            match c {
                ' ' | '\t' | '\r' | '\n' | ')' => true,
                _ => false,
            }
        };
        let start: &str = match s.find(|c: char| !is_trim_char(c)) {
            Some(pos) => &s[pos..],
            None => "",
        };
        // rfind возвращает байтовый индекс НАЧАЛА символа; чтобы включить
        // многобайтовый символ целиком (кириллица UTF-8), конец среза =
        // pos + len_utf8 символа.
        let end: &str = match start.rfind(|c: char| !is_trim_char(c)) {
            Some(pos) => {
                let char_len: usize = match start[pos..].chars().next() {
                    Some(ch) => ch.len_utf8(),
                    None => 1,
                };
                &start[..pos + char_len]
            }
            None => "",
        };
        end
    }

    /// Усечение строки по первому символу '#' ВНЕ строковых литералов
    /// ('...' и "..."). Свободная функция — результат привязан ко входу.
    fn slice_comment(line: &str) -> &str {
        let mut in_single_quote: bool = false;
        let mut in_double_quote: bool = false;
        let mut hash_index: Option<usize> = None;

        for (idx, ch) in line.char_indices() {
            match ch {
                '\'' if !in_double_quote => {
                    in_single_quote = !in_single_quote;
                }
                '"' if !in_single_quote => {
                    in_double_quote = !in_double_quote;
                }
                '#' if !in_single_quote && !in_double_quote => {
                    hash_index = Some(idx);
                    break;
                }
                _ => {}
            }
        }

        match hash_index {
            Some(pos) => &line[..pos],
            None => line,
        }
    }

    /// Обработка заголовка стадии: `on <stage_token>`.
    /// Верифицируется ровно один аргумент; он разбирается в BootStage.
    fn handle_stage_header(
        &mut self,
        line_no: usize,
        raw_line: &str,
        tokens: &[&str],
        active_stage: &mut Option<BootStage>,
    ) {
        match tokens.len() {
            2 => {
                let stage_result: Result<BootStage, ParseError> = BootStage::from_token(tokens[1]);
                match stage_result {
                    Ok(stage) => {
                        *active_stage = Some(stage);
                        match self.stage_order.iter().any(|s| *s == stage) {
                            true => {}
                            false => self.stage_order.push(stage),
                        }
                        self.registry.entry(stage).or_insert_with(|| Vec::new());
                        self.report.accepted_lines += 1;
                    }
                    Err(err) => {
                        self.push_warning(line_no, raw_line, ParseWarningKind::UnknownStage, err.message());
                    }
                }
            }
            0..=1 => self.push_warning(
                line_no,
                raw_line,
                ParseWarningKind::MalformedStageHeader,
                ParseError::MalformedStageHeader {
                    detail: "после 'on' ожидалось ровно одно имя стадии".to_string(),
                }
                .message(),
            ),
            3.. => self.push_warning(
                line_no,
                raw_line,
                ParseWarningKind::TooManyArguments,
                ParseError::TooManyArguments {
                    keyword: "on".to_string(),
                    expected: 1,
                    got: tokens.len().saturating_sub(1),
                }
                .message(),
            ),
        }
    }

    /// Обработка инструкции монтирования: `mount <type> <src> <dst> <mode>`.
    /// Жёстко ожидается 4 параметра; четвёртый — "ro"/"rw". Перед
    /// добавлением в реестр выполняется проверка Vault.
    fn handle_mount_line(
        &mut self,
        line_no: usize,
        raw_line: &str,
        tokens: &[&str],
        active_stage: &Option<BootStage>,
    ) {
        let parse_result: Result<Command, ParseError> = match tokens.len() {
            5 => {
                let mode_result: Result<MountMode, ParseError> = MountMode::from_token(tokens[4]);
                match mode_result {
                    Ok(mode) => {
                        let cmd: MountCmd = MountCmd::new(
                            tokens[1].to_string(),
                            tokens[2].to_string(),
                            tokens[3].to_string(),
                            mode,
                        );
                        Ok(Command::Mount(cmd))
                    }
                    Err(err) => Err(err),
                }
            }
            0..=4 => Err(ParseError::TooFewArguments {
                keyword: "mount".to_string(),
                expected: 4,
                got: tokens.len().saturating_sub(1),
            }),
            6.. => Err(ParseError::TooManyArguments {
                keyword: "mount".to_string(),
                expected: 4,
                got: tokens.len().saturating_sub(1),
            }),
        };

        match parse_result {
            Ok(cmd) => {
                match cmd {
                    Command::Mount(ref m) => {
                        // Vault-проверка ДО помещения в реестр. Стадия
                        // обязана быть активной (иначе — MissingStageContext).
                        match active_stage {
                            Some(stage) => {
                                let verdict: Result<(), VaultRejection> =
                                    vault_evaluate(&m.dst, &m.fs_type, m.mode, *stage);
                                match verdict {
                                    Ok(()) => {
                                        self.commit_command(active_stage, cmd, line_no, raw_line);
                                    }
                                    Err(rejection) => {
                                        // Команда отклонена политикой Vault
                                        // (предупреждение формируется здесь).
                                        let (kind, detail): (ParseWarningKind, String) =
                                            match rejection {
                                                VaultRejection::UserdataPolicy(msg) => (
                                                    ParseWarningKind::UserdataPolicyViolation,
                                                    msg,
                                                ),
                                                VaultRejection::NonExt4Userdata(msg) => (
                                                    ParseWarningKind::NonExt4Userdata,
                                                    msg,
                                                ),
                                                VaultRejection::TpmPartitionDenied(msg) => (
                                                    ParseWarningKind::TpmPartitionDenied,
                                                    msg,
                                                ),
                                            };
                                        self.push_warning(line_no, raw_line, kind, detail);
                                    }
                                }
                            }
                            None => {
                                self.push_warning(
                                    line_no,
                                    raw_line,
                                    ParseWarningKind::MissingStageContext,
                                    ParseError::MissingStageContext.message(),
                                );
                            }
                        }
                    }
                    // Недостижимо: здесь всегда Command::Mount, но match
                    // обязан быть исчерпывающим.
                    Command::Service(_) => {
                        self.commit_command(active_stage, cmd, line_no, raw_line);
                    }
                }
            }
            Err(err) => {
                let (kind, detail): (ParseWarningKind, String) = match &err {
                    ParseError::TooFewArguments { .. } => {
                        (ParseWarningKind::TooFewArguments, err.message())
                    }
                    ParseError::TooManyArguments { .. } => {
                        (ParseWarningKind::TooManyArguments, err.message())
                    }
                    ParseError::InvalidMountMode(_) => {
                        (ParseWarningKind::InvalidMountMode, err.message())
                    }
                    ParseError::UnknownStage(_) => {
                        (ParseWarningKind::UnknownStage, err.message())
                    }
                    ParseError::InvalidExecutionRing(_) => {
                        (ParseWarningKind::InvalidExecutionRing, err.message())
                    }
                    ParseError::MissingStageContext => {
                        (ParseWarningKind::MissingStageContext, err.message())
                    }
                    ParseError::EmptyCommand => {
                        (ParseWarningKind::EmptyCommand, err.message())
                    }
                    ParseError::LineTooLong { .. } => {
                        (ParseWarningKind::LineTooLong, err.message())
                    }
                    ParseError::MalformedStageHeader { .. } => {
                        (ParseWarningKind::MalformedStageHeader, err.message())
                    }
                };
                self.push_warning(line_no, raw_line, kind, detail);
            }
        }
    }

    /// Обработка объявления службы: `service <name> <path> <execution_ring>`.
    /// Ровно 3 параметра; execution_ring валидируется в диапазоне 0..=3.
    fn handle_service_line(
        &mut self,
        line_no: usize,
        raw_line: &str,
        tokens: &[&str],
        active_stage: &Option<BootStage>,
    ) {
        let parse_result: Result<Command, ParseError> = match tokens.len() {
            0..=2 => Err(ParseError::TooFewArguments {
                keyword: "service".to_string(),
                expected: 3,
                got: tokens.len().saturating_sub(1),
            }),
            3 => Err(ParseError::TooFewArguments {
                keyword: "service".to_string(),
                expected: 3,
                got: tokens.len().saturating_sub(1),
            }),
            4 => {
                let ring_parse: Result<u8, ParseError> = match tokens[3].parse::<u8>() {
                    Ok(ring) => match ring {
                        0..=3 => Ok(ring),
                        4..=u8::MAX => Err(ParseError::InvalidExecutionRing(tokens[3].to_string())),
                    },
                    Err(_) => Err(ParseError::InvalidExecutionRing(tokens[3].to_string())),
                };
                match ring_parse {
                    Ok(ring) => {
                        let cmd: ServiceCmd = ServiceCmd::new(
                            tokens[1].to_string(),
                            tokens[2].to_string(),
                            ring,
                        );
                        Ok(Command::Service(cmd))
                    }
                    Err(err) => Err(err),
                }
            }
            5.. => Err(ParseError::TooManyArguments {
                keyword: "service".to_string(),
                expected: 3,
                got: tokens.len().saturating_sub(1),
            }),
        };

        match parse_result {
            Ok(cmd) => {
                self.commit_command(active_stage, cmd, line_no, raw_line);
            }
            Err(err) => {
                let (kind, detail): (ParseWarningKind, String) = match &err {
                    ParseError::TooFewArguments { .. } => {
                        (ParseWarningKind::TooFewArguments, err.message())
                    }
                    ParseError::TooManyArguments { .. } => {
                        (ParseWarningKind::TooManyArguments, err.message())
                    }
                    ParseError::InvalidMountMode(_) => {
                        (ParseWarningKind::InvalidMountMode, err.message())
                    }
                    ParseError::UnknownStage(_) => {
                        (ParseWarningKind::UnknownStage, err.message())
                    }
                    ParseError::InvalidExecutionRing(_) => {
                        (ParseWarningKind::InvalidExecutionRing, err.message())
                    }
                    ParseError::MissingStageContext => {
                        (ParseWarningKind::MissingStageContext, err.message())
                    }
                    ParseError::EmptyCommand => {
                        (ParseWarningKind::EmptyCommand, err.message())
                    }
                    ParseError::LineTooLong { .. } => {
                        (ParseWarningKind::LineTooLong, err.message())
                    }
                    ParseError::MalformedStageHeader { .. } => {
                        (ParseWarningKind::MalformedStageHeader, err.message())
                    }
                };
                self.push_warning(line_no, raw_line, kind, detail);
            }
        }
    }

    /// Помещение команды в реестр активной стадии с проверкой контекста и
    /// лимита объёма стадии.
    fn commit_command(
        &mut self,
        active_stage: &Option<BootStage>,
        cmd: Command,
        line_no: usize,
        raw_line: &str,
    ) {
        let stage: BootStage = match active_stage {
            Some(stage) => *stage,
            None => {
                self.push_warning(
                    line_no,
                    raw_line,
                    ParseWarningKind::MissingStageContext,
                    ParseError::MissingStageContext.message(),
                );
                return;
            }
        };

        let bucket: &mut Vec<Command> = self.registry.entry(stage).or_insert_with(|| Vec::new());
        match bucket.len() >= MAX_STAGE_COMMANDS {
            true => self.push_warning(
                line_no,
                raw_line,
                ParseWarningKind::TooManyStageCommands,
                format!(
                    "стадия переполнена: лимит {} команд на стадию",
                    MAX_STAGE_COMMANDS
                ),
            ),
            false => {
                bucket.push(cmd);
                self.report.accepted_lines += 1;
            }
        }
    }

    /// Журналирование синтаксического инцидента. Разбор не прерывается.
    fn push_warning(
        &mut self,
        line_no: usize,
        raw_line: &str,
        kind: ParseWarningKind,
        detail: String,
    ) {
        self.warnings.push(ParseWarning {
            line_no,
            raw_line: raw_line.to_string(),
            kind,
            detail,
        });
        self.report.rejected_lines += 1;
    }
}

/// Эталонный скрипт init.deix (встроен в ядро для стадии init_boot).
/// В реальной сборке файл читается из защищённого раздела /init_boot
/// (EROFS, ReadOnly); здесь — константа для раннего самоконтроля ядра.
pub const INIT_DEIX_SCRIPT: &str = concat!(
    "# Скрипт инициализации и развертывания DeiX OS — мастер-карта разделов\n",
    "on init_boot\n",
    "    mount erofs /dev/block/by-name/kernel /kernel ro\n",
    "    mount erofs /dev/block/by-name/init_boot /init_boot ro\n",
    "    service pid1_core /bin/pid1_core 0\n",
    "\n",
    "on vendor_boot\n",
    "    mount erofs /dev/block/by-name/vendor_boot /vendor_boot ro\n",
    "\n",
    "on boot\n",
    "    mount erofs /dev/block/by-name/super /system ro\n",
    "    mount erofs /dev/block/by-name/boot /boot ro\n",
    "    mount ext4 /dev/block/by-name/userdata /userdata rw\n",
    "    service security_monitor /bin/security_monitor 3\n",
    "    service network_manager /bin/net_daemon 3\n",
    "\n",
    "on recovery\n",
    "    mount ext4 /dev/block/by-name/userdata /userdata rw\n",
    "    service twrp_shell /bin/twrp 3\n",
    "\n",
    "on fastbootd\n",
    "    mount erofs /dev/block/by-name/super /system rw\n",
    "    service fastbootd_daemon /bin/fastbootd 3\n",
    "\n",
    "on edl\n",
    "    mount erofs /dev/block/by-name/kernel /kernel rw\n",
    "    service edl_bridge /bin/edl_bridge 0\n",
);

/// СТАДИЯ INIT_BOOT (PID 1, Ring 0): разбор карты разделов init.deix и вывод
/// диагностики в консоль ядра (стиль dxinit::status / autostart::run).
/// Нарушение политики Vault (rw-монтирование системного раздела вне
/// Fastbootd/EDL/Recovery) приводит к panic! — ядро немедленно останавливается
/// (см. vault.rs). Вызывается из kernel_main() (src/lib.rs) ПОСЛЕ инициализации
/// аллокатора и ФС, но ДО запуска пользовательского пространства.
pub fn boot_report(script: &str) {
    let mut parser: InitParser = InitParser::new();
    let _registry: BTreeMap<BootStage, Vec<Command>> = parser.parse(script);

    crate::println!("  [init_parser] Стадия init_boot: init.deix разобран");
    for stage in parser.stage_order.iter() {
        match parser.registry.get(stage) {
            Some(commands) => {
                crate::println!(
                    "    [stage: {}] — {} команд(ы):",
                    stage.as_token(),
                    commands.len()
                );
                for command in commands.iter() {
                    crate::println!("      -> {}", command.describe());
                }
            }
            None => {
                crate::println!("    [stage: {}] — команд нет", stage.as_token());
            }
        }
    }
    match parser.warnings.is_empty() {
        true => crate::println!("  [init_parser] Синтаксических предупреждений нет"),
        false => {
            for warning in parser.warnings.iter() {
                crate::println!(
                    "    [WARN] строка {} | {} | {}",
                    warning.line_no,
                    warning.kind.describe(),
                    warning.detail
                );
            }
        }
    }
    crate::println!(
        "  [init_parser] Итог: всего {}, принято {}, отклонено {}, пропущено {}",
        parser.report.total_lines,
        parser.report.accepted_lines,
        parser.report.rejected_lines,
        parser.report.skipped_lines
    );
}
