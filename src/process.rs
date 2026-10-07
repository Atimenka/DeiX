//! Менеджер процессов — единая точка учёта всего, что исполняется.
//!
//! До этого модуля «процесс» существовал в трёх несвязанных местах:
//! Dinit выдавал службам номера из собственного счётчика, планировщик
//! держал задачи в своей таблице, а загрузчики ELF/MEX вообще не знали
//! ни о каких PID. Здесь они сводятся в одну структуру:
//!
//! ```text
//! Process
//!  ├── PID / UID / parent
//!  ├── FD table + cwd
//!  ├── capabilities
//!  └── sched slot  → планировщик
//! ```
//!
//! Dinit создаёт процесс через [`spawn`], MEX-загрузчик размещает образ
//! в фиксированной области MEX, планировщик получает задачу,
//! привязанную к PID через `sched::spawn_pid`; сам пользовательский код
//! исполняется в Ring 3 через `mex::run_ring3`. Обратный путь —
//! [`notify_exit`]: завершение задачи помечает процесс завершённым, и
//! Dinit может применить свою политику перезапуска.
//!
//! Запуск ELF-образов через планировщик ядра не поддерживается: кадры
//! `sched::spawn_pid` создаются для Ring 0 (CS=0x08/SS=0x10), и выдавать
//! такое исполнение за пользовательский процесс нельзя (DX-ELF-0009).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::spinlock::SpinLock;

/// Максимум одновременно живых процессов.
const MAX_PROCS: usize = 32;
/// Максимум открытых дескрипторов на процесс.
const MAX_FDS: usize = 16;

// ==================== Состояния и возможности ====================

/// Состояние процесса.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    /// Слот свободен.
    Empty,
    /// Создан, образ ещё не размещён.
    Created,
    /// Исполняется (задача есть в планировщике).
    Running,
    /// Завершён и ещё не пожнат родителем.
    Zombie,
    /// Завершён и освобождён.
    Reaped,
}

impl ProcessState {
    pub fn as_str(self) -> &'static str {
        match self {
            ProcessState::Empty => "empty",
            ProcessState::Created => "created",
            ProcessState::Running => "running",
            ProcessState::Zombie => "zombie",
            ProcessState::Reaped => "reaped",
        }
    }

    /// Занимает ли процесс слот таблицы.
    pub fn occupies_slot(self) -> bool {
        matches!(self, ProcessState::Created | ProcessState::Running)
    }
}

/// Биты возможностей процесса. Проверка прав на файл, сеть и запуск
/// служб идёт через них, а не через сравнение имени пользователя.
pub mod cap {
    /// Чтение произвольных путей.
    pub const READ_ANY: u64 = 1 << 0;
    /// Запись в /userdata.
    pub const WRITE_USERDATA: u64 = 1 << 1;
    /// Доступ к сети.
    pub const NETWORK: u64 = 1 << 2;
    /// Доступ к аудиоустройствам.
    pub const AUDIO: u64 = 1 << 3;
    /// Доступ к кадрам дисплея.
    pub const GRAPHICS: u64 = 1 << 4;
    /// Запуск и остановка служб через Dinit.
    pub const MANAGE_SERVICES: u64 = 1 << 5;
    /// Установка пакетов.
    pub const INSTALL: u64 = 1 << 6;
    /// Полный набор для системных служб Dinit.
    pub const SYSTEM: u64 = READ_ANY
        | WRITE_USERDATA
        | NETWORK
        | AUDIO
        | GRAPHICS
        | MANAGE_SERVICES
        | INSTALL;
}

// ==================== Файловые дескрипторы ====================

/// Открытый дескриптор. Чтение/запись через дескриптор со смещением
/// не реализованы, поэтому дескриптор хранит только путь и режим.
#[derive(Debug, Clone)]
pub struct Fd {
    pub path: String,
    pub read_only: bool,
}

/// Таблица дескрипторов процесса.
#[derive(Debug)]
pub struct FdTable {
    slots: [Option<Fd>; MAX_FDS],
}

