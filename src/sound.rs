// ❗ЗАВИСИМОТИ: инит скрипт pid 1 который будет ограничивать пользовательский
// ЯДЕРНЫЙ МОДУЛЬ DeiX OS (src/lib.rs, Ring 0). Интеграция в существующий код
// sound — АУДИО-ДРАЙВЕР (PC speaker через PIT-канал 2, порт 0x61).
// Реализует beep() (звуковой сигнал заданной частоты/длительности),
// предупреждающие сигналы: warn_triple(), ota_alert(),
// и ПРОИГРЫВАТЕЛЬ ЦИФРОВЫХ UI-ЗВУКОВ (DPS1 через ШИМ на PC speaker):
// файлы *.dps лежат в EROFS-разделе /super образа (как /system/media/audio/ui
// в Android — неизменяемые системные ресурсы вне kernel.bin).
// no_std-совместимо: порты ввода-вывода + alloc (чтение раздела с диска).


use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU8, Ordering};

use crate::port::{inb, outb};

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum SoundMode {
    Auto = 0,
    Hda = 1,
    Speaker = 2,
}

static SOUND_MODE: AtomicU8 = AtomicU8::new(0);

pub fn set_sound_mode(mode: SoundMode) {
    SOUND_MODE.store(mode as u8, Ordering::Relaxed);
}

pub fn get_sound_mode() -> SoundMode {
    match SOUND_MODE.load(Ordering::Relaxed) {
        1 => SoundMode::Hda,
        2 => SoundMode::Speaker,
        _ => SoundMode::Auto,
    }
}

const PIT_CMD: u16 = 0x43;
const PIT_CH2: u16 = 0x42;
const SPEAKER: u16 = 0x61;
/// Порт 0x80 — запись в него исторически используется как короткая
/// (~0.5–1 мкс) задержка шины ISA: нужна для ШИМ-воспроизведения, где
/// микросекундный таймер ядра (timer::uptime_ms) слишком грубый.
const SCRATCH: u16 = 0x80;

/// Включает PC speaker (бит 1 порта 0x61).
fn speaker_on() {
    unsafe {
        let v = inb(SPEAKER);
        outb(SPEAKER, (v & !0x02) | 0x02); // keep gate, set speaker bit
    }
}

/// Выключает PC speaker.
fn speaker_off() {
    unsafe {
        let v = inb(SPEAKER);
        outb(SPEAKER, v & !0x02);
    }
}

/// Программирует PIT-канал 2 на частоту `hz`.
fn pit_freq(hz: u32) {
    let divisor: u32 = (1193182u32 / hz.max(20)).clamp(1, 65535);
    unsafe {
        outb(PIT_CMD, 0xB6); // channel 2, lobyte/hibyte, square wave
        outb(PIT_CH2, (divisor & 0xFF) as u8);
        outb(PIT_CH2, ((divisor >> 8) & 0xFF) as u8);
    }
}

/// Блокирующая пауза (busy-wait по PIT-тикам таймера ядра).
fn delay_ms(ms: u64) {
    let t0 = crate::timer::uptime_ms();
    while crate::timer::uptime_ms().saturating_sub(t0) < ms {
        unsafe { core::arch::asm!("nop"); }
    }
}

/// Звуковой сигнал: частота `hz`, длительность `ms`.
/// При наличии контроллера Intel HDA выводит чистый звук через DMA (48 кГц стерео);
/// при отсутствии или в режиме Speaker — использует встроенный PC speaker (порт 0x61).
pub fn beep(hz: u32, ms: u64) {
    let mode = get_sound_mode();
    if mode != SoundMode::Speaker && crate::hda::is_ready() {
        crate::hda::play_tone(hz, ms);
        return;
    }
    beep_speaker(hz, ms);
}

/// Принудительный звуковой сигнал через встроенный PC speaker (порт 0x61, ШИМ/PIT).
pub fn beep_speaker(hz: u32, ms: u64) {
    pit_freq(hz);
    speaker_on();
    delay_ms(ms);
    speaker_off();
}

