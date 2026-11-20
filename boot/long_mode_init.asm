; long_mode_init.asm — маленький 64-битный "трамплин" перед вызовом Rust-ядра.
; Входит в состав kernel.bin (линкуется с ядром, база 0x100000).

global long_mode_start
extern kernel_main
extern __kernel_bss_start
extern __kernel_bss_end

section .text
bits 64
long_mode_start:
    ; обнуляем сегментные регистры данных (в long mode они почти не используются,
    ; но обнулить их — хороший тон)
    mov ax, 0
    mov ss, ax
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax

    ; Обнуляем .bss ЯДРА (HEAP_STORAGE и пр.) — секция NOBITS не хранится
    ; в kernel.bin, а stage2 обнуляет только СВОЙ .bss (таблицы страниц и
    ; стек). Без этого шага куча ядра содержит мусор от прошлой загрузки.
    mov rdi, __kernel_bss_start
    mov rcx, __kernel_bss_end
    cmp rcx, rdi
    jbe .bss_done
    sub rcx, rdi
    shr rcx, 3
    xor rax, rax
    cld
    rep stosq
.bss_done:
    ; Гарантируем IF=0 перед ядром: аппаратные прерывания (таймер, IRQ14
    ; от IDE/CD) НЕ должны приходить до установки IDT в interrupts::init().
    cli

    call kernel_main

    hlt
.hang:
    jmp .hang
