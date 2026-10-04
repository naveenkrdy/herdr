//! Fitting the board's text into its cells: control characters dropped, text wrapped
//! between words (never cut), or cut with `…` where one line is all there is.

use ratatui::buffer::Buffer;
use ratatui::style::Style;
use unicode_width::UnicodeWidthChar;

use crate::client::shell::render::{display_width, put_segment};

/// `text` without control characters: the terminal never draws them.
pub(super) fn clean(text: &str) -> String {
    text.chars().filter(|ch| !ch.is_control()).collect()
}

/// `file` with the home folder shown as `~`.
pub(super) fn home_path(file: &str, home_dir: Option<&str>) -> String {
    match home_dir.and_then(|home| file.strip_prefix(home)) {
        Some(rest) if rest.starts_with('/') => format!("~{rest}"),
        _ => file.to_owned(),
    }
}

/// `text` on one line, ending in `…` when it does not fit.
pub(super) fn put_fitted(buffer: &mut Buffer, x: u16, y: u16, end: u16, text: &str, style: Style) {
    let width = usize::from(end.saturating_sub(x));
    let text = crate::ui::truncate_end(&clean(text), width);
    put_segment(buffer, x, y, end, &text, style);
}

/// Where the filter's words show in the board's text, and the style that marks them.
#[derive(Clone, Copy)]
pub(super) struct Marks<'a> {
    pub(super) words: &'a [String],
    pub(super) style: Style,
}

impl Marks<'_> {
    /// `text` from `x` as `put_segment` draws it, each matched word patched with the mark;
    /// returns the column after it.
    pub(super) fn put(
        &self,
        buffer: &mut Buffer,
        x: u16,
        y: u16,
        end: u16,
        text: &str,
        style: Style,
    ) -> u16 {
        if self.words.is_empty() {
            return put_segment(buffer, x, y, end, text, style);
        }
        let mut x = x;
        for (run, marked) in marked_runs(text, self.words) {
            let style = if marked {
                style.patch(self.style)
            } else {
                style
            };
            x = put_segment(buffer, x, y, end, &run, style);
        }
        x
    }
}

/// `text` in runs, each inside a match of a word or not; `words` are lower case and a match
/// is by lower case, so `DECIDE` marks `decide`.
fn marked_runs(text: &str, words: &[String]) -> Vec<(String, bool)> {
    let chars: Vec<char> = text.chars().collect();
    let mut marked = vec![false; chars.len()];
    for word in words {
        let want: Vec<char> = word.chars().collect();
        if want.is_empty() {
            continue;
        }
        for start in 0..chars.len() {
            let mut lower = Vec::with_capacity(want.len());
            let mut end = start;
            while lower.len() < want.len() && end < chars.len() {
                lower.extend(chars[end].to_lowercase());
                end += 1;
            }
            if lower == want {
                marked[start..end].iter_mut().for_each(|mark| *mark = true);
            }
        }
    }
    let mut runs: Vec<(String, bool)> = Vec::new();
    for (ch, mark) in chars.into_iter().zip(marked) {
        match runs.last_mut() {
            Some((run, last)) if *last == mark => run.push(ch),
            _ => runs.push((ch.to_string(), mark)),
        }
    }
    runs
}