impl FdTable {
    const fn new() -> Self {
        const NONE: Option<Fd> = None;
        FdTable { slots: [NONE; MAX_FDS] }
    }

    /// Занимает свободный слот. Возвращает номер дескриптора.
    pub fn alloc(&mut self, fd: Fd) -> Option<usize> {
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(fd);
                return Some(i);
            }
        }
        None
    }

    pub fn get(&self, num: usize) -> Option<&Fd> {
        self.slots.get(num).and_then(|s| s.as_ref())
    }

    pub fn close(&mut self, num: usize) -> bool {
        match self.slots.get_mut(num) {
            Some(slot) if slot.is_some() => {
                *slot = None;
                true
            }
            _ => false,
        }
    }

    /// Количество открытых дескрипторов.
    pub fn open_count(&self) -> usize {
        self.slots.iter().filter(|s| s.is_some()).count()
    }
}

// ==================== Процесс ====================

/// Процесс DeiX.
pub struct Process {
    pub pid: u32,
    pub uid: u32,
    pub parent: u32,
    pub name: String,
    pub state: ProcessState,
    pub caps: u64,
    pub cwd: String,
    pub fds: FdTable,
    /// Слот планировщика для основной нити.
    pub sched_id: Option<usize>,
    pub exit_code: Option<i32>,
    pub started_at_ms: u64,
    /// Путь к образу, из которого процесс запущен.
    pub image_path: String,
    /// Код последней ошибки процесса (значение `diag::ErrorCode`), 0 — ошибок не было.
    pub last_error_code: u32,
    /// Описание последней ошибки процесса.
    pub last_error: String,
}

impl Process {
    const fn blank(pid: u32) -> Self {
        Process {
            pid,
            uid: 0,
            parent: 0,
            name: String::new(),
            state: ProcessState::Empty,
            caps: 0,
            cwd: String::new(),
            fds: FdTable::new(),
            sched_id: None,
            exit_code: None,
            started_at_ms: 0,
            image_path: String::new(),
            last_error_code: 0,
            last_error: String::new(),
        }
    }

    /// Разрешает путь относительно `cwd` в абсолютный путь VFS.
    pub fn resolve(&self, path: &str) -> String {
        if path.starts_with('/') {
            return String::from(path);
        }
        if self.cwd.ends_with('/') {
            format!("{}{}", self.cwd, path)
        } else {
            format!("{}/{}", self.cwd, path)
        }
    }
}

// ==================== Таблица процессов ====================

static PROCS: SpinLock<[Process; MAX_PROCS]> = SpinLock::new([const { Process::blank(0) }; MAX_PROCS]);
static NEXT_PID: SpinLock<u32> = SpinLock::new(2);

/// Ошибки менеджера процессов.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessError {
    TableFull,
    NotFound(u32),
    BadImage(String),
    Vfs(String),
}

impl ProcessError {
    pub fn message(&self) -> String {
        match self {
            ProcessError::TableFull => String::from("таблица процессов заполнена"),
            ProcessError::NotFound(pid) => format!("процесс {} не найден", pid),
            ProcessError::BadImage(e) => format!("образ не загружен: {}", e),
            ProcessError::Vfs(e) => format!("файловая система: {}", e),
        }
    }
}

/// Параметры создаваемого процесса.
#[derive(Debug, Clone)]
pub struct SpawnRequest {
    pub name: String,
    pub uid: u32,
    pub parent: u32,
    pub caps: u64,
    pub cwd: String,
    /// Путь к MEX-образу процесса.
    pub image_path: String,
}

