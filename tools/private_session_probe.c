/* CI-only: query the authenticated session through the actual CPL3 ABI. */
struct session { unsigned uid, len; unsigned long login_ms; char name[32]; };
static long call1(unsigned long nr, unsigned long a) {
    long result;
    __asm__ volatile("syscall" : "=a"(result) : "a"(nr), "D"(a) : "rcx", "r11", "memory");
    return result;
}
__attribute__((noreturn)) void _start(void) {
    struct session s = {0};
    long code = 0;
    if (call1(0x44580013UL, (unsigned long)&s) != 0 || s.len == 0 || s.len > 32 ||
        (s.uid != 0 && s.uid != 1000) || call1(0x44580013UL, 0x100000) != -14) code = 1;
    for (unsigned i = s.len; !code && i < 32; ++i) if (s.name[i]) code = 2;
    call1(60, code);
    for (;;) __asm__ volatile("ud2");
}
