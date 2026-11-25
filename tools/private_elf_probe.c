/* QEMU-only probe: this is not Dinit. It tests ELF loading from /system,
 * execution in CPL3, and the deliberately test-only syscall. */
__attribute__((noreturn)) void _start(void) {
#ifdef DEIX_PROBE_FAULT
    /* Deliberate supervisor-page read must fault in the private address space. */
    (void)*(volatile unsigned char *)0x100000;
    __asm__ volatile("syscall" : : "a"(60UL), "D"(99UL) : "rcx", "r11", "memory");
#else
    unsigned long cs;
    __asm__ volatile("mov %%cs, %0" : "=r"(cs));
    __asm__ volatile("syscall" : : "a"(0x4458564dUL), "D"(0x31UL), "S"(cs) : "rcx", "r11", "memory");
#endif
    for (;;) __asm__ volatile("ud2");
}
