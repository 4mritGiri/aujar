//! Input methods (IME) and composed text in the launcher's search field.
//!
//! GPUI talks to the platform's input method (Wayland `text-input-v3`, X11 XIM, ...) through
//! [`EntityInputHandler`], in UTF-16 offsets. The field itself keeps byte offsets into `query`,
//! so this module converts between the two. While an input method composes (pinyin, Hangul,
//! dead keys, compose sequences) the not-yet-committed text is "marked": it is shown underlined
//! and is not searched until it is committed.

use crate::LauncherView;
use gpui::{Bounds, Context, EntityInputHandler, Pixels, Point, UTF16Selection, Window};
use std::ops::Range;

/// The byte offset of UTF-16 offset `utf16` in `text` (clamped to the end, and to a character
/// boundary when it points inside a surrogate pair).
pub(crate) fn utf16_to_byte(text: &str, utf16: usize) -> usize {
    let mut units = 0;

    for (byte, c) in text.char_indices() {
        if units >= utf16 {
            return byte;
        }

        units += c.len_utf16();
    }

    text.len()
}

/// The UTF-16 offset of byte offset `byte` in `text` (clamped to the end).
pub(crate) fn byte_to_utf16(text: &str, byte: usize) -> usize {
    text.char_indices()
        .take_while(|(index, _)| *index < byte)
        .map(|(_, c)| c.len_utf16())
        .sum()
}

pub(crate) fn range_to_byte(text: &str, range: &Range<usize>) -> Range<usize> {
    let start = utf16_to_byte(text, range.start);

    start..utf16_to_byte(text, range.end).max(start)
}

pub(crate) fn range_to_utf16(text: &str, range: &Range<usize>) -> Range<usize> {
    byte_to_utf16(text, range.start)..byte_to_utf16(text, range.end)
}

/// Committed text the field accepts: no line breaks, tabs or other control characters (the
/// platform also routes Enter and Tab through the input handler).
pub(crate) fn printable(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).collect()
}

impl LauncherView {
    /// The range an input-method edit applies to: the one it names, else the text being
    /// composed, else the selection (or the caret).
    fn ime_target(&self, range_utf16: Option<Range<usize>>) -> Range<usize> {
        if let Some(range) = range_utf16 {
            return range_to_byte(&self.query, &range);
        }

        if let Some(marked) = self.marked.clone() {
            return marked;
        }

        let (start, end) = self.selection();

        start..end
    }

    /// Replace `range` with `text` (cut to the query length limit) and return the byte range
    /// the text now occupies. Snapshots for undo only when asked: a composition is one step.
    fn replace_bytes(&mut self, range: Range<usize>, text: &str, snapshot: bool) -> Range<usize> {
        let end = range.end.min(self.query.len());
        let start = range.start.min(end);
        let room = crate::MAX_QUERY_CHARS
            .saturating_sub(self.query.chars().count())
            .saturating_add(self.query[start..end].chars().count());
        let text: String = text.chars().take(room).collect();

        if snapshot {
            self.snapshot();
        }

        self.query.replace_range(start..end, &text);

        start..start + text.len()
    }
}

impl EntityInputHandler for LauncherView {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = range_to_byte(&self.query, &range_utf16);

        adjusted_range.replace(range_to_utf16(&self.query, &range));

        Some(self.query[range].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let (start, end) = self.selection();

        Some(UTF16Selection {
            range: range_to_utf16(&self.query, &(start..end)),
            reversed: self.cursor < self.anchor,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked
            .as_ref()
            .map(|range| range_to_utf16(&self.query, range))
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        // The composed text stays as ordinary text; now it is worth searching for.
        if self.marked.take().is_some() {
            self.edited(cx);
        }
    }

    /// Committed text: from an input method, or a key the field did not handle itself.
    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.closing {
            return;
        }

        let target = self.ime_target(range_utf16);
        let text = printable(text);
        let was_composing = self.marked.take().is_some();

        if text.is_empty() && target.is_empty() && !was_composing {
            return;
        }

        // A composition already took its undo snapshot when it started.
        let placed = self.replace_bytes(target, &text, !was_composing);

        self.cursor = placed.end;
        self.anchor = placed.end;
        self.edited(cx);
    }

    /// Text still being composed: shown underlined, not searched yet.
    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.closing {
            return;
        }

        let target = self.ime_target(range_utf16);
        let text = printable(new_text);

        // One undo step for the whole composition: only its first change is snapshotted.
        let placed = self.replace_bytes(target, &text, self.marked.is_none());
        let placed_text = self.query[placed.clone()].to_owned();

        self.marked = (!placed.is_empty()).then(|| placed.clone());

        // The input method may put a caret (or a selection) inside the composed text.
        let inside = new_selected_range
            .map(|range| range_to_byte(&placed_text, &range))
            .unwrap_or(placed_text.len()..placed_text.len());

        self.anchor = placed.start + inside.start;
        self.cursor = placed.start + inside.end;
        self.more = false;
        self.tips = false;
        cx.notify();
    }

    /// Where the input method's candidate window goes: under the caret.
    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(self.caret_bounds.get().unwrap_or(element_bounds))
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        // The field places the caret with its own per-character hit areas.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_offsets_are_the_same() {
        assert_eq!(utf16_to_byte("abc", 2), 2);
        assert_eq!(byte_to_utf16("abc", 2), 2);
        assert_eq!(utf16_to_byte("abc", 10), 3);
    }

    #[test]
    fn multibyte_and_astral_characters() {
        // "न" is 3 bytes / 1 unit, "😀" is 4 bytes / 2 units, "é" is 2 bytes / 1 unit.
        let text = "न😀é";

        assert_eq!(utf16_to_byte(text, 1), 3);
        assert_eq!(utf16_to_byte(text, 3), 7);
        assert_eq!(utf16_to_byte(text, 4), 9);
        assert_eq!(byte_to_utf16(text, 3), 1);
        assert_eq!(byte_to_utf16(text, 7), 3);
        assert_eq!(byte_to_utf16(text, text.len()), 4);

        // Inside the surrogate pair: stays on a character boundary.
        assert!(text.is_char_boundary(utf16_to_byte(text, 2)));
    }

    #[test]
    fn ranges_round_trip_and_never_invert() {
        let text = "नमस्ते world";
        let bytes = range_to_byte(text, &(2..6));

        assert_eq!(range_to_utf16(text, &bytes), 2..6);
        // An input method may send an end before the start; it must not panic or invert.
        let reversed = Range { start: 5, end: 2 };
        assert!(range_to_byte(text, &reversed).is_empty());
    }

    #[test]
    fn committed_text_drops_control_characters() {
        assert_eq!(printable("a\tb\nc"), "abc");
        assert_eq!(printable("\n"), "");
        assert_eq!(printable("नमस्ते"), "नमस्ते");
    }
}
