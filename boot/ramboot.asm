<<<<<<< HEAD
; ramboot.asm — загрузчик ядра из /system (реальный режим, BIOS int 13h).
; Ядро в long mode не может вызывать BIOS, поэтому его читает этот код: по
; карте загрузки (сектор LBA 2047, MBR уже положил её в 0x7000) он читает
; /system/kernel/kernel.bin порциями через int 13h в буфер 0x20000 и
; переносит в 0x100000 из защищённого режима. Затем управление получает
; stage2 (0x10000, 32-бит PM, плоский CS), он включает long mode.
;
; РАЗМЕЩЕНИЕ: код в НИЗКОЙ памяти 0x7E00 (org 0x7E00, DS=0). Плоские 32-битные
; сегменты PM: linear = IP.
;
; Поток загрузки:
;   MBR: карта -> 0x7000, stage2 -> 0x10000, ramboot -> 0x7E00, jmp 0x07E0:0
;   ramboot: ceil(kernel_size/512) секторов по участкам карты (до 40 штук)
;            порциями CHUNK_SECTORS -> буфер 0x20000 -> PM rep movsd -> 0x100000
;            jmp STAGE2_ADDR
;
; Карта загрузки (DEIXKIMG v2, docs/BOOTMAP.md): размер ядра u32 @12,
; таблица участков ядра @192 (LBA u32, секторов u32; секторов = 0 — конец).
=======
; ramboot.asm — RAM-ДИСК: дочитывает ВЕСЬ дисковый образ в высокую память.
; Зачем: ядро в long mode не может вызывать BIOS int13 и читает разделы через
; ATA PIO (порты 0x1F0) — это работает для внутреннего SATA, но НЕ для USB /
; Ventoy (виртуальный диск). Поэтому до перехода в long mode мы читаем весь
; образ (20 МиБ) через int13 и кладём его в RAM по адресу RAMDISK_DST, а ядро
; обращается к разделам как к памяти (см. src/ramdisk.rs).
;
; РАЗМЕЩЕНИЕ: код в НИЗКОЙ памяти 0x9000 (org 0x7E00, DS=0). Это позволяет
; использовать ПЛОСКИЕ 32-битные сегменты в protected mode (linear = IP),
; т.к. ramboot < 1 МиБ и адреса влезают в imm16.
;
; Поток загрузки (MBR):
;   MBR: stage2 -> 0x10000, ramboot -> 0x9000
;   MBR: jmp 0x500:0 (real mode)
;   ramboot: читает RAMDISK_SECTORS секторов с LBA 0 порциями по 64 сектора
;            в буфер 0x11000, переключается в 32-битный PM, rep movsd в
;            RAMDISK_DST (0x2000000), возвращается в RM, повторяет;
;            затем в PM копирует kernel.bin (KERNEL_SECTORS сект) из
;            RAMDISK_DST + KERNEL_LBA*512 в память ядра;
;            jmp STAGE2_ADDR (0x10000) — stage2 (32-бит PM, плоский).
;
; Константы (nasm -D, см. build.sh):
;   RAMDISK_SECTORS — сколько секторов образа читать в RAM (весь .img);
;   KERNEL_SECTORS  — сколько секторов kernel.bin копировать;
;   KERNEL_LBA      — стартовый сектор kernel.bin внутри образа
;                     (= 1 + секторы stage2 + 1 сектор ramboot);
;   RAMDISK_DST     — куда копировать RAM-диск (0x2000000 = 32 МиБ);
;   STAGE2_ADDR     — куда прыгнуть (0x10000).
>>>>>>> arena/e8a29038-deix

bits 16
org 0x7E00

<<<<<<< HEAD
%ifndef STAGE2_ADDR
STAGE2_ADDR equ 0x10000
%endif
KERNEL_DST    equ 0x100000        ; база ядра (linker_kernel.ld)
MAX_KERNEL_SECTORS equ 4096       ; защита от испорченной карты (2 МиБ)

BOOTMAP       equ 0x7000          ; карта загрузки, её кладёт MBR
KERNEL_EXTS   equ BOOTMAP + 192
KERNEL_EXT_COUNT equ 40

