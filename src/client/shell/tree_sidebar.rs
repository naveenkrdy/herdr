//! Tree layout for the expanded sidebar: each space behind a fold arrow with its main
//! lead's state (the agent in its first tab's first pane), then its git branch on a dim
//! line when `[ui.sidebar.spaces]` rows list `branch`, under it one branch per tab. A tab
//! with one pane is a single row (the agent it runs, or the tab's name); a split tab lists
//! its panes under its own row. A tab whose agent names the main lead hangs under the space;
//! one naming a lead pane in another tab of its space hangs under that tab. Rows are built
//! from the snapshot once per compose; render reads the rows and the spinner's clock.

use super::render::{
    display_width, put_segment, put_text, render_sidebar_background, ShellRenderState,
};
use super::*;
use crate::api::schema::AgentStatus;
use crate::config::WorkingSpinnerStyle;
use crate::protocol::{ClientShellAgent, ClientShellPane};

/// Top-level connectors sit under the space row's mark, one column into the row.
const TAB_INDENT: u16 = 1;

/// The space rows' mark column, from a row's padded start; the box's `!` sits in it.
pub(super) const MARK_COLUMN: u16 = TAB_INDENT;

/// The space rows' name column; the box's names and headings start in it.
pub(super) const NAME_COLUMN: u16 = TAB_INDENT + 2;

/// Deepest nesting drawn under a top-level tab; a tab nested deeper draws at this depth.
const MAX_NEST: usize = 3;

/// Each `ui.working_spinner` set's frames, in order.
pub(in crate::client::shell) fn spinner_frames(
    style: WorkingSpinnerStyle,
) -> &'static [&'static str] {
    match style {
        WorkingSpinnerStyle::Braille => &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"],
        WorkingSpinnerStyle::BrailleChase => &["⢎", "⠎", "⠊", "⠑", "⠱", "⡰", "⡠", "⢄"],
        WorkingSpinnerStyle::HalfCircles => &["◐", "◓", "◑", "◒"],
        // Nerd Font progress_spinner 1 to 6.
        WorkingSpinnerStyle::NerdSpinner => &[
            "\u{ee06}", "\u{ee07}", "\u{ee08}", "\u{ee09}", "\u{ee0a}", "\u{ee0b}",
        ],
        // Material Design circle slices 1 to 8.
        WorkingSpinnerStyle::CircleSlices => &[
            "\u{f0a9e}",
            "\u{f0a9f}",
            "\u{f0aa0}",
            "\u{f0aa1}",
            "\u{f0aa2}",
            "\u{f0aa3}",
            "\u{f0aa4}",
            "\u{f0aa5}",
        ],
        WorkingSpinnerStyle::Arc => &["◜", "◠", "◝", "◞", "◡", "◟"],
        WorkingSpinnerStyle::DotOrbit => &["⠁", "⠂", "⠄", "⡀", "⢀", "⠠", "⠐", "⠈"],
        WorkingSpinnerStyle::Pulse => &["·", "•", "●", "•"],
        WorkingSpinnerStyle::Line => &["|", "/", "─", "\\"],
        WorkingSpinnerStyle::Star => &["✶", "✸", "✹", "✺", "✹", "✷"],
        // Weather Icons moon phases, all 28.
        WorkingSpinnerStyle::MoonPhases => &[
            "\u{e38d}", "\u{e38e}", "\u{e38f}", "\u{e390}", "\u{e391}", "\u{e392}", "\u{e393}",
            "\u{e394}", "\u{e395}", "\u{e396}", "\u{e397}", "\u{e398}", "\u{e399}", "\u{e39a}",
            "\u{e39b}", "\u{e39c}", "\u{e39d}", "\u{e39e}", "\u{e39f}", "\u{e3a0}", "\u{e3a1}",
            "\u{e3a2}", "\u{e3a3}", "\u{e3a4}", "\u{e3a5}", "\u{e3a6}", "\u{e3a7}", "\u{e3a8}",
        ],
        WorkingSpinnerStyle::Diamond => &["◇", "◈", "◆", "◈"],
    }
}

/// How long a set shows each frame, in milliseconds: most sets turn every 100 ms, the
/// sets of a few broad shapes every 150 ms, and moon phases' 28 small steps every 50 ms.
pub(in crate::client::shell) const fn spinner_pace_ms(style: WorkingSpinnerStyle) -> u64 {
    match style {
        WorkingSpinnerStyle::HalfCircles
        | WorkingSpinnerStyle::Pulse
        | WorkingSpinnerStyle::Diamond => 150,
        WorkingSpinnerStyle::MoonPhases => 50,
        WorkingSpinnerStyle::NerdSpinner
        | WorkingSpinnerStyle::Braille
        | WorkingSpinnerStyle::CircleSlices
        | WorkingSpinnerStyle::Arc
        | WorkingSpinnerStyle::BrailleChase
        | WorkingSpinnerStyle::DotOrbit
        | WorkingSpinnerStyle::Line
        | WorkingSpinnerStyle::Star => 100,
    }
}

/// The set a working agent's row spins. A `nested` row (a worker: it names a lead and
/// leads no one) spins the `ui.sub_agent_spinner` set, which `auto` resolves to the main
/// set's partner; every other row spins the `ui.working_spinner` set. Either is braille
/// when its set needs the Nerd Font icons the tree is not drawing. Where that makes `auto`'s
/// partner the main set's own frames (circle slices and the Nerd spinner, both braille), the
/// worker spins the drawn main set's partner instead, so the two still look apart.
pub(in crate::client::shell) fn spinner_set(
    config: &ClientShellConfig,
    nested: bool,
) -> WorkingSpinnerStyle {
    let nerd = config.sidebar_tree_icons == crate::config::SidebarTreeIconsConfig::Nerd;
    let drawn = |style: WorkingSpinnerStyle| {
        if style.needs_nerd_font() && !nerd {
            WorkingSpinnerStyle::Braille
        } else {
            style
        }
    };
    let main = drawn(config.working_spinner);
    if !nested {
        return main;
    }
    let sub = drawn(config.sub_agent_spinner.resolve(config.working_spinner));
    if sub == main && config.sub_agent_spinner == crate::config::SubAgentSpinnerStyle::Auto {
        drawn(main.partner())
    } else {
        sub
    }
}

/// The spinner glyph of a working agent's row `spinner_ms` after the spinner started: its
/// [set](spinner_set)'s frame for that time at the set's own [pace](spinner_pace_ms), so a
/// lead and its worker each keep their pace and a late redraw lands on the right frame.
pub(in crate::client::shell) fn spinner_glyph(
    spinner_ms: u64,
    config: &ClientShellConfig,
    nested: bool,
) -> &'static str {
    let style = spinner_set(config, nested);
    let frames = spinner_frames(style);
    let frame = spinner_ms / spinner_pace_ms(style);
    // The remainder is below the frame count, so it fits a usize.
    frames[(frame % frames.len() as u64) as usize]
}

/// The mark of a finished agent, in every mark style, in the tree and the box.
pub(super) const DONE_MARK: &str = "✓";

/// A tree row's state mark, in either icon mode: the spinner while Working (a `nested`
/// row's, see [`spinner_glyph`]), `!` blocked and `✓` done in every mark style, else
/// Herdr's own for `ui.status_indicators`.
fn row_mark(
    status: AgentStatus,
    spinner_ms: u64,
    config: &ClientShellConfig,
    nested: bool,
) -> &'static str {
    match status {
        AgentStatus::Blocked => super::tree_panel::NEEDS_YOU_MARK,
        AgentStatus::Done => DONE_MARK,
        AgentStatus::Working => spinner_glyph(spinner_ms, config, nested),
        _ => status_icon(status, config.status_indicators),
    }
}

/// A lead's row mark, or a count's: see [`row_mark`].
pub(super) fn tree_mark(
    status: AgentStatus,
    spinner_ms: u64,
    config: &ClientShellConfig,
) -> &'static str {
    row_mark(status, spinner_ms, config, false)
}

/// The colour of a state's tree mark: `!` (blocked) red, as the yes-or-no mark reads
/// wherever it shows; every other state its own colour.
fn mark_color(status: AgentStatus, palette: &Palette) -> ratatui::style::Color {
    match status {
        AgentStatus::Blocked => palette.red,
        _ => status_color(status, palette),
    }
}

/// An agent's mark and its color, the one legend of the tree and its box: `?` or `!` while
/// it needs you, as the box lists it; else `tree_mark` in its state's color, the sub-agent
/// spinner while a `worker` works.
pub(in crate::client::shell) fn agent_mark(
    agent: &ClientShellAgent,
    spinner_ms: u64,
    config: &ClientShellConfig,
    worker: bool,
) -> (&'static str, ratatui::style::Color) {
    if let Some(reason) = super::tree_panel::needs_you(agent) {
        return super::tree_panel::reason_mark(reason, &config.palette);
    }
    let status = agent.agent_status;
    (
        row_mark(status, spinner_ms, config, worker),
        status_color(status, &config.palette),
    )
}

/// Where a tab row sits in its space's outline: its drawn depth below the top-level tabs,
/// whether no row of that depth follows before the outline steps back out, and per
/// shallower depth whether a `│` guide runs on past the row.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::client::shell) struct TabPlace {
    pub(super) depth: usize,
    pub(super) last: bool,
    pub(super) guides: [bool; MAX_NEST],
    /// Other tabs nest under this one: it leads them.
    pub(super) leads: bool,
}

/// Which agents' rows the tree shows, by the header's filter icon: all of them with the
/// plain shells, or only those that need you, work, or sit idle. Client state only.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::client::shell) enum TreeFilter {
    #[default]
    All,
    NeedsYou,
    Working,
    Idle,
}

impl TreeFilter {
    /// The filter a click on the icon steps to.
    pub(super) fn next(self) -> Self {
        match self {
            Self::All => Self::NeedsYou,
            Self::NeedsYou => Self::Working,
            Self::Working => Self::Idle,
            Self::Idle => Self::All,
        }
    }

    /// Whether `agent`'s row shows: one waiting on you by the ACTION REQUIRED box's own rule,
    /// one at work, or one idle and asking nothing.
    fn keeps(self, agent: &ClientShellAgent) -> bool {
        let needs_you = || super::tree_panel::needs_you(agent).is_some();
        match self {
            Self::All => true,
            Self::NeedsYou => needs_you(),
            Self::Working => agent.agent_status == AgentStatus::Working,
            Self::Idle => agent.agent_status == AgentStatus::Idle && !needs_you(),
        }
    }

    /// Whether a space's `counts` hold an agent this filter keeps.
    fn counts_in(self, counts: AgentCounts) -> bool {
        match self {
            Self::All => true,
            Self::NeedsYou => counts.needs_you > 0,
            Self::Working => counts.working > 0,
            Self::Idle => counts.idle > 0,
        }
    }

    /// The header icon and its colour: a dim funnel at rest, else the mark of the state
    /// shown.
    fn icon(self, palette: &Palette) -> (&'static str, ratatui::style::Color) {
        match self {
            Self::All => ("▽", palette.overlay1),
            Self::NeedsYou => ("?", palette.yellow),
            Self::Working => ("◐", palette.teal),
            Self::Idle => ("○", palette.overlay0),
        }
    }
}

