// ❗ЗАВИСИМОТИ: инит скрипт pid 1 который будет ограничивать пользовательский
// ЯДЕРНЫЙ МОДУЛЬ DeiX OS (src/lib.rs, Ring 0). Интеграция в существующий код
// kernel_loader — сэндвич ядра /kernel -> kernel.tar.gz -> kernel.img
// (tar-ustar; gzip RFC 1952). Разбор EROFS — в src/erofs.rs.
// no_std-совместимо (ядро DeiX OS): только core/alloc (BTreeMap, String, Vec),
// вывод — через crate::println!/crate::print! (стиль dxinit.rs).


use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;



/// Смещение суперблока EROFS в образе диска (байт 1024, как в Linux).
pub const EROFS_SUPERBLOCK_OFFSET: usize = 1024;



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









/// ПОСТРОИТЕЛИ БАЙТОВЫХ КОНТЕЙНЕРОВ (для pacman/duil/ds).
/// no_std-совместимо: чистые функции над Vec<u8>.

/// Запись восьмеричного поля tar-заголовка.
fn write_octal_field(header: &mut [u8; 512], off: usize, len: usize, value: usize) {
    let digits: usize = len - 1;
    let s: String = format!("{:0width$o}", value, width = digits);
    let bytes: &[u8] = s.as_bytes();
    let n: usize = bytes.len().min(len - 1);
    header[off..off + n].copy_from_slice(&bytes[..n]);
    header[off + len - 1] = 0;
}

/// Сборка POSIX ustar-архива из членов (имя, данные) с корректными
/// контрольными суммами. Возвращает байты архива (включая два нулевых
/// блока в конце).
pub fn build_tar_archive(members: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    for (name, data) in members.iter() {
        let mut header: [u8; 512] = [0u8; 512];
        let nb: &[u8] = name.as_bytes();
        let name_len: usize = nb.len().min(100);
        header[..name_len].copy_from_slice(&nb[..name_len]);
        write_octal_field(&mut header, 100, 8, 0o100644);
        write_octal_field(&mut header, 108, 8, 0);
        write_octal_field(&mut header, 116, 8, 0);
        write_octal_field(&mut header, 124, 12, data.len());
        write_octal_field(&mut header, 136, 12, 0);
        header[156] = b'0';
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        let sum: u32 = header
            .iter()
            .enumerate()
            .map(|(i, b): (usize, &u8)| -> u32 {
                match i >= 148 && i < 156 {
                    true => 0x20u32,
                    false => *b as u32,
                }
            })
            .sum();
        let cksum: String = format!("{:06o}\0 ", sum & 0o777777);
        header[148..156].copy_from_slice(cksum.as_bytes());
        out.extend_from_slice(&header);
        out.extend_from_slice(data);
        let pad: usize = match data.len() % 512 {
            0 => 0,
            rem => 512 - rem,
        };
        out.extend(core::iter::repeat(0u8).take(pad));
    }
    out.extend_from_slice(&[0u8; 1024]);
    out
}

