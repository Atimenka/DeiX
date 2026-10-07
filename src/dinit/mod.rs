//! DINIT — Подсистема PID 1 для DeiX OS (Ring 0).
//!
//! Dinit живёт в пространстве ядра (Ring 0), является корневым супервизором системы
//! и управляет:
//!   - Учётом встроенных служб (подсистемы ядра: supervisor, security monitor,
//!     сетевой стек, сброс журналов, аутентификация, журналирование).
//!   - Запуском и перезапуском Process-служб с экспоненциальным backoff;
//!     служба без существующего исполняемого файла не запускается (DX-DIN-0008).
//!   - Точками монтирования файловых систем с валидацией источника и режима.
//!   - Журналом аудита безопасности (AuditLog на 1024 записи).
//!   - Эвристическим монитором угроз (ликвидация по kill_signals).
//!   - Разбором декларативного сценария init.deix (стадии early_boot, boot).

pub mod namespace;
pub mod user;
pub mod service;
pub mod mount;
pub mod audit;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use crate::spinlock::SpinLock;

use namespace::DinitNamespace;
use user::UserState;
use service::{ServiceDescriptor, ServiceKind, ServiceStatus, RestartPolicy};
use mount::{MountPoint, MountCmd};
use audit::{AuditLog, AuditOp, AuditResult};
use crate::init_parser::{BootStage, Command, InitParser, INIT_DEIX_SCRIPT};
use crate::security_monitor::HeuristicAnalysisEngine;

/// Состояние супервизора Dinit
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DinitState {
    Initializing,
    Running,
}

impl DinitState {
    pub fn as_str(&self) -> &'static str {
        match self {
            DinitState::Initializing => "initializing",
            DinitState::Running => "running",
        }
    }
}

/// Корневой супервизор процессов DeiX OS (PID 1)
pub struct Dinit {
    pub pid: u32,
    pub state: DinitState,
    pub stage: BootStage,
    pub namespace: DinitNamespace,
    pub services: BTreeMap<String, ServiceDescriptor>,
    pub mounts: BTreeMap<String, MountPoint>,
    pub users: BTreeMap<u32, UserState>,
    pub audit: AuditLog,
    pub security: HeuristicAnalysisEngine,
    pub start_time: u64,
    pub supervisor_ticks: u64,
}

impl Dinit {
    pub fn new() -> Self {
        let ns = DinitNamespace::new("root_supervisor");
        let sec = HeuristicAnalysisEngine::new("dinit_secmon_ring0");

        Self {
            pid: 1,
            state: DinitState::Initializing,
            stage: BootStage::Boot,
            namespace: ns,
            services: BTreeMap::new(),
            mounts: BTreeMap::new(),
            users: BTreeMap::new(),
            audit: AuditLog::new(1024),
            security: sec,
            start_time: crate::timer::uptime_ms(),
            supervisor_ticks: 0,
        }
    }

    /// Полная инициализация и запуск всех подсистем PID 1
    pub fn bootstrap(&mut self) {
        let now = crate::timer::uptime_ms();
        crate::serial_println!("[dinit] Инициализация супервизора PID 1 (Ring 0)...");

        // 1. Регистрация административного пользователя root (UID 0)
        let root_user = UserState::new(0, "root", now);
        self.users.insert(0, root_user);
        self.audit.record(
            now,
            1,
            0,
            AuditOp::Login,
            "root",
            AuditResult::Allowed,
            "Корневой супервизор активирован",
        );

        // 2. Регистрация точек монтирования по умолчанию
        self.mount_internal("/dev/block/by-name/system", "/system", "erofs", true);
        self.mount_internal("/dev/block/by-name/userdata", "/userdata", "ext2", false);
        self.mount_internal("devfs", "/dev", "devfs", false);
        self.mount_internal("procfs", "/proc", "procfs", true);

        // 3. Регистрация встроенных служб: эти подсистемы живут внутри ядра,
        //    отдельных исполняемых файлов и PID у них нет.
        self.register_builtin_service("pid1_core", RestartPolicy::Always, true);
        self.register_builtin_service("security_monitor", RestartPolicy::Always, true);
        self.register_builtin_service("net_daemon", RestartPolicy::UnlessStopped, false);
        self.register_builtin_service("vfs_flusher", RestartPolicy::Always, false);
        self.register_builtin_service("auth_broker", RestartPolicy::Always, true);
        self.register_builtin_service("syslogd", RestartPolicy::Always, false);

        // 4. Разбор и применение декларативного init.deix сценария
        self.apply_init_script(INIT_DEIX_SCRIPT);

        // 5. Переход по стадиям загрузки: EarlyBoot -> Boot
        self.advance_stage(BootStage::EarlyBoot);
        self.advance_stage(BootStage::Boot);

        // 6. Запуск служб стадии Boot
        self.autostart_services();

        self.state = DinitState::Running;
        crate::serial_println!(
            "[dinit] Супервизор PID 1 переведён в состояние RUNNING (активно служб: {}, монтирований: {})",
            self.services.len(),
            self.mounts.len()
        );
    }