; 120 секторов за вызов int13: меньше переходов RM<->PM. Буфер
; 0x20000..0x2F000 (120*512 = 0xF000) не пересекает границу сегмента, лежит
; выше stage2 (0x10000) и ниже EBDA; 120 < 127 — лимит у части BIOS.
CHUNK_SECTORS equ 120
BUF_SEG       equ 0x2000
BUF_ADDR      equ 0x20000
=======
%ifndef RAMDISK_SECTORS
RAMDISK_SECTORS equ 40960
%endif
%ifndef KERNEL_SECTORS
KERNEL_SECTORS equ 2048
%endif
%ifndef RAMDISK_DST
RAMDISK_DST equ 0x2000000
%endif
%ifndef STAGE2_ADDR
STAGE2_ADDR equ 0x10000
%endif
%ifndef KERNEL_LBA
KERNEL_LBA equ 3
%endif
%ifndef KERNEL_SRC
KERNEL_SRC equ 0x800000   ; см. пояснение в boot/stage2.asm
%endif

; 120 секторов за вызов int13 (вместо прежних 64): для 20-МиБ образа это
; 342 чтения вместо 640 — каждое чтение сопровождается переходом
; RM→PM→RM, так что загрузка заметно быстрее. Буфер 0x11000..0x20000
; (120*512 = 0xF000) не пересекает границу сегмента и остаётся ниже
; стека 0x9FC00 и EBDA; 120 < 127 — лимит на передачу у части BIOS.
CHUNK_SECTORS equ 120
BUF_SEG       equ 0x1100          ; буфер чтения 0x11000
>>>>>>> arena/e8a29038-deix

start:
    cli
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ax, 0x9000
    mov ss, ax
    mov sp, 0xFC00
    ; (стек 0x9000:0xFC00 = 0x9FC00 — ниже EBDA, не пересекает ramboot в 0x5000)

    ; DL приходит из MBR — это НАСТОЯЩИЙ номер загрузочного диска от
    ; BIOS. На флешке он может быть 0x80, 0x81, 0x82 и выше; жёсткое
    ; 0x80 работало только потому, что в QEMU образ всегда первый диск.
    ; Значение 0 означает дискету — такого у нас быть не может,
    ; подстраховываемся значением 0x80.
    test dl, 0x80
    jnz .drive_ok
    mov dl, 0x80
.drive_ok:
    mov [boot_drive], dl

    ; A20 до копирования в >1 МиБ (иначе адрес заворачивается).
    in al, 0x92
    or al, 2
    out 0x92, al

<<<<<<< HEAD
    mov dword [ram_dst_phys], KERNEL_DST
    mov eax, [BOOTMAP + 12]         ; размер ядра в байтах
    add eax, 511
    shr eax, 9
    jz map_fail
    cmp eax, MAX_KERNEL_SECTORS
    ja map_fail
    mov [total_left], ax
    mov word [ext_ptr], KERNEL_EXTS
    mov word [ext_cnt], KERNEL_EXT_COUNT

.next_ext:
    mov si, [ext_ptr]
    cmp word [si + 6], 0
    jne map_fail
    mov ax, [si + 4]
    test ax, ax
    jz map_fail                     ; участки кончились, а ядро — нет
    mov bx, [total_left]
    cmp ax, bx
    jbe .ext_len
    mov ax, bx
.ext_len:
    mov [sectors_left], ax
    sub [total_left], ax
    mov ax, [si]
    mov [cur_lba_lo], ax
    mov ax, [si + 2]
    mov [cur_lba_hi], ax
    add word [ext_ptr], 8
    dec word [ext_cnt]
=======
    mov dword [ram_dst_phys], RAMDISK_DST
    mov word [sectors_left], RAMDISK_SECTORS
    mov word [cur_lba_lo], 0
    mov word [cur_lba_hi], 0
>>>>>>> arena/e8a29038-deix

.load_loop:
    cmp word [sectors_left], 0
    je .load_done

    mov ax, [sectors_left]
    cmp ax, CHUNK_SECTORS
    jbe .use_remaining
    mov ax, CHUNK_SECTORS
