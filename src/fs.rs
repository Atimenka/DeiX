//! FS + DTI: system files visible but read-only for non-DTI.
use crate::ext2;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Permissions(pub u8);
impl Permissions {
    pub const RW: Self = Permissions(6);
    pub const RWX: Self = Permissions(7);
}

#[derive(Debug, Clone)]
pub struct FileMeta {
    pub name: String, pub is_dir: bool, pub size: u64,
    pub owner: String, pub group: String, pub perms: Permissions,
    pub system: bool, pub created_at: u64,
}

impl FileMeta {
    pub fn format_entry(&self) -> String {
        let kind = if self.is_dir { "d" } else { "f" };
        let sys = if self.system { "sys" } else { "usr" };
        let perm_str = match self.perms {
            Permissions::RWX => "rwx",
            Permissions::RW => "rw-",
            _ => "r--",
        };
        format!("{}{} [{}] {:<8} {:<8} {:>8} B @{} {}", kind, perm_str, sys, self.owner, self.group, self.size, self.created_at, self.name)
    }
}

static META_DB: crate::spinlock::SpinLock<BTreeMap<String, FileMeta>> = crate::spinlock::SpinLock::new(BTreeMap::new());
static CUR_USER: crate::spinlock::SpinLock<String> = crate::spinlock::SpinLock::new(String::new());


/// Сброс «живых» глобалов ФС к начальному состоянию (используется install
/// перед копированием .data на целевой диск: BTreeMap/String содержат
/// указатели на кучу, которые недействительны на установленной системе).
pub fn reset_fs_state() {
    META_DB.lock().clear();
    *CUR_USER.lock() = String::new();
}

pub fn load_meta_db() {
    let raw = match ext2::read_file("FILES.DB") { Ok(d) => d, Err(_) => return };
    let text = match core::str::from_utf8(&raw) { Ok(t) => t, Err(_) => return };
    let mut db = META_DB.lock(); db.clear();
    for line in text.lines() { let line = line.trim(); if line.is_empty()||line.starts_with('#'){continue} let p:Vec<&str>=line.split('|').collect(); if p.len()<7{continue} db.insert(p[0].into(), FileMeta{name:p[0].into(),is_dir:p[1]=="d",size:p[2].parse().unwrap_or(0),owner:p[3].into(),group:p[4].into(),perms:Permissions(p[5].parse().unwrap_or(0)),system:p[6]=="sys",created_at:p.get(7).and_then(|s|s.parse().ok()).unwrap_or(0)}); }
}

pub fn list_dir() -> Vec<FileMeta> { META_DB.lock().values().cloned().collect() }

pub fn list_meta_entries() -> Vec<String> {
    list_dir().iter().map(|m| m.format_entry()).collect()
}


