//! Текстовые утилиты дизайн-системы: единственное место, где решается,
//! как символ Unicode превращается в глифы растрового шрифта 8x16.
//!
//! И отрисовка (`Renderer::draw_text`), и измерение ширины, и усечение
//! строк пользуются одним разложением `glyph_cells` — ширина текста на
//! экране по построению совпадает с расчётной. Раньше `draw_text`
//! шёл по БАЙТАМ строки: каждый байт UTF-8 рисовался отдельным глифом
//! CP866, и любая кириллица превращалась в мусор двойной ширины.

use crate::cp866;

/// Раскладывает символ Unicode в 1..=3 глифа шрифта (коды CP866/ASCII).
///
/// ASCII рисуется как есть, кириллица — по таблице CP866 (глифы
/// загружены, см. font_full.rs), частая типографика приводится к
/// ASCII-эквивалентам. Всё остальное — честный '?', а не случайный
/// байт из таблицы псевдографики.
pub fn glyph_cells(ch: char, out: &mut dyn FnMut(u8)) {
    if (' '..='~').contains(&ch) {
        out(ch as u8);
        return;
    }
    if let Some(b) = cp866::unicode_to_cp866(ch) {
        out(b);
        return;
    }
    match ch {
        '…' => {
            out(b'.');
            out(b'.');
            out(b'.');
        }
        '—' | '–' => out(b'-'),
        '«' | '»' | '“' | '”' | '„' => out(b'"'),
        '‘' | '’' => out(b'\''),
        '•' | '·' => out(b'.'),
        '\u{a0}' | '\t' => out(b' '),
        _ => out(b'?'),
    }
}

/// Ширина строки в ячейках шрифта (1 ячейка = 8 px).
pub fn text_cells(s: &str) -> usize {
    let mut cells = 0usize;
    for ch in s.chars() {
        glyph_cells(ch, &mut |_| cells += 1);
    }
    cells
}

/// Префикс строки, занимающий не более `max_cells` ячеек, и признак
/// того, что строка была усечена. Разрез — только по границе символа.
pub fn truncate_to_cells(s: &str, max_cells: usize) -> (&str, bool) {
    let mut cells = 0usize;
    for (i, ch) in s.char_indices() {
        let mut w = 0usize;
        glyph_cells(ch, &mut |_| w += 1);
        if cells + w > max_cells {
            return (&s[..i], true);
        }
        cells += w;
    }
    (s, false)
}

/// Байтовый индекс символа с порядковым номером `char_idx` (для
/// `String::insert`/`remove`/`split_off`, которым нужны границы
/// символов: байтовый индекс внутри UTF-8-последовательности — паника).
/// За концом строки возвращает `s.len()`.
pub fn char_byte_index(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}
