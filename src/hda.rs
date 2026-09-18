//! Драйвер Intel High Definition Audio (HDA / Azalia).
//!
//! Реализует поддержку PCI-аудиоконтроллера Intel HDA (Class 0x04, Subclass 0x03,
//! например ICH6 0x8086:0x2668 в QEMU `-device intel-hda -device hda-duplex`),
//! управление кодеками через кольцевые буферы CORB/RIRB, настройку тракта
//! воспроизведения (AFG -> DAC -> Pin Widget) и потоковый вывод звука через DMA
//! (48 кГц, 16 бит, стерео).
//!
//! В отличие от классического PC speaker (порты 0x61/0x42), Intel HDA передаёт
//! качественный цифровой звук напрямую в современные звуковые серверы хоста
//! (PipeWire / PulseAudio / ALSA).

use crate::pci::{self, PciDevice};
use crate::spinlock::SpinLock;
use crate::sync::without_interrupts;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

// ==================== Реестр смещений MMIO HDA ====================
const REG_GCAP: usize = 0x00;       // Global Capabilities (u16)
const REG_VMIN: usize = 0x02;       // Minor Version (u8)
const REG_VMAJ: usize = 0x03;       // Major Version (u8)
const REG_GCTL: usize = 0x08;       // Global Control (u32)
const REG_WAKEEN: usize = 0x0C;     // Wake Enable (u16)
const REG_STATESTS: usize = 0x0E;   // State Change Status (u16)
const REG_INTCTL: usize = 0x20;     // Interrupt Control (u32)
const REG_INTSTS: usize = 0x24;     // Interrupt Status (u32)

// CORB (Command Outbound Ring Buffer)
const REG_CORBLBASE: usize = 0x40;  // CORB Lower Base Address (u32)
const REG_CORBUBASE: usize = 0x44;  // CORB Upper Base Address (u32)
const REG_CORBWP: usize = 0x48;     // CORB Write Pointer (u16)
const REG_CORBRP: usize = 0x4A;     // CORB Read Pointer (u16)
const REG_CORBCTL: usize = 0x4C;    // CORB Control (u8)
const REG_CORBSTS: usize = 0x4D;    // CORB Status (u8)
const REG_CORBSIZE: usize = 0x4E;   // CORB Size (u8)

// RIRB (Response Inbound Ring Buffer)
const REG_RIRBLBASE: usize = 0x50;  // RIRB Lower Base Address (u32)
const REG_RIRBUBASE: usize = 0x54;  // RIRB Upper Base Address (u32)
const REG_RIRBWP: usize = 0x58;     // RIRB Write Pointer (u16)
const REG_RINTCNT: usize = 0x5A;    // Response Interrupt Count (u16)
const REG_RIRBCTL: usize = 0x5C;    // RIRB Control (u8)
const REG_RIRBSTS: usize = 0x5D;    // RIRB Status (u8)
const REG_RIRBSIZE: usize = 0x5E;   // RIRB Size (u8)

// Смещения регистров внутри Stream Descriptor
const SD_OFF_CTL0: usize = 0x00;    // Control 0 (u8: bit 0=SRST, bit 1=SRUN)
const SD_OFF_CTL1: usize = 0x01;    // Control 1 (u8: interrupts)
const SD_OFF_CTL2: usize = 0x02;    // Control 2 (u8: bits 7..4 = Stream Number)
const SD_OFF_STS: usize = 0x03;     // Status (u8)
const SD_OFF_LPIB: usize = 0x04;    // Link Position In Buffer (u32)
const SD_OFF_CBL: usize = 0x08;     // Cyclic Buffer Length (u32)
const SD_OFF_LVI: usize = 0x0C;     // Last Valid Index (u16)
const SD_OFF_FIFOS: usize = 0x10;   // FIFO Size (u16)
const SD_OFF_FMT: usize = 0x12;     // Format (u16)
const SD_OFF_BDLPL: usize = 0x18;   // BDL Pointer Lower (u32)
const SD_OFF_BDLPU: usize = 0x1C;   // BDL Pointer Upper (u32)

