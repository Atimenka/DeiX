// ЯДЕРНЫЙ МОДУЛЬ DeiX OS (src/lib.rs, Ring 0).
// install — УСТАНОВКА DeiX на физический диск.
//
// Ядро установки — InstallJob: пошаговый конечный автомат, который
// выполняет установку небольшими порциями. Это один и тот же код для
// двух интерфейсов:
//   * CLI-команда `install` (мастер с вопросами, как раньше);
//   * графический установщик (окно "Install DeiX" на рабочем столе) —
//     он вызывает job.step() по одному шагу на кадр и рисует честный
//     прогресс по реально выполненной работе.
//
// Этапы установки:
//   1. ЭТАП 2 — перенос данных с загрузочного диска (если включён):
//      /userdata обходится рекурсивно и переносится ЦЕЛИКОМ (аккаунты,
//      обои, настройки, загрузки, медиапак). При установке на Master
//      источник и цель — один диск: файлы читаются в память ДО
//      перезаписи MBR (она сбрасывает маркер шифрования); лимиты —
//      1 МиБ на файл и 6 МиБ суммарно, лишнее честно пропускается
//      с заметкой. При установке на Slave источник переживает
//      форматирование, и файлы копируются потоково (чтение→запись)
//      по одному за шаг, предел 8 МиБ на файл;
//   2. MBR с таблицей разделов (P1 /system EROFS, P2 /userdata ext2 —
//      размер /userdata ВЫБИРАЕТСЯ при установке);
//   3. служебная область до /system обнуляется, затем копируется карта
//      загрузки DEIXKIMG (LBA 2047) с загрузочного носителя;
//   4. образ /system (EROFS) посекторно: в нём лежат stage2, ramboot и
//      kernel.bin (/system/boot, /system/kernel), на которые указывает карта;
//   5. форматирование /userdata в ext2 (многогрупповый том под
//      выбранный размер — см. ext2::plan_format_raw);
//   6. USERS.DB: новый аккаунт + перенесённые (без дублей имён), затем
//      перенесённые файлы записываются на новый том;
//   7. ЭТАП 3 — подготовка первой загрузки: на новый /userdata пишется
//      маркер config/first_boot; при первом запуске рабочего стола он
//      даёт приветствие с напоминанием о шифровании и удаляется;
//   8. проверка (MBR, магии ФС, SHA-256 ядра по карте загрузки);
//   9. шифрование (XTS-AES-256) — только если цель = Master;
//  10. ЭТАП 3 — финальная проверка: USERS.DB перечитывается с нового
//      диска (если том не зашифрован), а мастер предлагает немедленную
//      перезагрузку (CLI спрашивает, в GUI — кнопка Reboot now).

use crate::ata::{self, Drive};
use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

// ==================== КОНСТАНТЫ УСТАНОВКИ ====================

/// Максимум секторов, заполняемых нулями в служебной области (MBR gap,
/// карта загрузки, область дампа) до начала /system (LBA 4096). Карта
/// загрузки (LBA 2047) записывается заново после заполнения.
const KERNEL_AREA_SECTORS: u32 = 3500;

/// Сектор карты загрузки DEIXKIMG (см. src/partition_map.rs, src/bootmap.rs).
const KERNEL_IMAGE_INFO_LBA: u32 = 2047;

/// Карта разделов: P1 /system фиксированный (совпадает с
/// tools/make_deix_fs.py), P2 /userdata начинается сразу после него,
/// а его РАЗМЕР выбирается при установке.
const PART_SYSTEM: (u32, u32) = (4096, 32768);
/// Начало /userdata (ext2::FS_START_LBA).
pub const USERDATA_START_LBA: u32 = 36864;
/// Минимальный размер /userdata: 16 МиБ (штатный образ).
pub const USERDATA_MIN_SECTORS: u32 = 32768;
/// Варианты размера /userdata для установщика, МиБ.
pub const USERDATA_PRESETS_MIB: [u32; 6] = [16, 32, 64, 128, 256, 512];

/// Максимальный размер /userdata на диске данной ёмкости.
pub fn max_userdata_sectors(disk_sectors: u32) -> u32 {
    disk_sectors.saturating_sub(USERDATA_START_LBA)
}

/// Секторов, копируемых/пишущихся за один шаг InstallJob (64 КиБ) —
/// достаточно мелко, чтобы графический установщик оставался отзывчивым.
const COPY_CHUNK_SECTORS: u32 = 128;

/// Предел размера одного переносимого файла (обои и настройки заметно
/// меньше; всё крупнее честно пропускается с заметкой).
const MIGRATE_FILE_MAX_BYTES: usize = 1024 * 1024;
/// Общий предел данных, удерживаемых в памяти при переносе (куча
/// 16 МиБ — оставляем запас системе).
const MIGRATE_TOTAL_MAX_BYTES: usize = 6 * 1024 * 1024;
/// Предел файла при потоковом копировании (установка на Slave: источник
/// цел, файл читается и пишется поштучно). Совпадает с пределом чтения
/// файла в ext2.
const MIGRATE_STREAM_FILE_MAX_BYTES: usize = 8 * 1024 * 1024;
/// Этап 3: маркер первой загрузки на новом /userdata.
const FIRST_BOOT_FILE: &str = "config/first_boot";

