/*
 * calc.cpp — Полноценное графическое C++ приложение "Калькулятор" для DeiX OS (v1.2 MexApi)
 * Поддерживает графический интерфейс, кнопки 0-9, операции +, -, *, /, Clear, '=',
 * обработку ввода клавиатуры и мыши, вычисление выражений.
 */

#include "include/deix/mex.hpp"

class CalculatorEngine {
private:
    char display[32];
    int display_len;
    int64_t current_val;
    int64_t stored_val;
    char pending_op;

public:
    CalculatorEngine() { reset(); }

    void reset() {
        display[0] = '0';
        display[1] = '\0';
        display_len = 1;
        current_val = 0;
        stored_val = 0;
        pending_op = 0;
    }

    void input_digit(char digit) {
        if (display_len == 1 && display[0] == '0') {
            display[0] = digit;
            display[1] = '\0';
            display_len = 1;
        } else if (display_len < 16) {
            display[display_len++] = digit;
            display[display_len] = '\0';
        }
        current_val = parse_int(display);
    }

    void set_operation(char op) {
        if (pending_op != 0) {
            evaluate();
        }
        stored_val = current_val;
        pending_op = op;
        display[0] = '0';
        display[1] = '\0';
        display_len = 1;
        current_val = 0;
    }

    void evaluate() {
        if (pending_op == '+') current_val = stored_val + current_val;
        else if (pending_op == '-') current_val = stored_val - current_val;
        else if (pending_op == '*') current_val = stored_val * current_val;
        else if (pending_op == '/' && current_val != 0) current_val = stored_val / current_val;

        pending_op = 0;
        int_to_str(current_val, display);
        display_len = str_len(display);
    }

    const char* get_display_text() const { return display; }
    char get_pending_op() const { return pending_op; }

private:
    static int64_t parse_int(const char* str) {
        int64_t val = 0;
        int sign = 1;
        if (*str == '-') { sign = -1; str++; }
        while (*str >= '0' && *str <= '9') {
            val = val * 10 + (*str - '0');
            str++;
        }
        return val * sign;
    }

    static int str_len(const char* str) {
        int len = 0;
        while (str[len]) len++;
        return len;
    }

    static void int_to_str(int64_t val, char* buf) {
        if (val == 0) {
            buf[0] = '0'; buf[1] = '\0';
            return;
        }
        char temp[32];
        int idx = 0;
        bool neg = val < 0;
        if (neg) val = -val;
        while (val > 0) {
            temp[idx++] = '0' + (val % 10);
            val /= 10;
        }
        int b_idx = 0;
        if (neg) buf[b_idx++] = '-';
        while (idx > 0) {
            buf[b_idx++] = temp[--idx];
        }
        buf[b_idx] = '\0';
    }
};

struct Button {
    uint32_t x, y, w, h;
    const char* label;
    char key;
};