    fn mount_internal(&mut self, src: &str, target: &str, fs_type: &str, read_only: bool) {
        let pt = MountPoint::new(target, src, fs_type, read_only);
        self.mounts.insert(String::from(target), pt);
        self.audit.record(
            crate::timer::uptime_ms(),
            self.pid,
            0,
            AuditOp::Mount,
            target,
            AuditResult::Allowed,
            if read_only { "Зарегистрирована точка монтирования (ro)" } else { "Зарегистрирована точка монтирования (rw)" },
        );
    }

    fn register_builtin_service(&mut self, name: &str, policy: RestartPolicy, critical: bool) {
        let mut desc = ServiceDescriptor::builtin(name, policy, critical);
        desc.auto_start = true;
        self.services.insert(String::from(name), desc);
    }

    /// Жива ли подсистема ядра, стоящая за встроенной службой.
    /// Возвращает Ok(()) или причину, по которой служба не активна.
    fn builtin_probe(name: &str) -> Result<(), &'static str> {
        match name {
            // Сетевой стек активен только при наличии инициализированного адаптера.
            "net_daemon" => {
                if crate::rtl8139::is_ready() {
                    Ok(())
                } else {
                    Err("сетевой адаптер RTL8139 не обнаружен")
                }
            }
            // Остальные подсистемы (супервизор, монитор безопасности, сброс
            // журналов, аутентификация, журналирование) входят в состав ядра
            // и активны с момента его инициализации.
            _ => Ok(()),
        }
    }

    /// Разбор сценария init.deix и наполнение таблиц
    pub fn apply_init_script(&mut self, script: &str) {
        let mut parser = InitParser::new();
        let registry = parser.parse(script);

        for (stage, commands) in registry.into_iter() {
            for cmd in commands {
                match cmd {
                    Command::Mount(m) => {
                        let read_only = m.mode.as_token() == "ro";
                        let mount_cmd = MountCmd::new(&m.src, &m.dst, &m.fs_type, read_only);
                        if let Err(err) = mount_cmd.validate_with_vault(stage) {
                            crate::serial_println!("[dinit:vault] Отклонено монтирование: {}", err);
                            self.audit.record(
                                crate::timer::uptime_ms(),
                                1,
                                0,
                                AuditOp::VaultRejection,
                                &m.dst,
                                AuditResult::Denied,
                                err,
                            );
                            continue;
                        }
                        self.mount_internal(&m.src, &m.dst, &m.fs_type, read_only);
                    }
                    Command::Service(s) => {
                        if !self.services.contains_key(&s.name) {
                            let desc = ServiceDescriptor::new(&s.name, &s.path, s.execution_ring);
                            self.services.insert(s.name.clone(), desc);
                        }
                    }
                }
            }
        }
    }

    /// Смена стадии загрузки ОС
    pub fn advance_stage(&mut self, new_stage: BootStage) {
        self.stage = new_stage;
        let now = crate::timer::uptime_ms();
        self.audit.record(
            now,
            1,
            0,
            AuditOp::StageAdvance,
            new_stage.as_token(),
            AuditResult::Allowed,
            "Переход на новую стадию загрузки",
        );
        crate::serial_println!("[dinit] Фаза загрузки изменена на: {}", new_stage.as_token());
    }

    /// Запуск всех служб, отмеченных для автозапуска.
    ///
    /// Служба, образ которой не найден или не загрузился, остаётся
    /// остановленной и печатает причину — статус Running ей не ставится.
    pub fn autostart_services(&mut self) {
        let names: Vec<String> = self.services.keys().cloned().collect();
        for name in names {
            let wanted = self
                .services
                .get(&name)
                .map(|d| d.auto_start && d.status == ServiceStatus::Stopped)
                .unwrap_or(false);
            if !wanted {
                continue;
            }
            let kind = self.services.get(&name).map(|d| d.kind).unwrap_or(ServiceKind::Process);
            match self.spawn_service(&name, "Автозапуск системной службы") {
                Ok(pid) => {
                    if kind == ServiceKind::Builtin {
                        crate::serial_println!("[dinit] Встроенная служба '{}' активна (подсистема ядра)", name);
                    } else {
                        crate::serial_println!("[dinit] Служба '{}' запущена с PID {}", name, pid);
                    }
                }
                Err(e) => {
                    crate::serial_println!("[dinit] Служба '{}' НЕ запущена: {}", name, e);
                }
            }
        }
    }

    /// Периодический квант супервизора (Heartbeat & Crash Recovery)
    pub fn supervisor_tick(&mut self) {
        self.supervisor_ticks += 1;
        let now = crate::timer::uptime_ms();

        // Актуализация состояния встроенных служб по живости их подсистем
        // (например, net_daemon активируется после инициализации адаптера).
        for (name, desc) in self.services.iter_mut() {
            if desc.kind != ServiceKind::Builtin {
                continue;
            }
            let alive = Self::builtin_probe(name).is_ok();
            if alive && desc.status == ServiceStatus::Stopped && desc.auto_start {
                desc.mark_builtin_running(now);
            } else if !alive && desc.status == ServiceStatus::Running {
                desc.mark_stopped();
            }
        }

        // 0. Обнаружение падений Process-служб: служба числится Running,
        //    а её процесс уже мёртв или исчез из таблицы. Это и есть
        //    heartbeat супервизора — без него RestartPolicy не на что
        //    реагировать. Встроенные службы пропускаются: у них нет PID.
        for (name, desc) in self.services.iter_mut() {
            if desc.kind == ServiceKind::Builtin || desc.status != ServiceStatus::Running {
                continue;
            }
            let Some(pid) = desc.pid else { continue };
            let dead = match crate::process::info(pid) {
                Ok(info) => {
                    info.state == crate::process::ProcessState::Zombie
                        || info.state == crate::process::ProcessState::Reaped
                }
                Err(_) => true,
            };
            if !dead {
                continue;
            }
            let exit_code = crate::process::info(pid).ok().and_then(|i| i.exit_code).unwrap_or(-1);

            // Штатное завершение (код 0) при политике без принудительного
            // перезапуска — это не авария.
            if exit_code == 0
                && matches!(desc.restart_policy, RestartPolicy::OnFailure)
            {
                desc.mark_exited(0);
                let _ = crate::process::reap(pid);
                crate::serial_println!(
                    "[dinit] Служба '{}' (pid {}) завершилась штатно (код 0)",
                    name, pid
                );
                continue;
            }

            let will_restart = desc.mark_crashed(exit_code, now);
            let _ = crate::process::reap(pid);
            self.audit.record(
                now,
                pid,
                0,
                AuditOp::ServiceCrash,
                name,
                AuditResult::Failed,
                "Процесс службы завершился аварийно",
            );

            if will_restart {
                crate::diag::error_with(
                    crate::diag::ErrorCode::new(crate::diag::Subsystem::Dinit, 2),
                    crate::diag::Action::Retry,
                    &alloc::format!(
                        "служба {} (pid {}) завершилась аварийно (код {}), перезапуск через {} мс",
                        name, pid, exit_code, desc.restart_backoff_ms
                    ),
                );
            } else if desc.is_critical {
                crate::diag::critical_with(
                    crate::diag::ErrorCode::new(crate::diag::Subsystem::Dinit, 6),
                    crate::diag::Action::DegradeSubsystem,
                    &alloc::format!(
                        "критическая служба {} падала {} раз — перезапуски прекращены",
                        name, desc.crash_count
                    ),
                );
            } else {
                crate::diag::error_with(
                    crate::diag::ErrorCode::new(crate::diag::Subsystem::Dinit, 6),
                    crate::diag::Action::DegradeSubsystem,
                    &alloc::format!(
                        "служба {} падала {} раз — перезапуски прекращены",
                        name, desc.crash_count
                    ),
                );
            }
        }

        // 1. Перезапуск служб, у которых подошёл backoff
        let mut to_restart = Vec::new();
        for (name, desc) in self.services.iter() {
            if desc.status == ServiceStatus::Restarting && desc.can_restart(now) {
                to_restart.push(name.clone());
            }
        }

        for name in to_restart {
            match self.spawn_service(&name, "Автоматический перезапуск службы по политике супервизора") {
                Ok(pid) => {
                    self.audit.record(
                        now,
                        pid,
                        0,
                        AuditOp::ServiceRestart,
                        &name,
                        AuditResult::Allowed,
                        "Служба перезапущена по политике супервизора",
                    );
                    crate::serial_println!(
                        "[dinit] Служба '{}' перезапущена (новый PID {})",
                        name, pid
                    );
                }
                Err(e) => {
                    crate::diag::error(
                        crate::diag::ErrorCode::new(crate::diag::Subsystem::Dinit, 1),
                        &alloc::format!("dinit: служба {} не перезапущена: {}", name, e),
                    );
                }
            }
        }

        // 2. Обработка очереди сигналов ликвидации от Security Monitor
        while let Some(kill) = self.security.kill_signals.pop() {
            crate::diag::critical_with(
                crate::diag::ErrorCode::new(crate::diag::Subsystem::Security, 5),
                crate::diag::Action::TerminateProcess,
                &alloc::format!("security: ликвидация pid {} — {}", kill.pid, kill.reason),
            );
            crate::serial_println!("[dinit:security] ИСПОЛНЕНИЕ: {}", kill.describe());
            self.audit.record(
                now,
                kill.pid,
                0,
                AuditOp::KillDispatched,
                &kill.reason,
                AuditResult::Terminated,
                "Ликвидация вредоносного процесса",
            );
            // Ликвидация в планировщике ядра
            crate::sched::terminate(kill.pid as usize);

            // Обновление состояния службы, если этот PID принадлежал ей
            for (_, desc) in self.services.iter_mut() {
                if desc.pid == Some(kill.pid) {
                    desc.mark_crashed(kill.signal, now);
                }
            }
        }
    }

    /// Регистрация пользовательской сессии
    pub fn register_user(&mut self, uid: u32, username: &str) {
        let now = crate::timer::uptime_ms();
        let user = UserState::new(uid, username, now);
        self.users.insert(uid, user);
        self.audit.record(
            now,
            self.pid,
            uid,
            AuditOp::Login,
            username,
            AuditResult::Allowed,
            "Успешный вход пользователя",
        );
        crate::serial_println!("[dinit] Пользователь '{}' (UID {}) зарегистрирован", username, uid);
    }

    /// Запуск службы.
    ///
    /// Для встроенной службы процесс не создаётся: проверяется живость
    /// подсистемы ядра, и служба помечается активной (PID: kernel).
    ///
    /// Для Process-службы создаётся процесс в менеджере процессов,
    /// загружается образ и создаётся задача планировщика, привязанная
    /// к PID. Служба, образ которой отсутствует или не загрузился,
    /// запущенной НЕ считается.
    ///
    /// Возвращает PID при успехе (0 для встроенных служб).
    fn spawn_service(&mut self, name: &str, reason: &str) -> Result<u32, String> {
        let now = crate::timer::uptime_ms();

        let (kind, binary_path) = {
            let desc = self.services.get(name).ok_or_else(|| String::from("Служба не найдена"))?;
            (desc.kind, desc.binary_path.clone())
        };

        if kind == ServiceKind::Builtin {
            Self::builtin_probe(name).map_err(String::from)?;
            if let Some(desc) = self.services.get_mut(name) {
                desc.mark_builtin_running(now);
            }
            self.audit.record(now, self.pid, 0, AuditOp::ServiceSpawn, name, AuditResult::Allowed, reason);
            return Ok(0);
        }

        if binary_path.is_empty() {
            return Err(String::from("у службы не задан путь к бинарнику"));
        }
        if !crate::vfs::exists(&binary_path) {
            crate::diag::error_with(
                crate::diag::ErrorCode::new(crate::diag::Subsystem::Dinit, 8),
                crate::diag::Action::DegradeSubsystem,
                &alloc::format!("служба {}: исполняемый файл {} отсутствует", name, binary_path),
            );
            return Err(alloc::format!("образ службы не найден: {}", binary_path));
        }

        let req = crate::process::SpawnRequest {
            name: String::from(name),
            // Службы Dinit исполняются от имени супервизора (UID 0).
            uid: 0,
            parent: self.pid,
            caps: crate::process::cap::SYSTEM,
            cwd: String::from("/userdata"),
            image_path: binary_path,
        };

        let pid = crate::process::spawn(&req).map_err(|e| e.message())?;

        match crate::process::exec(pid) {
            Ok(()) => {}
            Err(e) => {
                let _ = crate::process::kill(pid, -1);
                let _ = crate::process::reap(pid);
                return Err(alloc::format!("образ не запущен: {}", e.message()));
            }
        }

        if let Some(desc) = self.services.get_mut(name) {
            desc.mark_started(pid, now);
        }
        self.namespace.attach_process(pid);
        self.audit.record(
            now,
            pid,
            0,
            AuditOp::ServiceSpawn,
            name,
            AuditResult::Allowed,
            reason,
        );
        Ok(pid)
    }

    /// Запуск службы по имени
    pub fn start_service(&mut self, name: &str) -> Result<u32, String> {
        {
            let desc = self.services.get(name).ok_or_else(|| String::from("Служба не найдена"))?;
            if desc.status.is_active() {
                return Err(String::from("Служба уже запущена"));
            }
        }
        self.spawn_service(name, "Ручной запуск службы")
    }

    /// Остановка службы по имени
    pub fn stop_service(&mut self, name: &str) -> Result<(), String> {
        let now = crate::timer::uptime_ms();
        let desc = self
            .services
            .get_mut(name)
            .ok_or_else(|| String::from("Служба не найдена"))?;
        if desc.kind == ServiceKind::Builtin {
            return Err(String::from(
                "встроенная служба является подсистемой ядра и не может быть остановлена",
            ));
        }
        let old_pid = desc.pid;
        desc.mark_stopped();
        if let Some(p) = old_pid {
            self.namespace.detach_process(p);
            // Снимаем и процесс, и его задачу планировщика.
            let _ = crate::process::kill(p, 0);
            let _ = crate::process::reap(p);
        }
        self.audit.record(
            now,
            old_pid.unwrap_or(0),
            0,
            AuditOp::ServiceStop,
            name,
            AuditResult::Allowed,
            "Остановка службы оператором",
        );
        Ok(())
    }

    /// Перезапуск службы по имени
    pub fn restart_service(&mut self, name: &str) -> Result<u32, String> {
        self.stop_service(name)?;
        self.start_service(name)
    }

}

