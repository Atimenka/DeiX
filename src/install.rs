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
//   1. MBR с таблицей разделов (P1 /system EROFS, P2 /userdata ext2 —
//      размер /userdata ВЫБИРАЕТСЯ при установке);
//   2. stage2 + ramboot + сырой kernel.bin (копируется с загрузочного
//      носителя посекторно) + дескриптор DEIXKIMG;
//   3. образ /system (EROFS) посекторно;
//   4. форматирование /userdata в ext2 (многогрупповый том под
//      выбранный размер — см. ext2::plan_format_raw);
//   5. USERS.DB с учётной записью пользователя;
//   6. проверка (MBR, магии ФС, SHA-256 ядра);
//   7. шифрование (XTS-AES-256) — только если цель = Master.

use crate::ata::{self, Drive};
use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

// ==================== КОНСТАНТЫ УСТАНОВКИ ====================

/// Максимум секторов, заполняемых нулями в служебной области
/// (stage2 + ramboot + kernel + дескриптор + область дампа) до начала
/// /system (LBA 4096). Дескриптор ядра (LBA 2047) записывается заново
/// после заполнения.
const KERNEL_AREA_SECTORS: u32 = 3500;

/// Сектор дескриптора образа ядра DEIXKIMG (см. src/partition_map.rs).
const KERNEL_IMAGE_INFO_LBA: u32 = 2047;

/// Карта разделов: P1 /system фиксированный (совпадает с
/// tools/make_deix_fs.py), P2 /userdata начинается сразу после него,
/// а его РАЗМЕР выбирается при установке.
const PART_SYSTEM: (u32, u32) = (4096, 20480);
/// Начало /userdata (ext2::FS_START_LBA).
pub const USERDATA_START_LBA: u32 = 24576;
/// Минимальный размер /userdata: 8 МиБ (штатный образ).
pub const USERDATA_MIN_SECTORS: u32 = 16384;
/// Варианты размера /userdata для установщика, МиБ.
pub const USERDATA_PRESETS_MIB: [u32; 6] = [8, 16, 32, 64, 128, 256];

/// Максимальный размер /userdata на диске данной ёмкости.
pub fn max_userdata_sectors(disk_sectors: u32) -> u32 {
    disk_sectors.saturating_sub(USERDATA_START_LBA)
}

/// Секторов, копируемых/пишущихся за один шаг InstallJob (64 КиБ) —
/// достаточно мелко, чтобы графический установщик оставался отзывчивым.
const COPY_CHUNK_SECTORS: u32 = 128;

/// MBR-загрузчик, включённый в ядро (build.sh собирает boot_sector.bin ДО
/// компиляции ядра; NUM_SECTORS = размер stage2).
const BOOT_SECTOR: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/build/boot_sector.bin"));
/// Загрузчик второй стадии (stage2.bin) — маленький; грузится boot_sector'ом.
const STAGE2: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/build/stage2.bin"));
/// RAM-диск загрузчик (ramboot.bin): дочитывает весь образ в RAM и передаёт
/// управление stage2. Лежит сразу после stage2 (LBA 1+stage2_sectors).
const RAMBOOT: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/build/ramboot.bin"));

// Конец образа ядра (linker_kernel.ld: __image_end в конце .data, выровнен до 512).
extern "C" {
    static __image_end: u8;
}

/// База ядра в памяти (linker_kernel.ld): 0x100000 (1 МиБ).
const KERNEL_BASE: usize = 0x100000;

