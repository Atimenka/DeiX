//! Медиа-движок DeiX: музыка и видео в фоновой задаче планировщика.
//!
//! Форматы (честный список — что не понимает, то так и говорит):
//! * DXV1 — собственный контейнер DeiX: JPEG/PNG-кадры + PCM-звук
//!   (см. docs/MEDIA.md, конвертер tools/make_dxv.py);
//! * AVI (RIFF) — видео только Motion JPEG ('MJPG'), звук только PCM;
//! * WAV (RIFF) — PCM 8/16 бит, моно/стерео, до 96 кГц;
//! * DPS1 — родной формат UI-звуков DeiX (u8 моно, tools/wav2dps.py).
//!
//! H.264/MP4/MP3/GIF не поддерживаются: для них нужны кодек-библиотеки,
//! которых в ядре нет и которым не место в лимите kernel.bin (1 МиБ).
//!
//! Воспроизведение — отдельная задача ядра (sched::spawn): UI-поток не
//! блокируется ни на декодировании кадров, ни на DMA-звуке. Состояние
//! (play/pause/stop/громкость/прогресс) — атомики и спинлоки; текущий
//! видеокадр передаётся окну через общий буфер FRAME.

use crate::hda;
use crate::spinlock::SpinLock;
use alloc::format;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};

/// Состояния движка (STATE).
pub const ST_IDLE: u8 = 0;
pub const ST_LOADING: u8 = 1;
pub const ST_PLAYING: u8 = 2;
pub const ST_PAUSED: u8 = 3;
pub const ST_ERROR: u8 = 4;
pub const ST_DONE: u8 = 5;
pub const ST_STOPPED: u8 = 6;

/// Что сейчас играет (KIND).
pub const KIND_NONE: u8 = 0;
pub const KIND_AUDIO: u8 = 1;
pub const KIND_VIDEO: u8 = 2;

static STATE: AtomicU8 = AtomicU8::new(ST_IDLE);
static KIND: AtomicU8 = AtomicU8::new(KIND_NONE);
static VOLUME: AtomicU8 = AtomicU8::new(80);
static PAUSE_REQ: AtomicBool = AtomicBool::new(false);
static STOP_REQ: AtomicBool = AtomicBool::new(false);
static PROGRESS_MS: AtomicU32 = AtomicU32::new(0);
static DURATION_MS: AtomicU32 = AtomicU32::new(0);
static ERROR: SpinLock<String> = SpinLock::new(String::new());
static NOW_PLAYING: SpinLock<String> = SpinLock::new(String::new());
static CMD: SpinLock<Option<String>> = SpinLock::new(None);
static THREAD_STARTED: AtomicBool = AtomicBool::new(false);

/// Текущий видеокадр: UI рисует его в окно плеера. seq растёт с каждым
/// новым кадром — окно может не перерисовывать неизменившийся.
pub struct Frame {
    pub seq: u32,
    pub w: u32,
    pub h: u32,
    /// 0xRRGGBB, длина w*h (может быть пустым до первого кадра).
    pub pixels: Vec<u32>,
}

static FRAME: SpinLock<Frame> =
    SpinLock::new(Frame { seq: 0, w: 0, h: 0, pixels: Vec::new() });

// Локи движка (FRAME/CMD/ERROR/NOW_PLAYING) — обычные SpinLock: из
// обработчиков прерываний к ним никто не обращается, а маскировать
// прерывания вокруг них нельзя — владелец лока вытесняется таймером,
// и ожидание со снятым IF повесило бы систему (см. правило выбора в
// src/spinlock.rs).

pub fn with_frame<R>(f: impl FnOnce(&Frame) -> R) -> R {
    f(&FRAME.lock())
}

/// Копия текущего кадра (пиксели 0xRRGGBB, w, h); None — кадра ещё нет.
/// Копия позволяет рисовать кадр, не удерживая лок: публикация
/// следующего кадра медиазадачей не ждёт окончания отрисовки.
pub fn copy_frame() -> Option<(Vec<u32>, usize, usize)> {
    let f = FRAME.lock();
    if f.w == 0 || f.h == 0 || f.pixels.len() < (f.w * f.h) as usize {
        return None;
    }
    Some((f.pixels.clone(), f.w as usize, f.h as usize))
}

pub fn state() -> u8 {
    STATE.load(Ordering::Relaxed)
}

pub fn progress_ms() -> u32 {
    PROGRESS_MS.load(Ordering::Relaxed)
}

pub fn duration_ms() -> u32 {
    DURATION_MS.load(Ordering::Relaxed)
}

pub fn volume() -> u8 {
    VOLUME.load(Ordering::Relaxed)
}

pub fn error() -> String {
    (*ERROR.lock()).clone()
}

pub fn now_playing() -> String {
    (*NOW_PLAYING.lock()).clone()
}

