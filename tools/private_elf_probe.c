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
#ifdef DEIX_PROBE_FAULT
    (void)*(volatile unsigned char *)0x100000;
    call4(60, 99, 0, 0, 0);
#else
    struct message message;
    long pid = call4(0x44580003, 0, 0, 0, 0);
    if (pid <= 0 || call4(0x44580001, pid, 123, 456, 789) != 0 ||
        call4(0x44580002, 0x100000, 0, 0, 0) != -14 ||
        call4(0x44580002, (unsigned long)&message, 0, 0, 0) != 0 ||
        message.sender != (unsigned)pid || message.reserved != 0 ||
        message.tag != 123 || message.a != 456 || message.b != 789 ||
        call4(0x44580002, (unsigned long)&message, 0, 0, 0) != -11)
        call4(60, 42, 0, 0, 0);
    unsigned long cs;
    __asm__ volatile("mov %%cs, %0" : "=r"(cs));
    call4(0x4458564d, 0x31, cs, 0, 0);
#endif
    for (;;) __asm__ volatile("ud2");
}
