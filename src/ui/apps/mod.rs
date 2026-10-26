//! Модули графических приложений DeiX OS

pub mod about;
pub mod browser;
pub mod error_center;
pub mod files;
pub mod game2048;
pub mod installer;
/// Медиа-приложения (фото/видео/музыка). Осознанно без `pub use media::*`:
/// имена констант геометрии (ROW_H и т.п.) пересеклись бы с files::ROW_H.
pub mod media;
pub mod settings;
pub mod task_manager;
pub mod terminal;
pub mod updates;

pub use about::*;
pub use browser::*;
pub use installer::draw_installer;
pub use error_center::*;
pub use files::*;
pub use settings::*;
pub use task_manager::*;
pub use terminal::*;
