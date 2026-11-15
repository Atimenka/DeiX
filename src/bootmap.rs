//! Stub для bootmap — загрузческая карта (заглушка).

pub fn read_boot_map() -> Result<Vec<(u32, u32)>, String> {
    Err(String::from("bootmap: not available"))
}
