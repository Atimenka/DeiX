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

const RX_BUFFER_SIZE: usize = 65536; // 64K — ровно кольцо QEMU rtl8139 (MOD2 по 65536); 65552 разъезжалось на 16 после 64КБ
const TX_BUFFER_SIZE: usize = 1792; // максимум для одного дескриптора TSD

const NUM_TX_DESCRIPTORS: usize = 4;

#[repr(align(4))]
struct RxBuffer {
    bytes: [u8; RX_BUFFER_SIZE],
}

#[repr(align(4))]
struct TxBuffer([u8; TX_BUFFER_SIZE]);

static mut RX_BUFFER: RxBuffer = RxBuffer { bytes: [0; RX_BUFFER_SIZE] };
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

// ==================== Отложенная обработка RX ====================
//
// Обработчик прерывания карты обязан вернуться как можно быстрее и не
// имеет права аллоцировать: куча защищена локом, который может держать
// прерванный код, а разбор Ethernet/IP/TCP — это сотни инструкций на
// пакет. Поэтому ISR только копирует кадр в заранее выделенное кольцо,
// а весь стек работает потом, из `poll_deferred`.

/// Число слотов приёмного кольца.
const RX_RING_SLOTS: usize = 16;
/// Максимальный размер кадра в слоте.
const RX_RING_FRAME: usize = 1600;

struct RxRing {
    slots: [[u8; RX_RING_FRAME]; RX_RING_SLOTS],
    lens: [usize; RX_RING_SLOTS],
    /// Куда пишет ISR.
    head: usize,
    /// Откуда читает рабочий поток.
    tail: usize,
    /// Сколько кадров сейчас в кольце.
    count: usize,
    /// Кадры, потерянные из-за переполнения кольца.
    drops: u64,
}

impl RxRing {
    const fn new() -> Self {
        RxRing {
            slots: [[0; RX_RING_FRAME]; RX_RING_SLOTS],
            lens: [0; RX_RING_SLOTS],
            head: 0,
            tail: 0,
            count: 0,
            drops: 0,
        }
    }
}

/// Кольцо защищено `IrqSpinLock`: его одновременно трогают ISR карты и
/// рабочий поток, а на одном CPU это ровно тот случай, когда обычный
/// спинлок даёт мёртвую петлю.
static RX_RING: crate::spinlock::IrqSpinLock<RxRing> = crate::spinlock::IrqSpinLock::new(RxRing::new());

/// Кладёт кадр в приёмное кольцо. Вызывается из ISR, без аллокаций.
/// Возвращает `false`, если кольцо заполнено и кадр потерян.
fn rx_ring_push(frame: &[u8]) -> bool {
    let mut ring = RX_RING.lock();
    if ring.count >= RX_RING_SLOTS {
        ring.drops += 1;
        return false;
    }
    let len = frame.len().min(RX_RING_FRAME);
    let head = ring.head;
    ring.slots[head][..len].copy_from_slice(&frame[..len]);
    ring.lens[head] = len;
    ring.head = (head + 1) % RX_RING_SLOTS;
    ring.count += 1;
    true
}

/// Забирает следующий кадр из кольца. Вызывается из рабочего потока.
fn rx_ring_pop() -> Option<(usize, usize)> {
    let mut ring = RX_RING.lock();
    if ring.count == 0 {
        return None;
    }
    let slot = ring.tail;
    let len = ring.lens[slot];
    ring.tail = (ring.tail + 1) % RX_RING_SLOTS;
    ring.count -= 1;
    Some((slot, len))
}

/// Сколько кадров потеряно из-за переполнения приёмного кольца.
pub fn rx_drops() -> u64 {
    RX_RING.lock().drops
}

/// Разбирает накопившиеся в кольце кадры сетевым стеком.
///
/// Вызывается из основного цикла и из `tcp::poll_rx`, то есть вне
/// контекста прерывания — здесь уже можно аллоцировать.
pub fn poll_deferred() {
    while let Some((slot, len)) = rx_ring_pop() {
        // Копируем кадр из слота, чтобы не держать блокировку кольца
        // на время разбора стеком.
        let mut buf = [0u8; RX_RING_FRAME];
        {
            let ring = RX_RING.lock();
            buf[..len].copy_from_slice(&ring.slots[slot][..len]);
        }
        crate::net::on_ethernet_frame(&buf[..len]);
    }

    // Переполнения кольца фиксирует ISR (там журналировать нельзя —
    // форматирование и куча в прерывании запрещены), а регистрируем мы их
    // здесь, по дельте счётчика. Повторы схлопывает агрегация журнала.
    static LOGGED_DROPS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    let drops = rx_drops();
    let logged = LOGGED_DROPS.swap(drops, core::sync::atomic::Ordering::Relaxed);
    if drops > logged {
        crate::diag::warn(
            crate::diag::ErrorCode::new(crate::diag::Subsystem::Net, 7),
            &alloc::format!(
                "RTL8139: кольцо приёма переполнено, потеряно кадров: +{} (всего {})",
                drops - logged,
                drops
            ),
        );
    }
}

