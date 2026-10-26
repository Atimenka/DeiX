//! Обёртки над инструкциями in/out для работы с портами ввода-вывода x86.

#[inline]
pub unsafe fn outb(port: u16, value: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
}

#[inline]
pub unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    core::arch::asm!("in al, dx", out("al") value, in("dx") port, options(nomem, nostack, preserves_flags));
    value
}

#[inline]
pub unsafe fn outw(port: u16, value: u16) {
    core::arch::asm!("out dx, ax", in("dx") port, in("ax") value, options(nomem, nostack, preserves_flags));
}

#[inline]
pub unsafe fn inw(port: u16) -> u16 {
    let value: u16;
    core::arch::asm!("in ax, dx", out("ax") value, in("dx") port, options(nomem, nostack, preserves_flags));
    value
}

#[inline]
pub unsafe fn outl(port: u16, value: u32) {
    core::arch::asm!("out dx, eax", in("dx") port, in("eax") value, options(nomem, nostack, preserves_flags));
}

#[inline]
pub unsafe fn inl(port: u16) -> u32 {
    let value: u32;
    core::arch::asm!("in eax, dx", out("eax") value, in("dx") port, options(nomem, nostack, preserves_flags));
    value
}

/// Читает buf.len()/2 слов (16 бит) подряд из порта в буфер — используется
/// для быстрого чтения сектора диска через ATA PIO (256 слов = 512 байт).
///
/// Одна инструкция `rep insw` вместо цикла из `inw`: в эмуляторе каждый
/// обмен через порт обходится дорого, а повторяющаяся инструкция
/// обрабатывается внутри одного вызова. Слова пишутся в память в порядке
/// little-endian, как и в прежней побайтовой версии.
#[inline]
pub unsafe fn insw(port: u16, buf: &mut [u8]) {
    let words = buf.len() / 2;
    if words == 0 {
        return;
    }
    core::arch::asm!(
        "rep insw",
        inout("rcx") words => _,
        inout("rdi") buf.as_mut_ptr() => _,
        in("dx") port,
        options(nostack, preserves_flags),
    );
}

/// Пишет buf.len()/2 слов (16 бит) подряд в порт — парная функция к insw.
#[inline]
pub unsafe fn outsw(port: u16, buf: &[u8]) {
    let words = buf.len() / 2;
    if words == 0 {
        return;
    }
    core::arch::asm!(
        "rep outsw",
        inout("rcx") words => _,
        inout("rsi") buf.as_ptr() => _,
        in("dx") port,
        options(nostack, readonly, preserves_flags),
    );
}

#[inline]
pub unsafe fn io_wait() {
    // Запись в неиспользуемый порт 0x80 — стандартный трюк для короткой
    // задержки, чтобы старое железо успевало обработать предыдущую команду.
    outb(0x80, 0);
}
