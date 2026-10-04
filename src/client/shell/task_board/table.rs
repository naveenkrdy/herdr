//! The Tasks view's table: which cells a row shows at the main area's width, each task's
//! cells on one line, cut at a word (the open row's wrapped whole), and a row drawn inside
//! its section's rounded box, with a line between every two cells. The grid gives way to a stacked list as the width drops.
//! Pure, like the rest of the board.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use super::model::{DueLevel, Task, TaskGroup, TaskSection};
use super::render::Hint;
use super::text::{clean, cut, wrap, Marks};
use super::{AgentState, BoardAgent, Mark};
use crate::app::state::Palette;
use crate::client::shell::render::{display_width, put_segment};

/// What a row shows, by the main area's width. In a grid, each section's text column (Action,
/// Next step, Requested, Completed) takes the spare width first, as wide as its widest text
/// up to its tier's most, and Task the rest, never less than half of the two nor
/// `TASK_MIN_WIDTH`; the other cells as wide as their widest text up to their most.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tier {
    /// 110 and wider: every column a row uses, the text columns up to 45 columns of text.
    Full,
    /// 80 to 109: as Full, with Project, Owner and Blocking narrower, and no Age or Waiting,
    /// the first columns to give way.
    Narrow,
    /// 56 to 79: a narrower `#`, the text columns up to 16 columns of text, and no Project
    /// or Blocking either; the project folds back into Task, dim before it. IN PROGRESS has no Project or Task column.
    Small,
    /// Narrower: no grid; the number and task, then due, age and next action under them.
    Stacked,
}

impl Tier {
    fn of(width: u16) -> Self {
        match width {
            110.. => Self::Full,
            80..=109 => Self::Narrow,
            56..=79 => Self::Small,
            _ => Self::Stacked,
        }
    }

    /// The most a section's `cell` takes at this tier, its padding in; none where it gives
    /// way. Project's is what it keeps when the task needs the room; `#` and Task are sized
    /// apart.
    fn most(self, cell: Cell) -> Option<u16> {
        let full = self == Self::Full;
        match cell {
            Cell::Project | Cell::Blocking | Cell::Age | Cell::Waiting if self == Self::Small => {
                None
            }
            Cell::Age | Cell::Waiting if !full => None,
            Cell::Project => Some(if full { 18 } else { 12 }),
            Cell::Blocking => Some(if full { 22 } else { 16 }),
            // Its mark and a space before the name.
            Cell::Owner => Some(if full { 20 } else { 16 }),
            Cell::Due => Some(if self == Self::Small { 13 } else { 17 }),
            Cell::Age => Some(7),
            Cell::Waiting => Some(9),
            // Too narrow to give a text column more without cutting the task to a word or two;
            // a marked column 2 more, for its mark and a space.
            Cell::NextStep | Cell::Requested => Some(if self == Self::Small { 18 } else { 47 }),
            Cell::Action | Cell::Completed => Some(if self == Self::Small { 20 } else { 49 }),
            _ => None,
        }
    }
}

/// A grid cell: a section's, or IN PROGRESS's mark and its others.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cell {
    Number,
    Project,
    Task,
    Action,
    NextStep,
    Owner,
    Requested,
    Due,
    Blocking,
    Age,
    Waiting,
    Completed,
    Mark,
    Agent,
    WorkingOn,
    Stage,
    StepNow,
    Doing,
    Since,
}

/// How many kinds of cell there are, for [`Widest`].
const CELLS: usize = 19;

impl Cell {
    /// Its name in the header row.
    fn name(self) -> &'static str {
        match self {
            Self::Number => "#",
            Self::Project => "Project",
            Self::Task => "Task",
            Self::Action => "Action",
            Self::NextStep => "Next step",
            Self::Owner => "Owner",
            Self::Requested => "Requested",
            Self::Due => "Due",
            Self::Blocking => "Blocking",
            Self::Age => "Age",
            Self::Waiting => "Waiting",
            Self::Completed => "Completed",
            Self::Mark => "",
            Self::Agent => "Agent",
            Self::WorkingOn => "Working on",
            Self::Stage => "Stage",
            Self::StepNow => "Step now",
            Self::Doing => "Doing",
            Self::Since => "For",
        }
    }

    /// A text column: it takes the room a section's other cells leave, and wraps open.
    fn flexes(self) -> bool {
        matches!(
            self,
            Self::Action | Self::NextStep | Self::Requested | Self::Completed
        )
    }
}

/// A section's cells after `#`, Project and Task, in order, and its text column among them.
fn section_cells(section: TaskSection) -> (&'static [Cell], Cell) {
    match section {
        TaskSection::ActionRequired => (
            &[Cell::Action, Cell::Due, Cell::Blocking, Cell::Age],
            Cell::Action,
        ),
        TaskSection::Queued | TaskSection::InProgress => {
            (&[Cell::NextStep, Cell::Due, Cell::Age], Cell::NextStep)
        }
        TaskSection::Waiting | TaskSection::Unknown => (
            &[Cell::Owner, Cell::Requested, Cell::Blocking, Cell::Waiting],
            Cell::Requested,
        ),
        TaskSection::Completed => (&[Cell::Completed], Cell::Completed),
    }
}

/// A `#` cell's width, its padding in: narrower where the grid is small.
fn number_width(tier: Tier) -> u16 {
    if tier == Tier::Small {
        4
    } else {
        5
    }
}

/// A section's cells but its text column, each with its width, and the text column's
/// wanted width.
type Plan = (Vec<(Cell, u16)>, u16);

/// A text column's least width, its padding in.
const FLEX_LEAST: u16 = 8;

/// The widest text a section's rows put in each cell; none for a cell no row of it uses.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Widest([u16; CELLS]);

impl Widest {
    /// Takes in `task`'s texts as a row of `section` shows them; `today` as for its row.
    pub(super) fn task(&mut self, section: TaskSection, task: &Task, today: &str) {
        let cells = section_cells(section).0;
        for cell in std::iter::once(&Cell::Project).chain(cells) {
            let text = clean(&task_text(*cell, task, today));
            self.note(
                *cell,
                display_width(&text) + marked_width(task_mark_glyph(*cell, task, &text)),
            );
        }
    }

    /// Takes in a tab waiting on the user's texts, as its row at the top of ACTION REQUIRED
    /// shows them.
    pub(super) fn waiting(&mut self, agent: &BoardAgent) {
        self.note(Cell::Project, display_width(&clean(&agent.tab)));
        let action = waiting_text(agent).1;
        self.note(
            Cell::Action,
            display_width(action) + marked_width(mark_glyph(Cell::Action, action)),
        );
        self.note(Cell::Age, display_width("now"));
    }

    /// The widest place any row names, for the one Project width.
    pub(super) fn project(&self) -> u16 {
        self.of(Cell::Project)
    }

    fn note(&mut self, cell: Cell, width: u16) {
        let widest = &mut self.0[cell as usize];
        *widest = (*widest).max(width);
    }

    fn of(&self, cell: Cell) -> u16 {
        self.0[cell as usize]
    }
}