/// Keeps, under a filter other than All, only the rows of agents it keeps and the rows of
/// the spaces holding one (folded or not), by the row's own lead and counts; plain shells,
/// tab and branch lines go. In place: no row is built for it.
fn filter_rows(rows: &mut Vec<TreeRow>, filter: TreeFilter) {
    if filter == TreeFilter::All {
        return;
    }
    rows.retain(|row| match *row {
        TreeRow::Space { agent, counts, .. } => {
            agent.is_some_and(|(_, agent)| filter.keeps(agent)) || filter.counts_in(counts)
        }
        TreeRow::AgentTab { agent, .. }
        | TreeRow::Pane {
            agent: Some(agent), ..
        } => filter.keeps(agent),
        _ => false,
    });
}

/// A space's agents a folded space row counts: those waiting on you, by the ACTION REQUIRED
/// box's rule, and those at work. The idle ones asking nothing are counted for the filter
/// alone; done agents are not counted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::client::shell) struct AgentCounts {
    needs_you: usize,
    working: usize,
    idle: usize,
}

impl AgentCounts {
    /// The count `agent` falls in, if any: the filter's own rule, [`TreeFilter::keeps`].
    fn slot(&mut self, agent: &ClientShellAgent) -> Option<&mut usize> {
        if TreeFilter::NeedsYou.keeps(agent) {
            Some(&mut self.needs_you)
        } else if TreeFilter::Working.keeps(agent) {
            Some(&mut self.working)
        } else if TreeFilter::Idle.keeps(agent) {
            Some(&mut self.idle)
        } else {
            None
        }
    }

    fn add(&mut self, agent: &ClientShellAgent) {
        if let Some(count) = self.slot(agent) {
            *count += 1;
        }
    }

    fn remove(&mut self, agent: &ClientShellAgent) {
        if let Some(count) = self.slot(agent) {
            *count = count.saturating_sub(1);
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(in crate::client::shell) enum TreeRow<'a> {
    /// A space; `folded` when it shows as one line without its tabs, and then `counts`
    /// stand for its agents but `agent`, whose mark the row shows. `phase` is the main lead's,
    /// from its own pane. The space row stands for `pane`, its first tab's first pane, which
    /// has no row of its own; `agent` is the space's main lead, the agent in that pane.
    /// `foldable` when rows would follow it open; a space without is never `folded` and has
    /// no fold arrow.
    Space {
        workspace: &'a ClientShellWorkspace,
        folded: bool,
        foldable: bool,
        counts: AgentCounts,
        phase: Option<&'static str>,
        pane: Option<&'a ClientShellPane>,
        agent: Option<(&'a ClientShellPane, &'a ClientShellAgent)>,
        /// Git ahead/behind on the row: only when the space has no branch line.
        ahead_behind: Option<(usize, usize)>,
    },
    /// The space's git branch on its own line under the space row: its first pane's, else
    /// its folder's. `lone_pane` is the pane the space row stands for, and whether an agent
    /// runs in it;
    /// `joined` when rows follow, so a `│` keeps the tree joined past the line.
    /// `ahead_behind` is the folder's, when the line shows the folder's branch.
    SpaceBranch {
        workspace: &'a ClientShellWorkspace,
        branch: &'a str,
        lone_pane: Option<(&'a ClientShellPane, bool)>,
        joined: bool,
        ahead_behind: Option<(usize, usize)>,
    },
    /// A tab's name or number; `lone_pane` is its only pane when that pane is a shell.
    /// `folded` is set for a split tab, whose pane rows follow unless it is folded.
    Tab {
        tab: &'a ClientShellTab,
        place: TabPlace,
        lone_pane: Option<&'a ClientShellPane>,
        folded: Option<bool>,
    },
    /// A tab with one agent among its panes: the row is that agent, where the tab row
    /// would be. `folded` is set when the tab has other panes, which follow as its `Pane` rows
    /// unless it is folded.
    AgentTab {
        tab: &'a ClientShellTab,
        pane: &'a ClientShellPane,
        agent: &'a ClientShellAgent,
        place: TabPlace,
        folded: Option<bool>,
        /// Drawn under its lead: nested under the lead's tab, or the main lead's worker
        /// under the space row.
        under_lead: bool,
    },
    /// A pane under its tab's row (or its tab's agent); `position` is its 1-based place
    /// among the tab's panes and `tab` its tab's place. Not `nested`: a pane of the main
    /// lead's tab, hanging under the space row as the main lead does.
    Pane {
        pane: &'a ClientShellPane,
        agent: Option<&'a ClientShellAgent>,
        position: usize,
        tab: TabPlace,
        last: bool,
        nested: bool,
        /// Its agent is the main lead's worker, drawn under the space row that stands for
        /// the main lead.
        under_lead: bool,
    },
    /// The branch of the row above, dim under its name, when it differs from the branch of
    /// the row it hangs under. `guides` hold, per level left of that name, whether a `│`
    /// runs on; `leads` when rows nest under the row, so a `│` keeps them joined.
    BranchLine {
        pane: &'a ClientShellPane,
        branch: &'a str,
        guides: [bool; MAX_NEST + 2],
        depth: usize,
        leads: bool,
    },
}

impl TreeRow<'_> {
    /// The pane this row stands for, so revealing a focused pane finds its row.
    fn pane(&self) -> Option<&ClientShellPane> {
        match *self {
            TreeRow::Space { .. } | TreeRow::SpaceBranch { .. } | TreeRow::BranchLine { .. } => {
                None
            }
            TreeRow::Tab { lone_pane, .. } => lone_pane,
            TreeRow::AgentTab { pane, .. } | TreeRow::Pane { pane, .. } => Some(pane),
        }
    }

    fn workspace_id(&self) -> &str {
        match *self {
            TreeRow::Space { workspace, .. } | TreeRow::SpaceBranch { workspace, .. } => {
                &workspace.workspace_id
            }
            TreeRow::Tab { tab, .. } => &tab.workspace_id,
            TreeRow::AgentTab { pane, .. }
            | TreeRow::Pane { pane, .. }
            | TreeRow::BranchLine { pane, .. } => &pane.workspace_id,
        }
    }

    /// The row standing for the focused pane: its lone tab, its agent, or its split pane.
    fn is_focus_row(&self) -> bool {
        match *self {
            // The space row stands for its first pane, and its branch line fills with it.
            TreeRow::Space { pane, .. } => pane.is_some_and(|pane| pane.focused),
            TreeRow::SpaceBranch { lone_pane, .. } => {
                lone_pane.is_some_and(|(pane, _)| pane.focused)
            }
            // A folded split tab stands for its hidden panes.
            TreeRow::Tab {
                tab,
                lone_pane,
                folded,
                ..
            } => lone_pane.map_or(folded == Some(true) && tab.focused, |pane| pane.focused),
            // A folded tab's agent also stands for its hidden panes.
            TreeRow::AgentTab {
                tab, pane, folded, ..
            } => pane.focused || (folded == Some(true) && tab.focused),
            TreeRow::Pane { pane, .. } | TreeRow::BranchLine { pane, .. } => pane.focused,
        }
    }
}

/// Background of the focused row, the tree's one fill. Where a palette's dim surface is its
/// active row, that tone barely parts from the sidebar, so the row takes the selection colour.
pub(super) fn focus_band(palette: &Palette, navigating: bool) -> ratatui::style::Color {
    use ratatui::style::Color;
    if palette.surface_dim == palette.active_row_bg
        && palette.selection_bg != Color::Reset
        && palette.selection_bg != palette.active_row_bg
    {
        palette.selection_bg
    } else {
        super::sidebar::workspace_active_background(palette, navigating)
    }
}

/// The faint block under the current space's rows, one level below the focused row's fill:
/// a quarter of the way from the theme's background to that fill. The background is its panel or its dim
/// surface, whichever lies farther from the fill; the terminal's own is unknown. None when
/// either colour is not RGB, as in the terminal theme.
fn space_block(palette: &Palette, focus: ratatui::style::Color) -> Option<ratatui::style::Color> {
    use ratatui::style::Color;
    let rgb = |color| match color {
        Color::Rgb(r, g, b) => Some([r, g, b]),
        _ => None,
    };
    let focus = rgb(focus)?;
    let distance = |color: [u8; 3]| -> u16 {
        color
            .iter()
            .zip(focus)
            .map(|(a, b)| u16::from(a.abs_diff(b)))
            .sum()
    };
    let background = [palette.panel_bg, palette.surface_dim]
        .into_iter()
        .filter_map(rgb)
        .max_by_key(|color| distance(*color))?;
    // A weighted mean of u8 values fits in a u8.
    let quarter = |a: u8, b: u8| ((3 * u16::from(a) + u16::from(b)) / 4) as u8;
    Some(Color::Rgb(
        quarter(background[0], focus[0]),
        quarter(background[1], focus[1]),
        quarter(background[2], focus[2]),
    ))
}

/// The current space's bar, down column 0 beside each of its rows.
const SPACE_BAR: &str = "▎";

/// Click target of a tab or pane row, a panel item, or a space row's `+`; the rest of a
/// space row uses the shared workspace hits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::client::shell) enum TreeTarget {
    Tab(String),
    Pane(String),
    /// A lone agent's row: a click focuses the agent's pane, its menu is the tab's.
    AgentTab {
        tab_id: String,
        pane_id: String,
    },
    /// A space row's `+`: a new plain tab in that space.
    NewTab(String),
}

impl TreeTarget {
    /// What a left click asks the server for: focus the tab or pane, or open the new tab.
    pub(super) fn click_method(self) -> crate::api::schema::Method {
        use crate::api::schema::{Method, PaneTarget, TabCreateParams, TabTarget};
        match self {
            TreeTarget::Tab(tab_id) => Method::TabFocus(TabTarget { tab_id }),
            TreeTarget::Pane(pane_id) | TreeTarget::AgentTab { pane_id, .. } => {
                Method::PaneFocus(PaneTarget { pane_id })
            }
            // Focused and unnamed: the `+` asks no name, whatever `prompt_new_tab_name` says.
            TreeTarget::NewTab(workspace_id) => Method::TabCreate(TabCreateParams {
                workspace_id: Some(workspace_id),
                cwd: None,
                focus: true,
                label: None,
                env: Default::default(),
            }),
        }
    }
}

/// Name shown for an agent: its Herdr session name (`herdr agent start NAME`,
/// `agent rename`); for Claude, else the title it sets, which carries its /rename
/// session name; else the agent panel's order. Other agents' titles are arbitrary.
pub(in crate::client::shell) fn pane_label(agent: &ClientShellAgent) -> &str {
    let claude_title = || {
        agent
            .agent
            .as_deref()
            .and_then(crate::detect::parse_agent_label)
            .filter(|kind| *kind == crate::detect::Agent::Claude)
            .and_then(|_| non_blank(agent.terminal_title_stripped.as_deref()))
    };
    non_blank(agent.name.as_deref())
        .or_else(claude_title)
        .or(agent.display_agent.as_deref())
        .or(agent.agent.as_deref())
        .or(agent.title.as_deref())
        .unwrap_or("agent")
}

/// The last part of the folder a shell's program runs in, else the shell's own: `~` for the
/// home folder, `/` for the root; nothing when the pane reports no folder. Control
/// characters go, as from branch names: the path comes from the pane's machine.
pub(super) fn folder_name<'a>(
    pane: &'a ClientShellPane,
    home_dir: Option<&str>,
) -> Option<std::borrow::Cow<'a, str>> {
    let path = non_blank(pane.foreground_cwd.as_deref()).or(non_blank(pane.cwd.as_deref()))?;
    let path = path.trim_end_matches(['/', '\\']);
    let is_home = |home: &str| home.trim_end_matches(['/', '\\']) == path;
    if path.is_empty() {
        Some("/".into())
    } else if home_dir.is_some_and(is_home) {
        Some("~".into())
    } else {
        path.rsplit(['/', '\\'])
            .next()
            .map(clean_branch)
            .filter(|name| !name.is_empty())
    }
}

