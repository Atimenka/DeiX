//! DINIT — Подсистема PID 1 для DeiX OS (Ring 0).
//!
//! Dinit живёт в пространстве ядра (Ring 0), является корневым супервизором системы,
//! обладает собственным изолированным пространством имён (DinitNamespace) и управляет:
//!   - Запуском и перезапуском системных служб (Ring 0 / Ring 3) с экспоненциальным backoff.
//!   - Точками монтирования файловых систем с валидацией через KERNEL SECURITY VAULT.
//!   - Пользовательскими сессиями и изоляцией каталогов (/users/<username> vs /system).
//!   - Журналом аудита безопасности (AuditLog на 1024 записи).
//!   - Эвристическим монитором угроз (перехват ransomware и code injection -> SIGKILL).
//!   - Разбором и исполнением декларативного сценария init.deix (стадии BootStage).
//!
//! Иерархия контроля:
//!   Загрузчик -> Ядро -> Dinit (PID 1, Ring 0) -> Пользовательские процессы (Ring 3).

pub mod namespace;
pub mod user;
pub mod service;
pub mod mount;
pub mod authorize;
pub mod audit;
pub mod api;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use crate::spinlock::SpinLock;

use namespace::{Capabilities, DinitNamespace};
use user::UserState;
use service::{ServiceDescriptor, ServiceStatus, RestartPolicy};
use mount::{MountPoint, MountCmd};
use authorize::{FileOp, AccessError, check_permission};
use audit::{AuditLog, AuditOp, AuditResult};
use crate::init_parser::{BootStage, Command, InitParser};
use crate::security_monitor::{HeuristicAnalysisEngine, SecurityEvent};

/// Состояние супервизора Dinit
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DinitState {
    Initializing,
    Running,
    Suspended,
    Stopped,
    Faulted,
}

impl DinitState {
    pub fn as_str(&self) -> &'static str {
        match self {
            DinitState::Initializing => "initializing",
            DinitState::Running => "running",
            DinitState::Suspended => "suspended",
            DinitState::Stopped => "stopped",
            DinitState::Faulted => "faulted",
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
    pub next_pid: u32,
}

extern "C" fn service_dummy_loop() {
    loop {
        crate::sched::sleep_ms(1000);
    }
}

impl Dinit {
    pub fn new() -> Self {
        let caps = Capabilities::default_dinit();
        let ns = DinitNamespace::new(1, "root_supervisor", caps);
        let sec = HeuristicAnalysisEngine::new("dinit_secmon_ring0");

        Self {
            pid: 1,
            state: DinitState::Initializing,
            stage: BootStage::InitBoot,
            namespace: ns,
            services: BTreeMap::new(),
            mounts: BTreeMap::new(),
            users: BTreeMap::new(),
            audit: AuditLog::new(1024),
            security: sec,
            start_time: crate::timer::uptime_ms(),
            supervisor_ticks: 0,
            next_pid: 2,
        }
    }

    /// Инициализация подсистем PID 1 (создание корневого пользователя root)
    pub fn bootstrap(&mut self) {
        let now = crate::timer::uptime_ms();
        crate::serial_println!("[dinit] Инициализация супервизора PID 1 (Ring 0)...");

        // Регистрация административного пользователя root (UID 0)
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
    }

    /// Инициализация монтирований и служб из прочитанного init.deix
    pub fn init_with(&mut self, init_text: &str) {
        crate::serial_println!("[dinit] init.deix прочитан из /init_boot ({} байт)", init_text.len());
        self.apply_init_script(init_text);

        // Переход по стадиям загрузки: InitBoot -> VendorBoot -> Boot
        self.advance_stage(BootStage::VendorBoot);
        self.advance_stage(BootStage::Boot);

        // Запуск служб стадии Boot
        self.autostart_services();

        self.state = DinitState::Running;
        crate::serial_println!(
            "[dinit] Супервизор PID 1 переведён в состояние RUNNING (активно служб: {}, монтирований: {})",
            self.services.len(),
            self.mounts.len()
        );
    }

    fn mount_internal(
        &mut self,
        src: &str,
        target: &str,
        fs_type: &str,
        read_only: bool,
        now: u64,
    ) {
        let pt = MountPoint::new(target, src, fs_type, read_only, now);
        self.mounts.insert(String::from(target), pt);
    }

    fn register_core_service(
        &mut self,
        name: &str,
        path: &str,
        ring: u8,
        policy: RestartPolicy,
        critical: bool,
    ) {
        let mut desc = ServiceDescriptor::with_policy(name, path, ring, policy, critical);
        desc.auto_start = true;
        self.services.insert(String::from(name), desc);
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
                        self.mount_internal(&m.src, &m.dst, &m.fs_type, read_only, crate::timer::uptime_ms());
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
            AuditOp::ServiceSpawn,
            new_stage.as_token(),
            AuditResult::Allowed,
            "Переход на новую стадию загрузки",
        );
        crate::serial_println!("[dinit] Фаза загрузки изменена на: {}", new_stage.as_token());
    }

    /// Запуск всех служб, отмеченных для автозапуска
    pub fn autostart_services(&mut self) {
        let now = crate::timer::uptime_ms();
        let names: Vec<String> = self.services.keys().cloned().collect();
        for name in names {
            if let Some(desc) = self.services.get_mut(&name) {
                if desc.auto_start && desc.status == ServiceStatus::Stopped {
                    let pid = self.next_pid;
                    self.next_pid += 1;
                    let prio = match desc.execution_ring {
                        0 => crate::sched::Priority::High,
                        3 => crate::sched::Priority::Normal,
                        _ => crate::sched::Priority::Idle,
                    };
                    crate::sched::spawn_pid(&desc.name, service_dummy_loop, pid, prio);
                    desc.mark_started(pid, Some(pid), now);
                    self.namespace.attach_process(pid);
                    self.audit.record(
                        now,
                        pid,
                        0,
                        AuditOp::ServiceSpawn,
                        &desc.name,
                        AuditResult::Allowed,
                        "Автозапуск системной службы",
                    );
                    crate::serial_println!("[dinit] Служба '{}' запущена с PID {} (Priority: {:?})", desc.name, pid, prio);
                }
            }
        }
    }

