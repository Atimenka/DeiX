//! ПРОФИЛИ ПОЛЬЗОВАТЕЛЕЙ — разделение файлов и настроек по владельцам.
//!
//! Раньше все файлы лежали в одной куче на ext2-томе: любой аккаунт
//! видел и мог перезаписать чужие данные. Здесь появляется владелец.
//!
//! ## Настоящие каталоги
//!
//! Файлы лежат в реальной иерархии ext2:
//!
//! ```text
//!   /users/<имя>/files/notes.txt
//!   /users/<имя>/configs/wm.cfg
//! ```
//!
//! Менять файловую систему на ext4 или NTFS для этого не понадобилось:
//! ext2 поддерживает подкаталоги с самого начала (в томе всегда был
//! настоящий каталог `lost+found`). Не хватало лишь публичного API —
//! он добавлен в `ext2.rs`: `mkdir_in`, `mkdir_p`, `resolve_parent`,
//! `list_dir_path`.
//!
//! Ограничение остаётся честным: это разделение по владельцам, а не
//! изоляция уровня ядра. Пока весь код исполняется в Ring 0, любая
//! часть системы видит весь том — настоящие права доступа появятся
//! только с переносом программ в Ring 3.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::ext2;

/// Корень пользовательских профилей.
const USERS_ROOT: &str = "users";
/// Максимальная длина компонента пути.
const MAX_NAME: usize = 40;

/// Какая область профиля используется.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Area {
    /// `/users/<имя>/files` — документы пользователя.
    Files,
    /// `/users/<имя>/configs` — настройки приложений и системы.
    Configs,
}

impl Area {
    pub fn as_path(&self) -> &'static str {
        match self {
            Area::Files => "files",
            Area::Configs => "configs",
        }
    }
}

/// Ошибки работы с профилем.
#[derive(Debug)]
pub enum UserFsError {
    /// Имя содержит разделитель или недопустимо.
    BadName,
    /// Слишком длинное имя.
    TooLong,
    /// Файл не найден.
    NotFound,
    /// Ошибка тома.
    Disk,
}

impl UserFsError {
    pub fn message(&self) -> &'static str {
        match self {
            UserFsError::BadName => "недопустимое имя (символ '~' запрещён)",
            UserFsError::TooLong => "слишком длинное имя",
            UserFsError::NotFound => "файл не найден",
            UserFsError::Disk => "ошибка диска",
        }
    }
}

/// Проверяет, что компонент пути безопасен.
///
/// Запрещаем разделители и `..`: иначе пользователь вышел бы из своего
/// каталога и залез в чужой профиль (path traversal).
fn valid_component(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_NAME
        && s != "."
        && s != ".."
        && !s.contains('/')
        && !s.contains('\\')
}

/// Полный путь файла в профиле пользователя.
pub fn physical_name(user: &str, area: Area, name: &str) -> Result<String, UserFsError> {
    if !valid_component(user) || !valid_component(name) {
        return Err(UserFsError::BadName);
    }
    Ok(format!("/{}/{}/{}/{}", USERS_ROOT, user, area.as_path(), name))
}

/// Путь каталога области профиля.
fn area_dir(user: &str, area: Area) -> String {
    format!("/{}/{}/{}", USERS_ROOT, user, area.as_path())
}

/// Человекочитаемый путь — для вывода пользователю.
pub fn display_path(user: &str, area: Area, name: &str) -> String {
    format!("/users/{}/{}/{}", user, area.as_path(), name)
}

/// Записывает файл в профиль пользователя.
pub fn write(user: &str, area: Area, name: &str, data: &[u8]) -> Result<(), UserFsError> {
    let phys = physical_name(user, area, name)?;
    // Каталоги создаются автоматически (аналог mkdir -p).
    ext2::write_file_path(&phys, data).map_err(|_| UserFsError::Disk)
}

/// Читает файл из профиля пользователя.
pub fn read(user: &str, area: Area, name: &str) -> Result<Vec<u8>, UserFsError> {
    let phys = physical_name(user, area, name)?;
    ext2::read_file_path(&phys).map_err(|_| UserFsError::NotFound)
}

/// Удаляет файл из профиля.
pub fn remove(user: &str, area: Area, name: &str) -> Result<(), UserFsError> {
    let phys = physical_name(user, area, name)?;
    ext2::delete_file_path(&phys).map_err(|_| UserFsError::NotFound)
}

