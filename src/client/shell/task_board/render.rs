//! The Tasks view, drawn into the main area. Pure: reads the board and the palette, writes
//! the buffer, and returns where it drew.

use std::collections::HashMap;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use super::model::{DueLevel, Task, TaskGroup, TaskSection, WorkItem, WorkStage};
use super::sort::TaskSort;
use super::spacing;
use super::style;

use super::table::{
    cut_fields, detail_keys, duration, file_field, scheduled, LiveText, LiveWidest, RowLayout,
    Table, Widest, BOTTOM_RULE, HEAD_RULE, LIVE_IDLE, TOP_RULE,
};
use super::text::{at_most, clean, cut, put_fitted, wrap, Marks};
use super::{
    AgentState, BoardAgent, BoardView, Category, FilterPick, Fold, Mark, RowKey, RowPrompt, Spans,
    TaskBoard, TaskBoardHits, TaskKey,
};
use crate::app::state::Palette;
use crate::client::shell::render::{display_width, put_segment};
use crate::client::shell::tree_sidebar::{phase_word, stage_color};

/// The sections under IN PROGRESS in the order they show, with their headings.
const SECTIONS: [(&str, TaskSection); 4] = [
    ("ACTION REQUIRED", TaskSection::ActionRequired),
    ("QUEUED", TaskSection::Queued),
    ("WAITING ON OTHERS", TaskSection::Waiting),
    ("COMPLETED", TaskSection::Completed),
];

/// COMPLETED's index: the only section that folds.
const DONE_SECTION: usize = SECTIONS.len() - 1;

/// IN PROGRESS's heading, over the working agents' rows.
const IN_PROGRESS: &str = "IN PROGRESS";

/// A task's section under IN PROGRESS. A task in progress has a row of its own only while no
/// working agent's row shows it, and then shows where its group puts it.
fn section(task: &Task) -> usize {
    section_index(shown_section(task))
}

/// The section a task's own row shows in: a task in progress shows where its group puts it.
fn shown_section(task: &Task) -> TaskSection {
    match task.section() {
        TaskSection::InProgress => task.group_section(),
        section => section,
    }
}

/// A section's index in `SECTIONS`; a section a newer reader added waits on others.
fn section_index(section: TaskSection) -> usize {
    match section {
        TaskSection::ActionRequired => 0,
        TaskSection::Queued | TaskSection::InProgress => 1,
        TaskSection::Waiting | TaskSection::Unknown => 2,
        TaskSection::Completed => 3,
    }
}

/// A key and what it does, as the footer, a detail line, a question and the panel show it.
pub(super) type Hint = (&'static str, &'static str);

/// Each hint is its key as a filled block with a space each side, one space, its words dim,
/// and `spacing::HINT_GAP` columns before the next hint. A key of several glyphs keeps one
/// space between them.
const KEY_GAP: &str = " ";

/// The Tasks view's footer, most used first; the rest show on `?`. `o` opens the selected
/// task's file only while the server takes `tasks.open`; the keys that act on it only while
/// it takes `tasks.act`.
fn tasks_keys(can_act: bool, can_open: bool) -> (Vec<Hint>, Vec<Hint>) {
    let mut main = vec![("↑ ↓", "select"), ("Enter", "go to space")];
    if can_open {
        main.push(("o", "open file"));
    }
    let mut rest = Vec::new();
    if can_act {
        main.extend([("d", "done"), ("x", "delete"), ("m", "move")]);
        rest.extend([("J/K", "reorder"), ("n", "new task")]);
    }
    main.extend([("/", "search"), ("Tab", "Phases")]);
    rest.extend([
        ROW_ARROWS,
        ("s", "sort"),
        ("click a space", "only its tasks"),
        ("Esc", "close"),
    ]);
    (main, rest)
}

/// The arrows that open and close a row's detail, as `?` lists them; a selected row's footer
/// names the one that does something to it.
const ROW_ARROWS: Hint = ("→ ←", "details");
const OPEN_ROW: Hint = ("→", "details");
const CLOSE_ROW: Hint = ("←", "close");

/// The Work view's footer, most used first; the rest show on `?`. New work has no key (the
/// empty view says where it comes from); opening a card only while the server takes
/// `tasks.open`.
fn work_keys(can_open: bool) -> (Vec<Hint>, Vec<Hint>) {
    let mut main = vec![("← ↑ ↓ →", "select")];
    let mut rest = Vec::new();
    if can_open {
        main.push(("Double-click", "open"));
        rest.push(("o", "open file"));
    }
    main.extend([("/", "search"), ("Tab", "Tasks")]);
    rest.extend([
        ("f", "filter"),
        ("Enter", "go to lead's tab"),
        ("t", "its tasks"),
        ("click a space", "only its work"),
        ("Esc", "close"),
    ]);
    (main, rest)
}

/// A view's footer keys: the row it shows, and the rest `?` adds.
fn view_keys(view: BoardView, can_act: bool, can_open: bool) -> (Vec<Hint>, Vec<Hint>) {
    match view {
        BoardView::Tasks => tasks_keys(can_act, can_open),
        BoardView::Work => work_keys(can_open),
    }
}

/// One hint's width, as it is drawn.
fn hint_width(hint: &Hint) -> u16 {
    hint_texts(hint)
        .iter()
        .map(|text| display_width(text))
        .sum()
}

/// A line of hints' width, as it is drawn.
pub(super) fn hints_width(line: &[Hint]) -> u16 {
    let hints: u16 = line.iter().map(hint_width).sum();
    hints + spacing::HINT_GAP * (line.len() as u16).saturating_sub(1)
}

/// One hint's text: its key's block, then its words.
fn hint_texts((key, what): &Hint) -> [String; 2] {
    [format!(" {key} "), format!("{KEY_GAP}{what}")]
}

/// One hint's pieces: its key's block, then its words dim.
fn one_hint_spans(hint: &Hint, palette: &Palette) -> [(String, Style); 2] {
    let [block, words] = hint_texts(hint);
    [
        (block, style::key(palette)),
        (words, Style::default().fg(palette.overlay0)),
    ]
}

/// A line of hints' pieces, left to right: each key's block, then its words, and the gap
/// before the next.
fn hint_spans(hints: &[Hint], palette: &Palette) -> Vec<(String, Style)> {
    let mut spans = Vec::new();
    for (index, hint) in hints.iter().enumerate() {
        if index > 0 {
            spans.push((spacing::gap(spacing::HINT_GAP), Style::default()));
        }
        spans.extend(one_hint_spans(hint, palette));
    }
    spans
}

/// Draws `hints` from `x` on row `y`, each a target a click does its key by; returns the
/// column after the last.
pub(super) fn put_hints(
    buffer: &mut Buffer,
    x: u16,
    y: u16,
    end: u16,
    hints: &[Hint],
    palette: &Palette,
    targets: &mut Vec<(Rect, &'static str)>,
) -> u16 {
    let mut x = x;
    for (index, hint) in hints.iter().enumerate() {
        if index > 0 {
            x = x.saturating_add(spacing::HINT_GAP);
        }
        let start = x;
        let [(block, block_style), (words, words_style)] = one_hint_spans(hint, palette);
        let key_end = put_segment(buffer, x, y, end, &block, block_style);
        x = put_segment(buffer, key_end, y, end, &words, words_style);
        targets.extend(footer_targets(hint.0, start, key_end, x, y));
    }
    x
}

/// The most lines the notice, a failed read or the problems each take above the footer.
const INFO_MAX_LINES: usize = 3;

/// The chip row: under the header and a blank row, right over the body.
fn chip_row(area: Rect) -> u16 {
    area.y.saturating_add(2)
}

/// Where the foot and the body draw, for a footer of some key rows under some info lines.
struct FootLayout {
    /// The top key row.
    first_keys: u16,
    /// The rule over the keys.
    rule: u16,
    /// The first info line.
    foot_top: u16,
    /// The view's body: under the header, a blank row and the chip row, over a blank row and
    /// the foot.
    body: Rect,
}

impl FootLayout {
    fn new(area: Rect, key_rows: usize, info_rows: usize) -> Self {
        let bottom = area.bottom().saturating_sub(1);
        let first_keys = bottom.saturating_sub((key_rows as u16).saturating_sub(1));
        let rule = first_keys.saturating_sub(1);
        let body_top = chip_row(area).saturating_add(1);
        let foot_top = rule.saturating_sub(info_rows as u16).max(body_top);
        let body = Rect::new(
            area.x,
            body_top,
            area.width,
            foot_top.saturating_sub(1).saturating_sub(body_top),
        );
        Self {
            first_keys,
            rule,
            foot_top,
            body,
        }
    }
}

/// The key lines the footer draws, top down: one row of the view's most used keys, as many
/// as fit, and `? all keys`. With every key asked for (`?`), all of them, wrapped, and
/// `? fewer keys`. A key in `shown_elsewhere` (a button the Work panel draws, or a key the
/// open row's block draws), or one doing what a key there does, is left out. A selected row's `row` hints (its arrow, then its own
/// keys while it is closed) follow `↑ ↓` and take the place of the view's hints for the same
/// keys.
fn footer_plan(
    board: &TaskBoard,
    area: Rect,
    shown_elsewhere: &[Hint],
    row: &[Hint],
) -> Vec<Vec<Hint>> {
    let width = area.width.saturating_sub(2 * spacing::MARGIN);
    let (mut main, mut rest) = view_keys(board.view, board.can_act, board.can_open);
    if !row.is_empty() {
        let same = |(key, _): &Hint| row.iter().any(|(own, _)| own == key);
        main.retain(|hint| !same(hint));
        rest.retain(|hint| !same(hint) && *hint != ROW_ARROWS);
        let at = main.len().min(1);
        main.splice(at..at, row.iter().copied());
    }
    // A key the panel shows, or what it does under another key there (`Enter open file`).
    let here = |(key, words): &Hint| {
        !shown_elsewhere
            .iter()
            .any(|(shown, does)| shown == key || does == words)
    };
    let (main, rest): (Vec<Hint>, Vec<Hint>) = (
        main.into_iter().filter(here).collect(),
        rest.into_iter().filter(here).collect(),
    );
    if board.keys_shown {
        let mut every: Vec<_> = main.into_iter().chain(rest).collect();
        every.push(("?", "fewer keys"));
        // Each key with its words kept whole, as many lines as they take, up to half the
        // board.
        let mut wrapped: Vec<Vec<Hint>> = vec![Vec::new()];
        for key in every {
            let last = wrapped.last_mut().expect("one line");
            last.push(key);
            if last.len() > 1 && hints_width(last) > width {
                let key = last.pop().expect("just pushed");
                wrapped.push(vec![key]);
            }
        }
        // Cut to the room, `? fewer keys` stays last: it is the way back.
        let room = usize::from(area.height / 2).max(1);
        if wrapped.len() > room {
            wrapped.truncate(room);
            if let Some(last) = wrapped.last_mut() {
                last.pop();
                last.push(("?", "fewer keys"));
            }
        }
        return wrapped;
    }
    // The most used keys that fit before `? all keys`, which always ends the row.
    let mut line = main;
    line.push(("?", "all keys"));
    while line.len() > 1 && hints_width(&line) > width {
        line.remove(line.len() - 2);
    }
    vec![line]
}

/// The footer keys a click can do: those that name one key. `J/K` is two, each its own
/// cell; a hint naming several keys (`↑ ↓`) or a mouse gesture is none.
fn is_clickable_key(key: &str) -> bool {
    matches!(key, "Enter" | "Tab" | "Esc") || key.chars().count() == 1
}

/// Where a click does a hint's key: its block from `start` to `key_end`, its words to `end`,
/// on row `y`.
fn footer_targets(
    key: &'static str,
    start: u16,
    key_end: u16,
    end: u16,
    y: u16,
) -> impl Iterator<Item = (Rect, &'static str)> {
    let parts: Vec<&'static str> = key.split('/').collect();
    let whole = (is_clickable_key(key) && key_end > start)
        .then(|| (Rect::new(start, y, end.saturating_sub(start), 1), key));
    let split = (parts.len() > 1 && parts.iter().all(|part| is_clickable_key(part)))
        .then(|| {
            // Each part's cell inside the block, the `/` between them doing nothing.
            parts
                .into_iter()
                .enumerate()
                .filter_map(move |(index, part)| {
                    let x = start.saturating_add(1 + 2 * index as u16);
                    (x < key_end).then(|| (Rect::new(x, y, 1, 1), part))
                })
        })
        .into_iter()
        .flatten();
    whole.into_iter().chain(split)
}

/// Every key a view's footer names, each with what it does.
#[cfg(test)]
pub(in crate::client::shell) fn footer_keys(
    view: BoardView,
    can_act: bool,
    can_open: bool,
) -> Vec<Hint> {
    let (main, rest) = view_keys(view, can_act, can_open);
    main.into_iter().chain(rest).collect()
}

pub(in crate::client::shell) fn render_task_board(
    buffer: &mut Buffer,
    area: Rect,
    board: &TaskBoard,
    palette: &Palette,
    home_dir: Option<&str>,
    live: (&[BoardAgent], Mark),
) -> TaskBoardHits {
    let mut hits = TaskBoardHits {
        area,
        ..TaskBoardHits::default()
    };
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        render_too_small(buffer, area, palette, &mut hits);
        return hits;
    }
    render_board(buffer, area, board, palette, home_dir, live, &mut hits);
    // Over whatever the view drew.
    render_space_menu(buffer, area, board, palette, &mut hits);
    render_sort_menu(buffer, area, board, palette, &mut hits);
    render_work_filter_menu(buffer, area, board, live.0, palette, &mut hits);
    render_agent_menu(buffer, area, board, live.0, palette, &mut hits);
    hits
}

