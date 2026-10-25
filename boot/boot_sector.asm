; boot_sector.asm — MBR-загрузчик DeiX (512 байт, BIOS реальный режим).
; BIOS находит его по сигнатуре 0xAA55 и загружает по адресу 0x7c00.
; Используется для дискового образа (deix_disk.img, -drive в QEMU / прямая
; запись на физический носитель). Для .iso используется отдельный стаб
; boot/boot_sector_iso.asm.
;
; Загрузчик и ядро лежат ФАЙЛАМИ в /system (EROFS), а не в сырой области
; диска:
;   /system/boot/stage2.bin, /system/boot/ramboot.bin, /system/kernel/kernel.bin
; MBR не знает файловых систем: путь к файлам задаёт КАРТА ЗАГРУЗКИ в секторе
; LBA 2047 (формат DEIXKIMG, версия 2, см. docs/BOOTMAP.md): для каждого файла
; список участков диска (LBA, секторов). Карту пишут tools/make_deix_fs.py
; при сборке образа и ядро (install, sysfs convert).
;
; Задача:
;   1) вывести сообщение через BIOS (int 10h) и в COM1
;   2) прочитать карту загрузки (LBA 2047) в 0x7000 и проверить её
;   3) прочитать stage2 по карте в 0x10000
;   4) прочитать ramboot по карте в 0x7E00 и передать ему управление
;      (DL = номер загрузочного диска, полученный от BIOS)
;   Ядро в память 0x100000 грузит ramboot, затем stage2 включает long mode.

bits 16
org 0x7c00

start:
    cli
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov sp, 0x7c00
    sti

    mov [boot_drive], dl   ; BIOS кладёт номер загрузочного диска в dl

    ; COM1 38400 8N1
    mov dx, 0x3FB
    mov al, 0x80
    out dx, al
    mov dx, 0x3F8
    mov al, 0x03
    out dx, al
    mov dx, 0x3F9
    xor al, al
    out dx, al
    mov dx, 0x3FB
    mov al, 0x03
    out dx, al

    mov si, msg_loading
    call print_string
    mov si, msg_loading
    call serial_str

    ; ---- карта загрузки: LBA BOOTMAP_LBA -> 0x7000 ----
    mov word [sectors_left], 1
    mov word [cur_lba_lo], BOOTMAP_LBA
    mov word [cur_lba_hi], 0
    mov word [cur_segment], 0x0700
    call read_sectors_loop
    mov si, BOOTMAP
    mov di, bootmap_magic
    mov cx, 4
    repe cmpsw
    jne map_error
    cmp word [BOOTMAP + 48], 2      ; версия карты
    jne map_error

    ; ---- stage2 по карте в 0x10000 ----
    mov si, BOOTMAP + 64
    mov ax, [BOOTMAP + 52]
    mov word [cur_segment], 0x1000
    call load_extents
    mov al, 'S'
    call serial_byte

    ; ---- ramboot по карте в 0x7E00 (низкая память: org=0x7E00, DS=0,
    ; плоские сегменты PM). Ядро грузит сам ramboot. ----
    mov si, BOOTMAP + 128
    mov ax, [BOOTMAP + 56]
    mov word [cur_segment], 0x07E0
    call load_extents

    ; ---- передаём управление ramboot (real mode) ----
    ; DL = номер загрузочного диска, который дал BIOS. Раньше ramboot
    ; его игнорировал и жёстко пробовал 0x80/0x81 — на ноутбуке ASUS
    ; флешка получает другой номер (0x82+), чтение проваливалось, и
    ; RAM-диск оставался пустым: ядро грузилось, но все разделы были
    ; "не читается", потому что искались на несуществующем IDE-диске.
    mov dl, [boot_drive]
    jmp 0x07E0:0x0000

map_error:
    mov si, msg_map_error
    jmp fatal
disk_error:
    mov si, msg_disk_error
fatal:
    push si
    call print_string
    pop si
    call serial_str
    jmp $

; Читает участки диска подряд в память с сегмента cur_segment.
; SI = таблица участков (до 8 записей: LBA u32, секторов u32; секторов = 0 —
; конец), AX = сколько секторов нужно всего. Нехватка участков — ошибка карты.
load_extents:
    mov cx, 8
.next:
    mov dx, [si + 4]
    test dx, dx
    jz .done
    cmp dx, ax
    jbe .keep
    mov dx, ax
.keep:
    mov [sectors_left], dx
    sub ax, dx
    mov bx, [si]
    mov [cur_lba_lo], bx
    mov bx, [si + 2]
    mov [cur_lba_hi], bx
    push ax
    push cx
    push si
    call read_sectors_loop
    pop si
    pop cx
    pop ax
    add si, 8
    test ax, ax
    jz .done
    loop .next
.done:
    test ax, ax
    jnz map_error
    ret

print_string:
    pusha
.loop:
    lodsb
    cmp al, 0
    je .done
    mov ah, 0x0e
    int 0x10
    jmp .loop
.done:
    popa
    ret

serial_byte:
    push ax
.wait_tx:
    mov dx, 0x3FD
    in al, dx
    test al, 0x20
    jz .wait_tx
    pop ax
    mov dx, 0x3F8
    out dx, al
    ret