/// Предупреждение «внимание, действие» — три коротких сигнала.
pub fn warn_triple() {
    for _ in 0..3 {
        beep(880, 120);
        delay_ms(80);
    }
}

/// Сигнал OTA-обновления: длинный высокий + короткий.
pub fn ota_alert() {
    beep(1320, 300);
    delay_ms(120);
    beep(1760, 200);
    delay_ms(120);
    beep(1320, 300);
}

// ==================== UI-звуки (DPS1 через ШИМ) ====================
//
// Зачем: assets/*.wav (44.1 кГц/16-бит/стерео) — это десятки КБ чистого
// сигнала, который в kernel.bin не влезает (лимит размера ядра), а PC
// speaker не умеет DMA/PCM. Выход:
//   1) на этапе сборки tools/wav2dps.py гоняет WAV -> DPS1 (8 кГц, u8,
//      моно) — компактно и уже готово к проигрыванию;
//   2) build.sh кладёт *.dps в EROFS-раздел /super (make_deix_fs.py);
//   3) здесь ядро читает нужный файл с диска и отыгрывает ШИМ-ом:
//      каждый сэмпл — одно «вкл/выкл» динамика, скважность ~ амплитуде.
//
// Формат DPS1 (12-байтовый заголовок):
//   0..4  магия "DPS1"; 4..8 u32 частота Гц; 8..12 u32 число сэмплов;
//   далее — байты unsigned PCM (128 = тишина).

/// Реестр системных звуковых эффектов. Файлы — в /super (EROFS).
#[derive(Copy, Clone)]
pub enum UiSound {
    /// Вход в систему завершён, система готова (start.dps).
    Startup,
    /// Ошибка действия: неверный пароль и т.п. (error.dps).
    Error,
    /// Батарея разряжена (lowbat.dps). Источника события (ACPI) пока нет —
    /// API готово, подключится вместе с мониторингом батареи.
    BatteryLow,
    /// Батарея заряжена (fullbat.dps). Тоже ждёт ACPI.
    BatteryFull,
    /// Подключён/обнаружен USB-носитель (usbcon.dps). Играется при загрузке
    /// с флешки (активен RAM-диск — см. ramdisk::is_active).
    UsbConnect,
    /// USB-носитель отключён (usbdisc.dps). USB-стека нет — событие появится
    /// вместе с ним; API готово (доступно вручную: `sound usbdisc`).
    UsbDisconnect,
}

impl UiSound {
    /// Имя DPS-файла в EROFS-разделе /super.
    pub fn dps_name(self) -> &'static str {
        match self {
            UiSound::Startup => "start.dps",
            UiSound::Error => "error.dps",
            UiSound::BatteryLow => "lowbat.dps",
            UiSound::BatteryFull => "fullbat.dps",
            UiSound::UsbConnect => "usbcon.dps",
            UiSound::UsbDisconnect => "usbdisc.dps",
        }
    }

    /// Человекочитаемое описание (для `sound list` в CLI), русское.
    pub fn title(self) -> &'static str {
        match self {
            UiSound::Startup => "старт системы (вход выполнен)",
            UiSound::Error => "ошибка (напр., неверный пароль)",
            UiSound::BatteryLow => "батарея разряжена (API, события пока нет)",
            UiSound::BatteryFull => "батарея заряжена (API, события пока нет)",
            UiSound::UsbConnect => "USB-носитель обнаружен (загрузка с флешки)",
            UiSound::UsbDisconnect => "USB-носитель отключён (API вручную)",
        }
    }

    /// То же, что title(), но по-английски (язык CLI переключается `lang`).
    pub fn title_en(self) -> &'static str {
        match self {
            UiSound::Startup => "system startup (login complete)",
            UiSound::Error => "error (e.g. wrong password)",
            UiSound::BatteryLow => "battery low (API ready, no event source yet)",
            UiSound::BatteryFull => "battery full (API ready, no event source yet)",
            UiSound::UsbConnect => "USB drive detected (USB flash boot)",
            UiSound::UsbDisconnect => "USB drive removed (manual API for now)",
        }
    }
}

