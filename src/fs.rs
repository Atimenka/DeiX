//! FS + DTI: system files visible but read-only for non-DTI.
#![allow(dead_code)]
use crate::ext2;
use alloc::format;
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

pub fn set_current_user(n: &str) { *CUR_USER.lock() = n.into(); }
pub fn current_user() -> String { CUR_USER.lock().clone() }
pub fn is_root() -> bool { let u = CUR_USER.lock(); *u == "atimenka" || *u == "__dti__" || *u == "root" }
pub fn is_dti() -> bool { *CUR_USER.lock() == "__dti__" }

pub fn load_meta_db() {
    let raw = match ext2::read_file("FILES.DB") { Ok(d) => d, Err(_) => return };
    let text = match core::str::from_utf8(&raw) { Ok(t) => t, Err(_) => return };
    let mut db = META_DB.lock(); db.clear();
    for line in text.lines() { let line = line.trim(); if line.is_empty()||line.starts_with('#'){continue} let p:Vec<&str>=line.split('|').collect(); if p.len()<7{continue} db.insert(p[0].into(), FileMeta{name:p[0].into(),is_dir:p[1]=="d",size:p[2].parse().unwrap_or(0),owner:p[3].into(),group:p[4].into(),perms:Permissions(p[5].parse().unwrap_or(0)),system:p[6]=="sys",created_at:p.get(7).and_then(|s|s.parse().ok()).unwrap_or(0)}); }
}
pub fn save_meta_db() {
    let db = META_DB.lock(); let mut t = String::from("#DeiX FS\n");
    for (_,m) in db.iter() { t.push_str(&format!("{}|{}|{}|{}|{}|{}|{}|{}\n",m.name,if m.is_dir{"d"}else{"f"},m.size,m.owner,m.group,m.perms.as_octal(),if m.system{"sys"}else{"usr"},m.created_at)); }
    drop(db); if !ext2::is_formatted(){let _=ext2::format();} let _=ext2::write_file("FILES.DB",t.as_bytes());
}

pub fn create_file(n: &str) -> Result<(),&'static str> { let u=current_user(); let mut db=META_DB.lock(); if db.contains_key(n){return Err("exists")} db.insert(n.into(),FileMeta::new_file(n,&u)); drop(db); save_meta_db(); ext2::write_file(n,b"").map_err(|_|"disk")?; Ok(()) }
pub fn create_dir(n: &str) -> Result<(),&'static str> { let u=current_user(); let dn=n.trim_end_matches('/'); let mut db=META_DB.lock(); if db.contains_key(dn){return Err("exists")} db.insert(dn.into(),FileMeta::new_dir(dn,&u)); drop(db); save_meta_db(); ext2::write_file(dn,format!("#dir:{}\n",dn).as_bytes()).map_err(|_|"disk")?; Ok(()) }
pub fn delete_file(n: &str) -> Result<(),&'static str> { let u=current_user(); let db=META_DB.lock(); let m=db.get(n).ok_or("nf")?; if m.system && !is_root(){return Err("DTI required")} if m.owner!=u && !is_root(){return Err("not owner")} drop(db); META_DB.lock().remove(n); save_meta_db(); ext2::delete_file(n).map_err(|_|"disk")?; Ok(()) }
pub fn write_file(n: &str, d: &[u8]) -> Result<(),&'static str> { let u=current_user(); let mut db=META_DB.lock(); let m=db.get_mut(n).ok_or("nf")?; if m.system && !is_root()&&!is_dti(){return Err("SYS: read-only")} if m.owner!=u && !is_root(){return Err("not owner")} m.size=d.len()as u64; drop(db); save_meta_db(); ext2::write_file(n,d).map_err(|_|"disk")?; Ok(()) }
pub fn read_file(n: &str) -> Result<Vec<u8>,&'static str> { let db=META_DB.lock(); let m=db.get(n).ok_or("nf")?; if !m.perms.can_read(m.owner==current_user())&&!is_root(){return Err("no read")} drop(db); ext2::read_file(n).map_err(|_|"disk") }
pub fn list_dir() -> Vec<FileMeta> { META_DB.lock().values().cloned().collect() }
pub fn chmod(n: &str, p: Permissions) -> Result<(),&'static str> { let u=current_user(); let mut db=META_DB.lock(); let m=db.get_mut(n).ok_or("nf")?; if m.owner!=u&&!is_root(){return Err("not owner")} m.perms=p; drop(db); save_meta_db(); Ok(()) }
pub fn set_system(n: &str) -> Result<(),&'static str> { if !is_dti()&&!is_root(){return Err("DTI")} let mut db=META_DB.lock(); let m=db.get_mut(n).ok_or("nf")?; m.system=true; m.owner="__dti__".into(); drop(db); save_meta_db(); Ok(()) }

