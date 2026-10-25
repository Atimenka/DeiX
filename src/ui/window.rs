//! Управление окнами и отрисовка оконных контейнеров

use crate::ext2;
use crate::renderer::{Color, Rect, Renderer};
use crate::ui::apps;
use crate::ui::metrics::UiMetrics;
use crate::ui::theme::UiTheme;
use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

pub const RESOLUTION_PRESETS: [(u32, u32); 5] = [
    (640, 480),
    (800, 600),
    (1024, 768),
    (1280, 720),
    (1280, 1024),
];

#[derive(Clone, Debug)]
pub struct FileViewEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u32,
}

/// Страница мастера установки (окно "Install DeiX").
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InstallerPage {
    Disk,
    Scheme,
    Size,
    Account,
    Confirm,
    Progress,
    Done,
    Failed,
}

/// Состояние графического установщика. Всю работу делает
/// install::InstallJob — здесь только выбор параметров и навигация
/// по страницам мастера.
pub struct InstallerState {
    pub page: InstallerPage,
    pub disks: Vec<crate::install::TargetDisk>,
    pub disk_idx: usize,
    pub size_idx: usize,
    pub username: String,
    pub password: String,
    pub confirm: String,
    /// Активное поле страницы Account: 0 = username, 1 = password,
    /// 2 = confirm.
    pub field: usize,
    /// Сообщение валидации/подсказка под текущей страницей.
    pub note: Option<String>,
    pub job: Option<crate::install::InstallJob>,
    /// Заметки завершённой установки (шифрование и т.п.).
    pub result_notes: Vec<String>,
    pub error: Option<String>,
}

impl InstallerState {
    pub fn new() -> Self {
        InstallerState {
            page: InstallerPage::Disk,
            disks: crate::install::detect_disks(),
            disk_idx: 0,
            size_idx: 0,
            username: String::new(),
            password: String::new(),
            confirm: String::new(),
            field: 0,
            note: None,
            job: None,
            result_notes: Vec::new(),
            error: None,
        }
    }

    /// Доступные размеры /userdata (МиБ) для выбранного диска.
    pub fn size_options_mib(&self) -> Vec<u32> {
        let cap = self.disks.get(self.disk_idx).and_then(|d| d.sectors);
        crate::install::USERDATA_PRESETS_MIB
            .iter()
            .copied()
            .filter(|mib| match cap {
                Some(c) => mib * 2048 <= crate::install::max_userdata_sectors(c),
                None => true,
            })
            .collect()
    }

    /// Мутабельная ссылка на активное текстовое поле (страница Account).
    pub fn field_mut(&mut self) -> &mut String {
        match self.field {
            0 => &mut self.username,
            1 => &mut self.password,
            _ => &mut self.confirm,
        }
    }

    pub fn back(&mut self) {
        self.note = None;
        self.page = match self.page {
            InstallerPage::Scheme => InstallerPage::Disk,
            InstallerPage::Size => InstallerPage::Scheme,
            InstallerPage::Account => InstallerPage::Size,
            InstallerPage::Confirm => InstallerPage::Account,
            other => other,
        };
    }

    /// Кнопка Next/Install: валидация текущей страницы и переход.
    pub fn next(&mut self) {
        self.note = None;
        match self.page {
            InstallerPage::Disk => {
                if self.disks.is_empty() {
                    self.note = Some(String::from("No ATA disks found."));
                    return;
                }
                let min_cap =
                    crate::install::USERDATA_START_LBA + crate::install::USERDATA_MIN_SECTORS;
                if let Some(cap) = self.disks[self.disk_idx].sectors {
                    if cap < min_cap {
                        self.note = Some(format!(
                            "Disk too small: {} MiB, {} MiB required.",
                            cap / 2048,
                            min_cap / 2048
                        ));
                        return;
                    }
                }
                self.page = InstallerPage::Scheme;
            }
            InstallerPage::Scheme => {
                // Доступна только схема BIOS/MBR (UEFI — в следующем
                // обновлении), поэтому выбор здесь не хранится.
                self.page = InstallerPage::Size;
            }
            InstallerPage::Size => {
                let options = self.size_options_mib();
                if options.is_empty() {
                    self.note = Some(String::from("Disk has no room for /userdata."));
                    return;
                }
                if self.size_idx >= options.len() {
                    self.size_idx = options.len() - 1;
                }
                self.page = InstallerPage::Account;
            }
            InstallerPage::Account => {
                let u = self.username.trim();
                let user_ok = !u.is_empty()
                    && u.len() <= 32
                    && u.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
                if !user_ok {
                    self.note =
                        Some(String::from("Username: letters, digits, '_' or '-', up to 32."));
                    return;
                }
                if self.password.is_empty() {
                    self.note = Some(String::from("Password must not be empty."));
                    return;
                }
                if self.password != self.confirm {
                    self.note = Some(String::from("Passwords do not match."));
                    return;
                }
                self.page = InstallerPage::Confirm;
            }
            InstallerPage::Confirm => self.start_install(),
            InstallerPage::Progress
            | InstallerPage::Done
            | InstallerPage::Failed => {}
        }
    }

    fn start_install(&mut self) {
        let options = self.size_options_mib();
        let mib = options.get(self.size_idx).copied().unwrap_or(8);
        let drive = match self.disks.get(self.disk_idx) {
            Some(d) => d.drive,
            None => {
                self.note = Some(String::from("No target disk selected."));
                return;
            }
        };
        match crate::install::InstallJob::new(
            drive,
            mib * 2048,
            String::from(self.username.trim()),
            self.password.clone(),
        ) {
            Ok(job) => {
                self.job = Some(job);
                self.page = InstallerPage::Progress;
            }
            Err(e) => self.note = Some(e),
        }
    }
}

#[derive(Clone, Debug)]
pub struct BrowserTab {
    pub title: String,
    pub url: String,
    pub content: Vec<String>,
    pub scroll: usize,
    /// Абсолютные URL ссылок страницы: маркер `[N]` в тексте и строки
    /// секции «Links» указывают на links[N-1].
    pub links: Vec<String>,
    /// История навигации вкладки; hist_pos — текущая позиция
    /// (кнопки Back/Forward ходят по ней без повторной загрузки списка).
    pub history: Vec<String>,
    pub hist_pos: usize,
}

impl BrowserTab {
    /// Новая вкладка, сразу загружающая URL.
    pub fn open(url: &str) -> Self {
        let (content, links, title) = fetch_and_render_web_page(url);
        BrowserTab {
            title,
            url: String::from(url),
            content,
            scroll: 0,
            links,
            history: alloc::vec![String::from(url)],
            hist_pos: 0,
        }
    }

