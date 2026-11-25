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
;   ramboot, ФАЗА 1: ceil(kernel_size/512) секторов по участкам карты
;            (до 40 штук) порциями CHUNK_SECTORS -> буфер 0x20000 ->
;            PM rep movsd -> 0x100000
;   ramboot, ФАЗА 2: ВЕСЬ образ диска (RAMDISK_SECTORS = 69632 секторов
;            подряд из LBA 0 boot-диска) тем же циклом int13h AH=42h
;            -> буфер 0x20000 -> PM rep movsd -> 0x4000000 (RAM-диск).
;            Ядро потом читает все разделы из памяти, если аппаратный
;            ATA-диск не отвечает (USB/Ventoy) — см. ramdisk::init().
;            jmp STAGE2_ADDR
;
; Карта загрузки (DEIXKIMG v2, docs/BOOTMAP.md): размер ядра u32 @12,
; таблица участков ядра @192 (LBA u32, секторов u32; секторов = 0 — конец).

bits 16
org 0x7E00

%ifndef STAGE2_ADDR
STAGE2_ADDR equ 0x10000
%endif
KERNEL_DST    equ 0x100000        ; база ядра (linker_kernel.ld)
MAX_KERNEL_SECTORS equ 4096       ; защита от испорченной карты (2 МиБ)

; ---- RAM-ДИСК: весь образ диска (34 МиБ) ----
; ramboot читает ЛЮБОЙ загрузочный диск (флешку/Ventoy/SATA) не только
; ядро, но и ПОЛНЫЙ образ диска по LBA подряд из LBA 0 в 0x4000000. Это
; то, что делает ISO-стаб (boot_sector_iso.asm): ядро потом читает все
; разделы из памяти, без аппаратного ATA-контроллера (которого нет у
; флешки). Для обычного диска (QEMU/SATA) ядро всё равно предпочтёт
; ATA, если контроллер отвечает и LBA 0 совпадает с RAM-копией
; (см. ramdisk::init) — образ в RAM там только страховка.
RAMDISK_BASE    equ 0x4000000     ; RAM-диск: весь образ диска
RAMDISK_SECTORS equ 69632         ; 34 МиБ — окно RAM-диска (build.sh)

BOOTMAP       equ 0x7000          ; карта загрузки, её кладёт MBR
KERNEL_EXTS   equ BOOTMAP + 192
KERNEL_EXT_COUNT equ 40

; 120 секторов за вызов int13: меньше переходов RM<->PM. Буфер
; 0x20000..0x2F000 (120*512 = 0xF000) не пересекает границу сегмента, лежит
; выше stage2 (0x10000) и ниже EBDA; 120 < 127 — лимит у части BIOS.
CHUNK_SECTORS equ 120
BUF_SEG       equ 0x2000
BUF_ADDR      equ 0x20000

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

    mov dword [ram_dst_phys], KERNEL_DST
    mov eax, [BOOTMAP + 12]         ; размер ядра в байтах
    add eax, 511
    shr eax, 9
    jz map_fail
    cmp eax, MAX_KERNEL_SECTORS
    ja map_fail
    mov [total_left], eax
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
    movzx eax, ax
    mov [sectors_left], eax
    sub [total_left], eax
    mov ax, [si]
    mov [cur_lba_lo], ax
    mov ax, [si + 2]
    mov [cur_lba_hi], ax
    add word [ext_ptr], 8
    dec word [ext_cnt]

.load_loop:
    cmp dword [sectors_left], 0
    je .load_done

    mov eax, [sectors_left]
    cmp eax, CHUNK_SECTORS
    jbe .use_remaining
    mov eax, CHUNK_SECTORS
.use_remaining:
    mov [dap_count], ax
    mov [requested_count], ax

    mov word [dap_segment], BUF_SEG
    mov ax, [cur_lba_lo]
    mov [dap_lba_lo], ax
    mov ax, [cur_lba_hi]
    mov [dap_lba_lo + 2], ax

    mov ah, 0x42
    mov dl, [boot_drive]
    mov si, dap
    ; BIOS USB implementations may depend on hardware interrupts.
    sti
    int 0x13
    cli                          ; CLI preserves the BIOS carry flag
    jnc .read_ok
    ; Не продолжать с частичным образом: сигнатура MBR в первом чанке
    ; не подтверждает, что остальные разделы успешно скопированы.
    jmp disk_error
