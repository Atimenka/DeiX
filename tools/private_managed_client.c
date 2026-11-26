/* QEMU-only client of the managed-service lookup ABI; not Dinit. */
static inline long find(const void *ptr, unsigned long len) {
    long result;
    __asm__ volatile("syscall" : "=a"(result) : "a"(0x44580006UL), "D"(ptr), "S"(len) : "rcx", "r11", "memory");
    return result;
}
struct process_status { unsigned pid, state; int exit_code; unsigned has_exit; };
static inline long status(unsigned long pid, void *ptr) {
    long result;
    __asm__ volatile("syscall" : "=a"(result) : "a"(0x44580007UL), "D"(pid), "S"(ptr) : "rcx", "r11", "memory");
    return result;
}
__attribute__((noreturn)) void _start(void) {
    static const char service[] = "managed-recovery-test";
    static const char absent[] = "not-a-registered-service";
    static const char invalid[] = "\xff";
    long code = 0;
    struct process_status info;
    long pid = find(service, sizeof(service)-1);
    if (pid <= 0) code = 1;
    else if (status(pid, &info) != 0 || info.pid != (unsigned)pid || info.state != 2 || info.has_exit != 0) code = 6;
    else if (status(pid, (void *)0x100000) != -14 || status(0xffffffffUL, &info) != -2) code = 7;
    else if (find(absent, sizeof(absent)-1) != -2) code = 2;
    else if (find((void *)0x100000, 1) != -14) code = 3;
    else if (find(service, 0) != -22 || find(service, 129) != -22) code = 4;
    else if (find(invalid, 1) != -22) code = 5;
    __asm__ volatile("syscall" : : "a"(60UL), "D"(code) : "rcx", "r11", "memory");
    for (;;) __asm__ volatile("ud2");
}
