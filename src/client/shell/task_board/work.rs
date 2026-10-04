//! The Work view: every feature, bug and chore as a card in its stage's column, and the
//! selected card in full in a panel under the columns. Pure, like the Tasks view.

use std::ops::Range;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::model::{
    DueLevel, Task, TaskAgent, TaskGroup, TaskList, WorkItem, WorkKind, WorkStage, WorkStep,
};
use super::render::{hints_width, put_hints, Hint, CLOSE_WIDTH};
use super::spacing::{self, COLUMN_GAP};
use super::style;
use super::text::{at_most, clean, cut, home_path, put_fitted, wrap, Marks};
use super::{BoardAgent, Mark, Spans, TaskBoard, TaskBoardHits};
use crate::app::state::Palette;
use crate::client::shell::render::{display_width, put_segment};

/// The stages in their columns' order, with each column's label.
pub(super) const STAGES: [(WorkStage, &str); 5] = [
    (WorkStage::Plan, "PLAN"),
    (WorkStage::Build, "BUILD"),
    (WorkStage::Review, "REVIEW"),
    (WorkStage::Check, "VERIFY"),
    (WorkStage::Shipped, "SHIP"),
];

/// A column's heading rows: its label, then its colour's underline.
const HEADING_ROWS: u16 = 2;

/// A column's colour, by its index in `STAGES`.
fn stage_role(index: usize, palette: &Palette) -> ratatui::style::Color {
    STAGES
        .get(index)
        .and_then(|(stage, _)| style::stage_role(*stage, palette))
        .unwrap_or(palette.text)
}

/// SHIP keeps a card this many days after `shipped_on`.
const SHIPPED_DAYS: i64 = 7;

/// The card panel's least height: its two borders, its heading bar, its rule and its
/// buttons. A body too short for half of it to hold these shows no panel.
const PANEL_MIN_ROWS: u16 = 5;

/// A panel this tall also has room for its file and branch lines and a blank line over its
/// keys, and two lines above them.
const PANEL_FOOT_MIN_ROWS: u16 = PANEL_MIN_ROWS + 5;

/// What an empty stage's column says under its label.
const NO_ITEMS: &str = "no items";

const EMPTY: &str =
    "No features, bugs or chores yet: they appear when a lead writes an intent file.";

/// What the Work view narrows to: a space, and an agent of it; with the live `agents`, whose
/// leads a card's who line may name.
#[derive(Debug, Clone, Copy)]
pub(super) struct Scope<'a> {
    pub(super) space: Option<&'a str>,
    pub(super) agent: Option<&'a BoardAgent>,
    pub(super) agents: &'a [BoardAgent],
}

impl<'a> Scope<'a> {
    /// The board's space, and the agent it shows alone among `agents`.
    pub(super) fn of(board: &'a TaskBoard, agents: &'a [BoardAgent]) -> Self {
        Self {
            space: board.space.as_deref(),
            agent: super::shown_agent(board, agents),
            agents,
        }
    }

    /// Whether `card` is in the space and, with an agent shown alone, that agent's: its own
    /// agent's, else that of an open task among `tasks` that links it by its slug. Only an
    /// agent shown alone scans `tasks`, once per card without an agent of its own.
    pub(super) fn holds(&self, card: &WorkItem, tasks: &[Task]) -> bool {
        self.space.is_none_or(|space| card.space == space)
            && self.agent.is_none_or(|agent| {
                card_agent(card, tasks)
                    .is_some_and(|(named, space)| super::tags_agent(named, space, agent))
            })
    }
}

/// The cards each column shows, in the list's order: those in the `scope` that the filter's
/// `words` keep, in a stage this client knows, and in SHIPPED only this week (counted from the
/// list's own date, so render reads no clock).
pub(super) fn columns<'a>(
    list: &'a TaskList,
    scope: Scope,
    words: &[String],
) -> [Vec<&'a WorkItem>; 5] {
    let today = day_number(&list.generated);
    std::array::from_fn(|index| {
        let stage = STAGES[index].0;
        list.work
            .iter()
            .filter(|card| card.stage == stage && column_shows(card, list, today, scope, words))
            .collect()
    })
}

/// Whether its stage's column shows `card`: in the `scope`, kept by the filter's `words`, and
/// in SHIPPED only this week.
fn column_shows(
    card: &WorkItem,
    list: &TaskList,
    today: Option<i64>,
    scope: Scope,
    words: &[String],
) -> bool {
    scope.holds(card, &list.tasks)
        && (card.stage != WorkStage::Shipped || shipped_this_week(card, today))
        && super::filter::card_matches(card, (&list.tasks, scope.agents), words)
}

/// The selected card, when a column shows it: in a stage this client knows, in the `scope`,
/// kept by the filter's `words` and the Filter menu's picks.
fn selected_card<'a>(
    board: &TaskBoard,
    list: &'a TaskList,
    scope: Scope,
    words: &[String],
) -> Option<&'a WorkItem> {
    let file = board.work_selected.as_ref()?;
    let today = day_number(&list.generated);
    list.work.iter().find(|card| {
        card.file == *file
            && STAGES.iter().any(|(stage, _)| card.stage == *stage)
            && column_shows(card, list, today, scope, words)
            && board.work_filter.keeps(card, (&list.tasks, scope.agents))
    })
}

/// The selected card's panel's rows in a body `height` rows tall: exactly half, under the
/// columns and a blank row; in a short window less, so the columns keep a whole card; none
/// when that is too few to hold the panel.
fn panel_rows(height: u16) -> u16 {
    let rows = (height / 2).min(height.saturating_sub(HEADING_ROWS + 1 + CARD_HEIGHT as u16));
    if rows >= PANEL_MIN_ROWS {
        rows
    } else {
        0
    }
}

/// The panel's width across a body `width` wide: inside the board's margins.
fn panel_width(width: u16) -> u16 {
    width.saturating_sub(2 * spacing::MARGIN)
}

/// Where the card panel draws in the body, and the part of the body left to the columns.
struct PanelPlace {
    rect: Rect,
    columns: Rect,
}

/// The card panel's place in `body`: at the bottom, as tall as `panel_rows` allows; none when
/// the body is too short.
fn panel_place(body: Rect) -> Option<PanelPlace> {
    let rows = panel_rows(body.height);
    (rows > 0).then(|| PanelPlace {
        rect: Rect::new(
            body.x + spacing::MARGIN,
            body.bottom() - rows,
            panel_width(body.width),
            rows,
        ),
        columns: Rect {
            height: body.height - rows - 1,
            ..body
        },
    })
}

/// The columns inside a panel `width` wide, between its borders and pads.
fn panel_inner_width(width: u16) -> u16 {
    width.saturating_sub(2 + 2 * spacing::PAD)
}

/// A card's panel keys: its steps while it has any, its lead's tab when an agent is on it,
/// and its file while the server takes `tasks.open`.
fn panel_buttons(board: &TaskBoard, card: &WorkItem) -> Vec<Hint> {
    let mut buttons: Vec<Hint> = Vec::new();
    if !card.steps.is_empty() {
        buttons.push(("↑↓", "step"));
    }
    // Without an agent `Enter` opens the file, so `o` is not shown for it too.
    match (agent_pane(card), board.can_open) {
        (Some(_), true) => buttons.extend([("Enter", "go to lead's tab"), ("o", "open file")]),
        (Some(_), false) => buttons.push(("Enter", "go to lead's tab")),
        (None, true) => buttons.push(("Enter", "open file")),
        (None, false) => {}
    }
    buttons
}

/// The pane of the agent on `card`, where `Enter` goes; none without one.
pub(super) fn agent_pane(card: &WorkItem) -> Option<&str> {
    card.agent
        .as_ref()
        .map(|agent| agent.pane.as_str())
        .filter(|pane| !pane.is_empty())
}

