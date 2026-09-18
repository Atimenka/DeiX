; boot_sector.asm — классический MBR-загрузчик (512 байт, BIOS реальный режим).
; BIOS сам находит его по сигнатуре 0xAA55 и загружает по адресу 0x7c00.
; Используется для обычного дискового образа (deix_disk.img, -drive в QEMU
; / прямая запись на физический носитель). Для гибридного .iso (загрузка
; через -cdrom) используется ОТДЕЛЬНЫЙ загрузчик boot/boot_sector_iso.asm —
; см. подробное объяснение прямо в том файле: реальный SeaBIOS не
; поддерживает El Torito "hard disk emulation" для произвольных образов
; (была экспериментально обнаружена нулевая CHS-геометрия), поэтому ISO
; собирается через другой механизм (El Torito "no emulation" boot).
;
; Задача:
;   1) вывести сообщение через BIOS (int 10h)
;   2) прочитать stage2 (наш 32/64-битный код + ядро) с диска через int 13h
;      (extended read, AH=42h, LBA-адресация — доступно всегда для
;      настоящего/эмулированного жёсткого диска, подключённого как -drive)
;   3) включить линию A20
;   4) переключиться в 32-битный защищённый режим (через GDT)
;   5) прыгнуть на stage2, загруженный по физическому адресу 0x10000

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

    ; ---- читаем stage2 (NUM_SECTORS секторов) в 0x10000 ----
    mov word [sectors_left], NUM_SECTORS
    mov word [cur_lba_lo], 1        ; стартовый LBA (сектор 1, сразу после MBR)
    mov word [cur_lba_hi], 0
    mov word [cur_segment], 0x1000  ; физ. адрес 0x10000 = 0x1000:0x0000
    call read_sectors_loop
    mov al, 'S'
    call serial_byte

    ; ---- читаем ramboot (RAM-диск загрузчик, RAMBOOT_SECTORS секторов)
    ; в 0x9000 (низкая память: org=0x9000, DS=0, PM-сегменты плоские).
    ; kernel.bin НЕ читаем здесь — ramboot сам скопирует его из RAM-диска
    ; (читает весь образ через int13 в 0x2000000). ----
    mov word [sectors_left], RAMBOOT_SECTORS
    mov word [cur_lba_lo], RAMBOOT_LBA
    mov word [cur_lba_hi], 0
    mov word [cur_segment], 0x07E0  ; физ. адрес 0x7E00 = 0x07E0:0x0000
    call read_sectors_loop

    ; ---- передаём управление ramboot (real mode) ----
    ; DL = номер загрузочного диска, который дал BIOS. Раньше ramboot
    ; его игнорировал и жёстко пробовал 0x80/0x81 — на ноутбуке ASUS
    ; флешка получает другой номер (0x82+), чтение проваливалось, и
    ; RAM-диск оставался пустым: ядро грузилось, но все разделы были
    ; "не читается", потому что искались на несуществующем IDE-диске.
    mov dl, [boot_drive]
    jmp 0x07E0:0x0000

disk_error:
    mov si, msg_disk_error
    call print_string
    jmp $

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

msg_loading:    db "DeiX bootloader: loading stage2...", 13, 10, 0
msg_disk_error: db "Disk read error!", 13, 10, 0

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

; NUM_SECTORS передаётся снаружи через `nasm -D NUM_SECTORS=N` (см. build.sh),
; чтобы не читать с диска больше, чем реально есть (иначе BIOS вернёт ошибку
; на образах маленького размера). Значение по умолчанию — на случай ручной
; сборки без параметра.
%ifndef NUM_SECTORS
NUM_SECTORS equ 64
%endif
%ifndef KERNEL_SECTORS
KERNEL_SECTORS equ 2048
%endif
; ramboot лежит сразу после stage2 (LBA = 1 + NUM_SECTORS).
%ifndef RAMBOOT_LBA
RAMBOOT_LBA equ (1 + NUM_SECTORS)
%endif
%ifndef RAMBOOT_SECTORS
RAMBOOT_SECTORS equ 1
%endif
; kernel.bin лежит сразу после ramboot (LBA = 1 + NUM_SECTORS + RAMBOOT_SECTORS).
%ifndef KERNEL_LBA
KERNEL_LBA equ (1 + NUM_SECTORS + RAMBOOT_SECTORS)
%endif

; ---------------- MBR Partition Table (offset 446 = 0x1BE) ----------------
; Настоящая таблица разделов (не заглушка!) — одна запись, описывающая
; ext2-том, который и так уже занимает фиксированный диапазон секторов
; на диске (см. src/ext2.rs: FS_START_LBA=4096, TOTAL_SECTORS=8192).
; Сам загрузчик и ядро НЕ читают эту таблицу в рантайме (ext2.rs как и
; раньше использует жёстко зашитые константы) — партиционирование здесь
; сделано для того, чтобы ВНЕШНИЕ инструменты (fdisk, parted, GParted,
; программы разметки в Windows, автоматическое определение раздела в
; Linux при подключении диска) видели диск как нормально размеченный, а
; не как "неразмеченное сырое устройство". Это именно то поведение,
; которого ожидают от настоящего инсталлятора ОС (аналогично тому, как
; это делают инсталляторы Linux/Windows).
;
; Поле                  Значение         Смысл
; boot indicator         0x80             активный (загрузочный) раздел
; CHS start              0x41 0x02 0x00   legacy-поле, вычислено из LBA
;                                          (HPC=255, SPT=63 — стандартная
;                                          "large" геометрия, которую
;                                          принимает подавляющее
;                                          большинство инструментов)
; тип раздела             0x83             Linux native (тот же тип,
;                                          который использует настоящий
;                                          Linux для ext2/ext3/ext4)
; CHS end                 0xC3 0x03 0x00   legacy-поле, аналогично старту
; LBA start (32 бита)     4096             = FS_START_LBA в src/ext2.rs
; количество секторов     8192             = TOTAL_SECTORS в src/ext2.rs
;
; Секторы 1..4095 (до начала раздела) заняты нашим загрузчиком/ядром
; (stage2.bin) — это стандартная практика: похожим образом GRUB и другие
; BIOS-загрузчики резервируют небольшой промежуток между MBR и первым
; разделом ("MBR gap") для собственного кода второй стадии.
times (446-($-$$)) db 0
db 0x80                     ; boot indicator: активный раздел
db 0x41, 0x02, 0x00          ; CHS start (LBA 4096 при HPC=255,SPT=63)
db 0x83                     ; тип раздела: Linux native (ext2/3/4)
db 0xC3, 0x03, 0x00          ; CHS end (LBA 12287 при HPC=255,SPT=63)
dd 4096                     ; LBA start = FS_START_LBA (src/ext2.rs)
dd 8192                     ; количество секторов = TOTAL_SECTORS (src/ext2.rs)
times 16 db 0                ; запись 2 (не используется)
times 16 db 0                ; запись 3 (не используется)
times 16 db 0                ; запись 4 (не используется)

times 510-($-$$) db 0
dw 0xAA55
