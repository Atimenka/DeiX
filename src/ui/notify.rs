//! Всплывающие уведомления рабочего стола в правом нижнем углу.
//!
//! Любой модуль ядра кладёт сообщение в очередь через `push`; рабочий стол
//! раз в кадр анимации вызывает `frame`, рисует карточки через `draw` и
//! добавляет `region` в damage до восстановления обоев (см.
//! `Desktop::diff_exposed_regions`).
//!
//! Карточка выезжает справа, висит заданное время и уезжает с затуханием.
//! Пока курсор над карточкой, таймер стоит; клик по ней закрывает её.
//! Одновременно видны до `MAX_VISIBLE` карточек, остальные ждут в очереди.
//! Время жизни отсчитывается от первого показа, поэтому сообщение,
//! пришедшее в текстовом режиме, не пропадёт до запуска рабочего стола.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::renderer::{Color, IconType, Rect, Renderer};
use crate::spinlock::SpinLock;
use crate::ui::metrics::UiMetrics;
use crate::ui::text::{text_cells, truncate_to_cells};
use crate::ui::theme::UiTheme;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Info,
    Success,
    Warning,
    Error,
}

impl Kind {
    fn hold_ms(self) -> u64 {
        match self {
            Kind::Info | Kind::Success => 6_000,
            Kind::Warning => 8_000,
            Kind::Error => 10_000,
        }
    }

    fn color(self, theme: &UiTheme) -> Color {
        match self {
            Kind::Info => theme.accent,
            Kind::Success => Color::GREEN,
            Kind::Warning => Color::YELLOW,
            Kind::Error => Color::RED,
        }
    }
}

const MAX_QUEUE: usize = 16;
const MAX_VISIBLE: usize = 3;
const CARD_W: i32 = 360;
const MIN_CARD_W: i32 = 220;
const MARGIN_R: i32 = 16;
const GAP: i32 = 10;
const PAD: i32 = 14;
const BADGE: i32 = 36;
const RADIUS: i32 = 14;
const SHADOW: i32 = 8;
const BODY_LINES: usize = 3;
const ANIM_IN_MS: u64 = 340;
const ANIM_OUT_MS: u64 = 300;
const FRAME_MS: u64 = 33;

struct Toast {
    kind: Kind,
    title: String,
    body: String,
    lines: Vec<String>,
    w: i32,
    h: i32,
    shown_ms: Option<u64>,
    dismissed_ms: Option<u64>,
    hold_ms: u64,
    /// Расстояние от нижнего якоря до нижней кромки карточки; плавно
    /// стремится к `target`, когда соседи появляются или исчезают.
    slot: i32,
    slot_fresh: bool,
    rect: Rect,
    vis: u8,
    /// Доля оставшегося времени показа, 0..=255.
    remaining: u8,
    hovered: bool,
}

struct State {
    toasts: Vec<Toast>,
    last_ms: u64,
    next_frame_ms: u64,
}

static STATE: SpinLock<State> = SpinLock::new(State {
    toasts: Vec::new(),
    last_ms: 0,
    next_frame_ms: 0,
});

/// Ставит уведомление в очередь. Вызывается из обычного контекста (не из
/// обработчика прерывания): строки аллоцируются в куче.
pub fn push(kind: Kind, title: &str, body: &str) {
    let mut st = STATE.lock();
    if st.toasts.len() >= MAX_QUEUE {
        if let Some(i) = st.toasts.iter().position(|t| t.shown_ms.is_none()) {
            st.toasts.remove(i);
        }
    }
    st.toasts.push(Toast {
        kind,
        title: title.to_string(),
        body: body.to_string(),
        lines: Vec::new(),
        w: 0,
        h: 0,
        shown_ms: None,
        dismissed_ms: None,
        hold_ms: kind.hold_ms(),
        slot: 0,
        slot_fresh: true,
        rect: Rect::new(0, 0, 0, 0),
        vis: 0,
        remaining: 255,
        hovered: false,
    });
}

enum Phase {
    In(u32),
    Hold,
    Out(u32),
    Done,
}

fn timeline(t: &Toast, now: u64) -> (u64, u64, u64) {
    let shown = t.shown_ms.unwrap_or(now);
    let elapsed = now.saturating_sub(shown);
    let natural_out = ANIM_IN_MS + t.hold_ms;
    let out_start = match t.dismissed_ms {
        Some(d) => d.saturating_sub(shown).min(natural_out).max(ANIM_IN_MS),
        None => natural_out,
    };
    (elapsed, out_start, natural_out)
}