/// The keys the card panel draws in `body`: the selected card's while its panel shows and
/// they fit across it; none otherwise. The footer leaves these keys out.
pub(super) fn panel_keys(
    board: &TaskBoard,
    list: &TaskList,
    (scope, words): (Scope, &[String]),
    body: Rect,
) -> Vec<Hint> {
    let place = panel_place(body).filter(|_| !board.work_panel_closed);
    let (Some(card), Some(place)) = (selected_card(board, list, scope, words), place) else {
        return Vec::new();
    };
    let buttons = panel_buttons(board, card);
    if hints_width(&buttons) <= panel_inner_width(place.rect.width) {
        buttons
    } else {
        Vec::new()
    }
}

fn shipped_this_week(card: &WorkItem, today: Option<i64>) -> bool {
    match (today, card.shipped_on.as_deref().and_then(day_number)) {
        (Some(today), Some(shipped)) => today - shipped < SHIPPED_DAYS,
        // Without both dates there is no telling its age, so it shows.
        _ => true,
    }
}

/// Days since 1970-01-01 of the `YYYY-MM-DD` that starts `date`.
fn day_number(date: &str) -> Option<i64> {
    let mut parts = date.get(..10)?.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    // Howard Hinnant's days_from_civil.
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(era * 146_097 + day_of_era - 719_468)
}

/// A kind's mark and word.
pub(super) fn kind_mark(kind: WorkKind) -> (&'static str, &'static str) {
    match kind {
        WorkKind::Feature => ("◆", "feature"),
        WorkKind::Bug => ("✱", "bug"),
        // A square, apart from the rail's idle `○`.
        WorkKind::Chore => ("▣", "chore"),
        WorkKind::Unknown => ("·", "work"),
    }
}

/// The commit types a title may start with, as `feat:` or `feat(board):`.
const TITLE_PREFIXES: [&str; 7] = ["feat", "fix", "chore", "docs", "test", "refactor", "perf"];

/// A card's title as the board shows it: without a leading conventional prefix (`feat:`,
/// `fix(board):`), which the kind's mark already says. The list keeps the raw title.
pub(super) fn shown_title(title: &str) -> &str {
    let trimmed = title.trim_start();
    let Some(kind) = TITLE_PREFIXES
        .iter()
        .find(|kind| trimmed.starts_with(**kind))
    else {
        return title;
    };
    let rest = &trimmed[kind.len()..];
    // An optional `(scope)` and `!`, then the colon.
    let rest = match rest.strip_prefix('(') {
        Some(scoped) => match scoped.split_once(')') {
            Some((scope, after)) if !scope.contains(char::is_whitespace) => after,
            _ => return title,
        },
        None => rest,
    };
    let rest = rest.strip_prefix('!').unwrap_or(rest);
    match rest.strip_prefix(':') {
        Some(after) if !after.trim().is_empty() => after.trim_start(),
        _ => title,
    }
}

/// The lines inside a card's box: kind and project, a rule, the title's two, the progress
/// line, who, and the phase's status.
const CARD_LINES: usize = 7;

/// The card line drawn as a rule across its box, `├───┤`.
const RULE_LINE: usize = 1;

/// Every card's height on screen, its box included.
const CARD_HEIGHT: usize = CARD_LINES + 2;

/// Who has a card, as its card and its panel both say: the agent on it (`card_agent`), else
/// the owner of its first open linked task, else its space's lead among `agents`, else its
/// space. None only for a card with none of these.
pub(super) enum Holder<'a> {
    /// The agent, with the space its tag names it in.
    Agent(&'a TaskAgent, &'a str),
    /// The user: an owner `you`, in any case.
    You,
    Named(&'a str),
    /// The card's space, when nothing else has it: drawn, but never matched by the filter,
    /// which does not search the space a card sits in.
    Space(&'a str),
}

pub(super) fn holder<'a>(
    card: &'a WorkItem,
    tasks: &'a [Task],
    agents: &'a [BoardAgent],
) -> Option<Holder<'a>> {
    if let Some((agent, space)) = card_agent(card, tasks) {
        return Some(Holder::Agent(agent, space));
    }
    let owner = card.slug.as_deref().and_then(|slug| {
        tasks
            .iter()
            .filter(|task| task.card.as_deref() == Some(slug) && task.group != TaskGroup::Done)
            .find_map(|task| {
                task.owner
                    .as_deref()
                    .map(str::trim)
                    .filter(|owner| !owner.is_empty())
            })
    });
    match owner {
        Some(owner) if owner.eq_ignore_ascii_case("you") => Some(Holder::You),
        Some(owner) => Some(Holder::Named(owner)),
        None => super::space_lead(&card.space, agents)
            .map(|lead| Holder::Named(lead.tab.as_str()))
            .or_else(|| {
                Some(card.space.as_str())
                    .filter(|space| !space.is_empty())
                    .map(Holder::Space)
            }),
    }
}

/// The agent on a card, with the space its tag names it in: the card's own, else the agent of
/// an open task that links it by its slug (a reader sends a card's own agent as none).
fn card_agent<'a>(card: &'a WorkItem, tasks: &'a [Task]) -> Option<(&'a TaskAgent, &'a str)> {
    if let Some(agent) = &card.agent {
        return Some((agent, card.space.as_str()));
    }
    let slug = card.slug.as_deref()?;
    tasks
        .iter()
        .filter(|task| task.card.as_deref() == Some(slug) && task.group != TaskGroup::Done)
        .find_map(|task| Some((task.agent.as_ref()?, task.space.as_str())))
}

/// An agent on a card as its who line says it: `herdr-w1 · 12m`.
fn agent_words(agent: &TaskAgent) -> String {
    match agent.minutes {
        Some(minutes) => format!("{} · {minutes}m", agent.name),
        None => agent.name.clone(),
    }
}

/// A card's who line in words, as the filter matches it; None when only its space has it.
pub(super) fn who_text(
    card: &WorkItem,
    (tasks, agents): (&[Task], &[BoardAgent]),
) -> Option<String> {
    Some(match holder(card, tasks, agents)? {
        Holder::Agent(agent, _) => agent_words(agent),
        Holder::You => "You".to_owned(),
        Holder::Named(name) => name.to_owned(),
        Holder::Space(_) => return None,
    })
}

/// What a card's phase has to say, as `mark words` in the mark's colour, never bold: a phase
/// that waits on the user `? Awaiting your approval` (PLAN) or `? Awaiting your check` (VERIFY),
/// yellow; one passed `✓ Approved`, `✓ Review passed`, `✓ Verified`, `✓ Shipped 28 Sep`, green;
/// a BUILD deadline `! Overdue 3d` red or `! Due today` yellow; a card whose status is
/// `blocked` `✗ Blocked`, red; anything else neutral and grey: `· On track`, `· Due Thu`,
/// `· Awaiting review`, `· 2 open findings`. None for a stage this client does not know, even
/// a blocked one, or a ship without its day.
fn phase_status(
    card: &WorkItem,
    this_year: Option<&str>,
    palette: &Palette,
) -> Option<(String, Style)> {
    let say = |mark: &str, words: &str, colour| {
        Some((
            format!("{mark} {}", clean(words)),
            Style::default().fg(colour),
        ))
    };
    let yours = |words: &str| say("?", words, palette.yellow);
    let passed = |words: &str| say("✓", words, palette.green);
    let neutral = |words: &str| say("·", words, palette.overlay1);
    if !matches!(card.stage, WorkStage::Shipped | WorkStage::Unknown)
        && card.status.trim().eq_ignore_ascii_case("blocked")
    {
        return say("✗", "Blocked", palette.red);
    }
    match card.stage {
        // Only an approval that says so: `rejected` or `pending` still waits.
        WorkStage::Plan => match approved(card) {
            true => passed("Approved"),
            false => yours("Awaiting your approval"),
        },
        WorkStage::Build => {
            let Some(due) = card.due_text.as_deref() else {
                return neutral("On track");
            };
            let words = due_words(due, "Due");
            match card.due_level {
                Some(DueLevel::Overdue) => say("!", &words, palette.red),
                Some(DueLevel::Today) => say("!", &words, palette.yellow),
                _ => neutral(&words),
            }
        }
        WorkStage::Review => {
            let open = card.findings.iter().filter(|finding| finding.open).count();
            match (open, card.findings.len()) {
                (0, 0) => neutral("Awaiting review"),
                (0, _) => passed("Review passed"),
                (open, _) => neutral(&format!("{open} open {}", plural_word(open, "finding"))),
            }
        }
        WorkStage::Check => match card.verification {
            Some(_) => passed("Verified"),
            None => yours("Awaiting your check"),
        },
        WorkStage::Shipped => passed(&format!(
            "Shipped {}",
            day_month(card.shipped_on.as_deref()?, this_year)?
        )),
        WorkStage::Unknown => None,
    }
}

