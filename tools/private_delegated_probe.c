/* CI-only child: prove explicitly delegated IPC send reaches its parent. */
static inline long call4(unsigned long nr, unsigned long a, unsigned long b, unsigned long c, unsigned long d) {
    register unsigned long r10 __asm__("r10") = d;
    long result;
    __asm__ volatile("syscall" : "=a"(result) : "a"(nr), "D"(a), "S"(b), "d"(c), "r"(r10) : "rcx", "r11", "memory");
    return result;
}
__attribute__((noreturn)) void _start(void) {
    long parent = call4(0x4458000cUL, 0, 0, 0, 0);
    if (parent <= 0 || call4(0x44580001UL, parent, 0x99, 0x1234, 0) != 0)
        call4(60, 51, 0, 0, 0);
    for (;;) __asm__ volatile("pause");
}
