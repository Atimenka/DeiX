/* Тестовая программа для прослойки совместимости с Linux.
 * Собирается как СТАТИЧЕСКИЙ PIE и использует только настоящие
 * системные вызовы Linux x86-64 — никакого кода DeiX внутри нет.
 * Именно поэтому её запуск доказывает, что прослойка работает. */
static long sys1(long n, long a) {
    long r; __asm__ volatile("syscall" : "=a"(r) : "a"(n), "D"(a) : "rcx","r11","memory"); return r;
}
static long sys3(long n, long a, long b, long c) {
    long r; __asm__ volatile("syscall" : "=a"(r) : "a"(n),"D"(a),"S"(b),"d"(c) : "rcx","r11","memory"); return r;
}
static unsigned slen(const char *s){unsigned n=0;while(s[n])n++;return n;}
static void puts_(const char *s){ sys3(1 /*write*/, 1, (long)s, slen(s)); }

void _start(void) {
    puts_("  [prog] Здравствуй из настоящей программы Linux!\n");

    long pid = sys1(39 /*getpid*/, 0);
    puts_(pid == 1 ? "  [prog] getpid() = 1\n" : "  [prog] getpid() вернул иное\n");

    /* uname: проверяем, что ядро представляется как DeiX.
       Буфер обнуляем сами: если вызов не поддержан, puts_ не уйдёт
       читать мусор за пределы массива. */
    static char uts[65*6];   /* статический, а не на стеке: 390 байт
                                не должны занимать почти весь кадр */
    for (int i = 0; i < 65*6; i++) uts[i] = 0;
    long ur = sys1(63 /*uname*/, (long)uts);
    if (ur == 0 && uts[0]) { puts_("  [prog] uname() -> "); puts_(uts); puts_("\n"); }
    else puts_("  [prog] uname() не поддержан\n");

    /* brk: запрашиваем границу кучи и двигаем её */
    long b0 = sys1(12 /*brk*/, 0);
    long b1 = sys1(12, b0 + 4096);
    puts_(b1 >= b0 + 4096 ? "  [prog] brk() расширил кучу\n"
                          : "  [prog] brk() не сработал\n");

    /* пишем в полученную память — проверяем, что она настоящая */
    volatile char *p = (volatile char *)b0;
    p[0] = 42; p[4095] = 7;
    puts_((p[0]==42 && p[4095]==7) ? "  [prog] память из brk() доступна на запись\n"
                                   : "  [prog] память из brk() НЕ работает\n");

    sys3(60 /*exit*/, 0, 0, 0);
    __builtin_unreachable();
}
