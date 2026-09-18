//! Централизованная авторизация и матрица доступа к файлам и ресурсам
//!
//! Обеспечивает выполнение ключевых правил безопасности DeiX OS:
//! 1. Защита TPM: раздел /TPM и ключ шифрования закрыты АБСОЛЮТНО для всех (включая UID 0).
//! 2. Защита системных путей: системные разделы (/kernel, /system, /init_boot, /boot)
//!    доступны только на чтение/исполнение и не могут быть модифицированы из Ring 3.
//! 3. Изоляция пользовательских каталогов: пользователь имеет полный доступ
//!    только к своему домашнему каталогу (/users/<username>) и временным файлам (/tmp).

#![allow(dead_code)]

use alloc::format;
use alloc::string::String;

/// Типы файловых операций, подлежащих контролю доступа
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOp {
    Read,
    Write,
    Execute,
    Delete,
    Create,
    Chmod,
    Chown,
}

impl FileOp {
    pub fn is_modifying(&self) -> bool {
        matches!(
            self,
            FileOp::Write | FileOp::Delete | FileOp::Create | FileOp::Chmod | FileOp::Chown
        )
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            FileOp::Read => "READ",
            FileOp::Write => "WRITE",
            FileOp::Execute => "EXEC",
            FileOp::Delete => "DELETE",
            FileOp::Create => "CREATE",
            FileOp::Chmod => "CHMOD",
            FileOp::Chown => "CHOWN",
        }
    }
}

/// Ошибки проверки прав доступа
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessError {
    /// Отказано в доступе (недостаточно привилегий)
    PermissionDenied,
    /// Доступ категорически заблокирован KERNEL SECURITY VAULT
    VaultProtected,
    /// Попытка записи в файловую систему, смонтированную только для чтения
    ReadOnlyFilesystem,
    /// Некорректный или потенциально опасный путь (например, path traversal "../")
    InvalidPath,
}

impl AccessError {
    pub fn as_str(&self) -> &'static str {
        match self {
            AccessError::PermissionDenied => "EACCES: Permission denied",
            AccessError::VaultProtected => "EPERM: Kernel Security Vault violation",
            AccessError::ReadOnlyFilesystem => "EROFS: Read-only file system",
            AccessError::InvalidPath => "EINVAL: Invalid file path",
        }
    }
}

/// Проверка прав доступа к файловому пути
pub fn check_permission(
    uid: u32,
    username: &str,
    op: FileOp,
    path: &str,
) -> Result<(), AccessError> {
    // 0. Защита от path traversal атак (../)
    if path.contains("..") {
        return Err(AccessError::InvalidPath);
    }

    // 1. АБСОЛЮТНАЯ ЗАЩИТА TPM:
    // Раздел /tpm и любые его подкаталоги аппаратно запечатаны и недоступны НИКОМУ,
    // включая суперпользователя root (UID 0).
    if path.starts_with("/tpm") || path.starts_with("/TPM") {
        return Err(AccessError::VaultProtected);
    }

    // 2. ЗАЩИТА СИСТЕМНЫХ РАЗДЕЛОВ:
    // /kernel, /init_boot, /system, /boot, /vendor_boot защищены от модификации
    if op.is_modifying() {
        if path.starts_with("/kernel")
            || path.starts_with("/init_boot")
            || path.starts_with("/system")
            || path.starts_with("/boot")
            || path.starts_with("/vendor_boot")
            || path.starts_with("/super")
        {
            return Err(AccessError::ReadOnlyFilesystem);
        }
    }

    // 3. АДМИНИСТРАТОР (UID 0 / root):
    // Имеет полный доступ ко всей остальной файловой системе
    if uid == 0 {
        return Ok(());
    }

    // 4. ОБЫЧНЫЕ ПОЛЬЗОВАТЕЛИ (UID >= 1000):
    // Доступ к временному каталогу /tmp
    if path.starts_with("/tmp") {
        return Ok(());
    }

    // Полный доступ к собственному домашнему каталогу /users/<username>
    let user_home = format!("/users/{}", username);
    if path.starts_with(&user_home) {
        return Ok(());
    }

    // Запрет доступа к чужим домашним каталогам
    if path.starts_with("/users/") {
        return Err(AccessError::PermissionDenied);
    }

    // Чтение и исполнение общесистемных бинарников и библиотек
    if !op.is_modifying() {
        if path.starts_with("/system")
            || path.starts_with("/bin")
            || path.starts_with("/lib")
            || path.starts_with("/etc")
            || path == "/"
        {
            return Ok(());
        }
    }

    // Любые иные попытки записи или доступа к системным ресурсам пресекаются
    Err(AccessError::PermissionDenied)
}
