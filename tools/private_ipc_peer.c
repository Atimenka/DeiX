/* QEMU-only inter-process test: neither peer is Dinit. */
struct message { unsigned sender, reserved; unsigned long tag, a, b; };
static inline long call4(unsigned long nr, unsigned long a, unsigned long b, unsigned long c, unsigned long d) {
    register unsigned long r10 __asm__("r10") = d;
    long ret;
    __asm__ volatile("syscall" : "=a"(ret) : "a"(nr), "D"(a), "S"(b), "d"(c), "r"(r10) : "rcx", "r11", "memory");
    return ret;
}
static long receive(struct message *msg) {
    for (unsigned i = 0; i < 100000; ++i) {
        long r = call4(0x44580002, (unsigned long)msg, 0, 0, 0);
        if (r != -11) return r;
    }
    return -110;
}
__attribute__((noreturn)) void _start(void) {
    struct message msg;
    long result = 0;
#ifdef DEIX_IPC_SENDER
    /* Kernel test harness seeds the peer PID before this task starts. */
    if (receive(&msg) != 0 || msg.sender != 0 || msg.tag != 100 || !msg.a) result = 1;
    unsigned long peer = msg.a;
    if (!result && call4(0x44580001, peer, 200, 1234, 5678) != 0) result = 2;
    if (!result && (receive(&msg) != 0 || msg.sender != peer || msg.tag != 300 || msg.a != 4321)) result = 3;
#else
    if (receive(&msg) != 0 || !msg.sender || msg.tag != 200 || msg.a != 1234 || msg.b != 5678) result = 4;
    if (!result && call4(0x44580001, msg.sender, 300, 4321, 0) != 0) result = 5;
#endif
    call4(60, result, 0, 0, 0);
    for (;;) __asm__ volatile("ud2");
}