fn non_blank(text: Option<&str>) -> Option<&str> {
    text.map(str::trim).filter(|text| !text.is_empty())
}

/// Key of a folded space in the client's collapsed groups; the sections sidebar's
/// worktree keys are paths, so the prefix keeps the two apart.
pub(in crate::client::shell) fn tree_fold_key(workspace_id: &str) -> String {
    format!("space:{workspace_id}")
}

/// Key of a folded split tab in the client's collapsed groups, beside the space keys.
pub(in crate::client::shell) fn tree_tab_fold_key(tab_id: &str) -> String {
    format!("tab:{tab_id}")
}

/// Drops the fold of every space with a Blocked agent, so an agent waiting on you is in
/// view; returns whether any fold went. A later fold by hand stays until the next block.
pub(in crate::client::shell) fn unfold_blocked_spaces(
    snapshot: &ClientShellSnapshot,
    folds: &mut HashSet<String>,
) -> bool {
    let mut unfolded = false;
    for agent in &snapshot.agents {
        if agent.agent_status == AgentStatus::Blocked {
            unfolded |= folds.remove(&tree_fold_key(&agent.workspace_id));
        }
    }
    unfolded
}

/// A branch or folder name without control characters: it comes raw from a HEAD file or a
/// pane's machine, and a hidden character must not make two equal names differ.
pub(super) fn clean_branch(branch: &str) -> std::borrow::Cow<'_, str> {
    if branch.contains(char::is_control) {
        std::borrow::Cow::Owned(branch.chars().filter(|ch| !ch.is_control()).collect())
    } else {
        std::borrow::Cow::Borrowed(branch)
    }
}

/// A branch with a visible name.
fn shown_branch(branch: Option<&str>) -> Option<&str> {
    branch.filter(|branch| !clean_branch(branch).trim().is_empty())
}

/// `branch` when it is not `above`, the branch of the row it hangs under.
fn changed_branch<'a>(branch: Option<&'a str>, above: Option<&str>) -> Option<&'a str> {
    shown_branch(branch).filter(|branch| above.map(clean_branch) != Some(clean_branch(branch)))
}

/// Levels left of a row's name: its guides and, last, its own level's.
fn line_guides(
    place: TabPlace,
    own: impl IntoIterator<Item = bool>,
) -> ([bool; MAX_NEST + 2], usize) {
    let mut guides = [false; MAX_NEST + 2];
    let mut depth = 0;
    for runs_on in place.guides[..place.depth].iter().copied().chain(own) {
        guides[depth] = runs_on;
        depth += 1;
    }
    (guides, depth)
}

/// A token's value when it has visible text.
pub(super) fn token<'a>(tokens: &'a [(String, String)], key: &str) -> Option<&'a str> {
    let value = tokens.iter().find(|(name, _)| name == key)?.1.trim();
    (!value.is_empty()).then_some(value)
}

/// Draw order of one space's tabs and their places, from each tab's lead tab (an index
/// into the same list). A tab follows its lead tab and that tab's earlier followers; tabs
/// with the same lead keep tab order. Tabs whose leads loop back to them stay top-level.
fn nest_tabs(leads: &[Option<usize>]) -> Vec<(usize, TabPlace)> {
    const NEW: u8 = 0;
    const WALKING: u8 = 1;
    const SETTLED: u8 = 2;
    let count = leads.len();
    let mut parents = leads.to_vec();
    let mut marks = vec![NEW; count];
    let mut walk = Vec::new();
    for start in 0..count {
        let mut at = Some(start);
        while let Some(index) = at {
            match marks[index] {
                NEW => {
                    marks[index] = WALKING;
                    walk.push(index);
                    at = parents[index];
                }
                WALKING => {
                    // Back on this walk: `index` and the tabs after it lead in a circle.
                    if let Some(from) = walk.iter().position(|&tab| tab == index) {
                        for &tab in &walk[from..] {
                            parents[tab] = None;
                        }
                    }
                    at = None;
                }
                _ => at = None,
            }
        }
        for tab in walk.drain(..) {
            marks[tab] = SETTLED;
        }
    }

    let mut children = vec![Vec::new(); count];
    let mut stack = Vec::new();
    for (index, parent) in parents.iter().enumerate().rev() {
        match parent {
            Some(parent) => children[*parent].push(index),
            None => stack.push((index, 0)),
        }
    }
    // Depth first; children were collected last-first, so popping keeps tab order.
    let mut order = Vec::with_capacity(count);
    while let Some((index, depth)) = stack.pop() {
        order.push((index, depth.min(MAX_NEST)));
        stack.extend(children[index].iter().map(|&child| (child, depth + 1)));
    }

    // Bottom up: `below[d]` holds while a later row sits at depth `d` before the outline
    // steps out past `d`.
    let mut below = [false; MAX_NEST + 1];
    let mut placed = order
        .iter()
        .rev()
        .map(|&(index, depth)| {
            let mut guides = [false; MAX_NEST];
            guides[..depth].copy_from_slice(&below[..depth]);
            let place = TabPlace {
                depth,
                last: !below[depth],
                guides,
                leads: !children[index].is_empty(),
            };
            below[depth] = true;
            below[depth + 1..].fill(false);
            (index, place)
        })
        .collect::<Vec<_>>();
    placed.reverse();
    placed
}

/// The panes the tree and its box count. A plugin's pane (an explorer split beside a lead)
/// counts for nothing: it is no row, never a tab's first pane or a space's main lead, and
/// never a second pane that keeps a lone agent's tab open.
pub(super) fn tree_panes(snapshot: &ClientShellSnapshot) -> impl Iterator<Item = &ClientShellPane> {
    snapshot
        .panes
        .iter()
        .filter(|pane| pane.plugin_id.is_none())
}

/// The pane a space's row stands for, as `tree_rows` picks it: its first tab's first pane.
pub(super) fn space_row_pane<'a>(
    snapshot: &'a ClientShellSnapshot,
    workspace_id: &str,
) -> Option<&'a ClientShellPane> {
    let tab = snapshot
        .tabs
        .iter()
        .find(|tab| tab.workspace_id == workspace_id)?;
    tree_panes(snapshot).find(|pane| pane.tab_id == tab.tab_id)
}