/// Whether a card's plan is approved: only an approval that says so; `rejected` or `pending`
/// still waits.
fn approved(card: &WorkItem) -> bool {
    card.approval.as_deref().is_some_and(|approval| {
        approval
            .trim()
            .get(..8)
            .is_some_and(|word| word.eq_ignore_ascii_case("approved"))
    })
}

/// Whether a card's phase waits on the user: a plan not approved, a build in VERIFY with no
/// verification yet.
pub(super) fn waits_on_you(card: &WorkItem) -> bool {
    match card.stage {
        WorkStage::Plan => !approved(card),
        WorkStage::Check => card.verification.is_none(),
        _ => false,
    }
}

/// A deadline's words after `lead` (`Due Wed`, `due today`); one already overdue says so alone,
/// in the lead's case (`Overdue 3d`, `overdue 3d`).
fn due_words(due_text: &str, lead: &str) -> String {
    if due_text.starts_with("Overdue") {
        return if lead.starts_with(char::is_uppercase) {
            due_text.to_owned()
        } else {
            due_text.to_lowercase()
        };
    }
    match due_text {
        "Today" | "Tomorrow" => format!("{lead} {}", due_text.to_lowercase()),
        _ => format!("{lead} {due_text}"),
    }
}

/// A day as `28 Sep`, with its year when that is not `this_year` (the list's, so render reads
/// no clock): `5 Jan 2027`.
fn day_month(day: &str, this_year: Option<&str>) -> Option<String> {
    let (month, date) = super::table::short_date(day)?
        .split_once(' ')
        .map(|(month, date)| (month.to_owned(), date.to_owned()))?;
    let year = day.get(..4)?;
    Some(if this_year == Some(year) {
        format!("{date} {month}")
    } else {
        format!("{date} {month} {year}")
    })
}

/// A card's inside, `width` columns wide, always the same seven lines: its kind and project
/// (`◆ feature · herdr`), the rule `draw_card` draws, the title on two lines (cut at a word
/// with `…`; the panel shows it whole), its progress line, who has it (after the sub-agent
/// `spinner` while its agent works), and its phase's status. A line with nothing to say is
/// blank, so every card is one size. Every line starts at the same column.
fn card_lines(
    card: &WorkItem,
    (tasks, agents): (&[Task], &[BoardAgent]),
    width: u16,
    palette: &Palette,
    (spinner, this_year): (Option<Mark>, Option<&str>),
) -> Vec<Spans> {
    let text = Style::default().fg(palette.text);
    let dim = Style::default().fg(palette.overlay0);
    let (mark, word) = kind_mark(card.kind);
    let mut lines = Vec::with_capacity(CARD_LINES);
    let project = clean(&card.project);
    let kind = if project.is_empty() {
        format!(" {word}")
    } else {
        format!(" {word} · {project}")
    };
    // The mark and its word in the kind's colour, the project after them grey. A kind this
    // client does not know stays plain text.
    let kind_style = Style::default().fg(style::kind_role(card.kind, palette));
    let kind = crate::ui::truncate_end(
        &kind,
        usize::from(width.saturating_sub(display_width(mark))),
    );
    let word_end = kind
        .char_indices()
        .nth(word.chars().count() + 1)
        .map_or(kind.len(), |(at, _)| at);
    let (word_part, project_part) = kind.split_at(word_end);
    lines.push(vec![
        (mark.to_owned(), kind_style),
        (word_part.to_owned(), kind_style),
        (project_part.to_owned(), dim),
    ]);
    lines.push(Vec::new());
    let mut title = at_most(wrap(shown_title(&card.title), width), 2, width).into_iter();
    for _ in 0..2 {
        lines.push(
            title
                .next()
                .map(|line| vec![(line, text)])
                .unwrap_or_default(),
        );
    }
    lines.push(progress_line(card, width, spinner.is_some(), palette).unwrap_or_default());
    // Whoever holds it, grey; only a working agent's spinner has a colour.
    let who = Style::default().fg(palette.overlay1);
    lines.push(match (holder(card, tasks, agents), spinner) {
        (Some(Holder::Agent(agent, _)), Some(spinner)) => vec![
            (format!("{} ", spinner.0), Style::default().fg(spinner.1)),
            (
                cut(&clean(&agent_words(agent)), width.saturating_sub(2)),
                who,
            ),
        ],
        (Some(Holder::Agent(agent, _)), None) => {
            vec![(cut(&clean(&agent_words(agent)), width), who)]
        }
        (Some(Holder::You), _) => vec![(cut("You", width), who)],
        (Some(Holder::Named(name) | Holder::Space(name)), _) => {
            vec![(cut(&clean(name), width), who)]
        }
        (None, _) => Vec::new(),
    });
    lines.push(
        phase_status(card, this_year, palette)
            .map(|(status, style)| {
                // The mark stays whole; a narrow card cuts only its words.
                let (mark, words) = status.split_once(' ').unwrap_or((&status, ""));
                vec![(
                    format!("{mark} {}", cut(words, width.saturating_sub(2))),
                    style,
                )]
            })
            .unwrap_or_default(),
    );
    lines
}

/// A card's progress line, `━━━━━━────── 6/10`: its steps done over all of them, the bar yellow
/// while its agent works, red once it is late, green once it is closed, else text; the count
/// grey. None for a card with no steps.
fn progress_line(card: &WorkItem, width: u16, working: bool, palette: &Palette) -> Option<Spans> {
    let total = card.steps.len();
    if total == 0 {
        return None;
    }
    let done = card.steps.iter().filter(|step| step.done).count();
    let count = format!(" {done}/{total}");
    let bar = usize::from(width.saturating_sub(display_width(&count)));
    let filled = bar * done / total;
    let fill = if working {
        palette.yellow
    } else if card.due_level == Some(DueLevel::Overdue) {
        palette.red
    } else if card.status == "closed" {
        palette.green
    } else {
        palette.text
    };
    Some(vec![
        ("━".repeat(filled), Style::default().fg(fill)),
        (
            "─".repeat(bar - filled),
            Style::default().fg(palette.surface1),
        ),
        (count, Style::default().fg(palette.overlay1)),
    ])
}

fn put_spans(buffer: &mut Buffer, x: u16, y: u16, end: u16, spans: &Spans, marks: Marks) {
    let mut x = x;
    for (text, style) in spans {
        x = marks.put(buffer, x, y, end, text, *style);
    }
}

/// A card as a column draws it: its lines, and whether it is the selected card.
struct Block<'a> {
    card: &'a WorkItem,
    lines: Vec<Spans>,
    selected: bool,
    /// Whether its who line starts with the spinner.
    spins: bool,
}