.read_ok:
    ; int13 (BIOS) может оставить DS/ES в другом состоянии — сбрасываем.
    xor ax, ax
    mov ds, ax
    mov es, ax

    ; копируем чанк в RAM-диск (PM, плоские сегменты: linear = IP)
    movzx eax, word [requested_count]
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
    mov esi, BUF_ADDR         ; буфер (absolute)
    mov edi, [ram_dst_phys]   ; ядро (absolute)
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

    movzx eax, word [requested_count]
    sub [sectors_left], eax
    add [cur_lba_lo], ax
    adc word [cur_lba_hi], 0

    cmp byte [fill_active], 0
    je .load_loop
    ; ФАЗА 2 (заполнение RAM-диска): total_left — общий счётчик,
    ; истощаем его вместе со sectors_left; когда оба нули — всё готово.
    sub [total_left], eax
    cmp dword [total_left], 0
    jnz .load_loop
    jmp .load_complete

.load_done:
    cmp dword [total_left], 0
    je .load_complete
    cmp word [ext_cnt], 0
    jne .next_ext
    jmp map_fail
.load_complete:
    ; fill_active == 0: ядро загружено — запускаем заполнение RAM-диска.
    ; fill_active == 1: RAM-диск заполнен — в long mode к stage2.
    cmp byte [fill_active], 0
    jne .kernel_loaded

    ; ---- ФАЗА 2: весь образ диска (69632 сек) из LBA 0 в 0x4000000 ----
    ; Тот же цикл int13h AH=42h + PM rep movsd, что и для ядра, но с
    ; одним «условным участком»: LBA 0, RAMDISK_SECTORS секторов подряд.
    ; Источник (буфер 0x20000) и получатель (0x4000000) не пересекаются —
    ; копирование вперёд безопасно.
    mov byte [fill_active], 1
    mov eax, RAMDISK_SECTORS
    mov [sectors_left], eax
    mov [total_left], eax
    xor ax, ax
    mov [cur_lba_lo], ax
    mov [cur_lba_hi], ax
    mov word [ext_cnt], 0
    mov dword [ram_dst_phys], RAMDISK_BASE
    jmp .load_loop

.kernel_loaded:
    call setup_bios_framebuffer
    cli
    lgdt [gdt_descriptor]
    mov eax, cr0
    or eax, 1
    mov cr0, eax
    jmp CODE_SEG:pm_go

; VBE handoff at 0x6800 (outside bootmap / transfer buffers).
; Prefer common laptop HD modes, then retain 800x600 as a safe fallback.
; Only accept RGB565 or 24/32-bit RGB with a validated linear framebuffer.
; On failure leave text mode untouched. Do not touch the Bochs DISPI path.
setup_bios_framebuffer:
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov dword [0x6800], 0
%ifndef FORCE_BIOS_VBE
    mov dx, 0x1ce
    xor ax, ax
    out dx, ax
    mov dx, 0x1cf
    in ax, dx
    cmp ax, 0xb0c0
    jb .probe
    cmp ax, 0xb0c5
    jbe .done
%endif
.probe:
    mov dword [0x6000], 'VBE2'
    mov di, 0x6000
    mov ax, 0x4f00
    sti
    int 0x10
    cli
    cmp ax, 0x004f
    jne .done
    xor ax, ax
    mov ds, ax
    cmp dword [0x6000], 'VESA'
    jne .done
    mov word [vbe_resolution_index], 0
.resolution:
    mov si, [vbe_resolution_index]
    cmp si, 16                   ; four width/height pairs, best first
    jae .done
    mov ax, [vbe_resolutions + si]
    mov [vbe_width], ax
    mov ax, [vbe_resolutions + si + 2]
    mov [vbe_height], ax
    mov ax, [0x600e]
    mov [vbe_list_offset], ax
    mov ax, [0x6010]
    mov [vbe_list_segment], ax
    mov word [vbe_tries], 256
.next:
    cmp word [vbe_tries], 0
    je .next_resolution
    dec word [vbe_tries]
    mov ax, [vbe_list_segment]
    mov fs, ax
    mov si, [vbe_list_offset]
    mov cx, [fs:si]
    cmp cx, 0xffff
    je .next_resolution
    add word [vbe_list_offset], 2
    jnc .no_carry
    add word [vbe_list_segment], 0x1000