/// IN PROGRESS's widest texts: a tab's name, its task's place, a worker's numbered task, and
/// how long it has worked.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct LiveWidest {
    pub(super) agent: u16,
    pub(super) working_on: u16,
    pub(super) stage: u16,
    pub(super) step: u16,
    pub(super) doing: u16,
    pub(super) since: u16,
}

/// What an IN PROGRESS row's cells say, worked out once a frame from its card, its task and
/// its labels.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct LiveText<'a> {
    /// The number of the task Working on names, dim before it; none when it names no task.
    pub(super) number: Option<usize>,
    pub(super) working_on: &'a str,
    /// Its stage's word, in its stage colour when it has one.
    pub(super) stage: Option<(&'a str, Option<Color>)>,
    /// Step now and Doing; none reads a dim `working`.
    pub(super) step: Option<&'a str>,
    pub(super) doing: Option<&'a str>,
}

impl LiveText<'_> {
    /// Working on's width, its number with it.
    pub(super) fn working_on_width(&self) -> u16 {
        // `#`, its digits and a space.
        let number = self.number.map_or(0, |number| {
            number
                .checked_ilog10()
                .map_or(1, |log| log + 1)
                .try_into()
                .unwrap_or(u16::MAX)
                .saturating_add(2)
        });
        number.saturating_add(display_width(self.working_on))
    }
}

/// A cell LIVE has nothing more for.
pub(super) const LIVE_IDLE: &str = "working";

/// `room` shared between columns that want `wants`: each its want while the room lasts,
/// else an even share, a column that wants less leaving the rest to the others; the last
/// takes whatever is left, at least 3 while the room has them, taken from the widest other.
fn shares(room: u16, wants: &[u16]) -> Vec<u16> {
    let mut widths = vec![0; wants.len()];
    let mut order: Vec<usize> = (0..wants.len()).collect();
    order.sort_by_key(|&index| wants[index]);
    let mut left = room;
    for (done, &index) in order.iter().enumerate() {
        let share = left / (wants.len() - done) as u16;
        widths[index] = wants[index].min(share);
        left -= widths[index];
    }
    let Some(last) = wants.len().checked_sub(1) else {
        return widths;
    };
    widths[last] += left;
    while widths[last] < room.min(3) {
        let Some(giver) = (0..last).max_by_key(|&index| widths[index]) else {
            break;
        };
        if widths[giver] == 0 {
            break;
        }
        widths[giver] -= 1;
        widths[last] += 1;
    }
    widths
}

/// The task column gives way last.
const TASK_MIN_WIDTH: u16 = 16;

/// The most Project takes, its padding in: a name of 24 columns shows whole.
const PROJECT_MOST: u16 = 26;

/// LIVE's mark column, `◐` with its padding.
const LIVE_MARK_WIDTH: u16 = 3;

/// The most LIVE's Stage takes, its padding in: `shipped` and `waiting` show whole.
const LIVE_STAGE_MOST: u16 = 12;

/// LIVE's Agent column, its padding included: as wide as its widest text between these,
/// narrower only when Doing would drop below `TASK_MIN_WIDTH`.
const LIVE_NAME_WIDTH: (u16, u16) = (10, 24);

/// A stacked row's text starts this far in, after its number.
const STACKED_INDENT: u16 = 3;

/// A section box's pieces: its corners and the lines of one row across it.
pub(super) struct Rule {
    pub(super) left: &'static str,
    /// Where a line between two cells meets the rule; the rule itself in a stacked box.
    pub(super) cross: &'static str,
    pub(super) right: &'static str,
}

/// A box's first line, under the section's heading; no line meets it.
pub(super) const TOP_RULE: Rule = Rule {
    left: "╭",
    cross: "─",
    right: "╮",
};

/// Under the column names.
pub(super) const HEAD_RULE: Rule = Rule {
    left: "├",
    cross: "┼",
    right: "┤",
};

/// A box's last line.
pub(super) const BOTTOM_RULE: Rule = Rule {
    left: "╰",
    cross: "┴",
    right: "╯",
};

/// Where the table's borders and cells sit.
pub(super) struct Table {
    /// The left border's column; a stacked row's text starts two columns in.
    pub(super) left: u16,
    /// The right border's column; a stacked row's text ends two columns before it.
    pub(super) right: u16,
    tier: Tier,
    /// Each grid cell and its width, left to right; none when stacked.
    cells: Vec<(Cell, u16)>,
    /// Each grid cell's first column, after the line before it; worked out once here so
    /// drawing a row's line allocates nothing.
    starts: Vec<u16>,
}

impl Table {
    /// Each of `sections`' columns for `area`: `#`, Project and Task one width in all of
    /// them, then each section's own cells, only those a row of it uses (`widest`), its text
    /// column taking what the others leave so every box is one width. `project` is the
    /// widest place any row names, IN PROGRESS's too; only the sections `drawn` size the
    /// columns.
    pub(super) fn sections(
        area: Rect,
        sections: &[TaskSection],
        widest: &[Widest],
        drawn: &[bool],
        project: u16,
    ) -> Vec<Self> {
        let left = area.x.saturating_add(1);
        let right = area.right().saturating_sub(2);
        let tier = Tier::of(area.width);
        let table = || Self {
            left,
            right,
            tier,
            cells: Vec::new(),
            starts: Vec::new(),
        };
        if tier == Tier::Stacked {
            return sections.iter().map(|_| table()).collect();
        }
        // Each as wide as its name in the header row, and its widest text up to its most.
        let fitted = |widest: u16, cell: Cell, most: u16| {
            (widest.max(display_width(cell.name())) + 2).min(most)
        };
        let number = number_width(tier);
        // Each section's cells but its text column, and that column's least and wanted
        // widths.
        let plans: Vec<Plan> = sections
            .iter()
            .zip(widest)
            .map(|(section, widest)| {
                let (cells, flex) = section_cells(*section);
                let fixed = cells
                    .iter()
                    .filter(|cell| **cell != flex && widest.of(**cell) > 0)
                    .filter_map(|cell| {
                        let most = tier.most(*cell)?;
                        Some((*cell, fitted(widest.of(*cell), *cell, most)))
                    })
                    .collect();
                let most = tier.most(flex).unwrap_or(FLEX_LEAST);
                (fixed, fitted(widest.of(flex), flex, most).max(FLEX_LEAST))
            })
            .collect();
        // The columns after Task, a line before each, with the text column `flex` wide.
        let span = |fixed: &[(Cell, u16)], flex: u16| {
            fixed.iter().map(|(_, width)| width + 1).sum::<u16>() + flex + 1
        };
        let drawn_plans = || {
            plans
                .iter()
                .zip(drawn)
                .filter(|(_, drawn)| **drawn)
                .map(|(plan, _)| plan)
        };
        let most_of = |width: &dyn Fn(&Plan) -> u16| drawn_plans().map(width).max().unwrap_or(0);
        let fixed = most_of(&|(fixed, _)| span(fixed, 0) - 1);
        let wanted = most_of(&|(fixed, want)| span(fixed, *want));
        let least = most_of(&|(fixed, _)| span(fixed, FLEX_LEAST));
        let inner = right.saturating_sub(left).saturating_sub(1);
        // Project as wide as its widest name up to `PROJECT_MOST` while Task and the text
        // columns keep their least; narrower, never below its tier's most.
        let project = tier
            .most(Cell::Project)
            .filter(|_| project > 0)
            .map(|kept| {
                let spare = inner.saturating_sub(number + 1 + 1 + least + TASK_MIN_WIDTH);
                fitted(project, Cell::Project, PROJECT_MOST.min(spare).max(kept))
            });
        let room = inner.saturating_sub(number + 1 + project.map_or(0, |width| width + 1));
        // Below its least (half the room it shares with the text columns) the task column
        // takes from them, down to their least.
        let task_least = TASK_MIN_WIDTH.max(room.saturating_sub(fixed).div_ceil(2));
        let mut tail = wanted.min(room.saturating_sub(task_least)).max(least);
        if tail + 3 > room {
            // Too narrow for the minimums: Task keeps a few columns.
            tail = room.saturating_sub(3);
        }
        let task = room.saturating_sub(tail);
        sections
            .iter()
            .zip(plans)
            .map(|(section, (fixed, _))| {
                let (order, flex) = section_cells(*section);
                let flex_width = tail.saturating_sub(span(&fixed, 0)).max(3);
                let mut cells = vec![(Cell::Number, number)];
                cells.extend(project.map(|width| (Cell::Project, width)));
                cells.push((Cell::Task, task));
                for cell in order {
                    if *cell == flex {
                        cells.push((flex, flex_width));
                    } else if let Some(fixed) = fixed.iter().find(|(named, _)| named == cell) {
                        cells.push(*fixed);
                    }
                }
                let mut table = table();
                table.place(cells);
                table
            })
            .collect()
    }