/// MBR-загрузчик, включённый в ядро (build.sh собирает boot_sector.bin ДО
/// компиляции ядра). stage2 и ramboot в ядро не вшиваются: они приходят с
/// копией /system.
const BOOT_SECTOR: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/build/boot_sector.bin"));

// Конец образа ядра (linker_kernel.ld: __image_end в конце .data, выровнен до 512).
extern "C" {
    static __image_end: u8;
}

/// База ядра в памяти (linker_kernel.ld): 0x100000 (1 МиБ).
const KERNEL_BASE: usize = 0x100000;

/// Размер образа ядра в памяти: от базы 0x100000 до __image_end (для шкалы
/// прогресса).
fn kernel_size() -> usize {
    unsafe { (&__image_end as *const u8 as usize) - KERNEL_BASE }
}
fn kernel_sectors() -> u32 {
    ((kernel_size() + 511) / 512) as u32
}

// ==================== обнаружение дисков ====================

/// Кандидат в целевые диски установки.
pub struct TargetDisk {
    pub drive: Drive,
    pub label: &'static str,
    /// Ёмкость в секторах (IDENTIFY DEVICE); None = не сообщил.
    pub sectors: Option<u32>,
}

/// Список доступных ATA-дисков. Для Slave заодно вычитываются данные
/// IDENTIFY, оставленные is_slave_present в контроллере — без этого
/// первая запись на Slave терялась.
pub fn detect_disks() -> Vec<TargetDisk> {
    let mut disks = Vec::new();
    if ata::is_present() {
        disks.push(TargetDisk {
            drive: Drive::Master,
            label: "ATA Master (boot disk)",
            sectors: ata::sector_count(Drive::Master),
        });
    }
    if ata::is_slave_present() {
        disks.push(TargetDisk {
            drive: Drive::Slave,
            label: "ATA Slave (second disk)",
            sectors: ata::sector_count(Drive::Slave),
        });
    }
    disks
}

// ==================== MBR / разделы ====================

fn chs(lba: u32) -> [u8; 3] {
    let c = lba / (63 * 255);
    let h = (lba / 63) % 255;
    let s = (lba % 63) + 1;
    let (c, h, s) = if c > 1023 { (1023u32, 254u8, 63u8) } else { (c, h as u8, s as u8) };
    [h, (s & 0x3F) | (((c >> 2) & 0xC0) as u8), (c & 0xFF) as u8]
}

/// Собирает MBR: загрузчик + таблица разделов DeiX
/// (P1 /system, P2 /userdata выбранного размера).
fn build_mbr(userdata_sectors: u32) -> [u8; 512] {
    let mut mbr = [0u8; 512];
    let n = BOOT_SECTOR.len().min(512);
    mbr[..n].copy_from_slice(&BOOT_SECTOR[..n]);

    let prim = [
        (0x80u8, 0x83u8, PART_SYSTEM.0, PART_SYSTEM.1),
        (0x00u8, 0x83u8, USERDATA_START_LBA, userdata_sectors),
    ];
    for (i, (boot, typ, start, secs)) in prim.iter().enumerate() {
        let off = 446 + i * 16;
        mbr[off] = *boot;
        mbr[off + 1..off + 4].copy_from_slice(&chs(*start));
        mbr[off + 4] = *typ;
        mbr[off + 5..off + 8].copy_from_slice(&chs(start + secs - 1));
        mbr[off + 8..off + 12].copy_from_slice(&start.to_le_bytes());
        mbr[off + 12..off + 16].copy_from_slice(&secs.to_le_bytes());
    }
    mbr[510] = 0x55;
    mbr[511] = 0xAA;
    mbr
}

// ==================== ПРОВЕРКА УСТАНОВКИ ====================

