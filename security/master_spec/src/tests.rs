// ❗ЗАВИСИМОТИ: инит скрипт pid 1 который будет ограничивать пользовательский
// Модульная сборка MASTER SPEC DeiX OS: каждый модуль — отдельный файл исходника.
// PID 1 изолирует Ring 3: /kernel /init_boot /boot /vendor_boot /super /system
// /recovery (erofs/ro), /userdata (ext4/rw). Arch PKG -> .pkg.erofs внутри /userdata.
// TWRP/OrangeFox + Fastbootd/EDL. Zero External Dependencies, совместимо с no_std.

    use crate::arch_pkg_bridge::{ArchPkgMetadata, mount_arch_package, ErofsPackageContainer};
    use crate::init_parser::{BootStage, Command, InitParser};
    use crate::vault::SECURITY_VIOLATION_PANIC_TEXT;
    use crate::kernel_loader::{ErofsImage, KernelLoadError, load_and_boot_kernel, EROFS_SUPER_MAGIC};
    use crate::recovery_flash_engine::{
        EdlEmergencyFlasher, FastbootdProtocol, RecoveryEnvironment, fnv1a64,
    };
    use crate::security_monitor::{
        HeuristicAnalysisEngine, SecurityEvent, ThreatSeverity, ThreatVector, SIGKILL,
    };
    use std::collections::HashMap;
    use std::panic::{catch_unwind, AssertUnwindSafe};

    /// Вспомогательная функция: разбор скрипта с перехватом паники Vault.
    fn parse_catching_panic(
        source: &str,
    ) -> Result<HashMap<BootStage, Vec<Command>>, String> {
        let mut parser: InitParser = InitParser::new();
        let outcome: Result<HashMap<BootStage, Vec<Command>>, Box<dyn std::any::Any + Send>> =
            catch_unwind(AssertUnwindSafe(|| parser.parse(source)));
        match outcome {
            Ok(registry) => Ok(registry),
            Err(payload) => {
                let message: String = match payload.downcast_ref::<&str>() {
                    Some(msg) => msg.to_string(),
                    None => match payload.downcast_ref::<String>() {
                        Some(msg) => msg.clone(),
                        None => "неизвестный payload паники".to_string(),
                    },
                };
                Err(message)
            }
        }
    }

    /// VAULT: rw системного раздела вне прошивочного контекста -> паника на
    /// каждом из семи критических путей.
    #[test]
    fn vault_panics_on_rw_system_partition_outside_flash_context() {
        for path in ["/kernel", "/init_boot", "/boot", "/vendor_boot", "/super", "/system", "/recovery"] {
            let script: String = format!("on boot\nmount erofs /dev/sda1 {} rw\n", path);
            let result: Result<HashMap<BootStage, Vec<Command>>, String> = parse_catching_panic(&script);
            match result {
                Ok(_) => panic!("VAULT ПРОРВАН: {} принят с rw в стадии boot", path),
                Err(message) => assert_eq!(message, SECURITY_VIOLATION_PANIC_TEXT),
            }
        }
    }

    /// VAULT: системный раздел через не-erofs драйвер -> паника.
    #[test]
    fn vault_panics_on_non_erofs_system_mount() {
        let script: &str = "on boot\nmount ext4 /dev/sda1 /kernel ro\n";
        let result: Result<HashMap<BootStage, Vec<Command>>, String> = parse_catching_panic(script);
        match result {
            Ok(_) => panic!("VAULT ПРОРВАН: /kernel смонтирован ext4"),
            Err(message) => assert_eq!(message, SECURITY_VIOLATION_PANIC_TEXT),
        }
    }

    /// VAULT: rw системного раздела РАЗРЕШЁН в Fastbootd, Edl, Recovery.
    #[test]
    fn vault_allows_rw_in_flash_contexts() {
        for stage in ["fastbootd", "edl", "recovery"] {
            let script: String = format!("on {}\nmount erofs /dev/sda1 /kernel rw\n", stage);
            let result: Result<HashMap<BootStage, Vec<Command>>, String> = parse_catching_panic(&script);
            match result {
                Ok(_) => {}
                Err(message) => panic!("санкционированный контекст {} дал панику: {}", stage, message),
            }
        }
    }

    /// /userdata: rw на стадии init_boot — предупреждение, НЕ паника.
    #[test]
    fn userdata_rw_outside_allowed_stage_is_warning_not_panic() {
        let script: &str = "on init_boot\nmount ext4 /dev/sda1 /userdata rw\n";
        let mut parser: InitParser = InitParser::new();
        let _registry: HashMap<BootStage, Vec<Command>> = parser.parse(script);
        assert_eq!(parser.warnings.len(), 1);
        assert_eq!(parser.registry.get(&BootStage::InitBoot).unwrap().len(), 0);
    }

    /// /userdata: не-ext4 драйвер — предупреждение, НЕ паника.
    #[test]
    fn userdata_non_ext4_is_warning_not_panic() {
        let script: &str = "on boot\nmount erofs /dev/sda1 /userdata ro\n";
        let mut parser: InitParser = InitParser::new();
        let _registry: HashMap<BootStage, Vec<Command>> = parser.parse(script);
        assert_eq!(parser.warnings.len(), 1);
    }

    /// Служба с недопустимым кольцом (9) — синтаксическая ошибка, разбор идёт.
    #[test]
    fn service_invalid_ring_rejected() {
        let script: &str = "on boot\nservice evil /bin/evil 9\n";
        let mut parser: InitParser = InitParser::new();
        let _registry: HashMap<BootStage, Vec<Command>> = parser.parse(script);
        assert_eq!(parser.warnings.len(), 1);
        assert_eq!(parser.registry.get(&BootStage::Boot).unwrap().len(), 0);
    }

    /// СЭНДВИЧ ЯДРА: фикстура -> load_and_boot_kernel OK, магия 0xE0F5E0F5.
    #[test]
    fn kernel_sandwich_roundtrip() {
        let partition = crate::fixtures::build_kernel_partition_fixture();
        match load_and_boot_kernel(&partition) {
            Ok(()) => {}
            Err(err) => panic!("load_and_boot_kernel: {}", err.message()),
        }
        let archive = crate::kernel_loader::TarGzArchive::parse(partition.tar_gz).expect("tar.gz");
        let image = archive.unpack().expect("kernel.img");
        assert_eq!(image.superblock.magic, EROFS_SUPER_MAGIC);
    }

    /// Повреждённый EROFS-образ отклоняется ошибкой несовпадения магии.
    #[test]
    fn erofs_bad_magic_rejected() {
        let bad: Vec<u8> = vec![0u8; 2048];
        match ErofsImage::from_bytes("bad.img", bad) {
            Err(KernelLoadError::ErofsMagicMismatch { .. }) => {}
            Err(other) => panic!("ожидалась ErofsMagicMismatch: {}", other.message()),
            Ok(_) => panic!("образ без магии принят"),
        }
    }

    /// PKGBUILD и .BUILDINFO разбираются корректно.
    #[test]
    fn arch_metadata_parsing() {
        let pkgbuild: &str = concat!(
            "pkgname=deix-hello\n",
            "pkgver=1.2.3-1\n",
            "arch=('x86_64' 'aarch64')\n",
            "depends=('glibc' 'zlib')\n",
        );
        let meta: ArchPkgMetadata = ArchPkgMetadata::parse_pkgbuild(pkgbuild).expect("pkgbuild");
        assert_eq!(meta.pkgname, "deix-hello");
        assert_eq!(meta.pkgver, "1.2.3-1");
        assert_eq!(meta.arch, "x86_64");
        assert_eq!(meta.depends, vec!["glibc", "zlib"]);

        let buildinfo: &str = "pkgname = deix-hello\npkgver = 1.2.3-1\narch = x86_64\ndepend = glibc\n";
        let bi: ArchPkgMetadata = ArchPkgMetadata::parse_buildinfo(buildinfo).expect("buildinfo");
        assert_eq!(bi.pkgname, "deix-hello");
        assert_eq!(bi.depends, vec!["glibc"]);
    }

    /// Монтирование пакета вне /userdata отклоняется (защита системных разделов).
    #[test]
    fn arch_mount_denied_outside_userdata() {
        let declared: ArchPkgMetadata = ArchPkgMetadata::parse_pkgbuild("pkgname=deix-hello\npkgver=1.2.3-1\narch=('x86_64')\n")
            .expect("pkgbuild");
        let container: ErofsPackageContainer =
            ErofsPackageContainer::from_pkg_tar_zst(
                crate::fixtures::build_pkg_tar_zst("deix-hello", "1.2.3-1"),
                &declared,
                "/system",
            )
            .expect("container");
        match mount_arch_package(&container, "/userdata") {
            Ok(_) => panic!("пакет смонтирован в системное пространство!"),
            Err(_) => {}
        }
    }

    /// Монтирование пакета внутри /userdata проходит.
    #[test]
    fn arch_mount_ok_inside_userdata() {
        let declared: ArchPkgMetadata = ArchPkgMetadata::parse_pkgbuild("pkgname=deix-hello\npkgver=1.2.3-1\narch=('x86_64')\n")
            .expect("pkgbuild");
        let container: ErofsPackageContainer = ErofsPackageContainer::from_pkg_tar_zst(
            crate::fixtures::build_pkg_tar_zst("deix-hello", "1.2.3-1"),
            &declared,
            "/userdata",
        )
        .expect("container");
        let mount_point: String = mount_arch_package(&container, "/userdata").expect("mount");
        assert!(mount_point.starts_with("/userdata/pkg/deix-hello"));
        assert_eq!(container.image.superblock.magic, EROFS_SUPER_MAGIC);
    }

    /// TWRP: nandroid-бэкап + restore + дайджест FNV-1a детерминированы.
    #[test]
    fn recovery_nandroid_roundtrip() {
        let mut images: HashMap<String, Vec<u8>> = HashMap::new();
        images.insert("/boot".to_string(), vec![1u8, 2, 3, 4]);
        images.insert("/userdata".to_string(), vec![9u8; 64]);
        let mut recovery: RecoveryEnvironment = RecoveryEnvironment::new("twrp", images);
        let backup = recovery.nandroid_backup(&["/boot"]).expect("backup");
        assert_eq!(backup.len(), 1);
        assert_eq!(backup[0].digest, fnv1a64(&[1u8, 2, 3, 4]));
        assert_eq!(recovery.restore("/boot").expect("restore"), 4);
        let wiped: usize = recovery.factory_reset().expect("factory reset");
        assert!(wiped > 0);
    }

    /// Fastbootd: системный раздел принимает только EROFS-образ; повреждённый
    /// образ откатывается без изменений.
    #[test]
    fn fastbootd_atomic_flash_with_rollback() {
        let mut fastbootd: FastbootdProtocol = FastbootdProtocol::new();
        let good: Vec<u8> = crate::fixtures::build_erofs_image("system");
        fastbootd.flash_partition("/system", good).expect("flash ok");
        let garbage: Vec<u8> = vec![0xEE; 1024];
        match fastbootd.flash_partition("/system", garbage) {
            Ok(()) => panic!("повреждённый образ принят"),
            Err(_) => {}
        }
        assert!(fastbootd.committed.contains_key("/system"));
        // Размер закоммиченного образа не изменился (откат не тронул коммит).
        assert_eq!(fastbootd.committed.get("/system").unwrap().len(), 4096);
    }

    /// EDL: unbrick записывает образ и readback-верификация проходит.
    #[test]
    fn edl_unbrick_writes_and_verifies() {
        let mut raw: HashMap<String, Vec<u8>> = HashMap::new();
        raw.insert("mmcblk0".to_string(), vec![0u8; 8192]);
        let mut edl: EdlEmergencyFlasher = EdlEmergencyFlasher::new(raw);
        let bootloader: Vec<u8> = crate::fixtures::build_erofs_image("bootloader");
        edl.unbrick("mmcblk0", &bootloader).expect("unbrick");
        let readback: Vec<u8> = edl.read_raw("mmcblk0", 0, bootloader.len()).expect("read");
        assert_eq!(readback, bootloader);
    }

    /// Ransomware-паттерн -> критическая угроза + SIGKILL по PID.
    #[test]
    fn engine_flags_ransomware_and_emits_kill() {
        let mut engine: HeuristicAnalysisEngine =
            HeuristicAnalysisEngine::new("deix_heuristic_core_v1");
        let event: SecurityEvent =
            SecurityEvent::new(666, "sys_write", "/data/user/private/", 450, 0.92);
        let verdict = engine.compute_verdict(&event);
        assert!(verdict.risk_score > engine.risk_threshold);
        assert!(verdict.vectors.contains(&ThreatVector::RansomwareEncryption));
        assert_eq!(verdict.severity, ThreatSeverity::Critical);
        assert!(engine.analyze_event(&event));
        assert!(engine.process_event(&event));
        assert_eq!(engine.kill_signals.len(), 1);
        match engine.kill_signals.last() {
            Some(signal) => {
                assert_eq!(signal.pid, 666);
                assert_eq!(signal.signal, SIGKILL);
            }
            None => panic!("kill-сигнал не сформирован"),
        }
    }

    /// Легитимная запись редактора -> SAFE, kill-сигнал не требуется.
    #[test]
    fn engine_accepts_legitimate_writes() {
        let mut engine: HeuristicAnalysisEngine =
            HeuristicAnalysisEngine::new("deix_heuristic_core_v1");
        let event: SecurityEvent =
            SecurityEvent::new(1001, "sys_write", "/data/editor/notes.log", 10, 0.22);
        let verdict = engine.compute_verdict(&event);
        assert!(verdict.risk_score < engine.risk_threshold);
        assert!(!engine.analyze_event(&event));
        assert!(!engine.process_event(&event));
        assert!(engine.kill_signals.is_empty());
    }

    /// Code Injection (sys_ptrace на системный путь) -> риск = 1.0.
    #[test]
    fn engine_maxes_risk_on_code_injection() {
        let engine: HeuristicAnalysisEngine =
            HeuristicAnalysisEngine::new("deix_heuristic_core_v1");
        let event: SecurityEvent = SecurityEvent::new(31337, "sys_ptrace", "/kernel/core", 5, 0.31);
        let verdict = engine.compute_verdict(&event);
        assert_eq!(verdict.risk_score, 1.0);
        assert!(verdict.vectors.contains(&ThreatVector::CodeInjection));
        assert!(engine.analyze_event(&event));
    }
