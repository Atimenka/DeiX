//! Host-side tests for the real ELF loader. Does not execute the image.
#![allow(dead_code)]
extern crate alloc;
#[macro_export] macro_rules! println { ($($arg:tt)*) => {}; }
const USER_IMAGE_BASE: u64 = 0x0180_0000;
const USER_IMAGE_MAX: u64 = 0x0040_0000;
const USER_STACK_SIZE: u64 = 256 * 1024;
#[path = "../src/linux/elf.rs"] mod elf;
fn put16(b: &mut [u8], o: usize, v: u16) { b[o..o+2].copy_from_slice(&v.to_le_bytes()); }
fn put32(b: &mut [u8], o: usize, v: u32) { b[o..o+4].copy_from_slice(&v.to_le_bytes()); }
fn put64(b: &mut [u8], o: usize, v: u64) { b[o..o+8].copy_from_slice(&v.to_le_bytes()); }
fn image() -> Vec<u8> {
    let mut b = vec![0; 256];
    b[..4].copy_from_slice(b"\x7fELF"); b[4] = 2; b[5] = 1;
    put16(&mut b,16,3); put16(&mut b,18,62); put64(&mut b,24,0x80);
    put64(&mut b,32,64); put16(&mut b,54,56); put16(&mut b,56,1);
    put32(&mut b,64,1); put32(&mut b,68,5); put64(&mut b,72,0);
    put64(&mut b,80,0); put64(&mut b,96,256); put64(&mut b,104,256);
    b[128] = 0xc3; b
}
fn attempt(bytes: &[u8]) -> (bool, Vec<u8>) {
    let mut dst = vec![0xa5; 512 * 1024];
    let ok = elf::load_into(bytes, dst.as_mut_ptr() as u64, dst.len() as u64).is_ok();
    (ok, dst)
}
#[test] fn loads_valid_pie() { let (ok, dst) = attempt(&image()); assert!(ok); assert_eq!(dst[128],0xc3); }
#[test] fn rejects_filesz_bigger_than_memsz_without_writes() {
    let mut b=image(); put64(&mut b,104,10); let (ok,dst)=attempt(&b); assert!(!ok); assert!(dst.iter().all(|&v| v==0xa5));
}
#[test] fn rejects_overflows_and_invalid_entry_without_writes() {
    for mode in 0..4 { let mut b=image(); match mode {
        0=>put64(&mut b,32,u64::MAX), 1=>put64(&mut b,80,u64::MAX-3),
        2=>put64(&mut b,24,0x10000), _=>put64(&mut b,72,u64::MAX-3),
    } let (ok,dst)=attempt(&b); assert!(!ok); assert!(dst.iter().all(|&v|v==0xa5)); }
}
#[test] fn inspect_rejects_bad_table_without_panicking() {
    let mut b=image(); put64(&mut b,32,u64::MAX); assert!(elf::inspect(&b).is_err());
}
#[test] fn gcc_static_pie_is_accepted_when_fixture_provided() {
    if let Ok(path) = std::env::var("DEIX_TEST_ELF") {
        let b=std::fs::read(path).unwrap(); let (ok, _) = attempt(&b); assert!(ok);
    }
}
#[test] fn rejects_unterminated_dynamic_without_mutating_destination() {
    let mut b=image();
    put16(&mut b,56,2);
    put32(&mut b,120,2); put64(&mut b,128,224); put64(&mut b,152,16);
    put64(&mut b,224,7); put64(&mut b,232,0);
    let (ok,dst)=attempt(&b); assert!(!ok); assert!(dst.iter().all(|&v|v==0xa5));
}
#[test] fn rejects_segment_stack_overlap_without_mutating_destination() {
    let mut b=image(); put64(&mut b,104,300_000);
    let (ok,dst)=attempt(&b); assert!(!ok); assert!(dst.iter().all(|&v|v==0xa5));
}