/// Проверяет установленную систему на целевом диске:
/// MBR (подпись + таблица разделов), магия EROFS /system,
/// магия ext2 /userdata и SHA-256 сырого ядра по дескриптору DEIXKIMG.
/// Возвращает список обнаруженных проблем (пусто = всё в порядке).
fn verify_install(target: Drive, userdata_sectors: u32) -> Vec<String> {
    let mut problems: Vec<String> = Vec::new();
    let mut sec = [0u8; 512];

    // 1. MBR: подпись 0x55AA и записи P1/P2.
    if ata::read_sectors_from(target, 0, 1, &mut sec).is_err() {
        problems.push(String::from("MBR не читается"));
        return problems;
    }
    if sec[510] != 0x55 || sec[511] != 0xAA {
        problems.push(String::from("MBR: отсутствует подпись 0x55AA"));
    }
    let parts = [
        (446usize, PART_SYSTEM),
        (462usize, (USERDATA_START_LBA, userdata_sectors)),
    ];
    for (off, (start, secs)) in parts.iter() {
        let lba = u32::from_le_bytes([sec[off + 8], sec[off + 9], sec[off + 10], sec[off + 11]]);
        let cnt = u32::from_le_bytes([sec[off + 12], sec[off + 13], sec[off + 14], sec[off + 15]]);
        if lba != *start || cnt != *secs {
            problems.push(format!(
                "MBR: раздел @{} = LBA {}+{} (ожидалось {}+{})",
                off, lba, cnt, start, secs
            ));
        }
    }

    // 2. /system: суперблок EROFS лежит на смещении 1024 от начала раздела.
    if ata::read_sectors_from(target, PART_SYSTEM.0 + 2, 1, &mut sec).is_ok() {
        let magic = u32::from_le_bytes([sec[0], sec[1], sec[2], sec[3]]);
        if magic != 0xE0F5_E1E2 {
            problems.push(format!("/system: магия EROFS не найдена (0x{:08X})", magic));
        }
    } else {
        problems.push(String::from("/system: суперблок не читается"));
    }

    // 3. /userdata: суперблок ext2 в блоке 1 (LBA start+2), магия на смещении 56.
    if ata::read_sectors_from(target, USERDATA_START_LBA + 2, 1, &mut sec).is_ok() {
        let magic = u16::from_le_bytes([sec[56], sec[57]]);
        if magic != 0xEF53 {
            problems.push(format!("/userdata: магия ext2 не найдена (0x{:04X})", magic));
        }
    } else {
        problems.push(String::from("/userdata: суперблок не читается"));
    }

    // 4. Карта загрузки: разбор, участки внутри /system и SHA-256 ядра,
    // прочитанного по участкам карты с целевого диска.
    if ata::read_sectors_from(target, KERNEL_IMAGE_INFO_LBA, 1, &mut sec).is_err() {
        problems.push(String::from("карта загрузки не читается"));
        return problems;
    }
    match crate::bootmap::parse(&sec) {
        Ok(Some(map)) => {
            match crate::bootmap::read_extents(&map.kernel, map.kernel_size as usize, |lba, count, buf| {
                ata::read_sectors_from(target, lba, count, buf)
            }) {
                Ok(kernel) => {
                    if crate::crypto::sha256::sha256(&kernel) != map.kernel_sha256 {
                        problems.push(String::from(
                            "ядро: SHA-256 kernel.bin по карте загрузки не совпадает с записанным в карте",
                        ));
                    }
                }
                Err(e) => problems.push(format!("ядро: {}", e)),
            }
        }
        Ok(None) => problems.push(String::from("карта загрузки DEIXKIMG отсутствует")),
        Err(e) => problems.push(format!("карта загрузки: {}", e)),
    }

    problems
}

// ==================== ПОШАГОВАЯ УСТАНОВКА (InstallJob) ====================

/// Текущий этап установки.
enum Stage {
    Mbr,
    ZeroFill { next: u32 },
    BootMap,
    System { next: u32 },
    /// Этап 2: список файлов для переноса ещё только собирается.
    MigrateScan,
    /// Этап 2: чтение файлов источника в память (до форматирования).
    MigrateRead { idx: usize },
    UserdataFormat { g: u32 },
    UserdataFinish,
    UserDb,
    /// Этап 2: запись перенесённых файлов на новый том.
    MigrateWrite { idx: usize },
    /// Этап 3: маркер первой загрузки на новом диске.
    FirstBoot,
    Verify,
    Encrypt,
    /// Этап 3: перечитывание записанного (если том не зашифрован).
    FinalCheck,
    Done,
    Failed,
}

/// Пошаговая установка DeiX на диск. Каждый вызов step() выполняет
/// ограниченную порцию работы (не больше COPY_CHUNK_SECTORS записи) —
/// графический установщик вызывает её раз в кадр.
pub struct InstallJob {
    target: Drive,
    userdata_sectors: u32,
    username: String,
    password: String,
    /// Этап 2: переносить ли аккаунты и обои с загрузочного диска.
    migrate: bool,
    /// Найденные на источнике файлы для переноса: (путь, размер).
    migrate_list: Vec<(String, u32)>,
    /// Прочитанные в память файлы источника: (путь, данные).
    migrate_buf: Vec<(String, Vec<u8>)>,
    /// Сколько файлов уже записано на целевой диск (без USERS.DB).
    migrated_files: usize,
    /// Зашифрован ли целевой том в ходе установки (Master): если да,
    /// финальное перечитывание содержимого без ключа невозможно.
    encrypted_now: bool,
    stage: Stage,
    plan: crate::ext2::FormatPlan,
    /// Прогресс: выполнено/всего условных единиц (сектора записи).
    done_units: u32,
    total_units: u32,
    error: Option<String>,
    /// Заметки, накопленные по ходу (например "шифрование пропущено").
    notes: Vec<String>,
}

impl InstallJob {
    /// Готовит установку. `userdata_sectors` — размер P2 (>= 32768,
    /// то есть 16 МиБ). `migrate` — этап 2: перенести аккаунты и обои
    /// с загрузочного диска. Ошибка — если размер не проходит
    /// планирование ext2.
    pub fn new(
        target: Drive,
        userdata_sectors: u32,
        username: String,
        password: String,
        migrate: bool,
    ) -> Result<InstallJob, String> {
        if userdata_sectors < USERDATA_MIN_SECTORS {
            return Err(format!(
                "/userdata size {} sectors < minimum {}",
                userdata_sectors, USERDATA_MIN_SECTORS
            ));
        }
        let plan = crate::ext2::plan_format_raw(target, USERDATA_START_LBA, userdata_sectors)
            .map_err(|e| format!("ext2 plan failed: {:?}", e))?;

        let ks = kernel_sectors();
        let zero = KERNEL_AREA_SECTORS - 1;
        // ~524 сектора метаданных на группу ext2 (SB+GDT+карты+таблица
        // инодов) — оценка для шкалы прогресса; точность не критична.
        let userdata_units = plan.groups * 524 + 4;
        // Раскладка: 2 = MBR; zero = ZeroFill; 1 = BootMap; PART_SYSTEM.1 =
        // /system; userdata_units = форматирование + корневой каталог (4);
        // 2 = UserDb; ks = Verify (чтение ядра); 3 = FirstBoot + Encrypt +
        // FinalCheck (этап 3).
        let total_units = 2 + zero + 1 + PART_SYSTEM.1 + userdata_units + 2 + ks + 3;

        Ok(InstallJob {
            target,
            userdata_sectors,
            username,
            password,
            migrate,
            migrate_list: Vec::new(),
            migrate_buf: Vec::new(),
            migrated_files: 0,
            encrypted_now: false,
            // Этап 2 идёт ПЕРВЫМ: чтение данных источника должно
            // завершиться до перезаписи MBR/маркера шифрования.
            stage: if migrate { Stage::MigrateScan } else { Stage::Mbr },
            plan,
            done_units: 0,
            total_units,
            error: None,
            notes: Vec::new(),
        })
    }

