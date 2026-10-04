//! The tree sidebar's bottom box, about every space rather than the focused row: what
//! needs you (a blocked agent, an idle or done agent that asks) under ACTION REQUIRED,
//! oldest first, each with how long it has waited; then the agents finished and not seen
//! yet under COMPLETED. Each entry is one row with the tree's mark, under its part's heading and
//! count in the mark column; a blank row parts the two, the first row a short box gives up.
//! Agents at work are left to the tree. It sits above the
//! footer under one faint rule, with a blank row before the footer, at most a third of the
//! sidebar. A part with nothing in it hides, and with both empty there is no box and no
//! rule: the tree runs down to the footer. Built from the snapshot once per compose.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use super::render::{put_segment, ShellRenderState};
use super::tree_sidebar::{minutes_text, pane_label, put_label, token, TreeTarget};
use super::*;
use crate::api::schema::AgentStatus;
use crate::protocol::ClientShellAgent;
use unicode_width::UnicodeWidthChar;

/// Why an agent needs you.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Reason {
    /// It asked something (an `ask` token).
    Answer,
    /// Herdr's blocked state: a permission prompt.
    Approval,
}

/// Why `agent` needs you, if it does. A working agent is busy, so an ask it left behind is
/// stale; an ask says more than a block, but an ask of Markdown marks alone says nothing.
pub(super) fn needs_you(agent: &ClientShellAgent) -> Option<Reason> {
    let asks = || token(&agent.tokens, "ask").is_some_and(has_words);
    match agent.agent_status {
        AgentStatus::Working => None,
        _ if asks() => Some(Reason::Answer),
        AgentStatus::Blocked => Some(Reason::Approval),
        _ => None,
    }
}

/// `text` has a character besides whitespace, control characters and Markdown marks.
fn has_words(text: &str) -> bool {
    text.chars().any(|ch| {
        !(ch.is_whitespace()
            || ch.is_control()
            || matches!(ch, '`' | '*' | '_' | '#' | '>' | '[' | ']'))
    })
}

/// One agent to look at: its name as the box shows it (its space's, then `/` and the
/// agent's as the tree shows it for anyone but the space's main lead), and how long ago
/// this client first saw it wait or finish.
struct Entry<'a> {
    space: &'a str,
    agent: Option<&'a str>,
    /// The agent names no lead: its name stays whole while the space's part and the time
    /// can give way.
    lead: bool,
    ago: Option<Duration>,
    target: TreeTarget,
}

impl Entry<'_> {
    /// The name without control characters: the terminal never draws them, so they would
    /// only throw the line off.
    fn name_chars(&self) -> impl Iterator<Item = char> + '_ {
        let agent = self
            .agent
            .into_iter()
            .flat_map(|agent| std::iter::once('/').chain(agent.chars()));
        self.space
            .chars()
            .chain(agent)
            .filter(|ch| !ch.is_control())
    }
}

/// The mark of a blocked agent, in every mark style, in the box and the tree.
pub(super) const NEEDS_YOU_MARK: &str = "!";

/// The mark of an agent that asked a question, in every mark style, in the box and the tree.
const ASK_MARK: &str = "?";

/// The mark and colour of an agent that needs you: `?` yellow when it asked, `!` red when
/// it waits on a yes or no, so a permission reads apart from a question.
pub(super) fn reason_mark(
    reason: Reason,
    palette: &Palette,
) -> (&'static str, ratatui::style::Color) {
    match reason {
        Reason::Answer => (ASK_MARK, palette.yellow),
        Reason::Approval => (NEEDS_YOU_MARK, palette.red),
    }
}