fn phase(t: &Toast, now: u64) -> Phase {
    let (elapsed, out_start, _) = timeline(t, now);
    if elapsed < ANIM_IN_MS {
        Phase::In((elapsed * 1000 / ANIM_IN_MS) as u32)
    } else if elapsed < out_start {
        Phase::Hold
    } else {
        let o = elapsed - out_start;
        if o >= ANIM_OUT_MS {
            Phase::Done
        } else {
            Phase::Out((o * 1000 / ANIM_OUT_MS) as u32)
        }
    }
}

/// Кубическое замедление к концу: 0..=1000 -> 0..=1000.
fn ease_out(p: u32) -> u32 {
    let q = 1000 - p.min(1000);
    1000 - q * q * q / 1_000_000
}

/// Квадратичное ускорение: 0..=1000 -> 0..=1000.
fn ease_in(p: u32) -> u32 {
    let p = p.min(1000);
    p * p / 1000
}

/// Разбивает текст на строки не шире `max_cells` ячеек (по словам; слишком
/// длинное слово режется). Если строк больше `max_lines`, последняя
/// заканчивается многоточием.
fn wrap(text: &str, max_cells: usize, max_lines: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut overflow = false;

    'words: for word in text.split_whitespace() {
        let mut word = word;
        loop {
            let need = text_cells(word) + if cur.is_empty() { 0 } else { 1 };
            if text_cells(&cur) + need <= max_cells {
                if !cur.is_empty() {
                    cur.push(' ');
                }
                cur.push_str(word);
                continue 'words;
            }
            if !cur.is_empty() {
                lines.push(core::mem::take(&mut cur));
                if lines.len() >= max_lines {
                    overflow = true;
                    break 'words;
                }
                continue;
            }
            let (head, _) = truncate_to_cells(word, max_cells);
            if head.is_empty() {
                break 'words;
            }
            lines.push(head.to_string());
            if lines.len() >= max_lines {
                overflow = true;
                break 'words;
            }
            word = &word[head.len()..];
            if word.is_empty() {
                continue 'words;
            }
        }
    }
    if !overflow && !cur.is_empty() {
        lines.push(cur);
    }
    if overflow {
        if let Some(last) = lines.last_mut() {
            let (head, _) = truncate_to_cells(last, max_cells.saturating_sub(3));
            let mut s = head.to_string();
            s.push_str("...");
            *last = s;
        }
    }
    lines
}

fn prepare(t: &mut Toast, screen_w: i32) {
    t.w = CARD_W.min(screen_w - 2 * MARGIN_R).max(MIN_CARD_W);
    let text_cells_w = ((t.w - PAD * 2 - BADGE - 12) / 8).max(8) as usize;
    t.lines = wrap(&t.body, text_cells_w, BODY_LINES);
    let body_h = t.lines.len() as i32 * 16;
    let text_h = 16 + if body_h > 0 { 4 + body_h } else { 0 };
    t.h = PAD + text_h.max(BADGE) + 23;
}

fn anchor_y(screen_h: i32) -> i32 {
    let m = UiMetrics::fluent();
    screen_h - (m.dock_height + m.dock_margin) - 14
}