// Формат потока: 48 кГц, 16 бит, 2 канала (стерео), PCM
// bit 14 = 0 (PCM), bits 13..11 = 0 (48kHz), bits 6..4 = 001 (16-bit), bits 3..0 = 0001 (2 chan)
const HDA_FORMAT_48K_16B_STEREO: u16 = 0x0011;
const SAMPLE_RATE: u32 = 48000;
const BYTES_PER_SAMPLE: usize = 4; // 16 бит * 2 канала = 4 байта

// Размеры DMA-буферов
const CORB_ENTRIES: usize = 256;
const RIRB_ENTRIES: usize = 256;
const BDL_ENTRIES: usize = 2;
const PERIOD_SIZE: usize = 16384;   // 16 КиБ = 4096 сэмплов (~85.3 мс)
const BUFFER_SIZE: usize = PERIOD_SIZE * BDL_ENTRIES; // 32 КиБ (~170.6 мс)

#[repr(C, align(128))]
struct CorbRing([u32; CORB_ENTRIES]);

#[repr(C, align(128))]
struct RirbRing([(u32, u32); RIRB_ENTRIES]);

#[repr(C, align(128))]
#[derive(Clone, Copy)]
struct BdlEntry {
    addr_lo: u32,
    addr_hi: u32,
    len: u32,
    flags: u32, // bit 0 = IOC
}

#[repr(C, align(128))]
struct DmaAudioBuffer([u8; BUFFER_SIZE]);

static mut CORB_BUF: CorbRing = CorbRing([0; CORB_ENTRIES]);
static mut RIRB_BUF: RirbRing = RirbRing([(0, 0); RIRB_ENTRIES]);
static mut BDL_TABLE: [BdlEntry; BDL_ENTRIES] = [BdlEntry {
    addr_lo: 0,
    addr_hi: 0,
    len: 0,
    flags: 0,
}; BDL_ENTRIES];
static mut AUDIO_DMA_BUF: DmaAudioBuffer = DmaAudioBuffer([0; BUFFER_SIZE]);

pub struct HdaController {
    mmio_base: usize,
    pci_dev: PciDevice,
    output_stream_offset: usize,
    corb_wp: u8,
    dac_nid: u8,
    pin_nid: u8,
    codec_addr: u8,
    initialized: bool,
}

static CONTROLLER: SpinLock<Option<HdaController>> = SpinLock::new(None);

// Безопасный MMIO доступ
#[inline(always)]
unsafe fn read8(base: usize, off: usize) -> u8 {
    core::ptr::read_volatile((base + off) as *const u8)
}
#[inline(always)]
unsafe fn write8(base: usize, off: usize, val: u8) {
    core::ptr::write_volatile((base + off) as *mut u8, val)
}
#[inline(always)]
unsafe fn read16(base: usize, off: usize) -> u16 {
    core::ptr::read_volatile((base + off) as *const u16)
}
#[inline(always)]
unsafe fn write16(base: usize, off: usize, val: u16) {
    core::ptr::write_volatile((base + off) as *mut u16, val)
}
#[inline(always)]
unsafe fn read32(base: usize, off: usize) -> u32 {
    core::ptr::read_volatile((base + off) as *const u32)
}
#[inline(always)]
unsafe fn write32(base: usize, off: usize, val: u32) {
    core::ptr::write_volatile((base + off) as *mut u32, val)
}

fn spin_delay(loops: u32) {
    for _ in 0..loops {
        unsafe { core::arch::asm!("nop") };
    }
}

impl HdaController {
    /// Посылает 32-битный HDA-глагол (verb) через CORB и дожидается ответа из RIRB.
    fn send_command(&mut self, cad: u8, nid: u8, verb: u32, param: u32) -> Option<u32> {
        let base = self.mmio_base;
        let cmd = ((cad as u32 & 0x0F) << 28)
            | ((nid as u32 & 0xFF) << 20)
            | ((verb & 0x0FFF) << 8)
            | (param & 0xFF);

        self.corb_wp = self.corb_wp.wrapping_add(1);
        let wp = self.corb_wp;

        unsafe {
            CORB_BUF.0[wp as usize] = cmd;
            write16(base, REG_CORBWP, wp as u16);
        }

        // Ждём появления ответа в RIRB
        for _ in 0..100_000 {
            let rirb_wp = unsafe { (read16(base, REG_RIRBWP) & 0xFF) as u8 };
            if rirb_wp == wp {
                let resp = unsafe { RIRB_BUF.0[rirb_wp as usize].0 };
                return Some(resp);
            }
            spin_delay(10);
        }
        None
    }

