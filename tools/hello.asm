; hello.asm — Minimal .mex demo: prints a greeting via MexApi::print and exits.
;
; Assembles to a flat .bin, packed into .mex by tools/mex_pack.py.
; See docs/MEX_FORMAT.md for the full .mex specification.
;
; Calling convention (System V AMD64):
;   RDI = pointer to MexApi struct
;   Entry point is at offset 0 (entry_offset=0 in the .mex header).
;
; MexApi layout (v1.1, see src/mex.rs):
;   offset 0x00: print        (fn ptr: RDI=ptr, RSI=len)
;   offset 0x08: read_char    (fn ptr: returns AL)
;   offset 0x10: try_read_char(fn ptr: returns EAX, -1 if empty)
;   offset 0x18: uptime_ms    (fn ptr: returns RAX)
;   offset 0x20: read_file    (fn ptr)
;   offset 0x28: write_file   (fn ptr)
;   offset 0x30: ping         (fn ptr)  — new in v1.1
;   offset 0x38: get_mac      (fn ptr)  — new in v1.1
;   offset 0x40: get_ip       (fn ptr)  — new in v1.1
;   offset 0x48: arp_resolve  (fn ptr)  — new in v1.1

BITS 64
DEFAULT REL

SECTION .text

global _start
_start:
    ; RDI = &api
    mov r12, rdi                ; save api pointer

    ; Print greeting
    mov rdi, [r12]              ; api->print function pointer
    lea rsi, [rel msg]
    mov rdx, msg_len
    call rdi

    ; Print uptime via api->uptime_ms
    mov rax, [r12 + 0x18]       ; api->uptime_ms
    call rax
    ; RAX = uptime in ms, just report it's alive
    mov rdi, [r12]              ; api->print
    lea rsi, [rel alive_msg]
    mov rdx, alive_msg_len
    call rdi

    ; Try to get IP (new v1.1 API)
    mov rax, [r12 + 0x40]       ; api->get_ip
    lea rdi, [rel ip_buf]
    call rax
    ; If ip_buf is not 0.0.0.0, print it
    cmp byte [rel ip_buf], 0
    je .no_ip
    mov rdi, [r12]              ; api->print
    lea rsi, [rel ip_msg]
    mov rdx, ip_msg_len
    call rdi

.no_ip:
    xor eax, eax                ; return 0
    ret

SECTION .rodata

msg: db "Hello from DeiX .mex program! (MEX v1.1)", 0x0A, 0
msg_len equ $ - msg - 1         ; exclude null terminator

alive_msg: db "Program is alive and has API access.", 0x0A, 0
alive_msg_len equ $ - alive_msg - 1

ip_msg: db "Network API (get_ip) is available in v1.1.", 0x0A, 0
ip_msg_len equ $ - ip_msg - 1

SECTION .bss
ip_buf: resb 4
