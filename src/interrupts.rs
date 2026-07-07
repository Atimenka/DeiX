//! IDT (Interrupt Descriptor Table): обработчики исключений процессора
//! (division by zero, page fault, double fault, ...) и аппаратных
//! прерываний (таймер, клавиатура) через контроллер PIC 8259.

use crate::port::{inb, outb, io_wait};
use crate::{keyboard, println, timer};
use core::arch::asm;
use core::sync::atomic::{AtomicU8, Ordering};

/// Структура кадра стека, которую процессор кладёт сам перед вызовом
/// обработчика прерывания (нужна благодаря ABI "x86-interrupt").
#[repr(C)]
pub struct InterruptStackFrame {
    pub instruction_pointer: u64,
    pub code_segment: u64,
    pub cpu_flags: u64,
    pub stack_pointer: u64,
    pub stack_segment: u64,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct IdtEntry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    type_attr: u8,
    offset_mid: u16,
    offset_high: u32,
    zero: u32,
}

impl IdtEntry {
    const fn missing() -> Self {
        IdtEntry {
            offset_low: 0,
            selector: 0,
            ist: 0,
            type_attr: 0,
            offset_mid: 0,
            offset_high: 0,
            zero: 0,
        }
    }

    fn new(handler: u64, selector: u16, type_attr: u8) -> Self {
        IdtEntry {
            offset_low: (handler & 0xFFFF) as u16,
            selector,
            ist: 0,
            type_attr,
            offset_mid: ((handler >> 16) & 0xFFFF) as u16,
            offset_high: ((handler >> 32) & 0xFFFFFFFF) as u32,
            zero: 0,
        }
    }
}

#[repr(C, packed)]
struct IdtPointer {
    limit: u16,
    base: u64,
}

const IDT_ENTRIES: usize = 256;
static mut IDT: [IdtEntry; IDT_ENTRIES] = [IdtEntry::missing(); IDT_ENTRIES];

// Код сегмента, который наш загрузчик (boot/stage2.asm) положил в GDT
// как второй дескриптор (после null-дескриптора) — это стандартное
// смещение 0x08 для 64-битного code segment.
const KERNEL_CODE_SELECTOR: u16 = 0x08;

// Присутствует + DPL=0 + 64-битный interrupt gate (тип 0xE).
const GATE_PRESENT_INTERRUPT: u8 = 0b1000_1110;

// Смещения аппаратных прерываний после перенастройки PIC (см. remap_pic).
pub const PIC1_OFFSET: u8 = 32;
pub const PIC2_OFFSET: u8 = PIC1_OFFSET + 8;

const TIMER_INTERRUPT_ID: u8 = PIC1_OFFSET;
const KEYBOARD_INTERRUPT_ID: u8 = PIC1_OFFSET + 1;

// PCI-устройства (в т.ч. сетевая карта) сообщают свою IRQ-линию (0-15)
// только во время выполнения, после сканирования шины — поэтому конкретный
// вектор IDT для сетевой карты узнаём и регистрируем динамически в
// register_network_irq(), а не жёстко на этапе компиляции, как таймер и
// клавиатура. NO_IRQ (255) означает "сеть не найдена/не настроена".
const NO_IRQ: u8 = 255;
static NETWORK_IRQ_LINE: AtomicU8 = AtomicU8::new(NO_IRQ);

macro_rules! set_handler {
    ($index:expr, $handler:expr) => {
        unsafe {
            let addr = ($handler as *const ()) as u64;
            IDT[$index] = IdtEntry::new(addr, KERNEL_CODE_SELECTOR, GATE_PRESENT_INTERRUPT);
        }
    };
}