/// `branch_lines`: each space with a branch gets its branch line.
pub(in crate::client::shell) fn tree_rows<'a>(
    snapshot: &'a ClientShellSnapshot,
    folds: &HashSet<String>,
    branch_lines: bool,
) -> Vec<TreeRow<'a>> {
    // Index once per compose; each bucket keeps snapshot order.
    let agents = snapshot
        .agents
        .iter()
        .map(|agent| (agent.pane_id.as_str(), agent))
        .collect::<HashMap<_, _>>();
    let mut counts = HashMap::<&str, AgentCounts>::new();
    for agent in &snapshot.agents {
        counts
            .entry(agent.workspace_id.as_str())
            .or_default()
            .add(agent);
    }
    let mut tabs_by_workspace = HashMap::<&str, Vec<&ClientShellTab>>::new();
    // A tab's index within its space's list, for finding a lead's tab.
    let mut tab_index = HashMap::<&str, usize>::new();
    for tab in &snapshot.tabs {
        let tabs = tabs_by_workspace
            .entry(tab.workspace_id.as_str())
            .or_default();
        tab_index.insert(tab.tab_id.as_str(), tabs.len());
        tabs.push(tab);
    }
    let mut panes_by_tab = HashMap::<&str, Vec<&ClientShellPane>>::new();
    for pane in tree_panes(snapshot) {
        panes_by_tab
            .entry(pane.tab_id.as_str())
            .or_default()
            .push(pane);
    }
    let panes_by_id = tree_panes(snapshot)
        .map(|pane| (pane.pane_id.as_str(), pane))
        .collect::<HashMap<_, _>>();
    let panes_of = |tab: &ClientShellTab| {
        panes_by_tab
            .get(tab.tab_id.as_str())
            .map(Vec::as_slice)
            .unwrap_or_default()
    };
    // How many of a tab's panes run an agent.
    let agent_count = |panes: &[&ClientShellPane]| {
        panes
            .iter()
            .filter(|pane| agents.contains_key(pane.pane_id.as_str()))
            .count()
    };
    // The pane named by the tab's first `lead` token.
    let lead_of = |tab: &ClientShellTab| {
        panes_of(tab).iter().find_map(|pane| {
            agents
                .get(pane.pane_id.as_str())
                .and_then(|agent| token(&agent.tokens, "lead"))
        })
    };
    // The tab's lead pane when it is in another tab of the same space.
    let lead_pane = |tab: &ClientShellTab, lead: Option<&str>| {
        panes_by_id.get(lead?).copied().filter(|lead_pane| {
            lead_pane.workspace_id == tab.workspace_id && lead_pane.tab_id != tab.tab_id
        })
    };

    let mut rows = Vec::with_capacity(
        2 * snapshot.workspaces.len() + snapshot.tabs.len() + snapshot.panes.len(),
    );
    for workspace in &snapshot.workspaces {
        let tabs = tabs_by_workspace
            .get(workspace.workspace_id.as_str())
            .map(Vec::as_slice)
            .unwrap_or_default();
        // The first tab's first pane (layout order; the client has no root pane): the space
        // row stands for it, shell or agent, so a new space is one row. The main lead is the
        // agent in it; it shows on the space row, and drops out of its tab.
        let first_pane = tabs.first().and_then(|tab| panes_of(tab).first()).copied();
        let main_lead =
            first_pane.and_then(|pane| Some((pane, *agents.get(pane.pane_id.as_str())?)));
        let main_pane = first_pane.map(|pane| pane.pane_id.as_str());
        // Its tab has no row: its other panes, agents too, hang under the space row, so
        // no tab row's Close can take the first pane with them. Tab indexes then start one
        // later.
        let main_tab_gone = first_pane.is_some();
        let main_tab_panes = if main_tab_gone {
            panes_of(tabs[0])
        } else {
            &[]
        };
        let tabs = if main_tab_gone { &tabs[1..] } else { tabs };
        let foldable = !tabs.is_empty() || main_tab_panes.len() > 1;
        let folded = foldable && folds.contains(&tree_fold_key(&workspace.workspace_id));
        let phase = main_lead.and_then(|(_, agent)| lead_phase(agent));
        // The first pane's branch, else the folder's, else the one branch two or more other
        // panes all share, which then shows once, here, and not on each of them. Folded and
        // one-row spaces keep its line: it is part of the space row, nothing to fold.
        let shared_branch = || {
            let mut others = main_tab_panes
                .iter()
                .chain(tabs.iter().flat_map(|tab| panes_of(tab)))
                .filter(|pane| Some(pane.pane_id.as_str()) != main_pane)
                .map(|pane| shown_branch(pane.git_branch.as_deref()));
            let shared = others.next()??;
            let mut count = 1;
            for branch in others {
                (clean_branch(branch?) == clean_branch(shared)).then_some(())?;
                count += 1;
            }
            (count > 1).then_some(shared)
        };
        let space_branch = shown_branch(first_pane.and_then(|pane| pane.git_branch.as_deref()))
            .or(shown_branch(workspace.branch.as_deref()))
            .or_else(shared_branch);
        let line_branch = space_branch.filter(|_| branch_lines);
        rows.push(TreeRow::Space {
            workspace,
            folded,
            foldable,
            counts: {
                let mut counts = counts
                    .get(workspace.workspace_id.as_str())
                    .copied()
                    .unwrap_or_default();
                // The row's mark is its main lead's: counted once, there.
                if let Some((_, agent)) = main_lead {
                    counts.remove(agent);
                }
                counts
            },
            phase,
            pane: first_pane,
            agent: main_lead,
            ahead_behind: workspace.git_ahead_behind.filter(|_| line_branch.is_none()),
        });
        if let Some(branch) = line_branch {
            let folder = workspace.branch.as_deref().map(clean_branch);
            rows.push(TreeRow::SpaceBranch {
                workspace,
                branch,
                lone_pane: first_pane.map(|pane| (pane, main_lead.is_some())),
                joined: !folded && foldable,
                ahead_behind: workspace
                    .git_ahead_behind
                    .filter(|_| folder == Some(clean_branch(branch))),
            });
        }
        if folded {
            continue;
        }
        for (pane_index, pane) in main_tab_panes.iter().enumerate() {
            if Some(pane.pane_id.as_str()) == main_pane {
                continue;
            }
            let last = pane_index + 1 == main_tab_panes.len() && tabs.is_empty();
            let agent = agents.get(pane.pane_id.as_str()).copied();
            rows.push(TreeRow::Pane {
                pane,
                agent,
                position: pane_index + 1,
                tab: TabPlace::default(),
                last,
                nested: false,
                under_lead: agent.and_then(|agent| token(&agent.tokens, "lead")) == main_pane,
            });
            if let Some(branch) =
                changed_branch(pane.git_branch.as_deref(), space_branch).filter(|_| branch_lines)
            {
                let (guides, depth) = line_guides(TabPlace::default(), [!last]);
                rows.push(TreeRow::BranchLine {
                    pane,
                    branch,
                    guides,
                    depth,
                    leads: false,
                });
            }
        }
        let lead_panes = tabs
            .iter()
            .map(|tab| lead_pane(tab, lead_of(tab)))
            .collect::<Vec<_>>();
        // The main lead's workers are the space's own rows; others nest under their lead.
        let leads = lead_panes
            .iter()
            .map(|lead| {
                let lead = lead.filter(|lead| Some(lead.pane_id.as_str()) != main_pane)?;
                let index = *tab_index.get(lead.tab_id.as_str())?;
                index.checked_sub(usize::from(main_tab_gone))
            })
            .collect::<Vec<_>>();
        // Each tab's branch as the rows under it compare with: its lone pane's, else the
        // branch of the row it hangs under.
        let mut tab_branches = vec![None; tabs.len()];
        for (index, place) in nest_tabs(&leads) {
            let tab = tabs[index];
            let panes = panes_of(tab);
            let above = match leads[index] {
                Some(lead) if place.depth > 0 => tab_branches[lead],
                _ => space_branch,
            };
            // A tab with one agent is that agent's row; a lone shell is the tab's row.
            let lone = match panes {
                [pane] => Some((*pane, agents.get(pane.pane_id.as_str()).copied())),
                _ if agent_count(panes) == 1 => panes
                    .iter()
                    .find_map(|pane| Some((*pane, Some(*agents.get(pane.pane_id.as_str())?)))),
                _ => None,
            };
            if let Some((pane, agent)) = lone {
                let others = panes.len() > 1;
                let folded = others && folds.contains(&tree_tab_fold_key(&tab.tab_id));
                rows.push(match agent {
                    Some(agent) => TreeRow::AgentTab {
                        tab,
                        pane,
                        agent,
                        place,
                        folded: others.then_some(folded),
                        // Nested under the lead's tab, or the main lead's worker.
                        under_lead: place.depth > 0
                            || lead_panes[index]
                                .is_some_and(|lead| Some(lead.pane_id.as_str()) == main_pane),
                    },
                    None => TreeRow::Tab {
                        tab,
                        place,
                        lone_pane: Some(pane),
                        folded: None,
                    },
                });
                tab_branches[index] = shown_branch(pane.git_branch.as_deref()).or(above);
                let under = others && !folded;
                if let Some(branch) =
                    changed_branch(pane.git_branch.as_deref(), above).filter(|_| branch_lines)
                {
                    let (guides, depth) = line_guides(place, [!place.last]);
                    rows.push(TreeRow::BranchLine {
                        pane,
                        branch,
                        guides,
                        depth,
                        leads: place.leads || under,
                    });
                }
                if under {
                    let agent_pane = pane.pane_id.as_str();
                    let others = panes
                        .iter()
                        .enumerate()
                        .filter(|(_, other)| other.pane_id != agent_pane)
                        .collect::<Vec<_>>();
                    for (at, &(pane_index, pane)) in others.iter().enumerate() {
                        let last = at + 1 == others.len();
                        rows.push(TreeRow::Pane {
                            pane,
                            agent: None,
                            position: pane_index + 1,
                            tab: place,
                            last,
                            nested: true,
                            under_lead: false,
                        });
                        if let Some(branch) =
                            changed_branch(pane.git_branch.as_deref(), tab_branches[index])
                                .filter(|_| branch_lines)
                        {
                            let (guides, depth) = line_guides(place, [!place.last, !last]);
                            rows.push(TreeRow::BranchLine {
                                pane,
                                branch,
                                guides,
                                depth,
                                leads: false,
                            });
                        }
                    }
                }
                continue;
            }
            tab_branches[index] = above;
            let folded = folds.contains(&tree_tab_fold_key(&tab.tab_id));
            rows.push(TreeRow::Tab {
                tab,
                place,
                lone_pane: None,
                folded: Some(folded),
            });
            if folded {
                continue;
            }
            for (pane_index, pane) in panes.iter().enumerate() {
                if Some(pane.pane_id.as_str()) == main_pane {
                    continue;
                }
                let agent = agents.get(pane.pane_id.as_str()).copied();
                let last = pane_index + 1 == panes.len();
                rows.push(TreeRow::Pane {
                    pane,
                    agent,
                    position: pane_index + 1,
                    tab: place,
                    last,
                    nested: true,
                    under_lead: false,
                });
                if let Some(branch) =
                    changed_branch(pane.git_branch.as_deref(), above).filter(|_| branch_lines)
                {
                    let (guides, depth) = line_guides(place, [!place.last, !last]);
                    rows.push(TreeRow::BranchLine {
                        pane,
                        branch,
                        guides,
                        depth,
                        leads: false,
                    });
                }
            }
        }
    }
    rows
}

