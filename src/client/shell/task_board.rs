//! The task board: every space's tasks from `claude-tasks list --json`, drawn in the main
//! area instead of the panes while open. The board's look is this client's own.

mod filter;
pub(super) mod model;
mod render;
mod sort;
mod spacing;
pub(super) mod style;
mod table;
mod text;
mod work;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crossterm::event::KeyModifiers;
use model::TaskList;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

use super::tree_panel::{needs_you, Reason};
use super::tree_sidebar::{agent_mark, folder_name, pane_label, phase_word, token};
use super::{ClientEndpointId, ClientShellSnapshot};

#[cfg(test)]
pub(super) use filter::{card_owner, card_status, CardOwner, CardStatus};
pub(super) use filter::{FilterPick, WorkFilter};
#[cfg(test)]
pub(super) use render::footer_keys;
pub(super) use render::render_task_board;
pub(super) use sort::TaskSort;

/// A mark as the tree draws it: its glyph and its colour.
pub(super) type Mark = (&'static str, Color);

/// A live agent's tab: a working one shows in LIVE, one waiting on the user at the top of
/// NEEDS YOU; the agent menu lists every one of the space shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BoardAgent {
    pub(super) pane_id: String,
    /// Its space's label.
    pub(super) space: String,
    /// Its tab's name, as the sidebar names it.
    pub(super) tab: String,
    pub(super) state: AgentState,
    /// The pane of the agent it works for, by its `lead` token, when that pane runs another
    /// agent in its space: it is that lead's worker.
    pub(super) lead: Option<String>,
    /// Its state's mark as the tree draws it: the sub-agent spinner for a worker at work.
    pub(super) mark: Mark,
    /// What its pane's labels and its tab say, for its IN PROGRESS row.
    pub(super) labels: AgentLabels,
}

/// An agent's pane labels and its tab's own name and folder, each clean and none when
/// blank: its IN PROGRESS row's cells fall back through them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct AgentLabels {
    /// Its `stage` token: a lead's `build · herdr-w1 on B4c`.
    pub(super) stage: Option<String>,
    /// Its `doing` token: what it does this minute, empty once it stops.
    pub(super) doing: Option<String>,
    /// Its `task` token: a one-off tab's /rename name.
    pub(super) task: Option<String>,
    /// Its tab's name when someone named it.
    pub(super) tab_name: Option<String>,
    /// The last part of the folder its pane runs in, `~` for home.
    pub(super) folder: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AgentState {
    /// At work: on what, from its `task` token, else its `stage` token.
    Working(Option<String>),
    /// Waiting on the user, as the sidebar says: its question, or none when it is blocked for
    /// an approval.
    Waiting(Option<String>),
    /// Idle or done, asking nothing.
    Resting,
}

/// Every agent, in the snapshot's order, from the snapshot the client already has, each with
/// its tree mark `spinner_ms` into the spinner, and what it does or asks without control
/// characters. Only the open board reads it: once a frame, on a spinner tick, and on a key
/// that needs a worker.
pub(super) fn board_agents(
    snapshot: Option<&ClientShellSnapshot>,
    config: &super::ClientShellConfig,
    home_dir: Option<&str>,
    spinner_ms: u64,
) -> Vec<BoardAgent> {
    let Some(snapshot) = snapshot else {
        return Vec::new();
    };
    let labels: HashMap<&str, &str> = snapshot
        .workspaces
        .iter()
        .map(|workspace| (workspace.workspace_id.as_str(), workspace.label.as_str()))
        .collect();
    let spaces: HashMap<&str, &str> = snapshot
        .agents
        .iter()
        .map(|agent| (agent.pane_id.as_str(), agent.workspace_id.as_str()))
        .collect();
    let named_tabs: HashMap<&str, &str> = snapshot
        .tabs
        .iter()
        .filter(|tab| tab.custom_label)
        .map(|tab| (tab.tab_id.as_str(), tab.label.as_str()))
        .collect();
    let panes: HashMap<&str, &crate::protocol::ClientShellPane> = snapshot
        .panes
        .iter()
        .map(|pane| (pane.pane_id.as_str(), pane))
        .collect();
    snapshot
        .agents
        .iter()
        .map(|agent| {
            let state = if agent.agent_status == crate::api::schema::AgentStatus::Working {
                let doing = token(&agent.tokens, "task").map(text::clean).or_else(|| {
                    token(&agent.tokens, "stage")
                        .and_then(phase_word)
                        .map(str::to_owned)
                });
                AgentState::Working(doing)
            } else {
                match needs_you(agent) {
                    Some(Reason::Answer) => {
                        AgentState::Waiting(token(&agent.tokens, "ask").map(text::clean))
                    }
                    Some(Reason::Approval) => AgentState::Waiting(None),
                    None => AgentState::Resting,
                }
            };
            let lead = token(&agent.tokens, "lead").filter(|lead| {
                *lead != agent.pane_id && spaces.get(lead) == Some(&agent.workspace_id.as_str())
            });
            BoardAgent {
                pane_id: agent.pane_id.clone(),
                space: labels
                    .get(agent.workspace_id.as_str())
                    .copied()
                    .unwrap_or_default()
                    .to_owned(),
                tab: pane_label(agent).to_owned(),
                state,
                lead: lead.map(str::to_owned),
                mark: agent_mark(agent, spinner_ms, config, lead.is_some()),
                labels: AgentLabels {
                    stage: label(token(&agent.tokens, "stage")),
                    doing: label(token(&agent.tokens, "doing")),
                    task: label(token(&agent.tokens, "task")),
                    tab_name: label(named_tabs.get(agent.tab_id.as_str()).copied()),
                    folder: panes
                        .get(agent.pane_id.as_str())
                        .and_then(|pane| folder_name(pane, home_dir))
                        .map(std::borrow::Cow::into_owned),
                },
            }
        })
        .collect()
}

/// A label's text without control characters; none when blank.
fn label(text: Option<&str>) -> Option<String> {
    text.map(text::clean).filter(|text| !text.trim().is_empty())
}

/// One drawn line: its pieces left to right.
pub(super) type Spans = Vec<(String, Style)>;

/// The agent a task's or a card's tag in `space` names: the one on the tag's pane, else one
/// of its name in that space.
fn tagged_agent<'a>(
    named: &model::TaskAgent,
    space: &str,
    agents: &'a [BoardAgent],
) -> Option<&'a BoardAgent> {
    agents
        .iter()
        .find(|agent| tags_pane(named, agent))
        .or_else(|| agents.iter().find(|agent| tags_name(named, space, agent)))
}

/// What the agent a task's or a card's tag in `space` names is doing now, when the board lists
/// it at work.
pub(super) fn doing_of<'a>(
    named: &model::TaskAgent,
    space: &str,
    agents: &'a [BoardAgent],
) -> Option<&'a str> {
    match &tagged_agent(named, space, agents)?.state {
        AgentState::Working(doing) => doing.as_deref(),
        _ => None,
    }
}

/// Whether the agent a task's or a card's tag in `space` names is at work: its row or card
/// then shows the spinner.
pub(super) fn tag_works(
    named: Option<&model::TaskAgent>,
    space: &str,
    agents: &[BoardAgent],
) -> bool {
    named
        .and_then(|named| tagged_agent(named, space, agents))
        .is_some_and(|agent| matches!(agent.state, AgentState::Working(_)))
}

/// A worker's lead among `spaced`, when it is there and works for no one itself: the tree of
/// leads and workers LIVE and the agent menu draw is one level deep.
pub(super) fn lead_here<'a>(
    agent: &BoardAgent,
    spaced: &[&'a BoardAgent],
) -> Option<&'a BoardAgent> {
    let pane = agent.lead.as_deref()?;
    spaced
        .iter()
        .copied()
        .find(|lead| lead.pane_id == pane && lead.lead.is_none())
}

/// The lead of `space` among `agents`: the agent there its workers work for, else its only
/// agent that works for no one.
pub(super) fn space_lead<'a>(space: &str, agents: &'a [BoardAgent]) -> Option<&'a BoardAgent> {
    let mut tops = agents
        .iter()
        .filter(|agent| agent.space == space && agent.lead.is_none());
    let led = |top: &&BoardAgent| {
        agents
            .iter()
            .any(|agent| agent.lead.as_deref() == Some(top.pane_id.as_str()))
    };
    tops.clone().find(led).or_else(|| {
        let only = tops.next()?;
        tops.next().is_none().then_some(only)
    })
}

/// Whether a task's or a card's agent tag in `space` names `agent`: by its pane, or by its
/// name in that space.
pub(super) fn tags_agent(named: &model::TaskAgent, space: &str, agent: &BoardAgent) -> bool {
    tags_pane(named, agent) || tags_name(named, space, agent)
}

fn tags_pane(named: &model::TaskAgent, agent: &BoardAgent) -> bool {
    named.pane == agent.pane_id
}

fn tags_name(named: &model::TaskAgent, space: &str, agent: &BoardAgent) -> bool {
    !named.name.is_empty() && named.name == agent.tab && agent.space == space
}

/// Of `tagged` items, each with its agent tag and its space, the first whose tag names `agent`
/// by its pane, else the first by its name in its space.
fn tagged_with<'a, T>(
    agent: &BoardAgent,
    tagged: impl Iterator<Item = (&'a T, &'a model::TaskAgent, &'a str)> + Clone,
) -> Option<&'a T> {
    tagged
        .clone()
        .find(|(_, named, _)| tags_pane(named, agent))
        .or_else(|| {
            tagged
                .into_iter()
                .find(|(_, named, space)| tags_name(named, space, agent))
        })
        .map(|(item, ..)| item)
}

/// Whether the header offers its agent button: one space shows and it has more than one
/// agent.
pub(super) fn offers_agents(board: &TaskBoard, agents: &[BoardAgent]) -> bool {
    board.space.as_deref().is_some_and(|space| {
        agents
            .iter()
            .filter(|agent| agent.space == space)
            .nth(1)
            .is_some()
    })
}

/// The agent the board shows alone: the chosen one, while the header offers the choice and
/// it is an agent of the space shown.
pub(super) fn shown_agent<'a>(
    board: &TaskBoard,
    agents: &'a [BoardAgent],
) -> Option<&'a BoardAgent> {
    let (pane, space) = (board.agent.as_deref()?, board.space.as_deref()?);
    if !offers_agents(board, agents) {
        return None;
    }
    agents
        .iter()
        .find(|agent| agent.pane_id == pane && agent.space == space)
}

/// The agents LIVE draws from: those of the space shown, and only the one shown alone while
/// the agent button shows one.
pub(super) fn spaced_agents<'a>(
    board: &TaskBoard,
    agents: &'a [BoardAgent],
) -> Vec<&'a BoardAgent> {
    let chosen = shown_agent(board, agents);
    agents
        .iter()
        .filter(|agent| {
            board
                .space
                .as_ref()
                .is_none_or(|shown| agent.space == *shown)
        })
        .filter(|agent| chosen.is_none_or(|chosen| chosen.pane_id == agent.pane_id))
        .collect()
}

/// A worker's task: the open task whose agent is it, by its pane first, else by its name in
/// its space; never a done one.
pub(super) fn worker_task<'a>(agent: &BoardAgent, list: &'a TaskList) -> Option<&'a model::Task> {
    let open = list
        .tasks
        .iter()
        .filter(|task| task.group != model::TaskGroup::Done)
        .filter_map(|task| Some((task, task.agent.as_ref()?, task.space.as_str())));
    tagged_with(agent, open)
}

