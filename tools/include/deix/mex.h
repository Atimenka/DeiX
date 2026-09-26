#ifndef DEIX_MEX_H
#define DEIX_MEX_H

#include <stdint.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

// Базовые C-функции работы с памятью для freestanding-компиляции
__attribute__((weak)) void* memset(void* dest, int c, size_t n) {
    unsigned char* p = (unsigned char*)dest;
    while (n--) {
        *p++ = (unsigned char)c;
    }
    return dest;
}

__attribute__((weak)) void* memcpy(void* dest, const void* src, size_t n) {
    unsigned char* d = (unsigned char*)dest;
    const unsigned char* s = (const unsigned char*)src;
    while (n--) {
        *d++ = *s++;
    }
    return dest;
}

__attribute__((weak)) void* memmove(void* dest, const void* src, size_t n) {
    unsigned char* d = (unsigned char*)dest;
    const unsigned char* s = (const unsigned char*)src;
    if (d < s) {
        while (n--) *d++ = *s++;
    } else {
        d += n;
        s += n;
        while (n--) *--d = *--s;
    }
    return dest;
}

__attribute__((weak)) int memcmp(const void* s1, const void* s2, size_t n) {
    const unsigned char* p1 = (const unsigned char*)s1;
    const unsigned char* p2 = (const unsigned char*)s2;
    while (n--) {
        if (*p1 != *p2) return *p1 - *p2;
        p1++;
        p2++;
    }
    return 0;
}

typedef struct MexApi {
    void (*print)(const char* str, size_t len);
    int32_t (*read_char)(void);
    int32_t (*try_read_char)(void);
    void (*read_line)(char* buf, size_t max_len);
    void (*clear_screen)(void);
    void (*set_color)(uint8_t fg, uint8_t bg);
    
    void* (*alloc)(size_t size);
    void (*free)(void* ptr);
    void (*sleep_ms)(uint64_t ms);
    void (*yield)(void);
    
    void (*get_screen_size)(uint32_t* width, uint32_t* height);
    void (*fill_screen)(uint32_t color);
    void (*draw_pixel)(uint32_t x, uint32_t y, uint32_t color);
    void (*draw_rect)(uint32_t x, uint32_t y, uint32_t w, uint32_t h, uint32_t color);
    void (*draw_string)(uint32_t x, uint32_t y, const char* str, uint32_t color);
    void (*swap_buffers)(void);
    
    void (*get_mouse_state)(int32_t* x, int32_t* y, uint8_t* buttons);
    int32_t (*get_key_event)(uint8_t* scancode, uint8_t* flags);
    
    void (*beep)(uint32_t freq, uint32_t duration_ms);
    void (*play_sound)(const uint8_t* pcm_data, size_t len, uint32_t sample_rate);
} MexApi;

#ifdef __cplusplus
}
#endif

#endif // DEIX_MEX_H