/// One row of the box.
enum Line<'a> {
    /// A part's heading: `ACTION REQUIRED` in yellow, `COMPLETED` in Herdr's done colour, then
    /// how many entries the part has on a fill of that colour. A narrow sidebar cuts the title, never the count.
    Heading(&'static str, AgentStatus, usize),
    /// An entry that needs you: its mark, name and wait.
    Waiting(Entry<'a>, Reason),
    /// An entry done and not seen yet.
    Done(Entry<'a>),
    /// This many entries did not fit.
    More(usize),
    /// The blank row between ACTION REQUIRED and COMPLETED.
    Blank,
}

/// When this client first saw each pane's agent in some state, and the compose time.
type Times<'a> = Option<(&'a HashMap<String, Instant>, Instant)>;

/// Draws the box above the footer row of `content` (the sidebar left of its divider) and
/// returns the rows it takes, its rule and the blank row under it included: none when
/// `ui.sidebar_tree_panel` is off, nothing waits and nothing is finished, or a third of the
/// sidebar has no room for the rule, the blank row, a heading and a row.
/// `done_times` and `waiting_times` hold when this client first saw each pane's agent
/// finish or need you. An entry under the mouse draws on the focused tree row's grey.
pub(super) fn render_tree_panel(
    buffer: &mut Buffer,
    content: Rect,
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    state: &ShellRenderState<'_>,
    hits: &mut ShellHitMap,
) -> u16 {
    let (done_times, waiting_times) = (state.done_times, state.waiting_times);
    // A third of the sidebar, less the rule above the box and the blank row below it.
    let max_rows = (content.height / 3).saturating_sub(2);
    if !config.sidebar_tree_panel || max_rows == 0 {
        return 0;
    }
    let lines = lines(snapshot, done_times, waiting_times, usize::from(max_rows));
    if lines.is_empty() {
        return 0;
    }
    // At most `max_rows`, so it fits in a u16.
    let rows = lines.len() as u16 + 2;
    let area = Rect::new(
        content.x,
        content.bottom().saturating_sub(1 + rows),
        content.width,
        rows,
    );
    let palette = &config.palette;
    let rule = "─".repeat(usize::from(area.width.saturating_sub(2)));
    let faint = Style::default().fg(palette.surface1);
    put_segment(buffer, area.x + 1, area.y, area.right(), &rule, faint);
    // Lines sit on the tree's columns, inside its padding: marks and headings in its mark
    // column, the rest in its name column; they end where the tree rows' facts do.
    let row = super::tree_sidebar::padded(Rect::new(content.x, area.y, content.width, 1));
    let mark_x = row.x.saturating_add(super::tree_sidebar::MARK_COLUMN);
    let x = row.x.saturating_add(super::tree_sidebar::NAME_COLUMN);
    let end = super::tree_sidebar::facts_end(row, config.mouse_capture);
    let dim = Style::default().fg(palette.overlay0);
    // A name reads as a worker row's does; its mark carries the state.
    let name_style = Style::default().fg(super::tree_sidebar::quiet_name(palette));
    let hover = super::tree_sidebar::focus_band(palette, state.selected_workspace_id.is_some());
    for (y, line) in (area.y + 1..).zip(&lines) {
        let hit = Rect::new(area.x, y, area.width, 1);
        if state.hovered_row.is_some_and(|row| row.intersects(hit))
            && matches!(line, Line::Waiting(..) | Line::Done(_))
        {
            buffer.set_style(super::tree_sidebar::padded(hit), Style::default().bg(hover));
        }
        match line {
            Line::Heading(title, status, count) => {
                let role = status_color(*status, palette);
                let bold = Style::default().fg(role).add_modifier(Modifier::BOLD);
                // The count stays whole, a plain grey number as on the task board's headings;
                // a narrow sidebar cuts the title with `…`.
                let count = count.to_string();
                let room = usize::from(end.saturating_sub(mark_x)).saturating_sub(count.len() + 1);
                let title = crate::ui::truncate_end(title, room);
                let after = put_segment(buffer, mark_x, y, end, &title, bold);
                let after = put_segment(buffer, after, y, end, " ", dim);
                let quiet = Style::default().fg(palette.overlay1);
                put_segment(buffer, after, y, end, &count, quiet);
            }
            Line::More(hidden) => {
                put_label(buffer, x, y, end, &format!("+{hidden} more"), dim);
            }
            Line::Blank => {}
            Line::Waiting(entry, reason) => {
                let (mark, color) = reason_mark(*reason, palette);
                put_segment(buffer, mark_x, y, x, mark, Style::default().fg(color));
                put_entry(buffer, x, y, end, entry, false, name_style, dim);
                hits.tree_panel.push((hit, entry.target.clone()));
            }
            Line::Done(entry) => {
                let done = Style::default().fg(status_color(AgentStatus::Done, palette));
                put_segment(buffer, mark_x, y, x, super::tree_sidebar::DONE_MARK, done);
                put_entry(buffer, x, y, end, entry, true, name_style, dim);
                hits.tree_panel.push((hit, entry.target.clone()));
            }
        }
    }
    rows
}

/// Entries ACTION REQUIRED shows at most; the rest are counted on a `+N more` line.
const NEEDS_YOU_SHOWN: usize = 3;

/// Entries COMPLETED shows at most; the rest are counted on a `+N more` line.
const DONE_SHOWN: usize = 3;

/// Rows a part takes showing `shown` of its `total` entries: its heading, the entries, and
/// `+N more` for the rest; none without entries.
fn part_rows(total: usize, shown: usize) -> usize {
    if total == 0 {
        return 0;
    }
    1 + shown.min(total) + usize::from(shown < total)
}

/// The box's lines, one row each, at most `max_rows`: ACTION REQUIRED and at most
/// `NEEDS_YOU_SHOWN` of its entries when any wait, then a blank row, then COMPLETED and at
/// most `DONE_SHOWN` of its entries when any finished; each part counts the rest on `+N more`,
/// and none when there are neither. When a third of the sidebar is too short, the blank row
/// goes first, then COMPLETED shows fewer entries, then COMPLETED is left out, then ACTION
/// REQUIRED shows fewer entries, down to its heading and `+N more`.
fn lines<'a>(
    snapshot: &'a ClientShellSnapshot,
    done_times: Times<'_>,
    waiting_times: Times<'_>,
    max_rows: usize,
) -> Vec<Line<'a>> {
    let Entries {
        waiting,
        waiting_total,
        finished,
        finished_total,
    } = entries(snapshot, done_times, waiting_times);
    let (mut needs_total, mut done_total) = (waiting_total, finished_total);
    let mut needs_shown = NEEDS_YOU_SHOWN;
    let mut done_shown = DONE_SHOWN;
    // The blank row stands only between two parts.
    let mut blank = needs_total > 0 && done_total > 0;
    let rows = |needs_total, needs_shown, done_total, done_shown, blank| {
        part_rows(needs_total, needs_shown) + usize::from(blank) + part_rows(done_total, done_shown)
    };
    while rows(needs_total, needs_shown, done_total, done_shown, blank) > max_rows {
        if blank {
            blank = false;
        } else if done_total > 0 && done_shown > 0 {
            done_shown -= 1;
        } else if done_total > 0 {
            done_total = 0;
        } else if needs_total > 0 && needs_shown > 0 {
            needs_shown -= 1;
        } else {
            needs_total = 0;
        }
    }
    let mut lines = Vec::with_capacity(needs_shown.min(needs_total) + done_shown + 5);
    if needs_total > 0 {
        lines.push(Line::Heading(
            "ACTION REQUIRED",
            AgentStatus::Blocked,
            waiting_total,
        ));
        let hidden = needs_total.saturating_sub(needs_shown);
        lines.extend(
            waiting
                .into_iter()
                .take(needs_shown)
                .map(|(entry, reason)| Line::Waiting(entry, reason)),
        );
        if hidden > 0 {
            lines.push(Line::More(hidden));
        }
    }
    if blank {
        lines.push(Line::Blank);
    }
    if done_total > 0 {
        lines.push(Line::Heading(
            "COMPLETED",
            AgentStatus::Done,
            finished_total,
        ));
        let hidden = done_total.saturating_sub(done_shown);
        lines.extend(finished.into_iter().take(done_shown).map(Line::Done));
        if hidden > 0 {
            lines.push(Line::More(hidden));
        }
    }
    lines
}