    /// IN PROGRESS's columns for `area`: its mark, then Agent, Stage and For as their widest
    /// text (`widest`) allows, and Working on, Step now and Doing sharing the rest, each up to
    /// its widest text, Doing the room left. Agent gives way down to its name to keep the
    /// shared columns at `TASK_MIN_WIDTH`. Where the task grid is small only Working on and
    /// Doing share, and it stacks where the task grid stacks.
    pub(super) fn live(area: Rect, widest: LiveWidest) -> Self {
        let tier = Tier::of(area.width);
        let mut table = Self {
            left: area.x.saturating_add(1),
            right: area.right().saturating_sub(2),
            tier,
            cells: Vec::new(),
            starts: Vec::new(),
        };
        if tier == Tier::Stacked {
            return table;
        }
        let small = tier == Tier::Small;
        let fit = |widest: u16, cell: Cell| widest.max(display_width(cell.name())) + 2;
        let stage = (!small).then(|| fit(widest.stage, Cell::Stage).min(LIVE_STAGE_MOST));
        let mut shared = vec![(Cell::WorkingOn, widest.working_on)];
        if !small {
            shared.push((Cell::StepNow, widest.step));
        }
        shared.push((Cell::Doing, widest.doing));
        // Its cells: Mark, Agent, Stage, the shared ones and Since.
        let count = 3 + u16::from(stage.is_some()) + shared.len() as u16;
        let inner = table.right.saturating_sub(table.left).saturating_sub(count);
        let since = fit(widest.since, Cell::Since);
        let flex = inner.saturating_sub(LIVE_MARK_WIDTH + since + stage.unwrap_or(0));
        let mut agent = fit(widest.agent, Cell::Agent).clamp(LIVE_NAME_WIDTH.0, LIVE_NAME_WIDTH.1);
        // The shared columns keep their least; Agent gives way down to its name.
        let least = TASK_MIN_WIDTH * shared.len() as u16;
        while agent + least > flex && agent > 10 {
            agent -= 1;
        }
        let wants: Vec<u16> = shared
            .iter()
            .map(|(cell, text)| fit(*text, *cell))
            .collect();
        let widths = shares(flex.saturating_sub(agent), &wants);
        let mut cells = vec![(Cell::Mark, LIVE_MARK_WIDTH), (Cell::Agent, agent)];
        let mut shared = shared
            .into_iter()
            .zip(widths)
            .map(|((cell, _), width)| (cell, width));
        cells.extend(shared.next());
        cells.extend(stage.map(|width| (Cell::Stage, width)));
        cells.extend(shared);
        cells.push((Cell::Since, since));
        table.place(cells);
        table
    }

    /// Takes `cells` and works out each one's first column, after the line before it, once
    /// here so drawing a row's line allocates nothing.
    fn place(&mut self, cells: Vec<(Cell, u16)>) {
        let mut x = self.left.saturating_add(1);
        self.starts = cells
            .iter()
            .map(|(_, width)| {
                let start = x;
                x = x.saturating_add(*width).saturating_add(1);
                start
            })
            .collect();
        self.cells = cells;
    }

    /// How far in from the box's left border LIVE's Doing text starts, under which a LIVE
    /// row's detail lines up its values; none stacked.
    pub(super) fn doing_offset(&self) -> u16 {
        self.placed_cells()
            .find(|(_, (cell, _))| *cell == Cell::Doing)
            .map_or(0, |(start, _)| start.saturating_sub(self.left))
    }

    /// Whether IN PROGRESS's rows show what their worker is on: with a Working on column.
    pub(super) fn shows_task(&self) -> bool {
        self.shows(Cell::WorkingOn)
    }

    fn shows(&self, cell: Cell) -> bool {
        self.cells.iter().any(|(named, _)| *named == cell)
    }

    /// Whether an open row shows `cell`'s `text` whole: a text column wraps it, any other cell
    /// only when it fits its column; stacked, only its due day and its state (the text
    /// column's action) show.
    fn shows_whole(&self, cell: Cell, text: &str) -> bool {
        if !self.is_grid() {
            return cell == Cell::Due || cell.flexes();
        }
        self.cells
            .iter()
            .find(|(named, _)| *named == cell)
            .is_some_and(|(_, width)| {
                cell.flexes() || display_width(&clean(text)) <= text_width(*width)
            })
    }

    /// Whether rows sit in a grid with a line between every two cells; else they stack.
    pub(super) fn is_grid(&self) -> bool {
        self.tier != Tier::Stacked
    }

    /// How far in from the box's left border the lines under a row start: a detail block's
    /// labels, and the text after a question's `↳ `.
    pub(super) fn detail_indent(&self) -> u16 {
        if self.is_grid() {
            5
        } else {
            STACKED_INDENT + 1
        }
    }

    /// The columns between the outer borders.
    pub(super) fn inner_width(&self) -> u16 {
        self.right.saturating_sub(self.left).saturating_sub(1)
    }