    pub fn finished(&self) -> bool {
        matches!(self.stage, Stage::Done)
    }

    pub fn failed(&self) -> bool {
        matches!(self.stage, Stage::Failed)
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn notes(&self) -> &[String] {
        &self.notes
    }

    pub fn progress_percent(&self) -> u32 {
        if self.total_units == 0 {
            return 0;
        }
        (self.done_units.min(self.total_units) as u64 * 100 / self.total_units as u64) as u32
    }

    /// Название текущего этапа (для строки состояния установщика).
    pub fn stage_label(&self) -> &'static str {
        match self.stage {
            Stage::Mbr => "Writing partition table (MBR)",
            Stage::ZeroFill { .. } => "Clearing reserved boot area",
            Stage::BootMap => "Writing boot map",
            Stage::System { .. } => "Copying /system (EROFS)",
            Stage::MigrateScan => "Scanning this disk for your files",
            Stage::MigrateRead { .. } => "Reading your data from the boot disk",
            Stage::UserdataFormat { .. } => "Formatting /userdata (ext2)",
            Stage::UserdataFinish => "Creating /userdata root directory",
            Stage::UserDb => "Creating user account",
            Stage::MigrateWrite { .. } => "Restoring your data on the new disk",
            Stage::FirstBoot => "Preparing first boot",
            Stage::Verify => "Verifying installed system",
            Stage::Encrypt => "Enabling disk encryption",
            Stage::FinalCheck => "Final check of the new disk",
            Stage::Done => "Install complete",
            Stage::Failed => "Install failed",
        }
    }

    /// Выполняет один шаг. Возвращает true, пока работа не кончилась.
    pub fn step(&mut self) -> bool {
        if matches!(self.stage, Stage::Done | Stage::Failed) {
            return false;
        }
        if let Err(msg) = self.do_step() {
            self.error = Some(msg);
            self.stage = Stage::Failed;
            return false;
        }
        !matches!(self.stage, Stage::Done | Stage::Failed)
    }

