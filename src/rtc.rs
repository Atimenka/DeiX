//! Драйвер аппаратных часов реального времени (CMOS RTC, порты 0x70/0x71).
//!
//! RTC нужен системе диагностики для расширенного идентификатора события
//! вида `DX-GFX-0008-20261006-083154` и для датировки отчётов об отказе:
//! `timer::uptime_ms()` обнуляется при каждой загрузке и не даёт абсолютного
//! времени.
//!
//! Регистры RTC обновляются асинхронно, поэтому чтение выполняется дважды и
//! сравнивается — стандартный приём, защищающий от разрыва значений в момент
//! внутреннего переноса счётчика.

use crate::port::{inb, outb};

/// Индексный порт RTC. Бит 7 запрещает NMI на время доступа.
const RTC_INDEX: u16 = 0x70;
/// Порт данных RTC.
const RTC_DATA: u16 = 0x71;

/// Регистр секунд.
const REG_SECONDS: u8 = 0x00;
/// Регистр минут.
const REG_MINUTES: u8 = 0x02;
/// Регистр часов.
const REG_HOURS: u8 = 0x04;
/// Регистр дня месяца.
const REG_DAY: u8 = 0x07;
/// Регистр месяца.
const REG_MONTH: u8 = 0x08;
/// Регистр года (00..99).
const REG_YEAR: u8 = 0x09;
/// Регистр состояния A: бит 7 — идёт обновление регистров.
const REG_STATUS_A: u8 = 0x0A;
/// Регистр состояния B: бит 2 — формат чисел, бит 1 — 12/24 часа.
const REG_STATUS_B: u8 = 0x0B;
/// Регистр века (есть не на всех чипсетах; при отсутствии даёт 0).
const REG_CENTURY: u8 = 0x32;

/// Момент времени по аппаратным часам.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WallClock {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

impl WallClock {
    /// Строка вида `2026-10-06 08:31:54`.
    pub fn as_string(&self) -> alloc::string::String {
        alloc::format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            self.year,
            self.month,
            self.day,
            self.hour,
            self.minute,
            self.second
        )
    }

    /// Компактная метка времени `20261006-083154` — хвост расширенного
    /// идентификатора ошибки.
    pub fn compact(&self) -> alloc::string::String {
        alloc::format!(
            "{:04}{:02}{:02}-{:02}{:02}{:02}",
            self.year,
            self.month,
            self.day,
            self.hour,
            self.minute,
            self.second
        )
    }

}

/// Читает регистр RTC.
fn read_reg(reg: u8) -> u8 {
    unsafe {
        // Бит 7 гасит NMI на время пары in/out, чтобы прерывание не
        // переключило индексный регистр между нашими обращениями.
        outb(RTC_INDEX, reg | 0x80);
        inb(RTC_DATA)
    }
}

/// Идёт ли сейчас внутреннее обновление регистров.
fn update_in_progress() -> bool {
    read_reg(REG_STATUS_A) & 0x80 != 0
}

/// Переводит BCD-значение в двоичное: десятки*10 + единицы.
/// Десятки умножаются на 10 как 8x + 2x: (v>>4)*10 = (v&0xF0)>>1 + (v&0xF0)>>3.
fn bcd_to_bin(value: u8) -> u8 {
    ((value & 0xF0) >> 1) + ((value & 0xF0) >> 3) + (value & 0x0F)
}

/// Снимок полей часов без приведения формата.
struct Raw {
    second: u8,
    minute: u8,
    hour: u8,
    day: u8,
    month: u8,
    year: u8,
    century: u8,
    status_b: u8,
}

/// Читает все нужные регистры одним заходом.
fn read_raw() -> Raw {
    let status_b = read_reg(REG_STATUS_B);
    Raw {
        second: read_reg(REG_SECONDS),
        minute: read_reg(REG_MINUTES),
        hour: read_reg(REG_HOURS),
        day: read_reg(REG_DAY),
        month: read_reg(REG_MONTH),
        year: read_reg(REG_YEAR),
        century: read_reg(REG_CENTURY),
        status_b,
    }
}

/// Приводит сырые значения к двоичному формату и 24-часовой шкале.
fn normalize(raw: Raw) -> WallClock {
    let binary = raw.status_b & 0x04 != 0;
    let hour24 = raw.status_b & 0x02 != 0;

    let mut second = raw.second;
    let mut minute = raw.minute;
    let mut hour = raw.hour;
    let mut day = raw.day;
    let mut month = raw.month;
    let mut year = raw.year;
    let mut century = raw.century;

    if !binary {
        // В 12-часовом режиме старший бит старшей полудесятки — признак PM,
        // его снимают до перевода BCD.
        let pm = hour & 0x80 != 0;
        hour &= 0x7F;
        second = bcd_to_bin(second);
        minute = bcd_to_bin(minute);
        hour = bcd_to_bin(hour);
        if pm && hour < 12 {
            hour += 12;
        }
        if !hour24 && !pm && hour == 12 {
            hour = 0;
        }
        day = bcd_to_bin(day);
        month = bcd_to_bin(month);
        year = bcd_to_bin(year);
        century = bcd_to_bin(century);
    } else if !hour24 && hour & 0x80 != 0 {
        hour = (hour & 0x7F) + 12;
    }

    // Век: если регистр не реализован, он читается как 0 и мы выводим
    // столетие из двухзначного года.
    let full_year = if century >= 19 && century <= 29 {
        century as u16 * 100 + year as u16
    } else {
        2000 + year as u16
    };

    WallClock {
        year: full_year,
        month: month.clamp(1, 12),
        day: day.clamp(1, 31),
        hour: hour.min(23),
        minute: minute.min(59),
        second: second.min(59),
    }
}

/// Текущее время по аппаратным часам.
///
/// Читает регистры дважды и сравнивает результаты: если во время чтения
/// начался внутренний перенос (секунды → минуты → часы), значения
/// расходятся и выполняется повторная попытка.
pub fn now() -> WallClock {
    for _ in 0..8 {
        while update_in_progress() {
            core::hint::spin_loop();
        }
        let first = read_raw();
        while update_in_progress() {
            core::hint::spin_loop();
        }
        let second = read_raw();

        if first.second == second.second
            && first.minute == second.minute
            && first.hour == second.hour
            && first.day == second.day
            && first.month == second.month
            && first.year == second.year
        {
            return normalize(first);
        }
    }
    // Восемь попыток подряд дали расхождение — часы неисправны или
    // эмулируются нестандартно. Возвращаем последний прочитанный снимок:
    // приблизительное время лучше полного отсутствия.
    normalize(read_raw())
}

/// Проверяет, что часы отдают правдоподобное время.
///
/// Используется системой диагностики, чтобы не вставлять в расширенный
/// идентификатор даты вроде 1970-01-01 при неисправном RTC.
pub fn is_plausible() -> bool {
    let now = now();
    now.year >= 2024 && now.month >= 1 && now.month <= 12
}