    /// Each grid cell's first column, after the line before it, with the cell and its width.
    fn placed_cells(&self) -> impl Iterator<Item = (u16, &(Cell, u16))> + '_ {
        self.starts.iter().copied().zip(&self.cells)
    }

    /// The column-name row: each column's name in grey, cut to its cell and padded in it.
    /// `frame` draws the lines.
    pub(super) fn put_head(&self, buffer: &mut Buffer, y: u16, frame: Style, palette: &Palette) {
        if !self.is_grid() {
            return;
        }
        let head = Style::default().fg(palette.overlay1);
        self.put_lines(buffer, y, frame);
        for (start, (cell, width)) in self.placed_cells() {
            let (text_x, end) = cell_text(start, *width);
            super::text::put_fitted(buffer, text_x, y, end, cell.name(), head);
        }
    }

    /// A row across the box: `rule`'s corners at the borders, `─` between them and its
    /// cross where a line parts two cells.
    pub(super) fn put_rule(&self, buffer: &mut Buffer, y: u16, rule: &Rule, frame: Style) {
        let across = "─".repeat(usize::from(self.inner_width()));
        let end = self.right.saturating_add(1);
        put_segment(buffer, self.left, y, end, rule.left, frame);
        put_segment(buffer, self.left.saturating_add(1), y, end, &across, frame);
        put_segment(buffer, self.right, y, end, rule.right, frame);
        for x in self.line_columns() {
            put_segment(buffer, x, y, end, rule.cross, frame);
        }
    }

    /// The box's borders and, in a grid, the lines between its cells on row `y`.
    fn put_lines(&self, buffer: &mut Buffer, y: u16, frame: Style) {
        let end = self.right.saturating_add(1);
        for x in [self.left, self.right]
            .into_iter()
            .chain(self.line_columns())
        {
            put_segment(buffer, x, y, end, "│", frame);
        }
    }

    /// The columns of the lines between two grid cells.
    fn line_columns(&self) -> impl Iterator<Item = u16> + '_ {
        let count = self.cells.len().saturating_sub(1);
        self.placed_cells()
            .take(count)
            .map(|(start, (_, width))| start.saturating_add(*width))
    }

    /// A line for the detail under a row: the box's borders, and the selection's band
    /// between them when `band` has one.
    pub(super) fn put_band(&self, buffer: &mut Buffer, y: u16, band: Style, frame: Style) {
        buffer.set_style(
            Rect::new(self.left.saturating_add(1), y, self.inner_width(), 1),
            band,
        );
        let end = self.right.saturating_add(1);
        for x in [self.left, self.right] {
            put_segment(buffer, x, y, end, "│", frame);
        }
    }

    /// Where a stacked row's text starts and ends (exclusive), one column in from each
    /// border.
    fn stacked_span(&self) -> (u16, u16) {
        (self.left.saturating_add(2), self.right.saturating_sub(1))
    }
}

/// A cell's room for text: its width less a column of padding on each side.
fn text_width(width: u16) -> u16 {
    width.saturating_sub(2)
}

/// Where a cell's text starts and ends (exclusive): one column of padding on each side.
fn cell_text(start: u16, width: u16) -> (u16, u16) {
    (
        start.saturating_add(1),
        start.saturating_add(width.saturating_sub(1)),
    )
}

/// A piece of a row's text: the text, its style, and whether the filter's words mark in it.
type Piece = (String, Style, bool);

/// A task's lines: in a grid, one line of cells, each cut at a word to its column; stacked,
/// its number and task, then its age and state. Open, the cells that hold text wrap to show
/// it whole instead.
pub(super) struct RowLayout {
    body: RowBody,
    pub(super) height: usize,
}

enum RowBody {
    /// Each grid cell's lines of pieces, in the table's order.
    Grid(Vec<Vec<Vec<Piece>>>),
    /// Each line's pieces, left to right.
    Stacked(Vec<Vec<Piece>>),
}

impl RowLayout {
    /// A task's row, `number` its place among the rows (none while it has no row to show).
    /// `today` is the day the list was read,
    /// `YYYY-MM-DD`: the day a done task without its own date shows. `on` is the mark a task an
    /// agent is on starts with. `open`, its task and text column wrap whole.
    pub(super) fn new(
        task: &Task,
        number: Option<usize>,
        table: &Table,
        palette: &Palette,
        today: &str,
        on: Option<Mark>,
        open: bool,
    ) -> Self {
        let style = |cell| cell_style(cell, task, palette);
        let dim = Style::default().fg(palette.overlay0);
        // Stacked, the due day and the age are dim unless their style says more.
        let plain = Style::default().fg(palette.text);
        let stacked = |cell| match style(cell) {
            style if style == plain => dim,
            style => style,
        };
        let cell = |cell| {
            let text = task_text(cell, task, today);
            let mark = cell_mark(cell, task, &text, palette);
            (text, style(cell), mark)
        };
        Self::arrange(
            RowText {
                number: (
                    number.map_or_else(String::new, |number| number.to_string()),
                    style(Cell::Number),
                ),
                on,
                // A done task's place is crossed out with the rest of it.
                prefix: (
                    prefix_text(task),
                    if task.group == TaskGroup::Done {
                        style(Cell::Task)
                    } else {
                        dim
                    },
                ),
                project: style(Cell::Project),
                text: (&task.task, style(Cell::Task)),
                due: (due_text(task, today), style(Cell::Due)),
                age: (age_text(task), style(Cell::Age)),
                stacked_due: stacked(Cell::Due),
                stacked_age: stacked(Cell::Age),
                state: (state_shown(task).to_owned(), style(Cell::Action)),
                // Stacked, the state carries the mark its section's grid gives it.
                state_mark: match task.section() {
                    TaskSection::ActionRequired => {
                        cell_mark(Cell::Action, task, state_shown(task), palette)
                    }
                    TaskSection::Waiting | TaskSection::Unknown => {
                        cell_mark(Cell::Owner, task, state_shown(task), palette)
                    }
                    _ => None,
                },
                // With no grid an ACTION REQUIRED task's `-` says nothing, so its age stands
                // alone.
                stacked_state: !only_needs_you(task),
                cell: &cell,
                dim,
            },
            table,
            open,
        )
    }

    /// A tab waiting on the user, at the top of ACTION REQUIRED: its mark as the sidebar draws
    /// it (a question's yellow `?`, a yes or no's red `!`), its name as its place, what it
    /// asks, the mark again before where to answer, and `now`. `open`, its ask wraps whole.
    pub(super) fn waiting(
        agent: &BoardAgent,
        table: &Table,
        palette: &Palette,
        open: bool,
    ) -> Self {
        let dim = Style::default().fg(palette.overlay0);
        let text = Style::default().fg(palette.text);
        let (asks, action) = waiting_text(agent);
        let (glyph, color) = agent.mark;
        let mark = Some(agent.mark);
        let cell = |cell| match cell {
            Cell::Action => (action.to_owned(), text, mark),
            Cell::Age => ("now".to_owned(), dim, None),
            _ => (String::new(), dim, None),
        };
        Self::arrange(
            RowText {
                number: (glyph.to_owned(), Style::default().fg(color)),
                on: None,
                prefix: (&agent.tab, dim),
                project: dim,
                text: (asks, text),
                due: (String::new(), dim),
                age: ("now".to_owned(), dim),
                stacked_due: dim,
                stacked_age: dim,
                state: (action.to_owned(), text),
                state_mark: mark,
                stacked_state: true,
                cell: &cell,
                dim,
            },
            table,
            open,
        )
    }

