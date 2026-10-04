//! The board's one spacing system: every gap it draws between two elements is one of these,
//! so the header, the sections, the cards, the panel and the keys all line up alike.

/// Columns left bare at each side of the board's content.
pub(super) const MARGIN: u16 = 1;

/// Space inside every border and at each end of a button, chip or key block.
pub(super) const PAD: u16 = 1;

/// Columns between the buttons of one group: the Work panel's close button, its stage
/// picker's label.
pub(super) const BUTTON_GAP: u16 = 2;

/// Columns between the header's groups that draw no rule: the categories (or the tabs, with
/// none) and the filter box or time.
pub(super) const GROUP_GAP: u16 = 3;

/// Columns between the header's own buttons of one group: the tabs, the categories, the
/// back-to-all button, the close button after the filter box.
pub(super) const HEADER_GAP: u16 = 1;

/// Columns of blank each side of a `│` rule between two of the header's groups: one gap, as
/// between two buttons of a group.
pub(super) const SEP_PAD: u16 = HEADER_GAP;

/// The rule's own zone: `SEP_PAD` blank columns, the `│`, then `SEP_PAD` more.
pub(super) const SEP_WIDTH: u16 = 2 * SEP_PAD + 1;

/// Columns between two stage columns.
pub(super) const COLUMN_GAP: u16 = 2;

/// Columns between one key hint and the next; a key's block and its words part by one space.
pub(super) const HINT_GAP: u16 = 4;

/// `columns` spaces, for a gap drawn as text.
pub(super) fn gap(columns: u16) -> String {
    " ".repeat(usize::from(columns))
}
