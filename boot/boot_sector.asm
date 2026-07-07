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

    mov si, msg_loading
    call print_string

    ; ---- читаем stage2 с диска частями по CHUNK_SECTORS секторов ----
    mov word [sectors_left], NUM_SECTORS
    mov word [cur_lba_lo], 1        ; стартовый LBA (сектор 1, сразу после MBR)
    mov word [cur_lba_hi], 0
    mov word [cur_segment], 0x1000  ; физ. адрес 0x10000 = 0x1000:0x0000

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

    ; dap_lba_lo — младшие 32 бита 64-битного LBA-поля (2 слова);
    ; наше cur_lba_lo/cur_lba_hi — это как раз младшее и следующее слово
    ; этих самых 32 бит (старшие 32 бита dap_lba_hi всегда остаются 0,
    ; т.к. для загрузочного диска в несколько сотен секторов этого более
    ; чем достаточно).
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

    ; ---- включаем A20 (быстрый метод через порт 0x92) ----
    in al, 0x92
    or al, 2
    out 0x92, al

    cli
    lgdt [gdt_descriptor]

    mov eax, cr0
    or eax, 1
    mov cr0, eax

    jmp CODE_SEG:protected_mode_start

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

bits 32
protected_mode_start:
    mov ax, DATA_SEG
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    mov ss, ax
    mov esp, 0x90000

    jmp STAGE2_ADDR         ; передаём управление stage2 (см. build.sh / linker2.ld)

; ---------------- GDT (плоская модель, 32-бит) ----------------
gdt_start:
gdt_null:
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
gdt_end:

gdt_descriptor:
    dw gdt_end - gdt_start - 1
    dd gdt_start

CODE_SEG equ gdt_code - gdt_start
DATA_SEG equ gdt_data - gdt_start

STAGE2_ADDR equ 0x10000  ; физический адрес, куда грузим stage2

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