/// The header, the foot and the view's body: the tasks' sections, or the Work view.
fn render_board(
    buffer: &mut Buffer,
    area: Rect,
    board: &TaskBoard,
    palette: &Palette,
    home_dir: Option<&str>,
    (agents, worker_mark): (&[BoardAgent], Mark),
    hits: &mut TaskBoardHits,
) {
    let dim = Style::default().fg(palette.overlay0);
    let list = board.store.list();
    let words = super::filter::words(&board.filter);
    let marks = Marks {
        words: &words,
        style: style::mark(palette),
    };
    // The agent the header's agent button shows alone, if any: its own tasks and tab.
    let chosen = super::shown_agent(board, agents);
    // The live agents' tabs in the space shown, working or waiting on the user, and those the
    // filter keeps.
    let spaced_agents = super::spaced_agents(board, agents);
    let is_working = |agent: &BoardAgent| matches!(agent.state, AgentState::Working(_));
    let is_waiting = |agent: &BoardAgent| matches!(agent.state, AgentState::Waiting(_));
    let live_of = spaced_agents
        .iter()
        .filter(|agent| is_working(agent))
        .count();
    let waiting_of = spaced_agents
        .iter()
        .filter(|agent| is_waiting(agent))
        .count();
    let live = list.map_or_else(Vec::new, |list| live_agents(list, &spaced_agents, &words));
    let waiting = waiting_tabs(&spaced_agents, &words);
    let spaced_waiting: Vec<&BoardAgent> = spaced_agents
        .iter()
        .copied()
        .filter(|agent| is_waiting(agent))
        .collect();
    // The narrowed space's tasks, and those the filter keeps: the counts say one of the other.
    let spaced: Vec<&Task> = list
        .map(|list| {
            list.tasks
                .iter()
                .filter(|task| {
                    in_view(board, chosen, task)
                        && !named_waiting(task, &spaced_waiting)
                        && !in_live_row(task, &spaced_agents)
                })
                .collect()
        })
        .unwrap_or_default();
    let tasks: Vec<&Task> = list
        .map(|list| matched_tasks(board, list, agents, &words))
        .unwrap_or_default();
    let waiting_count = (waiting.len(), waiting_of);
    let working_count = (
        live.iter().filter(|agent| is_working(agent)).count(),
        live_of,
    );
    let agent_button = match (super::offers_agents(board, agents), chosen) {
        (false, _) => AgentButton::Hidden,
        (true, None) => AgentButton::All,
        (true, Some(agent)) => AgentButton::One(agent),
    };
    render_header(buffer, area, board, agent_button, palette, hits);
    // Before any list is read there is nothing to count, and zeros would read as real.
    if list.is_some() {
        let row = Rect::new(area.x, chip_row(area), area.width, 1);
        match board.view {
            BoardView::Tasks => render_task_chips(
                buffer,
                row,
                board,
                (&tasks, &spaced),
                (waiting_count, working_count),
                palette,
                hits,
            ),
            BoardView::Work => {
                render_work_picks(buffer, row, board, palette, hits);
            }
        }
    }
    if board.filter_editing {
        render_filter_hint(buffer, area, palette, hits);
    } else if let Some(chosen) = chosen {
        render_agent_note(buffer, area, board.view, chosen, palette);
    }
    let today = list.and_then(|list| list.generated.get(..10)).unwrap_or("");
    let tasks = in_shown_order(board, tasks, today);
    // Every row in the order it shows: IN PROGRESS's agents, the tabs waiting on the user with
    // ACTION REQUIRED, then the tasks, each while no other section shows alone.
    let mut entries: Vec<Entry> = Vec::new();
    if board
        .only_section
        .is_none_or(|only| only == TaskSection::InProgress)
    {
        entries.extend(live_tree(&live, &spaced_agents));
    }
    if board
        .only_section
        .is_none_or(|only| only == TaskSection::ActionRequired)
    {
        entries.extend(waiting.iter().map(|agent| Entry::Waiting(agent)));
    }
    entries.extend(tasks.iter().map(|task| Entry::Task(task)));

    // The foot, bottom up: keys, a rule, then the last action's notice, the last read's
    // error and the problems.
    let left = area.x.saturating_add(spacing::MARGIN);
    let inner = area.width.saturating_sub(2 * spacing::MARGIN);
    // Each on as many lines as it takes, up to three, the rest of a line under its text.
    let info_width = inner;
    // A line's label in its own style (red for a failure), the rest in another.
    let mut info: Vec<Spans> = Vec::new();
    let push_info =
        |info: &mut Vec<Spans>, (lead, lead_style): (&str, Style), text: &str, style: Style| {
            let lines = at_most(
                wrap(text, info_width.saturating_sub(display_width(lead))),
                INFO_MAX_LINES,
                info_width.saturating_sub(display_width(lead)),
            );
            let pad = " ".repeat(usize::from(display_width(lead)));
            for (index, line) in lines.into_iter().enumerate() {
                let lead = if index == 0 { lead } else { pad.as_str() };
                info.push(vec![(lead.to_owned(), lead_style), (line, style)]);
            }
        };
    let text = Style::default().fg(palette.text);
    let red = Style::default().fg(palette.red);
    if let Some(notice) = &board.notice {
        push_info(&mut info, ("", text), notice, text);
    }
    if let (Some(_), Some(error)) = (list, board.store.error()) {
        push_info(&mut info, ("Last read failed: ", red), error, text);
    }
    // The problems say only how many notes files have one, until a click on their lines or
    // `?` asks for each with its path; a click on the paths goes back to the count.
    let problem_start = info.len();
    if let Some(list) = list.filter(|list| !list.problems.is_empty()) {
        if board.keys_shown || board.problems_shown {
            let problems = list.problems.join("  ·  ");
            push_info(&mut info, ("Problems: ", red), &problems, text);
        } else {
            // The count has no label: it stays red whole.
            push_info(&mut info, ("", red), &problem_count(&list.problems), red);
        }
    }
    // Under `?` a click would change nothing drawn: the paths show either way.
    let problem_lines = if board.keys_shown {
        0..0
    } else {
        problem_start..info.len()
    };
    let foot = |key_rows: usize| FootLayout::new(area, key_rows, info.len());
    let selected = board
        .selected
        .as_ref()
        .and_then(|key| entries.iter().position(|entry| entry.key() == *key));
    // The selected row shows its detail block under it while it is the open one.
    let open = selected.filter(|index| board.open_row.as_ref() == Some(&entries[*index].key()));
    // The selected row's own keys, and the view's keys that do nothing to it (those that act
    // on a task, while a tab's row is selected). None with nothing selected, in the Work view,
    // or while a question or an action on the task takes the block's place.
    let covered = |task: &Task| {
        let key = TaskKey::of(task);
        board.prompt.as_ref().is_some_and(|(on, _)| *on == key)
            || board
                .acting
                .as_ref()
                .is_some_and(|acting| acting.task == key)
    };
    let row_keys = match selected.map(|index| entries[index]) {
        _ if board.view != BoardView::Tasks => None,
        Some(Entry::Task(task)) if !covered(task) => {
            let has_card = list.and_then(|read| super::task_card(task, read)).is_some();
            let keys = detail_keys(task, has_card, board.can_open, board.can_act);
            Some((keys, Vec::new()))
        }
        Some(Entry::Live(agent) | Entry::Waiting(agent)) => Some((
            list.map_or_else(|| vec![GO_TO_AGENT], |read| agent_keys(agent, read)),
            TASK_ONLY_KEYS.to_vec(),
        )),
        _ => None,
    };
    // Open, the row's block ends in its keys and `← close`, and the footer leaves them out.
    // Closed, the footer names `→ details`, then the row's keys in place of the view's.
    let (block_keys, footer_hidden, row_hints) = match row_keys {
        None => (Vec::new(), Vec::new(), Vec::new()),
        Some((keys, not_here)) if open.is_some() => {
            let hidden = keys.iter().copied().chain(not_here).collect();
            let block = keys.into_iter().chain([CLOSE_ROW]).collect();
            (block, hidden, Vec::new())
        }
        Some((keys, not_here)) => {
            let hints = std::iter::once(OPEN_ROW).chain(keys).collect();
            (Vec::new(), not_here, hints)
        }
    };
    // The Work panel's buttons leave the footer while it draws them. Where they show in the
    // body the whole footer leaves, they show in the taller one a shorter footer leaves too.
    let every_key = footer_plan(board, area, &footer_hidden, &row_hints);
    let panel_keys = match (board.view, list) {
        (BoardView::Work, Some(read)) => {
            let scope = super::work::Scope::of(board, agents);
            super::work::panel_keys(board, read, (scope, &words), foot(every_key.len()).body)
        }
        _ => Vec::new(),
    };
    let key_lines = if panel_keys.is_empty() {
        every_key
    } else {
        footer_plan(board, area, &panel_keys, &row_hints)
    };
    let FootLayout {
        first_keys,
        rule,
        foot_top,
        body,
    } = foot(key_lines.len());
    for (index, keys) in key_lines.iter().enumerate() {
        let y = first_keys.saturating_add(index as u16);
        put_hints(
            buffer,
            left,
            y,
            area.right(),
            keys,
            palette,
            &mut hits.footer_keys,
        );
    }
    put_segment(
        buffer,
        left,
        rule,
        area.right(),
        &"─".repeat(usize::from(inner)),
        dim,
    );
    for (index, spans) in info.iter().enumerate() {
        let y = foot_top.saturating_add(index as u16);
        if y < rule {
            let mut at = left;
            for (text, style) in spans {
                at = put_segment(buffer, at, y, area.right(), text, *style);
            }
            if problem_lines.contains(&index) {
                let line = Rect::new(left, y, at.saturating_sub(left).min(inner), 1);
                hits.problems = if hits.problems.is_empty() {
                    line
                } else {
                    hits.problems.union(line)
                };
            }
        }
    }

    let Some(read) = list else {
        let message = match board.store.error() {
            Some(error) => format!("Tasks unavailable: {error}"),
            None => "Reading tasks…".to_owned(),
        };
        put_fitted(
            buffer,
            body.x.saturating_add(1),
            body.y,
            body.right(),
            &message,
            Style::default().fg(palette.overlay1),
        );
        return;
    };
    if board.view == BoardView::Work {
        super::work::render_work(
            buffer,
            body,
            board,
            read,
            home_dir,
            (agents, worker_mark),
            palette,
            hits,
        );
        return;
    }
    if entries.is_empty() {
        let message = match (&board.space, words.is_empty(), board.only_section) {
            (_, false, _) => format!("No tasks match \"{}\"", clean(board.filter.trim())),
            (_, true, None) if chosen.is_some() => {
                format!(
                    "No tasks for {}",
                    chosen.map(|agent| clean(&agent.tab)).unwrap_or_default()
                )
            }
            (_, true, Some(TaskSection::InProgress)) => format!("No tasks in {IN_PROGRESS}"),
            (_, true, Some(only)) => format!("No tasks in {}", SECTIONS[section_index(only)].0),
            (Some(space), true, None) => format!("No tasks in {}", clean(space)),
            (None, true, None) => "No tasks".to_owned(),
        };
        put_fitted(
            buffer,
            body.x.saturating_add(1),
            body.y,
            body.right(),
            &message,
            dim,
        );
        return;
    }

    let numbers = RowNumbers::of(board, &tasks);
    // Each IN PROGRESS row's task, looked up once a frame: its row shows it and its minutes
    // on it.
    let live_tasks: Vec<Option<&Task>> = entries
        .iter()
        .map(|entry| match entry {
            Entry::Live(agent) => super::worker_task(agent, read),
            _ => None,
        })
        .collect();
    let minutes = |task: Option<&Task>| task?.agent.as_ref()?.minutes;
    // Each IN PROGRESS row's cells, from its card, its task and its labels.
    let live_texts: Vec<LiveText> = entries
        .iter()
        .zip(&live_tasks)
        .map(|(entry, task)| match entry {
            Entry::Live(agent) => live_text(agent, *task, &numbers, &read.work, palette),
            Entry::Task(_) | Entry::Waiting(_) => LiveText::default(),
        })
        .collect();
    let mut live_widest = LiveWidest::default();
    let lives = entries
        .iter()
        .zip(live_tasks.iter().zip(&live_texts))
        .filter_map(|(entry, task)| match entry {
            Entry::Live(agent) => Some((*agent, task)),
            _ => None,
        });
    let said = |text: Option<&str>| text.map_or(display_width(LIVE_IDLE), display_width);
    for (agent, (task, text)) in lives.take(LIVE_MOST) {
        live_widest.agent = live_widest.agent.max(display_width(&clean(&agent.tab)));
        live_widest.working_on = live_widest.working_on.max(text.working_on_width());
        let stage = text.stage.map_or(0, |(word, _)| display_width(word));
        live_widest.stage = live_widest.stage.max(stage);
        live_widest.step = live_widest.step.max(said(text.step));
        live_widest.doing = live_widest.doing.max(said(text.doing));
        if let Some(minutes) = minutes(*task) {
            live_widest.since = live_widest.since.max(display_width(&duration(minutes)));
        }
    }
    // Each section's widest texts: its own cells fit them, and one Project, `#` and Task
    // width fits every section's.
    let mut widest = [Widest::default(); SECTIONS.len()];
    for entry in &entries {
        match entry {
            Entry::Task(task) => {
                let index = section(task);
                widest[index].task(SECTIONS[index].1, task, today);
            }
            Entry::Waiting(agent) => widest[0].waiting(agent),
            Entry::Live(..) => {}
        }
    }
    let drawn: Vec<bool> = (0..SECTIONS.len())
        .map(|index| {
            entries
                .iter()
                .any(|entry| entry.part() == Part::Section(index))
                && (index != DONE_SECTION || board.done_expanded)
        })
        .collect();
    let project = widest.iter().map(Widest::project).fold(0, u16::max);
    let kinds: Vec<TaskSection> = SECTIONS.iter().map(|(_, kind)| *kind).collect();
    let section_columns = Table::sections(area, &kinds, &widest, &drawn, project);
    // Where every box's borders sit, and a row's detail lines up; the same in each.
    let columns = &section_columns[0];
    let live_columns = Table::live(area, live_widest);
    let table = |part: Part| match part {
        Part::Live => &live_columns,
        Part::Section(index) => &section_columns[index],
    };
    // Each row that starts with a spinner, for the spinner clock once the row is drawn.
    let mut spinning = vec![false; entries.len()];
    // The open row wraps its cells to show their text whole.
    let rows: Vec<RowLayout> = entries
        .iter()
        .zip(&live_tasks)
        .enumerate()
        .map(|(index, (entry, live_task_of))| (index, open == Some(index), entry, *live_task_of))
        .map(|(index, is_open, entry, live_task_of)| match entry {
            Entry::Task(task) => {
                // A task an agent is at work on, while it is not done, starts with its spinner.
                let on = (task.group != TaskGroup::Done
                    && super::tag_works(task.agent.as_ref(), &task.space, agents))
                .then_some(worker_mark);
                spinning[index] = on.is_some();
                RowLayout::new(
                    task,
                    numbers.of_task(task),
                    table(entry.part()),
                    palette,
                    today,
                    on,
                    is_open,
                )
            }
            Entry::Live(agent) => {
                spinning[index] = matches!(agent.state, AgentState::Working(_));
                RowLayout::live(
                    agent,
                    live_texts[index],
                    minutes(live_task_of),
                    &live_columns,
                    palette,
                    is_open,
                )
            }
            Entry::Waiting(agent) => {
                RowLayout::waiting(agent, table(entry.part()), palette, is_open)
            }
        })
        .collect();
    // The lines under a row: the selected row's detail block, or a question or typed line
    // open on a task, or the action it waits on.
    let indent = columns.detail_indent();
    let under_width = columns.inner_width().saturating_sub(indent + 1);
    let mut under: Vec<Option<Under>> = vec![None; entries.len()];
    let task_row = |key: &TaskKey| {
        entries
            .iter()
            .position(|entry| matches!(entry, Entry::Task(task) if TaskKey::of(task) == *key))
    };
    if let Some(index) = open {
        // The block holds only what its row does not show: the row shows the rest whole.
        let (fields, value_x, width) = match entries[index] {
            Entry::Task(task) => (
                task_fields(
                    task,
                    table(entries[index].part()),
                    (read, agents),
                    (home_dir, today),
                ),
                0,
                columns.inner_width(),
            ),
            // A LIVE row shows its card's title where it has one, else its task, while it has
            // a Working on column.
            Entry::Live(agent) => (
                agent_fields(agent, read, &numbers, agents, live_columns.shows_task()),
                live_columns.doing_offset(),
                live_columns.inner_width(),
            ),
            Entry::Waiting(agent) => (
                agent_fields(agent, read, &numbers, agents, false),
                0,
                columns.inner_width(),
            ),
        };
        under[index] = Some(Under {
            lines: detail_block(&fields, &block_keys, (indent, value_x), width, palette),
            banded: true,
        });
    }
    if let Some((key, prompt)) = &board.prompt {
        if let Some(index) = task_row(key) {
            let (text, keys) = prompt_text(prompt);
            under[index] = Some(Under {
                lines: led(
                    text_then_keys(
                        wrap_spaced(&text, under_width),
                        Style::default().fg(palette.text),
                        &keys,
                        under_width,
                        palette,
                    ),
                    indent,
                    dim,
                ),
                banded: selected == Some(index),
            });
        }
    }
    if let Some(acting) = &board.acting {
        if let Some(index) = task_row(&acting.task) {
            // The spinner in yellow, the words in grey.
            let grey = Style::default().fg(palette.overlay1);
            let lines = wrap_spaced(&format!("◐ {}…", acting.label), under_width)
                .into_iter()
                .enumerate()
                .map(|(index, line)| match line.strip_prefix("◐") {
                    Some(words) if index == 0 => vec![
                        ("◐".to_owned(), Style::default().fg(palette.yellow)),
                        (words.to_owned(), grey),
                    ],
                    _ => vec![(line, grey)],
                })
                .collect();
            under[index] = Some(Under {
                lines: led(lines, indent, dim),
                banded: selected == Some(index),
            });
        }
    }

    // Every line of LIVE and the sections, before scrolling: each heading on its own line
    // (a design pick, 2026-09-28: not drawn in the border), then a rounded box in its colour (in a
    // grid the column names and a rule under them, its rows, the bottom), a blank row
    // between two; folded LIVE or DONE its heading alone and none of its rows.
    let mut lines = Vec::new();
    // The folded headings' lines, for the reveal below.
    let mut live_folded_line = None;
    let mut done_folded_line = None;
    let mut push_row = |lines: &mut Vec<Line>, row: usize| {
        let start = lines.len();
        lines.extend((0..rows[row].height).map(|line| Line::Row(row, line)));
        let below = under[row].as_ref().map_or(0, |under| under.lines.len());
        lines.extend((0..below).map(|line| Line::Detail(row, line)));
        hits.spans.push((entries[row].key(), start, lines.len()));
    };
    let live_rows: Vec<usize> = (0..entries.len())
        .filter(|row| entries[*row].part() == Part::Live)
        .collect();
    if !live_rows.is_empty() {
        let (count, of) = (live_rows.len(), live_of);
        if board.live_folded {
            live_folded_line = Some(lines.len());
            lines.push(Line::Folded(Part::Live, Counts::of(count, of)));
        } else {
            let counts = Counts::of(count, of);
            lines.extend([Line::Heading(Part::Live, counts), Line::Top(Part::Live)]);
            if live_columns.is_grid() {
                lines.extend([Line::Head(Part::Live), Line::Rule(Part::Live)]);
            }
            for row in live_rows.iter().take(LIVE_MOST) {
                push_row(&mut lines, *row);
            }
            let more = live_rows.get(LIVE_MOST..).unwrap_or_default().len();
            if more > 0 {
                lines.push(Line::More(more));
            }
            lines.push(Line::Bottom(Part::Live));
        }
    }
    for (index, _) in SECTIONS.iter().enumerate() {
        let part = Part::Section(index);
        let members: Vec<usize> = (0..entries.len())
            .filter(|row| entries[*row].part() == part)
            .collect();
        let waiting_of = if index == 0 { waiting_of } else { 0 };
        let of = spaced
            .iter()
            .filter(|task| section(task) == index && !scheduled(task))
            .count()
            + waiting_of;
        // An empty section stays hidden, but ACTION REQUIRED says so while every section
        // shows and nothing in view needs the user: its heading alone, no box.
        if members.is_empty() {
            let says_none =
                index == 0 && of == 0 && board.only_section.is_none() && chosen.is_none();
            if says_none {
                if !lines.is_empty() {
                    lines.push(Line::Blank);
                }
                lines.push(Line::Heading(part, Counts::of(0, 0)));
            }
            continue;
        }
        // How many of the rows shown are scheduled (counted apart), overdue and due today; a
        // done task's day is past.
        let mut counts = Counts::of(members.len(), of);
        for row in &members {
            match entries[*row] {
                Entry::Task(task) if scheduled(task) => {
                    counts.shown -= 1;
                    counts.scheduled += 1;
                }
                _ => {}
            }
            match entries[*row] {
                Entry::Task(task) if task.group != TaskGroup::Done => match task.due_level {
                    Some(DueLevel::Overdue) => counts.overdue += 1,
                    Some(DueLevel::Today) => counts.today += 1,
                    _ => {}
                },
                _ => {}
            }
        }
        if !lines.is_empty() {
            lines.push(Line::Blank);
        }
        if index == DONE_SECTION && !board.done_expanded {
            done_folded_line = Some(lines.len());
            lines.push(Line::Folded(part, counts));
            continue;
        }
        lines.push(Line::Heading(part, counts));
        lines.push(Line::Top(part));
        // Each box its own column names, as its cells differ.
        if columns.is_grid() {
            lines.push(Line::Head(part));
            lines.push(Line::Rule(part));
        }
        for row in members {
            push_row(&mut lines, row);
        }
        lines.push(Line::Bottom(part));
    }

    hits.live_folded = live_folded_line.is_some();
    hits.done_folded = done_folded_line.is_some();
    // A heading counts as focused only while it is there to hold the focus.
    let focused = board.shown_focused_heading(hits);
    let is_focused = |part: Part| match part {
        Part::Live => focused == Some(Fold::Live),
        Part::Section(index) => index == DONE_SECTION && focused == Some(Fold::Done),
    };
    let viewport = usize::from(body.height);
    hits.viewport = viewport;
    hits.max_scroll = lines.len().saturating_sub(viewport);
    let mut scroll = board.scroll.min(hits.max_scroll);
    if board.reveal_selected {
        let span = match focused {
            Some(Fold::Live) => live_folded_line.map(|line| (line, line + 1)),
            Some(Fold::Done) => done_folded_line.map(|line| (line, line + 1)),
            None => selected.and_then(|row| {
                let key = entries[row].key();
                hits.spans
                    .iter()
                    .find(|(target, _, _)| *target == key)
                    .map(|(_, start, end)| (*start, *end))
            }),
        };
        if let Some((start, end)) = span {
            if start < scroll {
                scroll = start;
            } else if end > scroll + viewport {
                scroll = end.saturating_sub(viewport).min(start);
            }
        }
    }
    hits.scroll = scroll;

    let frame = |part: Part| match part {
        Part::Live => style::frame(palette.yellow, palette),
        Part::Section(index) => section_frame(index, palette),
    };
    let heading_line =
        |y: u16| Rect::new(area.x.saturating_add(1), y, area.width.saturating_sub(1), 1);
    let mut row_extents: Vec<Option<(u16, u16)>> = vec![None; rows.len()];
    for (offset, line) in lines.iter().skip(scroll).take(viewport).enumerate() {
        let y = body.y.saturating_add(offset as u16);
        match *line {
            Line::Heading(part, counts) | Line::Folded(part, counts) => {
                let folded = matches!(line, Line::Folded(..));
                let mut arrow = None;
                // LIVE and DONE fold: a click anywhere on the heading's line does it.
                let target = match part {
                    Part::Live => Some(&mut hits.live_heading),
                    Part::Section(DONE_SECTION) => Some(&mut hits.done_heading),
                    Part::Section(_) => None,
                };
                if let Some(target) = target {
                    *target = heading_line(y);
                    // The keyboard's focus on it shows as a selected row does.
                    if folded && is_focused(part) {
                        buffer.set_style(*target, Style::default().bg(palette.selection_bg));
                    }
                    arrow = Some(if folded { "▶" } else { "▼" });
                }
                let (title, role) = heading_of(part, palette);
                // Only COMPLETED says how far back it reaches; an empty ACTION REQUIRED says
                // nothing needs the user.
                let reach = match part {
                    Part::Section(DONE_SECTION) => Some("last 7 days"),
                    Part::Section(0) if counts.of == 0 && counts.scheduled == 0 => {
                        Some("nothing needs you")
                    }
                    _ => None,
                };
                put_heading(
                    buffer,
                    (columns.left, y, columns.right.saturating_add(1)),
                    (title, role),
                    arrow,
                    (counts, reach),
                    &words,
                    palette,
                );
            }
            Line::Top(part) => table(part).put_rule(buffer, y, &TOP_RULE, frame(part)),
            Line::Head(part) => table(part).put_head(buffer, y, frame(part), palette),
            Line::Rule(part) => table(part).put_rule(buffer, y, &HEAD_RULE, frame(part)),
            Line::Bottom(part) => table(part).put_rule(buffer, y, &BOTTOM_RULE, frame(part)),
            Line::Blank => {}
            Line::More(count) => {
                live_columns.put_band(buffer, y, Style::default(), frame(Part::Live));
                let x = live_columns.left.saturating_add(1);
                put_segment(
                    buffer,
                    x,
                    y,
                    live_columns.right,
                    &format!("  +{count} more"),
                    dim,
                );
            }
            Line::Row(row, line) => {
                let band = (selected == Some(row)).then_some(palette.selection_bg);
                let part = entries[row].part();
                rows[row].put_line(buffer, table(part), y, line, band, frame(part), marks);
                extend(&mut row_extents[row], y);
            }
            Line::Detail(row, line) => {
                let Some(under) = under[row].as_ref() else {
                    continue;
                };
                let band = if under.banded {
                    Style::default().bg(palette.selection_bg)
                } else {
                    Style::default()
                };
                let part = entries[row].part();
                let table = table(part);
                table.put_band(buffer, y, band, frame(part));
                let x = table.left.saturating_add(1);
                match under.lines.get(line) {
                    Some(UnderLine::Text(spans)) => {
                        let mut x = x;
                        for (text, style) in spans {
                            x = put_segment(buffer, x, y, table.right, text, band.patch(*style));
                        }
                    }
                    // Each key a target a click does it by, as the footer's are.
                    Some(UnderLine::Keys(indent, keys)) => {
                        let x = x.saturating_add(*indent);
                        let targets = &mut hits.footer_keys;
                        put_hints(buffer, x, y, table.right, keys, palette, targets);
                    }
                    None => {}
                }
                extend(&mut row_extents[row], y);
            }
        }
    }
    hits.spins |= row_extents
        .iter()
        .zip(&spinning)
        .any(|(extent, spins)| extent.is_some() && *spins);
    hits.rows = row_extents
        .iter()
        .enumerate()
        .filter_map(|(row, extent)| {
            let (top, bottom) = (*extent)?;
            Some((
                Rect::new(
                    columns.left,
                    top,
                    columns.right.saturating_add(1).saturating_sub(columns.left),
                    bottom.saturating_add(1).saturating_sub(top),
                ),
                entries[row].key(),
            ))
        })
        .collect();
}

