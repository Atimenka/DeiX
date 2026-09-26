; netping.asm — .mex v1.1 demo: network ping utility.
; Usage: run NETPING.MEX 10.0.2.2
;
; Uses the v1.1 MexApi network functions (ping, get_ip, get_mac).

BITS 64
DEFAULT REL

SECTION .text

global _start
_start:
    mov r12, rdi                ; save &api

    ; --- Header ---
    mov rdi, [r12]
    lea rsi, [rel banner]
    mov rdx, banner_len
    call rdi

    ; --- Get MAC ---
    mov rax, [r12 + 0x38]       ; api->get_mac
    lea rdi, [rel mac_buf]
    call rax

    mov rdi, [r12]
    lea rsi, [rel mac_prefix]
    mov rdx, mac_prefix_len
    call rdi

    ; Print MAC bytes as hex (simplified)
    mov rdi, [r12]
    lea rsi, [rel mac_placeholder]
    mov rdx, mac_placeholder_len
    call rdi

    ; --- Get IP ---
    mov rax, [r12 + 0x40]       ; api->get_ip
    lea rdi, [rel ip_buf]
    call rax

    mov rdi, [r12]
    lea rsi, [rel ip_prefix]
    mov rdx, ip_prefix_len
    call rdi

    mov rdi, [r12]
    lea rsi, [rel ip_placeholder]
    mov rdx, ip_placeholder_len
    call rdi

    ; --- Ping 10.0.2.2 ---
    mov rdi, [r12]
    lea rsi, [rel ping_start]
    mov rdx, ping_start_len
    call rdi

    mov rax, [r12 + 0x30]       ; api->ping
    lea rdi, [rel target_ip]    ; 10.0.2.2
    mov rsi, 4
    mov rdx, 3000               ; timeout 3s
    call rax

    cmp rax, -1
    je .fail

    mov rdi, [r12]
    lea rsi, [rel ping_ok]
    mov rdx, ping_ok_len
    call rdi
    jmp .done

.fail:
    mov rdi, [r12]
    lea rsi, [rel ping_fail]
    mov rdx, ping_fail_len
    call rdi

.done:
    xor eax, eax
    ret

SECTION .rodata

banner: db "=== DeiX Network Ping (MEX v1.1) ===", 0x0A, 0
banner_len equ $ - banner - 1

mac_prefix: db "MAC: ", 0
mac_prefix_len equ $ - mac_prefix - 1

mac_placeholder: db "(get_mac v1.1 OK)", 0x0A, 0
mac_placeholder_len equ $ - mac_placeholder - 1

ip_prefix: db "IP:  ", 0
ip_prefix_len equ $ - ip_prefix - 1

ip_placeholder: db "(get_ip v1.1 OK)", 0x0A, 0
ip_placeholder_len equ $ - ip_placeholder - 1

ping_start: db "Pinging 10.0.2.2 (gateway)...", 0x0A, 0
ping_start_len equ $ - ping_start - 1

ping_ok: db "  Reply received! Network is working.", 0x0A, 0
ping_ok_len equ $ - ping_ok - 1

ping_fail: db "  No reply (network card not present or host unreachable).", 0x0A, 0
ping_fail_len equ $ - ping_fail - 1

target_ip: db 10, 0, 2, 2      ; 10.0.2.2

SECTION .bss
mac_buf: resb 6
ip_buf:  resb 4