pub(in crate::client::shell) fn render_tree_sidebar(
    buffer: &mut Buffer,
    area: Rect,
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    state: &mut ShellRenderState<'_>,
    hits: &mut ShellHitMap,
) {
    let palette = &config.palette;
    render_sidebar_background(buffer, area, palette);
    hits.sidebar_divider = if area.is_empty() {
        Rect::default()
    } else {
        Rect::new(area.right().saturating_sub(1), area.y, 1, area.height)
    };
    let content = Rect::new(area.x, area.y, area.width.saturating_sub(1), area.height);
    if content.is_empty() {
        return;
    }
    // The header word starts in the mark column, the left edge of the rows under it.
    let header = padded(content);
    let header_x = header.x.saturating_add(MARK_COLUMN);
    put_text(
        buffer,
        header_x,
        content.y,
        header.right().saturating_sub(header_x),
        HEADER_WORD,
        Style::default().fg(palette.overlay0),
    );
    let branch_lines = config
        .spaces
        .rows
        .iter()
        .flatten()
        .any(|token| matches!(token.parts().0, crate::config::SpaceSidebarToken::Branch));
    let mut rows = tree_rows(snapshot, state.collapsed_groups, branch_lines);
    // A filter hides rows only while its icon shows, to say so and to take the click back.
    let filter = match config.mouse_capture && filter_icon_x(content).is_some() {
        true => state.tree_filter,
        false => TreeFilter::All,
    };
    filter_rows(&mut rows, filter);
    if config.mouse_capture {
        render_header_icons(
            buffer,
            content,
            every_space_folded(snapshot, state.collapsed_groups),
            (state.task_board_icon, state.tree_filter),
            config,
            hits,
        );
    }

    // The panel sits above the footer and takes at most a third of the sidebar.
    let panel_rows =
        super::tree_panel::render_tree_panel(buffer, content, snapshot, config, state, hits);
    // The header's own blank row is the only gap above the first space; `row_gap` rows
    // also stand after the last.
    let row_gap = config.spaces.row_gap;
    let edge_gap = if rows.is_empty() { 0 } else { row_gap };
    let lists = content
        .height
        .saturating_sub(WORKSPACE_HEADER_ROWS + 1 + panel_rows);
    let body = Rect::new(
        content.x,
        content.y.saturating_add(WORKSPACE_HEADER_ROWS),
        content.width,
        lists,
    );
    hits.workspace_body = body;
    let heights = vec![1; rows.len()];
    let gaps = (0..rows.len())
        .map(|index| match rows.get(index + 1) {
            // `row_gap` blank rows between spaces, as the sections sidebar, and after the last.
            Some(TreeRow::Space { .. }) => row_gap,
            None => edge_gap,
            _ => 0,
        })
        .collect::<Vec<_>>();
    let mut metrics =
        super::scroll::list_scroll_metrics(&heights, &gaps, body.height, *state.workspace_scroll);
    let reveal_navigation = !body.is_empty() && std::mem::take(state.reveal_navigation_workspace);
    let reveal_focus = !body.is_empty() && std::mem::take(state.reveal_focused_workspace);
    // Reveal in order: the space row, then its focused pane row, so a jump upward keeps
    // the space's own row on screen above the pane (the pane wins if both do not fit).
    let reveal_targets = if reveal_navigation {
        [
            rows.iter().position(|row| {
                matches!(row, TreeRow::Space { workspace, .. } if state.selected_workspace_id.is_some_and(
                    |target| target.matches(state.active_endpoint_id, &workspace.workspace_id)
                ))
            }),
            None,
        ]
    } else if reveal_focus {
        [
            rows.iter().position(
                |row| matches!(row, TreeRow::Space { workspace, .. } if workspace.focused),
            ),
            rows.iter()
                .position(|row| row.pane().is_some_and(|pane| pane.focused)),
        ]
    } else {
        [None, None]
    };
    for target in reveal_targets.into_iter().flatten() {
        *state.workspace_scroll = super::scroll::list_scroll_start_to_reveal(
            &heights,
            &gaps,
            body.height,
            *state.workspace_scroll,
            target,
        );
        metrics = super::scroll::list_scroll_metrics(
            &heights,
            &gaps,
            body.height,
            *state.workspace_scroll,
        );
    }
    hits.workspace_max_scroll = metrics.max_offset_from_bottom;
    hits.workspace_scroll_metrics = Some(metrics);
    *state.workspace_scroll = metrics
        .max_offset_from_bottom
        .saturating_sub(metrics.offset_from_bottom);
    let show_scrollbar = metrics.max_offset_from_bottom > 0 && body.width > 1;
    let row_width = body.width.saturating_sub(u16::from(show_scrollbar));
    let navigating = state.selected_workspace_id.is_some();
    let focus_band = focus_band(palette, navigating);
    let space_block = space_block(palette, focus_band);
    let focused_workspace = snapshot
        .workspaces
        .iter()
        .find(|workspace| workspace.focused)
        .map(|workspace| workspace.workspace_id.as_str());
    let working_times = state.working_times;
    // How long a pane's agent has been seen working.
    let worked = |pane_id: &str| {
        working_times.and_then(|(since, now)| {
            since
                .get(pane_id)
                .map(|since| now.saturating_duration_since(*since))
        })
    };
    let mut y = body.y;
    for (index, row) in rows.iter().enumerate().skip(*state.workspace_scroll) {
        if y >= body.bottom() {
            break;
        }
        // Rows and their bands keep one column of padding on each side; clicks take the
        // whole row.
        let hit = Rect::new(body.x, y, row_width, 1);
        let rect = padded(hit);
        let selected = matches!(row, TreeRow::Space { workspace, .. } | TreeRow::SpaceBranch { workspace, .. } if state
            .selected_workspace_id
            .is_some_and(|target| target.matches(state.active_endpoint_id, &workspace.workspace_id)));
        // The current space's rows sit on one faint block, its accent bar in the left pad;
        // every other space's rows keep a grey bar there, with no block. The row you are on
        // (or the space picked by keys) has a brighter fill inside it.
        let current = focused_workspace == Some(row.workspace_id());
        if let Some(block) = space_block.filter(|_| current) {
            buffer.set_style(hit, Style::default().bg(block));
        }
        // The row under the mouse takes the focused row's grey; a row with its own fill
        // keeps it. A dragged space's row takes the sections sidebar's drag fill.
        let hovered = state.hovered_row.is_some_and(|row| row.intersects(hit));
        let dragged = matches!(row, TreeRow::Space { .. } | TreeRow::SpaceBranch { .. })
            && state.dragged_workspace_id == Some(row.workspace_id());
        let background = if selected {
            Some(super::sidebar::workspace_selection_background(palette))
        } else if dragged {
            Some(palette.surface1)
        } else if row.is_focus_row() || hovered {
            Some(focus_band)
        } else {
            None
        };
        if let Some(background) = background {
            buffer.set_style(rect, Style::default().bg(background));
        }
        // One fixed cell per row: set it directly, with no width or grapheme work.
        if let Some(cell) = buffer.cell_mut((hit.x, y)).filter(|_| hit.width > 0) {
            let bar = if current {
                palette.accent
            } else {
                palette.overlay0
            };
            cell.set_symbol(SPACE_BAR)
                .set_style(Style::default().fg(bar));
        }
        match *row {
            TreeRow::Space {
                workspace,
                folded,
                foldable,
                counts,
                phase,
                pane,
                agent,
                ahead_behind,
            } => {
                // The `+` shows only on the space row under the mouse, the current one too.
                let plus = SpacePlus {
                    captured: config.mouse_capture,
                    shown: state.hovered_workspace_id == Some(workspace.workspace_id.as_str()),
                };
                let cells = render_space_row(
                    buffer,
                    rect,
                    (workspace, foldable.then_some(folded), counts, phase),
                    ahead_behind,
                    (
                        pane,
                        agent.map(|(pane, agent)| (agent, worked(&pane.pane_id))),
                    ),
                    plus,
                    state.spinner_ms,
                    config,
                );
                if let Some(plus) = cells.plus {
                    hits.tree_new_tab
                        .push((plus, TreeTarget::NewTab(workspace.workspace_id.clone())));
                }
                if let Some(pane) = pane {
                    space_pane_hits(hits, agent.is_some()).push((hit, pane.pane_id.clone()));
                }
                hits.workspaces.push(WorkspaceHit {
                    rect: hit,
                    endpoint_id: ClientEndpointId::Local,
                    workspace_id: workspace.workspace_id.clone(),
                    indented: false,
                    // The arrow reuses the sections sidebar's fold toggle, saved with it.
                    group_toggle: cells
                        .arrow
                        .map(|arrow| (arrow, tree_fold_key(&workspace.workspace_id))),
                });
            }
            TreeRow::SpaceBranch {
                workspace,
                branch,
                lone_pane,
                joined,
                ahead_behind,
            } => {
                // Under the space's name, after the `│` joining the rows below.
                if joined {
                    put_segment(
                        buffer,
                        rect.x.saturating_add(TAB_INDENT),
                        rect.y,
                        rect.right(),
                        "│",
                        Style::default().fg(palette.overlay0),
                    );
                }
                let x = rect.x.saturating_add(NAME_COLUMN);
                // Ends one column before the edge, as the other rows do; before ahead/behind
                // when it shows, at the right end where the rows' facts end.
                let mut end = rect.right().saturating_sub(1);
                let git = git_status_line(ahead_behind, palette);
                // A sidebar is far narrower than u16::MAX.
                let git_width = git.width() as u16;
                if git_width > 0 {
                    let git_end = facts_end(rect, config.mouse_capture);
                    let git_x = git_end.saturating_sub(git_width);
                    if git_x > x {
                        buffer.set_line(git_x, rect.y, &git, git_width);
                        end = git_x.saturating_sub(1);
                    }
                }
                put_branch_name(buffer, x, rect.y, end, branch, config);
                join_space_hits(hits, hit, workspace, lone_pane);
            }
            TreeRow::AgentTab {
                pane,
                agent,
                place,
                folded,
                under_lead,
                ..
            } => {
                let x = put_branch(buffer, rect, place, palette);
                // A name given by Rename pane wins, else the agent's whole name.
                let name = non_blank(pane.label.as_deref()).unwrap_or_else(|| pane_label(agent));
                let arrow = put_agent(
                    buffer,
                    rect,
                    x,
                    (pane, agent, name, worked(&pane.pane_id)),
                    (
                        agent_weight(agent, place.leads, tab_row_weight(place)),
                        folded,
                        place.leads,
                        under_lead,
                    ),
                    state.spinner_ms,
                    config,
                );
                if let Some(arrow) = arrow {
                    hits.tree_tab_folds.push((arrow, pane.tab_id.clone()));
                }
                hits.tree_rows.push((
                    hit,
                    TreeTarget::AgentTab {
                        tab_id: pane.tab_id.clone(),
                        pane_id: pane.pane_id.clone(),
                    },
                ));
            }
            TreeRow::Tab {
                tab,
                place,
                lone_pane,
                folded,
            } => {
                let arrow = render_tab_row(
                    buffer,
                    rect,
                    tab,
                    TabRowShape {
                        place,
                        lone_shell: lone_pane.is_some(),
                        folded,
                    },
                    state.spinner_ms,
                    config,
                );
                if let Some(arrow) = arrow {
                    hits.tree_tab_folds.push((arrow, tab.tab_id.clone()));
                }
                hits.tree_rows
                    .push((hit, TreeTarget::Tab(tab.tab_id.clone())));
            }
            TreeRow::Pane {
                pane,
                agent,
                position,
                tab,
                last,
                nested,
                under_lead,
            } => {
                render_pane_row(
                    buffer,
                    rect,
                    pane,
                    (agent, position, worked(&pane.pane_id), state.home_dir),
                    (tab, last, nested, under_lead),
                    state.spinner_ms,
                    config,
                );
                hits.tree_rows
                    .push((hit, TreeTarget::Pane(pane.pane_id.clone())));
            }
            TreeRow::BranchLine {
                branch,
                guides,
                depth,
                leads,
                ..
            } => {
                let dim = Style::default().fg(palette.overlay0);
                let mut x = rect.x.saturating_add(TAB_INDENT);
                for runs_on in &guides[..depth] {
                    x = put_segment(buffer, x, rect.y, rect.right(), guide(*runs_on), dim);
                }
                if leads {
                    x = put_segment(buffer, x, rect.y, rect.right(), "│ ", dim);
                }
                // Ends one column before the edge, as the other rows do.
                let end = rect.right().saturating_sub(1);
                put_branch_name(buffer, x, rect.y, end, branch, config);
                // Clicks and menus act as on the row above, whose hit grows over the line.
                if let Some((above, _)) = hits.tree_rows.last_mut() {
                    if above.bottom() == rect.y && above.x == hit.x {
                        above.height += 1;
                    }
                }
            }
        }
        y = y.saturating_add(1 + gaps[index]);
    }

    if show_scrollbar {
        let track = Rect::new(body.right().saturating_sub(1), body.y, 1, body.height);
        hits.workspace_scrollbar = track;
        super::scroll::render_list_scrollbar(buffer, track, metrics, palette);
    }

    // Where a dragged space would drop, as the sections sidebar draws it: in the body or on
    // the header's blank row above it, never on the header's word or the box below.
    if let Some(row) = state
        .workspace_drop_indicator_row
        .filter(|row| *row > content.y && *row >= body.y.saturating_sub(1) && *row < body.bottom())
    {
        put_text(
            buffer,
            body.x,
            row,
            body.width,
            &"─".repeat(usize::from(body.width)),
            Style::default().fg(palette.accent),
        );
    }

    render_tree_footer(buffer, content, snapshot, config, hits);
}

/// A branch line's text, dim, cut to end before `end`: in the Nerd set the git branch
/// glyph, a space and the name; plain, the name alone.
fn put_branch_name(
    buffer: &mut Buffer,
    x: u16,
    y: u16,
    end: u16,
    branch: &str,
    config: &ClientShellConfig,
) {
    let style = Style::default().fg(config.palette.overlay0);
    let branch = clean_branch(branch);
    match tree_icons(config.sidebar_tree_icons).branch {
        Some(glyph) => put_label(buffer, x, y, end, &format!("{glyph} {branch}"), style),
        None => put_label(buffer, x, y, end, &branch, style),
    };
}

/// A row under a space row acts as the space row for clicks: the space's hits grow over it,
/// so a space keeps one drop slot. A row alone at the top (its space row scrolled away)
/// gets its own.
fn join_space_hits(
    hits: &mut ShellHitMap,
    hit: Rect,
    workspace: &ClientShellWorkspace,
    lone_pane: Option<(&ClientShellPane, bool)>,
) {
    let joins = |above: Rect| above.bottom() == hit.y && above.x == hit.x;
    match hits.workspaces.last_mut() {
        Some(above) if above.workspace_id == workspace.workspace_id && joins(above.rect) => {
            above.rect.height += 1;
        }
        _ => hits.workspaces.push(WorkspaceHit {
            rect: hit,
            endpoint_id: ClientEndpointId::Local,
            workspace_id: workspace.workspace_id.clone(),
            indented: false,
            group_toggle: None,
        }),
    }
    if let Some((pane, agent)) = lone_pane {
        let space_panes = space_pane_hits(hits, agent);
        match space_panes.last_mut() {
            Some((above, pane_id)) if *pane_id == pane.pane_id && joins(*above) => {
                above.height += 1;
            }
            _ => space_panes.push((hit, pane.pane_id.clone())),
        }
    }
}