pub fn init() {
    set_handler!(0, divide_by_zero_handler);
    set_handler!(3, breakpoint_handler);
    set_handler!(6, invalid_opcode_handler);
    set_handler!(8, double_fault_handler);
    set_handler!(13, general_protection_fault_handler);
    set_handler!(14, page_fault_handler);

    set_handler!(TIMER_INTERRUPT_ID as usize, timer_interrupt_handler);
    set_handler!(KEYBOARD_INTERRUPT_ID as usize, keyboard_interrupt_handler);

    // IRQ12 всегда фиксирован за PS/2-мышью (в отличие от сетевой карты,
    // чья линия зависит от PCI-слота) — регистрируем настоящий обработчик
    // сразу, не через динамическую систему irq_stub_*.
    set_handler!((PIC1_OFFSET + 12) as usize, mouse_interrupt_handler);

    // Регистрируем обработчики для остальных линий IRQ 2-15, которые может
    // занять произвольное PCI-устройство (в т.ч. сетевая карта) — какая
    // именно линия используется, узнаём только после сканирования шины
    // PCI (см. register_network_irq).
    set_handler!((PIC1_OFFSET + 2) as usize, irq_stub_2);
    set_handler!((PIC1_OFFSET + 3) as usize, irq_stub_3);
    set_handler!((PIC1_OFFSET + 4) as usize, irq_stub_4);
    set_handler!((PIC1_OFFSET + 5) as usize, irq_stub_5);
    set_handler!((PIC1_OFFSET + 6) as usize, irq_stub_6);
    set_handler!((PIC1_OFFSET + 7) as usize, irq_stub_7);
    set_handler!((PIC1_OFFSET + 8) as usize, irq_stub_8);
    set_handler!((PIC1_OFFSET + 9) as usize, irq_stub_9);
    set_handler!((PIC1_OFFSET + 10) as usize, irq_stub_10);
    set_handler!((PIC1_OFFSET + 11) as usize, irq_stub_11);
    set_handler!((PIC1_OFFSET + 13) as usize, irq_stub_13);
    set_handler!((PIC1_OFFSET + 14) as usize, irq_stub_14);
    set_handler!((PIC1_OFFSET + 15) as usize, irq_stub_15);

    unsafe {
        let ptr = IdtPointer {
            limit: (core::mem::size_of::<[IdtEntry; IDT_ENTRIES]>() - 1) as u16,
            base: core::ptr::addr_of!(IDT) as u64,
        };
        asm!("lidt [{}]", in(reg) &ptr, options(readonly, nostack, preserves_flags));
    }

    remap_pic();

    // IRQ12 (PS/2-мышь) всегда на фиксированной линии — размаскируем
    // сразу, в отличие от сетевой карты, чью линию узнаём только после
    // сканирования PCI (см. register_network_irq).
    unmask_irq(12);

    unsafe { asm!("sti") }; // разрешаем аппаратные прерывания
}

/// Вызывается после того, как драйвер сетевой карты (rtl8139::init) узнал
/// свою IRQ-линию из PCI-конфигурации. Размаскирует нужную линию в PIC,
/// чтобы прерывания от карты реально начали доходить до CPU.
pub fn register_network_irq(irq_line: u8) {
    NETWORK_IRQ_LINE.store(irq_line, Ordering::SeqCst);
    unmask_irq(irq_line);
}

/// Размаскирует произвольную линию IRQ (0-15) в контроллере PIC 8259,
/// автоматически заботясь о каскадном входе IRQ2, если линия находится
/// на втором (ведомом) контроллере.
fn unmask_irq(irq_line: u8) {
    const PIC1_DATA: u16 = 0x21;
    const PIC2_DATA: u16 = 0xA1;

    unsafe {
        if irq_line < 8 {
            let mask = inb(PIC1_DATA);
            outb(PIC1_DATA, mask & !(1 << irq_line));
        } else {
            let mask = inb(PIC2_DATA);
            outb(PIC2_DATA, mask & !(1 << (irq_line - 8)));
            // IRQ2 на PIC1 — это каскадный вход от PIC2, его тоже нужно
            // размаскировать, иначе прерывания с PIC2 не дойдут до CPU.
            let mask1 = inb(PIC1_DATA);
            outb(PIC1_DATA, mask1 & !(1 << 2));
        }
    }
}

/// PIC 8259 по умолчанию шлёт IRQ0-15 как векторы 0x08-0x0F/0x70-0x77,
/// что пересекается с исключениями процессора (0-31). Перенастраиваем его
/// на диапазон 32-47, чтобы не было конфликтов.
fn remap_pic() {
    const PIC1_CMD: u16 = 0x20;
    const PIC1_DATA: u16 = 0x21;
    const PIC2_CMD: u16 = 0xA0;
    const PIC2_DATA: u16 = 0xA1;

    unsafe {
        let mask1 = inb(PIC1_DATA);
        let mask2 = inb(PIC2_DATA);

        outb(PIC1_CMD, 0x11);
        io_wait();
        outb(PIC2_CMD, 0x11);
        io_wait();

        outb(PIC1_DATA, PIC1_OFFSET);
        io_wait();
        outb(PIC2_DATA, PIC2_OFFSET);
        io_wait();

        outb(PIC1_DATA, 4); // сообщаем PIC1, что PIC2 висит на IRQ2
        io_wait();
        outb(PIC2_DATA, 2);
        io_wait();

        outb(PIC1_DATA, 0x01); // режим 8086
        io_wait();
        outb(PIC2_DATA, 0x01);
        io_wait();

        // Восстанавливаем маски, но явно включаем таймер (IRQ0) и
        // клавиатуру (IRQ1) — остальное пока не используем.
        outb(PIC1_DATA, mask1 & !0b0000_0011);
        outb(PIC2_DATA, mask2);
    }
}