serial_str:
    lodsb
    cmp al, 0
    je .done
    call serial_byte
    jmp serial_str
.done:
    ret

; ---------------- подпрограмма чтения с диска (int13/AH=42) ----------------
; Параметры: sectors_left, cur_lba_lo/hi, cur_segment (задаются ДО вызова).
bits 16
read_sectors_loop:
.read_loop:
    cmp word [sectors_left], 0
    je .read_done

    ; сколько секторов читаем в этой итерации: min(CHUNK_SECTORS, sectors_left)
    mov ax, [sectors_left]
    cmp ax, CHUNK_SECTORS
    jbe .use_remaining
    mov ax, CHUNK_SECTORS
.use_remaining:
    mov [dap_count], ax

    mov ax, [cur_segment]
    mov [dap_segment], ax

    mov ax, [cur_lba_lo]
    mov [dap_lba_lo], ax
    mov ax, [cur_lba_hi]
    mov [dap_lba_lo + 2], ax

    mov ah, 0x42
    mov dl, [boot_drive]
    mov si, dap
    int 0x13
    jc disk_error

    ; sectors_left -= прочитанное количество
    mov ax, [dap_count]
    sub [sectors_left], ax

    ; cur_lba += прочитанное количество (32-битная арифметика через два слова)
    add [cur_lba_lo], ax
    adc word [cur_lba_hi], 0

    ; cur_segment += (прочитано_секторов * 512) / 16 = прочитано_секторов * 32
    mov ax, [dap_count]
    shl ax, 5                 ; * 32
    add [cur_segment], ax

    jmp .read_loop
.read_done:
    ret

boot_drive: db 0

; Сколько секторов читаем за один вызов int 13h (64 сектора = 32KB —
; заведомо не пересекает 64KB-границу сегмента даже при offset=0).
CHUNK_SECTORS equ 64

sectors_left: dw 0
cur_lba_lo:   dw 0
cur_lba_hi:   dw 0
cur_segment:  dw 0

msg_loading:    db "DeiX: loading", 13, 10, 0
msg_disk_error: db "Disk error", 13, 10, 0
msg_map_error:  db "Bad boot map", 13, 10, 0
bootmap_magic:  db "DEIXKIMG"

; ---------------- Disk Address Packet (для int 13h, ah=0x42) ----------------
align 4
dap:
    db 0x10             ; размер пакета
    db 0                ; зарезервировано
dap_count:
    dw 0                ; сколько секторов читать (заполняется динамически)
    dw 0x0000           ; смещение буфера (offset) — всегда 0
dap_segment:
    dw 0                ; сегмент буфера (заполняется динамически)
dap_lba_lo:
    dw 0                ; младшее слово LBA
    dw 0                ; (LBA — 64-битное поле, но нам хватит младших 32 бит)
dap_lba_hi:
    dw 0
    dw 0

BOOTMAP_LBA equ 2047        ; = partition_map.rs KERNEL_IMAGE_INFO_LBA
BOOTMAP     equ 0x7000      ; карта загрузки в памяти (читает и ramboot)

; ---------------- MBR Partition Table (offset 446 = 0x1BE) ----------------
; Таблица разделов DeiX (источник истины — src/partition_map.rs,
; та же таблица пишется tools/make_deix_fs.py в собранный образ и
; src/install.rs::build_mbr на целевой диск):
;
;   P1 0x83 bootable  LBA 4096  + 20480 сект (4096..24575)   /system  (EROFS RO)
;   P2 0x83           LBA 24576 + 16384 сект (24576..40959)  /userdata (EXT2 RW)
;
; Сам загрузчик и ядро НЕ читают эту таблицу в рантайме — она нужна,
; чтобы внешние инструменты (fdisk, parted, GParted) видели диск как
; нормально размеченный. CHS-поля legacy, вычислены из LBA при
; HPC=255, SPT=63.
;
; Секторы 1..2046 свободны (стандартный "MBR gap", как у GRUB): stage2,
; ramboot и ядро лежат в /system. LBA 2047 — карта загрузки, 2048..2111 —
; область аварийного дампа.
times (446-($-$$)) db 0
; --- P1: /system (EROFS RO) ---
db 0x80                     ; boot indicator: активный раздел
db 0x41, 0x02, 0x00          ; CHS start (LBA 4096)
db 0x83                     ; тип раздела: Linux native
db 0x87, 0x06, 0x01          ; CHS end (LBA 24575)
dd 4096                     ; LBA start (= partition_map.rs /system)
dd 20480                    ; секторов   (= partition_map.rs /system)
; --- P2: /userdata (EXT2 RW) ---
db 0x00                     ; не загрузочный
db 0x87, 0x07, 0x01          ; CHS start (LBA 24576)
db 0x83                     ; тип раздела: Linux native
db 0x8C, 0x0A, 0x02          ; CHS end (LBA 40959)
dd 24576                    ; LBA start (= partition_map.rs /userdata)
dd 16384                    ; секторов   (= partition_map.rs /userdata)
times 16 db 0                ; запись 3 (не используется)
times 16 db 0                ; запись 4 (не используется)

times 510-($-$$) db 0
dw 0xAA55
