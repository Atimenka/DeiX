//! ГРАФИЧЕСКИЙ ЭКРАН ВХОДА, создание первой учётной записи и блокировка.
//!
//! Работает поверх того же framebuffer'а, что и загрузочное лого
//! (`bootlogo`), и переиспользует готовую логику учёток из `auth`:
//! `create_user` / `verify_login` / `has_any_users`. Второй системы
//! паролей здесь нет — только оболочка над существующей.
//!
//! ## Ввод
//!
//! Читаем два канала сразу — PS/2 и COM1, как это делает текстовый
//! `auth::read_line`. Без COM1 экран нельзя было бы проверить в
//! headless-QEMU, а весь цикл отладки держится на `-serial stdio`.
//!
//! ## Кириллица
//!
//! `renderer::draw_text` идёт по БАЙТАМ, поэтому UTF-8 (два байта на
//! русскую букву) рассыпается в мусор. Все подписи — латиницей: это
//! осознанное ограничение, а не недосмотр.

use alloc::string::String;

use crate::renderer::Color;

const BG: Color = Color(0x0A0E14);
const PANEL: Color = Color(0x121821);
const BORDER: Color = Color(0x1E2A38);
const ACCENT: Color = Color(0x00D0FF);
const TEXT: Color = Color(0xE8F0F8);
const HINT: Color = Color(0x8FB8D8);
const ERR: Color = Color(0xFF6B6B);
const OK_C: Color = Color(0x7ED957);

const MAX_INPUT: usize = 64;

/// Одно поле ввода.
struct Field {
    buf: [u8; MAX_INPUT],
    len: usize,
    mask: bool,
}

impl Field {
    const fn new(mask: bool) -> Self {
        Field {
            buf: [0; MAX_INPUT],
            len: 0,
            mask,
        }
    }
    fn clear(&mut self) {
        self.len = 0;
    }
    fn push(&mut self, c: u8) {
        if self.len < MAX_INPUT {
            self.buf[self.len] = c;
            self.len += 1;
        }
    }
    fn pop(&mut self) {
        if self.len > 0 {
            self.len -= 1;
        }
    }
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
    /// Что показывать: пароль скрываем звёздочками.
    fn display(&self, out: &mut [u8; MAX_INPUT]) -> usize {
        for i in 0..self.len {
            out[i] = if self.mask { b'*' } else { self.buf[i] };
        }
        self.len
    }
}

/// Рамка прямоугольника толщиной 1 px.
fn frame(r: &mut crate::renderer::Renderer, x: i32, y: i32, w: u32, h: u32, c: Color) {
    r.fill_rect(x, y, w, 1, c);
    r.fill_rect(x, y + h as i32 - 1, w, 1, c);
    r.fill_rect(x, y, 1, h, c);
    r.fill_rect(x + w as i32 - 1, y, 1, h, c);
}

/// Текст увеличенным шрифтом: каждый пиксель глифа — квадрат scale×scale.
fn text_scaled(
    r: &mut crate::renderer::Renderer,
    x: i32,
    y: i32,
    s: &str,
    color: Color,
    scale: i32,
) {
    let scale = scale.max(1);
    if scale == 1 {
        r.draw_text(x, y, s, color, None);
        return;
    }
    let mut cx = x;
    for byte in s.bytes() {
        let glyph = crate::font::read_glyph(byte);
        for (row, &bits) in glyph.iter().enumerate() {
            if bits == 0 {
                continue;
            }
            for col in 0..8i32 {
                if (bits >> (7 - col)) & 1 != 0 {
                    r.fill_rect(
                        cx + col * scale,
                        y + row as i32 * scale,
                        scale as u32,
                        scale as u32,
                        color,
                    );
                }
            }
        }
        cx += 8 * scale;
    }
}

fn text_center(r: &mut crate::renderer::Renderer, y: i32, s: &str, color: Color, scale: i32) {
    let w = s.len() as i32 * 8 * scale.max(1);
    text_scaled(r, (r.width() as i32 - w) / 2, y, s, color, scale);
}

#[derive(PartialEq, Clone, Copy)]
enum Focus {
    User,
    Pass,
    Confirm,
    Totp,
}

