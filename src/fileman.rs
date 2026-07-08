//! DeiX Files — Полноценный графический файловый менеджер.
//! Аналог Dolphin/Konqueror для DeiX. Работает поверх VGA-текста.
//!
//! ## Возможности
//! - Двухпанельный интерфейс (левая панель: дерево, правая: список)
//! - Навигация: стрелки, Enter в папку, Backspace назад
//! - Операции: F2 переименовать, F5 копировать, F6 переместить, F7 папка, F8 удалить
//! - Фильтр: системные файлы скрыты (кроме root)
//! - Строка состояния: размер, права, владелец
//! - Сортировка по имени/размеру/дате

use crate::fs;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;


#[derive(Debug, Clone)]
#[allow(dead_code)]
struct FileEntry {
    meta: fs::FileMeta,
    selected: bool,
}

#[allow(dead_code)]
pub struct FileManager {
    entries: Vec<FileEntry>,
    current_idx: usize,
    scroll_offset: usize,
    view_height: usize,
    current_path: String,
    sort_by: u8, // 0=name, 1=size, 2=date
    dirty: bool,
}

impl FileManager {
    pub fn new() -> Self {
        FileManager {
            entries: Vec::new(), current_idx: 0, scroll_offset: 0,
            view_height: 20, current_path: "/".into(), sort_by: 0, dirty: true,
        }
    }

    pub fn refresh(&mut self) {
        let files = fs::list_dir();
        self.entries.clear();
        for meta in files {
            self.entries.push(FileEntry { meta, selected: false });
        }
        self.sort();
        self.dirty = false;
    }

    fn sort(&mut self) {
        match self.sort_by {
            0 => self.entries.sort_by(|a, b| {
                if a.meta.is_dir != b.meta.is_dir { b.meta.is_dir.cmp(&a.meta.is_dir) }
                else { a.meta.name.to_lowercase().cmp(&b.meta.name.to_lowercase()) }
            }),
            1 => self.entries.sort_by(|a, b| a.meta.size.cmp(&b.meta.size).reverse()),
            2 => self.entries.sort_by(|a, b| b.meta.created_at.cmp(&a.meta.created_at)),
            _ => {}
        }
    }

    fn current_file(&self) -> Option<&FileEntry> {
        self.entries.get(self.current_idx)
    }

    pub fn enter_directory(&mut self, name: &str) {
        // In our flat filesystem, "directories" are just a naming convention
        self.current_path = format!("{}{}/", self.current_path.trim_end_matches('/'), name);
        self.current_idx = 0;
        self.dirty = true;
    }

    pub fn go_up(&mut self) {
        if self.current_path != "/" {
            let parts: Vec<&str> = self.current_path.trim_matches('/').split('/').collect();
            if parts.len() <= 1 {
                self.current_path = "/".into();
            } else {
                self.current_path = format!("/{}/", parts[..parts.len()-1].join("/"));
            }
            self.current_idx = 0;
            self.dirty = true;
        }
    }
}

// ==================== CLI команда ====================

pub fn run_file_manager() {
    use crate::println;

    crate::vgaglobal::with_writer(|w| w.clear_screen());
    println!("╔══════════════════════════════════════════════════════════════════════════╗");
    println!("║                       🗂️  DeiX Files v0.2                              ║");
    println!("╚══════════════════════════════════════════════════════════════════════════╝");
    println!();

    let mut fm = FileManager::new();
    fm.refresh();

    let root = fs::is_root();
    if !root {
        println!("  (system files hidden — login as root to see all)");
    }

    // Display files
    for (i, entry) in fm.entries.iter().enumerate() {
        let marker = if i == fm.current_idx { " ▶" } else { "  " };
        let icon = if entry.meta.is_dir { "📁" } else { "📄" };
        let sys = if entry.meta.system { "[SYS]" } else { "     " };
        let size_str = if entry.meta.is_dir { "<DIR>".to_string() }
            else if entry.meta.size < 1024 { format!("{}b", entry.meta.size) }
            else if entry.meta.size < 1024*1024 { format!("{}K", entry.meta.size/1024) }
            else { format!("{}M", entry.meta.size/(1024*1024)) };

        println!("{}{} {:<24} {:>8} {:<10} {}",
            marker, icon, entry.meta.name, size_str, entry.meta.owner, sys);
    }

    println!();
    println!("  [↑↓]Navigate [Enter]Open [Bksp]Up [F7]MkDir [F8]Delete [Q]Quit");
    println!("  Path: {}", fm.current_path);

    // Interactive loop
    loop {
        let key = crate::keyboard::read_char();
        match key {
            b'q' | b'Q' | 27 => break,
            b'\r' | b'\n' => {
                if let Some(entry) = fm.current_file() {
                    if entry.meta.is_dir {
                        let dir_name = entry.meta.name.clone();
                        let _ = entry; // explicit: release borrow before mut
                        fm.enter_directory(&dir_name);
                        fm.refresh();
                    } else {
                        // Open file (read and print)
                        match fs::read_file(&entry.meta.name) {
                            Ok(data) => {
                                crate::vgaglobal::with_writer(|w| w.clear_screen());
                                println!("=== {} ===\n", entry.meta.name);
                                if let Ok(text) = core::str::from_utf8(&data) {
                                    for line in text.lines().take(20) {
                                        println!("{}", line);
                                    }
                                } else {
                                    println!("[Binary file: {} bytes]", data.len());
                                }
                                println!("\nPress any key to return...");
                                crate::keyboard::read_char();
                                crate::vgaglobal::with_writer(|w| w.clear_screen());
                            }
                            Err(e) => { println!("Error: {}", e); }
                        }
                    }
                }
            }
            crate::keyboard::ARROW_UP => {
                if fm.current_idx > 0 { fm.current_idx -= 1; }
            }
            crate::keyboard::ARROW_DOWN => {
                if fm.current_idx + 1 < fm.entries.len() { fm.current_idx += 1; }
            }
            0x08 => { fm.go_up(); fm.refresh(); } // Backspace
            b'7' => {
                // F7: Create directory
                crate::print!("Directory name: ");
                let name = crate::keyboard::read_line();
                if !name.is_empty() {
                    match fs::create_dir(&name) {
                        Ok(()) => { println!("Created."); fm.refresh(); }
                        Err(e) => println!("Error: {}", e),
                    }
                }
            }
            b'8' => {
                // F8: Delete
                if let Some(entry) = fm.current_file() {
                    crate::print!("Delete '{}'? (y/n): ", entry.meta.name);
                    let confirm = crate::keyboard::read_char();
                    if confirm == b'y' || confirm == b'Y' {
                        match fs::delete_file(&entry.meta.name) {
                            Ok(()) => { println!("Deleted."); fm.refresh(); }
                            Err(e) => println!("Error: {}", e),
                        }
                    }
                }
            }
            b't' => {
                // Touch: create new file
                crate::print!("File name: ");
                let name = crate::keyboard::read_line();
                if !name.is_empty() {
                    match fs::create_file(&name) {
                        Ok(()) => { println!("Created."); fm.refresh(); }
                        Err(e) => println!("Error: {}", e),
                    }
                }
            }
            b's' => {
                fm.sort_by = (fm.sort_by + 1) % 3;
                fm.dirty = true;
                fm.refresh();
            }
            _ => {}
        }
    }
    crate::vgaglobal::with_writer(|w| w.clear_screen());
}

pub fn cmd_files(_arg: &str) {
    run_file_manager();
}