/// Whether `task` is in the space the board shows and, with an agent shown alone (`chosen`),
/// that agent's.
pub(super) fn in_view(board: &TaskBoard, chosen: Option<&BoardAgent>, task: &Task) -> bool {
    board
        .space
        .as_ref()
        .is_none_or(|shown| task.space == *shown)
        && chosen.is_none_or(|chosen| {
            task.agent
                .as_ref()
                .is_some_and(|named| super::tags_agent(named, &task.space, chosen))
        })
}

/// A NEEDS YOU task whose agent is a waiting tab shows once, as the tab, while that tab's row
/// is among `tabs`.
fn named_waiting(task: &Task, tabs: &[&BoardAgent]) -> bool {
    section(task) == 0
        && task.agent.as_ref().is_some_and(|named| {
            tabs.iter()
                .any(|agent| super::tags_agent(named, &task.space, agent))
        })
}

/// The tabs of `spaced` waiting on the user that the filter's `words` keep: each a row at the
/// top of NEEDS YOU, in place of a task that names it. The rows and the counts both take
/// them from here, so a task never hides without its tab showing.
fn waiting_tabs<'a>(spaced: &[&'a BoardAgent], words: &[String]) -> Vec<&'a BoardAgent> {
    spaced
        .iter()
        .copied()
        .filter(|agent| matches!(agent.state, AgentState::Waiting(_)))
        .filter(|agent| super::filter::agent_matches(agent, None, words))
        .collect()
}

