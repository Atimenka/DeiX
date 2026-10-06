//! Реестр кодов ошибок `DX-<SUBSYSTEM>-<CODE>`.
//!
//! Каждому конкретному сценарию сбоя сопоставлен ровно один код. Код — это
//! `(Subsystem, u16)`, упакованные в `u32`, поэтому `ErrorCode` остаётся
//! `Copy` и пригоден для записи в кольцевой буфер из контекста прерывания
//! без обращения к куче.
//!
//! Текстовое описание и рекомендованное действие живут в статической таблице
//! `TABLE` и подбираются линейным поиском по коду — таблица небольшая, а
//! поиск происходит уже вне критического пути журналирования.

/// Подсистема — средняя часть кода (`DX-MEM-0003`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Subsystem {
    Kernel = 0,
    Memory = 1,
    Vfs = 2,
    Erofs = 3,
    Ext2 = 4,
    Kmod = 5,
    Dinit = 6,
    Gfx = 7,
    Net = 8,
    Audio = 9,
    Disk = 10,
    Elf = 11,
    Pkg = 12,
    Security = 13,
}

/// Все подсистемы в порядке возрастания — для вывода списков и фильтров.
pub const ALL_SUBSYSTEMS: &[Subsystem] = &[
    Subsystem::Kernel,
    Subsystem::Memory,
    Subsystem::Vfs,
    Subsystem::Erofs,
    Subsystem::Ext2,
    Subsystem::Kmod,
    Subsystem::Dinit,
    Subsystem::Gfx,
    Subsystem::Net,
    Subsystem::Audio,
    Subsystem::Disk,
    Subsystem::Elf,
    Subsystem::Pkg,
    Subsystem::Security,
];

impl Subsystem {
    /// Короткий идентификатор из кода (`MEM`).
    pub const fn tag(self) -> &'static str {
        match self {
            Subsystem::Kernel => "KRN",
            Subsystem::Memory => "MEM",
            Subsystem::Vfs => "VFS",
            Subsystem::Erofs => "ERO",
            Subsystem::Ext2 => "EXT",
            Subsystem::Kmod => "KMD",
            Subsystem::Dinit => "DIN",
            Subsystem::Gfx => "GFX",
            Subsystem::Net => "NET",
            Subsystem::Audio => "AUD",
            Subsystem::Disk => "DISK",
            Subsystem::Elf => "ELF",
            Subsystem::Pkg => "PKG",
            Subsystem::Security => "SEC",
        }
    }

    /// Человекочитаемое название подсистемы.
    pub const fn name(self) -> &'static str {
        match self {
            Subsystem::Kernel => "Ядро",
            Subsystem::Memory => "Память",
            Subsystem::Vfs => "Виртуальная ФС",
            Subsystem::Erofs => "EROFS",
            Subsystem::Ext2 => "EXT2",
            Subsystem::Kmod => "Модули ядра",
            Subsystem::Dinit => "Dinit",
            Subsystem::Gfx => "Графика",
            Subsystem::Net => "Сеть",
            Subsystem::Audio => "Звук",
            Subsystem::Disk => "Диск",
            Subsystem::Elf => "ELF-загрузчик",
            Subsystem::Pkg => "Пакеты",
            Subsystem::Security => "Безопасность",
        }
    }

    /// Разбирает короткий идентификатор (`"MEM"`) обратно в подсистему.
    pub fn from_tag(tag: &str) -> Option<Self> {
        for s in ALL_SUBSYSTEMS {
            if s.tag().eq_ignore_ascii_case(tag) {
                return Some(*s);
            }
        }
        None
    }
}

/// Код ошибки: `subsystem` в старшем байтном поле, номер — в младших 16 битах.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ErrorCode(pub u32);

impl ErrorCode {
    /// Собирает код из подсистемы и номера.
    pub const fn new(sub: Subsystem, num: u16) -> Self {
        Self(((sub as u32) << 16) | (num as u32))
    }

    /// Подсистема кода.
    pub const fn subsystem(self) -> Subsystem {
        // `as u8` отбрасывает старшие биты; все значения Subsystem < 256.
        match (self.0 >> 16) as u8 {
            0 => Subsystem::Kernel,
            1 => Subsystem::Memory,
            2 => Subsystem::Vfs,
            3 => Subsystem::Erofs,
            4 => Subsystem::Ext2,
            5 => Subsystem::Kmod,
            6 => Subsystem::Dinit,
            7 => Subsystem::Gfx,
            8 => Subsystem::Net,
            9 => Subsystem::Audio,
            10 => Subsystem::Disk,
            11 => Subsystem::Elf,
            12 => Subsystem::Pkg,
            _ => Subsystem::Security,
        }
    }

    /// Числовая часть кода.
    pub const fn number(self) -> u16 {
        (self.0 & 0xFFFF) as u16
    }

    /// Форматирует код в `DX-MEM-0003` без обращения к куче.
    /// Возвращает длину записанного фрагмента.
    pub fn write_to(self, out: &mut [u8]) -> usize {
        let tag = self.subsystem().tag();
        let num = self.number();
        let d = [
            b'D',
            b'X',
            b'-',
            tag.as_bytes()[0],
            tag.as_bytes()[1],
            tag.as_bytes()[2],
            b'-',
            b'0' + ((num / 1000) % 10) as u8,
            b'0' + ((num / 100) % 10) as u8,
            b'0' + ((num / 10) % 10) as u8,
            b'0' + (num % 10) as u8,
        ];
        let n = d.len().min(out.len());
        out[..n].copy_from_slice(&d[..n]);
        n
    }

