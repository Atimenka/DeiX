//! Управление сервисами и демонами в Dinit (PID 1)
//!
//! Два типа служб:
//!   - `Builtin`  — подсистема внутри ядра (учёт состояния, без отдельного процесса);
//!   - `Process`  — отдельный исполняемый файл, запускаемый через process::spawn/exec.
//!
//! Для Process-служб реализована супервизия: запуск, остановка, отслеживание
//! жизненного цикла, перезапуск по политике (Always, OnFailure, ...) с
//! экспоненциальным backoff.

use alloc::format;
use alloc::string::String;

/// Политика перезапуска службы при завершении или сбое
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartPolicy {
    /// Перезапускать всегда (для критических системных демонов)
    Always,
    /// Перезапускать только при аварийном завершении (ненулевой код / краш)
    OnFailure,
    /// Перезапускать, пока служба не остановлена явной командой
    UnlessStopped,
}

impl RestartPolicy {
    pub fn as_str(&self) -> &'static str {
        match self {
            RestartPolicy::Always => "always",
            RestartPolicy::OnFailure => "on-failure",
            RestartPolicy::UnlessStopped => "unless-stopped",
        }
    }
}

/// Текущее состояние жизненного цикла службы
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceStatus {
    /// Служба остановлена
    Stopped,
    /// Служба активно исполняется
    Running,
    /// Служба ожидает таймера перезапуска (backoff)
    Restarting,
    /// Служба завершилась штатно с кодом выхода
    Exited(i32),
    /// Служба аварийно завершилась с кодом ошибки
    Failed(i32),
    /// Служба ликвидирована сигналом (например, SIGKILL от Security Monitor)
    Terminated(i32),
    /// Служба аварийно упала (исчерпан лимит перезапусков)
    Crashed,
}

impl ServiceStatus {
    pub fn is_active(&self) -> bool {
        matches!(self, ServiceStatus::Running)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ServiceStatus::Stopped => "stopped",
            ServiceStatus::Running => "running",
            ServiceStatus::Restarting => "restarting",
            ServiceStatus::Exited(_) => "exited",
            ServiceStatus::Failed(_) => "failed",
            ServiceStatus::Terminated(_) => "terminated",
            ServiceStatus::Crashed => "crashed",
        }
    }
}

/// Тип службы: встроенная в ядро подсистема или отдельный процесс.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceKind {
    /// Подсистема внутри ядра: отдельного процесса и PID у неё нет.
    Builtin,
    /// Отдельный исполняемый файл, запускаемый через менеджер процессов.
    Process,
}

impl ServiceKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ServiceKind::Builtin => "builtin",
            ServiceKind::Process => "process",
        }
    }
}

/// Дескриптор службы, управляемой супервизором Dinit
#[derive(Debug, Clone)]
pub struct ServiceDescriptor {
    /// Уникальное системное имя службы
    pub name: String,
    /// Тип службы: встроенная подсистема ядра или отдельный процесс
    pub kind: ServiceKind,
    /// Путь к исполняемому бинарному файлу (пустой для встроенных служб)
    pub binary_path: String,
    /// Кольцо исполнения (0 — ядро/Ring 0, 3 — пользовательское пространство/Ring 3)
    pub execution_ring: u8,
    /// Политика перезапуска
    pub restart_policy: RestartPolicy,
    /// Текущее состояние службы
    pub status: ServiceStatus,
    /// PID процесса (если запущен; у встроенных служб PID нет)
    pub pid: Option<u32>,
    /// Флаг автозапуска при инициализации соответствующей стадии
    pub auto_start: bool,
    /// Критическая служба: исчерпание лимита перезапусков фиксируется как CRITICAL
    pub is_critical: bool,
    /// Количество зафиксированных аварийных перезапусков
    pub crash_count: u32,
    /// Максимально допустимое число перезапусков подряд
    pub max_restarts: u32,
    /// Задержка перед следующим перезапуском (в мс)
    pub restart_backoff_ms: u64,
    /// Время последнего успешного старта (uptime_ms)
    pub last_start_time: u64,
    /// Время последнего краша (uptime_ms)
    pub last_crash_time: u64,
}

