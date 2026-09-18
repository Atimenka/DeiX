//! Централизованная авторизация действий в Dinit
//!
//! Проверка прав пользователей и изоляция каталогов (/users/<name>/ vs /system).

use super::audit::{AuditOp, AuditResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOp {
    Read,
    Write,
    Execute,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessError {
    PermissionDenied,
    VaultProtected,
    InvalidPath,
}

/// Центральная проверка прав доступа
pub fn check_permission(uid: u32, username: &str, op: FileOp, path: &str) -> Result<(), AccessError> {
    // 1. Защита TPM: раздел /TPM закрыт АБСОЛЮТНО ДЛЯ ВСЕХ (включая UID 0)
    if path.starts_with("/tpm") || path.starts_with("/TPM") {
        return Err(AccessError::VaultProtected);
    }

    // 2. Администратор (root / UID 0) имеет полный доступ ко всей системе
    if uid == 0 {
        return Ok(());
    }

    // 3. Обычные пользователи:
    // Полный доступ к своему каталогу /users/<username>
    let user_home_prefix = alloc::format!("/users/{}", username);
    if path.starts_with(&user_home_prefix) {
        return Ok(());
    }

    // Чтение системных библиотек и бинарников
    if (op == FileOp::Read || op == FileOp::Execute) && (path.starts_with("/system") || path.starts_with("/bin")) {
        return Ok(());
    }

    // Попытка записи в системный каталог или чужой профиль запрещена
    Err(AccessError::PermissionDenied)
}