/// Состояние отображения третьей строки экрана входа: что показывать
/// ниже поля пароля — переключатель LOCAL/CLOUD, поле TOTP-кода или
/// поле пароля диска (после облачного входа на зашифрованном томе).
#[derive(Clone, Copy)]
struct Mode {
    cloud: bool,
    totp: bool,
    disk: bool,
}

/// Перерисовывает экран входа целиком.
fn redraw(
    user: &Field,
    pass: &Field,
    confirm: &Field,
    focus: Focus,
    setup: bool,
    mode: Mode,
    status: &str,
    status_color: Color,
) {
    crate::renderer::with_renderer(|r| {
        let w = r.width() as i32;
        let h = r.height() as i32;
        r.clear(BG);

        let pw = 440u32;
        let ph = if setup { 270u32 } else { 250u32 };
        let px = (w - pw as i32) / 2;
        let py = (h - ph as i32) / 2;

        // Рисуем эмблему/логотип DeiX над панелью входа
        let logo_scale = 1;
        let logo_dim = (crate::bootlogo::size() / 2) as i32 * logo_scale;
        let logo_y = py - logo_dim - 16;
        if logo_y >= 10 {
            crate::bootlogo::draw_logo_div(r, (w - logo_dim) / 2, logo_y, logo_scale, 2);
        }

        r.fill_rect(px, py, pw, ph, PANEL);
        frame(r, px, py, pw, ph, BORDER);

        let title = if setup { "CREATE ACCOUNT" } else { "DeiX LOGIN" };
        text_center(r, py + 20, title, ACCENT, 2);

        let sub = if setup {
            "first run: this password also unlocks the disk"
        } else if mode.cloud {
            "cloud account: email + password"
        } else {
            "enter your credentials"
        };
        text_center(r, py + 50, sub, HINT, 1);

        let fx = px + 30;
        let fw = pw - 60;
        let mut fy = py + 78;

        let draw_field =
            |r: &mut crate::renderer::Renderer, label: &str, f: &Field, active: bool, y: i32| {
                r.draw_text(fx, y, label, HINT, None);
                let by = y + 14;
                r.fill_rect(fx, by, fw, 22, BG);
                frame(r, fx, by, fw, 22, if active { ACCENT } else { BORDER });

                let mut shown = [0u8; MAX_INPUT];
                let n = f.display(&mut shown);
                if let Ok(s) = core::str::from_utf8(&shown[..n]) {
                    r.draw_text(fx + 8, by + 7, s, TEXT, None);
                }
                if active {
                    r.fill_rect(fx + 8 + n as i32 * 8, by + 5, 8, 12, ACCENT);
                }
            };

        // Первый row: USERNAME или EMAIL в облачном режиме.
        let user_label = if mode.cloud { "EMAIL" } else { "USERNAME" };
        draw_field(r, user_label, user, focus == Focus::User, fy);
        fy += 50;
        draw_field(r, "PASSWORD", pass, focus == Focus::Pass, fy);
        if setup {
            fy += 50;
            draw_field(r, "CONFIRM PASSWORD", confirm, focus == Focus::Confirm, fy);
        } else {
            // Третья строка: TOTP-код, пароль диска или переключатель.
            fy += 50;
            if mode.totp {
                draw_field(r, "6-DIGIT CODE", pass, focus == Focus::Totp, fy);
            } else if mode.disk {
                draw_field(r, "DISK PASSWORD", pass, focus == Focus::Pass, fy);
            } else {
                let mode_label = if mode.cloud { "ACCOUNT: CLOUD" } else { "ACCOUNT: LOCAL" };
                let mode_color = if mode.cloud { ACCENT } else { HINT };
                text_center(r, fy + 8, mode_label, mode_color, 1);
                // Подпись «click to switch» слева.
                text_center(r, fy + 26, "click or press C to switch", HINT, 1);
            }
        }

        if !status.is_empty() {
            text_center(r, py + ph as i32 - 34, status, status_color, 1);
        }
        text_center(
            r,
            py + ph as i32 - 14,
            "Tab/mouse - select field   Enter - confirm",
            HINT,
            1,
        );

        // Курсор рисуем последним, чтобы он был поверх панели.
        let m = crate::mouse::snapshot();
        draw_cursor(r, m.x, m.y);

        r.present();
    });
}


