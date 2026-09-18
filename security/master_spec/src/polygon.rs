// ❗ЗАВИСИМОТИ: инит скрипт pid 1 который будет ограничивать пользовательский
// Модульная сборка MASTER SPEC DeiX OS: каждый модуль — отдельный файл исходника.
// PID 1 изолирует Ring 3: /kernel /init_boot /boot /vendor_boot /super /system
// /recovery (erofs/ro), /userdata (ext4/rw). Arch PKG -> .pkg.erofs внутри /userdata.
// TWRP/OrangeFox + Fastbootd/EDL. Zero External Dependencies, совместимо с no_std.

    use crate::arch_pkg_bridge::{
        ArchPkgMetadata, ErofsPackageContainer, mount_arch_package,
    };
    use crate::fixtures::{
        build_erofs_image, build_kernel_partition_fixture, build_pkg_tar_zst,
    };
    use crate::init_parser::{BootStage, Command, InitParser};
    use crate::kernel_loader::{
        ErofsImage, KernelPartition, TarGzArchive, load_and_boot_kernel,
    };
    use crate::partition_map::{PARTITION_MAP, validate_partition_map};
    use crate::recovery_flash_engine::{
        EdlEmergencyFlasher, FastbootdProtocol, NandroidBackupEntry, RecoveryEnvironment,
    };
    use crate::security_monitor::{
        HeuristicAnalysisEngine, KillSignal, SecurityEvent, ThreatSeverity, ThreatVerdict,
    };
    use crate::vault::SECURITY_VIOLATION_PANIC_TEXT;
    use std::collections::HashMap;
    use std::panic::{catch_unwind, AssertUnwindSafe};

    /// Полная имитация содержимого init.deix (мастер-карта разделов +
    /// инъекция синтаксических ошибок для проверки отказоустойчивости).
    pub const INIT_DEIX_SCRIPT: &str = concat!(
        "# Скрипт инициализации и развертывания DeiX OS — мастер-карта разделов\n",
        "on init_boot\n",
        "    mount erofs /dev/block/by-name/kernel /kernel ro\n",
        "    mount erofs /dev/block/by-name/init_boot /init_boot ro\n",
        "    service pid1_core /bin/pid1_core 0\n",
        "\n",
        "on vendor_boot\n",
        "    mount erofs /dev/block/by-name/vendor_boot /vendor_boot ro\n",
        "\n",
        "on boot\n",
        "    mount erofs /dev/block/by-name/super /system ro\n",
        "    mount erofs /dev/block/by-name/boot /boot ro\n",
        "    mount ext4 /dev/block/by-name/userdata /userdata rw\n",
        "    service security_monitor /bin/security_monitor 3\n",
        "    service network_manager /bin/net_daemon 3\n",
        "\n",
        "# Инъекция синтаксических ошибок для проверки отказоустойчивости\n",
        "    mount malformed_line_without_arguments\n",
        "    unknown_kernel_operation parameter1 parameter2\n",
        "    service broken_service\n",
        "    service bad_ring /bin/bad_ring 9\n",
        "\n",
        "on recovery\n",
        "    mount ext4 /dev/block/by-name/userdata /userdata rw\n",
        "    service twrp_shell /bin/twrp 3\n",
        "\n",
        "on fastbootd\n",
        "    mount erofs /dev/block/by-name/super /system rw\n",
        "    service fastbootd_daemon /bin/fastbootd 3\n",
        "\n",
        "on edl\n",
        "    mount erofs /dev/block/by-name/kernel /kernel rw\n",
        "    service edl_bridge /bin/edl_bridge 0\n",
    );

    /// Вредоносный буфер: несанкционированная модификация /kernel (rw) в
    /// штатной стадии boot — вне защищённых прошивочных протоколов. Должен
    /// мгновенно вызвать панику Vault (см. ШАГ 7).
    pub const MALICIOUS_SCRIPT: &str = "on boot\nmount ext4 /dev/sda1 /kernel rw";

    /// Точка входа полигона: последовательное исполнение семи шагов ТЗ.
    pub fn run() {
        step1_partition_map();
        step2_kernel_sandwich();
        step3_pid1_parser();
        step4_recovery_and_flash();
        step5_arch_package();
        step6_heuristic_monitor();
        step7_vault_stress();
        final_report();
    }

    /// ШАГ 1: инициализация и проверка карты разделов DeiX OS.
    fn step1_partition_map() {
        println!("============================================================");
        println!("  DeiX OS v0.2-dev — MASTER SPEC интеграционный полигон");
        println!("  [Ring 0] Vault + Fastbootd/EDL  |  [Ring 3] TWRP + Heuristic");
        println!("============================================================");
        println!("[kernel] long mode OK | paging OK | VGA driver OK | heap OK");
        println!("[kernel] PID 1: инициализация карты разделов...\n");
        println!("--- Карта разделов DeiX OS ---");
        for policy in PARTITION_MAP.iter() {
            println!(
                "    {:<12} {:>5} {:<2}  {:<7}  {}",
                policy.name, policy.fs, policy.default_mode, policy.ring, policy.description
            );
        }

        // Верификация делегируется модулю partition_map.
        match validate_partition_map() {
            Ok(()) => println!("    ✓ Политика разделов подтверждена: 7x erofs/ro + userdata ext4/rw\n"),
            Err(violations) => {
                for violation in violations.iter() {
                    println!("    [ERROR] {}", violation);
                }
                println!("    ✗ НАРУШЕНИЕ КАРТЫ РАЗДЕЛОВ\n");
            }
        }
    }

    /// ШАГ 2: сэндвич-распаковка ядра.
    fn step2_kernel_sandwich() {
        println!("=== [kernel_loader] Сэндвич ядра: /kernel -> kernel.tar.gz -> kernel.img ===");
        let partition: KernelPartition = build_kernel_partition_fixture();

        // Ручная цепочка для диагностики (магия суперблока).
        let archive: TarGzArchive = match TarGzArchive::parse(partition.tar_gz.clone()) {
            Ok(a) => a,
            Err(err) => {
                println!("    ✗ ОШИБКА gzip/tar: {}", err.message());
                return;
            }
        };
        println!(
            "    gzip: метод={} флаги={:#04x} длина_заголовка={}",
            archive.gzip.compression_method, archive.gzip.flags, archive.gzip.header_len
        );
        println!("    tar: членов в архиве = {}", archive.tar.members.len());
        for member in archive.tar.members.iter() {
            println!(
                "      -> {} ({} байт, каталог: {})",
                member.name, member.size, member.is_directory
            );
        }
        let kernel_image: ErofsImage = match archive.unpack() {
            Ok(img) => img,
            Err(err) => {
                println!("    ✗ ОШИБКА распаковки kernel.img: {}", err.message());
                return;
            }
        };
        println!(
            "    kernel.img: EROFS-суперблок магия={:#010x} блок={} блоков={} inos={}",
            kernel_image.superblock.magic,
            kernel_image.superblock.block_size,
            kernel_image.superblock.blocks,
            kernel_image.superblock.inos
        );
        match kernel_image.magic_ok() {
            true => println!("    ✓ Магия 0xE0F5E0F5 подтверждена — образ ядра валиден"),
            false => println!("    ✗ Магия не совпала — образ ядра повреждён"),
        }

        // Формальный прогон загрузчика.
        match load_and_boot_kernel(&partition) {
            Ok(()) => println!("    ✓ load_and_boot_kernel(): управление передано PID 1\n"),
            Err(err) => println!("    ✗ load_and_boot_kernel(): {}\n", err.message()),
        }
    }

    /// ШАГ 3: запуск парсера PID 1 для init.deix.
    fn step3_pid1_parser() {
        println!("=== [init_parser] PID 1: разбор init.deix ===");
        let mut parser: InitParser = InitParser::new();
        let registry: HashMap<BootStage, Vec<Command>> = parser.parse(INIT_DEIX_SCRIPT);

        for stage in parser.stage_order.iter() {
            match registry.get(stage) {
                Some(commands) => {
                    println!("[stage: {}] — {} команд(ы):", stage.as_token(), commands.len());
                    for command in commands.iter() {
                        println!("    -> {}", command.describe());
                    }
                }
                None => {
                    println!("[stage: {}] — объявлена, но не содержит команд", stage.as_token());
                }
            }
        }

        println!("--- Диагностика парсера (предупреждения) ---");
        match parser.warnings.is_empty() {
            true => println!("    Предупреждений нет."),
            false => {
                for warning in parser.warnings.iter() {
                    println!(
                        "    [WARN] строка {:>3} | {} | {} | строка: \"{}\"",
                        warning.line_no,
                        warning.kind.describe(),
                        warning.detail,
                        warning.raw_line
                    );
                }
            }
        }
        println!(
            "    Итог: всего {}, принято {}, отклонено {}, пропущено {} (инвариант: {} + {} + {} = {})",
            parser.report.total_lines,
            parser.report.accepted_lines,
            parser.report.rejected_lines,
            parser.report.skipped_lines,
            parser.report.accepted_lines,
            parser.report.rejected_lines,
            parser.report.skipped_lines,
            parser.report.total_lines
        );
        println!();
    }

    /// ШАГ 4: среда восстановления TWRP/OrangeFox и прошивальщики Fastbootd/EDL.
    fn step4_recovery_and_flash() {
        println!("=== [recovery_flash_engine] TWRP/OrangeFox + Fastbootd + EDL ===");

        // Модель образов разделов для среды восстановления.
        let mut images: HashMap<String, Vec<u8>> = HashMap::new();
        images.insert("/boot".to_string(), build_erofs_image("boot_image"));
        images.insert("/kernel".to_string(), build_erofs_image("kernel_image"));
        images.insert("/system".to_string(), build_erofs_image("system_image"));
        images.insert("/userdata".to_string(), vec![0xABu8; 1024]);

        // --- TWRP/OrangeFox: nandroid-бэкап и restore ---------------------
        let mut recovery: RecoveryEnvironment = RecoveryEnvironment::new("twrp_orangefox", images);
        let backup: Vec<NandroidBackupEntry> = match recovery.nandroid_backup(&["/boot", "/system"]) {
            Ok(entries) => entries,
            Err(err) => {
                println!("    ✗ nandroid_backup: {}", err.message());
                return;
            }
        };
        for entry in backup.iter() {
            println!(
                "    [twrp] бэкап {}: {} байт, дайджест FNV-1a = {:#018x}",
                entry.partition, entry.size_bytes, entry.digest
            );
        }
        match recovery.restore("/boot") {
            Ok(size) => println!("    [twrp] restore /boot: OK ({} байт)", size),
            Err(err) => println!("    [twrp] restore: {}", err.message()),
        }
        match recovery.factory_reset() {
            Ok(wiped) => println!("    [twrp] factory_reset: /userdata очищен ({} байт)\n", wiped),
            Err(err) => println!("    [twrp] factory_reset: {}", err.message()),
        }

        // --- Fastbootd: атомарная прошивка с откатом -----------------------
        let mut fastbootd: FastbootdProtocol = FastbootdProtocol::new();
        let system_img: Vec<u8> = build_erofs_image("fastbootd_system");
        match fastbootd.flash_partition("/system", system_img) {
            Ok(()) => println!("    [fastbootd] flash /system (EROFS): OK — атомарный коммит выполнен"),
            Err(err) => println!("    [fastbootd] flash /system: {}", err.message()),
        }
        // Негативный сценарий: повреждённый образ (не EROFS) -> откат.
        let garbage: Vec<u8> = vec![0xDE; 2048];
        match fastbootd.flash_partition("/system", garbage) {
            Ok(()) => println!("    [fastbootd] ПРОРЫВ: повреждённый образ принят (ошибка!)"),
            Err(err) => {
                println!("    [fastbootd] повреждённый образ отклонён: {}", err.message());
                println!("    [fastbootd] атомарный откат: стейджинг очищен, коммит не тронут");
            }
        }

        // --- EDL: аварийная запись в сырую память и unbrick -----------------
        let mut raw: HashMap<String, Vec<u8>> = HashMap::new();
        raw.insert("mmcblk0".to_string(), vec![0u8; 65536]);
        let mut edl: EdlEmergencyFlasher = EdlEmergencyFlasher::new(raw);
        let bootloader: Vec<u8> = build_erofs_image("bootloader");
        match edl.unbrick("mmcblk0", &bootloader) {
            Ok(()) => println!("    [edl] unbrick mmcblk0: загрузчик записан на offset 0, readback OK"),
            Err(err) => println!("    [edl] unbrick: {}", err.message()),
        }
        let patch: &[u8] = b"EDL_SECTOR_PATCH";
        match edl.raw_write_device("mmcblk0", 4096, patch) {
            Ok(()) => println!("    [edl] raw_write mmcblk0 @4096: OK (readback-верификация пройдена)\n"),
            Err(err) => println!("    [edl] raw_write: {}", err.message()),
        }
    }

    /// ШАГ 5: установка Arch-совместимого пакета в изолированное Ring 3.
    fn step5_arch_package() {
        println!("=== [arch_pkg_bridge] Arch PKG -> .pkg.erofs -> /userdata ===");

        // PKGBUILD-профиль пакета.
        let pkgbuild: &str = concat!(
            "pkgname=deix-hello\n",
            "pkgver=1.2.3-1\n",
            "arch=('x86_64')\n",
            "depends=('glibc' 'zlib')\n",
        );
        let declared: ArchPkgMetadata = match ArchPkgMetadata::parse_pkgbuild(pkgbuild) {
            Ok(meta) => meta,
            Err(err) => {
                println!("    ✗ PKGBUILD: {}", err.message());
                return;
            }
        };
        println!(
            "    PKGBUILD: pkgname={} pkgver={} arch={} depends={:?}",
            declared.pkgname, declared.pkgver, declared.arch, declared.depends
        );

        // Контейнер .pkg.tar.zst -> EROFS-модуль.
        let pkg_raw: Vec<u8> = build_pkg_tar_zst(&declared.pkgname, &declared.pkgver);
        let container: ErofsPackageContainer = match ErofsPackageContainer::from_pkg_tar_zst(
            pkg_raw,
            &declared,
            "/userdata",
        ) {
            Ok(c) => c,
            Err(err) => {
                println!("    ✗ конвертация .pkg.tar.zst: {}", err.message());
                return;
            }
        };
        println!(
            "    .pkg.erofs: магия суперблока {:#010x}, объём модуля {} байт",
            container.image.superblock.magic,
            container.image.raw.len()
        );

        // Монтирование в изолированную точку внутри /userdata.
        match mount_arch_package(&container, "/userdata") {
            Ok(mount_point) => {
                println!("    ✓ пакет смонтирован в Ring 3: {}", mount_point);
            }
            Err(err) => println!("    ✗ mount: {}", err.message()),
        }

        // Негативный сценарий: попытка монтирования в системное пространство.
        let hostile: ErofsPackageContainer = match ErofsPackageContainer::from_pkg_tar_zst(
            build_pkg_tar_zst("deix-hello", "1.2.3-1"),
            &declared,
            "/system",
        ) {
            Ok(c) => c,
            Err(err) => {
                println!("    ✗ конвертация (hostile): {}", err.message());
                return;
            }
        };
        match mount_arch_package(&hostile, "/userdata") {
            Ok(_) => println!("    ✗ ПРОРЫВ: пакет смонтирован вне /userdata!"),
            Err(err) => {
                println!("    ✓ системное пространство защищено: {}", err.message());
                println!();
            }
        }
    }

    /// ШАГ 6: эвристический монитор безопасности с ликвидацией через SIGKILL.
    fn step6_heuristic_monitor() {
        println!("=== [security_monitor] Ring 3: эвристический аудит (порог 0.85) ===");
        let mut engine: HeuristicAnalysisEngine =
            HeuristicAnalysisEngine::new("deix_heuristic_core_v1");

        // Событие А: легитимный процесс (текстовый редактор, /data, 10, 0.22).
        let event_a: SecurityEvent =
            SecurityEvent::new(1001, "sys_write", "/data/editor/notes.log", 10, 0.22);
        let verdict_a: ThreatVerdict = engine.compute_verdict(&event_a);
        let flagged_a: bool = engine.process_event(&event_a);
        println!(
            "    Событие А: PID {} sys_write freq={} entropy={:.2} -> risk {:.3}",
            event_a.pid, event_a.syscall_frequency, event_a.entropy_score, verdict_a.risk_score
        );
        match verdict_a.severity {
            ThreatSeverity::Safe => println!("    ✓ ВЕРДИКТ: SAFE — редактор признан безопасным"),
            ThreatSeverity::Suspicious => println!("    ~ ВЕРДИКТ: SUSPICIOUS"),
            ThreatSeverity::Critical => println!("    ⚠ ВЕРДИКТ: CRITICAL"),
        }
        match flagged_a {
            true => println!("    ⚠ Ложное срабатывание на событии А"),
            false => println!("    ✓ Kill-сигнал не требуется"),
        }

        // Событие Б: атака шифровальщика (sys_write, 450, 0.92).
        let event_b: SecurityEvent =
            SecurityEvent::new(666, "sys_write", "/data/user/private/", 450, 0.92);
        let verdict_b: ThreatVerdict = engine.compute_verdict(&event_b);
        let flagged_b: bool = engine.process_event(&event_b);
        println!(
            "    Событие Б: PID {} sys_write freq={} entropy={:.2} -> risk {:.3}",
            event_b.pid, event_b.syscall_frequency, event_b.entropy_score, verdict_b.risk_score
        );
        match flagged_b {
            true => {
                println!("    ⚠ КРИТИЧЕСКАЯ УГРОЗА: вектор {:?} превысил порог", verdict_b.vectors);
                match engine.kill_signals.last() {
                    Some(signal) => {
                        let sig: &KillSignal = signal;
                        println!("    [scheduler] >>> {}", sig.describe());
                        println!("    [scheduler] >>> Процесс {} уничтожен до завершения дисковой транзакции\n", sig.pid);
                    }
                    None => println!("    [scheduler] ОШИБКА: kill-сигнал не сформирован"),
                }
            }
            false => println!("    ✓ Событие Б не превысило порог\n"),
        }
    }

    /// ШАГ 7: стресс-тест безопасности Vault — несанкционированная
    /// модификация системного раздела вне защищённых протоколов -> panic!.
    fn step7_vault_stress() {
        println!("=== [vault] Ring 0: стресс-тест безопасности ===");
        println!("    Вредоносный скрипт: \"{}\"", MALICIOUS_SCRIPT);

        let mut attacker_parser: InitParser = InitParser::new();
        let outcome: Result<HashMap<BootStage, Vec<Command>>, Box<dyn std::any::Any + Send>> =
            catch_unwind(AssertUnwindSafe(|| attacker_parser.parse(MALICIOUS_SCRIPT)));

        match outcome {
            Ok(registry) => {
                println!(
                    "    ✗ VAULT ПРОРВАН: парсер вернул реестр {:?} — СБОРКА НЕКОРРЕКТНА.",
                    registry.keys()
                );
            }
            Err(payload) => {
                let panic_message: String = match payload.downcast_ref::<&str>() {
                    Some(msg) => msg.to_string(),
                    None => match payload.downcast_ref::<String>() {
                        Some(msg) => msg.clone(),
                        None => "неизвестный payload паники".to_string(),
                    },
                };
                match panic_message == SECURITY_VIOLATION_PANIC_TEXT {
                    true => {
                        println!("    ✓ VAULT GUARD АКТИВЕН: паника с эталонным сообщением подтверждена.");
                        println!("      Сообщение: \"{}\"", panic_message);
                    }
                    false => {
                        println!("    ⚠ Неожиданное сообщение паники: \"{}\"", panic_message);
                    }
                }
                println!("    [halt] Ядро остановлено ДО монтирования rw-флага на /kernel");
                let boot_commands: usize = match attacker_parser.registry.get(&BootStage::Boot) {
                    Some(commands) => commands.len(),
                    None => 0,
                };
                println!(
                    "    [halt] Команда mount НЕ зафиксирована в реестре (команд в стадии boot: {})",
                    boot_commands
                );
                println!("    [halt] CPU остановлен (hlt).\n");
            }
        }

        // Контрольный контраст: та же запись /kernel rw, но в санкционированном
        // прошивочном контексте fastbootd — ДОЛЖНА пройти без паники.
        println!("    Контраст: запись /kernel rw в контексте fastbootd (санкционировано):");
        let mut authorized_parser: InitParser = InitParser::new();
        let authorized_script: &str = "on fastbootd\nmount erofs /dev/sda1 /kernel rw";
        let outcome2: Result<HashMap<BootStage, Vec<Command>>, Box<dyn std::any::Any + Send>> =
            catch_unwind(AssertUnwindSafe(|| authorized_parser.parse(authorized_script)));
        match outcome2 {
            Ok(registry) => {
                match registry.get(&BootStage::Fastbootd) {
                    Some(commands) => {
                        println!("    ✓ Санкционированный контекст: fastbootd принял команду ({} шт.)", commands.len());
                    }
                    None => println!("    ⚠ Стадия fastbootd пуста"),
                }
            }
            Err(_) => println!("    ✗ Ложная паника в санкционированном контексте!"),
        }
        println!();
    }

    /// Сводный отчёт полигона.
    fn final_report() {
        println!("============================================================");
        println!("  [deix] Сквозной сценарий MASTER SPEC завершён.");
        println!("  Подсистемы: init_parser | kernel_loader | arch_pkg_bridge |");
        println!("               recovery_flash_engine | security_monitor");
        println!("  Vault: системные разделы erofs/ro, rw только в Fastbootd/EDL/Recovery.");
        println!("============================================================");
    }