/// The tasks the Tasks view has rows for before its section button, which the header counts:
/// in the view, kept by the filter's `words`, and not shown as a waiting tab's row.
fn matched_tasks<'a>(
    board: &TaskBoard,
    list: &'a super::model::TaskList,
    agents: &[BoardAgent],
    words: &[String],
) -> Vec<&'a Task> {
    let chosen = super::shown_agent(board, agents);
    let spaced = super::spaced_agents(board, agents);
    let waiting = waiting_tabs(&spaced, words);
    let live = live_agents(list, &spaced, words);
    list.tasks
        .iter()
        .filter(|task| {
            in_view(board, chosen, task)
                && super::filter::task_matches(task, words)
                && !named_waiting(task, &waiting)
                && !in_live_row(task, &live)
        })
        .collect()
}

/// An IN PROGRESS row's cells for `agent` on `task`, whose `card` names one of `work`:
/// Working on is the card's title, else the task (its row's number before it), else its
/// `task`, its stage, its tab's own name or its folder; Stage the card's, else its pane
/// stage's word, in its phase colour; Step now the card's first undone step, else the task's
/// next action, else that stage;
/// Doing its `doing`, else its stage text, else its `task` when Working on names another.
fn live_text<'a>(
    agent: &'a BoardAgent,
    task: Option<&'a Task>,
    numbers: &RowNumbers,
    work: &'a [WorkItem],
    palette: &Palette,
) -> LiveText<'a> {
    let labels = &agent.labels;
    let card = task
        .and_then(|task| task.card.as_deref())
        .and_then(|slug| work.iter().find(|card| card.slug.as_deref() == Some(slug)));
    let title = card
        .map(|card| card.title.as_str())
        .filter(|title| !title.trim().is_empty());
    // Working on names the card or the task, so its `task` label shows nowhere else.
    let named = title.is_some() || task.is_some();
    let (number, working_on) = match (title, task) {
        (Some(title), _) => (None, title),
        (None, Some(task)) => (numbers.of_task(task), task.task.as_str()),
        (None, None) => (
            None,
            [
                &labels.task,
                &labels.stage,
                &labels.tab_name,
                &labels.folder,
            ]
            .into_iter()
            .find_map(Option::as_deref)
            .unwrap_or_default(),
        ),
    };
    let stage = card
        .and_then(|card| card_stage(card.stage))
        .or_else(|| {
            let stage = labels.stage.as_deref()?;
            let word = stage.split('·').next().unwrap_or_default().trim();
            (!word.is_empty()).then(|| (word, phase_word(stage)))
        })
        .map(|(word, phase)| (word, phase.map(|phase| stage_color(phase, palette))));
    let step = card
        .and_then(|card| card.steps.iter().find(|step| !step.done))
        .map(|step| step.text.as_str())
        .or_else(|| task?.action.as_deref())
        .filter(|step| !step.trim().is_empty())
        .or(stage.map(|(word, _)| word));
    LiveText {
        number,
        working_on,
        stage,
        step,
        doing: labels
            .doing
            .as_deref()
            .or(labels.stage.as_deref())
            .or(labels.task.as_deref().filter(|_| named)),
    }
}

/// A card's stage as its file writes it, and its phase; none unknown.
fn card_stage(stage: WorkStage) -> Option<(&'static str, Option<&'static str>)> {
    match stage {
        WorkStage::Plan => Some(("plan", Some("Plan"))),
        WorkStage::Build => Some(("build", Some("Build"))),
        WorkStage::Review => Some(("review", Some("Review"))),
        WorkStage::Check => Some(("verify", Some("Verify"))),
        WorkStage::Shipped => Some(("shipped", Some("Ship"))),
        WorkStage::Unknown => None,
    }
}

/// The working tabs of `spaced` the filter's `words` keep, each an IN PROGRESS row that shows
/// the task its worker is on.
fn live_agents<'a>(
    list: &super::model::TaskList,
    spaced: &[&'a BoardAgent],
    words: &[String],
) -> Vec<&'a BoardAgent> {
    spaced
        .iter()
        .copied()
        .filter(|agent| {
            let task = super::worker_task(agent, list).map(|task| task.task.as_str());
            matches!(agent.state, AgentState::Working(_))
                && super::filter::agent_matches(agent, task, words)
        })
        .collect()
}

/// A task in progress that one of the working tabs `live` is on: that tab's IN PROGRESS row
/// shows it, so it has no row of its own.
fn in_live_row(task: &Task, live: &[&BoardAgent]) -> bool {
    task.section() == TaskSection::InProgress
        && task.agent.as_ref().is_some_and(|named| {
            live.iter().any(|agent| {
                matches!(agent.state, AgentState::Working(_))
                    && super::tags_agent(named, &task.space, agent)
            })
        })
}

/// `tasks` as their rows show, top down: the header's section button's section alone, each
/// section in turn, and in it the header's sort order (done tasks by the day they were done,
/// a task without its date as done `today`, the day the list was read).
fn in_shown_order<'a>(board: &TaskBoard, tasks: Vec<&'a Task>, today: &str) -> Vec<&'a Task> {
    let mut tasks: Vec<&Task> = tasks
        .into_iter()
        .filter(|task| {
            board.only_section.is_none_or(|only| {
                only != TaskSection::InProgress && section(task) == section_index(only)
            })
        })
        .collect();
    // A scheduled task ends its section under every order but the file's own.
    let later = |task: &Task| board.sort != TaskSort::File && scheduled(task);
    tasks.sort_by(|a, b| {
        section(a)
            .cmp(&section(b))
            .then_with(|| later(a).cmp(&later(b)))
            .then_with(|| board.sort.order(a, b, today))
    });
    tasks
}

/// The Tasks view's task rows' numbers, whichever view is open: the Work panel numbers a
/// task as its row does.
pub(super) fn shown_numbers(
    board: &TaskBoard,
    list: &super::model::TaskList,
    agents: &[BoardAgent],
) -> RowNumbers {
    let words = super::filter::words(&board.filter);
    let today = list.generated.get(..10).unwrap_or("");
    let shown = in_shown_order(board, matched_tasks(board, list, agents, &words), today);
    RowNumbers::of(board, &shown)
}

/// Each task row's number, its place among the rows top down from 1, worked out once a frame.
/// A done task has one only while DONE is open: folded, it has no row. Numbers are for
/// reading only: keys and actions name a task by its text.
pub(super) struct RowNumbers(HashMap<*const Task, usize>);

impl RowNumbers {
    /// The numbers of `shown`, the task rows in the order they show.
    fn of(board: &TaskBoard, shown: &[&Task]) -> Self {
        Self(
            shown
                .iter()
                .filter(|task| board.done_expanded || task.group != TaskGroup::Done)
                .enumerate()
                .map(|(index, task)| (std::ptr::from_ref::<Task>(task), index + 1))
                .collect(),
        )
    }

    /// `task`'s row number; none when it has no row.
    pub(super) fn of_task(&self, task: &Task) -> Option<usize> {
        self.0.get(&std::ptr::from_ref(task)).copied()
    }
}

/// How many LIVE rows show before the rest fold into a `+N more` row.
const LIVE_MOST: usize = 6;

/// An agent's row's key to its tab.
const GO_TO_AGENT: Hint = ("Enter", "go to agent");

/// The keys that act on a task, which a tab's row does not take.
const TASK_ONLY_KEYS: &[Hint] = &[
    ("o", ""),
    ("d", ""),
    ("x", ""),
    ("m", ""),
    ("J/K", ""),
    ("n", ""),
];

/// A part of the Tasks view with a heading over a box: LIVE, or a section by its index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Live,
    Section(usize),
}

