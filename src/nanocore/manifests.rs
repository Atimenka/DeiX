//! Nano-Core manifests — описание нано-служб в простом текстовом формате.
//!
//! Файл-манифест — это блочный текст, где каждый блок:
//!
//! ```text
//! [service]
//! name = grep
//! spawn = proc:grep_main
//! deps = fs, net
//! restart = on-fail
//! deadline_ms = 2000
//! ```
//!
//! Ключи:
//! - `name` — имя службы (обязательно);
//! - `spawn` — источник: `proc:<fn>` (внутренняя задача ядра) или
//!   `cmd:<строка>` (команда CLI);
//! - `deps` — список имён служб, от которых зависит старт;
//! - `restart` — политика перезапуска: `always`, `on-fail`, `never`;
//! - `deadline_ms` — дедлайн heartbeat'а (ms).
//!
//! Пары ключей одного блока должны быть уникальными; неизвестные
//! ключи игнорируются со записью в лог.

use alloc::string::String;
use alloc::vec::Vec;

/// Политика перезапуска службы.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartPolicy {
    /// Перезапущать всегда после остановки.
    Always,
    /// Перезапущать только после сбоя (не остановлено по целям).
    OnFail,
    /// Никогда не перезапускать автоматически.
    Never,
}

impl RestartPolicy {
    pub fn parse(s: &str) -> Option<RestartPolicy> {
        match s.to_ascii_lowercase().as_str() {
            "always" => Some(RestartPolicy::Always),
            "on-fail" | "onfail" => Some(RestartPolicy::OnFail),
            "never" => Some(RestartPolicy::Never),
            _ => None,
        }
    }
}

/// Описание одной нано-службы.
#[derive(Debug, Clone)]
pub struct ServiceManifest {
    pub name: String,
    pub spawn: String,
    pub deps: Vec<String>,
    pub restart: RestartPolicy,
    pub deadline_ms: u64,
}

impl ServiceManifest {
    pub fn new(name: impl Into<String>, spawn: impl Into<String>) -> Self {
        ServiceManifest {
            name: name.into(),
            spawn: spawn.into(),
            deps: Vec::new(),
            restart: RestartPolicy::Never,
            deadline_ms: 5000,
        }
    }
}

/// Сборка манифеста из строки файла.
///
/// Пример успешной парсинга:
/// `[service]` → блок; внутри `key = value`.
///
/// Возвращает None, если структура файла некорректна.
pub fn parse(text: &str) -> Option<Vec<ServiceManifest>> {
    let mut services: Vec<ServiceManifest> = Vec::new();
    let mut current: Option<ServiceManifest> = None;

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            if let Some(cur) = current.take() {
                if cur.name.is_empty() || cur.spawn.is_empty() { return None; }
                services.push(cur);
            }
            let title = line[1..line.len() - 1].trim().to_ascii_lowercase();
            if title == "service" {
                current = Some(ServiceManifest::new("", ""));
                continue;
            }
            // Другие блоки (например, [meta]) — пропускаем.
            current = None;
            continue;
        }
        let Some(cur) = current.as_mut() else {
            continue;
        };
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        match key.as_str() {
            "name" => {
                cur.name = String::from(value);
            }
            "spawn" => {
                cur.spawn = String::from(value);
            }
            "deps" => {
                for d in value.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                    cur.deps.push(String::from(d));
                }
            }
            "restart" => {
                if let Some(p) = RestartPolicy::parse(value) {
                    cur.restart = p;
                }
            }
            "deadline_ms" => {
                if let Ok(n) = value.parse::<u64>() {
                    cur.deadline_ms = n;
                }
            }
            _ => {} // неизвестный ключ — игнорируем
        }
    }

    // Завершаем последний блок (имя обязательно).
    if let Some(cur) = current {
        if cur.name.is_empty() || cur.spawn.is_empty() { return None; }
        services.push(cur);
    }
    if services.is_empty() {
        None
    } else {
        Some(services)
    }
}

/// Дефолтный манифест нано-кора (встроенный).
///
/// Службы:
/// - `hb-monitor` — задача слежения за heartbeat'ами;
/// - `ipc-drain` — сливник канала nc-log в журнал;
/// - `log-report` — дублер журнала в serial (с реальным IPC).
pub fn builtin() -> Vec<ServiceManifest> {
    let text = "\
[service]
name = hb-monitor
spawn = proc:nanocore_hb_monitor
restart = always
deadline_ms = 10000

[service]
name = ipc-drain
spawn = proc:nanocore_ipc_drain
deps = hb-monitor
restart = on-fail
deadline_ms = 15000

[service]
name = log-report
spawn = proc:nanocore_log_report
deps = ipc-drain
restart = never
deadline_ms = 30000
";
    parse(text).unwrap_or_default()
}
