//! EROFS filesystem (заглушка).
//!
//! Полноценный риддер erofs не реализован — для нужд ядра заглушка
//! возвращает ошибки, что позволяет vfs.rs корректно обрабатывать
//! отсутствие файловой системы.

use alloc::vec::Vec;

pub enum ErofsError {
    NotFound,
    Io,
    Corrupt,
}

impl ErofsError {
    pub fn message(&self) -> &'static str {
        match self {
            ErofsError::NotFound => "not found",
            ErofsError::Io => "io error",
            ErofsError::Corrupt => "corrupt filesystem",
        }
    }
}

pub struct Superblock;

pub struct Stat {
    pub size: u32,
}

impl Stat {
    pub fn is_dir(&self) -> bool { false }
}

pub struct FileEntry {
    pub name: alloc::string::String,
    pub is_dir: bool,
    pub size: u32,
}

pub fn build_image(_files: &[(&str, &[u8])]) -> Vec<u8> {
    Vec::new()
}

pub fn is_erofs(_data: &[u8]) -> bool {
    false
}

pub fn read_file(_image: &[u8], _path: &str) -> Result<Vec<u8>, ErofsError> {
    Err(ErofsError::Io)
}

pub fn parse_superblock(_image: &[u8]) -> Result<Superblock, ErofsError> {
    Err(ErofsError::Io)
}

pub fn stat(_image: &[u8], _path: &str) -> Result<Stat, ErofsError> {
    Err(ErofsError::NotFound)
}

pub fn list_dir(_image: &[u8], _path: &str) -> Result<Vec<FileEntry>, ErofsError> {
    Err(ErofsError::NotFound)
}
