/* QEMU-only client of the managed-service ABI; not Dinit. */
static inline long call4(unsigned long nr, unsigned long a, unsigned long b, unsigned long c, unsigned long d) {
    register unsigned long r10 __asm__("r10") = d;
    long result;
    __asm__ volatile("syscall" : "=a"(result) : "a"(nr), "D"(a), "S"(b), "d"(c), "r"(r10) : "rcx", "r11", "memory");
    return result;
}
static inline long call5(unsigned long nr, unsigned long a, unsigned long b, unsigned long c, unsigned long d, unsigned long e) {
    register unsigned long r10 __asm__("r10") = d;
    register unsigned long r8 __asm__("r8") = e;
    long result;
    __asm__ volatile("syscall" : "=a"(result) : "a"(nr), "D"(a), "S"(b), "d"(c), "r"(r10), "r"(r8) : "rcx", "r11", "memory");
    return result;
}
struct message { unsigned sender, reserved; unsigned long tag, a, b; };
static inline long find(const void *ptr, unsigned long len) { return call4(0x44580006UL, (unsigned long)ptr, len, 0, 0); }
struct process_status { unsigned pid, state; int exit_code; unsigned has_exit; };
struct result { int status; unsigned pid; };
static long wait_result(long token, struct result *out) {
    for (unsigned i = 0; i < 200000; i++) {
        long r = call4(0x4458000aUL, token, (unsigned long)out, 0, 0);
        if (r != -11) return r;
    }
    return -110;
}
__attribute__((noreturn)) void _start(void) {
    static const char service[] = "managed-recovery-test";
    static const char absent[] = "not-a-registered-service";
    static const char invalid[] = "\xff";
    static const char rpc[] = "rpc-service-test";
    static const char image[] = "/system/tests/private_managed_probe.elf";
    static const char badpath[] = "/system/services/../kernel/invalid.elf";
    long code = 0;
    struct process_status info;
    struct result result;
    long pid = find(service, sizeof(service)-1);
    if (pid <= 0) code = 1;
    else if (call4(0x44580007UL, pid, (unsigned long)&info, 0, 0) != 0 || info.pid != (unsigned)pid || info.state != 2 || info.has_exit != 0) code = 6;
    else if (call4(0x44580007UL, pid, 0x100000, 0, 0) != -14 || call4(0x44580007UL, 0xffffffffUL, (unsigned long)&info, 0, 0) != -2) code = 7;
    else if (find(absent, sizeof(absent)-1) != -2) code = 2;
    else if (find((void *)0x100000, 1) != -14) code = 3;
    else if (find(service, 0) != -22 || find(service, 129) != -22) code = 4;
    else if (find(invalid, 1) != -22) code = 5;
    else if (call4(0x44580008UL, (unsigned long)rpc, sizeof(rpc)-1, (unsigned long)badpath, sizeof(badpath)-1) != -22) code = 8;
    else {
        long token = call4(0x44580008UL, (unsigned long)rpc, sizeof(rpc)-1, (unsigned long)image, sizeof(image)-1);
        if (token <= 0 || call4(0x4458000aUL, token, 0x100000, 0, 0) != -14 || wait_result(token, &result) != 0 || result.status != 0 || !result.pid) code = 9;
        else {
            unsigned child = result.pid;
            if (find(rpc, sizeof(rpc)-1) != child || call4(0x4458000aUL, token, (unsigned long)&result, 0, 0) != -2) code = 10;
            else {
                token = call4(0x44580008UL, (unsigned long)rpc, sizeof(rpc)-1, (unsigned long)image, sizeof(image)-1);
                if (token <= 0 || wait_result(token, &result) != 0 || result.status != -17 || result.pid != child) code = 11;
                else {
                    token = call4(0x44580009UL, (unsigned long)rpc, sizeof(rpc)-1, child, 0);
                    if (token <= 0 || wait_result(token, &result) != 0 || result.status != 0 || result.pid != child || find(rpc, sizeof(rpc)-1) != -2) code = 12;
                }
            }
        }
    }
    if (!code) {
        unsigned char config[8];
        long token = call4(0x4458000dUL, 0, 0, 0, 0);
        if (token <= 0 || wait_result(token, &result) != 0 || result.status != 0 ||
            call4(0x44580004UL, (unsigned long)config, 0, sizeof(config), 0) != sizeof(config) ||
            config[0] != '#' || config[1] != ' ') code = 17;
    }
    if (!code) {
        static const char delegated[] = "delegated-ipc-test";
        static const char delegated_image[] = "/system/tests/private_delegated_probe.elf";
        const unsigned long ipc_send = 1UL << 7;
        long denied = call5(0x4458000bUL, (unsigned long)delegated, sizeof(delegated)-1,
                            (unsigned long)delegated_image, sizeof(delegated_image)-1, 1UL << 5);
        if (denied != -1) code = 13;
        else {
            long token = call5(0x4458000bUL, (unsigned long)delegated, sizeof(delegated)-1,
                               (unsigned long)delegated_image, sizeof(delegated_image)-1, ipc_send);
            if (token <= 0 || wait_result(token, &result) != 0 || result.status != 0 || !result.pid) code = 14;
            else {
                unsigned child = result.pid;
                struct message msg;
                long rc = -11;
                for (unsigned i = 0; i < 200000 && rc == -11; i++)
                    rc = call4(0x44580002UL, (unsigned long)&msg, 0, 0, 0);
                if (rc != 0 || msg.sender != child || msg.tag != 0x99 || msg.a != 0x1234) code = 15;
                token = call4(0x44580009UL, (unsigned long)delegated, sizeof(delegated)-1, child, 0);
                if (token <= 0 || wait_result(token, &result) != 0 || result.status != 0 || result.pid != child) code = 16;
            }
        }
    }
    call4(60, code, 0, 0, 0);
    for (;;) __asm__ volatile("ud2");
}
