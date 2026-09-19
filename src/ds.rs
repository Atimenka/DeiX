//! DeiX Script (DS) — Интерпретатор скриптов и командный процессор.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

#[derive(Debug, Clone)]
pub enum DsError {
    FileNotFound(String),
    SyntaxError(String),
    ExecutionError(String),
    RecursionLimit,
}

pub struct DsInterpreter {
    pub vars: BTreeMap<String, String>,
    pub functions: BTreeMap<String, Vec<String>>,
    pub env: BTreeMap<String, String>,
    pub recursion_depth: usize,
}

impl DsInterpreter {
    pub fn new() -> Self {
        let mut env = BTreeMap::new();
        env.insert(String::from("PATH"), String::from("/system/bin:/userdata/bin"));
        env.insert(String::from("HOME"), String::from("/users/root"));
        env.insert(String::from("SHELL"), String::from("/bin/ds"));

        Self {
            vars: BTreeMap::new(),
            functions: BTreeMap::new(),
            env,
            recursion_depth: 0,
        }
    }

    pub fn eval_expr(&self, expr: &str) -> String {
        let trimmed = expr.trim();
        if trimmed.starts_with('$') {
            let var_name = &trimmed[1..];
            if let Some(val) = self.vars.get(var_name) {
                return val.clone();
            }
            if let Some(val) = self.env.get(var_name) {
                return val.clone();
            }
            return String::new();
        }
        String::from(trimmed)
    }

    pub fn execute_line(&mut self, line: &str) -> Result<i64, DsError> {
        let mut line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return Ok(0);
        }

        // Подстановка переменных $VAR
        let mut expanded = String::new();
        let mut in_var = false;
        let mut var_buf = String::new();

        for ch in line.chars() {
            if ch == '$' {
                in_var = true;
                var_buf.clear();
            } else if in_var && (ch.is_alphanumeric() || ch == '_') {
                var_buf.push(ch);
            } else {
                if in_var {
                    in_var = false;
                    expanded.push_str(&self.eval_expr(&format!("${}", var_buf)));
                }
                expanded.push(ch);
            }
        }
        if in_var {
            expanded.push_str(&self.eval_expr(&format!("${}", var_buf)));
        }

        let line = expanded.trim();
        let mut parts = line.split_whitespace();
        let cmd = match parts.next() {
            Some(c) => c,
            None => return Ok(0),
        };