    /// Строковое представление (`DX-MEM-0003`) — для мест, где куча доступна.
    pub fn as_string(self) -> alloc::string::String {
        let mut buf = [0u8; 16];
        let n = self.write_to(&mut buf);
        alloc::string::String::from_utf8_lossy(&buf[..n]).into_owned()
    }

    /// Описание и действие из реестра, если код зарегистрирован.
    pub fn info(self) -> Option<&'static CodeInfo> {
        TABLE.iter().find(|c| c.code == self)
    }

    /// Разбирает `DX-MEM-0003` (регистр не важен).
    pub fn parse(text: &str) -> Option<Self> {
        let t = text.trim();
        let body = t.strip_prefix("DX-").or_else(|| t.strip_prefix("dx-"))?;
        let (tag, num) = body.split_once('-')?;
        let sub = Subsystem::from_tag(tag)?;
        let num: u32 = num.parse().ok()?;
        if num > u16::MAX as u32 {
            return None;
        }
        Some(Self::new(sub, num as u16))
    }
}

/// Описание кода в реестре.
pub struct CodeInfo {
    pub code: ErrorCode,
    /// Короткое название сбоя.
    pub summary: &'static str,
    /// Развёрнутое объяснение причины.
    pub detail: &'static str,
    /// Рекомендованное действие пользователя или системы.
    pub action: &'static str,
}

