; stage2.asm — грузится boot_sector'ом по физическому адресу 0x10000.
; Сюда мы попадаем уже в 32-битном защищённом режиме (без BIOS, без GRUB).
; Задача: включить paging + long mode, загрузить 64-битный GDT и прыгнуть
; в 64-битное ядро на Rust.

bits 32

global stage2_start
extern long_mode_start
extern __bss_start
extern __bss_end

section .text
stage2_start:
    ; Обнуляем секцию .bss ДО первого call/push — там лежат таблицы страниц,
    ; куча ядра (HEAP_STORAGE) и сам стек (stack_bottom/stack_top). Секция
    ; .bss физически не хранится в файле образа диска (мы её не пишем на
    ; диск), поэтому по стандарту ABI загрузчик обязан сам обнулить эту
    ; память перед стартом программы — без этого шага там остаётся
    ; произвольный мусор от предыдущего состояния физической RAM. Именно
    ; отсутствие этого шага стало причиной паники аллокатора кучи на
    ; некоторых машинах/версиях QEMU, где RAM не была изначально нулевой.
    mov edi, __bss_start
    mov ecx, __bss_end
    sub ecx, edi
    xor eax, eax
    cld
    rep stosb

    mov esp, stack_top

    call check_cpuid
    call check_long_mode

    call set_up_page_tables
    call enable_paging

    lgdt [gdt64.pointer]
    jmp gdt64.code_segment:long_mode_start

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

; ---------- paging ----------

; Отображаем (identity map) первые 4 GiB физической памяти вместо
; исходного 1 GiB — так нужно, чтобы дотянуться до линейного framebuffer
; видеокарты (Bochs VBE/std VGA в QEMU обычно кладёт framebuffer в районе
; 0xFC000000-0xFE000000, что выше 1 GiB, но укладывается в 4 GiB).
;
; Схема: P4[0] -> P3, P3[0..4) -> четыре таблицы P2, каждая из 512
; 2MiB-страниц = 4 * 512 * 2MiB = 4 GiB.
set_up_page_tables:
    mov eax, page_table_l3
    or eax, 0b11
    mov [page_table_l4], eax

    ; заполняем 4 записи P3, каждая указывает на свою таблицу P2
    ; (таблицы P2 идут в памяти подряд, каждая занимает ровно 4096 байт)
    mov ecx, 0
.l3_loop:
    mov eax, ecx
    imul eax, 4096
    add eax, page_table_l2
    or eax, 0b11
    mov [page_table_l3 + ecx * 8], eax

    inc ecx
    cmp ecx, 4
    jne .l3_loop

    ; заполняем все 4*512 записей P2 2MiB huge pages подряд, покрывая
    ; последовательно все 4 GiB физической памяти.
    mov ecx, 0
.l2_loop:
    mov eax, 0x200000       ; 2MiB
    mul ecx
    or eax, 0b10000011      ; present + writable + huge page
    mov [page_table_l2 + ecx * 8], eax

    inc ecx
    cmp ecx, 2048            ; 4 таблицы * 512 записей = 2048 записей всего
    jne .l2_loop

    ret

enable_paging:
    mov eax, page_table_l4
    mov cr3, eax

    mov eax, cr4
    or eax, 1 << 5
    mov cr4, eax

    mov ecx, 0xC0000080
    rdmsr
    or eax, 1 << 8
    wrmsr

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

section .bss
align 4096
page_table_l4:
    resb 4096
page_table_l3:
    resb 4096
page_table_l2:
    resb 4096 * 4    ; 4 таблицы P2 (по одной на каждый GiB из четырёх)
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