    /// `row` fitted to `table`: one line of cells in a grid, else its number and text over its
    /// age and state; each cut at a word, or wrapped whole while `open`. Where the grid has no
    /// Project column the place leads the task, dim.
    fn arrange(row: RowText, table: &Table, open: bool) -> Self {
        if !table.is_grid() {
            return Self::from_body(RowBody::Stacked(stacked_lines(&row, table, open)));
        }
        let prefix = if table.shows(Cell::Project) {
            ("", row.dim)
        } else {
            row.prefix
        };
        let cells = table
            .cells
            .iter()
            .map(|(cell, width)| {
                let width = text_width(*width);
                match cell {
                    Cell::Number => vec![vec![(cut(&row.number.0, width), row.number.1, false)]],
                    Cell::Project => vec![vec![(cut(row.prefix.0, width), row.project, true)]],
                    Cell::Task if open => wrap_pieces(&lead_whole(row.on, prefix, row.text), width),
                    Cell::Task => vec![lead_pieces(row.on, prefix, row.text, width)],
                    Cell::Mark | Cell::Agent | Cell::Doing | Cell::Since => Vec::new(),
                    cell => {
                        let (text, style, mark) = (row.cell)(*cell);
                        if let Some(mark) = mark {
                            if open && cell.flexes() {
                                marked_lines(mark, (&text, style), width, row.dim)
                            } else {
                                let words = (text.as_str(), style);
                                vec![lead_pieces(Some(mark), ("", row.dim), words, width)]
                            }
                        } else if cell.flexes() {
                            fit(&text, style, width, open)
                        } else {
                            // What a person owns or blocks is what a filter looks for.
                            let marked = matches!(cell, Cell::Owner | Cell::Blocking);
                            vec![vec![(cut(&text, width), style, marked)]]
                        }
                    }
                }
            })
            .collect();
        Self::from_body(RowBody::Grid(cells))
    }

    /// A row of `body`, as tall as its tallest cell or its lines.
    fn from_body(body: RowBody) -> Self {
        let height = match &body {
            RowBody::Grid(cells) => cells.iter().map(Vec::len).max().unwrap_or(0).max(1),
            RowBody::Stacked(lines) => lines.len(),
        };
        Self { body, height }
    }

    /// A LIVE row: its mark, the tab, then `text`'s cells (what it is on, with its task's
    /// number dim before it, its stage, its step now and what it does), and `minutes` on it
    /// dim, each cut at a word to its column. Every mark sits in one column, a worker's under
    /// its lead's. Stacked, the mark and the tab, then what it does and its minutes under
    /// them. `open`, its text cells wrap whole.
    pub(super) fn live(
        agent: &BoardAgent,
        text: LiveText,
        minutes: Option<u64>,
        table: &Table,
        palette: &Palette,
        open: bool,
    ) -> Self {
        let dim = Style::default().fg(palette.overlay0);
        let text_style = Style::default().fg(palette.text);
        let tab_style = Style::default().fg(palette.overlay1);
        let mark: Vec<Piece> = {
            let (glyph, color) = agent.mark;
            let glyph = (glyph.to_owned(), Style::default().fg(color), false);
            vec![glyph]
        };
        // `text` after `lead` and before `trail`, the text cut to what they leave of `width`,
        // or all of it in lines of `width` while open.
        let led =
            |lead: Option<Piece>, text: (&str, Style, bool), trail: Option<Piece>, width: u16| {
                let (text, style, marked) = text;
                let text = if open {
                    text.to_owned()
                } else {
                    let taken = [&lead, &trail]
                        .into_iter()
                        .flatten()
                        .map(|(text, _, _)| display_width(text))
                        .sum::<u16>();
                    cut(text, width.saturating_sub(taken))
                };
                let pieces: Vec<Piece> = lead
                    .into_iter()
                    .chain(std::iter::once((text, style, marked)))
                    .chain(trail)
                    .collect();
                if open {
                    wrap_pieces(&pieces, width)
                } else {
                    vec![pieces]
                }
            };
        let since = minutes.map(duration);
        // Step now or Doing: its text, else a dim `working`; its minutes after it when
        // `with_since`.
        let told = |said: Option<&str>, width: u16, with_since: bool| -> Vec<Vec<Piece>> {
            let said = said.map_or((LIVE_IDLE, dim, false), |said| (said, text_style, true));
            let trail = since
                .as_ref()
                .filter(|_| with_since)
                .map(|since| (format!(" · {since}"), dim, false));
            led(None, said, trail, width)
        };
        let name = |text: &str, style: Style, width: u16| fit(text, style, width, open);
        if !table.is_grid() {
            let (start, end) = table.stacked_span();
            let width = end.saturating_sub(start).saturating_sub(STACKED_INDENT);
            let indent = usize::from(STACKED_INDENT);
            let mark_width: usize = mark
                .iter()
                .map(|(text, _, _)| usize::from(display_width(text)))
                .sum();
            let mut first = mark;
            first.push((" ".repeat(indent.saturating_sub(mark_width)), dim, false));
            let lead = (" ".repeat(indent), dim, false);
            let mut lines = indented(first, &lead, name(&agent.tab, tab_style, width));
            let mut doing = told(text.doing, width, true);
            if doing.is_empty() {
                doing.push(Vec::new());
            }
            lines.extend(indented(vec![lead.clone()], &lead, doing));
            return Self::from_body(RowBody::Stacked(lines));
        }
        let cells = table
            .cells
            .iter()
            .map(|(cell, width)| {
                let width = text_width(*width);
                match cell {
                    Cell::Mark => vec![mark.clone()],
                    Cell::Agent => name(&agent.tab, tab_style, width),
                    Cell::WorkingOn => led(
                        text.number
                            .map(|number| (format!("#{number} "), dim, false)),
                        (text.working_on, text_style, true),
                        None,
                        width,
                    ),
                    Cell::Stage => text.stage.map_or_else(Vec::new, |(word, color)| {
                        name(
                            word,
                            Style::default().fg(color.unwrap_or(palette.text)),
                            width,
                        )
                    }),
                    Cell::StepNow => told(text.step, width, false),
                    Cell::Doing => told(text.doing, width, false),
                    Cell::Since => since.as_ref().map_or_else(Vec::new, |since| {
                        vec![vec![(cut(since, width), dim, false)]]
                    }),
                    _ => Vec::new(),
                }
            })
            .collect();
        Self::from_body(RowBody::Grid(cells))
    }

