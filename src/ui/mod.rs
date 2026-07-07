//! UI-драйвер: оконный менеджер поверх программного 2D-рендерера
//! (renderer.rs) с настоящим интерактивным циклом отрисовки — рабочий
//! стол, перетаскиваемые окна с заголовком/кнопками, курсор мыши в
//! реальном времени, кнопка "Пуск" со стартовым меню, панель задач с
//! часами и списком открытых окон.
//!
//! ВАЖНО про фон рабочего стола: он НЕ является картинкой (ни PNG, ни
//! bitmap-ресурсом, зашитым в бинарник) — это чистая процедурная графика,
//! вычисляемая на лету каждый кадр через renderer.rs (вертикальный
//! градиент + узор из диагональных полос, см. draw_wallpaper ниже).
//! У нас нет декодера PNG/JPEG (это отдельный большой кусок работы —
//! честно не делаем вид, что он есть), поэтому "не PNG-картинка" здесь
//! реализовано буквально через математику (градиент, синусоида смещения
//! волны, диагональные линии), а не подменой на другой формат файла.
//!
//! Работает полностью в программном режиме (без GPU-акселерации — её и
//! не может быть без проприетарного драйвера конкретного чипа, см.
//! gpu.rs), поэтому каждый кадр перерисовывается целиком. На разрешениях
//! вроде 800x600/1024x768 в QEMU этого достаточно для отзывчивого
//! интерфейса при частоте кадров, привязанной к таймеру (~30-60 FPS).

use crate::ext2;
use crate::keyboard;
use crate::mouse;
use crate::renderer::{Color, Renderer};
use crate::timer;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

const TITLEBAR_HEIGHT: i32 = 26;
const BUTTON_DIAMETER: i32 = 14;
const TASKBAR_HEIGHT: u32 = 36;
const START_BUTTON_WIDTH: i32 = 78;
const WINDOW_CORNER_RADIUS: i32 = 6;
const TASKBAR_ITEM_WIDTH: i32 = 150;

/// Содержимое окна определяет, как оно рисуется и реагирует на ввод.
pub enum WindowContent {
    /// Окно терминала: выполняет РЕАЛЬНЫЙ полный CLI (cli::execute) —
    /// см. run_mini_terminal_command ниже.
    Terminal {
        lines: Vec<String>,
        current_line: String,
    },
    /// Окно файлового менеджера: показывает содержимое корневого каталога
    /// ext2-тома (см. ext2.rs). Клик по строке файла открывает его в
    /// TextViewer (для .TXT) или пытается запустить как .MEX программу
    /// через crate::mex.
    Files {
        entries: Vec<ext2::FileEntry>,
        error: Option<String>,
    },
    /// Простой текстовый просмотрщик (TXT reader) — показывает
    /// содержимое одного файла с прокруткой (стрелки вверх/вниз).
    /// Открывается кликом на файл в окне Files.
    TextViewer {
        filename: String,
        lines: Vec<String>,
        scroll: usize,
        error: Option<String>,
    },
    /// Окно "О системе" — статичная информационная панель, открывается
    /// из меню "Пуск". Отдельный вариант (а не TextViewer с фейковым
    /// файлом), потому что содержимое собирается динамически (uptime,
    /// версия) при каждой отрисовке, а не читается с диска один раз.
    About,
    /// Окно выбора разрешения экрана — список пресетов, клик по строке
    /// запрашивает у Desktop смену видеорежима "на лету" (см.
    /// DesktopExit::ChangeResolution ниже). Открывается из меню "Пуск"
    /// ("Display settings").
    DisplaySettings,
}

/// Готовые разрешения, предлагаемые в окне "Display settings" — все
/// значения совместимы с Bochs VBE (см. vbe.rs), которая принимает
/// произвольные width/height, но эти конкретные пресеты являются
/// стандартными VESA-разрешениями, которые гарантированно поддерживает
/// QEMU/Bochs без искажений соотношения сторон.
pub const RESOLUTION_PRESETS: [(u32, u32); 5] = [
    (640, 480),
    (800, 600),
    (1024, 768),
    (1280, 720),
    (1280, 1024),
];

/// Результат выхода из интерактивного цикла рабочего стола (см.
/// Desktop::run_event_loop) — либо пользователь нажал Esc (настоящий
/// выход обратно в текстовый CLI), либо выбрал новое разрешение в окне
/// "Display settings" (тогда вызывающий код в cli.rs должен переключить
/// видеорежим и снова запустить run_event_loop, не выходя в текст).
pub enum DesktopExit {
    Quit,
    ChangeResolution(u32, u32),
}

pub struct Window {
    pub title: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub minimized: bool,
    /// true, если окно сейчас развёрнуто на весь экран (см.
    /// title_button_center(w, 2) — третья, зелёная кнопка). Пока окно
    /// развёрнуто, `x/y/width/height` временно перезаписаны размерами
    /// экрана, а исходная геометрия сохраняется в `restore_geometry`,
    /// чтобы вернуть окно на место при повторном клике/двойном клике.
    pub maximized: bool,
    restore_geometry: (i32, i32, u32, u32),
    pub content: WindowContent,
}

impl Window {
    fn new_terminal(x: i32, y: i32) -> Self {
        Window {
            title: String::from("Terminal"),
            x,
            y,
            width: 360,
            height: 230,
            minimized: false,
            maximized: false,
            restore_geometry: (x, y, 360, 230),
            content: WindowContent::Terminal {
                lines: alloc::vec![String::from("DeiX Terminal (full CLI). Type 'help'.")],
                current_line: String::new(),
            },
        }
    }

    fn new_files(x: i32, y: i32) -> Self {
        let (entries, error) = load_file_list();
        Window {
            title: String::from("Files"),
            x,
            y,
            width: 300,
            height: 220,
            minimized: false,
            maximized: false,
            restore_geometry: (x, y, 300, 220),
            content: WindowContent::Files { entries, error },
        }
    }

    fn new_about(x: i32, y: i32) -> Self {
        Window {
            title: String::from("About DeiX"),
            x,
            y,
            width: 300,
            height: 170,
            minimized: false,
            maximized: false,
            restore_geometry: (x, y, 300, 170),
            content: WindowContent::About,
        }
    }