    fn do_step(&mut self) -> Result<(), String> {
        match self.stage {
            Stage::Mbr => {
                let mbr = build_mbr(self.userdata_sectors);
                ata::write_sectors_to(self.target, 0, 1, &mbr)
                    .map_err(|_| String::from("MBR write failed"))?;
                // Сброс маркерного сектора 4095 (маркер шифрования).
                let zero = [0u8; 512];
                ata::write_sectors_to(self.target, 4095, 1, &zero)
                    .map_err(|_| String::from("marker sector write failed"))?;
                self.done_units += 2;
                self.stage = Stage::ZeroFill { next: 0 };
            }
            Stage::ZeroFill { next } => {
                // Служебная область до /system (кроме MBR) обнуляется: на
                // диске, где раньше стояла другая версия DeiX, в ней остались
                // старые stage2, ramboot, ядро и карта.
                let area = KERNEL_AREA_SECTORS - 1;
                if next >= area {
                    self.stage = Stage::BootMap;
                } else {
                    let n = COPY_CHUNK_SECTORS.min(area - next);
                    let zeros = alloc::vec![0u8; n as usize * 512];
                    ata::write_sectors_to(self.target, 1 + next, n as u8, &zeros)
                        .map_err(|_| String::from("zero-fill write failed"))?;
                    self.done_units += n;
                    self.stage = Stage::ZeroFill { next: next + n };
                }
            }
            Stage::BootMap => {
                // Карта загрузки (DEIXKIMG, LBA 2047) копируется с загрузочного
                // носителя: /system копируется с теми же LBA, поэтому
                // участки карты остаются верными.
                copy_sectors(self.target, KERNEL_IMAGE_INFO_LBA, 1)?;
                self.done_units += 1;
                self.stage = Stage::System { next: 0 };
            }
            Stage::System { next } => {
                let (start_sys, secs_sys) = PART_SYSTEM;
                let n = COPY_CHUNK_SECTORS.min(secs_sys - next);
                copy_sectors(self.target, start_sys + next, n)?;
                self.done_units += n;
                if next + n >= secs_sys {
                    self.stage = Stage::UserdataFormat { g: 0 };
                } else {
                    self.stage = Stage::System { next: next + n };
                }
            }
            Stage::MigrateScan => {
                self.migrate_list = scan_migrate_files();
                // Чтение и запись каждого файла — единицы прогресса.
                self.total_units += 1 + 2 * self.migrate_list.len() as u32;
                self.done_units += 1;
                if self.migrate_list.is_empty() {
                    self.notes.push(String::from(
                        "nothing to migrate: no user files on the boot disk",
                    ));
                } else if self.migrate_list.len() >= 256 {
                    self.notes.push(String::from(
                        "scan limit reached: only the first 256 files are migrated",
                    ));
                }
                self.stage = Stage::MigrateRead { idx: 0 };
            }
            Stage::MigrateRead { idx } => {
                if idx >= self.migrate_list.len() {
                    // Данные в памяти — можно перезаписывать диск.
                    self.stage = Stage::Mbr;
                    return Ok(());
                }
                let (path, size) = self.migrate_list[idx].clone();
                // При установке на Slave исходный диск переживает
                // форматирование — в память заранее читается только
                // USERS.DB (нужна для слияния аккаунтов), остальные
                // файлы копируются поштучно на этапе MigrateWrite.
                if self.target != Drive::Master && path != crate::auth::USERS_DB_FILE {
                    self.done_units += 1;
                    self.stage = Stage::MigrateRead { idx: idx + 1 };
                    return Ok(());
                }
                let used: usize = self.migrate_buf.iter().map(|(_, d)| d.len()).sum();
                if size as usize > MIGRATE_FILE_MAX_BYTES {
                    self.notes.push(format!("not migrated (file larger than 1 MiB): {}", path));
                } else if used.saturating_add(size as usize) > MIGRATE_TOTAL_MAX_BYTES {
                    self.notes.push(format!("not migrated (memory limit reached): {}", path));
                } else {
                    match crate::ext2::read_file_path(&path) {
                        Ok(data) => self.migrate_buf.push((path, data)),
                        Err(_) => self.notes.push(format!("not migrated (unreadable): {}", path)),
                    }
                }
                self.done_units += 1;
                self.stage = Stage::MigrateRead { idx: idx + 1 };
            }
            Stage::UserdataFormat { g } => {
                crate::ext2::format_group(&self.plan, g)
                    .map_err(|e| format!("ext2 format (group {}): {:?}", g, e))?;
                self.done_units += 524;
                if g + 1 >= self.plan.groups {
                    self.stage = Stage::UserdataFinish;
                } else {
                    self.stage = Stage::UserdataFormat { g: g + 1 };
                }
            }
            Stage::UserdataFinish => {
                crate::ext2::format_finish(&self.plan)
                    .map_err(|e| format!("ext2 format (root dir): {:?}", e))?;
                // Новый /userdata создан записью мимо кеша тома: сбрасываем
                // кеш, чтобы следующие этапы (шифрование при цели = Master)
                // видели свежую ФС, а не блоки прежнего тома.
                crate::ext2::invalidate_cache(crate::ext2::VolId::Userdata);
                self.done_units += 4;
                self.stage = Stage::UserDb;
            }
            Stage::UserDb => {
                // USERS.DB тем же форматом, что auth.rs (username:salt:hash),
                // ПРЯМО на целевой диск — create_user() писал бы на
                // ТЕКУЩИЙ (загрузочный) диск. Этап 2: к новому аккаунту
                // добавляются перенесённые с загрузочного диска; при
                // совпадении имён побеждает новый (его пароль только что
                // ввели в мастере).
                let mut text = build_users_db(&self.username, &self.password);
                let mut accounts = 0usize;
                if self.migrate {
                    let old = self
                        .migrate_buf
                        .iter()
                        .find(|(p, _)| p == crate::auth::USERS_DB_FILE)
                        .and_then(|(_, d)| core::str::from_utf8(d).ok());
                    if let Some(db) = old {
                        for line in db.lines() {
                            let name = line.split(':').next().unwrap_or("");
                            if name.is_empty() || name == self.username {
                                continue;
                            }
                            text.push_str(line);
                            text.push('\n');
                            accounts += 1;
                        }
                    }
                }
                if accounts > 0 {
                    self.notes
                        .push(format!("migrated {} account(s) from the boot disk", accounts));
                }
                crate::ext2::write_file_raw(
                    self.target,
                    USERDATA_START_LBA,
                    self.userdata_sectors,
                    crate::auth::USERS_DB_FILE,
                    text.as_bytes(),
                )
                .map_err(|e| format!("USERS.DB write failed: {:?}", e))?;
                self.done_units += 2;
                // Этап 3 (маркер первой загрузки) выполняется всегда: даже
                // без переноса новый диск должен поприветствовать пользователя
                // при первом запуске. Пустой MigrateWrite пролистывается мгновенно.
                self.stage = Stage::MigrateWrite { idx: 0 };
            }
            Stage::MigrateWrite { idx } => {
                // Для Master все данные уже в памяти (исходный диск перезаписан);
                // для Slave исходный диск цел, и файлы копируются поштучно
                // прямо с него (чтение→запись за один шаг).
                let from_ram = self.target == Drive::Master;
                let total = if from_ram {
                    self.migrate_buf.len()
                } else {
                    self.migrate_list.len()
                };
                if idx >= total {
                    if self.migrated_files > 0 {
                        self.notes.push(format!(
                            "migrated {} file(s) from the boot disk",
                            self.migrated_files
                        ));
                    }
                    self.stage = Stage::FirstBoot;
                    return Ok(());
                }
                let path = if from_ram {
                    self.migrate_buf[idx].0.clone()
                } else {
                    self.migrate_list[idx].0.clone()
                };
                if path == crate::auth::USERS_DB_FILE {
                    // Аккаунты уже записаны объединённой USERS.DB на шаге UserDb.
                    self.stage = Stage::MigrateWrite { idx: idx + 1 };
                    return Ok(());
                }
                let data = if from_ram {
                    core::mem::take(&mut self.migrate_buf[idx]).1
                } else if self.migrate_list[idx].1 as usize > MIGRATE_STREAM_FILE_MAX_BYTES {
                    self.notes.push(format!("not migrated (larger than 8 MiB): {}", path));
                    self.done_units += 1;
                    self.stage = Stage::MigrateWrite { idx: idx + 1 };
                    return Ok(());
                } else {
                    match crate::ext2::read_file_path(&path) {
                        Ok(d) => d,
                        Err(_) => {
                            self.notes.push(format!("not migrated (unreadable): {}", path));
                            self.done_units += 1;
                            self.stage = Stage::MigrateWrite { idx: idx + 1 };
                            return Ok(());
                        }
                    }
                };
                crate::ext2::write_file_path_raw(
                    self.target,
                    USERDATA_START_LBA,
                    self.userdata_sectors,
                    &path,
                    &data,
                )
                .map_err(|e| format!("migrate write {} failed: {:?}", path, e))?;
                self.migrated_files += 1;
                self.done_units += 1;
                self.stage = Stage::MigrateWrite { idx: idx + 1 };
            }
            Stage::FirstBoot => {
                // Этап 3: маркер первой загрузки нового диска. Для Master он
                // пишется ДО шифрования: enable_encryption собирает все файлы
                // тома и укладывает маркер под ключ вместе с ними.
                let marker = format!("installed by DeiX installer, {}\n", crate::OS_RELEASE);
                crate::ext2::write_file_path_raw(
                    self.target,
                    USERDATA_START_LBA,
                    self.userdata_sectors,
                    FIRST_BOOT_FILE,
                    marker.as_bytes(),
                )
                .map_err(|e| format!("first-boot marker write failed: {:?}", e))?;
                self.notes.push(String::from(
                    "first-boot marker written (welcome + encryption reminder)",
                ));
                self.done_units += 1;
                self.stage = Stage::Verify;
            }
            Stage::Verify => {
                let problems = verify_install(self.target, self.userdata_sectors);
                self.done_units += kernel_sectors();
                if !problems.is_empty() {
                    return Err(format!("verification failed: {}", problems.join("; ")));
                }
                self.stage = Stage::Encrypt;
            }
            Stage::Encrypt => {
                // Шифрование включается только для Master (текущего
                // диска): crypto_storage работает с томом /userdata
                // ТЕКУЩЕГО диска. Для Slave оно включится при первой
                // загрузке С НЕГО.
                if self.target == Drive::Master {
                    if crate::crypto_storage::is_encryption_enabled() {
                        self.notes.push(String::from("encryption already enabled"));
                        self.encrypted_now = true;
                    } else {
                        match crate::crypto_storage::enable_encryption(&self.password) {
                            Ok(()) => {
                                self.notes
                                    .push(String::from("disk encryption enabled (XTS-AES-256)"));
                                self.encrypted_now = true;
                            }
                            Err(_) => self.notes.push(String::from(
                                "encryption could not be enabled now (enable on first boot)",
                            )),
                        }
                    }
                } else {
                    self.notes.push(String::from(
                        "target is Slave: encryption will enable on first boot from it",
                    ));
                }
                self.done_units += 1;
                self.stage = Stage::FinalCheck;
            }
            Stage::FinalCheck => {
                // Этап 3: перечитывание USERS.DB с нового диска — проверка
                // содержимого поверх структурной проверки Verify. Если том
                // зашифрован в ходе установки, raw-чтение вернёт шифротекст:
                // тогда содержимое честно проверяется при первой загрузке.
                if self.encrypted_now {
                    self.notes.push(String::from(
                        "final check: volume is encrypted; content verified on first boot",
                    ));
                } else {
                    let db = crate::ext2::read_file_path_raw(
                        self.target,
                        USERDATA_START_LBA,
                        self.userdata_sectors,
                        crate::auth::USERS_DB_FILE,
                    )
                    .map_err(|e| format!("final check: USERS.DB unreadable: {:?}", e))?;
                    let accounts = db
                        .split(|&b| b == b'\n')
                        .filter(|line| !line.is_empty())
                        .count();
                    self.notes.push(format!(
                        "final check: {} account(s) readable on the new disk",
                        accounts
                    ));
                }
                self.done_units += 1;
                self.stage = Stage::Done;
            }
            Stage::Done | Stage::Failed => {}
        }
        Ok(())
    }
}