    /// Переход по URL: загрузка страницы + запись в историю.
    /// `push_history=false` — для Back/Forward/Reload (позиция в истории
    /// не меняется или уже изменена вызывающим).
    pub fn navigate(&mut self, url: &str, push_history: bool) {
        let (content, links, title) = fetch_and_render_web_page(url);
        self.url = String::from(url);
        self.title = title;
        self.content = content;
        self.links = links;
        self.scroll = 0;
        if push_history {
            // Отрезаем «вперёд»-хвост: новый переход начинает новую ветку.
            self.history.truncate(self.hist_pos + 1);
            self.history.push(String::from(url));
            self.hist_pos = self.history.len() - 1;
        }
    }

    /// Назад по истории. true — переход был.
    pub fn go_back(&mut self) -> bool {
        if self.hist_pos == 0 {
            return false;
        }
        self.hist_pos -= 1;
        let url = self.history[self.hist_pos].clone();
        self.navigate(&url, false);
        true
    }

    /// Вперёд по истории. true — переход был.
    pub fn go_forward(&mut self) -> bool {
        if self.hist_pos + 1 >= self.history.len() {
            return false;
        }
        self.hist_pos += 1;
        let url = self.history[self.hist_pos].clone();
        self.navigate(&url, false);
        true
    }
}

pub enum WindowContent {
    Terminal {
        lines: Vec<String>,
        current_line: String,
    },
    Files {
        current_partition: String,
        current_path: String,
        entries: Vec<FileViewEntry>,
        selected_idx: Option<usize>,
        search_query: String,
        error: Option<String>,
        status_msg: Option<String>,
    },
    FileEditor {
        filename: String,
        full_path: String,
        lines: Vec<String>,
        cursor_row: usize,
        cursor_col: usize,
        scroll: usize,
        modified: bool,
        read_only: bool,
        /// Открыт ли диалог подтверждения закрытия (Save/Discard/Cancel).
        /// Пока он открыт, ввод в текст не идёт, клики — только по кнопкам.
        confirm_close: bool,
        error: Option<String>,
        status_msg: Option<String>,
    },
    WebBrowser {
        tabs: Vec<BrowserTab>,
        active_tab: usize,
        address_input: String,
        bookmarks: Vec<String>,
        status_msg: Option<String>,
    },
    TaskManager {
        selected_pid: Option<usize>,
        status_msg: Option<String>,
    },
    ErrorCenter {
        /// Индекс выбранной записи в списке `error_center::visible_records`.
        selected: Option<usize>,
        /// Прокрутка списка (индекс первой видимой записи).
        scroll: usize,
    },
    ThemeSettings,
    About,
    DisplaySettings {
        /// Причина отказа последней попытки смены разрешения
        /// (например, нехватка памяти ядра) — показывается в окне.
        status: Option<String>,
    },
    /// Мастер установки DeiX на диск (Install DeiX).
    Installer {
        st: InstallerState,
    },
    /// Центр обновлений ОС (Updates).
    Updates {
        center: crate::update::center::Center,
    },
    /// Просмотрщик фото: библиотека (path пуст) или один файл.
    ImageViewer {
        path: String,
        library: Vec<String>,
        selected: usize,
        /// Декодированные пиксели 0xRRGGBB + размеры.
        img: Option<(Vec<u32>, usize, usize)>,
        error: Option<String>,
    },
    /// Видеоплеер: список файлов (list_mode) или воспроизведение.
    VideoPlayer {
        list: Vec<String>,
        idx: Option<usize>,
        list_mode: bool,
        status: Option<String>,
    },
    /// Плеер музыки: плейлист + транспорт.
    MusicPlayer {
        list: Vec<String>,
        idx: Option<usize>,
        status: Option<String>,
    },
}

impl WindowContent {
    /// Иконка приложения для Dock и Overview — единое соответствие
    /// «тип содержимого -> пиктограмма».
    pub fn icon(&self) -> crate::renderer::IconType {
        use crate::renderer::IconType;
        match self {
            WindowContent::Terminal { .. } => IconType::Terminal,
            WindowContent::Files { .. } | WindowContent::FileEditor { .. } => IconType::Files,
            WindowContent::WebBrowser { .. } => IconType::Browser,
            WindowContent::TaskManager { .. } => IconType::TaskManager,
            WindowContent::ErrorCenter { .. } => IconType::Settings,
            WindowContent::ThemeSettings => IconType::Theme,
            WindowContent::About => IconType::About,
            WindowContent::DisplaySettings { .. } => IconType::Display,
            WindowContent::Installer { .. } => IconType::Install,
            WindowContent::Updates { .. } => IconType::Reload,
            WindowContent::ImageViewer { .. } => IconType::Image,
            WindowContent::VideoPlayer { .. } => IconType::Video,
            WindowContent::MusicPlayer { .. } => IconType::Music,
        }
    }
}

pub struct Window {
    /// Стабильный идентификатор окна. Выдаётся Desktop'ом при открытии
    /// и не меняется до закрытия. Фокус, перетаскивание и изменение
    /// размера привязываются к id, а не к индексу в векторе: индексы
    /// сдвигаются при каждом изменении z-порядка (focus_window
    /// переставляет окно в конец) и при закрытии окон.
    pub id: u32,
    pub title: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub minimized: bool,
    pub maximized: bool,
    pub restore_geometry: (i32, i32, u32, u32),
    pub content: WindowContent,
}

/// Единая геометрия декораций окна. И отрисовка (`draw_window`), и
/// обработка мыши (`desktop.rs`) обязаны использовать эти прямоугольники:
/// два независимых источника координат уже приводили к несовпадению
/// видимой кнопки и её hitbox.
pub struct WindowFrame {
    /// Рамка окна целиком (заголовок + содержимое).
    pub outer: Rect,
    /// Строка заголовка.
    pub titlebar: Rect,
    /// Видимые кнопки (кликам даётся запас `BTN_HIT_PAD`).
    pub close: Rect,
    pub maximize: Rect,
    pub minimize: Rect,
    /// Часть заголовка, за которую окно перетаскивается (без кнопок).
    pub drag: Rect,
    /// Уголок изменения размера (справа внизу).
    pub resize: Rect,
    /// Область содержимого приложения.
    pub content: Rect,
}

impl WindowFrame {
    /// Запас hitbox вокруг видимой кнопки: 12 + 2*3 = 18 < шаг 20,
    /// зоны соседних кнопок не пересекаются.
    pub const BTN_HIT_PAD: i32 = 3;

