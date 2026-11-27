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
struct managed_snapshot { unsigned pid, state, uid, path_len; unsigned long caps; char path[256]; };
struct process_status { unsigned pid, state; int exit_code; unsigned has_exit; };
struct result { int status; unsigned pid; };
struct managed_listing { unsigned pid, state, uid, name_len, path_len, flags; unsigned long caps; char name[128], path[256]; };
struct heartbeat { unsigned health, has_last; unsigned long last, now; };
struct zombie { unsigned pid, name_len; int code; unsigned reserved; char name[128]; };
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
    else {
        struct managed_snapshot snap;
        long r = call4(0x4458000fUL, (unsigned long)service, sizeof(service)-1, (unsigned long)&snap, 0);
        if (r != 0 || snap.pid != (unsigned)pid || snap.state != 2 || snap.uid != 0 || snap.caps != 127 || snap.path_len != sizeof(image)-1) code = 19;
        for (unsigned i = 0; !code && i < sizeof(image)-1; i++) if (snap.path[i] != image[i]) code = 19;
        struct managed_listing listing;
        if (!code && (call4(0x44580017UL, pid-1, (unsigned long)&listing, 0, 0) != 0 ||
            listing.pid != (unsigned)pid || listing.state != 2 || listing.uid != 0 || listing.caps != 127 ||
            listing.name_len != sizeof(service)-1 || listing.path_len != sizeof(image)-1 || listing.flags)) code = 29;
        for (unsigned i = 0; !code && i < sizeof(service)-1; i++) if (listing.name[i] != service[i]) code = 29;
        for (unsigned i = 0; !code && i < sizeof(image)-1; i++) if (listing.path[i] != image[i]) code = 29;
        if (!code && (call4(0x44580017UL, pid-1, 0x100000, 0, 0) != -14 ||
            call4(0x44580017UL, 0xffffffffUL, (unsigned long)&listing, 0, 0) != -2 ||
            call4(0x44580017UL, 0x100000000UL, (unsigned long)&listing, 0, 0) != -22)) code = 30;
        if (!code && (call4(0x4458000fUL, (unsigned long)service, sizeof(service)-1, 0x100000, 0) != -14 ||
            call4(0x4458000fUL, (unsigned long)absent, sizeof(absent)-1, (unsigned long)&snap, 0) != -2)) code = 20;
    }
    if (!code && find(absent, sizeof(absent)-1) != -2) code = 2;
    else if (!code && find((void *)0x100000, 1) != -14) code = 3;
    else if (!code && (find(service, 0) != -22 || find(service, 129) != -22)) code = 4;
    else if (!code && find(invalid, 1) != -22) code = 5;
    else if (!code && call4(0x44580008UL, (unsigned long)rpc, sizeof(rpc)-1, (unsigned long)badpath, sizeof(badpath)-1) != -22) code = 8;
    else if (!code) {
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
        static const char name[] = "disguised-legacy-test";
        static const char path[] = "/system/tests/disguised_mex.elf";
        long token = call4(0x44580008UL, (unsigned long)name, sizeof(name)-1, (unsigned long)path, sizeof(path)-1);
        if (token <= 0 || wait_result(token, &result) != 0 || result.status != -5 ||
            find(name, sizeof(name)-1) != -2) code = 27;
    }
    if (!code) {
        static const char dead_name[] = "exited-service-test";
        static const char dead_image[] = "/system/tests/private_exit_probe.elf";
        struct zombie z;
        int exit_code = 0;
        long token = call4(0x44580008UL, (unsigned long)dead_name, sizeof(dead_name)-1, (unsigned long)dead_image, sizeof(dead_image)-1);
        if (token <= 0 || wait_result(token, &result) != 0 || result.status != 0 || !result.pid) code = 24;
        else {
            unsigned dead = result.pid;
            long r = -2;
            for (unsigned i = 0; i < 200000 && r == -2; i++)
                r = call4(0x44580015UL, dead-1, (unsigned long)&z, 0, 0);
            if (r != 0 || z.pid != dead || z.code != 37 || z.name_len != sizeof(dead_name)-1 || z.reserved) code = 25;
            for (unsigned i = 0; !code && i < sizeof(dead_name)-1; i++) if (z.name[i] != dead_name[i]) code = 25;
            if (!code && (call4(0x44580014UL, (unsigned long)rpc, sizeof(rpc)-1, dead, (unsigned long)&exit_code) != -2 ||
                call4(0x44580014UL, (unsigned long)dead_name, sizeof(dead_name)-1, dead, 0x100000) != -14 ||
                call4(0x44580014UL, (unsigned long)dead_name, sizeof(dead_name)-1, dead, (unsigned long)&exit_code) != 0 || exit_code != 37 ||
                call4(0x44580014UL, (unsigned long)dead_name, sizeof(dead_name)-1, dead, (unsigned long)&exit_code) != -2)) code = 26;
        }
    }
    if (!code) {
        char session[48];
        if (call4(0x44580013UL, (unsigned long)session, 0, 0, 0) != -2) code = 23;
    }
    if (!code) {
        long first = call4(0x44580010UL, 0, 0, 0, 0);
        long second = call4(0x44580010UL, 0, 0, 0, 0);
        if (first < 0 || second < first) code = 21;
    }
    if (!code) {
        unsigned char config[8];
        long old_generation = call4(0x44580011UL, 0, 0, 0, 0);
        long token = call4(0x4458000dUL, 0, 0, 0, 0);
        if (old_generation <= 0 || token <= 0 || wait_result(token, &result) != 0 || result.status != 0 ||
            call4(0x44580004UL, (unsigned long)config, 0, sizeof(config), 0) != sizeof(config) ||
            config[0] != '#' || config[1] != ' ') code = 17;
        long generation = call4(0x44580011UL, 0, 0, 0, 0);
        if (!code && (generation <= old_generation ||
            call4(0x44580012UL, (unsigned long)config, 0, sizeof(config), old_generation) != -116 ||
            call4(0x44580012UL, (unsigned long)config, 0, sizeof(config), generation) != sizeof(config) ||
            call4(0x44580012UL, 0x100000, 0, 1, generation) != -14)) code = 22;
    }
    if (!code) {
        static const char event[] = "dinit-prep-event";
        static const char bad_event[] = "forged\nrecord";
        long token = call4(0x4458000eUL, (unsigned long)event, sizeof(event)-1, 0, 0);
        if (token <= 0 || wait_result(token, &result) != 0 || result.status != 0 || !result.pid ||
            call4(0x4458000eUL, (unsigned long)bad_event, sizeof(bad_event)-1, 0, 0) != -22 ||
            call4(0x4458000eUL, 0x100000, 1, 0, 0) != -14) code = 18;
    }
    if (!code) {
        static const char delegated[] = "delegated-ipc-test";
        static const char delegated_image[] = "/system/tests/private_delegated_probe.elf";
        const unsigned long ipc_send = (1UL << 7) | (1UL << 10);
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
                struct heartbeat hb;
                if (!code && (call4(0x44580016UL, child, 10000, (unsigned long)&hb, 0) != 0 ||
                    hb.health != 2 || hb.has_last != 1 || hb.last > hb.now ||
                    call4(0x44580016UL, child, 10000, 0x100000, 0) != -14 ||
                    call4(0x44580016UL, 0xffffffffUL, 10000, (unsigned long)&hb, 0) != -2)) code = 28;
                token = call4(0x44580009UL, (unsigned long)delegated, sizeof(delegated)-1, child, 0);
                if (token <= 0 || wait_result(token, &result) != 0 || result.status != 0 || result.pid != child) code = 16;
            }
        }
    }
    call4(60, code, 0, 0, 0);
    for (;;) __asm__ volatile("ud2");
}
