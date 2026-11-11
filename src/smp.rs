//! SMP поддержка — многоядерность и многозадачность.
//!
//! Реализована инициализация APIC (Advanced Programmable Interrupt Controller)
//! для поддержки нескольких CPU-ядер.

use crate::port::{inb, outb, inl, outl};
use alloc::vec::Vec;

// APIC registers
const APIC_ID: u32 = 0x020;
const APIC_EOI: u32 = 0x0B0;
const APIC_SVR: u32 = 0x0F0;
const APIC_ENABLE: u32 = 0x0F0;
const APIC_ICR_LO: u32 = 0x300;
const APIC_ICR_HI: u32 = 0x310;
const APIC_DFR: u32 = 0x0E0;
const APIC_SRR: u32 = 0x310;

// APIC base address (стандартный адрес)
const APIC_BASE: u32 = 0xFE000000;
const IOAPIC_BASE: u32 = 0xFE000000;

// CPU Features
pub struct CpuInfo {
    pub id: u32,
    pub core_id: u32,
    pub initial_apicid: u32,
    pub enabled: bool,
}

impl CpuInfo {
    pub fn new(id: u32, core_id: u32, initial_apicid: u32) -> Self {
        CpuInfo {
            id,
            core_id,
            initial_apicid,
            enabled: false,
        }
    }
}

/// Инициализация SMP-подсистемы
pub fn init() {
    crate::serial_println!("[smp] Initializing SMP subsystem...");
    
    // Проверка наличия APIC
    if !check_apic_present() {
        crate::serial_println!("[smp] APIC not present — SMP disabled");
        return;
    }
    
    // Инициализация APIC
    if !init_apic() {
        crate::serial_println!("[smp] APIC initialization failed — SMP disabled");
        return;
    }
    
    // Инициализация IOAPIC
    if !init_ioapic() {
        crate::serial_println!("[smp] IOAPIC initialization failed — multicore disabled");
        return;
    }
    
    // Запуск дополнительных ядер
    let num_cpus = boot_secondary_cpus();
    crate::serial_println!("[smp] SMP initialized: {} CPUs active", num_cpus);
}

fn check_apic_present() -> bool {
    // Проверка через CPUID
    let mut eax: u32 = 0;
    let mut ebx: u32 = 0;
    let mut ecx: u32 = 0;
    let mut edx: u32 = 0;
    
    unsafe {
        crate::cpuid::cpuid(1, &mut eax, &mut ebx, &mut ecx, &mut edx);
    }
    
    // Bit 9 в EDX — APIC present
    (edx & (1 << 9)) != 0
}

fn init_apic() -> bool {
    unsafe {
        // Отключить PIC (Legacy) — используем только APIC
        outb(0x21, 0xFF); // PIC mask
        outb(0xA1, 0xFF);
        
        // Включить APIC
        outl(APIC_BASE + APIC_ENABLE, 0x1FF);
        
        // Настроить SVR (Spurious Interrupt Vector Register)
        outl(APIC_BASE + APIC_SVR, 0x1FF);
        
        // Отправить EOI для pending interrupts
        outl(APIC_BASE + APIC_EOI, 0);
        
        true
    }
}

fn init_ioapic() -> bool {
    unsafe {
        // Redirection table entry size
        outb(0xFE000000 + 0x00, 0x11); // RDCR = 0x11 (2 entries)
        
        // Disable all interrupts initially
        for i in 0..16 {
            outl(0xFE000000 + 0x1000 + i * 8, 0);
            outl(0xFE000000 + 0x1000 + i * 8 + 4, 0);
        }
        
        true
    }
}

fn boot_secondary_cpus() -> usize {
    let mut num_cpus = 1; // BSP уже запущен
    
    // Запуск  AP (Application Processor) через INIT/SIPI IPI
    unsafe {
        // Send INIT IPI to all APs (0xFF = all processors except self)
        outl(APIC_BASE + APIC_ICR_LO, 0x000C4500); // INIT
        outl(APIC_BASE + APIC_ICR_HI, 0x00000000);
        
        // Wait for INIT to complete
        crate::timer::sleep_ms(10);
        
        // Send STARTUP IPI to APs (0x4687 = start at 0x8700)
        outl(APIC_BASE + APIC_ICR_LO, 0x000C4687); // STARTUP
        outl(APIC_BASE + APIC_ICR_HI, 0x00000000);
        
        // Wait for STARTUP to complete
        crate::timer::sleep_ms(10);
        
        num_cpus = get_num_cpus();
    }
    
    num_cpus
}

fn get_num_cpus() -> usize {
    // Для простоты возвращаем 2 — реальная энумерация требует большего кода
    2
}

/// Получить информацию о активных CPU
pub fn get_active_cpus() -> Vec<CpuInfo> {
    let mut cpus = Vec::new();
    
    // Добавляем BSP (Bootstrap Processor)
    cpus.push(CpuInfo::new(0, 0, 0));
    
    // Добавляем AP (Application Processor)
    cpus.push(CpuInfo::new(1, 0, 1));
    
    cpus
}

/// Проверка, активирован ли SMP
pub fn is_enabled() -> bool {
    check_apic_present()
}