#[allow(clippy::too_many_arguments)] // Its place, the board and its list, and the three looks' inputs.
pub(super) fn render_work<'a>(
    buffer: &mut Buffer,
    body: Rect,
    board: &TaskBoard,
    list: &'a TaskList,
    home_dir: Option<&str>,
    (agents, worker_mark): (&[BoardAgent], Mark),
    palette: &Palette,
    hits: &mut TaskBoardHits,
) {
    let grey = Style::default().fg(palette.overlay1);
    let words = super::filter::words(&board.filter);
    let marks = Marks {
        words: &words,
        style: style::mark(palette),
    };
    let scope = Scope::of(board, agents);
    // An agent shown alone with no card still gets its empty columns.
    let spaced = Scope {
        agent: None,
        ..scope
    };
    if columns(list, spaced, &[]).iter().all(Vec::is_empty) {
        put_fitted(
            buffer,
            body.x.saturating_add(spacing::MARGIN),
            body.y,
            body.right(),
            EMPTY,
            grey,
        );
        return;
    }
    // The cards the filter's words and the Filter menu's picks keep; with either, each
    // stage's count says of how many.
    let picked = |cards: Vec<&'a WorkItem>| -> Vec<&'a WorkItem> {
        cards
            .into_iter()
            .filter(|card| board.work_filter.keeps(card, (&list.tasks, agents)))
            .collect()
    };
    let shown = columns(list, scope, &words).map(picked);
    let unfiltered = (!words.is_empty() || !board.work_filter.is_empty())
        .then(|| columns(list, scope, &[]).map(|cards| cards.len()));
    hits.work_columns = shown
        .iter()
        .map(|cards| cards.iter().map(|card| card.file.clone()).collect())
        .collect();
    if body.height <= HEADING_ROWS {
        return;
    }
    let selected = selected_card(board, list, scope, &words);
    // The stage a one-column board shows: the selected card's, else the first with cards.
    let picked = selected
        .and_then(|card| {
            shown
                .iter()
                .position(|cards| cards.iter().any(|c| c.file == card.file))
        })
        .or_else(|| shown.iter().position(|cards| !cards.is_empty()))
        .unwrap_or(0);
    // The selected card's panel, until it is closed, under the columns.
    let place = selected
        .filter(|_| !board.work_panel_closed)
        .and_then(|_| panel_place(body));
    let columns_area = place.as_ref().map_or(body, |place| place.columns);
    let columns_bottom = columns_area.bottom();
    let layout = ColumnLayout::fit(columns_area, picked);

    let mut label_ends = [0u16; 5];
    for slot in &layout.slots {
        let (_, heading) = STAGES[slot.stage];
        let end = slot.x + slot.width;
        // Filtered, `1 of 3`, or only `1` where the whole count would be cut.
        let count = match &unfiltered {
            Some(all)
                if display_width(&format!(
                    " {heading}  {} of {}",
                    shown[slot.stage].len(),
                    all[slot.stage]
                )) <= slot.width =>
            {
                format!("{} of {}", shown[slot.stage].len(), all[slot.stage])
            }
            _ => shown[slot.stage].len().to_string(),
        };
        let role = stage_role(slot.stage, palette);
        let label = style::label(role, palette);
        // Its colour's underline across the column, under its label.
        put_segment(
            buffer,
            slot.x,
            body.y + 1,
            end,
            &"━".repeat(usize::from(slot.width)),
            Style::default().fg(role),
        );
        if layout.picker {
            // `◂ BUILD  1 ▸`: each arrow steps to the stage beside, as ← and → do.
            let arrow = Style::default().fg(palette.overlay1);
            let x = put_segment(buffer, slot.x, body.y, end, "◂ ", arrow);
            let x = put_segment(
                buffer,
                x,
                body.y,
                end,
                &format!(" {heading}  {count} "),
                label,
            );
            let right = put_segment(buffer, x, body.y, end, " ▸", arrow);
            hits.work_stage_arrows = vec![
                (Rect::new(slot.x, body.y, 1, 1), false),
                (Rect::new(right.saturating_sub(1), body.y, 1, 1), true),
            ];
            label_ends[slot.stage] = right;
        } else {
            let text = format!(" {heading}  {count}");
            let padded = format!("{text:<width$}", width = usize::from(slot.width));
            put_fitted(buffer, slot.x, body.y, end, &padded, label);
            label_ends[slot.stage] = slot.x + display_width(&text);
        }
    }

    // The cards stack from under their label, with no row between them: their boxes part
    // them. A column taller than its rows scrolls; a card is never made smaller.
    let top = body.y + HEADING_ROWS;
    let rows = usize::from(columns_bottom.saturating_sub(top));
    // A column not drawn keeps its scroll, and takes no wheel.
    hits.work_column_areas = vec![Rect::default(); STAGES.len()];
    hits.work_scroll = board.work_scroll;
    for slot in &layout.slots {
        let (index, x, column_width) = (slot.stage, slot.x, slot.width);
        hits.work_column_areas[index] = Rect::new(x, top, column_width, rows as u16);
        let blocks: Vec<Block> = shown[index]
            .iter()
            .map(|card| {
                let spinner = card_agent(card, &list.tasks)
                    .is_some_and(|(agent, space)| super::tag_works(Some(agent), space, agents))
                    .then_some((worker_mark.0, palette.yellow));
                Block {
                    card,
                    lines: card_lines(
                        card,
                        (&list.tasks, agents),
                        column_width.saturating_sub(2 + 2 * spacing::PAD).max(1),
                        palette,
                        (spinner, list.generated.get(..4)),
                    ),
                    selected: selected.is_some_and(|selected| selected.file == card.file),
                    // Only a card's own agent spins, and it always has a who line.
                    spins: spinner.is_some(),
                }
            })
            .collect();
        draw_column(
            buffer,
            ColumnPlace {
                index,
                x,
                top,
                width: column_width,
                rows,
                label_end: label_ends[index],
                label_style: style::label(stage_role(index, palette), palette),
            },
            &blocks,
            board,
            palette,
            marks,
            hits,
        );
    }

    if let (Some(card), Some(place)) = (selected, place) {
        render_panel(
            buffer,
            PanelDraw {
                rect: place.rect,
                home_dir,
                agents,
                worker_mark,
            },
            card,
            list,
            board,
            palette,
            hits,
        );
    }
}

/// Where a stage column draws: its stage's index, its first column, its top row, its width
/// and its rows; and its label (on the row above), where its text ends and its look.
struct ColumnPlace {
    index: usize,
    x: u16,
    top: u16,
    width: u16,
    rows: usize,
    label_end: u16,
    label_style: Style,
}