/// The first waiting entries, oldest first, with why, and the first finished ones, newest
/// first, as many as each part shows; ties keep snapshot order, space by space. Each part
/// counts all of its entries.
struct Entries<'a> {
    waiting: Vec<(Entry<'a>, Reason)>,
    waiting_total: usize,
    finished: Vec<Entry<'a>>,
    finished_total: usize,
}

/// How many entries ACTION REQUIRED and COMPLETED hold, from the lists the box draws.
pub(super) fn totals(snapshot: &ClientShellSnapshot) -> (usize, usize) {
    let entries = entries(snapshot, None, None);
    (entries.waiting_total, entries.finished_total)
}

/// Adds `item` to `kept`, sorted by `key` and at most `cap` long; an equal key goes after
/// the ones already kept.
fn keep_first<T, K: Ord>(kept: &mut Vec<T>, item: T, cap: usize, key: impl Fn(&T) -> K) {
    let item_key = key(&item);
    let at = kept.partition_point(|kept| key(kept) <= item_key);
    if at < cap {
        kept.insert(at, item);
        kept.truncate(cap);
    }
}

fn entries<'a>(
    snapshot: &'a ClientShellSnapshot,
    done_times: Times<'_>,
    waiting_times: Times<'_>,
) -> Entries<'a> {
    let mut agents_by_space = HashMap::<&str, Vec<&ClientShellAgent>>::new();
    for agent in &snapshot.agents {
        agents_by_space
            .entry(agent.workspace_id.as_str())
            .or_default()
            .push(agent);
    }
    // A space's main lead is the agent in its first tab's first pane.
    let mut first_tab = HashMap::<&str, &str>::new();
    for tab in &snapshot.tabs {
        first_tab
            .entry(tab.workspace_id.as_str())
            .or_insert(tab.tab_id.as_str());
    }
    let mut first_pane = HashMap::<&str, &str>::new();
    for pane in super::tree_sidebar::tree_panes(snapshot) {
        first_pane
            .entry(pane.tab_id.as_str())
            .or_insert(pane.pane_id.as_str());
    }
    let ago = |times: Times<'_>, pane_id: &str| {
        times.and_then(|(since, now)| {
            since
                .get(pane_id)
                .map(|since| now.saturating_duration_since(*since))
        })
    };
    // Oldest first; one not timed yet counts as the newest.
    let waiting_key =
        |(entry, _): &(Entry<'_>, Reason)| std::cmp::Reverse(entry.ago.unwrap_or_default());
    // Newest first; one not timed yet counts as the oldest.
    let finished_key = |entry: &Entry<'_>| entry.ago.unwrap_or(Duration::MAX);
    let mut kept = Entries {
        waiting: Vec::with_capacity(NEEDS_YOU_SHOWN + 1),
        waiting_total: 0,
        finished: Vec::with_capacity(DONE_SHOWN + 1),
        finished_total: 0,
    };
    for workspace in &snapshot.workspaces {
        let agents = agents_by_space
            .get(workspace.workspace_id.as_str())
            .map(Vec::as_slice)
            .unwrap_or_default();
        let main_lead = first_tab
            .get(workspace.workspace_id.as_str())
            .and_then(|tab_id| first_pane.get(tab_id));
        for agent in agents {
            let reason = needs_you(agent);
            if reason.is_none() && agent.agent_status != AgentStatus::Done {
                continue;
            }
            let name = pane_label(agent);
            let entry = Entry {
                space: workspace.label.as_str(),
                agent: (main_lead != Some(&agent.pane_id.as_str())).then_some(name),
                lead: token(&agent.tokens, "lead").is_none(),
                ago: match reason {
                    Some(_) => ago(waiting_times, &agent.pane_id),
                    None => ago(done_times, &agent.pane_id),
                },
                target: TreeTarget::Pane(agent.pane_id.clone()),
            };
            match reason {
                Some(reason) => {
                    kept.waiting_total += 1;
                    let entry = (entry, reason);
                    keep_first(&mut kept.waiting, entry, NEEDS_YOU_SHOWN, waiting_key);
                }
                None => {
                    kept.finished_total += 1;
                    keep_first(&mut kept.finished, entry, DONE_SHOWN, finished_key);
                }
            }
        }
    }
    kept
}