.no_carry:
    mov [vbe_mode], cx
    xor ax, ax
    mov es, ax
    mov di, 0x5000
    mov ax, 0x4f01
    sti
    int 0x10
    cli
    cmp ax, 0x004f
    jne .next_reset
    xor ax, ax
    mov ds, ax
    mov ax, [0x5000]
    and ax, 0x0091
    cmp ax, 0x0091
    jne .next
    mov ax, [vbe_width]
    cmp word [0x5012], ax
    jne .next
    mov ax, [vbe_height]
    cmp word [0x5014], ax
    jne .next
    cmp byte [0x5019], 32
    je .depth_ok
    cmp byte [0x5019], 24
    je .depth_ok
    cmp byte [0x5019], 16
    jne .next
.depth_ok:
    cmp byte [0x5019], 32
    jne .pitch_format_ok
    test word [0x5010], 3
    jnz .next
.pitch_format_ok:
    cmp byte [0x501b], 6
    jne .next
    movzx eax, byte [0x5019]
    shr eax, 3
    movzx edx, word [vbe_width]
    imul eax, edx
    movzx edx, word [0x5010]
    cmp edx, eax
    jb .next
    cmp word [0x5010], 32768
    ja .next
    cmp byte [0x5019], 16
    je .check_565
    cmp word [0x501f], 0x1008  ; red: 8 bits at bit 16
    jne .next
    cmp word [0x5021], 0x0808
    jne .next
    cmp word [0x5023], 0x0008
    jne .next
    jmp .masks_ok
.check_565:
    cmp word [0x501f], 0x0b05  ; RGB 565
    jne .next
    cmp word [0x5021], 0x0506
    jne .next
    cmp word [0x5023], 0x0005
    jne .next
.masks_ok:
    mov eax, [0x5028]
    cmp eax, 0x40000000       ; PCI aperture, not RAM or kernel image
    jb .next
    test al, 3
    jnz .next
    mov [0x6804], eax
    movzx edx, word [0x5010]
    movzx ecx, word [vbe_height]
    imul edx, ecx
    add edx, eax
    jc .next
    mov ax, [0x5010]
    mov [0x6808], ax
    mov al, [0x5019]
    mov [0x680a], al
    mov ax, [vbe_width]
    mov [0x680c], ax
    mov ax, [vbe_height]
    mov [0x680e], ax
    mov bx, [vbe_mode]
    or bx, 0x4000
    mov ax, 0x4f02
    sti
    int 0x10
    cli
    cmp ax, 0x004f
    jne .next_reset
    xor ax, ax
    mov ds, ax
    mov dword [0x6800], 'DXFB'
    jmp .done
.next_reset:
    xor ax, ax
    mov ds, ax
    jmp .next
.next_resolution:
    add word [vbe_resolution_index], 4
    jmp .resolution
.done:
    xor ax, ax
    mov ds, ax
    mov es, ax
    ret

bits 32
pm_go:
    mov ax, DATA_SEG
    mov ds, ax
    mov es, ax
    jmp STAGE2_ADDR

bits 16
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
vbe_list_offset: dw 0
vbe_list_segment: dw 0
vbe_tries: dw 0
vbe_mode: dw 0
vbe_resolution_index: dw 0
vbe_width: dw 0
vbe_height: dw 0
vbe_resolutions: dw 1366, 768, 1360, 768, 1280, 720, 800, 600
boot_drive:    db 0
fill_active:   db 0              ; 0 = читаем ядро, 1 = заполняем RAM-диск
requested_count: dw 0            ; DAP count may be modified by BIOS
sectors_left:  dd 0
cur_lba_lo:    dw 0
cur_lba_hi:    dw 0
ram_dst_phys:  dd 0
chunk_bytes:   dd 0
total_left:    dd 0
ext_ptr:       dw 0
ext_cnt:       dw 0

map_fail:
    mov si, msg_map_err
    jmp disk_error.err_loop
disk_error:
    ; Причина — на экран (BIOS teletype) и в COM1 (его настроил MBR).
    mov si, msg_read_err
.err_loop:
    lodsb
    test al, al
    jz .err_hang
    mov ah, 0x0E
    mov bx, 0x0007
    int 0x10
    mov dx, 0x3F8
    out dx, al
    jmp .err_loop
.err_hang:
    jmp $

msg_read_err: db 13, 10, "RAMBOOT: disk read error", 13, 10, 0
msg_map_err:  db 13, 10, "RAMBOOT: bad boot map", 13, 10, 0

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

align 512, db 0