/// A column's cards from its scroll on, as many whole cards as its rows hold. Its label says
/// how many are out of sight, `▲ N` above once scrolled and `▼ N` below, at its right end:
/// a marker never takes a card's row. The selected card is scrolled into view when the board
/// asks. A column with no cards says `no items`, dim, where a card's first line would be.
fn draw_column(
    buffer: &mut Buffer,
    place: ColumnPlace,
    blocks: &[Block],
    board: &TaskBoard,
    palette: &Palette,
    marks: Marks,
    hits: &mut TaskBoardHits,
) {
    let ColumnPlace {
        index,
        x,
        top,
        width,
        rows,
        label_end,
        label_style,
    } = place;
    if blocks.is_empty() {
        hits.work_scroll[index] = 0;
        let row = top + 1;
        if usize::from(row - top) < rows {
            put_fitted(
                buffer,
                x + 1 + spacing::PAD,
                row,
                (x + width).saturating_sub(1 + spacing::PAD),
                NO_ITEMS,
                Style::default().fg(palette.overlay1),
            );
        }
        return;
    }
    // Whole cards only: a card is never cut.
    let fits = rows / CARD_HEIGHT;
    let mut first = board.work_scroll[index].min(blocks.len().saturating_sub(fits.max(1)));
    if board.reveal_selected {
        if let Some(selected) = blocks.iter().position(|block| block.selected) {
            first = first.min(selected).max((selected + 1).saturating_sub(fits));
        }
    }
    hits.work_scroll[index] = first;
    for (offset, block) in blocks.iter().skip(first).take(fits).enumerate() {
        let y = top + (offset * CARD_HEIGHT) as u16;
        draw_card(buffer, x, y, width, block, palette, marks);
        hits.spins |= block.spins;
        hits.cards.push((
            Rect::new(x, y, width, CARD_HEIGHT as u16),
            block.card.file.clone(),
        ));
    }
    // `▲ N  ▼ N` at the label's right end, each a click that pages its way: the page above
    // ends at the card before the first shown, the page below starts after the last.
    let below = blocks.len().saturating_sub(first + fits);
    let markers: Vec<(String, bool, usize)> = [
        (first > 0).then(|| (format!("▲ {first}"), false, first.saturating_sub(fits))),
        (below > 0).then(|| (format!("▼ {below}"), true, first + fits)),
    ]
    .into_iter()
    .flatten()
    .collect();
    let label_y = top.saturating_sub(HEADING_ROWS);
    let mut end = x + width - spacing::PAD;
    for (text, down, target) in markers.into_iter().rev() {
        let start = end.saturating_sub(display_width(&text));
        if start < label_end + spacing::BUTTON_GAP {
            break;
        }
        put_segment(buffer, start, label_y, end, &text, label_style);
        hits.work_markers.push(super::WorkMarker {
            rect: Rect::new(start, label_y, end - start, 1),
            column: index,
            down,
            first: target,
        });
        end = start.saturating_sub(spacing::BUTTON_GAP);
    }
}

/// A stage column is never narrower than this; a main area too narrow for five of them
/// shows one, with a stage picker.
const COLUMN_MIN_WIDTH: u16 = 14;

/// A drawn stage column: its stage's index, its first column and its width.
struct Slot {
    stage: usize,
    x: u16,
    width: u16,
}

/// Where the Work columns sit at the main area's width: all five stages, empty or not, sharing
/// the width inside the margins equally (the columns left over go one each to the first
/// columns); or, when that leaves a column under `COLUMN_MIN_WIDTH`, one column, the picked
/// stage's, with a picker.
struct ColumnLayout {
    slots: Vec<Slot>,
    picker: bool,
}

impl ColumnLayout {
    fn fit(body: Rect, picked: usize) -> Self {
        let inner = body.width.saturating_sub(2 * spacing::MARGIN);
        let left = body.x + spacing::MARGIN;
        let count = STAGES.len() as u16;
        let room = inner.saturating_sub(COLUMN_GAP * (count - 1));
        let (width, extra) = (room / count, room % count);
        if width < COLUMN_MIN_WIDTH {
            let slot = Slot {
                stage: picked,
                x: left,
                width: inner,
            };
            return Self {
                slots: vec![slot],
                picker: true,
            };
        }
        let mut x = left;
        let slots = (0..STAGES.len())
            .map(|stage| {
                let width = width + u16::from((stage as u16) < extra);
                let slot = Slot { stage, x, width };
                x += width + COLUMN_GAP;
                slot
            })
            .collect();
        Self {
            slots,
            picker: false,
        }
    }
}

/// A card's box with rounded corners, its lines a space inside each side; the accent marks
/// the selected card, the grid's lines the rest.
fn draw_card(
    buffer: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    block: &Block,
    palette: &Palette,
    marks: Marks,
) {
    let end = x + width;
    let border = Style::default().fg(if block.selected {
        palette.accent
    } else {
        palette.overlay0
    });
    let across = "─".repeat(usize::from(width.saturating_sub(2)));
    put_segment(buffer, x, y, end, &format!("╭{across}╮"), border);
    for (offset, line) in block.lines.iter().enumerate() {
        let row = y + 1 + offset as u16;
        if offset == RULE_LINE {
            put_segment(buffer, x, row, x + 1, "├", border);
            put_segment(
                buffer,
                x + 1,
                row,
                end - 1,
                &across,
                Style::default().fg(palette.overlay0),
            );
            put_segment(buffer, end - 1, row, end, "┤", border);
            continue;
        }
        put_segment(buffer, x, row, x + 1, "│", border);
        put_spans(
            buffer,
            x + 1 + spacing::PAD,
            row,
            end.saturating_sub(1 + spacing::PAD),
            line,
            marks,
        );
        put_segment(buffer, end - 1, row, end, "│", border);
    }
    let bottom = y + 1 + block.lines.len() as u16;
    put_segment(buffer, x, bottom, end, &format!("╰{across}╯"), border);
}

/// A kind's word as the panel's heading tag says it: `Feature`, `Bug`, `Chore`.
pub(super) fn kind_title(kind: WorkKind) -> &'static str {
    match kind {
        WorkKind::Feature => "Feature",
        WorkKind::Bug => "Bug",
        WorkKind::Chore => "Chore",
        WorkKind::Unknown => "Work",
    }
}

/// Where the card panel draws: its box, the home folder its file shows under, and the live
/// agents, one of which may be on the card.
struct PanelDraw<'a> {
    rect: Rect,
    home_dir: Option<&'a str>,
    agents: &'a [BoardAgent],
    worker_mark: Mark,
}