fn send_eoi(irq: u8) {
    unsafe {
        if irq >= 8 {
            outb(0xA0, 0x20);
        }
        outb(0x20, 0x20);
    }
}

// ---------------- обработчики исключений процессора ----------------

extern "x86-interrupt" fn divide_by_zero_handler(frame: InterruptStackFrame) {
    println!("[EXCEPTION] Division by zero at {:#x}", frame.instruction_pointer);
}

extern "x86-interrupt" fn breakpoint_handler(frame: InterruptStackFrame) {
    println!("[EXCEPTION] Breakpoint at {:#x}", frame.instruction_pointer);
}

extern "x86-interrupt" fn invalid_opcode_handler(frame: InterruptStackFrame) {
    println!("[EXCEPTION] Invalid opcode at {:#x}", frame.instruction_pointer);
    halt_loop();
}

extern "x86-interrupt" fn double_fault_handler(frame: InterruptStackFrame, _error_code: u64) -> ! {
    println!("[FATAL] Double fault at {:#x}", frame.instruction_pointer);
    halt_loop();
}

extern "x86-interrupt" fn general_protection_fault_handler(
    frame: InterruptStackFrame,
    error_code: u64,
) {
    println!(
        "[EXCEPTION] General protection fault (code {:#x}) at {:#x}",
        error_code, frame.instruction_pointer
    );
    halt_loop();
}

extern "x86-interrupt" fn page_fault_handler(frame: InterruptStackFrame, error_code: u64) {
    let cr2: u64;
    unsafe { asm!("mov {}, cr2", out(reg) cr2) };
    println!(
        "[EXCEPTION] Page fault accessing {:#x} (code {:#x}) at {:#x}",
        cr2, error_code, frame.instruction_pointer
    );
    halt_loop();
}

fn halt_loop() -> ! {
    loop {
        unsafe { asm!("hlt") };
    }
}

// ---------------- обработчики аппаратных прерываний ----------------

extern "x86-interrupt" fn timer_interrupt_handler(_frame: InterruptStackFrame) {
    timer::tick();
    send_eoi(0);
}

extern "x86-interrupt" fn keyboard_interrupt_handler(_frame: InterruptStackFrame) {
    let scancode = unsafe { inb(0x60) };
    keyboard::on_scancode(scancode);
    send_eoi(1);
}

extern "x86-interrupt" fn mouse_interrupt_handler(_frame: InterruptStackFrame) {
    let byte = unsafe { inb(0x60) };
    crate::mouse::on_data_byte(byte);
    send_eoi(12);
}

/// Общая логика для всех "неизвестных заранее" линий IRQ (2-15) — сейчас
/// единственный потребитель это сетевая карта, но структура при желании
/// расширяется на другие PCI-устройства (диск, звук и т.д.) точно так же.
fn handle_dynamic_irq(irq: u8) {
    if NETWORK_IRQ_LINE.load(Ordering::Relaxed) == irq {
        crate::rtl8139::on_interrupt();
    }
    send_eoi(irq);
}

macro_rules! irq_stub {
    ($name:ident, $irq:expr) => {
        extern "x86-interrupt" fn $name(_frame: InterruptStackFrame) {
            handle_dynamic_irq($irq);
        }
    };
}

irq_stub!(irq_stub_2, 2);
irq_stub!(irq_stub_3, 3);
irq_stub!(irq_stub_4, 4);
irq_stub!(irq_stub_5, 5);
irq_stub!(irq_stub_6, 6);
irq_stub!(irq_stub_7, 7);
irq_stub!(irq_stub_8, 8);
irq_stub!(irq_stub_9, 9);
irq_stub!(irq_stub_10, 10);
irq_stub!(irq_stub_11, 11);
irq_stub!(irq_stub_12, 12);
irq_stub!(irq_stub_13, 13);
irq_stub!(irq_stub_14, 14);
irq_stub!(irq_stub_15, 15);