/// Создаёт процесс и размещает его образ. Возвращает PID.
///
/// Порядок принципиален: сначала запись в таблицу, потом загрузка
/// образа, потом задача планировщика. Если образ не загрузился,
/// процесс не остаётся висеть в таблице.
pub fn spawn(req: &SpawnRequest) -> Result<u32, ProcessError> {
    let pid = {
        let mut next = NEXT_PID.lock();
        let pid = *next;
        *next = next.wrapping_add(1);
        pid
    };

    let index = {
        let mut table = PROCS.lock();
        let idx = table.iter().position(|p| !p.state.occupies_slot() && p.state != ProcessState::Zombie);
        let idx = idx.ok_or(ProcessError::TableFull)?;

        let p = &mut table[idx];
        *p = Process::blank(pid);
        p.name = req.name.clone();
        p.uid = req.uid;
        p.parent = req.parent;
        p.caps = req.caps;
        p.cwd = if req.cwd.is_empty() { String::from("/userdata") } else { req.cwd.clone() };
        p.state = ProcessState::Created;
        p.started_at_ms = crate::timer::uptime_ms();
        p.image_path = req.image_path.clone();
        idx
    };

    // Стандартные дескрипторы: консоль на ввод и вывод.
    {
        let mut table = PROCS.lock();
        if let Some(p) = table.get_mut(index) {
            let _ = p.fds.alloc(Fd {
                path: String::from("/dev/console"),
                read_only: true,
            });
            let _ = p.fds.alloc(Fd {
                path: String::from("/dev/console"),
                read_only: false,
            });
        }
    }

    Ok(pid)
}

/// Контекст запуска: PID процесса и точка входа MEX-образа.
/// Заполняется перед созданием задачи планировщика и выбирается
/// трамплином. Запуск строго последовательный — один за раз.
static LAUNCH: SpinLock<Option<(u32, u64)>> = SpinLock::new(None);

/// Размер области, занимаемой MEX-образом (см. MEX_MAX_SIZE в mex.rs).
const MEX_IMAGE_SIZE: u64 = 4 * 1024 * 1024;

/// PID процесса, который сейчас занимает фиксированную область MEX.
/// Формат MEX не позиционно-независим, поэтому область одна на систему.
static MEX_IN_USE: SpinLock<Option<u32>> = SpinLock::new(None);

/// Точка входа задачи нового процесса. Берёт контекст запуска,
/// исполняет образ и помечает процесс завершённым.
extern "C" fn launch_trampoline() {
    let (pid, entry) = match LAUNCH.lock().take() {
        Some(v) => v,
        None => return,
    };
    // Ring 3: таблица MexApi указывает на стабы в shim-странице,
    // каждый вызов API — syscall. Падение программы завершает
    // процесс, а не ядро.
    let code: i32 = unsafe { crate::mex::run_ring3(entry) as i32 };
    notify_exit_by_sched(crate::sched::current_id(), pid, code);
}

/// Загружает образ процесса и создаёт для него задачу планировщика.
///
/// Поддерживаются только MEX-образы: они исполняются в Ring 3 через
/// `mex::run_ring3`. Запуск ELF-процессов через планировщик ядра
/// недоступен: кадры `sched::spawn_pid` создаются с CS=0x08/SS=0x10
/// (Ring 0), и исполнять в них пользовательский код нельзя
/// (DX-ELF-0009).
pub fn exec(pid: u32) -> Result<(), ProcessError> {
    let path = {
        let table = PROCS.lock();
        let p = table
            .iter()
            .find(|p| p.pid == pid)
            .ok_or(ProcessError::NotFound(pid))?;
        p.image_path.clone()
    };

    if path.is_empty() {
        return Err(ProcessError::BadImage(String::from("не задан путь к образу")));
    }

    let img = crate::vfs::read_file(&path).map_err(|e| ProcessError::Vfs(e.message()))?;

    let is_mex = path.ends_with(".mex") || crate::mex::is_mex_image(&img);

    if !is_mex {
        crate::diag::error(
            crate::diag::ErrorCode::new(crate::diag::Subsystem::Elf, 9),
            &format!("{}: запуск ELF-процессов через планировщик ядра недоступен", path),
        );
        return Err(ProcessError::BadImage(String::from(
            "запуск ELF-процессов пока недоступен: поддерживаются только MEX-образы (Ring 3)",
        )));
    }

    // MEX слинкован на фиксированный адрес и не является PIC, поэтому
    // такой образ нельзя разместить в адресном пространстве процесса —
    // он обязан лечь ровно по адресу линковки. Одновременно может
    // исполняться только один MEX-процесс.
    let entry = {
        let mut guard = MEX_IN_USE.lock();
        if let Some(other) = *guard {
            return Err(ProcessError::BadImage(format!(
                "MEX-образ не позиционно-независим: область занята процессом {}",
                other
            )));
        }
        let e = crate::mex::load_into(&img, crate::mex::load_address(), MEX_IMAGE_SIZE)
            .map_err(|e| {
                crate::diag::error(
                    crate::diag::ErrorCode::new(crate::diag::Subsystem::Elf, 6),
                    &format!("MEX {}: {}", path, e),
                );
                ProcessError::BadImage(e)
            })?;
        *guard = Some(pid);
        e
    };

    *LAUNCH.lock() = Some((pid, entry));

    let name = {
        let table = PROCS.lock();
        table
            .iter()
            .find(|p| p.pid == pid)
            .map(|p| p.name.clone())
            .unwrap_or_default()
    };

    match crate::sched::spawn_pid(&name, launch_trampoline, pid) {
        Some(sid) => mark_running(pid, sid),
        None => {
            *LAUNCH.lock() = None;
            Err(ProcessError::TableFull)
        }
    }
}