pub fn cmd_mkdir(a: &str) { if a.is_empty(){crate::println!("mkdir <n>");return} match create_dir(a){Ok(())=>crate::println!("dir:{}",a),Err(e)=>crate::println!("ERR:{}",e)} }
pub fn cmd_chmod(a: &str) { let p:Vec<&str>=a.split_whitespace().collect(); if p.len()<2{crate::println!("chmod <oct> <f>");return} match u8::from_str_radix(p[0],8){Ok(o)=>match chmod(p[1],Permissions(o)){Ok(())=>crate::println!("OK"),Err(e)=>crate::println!("ERR:{}",e)},Err(_)=>crate::println!("bad octal")} }
pub fn cmd_touch(a: &str) { if a.is_empty(){crate::println!("touch <f>");return} match create_file(a){Ok(())=>crate::println!("file:{}",a),Err(_)=>{let mut db=META_DB.lock();if let Some(m)=db.get_mut(a){m.created_at=crate::timer::uptime_ms()}crate::println!("touched:{}",a)}} }

pub fn api_create_file(p:*const u8,l:usize)->i64{if p.is_null()||l==0||l>64{return -1}let s=unsafe{core::slice::from_raw_parts(p,l)};let n=core::str::from_utf8(s).unwrap_or("");if n.is_empty(){return -1}match create_file(n){Ok(())=>0,Err(_)=>-1}}
pub fn api_create_dir(p:*const u8,l:usize)->i64{if p.is_null()||l==0||l>64{return -1}let s=unsafe{core::slice::from_raw_parts(p,l)};let n=core::str::from_utf8(s).unwrap_or("");if n.is_empty(){return -1}match create_dir(n){Ok(())=>0,Err(_)=>-1}}
pub fn api_delete_file(p:*const u8,l:usize)->i64{if p.is_null()||l==0||l>64{return -1}let s=unsafe{core::slice::from_raw_parts(p,l)};let n=core::str::from_utf8(s).unwrap_or("");if n.is_empty(){return -1}match delete_file(n){Ok(())=>0,Err(_)=>-1}}
pub fn api_file_exists(p:*const u8,l:usize)->i64{if p.is_null()||l==0{return 0}let s=unsafe{core::slice::from_raw_parts(p,l)};let n=core::str::from_utf8(s).unwrap_or("");if n.is_empty(){return 0}if META_DB.lock().contains_key(n){1}else{0}}
pub fn api_file_size(p:*const u8,l:usize)->i64{if p.is_null()||l==0{return -1}let s=unsafe{core::slice::from_raw_parts(p,l)};let n=core::str::from_utf8(s).unwrap_or("");if n.is_empty(){return -1}META_DB.lock().get(n).map(|m|m.size as i64).unwrap_or(-1)}
pub fn api_chmod(p:*const u8,l:usize,mode:u8)->i64{if p.is_null()||l==0{return -1}let s=unsafe{core::slice::from_raw_parts(p,l)};let n=core::str::from_utf8(s).unwrap_or("");if n.is_empty(){return -1}match chmod(n,Permissions(mode)){Ok(())=>0,Err(_)=>-1}}
pub fn api_list_dir(out:*mut u8,cap:usize)->i64{if out.is_null()||cap<2{return -1}let files=list_dir();let mut t=String::new();for f in &files{let s=if f.system{" [SYS]"}else{""};t.push_str(&format!("{}{}{}\n",if f.is_dir{"D"}else{"F"},f.name,s));}let b=t.as_bytes();let c=b.len().min(cap-1);unsafe{core::ptr::copy_nonoverlapping(b.as_ptr(),out,c);out.add(c).write(0);}c as i64}
