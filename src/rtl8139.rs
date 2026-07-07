//! Драйвер сетевой карты RTL8139 — самая распространённая для эмуляции
//! Ethernet-карта (её же по умолчанию эмулирует QEMU для `-net nic` /
//! `-device rtl8139`), поэтому это стандартный "первый драйвер" для
//! учебных ОС.
//!
//! Так как наш загрузчик настраивает identity-mapping первого 1GiB
//! физической памяти (виртуальный адрес == физический адрес), можно
//! спокойно использовать адреса обычных статических буферов ядра как
//! физические адреса для DMA — какая-то отдельная работа со страницами
//! не требуется.

use crate::port::{inb, inw, outb, outl, outw};
use crate::spinlock::SpinLock;
use crate::sync::without_interrupts;
use crate::{pci, println};

// Регистры RTL8139 (смещения от базового I/O адреса, см. wiki.osdev.org/RTL8139).
const REG_MAC0: u16 = 0x00;
const REG_RBSTART: u16 = 0x30;
const REG_CMD: u16 = 0x37;
const REG_CAPR: u16 = 0x38;
const REG_IMR: u16 = 0x3C;
const REG_ISR: u16 = 0x3E;
const REG_RCR: u16 = 0x44;
const REG_CONFIG1: u16 = 0x52;

const REG_TSAD0: u16 = 0x20; // TSAD0..TSAD3 = 0x20, 0x24, 0x28, 0x2C
const REG_TSD0: u16 = 0x10; // TSD0..TSD3 = 0x10, 0x14, 0x18, 0x1C

const CMD_RESET: u8 = 0x10;
const CMD_RX_ENABLE: u8 = 0x08;
const CMD_TX_ENABLE: u8 = 0x04;

const ISR_ROK: u16 = 0x01; // Receive OK
const ISR_TOK: u16 = 0x04; // Transmit OK

const RX_BUFFER_SIZE: usize = 8192 + 16 + 1500; // 8K + 16 + WRAP-запас
const TX_BUFFER_SIZE: usize = 1792; // максимум для одного дескриптора TSD

const NUM_TX_DESCRIPTORS: usize = 4;

#[repr(align(4))]
#[allow(dead_code)]
struct RxBuffer([u8; RX_BUFFER_SIZE]);

#[repr(align(4))]
struct TxBuffer([u8; TX_BUFFER_SIZE]);

static mut RX_BUFFER: RxBuffer = RxBuffer([0; RX_BUFFER_SIZE]);
static mut TX_BUFFERS: [TxBuffer; NUM_TX_DESCRIPTORS] = [
    TxBuffer([0; TX_BUFFER_SIZE]),
    TxBuffer([0; TX_BUFFER_SIZE]),
    TxBuffer([0; TX_BUFFER_SIZE]),
    TxBuffer([0; TX_BUFFER_SIZE]),
];

struct Rtl8139State {
    io_base: u16,
    mac: [u8; 6],
    rx_offset: usize,
    tx_next: usize,
    initialized: bool,
}

impl Rtl8139State {
    const fn new() -> Self {
        Rtl8139State {
            io_base: 0,
            mac: [0; 6],
            rx_offset: 0,
            tx_next: 0,
            initialized: false,
        }
    }
}

static STATE: SpinLock<Rtl8139State> = SpinLock::new(Rtl8139State::new());

/// Пытается найти RTL8139 на шине PCI и проинициализировать её. Возвращает
/// true, если карта найдена и готова к работе (тогда можно слать/принимать
/// пакеты), false — если карты нет (например, QEMU запущен без `-net nic`).
pub fn init() -> bool {
    // Vendor ID 0x10EC (Realtek), Device ID 0x8139.
    let device = match pci::find_device(0x10EC, 0x8139) {
        Some(d) => d,
        None => return false,
    };

    pci::enable_bus_mastering(device.bus, device.slot, device.function);

    let io_base = match pci::read_bar0_io(device.bus, device.slot, device.function) {
        Some(addr) => addr,
        None => return false,
    };

    unsafe {
        // Включаем карту (LWAKE + LWPTN active high).
        outb(io_base + REG_CONFIG1, 0x00);

        // Программный сброс.
        outb(io_base + REG_CMD, CMD_RESET);
        // NB: на QEMU бывает баг с изначально выставленным битом RST —
        // это нормально, просто ждём, пока чип сам его сбросит.
        let mut attempts = 0;
        while inb(io_base + REG_CMD) & CMD_RESET != 0 {
            attempts += 1;
            if attempts > 1_000_000 {
                return false; // что-то пошло не так — не блокируем загрузку ОС
            }
        }

        // Читаем MAC-адрес карты (регистры MAC0-5, по байту).
        let mut mac = [0u8; 6];
        for (i, byte) in mac.iter_mut().enumerate() {
            *byte = inb(io_base + REG_MAC0 + i as u16);
        }

        // Настраиваем приёмный буфer.
        let rx_phys_addr = core::ptr::addr_of!(RX_BUFFER) as u32;
        outl(io_base + REG_RBSTART, rx_phys_addr);

        // Разрешаем прерывания Transmit OK и Receive OK.
        outw(io_base + REG_IMR, ISR_ROK | ISR_TOK);

        // RCR: принимаем broadcast + multicast + пакеты на наш MAC + WRAP=1.
        const AB: u32 = 1 << 3;
        const AM: u32 = 1 << 2;
        const APM: u32 = 1 << 1;
        const WRAP: u32 = 1 << 7;
        outl(io_base + REG_RCR, AB | AM | APM | WRAP);

        // Включаем приёмник и передатчик.
        outb(io_base + REG_CMD, CMD_RX_ENABLE | CMD_TX_ENABLE);

        let mut state = STATE.lock();
        state.io_base = io_base;
        state.mac = mac;
        state.rx_offset = 0;
        state.tx_next = 0;
        state.initialized = true;
    }

    let irq_line = pci::read_interrupt_line(device.bus, device.slot, device.function);
    crate::interrupts::register_network_irq(irq_line);

    let mac = STATE.lock().mac;
    println!(
        "RTL8139 network card found: MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}, IRQ line {}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5], irq_line
    );

    true
}