    pub fn of(w: &Window, m: &UiMetrics) -> Self {
        let titlebar_h = m.titlebar_height;
        let d = m.button_diameter as u32;
        let total_h = w.height + titlebar_h as u32;
        let btn_y = w.y + (titlebar_h - m.button_diameter) / 2;
        let right = w.x + w.width as i32;

        // Зона drag заканчивается до кнопок (с их hit-запасом), чтобы
        // промах мимо Close не начинал перетаскивание.
        let drag_w = (w.width as i32 - 62 - Self::BTN_HIT_PAD).max(0) as u32;

        WindowFrame {
            outer: Rect::new(w.x, w.y, w.width, total_h),
            titlebar: Rect::new(w.x, w.y, w.width, titlebar_h as u32),
            close: Rect::new(right - 22, btn_y, d, d),
            maximize: Rect::new(right - 42, btn_y, d, d),
            minimize: Rect::new(right - 62, btn_y, d, d),
            drag: Rect::new(w.x, w.y, drag_w, titlebar_h as u32),
            resize: Rect::new(right - 16, w.y + total_h as i32 - 16, 16, 16),
            content: Rect::new(w.x, w.y + titlebar_h, w.width, w.height),
        }
    }
}

/// Минимальный размер содержимого окна. Один источник и для ручного
/// изменения размера, и для прижатия к экрану (clamp_window_geometry):
/// раньше у них были разные минимумы (220x140 против 160x80).
pub const MIN_WINDOW_W: u32 = 220;
pub const MIN_WINDOW_H: u32 = 140;

/// Геометрия развёрнутого окна: занимает рабочую область (между верхней
/// панелью и Dock). `height` — высота СОДЕРЖИМОГО, заголовок добавляется
/// сверху, поэтому он вычитается из высоты рабочей области.
pub fn maximized_geometry(screen_w: i32, screen_h: i32, m: &UiMetrics) -> (i32, i32, u32, u32) {
    let wa = crate::ui::taskbar::work_area(screen_w, screen_h);
    let h = (wa.h as i32 - m.titlebar_height).max(120);
    (wa.x, wa.y, wa.w.max(160), h as u32)
}

impl Window {
    pub fn new_terminal(x: i32, y: i32) -> Self {
        Window {
            id: 0,
            title: String::from("Terminal"),
            x,
            y,
            width: 380,
            height: 240,
            minimized: false,
            maximized: false,
            restore_geometry: (x, y, 380, 240),
            content: WindowContent::Terminal {
                lines: alloc::vec![
                    alloc::format!("DeiX Interactive Terminal (v{})", crate::KERNEL_VERSION),
                    String::from("Type 'help' for commands or 'taskmgr' for system monitor."),
                ],
                current_line: String::new(),
            },
        }
    }

    pub fn new_files(x: i32, y: i32) -> Self {
        let current_partition = String::from("/userdata");
        let current_path = String::from("/");
        let (entries, error) = load_partition_entries(&current_partition, &current_path);
        Window {
            id: 0,
            title: String::from("Files"),
            x,
            y,
            width: 520,
            height: 340,
            minimized: false,
            maximized: false,
            restore_geometry: (x, y, 520, 340),
            content: WindowContent::Files {
                current_partition,
                current_path,
                entries,
                selected_idx: None,
                search_query: String::new(),
                error,
                status_msg: None,
            },
        }
    }

    pub fn new_browser(x: i32, y: i32, initial_url: &str) -> Self {
        let url = if initial_url.is_empty() { "deix://home" } else { initial_url };
        let tab0 = BrowserTab::open(url);

        Window {
            id: 0,
            title: String::from("DeiX Web Browser"),
            x,
            y,
            width: 640,
            height: 420,
            minimized: false,
            maximized: false,
            restore_geometry: (x, y, 640, 420),
            content: WindowContent::WebBrowser {
                tabs: alloc::vec![tab0],
                active_tab: 0,
                address_input: String::from(url),
                bookmarks: alloc::vec![
                    String::from("https://example.com"),
                    String::from("deix://home"),
                    String::from("deix://docs"),
                    String::from("http://deix.os"),
                ],
                status_msg: Some(String::from("HTTP/HTTPS client ready (TLS 1.3, certificate verification on).")),
            },
        }
    }

    pub fn new_task_manager(x: i32, y: i32) -> Self {
        Window {
            id: 0,
            title: String::from("Task Manager"),
            x,
            y,
            width: 500,
            height: 320,
            minimized: false,
            maximized: false,
            restore_geometry: (x, y, 500, 320),
            content: WindowContent::TaskManager {
                selected_pid: None,
                status_msg: Some(String::from("System monitor active.")),
            },
        }
    }

    pub fn new_theme_settings(x: i32, y: i32) -> Self {
        Window {
            id: 0,
            title: String::from("Personalization"),
            x,
            y,
            width: 480,
            height: 460,
            minimized: false,
            maximized: false,
            restore_geometry: (x, y, 480, 460),
            content: WindowContent::ThemeSettings,
        }
    }

    pub fn new_error_center(x: i32, y: i32) -> Self {
        Window {
            id: 0,
            title: String::from("Error Center"),
            x,
            y,
            width: 520,
            height: 360,
            minimized: false,
            maximized: false,
            restore_geometry: (x, y, 520, 360),
            content: WindowContent::ErrorCenter {
                selected: None,
                scroll: 0,
            },
        }
    }

    pub fn new_about(x: i32, y: i32) -> Self {
        Window {
            id: 0,
            title: String::from("About DeiX OS"),
            x,
            y,
            width: 380,
            height: 220,
            minimized: false,
            maximized: false,
            restore_geometry: (x, y, 380, 220),
            content: WindowContent::About,
        }
    }

    pub fn new_installer(x: i32, y: i32) -> Self {
        Window {
            id: 0,
            title: String::from("Install DeiX"),
            x,
            y,
            width: 520,
            height: 360,
            minimized: false,
            maximized: false,
            restore_geometry: (x, y, 520, 360),
            content: WindowContent::Installer { st: InstallerState::new() },
        }
    }

    pub fn new_updates(x: i32, y: i32) -> Self {
        Window {
            id: 0,
            title: String::from("Updates"),
            x,
            y,
            width: 480,
            height: 340,
            minimized: false,
            maximized: false,
            restore_geometry: (x, y, 480, 340),
            content: WindowContent::Updates { center: crate::update::center::Center::new() },
        }
    }

    /// Просмотрщик фото в режиме библиотеки (список + превью).
    pub fn new_image_viewer(x: i32, y: i32) -> Self {
        Window {
            id: 0,
            title: String::from("Photos"),
            x,
            y,
            width: 560,
            height: 400,
            minimized: false,
            maximized: false,
            restore_geometry: (x, y, 560, 400),
            content: WindowContent::ImageViewer {
                path: String::new(),
                library: crate::ui::apps::media::collect_images(),
                selected: 0,
                img: None,
                error: None,
            },
        }
    }

