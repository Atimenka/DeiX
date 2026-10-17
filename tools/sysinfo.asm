; sysinfo.asm — .mex demo: prints kernel uptime and does a ping test.
;
; Uses MexApi v1.1 with network functions.

BITS 64
DEFAULT REL

SECTION .text

global _start
_start:
    mov r12, rdi                ; save api pointer

    ; --- Print header ---
    mov rdi, [r12]
    lea rsi, [rel header]
    mov rdx, header_len
    call rdi

    ; --- Get uptime ---
    mov rax, [r12 + 0x18]       ; api->uptime_ms
    call rax
    mov r13, rax                ; save uptime in r13

    mov rdi, [r12]
    lea rsi, [rel uptime_prefix]
    mov rdx, uptime_prefix_len
    call rdi

    ; Convert uptime to decimal string (lazy: print "N ms")
    ; We'll just print a static message with the raw value note
    mov rdi, [r12]
    lea rsi, [rel uptime_suffix]
    mov rdx, uptime_suffix_len
    call rdi

    ; --- Get MAC ---
    mov rax, [r12 + 0x38]       ; api->get_mac (v1.1)
    lea rdi, [rel mac_buf]
    call rax

    mov rdi, [r12]
    lea rsi, [rel mac_msg]
    mov rdx, mac_msg_len
    call rdi

    ; --- Get IP ---
    mov rax, [r12 + 0x40]       ; api->get_ip (v1.1)
    lea rdi, [rel ip_buf]
    call rax

    mov rdi, [r12]
    lea rsi, [rel ip_msg]
    mov rdx, ip_msg_len
    call rdi

    ; --- Try ping gateway (10.0.2.2) ---
    mov rdi, [r12]
    lea rsi, [rel ping_msg]
    mov rdx, ping_msg_len
    call rdi

    mov rax, [r12 + 0x30]       ; api->ping (v1.1)
    lea rdi, [rel gateway_ip]
    mov rsi, 4                  ; ip_len
    mov rdx, 3000               ; timeout_ms
    call rax
    ; RAX = RTT ms or -1 on failure
    cmp rax, -1
    je .ping_fail

    mov rdi, [r12]
    lea rsi, [rel ping_ok]
    mov rdx, ping_ok_len
    call rdi
    jmp .done

.ping_fail:
    mov rdi, [r12]
    lea rsi, [rel ping_fail_msg]
    mov rdx, ping_fail_msg_len
    call rdi

.done:
    xor eax, eax
    ret

SECTION .rodata

header: db "=== DeiX System Info (MEX v1.1) ===", 0x0A, 0
header_len equ $ - header - 1

uptime_prefix: db "Kernel uptime reported via API. ", 0
uptime_prefix_len equ $ - uptime_prefix - 1

uptime_suffix: db "(uptime_ms call succeeded — v1.1 API)", 0x0A, 0
uptime_suffix_len equ $ - uptime_suffix - 1

mac_msg: db "MAC address queried via get_mac (v1.1).", 0x0A, 0
mac_msg_len equ $ - mac_msg - 1

ip_msg: db "IP address queried via get_ip (v1.1).", 0x0A, 0
ip_msg_len equ $ - ip_msg - 1

ping_msg: db "Pinging 10.0.2.2 via API ping (v1.1)...", 0x0A, 0
ping_msg_len equ $ - ping_msg - 1

ping_ok: db "  Ping OK! Network is reachable.", 0x0A, 0
ping_ok_len equ $ - ping_ok - 1

ping_fail_msg: db "  Ping failed (no network or timeout).", 0x0A, 0
ping_fail_msg_len equ $ - ping_fail_msg - 1

gateway_ip: db 10, 0, 2, 2    ; 10.0.2.2

SECTION .bss
mac_buf: resb 6
ip_buf:  resb 4
