/* CI-only managed process: exit immediately with a recognizable status. */
__attribute__((noreturn)) void _start(void) {
    __asm__ volatile("syscall" : : "a"(60UL), "D"(37UL) : "rcx", "r11", "memory");
    for (;;) __asm__ volatile("ud2");
}