/// Счётчики трафика. Обновляются атомарно: RX растёт в контексте
/// прерывания, поэтому блокировку здесь брать нельзя.
static RX_PACKETS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static RX_BYTES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static TX_PACKETS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static TX_BYTES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Накопленный трафик: `(rx_packets, rx_bytes, tx_packets, tx_bytes)`.
pub fn traffic_counters() -> (u64, u64, u64, u64) {
    use core::sync::atomic::Ordering;
    (
        RX_PACKETS.load(Ordering::Relaxed),
        RX_BYTES.load(Ordering::Relaxed),
        TX_PACKETS.load(Ordering::Relaxed),
        TX_BYTES.load(Ordering::Relaxed),
    )
}

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
        let rx_phys_addr = core::ptr::addr_of!(RX_BUFFER.bytes) as u32;
        outl(io_base + REG_RBSTART, rx_phys_addr);

        // Разрешаем прерывания Transmit OK и Receive OK.
        outw(io_base + REG_IMR, ISR_ROK | ISR_TOK);

        // RCR: принимаем broadcast + multicast + пакеты на наш MAC + WRAP=1.
        const AB: u32 = 1 << 3;
        const AM: u32 = 1 << 2;
        const APM: u32 = 1 << 1;
        const WRAP: u32 = 1 << 7;
        // RX-буфер 64K (QEMU читает размер из битов 12:11 RCR:
        // (val>>11)&3 = 3 -> 64K; биты 11:10 НЕ работают — буфер оставался
        // 16K и переполнялся при доставке файла ~250 КБ).
        const RXBUF64: u32 = 3 << 11;
        outl(io_base + REG_RCR, AB | AM | APM | WRAP | RXBUF64);

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

        TX_PACKETS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        TX_BYTES.fetch_add(data.len() as u64, core::sync::atomic::Ordering::Relaxed);

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
        // RX_OVERFLOW (бит 0x10): приёмный буфер переполнился — пакеты
        // теряются. Нужен перезапуск приёмника, иначе передача встаёт.
        if status & 0x10 != 0 {
            // RX-буфер переполнился: перезапускаем приёмник, иначе пакеты
            // теряются и передача большого ответа встаёт.
            outb(io_base + REG_CMD, CMD_RX_ENABLE);
            outb(io_base + REG_CMD, CMD_RX_ENABLE | CMD_TX_ENABLE);
        }
    }
}

/// Вычитывает из кольцевого RX-буфера все накопившиеся пакеты и передаёт
/// их обработчику Ethernet-уровня.
pub fn poll_rx() {
    let io_base = {
        let st = STATE.lock();
        if !st.initialized { return; }
        st.io_base
    };
    // Сначала выгребаем кадры из карты в кольцо, потом разбираем их
    // сетевым стеком уже вне контекста прерывания.
    unsafe { drain_rx_buffer(io_base); }
    poll_deferred();
}

unsafe fn drain_rx_buffer(io_base: u16) {
    loop {
        // Если приёмный буфер помечен пустым (бит 0 регистра CMD) — выходим.
        if inb(io_base + REG_CMD) & 0x01 != 0 {
            break;
        }

        let mut offset = STATE.lock().rx_offset;
        let rx_buf = core::ptr::addr_of!(RX_BUFFER.bytes) as *const u8;

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

        RX_PACKETS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        RX_BYTES.fetch_add(data_len as u64, core::sync::atomic::Ordering::Relaxed);

        // Копируем во временный стековый буфер, потому что данные в
        // кольцевом буфере физически могут "переворачиваться" через конец.
        let mut frame_buf = [0u8; 1600];
        for i in 0..data_len {
            let src_offset = (data_start + i) % RX_BUFFER_SIZE;
            frame_buf[i] = *rx_buf.add(src_offset);
        }

        // Только постановка в кольцо: разбор Ethernet/IP/TCP делает
        // poll_deferred уже вне контекста прерывания.
        rx_ring_push(&frame_buf[..data_len]);

        // Сдвигаем offset на размер пакета + заголовок, выравнивая по 4 байта
        // (так требует спецификация RTL8139).
        offset = (offset + packet_len + 4 + 3) & !3;
        offset %= RX_BUFFER_SIZE;

        STATE.lock().rx_offset = offset;
        outw(io_base + REG_CAPR, (offset as u16).wrapping_sub(16));
    }
}
