# 🪐 DeiX OS (v0.1-release)

A modern, highly-secure 64-bit Operating System written from scratch in **Rust** and **Assembly** for `x86_64` architecture. Developed in a record-breaking 3 days in an intense collaboration between a Human Architect and an AI Agent.

Операционная система нового поколения, написанная с нуля на **Rust** и **Ассемблере** под архитектуру `x86_64`. Создана за рекордные 3 дня в уникальном тандеме Человека-Архитектора и ИИ-Агента.

---

## ⚡ Core Technical Features / Технические Особенности

### 🦀 Pure Rust Kernel (`#![no_std]`)
* **Long Mode:** Boots directly into full 64-bit privilege mode, escaping the old 16-bit real-mode limitations.
* **Safe Memory:** Bulletproof heap allocator and page tables implemented strictly under Rust's ownership rules to eliminate memory corruption and Triple Faults.
* **Микроядерная база:** Стабильное ядро, управляющее памятью и изолирующее системные компоненты.

### 🎨 Next-Gen Graphical Server & Procedural UI
* **Procedural Desktop:** Beautiful live-animated diagonal background lines generated entirely via mathematical algorithms in real-time. Extremely lightweight (takes only a few bytes in the binary).
* **Z-Order Window Manager:** Supports multiple independent movable windows (Terminal, Files, Display Settings) with smooth drag-and-drop.
* **Dynamic Resolution:** Hot-swapping display resolutions natively (from 640x480 up to 1280x1024) powered by UEFI GOP (Graphics Output Protocol).

### 🔐 Ironclad Security & Full-Disk Crypto
* **Hardware Isolation (Ring 3):** User space, login forms, and applications run strictly in the isolated **User Ring 3**, using custom GDT, TSS tables, and fast hardware `syscall` instruction handling.
* **One-Way Password Hashing:** User credentials (like the default `atimenka` account) are protected with highly-secure one-way cryptographic hashing (SHA-256/Blake3) with unique Salts. Cleartext passwords are never stored.
* **EXT2 Crypto Engine:** Integrated symmetric block encryption (AES-256-XTS / ChaCha20) wired directly into the EXT2 filesystem initializer. The master key lives purely in RAM and is securely wiped using `write_volatile` on `halt`. Without the password, the disk remains unreadable raw data.

### 🌐 Advanced Localization & Network Base
* **Hardware Cyrillic Output:** Built-in Russian language parser supporting 2-byte UTF-8 strings. Fonts are loaded dynamically into **VGA Plane 2** for perfect native display.
* **Localization Tools:** Includes a standalone `.py` script to instantly bake and pack any custom font or language table into raw kernel-readable byte arrays.
* **Network Ready:** Embedded software network stack supporting IEEE 802.11 Wi-Fi, WPA2-PSK encryption, and an active Realtek RTL8139 driver infrastructure ready for the `ping` command.

---

## 📂 File System Layout / Структура Диска (EXT2)
DeiX operates using a dedicated disk image layout, separating the bootable kernel from the encrypted system partition:
* **`deix.iso`** - Live bootable CD-ROM image containing the core UEFI bootstrap and the Rust kernel.
* **`deix_disk.img`** - Fully encrypted EXT2 system hard drive holding binaries, apps (`.mex`), and the secure user databases (`USERS.DB`).

---

## 🚀 Quick Start (Running in QEMU) / Быстрый Запуск

To boot DeiX OS v0.1 with the encrypted disk attached, run the following commands in your terminal (or use our built-in `./run_iso.sh` script):

Чтобы запустить DeiX OS v0.1 с подключением зашифрованного диска, выполните следующие команды:

```bash
# Navigate to the root directory / Переходим в корень проекта
cd deix/

# Launch QEMU with raw hard drive attached
# Запуск QEMU с эмуляцией сырого жесткого диска
qemu-system-x86_64 -drive file=./build/deix_disk.img,format=raw -m 256M
```

---

## 💻 Built-in CLI Commands / Доступные Команды

Open the **DeiX Terminal** window and type `help` to explore available features:

* `ls` - List files on the encrypted EXT2 drive (e.g., viewing `USERS.DB`).
* `whoami` - Show the currently authenticated user session (Default: `atimenka`).
* `lang ru / lang en` - Hot-swap system language with native font plane flashing.
* `run <program.mex>` - Execute custom independent standalone executable applications.
* `encrypt confirm <pass>` - Trigger full AES disk encryption block locking.
* `useradd / passwd / users` - Advanced multi-user account management.
* `gpu info / gpu mode` - Check graphics capabilities or force video buffer resolution.
* `ping / wifi` - Test the internal network stack and hardware availability.
* `crash` - Test kernel hardware exception handlers.
* `halt / reboot` - Safely wipe crypto keys from RAM and power down the CPU.

---

## 🗺️ Roadmap / Планы на Будущее (v0.2 & v3.0)
- [ ] Migrate to a strict Microkernel Architecture separating all `.bin` server tasks.
- [ ] Implement a compatibility layer for executing native **Linux binaries via static `musl-libc` / `glibc`** inside the `.mex` ecosystem.
- [ ] Bring up the open-source **NVIDIA Fermi (GeForce GT 520M)** bare-metal hardware driver leveraging `linux-firmware` microcode injected via `include_bytes!()`.
- [ ] Full ACPI, xHCI USB, and NVMe hardware polling for real PC deployment.

---
Developed by **Atimenka** & AI. Released under the GPL-3.0 License.