/// Запуск воспроизведения файла (или замена текущего: старое честно
/// останавливается). Ошибки формата всплывают в STATE=ERROR и error().
pub fn play(path: &str) -> Result<(), String> {
    if path.trim().is_empty() {
        return Err(String::from("media: пустой путь"));
    }
    STOP_REQ.store(true, Ordering::Relaxed);
    PAUSE_REQ.store(false, Ordering::Relaxed);
    STATE.store(ST_LOADING, Ordering::Relaxed);
    *CMD.lock() = Some(String::from(path.trim()));
    ensure_thread();
    Ok(())
}

/// Пауза/продолжение (шаг ~100 мс — гранулярность звуковых кусков).
pub fn toggle_pause() {
    if !matches!(state(), ST_PLAYING | ST_PAUSED) {
        return;
    }
    let paused = !PAUSE_REQ.load(Ordering::Relaxed);
    PAUSE_REQ.store(paused, Ordering::Relaxed);
    STATE.store(if paused { ST_PAUSED } else { ST_PLAYING }, Ordering::Relaxed);
}

pub fn stop() {
    if state() == ST_IDLE {
        return;
    }
    STOP_REQ.store(true, Ordering::Relaxed);
    PAUSE_REQ.store(false, Ordering::Relaxed);
}

pub fn set_volume(v: u8) {
    VOLUME.store(v.clamp(0, 100), Ordering::Relaxed);
}

pub fn volume_up() {
    let v = volume().saturating_add(10).min(100);
    set_volume(v);
}

pub fn volume_down() {
    let v = volume().saturating_sub(10);
    set_volume(v);
}

/// Имя файла из полного пути (для заголовков).
pub fn file_name(path: &str) -> &str {
    match path.rsplit_once('/') {
        Some((_, name)) => name,
        None => path,
    }
}

pub fn is_image_file(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.ends_with(".png") || n.ends_with(".bmp") || n.ends_with(".jpg") || n.ends_with(".jpeg")
}

pub fn is_video_file(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.ends_with(".dxv") || n.ends_with(".avi")
}

pub fn is_audio_file(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.ends_with(".wav") || n.ends_with(".dps")
}

fn ensure_thread() {
    if THREAD_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    if crate::sched::spawn("media", media_task).is_none() {
        THREAD_STARTED.store(false, Ordering::SeqCst);
        crate::diag::notice(
            crate::diag::NONE,
            "media: планировщик не дал слот под задачу воспроизведения",
        );
    }
}

extern "C" fn media_task() {
    loop {
        let cmd = CMD.lock().take();
        let Some(path) = cmd else {
            crate::timer::sleep_ms(100);
            continue;
        };
        STOP_REQ.store(false, Ordering::Relaxed);
        PAUSE_REQ.store(false, Ordering::Relaxed);
        PROGRESS_MS.store(0, Ordering::Relaxed);
        DURATION_MS.store(0, Ordering::Relaxed);
        *ERROR.lock() = String::new();
        *NOW_PLAYING.lock() = path.clone();
        {
            let mut f = FRAME.lock();
            f.w = 0;
            f.h = 0;
            f.pixels.clear();
        }
        match load_and_play(&path) {
            Ok(()) => {
                let st = if STOP_REQ.load(Ordering::Relaxed) { ST_STOPPED } else { ST_DONE };
                STATE.store(st, Ordering::Relaxed);
            }
            Err(e) => {
                *ERROR.lock() = e;
                STATE.store(ST_ERROR, Ordering::Relaxed);
            }
        }
    }
}

// ---- Источники звука -----------------------------------------------------

/// Сырой PCM-фрагмент: срез файла + формат.
struct Pcm<'a> {
    data: &'a [u8],
    rate: u32,
    ch: u8,
    bits: u8,
}

impl<'a> Pcm<'a> {
    fn bytes_per_frame(&self) -> usize {
        self.ch as usize * (self.bits as usize / 8)
    }
    fn frame_count(&self) -> usize {
        if self.bytes_per_frame() == 0 {
            0
        } else {
            self.data.len() / self.bytes_per_frame()
        }
    }
    fn duration_ms(&self) -> u64 {
        if self.rate == 0 {
            0
        } else {
            self.frame_count() as u64 * 1000 / self.rate as u64
        }
    }
}

/// Линейный ресемплер источника в 48 кГц стерео i16 с громкостью.
struct Resampler<'a> {
    src: Pcm<'a>,
    /// Позиция в кадрах источника, fixed-point 16.16.
    pos_fp: u64,
    step_fp: u64,
}

const OUT_RATE: u32 = 48_000;

impl<'a> Resampler<'a> {
    fn new(src: Pcm<'a>) -> Self {
        let step_fp = ((src.rate as u64) << 16) / OUT_RATE as u64;
        Resampler { src, pos_fp: 0, step_fp }
    }