    /// Просмотрщик фото с открытым файлом: декодирует сразу; ошибка
    /// декодирования показывается в окне честно.
    pub fn new_image_viewer_file(x: i32, y: i32, path: &str) -> Self {
        let name = crate::media::file_name(path);
        let (img, error) = match crate::ui::apps::media::decode_image_file(path) {
            Ok(v) => (Some(v), None),
            Err(e) => (None, Some(e)),
        };
        Window {
            id: 0,
            title: format!("Photo - {}", name),
            x,
            y,
            width: 520,
            height: 400,
            minimized: false,
            maximized: false,
            restore_geometry: (x, y, 520, 400),
            content: WindowContent::ImageViewer {
                path: String::from(path),
                library: Vec::new(),
                selected: 0,
                img,
                error,
            },
        }
    }

    /// Видеоплеер в режиме списка файлов.
    pub fn new_video_player(x: i32, y: i32) -> Self {
        Window {
            id: 0,
            title: String::from("Video"),
            x,
            y,
            width: 520,
            height: 380,
            minimized: false,
            maximized: false,
            restore_geometry: (x, y, 520, 380),
            content: WindowContent::VideoPlayer {
                list: crate::ui::apps::media::collect_videos(),
                idx: None,
                list_mode: true,
                status: None,
            },
        }
    }

    /// Видеоплеер с немедленным запуском файла.
    pub fn new_video_player_file(x: i32, y: i32, path: &str) -> Self {
        let name = crate::media::file_name(path);
        let status = match crate::media::play(path) {
            Ok(()) => None,
            Err(e) => Some(e),
        };
        Window {
            id: 0,
            title: format!("Video - {}", name),
            x,
            y,
            width: 520,
            height: 380,
            minimized: false,
            maximized: false,
            restore_geometry: (x, y, 520, 380),
            content: WindowContent::VideoPlayer {
                list: alloc::vec![String::from(path)],
                idx: Some(0),
                list_mode: false,
                status,
            },
        }
    }

    /// Плеер музыки: плейлист из медиапака и загрузок.
    pub fn new_music_player(x: i32, y: i32) -> Self {
        Window {
            id: 0,
            title: String::from("Music"),
            x,
            y,
            width: 430,
            height: 380,
            minimized: false,
            maximized: false,
            restore_geometry: (x, y, 430, 380),
            content: WindowContent::MusicPlayer {
                list: crate::ui::apps::media::collect_audio(),
                idx: None,
                status: None,
            },
        }
    }

    /// Плеер музыки с немедленным запуском файла: файл добавляется в
    /// плейлист первым, если его там нет.
    pub fn new_music_player_file(x: i32, y: i32, path: &str) -> Self {
        let name = crate::media::file_name(path);
        let mut list = crate::ui::apps::media::collect_audio();
        let idx = match list.iter().position(|p| p == path) {
            Some(i) => i,
            None => {
                list.insert(0, String::from(path));
                0
            }
        };
        let status = match crate::media::play(path) {
            Ok(()) => None,
            Err(e) => Some(e),
        };
        Window {
            id: 0,
            title: format!("Music - {}", name),
            x,
            y,
            width: 430,
            height: 380,
            minimized: false,
            maximized: false,
            restore_geometry: (x, y, 430, 380),
            content: WindowContent::MusicPlayer {
                list,
                idx: Some(idx),
                status,
            },
        }
    }

    /// Открывает редактор, читая файл через VFS. Ошибка чтения не рисует
    /// пустой документ как настоящий: редактор открывается только для
    /// чтения и показывает причину в строке состояния.
    pub fn new_file_editor(x: i32, y: i32, filename: &str, path: &str, read_only: bool) -> Self {
        let (lines, error, read_only) = match crate::vfs::read_file(path) {
            Ok(data) => {
                let text = String::from_utf8_lossy(&data);
                let mut lines: Vec<String> =
                    text.lines().map(String::from).collect();
                if lines.is_empty() {
                    lines.push(String::new());
                }
                (lines, None, read_only)
            }
            Err(e) => (alloc::vec![String::new()], Some(e.message()), true),
        };
        Window {
            id: 0,
            title: format!("Editor - {}", filename),
            x,
            y,
            width: 440,
            height: 300,
            minimized: false,
            maximized: false,
            restore_geometry: (x, y, 440, 300),
            content: WindowContent::FileEditor {
                filename: filename.to_string(),
                full_path: path.to_string(),
                lines,
                cursor_row: 0,
                cursor_col: 0,
                scroll: 0,
                modified: false,
                read_only,
                confirm_close: false,
                error,
                status_msg: if read_only {
                    Some(String::from("read-only"))
                } else {
                    None
                },
            },
        }
    }

    pub fn new_display_settings(x: i32, _y: i32, cur_h: u32) -> Self {
        let height = RESOLUTION_PRESETS.len() as u32 * 28 + 40;
        let y_pos = (cur_h as i32 - height as i32) / 2;
        Window {
            id: 0,
            title: String::from("Display Settings"),
            x,
            y: y_pos.max(20),
            width: 240,
            height,
            minimized: false,
            maximized: false,
            restore_geometry: (x, y_pos.max(20), 240, height),
            content: WindowContent::DisplaySettings { status: None },
        }
    }
}