fn stage2_sectors() -> u32 {
    ((STAGE2.len() + 511) / 512) as u32
}
fn ramboot_sectors() -> u32 {
    ((RAMBOOT.len() + 511) / 512) as u32
}
/// Стартовый LBA kernel.bin на диске: сразу после stage2 + ramboot.
fn kernel_start_lba() -> u32 {
    1 + stage2_sectors() + ramboot_sectors()
}
/// Размер образа ядра на диске: от базы 0x100000 до __image_end.
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

    // 4. Сырое ядро: SHA-256 по дескриптору DEIXKIMG (если он записан).
    if ata::read_sectors_from(target, KERNEL_IMAGE_INFO_LBA, 1, &mut sec).is_ok()
        && &sec[0..8] == b"DEIXKIMG"
    {
        let lba = u32::from_le_bytes([sec[8], sec[9], sec[10], sec[11]]);
        let size = u32::from_le_bytes([sec[12], sec[13], sec[14], sec[15]]);
        let mut expected = [0u8; 32];
        expected.copy_from_slice(&sec[16..48]);

        let mut hasher = crate::crypto::sha256::Sha256::new();
        let mut left = size as usize;
        let mut cur = lba;
        let mut ok = true;
        while left > 0 {
            if ata::read_sectors_from(target, cur, 1, &mut sec).is_err() {
                problems.push(String::from("ядро: сектор не читается при проверке"));
                ok = false;
                break;
            }
            let take = left.min(512);
            hasher.update(&sec[..take]);
            left -= take;
            cur += 1;
        }
        if ok && hasher.finalize() != expected {
            problems.push(String::from(
                "ядро: SHA-256 сырого kernel.bin не совпадает с дескриптором DEIXKIMG",
            ));
        }
    }

    problems
}

// ==================== ПОШАГОВАЯ УСТАНОВКА (InstallJob) ====================

