/* Test-only ELF, not Dinit. */
struct message { unsigned sender, reserved; unsigned long tag, a, b; };
static inline long call4(unsigned long nr, unsigned long a, unsigned long b,
                         unsigned long c, unsigned long d) {
    register unsigned long r10 __asm__("r10") = d;
    long result;
    __asm__ volatile("syscall" : "=a"(result) : "a"(nr), "D"(a), "S"(b), "d"(c), "r"(r10) : "rcx", "r11", "memory");
    return result;
}
__attribute__((noreturn)) void _start(void) {
#if defined(DEIX_PROBE_NX_STACK)
    volatile unsigned char code[] = { 0xc3 }; /* RET */
    ((void (*)(void))(unsigned long)&code[0])();
    /* Reaching here would prove execution from the writable stack succeeded. */
    call4(60, 99, 0, 0, 0);
#elif defined(DEIX_PROBE_FAULT)
    (void)*(volatile unsigned char *)0x100000;
    call4(60, 99, 0, 0, 0);
#elif defined(DEIX_PROBE_DENIED)
    struct message message;
    long pid = call4(0x44580003, 0, 0, 0, 0);
    if (pid <= 0 || call4(0x44580001, pid, 1, 2, 3) != -1 ||
        call4(0x44580002, (unsigned long)&message, 0, 0, 0) != -1 ||
        call4(0x44580004, (unsigned long)&message, 0, 1, 0) != -1 ||
        call4(0x44580005, 0, 0, 0, 0) != -1 ||
        call4(0x44580006, (unsigned long)&message, 1, 0, 0) != -1 ||
        call4(0x44580007, 1, (unsigned long)&message, 0, 0) != -1 ||
        call4(0x44580008, (unsigned long)&message, 1, (unsigned long)&message, 1) != -1 ||
        call4(0x44580009, (unsigned long)&message, 1, 1, 0) != -1 ||
        call4(0x4458000a, 1, (unsigned long)&message, 0, 0) != -1 ||
        call4(0x4458000b, (unsigned long)&message, 1, (unsigned long)&message, 1) != -1 ||
        call4(0x4458000d, 0, 0, 0, 0) != -1 ||
        call4(0x4458000e, (unsigned long)&message, 1, 0, 0) != -1 ||
        call4(0x4458000f, (unsigned long)&message, 1, (unsigned long)&message, 0) != -1 ||
        call4(0x44580011, 0, 0, 0, 0) != -1 ||
        call4(0x44580012, (unsigned long)&message, 0, 1, 1) != -1 ||
        call4(0x44580013, (unsigned long)&message, 0, 0, 0) != -1)
        call4(60, 42, 0, 0, 0);
    unsigned long cs;
    __asm__ volatile("mov %%cs, %0" : "=r"(cs));
    call4(0x4458564d, 0x72, cs, 0, 0);
#else
    struct message message;
    unsigned char config[8];
    if (call4(0x44580005, 0, 0, 0, 0) != 0) call4(60, 48, 0, 0, 0);
    if (call4(0x44580004, (unsigned long)config, 0, 8, 0) != 8 || config[0] != '#' || config[1] != ' ' ||
        call4(0x44580004, 0x100000, 0, 8, 0) != -14 ||
        call4(0x44580004, (unsigned long)config, 0, 1025, 0) != -22 ||
        call4(0x44580004, (unsigned long)config, ~0UL, 8, 0) != 0)
        call4(60, 47, 0, 0, 0);
    long pid = call4(0x44580003, 0, 0, 0, 0);
    if (pid <= 0) call4(60, 41, 0, 0, 0);
    if (call4(0x44580001, pid, 123, 456, 789) != 0) call4(60, 42, 0, 0, 0);
    if (call4(0x44580002, 0x100000, 0, 0, 0) != -14) call4(60, 43, 0, 0, 0);
    if (call4(0x44580002, (unsigned long)&message, 0, 0, 0) != 0) call4(60, 44, 0, 0, 0);
    if (message.sender != (unsigned)pid || message.reserved != 0 ||
        message.tag != 123 || message.a != 456 || message.b != 789) call4(60, 45, 0, 0, 0);
    if (call4(0x44580002, (unsigned long)&message, 0, 0, 0) != -11) call4(60, 46, 0, 0, 0);
    unsigned long cs;
    __asm__ volatile("mov %%cs, %0" : "=r"(cs));
    call4(0x4458564d, 0x31, cs, 0, 0);
#endif
    for (;;) __asm__ volatile("ud2");
}
