//! Stub для dialog — UI диалоги (заглушка).

pub fn confirm(_msg: &str) -> bool {
    true
}

pub fn prompt_line(_msg: &str) -> String {
    String::new()
}

pub fn prompt_masked(_msg: &str) -> String {
    String::new()
}

pub fn choose(_msg: &str, _items: &[String]) -> Option<usize> {
    None
}