/// Текущий этап установки.
enum Stage {
    Mbr,
    BootFiles,
    Kernel { next: u32 },
    ZeroFill { next: u32 },
    KernelInfo,
    System { next: u32 },
    UserdataFormat { g: u32 },
    UserdataFinish,
    UserDb,
    Verify,
    Encrypt,
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
    /// Готовит установку. `userdata_sectors` — размер P2 (>= 16384).
    /// Ошибка — если размер не проходит планирование ext2.
    pub fn new(
        target: Drive,
        userdata_sectors: u32,
        username: String,
        password: String,
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
        let boot = stage2_sectors() + ramboot_sectors();
        let zero = KERNEL_AREA_SECTORS.saturating_sub(boot + ks);
        // ~524 сектора метаданных на группу ext2 (SB+GDT+карты+таблица
        // инодов) — оценка для шкалы прогресса; точность не критична.
        let userdata_units = plan.groups * 524 + 4;
        let total_units = 2 + boot + ks + zero + 1 + PART_SYSTEM.1 + userdata_units + 2 + ks + 1;

        Ok(InstallJob {
            target,
            userdata_sectors,
            username,
            password,
            stage: Stage::Mbr,
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
            Stage::BootFiles => "Writing bootloader (stage2 + ramboot)",
            Stage::Kernel { .. } => "Copying kernel image",
            Stage::ZeroFill { .. } => "Clearing reserved boot area",
            Stage::KernelInfo => "Writing kernel descriptor",
            Stage::System { .. } => "Copying /system (EROFS)",
            Stage::UserdataFormat { .. } => "Formatting /userdata (ext2)",
            Stage::UserdataFinish => "Creating /userdata root directory",
            Stage::UserDb => "Creating user account",
            Stage::Verify => "Verifying installed system",
            Stage::Encrypt => "Enabling disk encryption",
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
                self.stage = Stage::BootFiles;
            }
            Stage::BootFiles => {
                // stage2.bin — секторы 1..; ramboot.bin — сразу после.
                write_embedded(self.target, 1, STAGE2)?;
                write_embedded(self.target, 1 + stage2_sectors(), RAMBOOT)?;
                self.done_units += stage2_sectors() + ramboot_sectors();
                self.stage = Stage::Kernel { next: 0 };
            }
            Stage::Kernel { next } => {
                // kernel.bin: читаем ФАЙЛОВЫЕ байты с загрузочного
                // носителя (там точная копия kernel.bin с чистым .data)
                // и пишем на целевой диск. НЕ из памяти: "живой" .data
                // содержит указатели на кучу, недействительные на
                // установленной системе.
                let ks = kernel_sectors();
                let kstart = kernel_start_lba();
                let n = COPY_CHUNK_SECTORS.min(ks - next);
                copy_sectors(self.target, kstart + next, n)?;
                self.done_units += n;
                if next + n >= ks {
                    self.stage = Stage::ZeroFill { next: stage2_sectors() + ramboot_sectors() + ks };
                } else {
                    self.stage = Stage::Kernel { next: next + n };
                }
            }
            Stage::ZeroFill { next } => {
                // Дополняем область загрузчика+ядра нулями до границы
                // раздела (включая область аварийного дампа).
                if next >= KERNEL_AREA_SECTORS {
                    self.stage = Stage::KernelInfo;
                } else {
                    let n = COPY_CHUNK_SECTORS.min(KERNEL_AREA_SECTORS - next);
                    let zeros = alloc::vec![0u8; n as usize * 512];
                    ata::write_sectors_to(self.target, 1 + next, n as u8, &zeros)
                        .map_err(|_| String::from("zero-fill write failed"))?;
                    self.done_units += n;
                    self.stage = Stage::ZeroFill { next: next + n };
                }
            }
            Stage::KernelInfo => {
                // Дескриптор образа ядра (DEIXKIMG, LBA 2047): копируем с
                // загрузочного носителя — сырое ядро скопировано с тех же
                // LBA, дескриптор остаётся верным.
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
                self.done_units += 4;
                self.stage = Stage::UserDb;
            }
            Stage::UserDb => {
                // USERS.DB тем же форматом, что auth.rs (username:salt:hash),
                // ПРЯМО на целевой диск — create_user() писал бы на
                // ТЕКУЩИЙ (загрузочный) диск.
                let text = build_users_db(&self.username, &self.password);
                crate::ext2::write_file_raw(
                    self.target,
                    USERDATA_START_LBA,
                    self.userdata_sectors,
                    crate::auth::USERS_DB_FILE,
                    text.as_bytes(),
                )
                .map_err(|e| format!("USERS.DB write failed: {:?}", e))?;
                self.done_units += 2;
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
                    } else {
                        match crate::crypto_storage::enable_encryption(&self.password) {
                            Ok(()) => self
                                .notes
                                .push(String::from("disk encryption enabled (XTS-AES-256)")),
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
                self.stage = Stage::Done;
            }
            Stage::Done | Stage::Failed => {}
        }
        Ok(())
    }
}

/// Пишет встроенный бинарь (stage2/ramboot) на целевой диск по LBA.
fn write_embedded(target: Drive, lba: u32, data: &[u8]) -> Result<(), String> {
    let sectors = (data.len() + 511) / 512;
    for i in 0..sectors {
        let mut buf = [0u8; 512];
        let start = i * 512;
        let end = (start + 512).min(data.len());
        buf[..end - start].copy_from_slice(&data[start..end]);
        ata::write_sectors_to(target, lba + i as u32, 1, &buf)
            .map_err(|_| String::from("bootloader write failed"))?;
    }
    Ok(())
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
        crate::println!("  Step 2/6 - /userdata size: 8 MiB (test default)");
    } else {
        match max_mib {
            Some(m) => crate::println!("  Step 2/6 - /userdata size in MiB (8..{}), Enter = 8:", m),
            None => crate::println!("  Step 2/6 - /userdata size in MiB (>= 8), Enter = 8:"),
        }
        loop {
            crate::print!("  Size [MiB]: ");
            let line = read_input_line();
            let trimmed = line.trim();
            let mib = if trimmed.is_empty() { 8 } else { trimmed.parse::<u32>().unwrap_or(0) };
            if mib < 8 || mib > 2048 {
                crate::println!("  Invalid size. Enter a number from 8 to 2048.");
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

    // ---- Шаг 6: установка (пошаговый job) ----
    crate::println!("  Step 6/6 - installing DeiX to disk...");
    let mut job = match InstallJob::new(target, userdata_sectors, username.clone(), password) {
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
    crate::println!("  Reboot: 'reboot'");
    crate::println!("==================================================");
}