/// A row of the Tasks view.
#[derive(Clone, Copy)]
enum Entry<'a> {
    Task(&'a Task),
    /// A working tab in IN PROGRESS: a worker right after its lead.
    Live(&'a BoardAgent),
    /// A tab waiting on the user, at the top of NEEDS YOU.
    Waiting(&'a BoardAgent),
}

impl Entry<'_> {
    fn key(&self) -> RowKey {
        match self {
            Self::Task(task) => RowKey::Task(TaskKey::of(task)),
            Self::Live(agent) | Self::Waiting(agent) => RowKey::Agent(agent.pane_id.clone()),
        }
    }

    fn part(&self) -> Part {
        match self {
            Self::Task(task) => Part::Section(section(task)),
            Self::Live(..) => Part::Live,
            Self::Waiting(_) => Part::Section(0),
        }
    }
}

/// IN PROGRESS's rows in the snapshot's order: each working tab that works for no one here,
/// and each working lead, then its working workers right after it; a lead that does not
/// work has no row, and its working workers are top rows. `live` are the working tabs shown, `spaced`
/// every agent of the space shown.
fn live_tree<'a>(live: &[&'a BoardAgent], spaced: &[&'a BoardAgent]) -> Vec<Entry<'a>> {
    let lead_of = |agent: &BoardAgent| super::lead_here(agent, spaced);
    let shown = |agent: &BoardAgent| live.iter().any(|live| live.pane_id == agent.pane_id);
    let mut rows = Vec::new();
    for top in spaced
        .iter()
        .copied()
        .filter(|agent| lead_of(agent).is_none())
    {
        let workers: Vec<&BoardAgent> = live
            .iter()
            .copied()
            .filter(|worker| lead_of(worker).is_some_and(|lead| lead.pane_id == top.pane_id))
            .collect();
        if !shown(top) {
            rows.extend(workers.into_iter().map(Entry::Live));
            continue;
        }
        rows.push(Entry::Live(top));
        rows.extend(workers.into_iter().map(Entry::Live));
    }
    rows
}

/// An open task row's block: its card, the agent on it and what it does, its space's lead
/// (when another agent works for it, and it is not that agent), its card's progress, the cells
/// its row cut short, its file and its card's branch; each only when it has one. `today` as for
/// Due.
fn task_fields(
    task: &Task,
    table: &Table,
    (list, agents): (&super::model::TaskList, &[BoardAgent]),
    (home_dir, today): (Option<&str>, &str),
) -> Vec<(&'static str, String)> {
    let card = super::task_card(task, list).map(card_facts);
    let agent = task.agent.as_ref().map(|named| {
        let parts = std::iter::once(named.name.clone())
            .chain(named.minutes.map(duration))
            .chain(super::doing_of(named, &task.space, agents).map(str::to_owned));
        ("Agent", parts.collect::<Vec<_>>().join(" · "))
    });
    let lead = super::space_lead(&task.space, agents)
        .filter(|lead| {
            agents
                .iter()
                .any(|agent| agent.lead.as_deref() == Some(lead.pane_id.as_str()))
        })
        .filter(|lead| {
            task.agent
                .as_ref()
                .is_none_or(|named| named.pane != lead.pane_id && named.name != lead.tab)
        })
        .map(|lead| ("Lead", lead.tab.clone()));
    let (card_line, progress, branch) = card.map_or((None, None, None), |card| {
        (Some(("Card", card.card)), card.progress, card.branch)
    });
    card_line
        .into_iter()
        .chain(agent)
        .chain(lead)
        .chain(progress.map(|progress| ("Progress", progress)))
        .chain(cut_fields(task, shown_section(task), table, today))
        .chain(file_field(task, home_dir))
        .chain(branch.map(|branch| ("Branch", branch)))
        .collect()
}

/// An open agent's row's block: the lead it works for, its task (numbered as its row is) and
/// its card while its row does not show them, then its card's progress and branch. A row that
/// `shows_working_on` shows its card's title, stage and step, else its task.
fn agent_fields(
    agent: &BoardAgent,
    list: &super::model::TaskList,
    numbers: &RowNumbers,
    agents: &[BoardAgent],
    shows_working_on: bool,
) -> Vec<(&'static str, String)> {
    let lead = agent
        .lead
        .as_deref()
        .and_then(|pane| agents.iter().find(|lead| lead.pane_id == pane))
        .map(|lead| ("Lead", lead.tab.clone()));
    let card = super::agent_card(agent, list);
    let shows_card = shows_working_on && card.is_some_and(|card| !card.title.trim().is_empty());
    let task = task_field(agent, list, numbers).filter(|_| !(shows_working_on && !shows_card));
    let card = card.map(card_facts);
    let (card_line, progress, branch) = card.map_or((None, None, None), |card| {
        (
            Some(("Card", card.card)).filter(|_| !shows_card),
            card.progress,
            card.branch,
        )
    });
    lead.into_iter()
        .chain(task)
        .chain(card_line)
        .chain(progress.map(|progress| ("Progress", progress)))
        .chain(branch.map(|branch| ("Branch", branch)))
        .collect()
}

/// What an open row's block says of a Work card.
struct CardFacts {
    /// Its kind, title, stage and the step it is on: `◆ Feature · Board polish · build · step
    /// 2 of 3`.
    card: String,
    /// A bar of its done steps, how many of all, and the next: none without steps.
    progress: Option<String>,
    /// Its branch, commits and the day of the last one: none without a branch.
    branch: Option<String>,
}

/// How many cells a card's progress bar takes.
const PROGRESS_WIDTH: usize = 20;

fn card_facts(card: &WorkItem) -> CardFacts {
    let total = card.steps.len();
    let done = card.steps.iter().filter(|step| step.done).count();
    let next = card.steps.iter().position(|step| !step.done);
    let (mark, _) = super::work::kind_mark(card.kind);
    let mut parts = vec![
        format!("{mark} {}", super::work::kind_title(card.kind)),
        clean(super::work::shown_title(&card.title)),
    ];
    parts.extend(card_stage(card.stage).map(|(word, _)| word.to_owned()));
    parts.extend(next.map(|index| format!("step {} of {total}", index + 1)));
    let progress = (total > 0).then(|| {
        let filled = done * PROGRESS_WIDTH / total;
        let bar = format!(
            "{}{}",
            "━".repeat(filled),
            "─".repeat(PROGRESS_WIDTH - filled)
        );
        let next = next.map_or_else(String::new, |index| {
            format!(" · next: {}", clean(&card.steps[index].text))
        });
        format!("{bar} {done}/{total} steps{next}")
    });
    let branch = card
        .branch
        .as_deref()
        .filter(|branch| !branch.trim().is_empty())
        .map(|_| super::work::branch_text(card));
    CardFacts {
        card: parts.join(" · "),
        progress,
        branch,
    }
}

/// A LIVE tab's task, numbered as its row is, as a block field.
fn task_field(
    agent: &BoardAgent,
    list: &super::model::TaskList,
    numbers: &RowNumbers,
) -> Option<(&'static str, String)> {
    super::worker_task(agent, list).map(|task| {
        (
            "Task",
            match numbers.of_task(task) {
                Some(number) => format!("#{number} {}", task.task),
                None => task.task.clone(),
            },
        )
    })
}

/// A selected agent's row's keys: its tab, its task's card when it has one, and its task when
/// that has a row of its own.
fn agent_keys(agent: &BoardAgent, list: &super::model::TaskList) -> Vec<Hint> {
    let mut keys = vec![GO_TO_AGENT];
    if super::agent_card(agent, list).is_some() {
        keys.push(SHOW_CARD);
    }
    // A task in progress shows in the agent's own row, so it has no row to show.
    let task = super::worker_task(agent, list);
    if task.is_some_and(|task| task.section() != TaskSection::InProgress) {
        keys.push(SHOW_TASK);
    }
    keys
}

/// A row's keys to the card its task is on, and to an agent's task.
pub(super) const SHOW_CARD: Hint = ("w", "show its card");
const SHOW_TASK: Hint = ("t", "show its task");

/// A part's heading title and its colour: LIVE yellow as working, a section its own.
fn heading_of(part: Part, palette: &Palette) -> (&'static str, Color) {
    match part {
        Part::Live => (
            IN_PROGRESS,
            style::section_role(TaskSection::InProgress, palette),
        ),
        Part::Section(index) => (
            SECTIONS[index].0,
            style::section_role(SECTIONS[index].1, palette),
        ),
    }
}

fn extend(extent: &mut Option<(u16, u16)>, y: u16) {
    *extent = Some(match *extent {
        Some((top, bottom)) => (top.min(y), bottom.max(y)),
        None => (y, y),
    });
}

/// The lines drawn under a task's row.
#[derive(Clone)]
struct Under {
    lines: Vec<UnderLine>,
    /// In the selection's band, as the selected row is.
    banded: bool,
}

/// A line under a row, drawn from inside the box's left border.
#[derive(Clone)]
enum UnderLine {
    /// Text in pieces.
    Text(Vec<(String, Style)>),
    /// Key hints this far in, each a click target as the footer's are.
    Keys(u16, Vec<Hint>),
}

/// `lines` of text in `style`, then `keys` as blocks: after the last line when they fit
/// there, else on a line of their own.
fn text_then_keys(
    lines: Vec<String>,
    style: Style,
    keys: &[Hint],
    width: u16,
    palette: &Palette,
) -> Vec<Vec<(String, Style)>> {
    let mut drawn: Vec<Vec<(String, Style)>> =
        lines.into_iter().map(|line| vec![(line, style)]).collect();
    if keys.is_empty() {
        return drawn;
    }
    let keys_width = hints_width(keys);
    let after = drawn.last().map(|line| {
        line.iter()
            .map(|(text, _)| display_width(text))
            .sum::<u16>()
    });
    match after {
        Some(used) if used + spacing::HINT_GAP + keys_width <= width => {
            let last = drawn.last_mut().expect("a line");
            last.push((spacing::gap(spacing::HINT_GAP), Style::default()));
            last.extend(hint_spans(keys, palette));
        }
        _ => drawn.push(hint_spans(keys, palette)),
    }
    drawn
}

/// A selected row's detail block, each line in pieces from inside the box's left border:
/// every field's label dim in one column `indent` in, as wide as the longest label and two
/// more (or reaching `value_x`, when that is further in), its value beside it and wrapped
/// under its own start; then a blank line and `keys` as the footer draws them.
fn detail_block(
    fields: &[(&str, String)],
    keys: &[Hint],
    (indent, value_x): (u16, u16),
    width: u16,
    palette: &Palette,
) -> Vec<UnderLine> {
    let dim = Style::default().fg(palette.overlay0);
    let text = Style::default().fg(palette.text);
    let labels = fields
        .iter()
        .map(|(label, _)| display_width(label))
        .max()
        .unwrap_or(0);
    let value_x = (indent + labels + 2).max(value_x);
    let value_width = width.saturating_sub(value_x + 1).max(1);
    let lead = spacing::gap(indent);
    let mut lines = Vec::new();
    for (label, value) in fields {
        for (index, line) in wrap(value, value_width).into_iter().enumerate() {
            let label = if index == 0 { *label } else { "" };
            let label = format!("{label:<width$}", width = usize::from(value_x - indent));
            let pieces = vec![(lead.clone(), Style::default()), (label, dim), (line, text)];
            lines.push(UnderLine::Text(pieces));
        }
    }
    if keys.is_empty() {
        return lines;
    }
    // A blank line parts the keys from the fields over them, when there are any.
    if !lines.is_empty() {
        lines.push(UnderLine::Text(Vec::new()));
    }
    // As many keys to a line as fit, each key kept with its words.
    let room = width.saturating_sub(indent + 1);
    let mut row: Vec<Hint> = Vec::new();
    let mut rows = Vec::new();
    for key in keys {
        if !row.is_empty() && hints_width(&[row.as_slice(), &[*key]].concat()) > room {
            rows.push(std::mem::take(&mut row));
        }
        row.push(*key);
    }
    rows.push(row);
    lines.extend(rows.into_iter().map(|keys| UnderLine::Keys(indent, keys)));
    lines
}

/// `lines` drawn from inside the box's left border, `indent` in: the first after `↳ `, the
/// rest under its text.
fn led(lines: Vec<Vec<(String, Style)>>, indent: u16, dim: Style) -> Vec<UnderLine> {
    lines
        .into_iter()
        .enumerate()
        .map(|(index, pieces)| {
            let mut line = if index == 0 {
                vec![
                    (spacing::gap(indent.saturating_sub(2)), Style::default()),
                    ("↳ ".to_owned(), dim),
                ]
            } else {
                vec![(spacing::gap(indent), Style::default())]
            };
            line.extend(pieces);
            UnderLine::Text(line)
        })
        .collect()
}

/// How many notes files the read found a problem in, each problem naming its file before
/// its `: `: `1 notes file has a problem`, `2 notes files have a problem`.
fn problem_count(problems: &[String]) -> String {
    let mut files: Vec<&str> = problems
        .iter()
        .map(|problem| {
            problem
                .split_once(": ")
                .map_or(problem.as_str(), |(file, _)| file)
        })
        .collect();
    files.sort_unstable();
    files.dedup();
    match files.len() {
        1 => "1 notes file has a problem".to_owned(),
        count => format!("{count} notes files have a problem"),
    }
}

/// `text` on one line as written when it fits, its double spaces kept; else wrapped.
fn wrap_spaced(text: &str, width: u16) -> Vec<String> {
    let text = clean(text);
    if display_width(&text) <= width {
        vec![text]
    } else {
        wrap(&text, width)
    }
}

/// What a question or typed line on a row says, and its keys; a typed line ends in its
/// cursor.
fn prompt_text(prompt: &RowPrompt) -> (String, Vec<Hint>) {
    match prompt {
        RowPrompt::Delete => (
            "Delete this task?".to_owned(),
            vec![("y", "delete"), ("n", "keep")],
        ),
        RowPrompt::Move => (
            "Move to:".to_owned(),
            vec![("1", "you"), ("2", "me"), ("3", "other…")],
        ),
        RowPrompt::MoveTo(text) => (
            format!("Move to: {text}▏"),
            vec![("Enter", "move"), ("Esc", "cancel")],
        ),
        RowPrompt::New(text) => (
            format!("New task: {text}▏"),
            vec![("Enter", "add"), ("Esc", "cancel")],
        ),
    }
}

/// A section's box and its lines: its colour, quieted.
fn section_frame(index: usize, palette: &Palette) -> Style {
    style::frame(style::section_role(SECTIONS[index].1, palette), palette)
}

/// A heading from `x`, ending before `end`: its title in its colour, then how many rows it
/// shows (of how many, while the filter narrows it) in grey one space after it, then a
/// folding part's arrow, and at the far right in grey what else it counts, each only when
/// some are: `ACTION REQUIRED 3  ...  1 overdue · 1 due today · +1 scheduled`. When the
/// line is too narrow for all of it, the grey detail drops first.
#[allow(clippy::too_many_arguments)] // Its place, its text, arrow and counts, and the styles' inputs.
fn put_heading(
    buffer: &mut Buffer,
    (x, y, end): (u16, u16, u16),
    (title, role): (&str, Color),
    arrow: Option<&str>,
    (counts, reach): (Counts, Option<&str>),
    words: &[String],
    palette: &Palette,
) {
    let grey = Style::default().fg(palette.overlay0);
    let bold = Style::default().fg(role).add_modifier(Modifier::BOLD);
    let x = put_segment(buffer, x, y, end, &format!(" {title}"), bold);
    let x = put_segment(buffer, x, y, end, " ", Style::default());
    let count = if words.is_empty() {
        counts.shown.to_string()
    } else {
        format!("{} of {}", counts.shown, counts.of)
    };
    let quiet = Style::default().fg(palette.overlay1);
    let mut x = put_segment(buffer, x, y, end, &count, quiet);
    if let Some(arrow) = arrow {
        x = put_segment(buffer, x, y, end, &format!(" {arrow}"), grey);
    }
    let mut detail = Vec::new();
    if counts.overdue > 0 {
        detail.push(format!("{} overdue", counts.overdue));
    }
    if counts.today > 0 {
        detail.push(format!("{} due today", counts.today));
    }
    if counts.scheduled > 0 {
        detail.push(format!("+{} scheduled", counts.scheduled));
    }
    detail.extend(reach.map(str::to_string));
    let detail = detail.join(" · ");
    // Two columns clear of the title, else it drops.
    let width = display_width(&detail);
    if width > 0 && x.saturating_add(2).saturating_add(width) <= end {
        put_segment(buffer, end - width, y, end, &detail, grey);
    }
}

/// What a heading counts: the rows it shows and has before the filter, and how many of those
/// shown are overdue and due today.
#[derive(Debug, Clone, Copy)]
struct Counts {
    shown: usize,
    of: usize,
    /// Rows shown that start after today, left out of `shown` and `of`.
    scheduled: usize,
    overdue: usize,
    today: usize,
}

impl Counts {
    fn of(shown: usize, of: usize) -> Self {
        Self {
            shown,
            of,
            scheduled: 0,
            overdue: 0,
            today: 0,
        }
    }
}

/// One line of LIVE and the sections.
enum Line {
    /// A part's heading, above its box, with its counts.
    Heading(Part, Counts),
    /// A folded part's heading, with no box, with its counts.
    Folded(Part, Counts),
    /// A box's top border.
    Top(Part),
    /// A box's column-name row.
    Head(Part),
    /// The rule under a box's column names.
    Rule(Part),
    /// A box's bottom border.
    Bottom(Part),
    /// Between two parts.
    Blank,
    /// LIVE's last line when it has more tabs than it shows: how many more.
    More(usize),
    /// A row's line: the row's index, then the line within it.
    Row(usize, usize),
    /// A line under the selected row, or under a task a question or action is open on.
    Detail(usize, usize),
}

/// The smallest main area the board draws in; below it, one line says so.
const MIN_WIDTH: u16 = 40;
const MIN_HEIGHT: u16 = 10;

/// One line saying the size the board needs, and `×` at the far right.
fn render_too_small(buffer: &mut Buffer, area: Rect, palette: &Palette, hits: &mut TaskBoardHits) {
    if area.width < 5 || area.height == 0 {
        return;
    }
    let close_x = area.right().saturating_sub(CLOSE_WIDTH + 1);
    put_fitted(
        buffer,
        area.x.saturating_add(1),
        area.y,
        close_x.saturating_sub(1),
        &format!("Window too small for the board: {MIN_WIDTH} x {MIN_HEIGHT} needed"),
        Style::default().fg(palette.overlay1),
    );
    put_close(buffer, close_x, area.y, palette, hits);
}

/// The close button's width: `×` with a space each side.
pub(super) const CLOSE_WIDTH: u16 = 3;

/// The board's close button, ` × `, which closes it.
fn put_close(buffer: &mut Buffer, x: u16, y: u16, palette: &Palette, hits: &mut TaskBoardHits) {
    put_segment(
        buffer,
        x,
        y,
        x.saturating_add(CLOSE_WIDTH),
        " × ",
        style::key(palette),
    );
    hits.close = Rect::new(x, y, CLOSE_WIDTH, 1);
}

/// The search box's width at rest and while it narrows the board; `⌕` alone when the header
/// has no room for it.
const FILTER_BOX_WIDTH: u16 = 23;
const FILTER_BOX_COMPACT: &str = " ⌕ ";

/// A thin `│` between two of the header's groups, `SEP_PAD` after the group that ends at `x`.
fn put_separator(buffer: &mut Buffer, x: u16, y: u16, end: u16, style: Style) {
    put_segment(
        buffer,
        x.saturating_add(spacing::SEP_PAD),
        y,
        end,
        "│",
        style,
    );
}

/// The header, on the board's own ground: from the left the view tabs; then the space picker
/// (and, while one space with several agents shows, `›` and the agent picker naming the agent
/// shown alone, or none) and the search box, those that narrow the list, and in Tasks the
/// sort button naming the rows' order; then the category buttons; at the right, when the list
/// was read, its time, and the close button. A `│` parts the groups; each gap is the spacing
/// system's.
fn render_header(
    buffer: &mut Buffer,
    area: Rect,
    board: &TaskBoard,
    agent_button: AgentButton,
    palette: &Palette,
    hits: &mut TaskBoardHits,
) {
    let y = area.y;
    let dim = Style::default().fg(palette.overlay0);
    let close_x = area.right().saturating_sub(spacing::MARGIN + CLOSE_WIDTH);
    put_close(buffer, close_x, y, palette, hits);

    // The space button names the space the board shows, or all of them, as the sort button
    // names its order; `▾` opens its menu. Narrowed, a ` × ` beside it shows every space again.
    let space_label = board
        .space
        .as_ref()
        .map_or_else(|| "All".to_owned(), |space| clean(space));
    let full_button = format!(" Space: {space_label} ▾ ");
    let compact_button = match &board.space {
        Some(_) => full_button.clone(),
        None => " ▾ ".to_owned(),
    };
    let agent_label = match agent_button {
        AgentButton::Hidden => None,
        AgentButton::All => Some(" All agents ▾ ".to_owned()),
        AgentButton::One(agent) => Some(format!(" {} {} ▾ ", agent.mark.0, clean(&agent.tab))),
    };
    let back_width = if board.space.is_some() {
        spacing::HEADER_GAP + CLOSE_WIDTH
    } else {
        0
    } + agent_label.as_deref().map_or(0, |label| {
        2 * spacing::HEADER_GAP + display_width(AGENT_STEP) + display_width(label)
    });
    let tabs_width = display_width(TASKS_TAB) + spacing::HEADER_GAP + display_width(WORK_TAB);
    let sort_label = (board.view == BoardView::Tasks).then(|| sort_button(board.sort));
    let sort_room = sort_label
        .as_deref()
        .map_or(0, |label| spacing::HEADER_GAP + display_width(label));

    let time = board
        .store
        .list()
        .and_then(|list| clock_time(&list.generated))
        .map(|time| format!("Updated {time}"));
    let box_room = |compact: bool| {
        if compact && !board.filter_editing && board.filter.trim().is_empty() {
            display_width(FILTER_BOX_COMPACT)
        } else {
            FILTER_BOX_WIDTH
        }
    };
    // The header's width with these parts: the margins, the tabs, the space picker, the search
    // box and the sort button or not, the time or not, and the close button.
    let width_of = |(button, sort): (&str, bool), with_time, compact| {
        let mut used = 2 * spacing::MARGIN
            + tabs_width
            + spacing::SEP_WIDTH
            + display_width(button)
            + back_width
            + spacing::HEADER_GAP
            + box_room(compact)
            + if sort { sort_room } else { 0 }
            + spacing::GROUP_GAP
            + CLOSE_WIDTH;
        if let (true, Some(time)) = (with_time, &time) {
            used += display_width(time) + TIME_GAP;
        }
        used
    };
    // As the width drops: no time, then the sort button, then the search box's word, then
    // `Space: All` in the button. The tabs always stay.
    let ((button, sort_shown), with_time, compact_box) = [
        ((&full_button, true), true, false),
        ((&full_button, true), false, false),
        ((&full_button, false), false, false),
        ((&full_button, false), false, true),
    ]
    .into_iter()
    .find(|((button, sort), with_time, compact)| {
        width_of((button, *sort), *with_time, *compact) <= area.width
    })
    .unwrap_or(((&compact_button, false), false, true));

    // The right side, from the close button in: the time.
    let mut left_end = close_x.saturating_sub(spacing::GROUP_GAP);
    if let (true, Some(time)) = (with_time, &time) {
        let time_end = close_x.saturating_sub(TIME_GAP);
        let time_x = time_end.saturating_sub(display_width(time));
        put_segment(buffer, time_x, y, time_end, time, dim);
        left_end = time_x.saturating_sub(spacing::GROUP_GAP);
    }

    // The left side: the tabs, then the space picker, the search box and the sort button.
    let tasks_x = area.x.saturating_add(spacing::MARGIN);
    let on = board.view == BoardView::Tasks;
    let mut x = put_segment(
        buffer,
        tasks_x,
        y,
        left_end,
        TASKS_TAB,
        style::tab(palette, on),
    );
    hits.tasks_tab = Rect::new(tasks_x, y, x.saturating_sub(tasks_x), 1);
    let work_x = x.saturating_add(spacing::HEADER_GAP);
    x = put_segment(
        buffer,
        work_x,
        y,
        left_end,
        WORK_TAB,
        style::tab(palette, !on),
    );
    hits.work_tab = Rect::new(work_x, y, x.saturating_sub(work_x), 1);
    put_separator(buffer, x, y, left_end, dim);
    let start = x.saturating_add(spacing::SEP_WIDTH);
    x = put_segment(buffer, start, y, left_end, button, style::button(palette));
    hits.space_button = Rect::new(start, y, x.saturating_sub(start), 1);
    if board.space.is_some() {
        let back = x.saturating_add(spacing::HEADER_GAP);
        x = put_segment(buffer, back, y, left_end, " × ", style::key(palette));
        hits.back_to_all = Rect::new(back, y, x.saturating_sub(back), 1);
    }
    // Space, then agent: the path narrows left to right.
    if let Some(label) = &agent_label {
        let step = x.saturating_add(spacing::HEADER_GAP);
        let button_x = put_segment(buffer, step, y, left_end, AGENT_STEP, dim)
            .saturating_add(spacing::HEADER_GAP);
        // A picker as the space's is: chosen while it names one agent, else a grey button.
        let look = if matches!(agent_button, AgentButton::One(_)) {
            style::chosen(palette)
        } else {
            style::button(palette).fg(palette.overlay1)
        };
        x = put_segment(buffer, button_x, y, left_end, label, look);
        hits.agent_button = Rect::new(button_x, y, x.saturating_sub(button_x), 1);
        hits.spins |= matches!(
            agent_button,
            AgentButton::One(agent) if matches!(agent.state, AgentState::Working(_))
        );
    }
    // One group with the space picker: one column apart, as the tabs are.
    let box_x = x.saturating_add(spacing::HEADER_GAP);
    let box_width = box_room(compact_box).min(left_end.saturating_sub(box_x));
    put_filter_box(
        buffer,
        Rect::new(box_x, y, box_width, 1),
        compact_box,
        board,
        palette,
        hits,
    );
    x = box_x.saturating_add(box_width);
    if let (true, Some(label)) = (sort_shown, &sort_label) {
        let sort_x = x.saturating_add(spacing::HEADER_GAP);
        x = put_segment(buffer, sort_x, y, left_end, label, style::button(palette));
        hits.sort_button = Rect::new(sort_x, y, x.saturating_sub(sort_x), 1);
    }
}

/// The sort button's text: `Sort:`, the order's word and the menu's `▾`, the shape of the
/// space button beside it.
fn sort_button(sort: TaskSort) -> String {
    format!(" Sort: {} ▾ ", sort.word())
}

/// The header's agent button: none (every space, or a space with one agent or none), every
/// agent, or the one agent the board shows alone.
#[derive(Clone, Copy)]
enum AgentButton<'a> {
    Hidden,
    All,
    One(&'a BoardAgent),
}

/// Between the space button and the agent button.
const AGENT_STEP: &str = "›";

/// The line under the header while one agent shows alone: whose rows these are, and how to
/// show every agent again, over the blank row the board keeps there.
fn render_agent_note(
    buffer: &mut Buffer,
    area: Rect,
    view: BoardView,
    agent: &BoardAgent,
    palette: &Palette,
) {
    let what = match view {
        BoardView::Tasks => "tasks",
        BoardView::Work => "cards",
    };
    let dim = Style::default().fg(palette.overlay0);
    let y = area.y.saturating_add(1);
    let end = area.right().saturating_sub(spacing::MARGIN);
    let x = area.x.saturating_add(2 * spacing::MARGIN);
    let x = put_segment(buffer, x, y, end, "Showing ", dim);
    let x = put_segment(
        buffer,
        x,
        y,
        end,
        &clean(&agent.tab),
        Style::default().fg(palette.text),
    );
    put_segment(
        buffer,
        x,
        y,
        end,
        &format!("'s {what} · Esc shows all agents"),
        dim,
    );
}

/// The agent button's menu, while open: `All agents`, then the space's agents as a tree,
/// each lead with its mark and its workers under it, a dim connector, their marks and what
/// they are doing; under the button and over the board, the agent shown in the accent.
fn render_agent_menu(
    buffer: &mut Buffer,
    area: Rect,
    board: &TaskBoard,
    agents: &[BoardAgent],
    palette: &Palette,
    hits: &mut TaskBoardHits,
) {
    if !board.agent_menu || hits.agent_button.is_empty() {
        return;
    }
    let Some(space) = board.space.as_deref() else {
        return;
    };
    let fill = style::band(palette).fg(palette.text);
    let spaced: Vec<&BoardAgent> = agents.iter().filter(|agent| agent.space == space).collect();
    hits.spins |= spaced
        .iter()
        .any(|agent| matches!(agent.state, AgentState::Working(_)));
    // Each item's pieces, and its agent's pane: none is every agent.
    let mut items: Vec<(Spans, Option<String>)> =
        vec![(vec![("All agents".to_owned(), fill)], None)];
    let mark = |agent: &BoardAgent| (agent.mark.0.to_owned(), fill.fg(agent.mark.1));
    let tops = spaced
        .iter()
        .filter(|agent| super::lead_here(agent, &spaced).is_none());
    for top in tops {
        items.push((
            vec![mark(top), (format!(" {}", clean(&top.tab)), fill)],
            Some(top.pane_id.clone()),
        ));
        let workers: Vec<&&BoardAgent> = spaced
            .iter()
            .filter(|agent| {
                super::lead_here(agent, &spaced).is_some_and(|lead| lead.pane_id == top.pane_id)
            })
            .collect();
        for (index, worker) in workers.iter().enumerate() {
            let connector = if index + 1 == workers.len() {
                "╰"
            } else {
                "├"
            };
            let mut line = vec![
                (format!("  {connector}"), fill.fg(palette.overlay0)),
                mark(worker),
                (format!(" {}", clean(&worker.tab)), fill),
            ];
            if let AgentState::Working(Some(doing)) = &worker.state {
                line.push((format!(" · {}", clean(doing)), fill.fg(palette.text)));
            }
            items.push((line, Some(worker.pane_id.clone())));
        }
    }
    let widest = items
        .iter()
        .map(|(line, _)| {
            line.iter()
                .map(|(text, _)| display_width(text))
                .sum::<u16>()
        })
        .max()
        .unwrap_or(0);
    let x = hits.agent_button.x;
    let width = (widest + 4)
        .min(AGENT_MENU_MOST)
        .min(area.right().saturating_sub(x));
    let top = area.y.saturating_add(1);
    let height = (items.len() as u16 + 2).min(area.bottom().saturating_sub(top));
    if width < 5 || height < 3 {
        return;
    }
    let menu = Rect::new(x, top, width, height);
    let border = put_menu_box(buffer, menu, palette);
    let bottom = menu.bottom() - 1;
    for (index, (line, pane)) in items.into_iter().enumerate() {
        let y = top + 1 + index as u16;
        if y >= bottom {
            break;
        }
        put_segment(buffer, x, y, x + 1, "│", border);
        put_segment(buffer, menu.right() - 1, y, menu.right(), "│", border);
        let chosen = board.agent == pane;
        let end = menu.right() - 2;
        let mut at = x + 2;
        // The chosen item on the selection fill across its row, its words as they are.
        let row = Rect::new(x + 1, y, width - 2, 1);
        if chosen {
            buffer.set_style(row, style::chosen(palette));
        }
        for (text, style) in &line {
            let style = if chosen {
                style.bg(palette.selection_bg)
            } else {
                *style
            };
            // Its leading spaces stay; the rest is cut at a word.
            let words = text.trim_start();
            let lead = &text[..text.len() - words.len()];
            let room = end.saturating_sub(at).saturating_sub(display_width(lead));
            at = put_segment(
                buffer,
                at,
                y,
                end,
                &format!("{lead}{}", cut(words, room)),
                style,
            );
        }
        hits.agent_items.push((row, pane));
    }
}

/// The widest the agent menu grows, so what a worker is doing does not cross the board.
const AGENT_MENU_MOST: u16 = 60;

/// A header menu's box in `menu`, filled, its border drawn; returns the border's style for
/// the sides of each item's row.
fn put_menu_box(buffer: &mut Buffer, menu: Rect, palette: &Palette) -> Style {
    // Plain on any ground: a menu over a bold heading row would take its bold.
    let fill = style::band(palette)
        .fg(palette.text)
        .remove_modifier(Modifier::BOLD);
    for row in menu.rows() {
        put_segment(
            buffer,
            row.x,
            row.y,
            row.right(),
            &" ".repeat(usize::from(menu.width)),
            fill,
        );
    }
    let border = fill.fg(palette.overlay0);
    let across = "─".repeat(usize::from(menu.width - 2));
    put_segment(
        buffer,
        menu.x,
        menu.y,
        menu.right(),
        &format!("┌{across}┐"),
        border,
    );
    put_segment(
        buffer,
        menu.x,
        menu.bottom() - 1,
        menu.right(),
        &format!("└{across}┘"),
        border,
    );
    border
}

/// The view tabs' text.
const TASKS_TAB: &str = " Tasks ";
const WORK_TAB: &str = " Phases ";

/// Between the header's time and its close button: a button's gap and a button's padding,
/// so the time sits off the close button as two buttons sit apart.
const TIME_GAP: u16 = spacing::HEADER_GAP + spacing::PAD;

/// The line under the header while the filter takes the keys: its keys, under the box, over
/// the blank row the board keeps there, so nothing else moves.
fn render_filter_hint(buffer: &mut Buffer, area: Rect, palette: &Palette, hits: &TaskBoardHits) {
    const KEYS: &[Hint] = &[("Enter", "keep"), ("Esc", "clear")];
    let y = area.y.saturating_add(1);
    let end = area.right().saturating_sub(spacing::MARGIN);
    let x = hits.filter.x.min(end.saturating_sub(hints_width(KEYS)));
    put_hints(buffer, x, y, end, KEYS, palette, &mut Vec::new());
}

/// The search box in `cell`: `⌕ Search` at rest (`⌕` alone when `compact`), the text being
/// typed with its cursor (its end, when longer), or the kept text with a `×` that clears it.
/// A click on the box types into the filter.
fn put_filter_box(
    buffer: &mut Buffer,
    cell: Rect,
    compact: bool,
    board: &TaskBoard,
    palette: &Palette,
    hits: &mut TaskBoardHits,
) {
    let text = clean(board.filter.trim_start());
    let kept = !board.filter_editing && !text.trim().is_empty();
    let active = board.filter_editing || kept;
    let (x, y, width, end) = (cell.x, cell.y, cell.width, cell.right());
    let room = usize::from(width.saturating_sub(5));
    let body = if board.filter_editing {
        let count = text.chars().count();
        let shown: String = if count >= room {
            let tail: String = text.chars().skip(count + 2 - room.max(1)).collect();
            format!("…{tail}")
        } else {
            text
        };
        format!(" ⌕ {shown}▏")
    } else if kept {
        format!(" ⌕ {}", crate::ui::truncate_end(text.trim(), room))
    } else if compact {
        FILTER_BOX_COMPACT.to_owned()
    } else {
        " ⌕ Search".to_owned()
    };
    let padded = format!("{body:<width$}", width = usize::from(width));
    put_segment(
        buffer,
        x,
        y,
        end,
        &padded,
        style::filter_box(palette, active),
    );
    if kept && width >= 2 {
        let clear = end.saturating_sub(2);
        put_segment(
            buffer,
            clear,
            y,
            end,
            "× ",
            style::filter_box(palette, true),
        );
        hits.filter_clear = Rect::new(clear, y, 1, 1);
        hits.filter = Rect::new(x, y, clear.saturating_sub(x), 1);
    } else {
        hits.filter = Rect::new(x, y, width, 1);
    }
}

/// Every space the list names, in its order: its tasks' spaces, then its cards'.
fn read_spaces(list: Option<&super::model::TaskList>) -> Vec<String> {
    let mut spaces: Vec<String> = Vec::new();
    let Some(list) = list else {
        return spaces;
    };
    let named = list
        .tasks
        .iter()
        .map(|task| &task.space)
        .chain(list.work.iter().map(|card| &card.space));
    for space in named {
        if !space.is_empty() && !spaces.contains(space) {
            spaces.push(space.clone());
        }
    }
    spaces
}

/// The space button's menu, while open: `All spaces`, then each space, under the button and
/// over the board; the space shown in the accent.
fn render_space_menu(
    buffer: &mut Buffer,
    area: Rect,
    board: &TaskBoard,
    palette: &Palette,
    hits: &mut TaskBoardHits,
) {
    if !board.space_menu {
        return;
    }
    let spaces: Vec<Option<String>> = std::iter::once(None)
        .chain(read_spaces(board.store.list()).into_iter().map(Some))
        .collect();
    let items: Vec<(String, bool)> = spaces
        .iter()
        .map(|space| {
            let label = space
                .as_deref()
                .map_or_else(|| "All spaces".to_owned(), clean);
            (label, board.space == *space)
        })
        .collect();
    let widest = items
        .iter()
        .map(|(label, _)| display_width(label))
        .max()
        .unwrap_or(0);
    let rects = put_menu(buffer, area, (hits.space_button.x, widest), &items, palette);
    hits.space_items = rects.into_iter().zip(spaces).collect();
}

/// The sort button's menu, while open in Tasks: each order under the button and over the
/// board, the chosen one in the accent with a `✓`. As wide for every choice.
fn render_sort_menu(
    buffer: &mut Buffer,
    area: Rect,
    board: &TaskBoard,
    palette: &Palette,
    hits: &mut TaskBoardHits,
) {
    if !board.sort_menu || board.view != BoardView::Tasks {
        return;
    }
    // Opened by its key on a header too narrow for the button: under the search box.
    let x = match hits.sort_button.is_empty() {
        true => hits.filter.x,
        false => hits.sort_button.x,
    };
    const CHECK: &str = " ✓";
    let items: Vec<(String, bool)> = TaskSort::ALL
        .iter()
        .map(|sort| match *sort == board.sort {
            true => (format!("{}{CHECK}", sort.label()), true),
            false => (sort.label().to_owned(), false),
        })
        .collect();
    let widest = TaskSort::ALL
        .iter()
        .map(|sort| display_width(sort.label()))
        .max()
        .unwrap_or(0)
        + display_width(CHECK);
    let rects = put_menu(buffer, area, (x, widest), &items, palette);
    hits.sort_items = rects.into_iter().zip(TaskSort::ALL).collect();
}

/// A header menu from `x`, one row under the header, its items `widest` columns wide inside a
/// square box, each a row, the chosen ones in the accent; returns each item's row, top down,
/// as far as the board has room.
fn put_menu(
    buffer: &mut Buffer,
    area: Rect,
    (x, widest): (u16, u16),
    items: &[(String, bool)],
    palette: &Palette,
) -> Vec<Rect> {
    let text = Style::default().fg(palette.text);
    let rows: Vec<MenuRow> = items
        .iter()
        .map(|(label, chosen)| MenuRow {
            spans: vec![(label.clone(), text)],
            chosen: *chosen,
            item: true,
        })
        .collect();
    let top = area.y.saturating_add(1);
    put_menu_rows(buffer, area, (x, top, widest + 4), &rows, palette)
}

/// A menu's row: its spans, whether it is on the selection fill, and whether it is an item
/// (a heading or a blank row is not).
struct MenuRow {
    spans: Vec<(String, Style)>,
    chosen: bool,
    item: bool,
}

/// A menu `width` wide (its box included) from `x` and `top`, cut to the board: its rows inside
/// a square box, one column in from each border; returns each drawn item's row, top down.
fn put_menu_rows(
    buffer: &mut Buffer,
    area: Rect,
    (x, top, width): (u16, u16, u16),
    rows: &[MenuRow],
    palette: &Palette,
) -> Vec<Rect> {
    let width = width.min(area.right().saturating_sub(x));
    let height = (rows.len() as u16 + 2).min(area.bottom().saturating_sub(top));
    if width < 5 || height < 3 {
        return Vec::new();
    }
    let menu = Rect::new(x, top, width, height);
    let border = put_menu_box(buffer, menu, palette);
    let bottom = menu.bottom() - 1;
    let mut rects = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let y = top + 1 + index as u16;
        if y >= bottom {
            break;
        }
        put_segment(buffer, x, y, x + 1, "│", border);
        put_segment(buffer, menu.right() - 1, y, menu.right(), "│", border);
        let line = Rect::new(x + 1, y, width - 2, 1);
        if row.chosen {
            buffer.set_style(line, style::chosen(palette));
        }
        let mut at = x + 2;
        for (text, style) in &row.spans {
            at = put_segment(buffer, at, y, menu.right() - 2, text, *style);
        }
        if row.item {
            rects.push(line);
        }
    }
    rects
}

