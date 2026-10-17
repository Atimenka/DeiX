#ifndef DEIX_MEX_HPP
#define DEIX_MEX_HPP

#include "mex.h"

namespace DeiX {

class MexContext {
public:
    static const MexApi* api;
    static void init(const MexApi* api_ptr) {
        api = api_ptr;
    }
};

inline const MexApi* MexContext::api = nullptr;

class Console {
public:
    static void print(const char* str) {
        if (!MexContext::api || !MexContext::api->print) return;
        size_t len = 0;
        while (str[len]) len++;
        MexContext::api->print(str, len);
    }

    static void println(const char* str) {
        print(str);
        print("\n");
    }

    static int32_t readChar() {
        return MexContext::api && MexContext::api->read_char ? MexContext::api->read_char() : -1;
    }

    static int32_t tryReadChar() {
        return MexContext::api && MexContext::api->try_read_char ? MexContext::api->try_read_char() : -1;
    }

    static void clearScreen() {
        if (MexContext::api && MexContext::api->clear_screen) MexContext::api->clear_screen();
    }
};

class System {
public:
    static void* alloc(size_t size) {
        return MexContext::api && MexContext::api->alloc ? MexContext::api->alloc(size) : nullptr;
    }

    static void free(void* ptr) {
        if (MexContext::api && MexContext::api->free) MexContext::api->free(ptr);
    }

    static void sleepMs(uint64_t ms) {
        if (MexContext::api && MexContext::api->sleep_ms) MexContext::api->sleep_ms(ms);
    }

    static void yield() {
        if (MexContext::api && MexContext::api->yield) MexContext::api->yield();
    }

    static void beep(uint32_t freq, uint32_t duration_ms) {
        if (MexContext::api && MexContext::api->beep) MexContext::api->beep(freq, duration_ms);
    }
};

class Graphics {
public:
    static void getScreenSize(uint32_t& w, uint32_t& h) {
        if (MexContext::api && MexContext::api->get_screen_size) MexContext::api->get_screen_size(&w, &h);
    }

    static void fillScreen(uint32_t color) {
        if (MexContext::api && MexContext::api->fill_screen) MexContext::api->fill_screen(color);
    }

    static void drawPixel(uint32_t x, uint32_t y, uint32_t color) {
        if (MexContext::api && MexContext::api->draw_pixel) MexContext::api->draw_pixel(x, y, color);
    }

    static void drawRect(uint32_t x, uint32_t y, uint32_t w, uint32_t h, uint32_t color) {
        if (MexContext::api && MexContext::api->draw_rect) MexContext::api->draw_rect(x, y, w, h, color);
    }

    static void drawString(uint32_t x, uint32_t y, const char* str, uint32_t color) {
        if (MexContext::api && MexContext::api->draw_string) MexContext::api->draw_string(x, y, str, color);
    }

    static void swapBuffers() {
        if (MexContext::api && MexContext::api->swap_buffers) MexContext::api->swap_buffers();
    }
};

struct MouseState {
    int32_t x;
    int32_t y;
    uint8_t buttons;

    bool left() const { return (buttons & 1) != 0; }
    bool right() const { return (buttons & 2) != 0; }
};

class Input {
public:
    static MouseState getMouseState() {
        MouseState ms{0, 0, 0};
        if (MexContext::api && MexContext::api->get_mouse_state) {
            MexContext::api->get_mouse_state(&ms.x, &ms.y, &ms.buttons);
        }
        return ms;
    }
};

} // namespace DeiX

#endif // DEIX_MEX_HPP