/// The selected card in full, in a box: a heading bar (its kind, its title, where it lives,
/// and a ` × ` that closes the panel), a rule, its steps and its space's tasks under small
/// labels, its file and branch, and its keys on the last line, with a count of the lines
/// scrolled out of sight beside them.
fn render_panel(
    buffer: &mut Buffer,
    draw: PanelDraw,
    card: &WorkItem,
    list: &TaskList,
    board: &TaskBoard,
    palette: &Palette,
    hits: &mut TaskBoardHits,
) {
    let PanelDraw {
        rect,
        home_dir,
        agents,
        worker_mark,
    } = draw;
    let border = Style::default().fg(palette.overlay0);
    let dim = Style::default().fg(palette.overlay0);
    let (x, y, right) = (rect.x, rect.y, rect.right());
    let across = "─".repeat(usize::from(rect.width.saturating_sub(2)));
    let last = rect.bottom() - 1;
    put_segment(buffer, x, y, right, &format!("╭{across}╮"), border);
    put_segment(buffer, x, y + 2, right, &format!("├{across}┤"), border);
    put_segment(buffer, x, last, right, &format!("╰{across}╯"), border);
    for row in [y + 1].into_iter().chain(y + 3..last) {
        put_segment(buffer, x, row, x + 1, "│", border);
        put_segment(buffer, right - 1, row, right, "│", border);
    }
    let inner_x = x + 1 + spacing::PAD;
    let inner_end = right.saturating_sub(1 + spacing::PAD);
    let width = panel_inner_width(rect.width);
    // A panel too short for its foot keeps the branch and its commits in the heading.
    let foot = rect.height >= PANEL_FOOT_MIN_ROWS;
    let heading = (inner_x, y + 1, inner_end);
    let stage = stage_status(card, list.generated.get(..4), palette);
    let title_cut = put_panel_heading(buffer, heading, card, stage, !foot, palette, hits);

    // The lines between the rule and the file and branch, from the panel's scroll: the
    // selected step pulls it into view. A blank line parts them from the keys, unless it would
    // leave no line above it.
    let content_top = y + 3;
    let buttons_y = last - 1;
    let gap = if foot || buttons_y > content_top + 1 {
        1
    } else {
        0
    };
    let content_bottom = buttons_y.saturating_sub(if foot { 2 } else { 0 } + gap);
    let content_rows = usize::from(content_bottom.saturating_sub(content_top));
    let numbers = super::render::shown_numbers(board, list, agents);
    // The card's agent at work spins on the current step and says so.
    let working = card_agent(card, &list.tasks)
        .filter(|(agent, space)| super::tag_works(Some(agent), space, agents))
        .map(|(agent, _)| {
            let words = match agent.minutes {
                Some(minutes) => format!("{} working on it · {minutes}m", clean(&agent.name)),
                None => format!("{} working on it", clean(&agent.name)),
            };
            (worker_mark, words)
        });
    let body = panel_lines(
        card,
        (list, agents, &numbers),
        width,
        title_cut,
        (board.work_step, working),
        palette,
    );
    let max_scroll = body.lines.len().saturating_sub(content_rows);
    let mut scroll = board.work_panel_scroll.min(max_scroll);
    if let Some(lines) = board.work_step.and_then(|step| body.steps.get(step)) {
        if lines.start < scroll {
            scroll = lines.start;
        } else if lines.end > scroll + content_rows {
            scroll = lines.end.saturating_sub(content_rows).min(lines.start);
        }
    }
    hits.work_panel = rect;
    hits.work_panel_max_scroll = max_scroll;
    hits.work_panel_scroll = scroll;
    hits.work_step_count = body.steps.len();
    let band = Style::default().bg(palette.selection_bg);
    for (offset, line) in body
        .lines
        .iter()
        .skip(scroll)
        .take(content_rows)
        .enumerate()
    {
        let row = content_top + offset as u16;
        let index = scroll + offset;
        let step = body.steps.iter().position(|lines| lines.contains(&index));
        if step.is_some() && step == board.work_step {
            buffer.set_style(Rect::new(x + 1, row, rect.width.saturating_sub(2), 1), band);
        }
        if let Some(step) = step {
            hits.work_step_rows
                .push((Rect::new(x + 1, row, rect.width.saturating_sub(2), 1), step));
        }
        put_spans(
            buffer,
            inner_x,
            row,
            inner_end,
            line,
            Marks {
                words: &[],
                style: Style::default(),
            },
        );
    }
    if foot {
        let file = home_path(&card.file, home_dir);
        let text = Style::default().fg(palette.text);
        // Labelled as the fields are, so the file and branch line up under their values.
        let label = |word: &str| {
            (
                format!("{word:<FIELD_LABEL_WIDTH$}"),
                style::field_label(palette),
            )
        };
        let value_width = width.saturating_sub(FIELD_LABEL_WIDTH as u16);
        let file_line = vec![label("Spec"), (cut(&file, value_width), text)];
        put_spans(
            buffer,
            inner_x,
            buttons_y - 3,
            inner_end,
            &file_line,
            Marks {
                words: &[],
                style: Style::default(),
            },
        );
        // Commits with no branch named say so without the `branch` label.
        let branch_line = match card.branch.as_deref().filter(|branch| !branch.is_empty()) {
            Some(_) => vec![
                label("Branch"),
                (cut(&branch_text(card), value_width), text),
            ],
            None => vec![(cut(&branch_text(card), width), text)],
        };
        put_spans(
            buffer,
            inner_x,
            buttons_y - 2,
            inner_end,
            &branch_line,
            Marks {
                words: &[],
                style: Style::default(),
            },
        );
    }

    let buttons = panel_buttons(board, card);
    let buttons_end = if hints_width(&buttons) <= width {
        put_hints(
            buffer,
            inner_x,
            buttons_y,
            inner_end,
            &buttons,
            palette,
            &mut hits.footer_keys,
        )
    } else {
        inner_x
    };
    // What is out of sight, in words where they fit beside the buttons, else in marks.
    let shown = scroll..scroll + content_rows;
    let note = scroll_note(&body.items, &shown).and_then(|[full, short]| {
        [full, short]
            .into_iter()
            .find(|note| buttons_end + spacing::HINT_GAP + display_width(note) <= inner_end)
    });
    if let Some(note) = note {
        put_segment(
            buffer,
            inner_end - display_width(&note),
            buttons_y,
            inner_end,
            &note,
            dim,
        );
    }
}

/// The panel's branch line: the branch, how many commits, and the day of the last one.
pub(super) fn branch_text(card: &WorkItem) -> String {
    let mut parts: Vec<String> = card
        .branch
        .as_deref()
        .filter(|branch| !branch.is_empty())
        .map(clean)
        .into_iter()
        .collect();
    parts.push(match card.commits {
        1 => "1 commit".to_owned(),
        commits => format!("{commits} commits"),
    });
    if let Some(day) = card
        .last_commit
        .as_deref()
        .and_then(super::table::short_date)
    {
        parts.push(format!("last commit {day}"));
    }
    parts.join(" · ")
}

/// The least a title keeps of the heading bar before where the card lives gives way.
const HEADING_TITLE_MIN: u16 = 20;

/// The panel's heading bar from `x` to `end`: the kind as a tag, the title bold, its `stage`
/// (see [`stage_status`]) after it, and where the card lives dim at the right before the ` × `
/// that closes the panel, with its branch and commits when `with_branch` (a panel too short for
/// its branch line). A title too long for the room left is cut with `…`; where the card lives,
/// then the stage, give way when the title would keep less than `HEADING_TITLE_MIN`; the ×
/// never does. Returns whether the title was cut, so the panel shows it whole.
fn put_panel_heading(
    buffer: &mut Buffer,
    (x, y, end): (u16, u16, u16),
    card: &WorkItem,
    stage: Spans,
    with_branch: bool,
    palette: &Palette,
    hits: &mut TaskBoardHits,
) -> bool {
    let tag = style::key(palette);
    let (mark, _) = kind_mark(card.kind);
    let at = put_segment(buffer, x, y, end, " ", tag);
    let at = put_segment(buffer, at, y, end, mark, tag.fg(palette.text));
    let at = put_segment(
        buffer,
        at,
        y,
        end,
        &format!(" {} ", kind_title(card.kind)),
        tag,
    );
    let close_x = end.saturating_sub(CLOSE_WIDTH);
    put_segment(buffer, close_x, y, end, " × ", style::key(palette));
    hits.panel_close = Rect::new(close_x, y, CLOSE_WIDTH, 1);

    // The branch and its commits are on the panel's branch line, while it has one.
    let mut place = vec![clean(&card.project)];
    if let Some(agent) = &card.agent {
        place.push(match agent.minutes {
            Some(minutes) => format!("◐ {} · {minutes}m", clean(&agent.name)),
            None => format!("◐ {}", clean(&agent.name)),
        });
    }
    if with_branch {
        place.push(branch_text(card));
    }
    let place = place.join(" · ");
    let title_x = at + spacing::GROUP_GAP;
    let place_end = close_x.saturating_sub(spacing::BUTTON_GAP);
    let title = clean(shown_title(&card.title));
    let place_x = place_end.saturating_sub(display_width(&place));
    let title_end = if title_x + HEADING_TITLE_MIN + spacing::GROUP_GAP <= place_x {
        put_segment(
            buffer,
            place_x,
            y,
            place_end,
            &place,
            Style::default().fg(palette.overlay0),
        );
        place_x.saturating_sub(spacing::GROUP_GAP)
    } else {
        place_end
    };
    let stage_width: u16 = stage.iter().map(|(text, _)| display_width(text)).sum();
    let title_room = title_end.saturating_sub(title_x);
    let title_room = match title_room.checked_sub(stage_width + spacing::GROUP_GAP) {
        Some(room) if stage_width > 0 && room >= HEADING_TITLE_MIN => {
            let stage_x = title_x + display_width(&title).min(room) + spacing::GROUP_GAP;
            let marks = Marks {
                words: &[],
                style: Style::default(),
            };
            put_spans(buffer, stage_x, y, title_end, &stage, marks);
            room
        }
        _ => title_room,
    };
    put_fitted(
        buffer,
        title_x,
        y,
        title_x + title_room,
        &title,
        panel_title(palette),
    );
    display_width(&title) > title_room
}