pub fn is_ready() -> bool {
    without_interrupts(|| STATE.lock().initialized)
}

pub fn mac_address() -> [u8; 6] {
    without_interrupts(|| STATE.lock().mac)
}

/// Отправляет один Ethernet-фрейм (уже полностью собранный, начиная с
/// заголовка Ethernet). Максимальный размер — 1792 байта (ограничение
/// одного TX-дескриптора RTL8139).
pub fn send_frame(data: &[u8]) -> bool {
    if data.len() > TX_BUFFER_SIZE {
        return false;
    }

    without_interrupts(|| {
        let mut state = STATE.lock();
        if !state.initialized {
            return false;
        }

        let idx = state.tx_next;
        state.tx_next = (state.tx_next + 1) % NUM_TX_DESCRIPTORS;

        unsafe {
            let buf = &mut TX_BUFFERS[idx].0;
            buf[..data.len()].copy_from_slice(data);

            let phys_addr = buf.as_ptr() as u32;
            let tsad = state.io_base + REG_TSAD0 + (idx as u16) * 4;
            let tsd = state.io_base + REG_TSD0 + (idx as u16) * 4;

            outl(tsad, phys_addr);
            // Записываем размер пакета в TSD — это снимает "own"-бит и
            // запускает передачу. Минимальный Ethernet-фрейм — 60 байт
            // (без учёта FCS, который считает сама карта).
            let size = data.len().max(60) as u32;
            outl(tsd, size);
        }

        true
    })
}

/// Вызывается из обработчика прерывания IRQ карты (см. interrupts.rs).
/// Читает регистр ISR, подтверждает прерывание и, если пришёл пакет,
/// передаёт его выше по стеку (в net::on_ethernet_frame).
pub fn on_interrupt() {
    let io_base = {
        let state = STATE.lock();
        if !state.initialized {
            return;
        }
        state.io_base
    };

    unsafe {
        let status = inw(io_base + REG_ISR);
        // Важно подтвердить (написать) прерывание ДО чтения пакетов из
        // буфера — иначе на QEMU повторные пакеты не будут доставляться.
        outw(io_base + REG_ISR, status);

        if status & ISR_ROK != 0 {
            drain_rx_buffer(io_base);
        }
    }
}

/// Вычитывает из кольцевого RX-буфера все накопившиеся пакеты и передаёт
/// их обработчику Ethernet-уровня.
unsafe fn drain_rx_buffer(io_base: u16) {
    loop {
        // Если приёмный буфер помечен пустым (бит 0 регистра CMD) — выходим.
        if inb(io_base + REG_CMD) & 0x01 != 0 {
            break;
        }

        let mut offset = STATE.lock().rx_offset;
        let rx_buf = core::ptr::addr_of!(RX_BUFFER) as *const u8;

        // Заголовок пакета: 2 байта статус + 2 байта длина (включая заголовок).
        let header_ptr = rx_buf.add(offset) as *const u16;
        let packet_status = core::ptr::read_unaligned(header_ptr);
        let packet_len = core::ptr::read_unaligned(header_ptr.add(1)) as usize;

        const ROK: u16 = 0x01;
        if packet_status & ROK == 0 || packet_len < 4 || packet_len > 1600 {
            // Похоже на рассинхронизацию буфера — сбрасываем всё и выходим,
            // чтобы не улететь в чтение мусора по всей памяти.
            break;
        }

        // Сами данные фрейма начинаются через 4 байта после начала записи
        // и заканчиваются 4-байтовым CRC, который нам не нужен.
        let data_start = offset + 4;
        let data_len = packet_len - 4;

        // Копируем во временный стековый буфер, потому что данные в
        // кольцевом буфере физически могут "переворачиваться" через конец.
        let mut frame_buf = [0u8; 1600];
        for i in 0..data_len {
            let src_offset = (data_start + i) % RX_BUFFER_SIZE;
            frame_buf[i] = *rx_buf.add(src_offset);
        }

        crate::net::on_ethernet_frame(&frame_buf[..data_len]);

        // Сдвигаем offset на размер пакета + заголовок, выравнивая по 4 байта
        // (так требует спецификация RTL8139).
        offset = (offset + packet_len + 4 + 3) & !3;
        offset %= RX_BUFFER_SIZE;

        STATE.lock().rx_offset = offset;
        outw(io_base + REG_CAPR, (offset as u16).wrapping_sub(16));
    }
}
