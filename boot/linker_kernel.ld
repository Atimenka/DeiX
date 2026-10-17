/* linker_kernel.ld — линкует ЯДРО (long_mode_init.asm + Rust) в отдельный
   плоский бинарник kernel.bin с базой 0x100000 (1 МиБ). Его грузит stage2
   (уже в long mode) через ATA PIO с диска.
   .bss ядра (включая HEAP_STORAGE) размещаем на 0x300000 (3 МиБ) — вне
   legacy-дыры 0xA0000-0xFFFFF и выше кода ядра. __image_end используется
   install-командой для копирования ядра на целевой диск. */

ENTRY(long_mode_start)

/* Явные программные заголовки с нормальными правами (text R+X,
   rodata R, data/bss R+W). Без них ld сливает всё в один RWX-сегмент
   и binutils >= 2.39 предупреждает "LOAD segment with RWX permissions".
   На kernel.bin это не влияет: objcopy -O binary раскладывает байты по
   адресам секций, заголовки сегментов в плоский образ не попадают. */
PHDRS {
    text   PT_LOAD FLAGS(5);
    rodata PT_LOAD FLAGS(4);
    data   PT_LOAD FLAGS(6);
    bss    PT_LOAD FLAGS(6);
}

SECTIONS {
    . = 0x100000;

    .text :
    {
        *(.text .text.*)
    } :text

    .rodata :
    {
        *(.rodata .rodata.*)
    } :rodata

    .data :
    {
        __data_start = .;
        *(.data .data.*)
        /* .got/.got.plt — PROGBITS-секции после .data: objcopy -O binary
           включает их в kernel.bin. Если __image_end стоит до них, install
           копирует kernel на несколько байт короче (хвост теряется ->
           установленная система падает Invalid opcode). */
        *(.got .got.*)
        *(.got.plt .got.plt.*)
        . = ALIGN(512);
        __image_end = .;
    } :data

    /* Rust .bss (HEAP_STORAGE и др.) — на 3 МиБ, вне legacy-дыры
       0xA0000-0xFFFFF и выше кода ядра. Раньше стояла на 2 МиБ, но
       .rodata (встроенные корневые сертификаты и таблицы TLS) переросла
       окно 0x100000..0x200000, и ld падал с overlap — поэтому 3 МиБ.
       Конец секции (~0x1360000) обязан оставаться НИЖЕ Ring 3
       (0x1400000, см. src/usermode.rs).
       ПРОБОВАЛ поднять её ещё выше (8/12/14 МиБ работает, 16 МиБ и
       выше — загрузка обрывается сразу после stage2), чтобы освободить
       0x400000 под статические ELF Linux. Не вышло: причина в раннем
       коде загрузчика, а не в подкачке. Поэтому ELF-загрузчик
       (src/linux/elf.rs) грузит программы ВЫШЕ кучи, а не по их
       «родному» адресу 0x400000 — см. USER_IMAGE_BASE.
       Адрес секции задаётся НАПРЯМУЮ — `. = ...` внутри NOBITS-секции
       не двигает её в GNU ld. */
    .bss 0x300000 :
    {
        __kernel_bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(8);
        __kernel_bss_end = .;
    } :bss
}