/// Загружает URL и возвращает (строки страницы, ссылки, заголовок вкладки).
pub fn fetch_and_render_web_page(url: &str) -> (Vec<String>, Vec<String>, String) {
    let clean_url = url.trim();

    if clean_url == "deix://home" {
        return (alloc::vec![
            String::from("# DeiX Browser"),
            String::from("Limited text-oriented HTTP/HTTPS client."),
            format!("{} | build {}", crate::OS_RELEASE, crate::update::installed_build()),
            String::new(),
            String::from("## Quick Navigation (click a link):"),
            String::from("[1] https://example.com — HTTPS test page (TLS 1.3)"),
            String::from("[2] deix://docs — System Architecture & Manual"),
            String::from("[3] http://deix.os — System Status (local page)"),
            String::new(),
            String::from("## Honest limitations:"),
            String::from("- HTTPS: own TLS 1.3 client (X25519 + AES-128-GCM) with X.509 chain verification"),
            String::from("  (23 Mozilla root CAs, expiry via RTC, SAN hostname check; no OCSP/CRL revocation)"),
            String::from("- TLS 1.3 only: servers without it fail with an honest error"),
            String::from("- HTTP/1.1 with chunked transfer and gzip (DEFLATE) decompression; GET only, no keep-alive"),
            String::from("- Text rendering only: no CSS, JavaScript or images"),
            String::from("- DNS: real A-record lookups over UDP/53 (no DNSSEC, no IPv6)"),
            String::new(),
            String::from("## Wallpapers from the internet:"),
            String::from("Open a direct PNG/BMP image URL here (or run `wallpaper <url>` in the CLI)"),
            String::from("- the image is downloaded, scaled to the ext2 file limit and set as wallpaper."),
        ],
        alloc::vec![
            String::from("https://example.com"),
            String::from("deix://docs"),
            String::from("http://deix.os"),
        ],
        String::from("DeiX Browser"));
    } else if clean_url == "deix://docs" {
        return (alloc::vec![
            alloc::format!("# {} Documentation", crate::OS_RELEASE),
            String::from("Kernel Specs & Subsystem Guide"),
            String::new(),
            String::from("### 1. Preemptive Scheduler"),
            String::from("Round-robin context switching via PIT 100Hz interrupt."),
            String::new(),
            String::from("### 2. Kernel Module GUI Compositor"),
            String::from("Built-in compositor manages window layers, VBE double buffer & FPS."),
            String::new(),
            String::from("### 3. Network Stack"),
            String::from("RTL8139 Ethernet -> ARP / IPv4 -> ICMP + UDP (DNS) + TCP -> HTTP/1.1 + TLS 1.3 (https)."),
            String::from("Hostnames are resolved with a real DNS client (A records, UDP/53)."),
            String::from("TLS 1.3: X25519 + HKDF-SHA256 + AES-128-GCM; X.509 chains verified against 23 Mozilla roots."),
            String::from("HTTP/1.1: chunked transfer decoding + gzip (RFC 1951/1952) decompression."),
        ], Vec::new(), String::from("DeiX Docs"));
    } else if clean_url == "http://deix.os" {
        // Локальная страница с НАСТОЯЩИМИ значениями подсистем — никакой
        // выдуманной телеметрии.
        let heap = crate::allocator::stats();
        let ip = crate::net::my_ip();
        let gw = crate::net::gateway_ip();
        let dns = crate::net::dns_ip();
        let cpu = crate::cpuid::brand_string()
            .unwrap_or_else(|| crate::cpuid::vendor_string());
        return (alloc::vec![
            String::from("# DeiX OS Status (local page)"),
            alloc::format!("{} | Ring 0 Long Mode", crate::OS_RELEASE),
            String::new(),
            alloc::format!("CPU: {}", cpu.trim()),
            alloc::format!(
                "Heap: {} KiB used / {} KiB total",
                heap.used / 1024,
                heap.total / 1024
            ),
            alloc::format!(
                "Network: eth0 {}.{}.{}.{} (gw {}.{}.{}.{}, dns {}.{}.{}.{})",
                ip[0], ip[1], ip[2], ip[3], gw[0], gw[1], gw[2], gw[3],
                dns[0], dns[1], dns[2], dns[3]
            ),
        ], Vec::new(), String::from("DeiX Status"));
    }

    let full_url = if !clean_url.starts_with("http://") && !clean_url.starts_with("https://") && !clean_url.starts_with("deix://") {
        format!("http://{}", clean_url)
    } else {
        clean_url.to_string()
    };

    match crate::net::http::fetch_bytes(&full_url) {
        Ok(resp) => {
            if looks_binary(&resp) {
                render_download(&resp)
            } else {
                let (lines, links) = parse_html_to_browser_lines(&resp);
                let title = page_tab_title(&lines, &resp.final_url);
                (lines, links, title)
            }
        }
        // Показываем настоящую причину сбоя из net::http. Подменять её
        // «успешной» страницей нельзя: иначе сеть не диагностируется.
        Err(err) => (alloc::vec![
            format!("# Failed to load {}", full_url),
            String::new(),
            format!("Error: {}", err),
            String::new(),
            String::from("Possible causes:"),
            String::from("* network card not initialized (no eth0)"),
            String::from("* DNS server unreachable or the name does not exist"),
            String::from("* host unreachable or port closed"),
            String::from("* server does not support TLS 1.3 (older HTTPS sites will not load)"),
            String::from("* server certificate failed verification (expiry, hostname, CA chain)"),
            String::new(),
            network_summary_line(),
            String::from("Check in the CLI: `ifconfig`, `dns <host>`, `ping <ip>`."),
        ], Vec::new(), String::from("Load failed")),
    }
}

/// Строка с текущей сетевой конфигурацией для страницы ошибки загрузки.
fn network_summary_line() -> String {
    let ip = crate::net::my_ip();
    let gw = crate::net::gateway_ip();
    let dns = crate::net::dns_ip();
    format!(
        "Network now: eth0 {}.{}.{}.{} gw {}.{}.{}.{} dns {}.{}.{}.{}",
        ip[0], ip[1], ip[2], ip[3], gw[0], gw[1], gw[2], gw[3], dns[0], dns[1], dns[2], dns[3]
    )
}

/// Бинарный ли ответ: смотрим Content-Type, сигнатуры картинок и долю
/// нулевых байтов (сервера без Content-Type встречаются).
fn looks_binary(resp: &crate::net::http::HttpBytes) -> bool {
    let ct = resp.content_type.to_ascii_lowercase();
    if ct.starts_with("image/") || ct.starts_with("application/octet-stream") {
        return true;
    }
    if ct.starts_with("text/")
        || ct.contains("html")
        || ct.contains("json")
        || ct.contains("xml")
        || ct.contains("javascript")
    {
        return false;
    }
    if crate::png::is_png(&resp.body) || resp.body.starts_with(b"BM") {
        return true;
    }
    let probe = &resp.body[..resp.body.len().min(512)];
    !probe.is_empty() && probe.iter().filter(|&&b| b == 0).count() * 20 > probe.len()
}

/// Бинарный ответ: PNG/BMP сразу ставится обоями (это и есть «скачать
/// обои из интернета»), прочее сохраняется в /userdata/downloads.
fn render_download(resp: &crate::net::http::HttpBytes) -> (Vec<String>, Vec<String>, String) {
    let mut lines = alloc::vec![
        format!("# Download: {}", resp.final_url),
        format!(
            "Type: {} | Size: {} KiB",
            if resp.content_type.is_empty() { "unknown" } else { &resp.content_type },
            resp.body.len() / 1024
        ),
        String::new(),
    ];
    let is_image = crate::png::is_png(&resp.body) || resp.body.starts_with(b"BM");
    if is_image {
        match crate::ui::wallpaper::set_from_bytes(&resp.body) {
            Ok(msg) => {
                lines.push(format!("## {}", msg));
                lines.push(String::from("The new wallpaper is already on the desktop."));
            }
            Err(e) => lines.push(format!("## Wallpaper not set: {}", e)),
        }
    }
    let name = filename_from_url(&resp.final_url);
    let path = format!("/userdata/downloads/{}", name);
    match crate::vfs::write_file(&path, &resp.body) {
        Ok(()) => lines.push(format!("Saved: {}", path)),
        Err(e) => lines.push(format!("Not saved to downloads: {}", e.message())),
    }
    if !is_image {
        lines.push(String::new());
        lines.push(String::from("JPEG/GIF/WebP are not decodable here: only PNG (8-bit) and BMP 24/32."));
    }
    (lines, Vec::new(), String::from("Download"))
}

