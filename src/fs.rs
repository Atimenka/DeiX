//! FS + DTI: system files visible but read-only for non-DTI.
use crate::ext2;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Permissions(pub u8);
impl Permissions {
    pub const R: Self = Permissions(4); pub const RW: Self = Permissions(6); pub const RWX: Self = Permissions(7);
    pub fn as_octal(&self) -> u16 { self.0 as u16 }
    pub fn can_read(&self, _o: bool) -> bool { self.0 & 4 != 0 }
    pub fn can_write(&self, _o: bool) -> bool { self.0 & 2 != 0 }
}

#[derive(Debug, Clone)]
pub struct FileMeta {
    pub name: String, pub is_dir: bool, pub size: u64,
    pub owner: String, pub group: String, pub perms: Permissions,
    pub system: bool, pub created_at: u64,
}
impl FileMeta {
    pub fn new_file(n: &str, o: &str) -> Self { FileMeta{name:n.into(),is_dir:false,size:0,owner:o.into(),group:"users".into(),perms:Permissions::RW,system:false,created_at:crate::timer::uptime_ms()} }
    pub fn new_dir(n: &str, o: &str) -> Self { FileMeta{name:n.into(),is_dir:true,size:0,owner:o.into(),group:"users".into(),perms:Permissions::RWX,system:false,created_at:crate::timer::uptime_ms()} }
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


