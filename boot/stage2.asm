; stage2.asm — грузится boot_sector'ом по физическому адресу 0x10000.
; Сюда мы попадаем уже в 32-битном защищённом режиме (без BIOS, без GRUB).
; Задача: включить paging + long mode, загрузить 64-битный GDT, в 64-битном
; режиме прочитать kernel.bin с диска (ATA PIO) в память 0x100000 и прыгнуть
; туда (long_mode_start из kernel.bin).
;
; Константы (передаются через nasm -D, см. build.sh):
;   KERNEL_LBA      — стартовый сектор kernel.bin на диске;
;   KERNEL_SECTORS  — размер kernel.bin в секторах.

bits 32

global stage2_start
extern __bss_start
extern __bss_end

%ifndef KERNEL_SRC
KERNEL_SRC equ 0x800000   ; буфер kernel.bin (8 МиБ, ВЫШЕ legacy-дыры и .bss)
; ПОЧЕМУ 0x200000, а не 0x11000: старый буфер упирался в видеопамять VGA
; (0xA0000), из-за чего ядро физически не могло превысить 572 КиБ —
; загрузка обрывалась сразу после stage2. Копирование выполняет ramboot
; уже в 32-битном protected mode, поэтому адрес выше 1 МиБ доступен.
%endif
%ifndef KERNEL_SIZE_DWORDS
KERNEL_SIZE_DWORDS equ 179200   ; 716800 байт = 1400 секторов
%endif

KERNEL_DST equ 0x100000   ; куда копируем kernel.bin (база kernel.ld)

section .text
stage2_start:
    ; Обнуляем секцию .bss ДО первого call/push — там лежат таблицы страниц
    ; и стек загрузчика. Секция .bss физически не хранится в файле образа
    ; (NOBITS), поэтому загрузчик обязан сам обнулить эту память.
    mov edi, __bss_start
    mov ecx, __bss_end
    sub ecx, edi
    xor eax, eax
    cld
    rep stosb

    mov esp, stack_top

    ; Копируем kernel.bin из real-памяти (0x20000, куда его загрузил
    ; boot_sector через int13) в 0x100000 — там его ждёт линкер kernel.ld.
    ; НЕ используем ATA PIO: в этой QEMU-версии прямой доступ к портам
    ; 0x1F0 (READ SECTORS) возвращает ABRT, а BIOS int13 работает.
    mov esi, KERNEL_SRC      ; 0x20000
    mov edi, KERNEL_DST      ; 0x100000
    mov ecx, KERNEL_SIZE_DWORDS
    cld
    rep movsd

    call check_cpuid
    call check_long_mode

    call set_up_page_tables
    call enable_paging

    lgdt [gdt64.pointer]
    jmp gdt64.code_segment:long_mode_entry

    hlt

; ---------- проверки ----------

check_cpuid:
    pushfd
    pop eax
    mov ecx, eax
    xor eax, 1 << 21
    push eax
    popfd
    pushfd
    pop eax
    push ecx
    popfd
    cmp eax, ecx
    je .no_cpuid
    ret
.no_cpuid:
    mov al, "1"
    jmp error

check_long_mode:
    mov eax, 0x80000000
    cpuid
    cmp eax, 0x80000001
    jb .no_long_mode

    mov eax, 0x80000001
    cpuid
    test edx, 1 << 29
    jz .no_long_mode
    ret
.no_long_mode:
    mov al, "2"
    jmp error

; ---------- таблицы страниц ----------

set_up_page_tables:
    ; L4[0] -> L3
    mov eax, page_table_l3
    or eax, 0b11
    mov [page_table_l4], eax

    ; L3[0] -> L2 (нулевой гигабайт)
    mov eax, page_table_l2
    or eax, 0b11
    mov [page_table_l3], eax

    ; L2[0..511] -> 2 МиБ страницы (покрываем первые 1 ГиБ)
    mov ecx, 0
.map_l2:
    mov eax, 0x200000          ; 2 МиБ
    mul ecx
    or eax, 0b10000011         ; present | writable | huge
    mov [page_table_l2 + ecx * 8], eax
    inc ecx
    cmp ecx, 512
    jne .map_l2

    ; GPU MMIO (Bochs VBE framebuffer 0xFC000000-0x100000000, 4-й ГиБ):
    ; L3[3] -> page_table_l2_gpu, заполняем записи 480..511 (32 x 2 МиБ).
    ; Это нужно, чтобы recovery/fastbootd/DSM могли писать пиксели в
    ; framebuffer (без этого — page fault на 0xfd000000).
    mov eax, page_table_l2_gpu
    or eax, 0b11
    mov [page_table_l3 + 24], eax

    ; Записи 2016..2047 (блоки 0xFC000000..0x100000000, 4-й ГиБ):
    ; физический адрес блока = (1536 + idx) << 21, где idx = ecx - 2016.
    mov ecx, 2016
.map_gpu:
    mov eax, ecx
    shl eax, 21               ; ecx * 2 МиБ = 0xFC000000 при ecx=2016
    or eax, 0b10000011
    mov [page_table_l2_gpu + (ecx - 1536) * 8], eax
    inc ecx
    cmp ecx, 2048
    jne .map_gpu
    ret

enable_paging:
    ; PGE (page global) + PAE + PSE
    mov eax, cr4
    or eax, 1 << 7 | 1 << 5 | 1 << 4
    mov cr4, eax

    mov eax, page_table_l4
    mov cr3, eax

    ; EFER.LME (long mode enable)
    mov ecx, 0xC0000080
    rdmsr
    or eax, 1 << 8
    wrmsr

    ; PG (paging enable)
    mov eax, cr0
    or eax, 1 << 31
    mov cr0, eax

    ret

error:
    mov dword [0xb8000], 0x4f524f45
    mov dword [0xb8004], 0x4f3a4f52
    mov dword [0xb8008], 0x4f204f20
    mov byte  [0xb800a], al
    hlt

; ==================== 64-битный режим: читаем kernel.bin ====================

bits 64
long_mode_entry:
    mov ax, 0
    mov ss, ax
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax

    ; Прыгаем в ядро (long_mode_start в kernel.bin по адресу 0x100000).
    ; kernel.bin уже скопирован в 0x100000 (см. copy_kernel в 32-бит части).
    mov rax, KERNEL_DST
    jmp rax

section .bss
align 4096
page_table_l4:
    resb 4096
page_table_l3:
    resb 4096
page_table_l2:
    resb 4096        ; P2 нулевого гигабайта
page_table_l2_gpu:
    resb 4096        ; P2 для GPU MMIO (4-й гигабайт)
stack_bottom:
    resb 4096 * 4
stack_top:

section .rodata
gdt64:
    dq 0
.code_segment: equ $ - gdt64
    dq (1 << 43) | (1 << 44) | (1 << 47) | (1 << 53)
.pointer:
    dw $ - gdt64 - 1
    dq gdt64