/// A task's Work card: the one its `card` names by its slug.
pub(super) fn task_card<'a>(task: &model::Task, list: &'a TaskList) -> Option<&'a model::WorkItem> {
    let slug = task.card.as_deref()?;
    list.work
        .iter()
        .find(|card| card.slug.as_deref() == Some(slug))
}

/// An agent's Work card: its task's.
pub(super) fn agent_card<'a>(
    agent: &BoardAgent,
    list: &'a TaskList,
) -> Option<&'a model::WorkItem> {
    task_card(worker_task(agent, list)?, list)
}

/// A task's identity across reads: its space and its text, as the reader merges its copies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TaskKey {
    pub(super) space: String,
    pub(super) task: String,
}

impl TaskKey {
    pub(super) fn of(task: &model::Task) -> Self {
        Self {
            space: task.space.clone(),
            task: task.task.clone(),
        }
    }
}

/// A row the selection can rest on: a task's, or a live agent's tab's, by its pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RowKey {
    Task(TaskKey),
    Agent(String),
}

/// A section that folds, whose heading the keyboard's focus can rest on while it is folded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Fold {
    /// LIVE, open until folded.
    Live,
    /// DONE, folded until opened.
    Done,
}

/// What opened the board: the sidebar's icon, or the board's key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BoardOpener {
    Sidebar,
    Key,
}

/// A question or a typed line open on a task's row, waiting for its next key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RowPrompt {
    /// `y` deletes; any other key keeps the task.
    Delete,
    /// `1` you, `2` me, `3` a typed name.
    Move,
    /// The name typed so far.
    MoveTo(String),
    /// A new task under this one, typed so far.
    New(String),
}

/// A `tasks.act` sent and not yet answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Acting {
    pub(super) request_id: String,
    pub(super) task: TaskKey,
    /// What the row shows while it waits, as `◐ <label>…`.
    pub(super) label: String,
}

/// Which of the board's two views shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum BoardView {
    /// Every task, by who must act.
    #[default]
    Tasks,
    /// Every feature, bug and chore, by stage.
    Work,
}

/// A chip: a Tasks view section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Category {
    Section(model::TaskSection),
}

/// What a click on the board picked: a Work card, by its intent file, or a task's row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ClickTarget {
    Card(String),
    Row(RowKey),
}

/// The board's last click on a card or a row, and when: a second on the same one soon after
/// is a double click.
#[derive(Debug, Clone)]
pub(super) struct BoardClick {
    pub(super) target: ClickTarget,
    pub(super) at: Instant,
}

/// A file the board opened, followed until its popup closes, when the board comes back.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum FilePopup {
    #[default]
    None,
    /// The server opened it, at this time; its popup has not shown yet.
    Awaited(Instant),
    /// Its popup shows.
    Showing,
    /// No popup showed within `POPUP_LAPSE` (`less` may have come and gone between two
    /// surface updates), so the board came back at this time; a popup that shows soon after
    /// is still the file's.
    Lapsed(Instant),
}

/// How long the board waits for its file's popup before it comes back without one.
const POPUP_LAPSE: Duration = Duration::from_secs(3);

/// How long after coming back a late popup is still taken as the file's.
const LATE_POPUP: Duration = Duration::from_secs(30);

/// The board: the store it reads and this client's own view of it.
#[derive(Debug, Default)]
pub(super) struct TaskBoard {
    pub(super) store: TaskBoardStore,
    /// Drawn in the main area instead of the panes.
    pub(super) open: bool,
    pub(super) selected: Option<RowKey>,
    /// The row whose detail shows under it, opened by `→` or a click on the selected row;
    /// it shows only while it is the selected one. [`TaskBoard::select_row`] closes it when
    /// the selection moves, so one row is open at a time.
    pub(super) open_row: Option<RowKey>,
    /// The label of the space the board shows alone.
    pub(super) space: Option<String>,
    /// Lines of the sections scrolled off the top.
    pub(super) scroll: usize,
    /// The next frame scrolls the selected task into view.
    pub(super) reveal_selected: bool,
    pub(super) view: BoardView,
    /// The selected Work card, by its intent file.
    pub(super) work_selected: Option<String>,
    /// The first card each Work column shows, when it is too tall for its cards.
    pub(super) work_scroll: [usize; 5],
    /// Lines of the Work panel scrolled off its top; each card's panel starts at its top.
    pub(super) work_panel_scroll: usize,
    /// The request id of the `tasks.list` waiting for its reply.
    pub(super) asking: Option<String>,
    /// When the last `tasks.list` was sent.
    pub(super) asked_at: Option<Instant>,
    /// The endpoint and boot that do not advertise `tasks.list`: never asked again.
    pub(super) unsupported: Option<(ClientEndpointId, String)>,
    /// The question or typed line open on a task's row.
    pub(super) prompt: Option<(TaskKey, RowPrompt)>,
    /// The one action waiting for its answer; no other starts until it comes.
    pub(super) acting: Option<Acting>,
    /// Why the last action did not go through, or what to do first; shown above the keys.
    pub(super) notice: Option<String>,
    /// A `tasks.list` crossed the last action, so its reply may predate it: ask again.
    pub(super) reread: bool,
    /// The list shown may predate the server's (after an action, or a file it did not list),
    /// so the server may refuse an action: none starts until a list read after this lands.
    pub(super) list_stale: bool,
    /// Whether the server takes `tasks.act`, set each time the open board draws.
    pub(super) can_act: bool,
    /// Whether the server takes `tasks.open`, set each time the open board draws.
    pub(super) can_open: bool,
    /// The request id of the `tasks.open` waiting for its answer; no other starts until it
    /// comes.
    pub(super) opening: Option<String>,
    /// The file it opened, while its popup has not closed.
    pub(super) file_popup: FilePopup,
    /// The footer names every key, however many lines it takes; `?` turns it on and off.
    pub(super) keys_shown: bool,
    /// The foot shows each read problem with its path, not only how many notes files have
    /// one; a click on the problem line turns it on and off, and `?` shows them too.
    pub(super) problems_shown: bool,
    /// The text the board narrows its tasks and cards to; empty shows them all.
    pub(super) filter: String,
    /// The header's filter input takes the keys.
    pub(super) filter_editing: bool,
    /// The header's space button has its menu open.
    pub(super) space_menu: bool,
    /// The agent the board shows alone, by its pane: its tasks, its LIVE row, its cards.
    pub(super) agent: Option<String>,
    /// The header's agent button has its menu open.
    pub(super) agent_menu: bool,
    /// The order of every section's task rows.
    pub(super) sort: TaskSort,
    /// The header's sort button has its menu open.
    pub(super) sort_menu: bool,
    /// The Tasks view shows only this section, by its header button.
    pub(super) only_section: Option<model::TaskSection>,
    /// The Work view's Filter menu picks: the cards it keeps.
    pub(super) work_filter: WorkFilter,
    /// The Work view's Filter button has its menu open.
    pub(super) work_filter_menu: bool,
    /// The Filter menu's item under the keyboard, in `FilterPick::ALL`.
    pub(super) work_filter_cursor: usize,
    /// The selected card's panel was closed; selecting a card shows it again.
    pub(super) work_panel_closed: bool,
    /// The selected step in the card panel, open steps first; while there is one the panel
    /// has the keyboard's focus and `↑` `↓` move between steps.
    pub(super) work_step: Option<usize>,
    /// The last click on a card or a row.
    pub(super) last_click: Option<BoardClick>,
    /// DONE starts folded; a click on its heading, or Enter while it holds the
    /// keyboard's focus, opens (and refolds) it. Kept while the board stays open.
    pub(super) done_expanded: bool,
    /// LIVE was folded by a click on its heading; Enter while its heading holds the
    /// keyboard's focus, or another click, opens it.
    pub(super) live_folded: bool,
    /// The folded heading the keyboard's focus reached: LIVE's above the first row, DONE's
    /// past the last.
    pub(super) focused_heading: Option<Fold>,
}

/// How the sidebar's board icon shows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum BoardIcon {
    /// Grey: the board is closed. Closed, it reads no list, so it has nothing current to
    /// signal.
    #[default]
    Idle,
    /// Bold yellow: the board is open.
    Open,
}

impl TaskBoard {
    pub(super) fn icon(&self) -> BoardIcon {
        if self.open {
            BoardIcon::Open
        } else {
            BoardIcon::Idle
        }
    }

    /// The selected task, if the last list still has it and the last frame drew it (the
    /// filter or a space may hide it).
    fn selected_task(&self, hits: &TaskBoardHits) -> Option<&model::Task> {
        let RowKey::Task(key) = self.selected_drawn(hits)? else {
            return None;
        };
        self.store
            .list()?
            .tasks
            .iter()
            .find(|task| TaskKey::of(task) == *key)
    }

    /// The selected tab's pane, if the last frame drew its row.
    fn selected_agent(&self, hits: &TaskBoardHits) -> Option<&str> {
        match self.selected_drawn(hits)? {
            RowKey::Agent(pane) => Some(pane),
            RowKey::Task(_) => None,
        }
    }

    /// Selects `key`'s row; moving the selection closes the open row.
    fn select_row(&mut self, key: Option<RowKey>) {
        if self.selected != key {
            self.open_row = None;
        }
        self.selected = key;
    }

    /// Opens or closes the selected row's detail; false when the last frame drew no
    /// selected row to open, or the keyboard's focus is on a heading.
    fn open_selected_row(&mut self, open: bool, hits: &TaskBoardHits) -> bool {
        if self.shown_focused_heading(hits).is_some() {
            return false;
        }
        let Some(key) = self.selected_drawn(hits).cloned() else {
            return false;
        };
        self.open_row = open.then_some(key);
        self.reveal_selected = true;
        true
    }

    /// The selection, if the last frame drew its row.
    fn selected_drawn(&self, hits: &TaskBoardHits) -> Option<&RowKey> {
        let key = self.selected.as_ref()?;
        hits.spans
            .iter()
            .any(|(drawn, _, _)| drawn == key)
            .then_some(key)
    }

    /// The heading the keyboard's focus is on, while the last frame had it folded.
    fn shown_focused_heading(&self, hits: &TaskBoardHits) -> Option<Fold> {
        self.focused_heading.filter(|fold| match fold {
            Fold::Live => hits.live_folded,
            Fold::Done => hits.done_folded,
        })
    }

    /// After a frame: a focused heading the frame no longer had folded (LIVE with no tab
    /// working, DONE with nothing done) lets the focus go, so it does not come back with it.
    pub(super) fn drop_unshown_heading_focus(&mut self, hits: &TaskBoardHits) {
        self.focused_heading = self.shown_focused_heading(hits);
    }
}

/// What an endpoint error says: its message, or its code when the server sent an error
/// with an empty message (every other error the client makes has a message).
fn error_text(error: super::ClientShellEndpointError) -> String {
    match error {
        super::ClientShellEndpointError {
            code: Some(code),
            message,
        } if message.is_empty() => code,
        error => error.message,
    }
}

/// Lines the wheel scrolls the board by.
const WHEEL_LINES: usize = 3;

/// How often the open board asks for the list again, from the last ask.
const ASK_EVERY: Duration = Duration::from_secs(2);