/// Имя файла из URL: последний сегмент пути без query, только
/// безопасные символы; пустое → download.bin.
fn filename_from_url(url: &str) -> String {
    let no_scheme = url.split("://").nth(1).unwrap_or(url);
    let path = no_scheme.split(&['?', '#'][..]).next().unwrap_or("");
    let seg = path.rsplit('/').next().unwrap_or("");
    let clean: String = seg
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .take(48)
        .collect();
    if clean.is_empty() || clean == "." || clean == ".." {
        String::from("download.bin")
    } else {
        clean
    }
}

/// Заголовок вкладки: первая `## ...` строка (титул страницы) или хост URL.
fn page_tab_title(lines: &[String], url: &str) -> String {
    for l in lines.iter().take(8) {
        if let Some(t) = l.strip_prefix("## ") {
            if !t.trim().is_empty() {
                return String::from(t.trim());
            }
        }
    }
    let host = url.split("://").nth(1).unwrap_or(url);
    String::from(host.split('/').next().unwrap_or(host))
}

/// Разбирает HTML в строки страницы + список абсолютных URL ссылок.
/// Маркеры `[N]` в тексте и строки секции «Links on this page» ведут на
/// links[N-1] — обработчик кликов в desktop.rs парсит этот префикс.
pub fn parse_html_to_browser_lines(
    resp: &crate::net::http::HttpBytes,
) -> (Vec<String>, Vec<String>) {
    let html = String::from_utf8_lossy(&resp.body).into_owned();
    let mut links: Vec<String> = Vec::new();
    let mut lines = Vec::new();
    lines.push(format!("# Web Page: {}", resp.final_url));
    // Настоящая строка статуса из ответа сервера, а не вечное «200 OK».
    if resp.status == 0 {
        lines.push(String::from("Status: unknown (no HTTP status line in reply)"));
    } else {
        lines.push(format!("Status: {} {}", resp.status, resp.reason));
    }
    if resp.final_url.starts_with("https://") {
        lines.push(String::from(
            "TLS 1.3: encrypted, certificate chain verified (23 Mozilla roots; no OCSP/CRL)",
        ));
    }
    lines.push(String::new());

    let cs: Vec<char> = html.chars().collect();

    if let Some(title) = extract_title(&cs) {
        if !title.is_empty() {
            lines.push(format!("## {}", title));
            lines.push(String::new());
        }
    }

    // Проход по документу: содержимое <script>/<style> и комментарии
    // пропускаются (иначе минифицированный JS/CSS высыпается на экран
    // «разбросанными буквами»), блочные теги дают перенос строки,
    // HTML-сущности декодируются.
    let mut text = String::new();
    let mut i = 0usize;
    while i < cs.len() {
        if cs[i] == '<' {
            if starts_with_ci(&cs, i, "<!--") {
                i = find_ci(&cs, i, "-->").map(|p| p + 3).unwrap_or(cs.len());
                continue;
            }
            if starts_with_ci(&cs, i, "<script") {
                i = find_ci(&cs, i, "</script").unwrap_or(cs.len());
                i = skip_past_gt(&cs, i);
                text.push('\n');
                continue;
            }
            if starts_with_ci(&cs, i, "<style") {
                i = find_ci(&cs, i, "</style").unwrap_or(cs.len());
                i = skip_past_gt(&cs, i);
                text.push('\n');
                continue;
            }
            let tag_end = skip_past_gt(&cs, i);
            let name = tag_name(&cs, i);
            if name == "li" {
                text.push_str("\n* ");
            } else if is_block_tag(&name) {
                text.push('\n');
            } else if name == "a" && cs.get(i + 1) != Some(&'/') && links.len() < MAX_PAGE_LINKS {
                // Ссылка: href абсолютизируется относительно финального
                // URL, в текст вставляется маркер [N] перед якорем.
                if let Some(href) = extract_href(&cs, i, tag_end) {
                    let h = href.trim();
                    let skip = h.is_empty()
                        || h.starts_with('#')
                        || h.starts_with("javascript:")
                        || h.starts_with("mailto:")
                        || h.starts_with("data:");
                    if !skip {
                        let abs = if h.starts_with("deix://") {
                            String::from(h)
                        } else {
                            crate::net::http::absolutize(h, &resp.final_url)
                        };
                        links.push(abs);
                        text.push_str(&format!(" [{}]", links.len()));
                    }
                }
            }
            i = tag_end;
        } else if cs[i] == '&' {
            let (ch, next) = decode_entity(&cs, i);
            text.push(ch);
            i = next;
        } else {
            let c = cs[i];
            if c == '\r' {
                // игнорируем, перенос даст \n
            } else if c == '\n' || c == '\t' {
                text.push(' ');
            } else {
                text.push(c);
            }
            i += 1;
        }
    }

    // Сборка строк: схлопывание пробелов, перенос по словам на 72
    // символа, слова длиннее лимита режутся жёстко (длинные URL).
    const WRAP: usize = 72;
    let mut blank_pending = false;
    for para in text.split('\n') {
        if para.split_whitespace().next().is_none() {
            // пустой абзац: максимум одна пустая строка подряд
            blank_pending = true;
            continue;
        }
        if blank_pending && !lines.is_empty() {
            lines.push(String::new());
        }
        blank_pending = false;
        let mut line = String::new();
        for word in para.split_whitespace() {
            let mut rest = word;
            loop {
                if line.is_empty() {
                    if rest.chars().count() <= WRAP {
                        line.push_str(rest);
                        break;
                    }
                    // жёсткий разрез сверхдлинного слова по границе символа
                    let cut = crate::ui::text::char_byte_index(rest, WRAP);
                    line.push_str(&rest[..cut]);
                    lines.push(core::mem::take(&mut line));
                    rest = &rest[cut..];
                } else if line.chars().count() + 1 + rest.chars().count() <= WRAP {
                    line.push(' ');
                    line.push_str(rest);
                    break;
                } else {
                    lines.push(core::mem::take(&mut line));
                }
            }
        }
        if !line.is_empty() {
            lines.push(line);
        }
    }

    if lines.len() <= 3 {
        lines.push(String::from("HTML Body empty or binary content."));
    }

    // Секция переходов: кликабельные строки `[N] url`.
    if !links.is_empty() {
        lines.push(String::new());
        lines.push(String::from("## Links on this page (click to open):"));
        for (n, l) in links.iter().enumerate() {
            lines.push(format!("[{}] {}", n + 1, l));
        }
    }

    (lines, links)
}

