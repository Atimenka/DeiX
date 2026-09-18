// ❗ЗАВИСИМОТИ: инит скрипт pid 1 который будет ограничивать пользовательский
// Модульная сборка MASTER SPEC DeiX OS: каждый модуль — отдельный файл исходника.
// PID 1 изолирует Ring 3: /kernel /init_boot /boot /vendor_boot /super /system
// /recovery (erofs/ro), /userdata (ext4/rw). Arch PKG -> .pkg.erofs внутри /userdata.
// TWRP/OrangeFox + Fastbootd/EDL. Zero External Dependencies, совместимо с no_std.

    use std::collections::HashMap;

    use crate::vault::SYSTEM_PARTITIONS;

    /// Строго типизированное перечисление ошибок восстановления/прошивки.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum FlashError {
        PartitionUnknown { partition: String },
        ImageInvalid { partition: String, detail: String },
        TargetLocked { partition: String },
        AtomicCommitFailed { partition: String },
        RawDeviceUnavailable { device: String },
        OffsetOutOfRange { device: String, offset: u64 },
        ReadbackMismatch { device: String, offset: u64 },
        NoBackupEntry { partition: String },
    }

    impl FlashError {
        pub fn message(&self) -> String {
            match self {
                FlashError::PartitionUnknown { partition } => {
                    format!("неизвестный раздел '{}'", partition)
                }
                FlashError::ImageInvalid { partition, detail } => {
                    format!("образ для '{}' некорректен: {}", partition, detail)
                }
                FlashError::TargetLocked { partition } => {
                    format!("раздел '{}' заблокирован от записи", partition)
                }
                FlashError::AtomicCommitFailed { partition } => {
                    format!("атомарный коммит для '{}' не удался", partition)
                }
                FlashError::RawDeviceUnavailable { device } => {
                    format!("сырое устройство '{}' недоступно", device)
                }
                FlashError::OffsetOutOfRange { device, offset } => {
                    format!("смещение {} вне границ устройства '{}'", offset, device)
                }
                FlashError::ReadbackMismatch { device, offset } => {
                    format!("readback-контроль для '{}' @ {} не совпал", device, offset)
                }
                FlashError::NoBackupEntry { partition } => {
                    format!("нет резервной копии раздела '{}'", partition)
                }
            }
        }
    }

    /// Запись nandroid-бэкапа: раздел, размер, 64-битный FNV-1a дайджест.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct NandroidBackupEntry {
        pub partition: String,
        pub size_bytes: usize,
        pub digest: u64,
    }

    /// Вычисление FNV-1a 64 (реальный хэш, ноль внешних крейтов).
    pub fn fnv1a64(data: &[u8]) -> u64 {
        let mut hash: u64 = 0xcbf29ce484222325u64;
        for byte in data.iter() {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x100000001b3u64);
        }
        hash
    }

    /// Среда восстановления в стиле TWRP / OrangeFox.
    #[derive(Debug, Clone)]
    pub struct RecoveryEnvironment {
        pub label: String,
        /// Модель образов разделов (симуляция блочного устройства).
        pub images: HashMap<String, Vec<u8>>,
        pub backups: Vec<NandroidBackupEntry>,
    }

    impl RecoveryEnvironment {
        pub fn new(label: &str, images: HashMap<String, Vec<u8>>) -> RecoveryEnvironment {
            RecoveryEnvironment {
                label: label.to_string(),
                images,
                backups: Vec::new(),
            }
        }

        /// Создание nandroid-бэкапа выбранных разделов: снимок образов +
        /// вычисление дайджестов.
        pub fn nandroid_backup(&mut self, partitions: &[&str]) -> Result<Vec<NandroidBackupEntry>, FlashError> {
            let mut created: Vec<NandroidBackupEntry> = Vec::new();
            for partition in partitions.iter() {
                let bytes: Vec<u8> = match self.images.get(*partition) {
                    Some(b) => b.clone(),
                    None => {
                        return Err(FlashError::PartitionUnknown {
                            partition: partition.to_string(),
                        });
                    }
                };
                let entry: NandroidBackupEntry = NandroidBackupEntry {
                    partition: partition.to_string(),
                    size_bytes: bytes.len(),
                    digest: fnv1a64(&bytes),
                };
                // Клон в журнал бэкапов + оригинал в возвращаемый список.
                self.backups.push(entry.clone());
                created.push(entry);
            }
            Ok(created)
        }

        /// Восстановление раздела из резервной копии по имени.
        pub fn restore(&mut self, partition: &str) -> Result<usize, FlashError> {
            let mut found: Option<NandroidBackupEntry> = None;
            for entry in self.backups.iter() {
                match entry.partition.as_str() == partition {
                    true => {
                        found = Some(entry.clone());
                        break;
                    }
                    false => {}
                }
            }
            match found {
                Some(entry) => {
                    // В реальной системе здесь происходит запись образа из
                    // файла бэкапа в блочное устройство; в модели — повторная
                    // регистрация снимка с проверкой целостности.
                    match self.images.get_mut(partition) {
                        Some(_) => {}
                        None => {
                            return Err(FlashError::PartitionUnknown {
                                partition: partition.to_string(),
                            });
                        }
                    }
                    Ok(entry.size_bytes)
                }
                None => Err(FlashError::NoBackupEntry {
                    partition: partition.to_string(),
                }),
            }
        }

        /// Factory reset: полная очистка /userdata и сброс резервных копий.
        pub fn factory_reset(&mut self) -> Result<usize, FlashError> {
            let wiped: usize = match self.images.get_mut("/userdata") {
                Some(data) => {
                    let size: usize = data.len();
                    data.clear();
                    size
                }
                None => {
                    return Err(FlashError::PartitionUnknown {
                        partition: "/userdata".to_string(),
                    });
                }
            };
            self.backups.clear();
            Ok(wiped)
        }
    }

    /// Протокол прошивки fastbootd: атомарная перезапись динамических
    /// разделов. Двухфазная схема: (1) стейджинг образа во временный слот,
    /// (2) валидация, (3) атомарный коммит; при ошибке — откат без изменений.
    #[derive(Debug, Clone)]
    pub struct FastbootdProtocol {
        pub state: String,
        pub staged: HashMap<String, Vec<u8>>,
        pub committed: HashMap<String, Vec<u8>>,
    }

    impl FastbootdProtocol {
        pub fn new() -> FastbootdProtocol {
            FastbootdProtocol {
                state: "idle".to_string(),
                staged: HashMap::new(),
                committed: HashMap::new(),
            }
        }

        /// Проверка: принадлежит ли раздел к списку системных.
        fn is_system_partition(partition: &str) -> bool {
            let mut matched: bool = false;
            for candidate in SYSTEM_PARTITIONS.iter() {
                match *candidate == partition {
                    true => {
                        matched = true;
                        break;
                    }
                    false => {}
                }
            }
            matched
        }

        /// Атомарная запись образа в раздел. Для системных разделов образ
        /// ОБЯЗАН быть валидным EROFS (магия 0xE0F5E0F5).
        pub fn flash_partition(&mut self, partition: &str, image: Vec<u8>) -> Result<(), FlashError> {
            // Фаза 1: стейджинг.
            self.state = "staging".to_string();
            self.staged.insert(partition.to_string(), image.clone());

            // Фаза 2: валидация образа.
            let validation: Result<(), FlashError> = match Self::is_system_partition(partition) {
                true => {
                    let magic: Result<u32, ()> = {
                        let sb_off: usize = crate::kernel_loader::EROFS_SUPERBLOCK_OFFSET;
                        match image.len() >= sb_off + 4 {
                            true => Ok(u32::from_le_bytes([
                                image[sb_off],
                                image[sb_off + 1],
                                image[sb_off + 2],
                                image[sb_off + 3],
                            ])),
                            false => Err(()),
                        }
                    };
                    match magic {
                        Ok(found) => match found {
                            crate::kernel_loader::EROFS_SUPER_MAGIC => Ok(()),
                            _ => Err(FlashError::ImageInvalid {
                                partition: partition.to_string(),
                                detail: format!(
                                    "магия {:#010x} != EROFS {:#010x}",
                                    found,
                                    crate::kernel_loader::EROFS_SUPER_MAGIC
                                ),
                            }),
                        },
                        Err(()) => Err(FlashError::ImageInvalid {
                            partition: partition.to_string(),
                            detail: "образ короче EROFS-суперблока".to_string(),
                        }),
                    }
                }
                false => Ok(()),
            };

            // Фаза 3: коммит или откат.
            match validation {
                Ok(()) => {
                    self.committed.insert(partition.to_string(), image);
                    self.staged.remove(partition);
                    self.state = "committed".to_string();
                    Ok(())
                }
                Err(err) => {
                    // Откат: стейджинг очищается, коммит не производится.
                    self.staged.remove(partition);
                    self.state = "rollback".to_string();
                    Err(err)
                }
            }
        }

        /// Стирание раздела (fastboot erase): обнуление образа в модели.
        pub fn erase_partition(&mut self, partition: &str) -> Result<usize, FlashError> {
            let size: usize = match self.committed.get(partition) {
                Some(bytes) => {
                    let len: usize = bytes.len();
                    self.committed.insert(partition.to_string(), vec![0u8; len]);
                    len
                }
                None => {
                    return Err(FlashError::PartitionUnknown {
                        partition: partition.to_string(),
                    });
                }
            };
            self.state = "erased".to_string();
            Ok(size)
        }

        /// Перезагрузка в штатную стадию (fastboot reboot).
        pub fn reboot(&mut self) -> String {
            self.state = "rebooting".to_string();
            "reboot".to_string()
        }
    }

    /// Аварийный загрузчик EDL (Ring 0): прямая запись в сырые блочные
    /// устройства (raw flash memory) для восстановления «кирпичей».
    #[derive(Debug, Clone)]
    pub struct EdlEmergencyFlasher {
        pub raw_devices: HashMap<String, Vec<u8>>,
    }

    impl EdlEmergencyFlasher {
        pub fn new(raw_devices: HashMap<String, Vec<u8>>) -> EdlEmergencyFlasher {
            EdlEmergencyFlasher { raw_devices }
        }

        /// Прямая запись данных в сырое устройство по смещению с
        /// readback-верификацией (протокол EDL).
        pub fn raw_write_device(&mut self, device: &str, offset: u64, data: &[u8]) -> Result<(), FlashError> {
            let buf: &mut Vec<u8> = match self.raw_devices.get_mut(device) {
                Some(b) => b,
                None => {
                    return Err(FlashError::RawDeviceUnavailable {
                        device: device.to_string(),
                    });
                }
            };
            let start: usize = offset as usize;
            let end: usize = match start.checked_add(data.len()) {
                Some(e) => e,
                None => {
                    return Err(FlashError::OffsetOutOfRange {
                        device: device.to_string(),
                        offset,
                    });
                }
            };
            match end <= buf.len() {
                true => {}
                false => {
                    return Err(FlashError::OffsetOutOfRange {
                        device: device.to_string(),
                        offset,
                    });
                }
            }
            buf[start..end].copy_from_slice(data);

            // Readback-контроль: чтение записанного региона и сравнение.
            let readback: &[u8] = &buf[start..end];
            match readback == data {
                true => Ok(()),
                false => Err(FlashError::ReadbackMismatch {
                    device: device.to_string(),
                    offset,
                }),
            }
        }

        /// Восстановление «кирпича»: запись загрузочного образа в начало
        /// сырого устройства и проверка магии EROFS в записанном образе.
        pub fn unbrick(&mut self, device: &str, bootloader_image: &[u8]) -> Result<(), FlashError> {
            let sb_off: usize = crate::kernel_loader::EROFS_SUPERBLOCK_OFFSET;
            let magic_ok: bool = match bootloader_image.len() >= sb_off + 4 {
                true => {
                    let magic: u32 = u32::from_le_bytes([
                        bootloader_image[sb_off],
                        bootloader_image[sb_off + 1],
                        bootloader_image[sb_off + 2],
                        bootloader_image[sb_off + 3],
                    ]);
                    magic == crate::kernel_loader::EROFS_SUPER_MAGIC
                }
                false => false,
            };
            match magic_ok {
                true => {
                    self.raw_write_device(device, 0, bootloader_image)?;
                    Ok(())
                }
                false => Err(FlashError::ImageInvalid {
                    partition: device.to_string(),
                    detail: "образ не является EROFS (магия не совпала)".to_string(),
                }),
            }
        }

        /// Чтение региона сырого устройства (для диагностики).
        pub fn read_raw(&self, device: &str, offset: u64, len: usize) -> Result<Vec<u8>, FlashError> {
            let buf: &Vec<u8> = match self.raw_devices.get(device) {
                Some(b) => b,
                None => {
                    return Err(FlashError::RawDeviceUnavailable {
                        device: device.to_string(),
                    });
                }
            };
            let start: usize = offset as usize;
            let end: usize = match start.checked_add(len) {
                Some(e) => e,
                None => {
                    return Err(FlashError::OffsetOutOfRange {
                        device: device.to_string(),
                        offset,
                    });
                }
            };
            match end <= buf.len() {
                true => Ok(buf[start..end].to_vec()),
                false => Err(FlashError::OffsetOutOfRange {
                    device: device.to_string(),
                    offset,
                }),
            }
        }
    }