/// The Filter menu, under its button and ending on its right edge: STATUS, KIND and OWNER,
/// each under a grey heading and apart by a blank row; each item its mark in its colour, its
/// words, a `✓` once picked and a grey count of the cards picking it would show. The item
/// under the keyboard is on the selection fill.
fn render_work_filter_menu(
    buffer: &mut Buffer,
    area: Rect,
    board: &TaskBoard,
    agents: &[BoardAgent],
    palette: &Palette,
    hits: &mut TaskBoardHits,
) {
    let button = hits.work_filter_button;
    let list = board.store.list();
    let (Some(list), false) = (list, button.is_empty()) else {
        return;
    };
    if !board.work_filter_menu || board.view != BoardView::Work {
        return;
    }
    let scope = super::work::Scope::of(board, agents);
    let words = super::filter::words(&board.filter);
    let cards: Vec<&WorkItem> = super::work::columns(list, scope, &words)
        .into_iter()
        .flatten()
        .collect();
    let context = (list.tasks.as_slice(), agents);
    let counts: Vec<String> = FilterPick::ALL
        .iter()
        .map(|pick| {
            board
                .work_filter
                .count_with(*pick, &cards, context)
                .to_string()
        })
        .collect();
    const CHECK: &str = "✓ ";
    let words_width = FilterPick::ALL
        .iter()
        .map(|pick| display_width(pick.words().1))
        .max()
        .unwrap_or(0);
    let count_width = counts
        .iter()
        .map(|count| display_width(count))
        .max()
        .unwrap_or(0);
    // The mark and a space, the words, a gap, the check's room, the count.
    let inner = 2 + words_width + spacing::COLUMN_GAP + display_width(CHECK) + count_width;
    let grey = Style::default().fg(palette.overlay1);
    let text = Style::default().fg(palette.text);
    let mut rows = Vec::new();
    let mut group = "";
    for (index, pick) in FilterPick::ALL.iter().enumerate() {
        if pick.group() != group {
            if !group.is_empty() {
                rows.push(MenuRow {
                    spans: Vec::new(),
                    chosen: false,
                    item: false,
                });
            }
            group = pick.group();
            rows.push(MenuRow {
                spans: vec![(group.to_uppercase(), grey)],
                chosen: false,
                item: false,
            });
        }
        let (mark, words) = pick.words();
        let check = if board.work_filter.has(*pick) {
            CHECK
        } else {
            "  "
        };
        let pad = words_width - display_width(words) + spacing::COLUMN_GAP;
        let count = &counts[index];
        let count_pad = count_width - display_width(count);
        rows.push(MenuRow {
            spans: vec![
                (
                    format!("{mark:<1} "),
                    Style::default().fg(pick.role(palette)),
                ),
                (words.to_owned(), text),
                (spacing::gap(pad), text),
                (check.to_owned(), text),
                (format!("{}{count}", spacing::gap(count_pad)), grey),
            ],
            chosen: index == board.work_filter_cursor.min(FilterPick::ALL.len() - 1),
            item: true,
        });
    }
    let width = inner + 4;
    let x = button.right().saturating_sub(width).max(area.x);
    let rects = put_menu_rows(buffer, area, (x, button.y + 1, width), &rows, palette);
    hits.work_filter_items = rects.into_iter().zip(FilterPick::ALL).collect();
}