.use_remaining:
    mov [dap_count], ax

    mov word [dap_segment], BUF_SEG
    mov ax, [cur_lba_lo]
    mov [dap_lba_lo], ax
    mov ax, [cur_lba_hi]
    mov [dap_lba_lo + 2], ax

    mov ah, 0x42
    mov dl, [boot_drive]
    mov si, dap
    int 0x13
    jnc .read_ok
<<<<<<< HEAD
    ; Ошибка чтения: другой диск не пробуем (можно прочитать чужой носитель
    ; как ядро DeiX) — печатаем причину и останавливаемся.
=======
    ; ОШИБКА ЧТЕНИЯ. Раньше здесь был retry с диском 0x81 — это опасно:
    ; при сбое мы читали ЧУЖОЙ носитель (у пользователя там Windows) и
    ; складывали его содержимое в RAM как образ DeiX. Ядро потом
    ; сообщало, что все разделы "не читается". Теперь честно
    ; останавливаемся и печатаем букву 'R' в COM1 и на экран.
>>>>>>> arena/e8a29038-deix
    jmp disk_error
.read_ok:
    ; int13 (BIOS) может оставить DS/ES в другом состоянии — сбрасываем.
    xor ax, ax
    mov ds, ax
    mov es, ax

    ; копируем чанк в RAM-диск (PM, плоские сегменты: linear = IP)
    mov eax, [dap_count]
    shl eax, 9
    mov [chunk_bytes], eax

    cli
    lgdt [gdt_descriptor]
    mov eax, cr0
    or eax, 1
    mov cr0, eax
    ; СРАЗУ far jmp (между cr0 и jmp не должно быть инструкций: CS=0 после
    ; cr0=1 — NULL в PM, чтение кода через него даёт #GP).
    jmp CODE_SEG:.pm

.pm:
    bits 32
    mov ax, DATA_SEG
    mov ds, ax
    mov es, ax
    mov ax, DATA_SEG
    mov ss, ax
    mov esp, 0x90000
    cld
<<<<<<< HEAD
    mov esi, BUF_ADDR         ; буфер (absolute)
    mov edi, [ram_dst_phys]   ; ядро (absolute)
=======
    mov esi, 0x11000          ; буфер (absolute)
    mov edi, [ram_dst_phys]   ; RAM-диск (absolute)
>>>>>>> arena/e8a29038-deix
    mov ecx, [chunk_bytes]
    shr ecx, 2
    rep movsd

    mov eax, [chunk_bytes]
    add [ram_dst_phys], eax

    ; ---- ВЫХОД из PM в real mode (классическая схема) ----
    ; 1) far jmp на 16-битный код-сегмент (D=0): после него CPU в 16-битном
    jmp CODE16_SEG:.b16
bits 16
.b16:
    ; 2) 16-битные сегменты данных
    mov ax, DATA16_SEG
    mov ds, ax
    mov es, ax
    mov ax, DATA16_SEG
    mov ss, ax
    ; 3) сброс PE
    mov eax, cr0
    and al, 0xFE
    mov cr0, eax
    ; 4) far jmp на реальный сегмент (CS=0x07E0), IP = смещение от 0x7E00:
    ;    linear = 0x07E0<<4 + off = 0x7E00+off
    jmp 0x07E0:(.rm - 0x7E00)
.rm:
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ax, 0x9000
    mov ss, ax
    mov sp, 0xFC00

    mov ax, [dap_count]
    sub [sectors_left], ax
    add [cur_lba_lo], ax
    adc word [cur_lba_hi], 0

    jmp .load_loop
.load_done:
<<<<<<< HEAD
    cmp word [total_left], 0
    je .kernel_loaded
    cmp word [ext_cnt], 0
    jne .next_ext
    jmp map_fail
.kernel_loaded:
=======

    ; ---- копируем kernel.bin из RAM-диска (LBA KERNEL_LBA) в 0x11000 ----
>>>>>>> arena/e8a29038-deix
    cli
    lgdt [gdt_descriptor]
    mov eax, cr0
    or eax, 1
    mov cr0, eax
<<<<<<< HEAD
    jmp CODE_SEG:pm_go