    /// Посылает 16-битный payload-глагол (например, Format 0x2, Amp Gain 0x3).
    fn send_command_16(&mut self, cad: u8, nid: u8, verb_4b: u32, payload_16b: u32) -> Option<u32> {
        let base = self.mmio_base;
        let cmd = ((cad as u32 & 0x0F) << 28)
            | ((nid as u32 & 0xFF) << 20)
            | ((verb_4b & 0x0F) << 16)
            | (payload_16b & 0xFFFF);

        self.corb_wp = self.corb_wp.wrapping_add(1);
        let wp = self.corb_wp;

        unsafe {
            CORB_BUF.0[wp as usize] = cmd;
            write16(base, REG_CORBWP, wp as u16);
        }

        for _ in 0..100_000 {
            let rirb_wp = unsafe { (read16(base, REG_RIRBWP) & 0xFF) as u8 };
            if rirb_wp == wp {
                let resp = unsafe { RIRB_BUF.0[rirb_wp as usize].0 };
                return Some(resp);
            }
            spin_delay(10);
        }
        None
    }

    /// Запускает воспроизведение потока через DMA.
    fn start_stream(&self) {
        let sd = self.mmio_base + self.output_stream_offset;
        unsafe {
            let ctl0 = read8(sd, SD_OFF_CTL0);
            write8(sd, SD_OFF_CTL0, ctl0 | 0x02); // SRUN = 1
        }
    }

    /// Останавливает воспроизведение потока через DMA.
    fn stop_stream(&self) {
        let sd = self.mmio_base + self.output_stream_offset;
        unsafe {
            let ctl0 = read8(sd, SD_OFF_CTL0);
            write8(sd, SD_OFF_CTL0, ctl0 & !0x02); // SRUN = 0
        }
    }

    /// Текущая позиция чтения контроллером (LPIB).
    fn stream_position(&self) -> u32 {
        let sd = self.mmio_base + self.output_stream_offset;
        unsafe { read32(sd, SD_OFF_LPIB) }
    }
}

