/* linker_kernel.ld — линкует ЯДРО (long_mode_init.asm + Rust) в отдельный
   плоский бинарник kernel.bin с базой 0x100000 (1 МиБ). Его грузит stage2
   (уже в long mode) через ATA PIO с диска.
   .bss ядра (включая HEAP_STORAGE) размещаем на 0x200000 (2 МиБ) — вне
   legacy-дыры 0xA0000-0xFFFFF и выше кода ядра. __image_end используется
   install-командой для копирования ядра на целевой диск. */

ENTRY(long_mode_start)

SECTIONS {
    . = 0x100000;

    .text :
    {
        *(.text .text.*)
    }

    .rodata :
    {
        *(.rodata .rodata.*)
    }

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
    }

    /* Rust .bss (HEAP_STORAGE и др.) — на 2 МиБ, вне legacy-дыры
       0xA0000-0xFFFFF и выше кода ядра.
       ПРОБОВАЛ поднять её выше (8/12/14 МиБ работает, 16 МиБ и выше —
       загрузка обрывается сразу после stage2), чтобы освободить
       0x400000 под статические ELF Linux. Не вышло: причина в раннем
       коде загрузчика, а не в подкачке. Поэтому ELF-загрузчик
       (src/linux/elf.rs) грузит программы ВЫШЕ кучи, а не по их
       «родному» адресу 0x400000 — см. USER_IMAGE_BASE.
       Адрес секции задаётся НАПРЯМУЮ — `. = ...` внутри NOBITS-секции
       не двигает её в GNU ld. */
    .bss 0x200000 :
    {
        __bss_start = .;
        *(.bss .bss.*)
        __bss_end = .;
    }
}