impl TaskBoard {
    /// Another server, or the same server restarted: its tasks, and what was asked of it,
    /// are not this one's. Whether the board is open and its sort stay.
    pub(super) fn forget_endpoint(&mut self) {
        *self = Self {
            open: self.open,
            sort: self.sort,
            ..Self::default()
        };
    }
}

impl super::ClientShellState {
    pub(crate) fn tick_task_board(&mut self, now: Instant, outcome: &mut super::ClientShellInput) {
        // Closed, the board asks nothing: these two checks are all it costs.
        if !self.task_board.open {
            self.lapse_task_board_popup(now, outcome);
            return;
        }
        if self.task_board.asking.is_some() {
            return;
        }
        if self
            .task_board
            .asked_at
            .is_some_and(|at| now.saturating_duration_since(at) < ASK_EVERY)
        {
            return;
        }
        if !self.endpoint_is_online(&self.active_endpoint_id) {
            return;
        }
        let Some(boot_id) = self
            .snapshot
            .as_deref()
            .map(|snapshot| snapshot.boot_id.clone())
        else {
            return;
        };
        let method = crate::api::schema::Method::TasksList(Default::default());
        if !self.supports_endpoint_method(&method) {
            let connection = (self.active_endpoint_id.clone(), boot_id);
            if self.task_board.unsupported.as_ref() != Some(&connection) {
                self.task_board.store.receive(
                    Err("this server does not support tasks.list; update and restart it".into()),
                    now,
                );
                self.task_board.unsupported = Some(connection);
                outcome.repaint = true;
            }
            return;
        }
        let request_id = format!("client-shell-tasks:{}", self.next_request_id);
        self.next_request_id = self.next_request_id.saturating_add(1);
        self.task_board.asking = Some(request_id.clone());
        self.task_board.asked_at = Some(now);
        outcome
            .actions
            .push(super::ClientShellAction::EndpointSide {
                endpoint_id: self.active_endpoint_id.clone(),
                boot_id,
                request: Box::new(crate::api::schema::Request {
                    id: request_id,
                    method,
                }),
            });
    }

    /// `request_id` is the board's waiting `tasks.list`.
    pub(super) fn asks_task_list(&self, request_id: &str) -> bool {
        self.task_board.asking.as_deref() == Some(request_id)
    }

    /// Takes the reply to the board's `tasks.list` into its store; returns whether the board
    /// needs a repaint.
    pub(super) fn receive_task_list(
        &mut self,
        boot_id: &str,
        result: Result<crate::api::schema::ResponseResult, super::ClientShellEndpointError>,
    ) -> bool {
        self.task_board.asking = None;
        let crossed_action = std::mem::take(&mut self.task_board.reread);
        if crossed_action {
            self.task_board.asked_at = None;
        }
        if self
            .snapshot
            .as_deref()
            .is_none_or(|snapshot| snapshot.boot_id != boot_id)
        {
            return false;
        }
        let list = match result {
            Ok(crate::api::schema::ResponseResult::TasksList { list }) => {
                model::parse_task_list_value(list)
            }
            Ok(_) => Err("this server answered tasks.list with something else".into()),
            Err(error) => Err(error_text(error)),
        };
        // A failed read keeps the old list on screen; only a list read after the action ends
        // the wait.
        let stale = list.is_ok() && !crossed_action && self.task_board.list_stale;
        if stale {
            self.task_board.list_stale = false;
        }
        let mut changed = self.task_board.store.receive(list, Instant::now()) || stale;
        // New data can take the folded DONE heading away; its focus goes with it, so
        // a heading that comes back later does not come back focused.
        if self.task_board.focused_heading == Some(Fold::Done) && !self.shows_done() {
            self.task_board.focused_heading = None;
            changed = true;
        }
        // The open board rereads every two seconds: an unchanged list is not drawn again.
        self.task_board.open && changed
    }

    /// Whether the list read, in the space and filter the board shows, leaves DONE a
    /// task to show, and no section button hides it.
    fn shows_done(&self) -> bool {
        let board = &self.task_board;
        let words = filter::words(&board.filter);
        board
            .only_section
            .is_none_or(|section| section == model::TaskSection::Completed)
            && board.store.list().is_some_and(|list| {
                list.tasks.iter().any(|task| {
                    task.group == model::TaskGroup::Done
                        && board
                            .space
                            .as_ref()
                            .is_none_or(|space| task.space == *space)
                        && filter::task_matches(task, &words)
                })
            })
    }

    /// Forgets the board's `tasks.list` if `request_id` is it, cancelled or never sent, so
    /// the next tick asks again.
    pub(super) fn cancel_task_list(&mut self, request_id: &str) -> bool {
        let asking = self.task_board.asking.as_deref() == Some(request_id);
        if asking {
            self.task_board.asking = None;
            self.task_board.asked_at = None;
        }
        asking
    }

    /// Opens or closes the board. Opened by the sidebar's icon it shows the space you are in,
    /// with a focused worker's task selected; by its key, every space. Its view and scroll
    /// stay as they were while its space does.
    pub(super) fn toggle_task_board(&mut self, from: BoardOpener) {
        self.task_board.open = !self.task_board.open;
        // A question left open when the board closed (a click that went to an agent) would
        // take the next key; the board always opens without one.
        self.task_board.prompt = None;
        self.task_board.filter_editing = false;
        self.task_board.space_menu = false;
        self.task_board.agent_menu = false;
        self.task_board.sort_menu = false;
        self.task_board.work_filter_menu = false;
        self.task_board.reveal_selected = self.task_board.open;
        self.task_board.file_popup = FilePopup::None;
        self.task_board.focused_heading = None;
        if !self.task_board.open {
            return;
        }
        match from {
            BoardOpener::Sidebar => self.open_on_your_space(),
            BoardOpener::Key => match self.focused_board_agent() {
                Some(agent) => self.show_board_agent(agent),
                None => self.show_board_space(None),
            },
        }
    }

    /// The agent in the focused pane, a lead or a worker.
    fn focused_board_agent(&self) -> Option<BoardAgent> {
        let focused = self.snapshot.as_deref()?.focused_pane_id.as_deref()?;
        self.board_agent_on(focused)
    }

    /// The agent in `pane`, if one runs there.
    fn board_agent_on(&self, pane: &str) -> Option<BoardAgent> {
        board_agents(
            self.snapshot.as_deref(),
            &self.config,
            self.home_dir.as_deref(),
            0,
        )
        .into_iter()
        .find(|agent| agent.pane_id == pane)
    }

    /// The board on the space you are in, and the focused agent alone; a focused worker's task
    /// selected, in the Tasks view.
    fn open_on_your_space(&mut self) {
        let agent = self.focused_board_agent();
        let Some(snapshot) = self.snapshot.as_deref() else {
            return;
        };
        let space = snapshot
            .focused_workspace_id
            .as_deref()
            .and_then(|focused| {
                snapshot
                    .workspaces
                    .iter()
                    .find(|workspace| workspace.workspace_id == focused)
            })
            .map(|workspace| workspace.label.clone());
        let worker = agent.as_ref().filter(|agent| agent.lead.is_some());
        // A working worker's task in progress shows in the worker's IN PROGRESS row.
        let task = worker.and_then(|worker| {
            let list = self.task_board.store.list()?;
            worker_task(worker, list).map(|task| {
                let working = matches!(worker.state, AgentState::Working(_));
                if working && task.section() == model::TaskSection::InProgress {
                    RowKey::Agent(worker.pane_id.clone())
                } else {
                    RowKey::Task(TaskKey::of(task))
                }
            })
        });
        self.show_board_space(space);
        if let Some(agent) = agent {
            self.show_board_agent(agent);
        }
        if let Some(task) = task {
            self.set_board_view(BoardView::Tasks);
            self.task_board.select_row(Some(task));
        }
    }

    /// Narrows the board to `space` unless it already shows it, so its scroll stays.
    fn show_board_space(&mut self, space: Option<String>) {
        if self.task_board.space != space {
            self.set_board_space(space);
        }
    }

    /// Narrows the board to `agent`'s space and shows `agent` alone, unless it already does,
    /// so its scroll stays.
    fn show_board_agent(&mut self, agent: BoardAgent) {
        self.show_board_space(Some(agent.space));
        if self.task_board.agent.as_deref() != Some(agent.pane_id.as_str()) {
            self.set_board_agent(Some(agent.pane_id));
        }
    }

    /// The pane surface's popup came or went. When the file the board opened closes, the
    /// board comes back as it was: the same view, selection and scroll.
    pub(super) fn follow_task_board_popup(&mut self, popup: bool) {
        self.task_board.file_popup = match (self.task_board.file_popup, popup) {
            (FilePopup::Awaited(_), true) => FilePopup::Showing,
            (FilePopup::Lapsed(at), true) if at.elapsed() < LATE_POPUP => {
                self.task_board.open = false;
                FilePopup::Showing
            }
            // A popup that long after is not the file's: the board stops following it.
            (FilePopup::Lapsed(_), true) => FilePopup::None,
            (FilePopup::Showing, false) => {
                self.task_board.open = true;
                FilePopup::None
            }
            (state, _) => state,
        };
    }

    /// The board waits for its file's popup; when none has shown within `POPUP_LAPSE`, the
    /// board comes back as it would after the popup closed.
    fn lapse_task_board_popup(&mut self, now: Instant, outcome: &mut super::ClientShellInput) {
        let FilePopup::Awaited(since) = self.task_board.file_popup else {
            return;
        };
        if now.saturating_duration_since(since) < POPUP_LAPSE || self.popup_terminal_id.is_some() {
            return;
        }
        self.task_board.open = true;
        self.task_board.file_popup = FilePopup::Lapsed(now);
        outcome.repaint = true;
    }