/// `text` in lines no wider than `width`, broken between words, and inside a word only when
/// the word alone is wider. Never cut, never empty.
pub(super) fn wrap(text: &str, width: u16) -> Vec<String> {
    let width = usize::from(width.max(1));
    let text = clean(text);
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut used = 0;
    for word in text.split_whitespace() {
        let word_width = display_width(word) as usize;
        if used > 0 && used + 1 + word_width <= width {
            line.push(' ');
            line.push_str(word);
            used += 1 + word_width;
            continue;
        }
        let mut word = word;
        if used > 0 {
            // A word wider than any line starts beside the word before it, broken after its
            // last `-`, `/` or `_` that fits there; with none, it starts the next line.
            let room = width.saturating_sub(used + 1);
            if let Some(head) = (word_width > width)
                .then(|| break_prefix(word, room))
                .flatten()
            {
                line.push(' ');
                line.push_str(head);
                word = &word[head.len()..];
            }
            lines.push(std::mem::take(&mut line));
            used = 0;
        }
        // The line holds only this word now. One wider than the line breaks after its last
        // `-`, `/` or `_` that fits, else anywhere.
        for ch in word.chars() {
            let ch_width = ch.width().unwrap_or(0);
            if used > 0 && used + ch_width > width {
                let mark = line
                    .rfind(['-', '/', '_'])
                    .map(|at| at + 1)
                    .filter(|at| *at < line.len());
                let rest = mark.map(|at| line.split_off(at)).unwrap_or_default();
                lines.push(std::mem::take(&mut line));
                used = display_width(&rest) as usize;
                line = rest;
            }
            line.push(ch);
            used += ch_width;
        }
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

/// The longest start of `word` that ends after a `-`, `/` or `_` and is at most `room` wide;
/// none when no such start fits, or it would be the whole word.
fn break_prefix(word: &str, room: usize) -> Option<&str> {
    let mut used = 0;
    let mut end = None;
    for (at, ch) in word.char_indices() {
        used += ch.width().unwrap_or(0);
        if used > room {
            break;
        }
        if matches!(ch, '-' | '/' | '_') {
            end = Some(at + ch.len_utf8());
        }
    }
    end.filter(|end| *end < word.len()).map(|end| &word[..end])
}

/// `text` on one line of `width` columns: whole when it fits, else cut at the last word
/// boundary that leaves room for `…` (before a space, or after a `/`, `-` or `_`), any
/// separator left at the end dropped. A first word wider than the line is the only thing cut
/// inside a word. Whitespace between words reads as one space.
pub(super) fn cut(text: &str, width: u16) -> String {
    let text = clean(text).split_whitespace().collect::<Vec<_>>().join(" ");
    let width = usize::from(width);
    if display_width(&text) as usize <= width {
        return text;
    }
    let Some(room) = width.checked_sub(1) else {
        return String::new();
    };
    let (mut used, mut end, mut boundary) = (0, 0, None);
    for (at, ch) in text.char_indices() {
        let ch_width = ch.width().unwrap_or(0);
        if used + ch_width > room {
            break;
        }
        if ch == ' ' {
            boundary = Some(at);
        }
        used += ch_width;
        end = at + ch.len_utf8();
        if matches!(ch, '/' | '-' | '_') {
            boundary = Some(end);
        }
    }
    let head = match boundary {
        Some(at) if at > 0 => &text[..at],
        _ => &text[..end],
    };
    format!("{}…", head.trim_end_matches([' ', '·', ',', ';', ':', '(']))
}

/// Wrapped `lines` cut to `most`, the last kept one ending in `…` when any were cut: the
/// lines after it join it, which is always wider than `width`.
pub(super) fn at_most(mut lines: Vec<String>, most: usize, width: u16) -> Vec<String> {
    if lines.len() > most && most > 0 {
        let rest = lines.split_off(most - 1).join(" ");
        lines.push(crate::ui::truncate_end(&rest, usize::from(width)));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_wider_than_the_line_breaks_after_a_dash_slash_or_underscore_first() {
        assert_eq!(
            wrap("feat/task-board_design-pass", 12),
            ["feat/task-", "board_", "design-pass"]
        );
        assert_eq!(wrap("dotworks/task", 10), ["dotworks/", "task"]);
        assert_eq!(
            wrap("abcdefghij", 4),
            ["abcd", "efgh", "ij"],
            "none: mid-word"
        );
        assert_eq!(
            wrap("a-b fits", 20),
            ["a-b fits"],
            "a word that fits stays whole"
        );
    }

    #[test]
    fn a_cut_keeps_whole_words_and_ends_in_one_ellipsis() {
        assert_eq!(cut("fits on the line", 16), "fits on the line");
        assert_eq!(
            cut("pick a time for W6; say if repo", 28),
            "pick a time for W6; say if…"
        );
        // After a `/`, `-` or `_` is a word boundary too.
        assert_eq!(
            cut("board cells over 70/60 wide", 22),
            "board cells over 70/…"
        );
        // A separator left at the end goes with the cut.
        assert_eq!(cut("Try-list · after today", 12), "Try-list…");
        // A first word wider than the line is the only thing cut inside a word.
        assert_eq!(cut("abcdefghij klm", 5), "abcd…");
        // Spaces, tabs and newlines between words read as one space.
        assert_eq!(cut("two\n  lines", 20), "two lines");
        assert_eq!(cut("anything", 0), "");
        assert_eq!(cut("wide", 1), "…");
    }

    #[test]
    fn a_word_wider_than_any_line_starts_after_the_word_before_it_at_a_break_mark() {
        // `file` keeps its path beside it instead of standing alone on a line.
        assert_eq!(
            wrap(
                "file ~/Workdir/oakwood-workspace/WORKSPACE-STATUS.md:20",
                40
            ),
            [
                "file ~/Workdir/oakwood-workspace/",
                "WORKSPACE-STATUS.md:20"
            ]
        );
        // With no break mark in the room left, the word starts the next line.
        assert_eq!(wrap("ab cdefghijklmn", 8), ["ab", "cdefghij", "klmn"]);
    }

    #[test]
    fn wrapped_text_drops_control_characters() {
        // A card's title comes from its notes file and reaches the card and panel through
        // wrap alone, so wrap itself keeps an escape or a bell off the screen.
        assert_eq!(
            wrap("Fix \u{1b}[31mred\u{1b}[0m title\u{7}", 20),
            ["Fix [31mred[0m title"]
        );
    }
}