    fn new_display_settings(x: i32, y: i32) -> Self {
        let height = 40 + RESOLUTION_PRESETS.len() as u32 * 28;
        Window {
            title: String::from("Display settings"),
            x,
            y,
            width: 220,
            height,
            minimized: false,
            maximized: false,
            restore_geometry: (x, y, 220, height),
            content: WindowContent::DisplaySettings,
        }
    }

    /// Открывает файл в TextViewer. Если это .MEX программа — вместо
    /// показа "текста" (бинарного мусора) пытается её запустить и
    /// показывает результат выполнения в самом окне (как маленький
    /// лог), это гораздо полезнее для пользователя, кликающего на
    /// программу в файловом менеджере, чем нечитаемые байты.
    fn new_text_viewer(x: i32, y: i32, filename: &str) -> Self {
        if crate::mex::is_mex_filename(filename) {
            crate::vgaglobal::begin_capture();
            crate::cli::IN_GRAPHICAL_TERMINAL.store(true, core::sync::atomic::Ordering::Relaxed);
            crate::mex::run(filename, "");
            crate::cli::IN_GRAPHICAL_TERMINAL.store(false, core::sync::atomic::Ordering::Relaxed);
            let output = crate::vgaglobal::end_capture();
            let lines: Vec<String> = output.lines().map(String::from).collect();
            return Window {
                title: format!("Run: {}", filename),
                x,
                y,
                width: 420,
                height: 260,
                minimized: false,
                maximized: false,
                restore_geometry: (x, y, 420, 260),
                content: WindowContent::TextViewer {
                    filename: String::from(filename),
                    lines,
                    scroll: 0,
                    error: None,
                },
            };
        }

        let (lines, error) = match ext2::read_file(filename) {
            Ok(data) => match core::str::from_utf8(&data) {
                Ok(text) => (text.lines().map(String::from).collect(), None),
                Err(_) => (
                    Vec::new(),
                    Some(String::from("File is not valid UTF-8 text (binary file?)")),
                ),
            },
            Err(_) => (Vec::new(), Some(String::from("Failed to read file"))),
        };

        Window {
            title: format!("View: {}", filename),
            x,
            y,
            width: 420,
            height: 260,
            minimized: false,
            maximized: false,
            restore_geometry: (x, y, 420, 260),
            content: WindowContent::TextViewer {
                filename: String::from(filename),
                lines,
                scroll: 0,
                error,
            },
        }
    }
}

fn load_file_list() -> (Vec<ext2::FileEntry>, Option<String>) {
    if !ext2::is_formatted() {
        match ext2::format() {
            Ok(()) => {}
            Err(_) => return (Vec::new(), Some(String::from("Disk error while formatting"))),
        }
        // Кладём один демонстрационный файл, чтобы окно не было пустым
        // сразу после первого форматирования.
        let _ = ext2::write_file(
            "README.TXT",
            b"Welcome to DeiX ext2!\r\nThis is a real file on a real ext2 volume.\r\n\
              Click a file in this window to open it in the TXT reader\r\n\
              (or run it, if it's a .MEX program).\r\n",
        );
    }

    match ext2::list_root() {
        Ok(entries) => (entries, None),
        Err(_) => (Vec::new(), Some(String::from("Failed to read directory"))),
    }
}


pub struct Desktop {
    windows: Vec<Window>,
    dragging_window: Option<usize>,
    drag_offset: (i32, i32),
    prev_left_button: bool,
    /// Момент последнего клика по заголовку каждого окна (индекс окна,
    /// timestamp мс) — нужен для распознавания двойного клика
    /// (разворачивание/восстановление окна). Храним только последний
    /// клик, а не полную историю: этого достаточно, чтобы сравнить
    /// интервал между двумя последовательными кликами по одному и тому
    /// же окну.
    last_titlebar_click: Option<(usize, u64)>,
    focused_window: Option<usize>,
    start_menu_open: bool,
    should_exit: bool,
    /// Если Some — пользователь выбрал новое разрешение в окне Display
    /// settings; run_event_loop завершится с DesktopExit::ChangeResolution,
    /// чтобы cli.rs пересоздал framebuffer и запустил цикл заново, не
    /// возвращаясь в текстовый режим (см. cmd_gpu_mode).
    pending_resolution: Option<(u32, u32)>,
    /// Момент запуска (мс с момента старта ядра) — нужен, чтобы часы на
    /// панели задач и волновой узор обоев были привязаны к абсолютному
    /// времени работы системы, а не ко времени с открытия desktop.
    frame_counter: u64,
}

/// Максимальный интервал между двумя кликами по заголовку одного окна,
/// который всё ещё считается "двойным кликом" (в миллисекундах).
const DOUBLE_CLICK_MS: u64 = 400;

/// Один пункт меню "Пуск" — подпись + действие. Вынесено в отдельный
/// список (а не серию if/else, как было раньше), чтобы добавление нового
/// пункта меню не требовало трогать код обработки кликов и отрисовки в
/// двух разных местах отдельно — только один список ниже.
enum StartMenuAction {
    OpenTerminal,
    OpenFiles,
    OpenAbout,
    OpenDisplaySettings,
    Restart,
    Shutdown,
}