/// Шаг анимации: не чаще одного раза в `FRAME_MS`. Возвращает `true`, если
/// сцену нужно перерисовать.
pub fn frame(now: u64, screen_w: i32, screen_h: i32, mouse_x: i32, mouse_y: i32) -> bool {
    let mut guard = STATE.lock();
    let st = &mut *guard;
    if st.toasts.is_empty() || now < st.next_frame_ms {
        return false;
    }
    st.next_frame_ms = now + FRAME_MS;
    let dt = if st.last_ms == 0 {
        0
    } else {
        now.saturating_sub(st.last_ms).min(200)
    };
    st.last_ms = now;

    let mut shown = st.toasts.iter().filter(|t| t.shown_ms.is_some()).count();
    for t in st.toasts.iter_mut() {
        if shown >= MAX_VISIBLE {
            break;
        }
        if t.shown_ms.is_none() {
            prepare(t, screen_w);
            t.shown_ms = Some(now);
            shown += 1;
        }
    }

    for t in st.toasts.iter_mut() {
        if let Some(s) = t.shown_ms {
            t.hovered = t.vis > 0 && t.rect.contains(mouse_x, mouse_y);
            if t.hovered && t.dismissed_ms.is_none() && dt > 0 {
                if let Phase::Hold = phase(t, now) {
                    t.shown_ms = Some(s + dt);
                }
            }
        }
    }
    st.toasts
        .retain(|t| t.shown_ms.is_none() || !matches!(phase(t, now), Phase::Done));

    let anchor = anchor_y(screen_h);
    let mut acc = 0i32;
    for t in st.toasts.iter_mut().rev() {
        if t.shown_ms.is_none() {
            continue;
        }
        if t.slot_fresh {
            t.slot = acc;
            t.slot_fresh = false;
        } else {
            let d = acc - t.slot;
            t.slot += if d.abs() <= 1 { d } else { d / 3 };
        }
        acc += t.h + GAP;

        let (vis, slide) = match phase(t, now) {
            Phase::In(p) => {
                let e = ease_out(p);
                (
                    (e * 255 / 1000) as u8,
                    (t.w + MARGIN_R) * (1000 - e as i32) / 1000,
                )
            }
            Phase::Hold => (255, 0),
            Phase::Out(p) => {
                let e = ease_in(p);
                (((1000 - e) * 255 / 1000) as u8, t.w / 2 * e as i32 / 1000)
            }
            Phase::Done => (0, 0),
        };
        t.vis = vis;
        let (elapsed, out_start, natural_out) = timeline(t, now);
        let left = natural_out.saturating_sub(elapsed.min(out_start));
        t.remaining = (left * 255 / t.hold_ms.max(1)).min(255) as u8;
        t.rect = Rect::new(
            screen_w - MARGIN_R - t.w + slide,
            anchor - t.slot - t.h,
            t.w as u32,
            t.h as u32,
        );
    }
    true
}

/// Область экрана, которую занимают карточки (вместе с тенью). Рабочий
/// стол добавляет её в damage до восстановления обоев; `None`, если
/// сейчас ни одна карточка не показана.
pub fn region(screen_w: i32, screen_h: i32) -> Option<Rect> {
    let st = STATE.lock();
    let margin = SHADOW + 4;
    let mut x0 = i32::MAX;
    let mut y0 = i32::MAX;
    let mut x1 = i32::MIN;
    let mut y1 = i32::MIN;
    for t in st.toasts.iter().filter(|t| t.shown_ms.is_some() && t.w > 0) {
        x0 = x0.min(t.rect.x - margin);
        y0 = y0.min(t.rect.y - margin);
        x1 = x1.max(t.rect.x + t.rect.w as i32 + margin);
        y1 = y1.max(t.rect.y + t.rect.h as i32 + margin);
    }
    let (x0, y0, x1, y1) = (x0.max(0), y0.max(0), x1.min(screen_w), y1.min(screen_h));
    if x0 >= x1 || y0 >= y1 {
        return None;
    }
    Some(Rect::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32))
}

/// Клик мышью: если попал в карточку, закрывает её и возвращает `true`
/// (клик не должен проваливаться в окна под ней).
pub fn click(x: i32, y: i32, now: u64) -> bool {
    let mut st = STATE.lock();
    for t in st.toasts.iter_mut().rev() {
        if t.shown_ms.is_some() && t.vis > 0 && t.rect.contains(x, y) {
            if t.dismissed_ms.is_none() {
                t.dismissed_ms = Some(now);
            }
            return true;
        }
    }
    false
}

fn isqrt(v: i32) -> i32 {
    let mut x = 0;
    while (x + 1) * (x + 1) <= v {
        x += 1;
    }
    x
}

/// Закрашенный круг с альфой (края без сглаживания).
fn fill_disc_alpha(r: &mut Renderer, cx: i32, cy: i32, radius: i32, color: Color, alpha: u8) {
    for dy in -radius..=radius {
        let dx = isqrt(radius * radius - dy * dy);
        for px in (cx - dx)..=(cx + dx) {
            r.put_pixel_alpha(px, cy + dy, color, alpha);
        }
    }
}

