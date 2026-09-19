; boot_sector_iso.asm — загрузчик для El Torito "no emulation" CD/ISO-загрузки.
; Используется ТОЛЬКО при сборке deix.iso (см. build.sh, шаг [7/7]); для
; обычного дискового образа deix_disk.img по-прежнему используется
; boot/boot_sector.asm (классический MBR).
;
; ПОЧЕМУ ОТДЕЛЬНЫЙ ЗАГРУЗЧИК ДЛЯ ISO (история разведки — три отброшенных
; подхода, прежде чем нашёлся рабочий):
;
;   1) Первая попытка: обернуть готовый deix_disk.img (MBR + stage2) в
;      El Torito "hard disk emulation" (genisoimage/xorriso -hard-disk-boot).
;      РЕЗУЛЬТАТ: реальный SeaBIOS (в QEMU) для такого эмулированного диска
;      либо не поддерживает extended int13h (AH=42h/LBA) вовсе, либо (что
;      обнаружилось после добавления CHS-fallback через AH=08h) возвращает
;      геометрию 0 heads / 0 sectors-per-track — то есть режим "hard disk
;      emulation" в SeaBIOS практически нерабочий для произвольных образов.
;      Задокументированное поведение — см. заметки VirtualBox BIOS:
;      "NetWare 5.1 is one of the extremely few users of El Torito hard
;      disk emulation" и "Symantec Ghost... crashes if INT13X is not
;      supported on the emulated drive".
;
;   2) Вторая попытка: El Torito "no emulation" boot с load-segment=0xFE0
;      (адрес 0xFE00), чтобы sizeof(стаб)+STAGE2_ADDR(0x10000) совпадали.
;      РЕЗУЛЬТАТ: SeaBIOS игнорирует поле "load segment" из Boot Catalog и
;      ВСЕГДА физически размещает образ по линейному адресу 0x7c00 (это
;      известная нестыковка между текстом спецификации El Torito ("load
;      segment, обычно 0x7C0") и тем, как это поняли и реализовали
;      разработчики множества BIOS — экран остаётся чёрным, машина висит).
;
;   3) РАБОЧЕЕ РЕШЕНИЕ (этот файл): раз BIOS всё равно грузит "no
;      emulation" образ по 0x7c00, пишем маленький 512-байтный стаб
;      именно под это соглашение. Он сам переключается в 32-битный
;      protected mode (плоская адресация) и копирует уже загруженный
;      "хвост" образа (stage2.bin, физически лежащий сразу за стабом —
;      с адреса 0x7e00) на 0x10000, куда его ожидает увидеть Rust-код
;      (см. boot/linker_stage2.ld — тот же адрес, что и в обычном MBR-варианте).
;      Регионы src=[0x7e00..] и dst=[0x10000..] у нас ПЕРЕСЕКАЮТСЯ
;      (dst < src+size при типичном размере ядра), поэтому копирование
;      идёт СТРОГО в обратном направлении (std + rep movsd от конца к
;      началу) — иначе данные портятся "на лету" (это и вызывало
;      triple fault на первой версии стаба). После копирования выполняется
;      jmp на 0x10000 — начиная с этой точки long-mode-инициализация и
;      Rust-ядро АБСОЛЬТНО идентичны между .img и .iso сборками.

bits 16
org 0x7c00

STAGE2_SRC equ 0x7e00     ; где физически лежит stage2 после загрузки BIOS
STAGE2_DST equ 0x10000    ; куда его нужно переместить (linker_stage2.ld)
KERNEL_DST equ 0x800000   ; куда переместить kernel.bin (stage2 копирует его в 0x100000 с KERNEL_SRC=0x800000)
; RAM-ДИСК: полный 10-МиБ образ диска (MBR+stage2+kernel+все разделы) лежит
; в конце boot-образа; стаб копирует его в 0x2000000 (32 МиБ), а ядро
; читает ВСЕ разделы (BCB, /system, /OTA...) прямо из памяти — поэтому
; OS работает с ЛЮБОГО носителя (Ventoy/USB/CD) без драйверов диска.
RAMDISK_DST equ 0x2000000

start:
    cli
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov sp, 0x7c00
    sti

    mov si, msg_loading
    call print_string

    ; ---- A20 ----
    in al, 0x92
    or al, 2
    out 0x92, al

    cli
    lgdt [gdt_descriptor]

    mov eax, cr0
    or eax, 1
    mov cr0, eax

    jmp CODE_SEG:protected_mode_start

print_string:
    pusha
.loop:
    lodsb
    cmp al, 0
    je .done
    mov ah, 0x0e
    xor bx, bx
    int 0x10
    jmp .loop
.done:
    popa
    ret

msg_loading: db "DeiX (ISO): relocating stage2...", 13, 10, 0

bits 32
protected_mode_start:
    mov ax, DATA_SEG
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    mov ss, ax
    mov esp, 0x90000

    ; ---- 0) RAM-диск (10 МиБ) -> 0x2000000 (ОБРАТНО: src-область ПЕРЕСЕКАЕТСЯ
    ;      с dst — копирование вперёд затирает источник!) ----
    ; src = STAGE2_SRC + stage2 + kernel (в boot-образе после kernel.bin).
    std
    mov esi, STAGE2_SRC + (STAGE2_SIZE_DWORDS * 4) + (KERNEL_SIZE_DWORDS * 4) + (RAMDISK_SIZE_DWORDS * 4) - 4
    mov edi, RAMDISK_DST + (RAMDISK_SIZE_DWORDS * 4) - 4
    mov ecx, RAMDISK_SIZE_DWORDS
    rep movsd
    cld

    ; ---- 1) kernel.bin -> 0x800000 (ОБРАТНО: dst=0x800000,
    ;      пересечение — копирование вперёд затирает источник!) ----
    ; kernel лежит сразу после stage2 в boot-образе (src ~0x7e00 + stage2_size).
    std
    mov esi, STAGE2_SRC + (STAGE2_SIZE_DWORDS * 4) + (KERNEL_SIZE_DWORDS * 4) - 4
    mov edi, KERNEL_DST + (KERNEL_SIZE_DWORDS * 4) - 4
    mov ecx, KERNEL_SIZE_DWORDS
    rep movsd
    cld

    ; ---- 2) stage2.bin -> 0x10000 (ОБРАТНО: dst < src+size, пересекается) ----
    std
    mov esi, STAGE2_SRC + (STAGE2_SIZE_DWORDS * 4) - 4
    mov edi, STAGE2_DST + (STAGE2_SIZE_DWORDS * 4) - 4
    mov ecx, STAGE2_SIZE_DWORDS
    rep movsd
    cld

    jmp STAGE2_DST

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

; STAGE2_SIZE_DWORDS передаётся снаружи через `nasm -D` (аналогично
; NUM_SECTORS в обычном MBR-загрузчике) — количество 4-байтных слов,
; округлённое вверх, чтобы скопировать весь stage2.bin.
%ifndef STAGE2_SIZE_DWORDS
STAGE2_SIZE_DWORDS equ 100
%endif
%ifndef KERNEL_SIZE_DWORDS
KERNEL_SIZE_DWORDS equ 1000
%endif
; RAM-диск: 20480 секторов * 512 байт = 2621440 dword (образ 10 МиБ).
%ifndef RAMDISK_SIZE_DWORDS
RAMDISK_SIZE_DWORDS equ 2621440
%endif

times 512-($-$$) db 0