fn start_menu_items() -> [(&'static str, StartMenuAction); 6] {
    [
        ("Terminal", StartMenuAction::OpenTerminal),
        ("Files", StartMenuAction::OpenFiles),
        ("Display settings", StartMenuAction::OpenDisplaySettings),
        ("About DeiX", StartMenuAction::OpenAbout),
        ("Restart", StartMenuAction::Restart),
        ("Shut down", StartMenuAction::Shutdown),
    ]
}

const START_MENU_ITEM_HEIGHT: i32 = 30;
const START_MENU_WIDTH: i32 = 190;

impl Desktop {
    pub fn new() -> Self {
        Desktop {
            windows: Vec::new(),
            dragging_window: None,
            drag_offset: (0, 0),
            prev_left_button: false,
            last_titlebar_click: None,
            focused_window: None,
            start_menu_open: false,
            should_exit: false,
            pending_resolution: None,
            frame_counter: 0,
        }
    }

    fn bring_to_front(&mut self, index: usize) {
        let window = self.windows.remove(index);
        self.windows.push(window);
        self.focused_window = Some(self.windows.len() - 1);
    }

    fn open_terminal(&mut self, screen_w: i32, screen_h: i32) {
        let x = 40 + (self.windows.len() as i32 * 24) % (screen_w - 400).max(1);
        let y = 40 + (self.windows.len() as i32 * 24) % (screen_h - 300).max(1);
        self.windows.push(Window::new_terminal(x, y));
        self.focused_window = Some(self.windows.len() - 1);
    }

    fn open_files(&mut self, screen_w: i32, screen_h: i32) {
        let x = 80 + (self.windows.len() as i32 * 24) % (screen_w - 340).max(1);
        let y = 80 + (self.windows.len() as i32 * 24) % (screen_h - 280).max(1);
        self.windows.push(Window::new_files(x, y));
        self.focused_window = Some(self.windows.len() - 1);
    }

    fn open_about(&mut self, screen_w: i32, screen_h: i32) {
        let x = (screen_w - 300) / 2;
        let y = (screen_h - 170) / 2;
        self.windows.push(Window::new_about(x, y));
        self.focused_window = Some(self.windows.len() - 1);
    }

    fn open_display_settings(&mut self, screen_w: i32, screen_h: i32) {
        let width = 220;
        let height = 40 + RESOLUTION_PRESETS.len() as i32 * 28;
        let x = (screen_w - width) / 2;
        let y = (screen_h - height) / 2;
        self.windows.push(Window::new_display_settings(x, y));
        self.focused_window = Some(self.windows.len() - 1);
    }

    /// Разворачивает окно на весь экран (сохраняя исходную геометрию для
    /// восстановления) либо возвращает его к прежнему размеру/позиции,
    /// если оно уже развёрнуто. Панель задач остаётся видимой поверх
    /// развёрнутого окна (окно занимает область до неё, не всю высоту
    /// экрана), чтобы можно было переключаться между приложениями и
    /// открывать меню "Пуск", даже когда какое-то окно на весь экран —
    /// как в любой настоящей ОС.
    fn toggle_maximize(&mut self, index: usize, screen_w: i32, screen_h: i32) {
        let taskbar_h = TASKBAR_HEIGHT as i32;
        if let Some(w) = self.windows.get_mut(index) {
            if w.maximized {
                let (x, y, width, height) = w.restore_geometry;
                w.x = x;
                w.y = y;
                w.width = width;
                w.height = height;
                w.maximized = false;
            } else {
                w.restore_geometry = (w.x, w.y, w.width, w.height);
                w.x = 0;
                w.y = 0;
                w.width = screen_w as u32;
                w.height = (screen_h - taskbar_h - TITLEBAR_HEIGHT) as u32;
                w.maximized = true;
            }
        }
    }

    fn open_text_viewer(&mut self, screen_w: i32, screen_h: i32, filename: &str) {
        let x = (60 + (self.windows.len() as i32 * 24)) % (screen_w - 440).max(1);
        let y = (60 + (self.windows.len() as i32 * 24)) % (screen_h - 320).max(1);
        self.windows.push(Window::new_text_viewer(x, y, filename));
        self.focused_window = Some(self.windows.len() - 1);
    }

    /// Обрабатывает мышь (перетаскивание, закрытие/минимизация, фокус,
    /// кнопка "Пуск" и меню, клики по панели задач) и клавиатуру (ввод в
    /// фокусированное окно терминала). Вызывается один раз за кадр перед
    /// отрисовкой.
    fn handle_input(&mut self, screen_w: i32, screen_h: i32) {
        let m = mouse::snapshot();
        let just_pressed = m.left_button && !self.prev_left_button;

        if just_pressed {
            let taskbar_y = screen_h - TASKBAR_HEIGHT as i32;
            let in_start_button = m.x < START_BUTTON_WIDTH && m.y >= taskbar_y;

            if in_start_button {
                self.start_menu_open = !self.start_menu_open;
            } else if self.start_menu_open {
                self.handle_start_menu_click(m.x, m.y, taskbar_y, screen_w, screen_h);
                self.start_menu_open = false;
            } else if m.y >= taskbar_y {
                self.handle_taskbar_click(m.x, taskbar_y, screen_w);
            } else {
                self.handle_window_click(m.x, m.y, screen_w, screen_h);
            }
        }

        if !m.left_button {
            self.dragging_window = None;
        }

        if let Some(idx) = self.dragging_window {
            if let Some(w) = self.windows.get_mut(idx) {
                w.x = (m.x - self.drag_offset.0).clamp(0, screen_w - 40);
                w.y = (m.y - self.drag_offset.1).clamp(0, screen_h - TASKBAR_HEIGHT as i32 - 20);
            }
        }

        self.prev_left_button = m.left_button;

        // Клавиатурный ввод — только в фокусированное окно (и только
        // если оно не свёрнуто в панель задач).
        if let Some(idx) = self.focused_window {
            let minimized = self.windows.get(idx).map(|w| w.minimized).unwrap_or(true);
            if minimized {
                while keyboard::try_read_char().is_some() {}
                return;
            }
            if let Some(w) = self.windows.get_mut(idx) {
                match &mut w.content {
                    WindowContent::Terminal { lines, current_line } => {
                        while let Some(byte) = keyboard::try_read_char() {
                            match byte {
                                b'\n' => {
                                    let cmd = current_line.clone();
                                    lines.push(format!("> {}", cmd));
                                    run_mini_terminal_command(&cmd, lines);
                                    current_line.clear();
                                }
                                0x08 => {
                                    current_line.pop();
                                }
                                keyboard::ARROW_UP | keyboard::ARROW_DOWN => {
                                    // Навигация по истории команд пока не
                                    // реализована в графическом терминале
                                    // (отдельная от текстового CLI history —
                                    // не критично для MVP) — игнорируем,
                                    // чтобы управляющие байты не попадали в
                                    // current_line как обычные символы.
                                }
                                b if b >= 0x20 && b < 0x7F => {
                                    current_line.push(b as char);
                                }
                                _ => {}
                            }
                        }
                        // Ограничиваем историю строк, чтобы не расти бесконечно.
                        while lines.len() > 200 {
                            lines.remove(0);
                        }
                    }
                    WindowContent::TextViewer { lines, scroll, .. } => {
                        while let Some(byte) = keyboard::try_read_char() {
                            match byte {
                                keyboard::ARROW_UP => {
                                    *scroll = scroll.saturating_sub(1);
                                }
                                keyboard::ARROW_DOWN => {
                                    if *scroll + 1 < lines.len() {
                                        *scroll += 1;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    WindowContent::Files { .. } | WindowContent::About | WindowContent::DisplaySettings => {
                        // Не принимают клавиатурный ввод.
                        while keyboard::try_read_char().is_some() {}
                    }
                }
            }
        } else {
            // Если фокуса нет — всё равно вычитываем очередь клавиатуры,
            // чтобы не копилась и не переполнялась, пока открыт desktop.
            while keyboard::try_read_char().is_some() {}
        }
    }

    fn handle_start_menu_click(&mut self, x: i32, y: i32, taskbar_y: i32, screen_w: i32, screen_h: i32) {
        let items = start_menu_items();
        let menu_y = taskbar_y - items.len() as i32 * START_MENU_ITEM_HEIGHT;
        if x >= START_MENU_WIDTH || x < 0 {
            return;
        }
        let idx = ((y - menu_y) / START_MENU_ITEM_HEIGHT).max(0) as usize;
        if y < menu_y {
            return;
        }
        if let Some((_, action)) = items.get(idx) {
            match action {
                StartMenuAction::OpenTerminal => self.open_terminal(screen_w, screen_h),
                StartMenuAction::OpenFiles => self.open_files(screen_w, screen_h),
                StartMenuAction::OpenAbout => self.open_about(screen_w, screen_h),
                StartMenuAction::OpenDisplaySettings => self.open_display_settings(screen_w, screen_h),
                StartMenuAction::Restart => {
                    crate::cli::cmd_reboot();
                }
                StartMenuAction::Shutdown => {
                    crate::cli::cmd_halt();
                }
            }
        }
    }

    /// Клик по панели задач вне кнопки "Пуск": переключает
    /// свёрнутость/фокус окна, чей значок был нажат.
    fn handle_taskbar_click(&mut self, x: i32, _taskbar_y: i32, _screen_w: i32) {
        let relative_x = x - START_BUTTON_WIDTH - 8;
        if relative_x < 0 {
            return;
        }
        let idx = (relative_x / TASKBAR_ITEM_WIDTH) as usize;
        if idx >= self.windows.len() {
            return;
        }
        if self.windows[idx].minimized {
            self.windows[idx].minimized = false;
            self.bring_to_front(idx);
        } else if self.focused_window == Some(idx) {
            self.windows[idx].minimized = true;
            self.focused_window = None;
        } else {
            self.bring_to_front(idx);
        }
    }

    fn handle_window_click(&mut self, mx: i32, my: i32, screen_w: i32, screen_h: i32) {
        for i in (0..self.windows.len()).rev() {
            if self.windows[i].minimized {
                continue;
            }
            let w = &self.windows[i];
            let in_titlebar =
                mx >= w.x && mx < w.x + w.width as i32 && my >= w.y && my < w.y + TITLEBAR_HEIGHT;

            // Три круглые кнопки в стиле "светофора": close (красная),
            // minimize (жёлтая), maximize (зелёная) — выровнены по
            // правому краю заголовка, слева направо в этом порядке (как
            // читает глаз слева направо, кнопка "самая опасная" — close
            // — крайняя справа, чтобы её было сложнее случайно задеть).
            let (close_cx, close_cy) = title_button_center(w, 0);
            let (min_cx, min_cy) = title_button_center(w, 1);
            let (max_cx, max_cy) = title_button_center(w, 2);
            let in_close = circle_hit(mx, my, close_cx, close_cy, BUTTON_DIAMETER / 2);
            let in_minimize = circle_hit(mx, my, min_cx, min_cy, BUTTON_DIAMETER / 2);
            let in_maximize = circle_hit(mx, my, max_cx, max_cy, BUTTON_DIAMETER / 2);

            let in_window_body = mx >= w.x
                && mx < w.x + w.width as i32
                && my >= w.y
                && my < w.y + w.height as i32 + TITLEBAR_HEIGHT;

            if in_close {
                self.windows.remove(i);
                self.focused_window = None;
                return;
            } else if in_minimize {
                self.windows[i].minimized = true;
                self.focused_window = None;
                return;
            } else if in_maximize {
                self.toggle_maximize(i, screen_w, screen_h);
                self.focused_window = Some(i);
                self.bring_to_front(i);
                return;
            } else if in_titlebar {
                // Двойной клик по заголовку (не по кнопкам) тоже
                // разворачивает/восстанавливает окно — стандартное
                // поведение большинства оконных менеджеров.
                let now = timer::uptime_ms();
                let is_double_click = matches!(self.last_titlebar_click, Some((idx, t)) if idx == i && now.saturating_sub(t) <= DOUBLE_CLICK_MS);
                self.last_titlebar_click = Some((i, now));

                if is_double_click {
                    self.toggle_maximize(i, screen_w, screen_h);
                    self.last_titlebar_click = None;
                } else if !self.windows[i].maximized {
                    // Перетаскивание имеет смысл только для не-развёрнутого
                    // окна — развёрнутое окно всегда занимает фиксированную
                    // позицию (0,0) до панели задач.
                    self.dragging_window = Some(i);
                    self.drag_offset = (mx - w.x, my - w.y);
                }
                self.focused_window = Some(i);
                self.bring_to_front(i);
                return;
            } else if in_window_body {
                let content_y = w.y + TITLEBAR_HEIGHT;
                let clicked_line = ((my - content_y - 6) / 16).max(0) as usize;

                // Клик по строке файла в окне Files открывает его в
                // TextViewer (или запускает, если это .MEX программа —
                // см. Window::new_text_viewer).
                if let WindowContent::Files { entries, .. } = &w.content {
                    if let Some(entry) = entries.get(clicked_line) {
                        if !entry.is_directory {
                            let name = entry.name.clone();
                            self.focused_window = Some(i);
                            self.bring_to_front(i);
                            self.open_text_viewer(screen_w, screen_h, &name);
                            return;
                        }
                    }
                }

                // Клик по строке разрешения в окне Display settings
                // запускает смену видеорежима "на лету" (обрабатывается
                // в run_event_loop через pending_resolution).
                if let WindowContent::DisplaySettings = &w.content {
                    let clicked_row = ((my - content_y - 36) / 28).max(-1);
                    if clicked_row >= 0 {
                        if let Some(&(rw, rh)) = RESOLUTION_PRESETS.get(clicked_row as usize) {
                            self.pending_resolution = Some((rw, rh));
                            return;
                        }
                    }
                }

                self.focused_window = Some(i);
                self.bring_to_front(i);
                return;
            }
        }
    }

    fn render(&mut self, r: &mut Renderer) {
        let w = r.width() as i32;
        let h = r.height() as i32;

        draw_wallpaper(r, self.frame_counter, w, h);

        for i in 0..self.windows.len() {
            if self.windows[i].minimized {
                continue;
            }
            let is_focused = self.focused_window == Some(i);
            draw_window_shadow(r, &self.windows[i]);
            draw_window(r, &self.windows[i], is_focused);
        }

        draw_taskbar(r, self, w, h);

        if self.start_menu_open {
            draw_start_menu(r, w, h);
        }

        draw_cursor(r);

        // Кадр целиком собран в back buffer (обычная RAM) — теперь одним
        // проходом копируем его в реальный MMIO framebuffer. Без этого
        // финального шага пришлось бы писать в видеопамять поэлементно
        // прямо во время отрисовки каждого примитива, что и вызывало
        // видимые артефакты/подтормаживание при перетаскивании окон (см.
        // подробное объяснение в renderer.rs у структуры Renderer).
        r.present();
    }

    /// Главный цикл: перерисовывает кадр, пока не будет нажат Esc (выход
    /// обратно в текстовый CLI) или выбрано новое разрешение в окне
    /// Display settings (тогда цикл завершается с
    /// DesktopExit::ChangeResolution, и вызывающий код в cli.rs создаёт
    /// новый framebuffer и снова вызывает run_event_loop — без выхода в
    /// текстовый режим между этими двумя действиями). Частота кадров
    /// привязана к таймеру ~30 Гц — достаточно отзывчиво и не
    /// перегружает эмулируемый CPU постоянной перерисовкой на максимум.
    ///
    /// `preserve_windows`: если true, список открытых окон НЕ
    /// пересоздаётся демонстрационными Files/Terminal — используется при
    /// повторном входе в цикл после смены разрешения, чтобы пользователь
    /// не терял открытые окна и их содержимое.
    pub fn run_event_loop(&mut self, r: &mut Renderer, preserve_windows: bool) -> DesktopExit {
        let screen_w = r.width() as i32;
        let screen_h = r.height() as i32;

        if !preserve_windows {
            // Стартовые демонстрационные окна размещаются рядом, а не
            // друг на друге — Files слева, Terminal справа от него,
            // чтобы сразу после запуска оба были видны целиком без
            // необходимости что-то двигать. Terminal открывается
            // ПОСЛЕДНИМ, чтобы получить фокус по умолчанию (open_*()
            // отдаёт фокус вновь открытому окну) — так пользователь
            // может сразу печатать команды, не кликая по окну мышью.
            self.windows.push(Window::new_files(30, 40));
            self.windows.push(Window::new_terminal(360, 60));
            self.focused_window = Some(self.windows.len() - 1);
        } else {
            // При повторном входе после смены разрешения разворачиваем
            // заново любые окна, которые были в maximized-состоянии —
            // иначе они остались бы привязаны к размеру ПРЕДЫДУЩЕГО
            // экрана и торчали бы за пределы нового.
            for i in 0..self.windows.len() {
                if self.windows[i].maximized {
                    self.windows[i].maximized = false; // сброс, чтобы toggle сработал в "развернуть"
                    self.toggle_maximize(i, screen_w, screen_h);
                }
            }
        }

        let mut last_frame = timer::uptime_ms();
        const FRAME_INTERVAL_MS: u64 = 33; // ~30 FPS

        loop {
            // ESC (скан-код 0x01) обрабатывается на уровне сырых байт —
            // клавиатура транслирует его в управляющий байт ESC (0x1B) в
            // нашей ASCII-таблице не отображается, поэтому проверяем явно
            // через отдельный неблокирующий метод.
            if keyboard::try_read_escape() {
                self.should_exit = true;
            }

            self.handle_input(screen_w, screen_h);

            if let Some((w, h)) = self.pending_resolution.take() {
                return DesktopExit::ChangeResolution(w, h);
            }

            if self.should_exit {
                return DesktopExit::Quit;
            }

            let now = timer::uptime_ms();
            if now.saturating_sub(last_frame) >= FRAME_INTERVAL_MS {
                self.frame_counter = now;
                self.render(r);
                last_frame = now;
            }

            unsafe { core::arch::asm!("hlt") };
        }
    }
}

/// Выполняет команду через РЕАЛЬНЫЙ полный CLI (crate::cli::execute) —
/// то есть окно терминала на рабочем столе понимает ровно тот же набор
/// команд, что и обычный текстовый режим (help/ping/wifi/gpu/pkg/run/
/// ls/cat/write/... — весь список из cli.rs), а не отдельную урезанную
/// копию. Реализовано через временный "перехват" вывода print!/println!
/// в строку (см. vgaglobal::begin_capture/end_capture), которая затем
/// разбивается на строки и добавляется в прокручиваемую историю окна.
///
/// Пока выполнение внутри графического терминала помечено флагом
/// cli::IN_GRAPHICAL_TERMINAL — это отключает несколько команд, не
/// имеющих смысла или опасных в этом контексте (gpu mode/demo, reboot,
/// halt — см. подробности в cli.rs).
fn run_mini_terminal_command(cmd: &str, lines: &mut Vec<String>) {
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return;
    }

    // "clear" в обычном CLI очищает скрытый (в графическом режиме) VGA
    // text buffer — здесь же нужно явно очистить историю строк самого
    // окна, иначе команда визуально ничего не сделает.
    if cmd == "clear" {
        lines.clear();
        return;
    }

    crate::vgaglobal::begin_capture();
    crate::cli::IN_GRAPHICAL_TERMINAL.store(true, core::sync::atomic::Ordering::Relaxed);
    crate::cli::execute(cmd);
    crate::cli::IN_GRAPHICAL_TERMINAL.store(false, core::sync::atomic::Ordering::Relaxed);
    let output = crate::vgaglobal::end_capture();

    for line in output.lines() {
        lines.push(String::from(line));
    }
}

// ==================== Отрисовка: обои рабочего стола ====================

/// Рисует фон рабочего стола ПРОЦЕДУРНО — никакой картинки/PNG/bitmap-
/// ресурса здесь нет и быть не может (в ядре нет декодера изображений).
/// Вместо этого: вертикальный градиент неба + диагональные "волны"
/// светлее/темнее базового цвета, чья фаза медленно сдвигается со
/// временем (frame_counter — миллисекунды с начала работы ядра), отчего
/// узор выглядит как мягко "дышащий" фон, а не статичная заливка.
fn draw_wallpaper(r: &mut Renderer, frame_counter: u64, w: i32, h: i32) {
    let top = Color::rgb(15, 35, 70);
    let bottom = Color::rgb(0, 70, 120);
    r.fill_rect_gradient_v(0, 0, w as u32, h as u32, top, bottom);

    // Фаза узора: полный цикл раз в ~8 секунд, целочисленная арифметика
    // (нет плавающей точки в этом окружении — см. renderer::isqrt).
    let phase = ((frame_counter / 20) % 64) as i32;
    let stripe_spacing = 48;
    let stripe_width = 3;

    let mut offset = -h - phase;
    while offset < w + h {
        // Диагональная линия рисуется как последовательность коротких
        // горизонтальных отрезков со смещающимся x — дешевле, чем честный
        // Брезенхэм на весь экран, и не требует специального клиппинга.
        for y in (0..h).step_by(4) {
            let x = offset + y;
            r.fill_rect(x, y, stripe_width as u32, 4, Color::rgb(255, 255, 255).lerp(bottom, 235));
        }
        offset += stripe_spacing;
    }

    // Логотип-надпись по центру верхней части экрана — просто текст,
    // нарисованный тем же битмап-шрифтом, что и весь остальной UI (не
    // картинка).
    let label = "DeiX";
    let label_x = w / 2 - (label.len() as i32 * 8 * 2) / 2;
    draw_text_scaled(r, label_x, 24, label, Color::rgb(255, 255, 255).lerp(top, 40), 2);
}

/// Рисует текст с целочисленным масштабированием (каждый пиксель глифа
/// превращается в scale x scale блок) — используется для крупного
/// заголовка "DeiX" на обоях, без необходимости в отдельном крупном
/// шрифте.
fn draw_text_scaled(r: &mut Renderer, x: i32, y: i32, text: &str, color: Color, scale: i32) {
    let mut cursor_x = x;
    for byte in text.bytes() {
        let glyph = crate::font::read_glyph(byte);
        for (row, &line) in glyph.iter().enumerate() {
            if line == 0 {
                continue;
            }
            for col in 0..8 {
                if (line >> (7 - col)) & 1 != 0 {
                    r.fill_rect(
                        cursor_x + col as i32 * scale,
                        y + row as i32 * scale,
                        scale as u32,
                        scale as u32,
                        color,
                    );
                }
            }
        }
        cursor_x += 8 * scale;
    }
}

// ==================== Отрисовка: окна ====================

fn title_button_center(w: &Window, index: i32) -> (i32, i32) {
    // index=0 -> close (крайняя правая), index=1 -> minimize,
    // index=2 -> maximize (следующая левее).
    let cx = w.x + w.width as i32 - 14 - index * (BUTTON_DIAMETER + 8);
    let cy = w.y + TITLEBAR_HEIGHT / 2;
    (cx, cy)
}

fn circle_hit(px: i32, py: i32, cx: i32, cy: i32, radius: i32) -> bool {
    let dx = px - cx;
    let dy = py - cy;
    dx * dx + dy * dy <= radius * radius
}

/// Рисует мягкую тень под окном — несколько всё более широких и всё более
/// прозрачных (через shade_rect) прямоугольников со смещением вниз-вправо.
/// Простая, но эффективная имитация drop shadow без настоящего альфа-блендинга.
fn draw_window_shadow(r: &mut Renderer, w: &Window) {
    let full_h = w.height as i32 + TITLEBAR_HEIGHT;
    for i in (1..=4).rev() {
        let spread = i * 2;
        r.shade_rect(
            w.x - spread + 4,
            w.y - spread + 6,
            w.width + (spread * 2) as u32,
            (full_h + spread * 2) as u32,
            18,
        );
    }
}

fn draw_window(r: &mut Renderer, w: &Window, focused: bool) {
    let titlebar_top = if focused {
        Color::rgb(50, 110, 210)
    } else {
        Color::rgb(130, 130, 130)
    };
    let titlebar_bottom = if focused {
        Color::TITLEBAR_ACTIVE
    } else {
        Color::TITLEBAR_INACTIVE
    };

    // Заголовок с лёгким вертикальным градиентом и закруглёнными верхними
    // углами — вместо плоской однотонной полосы, как раньше.
    r.fill_rounded_rect(w.x, w.y, w.width, TITLEBAR_HEIGHT as u32 + WINDOW_CORNER_RADIUS as u32, WINDOW_CORNER_RADIUS, titlebar_top);
    r.fill_rect(w.x, w.y + WINDOW_CORNER_RADIUS, w.width, (TITLEBAR_HEIGHT - WINDOW_CORNER_RADIUS) as u32, titlebar_bottom);
    r.fill_rect_gradient_v(w.x, w.y, w.width, TITLEBAR_HEIGHT as u32, titlebar_top, titlebar_bottom);

    r.draw_text(w.x + 10, w.y + 5, &w.title, Color::WHITE, None);

    // Круглые кнопки "светофор" (close=красная, minimize=жёлтая,
    // maximize=зелёная) вместо прямоугольного крестика — более
    // современный/аккуратный вид, как в большинстве настольных ОС.
    let (close_cx, close_cy) = title_button_center(w, 0);
    let (min_cx, min_cy) = title_button_center(w, 1);
    let (max_cx, max_cy) = title_button_center(w, 2);
    draw_circle_button(r, close_cx, close_cy, BUTTON_DIAMETER / 2, Color::rgb(230, 70, 60));
    draw_circle_button(r, min_cx, min_cy, BUTTON_DIAMETER / 2, Color::rgb(230, 180, 40));
    draw_circle_button(r, max_cx, max_cy, BUTTON_DIAMETER / 2, Color::rgb(70, 190, 90));

    let content_y = w.y + TITLEBAR_HEIGHT;

    match &w.content {
        WindowContent::Terminal { lines, current_line } => {
            r.fill_rect(w.x, content_y, w.width, w.height, Color::rgb(18, 18, 22));
            let max_lines = (w.height as i32 - 8) / 16;
            let start = lines.len().saturating_sub(max_lines.max(1) as usize - 1);
            let mut line_y = content_y + 4;
            for line in &lines[start..] {
                r.draw_text(w.x + 6, line_y, truncate(line, (w.width as usize - 12) / 8), Color::rgb(90, 230, 120), None);
                line_y += 16;
            }
            let prompt = format!("> {}_", current_line);
            r.draw_text(w.x + 6, line_y, truncate(&prompt, (w.width as usize - 12) / 8), Color::rgb(90, 230, 120), None);
        }
        WindowContent::Files { entries, error } => {
            r.fill_rect(w.x, content_y, w.width, w.height, Color::rgb(240, 240, 245));
            let mut line_y = content_y + 6;
            if let Some(err) = error {
                r.draw_text(w.x + 6, line_y, err, Color::RED, None);
            } else if entries.is_empty() {
                r.draw_text(w.x + 6, line_y, "(empty)", Color::DARK_GRAY, None);
            } else {
                for (row, entry) in entries.iter().enumerate() {
                    // Лёгкая "зебра" на строках списка файлов — заметно
                    // легче ориентироваться взглядом, чем сплошной список.
                    if row % 2 == 1 {
                        r.fill_rect(w.x + 2, line_y - 2, w.width - 4, 16, Color::rgb(225, 230, 240));
                    }
                    let label = if entry.is_directory {
                        format!("[{}]", entry.name)
                    } else {
                        format!("{} ({} B)", entry.name, entry.size)
                    };
                    let color = if entry.is_directory { Color::rgb(0, 60, 160) } else { Color::BLACK };
                    r.draw_text(w.x + 6, line_y, truncate(&label, (w.width as usize - 12) / 8), color, None);
                    line_y += 16;
                    if line_y > content_y + w.height as i32 - 16 {
                        break;
                    }
                }
            }
        }
        WindowContent::TextViewer { lines, scroll, error, .. } => {
            r.fill_rect(w.x, content_y, w.width, w.height, Color::rgb(252, 252, 244));
            let mut line_y = content_y + 6;
            if let Some(err) = error {
                r.draw_text(w.x + 6, line_y, err, Color::RED, None);
            } else if lines.is_empty() {
                r.draw_text(w.x + 6, line_y, "(empty file)", Color::DARK_GRAY, None);
            } else {
                let max_lines = ((w.height as i32 - 12) / 16).max(1) as usize;
                let start = (*scroll).min(lines.len().saturating_sub(1));
                let end = (start + max_lines).min(lines.len());
                for line in &lines[start..end] {
                    r.draw_text(w.x + 6, line_y, truncate(line, (w.width as usize - 12) / 8), Color::BLACK, None);
                    line_y += 16;
                }
                if lines.len() > max_lines {
                    let indicator = format!("{}/{}", start + 1, lines.len());
                    r.draw_text(
                        w.x + w.width as i32 - (indicator.len() as i32 * 8) - 6,
                        content_y + 4,
                        &indicator,
                        Color::GRAY,
                        None,
                    );
                }
            }
        }
        WindowContent::About => {
            r.fill_rect(w.x, content_y, w.width, w.height, Color::rgb(250, 250, 250));
            let uptime_s = timer::uptime_ms() / 1000;
            let lines = [
                String::from("DeiX v0.1"),
                String::from("A mini x86_64 OS written in Rust"),
                String::from("Bootloader: BIOS MBR (no GRUB)"),
                String::from("Filesystem: ext2 (real, e2fsck-clean)"),
                format!("Uptime: {}s", uptime_s),
            ];
            let mut line_y = content_y + 12;
            for line in &lines {
                r.draw_text(w.x + 12, line_y, line, Color::rgb(20, 20, 30), None);
                line_y += 18;
            }
        }
        WindowContent::DisplaySettings => {
            r.fill_rect(w.x, content_y, w.width, w.height, Color::rgb(245, 246, 250));
            r.draw_text(w.x + 10, content_y + 10, "Choose resolution:", Color::rgb(20, 20, 30), None);
            let mut line_y = content_y + 36;
            for &(rw, rh) in RESOLUTION_PRESETS.iter() {
                let is_current = rw == r.width() && rh == r.height();
                if is_current {
                    r.fill_rect(w.x + 6, line_y - 3, w.width - 12, 22, Color::rgb(200, 220, 250));
                }
                let label = format!("{}x{}{}", rw, rh, if is_current { "  (current)" } else { "" });
                r.draw_text(w.x + 12, line_y, &label, Color::rgb(20, 20, 30), None);
                line_y += 28;
            }
        }
    }

    r.draw_rect(w.x, w.y, w.width, w.height as u32 + TITLEBAR_HEIGHT as u32, Color::rgb(10, 10, 15));
}

fn draw_circle_button(r: &mut Renderer, cx: i32, cy: i32, radius: i32, color: Color) {
    for y in -radius..=radius {
        let dx = isqrt_local((radius * radius - y * y).max(0));
        r.draw_hline(cx - dx, cy + y, (dx * 2 + 1) as u32, color);
    }
}

/// Локальная копия целочисленного квадратного корня — та же реализация,
/// что и в renderer.rs (не экспортируется оттуда, чтобы не разрастался
/// публичный API рендерера ради одной вспомогательной функции отрисовки
/// кнопок UI).
fn isqrt_local(n: i32) -> i32 {
    if n <= 0 {
        return 0;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

fn truncate(s: &str, max_chars: usize) -> &str {
    if s.len() <= max_chars {
        s
    } else {
        &s[..max_chars]
    }
}

// ==================== Отрисовка: панель задач и меню ====================

fn draw_taskbar(r: &mut Renderer, desktop: &Desktop, screen_w: i32, screen_h: i32) {
    let y = screen_h - TASKBAR_HEIGHT as i32;

    // Полупрозрачная (через shade+градиент) тёмная панель вместо плоской
    // заливки одним серым цветом.
    r.fill_rect_gradient_v(0, y, screen_w as u32, TASKBAR_HEIGHT, Color::rgb(35, 38, 48), Color::rgb(22, 24, 32));
    r.draw_hline(0, y, screen_w as u32, Color::rgb(70, 130, 220));

    // Кнопка "Пуск" — закруглённая, с изменением цвета при открытом меню.
    let start_color = if desktop.start_menu_open {
        Color::rgb(70, 130, 220)
    } else {
        Color::rgb(50, 100, 180)
    };
    r.fill_rounded_rect(4, y + 4, (START_BUTTON_WIDTH - 8) as u32, TASKBAR_HEIGHT - 8, 6, start_color);
    r.draw_text(16, y + (TASKBAR_HEIGHT as i32 - 16) / 2, "Start", Color::WHITE, None);

    // Список открытых окон в виде "вкладок" на панели задач — фокусное
    // окно выделено более светлым фоном, свёрнутое — приглушённым.
    for (i, w) in desktop.windows.iter().enumerate() {
        let item_x = START_BUTTON_WIDTH + 8 + i as i32 * TASKBAR_ITEM_WIDTH;
        if item_x + TASKBAR_ITEM_WIDTH > screen_w - 90 {
            break;
        }
        let is_focused = desktop.focused_window == Some(i) && !w.minimized;
        let bg = if is_focused {
            Color::rgb(60, 90, 150)
        } else if w.minimized {
            Color::rgb(30, 32, 40)
        } else {
            Color::rgb(45, 48, 58)
        };
        r.fill_rounded_rect(item_x, y + 5, (TASKBAR_ITEM_WIDTH - 6) as u32, TASKBAR_HEIGHT - 10, 5, bg);
        let label_color = if w.minimized { Color::GRAY } else { Color::WHITE };
        r.draw_text(item_x + 8, y + (TASKBAR_HEIGHT as i32 - 16) / 2, truncate(&w.title, 15), label_color, None);
    }

    // Часы (uptime в формате мм:сс) в правом углу панели.
    let ms = timer::uptime_ms();
    let secs = ms / 1000;
    let clock = format!("{:02}:{:02}", (secs / 60) % 100, secs % 60);
    r.draw_text(screen_w - 56, y + (TASKBAR_HEIGHT as i32 - 16) / 2, &clock, Color::WHITE, None);
}

fn draw_start_menu(r: &mut Renderer, screen_w: i32, screen_h: i32) {
    let items = start_menu_items();
    let taskbar_y = screen_h - TASKBAR_HEIGHT as i32;
    let menu_h = items.len() as i32 * START_MENU_ITEM_HEIGHT;
    let menu_y = taskbar_y - menu_h;

    // Затемняем область позади меню (кроме самого меню) — простой, но
    // эффективный способ визуально выделить, что сейчас модальная область.
    r.shade_rect(0, 0, screen_w as u32, taskbar_y as u32, 60);

    r.fill_rounded_rect(0, menu_y, START_MENU_WIDTH as u32, menu_h as u32, 8, Color::rgb(248, 248, 250));
    r.draw_rect(0, menu_y, START_MENU_WIDTH as u32, menu_h as u32, Color::rgb(40, 40, 50));

    for (i, (label, _)) in items.iter().enumerate() {
        let item_y = menu_y + i as i32 * START_MENU_ITEM_HEIGHT;
        if i > 0 {
            r.draw_hline(4, item_y, (START_MENU_WIDTH - 8) as u32, Color::rgb(225, 225, 230));
        }
        let color = if label.contains("Shut") || label.contains("Restart") {
            Color::rgb(180, 40, 40)
        } else {
            Color::rgb(20, 20, 30)
        };
        r.draw_text(16, item_y + (START_MENU_ITEM_HEIGHT - 16) / 2, label, color, None);
    }
}

fn draw_cursor(r: &mut Renderer) {
    let m = mouse::snapshot();
    let (x, y) = (m.x, m.y);

    // Классическая стрелка-курсор (несколько треугольных линий) с чёрной
    // обводкой и белой (или красной при зажатой ЛКМ) заливкой — как и
    // раньше, но с дополнительной диагональю для более узнаваемой формы
    // стрелки вместо "растопыренного веера" линий.
    let color = if m.left_button { Color::RED } else { Color::WHITE };
    let points: [(i32, i32); 7] = [
        (0, 0), (0, 14), (4, 11), (6, 16), (8, 15), (6, 10), (11, 10),
    ];
    for i in 0..points.len() {
        let (x0, y0) = points[i];
        let (x1, y1) = points[(i + 1) % points.len()];
        r.draw_line(x + x0, y + y0, x + x1, y + y1, Color::BLACK);
    }
    r.fill_rect(x + 1, y + 1, 4, 8, color);
}

/// Точка входа для CLI-команды `gpu mode` — запускает полноценный
/// интерактивный desktop и обрабатывает переключения разрешения "на
/// лету" (через окно Display settings), не выходя в текстовый режим
/// между ними. Возвращает управление в cli.rs только когда пользователь
/// нажимает Esc (настоящий выход в текстовый CLI).
///
/// `initial_width/initial_height` нужны, чтобы при смене разрешения
/// вызывающий код (cli.rs) знал, какой видеорежим установить перед
/// следующим вызовом with_renderer_long — сам Desktop не имеет доступа к
/// gpu::GpuInfo/vbe::set_mode напрямую (не хотим тащить их зависимость
/// в ui.rs, у которого и так достаточно ответственности).
pub fn run_desktop_session(mut on_resolution_change: impl FnMut(u32, u32) -> bool) {
    let mut desktop = Desktop::new();
    let mut preserve_windows = false;

    loop {
        let exit = crate::renderer::with_renderer_long(|r| desktop.run_event_loop(r, preserve_windows));

        match exit {
            Some(DesktopExit::Quit) | None => return,
            Some(DesktopExit::ChangeResolution(w, h)) => {
                preserve_windows = true;
                if !on_resolution_change(w, h) {
                    // Не удалось установить новый видеорежим (например,
                    // Bochs VBE отклонил недопустимую комбинацию) —
                    // возвращаемся в текстовый режим, а не зависаем в
                    // цикле с нерабочим framebuffer.
                    return;
                }
            }
        }
    }
}