fn draw_badge(r: &mut Renderer, kind: Kind, cx: i32, cy: i32, glyph: Color) {
    match kind {
        Kind::Success => {
            r.draw_icon(cx - 8, cy - 8, IconType::Check, glyph);
            r.draw_icon(cx - 7, cy - 8, IconType::Check, glyph);
        }
        Kind::Info => {
            r.draw_char(cx - 4, cy - 8, b'i', glyph, None);
            r.draw_char(cx - 3, cy - 8, b'i', glyph, None);
        }
        Kind::Warning => {
            r.draw_char(cx - 4, cy - 8, b'!', glyph, None);
            r.draw_char(cx - 3, cy - 8, b'!', glyph, None);
        }
        Kind::Error => {
            for dx in 0..2 {
                r.draw_line(cx - 5 + dx, cy - 5, cx + 5 + dx, cy + 5, glyph);
                r.draw_line(cx - 5 + dx, cy + 5, cx + 5 + dx, cy - 5, glyph);
            }
        }
    }
}

fn draw_card(r: &mut Renderer, theme: &UiTheme, t: &Toast) {
    let Rect { x, y, .. } = t.rect;
    let (w, h) = (t.w, t.h);
    let vis = t.vis as u32;
    let a = |base: u32| -> u8 { (base * vis / 255) as u8 };
    let bg = theme.window_bg;
    let mix = |c: Color| -> Color { bg.lerp(c, t.vis) };
    let accent = t.kind.color(theme);
    let fx = crate::ui::effects::current();

    if fx.shadows_enabled() {
        for i in 1..=SHADOW {
            let alpha = a(46 * (SHADOW - i + 1) as u32 / SHADOW as u32);
            r.draw_rounded_rect_outline(
                x - i,
                y - i + 3,
                (w + 2 * i) as u32,
                (h + 2 * i) as u32,
                RADIUS + i,
                Color::BLACK,
                alpha,
            );
        }
    }

    if theme.enable_blur && fx.blur_enabled() {
        r.apply_blur_rect(x, y, w as u32, h as u32, 2);
    }
    r.fill_rounded_rect_alpha(x, y, w as u32, h as u32, RADIUS, bg, a(theme.opacity.max(235) as u32));
    r.draw_rounded_rect_outline(x, y, w as u32, h as u32, RADIUS, Color::BLACK, a(120));
    r.draw_rounded_rect_outline(x + 1, y + 1, (w - 2) as u32, (h - 2) as u32, RADIUS - 1, Color::WHITE, a(26));
    r.fill_rect_alpha(x + RADIUS, y + 1, (w - 2 * RADIUS) as u32, 2, accent, a(170));

    let (bx, by) = (x + PAD, y + PAD);
    let (cx, cy) = (bx + BADGE / 2, by + BADGE / 2);
    fill_disc_alpha(r, cx, cy, BADGE / 2, accent, a(58));
    r.draw_rounded_rect_outline(bx, by, BADGE as u32, BADGE as u32, BADGE / 2, accent, a(210));
    draw_badge(r, t.kind, cx, cy, mix(accent));

    let tx = bx + BADGE + 12;
    let title_cells = (((w - PAD * 2 - BADGE - 12) / 8) as usize).saturating_sub(2);
    let (title, _) = truncate_to_cells(&t.title, title_cells);
    r.draw_text(tx, by, title, mix(theme.text_primary), None);
    r.draw_text(tx + 1, by, title, mix(theme.text_primary), None);
    for (i, line) in t.lines.iter().enumerate() {
        r.draw_text(tx, by + 20 + i as i32 * 16, line, mix(theme.text_secondary), None);
    }

    let close = if t.hovered { theme.text_primary } else { theme.text_secondary };
    let (qx, qy) = (x + w - PAD - 4, by + 6);
    r.draw_line(qx - 3, qy - 3, qx + 3, qy + 3, mix(close));
    r.draw_line(qx - 3, qy + 3, qx + 3, qy - 3, mix(close));

    let bar_w = w - 2 * PAD;
    let bar_y = y + h - 13;
    r.fill_rect_alpha(x + PAD, bar_y, bar_w as u32, 3, accent, a(38));
    let fill_w = bar_w * t.remaining as i32 / 255;
    if fill_w > 0 {
        r.fill_rect_alpha(x + PAD, bar_y, fill_w as u32, 3, accent, a(220));
    }
}

/// Рисует все показанные карточки поверх сцены (старые снизу вверх).
pub fn draw(r: &mut Renderer, theme: &UiTheme) {
    let st = STATE.lock();
    for t in st.toasts.iter() {
        if t.shown_ms.is_some() && t.vis > 0 && t.w > 0 {
            draw_card(r, theme, t);
        }
    }
}