/// Список файлов пользователя в указанной области.
/// Возвращает «короткие» имена, без служебного префикса.
pub fn list(user: &str, area: Area) -> Result<Vec<String>, UserFsError> {
    if !valid_component(user) {
        return Err(UserFsError::BadName);
    }
    let dir = area_dir(user, area);
    let entries = match ext2::list_dir_path(&dir) {
        Ok(e) => e,
        // Каталога ещё нет — профиль пуст, это не ошибка.
        Err(_) => return Ok(Vec::new()),
    };
    Ok(entries.into_iter().map(|e| e.name).collect())
}

/// Все пользователи, у которых есть хоть один файл на томе.
pub fn known_profiles() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Ok(entries) = ext2::list_dir_path("/users") {
        for e in entries {
            if e.is_directory {
                out.push(e.name);
            }
        }
    }
    out
}

/// Создаёт профиль: каталоги /users/<имя>/{files,configs} и стартовый
/// конфиг внутри.
pub fn init_profile(user: &str) -> Result<(), UserFsError> {
    if !valid_component(user) {
        return Err(UserFsError::BadName);
    }
    // Создаём саму иерархию каталогов.
    let _ = ext2::mkdir_p(&area_dir(user, Area::Files));
    let _ = ext2::mkdir_p(&area_dir(user, Area::Configs));

    // Не перетираем уже существующий профиль.
    if read(user, Area::Configs, "profile.cfg").is_ok() {
        return Ok(());
    }
    let greeting = format!(
        "# Профиль пользователя {}\n# Файлы:    /users/{}/files\n# Настройки: /users/{}/configs\n",
        user, user, user
    );
    write(user, Area::Configs, "profile.cfg", greeting.as_bytes())
}

// ==================== CLI ====================

/// `profile [ls|cat|write|rm|users]`
pub fn cmd_profile(arg: &str, current_user: &str) {
    if current_user.is_empty() {
        crate::println!("  [profile] нет активного пользователя");
        return;
    }

    let mut it = arg.trim().splitn(3, ' ');
    let sub = it.next().unwrap_or("");
    let a1 = it.next().unwrap_or("").trim();
    let a2 = it.next().unwrap_or("").trim();

    // Область по умолчанию — files; configs выбирается префиксом "cfg".
    let (area, name) = if let Some(rest) = a1.strip_prefix("cfg:") {
        (Area::Configs, rest)
    } else {
        (Area::Files, a1)
    };

    match sub {
        "ls" => {
            for ar in [Area::Files, Area::Configs] {
                match list(current_user, ar) {
                    Ok(files) => {
                        crate::println!("  /users/{}/{}:", current_user, ar.as_path());
                        if files.is_empty() {
                            crate::println!("    (пусто)");
                        }
                        for f in files {
                            crate::println!("    {}", f);
                        }
                    }
                    Err(e) => crate::println!("  ошибка: {}", e.message()),
                }
            }
        }
        "cat" if !name.is_empty() => match read(current_user, area, name) {
            Ok(d) => match core::str::from_utf8(&d) {
                Ok(t) => crate::println!("{}", t),
                Err(_) => crate::println!("  (двоичный файл, {} байт)", d.len()),
            },
            Err(e) => crate::println!("  {}", e.message()),
        },
        "write" if !name.is_empty() => match write(current_user, area, name, a2.as_bytes()) {
            Ok(()) => crate::println!("  записано: {}", display_path(current_user, area, name)),
            Err(e) => crate::println!("  ошибка: {}", e.message()),
        },
        "rm" if !name.is_empty() => match remove(current_user, area, name) {
            Ok(()) => crate::println!("  удалено: {}", display_path(current_user, area, name)),
            Err(e) => crate::println!("  ошибка: {}", e.message()),
        },
        "users" => {
            let profiles = known_profiles();
            crate::println!("  Профили на диске:");
            if profiles.is_empty() {
                crate::println!("    (нет)");
            }
            for p in profiles {
                let mark = if p == current_user { " <- вы" } else { "" };
                crate::println!("    /users/{}{}", p, mark);
            }
        }
        _ => {
            crate::println!("profile ls                  - файлы и настройки текущего пользователя");
            crate::println!("profile cat <файл>          - показать файл (cfg:<имя> — из configs)");
            crate::println!("profile write <файл> <текст> - записать файл");
            crate::println!("profile rm <файл>           - удалить файл");
            crate::println!("profile users               - список профилей на диске");
        }
    }
}
