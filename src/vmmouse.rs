//! Драйвер "VMware backdoor" для абсолютного позиционирования курсора мыши.
//!
//! Это официально задокументированный (хоть и не входящий в архитектуру
//! x86 как таковую) механизм: специальная последовательность регистров +
//! инструкция `in` на "магический" порт 0x5658, которую QEMU (и VMware)
//! перехватывают программно и подставляют туда своё собственное значение
//! регистров вместо настоящего чтения из порта. Именно так Bochs Display
//! Interface (см. vbe.rs) и множество других гостевых "паравиртуальных"
//! интерфейсов работают в QEMU "из коробки", без каких-либо дополнительных
//! флагов командной строки — устройства `vmmouse`/`vmport` присутствуют на
//! стандартной машине `pc` по умолчанию.
//!
//! Благодаря этому забирать координаты курсора можно не в виде относительных
//! смещений (как обычная PS/2 мышь), а сразу в абсолютных координатах экрана
//! хоста (масштабированных в диапазон 0..0xFFFF) — то есть курсор нашей ОС
//! будет один-в-один совпадать с курсором мыши хоста, без "захвата" окна
//! QEMU и без рассинхронизации.
//!
//! Референс: wiki.osdev.org/VMware_tools, исходники ToaruOS
//! (modules/vmware.c) и qemu/hw/i386/vmmouse.c.

const VMWARE_MAGIC: u32 = 0x564D_5868; // "hXMV" задом наперёд как ASCII
const VMWARE_PORT: u32 = 0x5658;

const CMD_GETVERSION: u32 = 10;
const CMD_ABSPOINTER_DATA: u32 = 39;
const CMD_ABSPOINTER_STATUS: u32 = 40;
const CMD_ABSPOINTER_COMMAND: u32 = 41;

const ABSPOINTER_ENABLE: u32 = 0x4541_4552; // "QEAE"
const ABSPOINTER_RELATIVE: u32 = 0xF5;
const ABSPOINTER_ABSOLUTE: u32 = 0x5342_4152; // "RABS"

/// Выполняет один "вызов" бэкдора: кладём значения в eax/ebx/ecx/edx и
/// выполняем `in eax, dx` — QEMU перехватывает эту инструкцию (это не
/// настоящий физический порт ввода-вывода) и подменяет содержимое всех
/// четырёх регистров результатом работы эмулируемого устройства.
///
/// RBX зарезервирован самим LLVM (используется как служебный регистр в
/// сгенерированном коде) и не может напрямую участвовать в inline asm как
/// input/output операнд — поэтому явно сохраняем/восстанавливаем его на
/// стеке вокруг самой инструкции `in`, используя временный регистр в
/// качестве переносчика значения.
#[inline]
unsafe fn backdoor_call(eax_in: u32, ebx_in: u32, ecx_in: u32, edx_in: u32) -> (u32, u32, u32, u32) {
    let (mut eax, mut ebx, mut ecx, mut edx) = (eax_in, ebx_in, ecx_in, edx_in);
    core::arch::asm!(
        "push rbx",
        "mov ebx, {ebx_tmp:e}",
        "in eax, dx",
        "mov {ebx_tmp:e}, ebx",
        "pop rbx",
        ebx_tmp = inout(reg) ebx,
        inout("eax") eax,
        inout("ecx") ecx,
        inout("edx") edx,
        options(nostack, preserves_flags)
    );
    (eax, ebx, ecx, edx)
}

fn call(command: u32, bx: u32) -> (u32, u32, u32, u32) {
    unsafe { backdoor_call(VMWARE_MAGIC, bx, command, VMWARE_PORT) }
}

/// Проверяет, что мы вообще внутри QEMU/VMware с рабочим бэкдором — читаем
/// заведомо безопасную команду CMD_GETVERSION и проверяем, что регистр EBX
/// вернулся с тем же магическим значением (на настоящем железе чтение из
/// неопределённого порта просто ничего не подтвердит, ошибок не будет).
pub fn is_present() -> bool {
    let (eax, ebx, _ecx, _edx) = call(CMD_GETVERSION, !VMWARE_MAGIC);
    ebx == VMWARE_MAGIC && eax != 0xFFFF_FFFF
}

/// Включает абсолютный режим позиционирования курсора (4 вызова бэкдора,
/// как того требует протокол — см. osdev wiki).
pub fn enable_absolute() {
    call(CMD_ABSPOINTER_COMMAND, ABSPOINTER_ENABLE);
    call(CMD_ABSPOINTER_STATUS, 0);
    call(CMD_ABSPOINTER_DATA, 1);
    call(CMD_ABSPOINTER_COMMAND, ABSPOINTER_ABSOLUTE);
}

/// Возвращает мышь в обычный относительный режим (PS/2-совместимый).
#[allow(dead_code)]
pub fn disable_absolute() {
    call(CMD_ABSPOINTER_COMMAND, ABSPOINTER_RELATIVE);
}

pub struct AbsPacket {
    pub buttons: u32,
    /// Координаты в диапазоне 0..=0xFFFF независимо от реального
    /// разрешения — нужно самим смасштабировать под текущий экран.
    pub x: u32,
    pub y: u32,
}

pub const LEFT_BUTTON: u32 = 0x20;
pub const RIGHT_BUTTON: u32 = 0x10;
pub const MIDDLE_BUTTON: u32 = 0x08;

/// Опрашивает бэкдор на предмет новых данных о позиции/кнопках мыши.
/// Вызывается из обработчика прерывания IRQ12 (см. mouse.rs) вместо
/// разбора обычного 3-байтового PS/2 пакета — сам байт от PS/2
/// контроллера в абсолютном режиме несёт только сигнал "есть новые
/// данные", реальные координаты нужно доставать именно отсюда.
pub fn poll() -> Option<AbsPacket> {
    let (status, _b, _c, _d) = call(CMD_ABSPOINTER_STATUS, 0);

    if status == 0xFFFF_0000 {
        // Ошибка — переинициализируем бэкдор и попробуем в следующий раз.
        enable_absolute();
        return None;
    }
    if (status & 0xFFFF) < 4 {
        return None;
    }

    let (eax, ebx, ecx, _edx) = call(CMD_ABSPOINTER_DATA, 4);
    let buttons = eax & 0xFFFF;
    let x = ebx;
    let y = ecx;

    Some(AbsPacket { buttons, x, y })
}