/// Рисует курсор мыши: стрелка из закрашенных пикселей.
///
/// Своя отрисовка, а не аппаратный спрайт: Bochs VBE его не
/// поддерживает, а тащить ради курсора отдельный слой композитинга
/// смысла нет.
fn draw_cursor(r: &mut crate::renderer::Renderer, x: i32, y: i32) {
    // Битовая маска стрелки 12x19: 1 — заливка, 2 — контур.
    const ARROW: [&str; 19] = [
        "2           ",
        "22          ",
        "212         ",
        "2112        ",
        "21112       ",
        "211112      ",
        "2111112     ",
        "21111112    ",
        "211111112   ",
        "2111111112  ",
        "21111111112 ",
        "211111122222",
        "21112112    ",
        "2112 2112   ",
        "212  2112   ",
        "22    2112  ",
        "2     2112  ",
        "       222  ",
        "            ",
    ];
    for (dy, row) in ARROW.iter().enumerate() {
        for (dx, ch) in row.bytes().enumerate() {
            let c = match ch {
                b'1' => Color(0xFFFFFF),
                b'2' => Color(0x000000),
                _ => continue,
            };
            r.put_pixel(x + dx as i32, y + dy as i32, c);
        }
    }
}

/// Прямоугольники полей ввода — чтобы понимать, куда кликнули.
/// Значения повторяют раскладку из `redraw`.
struct Layout {
    fx: i32,
    fw: u32,
    user_y: i32,
    pass_y: i32,
    confirm_y: i32,
    third_y: i32,
}

fn layout(r: &crate::renderer::Renderer, setup: bool) -> Layout {
    let w = r.width() as i32;
    let h = r.height() as i32;
    let pw = 440u32;
    let ph = if setup { 270u32 } else { 250u32 };
    let px = (w - pw as i32) / 2;
    let py = (h - ph as i32) / 2;
    Layout {
        fx: px + 30,
        fw: pw - 60,
        user_y: py + 78 + 14,
        pass_y: py + 128 + 14,
        confirm_y: py + 178 + 14,
        third_y: py + 178 + 14,
    }
}

/// Определяет, по какому полю кликнули.
fn hit_test(l: &Layout, setup: bool, mode: Mode, mx: i32, my: i32) -> Option<Focus> {
    let inside = |y: i32| mx >= l.fx && mx < l.fx + l.fw as i32 && my >= y && my < y + 22;
    if inside(l.user_y) {
        Some(Focus::User)
    } else if inside(l.pass_y) {
        Some(Focus::Pass)
    } else if setup && inside(l.confirm_y) {
        Some(Focus::Confirm)
    } else if !setup && !mode.totp && !mode.disk && my >= l.third_y && my < l.third_y + 44 {
        // Третья строка в режиме переключателя — клик переключает
        // LOCAL/CLOUD (или переводит фокус, если это поле TOTP/диска).
        Some(if mode.cloud { Focus::User } else { Focus::User })
    } else if !setup && mode.totp && inside(l.third_y) {
        Some(Focus::Totp)
    } else if !setup && mode.disk && inside(l.third_y) {
        Some(Focus::Pass)
    } else {
        None
    }
}

/// Y-координата строки переключателя LOCAL/CLOUD (центр панели).
fn mode_toggle_y(r: &crate::renderer::Renderer) -> i32 {
    let h = r.height() as i32;
    let py = (h - 250i32) / 2;
    py + 178
}

/// Читает байт из PS/2 или COM1. `None`, если ввода нет.
fn poll_key() -> Option<u8> {
    if crate::serial::is_data_ready() {
        Some(crate::serial::read_byte())
    } else {
        crate::keyboard::try_read_char()
    }
}

/// Статус диска после успешного облачного входа.
enum CloudDisk {
    /// Диск зашифрован, разблокирован cloud-паролем (он уже в LUKS
    /// с предыдущего входа) или разблокирован успешно.
    Unlocked,
    /// Диск зашифрован, cloud-пароль не в LUKS — нужен пароль диска.
    NeedPassword,
    /// Диск не зашифрован — разблокировка не нужна.
    Plain,
}