/// Копирует n секторов с загрузочного носителя (ata::read_sectors —
/// учитывает RAM-диск при загрузке с USB) на целевой диск, те же LBA.
fn copy_sectors(target: Drive, lba: u32, n: u32) -> Result<(), String> {
    let mut buf = alloc::vec![0u8; n as usize * 512];
    ata::read_sectors(lba, n as u8, &mut buf)
        .map_err(|_| format!("source read failed at LBA {}", lba))?;
    ata::write_sectors_to(target, lba, n as u8, &buf)
        .map_err(|_| format!("target write failed at LBA {}", lba))?;
    Ok(())
}

/// Этап 2: рекурсивно обходит текущий /userdata и собирает ВСЕ обычные
/// файлы (пути без ведущего '/' — как их понимает ext2): аккаунты, обои,
/// настройки, загрузки, медиапак — всё, что лежит на пользовательском
/// томе. lost+found пропускается; глубина ограничена 8 уровнями,
/// количество — 256 файлами (защита от неожиданного мусора).
fn scan_migrate_files() -> Vec<(String, u32)> {
    let mut out: Vec<(String, u32)> = Vec::new();
    scan_tree("", 0, &mut out);
    out
}

fn scan_tree(dir: &str, depth: usize, out: &mut Vec<(String, u32)>) {
    if depth >= 8 || out.len() >= 256 {
        return;
    }
    let entries = match crate::ext2::list_dir_path(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for e in entries {
        if out.len() >= 256 {
            return;
        }
        if e.name == "lost+found" {
            continue;
        }
        let path = if dir.is_empty() {
            e.name.clone()
        } else {
            format!("{}/{}", dir, e.name)
        };
        if e.is_directory {
            scan_tree(&path, depth + 1, out);
        } else {
            out.push((path, e.size));
        }
    }
}

/// Строит USERS.DB в формате auth.rs: `username:salt:sha256(salt+pass)`.
fn build_users_db(username: &str, password: &str) -> String {
    let mut salt = [0u8; 16];
    crate::rng::fill_random(&mut salt);
    let mut salted: Vec<u8> = Vec::new();
    salted.extend_from_slice(&salt);
    salted.extend_from_slice(password.as_bytes());
    let hash = crate::crypto::sha256::sha256(&salted);
    let mut text = String::new();
    text.push_str(username);
    text.push(':');
    for b in salt.iter() {
        text.push_str(&format!("{:02x}", b));
    }
    text.push(':');
    for b in hash.iter() {
        text.push_str(&format!("{:02x}", b));
    }
    text.push('\n');
    text
}

// ==================== ВВОД МАСТЕРА ====================

/// Чтение строки с клавиатуры PS/2 или COM1 (неблокирующий опрос).
fn read_input_line() -> String {
    let mut line = String::new();
    loop {
        let c: Option<u8> = if crate::serial::is_data_ready() {
            Some(crate::serial::read_byte())
        } else {
            crate::keyboard::try_read_char()
        };
        match c {
            Some(b'\n') | Some(b'\r') => break,
            Some(0x08) | Some(0x7F) => {
                line.pop();
            }
            Some(other) => line.push(other as char),
            None => {}
        }
    }
    line
}

// ==================== ИНТЕРАКТИВНЫЙ МАСТЕР (CLI) ====================

/// Главная команда `install` — мастер установки с интерфейсом.
/// Поддерживает неинтерактивный режим для автоматизации/тестов:
///   install --test <user> <pass>   (диск выбирается первым доступным,
///   /userdata = 8 МиБ, подтверждение стирания не требуется)
pub fn cmd_install(arg: &str) {
    crate::println!("==================================================");
    crate::println!("     DeiX OS - Installer (setup wizard)          ");
    crate::println!("==================================================");
    crate::println!("  Full layout: /system (EROFS RO), /userdata (EXT2 RW)");
    crate::println!();

    // ---- Режим --test (автоматизация) ----
    let parts: Vec<&str> = arg.split_whitespace().collect();
    let test_mode: bool = parts.first().map(|s| *s == "--test").unwrap_or(false);
    let mut test_user: String = String::new();
    let mut test_pass: String = String::new();
    if test_mode {
        test_user = parts.get(1).cloned().unwrap_or("user1").to_string();
        test_pass = parts.get(2).cloned().unwrap_or("pass123").to_string();
        crate::println!("  [test] non-interactive mode: user='{}'", test_user);
    }

    // ---- Шаг 1: выбор диска ----
    crate::println!("  Step 1/6 - select target disk:");
    let disks = detect_disks();
    if disks.is_empty() {
        crate::println!("  ERROR: no disk found (ATA).");
        return;
    }
    for (i, d) in disks.iter().enumerate() {
        match d.sectors {
            Some(s) => crate::println!("    {}) {} - {} MiB", i + 1, d.label, s / 2048),
            None => crate::println!("    {}) {} - size unknown", i + 1, d.label),
        }
    }
    let (target, target_sectors) = if test_mode {
        (disks[0].drive, disks[0].sectors)
    } else {
        crate::print!("  Your choice [1-{}]: ", disks.len());
        let choice = read_input_line();
        let idx = choice.trim().parse::<usize>().unwrap_or(1).saturating_sub(1).min(disks.len() - 1);
        (disks[idx].drive, disks[idx].sectors)
    };
    crate::println!(
        "  Target disk: {}",
        if target == Drive::Master { "ATA Master" } else { "ATA Slave" }
    );
    if let Some(cap) = target_sectors {
        if cap < USERDATA_START_LBA + USERDATA_MIN_SECTORS {
            crate::println!(
                "  ERROR: disk too small: {} sectors (< {} required).",
                cap,
                USERDATA_START_LBA + USERDATA_MIN_SECTORS
            );
            return;
        }
    }
    crate::println!();

    // ---- Шаг 2: размер /userdata ----
    let max_mib = target_sectors.map(|s| max_userdata_sectors(s) / 2048);
    let userdata_sectors: u32;
    if test_mode {
        userdata_sectors = USERDATA_MIN_SECTORS;
        crate::println!("  Step 2/6 - /userdata size: 16 MiB (test default)");
    } else {
        match max_mib {
            Some(m) => crate::println!("  Step 2/6 - /userdata size in MiB (16..{}), Enter = 16:", m),
            None => crate::println!("  Step 2/6 - /userdata size in MiB (>= 16), Enter = 16:"),
        }
        loop {
            crate::print!("  Size [MiB]: ");
            let line = read_input_line();
            let trimmed = line.trim();
            let mib = if trimmed.is_empty() { 16 } else { trimmed.parse::<u32>().unwrap_or(0) };
            if mib < 16 || mib > 2048 {
                crate::println!("  Invalid size. Enter a number from 16 to 2048.");
                continue;
            }
            if let Some(m) = max_mib {
                if mib > m {
                    crate::println!("  Disk only fits {} MiB of /userdata.", m);
                    continue;
                }
            }
            userdata_sectors = mib * 2048;
            break;
        }
    }
    crate::println!();

    // ---- Шаг 3: username ----
    let username: String;
    if test_mode {
        username = test_user.clone();
        crate::println!("  Step 3/6 - user: {}", username);
    } else {
        crate::println!("  Step 3/6 - username (letters, digits, '_' or '-', up to 32):");
        loop {
            crate::print!("  Username: ");
            let u = read_input_line();
            let valid = !u.trim().is_empty()
                && u.trim().len() <= 32
                && u.trim().chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
            if valid {
                username = u.trim().to_string();
                break;
            }
            crate::println!("  Invalid username. Try again.");
        }
    }
    crate::println!();

    // ---- Шаг 4: пароль + подтверждение ----
    let password: String;
    if test_mode {
        password = test_pass.clone();
        crate::println!("  Step 4/6 - password set (disk encryption key).");
    } else {
        crate::println!("  Step 4/6 - user password (disk encryption key):");
        loop {
            crate::print!("  Password: ");
            let p1 = read_input_line();
            crate::print!("  Confirm:  ");
            let p2 = read_input_line();
            if p1.is_empty() || p1 != p2 {
                crate::println!("  Passwords do not match or are empty. Try again.");
                continue;
            }
            password = p1;
            break;
        }
    }
    crate::println!();

    // ---- Шаг 5: подтверждение стирания ----
    if !test_mode {
        crate::println!("  Step 5/6 - WARNING: the disk will be FULLY WIPED!");
        crate::println!("  All data on the target disk will be destroyed.");
        crate::print!("  Type 'yes' to confirm: ");
        let confirm = read_input_line();
        if confirm.trim() != "yes" {
            crate::println!("  Install cancelled.");
            return;
        }
    } else {
        crate::println!("  Step 5/6 - (test) confirmation skipped.");
    }
    crate::println!();

    // ---- Этап 2: перенос всех пользовательских файлов с загрузочного диска ----
    let migrate = if test_mode {
        crate::println!("  Migrate data: skipped (test mode).");
        false
    } else {
        crate::print!("  Migrate all your files from this disk? [Y/n]: ");
        let ans = read_input_line();
        !matches!(ans.trim(), "n" | "N" | "no" | "No" | "NO")
    };
    crate::println!();

    // ---- Шаг 6: установка (пошаговый job) ----
    crate::println!("  Step 6/6 - installing DeiX to disk...");
    let mut job = match InstallJob::new(target, userdata_sectors, username.clone(), password, migrate) {
        Ok(j) => j,
        Err(e) => {
            crate::println!("  ERROR: {}", e);
            return;
        }
    };
    let mut last_label = "";
    loop {
        let label = job.stage_label();
        if label != last_label {
            crate::println!("  [install] {}...", label);
            last_label = label;
        }
        if !job.step() {
            break;
        }
    }

    if job.failed() {
        crate::println!("==================================================");
        crate::println!("  INSTALL FAILED: {}", job.error().unwrap_or("unknown error"));
        crate::println!("  The target disk may be unbootable. Re-run 'install'.");
        crate::println!("==================================================");
        return;
    }

    for note in job.notes() {
        crate::println!("  [install] note: {}", note);
    }
    crate::println!();
    crate::println!("==================================================");
    crate::println!("  INSTALL COMPLETE!");
    crate::println!("  User: {}", username);
    crate::println!("  /userdata: {} MiB (ext2)", userdata_sectors / 2048);
    crate::println!("  Disk is partitioned and ready to boot standalone.");
    crate::println!("==================================================");

    // Этап 3: предложение немедленно перезагрузиться. Для Slave BIOS
    // всё равно грузит первый диск — честно предупреждаем об этом.
    if !test_mode {
        if target == Drive::Slave {
            crate::println!("  Note: BIOS boots the first disk; select the new disk");
            crate::println!("  in the boot menu (in QEMU: put it first, e.g. -hda).");
        }
        crate::print!("  Reboot now? [y/N]: ");
        let ans = read_input_line();
        if matches!(ans.trim(), "y" | "Y" | "yes" | "Yes" | "YES") {
            crate::cli::cmd_reboot();
        }
    }
}