/// The 12-hour clock time, `2:05 pm`, of the ISO date and time `generated` gives, as written
/// there (in the reader's own offset); none when it has no time.
fn clock_time(generated: &str) -> Option<String> {
    let time = generated.get(10..16)?.strip_prefix('T')?;
    let (hour, minute) = time.split_once(':')?;
    let (hour, minute): (u8, u8) = (hour.parse().ok()?, minute.parse().ok()?);
    if hour > 23 || minute > 59 || generated.get(13..14) != Some(":") {
        return None;
    }
    let half = if hour < 12 { "am" } else { "pm" };
    let hour = match hour % 12 {
        0 => 12,
        hour => hour,
    };
    Some(format!("{hour}:{minute:02} {half}"))
}

/// A header category button: its words, full and short, its count, its colour, and whether
/// the view shows it alone.
struct Chip {
    category: Category,
    label: &'static str,
    short: &'static str,
    count: String,
    role: Color,
    on: bool,
}

impl Chip {
    /// Its text as it draws, in pieces, each with whether it is the mark: a space, the mark,
    /// then its words (none when short) and its count one space apart, and a space.
    fn texts(&self, short: bool) -> [(String, bool); 3] {
        let (mark, words) = self.label.split_once(' ').unwrap_or((self.label, ""));
        let rest = match short || words.is_empty() {
            true => format!(" {} ", self.count),
            false => format!(" {words} {} ", self.count),
        };
        let mark = if short { self.short } else { mark };
        [
            (" ".to_owned(), false),
            (mark.to_owned(), true),
            (rest, false),
        ]
    }

