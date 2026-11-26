/* QEMU-only client of the managed-service lookup ABI; not Dinit. */
static inline long find(const void *ptr, unsigned long len) {
    long result;
    __asm__ volatile("syscall" : "=a"(result) : "a"(0x44580006UL), "D"(ptr), "S"(len) : "rcx", "r11", "memory");
    return result;
}
__attribute__((noreturn)) void _start(void) {
    static const char service[] = "managed-recovery-test";
    static const char absent[] = "not-a-registered-service";
    static const char invalid[] = "\xff";
    long code = 0;
    if (find(service, sizeof(service)-1) <= 0) code = 1;
    else if (find(absent, sizeof(absent)-1) != -2) code = 2;
    else if (find((void *)0x100000, 1) != -14) code = 3;
    else if (find(service, 0) != -22 || find(service, 129) != -22) code = 4;
    else if (find(invalid, 1) != -22) code = 5;
    __asm__ volatile("syscall" : : "a"(60UL), "D"(code) : "rcx", "r11", "memory");
    for (;;) __asm__ volatile("ud2");
}