/// Инициализация контроллера Intel HDA.
pub fn init() -> bool {
    // 1. Поиск устройства на шине PCI: Class 0x04 (Multimedia), Subclass 0x03 (Audio Device)
    let pci_dev = match pci::find_device_by_class_subclass(0x04, 0x03) {
        Some(dev) => dev,
        None => match pci::find_device(0x8086, 0x2668) {
            Some(dev) => dev,
            None => return false,
        },
    };

    crate::serial_println!(
        "[hda] Найдено устройство PCI {:02X}:{:02X}.{} (vendor 0x{:04X}, device 0x{:04X})",
        pci_dev.bus, pci_dev.slot, pci_dev.function, pci_dev.vendor_id, pci_dev.device_id
    );

    // Включаем Bus Master и Memory Space
    pci::enable_bus_mastering(pci_dev.bus, pci_dev.slot, pci_dev.function);
    pci::enable_memory_space(pci_dev.bus, pci_dev.slot, pci_dev.function);

    let mmio_bar = pci::read_bar_mmio(pci_dev.bus, pci_dev.slot, pci_dev.function, 0);
    if mmio_bar == 0 {
        crate::serial_println!("[hda] Ошибка: BAR0 равен 0");
        return false;
    }
    let mmio_base = mmio_bar as usize;
    crate::serial_println!("[hda] BAR0 MMIO база: {:#X}", mmio_base);

    // 2. Аппаратный сброс контроллера (GCTL.CRST)
    unsafe {
        let gctl = read32(mmio_base, REG_GCTL);
        if gctl & 0x01 != 0 {
            // Если контроллер уже был включён — переводим в сброс
            write32(mmio_base, REG_GCTL, gctl & !0x01);
            for _ in 0..10_000 {
                if read32(mmio_base, REG_GCTL) & 0x01 == 0 {
                    break;
                }
                spin_delay(10);
            }
        }
        // Выводим из сброса
        write32(mmio_base, REG_GCTL, 0x01);
        let mut ready = false;
        for _ in 0..100_000 {
            if read32(mmio_base, REG_GCTL) & 0x01 != 0 {
                ready = true;
                break;
            }
            spin_delay(10);
        }
        if !ready {
            crate::serial_println!("[hda] Ошибка: контроллер не вышел из сброса");
            return false;
        }
    }

    // Даём кодекам время на инициализацию
    spin_delay(50_000);

    // Проверяем наличие кодеков (STATESTS)
    let statests = unsafe { read16(mmio_base, REG_STATESTS) };
    crate::serial_println!("[hda] STATESTS (кодеки): {:#X}", statests);
    if statests == 0 {
        crate::serial_println!("[hda] Предупреждение: кодеки не ответили в STATESTS, пробуем адрес 0");
    }

    // 3. Настройка кольца CORB
    unsafe {
        // Останавливаем CORB
        write8(mmio_base, REG_CORBCTL, 0x00);
        for _ in 0..10_000 {
            if read8(mmio_base, REG_CORBCTL) & 0x02 == 0 {
                break;
            }
        }
        // Размер CORB: 256 записей (код 0x02)
        write8(mmio_base, REG_CORBSIZE, 0x02);
        let corb_phys = core::ptr::addr_of!(CORB_BUF) as u64;
        write32(mmio_base, REG_CORBLBASE, corb_phys as u32);
        write32(mmio_base, REG_CORBUBASE, (corb_phys >> 32) as u32);

        // Сброс read pointer CORB (бит 15)
        write16(mmio_base, REG_CORBRP, 0x8000);
        for _ in 0..10_000 {
            if read16(mmio_base, REG_CORBRP) & 0x8000 != 0 {
                break;
            }
        }
        write16(mmio_base, REG_CORBRP, 0x0000);
        for _ in 0..10_000 {
            if read16(mmio_base, REG_CORBRP) & 0x8000 == 0 {
                break;
            }
        }
        write16(mmio_base, REG_CORBWP, 0x0000);
        // Запуск CORB (бит 1)
        write8(mmio_base, REG_CORBCTL, 0x02);
    }

    // 4. Настройка кольца RIRB
    unsafe {
        write8(mmio_base, REG_RIRBCTL, 0x00);
        for _ in 0..10_000 {
            if read8(mmio_base, REG_RIRBCTL) & 0x02 == 0 {
                break;
            }
        }
        // Размер RIRB: 256 записей (код 0x02)
        write8(mmio_base, REG_RIRBSIZE, 0x02);
        let rirb_phys = core::ptr::addr_of!(RIRB_BUF) as u64;
        write32(mmio_base, REG_RIRBLBASE, rirb_phys as u32);
        write32(mmio_base, REG_RIRBUBASE, (rirb_phys >> 32) as u32);
        // Сброс WP (бит 15)
        write16(mmio_base, REG_RIRBWP, 0x8000);
        write16(mmio_base, REG_RINTCNT, 1);
        write8(mmio_base, REG_RIRBSTS, 0x05); // сброс прерываний
        // Запуск RIRB (бит 1)
        write8(mmio_base, REG_RIRBCTL, 0x02);
    }

    // Определение смещения первого Output Stream
    let gcap = unsafe { read16(mmio_base, REG_GCAP) };
    let iss = ((gcap >> 8) & 0x0F) as usize;
    let oss = ((gcap >> 12) & 0x0F) as usize;
    let out_stream_off = 0x80 + iss * 0x20;
    crate::serial_println!(
        "[hda] GCAP: ISS={}, OSS={}; Output Stream 0 смещение: {:#X}",
        iss, oss, out_stream_off
    );

    let mut controller = HdaController {
        mmio_base,
        pci_dev,
        output_stream_offset: out_stream_off,
        corb_wp: 0,
        dac_nid: 2, // умолчание для QEMU hda-duplex
        pin_nid: 3, // умолчание для QEMU hda-duplex
        codec_addr: 0,
        initialized: false,
    };

    // 5. Опрос кодека и автоматическое обнаружение DAC и Pin
    let vendor_id = controller.send_command(0, 0, 0xF00, 0x00);
    crate::serial_println!("[hda] Codec 0 Root Vendor/Device ID: {:?}", vendor_id);

    // Включаем Audio Function Group (обычно NID 1)
    controller.send_command(0, 1, 0x705, 0x00); // Power D0

    // Ищем узлы DAC (Audio Output) и Pin Complex
    let sub_nodes = controller.send_command(0, 1, 0xF00, 0x04).unwrap_or(0x0002_0004);
    let start_nid = ((sub_nodes >> 16) & 0xFF) as u8;
    let count_nodes = (sub_nodes & 0xFF) as u8;

    let mut found_dac = None;
    let mut found_pin = None;

    for nid in start_nid..(start_nid + count_nodes) {
        // Включаем питание D0 для всех виджетов
        controller.send_command(0, nid, 0x705, 0x00);
        let caps = controller.send_command(0, nid, 0xF00, 0x09).unwrap_or(0);
        let wtype = (caps >> 20) & 0x0F;
        if wtype == 0x0 && found_dac.is_none() {
            found_dac = Some(nid);
        } else if wtype == 0x4 && found_pin.is_none() {
            found_pin = Some(nid);
        }
    }

    if let Some(dac) = found_dac {
        controller.dac_nid = dac;
    }
    if let Some(pin) = found_pin {
        controller.pin_nid = pin;
    }

    crate::serial_println!(
        "[hda] Сконфигурирован аудиотракт: DAC NID={}, Pin NID={}",
        controller.dac_nid, controller.pin_nid
    );

    // 6. Настройка DAC
    // Stream 1, Channel 0
    controller.send_command(0, controller.dac_nid, 0x706, 0x10);
    // Format 48kHz, 16-bit, stereo
    controller.send_command_16(0, controller.dac_nid, 0x2, HDA_FORMAT_48K_16B_STEREO as u32);
    // Unmute Output Amp, максимальная громкость 0x7F
    controller.send_command_16(0, controller.dac_nid, 0x3, 0xB07F);

    // 7. Настройка Pin Complex
    // Out Enable (0x40) + Headphone Enable (0x80) = 0xC0
    controller.send_command(0, controller.pin_nid, 0x707, 0xC0);
    // Включаем EAPD (внешний усилитель)
    controller.send_command(0, controller.pin_nid, 0x70C, 0x02);
    // Unmute Pin Amp
    controller.send_command_16(0, controller.pin_nid, 0x3, 0xB07F);

    // 8. Настройка DMA-дескрипторов потока (Stream Descriptor)
    let sd = mmio_base + out_stream_off;
    unsafe {
        // Останавливаем поток
        write8(sd, SD_OFF_CTL0, 0x00);
        for _ in 0..10_000 {
            if read8(sd, SD_OFF_CTL0) & 0x02 == 0 {
                break;
            }
        }
        // Сброс потока (SRST = 1, затем 0)
        write8(sd, SD_OFF_CTL0, 0x01);
        for _ in 0..10_000 {
            if read8(sd, SD_OFF_CTL0) & 0x01 != 0 {
                break;
            }
        }
        write8(sd, SD_OFF_CTL0, 0x00);
        for _ in 0..10_000 {
            if read8(sd, SD_OFF_CTL0) & 0x01 == 0 {
                break;
            }
        }

        // Заполняем BDL (таблица из 2 записей по 16 КиБ)
        let buf_phys = core::ptr::addr_of!(AUDIO_DMA_BUF) as u64;
        BDL_TABLE[0] = BdlEntry {
            addr_lo: buf_phys as u32,
            addr_hi: (buf_phys >> 32) as u32,
            len: PERIOD_SIZE as u32,
            flags: 0,
        };
        BDL_TABLE[1] = BdlEntry {
            addr_lo: (buf_phys + PERIOD_SIZE as u64) as u32,
            addr_hi: ((buf_phys + PERIOD_SIZE as u64) >> 32) as u32,
            len: PERIOD_SIZE as u32,
            flags: 0,
        };

        // Пишем адрес BDL
        let bdl_phys = core::ptr::addr_of!(BDL_TABLE) as u64;
        write32(sd, SD_OFF_BDLPL, bdl_phys as u32);
        write32(sd, SD_OFF_BDLPU, (bdl_phys >> 32) as u32);

        // Общая длина кольцевого буфера
        write32(sd, SD_OFF_CBL, BUFFER_SIZE as u32);
        // Индекс последней записи (2 записи - 1 = 1)
        write16(sd, SD_OFF_LVI, 1);
        // Формат потока
        write16(sd, SD_OFF_FMT, HDA_FORMAT_48K_16B_STEREO);
        // Номер потока (Stream ID = 1) в байте CTL2
        write8(sd, SD_OFF_CTL2, 0x10);
        // Сброс флагов статуса
        write8(sd, SD_OFF_STS, 0x1C);

        // Буфер DMA инициализируем тишиной
        core::ptr::write_bytes(core::ptr::addr_of_mut!(AUDIO_DMA_BUF) as *mut u8, 0, BUFFER_SIZE);
    }

    controller.initialized = true;
    *CONTROLLER.lock() = Some(controller);
    crate::serial_println!("[hda] Драйвер Intel HDA успешно инициализирован и готов к выводу звука!");
    true
}