    fn sample(&self, frame: usize, chan: usize) -> i32 {
        let bpf = self.src.bytes_per_frame().max(1);
        let off = frame * bpf + chan * (self.src.bits as usize / 8);
        let d = self.src.data;
        if self.src.bits == 16 {
            if off + 2 > d.len() {
                return 0;
            }
            i16::from_le_bytes([d[off], d[off + 1]]) as i32
        } else {
            if off >= d.len() {
                return 0;
            }
            ((d[off] as i32) - 128) << 8
        }
    }

    /// Следующие `out_frames` кадров 48 кГц стерео (пусто — источник
    /// исчерпан). Громкость 0..100 применяется здесь же.
    fn next_chunk(&mut self, out_frames: usize, vol: u8) -> Vec<i16> {
        let total = self.src.frame_count();
        let mut out = Vec::with_capacity(out_frames * 2);
        let vol32 = vol as i32;
        for _ in 0..out_frames {
            let idx = (self.pos_fp >> 16) as usize;
            if idx >= total {
                break;
            }
            let frac = (self.pos_fp & 0xFFFF) as i32;
            let next = (idx + 1).min(total - 1);
            for chan in 0..2usize {
                let src_chan = chan.min(self.src.ch as usize - 1);
                let s0 = self.sample(idx, src_chan);
                let s1 = self.sample(next, src_chan);
                let interp = s0 + (((s1 - s0) * frac) >> 16);
                let shaped = (interp * vol32) / 100;
                out.push(shaped.clamp(-32768, 32767) as i16);
            }
            self.pos_fp += self.step_fp;
        }
        out
    }

    /// Сколько миллисекунд источника уже выдано.
    fn played_ms(&self) -> u64 {
        if self.src.rate == 0 {
            0
        } else {
            (self.pos_fp >> 16) * 1000 / self.src.rate as u64
        }
    }
}

fn stopped() -> bool {
    STOP_REQ.load(Ordering::Relaxed)
}

fn paused() -> bool {
    PAUSE_REQ.load(Ordering::Relaxed)
}

/// Ждёт снятия паузы; false — запрошена остановка.
fn wait_while_paused() -> bool {
    while paused() && !stopped() {
        crate::timer::sleep_ms(20);
    }
    !stopped()
}

/// Кусок 100 мс — компромисс: DMA-звук непрерывен, а реакция на
/// pause/stop не дольше десятой доли секунды.
const CHUNK_FRAMES: usize = OUT_RATE as usize / 10;

fn audio_ok_or_err(has_audio: bool) -> Result<(), String> {
    if has_audio && !hda::is_ready() {
        return Err(String::from(
            "Intel HDA не найден или не инициализирован: цифровое аудио недоступно \
             (PC speaker умеет только UI-эффекты, не PCM-поток)",
        ));
    }
    Ok(())
}

fn play_pcm(mut r: Resampler, dur_ms: u64) {
    DURATION_MS.store(dur_ms.min(u32::MAX as u64) as u32, Ordering::Relaxed);
    STATE.store(ST_PLAYING, Ordering::Relaxed);
    while !stopped() {
        if !wait_while_paused() {
            break;
        }
        let chunk = r.next_chunk(CHUNK_FRAMES, volume());
        if chunk.is_empty() {
            break;
        }
        hda::play_pcm_stereo_48k(&chunk);
        PROGRESS_MS.store(r.played_ms().min(u32::MAX as u64) as u32, Ordering::Relaxed);
    }
}

// ---- Видео ---------------------------------------------------------------

enum FrameFmt {
    Jpeg,
    Png,
}

fn decode_frame(fmt: &FrameFmt, data: &[u8]) -> Option<crate::png::Image> {
    let img = match fmt {
        FrameFmt::Jpeg => crate::jpeg::decode(data).ok()?,
        FrameFmt::Png => crate::png::decode(data).ok()?,
    };
    if img.w == 0 || img.h == 0 || img.pixels.len() < img.w * img.h {
        return None;
    }
    Some(img)
}

fn publish_frame(img: &crate::png::Image) {
    let mut f = FRAME.lock();
    f.seq = f.seq.wrapping_add(1);
    f.w = img.w as u32;
    f.h = img.h as u32;
    f.pixels.clear();
    f.pixels.extend_from_slice(&img.pixels[..img.w * img.h]);
}