/// Полная таблица кодов. Именно она отвечает на «что означает этот код».
pub static TABLE: &[CodeInfo] = &[
    // ==================== Ядро (KRN) ====================
    CodeInfo { code: ErrorCode::new(Subsystem::Kernel, 1), summary: "Куча ядра исчерпана",
        detail: "Выделитель ядра не смог отдать блок: свободных страниц кучи нет.",
        action: "Перезагрузите систему; при повторе сократите число одновременно запущенных служб." },
    CodeInfo { code: ErrorCode::new(Subsystem::Kernel, 2), summary: "Переполнение стека потока",
        detail: "Поток ядра вышел за границу своего стека (защитная страница).",
        action: "Уменьшите глубину рекурсии в модуле; увеличьте размер стека потока." },
    CodeInfo { code: ErrorCode::new(Subsystem::Kernel, 3), summary: "Некорректная таблица страниц",
        detail: "Запись таблицы страниц содержит недопустимые флаги или адрес выше физического предела.",
        action: "Проверьте код, который правит таблицы страниц напрямую." },
    CodeInfo { code: ErrorCode::new(Subsystem::Kernel, 4), summary: "Необработанное исключение CPU",
        detail: "Процессор выдал исключение, для которого в IDT нет осмысленного обработчика.",
        action: "Сохраните отчёт об отказе и передайте разработчику." },
    CodeInfo { code: ErrorCode::new(Subsystem::Kernel, 5), summary: "Двойная ошибка (Double Fault)",
        detail: "Обработчик исключения сам вызвал исключение; стек обработчика повреждён.",
        action: "Система будет перезагружена. Сохраните отчёт об отказе." },
    CodeInfo { code: ErrorCode::new(Subsystem::Kernel, 6), summary: "Тройная ошибка (Triple Fault)",
        detail: "CPU не смог обработать даже Double Fault; дальнейшая работа невозможна.",
        action: "Выполняется аппаратный сброс. Проверьте отчёт об отказе после загрузки." },
    CodeInfo { code: ErrorCode::new(Subsystem::Kernel, 7), summary: "Отчёт об отказе сохранён",
        detail: "Дамп отказа ядра записан в энергонезависимое хранилище и доступен после перезагрузки.",
        action: "Откройте Центр ошибок → Отчёты об отказе." },
    CodeInfo { code: ErrorCode::new(Subsystem::Kernel, 8), summary: "Сбой инициализации GDT/TSS",
        detail: "Не удалось корректно настроить дескрипторы сегментов или TSS.",
        action: "Проверьте порядок инициализации ядра до включения прерываний." },
    CodeInfo { code: ErrorCode::new(Subsystem::Kernel, 9), summary: "Сбой регистрации обработчика прерываний",
        detail: "Вектор IDT уже занят или индекс вектора вне диапазона.",
        action: "Проверьте драйвер, регистрирующий обработчик." },
    CodeInfo { code: ErrorCode::new(Subsystem::Kernel, 10), summary: "Сбой системного таймера",
        detail: "PIT не подтвердил настройку; планировщик не получает тиков.",
        action: "Проверьте конфигурацию порта 0x40-0x43 и маски PIC." },
    CodeInfo { code: ErrorCode::new(Subsystem::Kernel, 11), summary: "Аварийное завершение потока ядра",
        detail: "Системный поток завершён из-за неисправимой ошибки в своей задаче.",
        action: "Откройте Диспетчер задач и проверьте состояние службы." },
    CodeInfo { code: ErrorCode::new(Subsystem::Kernel, 12), summary: "Невосстановимое внутреннее состояние",
        detail: "Инвариант ядра нарушен, безопасное продолжение работы невозможно.",
        action: "Сохраните отчёт об отказе и выполните перезагрузку." },

    // ==================== Память (MEM) ====================
    CodeInfo { code: ErrorCode::new(Subsystem::Memory, 1), summary: "Ошибка страницы (Page Fault)",
        detail: "Обращение к странице, которая не отображена или недоступна в текущем режиме.",
        action: "Проверьте адрес, по которому произошло обращение." },
    CodeInfo { code: ErrorCode::new(Subsystem::Memory, 2), summary: "Нарушение прав доступа",
        detail: "Код попытался выполнить операцию, запрещённую флагами страницы.",
        action: "Проверьте права сегмента: запись в read-only или исполнение в NX." },
    CodeInfo { code: ErrorCode::new(Subsystem::Memory, 3), summary: "Некорректный адрес",
        detail: "Передан адрес вне допустимого диапазона (NULL, неканонический адрес).",
        action: "Проверьте источник указателя." },
    CodeInfo { code: ErrorCode::new(Subsystem::Memory, 4), summary: "Переполнение стека",
        detail: "Указатель стека покинул отведённую область.",
        action: "Уменьшите размер локальных буферов или глубину рекурсии." },
    CodeInfo { code: ErrorCode::new(Subsystem::Memory, 5), summary: "Исчерпание кучи",
        detail: "Недостаточно памяти для запрашиваемого выделения.",
        action: "Освободите память или уменьшите размер буфера." },
    CodeInfo { code: ErrorCode::new(Subsystem::Memory, 6), summary: "Сбой выделения памяти",
        detail: "Выделитель вернул ошибку вместо адреса.",
        action: "Проверьте размер запроса и состояние кучи." },
    CodeInfo { code: ErrorCode::new(Subsystem::Memory, 7), summary: "Повреждение таблицы страниц",
        detail: "Структура таблицы страниц не соответствует ожидаемой.",
        action: "Проверьте код отображения памяти." },
    CodeInfo { code: ErrorCode::new(Subsystem::Memory, 8), summary: "Ошибка сегментации приложения",
        detail: "Пользовательская программа обратилась к недоступной ей памяти.",
        action: "Приложение будет остановлено; ядро продолжает работу." },
    CodeInfo { code: ErrorCode::new(Subsystem::Memory, 9), summary: "Нарушение доступа пользовательской памяти",
        detail: "Пользовательский код попытался обратиться к памяти ядра.",
        action: "Приложение будет остановлено." },
    CodeInfo { code: ErrorCode::new(Subsystem::Memory, 10), summary: "Некорректное освобождение памяти",
        detail: "Освобождение указателя, который не выдавался выделителем, либо повторное освобождение.",
        action: "Проверьте пары alloc/free в модуле." },
    CodeInfo { code: ErrorCode::new(Subsystem::Memory, 11), summary: "Пул памяти исчерпан",
        detail: "Фиксированный пул (кольцевые буферы, слоты задач) заполнен.",
        action: "Уменьшите интенсивность событий или увеличьте размер пула." },
    CodeInfo { code: ErrorCode::new(Subsystem::Memory, 12), summary: "Сбой DMA-буфера",
        detail: "Буфер прямого доступа к памяти недоступен или пересекает границу."
            , action: "Проверьте выравнивание и размер DMA-области." },

    // ==================== Виртуальная ФС (VFS) ====================
    CodeInfo { code: ErrorCode::new(Subsystem::Vfs, 1), summary: "Сбой монтирования",
        detail: "Том не удалось подключить к точке монтирования.",
        action: "Проверьте целостность тома и наличие драйвера ФС." },
    CodeInfo { code: ErrorCode::new(Subsystem::Vfs, 2), summary: "Сбой размонтирования",
        detail: "Том не удалось отключить: есть открытые дескрипторы.",
        action: "Закройте файлы и повторите операцию." },
    CodeInfo { code: ErrorCode::new(Subsystem::Vfs, 3), summary: "Файл не найден",
        detail: "По указанному пути нет ни файла, ни каталога.",
        action: "Проверьте путь и имя файла." },
    CodeInfo { code: ErrorCode::new(Subsystem::Vfs, 4), summary: "Доступ запрещён",
        detail: "У процесса нет прав на запрошенную операцию.",
        action: "Проверьте права доступа и мандаты процесса." },
    CodeInfo { code: ErrorCode::new(Subsystem::Vfs, 5), summary: "Файловая система только для чтения",
        detail: "Том смонтирован read-only (системный раздел).",
        action: "Пишите в /userdata вместо /system." },
    CodeInfo { code: ErrorCode::new(Subsystem::Vfs, 6), summary: "Каталог не пуст",
        detail: "Удаление каталога невозможно, пока в нём есть записи.",
        action: "Сначала удалите содержимое каталога." },
    CodeInfo { code: ErrorCode::new(Subsystem::Vfs, 7), summary: "Слишком много открытых файлов",
        detail: "Таблица дескрипторов процесса заполнена.",
        action: "Закройте неиспользуемые дескрипторы." },
    CodeInfo { code: ErrorCode::new(Subsystem::Vfs, 8), summary: "Некорректный путь",
        detail: "Путь пуст, содержит недопустимые компоненты или слишком длинный.",
        action: "Проверьте синтаксис пути." },
    CodeInfo { code: ErrorCode::new(Subsystem::Vfs, 9), summary: "Зацикливание символических ссылок",
        detail: "Разрешение пути превысило предельную глубину перехода по ссылкам.",
        action: "Проверьте цепочку ссылок на циклы." },
    CodeInfo { code: ErrorCode::new(Subsystem::Vfs, 10), summary: "Ошибка ввода-вывода",
        detail: "Нижележащий носитель вернул ошибку при чтении или записи.",
        action: "Проверьте состояние диска (DX-DISK-*)." },
    CodeInfo { code: ErrorCode::new(Subsystem::Vfs, 11), summary: "Несогласованность файловой системы",
        detail: "Метаданные тома противоречат друг другу.",
        action: "Выполните проверку целостности тома." },
    CodeInfo { code: ErrorCode::new(Subsystem::Vfs, 12), summary: "Сбой воспроизведения журнала",
        detail: "Журнал ФС не удалось применить при монтировании.",
        action: "Том будет смонтирован без журнала; выполните проверку." },

    // ==================== EROFS (ERO) ====================
    CodeInfo { code: ErrorCode::new(Subsystem::Erofs, 1), summary: "Некорректный суперблок",
        detail: "Суперблок EROFS не читается или содержит противоречивые поля.",
        action: "Пересоберите образ /system." },
    CodeInfo { code: ErrorCode::new(Subsystem::Erofs, 2), summary: "Магическое число не совпадает",
        detail: "На носителе нет подписи EROFS.",
        action: "Проверьте, что на раздел записан именно образ EROFS." },
    CodeInfo { code: ErrorCode::new(Subsystem::Erofs, 3), summary: "Ошибка таблицы inode",
        detail: "Таблица inode указывает за границы тома.",
        action: "Пересоберите образ." },
    CodeInfo { code: ErrorCode::new(Subsystem::Erofs, 4), summary: "Некорректный inode",
        detail: "Поля inode противоречивы (тип, размер, смещение).",
        action: "Пересоберите образ." },
    CodeInfo { code: ErrorCode::new(Subsystem::Erofs, 5), summary: "Повреждение записи каталога",
        detail: "Запись каталога имеет неверную длину или имя.",
        action: "Пересоберите образ." },
    CodeInfo { code: ErrorCode::new(Subsystem::Erofs, 6), summary: "Блок данных вне диапазона",
        detail: "Смещение блока данных превышает размер тома.",
        action: "Пересоберите образ." },
    CodeInfo { code: ErrorCode::new(Subsystem::Erofs, 7), summary: "Несовпадение контрольной суммы",
        detail: "Контрольная сумма суперблока не соответствует содержимому.",
        action: "Образ повреждён при записи — пересоберите и перезапишите." },
    CodeInfo { code: ErrorCode::new(Subsystem::Erofs, 8), summary: "Неподдерживаемая возможность",
        detail: "Образ использует возможность, которой нет в этой реализации EROFS.",
        action: "Соберите образ без сжатия и расширенных атрибутов." },
    CodeInfo { code: ErrorCode::new(Subsystem::Erofs, 9), summary: "Образ усечён",
        detail: "Размер тома меньше, чем требует суперблок.",
        action: "Проверьте размер раздела и полноту записи образа." },

    // ==================== EXT2 (EXT) ====================
    CodeInfo { code: ErrorCode::new(Subsystem::Ext2, 1), summary: "Некорректный суперблок",
        detail: "Суперблок EXT2 отсутствует или повреждён; том не отформатирован.",
        action: "Отформатируйте /userdata, если данные не нужны." },
    CodeInfo { code: ErrorCode::new(Subsystem::Ext2, 2), summary: "Некорректный inode",
        detail: "Inode содержит недопустимый тип, размер или число блоков.",
        action: "Выполните проверку целостности тома." },
    CodeInfo { code: ErrorCode::new(Subsystem::Ext2, 3), summary: "Сбой выделения блока",
        detail: "Свободный блок не найден, хотя счётчики обещали его наличие.",
        action: "Выполните проверку целостности тома." },
    CodeInfo { code: ErrorCode::new(Subsystem::Ext2, 4), summary: "Повреждение каталога",
        detail: "Запись каталога имеет неверную длину или ссылается на несуществующий inode.",
        action: "Выполните проверку целостности тома." },
    CodeInfo { code: ErrorCode::new(Subsystem::Ext2, 5), summary: "Сбой восстановления",
        detail: "Структуры тома не удалось привести в согласованное состояние.",
        action: "Сделайте резервную копию и отформатируйте том." },
    CodeInfo { code: ErrorCode::new(Subsystem::Ext2, 6), summary: "Ошибка чтения",
        detail: "Чтение с нижележащего носителя завершилось ошибкой.",
        action: "Проверьте состояние диска (DX-DISK-*)." },
    CodeInfo { code: ErrorCode::new(Subsystem::Ext2, 7), summary: "Ошибка записи",
        detail: "Запись на нижележащий носитель завершилась ошибкой.",
        action: "Проверьте состояние диска (DX-DISK-*)." },
    CodeInfo { code: ErrorCode::new(Subsystem::Ext2, 8), summary: "Несогласованность файловой системы",
        detail: "Метаданные тома противоречат друг другу.",
        action: "Выполните проверку целостности тома." },
    CodeInfo { code: ErrorCode::new(Subsystem::Ext2, 9), summary: "Нет свободных блоков",
        detail: "Место на /userdata закончилось.",
        action: "Удалите ненужные файлы." },
    CodeInfo { code: ErrorCode::new(Subsystem::Ext2, 10), summary: "Нет свободных inode",
        detail: "Исчерпан лимит числа файлов на томе.",
        action: "Удалите ненужные файлы или каталоги." },

    // ==================== Модули ядра (KMD) ====================
    CodeInfo { code: ErrorCode::new(Subsystem::Kmod, 1), summary: "Модуль не найден",
        detail: "Файл .kmod отсутствует в /system/kmod.",
        action: "Проверьте состав системного образа." },
    CodeInfo { code: ErrorCode::new(Subsystem::Kmod, 2), summary: "Некорректная подпись модуля",
        detail: "Заголовок .kmod не содержит ожидаемого магического числа.",
        action: "Пересоберите модуль актуальным инструментарием." },
    CodeInfo { code: ErrorCode::new(Subsystem::Kmod, 3), summary: "Сбой загрузки модуля",
        detail: "Тело модуля не удалось разместить в памяти.",
        action: "Проверьте доступную память и размер модуля." },
    CodeInfo { code: ErrorCode::new(Subsystem::Kmod, 4), summary: "Сбой инициализации модуля",
        detail: "Точка входа init() вернула код ошибки.",
        action: "Модуль будет помещён в карантин." },
    CodeInfo { code: ErrorCode::new(Subsystem::Kmod, 5), summary: "Аварийное завершение модуля",
        detail: "Код модуля вызвал исключение во время работы.",
        action: "Модуль будет помещён в карантин; ядро продолжит работу." },
    CodeInfo { code: ErrorCode::new(Subsystem::Kmod, 6), summary: "Нарушение доступа модуля к памяти",
        detail: "Модуль обратился за границы выделенного ему региона.",
        action: "Модуль будет помещён в карантин." },
    CodeInfo { code: ErrorCode::new(Subsystem::Kmod, 7), summary: "Неразрешённый символ модуля",
        detail: "Модуль требует сервис ядра, которого нет в KernelApi.",
        action: "Обновите модуль или ядро до совместимых версий." },
    CodeInfo { code: ErrorCode::new(Subsystem::Kmod, 8), summary: "Несовместимость ABI модуля",
        detail: "Версия KernelApi модуля не совпадает с версией ядра.",
        action: "Пересоберите модуль под текущую версию ядра." },
    CodeInfo { code: ErrorCode::new(Subsystem::Kmod, 9), summary: "Превышено время инициализации модуля",
        detail: "init() не вернул управление за отведённое время.",
        action: "Модуль будет помещён в карантин." },
    CodeInfo { code: ErrorCode::new(Subsystem::Kmod, 10), summary: "Отсутствует зависимость модуля",
        detail: "Модуль требует другой модуль, который не загружен.",
        action: "Проверьте порядок загрузки модулей." },

    // ==================== Dinit (DIN) ====================
    CodeInfo { code: ErrorCode::new(Subsystem::Dinit, 1), summary: "Сбой запуска службы",
        detail: "Исполняемый файл службы не найден или не удалось создать процесс.",
        action: "Проверьте путь к исполняемому файлу в описании службы." },
    CodeInfo { code: ErrorCode::new(Subsystem::Dinit, 2), summary: "Аварийное завершение службы",
        detail: "Процесс службы завершился неожиданно.",
        action: "Проверьте журнал службы (DX-DIN) и её последний код ошибки." },
    CodeInfo { code: ErrorCode::new(Subsystem::Dinit, 3), summary: "Превышено время ожидания службы",
        detail: "Служба не отчиталась о готовности за отведённое время.",
        action: "Увеличьте таймаут или проверьте зависимость службы." },
    CodeInfo { code: ErrorCode::new(Subsystem::Dinit, 4), summary: "Сбой зависимости",
        detail: "Служба, от которой зависит текущая, не запустилась.",
        action: "Устраните сбой в службе-зависимости." },
    CodeInfo { code: ErrorCode::new(Subsystem::Dinit, 5), summary: "Ошибка разбора конфигурации",
        detail: "Описание службы содержит неизвестное поле или неверный синтаксис.",
        action: "Проверьте файл описания службы." },
    CodeInfo { code: ErrorCode::new(Subsystem::Dinit, 6), summary: "Превышен предел перезапусков",
        detail: "Служба падала слишком часто; супервизор прекратил попытки.",
        action: "Устраните причину падения и запустите службу вручную." },
    CodeInfo { code: ErrorCode::new(Subsystem::Dinit, 7), summary: "Нарушение мандатов службой",
        detail: "Служба запросила операцию вне набора выданных ей мандатов.",
        action: "Проверьте набор мандатов в описании службы." },
    CodeInfo { code: ErrorCode::new(Subsystem::Dinit, 8), summary: "Исполняемый файл службы отсутствует",
        detail: "Путь binary_path из описания службы не существует в /system.",
        action: "Проверьте состав системного образа." },
    CodeInfo { code: ErrorCode::new(Subsystem::Dinit, 9), summary: "Отказано в запуске службы",
        detail: "Запуск службы запрещён политикой или правами вызывающего.",
        action: "Проверьте права инициатора." },
    CodeInfo { code: ErrorCode::new(Subsystem::Dinit, 10), summary: "Сбой супервизора служб",
        detail: "Супервизор не смог обработать событие жизненного цикла службы.",
        action: "Сохраните отчёт об отказе." },

    // ==================== Графика (GFX) ====================
    CodeInfo { code: ErrorCode::new(Subsystem::Gfx, 1), summary: "Сбой инициализации GPU",
        detail: "Видеоадаптер не ответил на инициализацию.",
        action: "Система переключится в текстовый режим консоли." },
    CodeInfo { code: ErrorCode::new(Subsystem::Gfx, 2), summary: "Сбой выделения видеопамяти",
        detail: "Недостаточно видеопамяти для кадра или поверхности.",
        action: "Уменьшите разрешение или число открытых окон." },
    CodeInfo { code: ErrorCode::new(Subsystem::Gfx, 3), summary: "Видеорежим не поддерживается",
        detail: "Запрошенное разрешение или глубина цвета недоступны.",
        action: "Выберите поддерживаемый режим в настройках экрана." },
    CodeInfo { code: ErrorCode::new(Subsystem::Gfx, 4), summary: "Кадровый буфер недоступен",
        detail: "Адрес framebuffer не получен от загрузчика или VBE.",
        action: "Система продолжит работу в текстовом режиме." },
    CodeInfo { code: ErrorCode::new(Subsystem::Gfx, 5), summary: "Сбой композитора",
        detail: "Композитор окон не смог собрать кадр.",
        action: "Окно будет перерисовано; при повторе закройте приложение." },
    CodeInfo { code: ErrorCode::new(Subsystem::Gfx, 6), summary: "Сбой выделения поверхности",
        detail: "Не удалось создать буфер окна требуемого размера.",
        action: "Уменьшите размер окна." },
    CodeInfo { code: ErrorCode::new(Subsystem::Gfx, 7), summary: "Превышено время ожидания VSync",
        detail: "Дисплей не подтвердил вертикальную синхронизацию.",
        action: "Композитор перейдёт на рисование без синхронизации." },
    CodeInfo { code: ErrorCode::new(Subsystem::Gfx, 8), summary: "Сбой отрисовки окна",
        detail: "Обработчик отрисовки приложения вернул ошибку.",
        action: "Окно останется в последнем корректном состоянии." },
    CodeInfo { code: ErrorCode::new(Subsystem::Gfx, 9), summary: "Зависание GPU",
        detail: "GPU не отвечает на запросы дольше допустимого времени.",
        action: "Выполняется сброс дисплея в текстовый режим." },

    // ==================== Сеть (NET) ====================
    CodeInfo { code: ErrorCode::new(Subsystem::Net, 1), summary: "Сетевой адаптер не обнаружен",
        detail: "Поддерживаемый NIC не найден на шине PCI.",
        action: "Сетевые функции будут недоступны." },
    CodeInfo { code: ErrorCode::new(Subsystem::Net, 2), summary: "Сбой инициализации адаптера",
        detail: "NIC не удалось перевести в рабочее состояние.",
        action: "Проверьте модель адаптера и доступность памяти под кольца." },
    CodeInfo { code: ErrorCode::new(Subsystem::Net, 3), summary: "Нет подключения (link down)",
        detail: "Физический линк интерфейса не поднят.",
        action: "Проверьте кабель и порт коммутатора." },
    CodeInfo { code: ErrorCode::new(Subsystem::Net, 4), summary: "Сбой получения адреса по DHCP",
        detail: "DHCP-сервер не выдал адрес за отведённое время.",
        action: "Настройте статический адрес или проверьте DHCP-сервер." },
    CodeInfo { code: ErrorCode::new(Subsystem::Net, 5), summary: "Сбой разрешения имени (DNS)",
        detail: "Имя не удалось преобразовать в адрес.",
        action: "Проверьте адрес DNS-сервера и доступность сети." },
    CodeInfo { code: ErrorCode::new(Subsystem::Net, 6), summary: "Сбой разрешения MAC (ARP)",
        detail: "Ответ на ARP-запрос не получен.",
        action: "Проверьте адрес шлюза и физическое подключение." },
    CodeInfo { code: ErrorCode::new(Subsystem::Net, 7), summary: "Пакет отброшен",
        detail: "Кольцо приёма переполнено, кадр не поместился.",
        action: "Счётчик потерь растёт — уменьшите нагрузку или увеличьте кольцо." },
    CodeInfo { code: ErrorCode::new(Subsystem::Net, 8), summary: "Ошибка контрольной суммы",
        detail: "Контрольная сумма IP/TCP/UDP не совпала.",
        action: "Пакет отброшен; проверьте целостность канала." },
    CodeInfo { code: ErrorCode::new(Subsystem::Net, 9), summary: "Ошибка сокета",
        detail: "Операция с сокетом вернула ошибку.",
        action: "Проверьте состояние соединения." },
    CodeInfo { code: ErrorCode::new(Subsystem::Net, 10), summary: "Соединение отклонено",
        detail: "Удалённая сторона ответила RST.",
        action: "Проверьте доступность порта на сервере." },
    CodeInfo { code: ErrorCode::new(Subsystem::Net, 11), summary: "Переполнение буфера",
        detail: "Буфер приёма или передачи заполнен.",
        action: "Уменьшите размер передаваемых данных." },

    // ==================== Звук (AUD) ====================
    CodeInfo { code: ErrorCode::new(Subsystem::Audio, 1), summary: "Звуковое устройство не найдено",
        detail: "Поддерживаемый аудиокодек не обнаружен на шине PCI.",
        action: "Звук будет недоступен." },
    CodeInfo { code: ErrorCode::new(Subsystem::Audio, 2), summary: "Сбой инициализации HDA",
        detail: "Контроллер High Definition Audio не вышел из сброса.",
        action: "Звук будет недоступен." },
    CodeInfo { code: ErrorCode::new(Subsystem::Audio, 3), summary: "Кодек не обнаружен",
        detail: "Контроллер HDA работает, но кодек не ответил.",
        action: "Звук будет недоступен." },
    CodeInfo { code: ErrorCode::new(Subsystem::Audio, 4), summary: "Ошибка DMA-буфера звука",
        detail: "Буфер потока не удалось выделить или выровнять.",
        action: "Воспроизведение будет остановлено." },
    CodeInfo { code: ErrorCode::new(Subsystem::Audio, 5), summary: "Частота дискретизации не поддерживается",
        detail: "Кодек не поддерживает запрошенную частоту.",
        action: "Используйте 48000 Гц." },
    CodeInfo { code: ErrorCode::new(Subsystem::Audio, 6), summary: "Опустошение буфера (underrun)",
        detail: "Данные не успели поступить до того, как буфер опустел.",
        action: "Возможны щелчки; уменьшите нагрузку на CPU." },
    CodeInfo { code: ErrorCode::new(Subsystem::Audio, 7), summary: "Устройство занято",
        detail: "Поток уже использует аудиоустройство.",
        action: "Остановите другое воспроизведение." },

    // ==================== Диск (DISK) ====================
    CodeInfo { code: ErrorCode::new(Subsystem::Disk, 1), summary: "Устройство не обнаружено",
        detail: "Контроллер не ответил на идентификацию.",
        action: "Проверьте подключение носителя." },
    CodeInfo { code: ErrorCode::new(Subsystem::Disk, 2), summary: "Ошибка чтения",
        detail: "Операция чтения секторов завершилась ошибкой.",
        action: "Проверьте носитель на дефекты." },
    CodeInfo { code: ErrorCode::new(Subsystem::Disk, 3), summary: "Ошибка записи",
        detail: "Операция записи секторов завершилась ошибкой.",
        action: "Проверьте носитель и признак защиты от записи." },
    CodeInfo { code: ErrorCode::new(Subsystem::Disk, 4), summary: "Превышено время ожидания устройства",
        detail: "Устройство не установило флаг готовности.",
        action: "Контроллер будет сброшен." },
    CodeInfo { code: ErrorCode::new(Subsystem::Disk, 5), summary: "Дефектный сектор",
        detail: "Носитель сообщил о неисправимой ошибке сектора.",
        action: "Сделайте резервную копию данных и замените носитель." },
    CodeInfo { code: ErrorCode::new(Subsystem::Disk, 6), summary: "Сброс контроллера",
        detail: "Контроллер сброшен после серии ошибок.",
        action: "Операция будет повторена; при повторе проверьте носитель." },
    CodeInfo { code: ErrorCode::new(Subsystem::Disk, 7), summary: "Некорректная таблица разделов",
        detail: "Таблица MBR/GPT повреждена или содержит пересекающиеся разделы.",
        action: "Проверьте разметку носителя." },

    // ==================== ELF (ELF) ====================
    CodeInfo { code: ErrorCode::new(Subsystem::Elf, 1), summary: "Некорректное магическое число ELF",
        detail: "Файл не является ELF-образом.",
        action: "Проверьте, что загружается именно исполняемый файл." },
    CodeInfo { code: ErrorCode::new(Subsystem::Elf, 2), summary: "Неподдерживаемая архитектура",
        detail: "Образ собран не для x86-64.",
        action: "Пересоберите программу для x86-64." },
    CodeInfo { code: ErrorCode::new(Subsystem::Elf, 3), summary: "Некорректный заголовок программы",
        detail: "Таблица программных заголовков повреждена или пуста.",
        action: "Пересоберите программу." },
    CodeInfo { code: ErrorCode::new(Subsystem::Elf, 4), summary: "Сбой применения перемещений",
        detail: "Запись перемещения имеет неизвестный тип или указывает за границы образа.",
        action: "Пересоберите программу как PIE." },
    CodeInfo { code: ErrorCode::new(Subsystem::Elf, 5), summary: "Отсутствует точка входа",
        detail: "Поле e_entry равно нулю или указывает вне загруженных сегментов.",
        action: "Проверьте компоновку программы." },
    CodeInfo { code: ErrorCode::new(Subsystem::Elf, 6), summary: "Секция вне границ",
        detail: "Сегмент PT_LOAD выходит за пределы отведённой области.",
        action: "Уменьшите размер программы или увеличьте область." },
    CodeInfo { code: ErrorCode::new(Subsystem::Elf, 7), summary: "Неподдерживаемый тип файла",
        detail: "Ожидается PIE (ET_DYN); ET_EXEC не перемещаем и не поддерживается.",
        action: "Соберите программу с -pie." },
    CodeInfo { code: ErrorCode::new(Subsystem::Elf, 8), summary: "Сбой разрешения символа",
        detail: "Символ, требуемый программой, не найден.",
        action: "Статически линкуйте программу." },

    // ==================== Пакеты (PKG) ====================
    CodeInfo { code: ErrorCode::new(Subsystem::Pkg, 1), summary: "Пакет не найден",
        detail: "Пакет с таким именем отсутствует в базе.",
        action: "Проверьте имя пакета." },
    CodeInfo { code: ErrorCode::new(Subsystem::Pkg, 2), summary: "Некорректная подпись пакета",
        detail: "Подпись пакета не проверена или не совпала.",
        action: "Установка отменена; получите пакет из доверенного источника." },
    CodeInfo { code: ErrorCode::new(Subsystem::Pkg, 3), summary: "Несовпадение контрольной суммы",
        detail: "Содержимое пакета не соответствует заявленной сумме.",
        action: "Пакет повреждён — скачайте его заново." },
    CodeInfo { code: ErrorCode::new(Subsystem::Pkg, 4), summary: "Отсутствует зависимость пакета",
        detail: "Требуемый пакет не установлен.",
        action: "Сначала установите недостающие зависимости." },
    CodeInfo { code: ErrorCode::new(Subsystem::Pkg, 5), summary: "Обнаружен конфликт пакетов",
        detail: "Пакет конфликтует с уже установленным.",
        action: "Удалите конфликтующий пакет." },
    CodeInfo { code: ErrorCode::new(Subsystem::Pkg, 6), summary: "Сбой установки пакета",
        detail: "Не удалось распаковать или разместить файлы пакета.",
        action: "Проверьте свободное место в /userdata." },
    CodeInfo { code: ErrorCode::new(Subsystem::Pkg, 7), summary: "Сбой удаления пакета",
        detail: "Файлы пакета не удалось удалить полностью.",
        action: "Удалите оставшиеся файлы вручную." },
    CodeInfo { code: ErrorCode::new(Subsystem::Pkg, 8), summary: "Повреждение базы пакетов",
        detail: "Файл базы пакетов не читается или противоречив.",
        action: "База будет перестроена при следующем сканировании." },
    CodeInfo { code: ErrorCode::new(Subsystem::Pkg, 9), summary: "Недостаточно места",
        detail: "В /userdata нет места для установки пакета.",
        action: "Освободите место и повторите установку." },
    CodeInfo { code: ErrorCode::new(Subsystem::Pkg, 10), summary: "Пакет уже установлен",
        detail: "Установлена та же или более новая версия пакета.",
        action: "Используйте обновление вместо установки." },

    // ==================== Безопасность (SEC) ====================
    CodeInfo { code: ErrorCode::new(Subsystem::Security, 1), summary: "Доступ запрещён",
        detail: "Операция запрещена политикой безопасности.",
        action: "Проверьте права и мандаты процесса." },
    CodeInfo { code: ErrorCode::new(Subsystem::Security, 2), summary: "Нарушение мандатов",
        detail: "Процесс выполнил операцию вне выданного ему набора мандатов.",
        action: "Проверьте набор мандатов процесса." },
    CodeInfo { code: ErrorCode::new(Subsystem::Security, 3), summary: "Некорректная подпись",
        detail: "Подпись образа или пакета не прошла проверку.",
        action: "Загрузка отменена." },
    CodeInfo { code: ErrorCode::new(Subsystem::Security, 4), summary: "Нарушение политики",
        detail: "Действие противоречит активной политике безопасности.",
        action: "Проверьте правила политики." },
    CodeInfo { code: ErrorCode::new(Subsystem::Security, 5), summary: "Сбой аудита",
        detail: "Событие не удалось записать в журнал аудита.",
        action: "Проверьте доступность журнала." },
    CodeInfo { code: ErrorCode::new(Subsystem::Security, 6), summary: "Сбой безопасной загрузки",
        detail: "Проверка целостности загрузочной цепочки не пройдена.",
        action: "Проверьте целостность загрузчика и ядра." },
];

/// Число зарегистрированных кодов — используется в самотестировании.
pub const CODE_COUNT: usize = TABLE.len();

/// Описание кода или запасной текст, если код не зарегистрирован.
pub fn summary_of(code: ErrorCode) -> &'static str {
    code.info().map(|c| c.summary).unwrap_or("Незарегистрированный код ошибки")
}

/// Рекомендованное действие по коду.
pub fn action_of(code: ErrorCode) -> &'static str {
    code.info().map(|c| c.action).unwrap_or("Сохраните отчёт об отказе.")
}

/// Развёрнутое объяснение по коду.
pub fn detail_of(code: ErrorCode) -> &'static str {
    code.info()
        .map(|c| c.detail)
        .unwrap_or("Код не найден в реестре ошибок DeiX.")
}
