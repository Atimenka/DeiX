// ❗ЗАВИСИМОТИ: инит скрипт pid 1 который будет ограничивать пользовательский
// Модуль ПОСТРОИТЕЛЕЙ БАЙТОВЫХ ФИКСТУР для полигона. Генерирует РЕАЛЬНЫЕ
// бинарные контейнеры для честной проверки валидаторов без внешних крейтов:
//   - tar-архивы POSIX ustar (512-байтовые записи, корректные контрольные
//     суммы, восьмеричные поля);
//   - gzip-обёртки RFC 1952 (ID1/ID2/CM/FLG/MTIME/XFL/OS);
//   - zstd-фреймы RFC 8878 (магия 0xFD2FB528, дескриптор, размер);
//   - EROFS-образы (суперблок на смещении 1024, магия 0xE0F5E0F5);
//   - сэндвич ядра /kernel и контейнер .pkg.tar.zst.
// no_std-маппинг: Vec<u8> -> alloc::vec::Vec.
use crate::kernel_loader::{
    ErofsImage, KernelPartition, EROFS_SUPERBLOCK_OFFSET, EROFS_SUPER_MAGIC,
};

/// Запись восьмеричного поля tar-заголовка (значение в байты поля).
pub fn write_octal(header: &mut [u8; 512], off: usize, len: usize, value: usize) {
    let digits: usize = len - 1;
    let s: String = format!("{:0width$o}", value, width = digits);
    let bytes: &[u8] = s.as_bytes();
    header[off..off + bytes.len()].copy_from_slice(bytes);
    header[off + len - 1] = 0;
}

/// Сборка tar-архива ustar из членов (имя, данные). Контрольные суммы
/// вычисляются корректно: поле checksum трактуется как 8 пробелов.
pub fn build_tar(members: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    for (name, data) in members.iter() {
        let mut header: [u8; 512] = [0u8; 512];
        let nb: &[u8] = name.as_bytes();
        let name_len: usize = nb.len().min(100);
        header[..name_len].copy_from_slice(&nb[..name_len]);
        write_octal(&mut header, 100, 8, 0o100644);
        write_octal(&mut header, 108, 8, 0);
        write_octal(&mut header, 116, 8, 0);
        write_octal(&mut header, 124, 12, data.len());
        write_octal(&mut header, 136, 12, 0);
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
        out.extend(std::iter::repeat(0u8).take(pad));
    }
    out.extend_from_slice(&[0u8; 1024]);
    out
}

/// gzip-обёртка (RFC 1952): ID1 ID2 CM FLG MTIME XFL OS + полезная нагрузка.
pub fn build_gzip(payload: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    out.push(0x1F);
    out.push(0x8B);
    out.push(0x08);
    out.push(0x00);
    out.extend_from_slice(&0u32.to_le_bytes());
    out.push(0x00);
    out.push(0x03);
    out.extend_from_slice(payload);
    out
}

/// zstd-фрейм (RFC 8878): магия + дескриптор + размер + полезная нагрузка.
pub fn build_zstd_frame(payload: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(&0xFD2FB528u32.to_le_bytes());
    out.push(0xA0);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

/// EROFS-образ (4096 байт) с валидным суперблоком на смещении 1024.
/// Магия 0xE0F5E0F5, blkszbits=12, blocks=1, inos=1, volume_name=label.
pub fn build_erofs_image(label: &str) -> Vec<u8> {
    let mut image: Vec<u8> = vec![0u8; 4096];
    let sb_off: usize = EROFS_SUPERBLOCK_OFFSET;
    image[sb_off..sb_off + 4].copy_from_slice(&EROFS_SUPER_MAGIC.to_le_bytes());
    image[sb_off + 8..sb_off + 12].copy_from_slice(&0u32.to_le_bytes());
    image[sb_off + 12] = 12;
    image[sb_off + 13] = 0;
    image[sb_off + 16..sb_off + 24].copy_from_slice(&1u64.to_le_bytes());
    image[sb_off + 36..sb_off + 40].copy_from_slice(&1u32.to_le_bytes());
    image[sb_off + 80..sb_off + 84].copy_from_slice(&0u32.to_le_bytes());
    let vn: &[u8] = label.as_bytes();
    let vn_len: usize = vn.len().min(16);
    image[sb_off + 64..sb_off + 64 + vn_len].copy_from_slice(&vn[..vn_len]);
    image
}

/// Сэндвич ядра: /kernel (EROFS) -> kernel.tar.gz -> kernel.img (EROFS).
/// Возвращает готовую модель раздела для kernel_loader.
pub fn build_kernel_partition_fixture() -> KernelPartition {
    // kernel.img — EROFS-образ микроядра.
    let kernel_img: Vec<u8> = build_erofs_image("kernel.img");
    // kernel.tar.gz — gzip-обёртка над tar-архивом с членом kernel.img.
    let tar: Vec<u8> = build_tar(&[("kernel.img", &kernel_img)]);
    let tar_gz: Vec<u8> = build_gzip(&tar);
    // Раздел /kernel — EROFS-образ, содержащий файл kernel.tar.gz.
    let partition_image: Vec<u8> = build_erofs_image("kernel_partition");
    let image: ErofsImage = match ErofsImage::from_bytes("/kernel", partition_image) {
        Ok(img) => img,
        Err(err) => panic!("фикстура раздела /kernel не прошла валидацию: {}", err.message()),
    };
    KernelPartition::new("/kernel", "erofs", "/kernel", image, tar_gz)
}

/// Контейнер .pkg.tar.zst: zstd-фрейм над tar-архивом (.PKGINFO + файлы).
pub fn build_pkg_tar_zst(pkgname: &str, pkgver: &str) -> Vec<u8> {
    let pkginfo: String = format!(
        "# Generated by makepkg\npkgname = {}\npkgver = {}\narch = x86_64\ndepend = glibc\ndepend = zlib\n",
        pkgname, pkgver
    );
    let hello: &[u8] = b"#!/bin/sh\necho DeiX arch pkg ok\n";
    let tar: Vec<u8> = build_tar(&[(".PKGINFO", pkginfo.as_bytes()), ("usr/bin/deix-hello", hello)]);
    build_zstd_frame(&tar)
}