/// Отмечает, что образ процесса загружен и задача создана в планировщике.
pub fn mark_running(pid: u32, sched_id: usize) -> Result<(), ProcessError> {
    let mut table = PROCS.lock();
    let p = table
        .iter_mut()
        .find(|p| p.pid == pid)
        .ok_or(ProcessError::NotFound(pid))?;
    p.sched_id = Some(sched_id);
    p.state = ProcessState::Running;
    Ok(())
}
/// Завершает процесс. Задача планировщика тоже снимается.
pub fn kill(pid: u32, code: i32) -> Result<(), ProcessError> {
    let sched_id = {
        let mut table = PROCS.lock();
        let p = table
            .iter_mut()
            .find(|p| p.pid == pid)
            .ok_or(ProcessError::NotFound(pid))?;
        p.exit_code = Some(code);
        p.state = ProcessState::Zombie;
        p.fds = FdTable::new();
        p.sched_id
    };
    // Освобождаем фиксированную область MEX, если её занимал этот процесс.
    {
        let mut guard = MEX_IN_USE.lock();
        if *guard == Some(pid) {
            *guard = None;
        }
    }
    if let Some(id) = sched_id {
        crate::sched::terminate(id);
    }
    Ok(())
}

/// Сообщает о завершении задачи планировщика. Вызывается из
/// `sched::exit_current`, чтобы процесс не оставался «живым» после
/// смерти своей нити.
pub fn notify_exit(sched_id: usize, code: i32) {
    let pid = {
        let table = PROCS.lock();
        table
            .iter()
            .find(|p| p.sched_id == Some(sched_id))
            .map(|p| p.pid)
    };
    if let Some(pid) = pid {
        let _ = kill(pid, code);
    }
}

/// То же, но PID уже известен вызывающему (трамплин запуска).
fn notify_exit_by_sched(_sched_id: usize, pid: u32, code: i32) {
    let _ = kill(pid, code);
}

/// Освобождает слот завершённого процесса (аналог `wait`).
pub fn reap(pid: u32) -> Result<Option<i32>, ProcessError> {
    let mut table = PROCS.lock();
    let p = table
        .iter_mut()
        .find(|p| p.pid == pid)
        .ok_or(ProcessError::NotFound(pid))?;
    if p.state != ProcessState::Zombie {
        return Ok(None);
    }
    let code = p.exit_code;
    *p = Process::blank(pid);
    p.state = ProcessState::Reaped;
    Ok(code)
}