/// The card's stage as its column heads it, bold in its colour, and its phase status as the
/// card says it: `BUILD · Overdue 3d`. None for a stage this client does not know.
fn stage_status(card: &WorkItem, this_year: Option<&str>, palette: &Palette) -> Spans {
    let Some((index, (_, label))) = STAGES
        .iter()
        .enumerate()
        .find(|(_, (stage, _))| *stage == card.stage)
    else {
        return Vec::new();
    };
    let mut spans = vec![(
        (*label).to_owned(),
        Style::default()
            .fg(stage_role(index, palette))
            .add_modifier(Modifier::BOLD),
    )];
    if let Some(status) = phase_status(card, this_year, palette) {
        spans.push((" · ".to_owned(), Style::default().fg(palette.overlay0)));
        spans.push(status);
    }
    spans
}

/// A card's title as the panel shows it: bold, in the text colour.
fn panel_title(palette: &Palette) -> Style {
    Style::default()
        .fg(palette.text)
        .add_modifier(Modifier::BOLD)
}

/// What a panel item is: one of the card's steps, or one of its space's tasks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    Step,
    Task,
}

/// `text` wrapped after `lead` (a mark and a number) as a list item: the first line starts
/// with `lead`, and the rest start under the first line's text, never under the lead.
fn list_item(lead: Spans, text: &str, style: Style, width: u16) -> Vec<Spans> {
    let indent: u16 = lead.iter().map(|(text, _)| display_width(text)).sum();
    let mut lead = Some(lead);
    wrap(text, width.saturating_sub(indent))
        .into_iter()
        .map(|line| match lead.take() {
            Some(mut first) => {
                first.push((line, style));
                first
            }
            None => vec![(format!("{}{line}", " ".repeat(usize::from(indent))), style)],
        })
        .collect()
}

/// The panel's lines under its rule, each item's lines for the count of those out of sight,
/// and each step's lines, open steps first.
struct PanelBody {
    lines: Vec<Spans>,
    items: Vec<(Range<usize>, Item)>,
    steps: Vec<Range<usize>>,
}

/// The panel's lines under its rule: the whole title when the heading cut it, then the card's
/// fields (see [`field_lines`]), then `STEPS  N of M done` and the steps, done ones first, then
/// the current one (the first undone) bold, then the rest, each its title (see [`step_title`])
/// on one line cut at the panel's width but the `selected_step`, whole; while the card's agent
/// is `working` (its spinner mark and words), the current step spins and ends with the words;
/// then its linked tasks, the tasks
/// whose `card` is its slug (`LINKED TASKS  N`), or for a card without a slug `TASKS  N` and
/// the tasks of its space and project; a task numbered as its row among the Tasks view's
/// `shown` rows.
fn panel_lines(
    card: &WorkItem,
    (list, agents, numbers): (&TaskList, &[BoardAgent], &super::render::RowNumbers),
    width: u16,
    whole_title: bool,
    (selected_step, working): (Option<usize>, Option<(Mark, String)>),
    palette: &Palette,
) -> PanelBody {
    let text = Style::default().fg(palette.text);
    let dim = Style::default().fg(palette.overlay0);
    let label = style::field_label(palette);
    let mut body = PanelBody {
        lines: Vec::new(),
        items: Vec::new(),
        steps: Vec::new(),
    };
    if whole_title {
        body.lines.extend(
            wrap(shown_title(&card.title), width)
                .into_iter()
                .map(|line| vec![(line, panel_title(palette))]),
        );
        body.lines.push(Vec::new());
    }
    let linked: Vec<&Task> = match card.slug.as_deref() {
        Some(slug) => list
            .tasks
            .iter()
            .filter(|task| task.card.as_deref() == Some(slug))
            .collect(),
        None => Vec::new(),
    };
    let fields = field_lines(card, (list, agents), &linked, width, palette);
    if !fields.is_empty() {
        body.lines.extend(fields);
        body.lines.push(Vec::new());
    }
    if !card.steps.is_empty() {
        let done = card.steps.iter().filter(|step| step.done).count();
        body.lines.push(vec![
            ("STEPS".to_owned(), label),
            (format!("  {done} of {} done", card.steps.len()), text),
        ]);
        let current = card.steps.iter().position(|step| !step.done);
        let done_steps = card.steps.iter().enumerate().filter(|(_, step)| step.done);
        let undone = card.steps.iter().enumerate().filter(|(_, step)| !step.done);
        for (index, step) in done_steps.chain(undone) {
            // Done ticks teal, as an agent that is done; the words read the same either way.
            let (mark, style, tail) = if step.done {
                let mark = (DONE_STEP_MARK.to_owned(), Style::default().fg(palette.teal));
                (mark, text, None)
            } else if current == Some(index) {
                let bold = text.add_modifier(Modifier::BOLD);
                match &working {
                    Some(((spinner, colour), words)) => {
                        let colour = Style::default().fg(*colour);
                        let mark = (format!("{spinner} "), colour);
                        (mark, bold, Some((format!("   {words}"), colour)))
                    }
                    None => ((CURRENT_STEP_MARK.to_owned(), text), bold, None),
                }
            } else {
                ((STEP_MARK.to_owned(), text), text, None)
            };
            let tail_width = tail.as_ref().map_or(0, |(words, _)| display_width(words));
            let room = width.saturating_sub(tail_width);
            // One line cut at the panel's width; the selected step whole, wrapped under its
            // text.
            let mut lines = if selected_step == Some(body.steps.len()) {
                list_item(vec![mark], step_title(step), style, room)
            } else {
                let indent = display_width(&mark.0);
                vec![vec![
                    mark,
                    (cut(step_title(step), room.saturating_sub(indent)), style),
                ]]
            };
            if let (Some(tail), Some(first)) = (tail, lines.first_mut()) {
                first.push(tail);
            }
            let start = body.lines.len();
            body.lines.extend(lines);
            body.items.push((start..body.lines.len(), Item::Step));
            body.steps.push(start..body.lines.len());
        }
    }
    let (tasks, heading) = if card.slug.is_some() {
        (linked, "LINKED TASKS")
    } else {
        let tasks = list
            .tasks
            .iter()
            .filter(|task| task.space == card.space && task.project == card.project)
            .collect();
        (tasks, "TASKS")
    };
    if !tasks.is_empty() {
        if !body.lines.last().is_none_or(Vec::is_empty) {
            body.lines.push(Vec::new());
        }
        body.lines.push(vec![
            (heading.to_owned(), label),
            (format!("  {}", tasks.len()), text),
        ]);
        for task in tasks {
            // Numbered as its Tasks row is; `·` when the Tasks view has no row for it.
            let lead = vec![match numbers.of_task(task) {
                Some(number) => (format!("#{number}  "), text),
                None => ("·  ".to_owned(), dim),
            }];
            let words = format!("{} ({})", task.task, group_word(task.group));
            let start = body.lines.len();
            body.lines.extend(list_item(lead, &words, text, width));
            body.items.push((start..body.lines.len(), Item::Task));
        }
    }
    if body.lines.is_empty() {
        let grey = Style::default().fg(palette.overlay1);
        body.lines
            .push(vec![("No steps or tasks yet.".to_owned(), grey)]);
    }
    body
}

/// The width of the panel's field labels, the longest (`Verification`) and two spaces.
const FIELD_LABEL_WIDTH: usize = 14;