/// Максимум ссылок со страницы: защищает кучу от каталогов-портянок.
const MAX_PAGE_LINKS: usize = 64;

/// Значение атрибута href внутри тега cs[start..end]: в кавычках
/// (одинарных/двойных) или до первого пробела/'>'.
fn extract_href(cs: &[char], start: usize, end: usize) -> Option<String> {
    let mut i = start;
    while i + 4 < end.min(cs.len()) {
        let is_href = (cs[i] == 'h' || cs[i] == 'H')
            && cs[i + 1].to_ascii_lowercase() == 'r'
            && cs[i + 2].to_ascii_lowercase() == 'e'
            && cs[i + 3].to_ascii_lowercase() == 'f'
            // слева — разделитель, а не часть другого атрибута
            && !cs[i - 1].is_ascii_alphanumeric();
        if !is_href {
            i += 1;
            continue;
        }
        let mut j = i + 4;
        while j < end && cs[j].is_whitespace() {
            j += 1;
        }
        if j >= end || cs[j] != '=' {
            i += 1;
            continue;
        }
        j += 1;
        while j < end && cs[j].is_whitespace() {
            j += 1;
        }
        if j >= end {
            return None;
        }
        let mut val = String::new();
        if cs[j] == '"' || cs[j] == '\'' {
            let q = cs[j];
            j += 1;
            while j < end && cs[j] != q && val.len() < 512 {
                val.push(cs[j]);
                j += 1;
            }
        } else {
            while j < end && !cs[j].is_whitespace() && cs[j] != '>' && val.len() < 512 {
                val.push(cs[j]);
                j += 1;
            }
        }
        return Some(val);
    }
    None
}

/// Регистронезависимое (ASCII) сравнение с образцом на позиции pos.
fn starts_with_ci(cs: &[char], pos: usize, pat: &str) -> bool {
    let pc: Vec<char> = pat.chars().collect();
    if pos + pc.len() > cs.len() {
        return false;
    }
    pc.iter()
        .enumerate()
        .all(|(k, p)| cs[pos + k].to_ascii_lowercase() == *p)
}

/// Поиск образца (ASCII, без регистра) начиная с from. Возвращает позицию.
fn find_ci(cs: &[char], from: usize, pat: &str) -> Option<usize> {
    let pc: Vec<char> = pat.chars().collect();
    if pc.is_empty() || cs.len() < pc.len() {
        return None;
    }
    (from..=cs.len() - pc.len()).find(|&p| {
        pc.iter()
            .enumerate()
            .all(|(k, q)| cs[p + k].to_ascii_lowercase() == *q)
    })
}

/// Позиция сразу за ближайшим '>' (конец тега).
fn skip_past_gt(cs: &[char], from: usize) -> usize {
    let mut i = from;
    while i < cs.len() {
        if cs[i] == '>' {
            return i + 1;
        }
        i += 1;
    }
    cs.len()
}

/// Имя тега в нижнем регистре: "</div class..>" -> "div".
fn tag_name(cs: &[char], pos: usize) -> String {
    let mut i = pos + 1;
    if i < cs.len() && cs[i] == '/' {
        i += 1;
    }
    let mut name = String::new();
    while i < cs.len() && cs[i].is_ascii_alphanumeric() && name.len() < 12 {
        name.push(cs[i].to_ascii_lowercase());
        i += 1;
    }
    name
}

/// Блочные теги, дающие перенос строки.
fn is_block_tag(name: &str) -> bool {
    matches!(
        name,
        "p" | "div" | "br" | "tr" | "ul" | "ol" | "table" | "section" | "article"
            | "header" | "footer" | "nav" | "form" | "blockquote" | "pre" | "hr"
            | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "title" | "head" | "body"
    )
}

/// Содержимое <title>...</title>, если есть.
fn extract_title(cs: &[char]) -> Option<String> {
    let open = find_ci(cs, 0, "<title")?;
    let start = skip_past_gt(cs, open);
    let close = find_ci(cs, start, "</title")?;
    let mut t = String::new();
    let mut i = start;
    while i < close {
        if cs[i] == '&' {
            let (ch, next) = decode_entity(cs, i);
            t.push(ch);
            i = next;
        } else {
            t.push(cs[i]);
            i += 1;
        }
    }
    let trimmed: String = t.split_whitespace().collect::<Vec<_>>().join(" ");
    if trimmed.len() > 120 {
        None
    } else {
        Some(trimmed)
    }
}

/// Декодирует HTML-сущность на позиции pos ('&'): возвращает символ и
/// позицию за сущностью. Нераспознанное — просто '&', позиция +1.
fn decode_entity(cs: &[char], pos: usize) -> (char, usize) {
    // ищем ';' в пределах 10 символов
    let mut end = pos + 1;
    while end < cs.len() && end - pos <= 10 && cs[end] != ';' {
        end += 1;
    }
    if end >= cs.len() || cs[end] != ';' {
        return ('&', pos + 1);
    }
    let name: String = cs[pos + 1..end].iter().collect();
    let decoded = match name.as_str() {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" | "#39" => Some('\''),
        "nbsp" => Some(' '),
        "copy" => Some('©'),
        "reg" => Some('®'),
        "deg" => Some('°'),
        "ndash" | "mdash" => Some('-'),
        "laquo" => Some('«'),
        "raquo" => Some('»'),
        "hellip" => Some('…'),
        _ => {
            if let Some(num) = name.strip_prefix("#x").or_else(|| name.strip_prefix("#X")) {
                u32::from_str_radix(num, 16).ok().and_then(char::from_u32)
            } else if let Some(num) = name.strip_prefix('#') {
                num.parse::<u32>().ok().and_then(char::from_u32)
            } else {
                None
            }
        }
    };
    match decoded {
        Some(c) => (c, end + 1),
        None => ('&', pos + 1),
    }
}

pub fn load_partition_entries(partition: &str, path: &str) -> (Vec<FileViewEntry>, Option<String>) {
    match partition {
        "/userdata" => {
            if !ext2::is_formatted() {
                let _ = ext2::format();
                let _ = ext2::write_file(
                    "README.TXT",
                    alloc::format!("Welcome to {}!\r\nEdit files directly in this window.\r\n", crate::OS_RELEASE).as_bytes(),
                );
            }
            match ext2::list_dir_path(path) {
                Ok(entries) => {
                    let mut items = Vec::new();
                    for e in entries {
                        items.push(FileViewEntry {
                            name: e.name,
                            is_dir: e.is_directory,
                            size: e.size,
                        });
                    }
                    (items, None)
                }
                Err(_) => (Vec::new(), Some(String::from("Failed to list directory"))),
            }
        }
        "/system" => {
            let full = if path == "/" || path.is_empty() {
                String::from("/system")
            } else {
                format!("/system{}", path)
            };
            match crate::vfs::readdir(&full) {
                Ok(entries) => {
                    let mut items = Vec::new();
                    for e in entries {
                        items.push(FileViewEntry {
                            name: e.name,
                            is_dir: e.is_dir,
                            size: e.size.min(u32::MAX as u64) as u32,
                        });
                    }
                    (items, None)
                }
                Err(e) => (Vec::new(), Some(format!("/system: {}", e.message()))),
            }
        }
        _ => (Vec::new(), Some(String::from("Unknown partition"))),
    }
}