/// Writes how long ago: without `ago_words` (a wait) as a working row's time; with them
/// `just now` under a minute, else `<N>m` and past an hour `<N>h<M>m`, then ` ago`.
fn push_ago(line: &mut String, ago: Option<Duration>, ago_words: bool) {
    let Some(ago) = ago else {
        return;
    };
    if !ago_words {
        if let Some(time) = minutes_text(ago) {
            line.push_str(&time);
        }
        return;
    }
    let minutes = ago.as_secs() / 60;
    // Writing to a String cannot fail.
    let _ = match minutes {
        0 => write!(line, "just now"),
        1..60 => write!(line, "{minutes}m"),
        _ => write!(line, "{}h{}m", minutes / 60, minutes % 60),
    };
    if minutes > 0 {
        line.push_str(" ago");
    }
}

/// An entry's line from `x` to `end`: its name, cut with `…` so how long ago it waited or
/// finished fits at the right after a space. A lead's own name gives way last: its space's
/// part shrinks first, then the time goes (see [`push_lead_name`]). One string holds both.
#[allow(clippy::too_many_arguments)] // Position, entry and the two styles of one line.
fn put_entry(
    buffer: &mut Buffer,
    x: u16,
    y: u16,
    end: u16,
    entry: &Entry<'_>,
    ago_words: bool,
    name_style: Style,
    ago_style: Style,
) {
    let room = usize::from(end.saturating_sub(x));
    let mut line = String::with_capacity(room + 8);
    push_ago(&mut line, entry.ago, ago_words);
    // ASCII, so bytes are columns.
    let mut ago_width = line.len();
    let name_room = room.saturating_sub(ago_width + usize::from(ago_width > 0));
    match entry.agent.filter(|_| entry.lead) {
        Some(agent) => {
            if !push_lead_name(&mut line, entry.space, agent, name_room) {
                line.clear();
                ago_width = 0;
                push_lead_name(&mut line, entry.space, agent, room);
            }
        }
        None => {
            push_cut(&mut line, entry.name_chars(), name_room);
        }
    }
    put_segment(buffer, x, y, end, &line[ago_width..], name_style);
    // Fits in u16: at most `room` columns.
    let ago_x = end.saturating_sub(ago_width as u16);
    put_segment(buffer, ago_x, y, end, &line[..ago_width], ago_style);
}

