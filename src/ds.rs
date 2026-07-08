//! DeiX Script (DS) — встроенный скриптовый язык.
use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::format;
use crate::{print, println};

#[derive(Debug, Clone, PartialEq)]
pub enum Value { Int(i64), Str(String), Bool(bool), None }

impl Value {
    pub fn as_str(&self) -> String { match self { Value::Str(s) => s.clone(), Value::Int(n) => n.to_string(), Value::Bool(b) => b.to_string(), Value::None => String::new() }}
    pub fn as_int(&self) -> i64 { match self { Value::Int(n) => *n, Value::Str(s) => s.parse().unwrap_or(0), Value::Bool(b) => if *b {1} else {0}, Value::None => 0 }}
    pub fn is_true(&self) -> bool { match self { Value::Bool(b) => *b, Value::Int(n) => *n != 0, Value::Str(s) => !s.is_empty(), Value::None => false }}
}

#[derive(Debug, Clone, PartialEq)]
enum Tok { Ident(String), StrLit(String), IntLit(i64), Eq, EqEq, NotEq, Lt, Gt, Plus, Minus, LBrace, RBrace, Dollar, LParen, RParen, Let, If, Else, For, In, While, True, False, Eof }

struct Lexer { chars: Vec<char>, pos: usize }

impl Lexer {
    fn new(s: &str) -> Self { Lexer { chars: s.chars().collect(), pos: 0 } }
    fn peek(&self) -> Option<char> { self.chars.get(self.pos).copied() }
    fn next(&mut self) -> Option<char> { let c = self.chars.get(self.pos).copied(); self.pos += 1; c }
    fn skip_ws(&mut self) {
        loop { match self.peek() {
            Some('#') => { while let Some(c) = self.next() { if c == '\n' { break; } } }
            Some(c) if c.is_whitespace() => { self.next(); }
            _ => break
        }}
    }
    fn read_str(&mut self, q: char) -> String { self.next(); let mut s = String::new(); while let Some(c) = self.next() { if c == q { break } else { s.push(c) } } s }
    fn read_num(&mut self, first: char) -> Tok { let mut s = String::from(first); while self.peek().map_or(false, |c| c.is_ascii_digit() || c.is_ascii_hexdigit()) { s.push(self.next().unwrap()); } Tok::IntLit(s.parse().unwrap_or(0)) }
    fn read_ident(&mut self, first: char) -> Tok { let mut s = String::from(first); while self.peek().map_or(false, |c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.') { s.push(self.next().unwrap()); } match s.as_str() { "let" => Tok::Let, "if" => Tok::If, "else" => Tok::Else, "for" => Tok::For, "in" => Tok::In, "while" => Tok::While, "true" => Tok::True, "false" => Tok::False, _ => Tok::Ident(s) } }

    fn tokenize(&mut self) -> Vec<Tok> {
        let mut v = Vec::new();
        loop {
            self.skip_ws();
            match self.next() {
                None => break,
                Some('=') => v.push(if self.peek() == Some('=') { self.next(); Tok::EqEq } else { Tok::Eq }),
                Some('!') => v.push(if self.peek() == Some('=') { self.next(); Tok::NotEq } else { Tok::Ident("!".into()) }),
                Some('<') => v.push(Tok::Lt), Some('>') => v.push(Tok::Gt),
                Some('+') => v.push(Tok::Plus), Some('-') => v.push(Tok::Minus),
                Some('{') => v.push(Tok::LBrace), Some('}') => v.push(Tok::RBrace),
                Some('$') => v.push(Tok::Dollar), Some('(') => v.push(Tok::LParen), Some(')') => v.push(Tok::RParen),
                Some(c) if c == '"' || c == '\'' => v.push(Tok::StrLit(self.read_str(c))),
                Some(c) if c.is_ascii_digit() => v.push(self.read_num(c)),
                Some(c) if c.is_alphanumeric() || c == '_' => v.push(self.read_ident(c)),
                Some(_) => {}
            }
        }
        v
    }
}

pub struct DsRuntime { vars: BTreeMap<String, Value>, toks: Vec<Tok>, pos: usize }

impl DsRuntime {
    pub fn new() -> Self { DsRuntime { vars: BTreeMap::new(), toks: Vec::new(), pos: 0 } }

    fn peek_tok(&self) -> &Tok { self.toks.get(self.pos).unwrap_or(&Tok::Eof) }
    fn next_tok(&mut self) -> Tok { if self.pos < self.toks.len() { let t = self.toks[self.pos].clone(); self.pos += 1; t } else { Tok::Eof } }
    fn skip(&mut self, t: Tok) -> bool { if *self.peek_tok() == t { self.next_tok(); true } else { false } }

    pub fn execute(&mut self, source: &str) -> Result<i64, String> {
        self.toks = Lexer::new(source).tokenize();
        self.pos = 0;
        let mut last = 0i64;
        while *self.peek_tok() != Tok::Eof { last = self.parse_stmt()?; }
        Ok(last)
    }

    fn parse_stmt(&mut self) -> Result<i64, String> {
        match self.peek_tok().clone() {
            Tok::RBrace | Tok::Eof => Ok(0),
            Tok::Let => { self.next_tok(); self.parse_assign() }
            Tok::If => { self.next_tok(); self.parse_if() }
            Tok::For => { self.next_tok(); self.parse_for() }
            Tok::While => { self.next_tok(); self.parse_while() }
            _ => self.parse_cmd()
        }
    }

    fn parse_assign(&mut self) -> Result<i64, String> {
        let name = match self.next_tok() { Tok::Ident(n) => n, _ => return Err("Expected identifier".into()) };
        self.skip(Tok::Eq);
        let val = self.eval_value();
        self.vars.insert(name, val);
        Ok(0)
    }

    fn eval_value(&mut self) -> Value {
        match self.peek_tok().clone() {
            Tok::Dollar => { self.next_tok(); self.skip(Tok::LParen);
                let mut inner = String::new(); let mut depth = 1;
                while depth > 0 { match self.next_tok() {
                    Tok::LParen => { depth += 1; inner.push('('); }
                    Tok::RParen => { depth -= 1; if depth > 0 { inner.push(')'); } }
                    Tok::Ident(s) => inner.push_str(&s),
                    Tok::StrLit(s) => inner.push_str(&s),
                    Tok::IntLit(n) => { inner.push_str(&n.to_string()); }
                    Tok::Eof => break,
                    _ => {}
                }}
                self.vars.get(inner.trim()).cloned().unwrap_or(Value::Str(inner))
            }
            Tok::Ident(n) => { self.next_tok(); self.vars.get(&n).cloned().unwrap_or(Value::Str(n.clone())) }
            Tok::StrLit(s) => { self.next_tok(); Value::Str(s.clone()) }
            Tok::IntLit(n) => { self.next_tok(); Value::Int(n) }
            Tok::True => { self.next_tok(); Value::Bool(true) }
            Tok::False => { self.next_tok(); Value::Bool(false) }
            _ => Value::None,
        }
    }

    fn eval_expr(&mut self) -> Value {
        let mut left = self.eval_value();
        while matches!(self.peek_tok(), Tok::EqEq | Tok::NotEq | Tok::Lt | Tok::Gt | Tok::Plus | Tok::Minus) {
            let op = self.next_tok();
            let right = self.eval_value();
            left = match op {
                Tok::Plus => Value::Int(left.as_int() + right.as_int()),
                Tok::Minus => Value::Int(left.as_int() - right.as_int()),
                Tok::EqEq => Value::Bool(left == right),
                Tok::NotEq => Value::Bool(left != right),
                Tok::Lt => Value::Bool(left.as_int() < right.as_int()),
                Tok::Gt => Value::Bool(left.as_int() > right.as_int()),
                _ => Value::None,
            };
        }
        left
    }

    fn parse_if(&mut self) -> Result<i64, String> {
        let cond = self.eval_expr();
        self.skip(Tok::LBrace);
        let mut code = 0i64;
        if cond.is_true() {
            while *self.peek_tok() != Tok::RBrace && *self.peek_tok() != Tok::Eof { code = self.parse_stmt()?; }
        } else {
            while *self.peek_tok() != Tok::RBrace && *self.peek_tok() != Tok::Eof { self.next_tok(); }
        }
        self.skip(Tok::RBrace);
        if *self.peek_tok() == Tok::Else { self.next_tok();
            if *self.peek_tok() == Tok::If { self.next_tok(); return self.parse_if(); }
            self.skip(Tok::LBrace);
            if !cond.is_true() { while *self.peek_tok() != Tok::RBrace && *self.peek_tok() != Tok::Eof { code = self.parse_stmt()?; } }
            else { while *self.peek_tok() != Tok::RBrace && *self.peek_tok() != Tok::Eof { self.next_tok(); } }
            self.skip(Tok::RBrace);
        }
        Ok(code)
    }

    fn parse_for(&mut self) -> Result<i64, String> {
        let var = match self.next_tok() { Tok::Ident(n) => n, _ => return Err("Expected var".into()) };
        if !self.skip(Tok::In) { return Err("Expected 'in'".into()); }
        let list = self.eval_value().as_str();
        self.skip(Tok::LBrace);
        let mut code = 0i64;
        let saved_pos = self.pos;
        for item in list.split_whitespace() {
            self.vars.insert(var.clone(), Value::Str(item.into()));
            self.pos = saved_pos;
            while *self.peek_tok() != Tok::RBrace && *self.peek_tok() != Tok::Eof { code = self.parse_stmt()?; }
        }
        self.skip(Tok::RBrace);
        Ok(code)
    }

    fn parse_while(&mut self) -> Result<i64, String> {
        let cond_start = self.pos;
        self.skip(Tok::LBrace);
        let body_start = self.pos;
        let mut code = 0i64;
        loop {
            self.pos = cond_start;
            if !self.eval_expr().is_true() { break; }
            self.pos = body_start;
            while *self.peek_tok() != Tok::RBrace && *self.peek_tok() != Tok::Eof { code = self.parse_stmt()?; }
        }
        while *self.peek_tok() != Tok::RBrace && *self.peek_tok() != Tok::Eof { self.next_tok(); }
        self.skip(Tok::RBrace);
        Ok(code)
    }

    fn parse_cmd(&mut self) -> Result<i64, String> {
        let cmd = match self.next_tok() { Tok::Ident(n) => n, _t => { return Ok(0); } };
        let mut args: Vec<Value> = Vec::new();
        while !matches!(self.peek_tok(), Tok::Eof | Tok::RBrace | Tok::LBrace | Tok::Let | Tok::If | Tok::For | Tok::While) {
            args.push(self.eval_value());
        }
        self.exec_cmd(&cmd, &args)
    }

    fn exec_cmd(&mut self, name: &str, args: &[Value]) -> Result<i64, String> {
        let arg_str: Vec<String> = args.iter().map(|a| a.as_str()).collect();
        let full = if arg_str.is_empty() { name.to_string() } else { format!("{} {}", name, arg_str.join(" ")) };
        match name {
            "echo" => { println!("{}", arg_str.join(" ")); Ok(0) }
            "print" => { print!("{}", arg_str.join(" ")); Ok(0) }
            "set" | "let" => { if args.len() >= 2 { self.vars.insert(args[0].as_str(), args[1].clone()); } Ok(0) }
            "sleep" => { let ms = args.get(0).map(|a| a.as_int() as u64).unwrap_or(1000); let t = crate::timer::uptime_ms() + ms; while crate::timer::uptime_ms() < t { unsafe { core::arch::asm!("hlt"); } } Ok(0) }
            "exec" => { crate::cli::execute(&full); Ok(0) }
            "exit" => { Err("exit".into()) }
            "uptime" => { let ms = crate::timer::uptime_ms(); println!("uptime: {}s", ms/1000); Ok(0) }
            _ => { crate::cli::execute(&full); Ok(0) }
        }
    }
}

pub fn run_file(filename: &str) -> Result<i64, String> {
    let data = crate::ext2::read_file(filename).map_err(|_| format!("Cannot read '{}'", filename))?;
    let src = core::str::from_utf8(&data).map_err(|_| "Invalid UTF-8".to_string())?;
    DsRuntime::new().execute(src)
}

pub fn run_string(code: &str) -> Result<i64, String> { DsRuntime::new().execute(code) }

pub fn cmd_ds(arg: &str) {
    if arg.is_empty() { println!("ds <file.dxs> | ds -c <code> | ds -i"); return; }
    if arg == "-i" {
        let mut rt = DsRuntime::new();
        loop { print!("ds> "); let line = crate::keyboard::read_line(); if line.trim() == "exit" { break; } if line.trim().is_empty() { continue; } match rt.execute(&line) { Ok(c) => println!("  => {}", c), Err(e) => println!("  ERR: {}", e) } }
        return;
    }
    if arg.starts_with("-c ") { match run_string(&arg[3..]) { Ok(c) => println!("=> {}", c), Err(e) => println!("ERR: {}", e) } return; }
    match run_file(arg) { Ok(c) => println!("Script '{}' => {}", arg, c), Err(e) => println!("ERR: {}", e) }
}
