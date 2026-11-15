//! Stub для erofs — EROFS filesystem (заглушка).

pub fn build_image(_files: &[(&str, &[u8])]) -> Vec<u8> {
    Vec::new()
}

pub fn is_erofs(_data: &[u8]) -> bool {
    false
}

pub fn read_file(_image: &[u8], _path: &str) -> Result<Vec<u8>, String> {
    Err(String::from("erofs: not available"))
}

pub fn parse_superblock(_image: &[u8]) -> Result<Superblock, String> {
    Err(String::from("erofs: not available"))
}

pub fn stat(_image: &[u8], _path: &str) -> Result<Stat, String> {
    Err(String::from("erofs: not available"))
}

pub fn list_dir(_image: &[u8], _path: &str) -> Result<Vec<FileEntry>, String> {
    Err(String::from("erofs: not available"))
}

pub enum ErofsError {
    NotFound,
    Io,
    Corrupt,
}

pub struct Superblock;
pub struct Stat;
pub struct FileEntry;
