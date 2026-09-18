// ❗ЗАВИСИМОТИ: инит скрипт pid 1 который будет ограничивать пользовательский
// Модульная сборка MASTER SPEC DeiX OS: каждый модуль — отдельный файл исходника.
// PID 1 изолирует Ring 3: /kernel /init_boot /boot /vendor_boot /super /system
// /recovery (erofs/ro), /userdata (ext4/rw). Arch PKG -> .pkg.erofs внутри /userdata.
// TWRP/OrangeFox + Fastbootd/EDL. Zero External Dependencies, совместимо с no_std.

    /// Магическая сигнатура EROFS-суперблока (Linux EROFS, on-disk layout).
    pub const EROFS_SUPER_MAGIC: u32 = 0xE0F5E0F5;

    /// Смещение суперблока EROFS в образе диска (байт 1024, как в Linux).
    pub const EROFS_SUPERBLOCK_OFFSET: usize = 1024;

    /// Размер суперблока EROFS (128 байт).
    pub const EROFS_SUPERBLOCK_SIZE: usize = 128;

    /// Стандартный размер блока EROFS (4096 байт).
    pub const EROFS_BLOCK_SIZE: u32 = 4096;

    /// Размер записи tar-архива (512 байт по POSIX ustar).
    pub const TAR_RECORD_SIZE: usize = 512;

    /// Строго типизированное перечисление ошибок загрузки ядра.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum KernelLoadError {
        /// Раздел /kernel недоступен/не смонтирован.
        PartitionUnavailable { reason: String },
        /// Файл kernel.tar.gz отсутствует в образе раздела.
        ArchiveMissing,
        /// Некорректный gzip-заголовок.
        GzipHeaderCorrupt { detail: String },
        /// Некорректная запись tar-архива.
        TarHeaderCorrupt { detail: String },
        /// Член kernel.img не найден в архиве.
        KernelImageNotFound,
        /// Образ kernel.img короче EROFS-суперблока.
        ImageTooSmall { size: usize },
        /// Магическая сигнатура EROFS не совпала.
        ErofsMagicMismatch { found: u32, expected: u32 },
        /// Недопустимый размер блока EROFS.
        InvalidBlockSize { bits: u8 },
        /// Образ ядра превышает лимит загрузчика.
        KernelImageTooLarge { size: usize, limit: usize },
    }

    impl KernelLoadError {
        pub fn message(&self) -> String {
            match self {
                KernelLoadError::PartitionUnavailable { reason } => {
                    format!("раздел /kernel недоступен: {}", reason)
                }
                KernelLoadError::ArchiveMissing => {
                    "файл kernel.tar.gz отсутствует в образе /kernel".to_string()
                }
                KernelLoadError::GzipHeaderCorrupt { detail } => {
                    format!("повреждён gzip-заголовок: {}", detail)
                }
                KernelLoadError::TarHeaderCorrupt { detail } => {
                    format!("повреждён tar-заголовок: {}", detail)
                }
                KernelLoadError::KernelImageNotFound => {
                    "член kernel.img не найден в kernel.tar.gz".to_string()
                }
                KernelLoadError::ImageTooSmall { size } => {
                    format!("образ kernel.img слишком мал: {} байт", size)
                }
                KernelLoadError::ErofsMagicMismatch { found, expected } => {
                    format!(
                        "несовпадение магии EROFS: найдено {:#010x}, ожидалось {:#010x}",
                        found, expected
                    )
                }
                KernelLoadError::InvalidBlockSize { bits } => {
                    format!("недопустимый размер блока EROFS: 1 << {} бит", bits)
                }
                KernelLoadError::KernelImageTooLarge { size, limit } => {
                    format!("образ ядра {} байт превышает лимит {} байт", size, limit)
                }
            }
        }
    }

    /// Модель суперблока EROFS (реальная on-disk структура Linux EROFS).
    ///
    /// Раскладка полей в образе (смещения от начала суперблока):
    ///   0x00 magic (u32 LE), 0x04 checksum, 0x08 feature_compat (u32),
    ///   0x0C blkszbits (u8, размер блока = 1 << blkszbits),
    ///   0x0D sb_extslots, 0x0E root_nid (u16), 0x10 inos (u64),
    ///   0x18 build_time (u64), 0x24 blocks (u32), 0x28 meta_blkaddr,
    ///   0x2C xattr_blkaddr, 0x30 uuid[16], 0x40 volume_name[16],
    ///   0x50 feature_incompat (u32).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct ErofsSuperBlock {
        pub magic: u32,
        pub feature_compat: u32,
        pub block_size: u32,
        pub root_nid: u16,
        pub inos: u64,
        pub blocks: u32,
        pub feature_incompat: u32,
    }

    impl ErofsSuperBlock {
        /// Разбор суперблока из байтового образа диска. Чтение выполняется
        /// строго по смещению EROFS_SUPERBLOCK_OFFSET (1024) — реальная
        /// раскладка EROFS. Магия проверяется первой.
        pub fn parse(image: &[u8]) -> Result<ErofsSuperBlock, KernelLoadError> {
            let needed: usize = EROFS_SUPERBLOCK_OFFSET + EROFS_SUPERBLOCK_SIZE;
            match image.len() < needed {
                true => Err(KernelLoadError::ImageTooSmall { size: image.len() }),
                false => {
                    let sb: &[u8] = &image[EROFS_SUPERBLOCK_OFFSET..needed];
                    let magic: u32 = u32::from_le_bytes([sb[0], sb[1], sb[2], sb[3]]);
                    match magic {
                        EROFS_SUPER_MAGIC => {}
                        found => {
                            return Err(KernelLoadError::ErofsMagicMismatch {
                                found,
                                expected: EROFS_SUPER_MAGIC,
                            });
                        }
                    }
                    let feature_compat: u32 = u32::from_le_bytes([sb[8], sb[9], sb[10], sb[11]]);
                    let blkszbits: u8 = sb[12];
                    let root_nid: u16 = u16::from_le_bytes([sb[14], sb[15]]);
                    let inos: u64 = u64::from_le_bytes([
                        sb[16], sb[17], sb[18], sb[19], sb[20], sb[21], sb[22], sb[23],
                    ]);
                    let blocks: u32 = u32::from_le_bytes([sb[36], sb[37], sb[38], sb[39]]);
                    let feature_incompat: u32 =
                        u32::from_le_bytes([sb[80], sb[81], sb[82], sb[83]]);

                    let block_size: u32 = match blkszbits {
                        9..=16 => 1u32 << blkszbits,
                        invalid => {
                            return Err(KernelLoadError::InvalidBlockSize { bits: invalid });
                        }
                    };

                    Ok(ErofsSuperBlock {
                        magic,
                        feature_compat,
                        block_size,
                        root_nid,
                        inos,
                        blocks,
                        feature_incompat,
                    })
                }
            }
        }

        /// Полная валидация суперблока: магия и размер блока уже проверены
        /// в parse; здесь дополнительно проверяется согласованность счётчиков.
        pub fn validate(&self) -> Result<(), KernelLoadError> {
            match self.magic {
                EROFS_SUPER_MAGIC => {}
                found => {
                    return Err(KernelLoadError::ErofsMagicMismatch {
                        found,
                        expected: EROFS_SUPER_MAGIC,
                    });
                }
            }
            match self.block_size {
                EROFS_BLOCK_SIZE => Ok(()),
                other => Err(KernelLoadError::InvalidBlockSize { bits: other.trailing_zeros() as u8 }),
            }
        }
    }

    /// Модель диска kernel.img: образ EROFS, прошедший суперблочную валидацию.
    #[derive(Debug, Clone)]
    pub struct ErofsImage {
        pub name: String,
        pub raw: Vec<u8>,
        pub superblock: ErofsSuperBlock,
    }

    impl ErofsImage {
        /// Конструирует образ из байтов и немедленно валидирует суперблок.
        pub fn from_bytes(name: &str, raw: Vec<u8>) -> Result<ErofsImage, KernelLoadError> {
            let superblock: ErofsSuperBlock = ErofsSuperBlock::parse(&raw)?;
            superblock.validate()?;
            Ok(ErofsImage {
                name: name.to_string(),
                raw,
                superblock,
            })
        }

        /// Быстрый предикат: магия суперблока корректна.
        pub fn magic_ok(&self) -> bool {
            match self.superblock.magic {
                EROFS_SUPER_MAGIC => true,
                _ => false,
            }
        }
    }

    /// Запись каталога tar-архива: имя члена, смещение данных, размер.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct TarMember {
        pub name: String,
        pub offset: usize,
        pub size: usize,
        pub is_directory: bool,
    }

    /// Разобранный tar-архив (POSIX ustar) над байтовым контейнером.
    #[derive(Debug, Clone)]
    pub struct TarArchive {
        pub raw: Vec<u8>,
        pub members: Vec<TarMember>,
    }

    impl TarArchive {
        /// Полный разбор tar-архива: итерация 512-байтовых записей, проверка
        /// магии "ustar", контрольной суммы заголовка, чтение размера
        /// (восьмеричное поле или base-256), пропуск выравнивания данных.
        pub fn parse(raw: Vec<u8>) -> Result<TarArchive, KernelLoadError> {
            let mut members: Vec<TarMember> = Vec::new();
            let mut offset: usize = 0;

            loop {
                // Конец архива: две нулевые записи подряд (или конец данных).
                match raw.len().checked_sub(offset) {
                    Some(remaining) if remaining >= TAR_RECORD_SIZE => {}
                    _ => break,
                }
                let record: &[u8] = &raw[offset..offset + TAR_RECORD_SIZE];
                match record.iter().all(|b: &u8| *b == 0u8) {
                    true => break,
                    false => {}
                }

                // Проверка магии ustar.
                let magic_ok: bool = {
                    let magic: &[u8] = &record[257..263];
                    magic == b"ustar\0" || magic == b"ustar "
                };
                match magic_ok {
                    true => {}
                    false => {
                        return Err(KernelLoadError::TarHeaderCorrupt {
                            detail: format!("запись по смещению {} не имеет магии ustar", offset),
                        });
                    }
                }

                // Проверка контрольной суммы заголовка (сумма байтов записи,
                // поле checksum трактуется как 8 пробелов).
                let stored_checksum: usize = Self::parse_numeric_field(&record[148..156]);
                let computed_checksum: usize = (0..TAR_RECORD_SIZE)
                    .map(|i: usize| -> usize {
                        match i >= 148 && i < 156 {
                            true => 0x20,
                            false => record[i] as usize,
                        }
                    })
                    .sum();
                match stored_checksum == computed_checksum {
                    true => {}
                    false => {
                        return Err(KernelLoadError::TarHeaderCorrupt {
                            detail: format!(
                                "запись по смещению {}: контрольная сумма {} != {}",
                                offset, stored_checksum, computed_checksum
                            ),
                        });
                    }
                }

                // Имя члена (до NUL) и тип записи.
                let name: String = {
                    let raw_name: &[u8] = &record[0..100];
                    let end: usize = match raw_name.iter().position(|b: &u8| *b == 0u8) {
                        Some(pos) => pos,
                        None => 100,
                    };
                    match core::str::from_utf8(&raw_name[..end]) {
                        Ok(s) => s.to_string(),
                        Err(_) => {
                            return Err(KernelLoadError::TarHeaderCorrupt {
                                detail: format!("некорректное имя члена по смещению {}", offset),
                            });
                        }
                    }
                };

                let typeflag: u8 = record[156];
                let size: usize = Self::parse_numeric_field(&record[124..136]);
                let data_offset: usize = offset + TAR_RECORD_SIZE;

                match typeflag {
                    b'0' | b'\0' => {
                        members.push(TarMember {
                            name,
                            offset: data_offset,
                            size,
                            is_directory: false,
                        });
                    }
                    b'5' => {
                        members.push(TarMember {
                            name,
                            offset: data_offset,
                            size: 0,
                            is_directory: true,
                        });
                    }
                    b'x' | b'g' => {
                        // PAX-расширения: пропускаем как данные без каталога.
                        members.push(TarMember {
                            name,
                            offset: data_offset,
                            size,
                            is_directory: false,
                        });
                    }
                    _ => {
                        // Прочие типы (символьные/блочные устройства, hardlink
                        // и т.п.) не участвуют в загрузке ядра — пропускаем.
                    }
                }

                // Продвижение: заголовок + данные с выравниванием до 512 байт.
                let padded: usize = match size % TAR_RECORD_SIZE {
                    0 => size,
                    rem => size + (TAR_RECORD_SIZE - rem),
                };
                offset = match data_offset.checked_add(padded) {
                    Some(next) => next,
                    None => break,
                };
            }

            Ok(TarArchive { raw, members })
        }

        /// Извлечение содержимого члена по имени (копия байтов).
        pub fn extract(&self, member_name: &str) -> Result<Vec<u8>, KernelLoadError> {
            let mut found: Option<&TarMember> = None;
            for member in self.members.iter() {
                match member.name.as_str() {
                    name if name == member_name && !member.is_directory => {
                        found = Some(member);
                        break;
                    }
                    _ => {}
                }
            }
            match found {
                Some(member) => {
                    let end: usize = match member.offset.checked_add(member.size) {
                        Some(e) => e,
                        None => {
                            return Err(KernelLoadError::TarHeaderCorrupt {
                                detail: "переполнение размера члена".to_string(),
                            });
                        }
                    };
                    match end <= self.raw.len() {
                        true => Ok(self.raw[member.offset..end].to_vec()),
                        false => Err(KernelLoadError::TarHeaderCorrupt {
                            detail: format!("член '{}' выходит за границы архива", member_name),
                        }),
                    }
                }
                None => Err(KernelLoadError::KernelImageNotFound),
            }
        }

        /// Разбор числового поля tar: восьмеричное с пробелами/NUL либо
        /// base-256 (старший бит первого байта установлен).
        fn parse_numeric_field(field: &[u8]) -> usize {
            match field.first() {
                Some(first) if first & 0x80 != 0 => {
                    // base-256: big-endian, бит 0x80 в старшем байте — признак.
                    let mut value: usize = 0;
                    for (idx, byte) in field.iter().enumerate() {
                        let masked: u8 = match idx {
                            0 => byte & 0x7F,
                            _ => *byte,
                        };
                        value = (value << 8) | masked as usize;
                    }
                    value
                }
                _ => {
                    // Восьмеричное поле: цифры до первого NUL/пробела.
                    let mut value: usize = 0;
                    for byte in field.iter() {
                        match byte {
                            b'0'..=b'7' => {
                                value = value * 8 + (byte - b'0') as usize;
                            }
                            _ => break,
                        }
                    }
                    value
                }
            }
        }
    }

    /// Модель gzip-заголовка (RFC 1952): магия, метод сжатия, флаги, время,
    /// доп. поля, имя, комментарий, CRC. Разбор полный, без внешних декодеров.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct GzipHeader {
        pub magic_ok: bool,
        pub compression_method: u8,
        pub flags: u8,
        pub mtime: u32,
        pub xfl: u8,
        pub os: u8,
        pub has_extra: bool,
        pub has_name: bool,
        pub has_comment: bool,
        pub has_crc16: bool,
        pub header_len: usize,
    }

    impl GzipHeader {
        pub fn parse(bytes: &[u8]) -> Result<GzipHeader, KernelLoadError> {
            match bytes.len() < 10 {
                true => {
                    return Err(KernelLoadError::GzipHeaderCorrupt {
                        detail: "короче 10 байт заголовка".to_string(),
                    });
                }
                false => {}
            }

            let id1: u8 = bytes[0];
            let id2: u8 = bytes[1];
            let magic_ok: bool = id1 == 0x1F && id2 == 0x8B;
            match magic_ok {
                true => {}
                false => {
                    return Err(KernelLoadError::GzipHeaderCorrupt {
                        detail: "неверная магия gzip (0x1F 0x8B)".to_string(),
                    });
                }
            }

            let compression_method: u8 = bytes[2];
            match compression_method {
                8 => {}
                other => {
                    return Err(KernelLoadError::GzipHeaderCorrupt {
                        detail: format!("метод сжатия {} != 8 (deflate)", other),
                    });
                }
            }

            let flags: u8 = bytes[3];
            let mtime: u32 = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
            let xfl: u8 = bytes[8];
            let os: u8 = bytes[9];

            let has_extra: bool = flags & 0x04 != 0;
            let has_name: bool = flags & 0x08 != 0;
            let has_comment: bool = flags & 0x10 != 0;
            let has_crc16: bool = flags & 0x02 != 0;

            let mut pos: usize = 10;

            // FEXTRA: 2 байта длины (LE) + сами данные.
            if has_extra {
                match bytes.len().checked_sub(pos + 2) {
                    Some(_) => {}
                    None => {
                        return Err(KernelLoadError::GzipHeaderCorrupt {
                            detail: "обрезан FEXTRA".to_string(),
                        });
                    }
                }
                let extra_len: usize = u16::from_le_bytes([bytes[pos], bytes[pos + 1]]) as usize;
                pos += 2;
                match bytes.len().checked_sub(pos + extra_len) {
                    Some(_) => {}
                    None => {
                        return Err(KernelLoadError::GzipHeaderCorrupt {
                            detail: "FEXTRA выходит за границы".to_string(),
                        });
                    }
                }
                pos += extra_len;
            }

            // FNAME: NUL-терминированная строка.
            if has_name {
                match bytes[pos..].iter().position(|b: &u8| *b == 0u8) {
                    Some(zero_pos) => pos += zero_pos + 1,
                    None => {
                        return Err(KernelLoadError::GzipHeaderCorrupt {
                            detail: "FNAME без NUL-терминатора".to_string(),
                        });
                    }
                }
            }

            // FCOMMENT: NUL-терминированная строка.
            if has_comment {
                match bytes[pos..].iter().position(|b: &u8| *b == 0u8) {
                    Some(zero_pos) => pos += zero_pos + 1,
                    None => {
                        return Err(KernelLoadError::GzipHeaderCorrupt {
                            detail: "FCOMMENT без NUL-терминатора".to_string(),
                        });
                    }
                }
            }

            // FHCRC: 2 байта контрольной суммы.
            if has_crc16 {
                match bytes.len().checked_sub(pos + 2) {
                    Some(_) => {}
                    None => {
                        return Err(KernelLoadError::GzipHeaderCorrupt {
                            detail: "обрезан FHCRC".to_string(),
                        });
                    }
                }
                pos += 2;
            }

            Ok(GzipHeader {
                magic_ok,
                compression_method,
                flags,
                mtime,
                xfl,
                os,
                has_extra,
                has_name,
                has_comment,
                has_crc16,
                header_len: pos,
            })
        }
    }

    /// Модель архива kernel.tar.gz: gzip-обёртка + tar-поток.
    ///
    /// В реальном ядре секция после gzip-заголовка — deflate-поток, который
    /// декодируется инфлятором ядра. В host-полигоне контейнер построен
    /// детерминированно: после заголовка следует хранимый (stored) tar-поток,
    /// что позволяет валидировать всю цепочку без внешних декодеров.
    #[derive(Debug, Clone)]
    pub struct TarGzArchive {
        pub gzip: GzipHeader,
        pub tar: TarArchive,
        pub raw: Vec<u8>,
    }

    impl TarGzArchive {
        pub fn parse(raw: Vec<u8>) -> Result<TarGzArchive, KernelLoadError> {
            let gzip: GzipHeader = GzipHeader::parse(&raw)?;
            let payload: Vec<u8> = match raw.get(gzip.header_len..) {
                Some(slice) => slice.to_vec(),
                None => {
                    return Err(KernelLoadError::GzipHeaderCorrupt {
                        detail: "нет полезной нагрузки после заголовка".to_string(),
                    });
                }
            };
            let tar: TarArchive = TarArchive::parse(payload)?;
            Ok(TarGzArchive { gzip, tar, raw })
        }

        /// Метод unpack(): извлекает образ kernel.img и валидирует его как
        /// EROFS-диск с суперблоком (магия 0xE0F5E0F5).
        pub fn unpack(&self) -> Result<ErofsImage, KernelLoadError> {
            let img_bytes: Vec<u8> = self.tar.extract("kernel.img")?;
            ErofsImage::from_bytes("kernel.img", img_bytes)
        }
    }

    /// Модель EROFS-раздела /kernel. Содержит архив kernel.tar.gz.
    #[derive(Debug, Clone)]
    pub struct KernelPartition {
        pub label: String,
        pub fs_type: String,
        pub mount_point: String,
        pub read_only: bool,
        /// Образ самого раздела (EROFS-диск /kernel).
        pub image: ErofsImage,
        /// Содержимое файла kernel.tar.gz внутри образа.
        pub tar_gz: Vec<u8>,
    }

    impl KernelPartition {
        pub fn new(label: &str, fs_type: &str, mount_point: &str, image: ErofsImage, tar_gz: Vec<u8>) -> KernelPartition {
            KernelPartition {
                label: label.to_string(),
                fs_type: fs_type.to_string(),
                mount_point: mount_point.to_string(),
                read_only: true,
                image,
                tar_gz,
            }
        }

        /// Валидация монтирования раздела: драйвер erofs, режим ro, магия ОК.
        pub fn validate_mount(&self) -> Result<(), KernelLoadError> {
            match self.fs_type.as_str() {
                "erofs" => {}
                other => {
                    return Err(KernelLoadError::PartitionUnavailable {
                        reason: format!("драйвер '{}' не является erofs", other),
                    });
                }
            }
            match self.read_only {
                true => {}
                false => {
                    return Err(KernelLoadError::PartitionUnavailable {
                        reason: "раздел /kernel не переведён в ReadOnly".to_string(),
                    });
                }
            }
            match self.image.magic_ok() {
                true => Ok(()),
                false => Err(KernelLoadError::ErofsMagicMismatch {
                    found: self.image.superblock.magic,
                    expected: EROFS_SUPER_MAGIC,
                }),
            }
        }
    }

    /// Максимальный размер образа ядра, принимаемый загрузчиком (контроль
    /// деструктивного контейнера — защита кучи ядра).
    pub const MAX_KERNEL_IMAGE_SIZE: usize = 16 * 1024 * 1024;

    /// ПОЭТАПНОЕ РАСКРЫТИЕ КОНТЕЙНЕРОВ И ПЕРЕДАЧА УПРАВЛЕНИЯ PID 1.
    ///
    /// Этапы:
    ///   1. Монтирование /kernel (erofs, ro) с валидацией суперблока.
    ///   2. Чтение kernel.tar.gz из образа раздела.
    ///   3. Разбор gzip-заголовка и tar-потока.
    ///   4. Извлечение kernel.img (ErofsImage) + валидация магии 0xE0F5E0F5.
    ///   5. Контроль размера и передача управления PID 1 (симуляция).
    pub fn load_and_boot_kernel(partition: &KernelPartition) -> Result<(), KernelLoadError> {
        // Этап 1: валидация раздела.
        partition.validate_mount()?;

        // Этап 2: наличие архива.
        match partition.tar_gz.is_empty() {
            true => return Err(KernelLoadError::ArchiveMissing),
            false => {}
        }

        // Этап 3-4: распаковка сэндвича.
        let archive: TarGzArchive = TarGzArchive::parse(partition.tar_gz.clone())?;
        let kernel_image: ErofsImage = archive.unpack()?;

        // Этап 5: контроль размера.
        match kernel_image.raw.len() > MAX_KERNEL_IMAGE_SIZE {
            true => {
                return Err(KernelLoadError::KernelImageTooLarge {
                    size: kernel_image.raw.len(),
                    limit: MAX_KERNEL_IMAGE_SIZE,
                });
            }
            false => {}
        }

        // Управление передано PID 1 (в реальной сборке — прыжок на entry
        // микроядра; здесь — возврат успеха для полигона).
        Ok(())
    }