/// Показывает один кадр без звука (фото-режим не нужен: видео всегда
/// играет здесь). Кадры идут по расписанию fps; опоздавшие не ждут.
fn play_video_frames(
    data: &[u8],
    frames: &[(u32, u32)],
    fmt: FrameFmt,
    fps: u32,
    audio: Option<Pcm>,
    dur_ms: u64,
) {
    DURATION_MS.store(dur_ms.min(u32::MAX as u64) as u32, Ordering::Relaxed);
    STATE.store(ST_PLAYING, Ordering::Relaxed);
    let fps = fps.clamp(1, 120);
    let frame_ms = 1000u64 / fps as u64;
    let start = crate::timer::uptime_ms();
    let mut paused_total = 0u64;
    let mut pause_start = 0u64;
    let mut audio_ms = 0u64;
    let mut resampler = audio.map(Resampler::new);
    let mut skipped = 0u32;

    for (i, &(off, len)) in frames.iter().enumerate() {
        if stopped() {
            break;
        }
        // Пауза: учитываем её длительность в расписании, иначе после
        // снятия все кадры окажутся «просроченными».
        if paused() {
            if pause_start == 0 {
                pause_start = crate::timer::uptime_ms();
            }
            if !wait_while_paused() {
                break;
            }
            paused_total += crate::timer::uptime_ms() - pause_start;
            pause_start = 0;
        }
        let target = start + paused_total + i as u64 * frame_ms;

        // Звук впереди видео не больше чем на 300 мс — так pause/stop
        // срабатывают быстро, а рассинхрон не слышен.
        if let Some(r) = resampler.as_mut() {
            while audio_ms < i as u64 * frame_ms + 300 && !stopped() && !paused() {
                let chunk = r.next_chunk(CHUNK_FRAMES, volume());
                if chunk.is_empty() {
                    break;
                }
                hda::play_pcm_stereo_48k(&chunk);
                audio_ms = r.played_ms();
            }
        }

        let end = (off as usize).saturating_add(len as usize).min(data.len());
        if (off as usize) < data.len() {
            match decode_frame(&fmt, &data[off as usize..end]) {
                Some(img) => publish_frame(&img),
                None => {
                    skipped += 1;
                    if skipped == 1 {
                        crate::diag::notice(
                            crate::diag::NONE,
                            "media: кадр не декодировался и пропущен (дальнейшие пропуски не дублируются в журнале)",
                        );
                    }
                }
            }
        }
        PROGRESS_MS.store((i as u64 * frame_ms).min(u32::MAX as u64) as u32, Ordering::Relaxed);
        let now = crate::timer::uptime_ms();
        if now < target {
            crate::timer::sleep_ms(target - now);
        }
    }
}

// ---- Разбор контейнеров ----------------------------------------------------

fn load_and_play(path: &str) -> Result<(), String> {
    let data = crate::vfs::read_file(path)
        .map_err(|e| format!("не прочитан {}: {}", path, e.message()))?;
    if data.len() < 12 {
        return Err(String::from("файл короче 12 байт: это не медиафайл"));
    }
    if &data[..4] == b"DXV1" {
        KIND.store(KIND_VIDEO, Ordering::Relaxed);
        return play_dxv(&data);
    }
    if &data[..4] == b"RIFF" {
        if &data[8..12] == b"AVI " {
            KIND.store(KIND_VIDEO, Ordering::Relaxed);
            return play_avi(&data);
        }
        if &data[8..12] == b"WAVE" {
            KIND.store(KIND_AUDIO, Ordering::Relaxed);
            return play_wav(&data);
        }
        return Err(String::from(
            "RIFF-контейнер не AVI и не WAVE — не поддерживается",
        ));
    }
    if &data[..4] == b"DPS1" {
        KIND.store(KIND_AUDIO, Ordering::Relaxed);
        return play_dps(&data);
    }
    let lower = path.to_ascii_lowercase();
    let hint = if lower.ends_with(".mp4") || lower.ends_with(".mkv") || lower.ends_with(".webm") {
        " H.264/VP9 не поддерживаются — конвертируйте в AVI (MJPEG) или DXV (tools/make_dxv.py)"
    } else if lower.ends_with(".mp3") || lower.ends_with(".ogg") || lower.ends_with(".flac") {
        " MP3/OGG/FLAC не поддерживаются — конвертируйте в WAV (PCM)"
    } else if lower.ends_with(".gif") {
        " GIF не поддерживается — сохраните кадр как PNG/JPEG"
    } else {
        ""
    };
    Err(format!(
        "формат не распознан: поддерживаются DXV1, AVI (MJPEG+PCM), WAV (PCM), DPS1.{}",
        hint
    ))
}

// ---- DXV1 -----------------------------------------------------------------
//
// Заголовок (20 байт, LE):
//   0..4   "DXV1"
//   4..6   u16 fps
//   6..8   u16 ширина (справочно)
//   8..10  u16 высота (справочно)
//   10     u8  формат кадра: 0 = JPEG, 1 = PNG
//   11     u8  звук: 0 = нет, 1 = PCM s16le моно, 2 = PCM s16le стерео
//   12..16 u32 частота звука, Гц
//   16..20 u32 число видеокадров
// Далее куски до конца файла: u8 tag ('V' видео, 'A' аудио), u32 len LE,
// данные. Куски идут во временном порядке.