    /// Периодический квант супервизора (Heartbeat & Crash Recovery)
    pub fn supervisor_tick(&mut self) {
        self.supervisor_ticks += 1;
        let now = crate::timer::uptime_ms();

        // 1. Проверка состояния и перезапуск упавших служб
        let mut to_restart = Vec::new();
        for (name, desc) in self.services.iter() {
            if desc.status == ServiceStatus::Restarting && desc.can_restart(now) {
                to_restart.push(name.clone());
            }
        }

        for name in to_restart {
            let pid = self.next_pid;
            self.next_pid += 1;
            if let Some(desc) = self.services.get_mut(&name) {
                desc.mark_started(pid, None, now);
                self.namespace.attach_process(pid);
                self.audit.record(
                    now,
                    pid,
                    0,
                    AuditOp::ServiceRestart,
                    &desc.name,
                    AuditResult::Allowed,
                    "Автоматический перезапуск службы по политике супервизора",
                );
                crate::serial_println!("[dinit] Служба '{}' перезапущена (новый PID {})", desc.name, pid);
            }
        }

        // 2. Обработка очереди сигналов ликвидации от Security Monitor
        while let Some(kill) = self.security.kill_signals.pop() {
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
            crate::sched::terminate_by_pid(kill.pid);

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

    /// Завершение пользовательской сессии
    pub fn unregister_user(&mut self, uid: u32) {
        let now = crate::timer::uptime_ms();
        if let Some(user) = self.users.remove(&uid) {
            self.audit.record(
                now,
                self.pid,
                uid,
                AuditOp::Logout,
                &user.username,
                AuditResult::Allowed,
                "Выход пользователя из системы",
            );
            crate::serial_println!("[dinit] Пользователь '{}' отключён", user.username);
        }
    }

    /// Проверка прав доступа через центральную матрицу
    pub fn check_permission(
        &mut self,
        uid: u32,
        username: &str,
        op: FileOp,
        path: &str,
    ) -> Result<(), AccessError> {
        let res = check_permission(uid, username, op, path);
        if let Err(err) = res {
            let now = crate::timer::uptime_ms();
            self.audit.record(
                now,
                self.pid,
                uid,
                AuditOp::Violation,
                path,
                AuditResult::Denied,
                err.as_str(),
            );
        }
        res
    }

    /// Запуск службы по имени
    pub fn start_service(&mut self, name: &str) -> Result<u32, &'static str> {
        let now = crate::timer::uptime_ms();
        let desc = self.services.get_mut(name).ok_or("Служба не найдена")?;
        if desc.status.is_active() {
            return Err("Служба уже запущена");
        }
        let pid = self.next_pid;
        self.next_pid += 1;
        desc.mark_started(pid, None, now);
        self.namespace.attach_process(pid);
        self.audit.record(
            now,
            pid,
            0,
            AuditOp::ServiceSpawn,
            name,
            AuditResult::Allowed,
            "Ручной запуск службы",
        );
        Ok(pid)
    }

    /// Остановка службы по имени
    pub fn stop_service(&mut self, name: &str) -> Result<(), &'static str> {
        let now = crate::timer::uptime_ms();
        let desc = self.services.get_mut(name).ok_or("Служба не найдена")?;
        let old_pid = desc.pid;
        desc.mark_stopped();
        if let Some(p) = old_pid {
            self.namespace.detach_process(p);
            crate::sched::terminate_by_pid(p);
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
    pub fn restart_service(&mut self, name: &str) -> Result<u32, &'static str> {
        self.stop_service(name)?;
        self.start_service(name)
    }

    /// Анализ события безопасности эвристическим движком
    pub fn report_security_event(&mut self, event: &SecurityEvent) -> bool {
        let flagged = self.security.process_event(event);
        if flagged {
            let now = crate::timer::uptime_ms();
            self.audit.record(
                now,
                event.pid,
                0,
                AuditOp::ThreatAlert,
                &event.target_path,
                AuditResult::Terminated,
                "Эвристический монитор зафиксировал критическую угрозу",
            );
        }
        flagged
    }
}

/// Глобальный экземпляр супервизора ядра
pub static DINIT: SpinLock<Option<Dinit>> = SpinLock::new(None);

/// Инициализация подсистемы Dinit ядром с текстом init.deix
pub fn init_with(init_text: &str) {
    let mut lock = DINIT.lock();
    if lock.is_none() {
        let mut dinit = Dinit::new();
        dinit.bootstrap();
        dinit.init_with(init_text);
        *lock = Some(dinit);
    }
}

/// Инициализация подсистемы Dinit по умолчанию (fallback)
pub fn init() {
    let text = crate::bootchain::get_init_deix().unwrap_or_else(|| String::from(crate::init_parser::FALLBACK_INIT));
    init_with(&text);
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
            crate::println!("  Записей аудита:    {} (нарушений: {}, угроз ликвидировано: {})",
                dinit.audit.len(),
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
                crate::println!("  Допустимые: init_boot, vendor_boot, boot, recovery, fastbootd, edl");
            }
        }

        "reload" => {
            crate::println!("  [dinit] Перезагрузка сценария init.deix...");
            let script = crate::bootchain::get_init_deix().unwrap_or_else(|| String::from(crate::init_parser::FALLBACK_INIT));
            dinit.apply_init_script(&script);
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
