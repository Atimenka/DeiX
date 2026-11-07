//! Диагностический экран загрузки: контрольный список подсистем.
//!
//! Каждая строка — результат реальной проверки, выполненной в момент
//! вывода, а не воспоминание о том, что когда-то печатал код инициализации.
//! Сбой проверки регистрируется в журнале с кодом соответствующей
//! подсистемы.

use super::code::{ErrorCode, Subsystem};
use super::NONE;
use crate::println;

/// Результат одной проверки.
enum Check {
    Ok,
    Warn(&'static str, ErrorCode),
    Fail(&'static str, ErrorCode),
}

/// Печатает строку контрольного списка и регистрирует событие.
fn report(name: &str, check: Check) {
    match check {
        Check::Ok => {
            println!("  [ OK ] {}", name);
            super::info(NONE, &alloc::format!("boot: {} — OK", name));
        }
        Check::Warn(reason, code) => {
            println!("  [WARN] {} — {}", name, reason);
            super::warn(code, &alloc::format!("boot: {} — {}", name, reason));
        }
        Check::Fail(reason, code) => {
            println!("  [FAIL] {} — {}", name, reason);
            println!("         {}", code.as_string());
            println!("         {}", super::code::detail_of(code));
            super::error(code, &alloc::format!("boot: {} — {}", name, reason));
        }
    }
}

/// Печатает контрольный список загрузки.
///
/// Вызывается в конце инициализации ядра, когда все подсистемы уже
/// попытались подняться. `net_ok`, `audio_ok`, `gpu_ok` — результаты
/// инициализации драйверов, известные только кодом загрузки.
pub fn screen(net_ok: bool, audio_ok: bool, gpu_ok: bool) {
    println!("");
    println!("  DEIX BOOT");
    println!("");

    report("CPU", Check::Ok); // раз этот код исполняется — процессор жив
    report(
        "MEMORY",
        if crate::allocator::total_heap_bytes() > 0 {
            Check::Ok
        } else {
            Check::Fail("куча не инициализирована", ErrorCode::new(Subsystem::Memory, 6))
        },
    );
    report(
        "TIMER",
        if crate::timer::uptime_ms() > 0 {
            Check::Ok
        } else {
            Check::Fail("PIT не тикает", ErrorCode::new(Subsystem::Kernel, 10))
        },
    );
    report(
        "STORAGE",
        if crate::ata::is_present() {
            Check::Ok
        } else {
            Check::Fail("ATA-устройство не обнаружено", ErrorCode::new(Subsystem::Disk, 1))
        },
    );

    // /system: раздел должен существовать и открываться как EROFS.
    report(
        "/system",
        match crate::vfs::stat("/system") {
            Ok(_) => Check::Ok,
            Err(_) => Check::Fail(
                "образ EROFS не читается",
                ErrorCode::new(Subsystem::Erofs, 1),
            ),
        },
    );
    report(
        "/userdata",
        if crate::ext2::is_formatted() {
            Check::Ok
        } else {
            Check::Warn(
                "EXT2 не отформатирован (первый запуск)",
                ErrorCode::new(Subsystem::Ext2, 1),
            )
        },
    );

    // KMOD: сбой — если хоть один модуль загрузился с ошибкой.
    let modules = crate::module::get_loaded_modules();
    let kmod_failed = modules
        .iter()
        .any(|m| m.status != crate::module::ModuleStatus::Initialized);
    report(
        "KMOD",
        if kmod_failed {
            Check::Warn("часть модулей не работает", ErrorCode::new(Subsystem::Kmod, 4))
        } else {
            Check::Ok
        },
    );

    report(
        "DINIT",
        if crate::dinit::DINIT.lock().is_some() {
            Check::Ok
        } else {
            Check::Fail("супервизор не создан", ErrorCode::new(Subsystem::Dinit, 10))
        },
    );
    report(
        "NETWORK",
        if net_ok {
            Check::Ok
        } else {
            Check::Warn("сетевой адаптер не найден", ErrorCode::new(Subsystem::Net, 1))
        },
    );
    report(
        "AUDIO",
        if audio_ok {
            Check::Ok
        } else {
            Check::Warn("HDA недоступен, только PC-speaker", ErrorCode::new(Subsystem::Audio, 1))
        },
    );
    report(
        "GRAPHICS",
        if gpu_ok {
            Check::Ok
        } else {
            Check::Warn("GPU не обнаружен, текстовый режим", ErrorCode::new(Subsystem::Gfx, 1))
        },
    );

    println!("");
    println!("  BOOT COMPLETE");
    println!("");
}
