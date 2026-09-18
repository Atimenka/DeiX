// ❗ЗАВИСИМОТИ: инит скрипт pid 1 который будет ограничивать пользовательский
// Модульная сборка MASTER SPEC DeiX OS: каждый модуль — отдельный файл исходника.
// PID 1 изолирует Ring 3: /kernel /init_boot /boot /vendor_boot /super /system
// /recovery (erofs/ro), /userdata (ext4/rw). Arch PKG -> .pkg.erofs внутри /userdata.
// TWRP/OrangeFox + Fastbootd/EDL. Zero External Dependencies, совместимо с no_std.

    use crate::kernel_loader::{ErofsImage, ErofsSuperBlock, KernelLoadError, TarArchive};

    /// Магическая сигнатура zstd-фрейма (RFC 8878): байты 28 B5 2F FD (LE).
    pub const ZSTD_MAGIC: u32 = 0xFD2FB528;

    /// Строго типизированное перечисление ошибок пакетного моста.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum PkgError {
        MissingField { field: String },
        InvalidField { field: String, detail: String },
        ZstdFrameInvalid { detail: String },
        TarInvalid { detail: String },
        PkgInfoMismatch { detail: String },
        MountTargetDenied { target: String },
        ErofsBuildFailed { detail: String },
    }

    impl PkgError {
        pub fn message(&self) -> String {
            match self {
                PkgError::MissingField { field } => {
                    format!("отсутствует обязательное поле '{}'", field)
                }
                PkgError::InvalidField { field, detail } => {
                    format!("некорректное поле '{}': {}", field, detail)
                }
                PkgError::ZstdFrameInvalid { detail } => {
                    format!("некорректный zstd-фрейм: {}", detail)
                }
                PkgError::TarInvalid { detail } => {
                    format!("некорректный tar-поток: {}", detail)
                }
                PkgError::PkgInfoMismatch { detail } => {
                    format!("рассинхронизация метаданных: {}", detail)
                }
                PkgError::MountTargetDenied { target } => {
                    format!("точка монтирования '{}' вне /userdata — отказано", target)
                }
                PkgError::ErofsBuildFailed { detail } => {
                    format!("не удалось собрать EROFS-модуль: {}", detail)
                }
            }
        }
    }

    /// Модель метаданных Arch-пакета: PKGBUILD / .BUILDINFO / .PKGINFO.
    ///
    /// Поля:
    /// * `pkgname` — имя пакета.
    /// * `pkgver`  — версия-релиз (например, "1.2.3-1").
    /// * `arch`    — целевая архитектура (для DeiX: "x86_64").
    /// * `depends` — список зависимостей.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct ArchPkgMetadata {
        pub pkgname: String,
        pub pkgver: String,
        pub arch: String,
        pub depends: Vec<String>,
    }

    impl ArchPkgMetadata {
        /// Парсинг PKGBUILD-стиля: строки `pkgname=foo`, `pkgver=1.2.3-1`,
        /// `arch=('x86_64')`, `depends=('a' 'b')`. Кавычки и скобки
        /// обрабатываются; значения вне кавычек тоже принимаются.
        pub fn parse_pkgbuild(text: &str) -> Result<ArchPkgMetadata, PkgError> {
            let mut pkgname: Option<String> = None;
            let mut pkgver: Option<String> = None;
            let mut arch: Option<String> = None;
            let mut depends: Vec<String> = Vec::new();

            for raw_line in text.lines() {
                let line: &str = raw_line.trim();
                if line.is_empty() {
                    continue;
                }
                match line.split_once('=') {
                    Some((key, value)) => {
                        let key: &str = key.trim();
                        let value: &str = value.trim();
                        match key {
                            "pkgname" => pkgname = Some(Self::strip_quotes(value)),
                            "pkgver" => pkgver = Some(Self::strip_quotes(value)),
                            "arch" => {
                                arch = Some(Self::first_list_value(value).unwrap_or_else(|| {
                                    Self::strip_quotes(value)
                                }));
                            }
                            "depends" => {
                                let items: Vec<String> = Self::list_values(value);
                                match items.is_empty() {
                                    true => {}
                                    false => depends = items,
                                }
                            }
                            _ => {}
                        }
                    }
                    None => {}
                }
            }

            match pkgname.as_ref() {
                Some(name) if !name.is_empty() => {}
                _ => return Err(PkgError::MissingField { field: "pkgname".to_string() }),
            }
            match pkgver.as_ref() {
                Some(ver) if !ver.is_empty() => {}
                _ => return Err(PkgError::MissingField { field: "pkgver".to_string() }),
            }
            match arch.as_ref() {
                Some(a) if !a.is_empty() => {}
                _ => return Err(PkgError::MissingField { field: "arch".to_string() }),
            }

            Ok(ArchPkgMetadata {
                pkgname: pkgname.unwrap_or_default(),
                pkgver: pkgver.unwrap_or_default(),
                arch: arch.unwrap_or_default(),
                depends,
            })
        }

        /// Парсинг .BUILDINFO / .PKGINFO (формат "ключ = значение").
        pub fn parse_buildinfo(text: &str) -> Result<ArchPkgMetadata, PkgError> {
            let mut pkgname: Option<String> = None;
            let mut pkgver: Option<String> = None;
            let mut arch: Option<String> = None;
            let mut depends: Vec<String> = Vec::new();

            for raw_line in text.lines() {
                let line: &str = raw_line.trim();
                if line.is_empty() {
                    continue;
                }
                match line.split_once('=') {
                    Some((key, value)) => {
                        match key.trim() {
                            "pkgname" => pkgname = Some(Self::strip_quotes(value.trim())),
                            "pkgver" => pkgver = Some(Self::strip_quotes(value.trim())),
                            "arch" => arch = Some(Self::strip_quotes(value.trim())),
                            "depend" => depends.push(Self::strip_quotes(value.trim())),
                            _ => {}
                        }
                    }
                    None => {}
                }
            }

            match pkgname.as_ref() {
                Some(name) if !name.is_empty() => {}
                _ => return Err(PkgError::MissingField { field: "pkgname".to_string() }),
            }
            match pkgver.as_ref() {
                Some(ver) if !ver.is_empty() => {}
                _ => return Err(PkgError::MissingField { field: "pkgver".to_string() }),
            }

            Ok(ArchPkgMetadata {
                pkgname: pkgname.unwrap_or_default(),
                pkgver: pkgver.unwrap_or_default(),
                arch: arch.unwrap_or_else(|| "x86_64".to_string()),
                depends,
            })
        }

        /// Снятие одинарных/двойных кавычек с обеих сторон строки.
        fn strip_quotes(value: &str) -> String {
            let v: &str = value.trim();
            match (v.starts_with('\''), v.ends_with('\'')) {
                (true, true) => v[1..v.len().saturating_sub(1)].to_string(),
                _ => match (v.starts_with('"'), v.ends_with('"')) {
                    (true, true) => v[1..v.len().saturating_sub(1)].to_string(),
                    _ => v.to_string(),
                },
            }
        }

        /// Извлечение первого строкового литерала из списка вида
        /// ('x86_64' 'arm64') или ("a" "b").
        fn first_list_value(value: &str) -> Option<String> {
            let values: Vec<String> = Self::list_values(value);
            match values.is_empty() {
                true => None,
                false => Some(values[0].clone()),
            }
        }

        /// Извлечение всех строковых литералов из списка bash-стиля.
        fn list_values(value: &str) -> Vec<String> {
            let mut result: Vec<String> = Vec::new();
            let mut current: String = String::new();
            let mut in_quote: Option<char> = None;
            for ch in value.chars() {
                match in_quote {
                    Some(q) => {
                        match ch == q {
                            true => {
                                result.push(current.clone());
                                current.clear();
                                in_quote = None;
                            }
                            false => current.push(ch),
                        }
                    }
                    None => {
                        match ch {
                            '\'' | '"' => in_quote = Some(ch),
                            _ => {}
                        }
                    }
                }
            }
            match in_quote {
                Some(_) => {
                    // Незакрытая кавычка: используем то, что накопили.
                    match current.is_empty() {
                        true => {}
                        false => result.push(current),
                    }
                }
                None => {}
            }
            result
        }
    }

    /// Модель zstd-фрейма (RFC 8878). Реальный структурный разбор: магия,
    /// дескриптор заголовка фрейма, флаг single-segment, размер содержимого.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct ZstdFrameHeader {
        pub magic: u32,
        pub single_segment: bool,
        pub content_size: Option<u64>,
        pub header_len: usize,
    }

    impl ZstdFrameHeader {
        pub fn parse(bytes: &[u8]) -> Result<ZstdFrameHeader, PkgError> {
            match bytes.len() < 4 {
                true => {
                    return Err(PkgError::ZstdFrameInvalid {
                        detail: "короче 4 байт магии".to_string(),
                    });
                }
                false => {}
            }
            let magic: u32 = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            match magic {
                ZSTD_MAGIC => {}
                other => {
                    return Err(PkgError::ZstdFrameInvalid {
                        detail: format!("магия {:#010x} != {:#010x}", other, ZSTD_MAGIC),
                    });
                }
            }

            match bytes.get(4) {
                Some(descriptor) => {
                    let frame_content_size_flag: u8 = (descriptor >> 6) & 0x03;
                    let single_segment: bool = (descriptor >> 5) & 0x01 != 0;
                    let mut pos: usize = 5;

                    let content_size: Option<u64> = match single_segment {
                        true => {
                            // Для single-segment поле размера: 1/2/4/8 байт
                            // в зависимости от флага.
                            let field_len: usize = match frame_content_size_flag {
                                0 => 1,
                                1 => 2,
                                2 => 4,
                                3 => 8,
                                _ => 1,
                            };
                            match bytes.len().checked_sub(pos + field_len) {
                                Some(_) => {}
                                None => {
                                    return Err(PkgError::ZstdFrameInvalid {
                                        detail: "обрезано поле размера содержимого".to_string(),
                                    });
                                }
                            }
                            let mut value: u64 = 0;
                            for i in 0..field_len {
                                value |= (bytes[pos + i] as u64) << (8 * i);
                            }
                            pos += field_len;
                            Some(value)
                        }
                        false => {
                            // Без single_segment: окно + опциональный размер.
                            match frame_content_size_flag {
                                0 => None,
                                1 => {
                                    match bytes.get(pos) {
                                        Some(_) => {
                                            pos += 1;
                                            Some(1)
                                        }
                                        None => None,
                                    }
                                }
                                2 => {
                                    match bytes.len().checked_sub(pos + 2) {
                                        Some(_) => {
                                            let v: u16 =
                                                u16::from_le_bytes([bytes[pos], bytes[pos + 1]]);
                                            pos += 2;
                                            Some(v as u64)
                                        }
                                        None => None,
                                    }
                                }
                                3 => {
                                    match bytes.len().checked_sub(pos + 4) {
                                        Some(_) => {
                                            let v: u32 = u32::from_le_bytes([
                                                bytes[pos],
                                                bytes[pos + 1],
                                                bytes[pos + 2],
                                                bytes[pos + 3],
                                            ]);
                                            pos += 4;
                                            Some(v as u64)
                                        }
                                        None => None,
                                    }
                                }
                                _ => None,
                            }
                        }
                    };

                    Ok(ZstdFrameHeader {
                        magic,
                        single_segment,
                        content_size,
                        header_len: pos,
                    })
                }
                None => Err(PkgError::ZstdFrameInvalid {
                    detail: "нет дескриптора заголовка фрейма".to_string(),
                }),
            }
        }
    }

    /// Модель контейнера .pkg.tar.zst: zstd-фрейм + tar-поток.
    #[derive(Debug, Clone)]
    pub struct PkgTarZstContainer {
        pub frame: ZstdFrameHeader,
        pub tar: TarArchive,
        pub raw: Vec<u8>,
    }

    impl PkgTarZstContainer {
        pub fn parse(raw: Vec<u8>) -> Result<PkgTarZstContainer, PkgError> {
            let frame: ZstdFrameHeader = ZstdFrameHeader::parse(&raw)?;
            let payload: Vec<u8> = match raw.get(frame.header_len..) {
                Some(slice) => slice.to_vec(),
                None => {
                    return Err(PkgError::ZstdFrameInvalid {
                        detail: "нет полезной нагрузки после заголовка фрейма".to_string(),
                    });
                }
            };
            let tar: TarArchive = match TarArchive::parse(payload) {
                Ok(t) => t,
                Err(KernelLoadError::TarHeaderCorrupt { detail }) => {
                    return Err(PkgError::TarInvalid { detail });
                }
                Err(other) => {
                    return Err(PkgError::TarInvalid {
                        detail: other.message(),
                    });
                }
            };
            Ok(PkgTarZstContainer { frame, tar, raw })
        }
    }

    /// EROFS-модуль пакета (.pkg.erofs): изолированный образ, монтируемый
    /// в пользовательском пространстве внутри /userdata.
    #[derive(Debug, Clone)]
    pub struct ErofsPackageContainer {
        pub pkg_name: String,
        pub image: ErofsImage,
        pub mount_point: String,
        pub metadata: ArchPkgMetadata,
    }

    impl ErofsPackageContainer {
        /// Конвертация .pkg.tar.zst в EROFS-модуль:
        /// 1. Разбор zstd-фрейма и tar-потока.
        /// 2. Извлечение и валидация .PKGINFO.
        /// 3. Согласование метаданных с переданным PKGBUILD-профилем.
        /// 4. Формирование EROFS-образа .pkg.erofs (магия 0xE0F5E0F5).
        pub fn from_pkg_tar_zst(
            raw: Vec<u8>,
            declared: &ArchPkgMetadata,
            userdata_root: &str,
        ) -> Result<ErofsPackageContainer, PkgError> {
            let container: PkgTarZstContainer = PkgTarZstContainer::parse(raw)?;

            // Извлечение .PKGINFO из tar-потока.
            let pkginfo_bytes: Vec<u8> = match container.tar.extract(".PKGINFO") {
                Ok(bytes) => bytes,
                Err(_) => {
                    return Err(PkgError::InvalidField {
                        field: ".PKGINFO".to_string(),
                        detail: "член .PKGINFO не найден в архиве".to_string(),
                    });
                }
            };
            let pkginfo_text: String = match core::str::from_utf8(&pkginfo_bytes) {
                Ok(text) => text.to_string(),
                Err(_) => {
                    return Err(PkgError::InvalidField {
                        field: ".PKGINFO".to_string(),
                        detail: "некорректная UTF-8 кодировка".to_string(),
                    });
                }
            };
            let pkginfo: ArchPkgMetadata = ArchPkgMetadata::parse_buildinfo(&pkginfo_text)?;

            // Согласование метаданных (защита от подмены пакета).
            match pkginfo.pkgname == declared.pkgname {
                true => {}
                false => {
                    return Err(PkgError::PkgInfoMismatch {
                        detail: format!(
                            "pkgname {} != заявленный {}",
                            pkginfo.pkgname, declared.pkgname
                        ),
                    });
                }
            }
            match pkginfo.pkgver == declared.pkgver {
                true => {}
                false => {
                    return Err(PkgError::PkgInfoMismatch {
                        detail: format!("pkgver {} != заявленный {}", pkginfo.pkgver, declared.pkgver),
                    });
                }
            }
            match declared.arch.as_str() {
                "x86_64" => {}
                other => {
                    return Err(PkgError::InvalidField {
                        field: "arch".to_string(),
                        detail: format!("архитектура '{}' не поддерживается (ожидается x86_64)", other),
                    });
                }
            }

            // Формирование EROFS-образа .pkg.erofs: синтетический контейнер
            // с валидным суперблоком (магия 0xE0F5E0F5).
            let image_raw: Vec<u8> = build_package_erofs_image(&declared.pkgname);
            let image: ErofsImage = match ErofsImage::from_bytes("pkg.erofs", image_raw) {
                Ok(img) => img,
                Err(err) => {
                    return Err(PkgError::ErofsBuildFailed {
                        detail: err.message(),
                    });
                }
            };

            let mount_point: String = format!(
                "{}/pkg/{}/",
                userdata_root.trim_end_matches('/'),
                declared.pkgname
            );

            Ok(ErofsPackageContainer {
                pkg_name: declared.pkgname.clone(),
                image,
                mount_point,
                metadata: declared.clone(),
            })
        }

        /// Валидация: образ модуля корректен (магия EROFS).
        pub fn validate(&self) -> bool {
            match self.image.magic_ok() {
                true => true,
                false => false,
            }
        }
    }

    /// МОНТИРОВАНИЕ ARCH-ПАКЕТА В ИЗОЛИРОВАННУЮ ТОЧКУ VFS.
    ///
    /// Гарантия неизменяемости: точка монтирования ОБЯЗАНА находиться
    /// внутри /userdata. Любая попытка смонтировать пакет в системное
    /// пространство (/system, /kernel, /boot, ...) отклоняется ошибкой
    /// MountTargetDenied — системные EROFS-разделы остаются нетронутыми.
    pub fn mount_arch_package(
        container: &ErofsPackageContainer,
        userdata_root: &str,
    ) -> Result<String, PkgError> {
        let root: &str = userdata_root.trim_end_matches('/');
        let target_ok: bool = {
            let base: &str = container.mount_point.as_str();
            match base.strip_prefix(root) {
                Some(suffix) => suffix.starts_with("/pkg/"),
                None => false,
            }
        };
        match target_ok {
            true => {}
            false => {
                return Err(PkgError::MountTargetDenied {
                    target: container.mount_point.clone(),
                });
            }
        }
        match container.validate() {
            true => Ok(container.mount_point.clone()),
            false => Err(PkgError::ErofsBuildFailed {
                detail: "образ .pkg.erofs не прошёл валидацию".to_string(),
            }),
        }
    }

    /// Сборка синтетического EROFS-образа пакета: 4096 байт, суперблок на
    /// смещении 1024 с магией 0xE0F5E0F5 и корректными полями раскладки.
    fn build_package_erofs_image(pkg_name: &str) -> Vec<u8> {
        let mut image: Vec<u8> = vec![0u8; 4096];
        let sb_off: usize = crate::kernel_loader::EROFS_SUPERBLOCK_OFFSET;
        // Магия 0xE0F5E0F5 (little-endian).
        image[sb_off..sb_off + 4].copy_from_slice(&crate::kernel_loader::EROFS_SUPER_MAGIC.to_le_bytes());
        // feature_compat @8 = 0.
        image[sb_off + 8..sb_off + 12].copy_from_slice(&0u32.to_le_bytes());
        // blkszbits @12 = 12 (блок 4096).
        image[sb_off + 12] = 12;
        // sb_extslots @13 = 0.
        image[sb_off + 13] = 0;
        // root_nid @14 (u16) = 0.
        // inos @16 (u64) = 2 (корень + каталог пакета).
        image[sb_off + 16..sb_off + 24].copy_from_slice(&2u64.to_le_bytes());
        // blocks @36 (u32) = 1.
        image[sb_off + 36..sb_off + 40].copy_from_slice(&1u32.to_le_bytes());
        // feature_incompat @80 (u32) = 0.
        image[sb_off + 80..sb_off + 84].copy_from_slice(&0u32.to_le_bytes());
        // volume_name @64 (16 байт): имя пакета (обрезанное).
        let vn: &[u8] = pkg_name.as_bytes();
        let vn_len: usize = vn.len().min(16);
        image[sb_off + 64..sb_off + 64 + vn_len].copy_from_slice(&vn[..vn_len]);
        image
    }

    /// Публичная обёртка: пере-экспорт суперблока для диагностики.
    pub fn superblock_of(container: &ErofsPackageContainer) -> ErofsSuperBlock {
        container.image.superblock
    }
