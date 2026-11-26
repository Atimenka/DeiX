/* CI-only process. It stays live until the supervisor stops it; not Dinit. */
__attribute__((noreturn)) void _start(void) {
    for (;;) __asm__ volatile("pause");
}