    /// Draws line `line` of the row on `y`, its box's borders and lines in `frame`.
    #[allow(clippy::too_many_arguments)] // The row's place, its band, and the three styles.
    pub(super) fn put_line(
        &self,
        buffer: &mut Buffer,
        table: &Table,
        y: u16,
        line: usize,
        band: Option<Color>,
        frame: Style,
        marks: Marks,
    ) {
        let band_style = band.map_or_else(Style::default, |color| Style::default().bg(color));
        if band.is_some() {
            buffer.set_style(
                Rect::new(table.left.saturating_add(1), y, table.inner_width(), 1),
                band_style,
            );
        }
        table.put_lines(buffer, y, frame);
        let put = |buffer: &mut Buffer, x: u16, end: u16, pieces: &[Piece]| {
            let mut x = x;
            for (text, style, marked) in pieces {
                // The number and the age are never what a filter's words are in.
                let marks = if *marked {
                    marks
                } else {
                    Marks {
                        words: &[],
                        ..marks
                    }
                };
                x = marks.put(buffer, x, y, end, text, style.patch(band_style));
            }
        };
        match &self.body {
            RowBody::Grid(cells) => {
                for ((start, (_, width)), lines) in table.placed_cells().zip(cells) {
                    if let Some(pieces) = lines.get(line) {
                        let (start, end) = cell_text(start, *width);
                        put(buffer, start, end, pieces);
                    }
                }
            }
            RowBody::Stacked(lines) => {
                let (x, end) = table.stacked_span();
                put(buffer, x, end, lines.get(line).map_or(&[], Vec::as_slice));
            }
        }
    }
}

/// The day a done task was done, `YYYY-MM-DD`: its own date, else `today`.
pub(super) fn done_day<'a>(task: &'a Task, today: &'a str) -> &'a str {
    task.done_on.as_deref().unwrap_or(today)
}

/// `2026-09-27` as `Sep 27`; none when `day` is not such a date.
pub(super) fn short_date(day: &str) -> Option<String> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mut parts = day.get(..10)?.split('-');
    let (_, month, date) = (parts.next()?, parts.next()?, parts.next()?);
    let month = MONTHS.get(usize::from(month.parse::<u8>().ok()?.checked_sub(1)?))?;
    let date: u8 = date.parse().ok().filter(|date| (1..=31).contains(date))?;
    Some(format!("{month} {date}"))
}

/// Where a task lives, dim before its text: its project, else its space.
pub(super) fn prefix_text(task: &Task) -> &str {
    if task.project.is_empty() {
        &task.space
    } else {
        &task.project
    }
}

/// A cell's colour: done tasks crossed out and dim but their day grey; a task not started
/// yet dim in every cell; the number, the due day and the age dim, an overdue due red and
/// one due today or a stale age in text; the place and an owner grey; the rest in text. The
/// state's colour is its mark's alone ([`cell_mark`]).
fn cell_style(cell: Cell, task: &Task, palette: &Palette) -> Style {
    let done = task.group == TaskGroup::Done;
    let dim = Style::default().fg(palette.overlay0);
    let grey = Style::default().fg(palette.overlay1);
    match cell {
        Cell::Completed if done => grey,
        _ if done => dim.add_modifier(Modifier::CROSSED_OUT),
        _ if starts_later(task) => dim,
        Cell::Due if task.due_level == Some(DueLevel::Overdue) => Style::default().fg(palette.red),
        Cell::Due if task.due_level == Some(DueLevel::Today) => Style::default().fg(palette.text),
        Cell::Age | Cell::Waiting if task.stale => Style::default().fg(palette.text),
        Cell::Number | Cell::Due | Cell::Age | Cell::Waiting => dim,
        Cell::Project | Cell::Owner | Cell::Completed => grey,
        _ => Style::default().fg(palette.text),
    }
}

/// The mark a task cell's words start with, in its section's colour: what you must do a
/// `?`, who a task waits on a `…`, the day it was done a `✓`; none for the rest.
fn cell_mark(cell: Cell, task: &Task, text: &str, palette: &Palette) -> Option<Mark> {
    let (glyph, section) = task_mark_glyph(cell, task, text)?;
    Some((glyph, super::style::section_role(section, palette)))
}

/// [`mark_glyph`] for a task's cell: a Claude task's action is Claude's own next step, not
/// the user's, so it takes no `?`.
fn task_mark_glyph(cell: Cell, task: &Task, text: &str) -> Option<(&'static str, TaskSection)> {
    if cell == Cell::Action && task.group == TaskGroup::Claude {
        return None;
    }
    mark_glyph(cell, text)
}

fn mark_glyph(cell: Cell, text: &str) -> Option<(&'static str, TaskSection)> {
    if text.trim().is_empty() {
        return None;
    }
    match cell {
        Cell::Action => Some(("?", TaskSection::ActionRequired)),
        Cell::Owner => Some(("…", TaskSection::Waiting)),
        Cell::Completed => Some(("✓", TaskSection::Completed)),
        _ => None,
    }
}

/// The columns a mark and its space take before its text.
fn marked_width(mark: Option<(&str, TaskSection)>) -> u16 {
    mark.map_or(0, |(glyph, _)| display_width(glyph) + 1)
}