/// Проверяет, разблокирован ли том после облачного входа. Добавляет
/// cloud-пароль в LUKS (работает, только если том уже разблокирован,
/// т.е. cloud-пароль был добавлен в предыдущий вход). Проверяет
/// разблокировку cloud-паролем. Если и то, и другое не удалось —
/// диск зашифрован другим паролем, нужен пароль диска.
fn cloud_disk_status(password: &str) -> CloudDisk {
    if crate::crypto_storage::is_encryption_enabled() {
        let _ = crate::luks::add_password_with_master(password);
        if crate::crypto_storage::try_unlock(password) {
            CloudDisk::Unlocked
        } else {
            CloudDisk::NeedPassword
        }
    } else {
        CloudDisk::Plain
    }
}

/// Экран входа / создания первой учётки.
///
/// Возвращает имя пользователя. `None` — графики нет, вызывающий должен
/// откатиться на текстовый вариант.
pub fn run() -> Option<String> {
    crate::renderer::ensure_framebuffer();
    if !crate::renderer::has_active_framebuffer() {
        return None;
    }

    // Страховка: экран целиком построен на ожидании прерываний
    // (клавиатура, мышь, таймер). Если IF почему-то сброшен, hlt ниже
    // заснёт навсегда — включаем прерывания явно.
    unsafe { core::arch::asm!("sti") };

    let setup = !crate::auth::has_any_users();
    crate::serial_println!("[loginui] графический вход, создание учётки: {}", setup);

    let mut user = Field::new(false);
    let mut pass = Field::new(true);
    let mut confirm = Field::new(true);
    let mut focus = Focus::User;
    let mut status = String::new();
    let mut status_color = HINT;

    // Облачный режим: cloud_mode — активен переключатель,
    // totp_pending — ожидание 6-значного кода, disk_unlock —
    // ожидание пароля диска после облачного входа на зашифрованном
    // томе (cloud-пароль ещё не в LUKS, нужен старый пароль диска,
    // чтобы разблокировать том и добавить cloud-пароль в слоты).
    let mut cloud_mode = false;
    let mut totp_pending = false;
    let mut disk_unlock = false;
    let mut cloud_password = String::new();
    let mut cloud_email = String::new();
    let mut totp_factor = String::new();
    let mut totp_token = String::new();
    let mut cloud_local = String::new();

    redraw(
        &user, &pass, &confirm, focus, setup,
        Mode { cloud: cloud_mode, totp: totp_pending, disk: disk_unlock },
        &status, status_color,
    );

    // Предыдущее состояние мыши: перерисовываем экран только когда
    // курсор реально сдвинулся, иначе 60 кадров в секунду впустую.
    let mut last_mouse = (-1i32, -1i32);
    let mut last_click = false;

    loop {
        let mode = Mode { cloud: cloud_mode, totp: totp_pending, disk: disk_unlock };

        let c = match poll_key() {
            Some(c) => c,
            None => {
                // Клавиш нет — обслуживаем мышь.
                let m = crate::mouse::snapshot();
                let moved = (m.x, m.y) != last_mouse;

                // Клик по полю переводит на него фокус. Клик по строке
                // переключателя (LOCAL/CLOUD) включает облачный режим.
                if m.left_button && !last_click {
                    let hit = crate::renderer::with_renderer_ret(|r| {
                        let l = layout(r, setup);
                        hit_test(&l, setup, mode, m.x, m.y)
                    })
                    .flatten();
                    if let Some(f) = hit {
                        focus = f;
                    } else {
                        // Клик на строке переключателя LOCAL/CLOUD.
                        let toggle_y = crate::renderer::with_renderer_ret(|r| mode_toggle_y(r))
                            .unwrap_or(0);
                        if !setup && !totp_pending && !disk_unlock
                            && m.y >= toggle_y && m.y < toggle_y + 44
                        {
                            cloud_mode = !cloud_mode;
                            user.clear();
                            pass.clear();
                            focus = Focus::User;
                            status = String::from("enter cloud email + password");
                            status_color = HINT;
                        }
                    }
                }
                last_click = m.left_button;

                if moved || m.left_button {
                    last_mouse = (m.x, m.y);
                    let mode = Mode { cloud: cloud_mode, totp: totp_pending, disk: disk_unlock };
                    redraw(&user, &pass, &confirm, focus, setup, mode, &status, status_color);
                } else if !crate::bootchain::pump() {
                    // Порция отложенной сверки ядра обработана — сразу
                    // опрашиваем ввод снова. Если делать нечего: hlt, спим
                    // до прерывания, не жжём процессор впустую.
                    crate::sched::idle_wait();
                }
                continue;
            }
        };

        let mut dirty = true;
        match c {
            b'\t' => {
                focus = match (focus, setup, totp_pending) {
                    (Focus::User, _, _) => Focus::Pass,
                    (Focus::Pass, true, _) => Focus::Confirm,
                    (Focus::Pass, false, true) => Focus::Totp,
                    (Focus::Pass, false, false) => Focus::User,
                    (Focus::Confirm, _, _) => Focus::User,
                    (Focus::Totp, _, _) => Focus::User,
                };
            }
            b'\n' | b'\r' => {
                let last = if setup {
                    Focus::Confirm
                } else if totp_pending {
                    Focus::Totp
                } else {
                    Focus::Pass
                };
                if focus != last {
                    focus = match (focus, setup, totp_pending) {
                        (Focus::User, _, _) => Focus::Pass,
                        (Focus::Pass, true, _) => Focus::Confirm,
                        (Focus::Pass, false, true) => Focus::Totp,
                        _ => Focus::User,
                    };
                } else if setup {
                    if user.len == 0 || pass.len == 0 {
                        status = String::from("username and password required");
                        status_color = ERR;
                    } else if pass.as_str() != confirm.as_str() {
                        status = String::from("passwords do not match");
                        status_color = ERR;
                        pass.clear();
                        confirm.clear();
                        focus = Focus::Pass;
                    } else {
                        match crate::auth::create_user(user.as_str(), pass.as_str()) {
                            Ok(()) => {
                                crate::bootprof::mark("login: account record created");
                                // ПЕРВАЯ НАСТРОЙКА: включаем шифрование тома
                                // паролем аккаунта. Без этого вызова том
                                // оставался ОТКРЫТЫМ — USERS.DB и все файлы
                                // читались обычным 7-Zip прямо из образа.
                                // Текстовый auth::run_login_screen это делал,
                                // а графический вход — нет.
                                status = String::from("encrypting disk, wait...");
                                redraw(&user, &pass, &confirm, focus, setup, mode,
                                       &status, status_color);
                                match crate::crypto_storage::enable_encryption(pass.as_str()) {
                                    Ok(()) => crate::serial_println!(
                                        "[loginui] шифрование тома включено (XTS-AES-256)"),
                                    Err(_) => crate::serial_println!(
                                        "[loginui] ВНИМАНИЕ: шифрование НЕ включено (ошибка диска)"),
                                }
                                crate::bootprof::mark("login: volume encryption enabled");
                                status = String::from("account created, signing in...");
                                status_color = OK_C;
                                redraw(
                                    &user, &pass, &confirm, focus, setup, mode, &status, status_color,
                                );
                                return Some(String::from(user.as_str()));
                            }
                            Err(e) => {
                                status = String::from(match e {
                                    crate::auth::AuthError::UserAlreadyExists => {
                                        "user already exists"
                                    }
                                    crate::auth::AuthError::InvalidUsername => "invalid username",
                                    crate::auth::AuthError::DiskError => "disk error",
                                    crate::auth::AuthError::CorruptDatabase => {
                                        "user database corrupt"
                                    }
                                    _ => "cannot create account",
                                });
                                status_color = ERR;
                            }
                        }
                    }
                } else if cloud_mode && totp_pending {
                    // Второй этап 2FA: 6-значный код из приложения.
                    let code = String::from(pass.as_str());
                    match crate::cloudauth::finish_totp(&totp_token, &totp_factor, &code) {
                        Ok((username, user_id)) => {
                            match crate::cloudauth::cache_login(&username, &cloud_password, &cloud_email, &user_id) {
                                Ok(local_username) => {
                                    match cloud_disk_status(&cloud_password) {
                                        CloudDisk::Unlocked | CloudDisk::Plain => {
                                            crate::bootprof::mark("login: cloud verified");
                                            status = String::from("welcome!");
                                            status_color = OK_C;
                                            let mode = Mode { cloud: true, totp: false, disk: false };
                                            redraw(&user, &pass, &confirm, focus, setup, mode, &status, status_color);
                                            return Some(local_username);
                                        }
                                        CloudDisk::NeedPassword => {
                                            cloud_local = local_username;
                                            disk_unlock = true;
                                            totp_pending = false;
                                            pass.clear();
                                            focus = Focus::Pass;
                                            status = String::from("cloud verified - enter disk password");
                                            status_color = HINT;
                                        }
                                    }
                                }
                                Err(e) => {
                                    status = e;
                                    status_color = ERR;
                                    totp_pending = false;
                                    pass.clear();
                                }
                            }
                        }
                        Err(e) => {
                            status = e;
                            status_color = ERR;
                            pass.clear();
                        }
                    }
                } else if cloud_mode && disk_unlock {
                    // Пароль диска: разблокируем том, затем добавим
                    // cloud-пароль в LUKS (теперь том разблокирован).
                    if crate::crypto_storage::try_unlock(pass.as_str()) {
                        let _ = crate::luks::add_password_with_master(&cloud_password);
                        crate::bootprof::mark("login: cloud password synced to LUKS");
                        status = String::from("welcome!");
                        status_color = OK_C;
                        disk_unlock = false;
                        let mode = Mode { cloud: cloud_mode, totp: totp_pending, disk: disk_unlock };
                        redraw(&user, &pass, &confirm, focus, setup, mode, &status, status_color);
                        return Some(cloud_local);
                    } else {
                        status = String::from("invalid disk password");
                        status_color = ERR;
                        pass.clear();
                    }
                } else if cloud_mode {
                    // Первый этап облачного входа: email + пароль.
                    let email = String::from(user.as_str());
                    let password = String::from(pass.as_str());
                    if email.is_empty() || password.is_empty() {
                        status = String::from("email and password required");
                        status_color = ERR;
                    } else {
                        match crate::cloudauth::begin(&email, &password) {
                            Ok(crate::cloudauth::CloudStep::Done { username, user_id }) => {
                                match crate::cloudauth::cache_login(&username, &password, &email, &user_id) {
                                    Ok(local_username) => {
                                        match cloud_disk_status(&password) {
                                            CloudDisk::Unlocked | CloudDisk::Plain => {
                                                crate::bootprof::mark("login: cloud verified");
                                                status = String::from("welcome!");
                                                status_color = OK_C;
                                                let mode = Mode { cloud: true, totp: false, disk: false };
                                                redraw(&user, &pass, &confirm, focus, setup, mode, &status, status_color);
                                                return Some(local_username);
                                            }
                                            CloudDisk::NeedPassword => {
                                                cloud_local = local_username;
                                                cloud_password = password;
                                                disk_unlock = true;
                                                pass.clear();
                                                focus = Focus::Pass;
                                                status = String::from("cloud verified - enter disk password");
                                                status_color = HINT;
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        status = e;
                                        status_color = ERR;
                                    }
                                }
                            }
                            Ok(crate::cloudauth::CloudStep::NeedTotp { factor_id, access_token }) => {
                                cloud_password = password;
                                cloud_email = email;
                                totp_factor = factor_id;
                                totp_token = access_token;
                                totp_pending = true;
                                pass.clear();
                                focus = Focus::Totp;
                                status = String::from("enter 6-digit code from app");
                                status_color = HINT;
                            }
                            Err(e) => {
                                status = e;
                                status_color = ERR;
                                pass.clear();
                            }
                        }
                    }
                } else {
                    // Зашифрованный том сначала надо РАЗБЛОКИРОВАТЬ: пока
                    // XTS-движок не активирован верным паролем, ext2 читает
                    // мусор и verify_login гарантированно провалится.
                    if crate::crypto_storage::is_encryption_enabled()
                        && !crate::crypto_storage::try_unlock(pass.as_str())
                    {
                        status = String::from("invalid username or password");
                        status_color = ERR;
                        pass.clear();
                        focus = Focus::Pass;
                        let _ = crate::sound::play_ui(crate::sound::UiSound::Error);
                        redraw(&user, &pass, &confirm, focus, setup, mode, &status, status_color);
                        continue;
                    }
                    crate::bootprof::mark("login: volume unlocked");
                    match crate::auth::verify_login(user.as_str(), pass.as_str()) {
                        Ok(()) => {
                            crate::bootprof::mark("login: password verified");
                            status = String::from("welcome!");
                            status_color = OK_C;
                            redraw(&user, &pass, &confirm, focus, setup, mode, &status, status_color);
                            return Some(String::from(user.as_str()));
                        }
                        Err(_) => {
                            // Одно сообщение и для неверного имени, и для
                            // пароля: не подсказываем, что именно не так.
                            status = String::from("invalid username or password");
                            status_color = ERR;
                            pass.clear();
                            focus = Focus::Pass;
                            let _ = crate::sound::play_ui(crate::sound::UiSound::Error);
                        }
                    }
                }
            }
            0x08 | 0x7F => match focus {
                Focus::User => user.pop(),
                Focus::Pass => pass.pop(),
                Focus::Confirm => confirm.pop(),
                Focus::Totp => pass.pop(),
            },
            0x20..=0x7E => match focus {
                Focus::User => user.push(c),
                Focus::Pass => pass.push(c),
                Focus::Confirm => confirm.push(c),
                Focus::Totp => pass.push(c),
            },
            _ => dirty = false,
        }

        if dirty {
            redraw(
                &user, &pass, &confirm, focus, setup,
                Mode { cloud: cloud_mode, totp: totp_pending, disk: disk_unlock },
                &status, status_color,
            );
        }
    }
}

/// Возвращает текстовый режим VGA после графического экрана.
///
/// Без этого картинка остаётся на мониторе навсегда, а CLI пишет в
/// невидимый текстовый буфер поверх графики — именно так выглядела
/// жалоба «лого не пропадает».
pub fn back_to_text() {
    crate::vbe::restore_text_mode();
    crate::renderer::reset_renderer();

    // После графики в видеопамяти лежит мусор от framebuffer'а.
    // Сначала очищаем текстовый буфер 0xB8000 (80x25, пробелы 0x0F20),
    // затем восстанавливаем полный шрифт со всеми 256 глифами
    // (restore_full_font вместо только кириллицы install_cyrillic_font).
    unsafe {
        let vram = 0xB8000 as *mut u16;
        for i in 0..80 * 25 {
            core::ptr::write_volatile(vram.add(i), 0x0F20); // серый на чёрном
        }
    }

    crate::font::restore_full_font();
    crate::vgaglobal::early_init_writer();
}

/// ЭКРАН БЛОКИРОВКИ: ждёт пароль текущего пользователя.
/// Возвращает управление только после верного пароля.
pub fn lock(username: &str) {
    crate::renderer::ensure_framebuffer();
    if !crate::renderer::has_active_framebuffer() {
        crate::println!("  [lock] графика недоступна — блокировка невозможна");
        return;
    }

    unsafe { core::arch::asm!("sti") };

    let mut pass = Field::new(true);
    let mut status = String::new();

    loop {
        crate::renderer::with_renderer(|r| {
            let w = r.width() as i32;
            let h = r.height() as i32;
            r.clear(BG);

            text_center(r, h / 2 - 130, "LOCKED", ACCENT, 3);
            text_center(r, h / 2 - 85, username, TEXT, 2);

            let fw = 360u32;
            let fx = (w - fw as i32) / 2;
            let by = h / 2 - 10;
            r.fill_rect(fx, by, fw, 24, PANEL);
            frame(r, fx, by, fw, 24, ACCENT);

            let mut shown = [0u8; MAX_INPUT];
            let n = pass.display(&mut shown);
            if let Ok(s) = core::str::from_utf8(&shown[..n]) {
                r.draw_text(fx + 8, by + 8, s, TEXT, None);
            }
            r.fill_rect(fx + 8 + n as i32 * 8, by + 6, 8, 12, ACCENT);

            if !status.is_empty() {
                text_center(r, by + 44, &status, ERR, 1);
            }
            text_center(r, h - 44, "enter password to unlock", HINT, 1);
            r.present();
        });

        let c = match poll_key() {
            Some(c) => c,
            None => {
                crate::sched::idle_wait();
                continue;
            }
        };

        match c {
            b'\n' | b'\r' => match crate::auth::verify_login(username, pass.as_str()) {
                Ok(()) => return,
                Err(_) => {
                    status = String::from("wrong password");
                    pass.clear();
                }
            },
            0x08 | 0x7F => pass.pop(),
            0x20..=0x7E => pass.push(c),
            _ => {}
        }
    }
}

/// CLI: `lock` — заблокировать экран.
pub fn cmd_lock(current_user: &str) {
    lock(current_user);
    back_to_text();
    crate::println!("  Экран разблокирован.");
}
