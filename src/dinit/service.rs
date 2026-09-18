//! Управление сервисами и демонами в Dinit (PID 1)
//!
//! Реализует супервизию процессов: запуск, остановку, отслеживание жизненного цикла,
//! перезапуск по политике (Always, OnFailure, Never) с экспоненциальным backoff,
//! контроль зависимостей и разделение по кольцам привилегий (Ring 0 / Ring 3).

#![allow(dead_code)]

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// Политика перезапуска службы при завершении или сбое
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartPolicy {
    /// Перезапускать всегда (для критических системных демонов)
    Always,
    /// Перезапускать только при аварийном завершении (ненулевой код / краш)
    OnFailure,
    /// Никогда не перезапускать (однократный запуск)
    Never,
    /// Перезапускать, пока служба не остановлена явной командой
    UnlessStopped,
}

impl RestartPolicy {
    pub fn from_str(s: &str) -> Self {
        match s {
            "always" => RestartPolicy::Always,
            "on_failure" | "on-failure" => RestartPolicy::OnFailure,
            "unless_stopped" | "unless-stopped" => RestartPolicy::UnlessStopped,
            _ => RestartPolicy::Never,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            RestartPolicy::Always => "always",
            RestartPolicy::OnFailure => "on-failure",
            RestartPolicy::Never => "never",
            RestartPolicy::UnlessStopped => "unless-stopped",
        }
    }
}

/// Текущее состояние жизненного цикла службы
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceStatus {
    /// Служба остановлена
    Stopped,
    /// Служба находится в процессе запуска
    Starting,
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
    /// Служба отключена администратором
    Disabled,
}

impl ServiceStatus {
    pub fn is_active(&self) -> bool {
        matches!(self, ServiceStatus::Running | ServiceStatus::Starting)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ServiceStatus::Stopped => "stopped",
            ServiceStatus::Starting => "starting",
            ServiceStatus::Running => "running",
            ServiceStatus::Restarting => "restarting",
            ServiceStatus::Exited(_) => "exited",
            ServiceStatus::Failed(_) => "failed",
            ServiceStatus::Terminated(_) => "terminated",
            ServiceStatus::Crashed => "crashed",
            ServiceStatus::Disabled => "disabled",
        }
    }
}

/// Дескриптор службы, управляемой супервизором Dinit
#[derive(Debug, Clone)]
pub struct ServiceDescriptor {
    /// Уникальное системное имя службы
    pub name: String,
    /// Путь к исполняемому бинарному файлу
    pub binary_path: String,
    /// Аргументы командной строки
    pub args: Vec<String>,
    /// Кольцо исполнения (0 — ядро/Ring 0, 3 — пользовательское пространство/Ring 3)
    pub execution_ring: u8,
    /// Политика перезапуска
    pub restart_policy: RestartPolicy,
    /// Текущее состояние службы
    pub status: ServiceStatus,
    /// PID процесса (если запущен)
    pub pid: Option<u32>,
    /// Идентификатор задачи в планировщике ядра
    pub task_id: Option<usize>,
    /// Имена служб, от которых зависит данный сервис
    pub dependencies: Vec<String>,
    /// Флаг автозапуска при инициализации соответствующей стадии
    pub auto_start: bool,
    /// Является ли служба критической для системы (падение ядра при отказе)
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
            binary_path: String::from(binary_path),
            args: Vec::new(),
            execution_ring,
            restart_policy: RestartPolicy::OnFailure,
            status: ServiceStatus::Stopped,
            pid: None,
            task_id: None,
            dependencies: Vec::new(),
            auto_start: true,
            is_critical: false,
            crash_count: 0,
            max_restarts: 5,
            restart_backoff_ms: 500,
            last_start_time: 0,
            last_crash_time: 0,
        }
    }

    /// Полноценный конструктор со всеми параметрами политики
    pub fn with_policy(
        name: &str,
        binary_path: &str,
        execution_ring: u8,
        policy: RestartPolicy,
        critical: bool,
    ) -> Self {
        let mut desc = Self::new(name, binary_path, execution_ring);
        desc.restart_policy = policy;
        desc.is_critical = critical;
        desc
    }

    /// Добавление зависимости
    pub fn with_dependency(mut self, dep_name: &str) -> Self {
        self.dependencies.push(String::from(dep_name));
        self
    }

    /// Проверка, готов ли сервис к перезапуску с учётом backoff
    pub fn can_restart(&self, now: u64) -> bool {
        if self.status == ServiceStatus::Disabled || self.status == ServiceStatus::Stopped {
            return false;
        }
        if self.crash_count >= self.max_restarts {
            return false;
        }
        match self.restart_policy {
            RestartPolicy::Never => false,
            RestartPolicy::UnlessStopped => true,
            RestartPolicy::Always | RestartPolicy::OnFailure => {
                now.saturating_sub(self.last_crash_time) >= self.restart_backoff_ms
            }
        }
    }

    /// Отметка успешного старта
    pub fn mark_started(&mut self, pid: u32, task_id: Option<usize>, now: u64) {
        self.status = ServiceStatus::Running;
        self.pid = Some(pid);
        self.task_id = task_id;
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
        self.task_id = None;
    }

    /// Обработка аварийного падения / ликвидации
    pub fn mark_crashed(&mut self, signal: i32, now: u64) -> bool {
        self.last_crash_time = now;
        self.crash_count += 1;
        self.restart_backoff_ms = (self.restart_backoff_ms * 2).min(10_000);
        self.pid = None;
        self.task_id = None;

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
        self.task_id = None;
    }

    /// Форматированная строка состояния для CLI и дампа
    pub fn format_status(&self, now: u64) -> String {
        let uptime_str = if self.status == ServiceStatus::Running && self.last_start_time > 0 {
            let s = now.saturating_sub(self.last_start_time) / 1000;
            format!("{}s", s)
        } else {
            String::from("-")
        };

        let pid_str = match self.pid {
            Some(p) => format!("{}", p),
            None => String::from("-"),
        };

        format!(
            "{:<16} Ring {:<1} [{:<10}] PID: {:<5} Crashes: {:<2} Up: {:<6} Path: {}",
            self.name,
            self.execution_ring,
            self.status.as_str(),
            pid_str,
            self.crash_count,
            uptime_str,
            self.binary_path
        )
    }
}