/// Все эффекты реестра (для перечисления в CLI).
pub const UI_SOUNDS: [UiSound; 6] = [
    UiSound::Startup,
    UiSound::Error,
    UiSound::BatteryLow,
    UiSound::BatteryFull,
    UiSound::UsbConnect,
    UiSound::UsbDisconnect,
];

/// Псевдонимы для CLI: "start" -> "start.dps" и т.д.; неизвестное имя
/// возвращается как есть (вдруг там прямое "file.dps").
fn resolve_alias<'a>(name: &'a str) -> &'a str {
    match name {
        "start" => UiSound::Startup.dps_name(),
        "error" => UiSound::Error.dps_name(),
        "lowbat" => UiSound::BatteryLow.dps_name(),
        "fullbat" => UiSound::BatteryFull.dps_name(),
        "usbcon" => UiSound::UsbConnect.dps_name(),
        "usbdisc" => UiSound::UsbDisconnect.dps_name(),
        other => other,
    }
}

/// Читает весь /super с диска и достаёт из его EROFS файл `name`.
fn load_dps(name: &str) -> Result<Vec<u8>, &'static str> {
    let layout = crate::partition_map::lookup_layout("/super")
        .ok_or("sound: в карте разделов нет /super")?;
    let image = crate::bootchain::read_partition_image(layout)
        .map_err(|_| "sound: не удалось прочитать /super с диска")?;
    crate::erofs::read_file(&image, name)
        .map_err(|_| "sound: звука нет в образе (пересоберите: build.sh [2e/8])")
}

/// Разбирает заголовок DPS1; возвращает (частота, сэмплы).
fn parse_dps(dps: &[u8]) -> Result<(u32, &[u8]), &'static str> {
    if dps.len() < 12 || &dps[..4] != b"DPS1" {
        return Err("sound: плохой файл — это не DPS1");
    }
    let rate = u32::from_le_bytes([dps[4], dps[5], dps[6], dps[7]]);
    let len = u32::from_le_bytes([dps[8], dps[9], dps[10], dps[11]]) as usize;
    if !(2000..=48000).contains(&rate) || len == 0 || len > (rate as usize) * 30 {
        // до 48 кГц и до 30 секунд на эффект; защита от повреждённых данных
        return Err("sound: повреждённый DPS1 (rate/len за границами)");
    }
    if dps.len() < 12 + len {
        return Err("sound: DPS1 обрезан");
    }
    Ok((rate, &dps[12..12 + len]))
}

/// Считывает текущее значение счётчика PIT Канал 0 (без нарушения его счёта).
fn pit_read_channel0() -> u16 {
    unsafe {
        outb(0x43, 0x00); // latch counter 0
        let lo = inb(0x40);
        let hi = inb(0x40);
        u16::from_le_bytes([lo, hi])
    }
}

/// Калиброванная пауза в тиках таймера PIT (~0.838 мкс на тик при 1.193 МГц).
fn pit_wait_ticks(ticks_to_wait: u32) {
    if ticks_to_wait == 0 {
        return;
    }
    const DIVISOR: u32 = 1193182 / 100; // 11931 тиков на квант 10 мс
    let mut elapsed: u32 = 0;
    let mut prev = pit_read_channel0() as u32;

    while elapsed < ticks_to_wait {
        let curr = pit_read_channel0() as u32;
        let delta = if prev >= curr {
            prev - curr
        } else {
            prev + DIVISOR - curr
        };
        elapsed += delta;
        prev = curr;
        unsafe { core::arch::asm!("pause"); }
    }
}

