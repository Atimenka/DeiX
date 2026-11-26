//! Parse the exact init.deix included in the kernel and system image.
#![allow(dead_code)]
extern crate alloc;
#[macro_export] macro_rules! println { ($($arg:tt)*) => {{}}; }
#[path = "../src/init_parser.rs"] mod init_parser;
#[path = "../src/vault.rs"] mod vault;
use init_parser::{BootStage, Command, InitParser, MountMode, INIT_DEIX_SCRIPT};
#[test]
fn boot_configuration_preserves_mount_policy() {
    let mut parser = InitParser::new();
    let registry = parser.parse(INIT_DEIX_SCRIPT);
    assert!(parser.warnings.is_empty());
    assert_eq!(parser.report.rejected_lines, 0);
    let commands = registry.get(&BootStage::Boot).unwrap();
    assert_eq!(commands.len(), 2);
    assert!(matches!(&commands[0], Command::Mount(m) if m.dst == "/system" && m.fs_type == "erofs" && m.mode == MountMode::ReadOnly));
    assert!(matches!(&commands[1], Command::Mount(m) if m.dst == "/userdata" && m.fs_type == "ext2" && m.mode == MountMode::ReadWrite));
}