/// The card's fields, each under its label and only when it has a value: `Objective`, `Status`
/// (the card's phase status, drawn as on the card), `Owner` (its working agent and what it is
/// doing, else the Owner of its first open linked task), `Blocked by` (each open linked task
/// that waits on you, `You · pick a time`), its phase's own field (PLAN `Approval`, BUILD
/// `Current step`, REVIEW `Findings`, VERIFY `Verification`, SHIP `Release`), and `Timeline`
/// (`started 26 Sep · due Thu · last activity 29 Sep`). A value wraps under itself.
fn field_lines(
    card: &WorkItem,
    (list, agents): (&TaskList, &[BoardAgent]),
    linked: &[&Task],
    width: u16,
    palette: &Palette,
) -> Vec<Spans> {
    let text = Style::default().fg(palette.text);
    let dim = Style::default().fg(palette.overlay0);
    let this_year = list.generated.get(..4);
    let mut lines = Vec::new();
    let label_style = style::field_label(palette);
    let mut field = |label: &str, value: Spans, words: &str, style: Style| {
        let mut lead = vec![(format!("{label:<FIELD_LABEL_WIDTH$}"), label_style)];
        lead.extend(value);
        let start = lines.len();
        for (index, line) in words
            .lines()
            .filter(|line| !line.trim().is_empty())
            .enumerate()
        {
            let lead = if index == 0 {
                lead.clone()
            } else {
                vec![(" ".repeat(FIELD_LABEL_WIDTH), dim)]
            };
            lines.extend(list_item(lead, &clean(line), style, width));
        }
        // A value with no words (a finding's) still shows its label and lead.
        if lines.len() == start {
            lines.push(lead);
        }
    };
    if let Some(objective) = &card.objective {
        field("Objective", Vec::new(), objective, text);
    }
    if let Some((status, style)) = phase_status(card, this_year, palette) {
        field("Status", Vec::new(), &status, style);
    }
    let open: Vec<&Task> = linked
        .iter()
        .copied()
        .filter(|task| task.group != TaskGroup::Done)
        .collect();
    let owner = holder(card, &list.tasks, agents).map(|holder| match holder {
        Holder::Agent(named, space) => match super::doing_of(named, space, agents) {
            Some(doing) => format!("{}: {doing}", named.name),
            None => named.name.clone(),
        },
        Holder::You => "You".to_owned(),
        Holder::Named(name) | Holder::Space(name) => name.to_owned(),
    });
    if let Some(owner) = owner {
        field(
            "Owner",
            Vec::new(),
            &owner,
            Style::default().fg(palette.overlay1),
        );
    }
    let yours = open.iter().filter(|task| {
        task.owner
            .as_deref()
            .is_some_and(|owner| owner.trim().eq_ignore_ascii_case("you"))
    });
    for (index, task) in yours.enumerate() {
        let action = task
            .action
            .as_deref()
            .filter(|action| !action.trim().is_empty())
            .unwrap_or(&task.task);
        let label = if index == 0 { "Blocked by" } else { "" };
        field(label, Vec::new(), &format!("You · {action}"), text);
    }
    match card.stage {
        WorkStage::Plan => {
            if let Some(approval) = &card.approval {
                field("Approval", Vec::new(), approval, text);
            }
        }
        WorkStage::Build => {
            if let Some(step) = card.steps.iter().find(|step| !step.done) {
                field("Current step", Vec::new(), step_title(step), text);
            }
        }
        WorkStage::Review => {
            let open = card.findings.iter().filter(|finding| finding.open);
            let fixed = card.findings.iter().filter(|finding| !finding.open);
            for (index, finding) in open.chain(fixed).enumerate() {
                let label = if index == 0 { "Findings" } else { "" };
                let severity = finding
                    .severity
                    .as_deref()
                    .map(|severity| severity.trim().to_uppercase());
                let colour = match severity.as_deref() {
                    _ if !finding.open => dim,
                    Some("HIGH") => Style::default().fg(palette.red),
                    // Peach, a warning: yellow is only for what waits on you.
                    Some("MEDIUM") => Style::default().fg(palette.peach),
                    _ => dim,
                };
                let word = severity
                    .map(|severity| vec![(format!("{} ", clean(&severity)), colour)])
                    .unwrap_or_default();
                let style = if finding.open { text } else { dim };
                field(label, word, &finding.text, style);
            }
        }
        WorkStage::Check => {
            if let Some(verification) = &card.verification {
                field("Verification", Vec::new(), verification, text);
            }
        }
        WorkStage::Shipped => {
            if let Some(release) = &card.release {
                field("Release", Vec::new(), release, text);
            }
        }
        WorkStage::Unknown => {}
    }
    let started = card
        .started
        .as_deref()
        .and_then(|day| day_month(day, this_year))
        .map(|day| format!("started {day}"));
    let due = card.due_text.as_deref().map(|due| due_words(due, "due"));
    let active = card
        .last_activity
        .as_deref()
        .and_then(|day| day_month(day, this_year))
        .map(|day| format!("last activity {day}"));
    let timeline: Vec<String> = [started, due, active].into_iter().flatten().collect();
    if !timeline.is_empty() {
        field("Timeline", Vec::new(), &timeline.join(" · "), text);
    }
    lines
}

/// A step as the panel names it: its short text, else its whole line. `o` opens the file
/// with the whole step.
fn step_title(step: &WorkStep) -> &str {
    if step.text.trim().is_empty() {
        step.full.trim()
    } else {
        step.text.trim()
    }
}

/// A step's mark before its text, `○ ` open, `▸ ` the current one and `✓ ` done, never a box
/// to tick: its width is the hanging indent of its other lines. `✓` is the done mark in every
/// icon style, as the tree's is.
const STEP_MARK: &str = "○ ";
const CURRENT_STEP_MARK: &str = "▸ ";
const DONE_STEP_MARK: &str = "✓ ";

/// What the panel's last line says of the items out of sight, with `shown` the lines on
/// screen, in words and in marks: `▲ 2 above · ▼ 16 more steps and 2 tasks · scroll` and
/// `▲ 2  ▼ 18`; none when all show.
fn scroll_note(items: &[(Range<usize>, Item)], shown: &Range<usize>) -> Option<[String; 2]> {
    let above = items
        .iter()
        .filter(|(lines, _)| lines.start < shown.start)
        .count();
    let below = |kind: Item| {
        items
            .iter()
            .filter(|(lines, item)| *item == kind && lines.end > shown.end)
            .count()
    };
    let mut parts = Vec::new();
    if above > 0 {
        parts.push(format!("▲ {above} above"));
    }
    match (below(Item::Step), below(Item::Task)) {
        (0, 0) => {}
        (steps, 0) => parts.push(format!("▼ {steps} more {}", plural_word(steps, "step"))),
        (0, tasks) => parts.push(format!("▼ {tasks} more {}", plural_word(tasks, "task"))),
        (steps, tasks) => parts.push(format!(
            "▼ {steps} more {} and {tasks} {}",
            plural_word(steps, "step"),
            plural_word(tasks, "task")
        )),
    }
    let below = below(Item::Step) + below(Item::Task);
    let marks: Vec<String> = [(above, "▲"), (below, "▼")]
        .into_iter()
        .filter(|(count, _)| *count > 0)
        .map(|(count, mark)| format!("{mark} {count}"))
        .collect();
    (!parts.is_empty()).then(|| {
        [
            format!("{} · scroll", parts.join(" · ")),
            marks.join(&spacing::gap(spacing::BUTTON_GAP)),
        ]
    })
}

/// `one`, or `ones` when `count` is not one.
fn plural_word(count: usize, one: &str) -> String {
    if count == 1 {
        one.to_owned()
    } else {
        format!("{one}s")
    }
}

fn group_word(group: TaskGroup) -> &'static str {
    match group {
        TaskGroup::You => "you",
        TaskGroup::Claude => "claude",
        TaskGroup::Waiting | TaskGroup::Unknown => "waiting",
        TaskGroup::Done => "done",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stage_this_client_does_not_know_has_no_status_even_blocked() {
        let palette = Palette::catppuccin();
        let card = WorkItem {
            stage: WorkStage::Unknown,
            status: "blocked".into(),
            ..WorkItem::default()
        };
        assert_eq!(phase_status(&card, None, &palette), None);
        let known = WorkItem {
            stage: WorkStage::Build,
            ..card
        };
        assert_eq!(
            phase_status(&known, None, &palette).map(|(words, _)| words),
            Some("✗ Blocked".to_owned())
        );
    }
}
