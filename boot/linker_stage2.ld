/* linker_stage2.ld — линкует ТОЛЬКО загрузчик (stage2.asm) в плоский
   бинарник с базой 0x10000 (туда его грузит boot_sector.bin).
   Задача stage2: настроить paging, перейти в long mode, прочитать
   kernel.bin с диска (ATA PIO) в память 0x100000 и прыгнуть туда.
   Таблицы страниц и стек — в .bss на 0x100000 (вне legacy-дыры). */

ENTRY(stage2_start)

SECTIONS {
    . = 0x10000;

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
        *(.data .data.*)
    }

    /* Таблицы страниц и стек поднимаем на 32 МиБ (0x2000000):
       - ВЫШЕ VGA-дыры 0xA0000-0xFFFFF;
       - ВЫШЕ kernel.bin (грузится в 0x100000, ~0.5 МиБ);
       - ВЫШЕ кучи ядра (HEAP_STORAGE, 0x200000..0x1200000).
       Иначе загружаемый kernel.bin затирал бы таблицы страниц stage2.
       Адрес секции задаётся НАПРЯМУЮ (`.bss 0x2000000 :`) — конструкция
       `. = 0x2000000;` внутри NOBITS-секции не двигает её в GNU ld. */
    .bss 0x2000000 :
    {
        __stage2_bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(8);
        __stage2_bss_end = .;
    }
}