extern "C" int64_t mex_main(const MexApi* api) {
    DeiX::MexContext::init(api);

    DeiX::Console::println("=== DeiX GUI Calculator (C++ Application v1.2) ===");
    DeiX::System::beep(1000, 100);

    CalculatorEngine calc;

    uint32_t screen_w = 800, screen_h = 600;
    DeiX::Graphics::getScreenSize(screen_w, screen_h);

    uint32_t win_x = (screen_w > 320) ? (screen_w - 320) / 2 : 10;
    uint32_t win_y = (screen_h > 420) ? (screen_h - 420) / 2 : 10;

    Button buttons[] = {
        {win_x + 20,  win_y + 100, 60, 50, "C", 'C'},
        {win_x + 90,  win_y + 100, 60, 50, "(", '('},
        {win_x + 160, win_y + 100, 60, 50, ")", ')'},
        {win_x + 230, win_y + 100, 60, 50, "/", '/'},

        {win_x + 20,  win_y + 160, 60, 50, "7", '7'},
        {win_x + 90,  win_y + 160, 60, 50, "8", '8'},
        {win_x + 160, win_y + 160, 60, 50, "9", '9'},
        {win_x + 230, win_y + 160, 60, 50, "*", '*'},

        {win_x + 20,  win_y + 220, 60, 50, "4", '4'},
        {win_x + 90,  win_y + 220, 60, 50, "5", '5'},
        {win_x + 160, win_y + 220, 60, 50, "6", '6'},
        {win_x + 230, win_y + 220, 60, 50, "-", '-'},

        {win_x + 20,  win_y + 280, 60, 50, "1", '1'},
        {win_x + 90,  win_y + 280, 60, 50, "2", '2'},
        {win_x + 160, win_y + 280, 60, 50, "3", '3'},
        {win_x + 230, win_y + 280, 60, 50, "+", '+'},

        {win_x + 20,  win_y + 340, 130, 50, "0", '0'},
        {win_x + 160, win_y + 340, 60,  50, ".", '.'},
        {win_x + 230, win_y + 340, 60,  50, "=", '='}
    };
    int num_buttons = sizeof(buttons) / sizeof(buttons[0]);

    bool running = true;
    bool prev_mouse_down = false;

    while (running) {
        // Отрисовка фона и окна калькулятора
        DeiX::Graphics::fillScreen(0x001B2230); // Темно-серый экран
        DeiX::Graphics::drawRect(win_x, win_y, 310, 410, 0x002B3548); // Рамка окна
        DeiX::Graphics::drawRect(win_x + 10, win_y + 10, 290, 30, 0x00111622); // Заголовок
        DeiX::Graphics::drawString(win_x + 20, win_y + 18, "DeiX C++ Calculator", 0x00E0E6ED);

        // Дисплей ввода/результата
        DeiX::Graphics::drawRect(win_x + 15, win_y + 50, 280, 40, 0x000B0E17);
        DeiX::Graphics::drawString(win_x + 30, win_y + 62, calc.get_display_text(), 0x0000FFCC);

        // Отрисовка кнопок
        for (int i = 0; i < num_buttons; i++) {
            uint32_t btn_color = (buttons[i].key == '=') ? 0x0027AE60 : 0x003D4C63;
            DeiX::Graphics::drawRect(buttons[i].x, buttons[i].y, buttons[i].w, buttons[i].h, btn_color);
            DeiX::Graphics::drawString(buttons[i].x + buttons[i].w / 2 - 4, buttons[i].y + 18, buttons[i].label, 0x00FFFFFF);
        }

        DeiX::Graphics::swapBuffers();

        // Обработка мыши
        auto mouse = DeiX::Input::getMouseState();
        bool mouse_click = mouse.left() && !prev_mouse_down;
        prev_mouse_down = mouse.left();

        if (mouse_click) {
            for (int i = 0; i < num_buttons; i++) {
                if (mouse.x >= (int32_t)buttons[i].x && mouse.x <= (int32_t)(buttons[i].x + buttons[i].w) &&
                    mouse.y >= (int32_t)buttons[i].y && mouse.y <= (int32_t)(buttons[i].y + buttons[i].h)) {
                    
                    DeiX::System::beep(800, 30);
                    char k = buttons[i].key;

                    if (k >= '0' && k <= '9') {
                        calc.input_digit(k);
                    } else if (k == '+' || k == '-' || k == '*' || k == '/') {
                        calc.set_operation(k);
                    } else if (k == '=') {
                        calc.evaluate();
                    } else if (k == 'C') {
                        calc.reset();
                    }
                }
            }
        }

        // Обработка клавиш
        int32_t key = DeiX::Console::tryReadChar();
        if (key > 0) {
            if (key >= '0' && key <= '9') {
                calc.input_digit((char)key);
            } else if (key == '+' || key == '-' || key == '*' || key == '/') {
                calc.set_operation((char)key);
            } else if (key == '=' || key == '\r' || key == '\n') {
                calc.evaluate();
            } else if (key == 'c' || key == 'C') {
                calc.reset();
            } else if (key == 'q' || key == 27) { // ESC или 'q' для выхода
                running = false;
            }
        }

        DeiX::System::sleepMs(20);
    }

    DeiX::Console::println("Приложение Калькулятор закрыто.");
    return 0;
}