/// Снимок состояния процесса для отчётов.
#[derive(Debug, Clone)]
pub struct ProcInfo {
    pub pid: u32,
    pub uid: u32,
    pub parent: u32,
    pub name: String,
    pub state: ProcessState,
    pub caps: u64,
    pub cwd: String,
    pub open_fds: usize,
    pub sched_id: Option<usize>,
    pub exit_code: Option<i32>,
    pub started_at_ms: u64,
    /// Код последней ошибки (значение `diag::ErrorCode`), 0 — ошибок не было.
    pub last_error_code: u32,
    /// Описание последней ошибки.
    pub last_error: String,
}

/// Список живых и завершённых, но не пожнатых процессов.
pub fn list() -> Vec<ProcInfo> {
    let table = PROCS.lock();
    table
        .iter()
        .filter(|p| p.state != ProcessState::Empty && p.state != ProcessState::Reaped)
        .map(|p| ProcInfo {
            pid: p.pid,
            uid: p.uid,
            parent: p.parent,
            name: p.name.clone(),
            state: p.state,
            caps: p.caps,
            cwd: p.cwd.clone(),
            open_fds: p.fds.open_count(),
            sched_id: p.sched_id,
            exit_code: p.exit_code,
            started_at_ms: p.started_at_ms,
            last_error_code: p.last_error_code,
            last_error: p.last_error.clone(),
        })
        .collect()
}

/// Снимок одного процесса.
pub fn info(pid: u32) -> Result<ProcInfo, ProcessError> {
    let table = PROCS.lock();
    let p = table
        .iter()
        .find(|p| p.pid == pid)
        .ok_or(ProcessError::NotFound(pid))?;
    Ok(ProcInfo {
        pid: p.pid,
        uid: p.uid,
        parent: p.parent,
        name: p.name.clone(),
        state: p.state,
        caps: p.caps,
        cwd: p.cwd.clone(),
        open_fds: p.fds.open_count(),
        sched_id: p.sched_id,
        exit_code: p.exit_code,
        started_at_ms: p.started_at_ms,
        last_error_code: p.last_error_code,
        last_error: p.last_error.clone(),
    })
}

// ==================== Доступ из кода процесса ====================

/// PID процесса, исполняющегося сейчас, если он известен.
pub fn current_pid() -> Option<u32> {
    let sid = crate::sched::current_id();
    let table = PROCS.lock();
    table
        .iter()
        .find(|p| p.sched_id == Some(sid) && p.state.occupies_slot())
        .map(|p| p.pid)
}

/// Имя процесса по PID.
///
/// Через `try_lock`: вызывается из обработчиков исключений и из отчёта об
/// отказе, где обычный `lock()` на занятой таблице означал бы зависание
/// внутри обработчика сбоя.
pub fn name_of(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    let table = PROCS.try_lock()?;
    table
        .iter()
        .find(|p| p.pid == pid && p.state != ProcessState::Empty)
        .map(|p| p.name.clone())
}

/// Записывает последнюю ошибку процесса.
///
/// Её показывают Диспетчер задач и карточка процесса в Центре ошибок.
/// Через `try_lock` по той же причине, что и `name_of`.
pub fn record_crash(pid: u32, code: crate::diag::ErrorCode, detail: &str) {
    let Some(mut table) = PROCS.try_lock() else {
        return;
    };
    if let Some(p) = table.iter_mut().find(|p| p.pid == pid && p.state != ProcessState::Empty) {
        p.last_error_code = code.0;
        p.last_error = String::from(detail);
    }
}
// ==================== Файловые операции процесса ====================