fn play_dxv(data: &[u8]) -> Result<(), String> {
    if data.len() < 20 {
        return Err(String::from("DXV: файл короче заголовка"));
    }
    let fps = u16::from_le_bytes([data[4], data[5]]) as u32;
    let fmt_byte = data[10];
    let afmt = data[11];
    let arate = u32::from_le_bytes([data[12], data[13], data[14], data[15]]);
    let frames_declared = u32::from_le_bytes([data[16], data[17], data[18], data[19]]);
    if fps == 0 {
        return Err(String::from("DXV: fps = 0"));
    }
    let fmt = match fmt_byte {
        0 => FrameFmt::Jpeg,
        1 => FrameFmt::Png,
        n => return Err(format!("DXV: формат кадра {} неизвестен (0 = JPEG, 1 = PNG)", n)),
    };
    let mut frames: Vec<(u32, u32)> = Vec::new();
    let mut audio_parts: Vec<(u32, u32)> = Vec::new();
    let mut pos = 20usize;
    while pos + 5 <= data.len() {
        let tag = data[pos];
        let len = u32::from_le_bytes([data[pos + 1], data[pos + 2], data[pos + 3], data[pos + 4]]);
        pos += 5;
        let end = (pos as u64 + len as u64).min(data.len() as u64) as usize;
        match tag {
            b'V' => frames.push((pos as u32, (end - pos) as u32)),
            b'A' => audio_parts.push((pos as u32, (end - pos) as u32)),
            _ => return Err(format!("DXV: неизвестный кусок '{}' на {}", tag as char, pos - 5)),
        }
        pos = end;
    }
    if frames.is_empty() {
        return Err(String::from("DXV: в файле нет видеокадров"));
    }
    if frames_declared != frames.len() as u32 {
        crate::diag::notice(
            crate::diag::NONE,
            &format!(
                "DXV: заголовок обещает {} кадров, в файле {} — играем что есть",
                frames_declared,
                frames.len()
            ),
        );
    }

    // Звук: куски 'A' склеиваются в один поток (контейнер пишет их по
    // порядку). Для склейки нужен непрерывный срез — собираем во временный
    // буфер только если кусков больше одного.
    let mut audio_holder: Option<Vec<u8>> = None;
    let pcm: Option<Pcm> = match afmt {
        0 => None,
        1 | 2 => {
            let ch = if afmt == 1 { 1u8 } else { 2u8 };
            if arate < 4000 || arate > 96000 {
                return Err(format!("DXV: частота звука {} Гц вне 4000..96000", arate));
            }
            if audio_parts.is_empty() {
                None
            } else if audio_parts.len() == 1 {
                let (off, len) = audio_parts[0];
                Some(Pcm {
                    data: &data[off as usize..(off + len) as usize],
                    rate: arate,
                    ch,
                    bits: 16,
                })
            } else {
                let mut buf = Vec::new();
                for &(off, len) in audio_parts.iter() {
                    buf.extend_from_slice(&data[off as usize..(off + len) as usize]);
                }
                audio_holder = Some(buf);
                // Pcm одолжится из склеенного буфера в play_dxv_audio.
                None
            }
        }
        n => return Err(format!("DXV: звуковой формат {} неизвестен (0/1/2)", n)),
    };

    audio_ok_or_err(pcm.is_some() || (afmt != 0 && !audio_parts.is_empty()))?;

    let dur_ms = frames.len() as u64 * 1000 / fps as u64;
    if let Some(p) = pcm {
        play_video_frames(data, &frames, fmt, fps, Some(p), dur_ms);
        return Ok(());
    }
    if let Some(buf) = audio_holder {
        let ch = if afmt == 1 { 1u8 } else { 2u8 };
        // Отдельный вызов: Pcm должен жить столько же, сколько buf.
        play_dxv_audio(data, &frames, fmt, fps, dur_ms, &buf, arate, ch);
        return Ok(());
    }
    play_video_frames(data, &frames, fmt, fps, None, dur_ms);
    Ok(())
}

/// То же, что play_video_frames, но звук — из склеенного буфера (время
/// жизни привязано к этому кадру стека).
#[allow(clippy::too_many_arguments)]
fn play_dxv_audio(
    data: &[u8],
    frames: &[(u32, u32)],
    fmt: FrameFmt,
    fps: u32,
    dur_ms: u64,
    audio_buf: &[u8],
    arate: u32,
    ch: u8,
) {
    let pcm = Pcm { data: audio_buf, rate: arate, ch, bits: 16 };
    play_video_frames(data, frames, fmt, fps, Some(pcm), dur_ms);
}

// ---- AVI (RIFF) -----------------------------------------------------------