/// Pushes a lead's `space/agent` onto `line` within `room` columns: the space's part cut
/// down to its first letter and `…` before the agent's name is cut. Returns whether the
/// agent's whole name fit; when it did not, its name is cut in its middle as the tree cuts
/// a lead's (`h…/he…-lead`).
fn push_lead_name(line: &mut String, space: &str, agent: &str, room: usize) -> bool {
    let visible = |text: &str| {
        text.chars()
            .filter(|ch| !ch.is_control())
            .collect::<String>()
    };
    let (space, agent) = (visible(space), visible(agent));
    let agent_width = usize::from(super::render::display_width(&agent));
    let space_least = usize::from(super::render::display_width(&space)).min(2);
    let fits = room
        .checked_sub(agent_width + 1)
        .filter(|&space_room| space_room >= space_least);
    let space_room = fits.unwrap_or(space_least);
    let used = push_cut(line, space.chars(), space_room);
    line.push('/');
    let agent_room = room.saturating_sub(used + 1);
    push_cut(
        line,
        super::tree_sidebar::cut_lead_name(&agent, agent_room).chars(),
        agent_room,
    );
    fits.is_some()
}

/// Pushes `chars` onto `line`, at most `width` columns, the last one `…` when any are
/// left out; returns the columns pushed. A zero-width character is kept only right after
/// a visible one (a combining mark), so the line stays within twice `width` characters.
fn push_cut(line: &mut String, chars: impl Iterator<Item = char>, width: usize) -> usize {
    let start = line.len();
    let mut used = 0;
    let mut after_visible = false;
    let mut chars = chars.peekable();
    while let Some(ch) = chars.next_if(|ch| used + ch.width().unwrap_or(0) <= width) {
        let ch_width = ch.width().unwrap_or(0);
        if ch_width > 0 || after_visible {
            line.push(ch);
        }
        after_visible = ch_width > 0;
        used += ch_width;
    }
    if chars.peek().is_none() {
        return used;
    }
    // Room for `…`, and no space before it.
    while line.len() > start && (used >= width || line.ends_with(' ')) {
        if let Some(ch) = line.pop() {
            used -= ch.width().unwrap_or(0);
        }
    }
    if used < width {
        line.push('…');
        used += 1;
    }
    used
}

#[cfg(test)]
mod tests {
    use super::push_cut;

    #[test]
    fn a_cut_line_stays_bounded_by_zero_width_characters() {
        let text = format!("ok{}go", "\u{200b}".repeat(1000));
        let mut line = String::new();
        let width = push_cut(&mut line, text.chars(), 8);
        assert_eq!(width, 4);
        assert!(line.chars().count() <= 16, "{} chars", line.chars().count());
        let mut accented = String::new();
        push_cut(&mut accented, "e\u{301}t\u{301}".chars(), 8);
        assert_eq!(
            accented, "e\u{301}t\u{301}",
            "a combining mark stays on its letter"
        );
    }
}