/// ШИМ-проигрывание unsigned-u8 PCM на PC speaker с аппаратной калибровкой скорости по PIT.
///
/// Устраняет эффект «быстрого воспроизведения» (чипманк/ускоренный звук) в QEMU/KVM:
/// длительность каждого сэмпла строго отсчитывается по генератору PIT (1_193_182 Гц),
/// благодаря чему темп и тональность звука 100% совпадают с оригинальной записью.
fn play_pcm8_max(samples: &[u8], rate: u32, max_samples: Option<usize>) {
    let effective_rate = rate.clamp(2000, 48000);
    let sample_ticks: u32 = (1_193_182u32 / effective_rate).max(4);

    let orig = unsafe { inb(SPEAKER) };
    let slice = match max_samples {
        Some(max_s) => &samples[..samples.len().min(max_s)],
        None => samples,
    };

    for &s in slice {
        let high_ticks = (sample_ticks * (s as u32)) / 255;
        let low_ticks = sample_ticks.saturating_sub(high_ticks);

        if high_ticks > 0 {
            unsafe {
                let cur = inb(SPEAKER);
                outb(SPEAKER, (cur & !0x03) | 0x02);
            }
            pit_wait_ticks(high_ticks);
        }
        if low_ticks > 0 {
            unsafe {
                let cur = inb(SPEAKER);
                outb(SPEAKER, cur & !0x03);
            }
            pit_wait_ticks(low_ticks);
        }
    }

    // Возвращаем порт в исходное состояние (бит 1 выключен)
    unsafe {
        outb(SPEAKER, orig & !0x02);
    }
}

fn play_pcm8(samples: &[u8], rate: u32) {
    play_pcm8_max(samples, rate, None);
}

/// Играет системный эффект (читает DPS из /super при каждом вызове).
/// Ошибки не фатальны: если звука нет в старом образе — тихо возвращаем Err,
/// система продолжает работать беззвучно, как раньше.
/// Приоритетно использует Intel HDA (если режим Auto/HDA); при отсутствии или режиме Speaker — ШИМ на PC speaker.
pub fn play_ui(sound: UiSound) -> Result<(), &'static str> {
    let raw = load_dps(sound.dps_name())?;
    let mode = get_sound_mode();
    if mode != SoundMode::Speaker && crate::hda::is_ready() {
        if crate::hda::play_dps(&raw).is_ok() {
            return Ok(());
        }
    }
    let (rate, samples) = parse_dps(&raw)?;
    // Для UI-звуков на PC speaker ограничиваем время звука 250 мс,
    // чтобы не блокировать загрузку ОС на 3.7–5.5 секунд.
    let max_boot_samples = (rate as usize) / 4;
    play_pcm8_max(samples, rate, Some(max_boot_samples));
    Ok(())
}

/// Играет эффект по имени/псевдониму (для CLI `sound play <имя>`).
pub fn play_named(name: &str) -> Result<(), &'static str> {
    let raw = load_dps(resolve_alias(name))?;
    let mode = get_sound_mode();
    if mode != SoundMode::Speaker && crate::hda::is_ready() {
        if crate::hda::play_dps(&raw).is_ok() {
            return Ok(());
        }
    }
    let (rate, samples) = parse_dps(&raw)?;
    play_pcm8(samples, rate);
    Ok(())
}

/// Принудительно играет эффект через PC speaker (ШИМ порт 0x61).
pub fn play_speaker_named(name: &str) -> Result<(), &'static str> {
    let raw = load_dps(resolve_alias(name))?;
    let (rate, samples) = parse_dps(&raw)?;
    play_pcm8(samples, rate);
    Ok(())
}

/// Список файлов, реально присутствующих в EROFS /super (для `sound list`,
/// чтобы CLI мог отметить недоступные эффекты в старых образах).
pub fn media_files() -> Result<Vec<String>, &'static str> {
    let layout = crate::partition_map::lookup_layout("/super")
        .ok_or("sound: в карте разделов нет /super")?;
    let image = crate::bootchain::read_partition_image(layout)
        .map_err(|_| "sound: не удалось прочитать /super с диска")?;
    crate::erofs::list_files(&image)
        .map(|v| v.into_iter().map(|(n, _)| n).collect())
        .map_err(|_| "sound: /super не EROFS")
}