/// Проверка, доступен ли драйвер Intel HDA.
pub fn is_ready() -> bool {
    without_interrupts(|| CONTROLLER.lock().as_ref().map(|c| c.initialized).unwrap_or(false))
}

/// Воспроизведение непрерывного массива 16-битных стерео-сэмплов (48 кГц).
pub fn play_pcm_stereo_48k(samples: &[i16]) {
    without_interrupts(|| {
        let mut ctrl_lock = CONTROLLER.lock();
        let ctrl = match ctrl_lock.as_mut() {
            Some(c) if c.initialized => c,
            _ => return,
        };

        let raw_bytes: &[u8] = unsafe {
            core::slice::from_raw_parts(samples.as_ptr() as *const u8, samples.len() * 2)
        };

        let total_bytes = raw_bytes.len();
        if total_bytes == 0 {
            return;
        }

        let mut offset = 0usize;
        let dma_ptr = unsafe { core::ptr::addr_of_mut!(AUDIO_DMA_BUF.0) as *mut u8 };

        // Заполняем весь начальный буфер DMA первыми данными
        let init_take = total_bytes.min(BUFFER_SIZE);
        unsafe {
            core::ptr::copy_nonoverlapping(raw_bytes.as_ptr(), dma_ptr, init_take);
            if init_take < BUFFER_SIZE {
                core::ptr::write_bytes(dma_ptr.add(init_take), 0, BUFFER_SIZE - init_take);
            }
        }
        offset += init_take;

        ctrl.start_stream();

        // Потоковая подгрузка данных в циклическом буфере DMA
        let mut last_period: usize = 0;
        let start_ms = crate::timer::uptime_ms();
        let expected_duration_ms = (total_bytes as u64 * 1000) / (SAMPLE_RATE as u64 * BYTES_PER_SAMPLE as u64) + 150;

        while offset < total_bytes {
            let lpib = ctrl.stream_position() as usize;
            let current_period = (lpib / PERIOD_SIZE) % BDL_ENTRIES;

            // Если контроллер перешёл в следующий период, освободившийся предыдущий можно перезаписать
            if current_period != last_period {
                let write_period = last_period;
                let write_dest = unsafe { dma_ptr.add(write_period * PERIOD_SIZE) };
                let remaining = total_bytes - offset;
                let chunk_size = remaining.min(PERIOD_SIZE);

                unsafe {
                    core::ptr::copy_nonoverlapping(raw_bytes.as_ptr().add(offset), write_dest, chunk_size);
                    if chunk_size < PERIOD_SIZE {
                        core::ptr::write_bytes(write_dest.add(chunk_size), 0, PERIOD_SIZE - chunk_size);
                    }
                }
                offset += chunk_size;
                last_period = current_period;
            }

            // Защита от бесконечного зависания
            if crate::timer::uptime_ms().saturating_sub(start_ms) > expected_duration_ms {
                break;
            }

            spin_delay(500);
        }

        // Дожидаемся завершения воспроизведения остатка
        let tail_wait_start = crate::timer::uptime_ms();
        while crate::timer::uptime_ms().saturating_sub(tail_wait_start) < 180 {
            spin_delay(1000);
        }

        ctrl.stop_stream();
        unsafe {
            core::ptr::write_bytes(dma_ptr, 0, BUFFER_SIZE);
        }
    });
}