impl ServiceDescriptor {
    pub fn new(name: &str, binary_path: &str, execution_ring: u8) -> Self {
        Self {
            name: String::from(name),
            kind: ServiceKind::Process,
            binary_path: String::from(binary_path),
            execution_ring,
            restart_policy: RestartPolicy::OnFailure,
            status: ServiceStatus::Stopped,
            pid: None,
            auto_start: true,
            is_critical: false,
            crash_count: 0,
            max_restarts: 5,
            restart_backoff_ms: 500,
            last_start_time: 0,
            last_crash_time: 0,
        }
    }

    /// Конструктор встроенной службы: подсистема живёт в ядре,
    /// отдельного процесса и PID у неё нет.
    pub fn builtin(name: &str, policy: RestartPolicy, critical: bool) -> Self {
        let mut desc = Self::new(name, "", 0);
        desc.kind = ServiceKind::Builtin;
        desc.restart_policy = policy;
        desc.is_critical = critical;
        desc
    }

    /// Проверка, готов ли сервис к перезапуску с учётом backoff
    pub fn can_restart(&self, now: u64) -> bool {
        if self.status == ServiceStatus::Stopped {
            return false;
        }
        if self.crash_count >= self.max_restarts {
            return false;
        }
        match self.restart_policy {
            RestartPolicy::UnlessStopped => true,
            RestartPolicy::Always | RestartPolicy::OnFailure => {
                now.saturating_sub(self.last_crash_time) >= self.restart_backoff_ms
            }
        }
    }

    /// Отметка активности встроенной службы (подсистема ядра работает)
    pub fn mark_builtin_running(&mut self, now: u64) {
        self.status = ServiceStatus::Running;
        self.pid = None;
        self.last_start_time = now;
    }

    /// Отметка успешного старта Process-службы
    pub fn mark_started(&mut self, pid: u32, now: u64) {
        self.status = ServiceStatus::Running;
        self.pid = Some(pid);
        self.last_start_time = now;
        // Если служба проработала достаточно долго (более 30 секунд), сбрасываем счётчик крашей
        if now.saturating_sub(self.last_crash_time) > 30_000 {
            self.crash_count = 0;
            self.restart_backoff_ms = 500;
        }
    }

    /// Обработка штатного завершения
    pub fn mark_exited(&mut self, code: i32) {
        self.status = if code == 0 {
            ServiceStatus::Exited(code)
        } else {
            ServiceStatus::Failed(code)
        };
        self.pid = None;
    }

    /// Обработка аварийного падения / ликвидации
    pub fn mark_crashed(&mut self, signal: i32, now: u64) -> bool {
        self.last_crash_time = now;
        self.crash_count += 1;
        self.restart_backoff_ms = (self.restart_backoff_ms * 2).min(10_000);
        self.pid = None;

        if signal != 0 {
            self.status = ServiceStatus::Terminated(signal);
        } else {
            self.status = ServiceStatus::Failed(-1);
        }

        if self.crash_count >= self.max_restarts {
            self.status = ServiceStatus::Crashed;
            false
        } else {
            self.status = ServiceStatus::Restarting;
            true
        }
    }

    /// Остановка службы
    pub fn mark_stopped(&mut self) {
        self.status = ServiceStatus::Stopped;
        self.pid = None;
    }

    /// Форматированная строка состояния для CLI и дампа
    pub fn format_status(&self, now: u64) -> String {
        let uptime_str = if self.status == ServiceStatus::Running && self.last_start_time > 0 {
            let s = now.saturating_sub(self.last_start_time) / 1000;
            format!("{}s", s)
        } else {
            String::from("-")
        };

        let pid_str = match (self.kind, self.pid) {
            (ServiceKind::Builtin, _) => String::from("kernel"),
            (ServiceKind::Process, Some(p)) => format!("{}", p),
            (ServiceKind::Process, None) => String::from("-"),
        };

        let path_str = if self.kind == ServiceKind::Builtin {
            "(встроена в ядро)"
        } else {
            self.binary_path.as_str()
        };

        format!(
            "{:<16} {:<7} Ring {:<1} [{:<10}] PID: {:<6} Policy: {:<9} Crashes: {:<2} Up: {:<6} {}",
            self.name,
            self.kind.as_str(),
            self.execution_ring,
            self.status.as_str(),
            pid_str,
            self.restart_policy.as_str(),
            self.crash_count,
            uptime_str,
            path_str
        )
    }
}