/// What a section row's cells hold before they are fitted to their columns, each with its
/// style.
struct RowText<'a> {
    number: (String, Style),
    /// The mark of the agent on it, before its text.
    on: Option<Mark>,
    /// Where it lives, dim before its text; none when empty.
    prefix: (&'a str, Style),
    /// The Project column's style, where the grid shows it apart from the text.
    project: Style,
    text: (&'a str, Style),
    due: (String, Style),
    age: (String, Style),
    /// Stacked, the due day's and the age's styles: dim unless they say more.
    stacked_due: Style,
    stacked_age: Style,
    state: (String, Style),
    /// Stacked, the state shows after the age.
    stacked_state: bool,
    /// Stacked, the mark before the state.
    state_mark: Option<Mark>,
    /// In a grid, each other cell's text, style and the mark before it.
    cell: &'a dyn Fn(Cell) -> (String, Style, Option<Mark>),
    /// The dim of the separators between them.
    dim: Style,
}

/// `text` in lines of `width` after its mark and a space, the lines under the first
/// starting under its words.
fn marked_lines(
    (glyph, color): Mark,
    (text, style): (&str, Style),
    width: u16,
    dim: Style,
) -> Vec<Vec<Piece>> {
    let mark = format!("{glyph} ");
    let indent = display_width(&mark);
    let mut lines = wrap_pieces(
        &[(text.to_owned(), style, true)],
        width.saturating_sub(indent),
    );
    for (at, line) in lines.iter_mut().enumerate() {
        let lead = if at == 0 {
            (mark.clone(), Style::default().fg(color), false)
        } else {
            (" ".repeat(usize::from(indent)), dim, false)
        };
        line.insert(0, lead);
    }
    lines
}

/// Stacked, the state's mark and a space, before the state.
fn state_mark_piece(row: &RowText) -> Option<Piece> {
    row.state_mark
        .map(|(glyph, color)| (format!("{glyph} "), Style::default().fg(color), false))
}

/// A task cell's pieces in `width` columns: the agent's mark `on` it and a space, then
/// `prefix · ` (at most half the cell, cut at a word), then the text cut at a word to the
/// rest.
fn lead_pieces(
    on: Option<Mark>,
    (prefix, prefix_style): (&str, Style),
    (text, style): (&str, Style),
    width: u16,
) -> Vec<Piece> {
    let mut pieces = Vec::new();
    let mut width = width;
    if let Some((glyph, color)) = on {
        let mark = format!("{glyph} ");
        width = width.saturating_sub(display_width(&mark));
        pieces.push((mark, Style::default().fg(color), false));
    }
    if prefix.is_empty() {
        pieces.push((cut(text, width), style, true));
        return pieces;
    }
    let prefix = format!("{} · ", cut(prefix, (width / 2).saturating_sub(3)));
    let rest = width.saturating_sub(display_width(&prefix));
    pieces.extend([(prefix, prefix_style, true), (cut(text, rest), style, true)]);
    pieces
}

/// A task cell's pieces whole, to wrap: the agent's mark `on` it and a space, then
/// `prefix · ` and the text.
fn lead_whole(
    on: Option<Mark>,
    (prefix, prefix_style): (&str, Style),
    (text, style): (&str, Style),
) -> Vec<Piece> {
    let mut pieces = Vec::new();
    if let Some((glyph, color)) = on {
        pieces.push((format!("{glyph} "), Style::default().fg(color), false));
    }
    if !prefix.is_empty() {
        pieces.push((format!("{prefix} · "), prefix_style, true));
    }
    pieces.push((text.to_owned(), style, true));
    pieces
}

/// `text`, filter-marked, cut to `width`, or while `open` in lines of `width`.
fn fit(text: &str, style: Style, width: u16, open: bool) -> Vec<Vec<Piece>> {
    if open {
        wrap_pieces(&[(text.to_owned(), style, true)], width)
    } else {
        vec![vec![(cut(text, width), style, true)]]
    }
}

/// `pieces` as one text in lines no wider than `width`, broken where [`wrap`] breaks it, each
/// character in its piece's style and mark, and a space in the one before it.
fn wrap_pieces(pieces: &[Piece], width: u16) -> Vec<Vec<Piece>> {
    // `wrap` keeps every character but spaces, in order, so each finds its piece again.
    let mut joined = String::new();
    let mut owners = Vec::new();
    for (at, (text, _, _)) in pieces.iter().enumerate() {
        for ch in clean(text).chars() {
            if !ch.is_whitespace() {
                owners.push(at);
            }
            joined.push(ch);
        }
    }
    let mut owners = owners.into_iter();
    let mut owner = 0;
    wrap(&joined, width)
        .into_iter()
        .map(|line| {
            let mut out: Vec<Piece> = Vec::new();
            for ch in line.chars() {
                if !ch.is_whitespace() {
                    owner = owners.next().unwrap_or(owner);
                }
                let Some((_, style, marked)) = pieces.get(owner) else {
                    continue;
                };
                match out.last_mut() {
                    Some((text, last, last_marked)) if last == style && last_marked == marked => {
                        text.push(ch);
                    }
                    _ => out.push((ch.to_string(), *style, *marked)),
                }
            }
            out
        })
        .collect()
}

/// `lines` each after a lead: `first` before the first, `rest` before every other.
fn indented(first: Vec<Piece>, rest: &Piece, lines: Vec<Vec<Piece>>) -> Vec<Vec<Piece>> {
    let mut first = Some(first);
    lines
        .into_iter()
        .map(|line| {
            let mut out = first.take().unwrap_or_else(|| vec![rest.clone()]);
            out.extend(line);
            out
        })
        .collect()
}

/// A stacked row: `1  project · task` on one line, then `due · age · next action` (each only
/// when it says something) on one; each cut at a word, each piece in its cell's colour. Open,
/// each wraps whole, its further lines under its text.
fn stacked_lines(row: &RowText, table: &Table, open: bool) -> Vec<Vec<Piece>> {
    let (start, end) = table.stacked_span();
    let width = end.saturating_sub(start).saturating_sub(STACKED_INDENT);
    let indent = " ".repeat(usize::from(STACKED_INDENT));
    let number = format!(
        "{:<width$}",
        row.number.0,
        width = usize::from(STACKED_INDENT)
    );
    let says_state = !row.state.0.is_empty() && row.stacked_state;
    // The second line's parts before the action, `·` between them.
    let mut second: Vec<Piece> = Vec::new();
    for (text, style) in [(&row.due.0, row.stacked_due), (&row.age.0, row.stacked_age)] {
        if text.is_empty() {
            continue;
        }
        if !second.is_empty() {
            second.push((" · ".to_owned(), row.dim, false));
        }
        second.push((text.clone(), style, false));
    }
    if says_state && !second.is_empty() {
        second.push((" · ".to_owned(), row.dim, false));
    }
    if open {
        let lead = (indent, row.dim, false);
        let mut lines = indented(
            vec![(number, row.number.1, false)],
            &lead,
            wrap_pieces(&lead_whole(row.on, row.prefix, row.text), width),
        );
        if says_state {
            second.extend(state_mark_piece(row));
            second.push((row.state.0.clone(), row.state.1, true));
        }
        lines.extend(indented(
            vec![lead.clone()],
            &lead,
            wrap_pieces(&second, width),
        ));
        return lines;
    }
    let mut first = vec![(number, row.number.1, false)];
    first.extend(lead_pieces(row.on, row.prefix, row.text, width));
    if says_state {
        second.extend(state_mark_piece(row));
        let used: u16 = second.iter().map(|(text, _, _)| display_width(text)).sum();
        second.push((
            cut(&row.state.0, width.saturating_sub(used)),
            row.state.1,
            true,
        ));
    }
    second.insert(0, (indent, row.dim, false));
    vec![first, second]
}

/// A waiting tab's question (or that it asks for an approval), and where to answer it.
pub(super) fn waiting_text(agent: &BoardAgent) -> (&str, &'static str) {
    match &agent.state {
        AgentState::Waiting(Some(ask)) => (ask, "answer in tab"),
        _ => ("asks for an approval", "approve in tab"),
    }
}

/// Where: the space and project, or the project alone once the board shows one space.
pub(super) fn where_text(task: &Task, narrowed: bool) -> String {
    if task.project.is_empty() {
        task.space.clone()
    } else if narrowed {
        task.project.clone()
    } else {
        format!("{}/{}", task.space, task.project)
    }
}

/// A task that starts after today: dim at the end of its section, and counted apart.
pub(super) fn scheduled(task: &Task) -> bool {
    task.scheduled || starts_later(task)
}

/// A task that says when it starts: not begun yet, so its row is dim.
fn starts_later(task: &Task) -> bool {
    task.starts_text
        .as_deref()
        .is_some_and(|starts| !starts.is_empty())
}

/// The reader's action, or from an older reader `✓ done` once done, else what the task waits
/// on; blank for a NEEDS YOU task that only waits on you, and for words of only spaces. The
/// agent on it shows in LIVE and in its detail, not here.
pub(super) fn state_shown(task: &Task) -> &str {
    match &task.action {
        None if task.group == TaskGroup::Done => "✓ done",
        _ if only_needs_you(task) => "",
        Some(action) => action.trim(),
        None => task.state.trim(),
    }
}

/// Due: the reader's words, or from an older reader the day a done task was done (`Sep 27`,
/// else `today`, the day the list was read); with neither, when a task that starts later
/// starts (`Starts Tue`).
pub(super) fn due_text(task: &Task, today: &str) -> String {
    match task.due_text.as_deref() {
        Some(due) if !due.is_empty() => due.to_owned(),
        None if task.group == TaskGroup::Done => {
            short_date(done_day(task, today)).unwrap_or_else(|| "today".to_owned())
        }
        _ => task
            .starts_text
            .clone()
            .filter(|_| starts_later(task))
            .unwrap_or_default(),
    }
}

/// Age: the reader's words, or from an older reader its days (`today`, `3d`); nothing for a
/// done task, whose day is under Due.
pub(super) fn age_text(task: &Task) -> String {
    match &task.age_text {
        Some(age) => age.clone(),
        None if task.group == TaskGroup::Done => String::new(),
        // A day's age: `now` is a waiting tab's live ask, and the sidebar's.
        None if task.age_days == 0 => "today".to_owned(),
        None => format!("{}d", task.age_days),
    }
}

/// What `task`'s row puts in a section's `cell`, as its row shows it; `today` as for Due.
fn task_text(cell: Cell, task: &Task, today: &str) -> String {
    match cell {
        Cell::Project => prefix_text(task).to_owned(),
        Cell::Task => task.task.clone(),
        Cell::Action | Cell::NextStep => state_shown(task).to_owned(),
        Cell::Owner => owner_text(task).to_owned(),
        Cell::Requested => requested_text(task).to_owned(),
        Cell::Due | Cell::Completed => due_text(task, today),
        Cell::Blocking => task.blocking.clone().unwrap_or_default(),
        Cell::Age | Cell::Waiting => age_text(task),
        Cell::Number
        | Cell::Mark
        | Cell::Agent
        | Cell::WorkingOn
        | Cell::Stage
        | Cell::StepNow
        | Cell::Doing
        | Cell::Since => String::new(),
    }
}

/// Who a task waits on: the reader's owner, or from an older reader what its state names
/// before `: `.
pub(super) fn owner_text(task: &Task) -> &str {
    match &task.owner {
        Some(owner) => owner,
        None => task.state.split_once(": ").map_or("", |(owner, _)| owner),
    }
}

/// What a task's owner was asked: its action after `owner: `, nothing when the action only
/// names the owner, else the whole action.
pub(super) fn requested_text(task: &Task) -> &str {
    let action = task.action.as_deref().unwrap_or(&task.state);
    let owner = owner_text(task);
    match action.split_once(": ") {
        Some((named, asked)) if owner.is_empty() || named == owner => asked,
        _ if action == owner => "",
        _ => action,
    }
}

/// A NEEDS YOU task whose State only says so: its section already does, so it shows blank.
fn only_needs_you(task: &Task) -> bool {
    task.group == TaskGroup::You && task.action.as_ref().unwrap_or(&task.state) == "needs you"
}

pub(super) fn duration(minutes: u64) -> String {
    if minutes < 60 {
        format!("{minutes}m")
    } else {
        format!("{}h", minutes / 60)
    }
}

/// The cells of `task`'s row in `section` that `table` cuts short or leaves out, each whole
/// and named as its column is: an open row wraps its task and text column whole, so only its
/// project, owner, due day and what it blocks can be cut, and a stacked row shows only its
/// place, task, due day and state.
pub(super) fn cut_fields(
    task: &Task,
    section: TaskSection,
    table: &Table,
    today: &str,
) -> Vec<(&'static str, String)> {
    // Without a Project column the place leads the task, which wraps open.
    let project = (table.is_grid() && table.shows(Cell::Project))
        .then(|| task_text(Cell::Project, task, today))
        .filter(|project| !project.is_empty() && !table.shows_whole(Cell::Project, project))
        .map(|project| (Cell::Project.name(), project));
    let cells = section_cells(section)
        .0
        .iter()
        .copied()
        .filter(|cell| {
            matches!(
                cell,
                Cell::Action
                    | Cell::NextStep
                    | Cell::Owner
                    | Cell::Requested
                    | Cell::Due
                    | Cell::Blocking
            )
        })
        .filter_map(|cell| {
            let value = task_text(cell, task, today);
            (!value.is_empty() && value != "-" && !table.shows_whole(cell, &value))
                .then_some((cell.name(), value))
        });
    project.into_iter().chain(cells).collect()
}

/// A task's notes file and line, and how many more files hold it; none without one.
pub(super) fn file_field(task: &Task, home_dir: Option<&str>) -> Option<(&'static str, String)> {
    let copy = task.copies.first()?;
    let path = super::text::home_path(&copy.file, home_dir);
    let more = match task.copies.len() - 1 {
        0 => String::new(),
        1 => " (+1 more file)".to_owned(),
        more => format!(" (+{more} more files)"),
    };
    Some(("File", format!("{path}:{}{more}", copy.line)))
}

/// The keys the selected task's detail offers: what `Enter` does (go to the agent, else its
/// space), `w` to show its card while it has one, `o` to open the file while the server takes
/// `tasks.open`, and `d` while it takes `tasks.act`.
pub(super) fn detail_keys(task: &Task, has_card: bool, can_open: bool, can_act: bool) -> Vec<Hint> {
    let mut keys = Vec::new();
    if task.agent.is_some() {
        keys.push(("Enter", "go to agent"));
    } else if !task.space.is_empty() {
        keys.push(("Enter", "go to space"));
    }
    if has_card {
        keys.push(super::render::SHOW_CARD);
    }
    if can_open && !task.copies.is_empty() {
        keys.push(("o", "open file"));
    }
    if can_act && task.group != TaskGroup::Done {
        keys.push(("d", "done"));
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where a table's last cell ends: its last start plus its width.
    fn last_end(table: &Table) -> u16 {
        table
            .placed_cells()
            .last()
            .map_or(0, |(start, (_, width))| start + width)
    }

    #[test]
    fn in_progress_rows_end_where_the_task_grid_ends() {
        let widest = LiveWidest {
            agent: 9,
            working_on: 30,
            stage: 5,
            step: 20,
            doing: 25,
            since: 3,
        };
        // Full, Narrow, and Small down to its least.
        for width in [186, 100, 70, 56] {
            let area = Rect::new(0, 0, width, 40);
            let grid = Table::sections(
                area,
                &[TaskSection::Queued],
                &[Widest::default()],
                &[true],
                0,
            );
            let live = Table::live(area, widest);
            assert_eq!(last_end(&live), last_end(&grid[0]), "width {width}");
        }
    }

    #[test]
    fn shares_never_add_up_to_more_than_the_room() {
        for room in 0..=12 {
            for wants in [vec![8, 8, 8], vec![1, 20], vec![20], vec![2, 2, 30]] {
                let widths = shares(room, &wants);
                assert!(
                    widths.iter().sum::<u16>() <= room,
                    "room {room} wants {wants:?} got {widths:?}"
                );
                // The last keeps 3 columns whenever the room has them.
                assert!(
                    widths[widths.len() - 1] >= room.min(3),
                    "room {room} got {widths:?}"
                );
            }
        }
    }
}