    /// A key while the board is open; none of them reach the panes under it.
    pub(super) fn route_task_board_key(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut super::ClientShellInput,
    ) {
        use crossterm::event::KeyCode;
        let (mut code, mut modifiers) =
            crate::config::normalize_key_combo((key.code, key.modifiers));
        // Shift only makes a capital (`J`, `K`) or a typed character.
        if let KeyCode::Char(ch) = code {
            if modifiers.contains(KeyModifiers::SHIFT) {
                modifiers.remove(KeyModifiers::SHIFT);
                let shifted = key.shifted_codepoint.and_then(char::from_u32);
                code = KeyCode::Char(shifted.unwrap_or_else(|| ch.to_ascii_uppercase()));
            }
        }
        if !modifiers.is_empty() {
            return;
        }
        // An open sort menu takes ↑ and ↓ to move its check, sorting at once; any other key
        // closes it, and Esc and Enter do nothing else.
        if std::mem::take(&mut self.task_board.sort_menu) {
            outcome.repaint = true;
            match code {
                KeyCode::Up | KeyCode::Char('k') | KeyCode::Down | KeyCode::Char('j') => {
                    let down = matches!(code, KeyCode::Down | KeyCode::Char('j'));
                    self.set_board_sort(self.task_board.sort.step(down));
                    self.task_board.sort_menu = true;
                    return;
                }
                KeyCode::Esc | KeyCode::Enter => return,
                _ => {}
            }
        }
        // An open Filter menu takes ↑ and ↓ to move between its items and Enter or Space to
        // pick one, staying open; Esc and `f`, the key that opened it, close it, and any other
        // key closes it and does its own.
        if std::mem::take(&mut self.task_board.work_filter_menu) {
            outcome.repaint = true;
            let last = FilterPick::ALL.len() - 1;
            let cursor = self.task_board.work_filter_cursor.min(last);
            let stays = match code {
                KeyCode::Up | KeyCode::Char('k') => Some(cursor.saturating_sub(1)),
                KeyCode::Down | KeyCode::Char('j') => Some((cursor + 1).min(last)),
                KeyCode::Enter | KeyCode::Char(' ') => {
                    self.toggle_work_pick(FilterPick::ALL[cursor]);
                    Some(cursor)
                }
                KeyCode::Esc | KeyCode::Char('f') => return,
                _ => None,
            };
            if let Some(cursor) = stays {
                self.task_board.work_filter_cursor = cursor;
                self.task_board.work_filter_menu = true;
                return;
            }
        }
        // An open space or agent menu closes on any key; Esc does nothing else.
        let space_menu = std::mem::take(&mut self.task_board.space_menu);
        if space_menu | std::mem::take(&mut self.task_board.agent_menu) {
            outcome.repaint = true;
            if code == KeyCode::Esc {
                return;
            }
        }
        if self.task_board.filter_editing {
            self.route_filter_key(code);
            outcome.repaint = true;
            return;
        }
        if self.task_board.prompt.is_some() {
            self.route_prompt_key(code, outcome);
            outcome.repaint = true;
            return;
        }
        match code {
            KeyCode::Tab => self.switch_board_view(),
            // Esc closes the innermost thing first: a selected step, the agent shown alone,
            // the card panel, the filter, then the Filter menu's picks.
            KeyCode::Esc if self.task_board.work_step.is_some() => self.task_board.work_step = None,
            KeyCode::Esc if self.task_board.agent.is_some() => self.set_board_agent(None),
            KeyCode::Esc if self.shows_card_panel() => self.task_board.work_panel_closed = true,
            KeyCode::Esc if !self.task_board.filter.is_empty() => self.set_board_filter(""),
            KeyCode::Esc
                if self.task_board.view == BoardView::Work
                    && !self.task_board.work_filter.is_empty() =>
            {
                self.task_board.work_filter = WorkFilter::default();
                self.task_board.work_scroll = [0; 5];
            }
            KeyCode::Esc => self.task_board.open = false,
            KeyCode::Char('?') => self.task_board.keys_shown = !self.task_board.keys_shown,
            KeyCode::Char('/') => self.task_board.filter_editing = true,
            _ if self.task_board.view == BoardView::Work => {
                if !self.route_work_key(code, outcome) {
                    return;
                }
            }
            KeyCode::Char(action @ ('d' | 'x' | 'm' | 'J' | 'K' | 'n')) => {
                if !self.route_action_key(action, outcome) {
                    return;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => self.step_task_selection(false),
            KeyCode::Down | KeyCode::Char('j') => self.step_task_selection(true),
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Left | KeyCode::Char('h') => {
                let open = matches!(code, KeyCode::Right | KeyCode::Char('l'));
                if !self
                    .task_board
                    .open_selected_row(open, &self.hits.task_board)
                {
                    return;
                }
            }
            KeyCode::Enter => match self.task_board.shown_focused_heading(&self.hits.task_board) {
                Some(fold) => self.toggle_fold(fold),
                None => self.act_on_selection(outcome),
            },
            KeyCode::Char('o') => self.open_selected_task_file(outcome),
            // Only the Tasks view gets here: Work took its own keys above.
            KeyCode::Char('s') => self.task_board.sort_menu = true,
            KeyCode::Char('t') => {
                if !self.show_agent_task() {
                    return;
                }
            }
            KeyCode::Char('w') => {
                if !self.show_row_card() {
                    return;
                }
            }
            KeyCode::PageUp => {
                let page = self.hits.task_board.viewport.max(1);
                self.task_board.scroll = self.task_board.scroll.saturating_sub(page);
            }
            KeyCode::PageDown => {
                let page = self.hits.task_board.viewport.max(1);
                self.task_board.scroll = self
                    .task_board
                    .scroll
                    .saturating_add(page)
                    .min(self.hits.task_board.max_scroll);
            }
            _ => return,
        }
        outcome.repaint = true;
    }

    /// A key while the header's filter input is open: a character adds, backspace takes
    /// one back, enter keeps the text and returns the keys to the board, esc clears it.
    fn route_filter_key(&mut self, code: crossterm::event::KeyCode) {
        use crossterm::event::KeyCode;
        match code {
            KeyCode::Enter => {
                self.task_board.filter_editing = false;
                let kept = self.task_board.filter.trim().to_owned();
                self.set_board_filter(&kept);
            }
            KeyCode::Esc => {
                self.task_board.filter_editing = false;
                self.set_board_filter("");
            }
            code => {
                let mut text = self.task_board.filter.clone();
                edit_line(&mut text, code);
                self.set_board_filter(&text);
            }
        }
    }

    /// Narrows the board to `text`; a changed filter starts the lists at their tops.
    fn set_board_filter(&mut self, text: &str) {
        if self.task_board.filter != text {
            self.task_board.filter = text.to_owned();
            self.task_board.scroll = 0;
            self.task_board.work_scroll = [0; 5];
            self.task_board.reveal_selected = true;
            self.task_board.focused_heading = None;
        }
    }

    /// Whether the server takes `tasks.act`; without it the action keys do nothing and
    /// the footer leaves them out.
    pub(super) fn can_act_on_tasks(&self) -> bool {
        self.supports_endpoint_method(&crate::api::schema::Method::TasksAct(Default::default()))
    }

    /// An action key on the Tasks view; false when it does nothing.
    fn route_action_key(&mut self, action: char, outcome: &mut super::ClientShellInput) -> bool {
        if !self.can_act_on_tasks()
            || self.task_board.acting.is_some()
            || self.task_board.list_stale
        {
            return false;
        }
        let Some(task) = self
            .task_board
            .selected_task(&self.hits.task_board)
            .cloned()
        else {
            // A tab's row has no task to act on.
            if self
                .task_board
                .selected_agent(&self.hits.task_board)
                .is_some()
            {
                return false;
            }
            self.task_board.notice = Some("Select a task first, with ↑ ↓ or a click.".into());
            return true;
        };
        self.task_board.notice = None;
        let key = TaskKey::of(&task);
        // A move or a new row goes by the file's order: the board shows that order, so the
        // change shows at once.
        if matches!(action, 'J' | 'K' | 'n') {
            self.set_board_sort(TaskSort::File);
        }
        match action {
            'd' => self.send_task_act(&task, "done", Default::default(), outcome),
            'x' => self.task_board.prompt = Some((key, RowPrompt::Delete)),
            'm' => self.task_board.prompt = Some((key, RowPrompt::Move)),
            'n' => self.task_board.prompt = Some((key, RowPrompt::New(String::new()))),
            'J' | 'K' => {
                let direction = if action == 'J' { "down" } else { "up" };
                let params = crate::api::schema::TasksActParams {
                    direction: Some(direction.into()),
                    ..Default::default()
                };
                self.send_task_act(&task, "reorder", params, outcome);
            }
            _ => return false,
        }
        true
    }

    /// A key while a row's question or typed line is open; every key is the prompt's.
    fn route_prompt_key(
        &mut self,
        code: crossterm::event::KeyCode,
        outcome: &mut super::ClientShellInput,
    ) {
        use crossterm::event::KeyCode;
        let Some((key, prompt)) = self.task_board.prompt.take() else {
            return;
        };
        let task = self
            .task_board
            .store
            .list()
            .and_then(|list| list.tasks.iter().find(|task| TaskKey::of(task) == key))
            .cloned();
        let Some(task) = task else {
            // The task left the list while the question was open.
            return;
        };
        let move_to = |to: &str| crate::api::schema::TasksActParams {
            to: Some(to.to_owned()),
            ..Default::default()
        };
        match (prompt, code) {
            (RowPrompt::Delete, KeyCode::Char('y')) => {
                self.send_task_act(&task, "delete", Default::default(), outcome);
            }
            (RowPrompt::Move, KeyCode::Char('1')) => {
                self.send_task_act(&task, "move", move_to("you"), outcome);
            }
            (RowPrompt::Move, KeyCode::Char('2')) => {
                self.send_task_act(&task, "move", move_to("me"), outcome);
            }
            (RowPrompt::Move, KeyCode::Char('3')) => {
                self.task_board.prompt = Some((key, RowPrompt::MoveTo(String::new())));
            }
            (RowPrompt::MoveTo(_) | RowPrompt::New(_), KeyCode::Esc) => {}
            (RowPrompt::MoveTo(text), KeyCode::Enter) => {
                if !text.trim().is_empty() {
                    self.send_task_act(&task, "move", move_to(text.trim()), outcome);
                }
            }
            (RowPrompt::New(text), KeyCode::Enter) => {
                if !text.trim().is_empty() {
                    self.send_new_task(&task, text.trim(), outcome);
                }
            }
            (RowPrompt::MoveTo(mut text), code) => {
                edit_line(&mut text, code);
                self.task_board.prompt = Some((key, RowPrompt::MoveTo(text)));
            }
            (RowPrompt::New(mut text), code) => {
                edit_line(&mut text, code);
                self.task_board.prompt = Some((key, RowPrompt::New(text)));
            }
            // Any other answer keeps the task as it is.
            (RowPrompt::Delete | RowPrompt::Move, _) => {}
        }
    }

    /// Sends `action` on `task`, naming it by every copy the list gave, with `params`'
    /// other fields.
    fn send_task_act(
        &mut self,
        task: &model::Task,
        action: &str,
        params: crate::api::schema::TasksActParams,
        outcome: &mut super::ClientShellInput,
    ) {
        let copies = task
            .copies
            .iter()
            .map(|copy| crate::api::schema::TaskCopy {
                file: copy.file.clone(),
                row: copy.row.clone(),
            })
            .collect::<Vec<_>>();
        if copies.is_empty() {
            self.task_board.notice = Some("This task has no file row to change.".into());
            return;
        }
        let label = match (action, &params.to, &params.direction) {
            ("move", Some(to), _) => format!("move to {to}"),
            ("reorder", _, Some(direction)) => format!("move {direction}"),
            _ => action.to_owned(),
        };
        let params = crate::api::schema::TasksActParams {
            action: action.to_owned(),
            copies,
            ..params
        };
        self.send_act_request(TaskKey::of(task), label, params, outcome);
    }

    /// Adds `text` as a new task of mine in the file of `task`'s first copy.
    fn send_new_task(
        &mut self,
        task: &model::Task,
        text: &str,
        outcome: &mut super::ClientShellInput,
    ) {
        let Some(copy) = task.copies.first() else {
            self.task_board.notice = Some("This task has no file to add a task to.".into());
            return;
        };
        let params = crate::api::schema::TasksActParams {
            action: "new".into(),
            file: Some(copy.file.clone()),
            task: Some(text.to_owned()),
            who: Some("me".into()),
            ..Default::default()
        };
        self.send_act_request(TaskKey::of(task), "new task".into(), params, outcome);
    }

    /// Sends one `tasks.act` beside the command lane, as `tasks.list` goes; its reply is
    /// taken by its id.
    fn send_act_request(
        &mut self,
        task: TaskKey,
        label: String,
        params: crate::api::schema::TasksActParams,
        outcome: &mut super::ClientShellInput,
    ) {
        let boot_id = self
            .snapshot
            .as_deref()
            .map(|snapshot| snapshot.boot_id.clone())
            .filter(|_| self.endpoint_is_online(&self.active_endpoint_id));
        let Some(boot_id) = boot_id else {
            self.task_board.notice = Some("Not connected to the server; try again.".into());
            return;
        };
        let request_id = format!("client-shell-tasks-act:{}", self.next_request_id);
        self.next_request_id = self.next_request_id.saturating_add(1);
        self.task_board.acting = Some(Acting {
            request_id: request_id.clone(),
            task,
            label,
        });
        outcome
            .actions
            .push(super::ClientShellAction::EndpointSide {
                endpoint_id: self.active_endpoint_id.clone(),
                boot_id,
                request: Box::new(crate::api::schema::Request {
                    id: request_id,
                    method: crate::api::schema::Method::TasksAct(params),
                }),
            });
    }

    /// `request_id` is the board's waiting `tasks.act`.
    pub(super) fn acts_on_task(&self, request_id: &str) -> bool {
        self.task_board
            .acting
            .as_ref()
            .is_some_and(|acting| acting.request_id == request_id)
    }

    /// Takes the answer to the board's `tasks.act`: a refusal or an error goes to the
    /// notice line, and the list is read again either way. Returns whether to repaint.
    pub(super) fn receive_task_act(
        &mut self,
        boot_id: &str,
        result: Result<crate::api::schema::ResponseResult, super::ClientShellEndpointError>,
    ) -> bool {
        self.task_board.acting = None;
        if self
            .snapshot
            .as_deref()
            .is_none_or(|snapshot| snapshot.boot_id != boot_id)
        {
            return false;
        }
        self.task_board.notice = match result {
            Ok(crate::api::schema::ResponseResult::TasksAct { ok: true, .. }) => None,
            Ok(crate::api::schema::ResponseResult::TasksAct { ok: false, reason }) => Some(
                reason
                    .filter(|reason| !reason.is_empty())
                    .unwrap_or_else(|| "the task file refused the change".into()),
            ),
            Ok(_) => Some("this server answered tasks.act with something else".into()),
            Err(error) => Some(error_text(error)),
        };
        self.read_task_list_again();
        self.task_board.open
    }

    /// Forgets the board's `tasks.act` if `request_id` is it, cancelled or never sent; it
    /// may still have run, so the list is read again.
    pub(super) fn cancel_task_act(&mut self, request_id: &str) -> bool {
        if !self.acts_on_task(request_id) {
            return false;
        }
        self.task_board.acting = None;
        self.task_board.notice =
            Some("The action was interrupted; check the list before trying again.".into());
        self.read_task_list_again();
        true
    }

    /// The next tick asks for the list at once, and no action starts until it lands; a list
    /// already asked for may predate the action, so its reply asks again.
    fn read_task_list_again(&mut self) {
        self.task_board.list_stale = true;
        if self.task_board.asking.is_some() {
            self.task_board.reread = true;
        } else {
            self.task_board.asked_at = None;
        }
    }

    fn switch_board_view(&mut self) {
        self.set_board_view(match self.task_board.view {
            BoardView::Tasks => BoardView::Work,
            BoardView::Work => BoardView::Tasks,
        });
        self.task_board.reveal_selected = true;
    }

    /// Shows `view`; the keyboard's focus leaves the DONE heading.
    fn set_board_view(&mut self, view: BoardView) {
        self.task_board.view = view;
        self.task_board.focused_heading = None;
        // The sort menu is the Tasks view's, the Filter menu and a selected step the Work
        // view's: none stays, unseen, in the other.
        self.task_board.sort_menu &= view == BoardView::Tasks;
        self.task_board.work_filter_menu &= view == BoardView::Work;
        if view != BoardView::Work {
            self.task_board.work_step = None;
        }
    }

    /// Narrows the board to `space`, or shows every space; the list starts at its top, every
    /// agent shows, and the keyboard's focus leaves the DONE heading.
    fn set_board_space(&mut self, space: Option<String>) {
        self.task_board.space = space;
        self.task_board.scroll = 0;
        self.task_board.focused_heading = None;
        self.task_board.agent = None;
    }

    /// Orders every section's task rows by `sort`; the selected row stays in view.
    fn set_board_sort(&mut self, sort: TaskSort) {
        if self.task_board.sort != sort {
            self.task_board.sort = sort;
            self.task_board.reveal_selected = true;
        }
    }

    /// Shows the agent on `pane` alone, or every agent; the lists start at their tops, and a
    /// selected row or card the new filter hides is dropped.
    fn set_board_agent(&mut self, pane: Option<String>) {
        self.task_board.agent = pane;
        self.task_board.scroll = 0;
        self.task_board.work_scroll = [0; 5];
        self.task_board.reveal_selected = true;
        self.task_board.focused_heading = None;
        self.drop_hidden_selection();
    }

    /// Drops the selected Tasks row and the selected Work card when the agent shown alone
    /// hides them, so nothing unseen stays selected and the card panel shows only that
    /// agent's card.
    fn drop_hidden_selection(&mut self) {
        let agents = board_agents(
            self.snapshot.as_deref(),
            &self.config,
            self.home_dir.as_deref(),
            0,
        );
        let board = &self.task_board;
        let (Some(chosen), Some(list)) = (shown_agent(board, &agents), board.store.list()) else {
            return;
        };
        let row_hidden = match &board.selected {
            Some(RowKey::Agent(pane)) => *pane != chosen.pane_id,
            Some(RowKey::Task(key)) => list.tasks.iter().any(|task| {
                TaskKey::of(task) == *key && !render::in_view(board, Some(chosen), task)
            }),
            None => false,
        };
        let scope = work::Scope::of(board, &agents);
        let card_hidden = board.work_selected.as_ref().is_some_and(|file| {
            list.work
                .iter()
                .any(|card| card.file == *file && !scope.holds(card, &list.tasks))
        });
        if row_hidden {
            self.task_board.select_row(None);
        }
        if card_hidden {
            self.task_board.work_selected = None;
            self.task_board.work_step = None;
            self.task_board.work_panel_scroll = 0;
        }
    }

    /// A Work view key; false when the key is not one of its own.
    fn route_work_key(
        &mut self,
        code: crossterm::event::KeyCode,
        outcome: &mut super::ClientShellInput,
    ) -> bool {
        use crossterm::event::KeyCode;
        let panel_step = self.task_board.work_step;
        let down = matches!(code, KeyCode::Down | KeyCode::Char('j'));
        match code {
            // While a step is selected the arrows move between steps; up from the first, or
            // left or right, gives the keys back to the cards.
            KeyCode::Up | KeyCode::Char('k') if panel_step.is_some() => {
                self.task_board.work_step = panel_step.and_then(|step| step.checked_sub(1));
            }
            _ if down && panel_step.is_some() => {
                let last = self.hits.task_board.work_step_count.saturating_sub(1);
                self.task_board.work_step = panel_step.map(|step| (step + 1).min(last));
            }
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Right | KeyCode::Char('l')
                if panel_step.is_some() =>
            {
                self.task_board.work_step = None;
            }
            // Past the last card of its column the keys go into the panel's steps.
            _ if down && self.enters_card_panel() => {
                self.task_board.work_step = Some(0);
            }
            KeyCode::Up | KeyCode::Char('k') => self.step_work_selection(0, -1),
            KeyCode::Down | KeyCode::Char('j') => self.step_work_selection(0, 1),
            KeyCode::Left | KeyCode::Char('h') => self.step_work_selection(-1, 0),
            KeyCode::Right | KeyCode::Char('l') => self.step_work_selection(1, 0),
            KeyCode::Enter => return self.enter_card(outcome),
            KeyCode::Char('t') => self.show_card_tasks(),
            // The menu hangs under its button, so with no button drawn `f` opens nothing.
            KeyCode::Char('f') if !self.hits.task_board.work_filter_button.is_empty() => {
                self.task_board.work_filter_menu = true;
            }
            KeyCode::Char('o') => return self.open_card_file(outcome),
            KeyCode::PageDown => return self.page_work_column(true),
            KeyCode::PageUp => return self.page_work_column(false),
            _ => return false,
        }
        true
    }

    /// Whether the server takes `tasks.open`; without it `enter` on a card does nothing and
    /// the footer leaves it out.
    pub(super) fn can_open_task_files(&self) -> bool {
        self.supports_endpoint_method(&crate::api::schema::Method::TasksOpen(Default::default()))
    }

    /// Whether a file may be opened now: the server takes `tasks.open` and none is waiting.
    fn can_open_now(&self) -> bool {
        self.can_open_task_files() && self.task_board.opening.is_none()
    }

    /// Asks the server to open the selected card's intent file in a popup; false when it
    /// does nothing.
    fn open_card_file(&mut self, outcome: &mut super::ClientShellInput) -> bool {
        if !self.can_open_now() {
            return false;
        }
        let Some(file) = self.selected_card().map(|card| card.file.clone()) else {
            self.task_board.notice = Some("Select a card first, with ← ↑ ↓ → or a click.".into());
            return true;
        };
        self.open_task_file(file, outcome);
        true
    }

    /// Asks the server to open `file`, one the list names, in a popup, beside the command
    /// lane as `tasks.act` goes.
    fn open_task_file(&mut self, file: String, outcome: &mut super::ClientShellInput) {
        let boot_id = self
            .snapshot
            .as_deref()
            .map(|snapshot| snapshot.boot_id.clone())
            .filter(|_| self.endpoint_is_online(&self.active_endpoint_id));
        let Some(boot_id) = boot_id else {
            self.task_board.notice = Some("Not connected to the server; try again.".into());
            return;
        };
        self.task_board.notice = None;
        let request_id = format!("client-shell-tasks-open:{}", self.next_request_id);
        self.next_request_id = self.next_request_id.saturating_add(1);
        self.task_board.opening = Some(request_id.clone());
        outcome
            .actions
            .push(super::ClientShellAction::EndpointSide {
                endpoint_id: self.active_endpoint_id.clone(),
                boot_id,
                request: Box::new(crate::api::schema::Request {
                    id: request_id,
                    method: crate::api::schema::Method::TasksOpen(
                        crate::api::schema::TasksOpenParams { file },
                    ),
                }),
            });
    }

    /// `request_id` is the board's waiting `tasks.open`.
    pub(super) fn opens_task_file(&self, request_id: &str) -> bool {
        self.task_board.opening.as_deref() == Some(request_id)
    }

    /// Takes the answer to the board's `tasks.open`: once the popup is open the board closes
    /// so it shows over the panes, and comes back when it closes; an error goes to the notice
    /// line, and a file the server's list does not name reads the list again. Returns whether
    /// to repaint.
    pub(super) fn receive_task_open(
        &mut self,
        boot_id: &str,
        result: Result<crate::api::schema::ResponseResult, super::ClientShellEndpointError>,
    ) -> bool {
        self.task_board.opening = None;
        if self
            .snapshot
            .as_deref()
            .is_none_or(|snapshot| snapshot.boot_id != boot_id)
        {
            return false;
        }
        match result {
            Ok(_) => {
                self.task_board.notice = None;
                self.task_board.open = false;
                // The popup may have shown before this answer came.
                self.task_board.file_popup = if self.popup_terminal_id.is_some() {
                    FilePopup::Showing
                } else {
                    FilePopup::Awaited(Instant::now())
                };
                return true;
            }
            Err(error) => {
                if error.code.as_deref() == Some("tasks_not_listed") {
                    self.read_task_list_again();
                }
                self.task_board.notice = Some(error_text(error));
            }
        }
        self.task_board.open
    }

    /// Forgets the board's `tasks.open` if `request_id` is it, cancelled or never sent.
    pub(super) fn cancel_task_open(&mut self, request_id: &str) -> bool {
        if !self.opens_task_file(request_id) {
            return false;
        }
        self.task_board.opening = None;
        true
    }

    /// Moves the Work selection a column across or a card along, by the last frame's
    /// columns; with none selected, the first card of the first column that has one. Across
    /// keeps the card's row where the next column has one, and skips empty columns.
    fn step_work_selection(&mut self, across: isize, along: isize) {
        let columns = &self.hits.task_board.work_columns;
        let current = self.task_board.work_selected.as_ref().and_then(|file| {
            columns.iter().enumerate().find_map(|(column, cards)| {
                cards
                    .iter()
                    .position(|card| card == file)
                    .map(|row| (column, row))
            })
        });
        let next = match current {
            None => columns
                .iter()
                .position(|cards| !cards.is_empty())
                .map(|column| (column, 0)),
            Some((column, row)) if across == 0 => {
                let last = columns[column].len().saturating_sub(1);
                Some((column, row.saturating_add_signed(along).min(last)))
            }
            Some((column, row)) => {
                let mut next = column;
                loop {
                    match next.checked_add_signed(across) {
                        Some(candidate) if candidate < columns.len() => next = candidate,
                        _ => break None,
                    }
                    if !columns[next].is_empty() {
                        break Some((next, row.min(columns[next].len() - 1)));
                    }
                }
                .or(Some((column, row)))
            }
        };
        if let Some((column, row)) = next {
            let file = columns[column][row].clone();
            self.select_card(file);
            self.task_board.reveal_selected = true;
        }
    }

    /// Selects the card of `file` and shows its panel, the keys on the cards; another card's
    /// panel starts at its top.
    fn select_card(&mut self, file: String) {
        if self.task_board.work_selected.as_ref() != Some(&file) {
            self.task_board.work_panel_scroll = 0;
        }
        self.task_board.work_selected = Some(file);
        self.task_board.work_panel_closed = false;
        self.task_board.work_step = None;
    }

    /// Whether the next down arrow leaves the cards for the panel's first step: the last
    /// frame drew the panel with steps, and the selected card is the last of its column.
    fn enters_card_panel(&self) -> bool {
        let hits = &self.hits.task_board;
        if hits.work_step_count == 0 || hits.work_panel.is_empty() {
            return false;
        }
        let Some(file) = self.task_board.work_selected.as_ref() else {
            return false;
        };
        let columns = &hits.work_columns;
        columns.iter().any(|cards| {
            cards
                .iter()
                .position(|card| card == file)
                .is_some_and(|row| row + 1 == cards.len())
        })
    }

    /// The last frame drew the selected card's panel.
    fn shows_card_panel(&self) -> bool {
        self.task_board.view == BoardView::Work && !self.hits.task_board.panel_close.is_empty()
    }

    /// A chip: shows only its section, or all again when it already does; the lists start at
    /// their tops.
    fn toggle_category(&mut self, category: Category) {
        fn toggle<T: PartialEq>(only: &mut Option<T>, value: T) {
            *only = if only.as_ref() == Some(&value) {
                None
            } else {
                Some(value)
            };
        }
        match category {
            Category::Section(section) => toggle(&mut self.task_board.only_section, section),
        }
        self.task_board.scroll = 0;
        self.task_board.work_scroll = [0; 5];
        self.task_board.reveal_selected = true;
        self.task_board.focused_heading = None;
    }

    /// A Filter menu item: picks it in place of its group's other, or clears it when picked
    /// already; the columns start at their tops.
    fn toggle_work_pick(&mut self, pick: FilterPick) {
        self.task_board.work_filter.toggle(pick);
        self.task_board.work_scroll = [0; 5];
    }

    /// A pill's `×`: clears its pick; the one pill a narrow row sums them in (`None`) clears
    /// them all. The columns start at their tops.
    fn clear_work_pill(&mut self, pill: Option<FilterPick>) {
        match pill {
            Some(pick) => self.toggle_work_pick(pick),
            None => {
                self.task_board.work_filter = WorkFilter::default();
                self.task_board.work_scroll = [0; 5];
            }
        }
    }

    /// Whether a click on `target` now is the second of a double click: the same card or row
    /// clicked within the double-click time. A double click is not the first of another.
    fn is_double_click(&mut self, target: ClickTarget) -> bool {
        let now = Instant::now();
        let double = self.task_board.last_click.as_ref().is_some_and(|last| {
            last.target == target
                && now.saturating_duration_since(last.at) <= super::mouse::DOUBLE_CLICK_TIME
        });
        self.task_board.last_click = (!double).then_some(BoardClick { target, at: now });
        double
    }

    /// The last frame's column of the selected card.
    fn selected_card_column(&self) -> Option<usize> {
        let file = self.task_board.work_selected.as_ref()?;
        self.hits
            .task_board
            .work_columns
            .iter()
            .position(|cards| cards.contains(file))
    }

    /// Scrolls a Work column a card down or up; a column whose cards all fit stays put.
    fn scroll_work_column(&mut self, column: usize, down: bool) {
        let last = self.hits.task_board.work_columns[column]
            .len()
            .saturating_sub(1);
        let scroll = &mut self.task_board.work_scroll[column];
        *scroll = if down {
            scroll.saturating_add(1).min(last)
        } else {
            scroll.saturating_sub(1)
        };
    }

    /// Pages the selected card's column down or up, as its `▼ N` or `▲ N` does;
    /// false when there is no page that way.
    fn page_work_column(&mut self, down: bool) -> bool {
        let Some(column) = self.selected_card_column() else {
            return false;
        };
        let Some(first) = self
            .hits
            .task_board
            .work_markers
            .iter()
            .find(|marker| marker.column == column && marker.down == down)
            .map(|marker| marker.first)
        else {
            return false;
        };
        self.task_board.work_scroll[column] = first;
        true
    }

    /// The selected card, if the last list still has it and the last frame showed it (the
    /// filter or a space may hide it).
    fn selected_card(&self) -> Option<&model::WorkItem> {
        let file = self.task_board.work_selected.as_ref()?;
        if !self
            .hits
            .task_board
            .work_columns
            .iter()
            .flatten()
            .any(|drawn| drawn == file)
        {
            return None;
        }
        self.task_board
            .store
            .list()?
            .work
            .iter()
            .find(|card| card.file == *file)
    }

    /// `Enter` on the selected card: focuses its agent and closes the board, so its pane
    /// shows; without an agent, or with no card selected, goes as `o` does. False when it
    /// does nothing.
    fn enter_card(&mut self, outcome: &mut super::ClientShellInput) -> bool {
        let Some(card) = self.selected_card() else {
            return self.open_card_file(outcome);
        };
        let Some(pane_id) = work::agent_pane(card).map(str::to_owned) else {
            return self.open_card_file(outcome);
        };
        self.push_endpoint_method(
            crate::api::schema::Method::PaneFocus(crate::api::schema::PaneTarget { pane_id }),
            outcome,
        );
        self.task_board.open = false;
        true
    }

    /// The Tasks view, narrowed to the selected card's space.
    fn show_card_tasks(&mut self) {
        let Some(space) = self.selected_card().map(|card| card.space.clone()) else {
            return;
        };
        self.set_board_view(BoardView::Tasks);
        self.set_board_space((!space.is_empty()).then_some(space));
    }

    /// Moves the keyboard's focus one place down or up in the drawn order: the folded LIVE
    /// heading, each row, the folded DONE heading. With nothing selected, down starts at the
    /// first row (a heading when there is none) and up at the last row.
    fn step_task_selection(&mut self, down: bool) {
        #[derive(Clone, Copy)]
        enum Stop<'a> {
            Heading(Fold),
            Row(&'a RowKey),
        }
        let hits = &self.hits.task_board;
        let board = &self.task_board;
        let mut stops = Vec::new();
        if hits.live_folded {
            stops.push(Stop::Heading(Fold::Live));
        }
        stops.extend(hits.spans.iter().map(|(key, _, _)| Stop::Row(key)));
        if hits.done_folded {
            stops.push(Stop::Heading(Fold::Done));
        }
        let current = stops.iter().position(|stop| match *stop {
            Stop::Heading(fold) => board.shown_focused_heading(hits) == Some(fold),
            Stop::Row(key) => {
                board.focused_heading.is_none() && board.selected.as_ref() == Some(key)
            }
        });
        let is_row = |stop: &&Stop| matches!(stop, Stop::Row(_));
        let next = match current {
            Some(index) if down => stops.get(index + 1).or(stops.get(index)),
            Some(index) => stops.get(index.saturating_sub(1)),
            None if down => stops.iter().find(is_row).or(stops.first()),
            None => stops.iter().rev().find(is_row),
        };
        let next = next.copied().map(|stop| match stop {
            Stop::Heading(fold) => (Some(fold), None),
            Stop::Row(key) => (None, Some(key.clone())),
        });
        if let Some((heading, selected)) = next {
            self.task_board.focused_heading = heading;
            self.task_board.select_row(selected);
        }
        self.task_board.reveal_selected = true;
    }

    /// Folds or opens LIVE or DONE: a click on its heading, or Enter while the keyboard's
    /// focus is on it.
    fn toggle_fold(&mut self, fold: Fold) {
        match fold {
            Fold::Live => self.task_board.live_folded = !self.task_board.live_folded,
            Fold::Done => self.task_board.done_expanded = !self.task_board.done_expanded,
        }
        self.task_board.focused_heading = None;
    }

    /// Enter on the selection: a tab's row goes to that tab; a task goes to its agent while
    /// one is on it, else its space, and with no space to reach opens its notes file instead.
    fn act_on_selection(&mut self, outcome: &mut super::ClientShellInput) {
        if let Some(pane_id) = self.task_board.selected_agent(&self.hits.task_board) {
            self.go_to_task_agent(pane_id.to_owned(), outcome);
            return;
        }
        let Some(task) = self.task_board.selected_task(&self.hits.task_board) else {
            return;
        };
        let pane = task
            .agent
            .as_ref()
            .map(|agent| agent.pane.clone())
            .filter(|pane| !pane.is_empty());
        if let Some(pane_id) = pane {
            self.go_to_task_agent(pane_id, outcome);
            return;
        }
        let space = task.space.clone();
        let file = task.copies.first().map(|copy| copy.file.clone());
        match self.workspace_id_for_space(&space, file.as_deref()) {
            Some(workspace_id) => self.go_to_task_space(workspace_id, outcome),
            None => self.open_task_file_if_ready(file, outcome),
        }
    }

    /// The selected row's agent: an IN PROGRESS row's, or a waiting tab's.
    fn selected_board_agent(&self) -> Option<BoardAgent> {
        let pane = self.task_board.selected_agent(&self.hits.task_board)?;
        board_agents(
            self.snapshot.as_deref(),
            &self.config,
            self.home_dir.as_deref(),
            0,
        )
        .into_iter()
        .find(|agent| agent.pane_id == pane)
    }

    /// `t` on a selected agent's row: selects the task it is on and scrolls to it; false when
    /// there is none.
    fn show_agent_task(&mut self) -> bool {
        // A task in progress has no row of its own: the agent's row shows it.
        let key = self.selected_board_agent().and_then(|agent| {
            worker_task(&agent, self.task_board.store.list()?)
                .filter(|task| task.section() != model::TaskSection::InProgress)
                .map(|task| RowKey::Task(TaskKey::of(task)))
        });
        let Some(key) = key else {
            return false;
        };
        self.task_board.select_row(Some(key));
        self.task_board.focused_heading = None;
        self.task_board.reveal_selected = true;
        true
    }

    /// `w` on a selected row: the Work view with the card its task is on selected; false when
    /// there is none.
    fn show_row_card(&mut self) -> bool {
        let file = self.task_board.store.list().and_then(|list| {
            let card = match self.task_board.selected_task(&self.hits.task_board) {
                Some(task) => task_card(task, list),
                None => agent_card(&self.selected_board_agent()?, list),
            };
            card.map(|card| card.file.clone())
        });
        let Some(file) = file else {
            return false;
        };
        self.set_board_view(BoardView::Work);
        self.select_card(file);
        self.task_board.reveal_selected = true;
        true
    }

    /// `o` on the selected task: its notes file, when one exists and the server takes
    /// `tasks.open`. Unlike Enter, this never goes to the agent.
    fn open_selected_task_file(&mut self, outcome: &mut super::ClientShellInput) {
        let Some(task) = self.task_board.selected_task(&self.hits.task_board) else {
            return;
        };
        let file = task.copies.first().map(|copy| copy.file.clone());
        self.open_task_file_if_ready(file, outcome);
    }

    /// Opens `file` when the server takes `tasks.open`; does nothing without one, or without
    /// a file.
    fn open_task_file_if_ready(
        &mut self,
        file: Option<String>,
        outcome: &mut super::ClientShellInput,
    ) {
        if let Some(file) = file {
            if self.can_open_now() {
                self.open_task_file(file, outcome);
            }
        }
    }

    /// The one workspace `space` names, or, when more than one shares that label, the only
    /// one whose directory holds `file`; none when it names no workspace, or several and
    /// `file` cannot tell them apart.
    fn workspace_id_for_space(&self, space: &str, file: Option<&str>) -> Option<String> {
        let snapshot = self.snapshot.as_deref()?;
        let matches: Vec<&super::ClientShellWorkspace> = snapshot
            .workspaces
            .iter()
            .filter(|workspace| workspace.label == space)
            .collect();
        match matches.as_slice() {
            [] => None,
            [only] => Some(only.workspace_id.clone()),
            many => {
                // Under the Home policy every workspace starts in $HOME, so several can hold
                // the file: only one that alone holds it is a pick.
                let file = std::path::Path::new(file?);
                let mut holding = many
                    .iter()
                    .filter(|workspace| file.starts_with(&workspace.new_workspace_cwd));
                match (holding.next(), holding.next()) {
                    (Some(workspace), None) => Some(workspace.workspace_id.clone()),
                    _ => None,
                }
            }
        }
    }

    /// Focuses the task's agent in `pane_id` and closes the board, so its pane shows.
    fn go_to_task_agent(&mut self, pane_id: String, outcome: &mut super::ClientShellInput) {
        self.push_endpoint_method(
            crate::api::schema::Method::PaneFocus(crate::api::schema::PaneTarget { pane_id }),
            outcome,
        );
        self.task_board.open = false;
    }

    /// Focuses the task's space and closes the board, so it shows.
    fn go_to_task_space(&mut self, workspace_id: String, outcome: &mut super::ClientShellInput) {
        self.push_endpoint_method(
            crate::api::schema::Method::WorkspaceFocus(crate::api::schema::WorkspaceTarget {
                workspace_id,
            }),
            outcome,
        );
        self.task_board.open = false;
    }

    /// The agent whose sidebar row is at `point`: a tree row of its pane or its lone tab, or
    /// its mark on the rail or in the agents list.
    fn board_agent_at(&self, point: (u16, u16)) -> Option<BoardAgent> {
        use super::tree_sidebar::TreeTarget;
        let tree = self
            .hits
            .tree_rows
            .iter()
            .filter_map(|(rect, target)| match target {
                TreeTarget::Pane(pane) | TreeTarget::AgentTab { pane_id: pane, .. } => {
                    Some((rect, pane))
                }
                TreeTarget::Tab(_) | TreeTarget::NewTab(_) => None,
            });
        let pane = tree
            .chain(self.hits.agents.iter().map(|(rect, pane)| (rect, pane)))
            .find(|(rect, _)| super::contains(**rect, point))
            .map(|(_, pane)| pane)?;
        self.board_agent_on(pane)
    }

    /// The board's share of the mouse: its icon, its own clicks and wheel, and a space
    /// click while open. True when the board took the event.
    pub(super) fn route_task_board_mouse(
        &mut self,
        mouse: crossterm::event::MouseEvent,
        outcome: &mut super::ClientShellInput,
    ) -> bool {
        use crossterm::event::{MouseButton, MouseEventKind};
        let point = (mouse.column, mouse.row);
        let press = mouse.kind == MouseEventKind::Down(MouseButton::Left);
        if press && super::contains(self.hits.tree_task_board, point) {
            self.toggle_task_board(BoardOpener::Sidebar);
            outcome.repaint = true;
            return true;
        }
        if !self.task_board.open {
            return false;
        }
        let board = &self.hits.task_board;
        if super::contains(board.area, point) {
            if self.task_board.view == BoardView::Work
                && matches!(
                    mouse.kind,
                    MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                )
            {
                // The wheel scrolls the column under it a card at a time, the panel under
                // it by lines, and anywhere else the selected card's column.
                let down = mouse.kind == MouseEventKind::ScrollDown;
                let column = board
                    .work_column_areas
                    .iter()
                    .position(|area| super::contains(*area, point));
                if let Some(column) = column {
                    self.scroll_work_column(column, down);
                } else if super::contains(board.work_panel, point) {
                    // From where it drew, which a selected step may have moved; the step
                    // lets go so the lines stay where the wheel puts them.
                    let (max, drawn) = (board.work_panel_max_scroll, board.work_panel_scroll);
                    self.task_board.work_step = None;
                    self.task_board.work_panel_scroll = if down {
                        drawn.saturating_add(WHEEL_LINES).min(max)
                    } else {
                        drawn.saturating_sub(WHEEL_LINES)
                    };
                } else if let Some(column) = self.selected_card_column() {
                    self.scroll_work_column(column, down);
                } else {
                    return true;
                }
                outcome.repaint = true;
                return true;
            }
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    self.task_board.scroll = self.task_board.scroll.saturating_sub(WHEEL_LINES);
                }
                MouseEventKind::ScrollDown => {
                    self.task_board.scroll = self
                        .task_board
                        .scroll
                        .saturating_add(WHEEL_LINES)
                        .min(board.max_scroll);
                }
                MouseEventKind::Down(MouseButton::Left) if self.task_board.space_menu => {
                    // An open space menu takes the click: an item shows its space, anywhere
                    // else closes it.
                    self.task_board.space_menu = false;
                    if let Some(space) = board
                        .space_items
                        .iter()
                        .find(|(rect, _)| super::contains(*rect, point))
                        .map(|(_, space)| space.clone())
                    {
                        self.set_board_space(space);
                    }
                }
                MouseEventKind::Down(MouseButton::Left) if self.task_board.agent_menu => {
                    // An open agent menu takes the click as the space menu does.
                    self.task_board.agent_menu = false;
                    if let Some(pane) = board
                        .agent_items
                        .iter()
                        .find(|(rect, _)| super::contains(*rect, point))
                        .map(|(_, pane)| pane.clone())
                    {
                        self.set_board_agent(pane);
                    }
                }
                MouseEventKind::Down(MouseButton::Left) if self.task_board.sort_menu => {
                    // An open sort menu takes the click as the space menu does.
                    self.task_board.sort_menu = false;
                    if let Some(sort) = board
                        .sort_items
                        .iter()
                        .find(|(rect, _)| super::contains(*rect, point))
                        .map(|(_, sort)| *sort)
                    {
                        self.set_board_sort(sort);
                    }
                }
                MouseEventKind::Down(MouseButton::Left) if self.task_board.work_filter_menu => {
                    // An open Filter menu takes the click: an item picks or clears it and the
                    // menu stays; anywhere else closes it, a pill's `×` clearing its pick too.
                    let pick = board
                        .work_filter_items
                        .iter()
                        .find(|(rect, _)| super::contains(*rect, point))
                        .map(|(_, pick)| *pick);
                    self.task_board.work_filter_menu = pick.is_some();
                    if let Some(pick) = pick {
                        if let Some(index) = FilterPick::ALL.iter().position(|item| *item == pick) {
                            self.task_board.work_filter_cursor = index;
                        }
                        self.toggle_work_pick(pick);
                    } else if let Some(pill) = pill_at(board, point) {
                        self.clear_work_pill(pill);
                    }
                }
                MouseEventKind::Down(MouseButton::Left)
                    if super::contains(board.work_filter_button, point) =>
                {
                    self.task_board.work_filter_menu = true;
                }
                MouseEventKind::Down(MouseButton::Left) if pill_at(board, point).is_some() => {
                    if let Some(pill) = pill_at(board, point) {
                        self.clear_work_pill(pill);
                    }
                }
                MouseEventKind::Down(MouseButton::Left)
                    if super::contains(board.sort_button, point) =>
                {
                    self.task_board.sort_menu = true;
                }
                MouseEventKind::Down(MouseButton::Left)
                    if super::contains(board.space_button, point) =>
                {
                    self.task_board.space_menu = true;
                }
                MouseEventKind::Down(MouseButton::Left)
                    if super::contains(board.agent_button, point) =>
                {
                    self.task_board.agent_menu = true;
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    // A footer hint, a panel button or a stage arrow does its key.
                    let footer = board
                        .footer_keys
                        .iter()
                        .find(|(rect, _)| super::contains(*rect, point))
                        .and_then(|(_, key)| footer_key_code(key));
                    let arrow = board
                        .work_stage_arrows
                        .iter()
                        .find(|(rect, _)| super::contains(*rect, point))
                        .map(|(_, right)| match right {
                            true => crossterm::event::KeyCode::Right,
                            false => crossterm::event::KeyCode::Left,
                        });
                    if let Some(code) = footer.or(arrow) {
                        let key = crate::input::TerminalKey::new(code, KeyModifiers::NONE);
                        self.route_task_board_key(&key, outcome);
                        return true;
                    } else if super::contains(board.close, point) {
                        self.task_board.open = false;
                    } else if super::contains(board.panel_close, point) {
                        self.task_board.work_panel_closed = true;
                    } else if let Some(step) = board
                        .work_step_rows
                        .iter()
                        .find(|(rect, _)| super::contains(*rect, point))
                        .map(|(_, step)| *step)
                    {
                        self.task_board.work_step = Some(step);
                    } else if let Some(category) = board
                        .categories
                        .iter()
                        .find(|(rect, _)| super::contains(*rect, point))
                        .map(|(_, category)| *category)
                    {
                        self.toggle_category(category);
                    } else if super::contains(board.problems, point) {
                        self.task_board.problems_shown = !self.task_board.problems_shown;
                    } else if super::contains(board.filter_clear, point) {
                        self.task_board.filter_editing = false;
                        self.set_board_filter("");
                    } else if super::contains(board.filter, point) {
                        self.task_board.filter_editing = true;
                    } else if let Some(marker) = board
                        .work_markers
                        .iter()
                        .find(|marker| super::contains(marker.rect, point))
                    {
                        self.task_board.work_scroll[marker.column] = marker.first;
                    } else if super::contains(board.tasks_tab, point) {
                        self.set_board_view(BoardView::Tasks);
                    } else if super::contains(board.work_tab, point) {
                        self.set_board_view(BoardView::Work);
                    } else if let Some(file) = board
                        .cards
                        .iter()
                        .find(|(rect, _)| super::contains(*rect, point))
                        .map(|(_, file)| file.clone())
                    {
                        // One click selects; a double click opens it, as Enter does.
                        if self.is_double_click(ClickTarget::Card(file.clone()))
                            && self.task_board.work_selected.as_ref() == Some(&file)
                        {
                            self.open_card_file(outcome);
                        } else {
                            self.select_card(file);
                        }
                    } else if super::contains(board.back_to_all, point) {
                        self.set_board_space(None);
                    } else if super::contains(board.live_heading, point) {
                        self.toggle_fold(Fold::Live);
                    } else if super::contains(board.done_heading, point) {
                        self.toggle_fold(Fold::Done);
                    } else if let Some(key) = board
                        .rows
                        .iter()
                        .find(|(rect, _)| super::contains(*rect, point))
                        .map(|(_, key)| key.clone())
                    {
                        // One click selects, a click on the selected row opens or closes it,
                        // and a double click acts, as Enter does.
                        let selected = self.task_board.selected.as_ref() == Some(&key);
                        if self.is_double_click(ClickTarget::Row(key.clone())) && selected {
                            self.act_on_selection(outcome);
                        } else if selected && self.task_board.focused_heading.is_none() {
                            let open = self.task_board.open_row.as_ref() != Some(&key);
                            self.task_board
                                .open_selected_row(open, &self.hits.task_board);
                        } else {
                            self.task_board.select_row(Some(key));
                            self.task_board.focused_heading = None;
                            self.task_board.prompt = None;
                        }
                    }
                }
                _ => return true,
            }
            outcome.repaint = true;
            return true;
        }
        if !press {
            return false;
        }
        // An agent's tree row or rail mark shows that agent alone; the board stays and no
        // pane focuses.
        if let Some(agent) = self.board_agent_at(point) {
            self.show_board_agent(agent);
            outcome.repaint = true;
            return true;
        }
        // Any other tree row, a panel item or a pane without an agent focuses its pane as
        // always; the board closes so that pane shows.
        let focuses = self
            .hits
            .tree_rows
            .iter()
            .chain(&self.hits.tree_panel)
            .chain(&self.hits.tree_new_tab)
            .map(|(rect, _)| rect)
            .chain(self.hits.agents.iter().map(|(rect, _)| rect))
            .any(|rect| super::contains(*rect, point));
        if focuses {
            self.task_board.open = false;
            outcome.repaint = true;
            return false;
        }
        // A space row narrows the board to that space; its fold arrow still folds.
        let on_fold = self.hits.workspaces.iter().any(|hit| {
            hit.group_toggle
                .as_ref()
                .is_some_and(|(rect, _)| super::contains(*rect, point))
        });
        let label = self
            .active_endpoint_workspace_at(point)
            .filter(|_| !on_fold)
            .and_then(|workspace_id| {
                self.snapshot
                    .as_deref()?
                    .workspaces
                    .iter()
                    .find(|workspace| workspace.workspace_id == workspace_id)
                    .map(|workspace| workspace.label.clone())
            });
        let Some(label) = label else {
            return false;
        };
        self.set_board_space(Some(label));
        outcome.repaint = true;
        true
    }
}