/// Глобальный экземпляр супервизора ядра
pub static DINIT: SpinLock<Option<Dinit>> = SpinLock::new(None);

/// Инициализация подсистемы Dinit ядром
pub fn init() {
    let mut lock = DINIT.lock();
    if lock.is_none() {
        let mut dinit = Dinit::new();
        dinit.bootstrap();
        *lock = Some(dinit);
    }
}

/// Периодический тик супервизора
pub fn tick() {
    if let Some(mut lock) = DINIT.try_lock() {
        if let Some(dinit) = lock.as_mut() {
            dinit.supervisor_tick();
        }
    }
}

/// Обработчик команд CLI: `dinit [status|services|mounts|users|audit|stage|security|reload|start|stop|restart]`
pub fn cmd_dinit(line: &str) {
    let mut parts = line.split_whitespace();
    let subcmd = parts.next().unwrap_or("status");

    let mut lock = DINIT.lock();
    let dinit = match lock.as_mut() {
        Some(d) => d,
        None => {
            crate::println!("  [dinit] Ошибка: супервизор Dinit не инициализирован");
            return;
        }
    };

    let now = crate::timer::uptime_ms();

    match subcmd {
        "status" => {
            crate::println!("=== DINIT (PID 1) — DeiX OS Supervisor ===");
            crate::println!("  Состояние:     {}", dinit.state.as_str());
            crate::println!("  Текущая стадия: {}", dinit.stage.as_token());
            crate::println!("  Аптайм PID 1:  {} сек", now.saturating_sub(dinit.start_time) / 1000);
            crate::println!("  Тики супервизора: {}", dinit.supervisor_ticks);
            crate::println!("  Активных служб:    {} (всего зарегистрировано: {})",
                dinit.services.values().filter(|s| s.status.is_active()).count(),
                dinit.services.len());
            crate::println!("  Точек монтирования: {}", dinit.mounts.len());
            crate::println!("  Пользовательских сессий: {}", dinit.users.len());
            crate::println!("  Процессов под супервизией ({}): {}",
                dinit.namespace.name,
                dinit.namespace.active_pids.len());
            crate::println!("  Записей аудита:    {} (всего событий: {}, разрешено: {}, отказано: {})",
                dinit.audit.len(),
                dinit.audit.stats.total_events,
                dinit.audit.stats.allowed_events,
                dinit.audit.stats.denied_events);
            crate::println!("  Нарушений: {}, угроз ликвидировано: {}",
                dinit.audit.stats.violations,
                dinit.audit.stats.threats_intercepted);
            crate::println!("  Security Monitor:  порог риска {:.2}, проанализировано событий: {}",
                dinit.security.risk_threshold,
                dinit.security.events_analyzed);
        }

        "services" | "svc" => {
            let action = parts.next().unwrap_or("list");
            match action {
                "list" => {
                    crate::println!("=== СЛУЖБЫ И ДЕМОНЫ DINIT (PID 1) ===");
                    for desc in dinit.services.values() {
                        crate::println!("  {}", desc.format_status(now));
                    }
                }
                "start" => {
                    if let Some(name) = parts.next() {
                        match dinit.start_service(name) {
                            Ok(0) => crate::println!("  [dinit] Встроенная служба '{}' активна (подсистема ядра)", name),
                            Ok(pid) => crate::println!("  [dinit] Служба '{}' запущена с PID {}", name, pid),
                            Err(e) => crate::println!("  [dinit] Ошибка запуска: {}", e),
                        }
                    } else {
                        crate::println!("  Использование: dinit services start <имя>");
                    }
                }
                "stop" => {
                    if let Some(name) = parts.next() {
                        match dinit.stop_service(name) {
                            Ok(()) => crate::println!("  [dinit] Служба '{}' остановлена", name),
                            Err(e) => crate::println!("  [dinit] Ошибка остановки: {}", e),
                        }
                    } else {
                        crate::println!("  Использование: dinit services stop <имя>");
                    }
                }
                "restart" => {
                    if let Some(name) = parts.next() {
                        match dinit.restart_service(name) {
                            Ok(0) => crate::println!("  [dinit] Встроенная служба '{}' активна (подсистема ядра)", name),
                            Ok(pid) => crate::println!("  [dinit] Служба '{}' перезапущена с PID {}", name, pid),
                            Err(e) => crate::println!("  [dinit] Ошибка перезапуска: {}", e),
                        }
                    } else {
                        crate::println!("  Использование: dinit services restart <имя>");
                    }
                }
                _ => crate::println!("  Неизвестное действие. Доступно: list, start, stop, restart"),
            }
        }

        "mounts" | "mnt" => {
            crate::println!("=== ТОЧКИ МОНТИРОВАНИЯ (KERNEL SECURITY VAULT) ===");
            for pt in dinit.mounts.values() {
                crate::println!("  {}", pt.format_line());
            }
        }

        "users" | "u" => {
            crate::println!("=== АКТИВНЫЕ ПОЛЬЗОВАТЕЛИ В NAMESPACE ===");
            for u in dinit.users.values() {
                crate::println!("  {}", u.format_line(now));
            }
        }

        "audit" => {
            let action = parts.next().unwrap_or("tail");
            match action {
                "tail" => {
                    let count = parts.next().and_then(|s| s.parse().ok()).unwrap_or(15);
                    crate::println!("=== ЖУРНАЛ АУДИТА DINIT (последние {} записей) ===", count);
                    for entry in dinit.audit.tail(count) {
                        crate::println!("  {}", entry.format_line());
                    }
                }
                "violations" => {
                    crate::println!("=== ЖУРНАЛ НАРУШЕНИЙ И ОТКЛОНЁННЫХ ОПЕРАЦИЙ ===");
                    let viols = dinit.audit.violations();
                    if viols.is_empty() {
                        crate::println!("  Нарушений не зафиксировано");
                    } else {
                        for entry in viols {
                            crate::println!("  {}", entry.format_line());
                        }
                    }
                }
                "clear" => {
                    dinit.audit.clear();
                    crate::println!("  [dinit] Журнал аудита очищен");
                }
                _ => crate::println!("  Использование: dinit audit [tail|violations|clear]"),
            }
        }

        "security" | "sec" => {
            crate::println!("=== HEURISTIC SECURITY MONITOR (Ring 3 / Ring 0) ===");
            crate::println!("  ID монитора:       {}", dinit.security.monitor_id);
            crate::println!("  Порог риска:       {:.2}", dinit.security.risk_threshold);
            crate::println!("  Событий проверено: {}", dinit.security.events_analyzed);
            crate::println!("  Критических тревог: {}", dinit.security.critical_alerts);
            crate::println!("  Отслежено процессов: {}", dinit.security.profiles.len());
            crate::println!("  В очереди SIGKILL:  {}", dinit.security.kill_signals.len());
        }

        "stage" => {
            if let Some(st_str) = parts.next() {
                match BootStage::from_token(st_str) {
                    Ok(st) => {
                        dinit.advance_stage(st);
                        crate::println!("  [dinit] Фаза переключена на: {}", st.as_token());
                    }
                    Err(e) => crate::println!("  [dinit] Недопустимая фаза: {}", e.message()),
                }
            } else {
                crate::println!("  Текущая стадия: {}", dinit.stage.as_token());
                crate::println!("  Допустимые: early_boot, boot");
            }
        }

        "reload" => {
            crate::println!("  [dinit] Перезагрузка сценария init.deix...");
            dinit.apply_init_script(INIT_DEIX_SCRIPT);
            dinit.autostart_services();
            crate::println!("  [dinit] Сценарий перезагружен успешно");
        }

        _ => {
            crate::println!("Использование команды dinit:");
            crate::println!("  dinit status                - сводный статус супервизора PID 1");
            crate::println!("  dinit services [list|start|stop|restart] - управление службами");
            crate::println!("  dinit mounts                - список активных точек монтирования");
            crate::println!("  dinit users                 - активные пользователи");
            crate::println!("  dinit audit [tail|violations|clear] - журнал аудита");
            crate::println!("  dinit security              - состояние эвристического монитора");
            crate::println!("  dinit stage <name>          - переключение стадии загрузки");
            crate::println!("  dinit reload                - перечитать сценарий init.deix");
        }
    }
}