/// Where a space row's hit for its first pane goes: an agent's opens the agent's menu on a
/// right click; a shell's keeps the space's, so its Close closes the whole space.
fn space_pane_hits(hits: &mut ShellHitMap, agent: bool) -> &mut Vec<(Rect, String)> {
    if agent {
        &mut hits.tree_space_agents
    } else {
        &mut hits.tree_space_shells
    }
}

/// `area` without one column on each side: the tree's padding, for its rows and the box.
pub(super) fn padded(area: Rect) -> Rect {
    // Both pads come off the width; the left one also moves the start. A narrower area
    // gives up what it has.
    let shrink = area.width.min(2);
    let offset = shrink.min(1);
    Rect::new(
        area.x.saturating_add(offset),
        area.y,
        area.width - shrink,
        area.height,
    )
}

/// A space row's new tab `+`.
#[derive(Debug, Clone, Copy)]
struct SpacePlus {
    /// The mouse is captured, so the row keeps the `+` column whether or not it is drawn.
    captured: bool,
    /// Drawn on this row: the space row under the mouse.
    shown: bool,
}

/// Rows narrower than this keep every column for the arrow and the name, and have no `+`.
const SPACE_PLUS_MIN_WIDTH: u16 = 8;

/// Where a row's right-side facts end (exclusive), the same column on every tree row and
/// in the box: before the space rows' `+` column and the gap after it when the mouse is
/// captured, else one column before the edge.
pub(super) fn facts_end(rect: Rect, captured: bool) -> u16 {
    let plus_x = rect.right().saturating_sub(2);
    if captured && rect.width >= SPACE_PLUS_MIN_WIDTH {
        plus_x.saturating_sub(1)
    } else {
        plus_x.saturating_add(1)
    }
}

/// Draws a space row: its main lead's own mark when it has one, else its shell's, bold name
/// and its fold arrow, then at the right its main lead's phase word, else its
/// working time and, open, git ahead/behind or, folded, its agents by state. The name is bright
/// only while `pane`, the pane the row stands for, is focused. `fold` is `None` when there is
/// nothing to fold: no arrow. Returns the cells clicks act on.
fn render_space_row(
    buffer: &mut Buffer,
    rect: Rect,
    (workspace, fold, counts, phase): (
        &ClientShellWorkspace,
        Option<bool>,
        AgentCounts,
        Option<&'static str>,
    ),
    ahead_behind: Option<(usize, usize)>,
    (pane, agent): (
        Option<&ClientShellPane>,
        Option<(&ClientShellAgent, Option<std::time::Duration>)>,
    ),
    plus: SpacePlus,
    spinner_ms: u64,
    config: &ClientShellConfig,
) -> SpaceRowCells {
    let palette = &config.palette;
    let right = rect.right();
    let folded = fold == Some(true);
    let x = rect.x.saturating_add(MARK_COLUMN);
    // `agent` is the space's main lead: its own mark, and its time while it works;
    // `counts` leave it out. Without one, the shell's mark when the row stands for a
    // shell, else the column stays blank, so every space name starts in one column.
    let x = match agent {
        Some((agent, _)) => {
            let (mark, color) = agent_mark(agent, spinner_ms, config, false);
            put_mark(buffer, x, rect, mark, color)
        }
        None if pane.is_some() => put_shell_mark(buffer, x, rect, config),
        None => put_mark(buffer, x, rect, " ", palette.overlay0),
    };
    let worked = agent.and_then(|(agent, worked)| worked_text(Some((agent.agent_status, worked))));
    // A focused child leaves the name quiet, as the row's fill does.
    let label_style = Style::default()
        .fg(if pane.is_some_and(|pane| pane.focused) {
            palette.text
        } else {
            palette.subtext0
        })
        .add_modifier(Modifier::BOLD);
    // One column before the edge, as `⎇`: the `+`, or the facts when there is no `+`.
    let mark_x = right.saturating_sub(2);
    let plus_cell = (plus.captured && rect.width >= SPACE_PLUS_MIN_WIDTH)
        .then(|| Rect::new(mark_x, rect.y, 1, 1));
    let shown = plus_cell.filter(|_| plus.shown);
    if let Some(cell) = shown {
        put_segment(
            buffer,
            cell.x,
            cell.y,
            cell.right(),
            "+",
            Style::default().fg(palette.overlay1),
        );
    }
    // The facts end before the `+` column; the name gives way first.
    let facts_end = facts_end(rect, plus.captured);
    // The fold arrow follows the name, one space after it; folded, its hidden tabs' counts
    // follow the arrow.
    // The whole name first: the counts go, then git, then the phase or time.
    let name_width = display_width(&workspace.label);
    let counts_width = Some(counts_width(counts))
        .filter(|width| folded && name_width + 2 + width < facts_end.saturating_sub(x))
        .unwrap_or(0);
    let arrow_width = if fold.is_some() { 2 + counts_width } else { 0 };
    let room = facts_end
        .saturating_sub(x)
        .saturating_sub(name_width + arrow_width + 1);
    let facts = [FactsCut::All, FactsCut::WordOnly]
        .into_iter()
        .map(|cut| space_facts((phase, worked.clone(), ahead_behind), folded, cut, config))
        // A sidebar is far narrower than u16::MAX.
        .find(|facts| facts.width() as u16 <= room)
        .unwrap_or_default();
    let facts_width = facts.width() as u16;
    let facts_x = facts_end.saturating_sub(facts_width).max(x);
    if facts_width > 0 {
        buffer.set_line(facts_x, rect.y, &facts, facts_end.saturating_sub(facts_x));
    }
    let name_right = if facts_width > 0 {
        facts_x.saturating_sub(1).max(x)
    } else {
        facts_end
    }
    .saturating_sub(arrow_width)
    .max(x);
    let name = crate::ui::truncate_end(&workspace.label, usize::from(name_right.saturating_sub(x)));
    let name_end = put_segment(buffer, x, rect.y, name_right, &name, label_style);
    let arrow = fold.and_then(|folded| put_fold_arrow(buffer, name_end, rect, folded, config));
    if let Some(arrow) = arrow.filter(|_| counts_width > 0) {
        put_counts(
            buffer,
            (
                arrow.right(),
                rect.y,
                name_right.saturating_add(arrow_width),
            ),
            counts,
            spinner_ms,
            config,
        );
    }
    SpaceRowCells { plus: shown, arrow }
}

/// The cells a space row draws that clicks act on.
struct SpaceRowCells {
    /// The new tab `+`, when drawn.
    plus: Option<Rect>,
    /// The fold arrow, when the space folds and the arrow fits.
    arrow: Option<Rect>,
}

/// A fold arrow in grey one space after a name ending at `name_end`, from the
/// tree's icon set. Returns its cell, or nothing when the row has no room for it.
fn put_fold_arrow(
    buffer: &mut Buffer,
    name_end: u16,
    rect: Rect,
    folded: bool,
    config: &ClientShellConfig,
) -> Option<Rect> {
    let icons = tree_icons(config.sidebar_tree_icons);
    let x = name_end.saturating_add(1);
    if x >= rect.right() {
        return None;
    }
    put_segment(
        buffer,
        x,
        rect.y,
        rect.right(),
        if folded { icons.folded } else { icons.open },
        Style::default().fg(config.palette.overlay1),
    );
    Some(Rect::new(x, rect.y, 1, 1))
}

/// How much of a space row's facts to draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FactsCut {
    All,
    /// The phase word alone, or the time.
    WordOnly,
}

/// A space row's right side: its main lead's phase word in its color, else how long the
/// main lead has worked; then, open, git
/// ahead/behind, one space apart; empty when there is none of them. A folded row's counts
/// sit after its arrow instead.
fn space_facts(
    (phase, worked, ahead_behind): (Option<&'static str>, Option<String>, Option<(usize, usize)>),
    folded: bool,
    cut: FactsCut,
    config: &ClientShellConfig,
) -> ratatui::text::Line<'static> {
    use ratatui::text::Span;
    let palette = &config.palette;
    let mut spans = Vec::new();
    spans.extend(stage_or_worked(phase, worked, palette));
    let facts = if cut != FactsCut::All || folded {
        Vec::new()
    } else {
        git_status_line(ahead_behind, palette).spans
    };
    if !spans.is_empty() && !facts.is_empty() {
        spans.push(Span::raw(" "));
    }
    spans.extend(facts);
    ratatui::text::Line::from(spans)
}

/// A lead's phase word in its color, else how long it has worked, dim.
fn stage_or_worked(
    phase: Option<&'static str>,
    worked: Option<String>,
    palette: &Palette,
) -> Option<ratatui::text::Span<'static>> {
    use ratatui::text::Span;
    match (phase, worked) {
        (Some(word), _) => Some(Span::styled(
            word,
            Style::default().fg(stage_color(word, palette)),
        )),
        (None, Some(worked)) => Some(Span::styled(worked, Style::default().fg(palette.overlay0))),
        (None, None) => None,
    }
}

/// How long a working agent has worked (see [`minutes_text`]). Nothing when it is not
/// working.
fn worked_text(agent: Option<(AgentStatus, Option<std::time::Duration>)>) -> Option<String> {
    let (AgentStatus::Working, Some(worked)) = agent? else {
        return None;
    };
    minutes_text(worked)
}

/// A time in whole minutes (`4m`, `1h15m`); nothing under a minute.
pub(super) fn minutes_text(time: std::time::Duration) -> Option<String> {
    let minutes = time.as_secs() / 60;
    match (minutes / 60, minutes % 60) {
        (0, 0) => None,
        (0, minutes) => Some(format!("{minutes}m")),
        (hours, 0) => Some(format!("{hours}h")),
        (hours, minutes) => Some(format!("{hours}h{minutes}m")),
    }
}

/// A lead's phase word, from its pane's `stage` token as `herdr-stage` writes it on the lead's
/// own pane (`build · herdr-w1, herdr-w2 on 2 tasks`). None when the word is no phase, and for
/// a worker (it has a `lead`): it shows its time instead.
fn lead_phase(agent: &ClientShellAgent) -> Option<&'static str> {
    if token(&agent.tokens, "lead").is_some() {
        return None;
    }
    phase_word(token(&agent.tokens, "stage")?)
}

/// The phase word of a lead's stage token (`review · 2 findings` is Review, `check` is
/// Verify), never its raw text; None for a word that is no phase (`waiting`).
pub(super) fn phase_word(stage: &str) -> Option<&'static str> {
    let word = stage.split('·').next().unwrap_or_default().trim();
    [
        ("plan", "Plan"),
        ("build", "Build"),
        ("review", "Review"),
        ("check", "Verify"),
        ("verify", "Verify"),
        ("ship", "Ship"),
    ]
    .into_iter()
    .find(|(raw, _)| word.eq_ignore_ascii_case(raw))
    .map(|(_, phase)| phase)
}

/// Color of a phase word (see [`phase_word`]).
pub(super) fn stage_color(word: &str, palette: &Palette) -> ratatui::style::Color {
    use super::task_board::model::WorkStage;
    let stage = match word {
        "Plan" => WorkStage::Plan,
        "Build" => WorkStage::Build,
        "Review" => WorkStage::Review,
        "Verify" => WorkStage::Check,
        "Ship" => WorkStage::Shipped,
        _ => WorkStage::Unknown,
    };
    super::task_board::style::stage_role(stage, palette).unwrap_or(palette.overlay0)
}