        match cmd {
            "echo" | "print" => {
                let rest: Vec<&str> = parts.collect();
                crate::println!("{}", rest.join(" "));
                Ok(0)
            }

            "let" | "set" => {
                let var_decl = parts.collect::<Vec<&str>>().join(" ");
                if let Some((k, v)) = var_decl.split_once('=') {
                    let k = k.trim().to_string();
                    let v = v.trim().to_string();
                    self.vars.insert(k, v);
                }
                Ok(0)
            }

            "env" => {
                if let Some(name) = parts.next() {
                    if let Some(val) = parts.next() {
                        self.env.insert(name.to_string(), val.to_string());
                    } else if let Some(val) = self.env.get(name) {
                        crate::println!("{}={}", name, val);
                    }
                } else {
                    for (k, v) in self.env.iter() {
                        crate::println!("{}={}", k, v);
                    }
                }
                Ok(0)
            }

            "test" => {
                let expr = parts.collect::<Vec<&str>>().join(" ");
                let res = !expr.is_empty() && expr != "0" && expr != "false";
                Ok(if res { 0 } else { 1 })
            }

            "cd" => {
                let path = parts.next().unwrap_or("/");
                crate::cli::set_cwd(path);
                Ok(0)
            }

            "pwd" => {
                crate::println!("{}", crate::cli::get_cwd());
                Ok(0)
            }

            "ls" => {
                let cwd = crate::cli::get_cwd();
                let _path = parts.next().unwrap_or(&cwd);
                if crate::ext2::is_formatted() {
                    if let Ok(entries) = crate::ext2::list_root() {
                        for e in entries {
                            crate::println!("  {}", e.name);
                        }
                    }
                }
                Ok(0)
            }

            "cat" => {
                if let Some(file) = parts.next() {
                    if crate::ext2::is_formatted() {
                        if let Ok(data) = crate::ext2::read_file(file) {
                            if let Ok(text) = core::str::from_utf8(&data) {
                                crate::println!("{}", text);
                            }
                        }
                    }
                }
                Ok(0)
            }

            "mkdir" => {
                if let Some(_dir) = parts.next() {
                    if crate::ext2::is_formatted() {
                        crate::println!("mkdir: создание каталогов на ext2");
                    }
                }
                Ok(0)
            }

            "rm" => {
                if let Some(file) = parts.next() {
                    if crate::ext2::is_formatted() {
                        let _ = crate::ext2::delete_file(file);
                    }
                }
                Ok(0)
            }

            "read" => {
                if let Some(var_name) = parts.next() {
                    let mut input = String::new();
                    // Заглушка чтению ввода
                    input.push_str("line");
                    self.vars.insert(var_name.to_string(), input);
                }
                Ok(0)
            }

            "source" => {
                if let Some(path) = parts.next() {
                    if self.recursion_depth > 10 {
                        return Err(DsError::RecursionLimit);
                    }
                    self.recursion_depth += 1;
                    let res = run_file(path);
                    self.recursion_depth -= 1;
                    return res;
                }
                Ok(0)
            }

            "exit" => {
                let code = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
                Ok(code)
            }

            "dinit" => {
                let rest = parts.collect::<Vec<&str>>().join(" ");
                crate::dinit::cmd_dinit(&rest);
                Ok(0)
            }

            "taskmgr" => {
                let rest = parts.collect::<Vec<&str>>().join(" ");
                crate::taskmgr::cmd_taskmgr(&rest);
                Ok(0)
            }

            _ => {
                // Передача неизвестной команды в системный CLI DeiX OS
                crate::cli::execute(line);
                Ok(0)
            }
        }
    }

    pub fn run_script(&mut self, script: &str) -> Result<i64, DsError> {
        let mut last_code = 0;
        for line in script.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("#!") {
                continue; // Пропуск Shebang #!/bin/ds
            }
            last_code = self.execute_line(trimmed)?;
        }
        Ok(last_code)
    }
}

pub fn run_string(code: &str) -> Result<i64, DsError> {
    let mut interp = DsInterpreter::new();
    interp.run_script(code)
}

pub fn run_file(path: &str) -> Result<i64, DsError> {
    if !crate::ext2::is_formatted() {
        return Err(DsError::FileNotFound(String::from("Диск ext2 не отформатирован")));
    }
    match crate::ext2::read_file(path) {
        Ok(data) => {
            if let Ok(script) = core::str::from_utf8(&data) {
                run_string(script)
            } else {
                Err(DsError::ExecutionError(String::from("Файл не является UTF-8")))
            }
        }
        Err(_) => Err(DsError::FileNotFound(String::from(path))),
    }
}

pub fn cmd_ds(arg: &str) {
    let mut parts = arg.trim().split_whitespace();
    let sub = parts.next().unwrap_or("");

    match sub {
        "-c" => {
            let code = parts.collect::<Vec<&str>>().join(" ");
            match run_string(&code) {
                Ok(res) => crate::println!("  [ds] Выполнено с кодом {}", res),
                Err(e) => crate::println!("  [ds] Ошибка исполнения: {:?}", e),
            }
        }
        "-i" | "repl" => {
            crate::println!("=== DeiX Script (DS) Interactive REPL ===");
            crate::println!("Введите 'exit' для выхода.");
            let mut interp = DsInterpreter::new();
            // В интерактивном режиме
            let demo_lines = ["echo Hello from DS REPL!", "let x = 42", "echo x = $x"];
            for line in demo_lines {
                crate::print!("ds> ");
                crate::println!("{}", line);
                let _ = interp.execute_line(line);
            }
        }
        path if !path.is_empty() => {
            match run_file(path) {
                Ok(res) => crate::println!("  [ds] Скрипт '{}' завершён с кодом {}", path, res),
                Err(e) => crate::println!("  [ds] Ошибка выполнения скрипта: {:?}", e),
            }
        }
        _ => {
            crate::println!("Использование: ds script.dxs | ds -c \"code\" | ds -i");
        }
    }
}