pub fn draw_window(
    r: &mut Renderer,
    theme: &UiTheme,
    w: &Window,
    is_focused: bool,
    is_dragging: bool,
) {
    let ui_m = UiMetrics::fluent();
    let titlebar_h = ui_m.titlebar_height;
    let button_d = ui_m.button_diameter;
    let total_h = w.height + titlebar_h as u32;
    let frame = WindowFrame::of(w, &ui_m);

    // Профиль производительности реально меняет путь отрисовки:
    // Balanced убирает blur, Performance — ещё и тени (см. ui::effects).
    let fx = crate::ui::effects::current();
    if theme.enable_blur && fx.blur_enabled() && !is_dragging {
        r.apply_blur_rect(w.x - 4, w.y - 4, w.width + 8, total_h + 8, 2);
    }

    if fx.shadows_enabled() {
        r.draw_soft_shadow(w.x, w.y, w.width, total_h, 8, 3, 70);
    }
    if !is_focused {
        r.shade_rect(w.x, w.y, w.width, total_h, 15);
    }

    let title_bg = if is_focused { theme.titlebar_active } else { theme.titlebar_inactive };

    r.fill_rounded_rect_alpha(
        w.x,
        w.y,
        w.width,
        titlebar_h as u32 + theme.corner_radius as u32,
        theme.corner_radius,
        title_bg,
        theme.opacity,
    );

    let inset = theme.corner_radius;
    let sheen_w = (w.width as i32 - 2 * inset).max(0) as u32;
    r.fill_fade_v(w.x + inset, w.y, sheen_w, titlebar_h as u32, Color::WHITE, 26, 0);
    r.fill_rect_alpha(w.x + inset, w.y, sheen_w, 1, Color::WHITE, 40);
    if is_focused {
        r.fill_rect_alpha(w.x, w.y + titlebar_h - 1, w.width, 1, theme.accent, 190);
    } else {
        r.fill_rect_alpha(w.x, w.y + titlebar_h - 1, w.width, 1, Color::BLACK, 60);
    }

    let title_color = if is_focused { theme.text_primary } else { theme.text_secondary };
    // Длинный заголовок (имя файла в редакторе) не должен заезжать под
    // кнопки: слева отступ 12, справа зона кнопок начинается на width-62.
    let max_title_cells = ((w.width as i32 - 80) / 8).max(4) as usize;
    let title = crate::ui::text::truncate_to_cells(&w.title, max_title_cells).0;
    r.draw_text(w.x + 12, w.y + 6, title, title_color, None);

    // Кнопки — строго по прямоугольникам WindowFrame (теми же
    // пользуется обработчик кликов в desktop.rs).
    let rad = button_d / 2;
    r.fill_circle(frame.minimize.x + rad, frame.minimize.y + rad, rad, Color::rgb(234, 179, 8));
    r.draw_hline(frame.minimize.x + 3, frame.minimize.y + rad, button_d as u32 - 6, Color::rgb(120, 80, 0));
    r.fill_circle(frame.maximize.x + rad, frame.maximize.y + rad, rad, Color::GREEN);
    r.fill_circle(frame.close.x + rad, frame.close.y + rad, rad, Color::RED);

    let content_y = w.y + titlebar_h;

    match &w.content {
        WindowContent::Terminal { lines, current_line } => {
            apps::draw_terminal(r, theme, w, lines, current_line, content_y);
        }
        WindowContent::Files {
            current_partition,
            current_path,
            entries,
            selected_idx,
            search_query,
            error,
            status_msg,
        } => {
            apps::draw_files(
                r,
                theme,
                w,
                current_partition,
                current_path,
                entries,
                *selected_idx,
                search_query,
                error.as_ref(),
                status_msg.as_ref(),
                content_y,
            );
        }
        WindowContent::FileEditor {
            filename,
            full_path: _,
            lines,
            cursor_row,
            cursor_col,
            scroll,
            modified,
            read_only,
            confirm_close,
            error,
            status_msg,
        } => {
            apps::draw_file_editor(
                r,
                theme,
                w,
                filename,
                lines,
                *cursor_row,
                *cursor_col,
                *scroll,
                *modified,
                *read_only,
                *confirm_close,
                error.as_ref(),
                status_msg.as_ref(),
                content_y,
            );
        }
        WindowContent::WebBrowser {
            tabs,
            active_tab,
            address_input,
            bookmarks,
            status_msg,
        } => {
            apps::draw_browser(
                r,
                theme,
                w,
                tabs,
                *active_tab,
                address_input,
                bookmarks,
                status_msg.as_ref(),
                content_y,
            );
        }
        WindowContent::TaskManager {
            selected_pid,
            status_msg,
        } => {
            apps::draw_task_manager(
                r,
                theme,
                w,
                *selected_pid,
                status_msg.as_ref(),
                content_y,
            );
        }
        WindowContent::ThemeSettings => {
            apps::draw_theme_settings(r, theme, w, content_y);
        }
        WindowContent::ErrorCenter { selected, scroll } => {
            apps::draw_error_center(r, theme, w, *selected, *scroll, content_y);
        }
        WindowContent::About => {
            apps::draw_about(r, theme, w, content_y);
        }
        WindowContent::DisplaySettings { status } => {
            apps::draw_display_settings(r, theme, w, content_y, status.as_deref());
        }
        WindowContent::Installer { st } => {
            apps::draw_installer(r, theme, w, st, content_y);
        }
        WindowContent::Updates { center } => {
            apps::draw_updates(r, theme, w, center, content_y);
        }
        WindowContent::ImageViewer { path, library, selected, img, error } => {
            apps::media::draw_image_viewer(
                r, theme, w, path, library, *selected, img, error.as_ref(), content_y,
            );
        }
        WindowContent::VideoPlayer { list, idx, list_mode, status } => {
            apps::media::draw_video_player(
                r, theme, w, list, *idx, *list_mode, status.as_ref(), content_y,
            );
        }
        WindowContent::MusicPlayer { list, idx, status } => {
            apps::media::draw_music_player(r, theme, w, list, *idx, status.as_ref(), content_y);
        }
    }
}
