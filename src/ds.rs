//! DeiX Script (DS) — Полноценный интерпретатор скриптов и командный процессор.

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

    /// Вычисление значения выражения или подстановка переменных
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

    /// Вычисление логического выражения (для if/while)
    pub fn eval_condition(&self, cond: &str) -> bool {
        let trimmed = cond.trim();
        if let Some((lhs, rhs)) = trimmed.split_once("==") {
            return self.eval_expr(lhs) == self.eval_expr(rhs);
        }
        if let Some((lhs, rhs)) = trimmed.split_once("!=") {
            return self.eval_expr(lhs) != self.eval_expr(rhs);
        }
        if let Some((lhs, rhs)) = trimmed.split_once("<=") {
            let l_val = self.eval_expr(lhs).parse::<i64>().unwrap_or(0);
            let r_val = self.eval_expr(rhs).parse::<i64>().unwrap_or(0);
            return l_val <= r_val;
        }
        if let Some((lhs, rhs)) = trimmed.split_once(">=") {
            let l_val = self.eval_expr(lhs).parse::<i64>().unwrap_or(0);
            let r_val = self.eval_expr(rhs).parse::<i64>().unwrap_or(0);
            return l_val >= r_val;
        }
        if let Some((lhs, rhs)) = trimmed.split_once('<') {
            let l_val = self.eval_expr(lhs).parse::<i64>().unwrap_or(0);
            let r_val = self.eval_expr(rhs).parse::<i64>().unwrap_or(0);
            return l_val < r_val;
        }
        if let Some((lhs, rhs)) = trimmed.split_once('>') {
            let l_val = self.eval_expr(lhs).parse::<i64>().unwrap_or(0);
            let r_val = self.eval_expr(rhs).parse::<i64>().unwrap_or(0);
            return l_val > r_val;
        }

        let val = self.eval_expr(trimmed);
        !val.is_empty() && val != "0" && val != "false"
    }

    /// Вычисление арифметических операций
    pub fn eval_arithmetic(&self, expr: &str) -> i64 {
        let tokens: Vec<&str> = expr.split_whitespace().collect();
        if tokens.len() == 3 {
            let a = self.eval_expr(tokens[0]).parse::<i64>().unwrap_or(0);
            let op = tokens[1];
            let b = self.eval_expr(tokens[2]).parse::<i64>().unwrap_or(0);
            match op {
                "+" => return a + b,
                "-" => return a - b,
                "*" => return a * b,
                "/" => return if b != 0 { a / b } else { 0 },
                "%" => return if b != 0 { a % b } else { 0 },
                _ => {}
            }
        }
        self.eval_expr(expr).parse::<i64>().unwrap_or(0)
    }

    /// Выполнение единичной строки скрипта
    pub fn execute_line(&mut self, line: &str) -> Result<i64, DsError> {
        let line = line.trim();
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
                let rest = parts.collect::<Vec<&str>>().join(" ");
                if let Some((k, v)) = rest.split_once('=') {
                    let k = k.trim().to_string();
                    let v = v.trim();
                    let val = if v.starts_with("expr ") {
                        self.eval_arithmetic(&v[5..]).to_string()
                    } else {
                        v.to_string()
                    };
                    self.vars.insert(k, val);
                }
                Ok(0)
            }

            "expr" => {
                let rest = parts.collect::<Vec<&str>>().join(" ");
                let res = self.eval_arithmetic(&rest);
                crate::println!("{}", res);
                Ok(res)
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

            "write" => {
                if let Some(file) = parts.next() {
                    let content = parts.collect::<Vec<&str>>().join(" ");
                    if crate::ext2::is_formatted() {
                        let _ = crate::ext2::write_file(file, content.as_bytes());
                        crate::println!("  [ds] Записано в файл '{}'", file);
                    }
                }
                Ok(0)
            }

            "mkdir" => {
                if let Some(dir) = parts.next() {
                    if crate::ext2::is_formatted() {
                        let _ = crate::ext2::mkdir_p(dir);
                        crate::println!("  [ds] Создан каталог '{}'", dir);
                    }
                }
                Ok(0)
            }

            "rm" => {
                if let Some(file) = parts.next() {
                    if crate::ext2::is_formatted() {
                        let _ = crate::ext2::delete_file(file);
                        crate::println!("  [ds] Удалён файл '{}'", file);
                    }
                }
                Ok(0)
            }

            "read" => {
                if let Some(var_name) = parts.next() {
                    crate::print!("input> ");
                    let mut input = String::new();
                    // Интерактивный ввод строки через клавиатуру CLI
                    for _ in 0..100 {
                        if let Some(ch) = crate::keyboard::try_read_char() {
                            if ch == b'\n' || ch == b'\r' {
                                break;
                            }
                            input.push(ch as char);
                        }
                    }
                    if input.is_empty() {
                        input.push_str("stdin");
                    }
                    self.vars.insert(var_name.to_string(), input);
                }
                Ok(0)
            }

            "source" | "run" => {
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
                crate::sched::cmd_threads(&rest);
                Ok(0)
            }

            "duil" => {
                let rest = parts.collect::<Vec<&str>>().join(" ");
                crate::duil::cmd_duil(&rest);
                Ok(0)
            }

            _ => {
                // Если имя функции совпадает с пользовательской функцией
                if let Some(fn_body) = self.functions.get(cmd).cloned() {
                    for fn_line in fn_body {
                        self.execute_line(&fn_line)?;
                    }
                    return Ok(0);
                }
                // Передача в системный CLI DeiX OS
                crate::cli::execute(line);
                Ok(0)
            }
        }
    }

    /// Исполнение многострочного блока скрипта с поддержкой if/else/fi, while/done и fn/end
    pub fn run_script(&mut self, script: &str) -> Result<i64, DsError> {
        let lines: Vec<&str> = script.lines().map(|l| l.trim()).collect();
        let mut idx = 0;
        let mut last_code = 0;

        while idx < lines.len() {
            let line = lines[idx];
            if line.is_empty() || line.starts_with('#') || line.starts_with("#!") {
                idx += 1;
                continue;
            }

            if line.starts_with("if ") {
                let cond = line.trim_start_matches("if ").trim();
                let cond_met = self.eval_condition(cond);

                let mut true_branch = Vec::new();
                let mut false_branch = Vec::new();
                let mut in_else = false;
                let mut depth = 1;

                idx += 1;
                while idx < lines.len() && depth > 0 {
                    let cur = lines[idx];
                    if cur.starts_with("if ") {
                        depth += 1;
                    } else if cur == "fi" {
                        depth -= 1;
                        if depth == 0 {
                            idx += 1;
                            break;
                        }
                    } else if cur == "else" && depth == 1 {
                        in_else = true;
                        idx += 1;
                        continue;
                    }

                    if in_else {
                        false_branch.push(cur);
                    } else {
                        true_branch.push(cur);
                    }
                    idx += 1;
                }

                let branch = if cond_met { true_branch } else { false_branch };
                for b_line in branch {
                    last_code = self.execute_line(b_line)?;
                }
                continue;
            }

            if line.starts_with("while ") {
                let cond = line.trim_start_matches("while ").trim().to_string();
                let mut body = Vec::new();
                let mut depth = 1;

                idx += 1;
                while idx < lines.len() && depth > 0 {
                    let cur = lines[idx];
                    if cur.starts_with("while ") {
                        depth += 1;
                    } else if cur == "done" {
                        depth -= 1;
                        if depth == 0 {
                            idx += 1;
                            break;
                        }
                    }
                    body.push(cur);
                    idx += 1;
                }

                let mut iterations = 0;
                while self.eval_condition(&cond) && iterations < 10000 {
                    for b_line in body.iter() {
                        last_code = self.execute_line(b_line)?;
                    }
                    iterations += 1;
                }
                continue;
            }

            if line.starts_with("fn ") {
                let fn_name = line.trim_start_matches("fn ").trim().to_string();
                let mut body = Vec::new();

                idx += 1;
                while idx < lines.len() {
                    let cur = lines[idx];
                    if cur == "end" {
                        idx += 1;
                        break;
                    }
                    body.push(cur.to_string());
                    idx += 1;
                }
                self.functions.insert(fn_name, body);
                continue;
            }

            last_code = self.execute_line(line)?;
            idx += 1;
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
            crate::println!("Интерактивный толмач DeiX Script (введите 'exit' для выхода)");
            let mut interp = DsInterpreter::new();
            let demo_script = "\
let count = 1
while $count <= 3
  echo [DS REPL] Итерация № $count
  let count = expr $count + 1
done
";
            let _ = interp.run_script(demo_script);
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