/// The key a clickable footer hint names.
/// The pill whose `×` is at `point`, as `clear_work_pill` takes it; none when
/// no pill's `×` is there.
fn pill_at(board: &TaskBoardHits, point: (u16, u16)) -> Option<Option<FilterPick>> {
    board
        .work_filter_pills
        .iter()
        .find(|(rect, _)| super::contains(*rect, point))
        .map(|(_, pick)| *pick)
}

fn footer_key_code(key: &str) -> Option<crossterm::event::KeyCode> {
    use crossterm::event::KeyCode;
    match key {
        "Enter" => Some(KeyCode::Enter),
        "Tab" => Some(KeyCode::Tab),
        "Esc" => Some(KeyCode::Esc),
        "→" => Some(KeyCode::Right),
        "←" => Some(KeyCode::Left),
        key => {
            let mut chars = key.chars();
            match (chars.next(), chars.next()) {
                (Some(ch), None) => Some(KeyCode::Char(ch)),
                _ => None,
            }
        }
    }
}

/// A typed line's edit: a character adds, backspace takes the last one back.
fn edit_line(text: &mut String, code: crossterm::event::KeyCode) {
    match code {
        crossterm::event::KeyCode::Char(ch) if !ch.is_control() => text.push(ch),
        crossterm::event::KeyCode::Backspace => {
            text.pop();
        }
        _ => {}
    }
}