bits 32
pm_go:
    mov ax, DATA_SEG
    mov ds, ax
    mov es, ax
    jmp STAGE2_ADDR

bits 16
=======
    jmp CODE_SEG:pm_copy_kernel

bits 32
pm_copy_kernel:
    mov ax, DATA_SEG
    mov ds, ax
    mov es, ax
    mov esi, RAMDISK_DST + (KERNEL_LBA * 512)
    mov edi, 0x100000         ; KERNEL_DST (1 МиБ)
    mov ecx, KERNEL_SECTORS * 512 / 4
    rep movsd

    ; ---- jmp stage2 (0x10000, 32-бит PM, плоский CS) ----
    jmp STAGE2_ADDR

>>>>>>> arena/e8a29038-deix
; ---------------- GDT (плоские сегменты, base 0) ----------------
align 8
gdt_start:
    dq 0
gdt_code:
    dw 0xFFFF
    dw 0
    db 0
    db 10011010b
    db 11001111b
    db 0
gdt_data:
    dw 0xFFFF
    dw 0
    db 0
    db 10010010b
    db 11001111b
    db 0
gdt_code16:
    dw 0xFFFF
    dw 0
    db 0
    db 10011010b
    db 00001111b
    db 0
gdt_data16:
    dw 0xFFFF
    dw 0
    db 0
    db 10010010b
    db 00001111b
    db 0
gdt_end:
gdt_descriptor:
    dw gdt_end - gdt_start - 1
    dd gdt_start

CODE_SEG equ gdt_code - gdt_start      ; 0x08 code32
DATA_SEG equ gdt_data - gdt_start      ; 0x10 data32
CODE16_SEG equ gdt_code16 - gdt_start  ; 0x18 code16
DATA16_SEG equ gdt_data16 - gdt_start  ; 0x20 data16

; ---------------- данные (DS=0, адреса 0x9000+off) ----------------
boot_drive:    db 0
<<<<<<< HEAD
=======
hexdig:        db "0123456789ABCDEF"
>>>>>>> arena/e8a29038-deix
sectors_left:  dw 0
cur_lba_lo:    dw 0
cur_lba_hi:    dw 0
ram_dst_phys:  dd 0
chunk_bytes:   dd 0
<<<<<<< HEAD
total_left:    dw 0
ext_ptr:       dw 0
ext_cnt:       dw 0

map_fail:
    mov si, msg_map_err
    jmp disk_error.err_loop
disk_error:
    ; Причина — на экран (BIOS teletype) и в COM1 (его настроил MBR).
=======

disk_error:
    ; Раньше здесь был молчаливый вечный цикл: на реальном железе это
    ; выглядело как зависание без единого признака причины.
    ; Печатаем "RAMBOOT: read error" через BIOS teletype (int 10h) и
    ; дублируем в COM1 — так видно, что упало именно чтение образа.
>>>>>>> arena/e8a29038-deix
    mov si, msg_read_err
.err_loop:
    lodsb
    test al, al
    jz .err_hang
<<<<<<< HEAD
    mov ah, 0x0E
    mov bx, 0x0007
    int 0x10
=======
    ; экран
    mov ah, 0x0E
    mov bx, 0x0007
    int 0x10
    ; COM1 (порт уже настроен MBR)
>>>>>>> arena/e8a29038-deix
    mov dx, 0x3F8
    out dx, al
    jmp .err_loop
.err_hang:
    jmp $

msg_read_err: db 13, 10, "RAMBOOT: disk read error", 13, 10, 0
<<<<<<< HEAD
msg_map_err:  db 13, 10, "RAMBOOT: bad boot map", 13, 10, 0
=======
>>>>>>> arena/e8a29038-deix

; ---------------- Disk Address Packet ----------------
align 4
dap:
    db 0x10, 0
dap_count:
    dw 0
    dw 0x0000
dap_segment:
    dw 0
dap_lba_lo:
    dw 0, 0
dap_lba_hi:
    dw 0, 0

<<<<<<< HEAD
align 512, db 0
=======
times 510-($-$$) db 0
dw 0xAA55
>>>>>>> arena/e8a29038-deix