struct AviStream {
    /// 'vids' или 'auds'.
    kind: [u8; 4],
    /// FourCC обработчика видео (для vids).
    handler: [u8; 4],
    /// dwRate/dwScale (для vids — кадры в секунду; для auds не нужно).
    fps_num: u32,
    fps_den: u32,
    /// WAVEFORMATEX (для auds).
    audio: Option<(u16, u16, u32, u16)>, // (tag, ch, rate, bits)
}

fn play_avi(data: &[u8]) -> Result<(), String> {
    // Заполняются после обхода чанков (однократное присваивание).
    let streams: Vec<AviStream>;
    let video_frames: Vec<(u32, u32)>;
    let mut video_stream = usize::MAX;
    let mut audio_stream = usize::MAX;

    // Проход по чанкам: рекурсивно для LIST hdrl/strl/movi/rec.
    fn walk(
        data: &[u8],
        start: usize,
        end: usize,
        st: &mut AviWalk,
    ) -> Result<(), String> {
        let mut pos = start;
        while pos + 8 <= end {
            let fourcc = [data[pos], data[pos + 1], data[pos + 2], data[pos + 3]];
            let size = u32::from_le_bytes([
                data[pos + 4],
                data[pos + 5],
                data[pos + 6],
                data[pos + 7],
            ]) as usize;
            let body = pos + 8;
            let body_end = (body + size).min(end);
            match &fourcc {
                b"LIST" => {
                    if body_end >= body + 4 {
                        let lt = [data[body], data[body + 1], data[body + 2], data[body + 3]];
                        match &lt {
                            b"hdrl" | b"strl" => walk(data, body + 4, body_end, st)?,
                            b"movi" | b"rec " => walk(data, body + 4, body_end, st)?,
                            _ => {}
                        }
                    }
                }
                b"strh" => {
                    if body_end >= body + 24 {
                        st.streams.push(AviStream {
                            kind: [data[body], data[body + 1], data[body + 2], data[body + 3]],
                            handler: [
                                data[body + 4],
                                data[body + 5],
                                data[body + 6],
                                data[body + 7],
                            ],
                            fps_num: u32::from_le_bytes([
                                data[body + 12],
                                data[body + 13],
                                data[body + 14],
                                data[body + 15],
                            ]),
                            fps_den: u32::from_le_bytes([
                                data[body + 16],
                                data[body + 17],
                                data[body + 18],
                                data[body + 19],
                            ]),
                            audio: None,
                        });
                    }
                }
                b"strf" => {
                    let Some(s) = st.streams.last_mut() else {
                        return Err(String::from("AVI: strf без strh"));
                    };
                    if s.kind == *b"vids" {
                        // BITMAPINFOHEADER: biCompression на +16.
                        if body_end >= body + 20 {
                            s.handler = [
                                data[body + 16],
                                data[body + 17],
                                data[body + 18],
                                data[body + 19],
                            ];
                        }
                    } else if s.kind == *b"auds" && body_end >= body + 16 {
                        let tag = u16::from_le_bytes([data[body], data[body + 1]]);
                        let ch = u16::from_le_bytes([data[body + 2], data[body + 3]]);
                        let rate = u32::from_le_bytes([
                            data[body + 4],
                            data[body + 5],
                            data[body + 6],
                            data[body + 7],
                        ]);
                        let bits = u16::from_le_bytes([data[body + 14], data[body + 15]]);
                        s.audio = Some((tag, ch, rate, bits));
                    }
                }
                _ => {
                    // '00dc'/'00db' — кадр видео потока 0, '01wb' — блок
                    // звука потока 1. Прочие (JUNK, idx1) пропускаются.
                    let digits = [fourcc[0], fourcc[1]];
                    let tail = [fourcc[2], fourcc[3]];
                    if tail == *b"dc" || tail == *b"db" {
                        let idx = chunk_stream_index(digits);
                        if let Some(i) = idx {
                            st.video_by_stream.entry(i).or_insert_with(Vec::new).push((body as u32, (body_end - body) as u32));
                        }
                    } else if tail == *b"wb" {
                        let idx = chunk_stream_index(digits);
                        if let Some(i) = idx {
                            st.audio_by_stream.entry(i).or_insert_with(Vec::new).push((body as u32, (body_end - body) as u32));
                        }
                    }
                }
            }
            // Чанки выровнены на чётную границу.
            pos = body_end + (size & 1);
        }
        Ok(())
    }

    struct AviWalk {
        streams: Vec<AviStream>,
        video_by_stream: VecMap,
        audio_by_stream: VecMap,
    }
    // Мини-карта (до 8 потоков) без HashMap — в ядре её нет.
    struct VecMap {
        keys: Vec<usize>,
        vals: Vec<Vec<(u32, u32)>>,
    }
    impl VecMap {
        fn entry(&mut self, k: usize) -> Entry<'_> {
            Entry { map: self, k }
        }
        fn get(&self, k: usize) -> Option<&Vec<(u32, u32)>> {
            self.keys.iter().position(|x| *x == k).map(|i| &self.vals[i])
        }
    }
    struct Entry<'m> {
        map: &'m mut VecMap,
        k: usize,
    }
    impl<'m> Entry<'m> {
        fn or_insert_with(&mut self, v: impl FnOnce() -> Vec<(u32, u32)>) -> &mut Vec<(u32, u32)> {
            if let Some(p) = self.map.keys.iter().position(|x| *x == self.k) {
                return &mut self.map.vals[p];
            }
            self.map.keys.push(self.k);
            self.map.vals.push(v());
            self.map.vals.last_mut().unwrap()
        }
    }

    fn chunk_stream_index(digits: [u8; 2]) -> Option<usize> {
        let d0 = (digits[0] as char).to_digit(10)?;
        let d1 = (digits[1] as char).to_digit(10)?;
        Some((d0 * 10 + d1) as usize)
    }

    let mut walk_state = AviWalk {
        streams: Vec::new(),
        video_by_stream: VecMap { keys: Vec::new(), vals: Vec::new() },
        audio_by_stream: VecMap { keys: Vec::new(), vals: Vec::new() },
    };
    walk(data, 12, data.len(), &mut walk_state)?;
    streams = walk_state.streams;

    for (i, s) in streams.iter().enumerate() {
        if s.kind == *b"vids" && video_stream == usize::MAX {
            video_stream = i;
        }
        if s.kind == *b"auds" && audio_stream == usize::MAX {
            audio_stream = i;
        }
    }
    if video_stream == usize::MAX {
        return Err(String::from("AVI: в файле нет видеопотока"));
    }
    let vs = &streams[video_stream];
    if !(vs.handler == *b"MJPG" || vs.handler == *b"JPEG" || vs.handler == *b"jpg ") {
        return Err(format!(
            "AVI: видео-кодек '{}' не поддерживается — нужен Motion JPEG (MJPG); \
             H.264/Xvid/DivX ядро декодировать не умеет",
            String::from_utf8_lossy(&vs.handler).trim().to_string()
        ));
    }
    video_frames = walk_state.video_by_stream.get(video_stream).cloned().unwrap_or_default();
    if video_frames.is_empty() {
        return Err(String::from("AVI: видеопоток пуст (нет чанков 'dc'/'db')"));
    }
    let fps = if vs.fps_den == 0 { 25 } else { (vs.fps_num / vs.fps_den).clamp(1, 120) };

    // Звук.
    let mut pcm: Option<Pcm> = None;
    let mut audio_buf: Vec<u8> = Vec::new();
    if audio_stream != usize::MAX {
        let asx = &streams[audio_stream];
        let Some((tag, ch, rate, bits)) = asx.audio else {
            return Err(String::from("AVI: у аудиопотока нет WAVEFORMATEX"));
        };
        if tag != 1 {
            return Err(format!(
                "AVI: аудио-формат tag {} не поддерживается — нужен PCM (tag 1); MP3 внутри AVI не декодируется",
                tag
            ));
        }
        if ch < 1 || ch > 2 || (bits != 8 && bits != 16) || rate < 4000 || rate > 96000 {
            return Err(format!(
                "AVI: PCM {} Гц, {} кан., {} бит вне поддержки (8/16 бит, 1-2 канала, 4-96 кГц)",
                rate, ch, bits
            ));
        }
        let parts = walk_state.audio_by_stream.get(audio_stream).cloned().unwrap_or_default();
        if parts.len() == 1 {
            let (off, len) = parts[0];
            pcm = Some(Pcm {
                data: &data[off as usize..(off + len) as usize],
                rate,
                ch: ch as u8,
                bits: bits as u8,
            });
        } else if !parts.is_empty() {
            for &(off, len) in parts.iter() {
                audio_buf.extend_from_slice(&data[off as usize..(off + len) as usize]);
            }
        }
    }
    audio_ok_or_err(pcm.is_some() || !audio_buf.is_empty())?;

    let dur_ms = video_frames.len() as u64 * 1000 / fps as u64;
    if let Some(p) = pcm {
        play_video_frames(data, &video_frames, FrameFmt::Jpeg, fps, Some(p), dur_ms);
    } else if !audio_buf.is_empty() {
        let (tag, ch, rate, bits) = streams[audio_stream].audio.unwrap();
        let _ = tag;
        play_video_audio_buf(
            data,
            &video_frames,
            fps,
            dur_ms,
            &audio_buf,
            rate,
            ch as u8,
            bits as u8,
        );
    } else {
        play_video_frames(data, &video_frames, FrameFmt::Jpeg, fps, None, dur_ms);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn play_video_audio_buf(
    data: &[u8],
    frames: &[(u32, u32)],
    fps: u32,
    dur_ms: u64,
    audio_buf: &[u8],
    rate: u32,
    ch: u8,
    bits: u8,
) {
    let pcm = Pcm { data: audio_buf, rate, ch, bits };
    play_video_frames(data, frames, FrameFmt::Jpeg, fps, Some(pcm), dur_ms);
}

// ---- WAV -------------------------------------------------------------------

/// Начало тела чанка с данным fourcc (первое вхождение).
fn find_chunk(data: &[u8], fourcc: &[u8; 4]) -> Option<usize> {
    let mut pos = 12usize;
    while pos + 8 <= data.len() {
        if &data[pos..pos + 4] == fourcc {
            return Some(pos + 8);
        }
        let size = u32::from_le_bytes([
            data[pos + 4],
            data[pos + 5],
            data[pos + 6],
            data[pos + 7],
        ]) as usize;
        pos += 8 + size + (size & 1);
    }
    None
}

fn play_wav(data: &[u8]) -> Result<(), String> {
    // fmt-чанк: tag, каналы, частота, битность; data-чанк: PCM.
    let mut pos = 12usize;
    let mut fmt: Option<(u16, u16, u32, u16)> = None;
    let mut pcm_range: Option<(usize, usize)> = None;
    while pos + 8 <= data.len() {
        let fourcc = &data[pos..pos + 4];
        let size = u32::from_le_bytes([
            data[pos + 4],
            data[pos + 5],
            data[pos + 6],
            data[pos + 7],
        ]) as usize;
        let body = pos + 8;
        let body_end = (body + size).min(data.len());
        if fourcc == b"fmt " {
            if body_end < body + 16 {
                return Err(String::from("WAV: fmt-чанк короче 16 байт"));
            }
            let tag = u16::from_le_bytes([data[body], data[body + 1]]);
            let ch = u16::from_le_bytes([data[body + 2], data[body + 3]]);
            let rate = u32::from_le_bytes([
                data[body + 4],
                data[body + 5],
                data[body + 6],
                data[body + 7],
            ]);
            let bits = u16::from_le_bytes([data[body + 14], data[body + 15]]);
            fmt = Some((tag, ch, rate, bits));
        } else if fourcc == b"data" {
            pcm_range = Some((body, body_end));
        }
        pos = body_end + (size & 1);
    }
    let Some((tag, ch, rate, bits)) = fmt else {
        return Err(String::from("WAV: нет fmt-чанка"));
    };
    if tag != 1 && tag != 0xFFFE {
        return Err(format!(
            "WAV: формат tag {} не поддерживается — нужен PCM (1); MP3/ADPCM-дорожки не декодируются",
            tag
        ));
    }
    if tag == 0xFFFE {
        // WAVEFORMATEXTENSIBLE: SubFormat на +24 должен начинаться с
        // 0x0001 (KSDATAFORMAT_SUBTYPE_PCM) — иначе это не PCM.
        let fmt_pos = find_chunk(data, b"fmt ");
        let ok = match fmt_pos {
            Some(b) if b + 26 <= data.len() => data[b + 24] == 0x01 && data[b + 25] == 0x00,
            _ => false,
        };
        if !ok {
            return Err(String::from(
                "WAV: WAVEFORMATEXTENSIBLE без PCM-подформата не поддерживается",
            ));
        }
    }
    let (body, body_end) = pcm_range.ok_or_else(|| String::from("WAV: нет data-чанка"))?;
    if ch < 1 || ch > 2 || (bits != 8 && bits != 16) || rate < 4000 || rate > 96000 {
        return Err(format!(
            "WAV: {} Гц, {} кан., {} бит вне поддержки (PCM 8/16 бит, 1-2 канала, 4-96 кГц)",
            rate, ch, bits
        ));
    }
    audio_ok_or_err(true)?;
    let pcm = Pcm {
        data: &data[body..body_end],
        rate,
        ch: ch as u8,
        bits: bits as u8,
    };
    let dur = pcm.duration_ms();
    play_pcm(Resampler::new(pcm), dur);
    Ok(())
}

// ---- DPS1 -------------------------------------------------------------------

fn play_dps(data: &[u8]) -> Result<(), String> {
    if data.len() < 12 {
        return Err(String::from("DPS: файл короче заголовка"));
    }
    let rate = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    let n = u32::from_le_bytes([data[8], data[9], data[10], data[11]]) as usize;
    if !(2000..=48000).contains(&rate) || n == 0 {
        return Err(format!("DPS: rate {} / len {} вне допустимого (2000..48000 Гц)", rate, n));
    }
    if data.len() < 12 + n {
        return Err(String::from("DPS: файл обрезан"));
    }
    audio_ok_or_err(true)?;
    KIND.store(KIND_AUDIO, Ordering::Relaxed);
    let pcm = Pcm { data: &data[12..12 + n], rate, ch: 1, bits: 8 };
    let dur = pcm.duration_ms();
    play_pcm(Resampler::new(pcm), dur);
    Ok(())
}