/// The columns a folded row's counts take after its arrow: a space, then `?N` and the
/// working mark and its count, one space apart; none when both are zero.
fn counts_width(counts: AgentCounts) -> u16 {
    let digits = |count: usize| count.checked_ilog10().map_or(1, |log| log + 1) as u16;
    [counts.needs_you, counts.working]
        .into_iter()
        .filter(|count| *count > 0)
        .map(|count| 2 + digits(count))
        .sum()
}

/// A folded row's counts from `x` to `end` on row `y`: a yellow `?` and how many wait on you,
/// then the sub-agent spinner and how many work, each number dim.
fn put_counts(
    buffer: &mut Buffer,
    (x, y, end): (u16, u16, u16),
    counts: AgentCounts,
    spinner_ms: u64,
    config: &ClientShellConfig,
) {
    let palette = &config.palette;
    let dim = Style::default().fg(palette.overlay0);
    let (ask, yellow) = super::tree_panel::reason_mark(super::tree_panel::Reason::Answer, palette);
    let mut x = x;
    for (mark, color, count) in [
        (ask, yellow, counts.needs_you),
        (
            spinner_glyph(spinner_ms, config, true),
            status_color(AgentStatus::Working, palette),
            counts.working,
        ),
    ] {
        if count == 0 {
            continue;
        }
        x = put_segment(
            buffer,
            x.saturating_add(1),
            y,
            end,
            mark,
            Style::default().fg(color),
        );
        x = put_segment(buffer, x, y, end, &count.to_string(), dim);
    }
}

/// Ahead/behind in the sections sidebar's token format, all dim; empty when both are zero.
fn git_status_line(
    ahead_behind: Option<(usize, usize)>,
    palette: &Palette,
) -> ratatui::text::Line<'static> {
    let Some((ahead, behind)) = ahead_behind else {
        return ratatui::text::Line::default();
    };
    let tokens = [crate::ui::ResolvedToken {
        kind: crate::ui::ResolvedTokenKind::GitStatus { ahead, behind },
        style: Default::default(),
    }];
    let dim = Style::default().fg(palette.overlay0);
    let spans = crate::ui::resolved_token_spans(
        &tokens,
        ("", dim),
        dim,
        dim,
        dim,
        dim,
        palette,
        usize::MAX,
    );
    ratatui::text::Line::from(
        spans
            .into_iter()
            .map(|span| span.patch_style(dim))
            .collect::<Vec<_>>(),
    )
}

/// A tab row's weight: bold for a lead (tabs nest under it), normal for a nested worker,
/// else bold only while focused.
#[derive(Debug, Clone, Copy)]
enum Weight {
    Bold,
    Normal,
    FocusBold,
}

fn tab_row_weight(place: TabPlace) -> Weight {
    if place.leads {
        Weight::Bold
    } else if place.depth > 0 {
        Weight::Normal
    } else {
        Weight::FocusBold
    }
}

/// An agent row's weight: a worker (it names a lead) that leads no one is normal weight,
/// focused or not, also under the main lead; else as `weight`.
fn agent_weight(agent: &ClientShellAgent, leads: bool, weight: Weight) -> Weight {
    if is_lead(agent, leads) {
        weight
    } else {
        Weight::Normal
    }
}

/// A lead: an agent that names no lead, or one other tabs nest under (`leads`).
fn is_lead(agent: &ClientShellAgent, leads: bool) -> bool {
    leads || token(&agent.tokens, "lead").is_none()
}

/// A lead's name within `width` columns: cut in its middle, its last `-part` kept whole
/// (`oakw…-lead`). A name without a `-part`, or with no room for `x…-part`, is cut at its
/// end.
pub(super) fn cut_lead_name(name: &str, width: usize) -> std::borrow::Cow<'_, str> {
    if usize::from(display_width(name)) <= width {
        return name.into();
    }
    let Some(dash) = name.rfind('-').filter(|&dash| dash > 0) else {
        return crate::ui::truncate_end(name, width).into();
    };
    let (head, tail) = name.split_at(dash);
    // The head is wider than its room here, so it is cut and ends in `…`.
    match width.checked_sub(usize::from(display_width(tail)) + 1) {
        Some(head_room @ 1..) => {
            format!("{}{tail}", crate::ui::truncate_end(head, head_room + 1)).into()
        }
        _ => crate::ui::truncate_end(name, width).into(),
    }
}

/// The tree's icons in one set: plain Unicode every font draws, or Nerd Font glyphs.
pub(super) struct TreeIcons {
    /// Opens the task board.
    pub(super) board: &'static str,
    pub(super) search: &'static str,
    /// Folds every space; shown while any space is open.
    pub(super) fold_all: &'static str,
    /// Opens every space; shown while all are folded.
    pub(super) open_all: &'static str,
    pub(super) new: &'static str,
    /// A row's fold arrow while open, after its name.
    pub(super) open: &'static str,
    /// A row's fold arrow while folded.
    pub(super) folded: &'static str,
    /// A shell's mark before its name, where an agent shows its state.
    pub(super) shell: &'static str,
    /// Before a branch line's name; plain has none.
    pub(super) branch: Option<&'static str>,
}

const PLAIN_ICONS: TreeIcons = TreeIcons {
    board: "☑",
    // The tree's own fold arrows, for all spaces at once.
    search: "/",
    fold_all: "▾",
    open_all: "▸",
    new: "+",
    open: "▾",
    folded: "▸",
    shell: "·",
    branch: None,
};

/// Material Design checkbox marked outline, magnify, chevron double up and down, and plus;
/// Octicons chevron down and right, Devicons terminal, Octicons git branch.
const NERD_ICONS: TreeIcons = TreeIcons {
    board: "\u{f0135}",
    search: "\u{f0349}",
    fold_all: "\u{f013f}",
    open_all: "\u{f013c}",
    new: "\u{f0415}",
    open: "\u{f47c}",
    folded: "\u{f460}",
    shell: "\u{e795}",
    branch: Some("\u{f418}"),
};

pub(super) fn tree_icons(icons: crate::config::SidebarTreeIconsConfig) -> &'static TreeIcons {
    match icons {
        crate::config::SidebarTreeIconsConfig::Plain => &PLAIN_ICONS,
        crate::config::SidebarTreeIconsConfig::Nerd => &NERD_ICONS,
    }
}

/// The footer's menu icon: `≡`, or the Nerd Font bars.
fn menu_icon(icons: crate::config::SidebarTreeIconsConfig) -> &'static str {
    match icons {
        crate::config::SidebarTreeIconsConfig::Plain => "≡",
        // Font Awesome bars.
        crate::config::SidebarTreeIconsConfig::Nerd => "\u{f0c9}",
    }
}

/// The footer: `≡ menu` at the left in the tree's mark column, `«` alone at the right in the
/// header's last icon column so the last column stays clear; the header's `+` makes a new
/// space, so there is no `new`. The menu's attention dot takes the icon's place.
fn render_tree_footer(
    buffer: &mut Buffer,
    content: Rect,
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    hits: &mut ShellHitMap,
) {
    if content.is_empty() {
        return;
    }
    let palette = &config.palette;
    let y = content.bottom().saturating_sub(1);
    let dim = Style::default().fg(palette.overlay0);
    let toggle_x = content.right().saturating_sub(2);
    hits.sidebar_toggle = Rect::new(toggle_x, y, u16::from(content.width > 1), 1);
    put_segment(buffer, toggle_x, y, toggle_x.saturating_add(1), "«", dim);
    // `≡ menu`, six columns, and at least one space before `«`.
    let launcher_x = padded(content).x.saturating_add(MARK_COLUMN);
    if !config.mouse_capture || launcher_x.saturating_add(7) > toggle_x {
        return;
    }
    hits.global_launcher = Rect::new(launcher_x, y, 6, 1);
    let (icon, icon_style) = if super::global_menu::global_menu_attention(snapshot) {
        ("●", Style::default().fg(palette.accent))
    } else {
        (menu_icon(config.sidebar_tree_icons), dim)
    };
    put_segment(
        buffer,
        launcher_x,
        y,
        launcher_x.saturating_add(1),
        icon,
        icon_style,
    );
    put_segment(
        buffer,
        launcher_x.saturating_add(2),
        y,
        toggle_x.saturating_sub(1),
        "menu",
        dim,
    );
}

/// Every space of the snapshot is folded, so the header offers to open them all.
pub(in crate::client::shell) fn every_space_folded(
    snapshot: &ClientShellSnapshot,
    folds: &HashSet<String>,
) -> bool {
    !snapshot.workspaces.is_empty()
        && snapshot
            .workspaces
            .iter()
            .all(|workspace| folds.contains(&tree_fold_key(&workspace.workspace_id)))
}

/// The header's word.
const HEADER_WORD: &str = "SPACES";

/// The first column after the header word, from the sidebar's content edge: the tree's left
/// pad, the mark column, then the word.
// Six ASCII letters, so the cast is exact.
const HEADER_WORD_END: u16 = 1 + MARK_COLUMN + HEADER_WORD.len() as u16;

/// Sidebars narrower than this draw no header icons: `+` would touch the word.
const HEADER_PLUS_MIN_WIDTH: u16 = HEADER_WORD_END + 2;

/// Sidebars narrower than this keep only the header's `+`: search and fold all would reach
/// into the word and the blank column after it.
const HEADER_ICONS_MIN_WIDTH: u16 = HEADER_WORD_END + 7;

/// Sidebars narrower than this leave out the task board icon, which would crowd the word;
/// its key still opens the board.
const BOARD_ICON_MIN_WIDTH: u16 = HEADER_WORD_END + 11;

/// The header's icons, two columns apart and the last one column before the edge: new space
/// first, then the task board, search, fold all (or open all). The filter icon sits one
/// column after the word, and gives way first when it would touch them.
/// The filter icon's column in the header over `content`, when it has room: it gives way
/// with search and fold all, and before `+`, never touching `+`, whose column follows the
/// icons that fit.
fn filter_icon_x(content: Rect) -> Option<u16> {
    if content.width < HEADER_ICONS_MIN_WIDTH {
        return None;
    }
    let plus_from_edge = if content.width >= BOARD_ICON_MIN_WIDTH {
        8
    } else {
        6
    };
    let plus_x = content.right().saturating_sub(plus_from_edge);
    let filter_x = content.x.saturating_add(HEADER_WORD_END + 1);
    (filter_x.saturating_add(2) <= plus_x).then_some(filter_x)
}

fn render_header_icons(
    buffer: &mut Buffer,
    content: Rect,
    all_folded: bool,
    (board, filter): (super::task_board::BoardIcon, TreeFilter),
    config: &ClientShellConfig,
    hits: &mut ShellHitMap,
) {
    if content.width < HEADER_PLUS_MIN_WIDTH {
        return;
    }
    let icons = tree_icons(config.sidebar_tree_icons);
    let style = Style::default().fg(config.palette.overlay1);
    let cell =
        |from_edge: u16| Rect::new(content.right().saturating_sub(from_edge), content.y, 1, 1);
    let mut put = |cell: Rect, icon: &str, style: Style| {
        put_segment(buffer, cell.x, cell.y, cell.right(), icon, style);
        cell
    };
    // From the right edge in; `+` takes the next cell after the icons that fit.
    let mut next = 2;
    if content.width >= HEADER_ICONS_MIN_WIDTH {
        let fold = if all_folded {
            icons.open_all
        } else {
            icons.fold_all
        };
        hits.tree_fold_all = put(cell(2), fold, style);
        hits.tree_search = put(cell(4), icons.search, style);
        next = 6;
    }
    if content.width >= BOARD_ICON_MIN_WIDTH {
        // Grey like the others; yellow while the board is open. No icon fills its cell: in
        // a tall cell a fill reads as a padded block.
        let board_style = match board {
            super::task_board::BoardIcon::Idle => style,
            super::task_board::BoardIcon::Open => Style::default().fg(config.palette.yellow),
        };
        hits.tree_task_board = put(cell(6), icons.board, board_style);
        next = 8;
    }
    hits.new_workspace_header = put(cell(next), icons.new, style);
    if let Some(filter_x) = filter_icon_x(content) {
        let (icon, color) = filter.icon(&config.palette);
        hits.tree_filter = put(
            Rect::new(filter_x, content.y, 1, 1),
            icon,
            Style::default().fg(color),
        );
    }
}

