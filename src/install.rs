//! Команда `install` — устанавливает DeiX на второй физический диск
//! (ATA Primary Slave), клонируя весь загрузочный образ (MBR + ядро +
//! ext2-раздел) посекторно с диска, с которого сама система сейчас
//! загружена (ATA Primary Master).
//!
//! Честно о том, что это такое: это НЕ "умный" установщик с разметкой
//! разделов, выбором файловой системы, графическим мастером и т.д. — это
//! прямой аналог posекторного клонирования диска (как `dd` в Unix).
//! Работает потому, что DeiX и так уже размещает всё как один
//! самодостаточный образ (MBR boot-сектор + ядро + ext2-том с данными
//! пользователя) — скопировав его байт-в-байт на другой диск, мы получаем
//! второй диск, с которого DeiX точно так же загрузится и будет работать
//! абсолютно независимо от исходного (Live) диска.
//!
//! Именно так реализуются вопросы вроде "поставь ОС на HDD, а не только
//! Live с флешки" для мини-ОС без настоящей файловой системы разделов
//! (GPT/MBR partition table в классическом смысле, LVM и т.д.) — считать
//! это "как в Linux/Windows" в смысле результата (после этого второй диск
//! самостоятельно грузится), но не в смысле процесса (там полноценный
//! мастер с разметкой, здесь — прямое клонирование).

use crate::ata::{self, Drive};
use crate::{print, println, println_t, t};

/// Сколько всего секторов копируем — должно совпадать с тем, чем
/// реально заполнен образ диска (см. build.sh: MIN_SECTORS=12800,
/// покрывает boot+kernel+весь ext2-том). Проверяем перед копированием,
/// что целевой диск не меньше этого объёма (см. cmd_install).
const TOTAL_SECTORS_TO_COPY: u32 = 12800;

const CHUNK_SECTORS: u8 = 64; // 32 КиБ за один проход — разумный компромисс

pub fn cmd_install() {
    println!(
        "{}",
        t!(
            en: "DeiX Installer",
            ru: "Установщик DeiX"
        )
    );
    println!(
        "{}",
        t!(
            en: "This copies the entire boot disk (bootloader + kernel + ext2 data) \
                 sector-by-sector onto the SECOND physical disk (ATA Primary Slave — \
                 the second '-drive' in QEMU, or the second HDD on a real machine). \
                 This is a direct disk clone (like 'dd'), not a partitioning wizard — \
                 but the result is a fully independent, self-bootable disk.",
            ru: "Эта команда копирует весь загрузочный диск (загрузчик + ядро + \
                 данные ext2) посекторно на ВТОРОЙ физический диск (ATA Primary \
                 Slave — второй '-drive' в QEMU, или второй HDD на реальной машине). \
                 Это прямое клонирование диска (как 'dd'), а не мастер разметки — \
                 но результат: полностью независимый, самостоятельно загружаемый диск."
        )
    );
    println!();

    if !ata::is_slave_present() {
        println!(
            "{}",
            t!(
                en: "ERROR: no second disk detected (ATA Primary Slave). \
                     In QEMU, add a second disk, e.g.:\n  \
                     qemu-system-x86_64 -drive format=raw,file=deix_disk.img \\\n    \
                     -drive format=raw,file=target_hdd.img -m 512M -net nic,model=rtl8139 -net user\n\
                     (create target_hdd.img first, e.g. with \
                     'qemu-img create -f raw target_hdd.img 8M')",
                ru: "ОШИБКА: второй диск не найден (ATA Primary Slave). \
                     В QEMU добавь второй диск, например:\n  \
                     qemu-system-x86_64 -drive format=raw,file=deix_disk.img \\\n    \
                     -drive format=raw,file=target_hdd.img -m 512M -net nic,model=rtl8139 -net user\n\
                     (сначала создай target_hdd.img, например: \
                     'qemu-img create -f raw target_hdd.img 8M')"
            )
        );
        return;
    }

    let slave_sectors = match ata::slave_sector_count() {
        Some(s) => s,
        None => {
            println!(
                "{}",
                t!(
                    en: "ERROR: could not read target disk size (IDENTIFY failed).",
                    ru: "ОШИБКА: не удалось прочитать размер целевого диска (IDENTIFY не удался)."
                )
            );
            return;
        }
    };

    if slave_sectors < TOTAL_SECTORS_TO_COPY {
        println_t!(
            en: "ERROR: target disk is too small ({} sectors, need at least {}). \
                 Create a bigger image, e.g. 'qemu-img create -f raw target_hdd.img 8M'.",
            ru: "ОШИБКА: целевой диск слишком мал ({} секторов, нужно минимум {}). \
                 Создай образ побольше, например: 'qemu-img create -f raw target_hdd.img 8M'.";
            slave_sectors, TOTAL_SECTORS_TO_COPY
        );
        return;
    }

    println_t!(
        en: "Target disk OK ({} sectors available). Copying {} sectors ({} KiB)...",
        ru: "Целевой диск подходит ({} секторов доступно). Копируем {} секторов ({} КиБ)...";
        slave_sectors, TOTAL_SECTORS_TO_COPY, TOTAL_SECTORS_TO_COPY * 512 / 1024
    );

    let mut buf = [0u8; CHUNK_SECTORS as usize * 512];
    let mut lba: u32 = 0;
    let mut last_percent: u32 = u32::MAX;

    while lba < TOTAL_SECTORS_TO_COPY {
        let remaining = TOTAL_SECTORS_TO_COPY - lba;
        let this_chunk = remaining.min(CHUNK_SECTORS as u32) as u8;
        let chunk_bytes = this_chunk as usize * 512;

        if ata::read_sectors_from(Drive::Master, lba, this_chunk, &mut buf[..chunk_bytes]).is_err() {
            println_t!(
                en: "ERROR: failed to read sector {} from source disk. Installation aborted.",
                ru: "ОШИБКА: не удалось прочитать сектор {} с исходного диска. Установка прервана.";
                lba
            );
            return;
        }

        if ata::write_sectors_to(Drive::Slave, lba, this_chunk, &buf[..chunk_bytes]).is_err() {
            println_t!(
                en: "ERROR: failed to write sector {} to target disk. Installation aborted.",
                ru: "ОШИБКА: не удалось записать сектор {} на целевой диск. Установка прервана.";
                lba
            );
            return;
        }

        lba += this_chunk as u32;

        let percent = lba * 100 / TOTAL_SECTORS_TO_COPY;
        if percent != last_percent {
            print!("\r{}: {}%   ", t!(en: "Progress", ru: "Прогресс"), percent);
            last_percent = percent;
        }
    }

    println!();
    println!();
    println!(
        "{}",
        t!(
            en: "Installation complete! The second disk is now an independent, \
                 self-bootable DeiX installation.",
            ru: "Установка завершена! Второй диск теперь независимая, самостоятельно \
                 загружаемая установка DeiX."
        )
    );
    println!(
        "{}",
        t!(
            en: "To boot from it, make it the primary drive next time, e.g.:\n  \
                 qemu-system-x86_64 -drive format=raw,file=target_hdd.img -m 512M \\\n    \
                 -net nic,model=rtl8139 -net user",
            ru: "Чтобы загрузиться с него, в следующий раз сделай его основным диском, например:\n  \
                 qemu-system-x86_64 -drive format=raw,file=target_hdd.img -m 512M \\\n    \
                 -net nic,model=rtl8139 -net user"
        )
    );
}