/// Where the last frame drew the board.
#[derive(Debug, Default, Clone)]
pub(super) struct TaskBoardHits {
    /// The whole board: a click anywhere in it is the board's.
    pub(super) area: Rect,
    /// Each visible row's lines, its detail block included.
    pub(super) rows: Vec<(Rect, RowKey)>,
    /// The header's `×` at the far right: closes the board.
    pub(super) close: Rect,
    /// The ` × ` beside a narrowed space's button: shows every space again.
    pub(super) back_to_all: Rect,
    /// Each task's first line and the line after its last, in the sections' order, before
    /// scrolling.
    pub(super) spans: Vec<(RowKey, usize, usize)>,
    /// How many lines of the sections fit.
    pub(super) viewport: usize,
    pub(super) max_scroll: usize,
    /// The scroll this frame drew with.
    pub(super) scroll: usize,
    /// The header's TASKS and WORK tabs.
    pub(super) tasks_tab: Rect,
    pub(super) work_tab: Rect,
    /// Whether the frame drew a working agent's spinner: its row, its task's, its card's,
    /// or its mark in the header's agent button or menu. Only then does the spinner clock
    /// turn for the board.
    pub(super) spins: bool,
    /// Each visible Work card, by its intent file.
    pub(super) cards: Vec<(Rect, String)>,
    /// Every card each Work column shows, in order, by intent file, for the arrow keys.
    pub(super) work_columns: Vec<Vec<String>>,
    /// Each Work column's card area, for the wheel.
    pub(super) work_column_areas: Vec<Rect>,
    /// The first card each Work column drew.
    pub(super) work_scroll: [usize; 5],
    /// The selected card's panel, for the wheel, and how far its lines scroll.
    pub(super) work_panel: Rect,
    pub(super) work_panel_max_scroll: usize,
    /// The scroll the panel drew with: the selected step pulls it into view.
    pub(super) work_panel_scroll: usize,
    /// How many steps the panel lists, and each drawn step line with its step.
    pub(super) work_step_count: usize,
    pub(super) work_step_rows: Vec<(Rect, usize)>,
    /// Each Work column's `▲ N` and `▼ N` in its label: its column, whether it scrolls down,
    /// and the first card of the page a click scrolls to.
    pub(super) work_markers: Vec<WorkMarker>,
    /// Each footer hint and panel button a click can do, by the key it names (`J/K` as `J`
    /// and `K`).
    pub(super) footer_keys: Vec<(Rect, &'static str)>,
    /// The Work stage picker's `◂` and `▸`, and whether it steps right: a click is `←` or
    /// `→`.
    pub(super) work_stage_arrows: Vec<(Rect, bool)>,
    /// The header's filter box: a click types into it.
    pub(super) filter: Rect,
    /// The filter box's `×` while a filter is kept: a click clears it.
    pub(super) filter_clear: Rect,
    /// The foot's short problem line: a click shows each problem with its path, or hides them.
    pub(super) problems: Rect,
    /// The header's space button: a click opens its menu.
    pub(super) space_button: Rect,
    /// The space menu's items, top down: none is every space.
    pub(super) space_items: Vec<(Rect, Option<String>)>,
    /// The header's agent button: a click opens its menu.
    pub(super) agent_button: Rect,
    /// The agent menu's items, top down, by pane: none is every agent.
    pub(super) agent_items: Vec<(Rect, Option<String>)>,
    /// The header's sort button: a click opens its menu.
    pub(super) sort_button: Rect,
    /// The sort menu's items, top down.
    pub(super) sort_items: Vec<(Rect, TaskSort)>,
    /// The Tasks view's chips.
    pub(super) categories: Vec<(Rect, Category)>,
    /// The Work view's Filter button: a click opens its menu.
    pub(super) work_filter_button: Rect,
    /// The Filter menu's items, top down.
    pub(super) work_filter_items: Vec<(Rect, FilterPick)>,
    /// Each pill's `×` and the pick it clears; none for the one pill that sums them all.
    pub(super) work_filter_pills: Vec<(Rect, Option<FilterPick>)>,
    /// The card panel's ` × `: closes the panel.
    pub(super) panel_close: Rect,
    /// LIVE's heading line, this frame: a click anywhere on it folds or opens it.
    pub(super) live_heading: Rect,
    /// LIVE is folded and its heading is in the list, drawn or scrolled off: Up past the
    /// first row reaches it.
    pub(super) live_folded: bool,
    /// DONE's heading line, this frame: a click anywhere on it folds or opens it.
    pub(super) done_heading: Rect,
    /// DONE is folded and its heading is in the list, drawn or scrolled off: Down past
    /// the last row reaches it.
    pub(super) done_folded: bool,
}

/// A Work column's `▲ N` or `▼ N`, the cards above or below it in its label.
#[derive(Debug, Clone)]
pub(super) struct WorkMarker {
    pub(super) rect: Rect,
    /// The column's stage index.
    pub(super) column: usize,
    /// It scrolls down (`▼`); else up.
    pub(super) down: bool,
    /// The first card of the page a click scrolls to.
    pub(super) first: usize,
}

/// The one place the board reads its data from: the last task list this client read, when
/// it read it, and the last read's error.
#[derive(Debug, Default)]
pub(super) struct TaskBoardStore {
    list: Option<TaskList>,
    read_at: Option<Instant>,
    error: Option<String>,
}

impl TaskBoardStore {
    pub(super) fn list(&self) -> Option<&TaskList> {
        self.list.as_ref()
    }

    // The time of the list shown; nothing draws it yet, tests read it.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn read_at(&self) -> Option<Instant> {
        self.read_at
    }

    pub(super) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Keeps a read list and clears the error; a failed read keeps the last list. Returns
    /// whether what the board draws from it changed.
    pub(super) fn receive(&mut self, result: Result<TaskList, String>, at: Instant) -> bool {
        match result {
            Ok(list) => {
                let changed = self.error.is_some()
                    || self.list.as_ref().is_none_or(|old| !old.draws_like(&list));
                self.list = Some(list);
                self.read_at = Some(at);
                self.error = None;
                changed
            }
            Err(error) => {
                let changed = self.error.as_deref() != Some(error.as_str());
                self.error = Some(error);
                changed
            }
        }
    }

    #[cfg(test)]
    pub(super) fn receive_json(&mut self, json: &str, at: Instant) {
        self.receive(model::parse_task_list(json), at);
    }
}