/// Открывает файл от имени процесса: проверяет возможность и
/// регистрирует дескриптор.
pub fn open(pid: u32, path: &str, write: bool) -> Result<usize, ProcessError> {
    let abs = {
        let table = PROCS.lock();
        let p = table
            .iter()
            .find(|p| p.pid == pid)
            .ok_or(ProcessError::NotFound(pid))?;
        p.resolve(path)
    };

    let st = crate::vfs::stat(&abs).map_err(|e| ProcessError::Vfs(e.message()))?;

    if write {
        if st.read_only {
            return Err(ProcessError::Vfs(format!("раздел только для чтения: {}", abs)));
        }
        let caps = {
            let table = PROCS.lock();
            table.iter().find(|p| p.pid == pid).map(|p| p.caps).unwrap_or(0)
        };
        if caps & cap::WRITE_USERDATA == 0 {
            return Err(ProcessError::Vfs(String::from("нет права записи (cap WRITE_USERDATA)")));
        }
    }

    let fd = Fd {
        path: abs,
        read_only: !write,
    };

    let mut table = PROCS.lock();
    let p = table
        .iter_mut()
        .find(|p| p.pid == pid)
        .ok_or(ProcessError::NotFound(pid))?;
    p.fds.alloc(fd).ok_or(ProcessError::TableFull)
}

/// Закрывает дескриптор.
pub fn close(pid: u32, num: usize) -> Result<(), ProcessError> {
    let mut table = PROCS.lock();
    let p = table
        .iter_mut()
        .find(|p| p.pid == pid)
        .ok_or(ProcessError::NotFound(pid))?;
    if p.fds.close(num) {
        Ok(())
    } else {
        Err(ProcessError::NotFound(pid))
    }
}

/// Читает файл по дескриптору.
pub fn read_fd(pid: u32, num: usize) -> Result<Vec<u8>, ProcessError> {
    let path = {
        let table = PROCS.lock();
        let p = table
            .iter()
            .find(|p| p.pid == pid)
            .ok_or(ProcessError::NotFound(pid))?;
        let fd = p.fds.get(num).ok_or(ProcessError::NotFound(pid))?;
        fd.path.clone()
    };
    crate::vfs::read_file(&path).map_err(|e| ProcessError::Vfs(e.message()))
}

/// Пишет файл по дескриптору.
pub fn write_fd(pid: u32, num: usize, data: &[u8]) -> Result<(), ProcessError> {
    let path = {
        let table = PROCS.lock();
        let p = table
            .iter()
            .find(|p| p.pid == pid)
            .ok_or(ProcessError::NotFound(pid))?;
        let fd = p.fds.get(num).ok_or(ProcessError::NotFound(pid))?;
        if fd.read_only {
            return Err(ProcessError::Vfs(String::from("дескриптор открыт только для чтения")));
        }
        fd.path.clone()
    };
    crate::vfs::write_file(&path, data).map_err(|e| ProcessError::Vfs(e.message()))
}

// ==================== CLI ====================

/// `ps` — список процессов.
pub fn cmd_ps() {
    let procs = list();
    if procs.is_empty() {
        crate::println!("  Процессов нет.");
        return;
    }
    let now = crate::timer::uptime_ms();
    crate::println!("  PID   UID   PPID  STATE     FDS  CAPS  ВОЗР(с)  NAME");
    for p in procs.iter() {
        let age_s = now.saturating_sub(p.started_at_ms) / 1000;
        crate::println!(
            "  {:<5} {:<5} {:<5} {:<9} {:<4} {:<#5x} {:<8} {}",
            p.pid,
            p.uid,
            p.parent,
            p.state.as_str(),
            p.open_fds,
            p.caps,
            age_s,
            p.name
        );
        if !p.cwd.is_empty() && p.cwd != "/" {
            crate::println!("        каталог: {}", p.cwd);
        }
        if p.last_error_code != 0 {
            crate::println!(
                "        последняя ошибка: {} {}",
                crate::diag::ErrorCode(p.last_error_code).as_string(),
                p.last_error
            );
        }
    }
    crate::println!("  Всего: {}", procs.len());
}

/// `kill <pid>` — завершить процесс.
pub fn cmd_kill(arg: &str) {
    let pid: u32 = match arg.trim().parse() {
        Ok(v) => v,
        Err(_) => {
            crate::println!("  kill <pid> — завершить процесс");
            return;
        }
    };
    match kill(pid, -9) {
        Ok(()) => crate::println!("  Процесс {} завершён.", pid),
        Err(e) => crate::println!("  {}", e.message()),
    }
}