/// Воспроизведение звукового тона заданной частоты и длительности через Intel HDA.
pub fn play_tone(hz: u32, duration_ms: u64) {
    if !is_ready() || hz == 0 || duration_ms == 0 {
        return;
    }

    let total_samples = (SAMPLE_RATE as u64 * duration_ms / 1000) as usize;
    let period_samples = (SAMPLE_RATE / hz.max(20)).max(2) as usize;

    let mut pcm: Vec<i16> = Vec::with_capacity(total_samples * 2);
    let volume: i16 = 12000; // ~40% от максимума для комфортного звука

    for i in 0..total_samples {
        let phase = i % period_samples;
        // Квадратная волна (меандр) со сглаживанием
        let sample = if phase < period_samples / 2 {
            volume
        } else {
            -volume
        };
        pcm.push(sample); // Левый канал
        pcm.push(sample); // Правый канал
    }

    play_pcm_stereo_48k(&pcm);
}

/// Воспроизведение цифрового DPS1 звука (8 кГц, 8 бит моно) через ресэмплинг в HDA (48 кГц, 16 бит стерео).
pub fn play_dps(dps_data: &[u8]) -> Result<(), &'static str> {
    if !is_ready() {
        return Err("HDA controller not ready");
    }
    if dps_data.len() < 12 || &dps_data[..4] != b"DPS1" {
        return Err("Not DPS1 audio format");
    }

    let rate = u32::from_le_bytes([dps_data[4], dps_data[5], dps_data[6], dps_data[7]]);
    let num_samples = u32::from_le_bytes([dps_data[8], dps_data[9], dps_data[10], dps_data[11]]) as usize;

    if dps_data.len() < 12 + num_samples || rate == 0 {
        return Err("Corrupted DPS1 data");
    }

    let samples_8k = &dps_data[12..12 + num_samples];
    // Коэффициент ресэмплинга (для 8000 Гц в 48000 Гц коэффициент ровно 6)
    let ratio = (SAMPLE_RATE / rate.max(1)).clamp(1, 24) as usize;

    let mut out_pcm: Vec<i16> = Vec::with_capacity(num_samples * ratio * 2);

    for &s in samples_8k {
        // Преобразуем unsigned 8-bit (128=тишина) в signed 16-bit
        let sample_16 = ((s as i16) - 128).saturating_mul(180);
        for _ in 0..ratio {
            out_pcm.push(sample_16); // Левый
            out_pcm.push(sample_16); // Правый
        }
    }

    play_pcm_stereo_48k(&out_pcm);
    Ok(())
}

/// Диагностическая информация о контроллере Intel HDA.
pub fn get_info() -> Option<String> {
    CONTROLLER.lock().as_ref().map(|c| {
        let gcap = unsafe { read16(c.mmio_base, REG_GCAP) };
        let iss = (gcap >> 8) & 0x0F;
        let oss = (gcap >> 12) & 0x0F;
        let gctl = unsafe { read32(c.mmio_base, REG_GCTL) };
        let statests = unsafe { read16(c.mmio_base, REG_STATESTS) };
        format!(
            "Intel HDA Controller:\n\
             PCI Device: {:02X}:{:02X}.{} (Vendor 0x{:04X}, Device 0x{:04X})\n\
             MMIO Base:  0x{:X}\n\
             Global:     CRST={}, Codecs={:#X}, ISS={}, OSS={}\n\
             Audio Path: DAC NID={}, Pin NID={}\n\
             DMA Stream: 48 kHz, 16-bit Stereo PCM (Output Stream 0)",
            c.pci_dev.bus, c.pci_dev.slot, c.pci_dev.function,
            c.pci_dev.vendor_id, c.pci_dev.device_id,
            c.mmio_base,
            gctl & 1, statests, iss, oss,
            c.dac_nid, c.pin_nid
        )
    })
}