    fn width(&self, short: bool) -> u16 {
        self.texts(short)
            .iter()
            .map(|(text, _)| display_width(text))
            .sum()
    }
}

/// Draws `chip` from `x`: its mark in its colour, its words and count grey; the one shown
/// alone on a tint of its colour. Returns the column after it.
fn put_chip(
    buffer: &mut Buffer,
    x: u16,
    y: u16,
    end: u16,
    chip: &Chip,
    short: bool,
    palette: &Palette,
) -> u16 {
    let (mark_style, words_style) = style::chip(chip.role, chip.on, palette);
    let mut x = x;
    for (text, is_mark) in chip.texts(short) {
        let style = if is_mark { mark_style } else { words_style };
        x = put_segment(buffer, x, y, end, &text, style);
    }
    x
}

/// `shown`, or `shown of all` while a filter narrows what shows.
fn chip_count(shown: usize, all: Option<usize>) -> String {
    match all {
        Some(all) => format!("{shown} of {all}"),
        None => shown.to_string(),
    }
}

/// The Tasks view's chips, each showing its section alone, its icon and words, or its icon
/// alone when narrow, in the order the sections show: In progress counts the working agents,
/// Action the tasks and the tabs waiting on the user, Queued and Waiting their tasks; in the
/// space shown and by the filter's words (of those in the space, while a filter is set).
/// Scheduled tasks are left out, as the section titles count them apart; COMPLETED has none.
fn task_chips(
    (tasks, spaced): (&[&Task], &[&Task]),
    (waiting, working): ((usize, usize), (usize, usize)),
    filtering: bool,
    only: Option<TaskSection>,
    palette: &Palette,
) -> Vec<Chip> {
    let count = |tasks: &[&Task], shown: TaskSection| {
        tasks
            .iter()
            .filter(|task| section(task) == section_index(shown) && !scheduled(task))
            .count()
    };
    [
        (TaskSection::InProgress, "◐ In progress", "◐"),
        (TaskSection::ActionRequired, "! Action required", "!"),
        (TaskSection::Queued, "◌ Queued", "◌"),
        (TaskSection::Waiting, "… Waiting on others", "…"),
    ]
    .into_iter()
    .map(|(shown, label, short)| {
        let (shown_count, all) = match shown {
            TaskSection::InProgress => (working.0, working.1),
            TaskSection::ActionRequired => (
                count(tasks, shown) + waiting.0,
                count(spaced, shown) + waiting.1,
            ),
            _ => (count(tasks, shown), count(spaced, shown)),
        };
        Chip {
            category: Category::Section(shown),
            label,
            short,
            count: chip_count(shown_count, filtering.then_some(all)),
            role: style::section_role(shown, palette),
            on: only == Some(shown),
        }
    })
    .collect()
}

/// The Tasks view's chip row: its chips one gap apart, the last on the table's right edge;
/// narrow, each its icon and count, then the last ones leave.
fn render_task_chips(
    buffer: &mut Buffer,
    row: Rect,
    board: &TaskBoard,
    shown: (&[&Task], &[&Task]),
    counts: ((usize, usize), (usize, usize)),
    palette: &Palette,
    hits: &mut TaskBoardHits,
) {
    let filtering = !super::filter::words(&board.filter).is_empty();
    let chips = task_chips(shown, counts, filtering, board.only_section, palette);
    let left = row.x.saturating_add(spacing::MARGIN);
    let right = row.right().saturating_sub(spacing::MARGIN);
    let room = right.saturating_sub(left);
    let width_of = |short: bool, keep: usize| -> u16 {
        chips
            .iter()
            .take(keep)
            .map(|chip| chip.width(short))
            .sum::<u16>()
            + spacing::HEADER_GAP * (keep as u16).saturating_sub(1)
    };
    let (short, keep) = [false, true]
        .into_iter()
        .map(|short| (short, chips.len()))
        .chain((0..chips.len()).rev().map(|keep| (true, keep)))
        .find(|(short, keep)| width_of(*short, *keep) <= room)
        .unwrap_or((true, 0));
    let mut x = right.saturating_sub(width_of(short, keep));
    for chip in chips.iter().take(keep) {
        let chip_x = x;
        x = put_chip(buffer, chip_x, row.y, right, chip, short, palette);
        hits.categories.push((
            Rect::new(chip_x, row.y, x.saturating_sub(chip_x), 1),
            chip.category,
        ));
        x = x.saturating_add(spacing::HEADER_GAP);
    }
}

/// The Filter button's text, the shape of the sort button; narrow, its `▾` alone.
const FILTER_BUTTON: &str = " Filter ▾ ";
const FILTER_BUTTON_NARROW: &str = " ▾ ";

/// A pill's clear, after its words.
const PILL_CLEAR: &str = "× ";

/// Pills as they draw: each one's words and the pick its clear clears (none: every pick).
type Pills = Vec<(String, Option<FilterPick>)>;

/// A pill's words, before its clear: its group and pick (`Status: Needs you`), or the pick
/// alone.
fn pill_words(pick: FilterPick, value_only: bool) -> String {
    let (_, words) = pick.words();
    match value_only {
        true => format!(" {words} "),
        false => format!(" {}: {words} ", pick.group()),
    }
}

/// The Work view's chip row: the Filter button on the columns' right edge, and left of it a
/// pill for each pick, one gap apart. Narrow, the pills name only their pick, then the button
/// shows its `▾` alone, then one pill says how many picks there are and clears them all: a
/// pick never leaves the row unseen.
fn render_work_picks(
    buffer: &mut Buffer,
    row: Rect,
    board: &TaskBoard,
    palette: &Palette,
    hits: &mut TaskBoardHits,
) {
    let left = row.x.saturating_add(spacing::MARGIN);
    let right = row.right().saturating_sub(spacing::MARGIN);
    let room = right.saturating_sub(left);
    let picks = board.work_filter.picks();
    let pill_width = |words: &str| display_width(words).saturating_add(display_width(PILL_CLEAR));
    let summed = format!(" {} filters ", picks.len());
    // Each pill's words and the pick it clears, one plan per width, widest first.
    let plans: Vec<(Pills, &str)> = vec![
        (
            picks
                .iter()
                .map(|pick| (pill_words(*pick, false), Some(*pick)))
                .collect(),
            FILTER_BUTTON,
        ),
        (
            picks
                .iter()
                .map(|pick| (pill_words(*pick, true), Some(*pick)))
                .collect(),
            FILTER_BUTTON,
        ),
        (
            picks
                .iter()
                .map(|pick| (pill_words(*pick, true), Some(*pick)))
                .collect(),
            FILTER_BUTTON_NARROW,
        ),
        (
            match picks.len() {
                0 => Vec::new(),
                1 => vec![(pill_words(picks[0], true), Some(picks[0]))],
                _ => vec![(summed.clone(), None)],
            },
            FILTER_BUTTON_NARROW,
        ),
    ];
    let width_of = |(pills, button): &(Pills, &str)| -> u16 {
        pills
            .iter()
            .map(|(words, _)| pill_width(words) + spacing::HEADER_GAP)
            .sum::<u16>()
            + display_width(button)
    };
    let last = plans.len() - 1;
    let (pills, button) = plans
        .iter()
        .find(|plan| width_of(plan) <= room)
        .unwrap_or(&plans[last]);
    let mut x = right.saturating_sub(width_of(&(pills.clone(), *button)).min(room));
    let fill = style::button(palette);
    let clear = fill.fg(palette.overlay1);
    for (words, pick) in pills {
        x = put_segment(buffer, x, row.y, right, words, fill);
        let clear_x = x;
        x = put_segment(buffer, x, row.y, right, PILL_CLEAR, clear);
        hits.work_filter_pills.push((
            Rect::new(clear_x, row.y, x.saturating_sub(clear_x), 1),
            *pick,
        ));
        x = x.saturating_add(spacing::HEADER_GAP);
    }
    let button_x = x;
    x = put_segment(buffer, button_x, row.y, right, button, fill);
    hits.work_filter_button = Rect::new(button_x, row.y, x.saturating_sub(button_x), 1);
}