/// What a tab row's shape depends on besides the tab itself.
#[derive(Debug, Clone, Copy)]
struct TabRowShape {
    place: TabPlace,
    /// Its only pane is a shell, which gets the shell mark.
    lone_shell: bool,
    /// Set for a split tab, which gets the fold arrow: whether it is folded.
    folded: Option<bool>,
}

/// A tab's row: its connector, the shell mark, then its name and a split tab's fold arrow.
/// A folded tab shows its agents' state before its name, as its hidden rows would.
/// Returns the arrow's cell.
fn render_tab_row(
    buffer: &mut Buffer,
    rect: Rect,
    tab: &ClientShellTab,
    TabRowShape {
        place,
        lone_shell,
        folded,
    }: TabRowShape,
    spinner_ms: u64,
    config: &ClientShellConfig,
) -> Option<Rect> {
    let palette = &config.palette;
    let mut x = put_branch(buffer, rect, place, palette);
    if lone_shell {
        x = put_shell_mark(buffer, x, rect, config);
    }
    // A split tab keeps the mark column, blank open, so names line up with the rows
    // around it; folded it shows its agents' state there.
    if let Some(folded) = folded {
        x = if folded && tab.agent_status != AgentStatus::Unknown {
            put_mark(
                buffer,
                x,
                rect,
                tree_mark(tab.agent_status, spinner_ms, config),
                mark_color(tab.agent_status, palette),
            )
        } else {
            put_mark(buffer, x, rect, " ", palette.overlay0)
        };
    }
    // A named tab shows only its name. An unnamed tab's label is its position, as the
    // tab bar shows it; `number` is a creation counter and would drift from the bar.
    let position;
    let (text, color) = if tab.custom_label {
        (tab.label.as_str(), palette.overlay1)
    } else {
        position = format!("tab {}", tab.label);
        (position.as_str(), palette.overlay0)
    };
    // A lone shell's row is that shell's: its colours, apart from the dim branch lines.
    let color = if lone_shell { palette.subtext0 } else { color };
    let style = Style::default().fg(if tab.focused { palette.text } else { color });
    // A tab row is only bold while it leads; a focused tab shows in the text color.
    let style = match tab_row_weight(place) {
        Weight::Bold => style.add_modifier(Modifier::BOLD),
        Weight::Normal | Weight::FocusBold => style,
    };
    // Room for ` ▾` after the name when the tab folds.
    let name_right = rect
        .right()
        .saturating_sub(if folded.is_some() { 2 } else { 0 });
    let name_end = put_label(buffer, x, rect.y, name_right.max(x), text, style);
    folded.and_then(|folded| put_fold_arrow(buffer, name_end, rect, folded, config))
}

/// The tab's connector, under the space's mark and under its lead's name when nested;
/// returns where the row's content starts.
fn put_branch(buffer: &mut Buffer, rect: Rect, place: TabPlace, palette: &Palette) -> u16 {
    let x = put_guides(buffer, rect, place, palette);
    put_connector(buffer, x, rect, place.last, palette)
}

/// `├╴` or, for the last row of its level, `╰╴`: a half stub, so the line stops short of
/// the mark after it. Returns where the row's content starts.
fn put_connector(buffer: &mut Buffer, x: u16, rect: Rect, last: bool, palette: &Palette) -> u16 {
    put_segment(
        buffer,
        x,
        rect.y,
        rect.right(),
        if last { "╰╴" } else { "├╴" },
        Style::default().fg(palette.overlay0),
    )
}

/// A level's guide, four columns wide so the next level's connector sits under the name:
/// `│` while rows of that level follow, else blank.
fn guide(runs_on: bool) -> &'static str {
    if runs_on {
        "│   "
    } else {
        "    "
    }
}

/// The `│` guides of a tab's shallower levels, from the tab column; returns the column of
/// the tab's own connector.
fn put_guides(buffer: &mut Buffer, rect: Rect, place: TabPlace, palette: &Palette) -> u16 {
    let mut x = rect.x.saturating_add(TAB_INDENT);
    for runs_on in place.guides.iter().take(place.depth) {
        x = put_segment(
            buffer,
            x,
            rect.y,
            rect.right(),
            guide(*runs_on),
            Style::default().fg(palette.overlay0),
        );
    }
    x
}

/// A mark and the space after it; returns where the name starts.
/// A shell's mark: its kind, not a state, so in the shell's name colour.
fn put_shell_mark(buffer: &mut Buffer, x: u16, rect: Rect, config: &ClientShellConfig) -> u16 {
    put_mark(
        buffer,
        x,
        rect,
        tree_icons(config.sidebar_tree_icons).shell,
        config.palette.subtext0,
    )
}

fn put_mark(
    buffer: &mut Buffer,
    x: u16,
    rect: Rect,
    mark: &str,
    color: ratatui::style::Color,
) -> u16 {
    let x = put_segment(
        buffer,
        x,
        rect.y,
        rect.right(),
        mark,
        Style::default().fg(color),
    );
    put_segment(buffer, x, rect.y, rect.right(), " ", Style::default())
}

fn render_pane_row(
    buffer: &mut Buffer,
    rect: Rect,
    pane: &ClientShellPane,
    (agent, position, worked, home_dir): (
        Option<&ClientShellAgent>,
        usize,
        Option<std::time::Duration>,
        Option<&str>,
    ),
    (tab, last, nested, under_lead): (TabPlace, bool, bool, bool),
    spinner_ms: u64,
    config: &ClientShellConfig,
) {
    let palette = &config.palette;
    let right = rect.right();
    // Nested under its tab's row; else under the space row, where tab rows start.
    let mut x = rect.x.saturating_add(TAB_INDENT);
    if nested {
        x = put_guides(buffer, rect, tab, palette);
        x = put_segment(
            buffer,
            x,
            rect.y,
            right,
            guide(!tab.last),
            Style::default().fg(palette.overlay0),
        );
    }
    x = put_connector(buffer, x, rect, last, palette);
    match agent {
        Some(agent) => {
            put_agent(
                buffer,
                rect,
                x,
                (
                    pane,
                    agent,
                    non_blank(pane.label.as_deref()).unwrap_or_else(|| pane_label(agent)),
                    worked,
                ),
                (
                    agent_weight(agent, false, Weight::FocusBold),
                    None,
                    false,
                    under_lead,
                ),
                spinner_ms,
                config,
            );
        }
        // A shell's name is the one Rename pane gave it, else its folder's, else its place
        // in the split.
        None => {
            // Shell colours, apart from the dim branch lines.
            let x = put_shell_mark(buffer, x, rect, config);
            let name = non_blank(pane.label.as_deref())
                .map(std::borrow::Cow::Borrowed)
                .or_else(|| folder_name(pane, home_dir))
                .unwrap_or_else(|| format!("pane {position}").into());
            put_label(
                buffer,
                x,
                rect.y,
                right,
                &name,
                pane_name_style(pane, Weight::FocusBold, palette),
            );
        }
    }
}

/// State mark (the spinner while Working), a space, then `name`, the agent's name as the
/// row shows it, the fold arrow when `fold` is set (its tab's other panes follow), and at
/// the right a lead's phase word, else while it works how long, dim, when that fits beside
/// the name. `leads`: other tabs nest under it, so it is a lead even when it names one. A
/// lead's name too long for its room is cut in its middle. `under_lead`: the row is drawn
/// under its lead, so it spins the nested spinner. Returns the arrow's cell.
fn put_agent(
    buffer: &mut Buffer,
    rect: Rect,
    x: u16,
    (pane, agent, name, worked): (
        &ClientShellPane,
        &ClientShellAgent,
        &str,
        Option<std::time::Duration>,
    ),
    (weight, fold, leads, under_lead): (Weight, Option<bool>, bool, bool),
    spinner_ms: u64,
    config: &ClientShellConfig,
) -> Option<Rect> {
    let palette = &config.palette;
    let right = rect.right();
    let status = agent.agent_status;
    // A row drawn under its lead spins the nested spinner.
    let (mark, color) = agent_mark(agent, spinner_ms, config, under_lead);
    let x = put_mark(buffer, x, rect, mark, color);
    // Room for ` ▾` after the name when the row folds.
    let name_right = right
        .saturating_sub(if fold.is_some() { 2 } else { 0 })
        .max(x);
    // A lead's name keeps its last part; a worker's is cut at its end.
    let name = if is_lead(agent, leads) {
        cut_lead_name(name, usize::from(name_right - x))
    } else {
        name.into()
    };
    let mut end = put_label(
        buffer,
        x,
        rect.y,
        name_right,
        &name,
        pane_name_style(pane, weight, palette),
    );
    let arrow = fold.and_then(|folded| put_fold_arrow(buffer, end, rect, folded, config));
    if let Some(arrow) = arrow {
        end = arrow.right();
    }
    let fact = stage_or_worked(
        lead_phase(agent),
        worked_text(Some((status, worked))),
        palette,
    );
    // At the right, ending where every row's facts end, when it fits beside the name.
    let fact_end = facts_end(rect, config.mouse_capture);
    if let Some(fact) = fact {
        // A sidebar is far narrower than u16::MAX.
        let fact_x = fact_end.saturating_sub(fact.width() as u16);
        if fact_x > end {
            put_segment(buffer, fact_x, rect.y, fact_end, &fact.content, fact.style);
        }
    }
    arrow
}

/// A pane's name: in the text color while focused, else [`quiet_name`], weighted by `weight`.
fn pane_name_style(pane: &ClientShellPane, weight: Weight, palette: &Palette) -> Style {
    let style = Style::default().fg(if pane.focused {
        palette.text
    } else {
        quiet_name(palette)
    });
    match weight {
        Weight::Bold => style.add_modifier(Modifier::BOLD),
        Weight::FocusBold if pane.focused => style.add_modifier(Modifier::BOLD),
        Weight::Normal | Weight::FocusBold => style,
    }
}

/// An agent's or shell's name on a row not focused: quiet, so its mark carries its state.
pub(super) fn quiet_name(palette: &Palette) -> ratatui::style::Color {
    palette.subtext0
}

/// `label` cut to end before `right`; returns where it ends.
pub(super) fn put_label(
    buffer: &mut Buffer,
    x: u16,
    y: u16,
    right: u16,
    label: &str,
    style: Style,
) -> u16 {
    let label = crate::ui::truncate_end(label, usize::from(right.saturating_sub(x)));
    put_segment(buffer, x, y, right, &label, style)
}
