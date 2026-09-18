// ❗ЗАВИСИМОТИ: инит скрипт pid 1 который будет ограничивать пользовательский
// Модульная сборка MASTER SPEC DeiX OS: каждый модуль — отдельный файл исходника.
// PID 1 изолирует Ring 3: /kernel /init_boot /boot /vendor_boot /super /system
// /recovery (erofs/ro), /userdata (ext4/rw). Arch PKG -> .pkg.erofs внутри /userdata.
// TWRP/OrangeFox + Fastbootd/EDL. Zero External Dependencies, совместимо с no_std.

#![allow(dead_code)]

//! ==========================================================================
//! DeiX OS — MASTER SPEC: PARTITIONS, EROFS SANDWICH, ARCH PKG, TWRP/EDL
//! ==========================================================================
//! Точка входа модульной сборки. Каждый модуль расположен в отдельном файле:
//!   init_parser, kernel_loader, arch_pkg_bridge, recovery_flash_engine,
//!   security_monitor, vault, partition_map, fixtures, polygon, tests.
//! Сборка (ноль внешних крейтов): rustc --edition 2021 -D warnings -O main.rs
//! Тесты: rustc --edition 2021 --test -D warnings main.rs -o deix_tests
//! Стилистика — ветка development репозитория Atimenka/DeiX (коммит ef8132b).

mod init_parser;
mod kernel_loader;
mod arch_pkg_bridge;
mod recovery_flash_engine;
mod security_monitor;
mod vault;
mod partition_map;
mod fixtures;
mod polygon;

#[cfg(test)]
mod tests;

/// Точка входа: делегирует управление модулю `polygon` (отладочный полигон).
fn main() {
    polygon::run();
}
