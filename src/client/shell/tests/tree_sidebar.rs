use super::*;
use crate::client::shell::render::ShellRenderState;
use crate::client::shell::tree_sidebar::{
    pane_label, render_tree_sidebar, spinner_glyph, spinner_pace_ms, spinner_set, tree_fold_key,
    tree_mark, tree_rows, tree_tab_fold_key, TreeFilter, TreeRow, TreeTarget,
};

fn tree_workspace(
    id: &str,
    number: usize,
    label: &str,
    status: AgentStatus,
) -> ClientShellWorkspace {
    let mut workspace = snapshot().workspaces[0].clone();
    workspace.workspace_id = id.into();
    workspace.number = number;
    workspace.label = label.into();
    workspace.custom_label = true;
    workspace.focused = false;
    workspace.agent_status = status;
    workspace.branch = None;
    workspace.git_ahead_behind = None;
    workspace
}

fn tree_tab(id: &str, workspace_id: &str, number: usize, label: Option<&str>) -> ClientShellTab {
    ClientShellTab {
        tab_id: id.into(),
        workspace_id: workspace_id.into(),
        number,
        label: label.map_or_else(|| number.to_string(), str::to_owned),
        custom_label: label.is_some(),
        zoomed: false,
        focused: false,
        agent_status: AgentStatus::Unknown,
    }
}

fn tree_pane(id: &str, workspace_id: &str, tab_id: &str) -> ClientShellPane {
    ClientShellPane {
        pane_id: id.into(),
        workspace_id: workspace_id.into(),
        tab_id: tab_id.into(),
        label: None,
        cwd: Some("/repo".into()),
        foreground_cwd: None,
        focused: false,
        right_click_passthrough: false,
        git_branch: None,
        plugin_id: None,
    }
}

fn tree_agent(pane: &ClientShellPane, agent: &str, status: AgentStatus) -> ClientShellAgent {
    ClientShellAgent {
        pane_id: pane.pane_id.clone(),
        workspace_id: pane.workspace_id.clone(),
        tab_id: pane.tab_id.clone(),
        name: None,
        display_agent: None,
        agent: Some(agent.into()),
        title: Some(format!("{agent} title")),
        terminal_title: None,
        terminal_title_stripped: None,
        agent_status: status,
        state_change_seq: 0,
        state_labels: Vec::new(),
        tokens: Vec::new(),
        focused: pane.focused,
    }
}

/// The mockup: two spaces, a working agent, shells, a blocked and a done agent.
/// Tabs and panes are interleaved across spaces to prove grouping does not rely on order.
fn tree_snapshot() -> ClientShellSnapshot {
    let mut projected = snapshot();
    let mut herdr = tree_workspace("ws_1", 1, "herdr", AgentStatus::Working);
    herdr.focused = true;
    projected.workspaces = vec![
        herdr,
        tree_workspace("ws_2", 2, "dotworks", AgentStatus::Blocked),
    ];
    let mut main = tree_tab("tab_1", "ws_1", 1, Some("main"));
    main.focused = true;
    projected.tabs = vec![
        main,
        tree_tab("tab_3", "ws_2", 1, None),
        tree_tab("tab_2", "ws_1", 2, Some("logs")),
    ];
    let mut claude = tree_pane("pane_1", "ws_1", "tab_1");
    claude.focused = true;
    projected.panes = vec![
        claude,
        tree_pane("pane_4", "ws_2", "tab_3"),
        tree_pane("pane_2", "ws_1", "tab_1"),
        tree_pane("pane_3", "ws_1", "tab_2"),
        tree_pane("pane_5", "ws_2", "tab_3"),
    ];
    projected.agents = vec![
        tree_agent(&projected.panes[0], "claude", AgentStatus::Working),
        tree_agent(&projected.panes[1], "codex", AgentStatus::Blocked),
        tree_agent(&projected.panes[4], "claude", AgentStatus::Done),
    ];
    projected.focused_workspace_id = Some("ws_1".into());
    projected.focused_tab_id = Some("tab_1".into());
    projected.focused_pane_id = Some("pane_1".into());
    projected
}

struct TreeRender {
    rows: Vec<String>,
    buffer: Buffer,
    hits: ShellHitMap,
    targets: Vec<(Rect, TreeTarget)>,
    scroll: usize,
}

struct TreeRenderInput {
    height: u16,
    /// The spinner's clock: braille shows its frame `spinner_ms / 100`.
    spinner_ms: u64,
    scroll: usize,
    reveal_focused: bool,
    /// Folded spaces (`ws_N`) and split tabs (`tab_N`).
    folded: &'static [&'static str],
    /// Panes (`pane_N`) the client has seen working for this many seconds.
    working_for: &'static [(&'static str, u64)],
    /// Panes (`pane_N`) the client saw turn done this many seconds ago.
    done_for: &'static [(&'static str, u64)],
    /// Panes (`pane_N`) the client first saw need you this many seconds ago.
    waiting_for: &'static [(&'static str, u64)],
    width: u16,
    /// The space row under the mouse.
    hovered: Option<&'static str>,
    /// The row under the mouse.
    hovered_row: Option<Rect>,
    /// The space being dragged.
    dragged: Option<&'static str>,
    /// The user's home folder.
    home: Option<&'static str>,
    /// The header's filter.
    filter: TreeFilter,
    /// How the header's board icon shows.
    board: super::super::task_board::BoardIcon,
}

fn render_tree(
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    input: TreeRenderInput,
) -> TreeRender {
    let area = Rect::new(0, 0, input.width, input.height);
    let mut buffer = Buffer::empty(area);
    let diagnostics = super::super::machine_diagnostics::MachineDiagnostics::default();
    let collapsed_endpoints = HashSet::new();
    let collapsed_groups = input
        .folded
        .iter()
        .map(|id| {
            if id.starts_with("tab_") {
                tree_tab_fold_key(id)
            } else {
                tree_fold_key(id)
            }
        })
        .collect::<HashSet<_>>();
    let remote_collapsed_groups = HashMap::new();
    let mut scroll = input.scroll;
    let mut agent_scroll = 0;
    let mut tab_scroll = 0;
    let mut reveal_focused = input.reveal_focused;
    let mut reveal_tab = false;
    let mut reveal_navigation = false;
    let now = std::time::Instant::now();
    let since = |times: &[(&str, u64)]| {
        times
            .iter()
            .map(|(pane_id, seconds)| {
                let since = now
                    .checked_sub(std::time::Duration::from_secs(*seconds))
                    .expect("a start after the clock's origin");
                ((*pane_id).to_owned(), since)
            })
            .collect::<HashMap<_, _>>()
    };
    let working_since = since(input.working_for);
    let done_since = since(input.done_for);
    let waiting_since = since(input.waiting_for);
    let mut state = ShellRenderState {
        machine_diagnostics: &diagnostics,
        endpoints: &[],
        active_endpoint_id: &ClientEndpointId::Local,
        collapsed_endpoints: &collapsed_endpoints,
        collapsed_groups: &collapsed_groups,
        remote_collapsed_groups: &remote_collapsed_groups,
        workspace_scroll: &mut scroll,
        agent_scroll: &mut agent_scroll,
        tab_scroll: &mut tab_scroll,
        reveal_focused_workspace: &mut reveal_focused,
        reveal_focused_tab: &mut reveal_tab,
        sidebar_collapsed: false,
        sidebar_section_split: 0.5,
        tab_drag_insert_index: None,
        selected_workspace_id: None,
        reveal_navigation_workspace: &mut reveal_navigation,
        dragged_workspace_id: input.dragged,
        workspace_drop_indicator_row: None,
        spinner_ms: input.spinner_ms,
        hovered_workspace_id: input.hovered,
        hovered_row: input.hovered_row,
        working_times: Some((&working_since, now)),
        done_times: Some((&done_since, now)),
        waiting_times: Some((&waiting_since, now)),
        home_dir: input.home,
        task_board_icon: input.board,
        tree_filter: input.filter,
    };
    let mut hits = ShellHitMap::default();
    render_tree_sidebar(&mut buffer, area, snapshot, config, &mut state, &mut hits);
    let rows = (0..area.height)
        .map(|y| {
            (0..area.width.saturating_sub(1))
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect();
    TreeRender {
        rows,
        buffer,
        targets: std::mem::take(&mut hits.tree_rows),
        hits,
        scroll,
    }
}

fn tall(spinner_ms: u64) -> TreeRenderInput {
    TreeRenderInput {
        height: 20,
        spinner_ms,
        scroll: 0,
        reveal_focused: false,
        folded: &[],
        working_for: &[],
        done_for: &[],
        waiting_for: &[],
        width: 26,
        hovered: None,
        hovered_row: None,
        dragged: None,
        home: None,
        filter: TreeFilter::All,
        board: Default::default(),
    }
}

/// The header row: the word, then new space, the task board, search and fold all, the last
/// one column before the sidebar's content edge.
const HEADER: &str = "  SPACES ▽       + ☑ / ▾";

#[test]
fn tree_rows_list_each_space_then_its_tabs_then_their_panes() {
    let snapshot = tree_snapshot();
    let order = tree_rows(&snapshot, &HashSet::new(), true)
        .iter()
        .map(|row| match row {
            TreeRow::Space { workspace, .. } => format!("space {}", workspace.workspace_id),
            TreeRow::SpaceBranch { branch, .. } => format!("branch {branch}"),
            TreeRow::Tab {
                tab,
                place,
                lone_pane,
                ..
            } => format!(
                "tab {} last={} lone={}",
                tab.tab_id,
                place.last,
                lone_pane.map_or("-", |pane| pane.pane_id.as_str())
            ),
            TreeRow::AgentTab { pane, place, .. } => {
                format!("agent tab {} last={}", pane.pane_id, place.last)
            }
            TreeRow::Pane {
                pane,
                position,
                tab,
                last,
                ..
            } => format!(
                "pane {} #{position} tab_last={} last={last}",
                pane.pane_id, tab.last
            ),
            TreeRow::BranchLine { pane, branch, .. } => {
                format!("branch line {} {branch}", pane.pane_id)
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(
        order,
        // Each space's main lead (claude, codex: the first tab's first pane) is its space
        // row; the rest of their first tabs hang under it.
        [
            "space ws_1",
            "pane pane_2 #2 tab_last=false last=false",
            "tab tab_2 last=true lone=pane_3",
            "space ws_2",
            "pane pane_5 #2 tab_last=false last=true"
        ]
    );
}

#[test]
fn tree_agent_rows_show_the_session_name_first() {
    let pane = tree_pane("pane_1", "ws_1", "tab_1");
    let mut agent = tree_agent(&pane, "claude", AgentStatus::Idle);
    assert_eq!(pane_label(&agent), "claude");
    agent.display_agent = Some("claude code".into());
    assert_eq!(pane_label(&agent), "claude code");
    agent.name = Some("reviewer".into());
    assert_eq!(pane_label(&agent), "reviewer", "the session name wins");
    agent.display_agent = None;
    agent.name = None;
    agent.agent = None;
    assert_eq!(pane_label(&agent), "claude title");
    agent.title = None;
    assert_eq!(
        pane_label(&agent),
        "agent",
        "agent entry with no name at all"
    );
}

#[test]
fn tree_claude_rows_fall_back_to_the_session_title_claude_sets() {
    let pane = tree_pane("pane_1", "ws_1", "tab_1");
    let mut claude = tree_agent(&pane, "claude", AgentStatus::Idle);
    // A /rename inside Claude reaches Herdr only as the terminal title.
    claude.terminal_title_stripped = Some("dotworks-lead".into());
    assert_eq!(pane_label(&claude), "dotworks-lead");
    claude.name = Some("reviewer".into());
    assert_eq!(pane_label(&claude), "reviewer", "a Herdr name still wins");
    claude.name = Some(" ".into());
    assert_eq!(
        pane_label(&claude),
        "dotworks-lead",
        "a blank name falls through"
    );
    claude.terminal_title_stripped = Some("  ".into());
    assert_eq!(pane_label(&claude), "claude", "a blank title falls through");

    let mut codex = tree_agent(&pane, "codex", AgentStatus::Idle);
    codex.terminal_title_stripped = Some("npm test".into());
    assert_eq!(
        pane_label(&codex),
        "codex",
        "other agents' titles are arbitrary"
    );
}

#[test]
fn tree_sidebar_draws_spaces_tabs_and_panes_with_connectors() {
    let snapshot = tree_snapshot();
    let config = ClientShellConfig::from_config(&Config::default());
    let render = render_tree(&snapshot, &config, tall(200));
    assert_eq!(
        render.rows[..9],
        [
            HEADER,
            "",
            "▎ ⠹ herdr ▾",
            "▎ ├╴· repo",
            "▎ ╰╴· logs",
            "▎ ! dotworks ▾",
            "▎ ╰╴✓ claude",
            "",
            "",
        ]
    );
    assert_eq!(
        render.rows[19], "  ≡ menu               «",
        "{:#?}",
        render.rows
    );

    let palette = &config.palette;
    let focused_space = &render.buffer[(4, 2)];
    assert_eq!(focused_space.fg, palette.text);
    assert!(focused_space.modifier.contains(Modifier::BOLD));
    assert_eq!(
        focused_space.bg, palette.selection_bg,
        "its main lead's pane is focused: the focused row's colour"
    );
    let other_space = &render.buffer[(4, 5)];
    assert_eq!(other_space.fg, palette.subtext0);
    assert!(
        other_space.modifier.contains(Modifier::BOLD),
        "every space name is bold"
    );
    assert_eq!(render.buffer[(2, 3)].fg, palette.overlay0, "connector");
    assert_eq!(
        render.buffer[(2, 2)].fg,
        working_colour(palette),
        "main lead's working mark"
    );
    assert_eq!(render.buffer[(6, 3)].fg, palette.subtext0, "shell name");
    assert_eq!(render.buffer[(13, 5)].fg, palette.overlay1, "fold arrow");
    assert_eq!(render.buffer[(6, 4)].fg, palette.subtext0, "lone shell tab");
    assert_eq!(render.buffer[(6, 6)].fg, palette.subtext0, "pane agent");
}

#[test]
fn tree_connectors_end_in_a_short_stub_that_leaves_air_before_the_mark() {
    // Tabs, split panes, a lead's nested workers and a nested lead's own workers.
    let config = ClientShellConfig::from_config(&Config::default());
    for projected in [tree_snapshot(), picture_branches()] {
        let render = render_tree(&projected, &config, tall_panel(200));
        let mut connectors = 0;
        for (y, row) in render.rows.iter().enumerate() {
            let cells = row.chars().collect::<Vec<_>>();
            for (x, cell) in cells.iter().enumerate() {
                if !matches!(cell, '├' | '╰') {
                    continue;
                }
                connectors += 1;
                assert_eq!(cells.get(x + 1), Some(&'╴'), "row {y}: {row:?}");
                // The mark keeps its column, two after the corner, with a space after it.
                assert!(
                    cells
                        .get(x + 2)
                        .is_some_and(|mark| *mark != ' ' && *mark != '─'),
                    "row {y}: {row:?}"
                );
                assert_eq!(cells.get(x + 3), Some(&' '), "row {y}: {row:?}");
            }
        }
        assert!(connectors >= 3, "{:#?}", render.rows);
    }
}

#[test]
fn tree_rows_show_a_name_or_a_number_never_both() {
    let mut projected = snapshot();
    let mut web = tree_workspace("ws_7", 7, "web", AgentStatus::Idle);
    web.focused = true;
    projected.workspaces = vec![web];
    // The first tab's shell is the space row; the tabs after it keep their rows.
    projected.tabs = vec![
        tree_tab("tab_2", "ws_7", 2, Some("main")),
        tree_tab("tab_3", "ws_7", 3, Some("api")),
        tree_tab("tab_4", "ws_7", 4, None),
    ];
    projected.panes = vec![
        tree_pane("pane_0", "ws_7", "tab_2"),
        tree_pane("pane_1", "ws_7", "tab_3"),
        tree_pane("pane_2", "ws_7", "tab_4"),
    ];
    projected.agents = Vec::new();
    let config = ClientShellConfig::from_config(&Config::default());
    let render = render_tree(&projected, &config, tall(0));
    assert_eq!(
        render.rows[2..7],
        ["▎ · web ▾", "▎ ├╴· api", "▎ ╰╴· tab 4", "", "",],
        "space shows no number, named tab only its name, unnamed tab only its number"
    );
    let palette = &config.palette;
    // Both tabs are lone shells: their rows take the shell name colour.
    assert_eq!(render.buffer[(6, 3)].fg, palette.subtext0, "tab name");
    assert_eq!(render.buffer[(6, 4)].fg, palette.subtext0, "tab number");
}

#[test]
fn tree_unnamed_tab_shows_its_tab_bar_position_not_its_creation_number() {
    let mut projected = snapshot();
    let mut herdr = tree_workspace("ws_1", 1, "herdr", AgentStatus::Idle);
    herdr.focused = true;
    projected.workspaces = vec![herdr];
    let mut added = tree_tab("tab_10", "ws_1", 10, None);
    // The server labels an unnamed tab with its position, as the tab bar shows it.
    added.label = "3".into();
    // After the first tab, whose shell is the space row.
    projected.tabs = vec![tree_tab("tab_1", "ws_1", 1, Some("main")), added];
    projected.panes = vec![
        tree_pane("pane_0", "ws_1", "tab_1"),
        tree_pane("pane_1", "ws_1", "tab_10"),
    ];
    projected.agents = Vec::new();
    let config = ClientShellConfig::from_config(&Config::default());
    let render = render_tree(&projected, &config, tall(0));
    assert_eq!(render.rows[2..4], ["▎ · herdr ▾", "▎ ╰╴· tab 3"]);
}

/// The mockup's spaces with git: herdr on a branch ahead of its upstream, dotworks without git.
fn git_snapshot() -> ClientShellSnapshot {
    let mut projected = tree_snapshot();
    projected.workspaces[0].branch = Some("feat/sidebar-tree".into());
    projected.workspaces[0].git_ahead_behind = Some((6, 0));
    projected
}

fn git_line(branch: &str, ahead_behind: Option<(usize, usize)>) -> String {
    let mut projected = git_snapshot();
    projected.workspaces[0].branch = Some(branch.into());
    projected.workspaces[0].git_ahead_behind = ahead_behind;
    let config = ClientShellConfig::from_config(&Config::default());
    render_tree(&projected, &config, tall(0)).rows[3].clone()
}

#[test]
fn tree_space_row_shows_ahead_behind_and_the_branch_under_it() {
    let config = ClientShellConfig::from_config(&Config::default());
    let render = render_tree(&git_snapshot(), &config, tall(200));
    assert_eq!(
        render.rows[2..5],
        ["▎ ⠹ herdr ▾", "▎ │ feat/sidebar-… ↑6", "▎ ├╴· repo"],
        "ahead/behind at the branch line's end, where the facts end before the `+`"
    );
    for x in [19, 20] {
        assert_eq!(render.buffer[(x, 3)].fg, config.palette.overlay0, "dim");
    }
}

#[test]
fn tree_branch_line_uses_the_sections_sidebar_ahead_behind_format() {
    assert_eq!(git_line("main", Some((2, 3))), "▎ │ main        ↑2 ↓3");
    assert_eq!(git_line("main", Some((0, 4))), "▎ │ main           ↓4");
    assert_eq!(git_line("main", Some((0, 0))), "▎ │ main");
    assert_eq!(git_line("main", None), "▎ │ main");
}

#[test]
fn tree_space_row_drops_ahead_behind_before_cutting_a_long_name() {
    let mut projected = git_snapshot();
    projected.workspaces[0].label = "oakwood-workspace-lead".into();
    projected.workspaces[0].git_ahead_behind = Some((14, 2));
    // Without branch lines ahead/behind stays on the space row.
    let mut config = ClientShellConfig::from_config(&Config::default());
    config.spaces.rows = vec![vec![crate::config::SpaceSidebarToken::Workspace]];
    assert_eq!(
        render_tree(&projected, &config, tall(0)).rows[2],
        "▎ ⠋ oakwood-worksp… ▾",
        "the name keeps priority"
    );
}

#[test]
fn tree_highlight_bars_and_blocks_the_current_space_and_fills_the_focused_row_brighter() {
    let config = ClientShellConfig::from_config(&Config::default());
    let palette = &config.palette;
    // The main lead's pane stands for the space row; focus the shell beside it instead.
    let mut projected = git_snapshot();
    for pane in &mut projected.panes {
        pane.focused = pane.pane_id == "pane_2";
    }
    let render = render_tree(&projected, &config, tall(0));
    let cell = |x: u16, y: u16| &render.buffer[(x, y)];
    // herdr's space row, its branch line, the focused shell and logs; then dotworks.
    for y in 2..6 {
        assert_eq!(cell(0, y).symbol(), "▎", "the current space's bar, row {y}");
        assert_eq!(cell(0, y).fg, palette.accent, "in the accent, row {y}");
    }
    for y in 6..8 {
        assert_eq!(cell(0, y).symbol(), "▎", "another space's bar, row {y}");
        assert_eq!(cell(0, y).fg, palette.overlay0, "in the grey, row {y}");
    }
    assert_eq!(
        cell(19, 4).bg,
        palette.selection_bg,
        "the focused row's fill"
    );
    let block = cell(19, 2).bg;
    assert_ne!(block, palette.sidebar_bg, "the current space's block");
    assert_ne!(
        block, palette.selection_bg,
        "a fainter level than the focused row"
    );
    for y in [2, 3, 5] {
        for x in [0, 1, 19, 24] {
            assert_eq!(
                cell(x, y).bg,
                block,
                "the block on the current space's other rows, row {y} column {x}"
            );
        }
    }
    assert_eq!(
        cell(0, 4).bg,
        block,
        "the bar keeps the block beside the focus"
    );
    for y in 6..8 {
        assert_eq!(cell(19, y).bg, palette.sidebar_bg, "another space, row {y}");
    }

    // No half-block rows in the gaps, even where the terminal draws them whole.
    let mut config = ClientShellConfig::from_config(&Config::default());
    config.spaces.row_gap = 1;
    let render = render_tree(&three_spaces(1), &config, tall(200));
    assert!(
        !render
            .rows
            .iter()
            .any(|row| row.contains('▄') || row.contains('▀')),
        "{:#?}",
        render.rows
    );
}

#[test]
fn tree_blocks_the_current_space_and_fills_the_focused_row_in_every_theme() {
    let config = ClientShellConfig::from_config(&Config::default());
    // The main lead's pane stands for the space row; focus the shell beside it instead.
    let focused_shell = || {
        let mut projected = git_snapshot();
        for pane in &mut projected.panes {
            pane.focused = pane.pane_id == "pane_2";
        }
        projected
    };
    let render = render_tree(&focused_shell(), &config, tall(0));
    let palette = &config.palette;
    // The default palette's dim surface is its active row, so the focused row takes the
    // selection colour; the current space's other rows are the block.
    let focus = palette.selection_bg;
    let block = ratatui::style::Color::Rgb(30, 30, 44);
    let band = |y: u16| [render.buffer[(1, y)].bg, render.buffer[(19, y)].bg];
    for (y, row) in [(2, "space"), (3, "branch line"), (5, "logs")] {
        assert_eq!(band(y), [block; 2], "{row} row");
    }
    assert_eq!(band(4), [focus; 2], "focused pane row");
    for y in 6..8 {
        assert_eq!(
            band(y),
            [palette.sidebar_bg; 2],
            "gap and folded space, row {y}"
        );
    }

    let mut lone = branch_snapshot();
    for pane in &mut lone.panes {
        pane.focused = pane.pane_id == "pane_3";
    }
    let render = render_tree(&lone, &config, tall(0));
    assert_eq!(
        render.buffer[(19, 4)].bg,
        focus,
        "lone agent row of the focused pane"
    );
    assert_eq!(
        render.buffer[(19, 3)].bg,
        block,
        "lone shell tab not focused"
    );

    let mut theme = Config::default();
    theme.theme.name = Some("tokyo-night".into());
    let config = ClientShellConfig::from_config(&theme);
    let render = render_tree(&focused_shell(), &config, tall(0));
    let palette = &config.palette;
    assert_ne!(palette.surface_dim, palette.active_row_bg);
    assert_eq!(
        render.buffer[(19, 2)].bg,
        ratatui::style::Color::Rgb(28, 29, 42),
        "tokyo: the block, a quarter of the way to its focus fill"
    );
    assert_eq!(
        render.buffer[(19, 4)].bg,
        palette.active_row_bg,
        "tokyo focus fill"
    );

    // The terminal theme has one tone and no selection color: its active row fills, and
    // with no colour to blend there is no block.
    let mut theme = Config::default();
    theme.theme.name = Some("terminal".into());
    let config = ClientShellConfig::from_config(&theme);
    let render = render_tree(&focused_shell(), &config, tall(0));
    let palette = &config.palette;
    assert_eq!(render.buffer[(19, 5)].bg, palette.sidebar_bg, "no block");
    assert_eq!(
        render.buffer[(19, 4)].bg,
        palette.active_row_bg,
        "focus fill"
    );
}

#[test]
fn tree_space_block_is_a_quarter_of_the_way_from_the_background_to_the_focus_fill() {
    use ratatui::style::Color::Rgb;
    let focused_shell = || {
        let mut projected = git_snapshot();
        for pane in &mut projected.panes {
            pane.focused = pane.pane_id == "pane_2";
        }
        projected
    };
    // (theme, block, focused row): two levels over the terminal's own background.
    for (name, block, focus) in [
        ("vesper", Rgb(20, 20, 20), Rgb(35, 35, 35)),
        ("catppuccin", Rgb(30, 30, 44), Rgb(49, 50, 68)),
        ("catppuccin-latte", Rgb(226, 232, 245), Rgb(189, 208, 245)),
        ("one-light", Rgb(241, 242, 244), Rgb(216, 219, 226)),
    ] {
        let mut theme = Config::default();
        theme.theme.name = Some(name.into());
        let config = ClientShellConfig::from_config(&theme);
        let render = render_tree(&focused_shell(), &config, tall(0));
        assert_eq!(render.buffer[(19, 2)].bg, block, "{name}: block");
        assert_eq!(render.buffer[(19, 4)].bg, focus, "{name}: focused row");
        assert_eq!(
            render.buffer[(19, 6)].bg,
            config.palette.sidebar_bg,
            "{name}: another space stays plain"
        );
        assert_eq!(render.buffer[(0, 2)].symbol(), "▎", "{name}: the bar");
        assert_eq!(render.buffer[(0, 2)].fg, config.palette.accent);
    }
}

/// herdr current with its tabs, dotworks with a tab of two agents, oakwood folded.
fn grey_line_spaces() -> ClientShellSnapshot {
    let mut projected = tree_snapshot();
    projected
        .workspaces
        .push(tree_workspace("ws_3", 3, "oakwood", AgentStatus::Idle));
    push_shell_tab(&mut projected, "ws_3", 6, "main");
    projected
}

/// Each space's rows, top to bottom, and the blank gap rows between and after them.
fn grey_line_rows(render: &TreeRender, row_gap: u16) -> (Vec<Vec<u16>>, Vec<u16>) {
    let starts = render
        .hits
        .workspaces
        .iter()
        .map(|hit| hit.rect.y)
        .collect::<Vec<_>>();
    assert_eq!(starts.len(), 3, "{:#?}", render.rows);
    let end = starts[2] + 1;
    let spaces = (0..3)
        .map(|index| {
            let stop = starts.get(index + 1).map_or(end, |next| next - row_gap);
            (starts[index]..stop).collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let gaps = (1..3)
        .flat_map(|index| starts[index] - row_gap..starts[index])
        .chain(end..end + row_gap)
        .collect();
    (spaces, gaps)
}

#[test]
fn tree_other_spaces_draw_a_grey_bar_without_the_tint_and_gap_rows_stay_blank() {
    let focused_shell = || {
        let mut projected = grey_line_spaces();
        for pane in &mut projected.panes {
            pane.focused = pane.pane_id == "pane_2";
        }
        projected
    };
    let input = || TreeRenderInput {
        width: 40,
        folded: &["ws_3"],
        ..tall(0)
    };
    for row_gap in [0, 1] {
        let mut config = ClientShellConfig::from_config(&Config::default());
        config.spaces.row_gap = row_gap;
        let palette = config.palette.clone();
        let render = render_tree(&focused_shell(), &config, input());
        let (spaces, gaps) = grey_line_rows(&render, row_gap);
        let cell = |x: u16, y: u16| &render.buffer[(x, y)];
        let block = cell(19, spaces[0][0]).bg;
        assert_ne!(block, palette.sidebar_bg, "row_gap {row_gap}: herdr's tint");
        for &y in &spaces[0] {
            assert_eq!(cell(0, y).symbol(), "▎", "row_gap {row_gap}: herdr row {y}");
            assert_eq!(
                cell(0, y).fg,
                palette.accent,
                "row_gap {row_gap}: herdr row {y}"
            );
            assert_eq!(cell(0, y).bg, block, "row_gap {row_gap}: herdr row {y}");
        }
        for (name, rows) in [("dotworks", &spaces[1]), ("oakwood", &spaces[2])] {
            for &y in rows {
                let what = format!("row_gap {row_gap}: {name} row {y}: {:#?}", render.rows);
                assert_eq!(cell(0, y).symbol(), "▎", "{what}");
                assert_eq!(cell(0, y).fg, palette.overlay0, "{what}");
                for x in [0, 1, 19] {
                    assert_eq!(
                        cell(x, y).bg,
                        palette.sidebar_bg,
                        "no tint, column {x}: {what}"
                    );
                }
            }
        }
        assert_eq!(
            spaces[1].len(),
            2,
            "dotworks: its space row and one agent: {:#?}",
            render.rows
        );
        assert_eq!(gaps.len(), usize::from(row_gap) * 3);
        for y in gaps {
            assert_eq!(
                cell(0, y).symbol(),
                " ",
                "row_gap {row_gap}: gap row {y} blank"
            );
        }

        // The mouse over a dotworks row gives it the focus fill; its grey bar stays.
        let hovered = spaces[1][1];
        let render = render_tree(
            &focused_shell(),
            &config,
            TreeRenderInput {
                hovered_row: Some(Rect::new(0, hovered, 40, 1)),
                ..input()
            },
        );
        let cell = &render.buffer[(19, hovered)];
        assert_eq!(
            cell.bg,
            crate::client::shell::tree_sidebar::focus_band(&palette, false)
        );
        let bar = &render.buffer[(0, hovered)];
        assert_eq!(bar.symbol(), "▎", "row_gap {row_gap}: hovered bar");
        assert_eq!(bar.fg, palette.overlay0, "row_gap {row_gap}: hovered bar");

        // A dragged space's row takes the drag fill; its grey bar stays.
        let dragged = spaces[1][0];
        let render = render_tree(
            &focused_shell(),
            &config,
            TreeRenderInput {
                dragged: Some("ws_2"),
                ..input()
            },
        );
        assert_eq!(
            render.buffer[(19, dragged)].bg,
            palette.surface1,
            "drag fill"
        );
        assert_eq!(render.buffer[(0, dragged)].symbol(), "▎", "dragged bar");
        assert_eq!(
            render.buffer[(0, dragged)].fg,
            palette.overlay0,
            "dragged bar"
        );
    }

    // Every theme: the other spaces' grey is not the current space's accent nor an RGB
    // background, and the terminal theme's grey is drawn as it is.
    let mut same = Vec::new();
    let mut vanish = Vec::new();
    for name in crate::config::THEME_NAMES {
        let mut theme = Config::default();
        theme.theme.name = Some((*name).into());
        let config = ClientShellConfig::from_config(&theme);
        let render = render_tree(&focused_shell(), &config, input());
        let (spaces, _) = grey_line_rows(&render, config.spaces.row_gap);
        let palette = &config.palette;
        if palette.overlay0 == palette.accent {
            same.push(*name);
        }
        // The bundled themes leave the sidebar on the terminal's background, so the
        // theme's own panel background stands in for it.
        if [palette.sidebar_bg, palette.panel_bg]
            .into_iter()
            .any(|bg| matches!(bg, ratatui::style::Color::Rgb(..)) && bg == palette.overlay0)
        {
            vanish.push(*name);
        }
        for &y in &spaces[0] {
            assert_eq!(
                render.buffer[(0, y)].fg,
                palette.accent,
                "{name}: herdr row {y}"
            );
        }
        for &y in spaces[1].iter().chain(&spaces[2]) {
            assert_eq!(render.buffer[(0, y)].symbol(), "▎", "{name}: row {y}");
            assert_eq!(
                render.buffer[(0, y)].fg,
                palette.overlay0,
                "{name}: row {y}"
            );
        }
    }
    assert!(same.is_empty(), "grey equals the accent in {same:?}");
    assert!(
        vanish.is_empty(),
        "grey equals the background in {vanish:?}"
    );
}

#[test]
fn tree_a_key_picked_other_space_keeps_its_grey_bar_under_the_pick_fill() {
    let mut state = three_spaces_state();
    let endpoint = state.active_endpoint_id.clone();
    state.mode = ClientShellMode::Navigate;
    state.navigate_workspace_id = state.navigation_target(&endpoint, "ws_3");
    let frame = state.compose(106, 30).expect("frame");
    let buffer = frame.to_ratatui_buffer().expect("frame should reconstruct");
    let row = space_rect(&state, "ws_3");
    let palette = &state.config.palette;
    assert_eq!(
        buffer[(row.x + 10, row.y)].bg,
        crate::client::shell::sidebar::workspace_selection_background(palette),
        "the pick fill"
    );
    assert_eq!(buffer[(row.x, row.y)].symbol(), "▎", "the grey bar");
    assert_eq!(buffer[(row.x, row.y)].fg, palette.overlay0, "the grey bar");
}

#[test]
fn tree_hover_and_a_key_pick_draw_over_the_space_tint() {
    let mut projected = tree_snapshot();
    // The focus sits on the shell beside the space row, so the space row has only the tint.
    for pane in &mut projected.panes {
        pane.focused = pane.pane_id == "pane_2";
    }
    let mut state = ClientShellState::new(tree_config(true));
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.compose(106, 30).expect("first frame");
    let row = state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.workspace_id == "ws_1")
        .expect("space row")
        .rect;
    let fill = |state: &mut ClientShellState| {
        let frame = state.compose(106, 30).expect("frame");
        frame.to_ratatui_buffer().expect("frame should reconstruct")[(row.x + 10, row.y)].bg
    };
    let palette = state.config.palette.clone();
    let tint = fill(&mut state);
    assert_ne!(tint, palette.sidebar_bg, "the space row sits on the tint");

    mouse_at(&mut state, MouseEventKind::Moved, row.x + 4, row.y);
    assert_eq!(
        fill(&mut state),
        crate::client::shell::tree_sidebar::focus_band(&palette, false),
        "hover draws the focus fill over the tint"
    );

    mouse_at(&mut state, MouseEventKind::Moved, 80, 25);
    assert_eq!(fill(&mut state), tint, "the tint comes back");
    let endpoint = state.active_endpoint_id.clone();
    state.mode = ClientShellMode::Navigate;
    state.navigate_workspace_id = state.navigation_target(&endpoint, "ws_1");
    assert_eq!(
        fill(&mut state),
        crate::client::shell::sidebar::workspace_selection_background(&palette),
        "a key pick draws its fill over the tint"
    );
}

/// Nadine's look with detail lines: a stage and a git line on herdr, a task on the lone
/// agent and on the split's agent pane.
fn detail_snapshot() -> ClientShellSnapshot {
    let mut projected = branch_snapshot();
    projected.workspaces[0].branch = Some("feat/sidebar-tree".into());
    projected.agents[0].tokens = vec![("task".into(), "watching leads".into())];
    projected.agents[1].tokens = vec![("task".into(), "review diff".into())];
    projected
}

#[test]
fn tree_rows_stay_one_line_with_long_task_stage_and_branch() {
    let mut projected = detail_snapshot();
    projected.workspaces[0].branch = Some("a-very-long-branch-name-that-overflows".into());
    projected.agents[0].tokens = vec![
        (
            "task".into(),
            "watching the herdr and oakwood leads today and tomorrow".into(),
        ),
        (
            "stage".into(),
            "review · checking the tree layout and all of its many rows".into(),
        ),
    ];
    projected.agents[1].agent_status = AgentStatus::Idle;
    let config = ClientShellConfig::from_config(&Config::default());
    let render = render_tree(&projected, &config, tall(200));
    assert_eq!(
        render.rows[2..10],
        [
            "▎ · herdr ▾",
            "▎ │ a-very-long-branch…",
            "▎ ├╴· tab 2",
            "▎ ├╴⠹ herdr-w7 Review",
            "▎ ╰╴○ codex ▾",
            "▎     ╰╴· repo",
            "",
            ""
        ]
    );
    for y in 1..19 {
        assert_eq!(
            render.buffer[(24, y)].symbol(),
            " ",
            "row {y} stays inside the sidebar"
        );
    }
}

#[test]
fn tree_space_rows_show_the_stage_word_then_git_or_folded_counts() {
    let config = ClientShellConfig::from_config(&Config::default());
    let palette = &config.palette;
    let mut projected = tree_snapshot();
    // The main leads: claude in herdr, codex in dotworks.
    projected.agents[0].tokens = vec![("stage".into(), "plan · checking".into())];
    projected.workspaces[0].git_ahead_behind = Some((6, 1));
    projected.agents[1].tokens = vec![("stage".into(), "plan · sketch".into())];
    // Wide enough for the arrow, the stage word and git or the counts.
    let folded = || TreeRenderInput {
        folded: &["ws_2"],
        width: 30,
        ..tall(200)
    };
    let render = render_tree(&projected, &config, folded());
    assert_eq!(
        render.rows[2], "▎ ⠹ herdr ▾    Plan ↑6 ↓1",
        "open: the stage word, then git"
    );
    assert_eq!(render.buffer[(15, 2)].fg, palette.mauve, "plan");
    assert_eq!(render.buffer[(20, 2)].fg, palette.overlay0, "git dim");
    assert_eq!(
        render.rows[5], "▎ ! dotworks ▸       Plan",
        "folded: the stage word alone; its done agent is not counted"
    );
    assert_eq!(render.buffer[(21, 5)].fg, palette.mauve, "plan");
    assert_eq!(
        render.buffer[(2, 5)].fg,
        approval_colour(palette),
        "main lead needs you"
    );
    assert!(
        render.buffer[(4, 5)].modifier.contains(Modifier::BOLD),
        "every space name is bold, not only the focused one"
    );

    for (stage, word, color) in [
        (
            "review",
            "Review",
            crate::client::shell::task_board::style::rose(palette),
        ),
        ("build", "Build", palette.yellow),
        ("check · try it", "Verify", palette.blue),
        ("ship", "Ship", palette.green),
    ] {
        projected.agents[0].tokens = vec![("stage".into(), stage.into())];
        let render = render_tree(&projected, &config, folded());
        let x = render.rows[2].find(word).expect("stage word drawn");
        let x = render.rows[2][..x].chars().count() as u16;
        assert_eq!(render.buffer[(x, 2)].fg, color, "{stage}");
    }

    // No stage: nothing on an open space without git; folded, no git and only the
    // counts after the arrow (none here: its done agent is not counted).
    projected.agents[0].tokens.clear();
    projected.workspaces[0].git_ahead_behind = None;
    projected.agents[1].tokens.clear();
    projected.workspaces[1].git_ahead_behind = Some((2, 0));
    projected.agents[1].agent_status = AgentStatus::Idle;
    let render = render_tree(&projected, &config, folded());
    assert_eq!(render.rows[2], "▎ ⠹ herdr ▾");
    assert_eq!(render.rows[5], "▎ ○ dotworks ▸", "no git on a folded row");
}

/// herdr's main lead with `stage`, two workers beside it, `herdr-w1` on its main tab and
/// `herdr-w2` in `logs`, both working, and a working `herdr-w1` in dotworks too.
fn phase_snapshot(stage: &str) -> ClientShellSnapshot {
    let mut projected = tree_snapshot();
    projected.agents[0].tokens = vec![("stage".into(), stage.into())];
    projected.agents[1].name = Some("herdr-w1".into());
    projected.agents[1].agent_status = AgentStatus::Working;
    for (pane, name) in [(2, "herdr-w1"), (3, "herdr-w2")] {
        let mut agent = tree_agent(&projected.panes[pane], "claude", AgentStatus::Working);
        agent.name = Some(name.into());
        projected.agents.push(agent);
    }
    projected
}

/// herdr's space row at `width`, with its facts' columns from the name on.
fn space_row(projected: &ClientShellSnapshot, width: u16) -> (String, Buffer) {
    let config = ClientShellConfig::from_config(&Config::default());
    let render = render_tree(projected, &config, TreeRenderInput { width, ..tall(200) });
    (render.rows[2].clone(), render.buffer)
}

/// herdr's space row at `width` with `facts` at its right, where every row's facts end: five
/// columns before the edge, past the `+` column.
fn herdr_row(facts: &str, width: usize) -> String {
    let room = width - 5 - "▎ ⠹ herdr ▾ ".chars().count();
    format!("▎ ⠹ herdr ▾ {facts:>room$}")
}

/// The column where `text` starts on row 2 of a render, by screen column.
fn column_of(row: &str, text: &str) -> u16 {
    let x = row.find(text).expect("drawn");
    row[..x].chars().count() as u16
}

#[test]
fn tree_space_row_says_the_leads_phase_word_and_never_an_agents_name() {
    let config = ClientShellConfig::from_config(&Config::default());
    let palette = &config.palette;
    let (row, buffer) = space_row(&phase_snapshot("build · herdr-w1 on B2a Phases layout"), 34);
    assert_eq!(row, herdr_row("Build", 34));
    assert_eq!(
        buffer[(column_of(&row, "Build"), 2)].fg,
        palette.yellow,
        "the word in its phase color"
    );
    // However many agents its stage names, working or not, in this space or another.
    for stage in [
        "build · herdr-w1 on B2a Pha",
        "build · herdr-w1, herdr-w2 on 2 tasks",
        "build ·  herdr-w1 ,herdr-w2  on 2 tasks",
    ] {
        let (row, _) = space_row(&phase_snapshot(stage), 36);
        assert_eq!(row, herdr_row("Build", 36), "{stage}");
    }
    let mut projected = phase_snapshot("build · herdr-w1 on B2a");
    projected.agents[3].agent_status = AgentStatus::Idle;
    let (row, _) = space_row(&projected, 36);
    assert_eq!(row, herdr_row("Build", 36), "idle");

    // A worker's own row keeps its name.
    let render = render_tree(
        &phase_snapshot("build · herdr-w1 on B2a"),
        &config,
        TreeRenderInput {
            width: 34,
            ..tall(200)
        },
    );
    assert!(
        render.rows[3..]
            .iter()
            .any(|row| row.contains("╴") && row.contains("herdr-w1")),
        "{:#?}",
        render.rows
    );
}

#[test]
fn tree_space_row_shows_a_phase_word_never_the_raw_stage() {
    let config = ClientShellConfig::from_config(&Config::default());
    let palette = &config.palette;
    for (stage, word, color) in [
        ("plan", "Plan", palette.mauve),
        (
            "review · 2 findings",
            "Review",
            crate::client::shell::task_board::style::rose(palette),
        ),
        ("verify", "Verify", palette.blue),
        ("check · try it", "Verify", palette.blue),
        ("ship", "Ship", palette.green),
        ("Build", "Build", palette.yellow),
        ("BUILD · nobody", "Build", palette.yellow),
        ("build", "Build", palette.yellow),
        ("build ·", "Build", palette.yellow),
        ("build · ", "Build", palette.yellow),
    ] {
        let (row, buffer) = space_row(&phase_snapshot(stage), 30);
        assert_eq!(row, herdr_row(word, 30), "{stage}");
        assert_eq!(buffer[(column_of(&row, word), 2)].fg, color, "{stage}");
    }
}

#[test]
fn tree_space_row_shows_no_phase_for_a_word_that_is_not_one() {
    let config = ClientShellConfig::from_config(&Config::default());
    let worked = |projected: &ClientShellSnapshot| {
        render_tree(
            projected,
            &config,
            TreeRenderInput {
                width: 30,
                working_for: &[("pane_1", 300)],
                ..tall(200)
            },
        )
        .rows[2]
            .clone()
    };
    for stage in ["waiting · Nadine: 3 picks", "research", "", " · herdr-w1"] {
        assert_eq!(
            worked(&phase_snapshot(stage)),
            herdr_row("5m", 30),
            "{stage:?}: its worked time"
        );
    }
    let mut projected = phase_snapshot("build");
    projected.agents[0].tokens.clear();
    assert_eq!(worked(&projected), herdr_row("5m", 30), "no token");
    let (row, _) = space_row(&phase_snapshot("waiting · Nadine: 3 picks"), 30);
    assert_eq!(row, "▎ ⠹ herdr ▾", "nothing when it has no time");

    // A worker keeps its time, whatever its stage says.
    let mut projected = phase_snapshot("build · herdr-w1 on B2a");
    projected.agents[0]
        .tokens
        .push(("lead".into(), "pane_9".into()));
    assert_eq!(worked(&projected), herdr_row("5m", 30), "a worker");
}

#[test]
fn tree_space_row_drops_its_phase_word_when_narrow() {
    let projected = phase_snapshot("build · herdr-w1 on B2a Phases layout");
    for width in [33, 22] {
        let (row, _) = space_row(&projected, width);
        assert_eq!(
            row,
            herdr_row("Build", width.into()),
            "the word fits at {width}"
        );
    }
    for width in [21, 18] {
        let (row, _) = space_row(&projected, width);
        assert_eq!(
            row, "▎ ⠹ herdr ▾",
            "then nothing, down to the narrowest sidebar"
        );
    }
}

fn drawn_tab(state: &ClientShellState, tab_id: &str) -> bool {
    state
        .hits
        .tree_rows
        .iter()
        .any(|(_, target)| *target == TreeTarget::Tab(tab_id.into()))
}

fn drawn_pane(state: &ClientShellState, pane_id: &str) -> bool {
    state
        .hits
        .tree_rows
        .iter()
        .any(|(_, target)| *target == TreeTarget::Pane(pane_id.into()))
}

/// The box: its rows from its first heading down to the blank row above the footer, after
/// checking the rule sits right above that heading and the blank row right above the footer.
fn panel(render: &TreeRender) -> Vec<String> {
    let footer = render.rows.len() - 1;
    let top = (0..footer)
        .find(|&y| is_box_heading(&render.rows[y]))
        .unwrap_or_else(|| panic!("a heading: {:#?}", render.rows));
    assert!(
        top > 0 && is_box_rule(&render.rows[top - 1]) && render.rows[footer - 1].is_empty(),
        "a rule above the box and a blank row below it: {:#?}",
        render.rows
    );
    render.rows[top..footer - 1].to_vec()
}

/// The box's rule: `─` from the second column to one before the edge.
fn is_box_rule(row: &str) -> bool {
    row.strip_prefix(' ')
        .is_some_and(|rule| !rule.is_empty() && rule.chars().all(|c| c == '─'))
}

/// A box heading's row: `ACTION REQUIRED` or `COMPLETED` in the mark column, cut to fit,
/// then its count one space after it.
fn is_box_heading(row: &str) -> bool {
    ["ACTION REQUIRED", "COMPLETED"].iter().any(|title| {
        row.strip_prefix("  ")
            .and_then(|row| row.rsplit_once(' '))
            .is_some_and(|(shown, count)| {
                count.parse::<usize>().is_ok()
                    && (title.starts_with(shown)
                        || shown
                            .strip_suffix('…')
                            .is_some_and(|cut| !cut.is_empty() && title.starts_with(cut)))
            })
    })
}

/// No panel: no heading, and the tree runs from under the header down to the footer.
fn assert_no_panel(render: &TreeRender) {
    assert!(
        !render.rows.iter().any(|row| is_box_heading(row)),
        "{:#?}",
        render.rows
    );
    let footer = render.rows.len() as u16 - 1;
    assert_eq!(
        render.hits.workspace_body.bottom(),
        footer,
        "{:#?}",
        render.rows
    );
}

/// Room for every item of `panel_snapshot`: a third of 45 rows, minus the two rules.
fn tall_panel(spinner_ms: u64) -> TreeRenderInput {
    TreeRenderInput {
        height: 45,
        ..tall(spinner_ms)
    }
}

/// `lead_space` agents with the given states, in order.
fn agents_space(agents: &[(&str, AgentStatus)]) -> ClientShellSnapshot {
    let names = agents
        .iter()
        .map(|(name, _)| (*name, None))
        .collect::<Vec<_>>();
    let mut projected = lead_space(&names);
    for (agent, (_, status)) in projected.agents.iter_mut().zip(agents) {
        agent.agent_status = *status;
    }
    projected
}

/// One of each panel item over two spaces: `web`, whose check stage adds nothing,
/// with a done agent on a task, a blocked one and one asking; `dotworks` with a
/// blocked agent that also asks and a done one without a task.
fn panel_snapshot() -> ClientShellSnapshot {
    let mut projected = agents_space(&[
        ("w7", AgentStatus::Done),
        ("cx", AgentStatus::Blocked),
        ("rev", AgentStatus::Idle),
    ]);
    projected.workspaces[0].label = "web".into();
    projected.agents[0].tokens = vec![
        ("task".into(), "wire panel".into()),
        ("stage".into(), "check · tree panel".into()),
    ];
    projected.agents[2].tokens = vec![("ask".into(), "merge now?".into())];
    projected
        .workspaces
        .push(tree_workspace("ws_2", 2, "dotworks", AgentStatus::Blocked));
    for (id, name, status, ask) in [
        (4, "db", AgentStatus::Blocked, Some("which db?")),
        (5, "pair", AgentStatus::Done, None),
    ] {
        push_shell_tab(&mut projected, "ws_2", id, name);
        let pane = projected.panes.last().expect("pushed pane");
        let mut agent = tree_agent(pane, "claude", status);
        agent.name = Some(name.into());
        agent.tokens = ask
            .map(|ask| vec![("ask".into(), ask.into())])
            .unwrap_or_default();
        projected.agents.push(agent);
    }
    projected
}

const PANEL: [&str; 8] = [
    "  ACTION REQUIRED 3",
    "  ! web/cx",
    "  ? web/rev",
    "  ? dotworks",
    "",
    "  COMPLETED 2",
    "  ✓ web",
    "  ✓ dotworks/pair",
];

#[test]
fn tree_box_entries_are_one_row_under_plain_headings_at_the_mark_column() {
    let config = ClientShellConfig::from_config(&Config::default());
    let render = render_tree(&panel_snapshot(), &config, tall_panel(200));
    let footer = render.rows.len() - 1;
    assert_eq!(
        render.rows[footer - 10..],
        [
            " ───────────────────────",
            "  ACTION REQUIRED 3",
            "  ! web/cx",
            "  ? web/rev",
            "  ? dotworks",
            "",
            "  COMPLETED 2",
            "  ✓ web",
            "  ✓ dotworks/pair",
            "",
            "  ≡ menu               «",
        ],
        "plain headings under a rule above the box, a blank row under it; no reason rows: {:#?}",
        render.rows
    );
    let palette = &config.palette;
    let heading = (footer - 9) as u16;
    let cell = |x: u16, y: u16| &render.buffer[(x, y)];
    let rule = heading - 1;
    assert_eq!(cell(0, rule).symbol(), " ", "the rule starts one column in");
    for x in 1..24 {
        assert_eq!(cell(x, rule).symbol(), "─", "the rule, column {x}");
        assert_eq!(cell(x, rule).fg, palette.surface1, "the rule is faint");
    }
    assert_eq!(
        render.hits.workspace_body.bottom(),
        heading - 1,
        "the tree stops above the rule over the box"
    );
    // In the mark column, above the `!`, `?` and `✓`.
    assert_eq!(
        cell(2, heading).fg,
        needs_you_colour(palette),
        "ACTION REQUIRED"
    );
    assert!(cell(2, heading).modifier.contains(Modifier::BOLD));
    // The count is a plain grey number, one space after the title.
    assert_eq!(cell(18, heading).fg, palette.overlay1, "the count");
    // A blank row between the parts, then COMPLETED.
    assert_eq!(cell(2, heading + 5).fg, done_colour(palette), "COMPLETED");
    assert!(cell(2, heading + 5).modifier.contains(Modifier::BOLD));
    assert_eq!(cell(12, heading + 5).fg, palette.overlay1, "its count");
    let rows = render
        .hits
        .tree_panel
        .iter()
        .map(|(rect, target)| (rect.y, rect.height, target.clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        rows,
        [
            (heading + 1, 1, TreeTarget::Pane("pane_2".into())),
            (heading + 2, 1, TreeTarget::Pane("pane_3".into())),
            (heading + 3, 1, TreeTarget::Pane("pane_4".into())),
            (heading + 6, 1, TreeTarget::Pane("pane_1".into())),
            (heading + 7, 1, TreeTarget::Pane("pane_5".into())),
        ],
        "each entry clicks as one row; the blank row does not"
    );
}

#[test]
fn tree_layout_a_one_gap_under_the_header_plus_first_menu_left_one_rule_over_the_box() {
    let mut config = ClientShellConfig::from_config(&Config::default());
    config.spaces.row_gap = 1;
    let render = render_tree(&tree_snapshot(), &config, tall(200));
    assert_eq!(
        render.rows[..3],
        ["  SPACES ▽       + ☑ / ▾", "", "▎ ⠹ herdr ▾"],
        "one blank row under the header whatever the gap between spaces; + first: {:#?}",
        render.rows
    );
    let x = |rect: Rect| rect.x;
    assert_eq!(
        [
            x(render.hits.new_workspace_header),
            x(render.hits.tree_task_board),
            x(render.hits.tree_search),
            x(render.hits.tree_fold_all),
        ],
        [17, 19, 21, 23]
    );
    assert_eq!(
        render.rows[19], "  ≡ menu               «",
        "the menu in the mark column at the left, « alone at the right"
    );
    assert_eq!(render.hits.global_launcher, Rect::new(2, 19, 6, 1));
    assert_eq!(render.hits.sidebar_toggle.x, 23);

    let render = render_tree(&panel_snapshot(), &config, tall_panel(200));
    let footer = render.rows.len() - 1;
    assert_eq!(
        render.rows[footer - 10..footer - 8],
        [" ───────────────────────", "  ACTION REQUIRED 3"],
        "the rule above the box stays: {:#?}",
        render.rows
    );
    assert_eq!(
        render.rows[footer - 2..footer],
        ["  ✓ dotworks/pair", ""],
        "no rule under the box, a blank row before the footer: {:#?}",
        render.rows
    );
}

#[test]
fn tree_panel_lists_what_needs_you_then_what_is_done_across_spaces() {
    let config = ClientShellConfig::from_config(&Config::default());
    let render = render_tree(&panel_snapshot(), &config, tall_panel(200));
    assert_eq!(
        panel(&render),
        PANEL,
        "spaces in order, all waited as long; each space's main lead by its space's name"
    );
    let palette = &config.palette;
    // Footer at 44, the rule at 34 and a blank row at 43, the eight rows at 35..43:
    // ACTION REQUIRED at 35, the blank row between the parts at 39, COMPLETED at 40.
    let cell = |x: u16, y: u16| &render.buffer[(x, y)];
    assert_eq!(cell(2, 35).fg, needs_you_colour(palette), "ACTION REQUIRED");
    assert!(cell(2, 35).modifier.contains(Modifier::BOLD));
    assert_eq!(cell(2, 36).fg, approval_colour(palette), "blocked mark");
    assert_eq!(
        cell(2, 37).fg,
        palette.yellow,
        "an idle agent's ask needs you too, as a question"
    );
    assert_eq!(cell(2, 41).fg, done_colour(palette), "done mark");
    assert_eq!(cell(4, 36).fg, palette.subtext0, "name, as a worker row's");
    assert!(!cell(4, 36).modifier.contains(Modifier::BOLD));
    assert_eq!(cell(2, 40).fg, done_colour(palette), "COMPLETED");
    assert!(cell(2, 40).modifier.contains(Modifier::BOLD));
}

#[test]
fn tree_panel_and_footer_use_nerd_font_icons_when_icons_are_nerd() {
    let mut config = ClientShellConfig::from_config(&Config::default());
    config.sidebar_tree_icons = crate::config::SidebarTreeIconsConfig::Nerd;
    let render = render_tree(&panel_snapshot(), &config, tall_panel(200));
    let panel = panel(&render);
    assert_eq!(panel[0], "  ACTION REQUIRED 3");
    assert_eq!(panel[1], "  ! web/cx");
    assert_eq!(panel[6], "  ✓ web", "the done mark with nerd icons too");
    assert_eq!(render.buffer[(2, 36)].fg, approval_colour(&config.palette));
    let footer = render.rows.last().expect("footer");
    assert_eq!(footer, "  \u{f0c9} menu               «");

    config.sidebar_tree_icons = crate::config::SidebarTreeIconsConfig::Plain;
    let plain = render_tree(&panel_snapshot(), &config, tall_panel(200));
    let footer = plain.rows.last().expect("footer");
    assert_eq!(footer, "  ≡ menu               «");
}

#[test]
fn tree_footer_is_the_menu_at_the_left_and_collapse_alone_at_the_right() {
    for (icons, menu) in [
        (crate::config::SidebarTreeIconsConfig::Plain, "≡"),
        (crate::config::SidebarTreeIconsConfig::Nerd, "\u{f0c9}"),
    ] {
        let mut config = ClientShellConfig::from_config(&Config::default());
        config.sidebar_tree_icons = icons;
        for width in [30, 36] {
            let render = render_tree(
                &tree_snapshot(),
                &config,
                TreeRenderInput { width, ..tall(200) },
            );
            let footer = render.rows.last().expect("footer");
            let last_icon = render.hits.tree_fold_all.x;
            assert_eq!(
                *footer,
                format!("  {menu} menu{}«", " ".repeat(usize::from(last_icon) - 8)),
                "{icons:?} at {width}: no new; the menu in the mark column, « alone at the right"
            );
            assert_eq!(
                footer.chars().count(),
                usize::from(last_icon) + 1,
                "{icons:?} at {width}: « in the header's last icon column, the last column clear"
            );
            assert_eq!(
                render.hits.sidebar_toggle.x, last_icon,
                "« clicks where it shows"
            );
            assert_eq!(
                render.hits.new_workspace,
                Rect::default(),
                "no footer new: the header + makes a space"
            );
            let launcher = render.hits.global_launcher;
            assert_eq!(
                (launcher.x, launcher.width),
                (2, 6),
                "the menu clicks over its icon and word"
            );
        }
    }
}

#[test]
fn tree_panel_hides_a_section_with_nothing_in_it() {
    let config = ClientShellConfig::from_config(&Config::default());
    let done = agents_space(&[("w7", AgentStatus::Done), ("i", AgentStatus::Idle)]);
    assert_eq!(
        panel(&render_tree(&done, &config, tall(200))),
        ["  COMPLETED 1", "  ✓ herdr"],
        "the first tab's agent is the space's main lead"
    );
    let blocked = agents_space(&[("cx", AgentStatus::Blocked), ("w", AgentStatus::Working)]);
    assert_eq!(
        panel(&render_tree(&blocked, &config, tall(200))),
        ["  ACTION REQUIRED 1", "  ! herdr"]
    );
}

#[test]
fn tree_panel_leaves_a_working_agent_out_of_needs_you_even_with_an_ask() {
    let mut projected = agents_space(&[
        ("busy", AgentStatus::Working),
        ("done", AgentStatus::Done),
        ("stuck", AgentStatus::Blocked),
    ]);
    projected.agents[0].tokens = vec![
        ("ask".into(), "old question".into()),
        ("task".into(), "still going".into()),
    ];
    projected.agents[1].tokens = vec![("ask".into(), "ship it?".into())];
    projected.agents[2].tokens = vec![("ask".into(), "which db?".into())];
    let config = ClientShellConfig::from_config(&Config::default());
    assert_eq!(
        panel(&render_tree(&projected, &config, tall_panel(200))),
        ["  ACTION REQUIRED 2", "  ? herdr/done", "  ? herdr/stuck"],
        "a stale ask on a working agent is ignored; any other agent with an ask needs you"
    );
}

#[test]
fn tree_box_ask_of_marks_alone_counts_as_no_ask() {
    use crate::client::shell::tree_panel::{needs_you, Reason};
    let pane = tree_pane("pane_1", "ws_1", "tab_1");
    for ask in ["", "  ", "**", "` _ #", "> []", "\u{7}"] {
        let mut blocked = tree_agent(&pane, "claude", AgentStatus::Blocked);
        blocked.tokens = vec![("ask".into(), ask.into())];
        assert_eq!(
            needs_you(&blocked),
            Some(Reason::Approval),
            "blocked, ask {ask:?}"
        );
        let mut idle = tree_agent(&pane, "claude", AgentStatus::Idle);
        idle.tokens = vec![("ask".into(), ask.into())];
        assert_eq!(needs_you(&idle), None, "idle, ask {ask:?}");
    }
    let mut idle = tree_agent(&pane, "claude", AgentStatus::Idle);
    idle.tokens = vec![("ask".into(), "**merge?**".into())];
    assert_eq!(needs_you(&idle), Some(Reason::Answer), "marks around words");
}

#[test]
fn tree_panel_hides_when_nothing_needs_you_or_is_done() {
    // More idle agents than rows, so the tree could use every one of them.
    let names = (0..20).map(|n| format!("i{n}")).collect::<Vec<_>>();
    let agents = names
        .iter()
        .map(|name| (name.as_str(), AgentStatus::Idle))
        .collect::<Vec<_>>();
    let mut projected = agents_space(&agents);
    projected.agents[1].agent_status = AgentStatus::Working;
    projected.agents[0].tokens = vec![
        ("ask".into(), "  ".into()),
        ("stage".into(), "build · check later".into()),
    ];
    let config = ClientShellConfig::from_config(&Config::default());
    let render = render_tree(&projected, &config, tall(0));
    // A working agent, a stage and a blank ask add nothing, so there is no panel at all.
    assert_no_panel(&render);
    let footer = render.rows.len() as u16 - 1;
    assert_eq!(
        render.targets.last().map(|(rect, _)| rect.bottom()),
        Some(footer),
        "tree rows run down to the footer: {:#?}",
        render.rows
    );
}

#[test]
fn tree_panel_is_capped_at_a_third_and_counts_what_it_hides() {
    let config = ClientShellConfig::from_config(&Config::default());
    let input = TreeRenderInput {
        height: 26,
        ..tall(0)
    };
    let render = render_tree(&panel_snapshot(), &config, input);
    assert_eq!(
        panel(&render),
        [
            "  ACTION REQUIRED 3",
            "  ! web/cx",
            "  ? web/rev",
            "  ? dotworks",
            "  COMPLETED 2",
            "    +2 more"
        ],
        "a third of 26 rows, less the two rules; COMPLETED gives way first, `+N more` counts only its own part"
    );
    // Footer at 25, the rule above it at 24.
    assert_eq!(
        render.buffer[(4, 23)].fg,
        config.palette.overlay0,
        "+N more"
    );

    let projected = agents_space(&[
        ("cx", AgentStatus::Blocked),
        ("a", AgentStatus::Done),
        ("b", AgentStatus::Done),
        ("c", AgentStatus::Done),
    ]);
    // A third of 15 is five rows: three between the rules.
    let three_rows = TreeRenderInput {
        height: 15,
        ..tall(0)
    };
    assert_eq!(
        panel(&render_tree(&projected, &config, three_rows)),
        ["  ACTION REQUIRED 1", "  ! herdr"],
        "no room for COMPLETED: it is left out"
    );
    let two_rows = TreeRenderInput {
        height: 12,
        ..tall(0)
    };
    assert_eq!(
        panel(&render_tree(&panel_snapshot(), &config, two_rows)),
        ["  ACTION REQUIRED 3", "    +3 more"],
        "two rows: the heading and how many wait"
    );
}

#[test]
fn tree_panel_switch_off_gives_the_tree_every_row() {
    let mut config = ClientShellConfig::from_config(&Config::default());
    config.sidebar_tree_panel = false;
    // A focused agent: the switch, not an empty panel, keeps the rows free.
    let mut projected = panel_snapshot();
    projected.panes[0].focused = true;
    let render = render_tree(&projected, &config, tall(0));
    assert!(
        !render.rows.iter().any(|row| is_box_heading(row)),
        "{:#?}",
        render.rows
    );
    assert!(!render
        .rows
        .iter()
        .any(|row| row.contains("ACTION REQUIRED")));
    assert_eq!(render.hits.workspace_body.height, 17);
}

/// A client showing `projected`. The client counts an agent done only once it saw it work,
/// so its done agents are first shown working.
fn panel_state(projected: ClientShellSnapshot, height: u16) -> ClientShellState {
    let mut state = ClientShellState::new(tree_config(true));
    let mut working = projected.clone();
    let mut projected = projected;
    for (before, after) in working.agents.iter_mut().zip(&mut projected.agents) {
        if after.agent_status == AgentStatus::Done {
            before.agent_status = AgentStatus::Working;
            after.state_change_seq += 1;
        }
    }
    state.set_snapshot(Box::new(working));
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.compose(106, height).expect("panel frame");
    state
}

fn panel_row(state: &ClientShellState, target: &TreeTarget) -> Rect {
    state
        .hits
        .tree_panel
        .iter()
        .find(|(_, candidate)| candidate == target)
        .unwrap_or_else(|| panic!("{target:?} not drawn"))
        .0
}

#[test]
fn tree_panel_items_focus_their_pane_on_click() {
    let mut state = panel_state(panel_snapshot(), 45);
    let pane = |id: &str| TreeTarget::Pane(id.into());
    assert_eq!(
        state
            .hits
            .tree_panel
            .iter()
            .map(|(_, target)| target.clone())
            .collect::<Vec<_>>(),
        [
            pane("pane_2"),
            pane("pane_3"),
            pane("pane_4"),
            pane("pane_5")
        ],
        "one target per item; w7 (pane_1) is on screen, so it is seen and not done"
    );
    assert_eq!(
        state
            .hits
            .tree_panel
            .iter()
            .map(|(rect, _)| rect.height)
            .collect::<Vec<_>>(),
        [1, 1, 1, 1],
        "each entry is one row"
    );

    for pane_id in ["pane_2", "pane_4", "pane_5"] {
        let row = panel_row(&state, &pane(pane_id));
        assert!(
            matches!(
                sent_method(&click(&mut state, row)),
                crate::api::schema::Method::PaneFocus(target) if target.pane_id == pane_id
            ),
            "{pane_id}"
        );
    }
}

#[test]
fn tree_panel_heading_and_summary_lines_are_not_clickable() {
    let above = |rect: Rect| Rect::new(rect.x, rect.y - 1, rect.width, 1);
    let mut state = panel_state(panel_snapshot(), 45);
    let heading = above(panel_row(&state, &TreeTarget::Pane("pane_2".into())));
    assert!(click(&mut state, heading).actions.is_empty(), "{heading:?}");

    // 26 rows: ACTION REQUIRED and its three entries, COMPLETED and one, then `+1 more`, between
    // the rules.
    let mut state = panel_state(panel_snapshot(), 26);
    let last = state.hits.tree_panel.last().expect("an item").0;
    let more = Rect::new(last.x, last.bottom(), last.width, 1);
    assert_eq!(state.hits.tree_panel.len(), 4);
    assert!(click(&mut state, more).actions.is_empty());

    // Nothing to show: no panel, the tree body reaches the footer.
    let quiet = agents_space(&[("i", AgentStatus::Idle)]);
    let state = panel_state(quiet, 30);
    assert!(state.hits.tree_panel.is_empty());
    assert_eq!(
        state.hits.workspace_body.bottom(),
        state.hits.sidebar_toggle.y
    );
}

/// Nadine's look with pane branches: `tab 2` and the split's codex pane on `fix`, the
/// `main` shell tab on the space's branch.
fn branch_marks_snapshot() -> ClientShellSnapshot {
    let mut projected = branch_snapshot();
    projected.workspaces[0].branch = Some("main".into());
    projected.panes[0].git_branch = Some("main".into());
    projected.panes[1].git_branch = Some("fix".into());
    projected.panes[3].git_branch = Some("fix".into());
    projected
}

#[test]
fn tree_space_row_keeps_its_name_when_the_facts_do_not_fit() {
    let config = ClientShellConfig::from_config(&Config::default());
    let mut projected = agents_space(&[
        ("a", AgentStatus::Blocked),
        ("b", AgentStatus::Working),
        ("c", AgentStatus::Done),
        ("d", AgentStatus::Idle),
    ]);
    projected.agents[0].tokens = vec![("stage".into(), "check · try it".into())];
    let folded = TreeRenderInput {
        folded: &["ws_1"],
        ..tall(200)
    };
    assert_eq!(
        render_tree(&projected, &config, folded).rows[2],
        format!("▎ ! herdr ▸ {}1 Verify", spinner_glyph(200, &config, true)),
        "the working count after the arrow, then the stage word"
    );
    projected.workspaces[0].label = "herdr-worktrees-sidebar".into();
    projected.agents[0].tokens = vec![("stage".into(), "review · why".into())];
    let folded = TreeRenderInput {
        folded: &["ws_1"],
        ..tall(200)
    };
    assert_eq!(
        render_tree(&projected, &config, folded).rows[2],
        "▎ ! herdr-worktree… ▸",
        "the counts go first, then the stage word; the name stays"
    );
}

#[test]
fn tree_panel_marks_needs_you_entries_the_same_in_either_mark_style() {
    let mut config = ClientShellConfig::from_config(&Config::default());
    config.status_indicators = crate::config::StatusIndicatorStyle::Symbols;
    let render = render_tree(&panel_snapshot(), &config, tall_panel(200));
    let panel = panel(&render);
    assert_eq!(
        panel[..7],
        [
            "  ACTION REQUIRED 3",
            "  ! web/cx",
            "  ? web/rev",
            "  ? dotworks",
            "",
            "  COMPLETED 2",
            "  ✓ web"
        ],
        "`!` blocked, `?` asked, idle ones too"
    );
    assert_eq!(
        render.buffer[(4, 35)].fg,
        needs_you_colour(&config.palette),
        "heading"
    );
    for (y, color) in [
        (36, approval_colour(&config.palette)),
        (37, config.palette.yellow),
        (38, config.palette.yellow),
    ] {
        assert_eq!(render.buffer[(2, y)].fg, color, "row {y}");
    }
}

#[test]
fn tree_client_notes_when_it_sees_an_agent_turn_done() {
    let mut config = tree_config(true);
    config.sidebar_width = 36;
    let mut state = ClientShellState::new(config);
    let mut projected = agents_space(&[("lead", AgentStatus::Idle), ("w1", AgentStatus::Working)]);
    state.set_snapshot(Box::new(projected.clone()));
    let since = |state: &ClientShellState| state.endpoints[0].done_since.clone();
    assert!(since(&state).is_empty(), "working: not done");

    projected.revision += 1;
    projected.agents[1].agent_status = AgentStatus::Done;
    projected.agents[1].state_change_seq += 1;
    state.set_snapshot(Box::new(projected.clone()));
    let first = since(&state);
    assert_eq!(first.keys().collect::<Vec<_>>(), ["pane_2"]);
    state.set_pane_surface(surface());
    let text = frame_text(&state.compose(106, 45).expect("frame"));
    assert!(text.contains("just now"), "{text}");

    projected.revision += 1;
    state.set_snapshot(Box::new(projected.clone()));
    assert_eq!(since(&state), first, "still done: the moment stays");

    projected.revision += 1;
    projected.agents[1].agent_status = AgentStatus::Working;
    projected.agents[1].state_change_seq += 1;
    state.set_snapshot(Box::new(projected));
    assert!(since(&state).is_empty(), "working again: forgotten");
}

#[test]
fn tree_panel_names_draw_no_control_characters() {
    let mut projected = agents_space(&[
        ("a\u{1b}[31mb\u{7}", AgentStatus::Idle),
        ("c\u{9b}d", AgentStatus::Idle),
    ]);
    projected.workspaces[0].label = "we\u{1b}]0;x\u{7}b\u{0}".into();
    for agent in &mut projected.agents {
        agent.tokens = vec![("ask".into(), "ok".into())];
    }
    let config = ClientShellConfig::from_config(&Config::default());
    let render = render_tree(
        &projected,
        &config,
        TreeRenderInput {
            width: 40,
            ..tall_panel(200)
        },
    );
    let panel = panel(&render);
    let rows = render.rows.len() as u16;
    for y in rows - 1 - panel.len() as u16..rows - 1 {
        for x in 0..render.buffer.area.width {
            let symbol = render.buffer[(x, y)].symbol();
            assert!(
                !symbol.chars().any(char::is_control),
                "row {y} col {x}: {symbol:?} in {panel:?}"
            );
        }
    }
    assert_eq!(
        panel,
        ["  ACTION REQUIRED 2", "  ? we]0;xb", "  ? we]0;xb/cd"],
        "control characters go, the rest of each name stays"
    );
}

/// The approved box's spaces: `oakwood-workspace` whose one agent asks; `acme` whose main
/// lead idles, with `acme-api-lead` blocked on a permission, its working `w1`, and
/// `acme-web-lead` done; `herdr` whose lead idles with two workers waiting. Tab `tab_N` holds
/// pane `pane_N`, one agent each.
fn box_snapshot() -> ClientShellSnapshot {
    let mut projected = snapshot();
    projected.workspaces = vec![
        tree_workspace("ws_1", 1, "oakwood-workspace", AgentStatus::Idle),
        tree_workspace("ws_2", 2, "acme", AgentStatus::Blocked),
        tree_workspace("ws_3", 3, "herdr", AgentStatus::Blocked),
    ];
    projected.tabs = Vec::new();
    projected.panes = Vec::new();
    projected.agents = Vec::new();
    // (space, agent name, state, ask, lead pane)
    type BoxAgent = (
        &'static str,
        &'static str,
        AgentStatus,
        Option<&'static str>,
        Option<&'static str>,
    );
    let agents: [BoxAgent; 8] = [
        (
            "ws_1",
            "oakwood-lead",
            AgentStatus::Idle,
            Some("`merge` it?"),
            None,
        ),
        ("ws_2", "acme-lead", AgentStatus::Idle, None, None),
        ("ws_2", "acme-api-lead", AgentStatus::Blocked, None, None),
        ("ws_2", "w1", AgentStatus::Working, None, Some("pane_3")),
        ("ws_2", "acme-web-lead", AgentStatus::Done, None, None),
        ("ws_3", "herdr-lead", AgentStatus::Idle, None, None),
        (
            "ws_3",
            "herdr-w1",
            AgentStatus::Blocked,
            None,
            Some("pane_6"),
        ),
        (
            "ws_3",
            "herdr-w2",
            AgentStatus::Idle,
            Some("which?"),
            Some("pane_6"),
        ),
    ];
    for (index, (workspace_id, name, status, ask, lead)) in agents.into_iter().enumerate() {
        let tab_id = format!("tab_{}", index + 1);
        let number = projected
            .tabs
            .iter()
            .filter(|tab| tab.workspace_id == workspace_id)
            .count()
            + 1;
        projected
            .tabs
            .push(tree_tab(&tab_id, workspace_id, number, Some(name)));
        let pane = tree_pane(&format!("pane_{}", index + 1), workspace_id, &tab_id);
        let mut agent = tree_agent(&pane, "claude", status);
        agent.name = Some(name.into());
        agent.tokens = ask
            .map(|ask| ("ask".to_owned(), ask.to_owned()))
            .into_iter()
            .chain(lead.map(|lead| ("lead".to_owned(), lead.to_owned())))
            .collect();
        projected.panes.push(pane);
        projected.agents.push(agent);
    }
    projected.focused_workspace_id = None;
    projected.focused_tab_id = None;
    projected.focused_pane_id = None;
    projected
}

/// The approved box's width, times and height.
#[test]
fn tree_box_titles_say_action_required_and_completed_cut_to_fit() {
    let config = ClientShellConfig::from_config(&Config::default());
    let render = render_tree(&box_snapshot(), &config, box_input());
    let panel = panel(&render);
    assert_eq!(panel[0], "  ACTION REQUIRED 4");
    assert_eq!(panel[6], "  COMPLETED 1");
    // The count is a plain grey number one space after the title, as the task board's
    // headings; the title bold in the colour with no fill.
    let palette = &config.palette;
    for (title, count, role) in [
        ("ACTION REQUIRED", "4", palette.yellow),
        ("COMPLETED", "1", status_color(AgentStatus::Done, palette)),
    ] {
        let y = render
            .rows
            .iter()
            .position(|row| row.starts_with(&format!("  {title} ")))
            .expect("heading drawn") as u16;
        let x = (title.len() + 3) as u16;
        let count_cell = &render.buffer[(x, y)];
        assert_eq!(
            (count_cell.fg, count_cell.symbol(), count_cell.bg),
            (palette.overlay1, count, ratatui::style::Color::Reset)
        );
        let first = &render.buffer[(2, y)];
        assert_eq!((first.fg, first.bg), (role, ratatui::style::Color::Reset));
        assert!(first.modifier.contains(ratatui::style::Modifier::BOLD));
    }

    // The narrowest sidebar: the title is cut, its count stays, both before the facts'
    // edge, well inside the border.
    for width in [18, 14] {
        let render = render_tree(
            &box_snapshot(),
            &config,
            TreeRenderInput {
                width,
                ..box_input()
            },
        );
        let headings = render
            .rows
            .iter()
            .filter(|row| is_box_heading(row))
            .collect::<Vec<_>>();
        let (action, completed) = match width {
            18 => ("  ACTION R… 4", "  COMPLETED 1"),
            _ => ("  ACTI… 4", "  COMP… 1"),
        };
        assert_eq!(headings, [action, completed], "width {width}");
        for row in headings {
            assert!(
                row.chars().count() <= usize::from(width) - 5,
                "{row:?} ends where the tree's facts do"
            );
        }
    }
}

fn box_input() -> TreeRenderInput {
    TreeRenderInput {
        width: 30,
        waiting_for: &[
            ("pane_1", 480),
            ("pane_3", 90),
            ("pane_7", 30),
            ("pane_8", 20),
        ],
        done_for: &[("pane_5", 180)],
        ..tall_panel(200)
    }
}

#[test]
fn tree_box_has_needs_you_and_done_parts_each_under_its_heading() {
    let config = ClientShellConfig::from_config(&Config::default());
    let render = render_tree(&box_snapshot(), &config, box_input());
    let panel = panel(&render);
    assert!(
        panel[0] == "  ACTION REQUIRED 4",
        "a plain heading with its count: {panel:?}"
    );
    let done = panel
        .iter()
        .position(|line| line == "  COMPLETED 1")
        .unwrap_or_else(|| panic!("a COMPLETED heading: {panel:?}"));
    assert_eq!(
        panel.iter().filter(|line| line.is_empty()).count(),
        1,
        "one blank line between the parts: {panel:?}"
    );
    assert!(
        panel[done - 1].is_empty(),
        "right above COMPLETED: {panel:?}"
    );
    assert!(
        !panel
            .iter()
            .any(|line| line.contains("merge") || line.contains("which")),
        "no ask text: {panel:?}"
    );
    assert!(
        panel[done + 1].starts_with("  ✓ "),
        "a finished entry has the done mark: {panel:?}"
    );
    let rows = render.rows.len();
    // Above the rule over the footer.
    let heading = rows - 2 - panel.len();
    let palette = &config.palette;
    assert_eq!(
        render.buffer[(4, heading as u16)].fg,
        needs_you_colour(palette)
    );
    assert!(render.buffer[(4, heading as u16)]
        .modifier
        .contains(Modifier::BOLD));
    let done_y = (heading + done) as u16;
    assert_eq!(render.buffer[(4, done_y)].fg, done_colour(palette));
    assert!(render.buffer[(4, done_y)].modifier.contains(Modifier::BOLD));

    let mut waiting_only = box_snapshot();
    waiting_only.agents[4].agent_status = AgentStatus::Idle;
    let panel = self::panel(&render_tree(&waiting_only, &config, box_input()));
    assert!(panel[0].starts_with("  ACTION REQUIRED "), "{panel:?}");
    assert!(
        !panel
            .iter()
            .any(|line| line.is_empty() || line.contains("COMPLETED")),
        "nothing done: no COMPLETED part and no blank line: {panel:?}"
    );

    let mut done_only = box_snapshot();
    for agent in &mut done_only.agents {
        agent.tokens.retain(|(key, _)| key != "ask");
        if agent.agent_status == AgentStatus::Blocked {
            agent.agent_status = AgentStatus::Idle;
        }
    }
    let panel = self::panel(&render_tree(&done_only, &config, box_input()));
    assert!(
        panel[0].starts_with("  COMPLETED "),
        "nothing waits: COMPLETED alone: {panel:?}"
    );
    assert!(
        !panel.iter().any(String::is_empty),
        "no blank line without two parts: {panel:?}"
    );
}

#[test]
fn tree_box_needs_you_entries_show_how_long_they_waited_oldest_first() {
    let config = ClientShellConfig::from_config(&Config::default());
    let render = render_tree(&box_snapshot(), &config, box_input());
    let panel = panel(&render);
    assert_eq!(
        panel[..8],
        [
            "  ACTION REQUIRED 4",
            "  ? oakwood-workspace  8m",
            "  ! acme/acme-api-lead 1m",
            "  ! herdr/herdr-w1",
            "    +1 more",
            "",
            "  COMPLETED 1",
            "  ✓ acme/acme-web-lead"
        ],
        "oldest first; the space's main lead by the space's name, anyone else space/agent; \
         under a minute no time, as on a working row"
    );
    let untimed = TreeRenderInput {
        waiting_for: &[("pane_1", 480), ("pane_3", 90)],
        ..box_input()
    };
    let panel_untimed = self::panel(&render_tree(&box_snapshot(), &config, untimed));
    assert!(
        matches!(
            panel_untimed[3].as_str(),
            "  ! herdr/herdr-w1" | "   ! herdr/w2"
        ),
        "a wait with no recorded start (already waiting when this client started) shows no \
         time: {panel_untimed:?}"
    );
    let wait_at = |waiting_for| {
        let input = TreeRenderInput {
            waiting_for,
            ..box_input()
        };
        self::panel(&render_tree(&box_snapshot(), &config, input))[3].clone()
    };
    assert_eq!(
        wait_at(&[("pane_1", 480), ("pane_3", 90), ("pane_7", 30)]),
        "  ! herdr/herdr-w1",
        "a wait under a minute shows no time"
    );
    assert_eq!(
        wait_at(&[("pane_1", 480), ("pane_3", 90), ("pane_7", 60)]),
        "  ! herdr/herdr-w1     1m",
        "a wait shows its time once one minute has passed"
    );
    let rows = render.rows.len();
    // The first entry, under the heading.
    let first = (rows - 1 - panel.len()) as u16;
    let palette = &config.palette;
    assert_eq!(
        render.buffer[(2, first)].fg,
        palette.yellow,
        "a question's mark"
    );
    assert_eq!(
        render.buffer[(4, first)].fg,
        palette.subtext0,
        "name, as a worker row's"
    );
    assert!(!render.buffer[(4, first)].modifier.contains(Modifier::BOLD));
    assert_eq!(render.buffer[(23, first)].fg, palette.overlay0, "dim time");

    let narrow = TreeRenderInput {
        width: 26,
        ..box_input()
    };
    let panel = self::panel(&render_tree(&box_snapshot(), &config, narrow));
    assert_eq!(
        panel[1..3],
        ["  ? oakwood-works… 8m", "  ! ac…/acme-api-lead"],
        "a space's long name is cut so the time shows; a lead's stays whole, its time goes"
    );

    let mut state = panel_state(box_snapshot(), 45);
    let rect = panel_row(&state, &TreeTarget::Pane("pane_1".into()));
    assert_eq!(rect.height, 1, "one row per entry");
    assert!(matches!(
        sent_method(&click(&mut state, rect)),
        crate::api::schema::Method::PaneFocus(target) if target.pane_id == "pane_1"
    ));
}

#[test]
fn tree_box_shows_three_entries_that_need_you_then_how_many_more() {
    let config = ClientShellConfig::from_config(&Config::default());
    let render = render_tree(&box_snapshot(), &config, box_input());
    let panel = panel(&render);
    assert_eq!(
        panel[..9.min(panel.len())],
        [
            "  ACTION REQUIRED 4",
            "  ? oakwood-workspace  8m",
            "  ! acme/acme-api-lead 1m",
            "  ! herdr/herdr-w1",
            "    +1 more",
            "",
            "  COMPLETED 1",
            "  ✓ acme/acme-web-lead"
        ],
        "the newest waits behind `+1 more`"
    );
    // `+1 more` is the box's fifth row under its top rule.
    let more = (render.rows.len() - 2 - panel.len() + 4) as u16;
    assert_eq!(render.buffer[(4, more)].fg, config.palette.overlay0, "dim");
    assert!(
        !render
            .hits
            .tree_panel
            .iter()
            .any(|(rect, _)| rect.y == more),
        "`+1 more` is not clickable"
    );
}

#[test]
fn tree_box_shrinks_the_space_before_a_leads_name_then_drops_the_time() {
    // No main lead (a shell tab comes first): `herdr-lead` and a worker, both blocked.
    let mut projected = with_first_shell(lead_space(&[
        ("herdr-lead", None),
        ("herdr-worker-long", Some("pane_1")),
    ]));
    for agent in &mut projected.agents {
        agent.agent_status = AgentStatus::Blocked;
    }
    let config = ClientShellConfig::from_config(&Config::default());
    for (width, want) in [
        (26, ["  ! h…/herdr-lead 12m", "  ! herdr/herdr-w… 1m"]),
        (
            30,
            ["  ! herdr/herdr-lead  12m", "  ! herdr/herdr-worke… 1m"],
        ),
        (22, ["  ! h…/herdr-lead", "  ! herdr/her… 1m"]),
        (20, ["  ! h…/he…-lead", "  ! herdr/h… 1m"]),
    ] {
        let input = TreeRenderInput {
            width,
            waiting_for: &[("pane_1", 720), ("pane_2", 60)],
            ..tall_panel(0)
        };
        let panel = panel(&render_tree(&projected, &config, input));
        assert_eq!(
            panel[1..3],
            want,
            "a lead: the space shrinks, then the time goes, then the name is cut in its \
             middle; a worker as before, at {width} columns: {panel:#?}"
        );
    }
}

#[test]
fn tree_box_parts_have_a_blank_row_between_them_given_up_before_any_entry() {
    let config = ClientShellConfig::from_config(&Config::default());
    let picture = [
        "  ACTION REQUIRED 4",
        "  ? oakwood-workspace  8m",
        "  ! acme/acme-api-lead 1m",
        "  ! herdr/herdr-w1",
        "    +1 more",
        "",
        "  COMPLETED 1",
        "  ✓ acme/acme-web-lead",
    ];
    let render = render_tree(&box_snapshot(), &config, box_input());
    assert_eq!(panel(&render), picture);
    // A third of 27 rows less the two rules is seven: one short, so the blank row goes and
    // every entry stays.
    let one_short = TreeRenderInput {
        height: 27,
        ..box_input()
    };
    let render = render_tree(&box_snapshot(), &config, one_short);
    let without_blank = picture
        .iter()
        .filter(|row| !row.is_empty())
        .copied()
        .collect::<Vec<_>>();
    assert_eq!(panel(&render), without_blank);
}

#[test]
fn tree_box_matches_the_approved_picture() {
    let config = ClientShellConfig::from_config(&Config::default());
    let render = render_tree(&box_snapshot(), &config, box_input());
    let panel = panel(&render);
    assert_eq!(
        panel,
        [
            "  ACTION REQUIRED 4",
            "  ? oakwood-workspace  8m",
            "  ! acme/acme-api-lead 1m",
            "  ! herdr/herdr-w1",
            "    +1 more",
            "",
            "  COMPLETED 1",
            "  ✓ acme/acme-web-lead"
        ]
    );
    // Right above the rule over the footer.
    let row = (render.rows.len() - 3) as u16;
    let palette = &config.palette;
    assert_eq!(
        render.buffer[(2, row)].fg,
        done_colour(palette),
        "done mark"
    );
    assert_eq!(
        render.buffer[(4, row)].fg,
        palette.subtext0,
        "name, as a worker row's"
    );
    // A lead's whole name, its time left out for it.
    assert_eq!(
        render.buffer[(21, row)].fg,
        palette.subtext0,
        "the name's last letter"
    );
}

#[test]
fn tree_box_done_rows_are_newest_first_three_at_most_and_keep_their_time() {
    let projected = agents_space(&[
        ("lead", AgentStatus::Idle),
        ("a-very-long-worker-name", AgentStatus::Done),
        ("b", AgentStatus::Done),
        ("c", AgentStatus::Done),
        ("d", AgentStatus::Done),
    ]);
    let input = TreeRenderInput {
        width: 30,
        done_for: &[
            ("pane_2", 30),
            ("pane_3", 125),
            ("pane_4", 3900),
            ("pane_5", 600),
        ],
        ..tall_panel(200)
    };
    let config = ClientShellConfig::from_config(&Config::default());
    assert_eq!(
        panel(&render_tree(&projected, &config, input)),
        [
            "  COMPLETED 4",
            "  ✓ h…/a-very-long-…-name",
            "  ✓ herdr/b        2m ago",
            "  ✓ herdr/d       10m ago",
            "    +1 more"
        ],
        "a long name is cut so the time shows; the oldest waits behind `+1 more`"
    );
}

#[test]
fn tree_client_notes_when_it_first_sees_an_agent_need_you() {
    let mut state = ClientShellState::new(tree_config(true));
    let mut projected = agents_space(&[("lead", AgentStatus::Idle), ("w1", AgentStatus::Working)]);
    state.set_snapshot(Box::new(projected.clone()));
    let since = |state: &ClientShellState| state.endpoints[0].waiting_since.clone();
    assert!(since(&state).is_empty(), "nobody waits");

    projected.revision += 1;
    projected.agents[1].agent_status = AgentStatus::Blocked;
    projected.agents[1].state_change_seq += 1;
    projected.agents[0].tokens = vec![("ask".into(), "go?".into())];
    state.set_snapshot(Box::new(projected.clone()));
    let first = since(&state);
    let mut waiting = first.keys().cloned().collect::<Vec<_>>();
    waiting.sort();
    assert_eq!(waiting, ["pane_1", "pane_2"], "an ask and a block");

    projected.revision += 1;
    state.set_snapshot(Box::new(projected.clone()));
    assert_eq!(since(&state), first, "still waiting: the moment stays");

    projected.revision += 1;
    projected.agents[1].agent_status = AgentStatus::Working;
    projected.agents[1].state_change_seq += 1;
    projected.agents[0].tokens.clear();
    state.set_snapshot(Box::new(projected));
    assert!(since(&state).is_empty(), "answered: forgotten");
}

#[test]
fn tree_state_marks_are_herdrs_own_in_nerd_mode_too() {
    let mut config = ClientShellConfig::from_config(&Config::default());
    let plain = render_tree(&panel_snapshot(), &config, tall_panel(200));
    config.sidebar_tree_icons = crate::config::SidebarTreeIconsConfig::Nerd;
    let nerd = render_tree(&panel_snapshot(), &config, tall_panel(200));
    assert_eq!(panel(&nerd), panel(&plain));
    assert_eq!(panel(&nerd)[..2], ["  ACTION REQUIRED 3", "  ! web/cx"]);
    let folded = TreeRenderInput {
        folded: &["ws_2"],
        ..tall(200)
    };
    let render = render_tree(&tree_snapshot(), &config, folded);
    assert_eq!(
        render.rows[5], "▎ ! dotworks \u{f460}",
        "a done agent is not counted"
    );
    assert_eq!(
        render.buffer[(2, 5)].fg,
        approval_colour(&config.palette),
        "needs you"
    );
}

#[test]
fn tree_rows_on_another_branch_get_their_own_line() {
    let config = ClientShellConfig::from_config(&Config::default());
    let render = render_tree(&branch_marks_snapshot(), &config, tall(200));
    assert_eq!(
        render.rows[2..12],
        [
            "▎ · herdr ▾",
            "▎ │ main",
            "▎ ├╴· tab 2",
            "▎ │   fix",
            "▎ ├╴⠹ herdr-w7",
            "▎ ╰╴! codex ▾",
            "▎     │ fix",
            "▎     ╰╴· repo",
            "",
            ""
        ],
        "same branch or no pane branch: no line; a split tab's own row gets none"
    );
    assert_eq!(
        render.buffer[(6, 4)].fg,
        config.palette.subtext0,
        "lone shell tab"
    );

    let mut no_space_branch = branch_marks_snapshot();
    no_space_branch.workspaces[0].branch = None;
    let render = render_tree(&no_space_branch, &config, tall(200));
    assert_eq!(
        render.rows[2..5],
        ["▎ · herdr ▾", "▎ │ main", "▎ ├╴· tab 2"],
        "a pane branch in a space without one"
    );
}

#[test]
fn tree_shell_rows_have_their_own_colors_apart_from_branch_lines() {
    let config = ClientShellConfig::from_config(&Config::default());
    let palette = &config.palette;
    let projected = branch_marks_snapshot();
    let render = render_tree(&projected, &config, tall(200));
    assert_eq!(render.rows[8], "▎     │ fix");
    assert_eq!(render.rows[9], "▎     ╰╴· repo");
    assert_eq!(
        render.buffer[(8, 9)].fg,
        shell_mark_colour(palette),
        "shell mark"
    );
    let name = &render.buffer[(10, 9)];
    assert_eq!(name.fg, palette.subtext0, "shell name");
    assert!(!name.modifier.contains(Modifier::BOLD), "normal weight");
    assert_eq!(render.buffer[(8, 8)].fg, palette.overlay0, "branch line");
    // A tab whose only pane is a shell is drawn as that shell.
    assert_eq!(render.rows[4], "▎ ├╴· tab 2");
    assert_eq!(
        render.buffer[(4, 4)].fg,
        shell_mark_colour(palette),
        "lone shell mark"
    );
    assert_eq!(
        render.buffer[(6, 4)].fg,
        palette.subtext0,
        "lone shell name"
    );

    // Focused, the name keeps the focused text colour and weight.
    let (_, target) = render
        .targets
        .iter()
        .find(|(rect, _)| rect.y == 9)
        .expect("shell row hit");
    let TreeTarget::Pane(shell) = target else {
        panic!("shell row targets its pane: {target:?}");
    };
    let mut focused = projected.clone();
    for pane in &mut focused.panes {
        pane.focused = pane.pane_id == *shell;
    }
    let render = render_tree(&focused, &config, tall(200));
    let name = &render.buffer[(10, 9)];
    assert_eq!(name.fg, palette.text, "focused shell name");
    assert!(name.modifier.contains(Modifier::BOLD), "focused is bold");
    assert_eq!(
        render.buffer[(8, 9)].fg,
        shell_mark_colour(palette),
        "focused shell mark"
    );
}

#[test]
fn tree_branch_lines_are_cut_to_the_row_and_lose_control_characters() {
    let config = ClientShellConfig::from_config(&Config::default());
    let mut projected = branch_marks_snapshot();
    projected.panes[1].git_branch = Some("feature/sidebar-tree-and-more".into());
    projected.panes[3].git_branch = Some("fix\u{1b}".into());
    let render = render_tree(&projected, &config, tall(200));
    assert_eq!(
        render.rows[5], "▎ │   feature/sidebar-…",
        "cut one column before the edge"
    );
    assert_eq!(render.rows[8], "▎     │ fix", "control characters dropped");
}

#[test]
fn tree_branch_of_only_control_characters_draws_as_no_branch() {
    let config = ClientShellConfig::from_config(&Config::default());
    let mut none = branch_marks_snapshot();
    none.workspaces[0].branch = None;
    for pane in &mut none.panes {
        pane.git_branch = None;
    }
    let mut control = none.clone();
    control.workspaces[0].branch = Some("\u{7}".into());
    for pane in &mut control.panes {
        pane.git_branch = Some(" \u{1b}".into());
    }
    assert_eq!(
        render_tree(&control, &config, tall(200)).rows,
        render_tree(&none, &config, tall(200)).rows,
        "a branch that cleans to nothing gets no line"
    );
}

#[test]
fn tree_branch_names_lose_control_characters() {
    let mut projected = branch_marks_snapshot();
    projected.workspaces[0].branch = Some("ma\u{7}in".into());
    // The first pane is the space row; tab 2's shell is compared with it.
    projected.panes[0].git_branch = None;
    projected.panes[1].git_branch = Some("ma\u{1b}in".into());
    for pane in &mut projected.panes {
        pane.focused = pane.pane_id == "pane_1";
    }
    let config = ClientShellConfig::from_config(&Config::default());
    let render = render_tree(&projected, &config, tall(200));
    assert_eq!(
        render.rows[3..6],
        ["▎ │ main", "▎ ├╴· tab 2", "▎ ├╴⠹ herdr-w7"],
        "equal once cleaned: no line"
    );
}

#[test]
fn tree_header_is_the_word_spaces_and_its_icons() {
    let config = ClientShellConfig::from_config(&Config::default());
    let render = render_tree(&tree_snapshot(), &config, tall(200));
    assert_eq!(
        render.rows[..9],
        [
            HEADER,
            "",
            "▎ ⠹ herdr ▾",
            "▎ ├╴· repo",
            "▎ ╰╴· logs",
            "▎ ! dotworks ▾",
            "▎ ╰╴✓ claude",
            "",
            "",
        ],
        "no agent totals; no blank row between spaces"
    );
    assert_eq!(
        render.hits.new_workspace_header,
        Rect::new(17, 0, 1, 1),
        "header +, first of the icons"
    );
}

#[test]
fn tree_header_word_starts_in_the_mark_column_with_the_box_and_the_menu() {
    let config = ClientShellConfig::from_config(&Config::default());
    let render = render_tree(&box_snapshot(), &config, box_input());
    let column = |row: &str, text: &str| {
        row.find(text)
            .map(|at| row[..at].chars().count())
            .unwrap_or_else(|| panic!("{text:?} in {row:?}"))
    };
    let edge = column(&render.rows[0], "SPACES");
    assert_eq!(edge, 2, "{:?}", render.rows[0]);
    // A space row's mark, the box's heading and the footer's menu share it.
    let space = render
        .rows
        .iter()
        .find(|row| row.starts_with("  ") && !row.trim().is_empty())
        .expect("a space row");
    assert_eq!(
        space.chars().position(|ch| ch != ' '),
        Some(edge),
        "{space:?}"
    );
    for text in ["ACTION REQUIRED", "COMPLETED", "≡"] {
        let row = render
            .rows
            .iter()
            .find(|row| row.contains(text))
            .unwrap_or_else(|| panic!("{text:?}: {:#?}", render.rows));
        assert_eq!(column(row, text), edge, "{text:?}");
    }
}

#[test]
fn tree_header_has_search_fold_all_and_new_space_icons() {
    let mut config = ClientShellConfig::from_config(&Config::default());
    let render = render_tree(&tree_snapshot(), &config, tall(200));
    assert_eq!(render.rows[0], HEADER);
    assert_eq!(render.buffer[(2, 0)].fg, config.palette.overlay0, "dim");
    assert_eq!(render.hits.new_workspace_header, Rect::new(17, 0, 1, 1));
    assert_eq!(render.hits.tree_task_board, Rect::new(19, 0, 1, 1));
    assert_eq!(render.hits.tree_search, Rect::new(21, 0, 1, 1));
    assert_eq!(render.hits.tree_fold_all, Rect::new(23, 0, 1, 1));
    let all_folded = || TreeRenderInput {
        folded: &["ws_1", "ws_2"],
        ..tall(200)
    };
    assert_eq!(
        render_tree(&tree_snapshot(), &config, all_folded()).rows[0],
        "  SPACES ▽       + ☑ / ▸",
        "every space folded: the arrow opens them all"
    );

    config.sidebar_tree_icons = crate::config::SidebarTreeIconsConfig::Nerd;
    assert_eq!(
        render_tree(&tree_snapshot(), &config, tall(200)).rows[0],
        "  SPACES ▽       \u{f0415} \u{f0135} \u{f0349} \u{f013f}"
    );
    assert_eq!(
        render_tree(&tree_snapshot(), &config, all_folded()).rows[0],
        "  SPACES ▽       \u{f0415} \u{f0135} \u{f0349} \u{f013c}"
    );
}

/// The tree snapshot with an idle agent in herdr's logs tab.
fn filter_snapshot() -> ClientShellSnapshot {
    let mut projected = tree_snapshot();
    let logs = projected.panes[3].clone();
    projected
        .agents
        .push(tree_agent(&logs, "pi", AgentStatus::Idle));
    projected
}

#[test]
fn tree_header_filter_icon_sits_after_the_word_and_shows_each_state() {
    let config = ClientShellConfig::from_config(&Config::default());
    let palette = &config.palette;
    for (filter, icon, color) in [
        (TreeFilter::All, "▽", palette.overlay1),
        (TreeFilter::NeedsYou, "?", palette.yellow),
        (TreeFilter::Working, "◐", palette.teal),
        (TreeFilter::Idle, "○", palette.overlay0),
    ] {
        let render = render_tree(
            &filter_snapshot(),
            &config,
            TreeRenderInput {
                filter,
                ..tall(200)
            },
        );
        assert!(
            render.rows[0].starts_with(&format!("  SPACES {icon}       + ☑ / ▾")),
            "{filter:?}: {:?}",
            render.rows[0]
        );
        assert_eq!(render.hits.tree_filter, Rect::new(9, 0, 1, 1));
        assert_eq!(render.buffer[(9, 0)].fg, color, "{filter:?}");
    }
    // Narrow, it gives way before `+`.
    let narrow = render_tree(
        &filter_snapshot(),
        &config,
        TreeRenderInput {
            width: 16,
            ..tall(200)
        },
    );
    assert!(narrow.hits.tree_filter.is_empty(), "{:?}", narrow.rows[0]);
    assert!(!narrow.hits.new_workspace_header.is_empty());
}

#[test]
fn tree_filter_shows_only_the_agents_in_its_state_and_hides_spaces_with_none() {
    let config = ClientShellConfig::from_config(&Config::default());
    let rows = |filter| {
        let render = render_tree(
            &filter_snapshot(),
            &config,
            TreeRenderInput {
                filter,
                ..tall(200)
            },
        );
        render.rows[2..]
            .iter()
            .filter(|row| !row.trim().is_empty())
            .take_while(|row| !row.contains('─') && !row.contains("ACTION REQUIRED"))
            .cloned()
            .collect::<Vec<_>>()
    };
    let all = rows(TreeFilter::All);
    assert!(
        all.iter().any(|row| row.contains("· repo")),
        "shells under All: {all:#?}"
    );
    assert_eq!(rows(TreeFilter::NeedsYou), ["▎ ! dotworks ▾"]);
    assert_eq!(rows(TreeFilter::Working), ["▎ ⠹ herdr ▾"]);
    let idle = rows(TreeFilter::Idle);
    assert_eq!(idle.len(), 2, "{idle:#?}");
    assert!(
        idle[0].contains("herdr") && idle[1].contains("pi"),
        "{idle:#?}"
    );
}

/// An open space whose first tab is a shell, so its row stands for no agent, and two agent
/// tabs: `states` gives each agent its state and whether it asks you something.
fn open_space_snapshot(states: [(AgentStatus, bool); 2]) -> ClientShellSnapshot {
    let mut projected = tree_snapshot();
    projected
        .workspaces
        .push(tree_workspace("ws_3", 3, "ops", AgentStatus::Unknown));
    for (tab, number) in [("tab_7", 1), ("tab_8", 2), ("tab_9", 3)] {
        projected.tabs.push(tree_tab(tab, "ws_3", number, None));
    }
    let shell = tree_pane("pane_7", "ws_3", "tab_7");
    let panes = [
        tree_pane("pane_8", "ws_3", "tab_8"),
        tree_pane("pane_9", "ws_3", "tab_9"),
    ];
    projected.panes.push(shell);
    for ((pane, name), (status, asks)) in panes.iter().zip(["wone", "wtwo"]).zip(states) {
        let mut agent = tree_agent(pane, name, status);
        if asks {
            agent.tokens = vec![("ask".into(), "which branch?".into())];
        }
        projected.agents.push(agent);
    }
    projected.panes.extend(panes);
    projected
}

#[test]
fn tree_filter_keeps_an_open_spaces_row_exactly_when_it_keeps_one_of_its_agents() {
    let config = ClientShellConfig::from_config(&Config::default());
    // The rows of space ops and its agents, drawn or not with it.
    let ops_rows = |states, filter| {
        let render = render_tree(
            &open_space_snapshot(states),
            &config,
            TreeRenderInput {
                filter,
                height: 40,
                width: 40,
                ..tall(200)
            },
        );
        render.rows[2..]
            .iter()
            .take_while(|row| !row.contains('─') && !row.contains("ACTION REQUIRED"))
            .filter(|row| row.contains("ops") || row.contains("wone") || row.contains("wtwo"))
            .map(|row| row.trim().to_owned())
            .collect::<Vec<_>>()
    };
    let (idle, working, blocked) = (
        AgentStatus::Idle,
        AgentStatus::Working,
        AgentStatus::Blocked,
    );
    // Two agents a filter keeps: the open space's row stays with both their rows.
    for (states, filter) in [
        ([(blocked, false), (idle, true)], TreeFilter::NeedsYou),
        ([(working, false), (working, false)], TreeFilter::Working),
        ([(idle, false), (idle, false)], TreeFilter::Idle),
    ] {
        let rows = ops_rows(states, filter);
        assert_eq!(rows.len(), 3, "{filter:?}: {rows:#?}");
        assert!(rows[0].contains("ops"), "{filter:?}: {rows:#?}");
        assert!(
            rows[1].contains("wone") && rows[2].contains("wtwo"),
            "{filter:?}: {rows:#?}"
        );
    }
    // An idle agent that asks you something counts as needing you, never as idle: under
    // every filter the space row shows exactly when one of its agents' rows does.
    let mixed = [(idle, true), (working, false)];
    for (filter, agent) in [
        (TreeFilter::NeedsYou, Some("wone")),
        (TreeFilter::Working, Some("wtwo")),
        (TreeFilter::Idle, None),
    ] {
        let rows = ops_rows(mixed, filter);
        match agent {
            Some(name) => {
                assert_eq!(rows.len(), 2, "{filter:?}: {rows:#?}");
                assert!(rows[0].contains("ops"), "{filter:?}: {rows:#?}");
                assert!(rows[1].contains(name), "{filter:?}: {rows:#?}");
            }
            None => assert!(rows.is_empty(), "{filter:?}: {rows:#?}"),
        }
    }
}

#[test]
fn a_sidebar_too_narrow_for_the_filter_icon_filters_nothing() {
    let config = ClientShellConfig::from_config(&Config::default());
    let rows = |filter, width| {
        let render = render_tree(
            &filter_snapshot(),
            &config,
            TreeRenderInput {
                filter,
                width,
                ..tall(200)
            },
        );
        (render.rows, render.hits.tree_filter)
    };
    let (all, icon) = rows(TreeFilter::All, 16);
    assert!(icon.is_empty(), "no room for the icon at 16");
    let (working, _) = rows(TreeFilter::Working, 16);
    assert_eq!(working, all, "no rows hide without the icon that says so");
    // With room, the filter still hides rows.
    let (all, _) = rows(TreeFilter::All, 26);
    let (working, icon) = rows(TreeFilter::Working, 26);
    assert!(!icon.is_empty());
    assert_ne!(working, all);
}

#[test]
fn tree_filter_icon_cycles_on_click_and_a_right_click_shows_all() {
    let mut state = tree_state(true);
    state.compose(106, 30).expect("tree frame");
    for want in [
        TreeFilter::NeedsYou,
        TreeFilter::Working,
        TreeFilter::Idle,
        TreeFilter::All,
        TreeFilter::NeedsYou,
    ] {
        let icon = state.hits.tree_filter;
        let outcome = press_cell(&mut state, icon);
        assert!(outcome.repaint);
        assert_eq!(state.tree_filter, want);
        state.compose(106, 30).expect("tree frame");
    }
    let icon = state.hits.tree_filter;
    mouse_at(
        &mut state,
        MouseEventKind::Down(MouseButton::Right),
        icon.x,
        icon.y,
    );
    assert_eq!(state.tree_filter, TreeFilter::All);
}

/// A left press and release on the cell at `rect`'s top-left corner.
fn press_cell(state: &mut ClientShellState, rect: Rect) -> ClientShellInput {
    assert!(!rect.is_empty(), "drawn");
    let mut outcome = mouse_at(
        state,
        MouseEventKind::Down(MouseButton::Left),
        rect.x,
        rect.y,
    );
    let up = mouse_at(state, MouseEventKind::Up(MouseButton::Left), rect.x, rect.y);
    outcome.actions.extend(up.actions);
    outcome
}

#[test]
fn tree_header_search_opens_the_go_to_navigator() {
    let mut state = tree_state(true);
    state.compose(106, 30).expect("tree frame");
    let search = state.hits.tree_search;
    let outcome = press_cell(&mut state, search);
    assert!(outcome.actions.is_empty(), "{:?}", outcome.actions);
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Navigator(_))
    ));
}

#[test]
fn tree_header_fold_all_folds_every_space_then_opens_them_all_and_saves_it() {
    let path = fold_preferences_path();
    let mut config = Config::default();
    config.ui.sidebar_tree = true;
    let config = ClientShellConfig::from_config(&config).with_preferences_path(path.clone());
    let mut state = ClientShellState::new(config);
    state.collapsed_groups.insert(tree_tab_fold_key("tab_1"));
    show(&mut state, calm_snapshot("ws_1", 1));
    click_arrow(&mut state, "ws_2");

    let fold_all = state.hits.tree_fold_all;
    press_cell(&mut state, fold_all);
    let folded = HashSet::from([
        tree_fold_key("ws_1"),
        tree_fold_key("ws_2"),
        tree_tab_fold_key("tab_1"),
    ]);
    assert_eq!(saved_groups(&path), folded, "one space was folded already");
    state.compose(106, 30).expect("folded frame");
    assert!(!drawn_tab(&state, "tab_2") && !drawn_pane(&state, "pane_5"));

    let fold_all = state.hits.tree_fold_all;
    press_cell(&mut state, fold_all);
    assert_eq!(
        saved_groups(&path),
        HashSet::from([tree_tab_fold_key("tab_1")]),
        "tab folds stay"
    );
    state.compose(106, 30).expect("open frame");
    assert!(drawn_tab(&state, "tab_2") && drawn_pane(&state, "pane_5"));
    std::fs::remove_file(path).expect("remove tree fold preferences");
}

#[test]
fn tree_header_plus_always_asks_for_the_new_space_name() {
    let press = |state: &mut ClientShellState, target: Rect| {
        assert!(!target.is_empty(), "button drawn");
        let at = |kind| {
            RawInputEvent::Mouse(crossterm::event::MouseEvent {
                kind,
                column: target.x,
                row: target.y,
                modifiers: KeyModifiers::empty(),
            })
        };
        let mut outcome =
            state.handle_raw_events(vec![at(MouseEventKind::Down(MouseButton::Left))]);
        outcome.actions.extend(
            state
                .handle_raw_events(vec![at(MouseEventKind::Up(MouseButton::Left))])
                .actions,
        );
        outcome
    };
    // The name prompt setting is off by default.
    let mut state = tree_state(true);
    assert!(!state.config.prompt_new_workspace_name);
    state.compose(106, 30).expect("tree frame");
    let header = state.hits.new_workspace_header;
    let opened = press(&mut state, header);
    assert!(opened.actions.is_empty(), "{:?}", opened.actions);
    assert!(
        matches!(
            state.overlay.as_ref(),
            Some(ClientShellOverlay::Rename(ClientRenameOverlay {
                input,
                target: ClientRenameTarget::NewWorkspace { .. },
                ..
            })) if input.as_str() == "repo"
        ),
        "the header + opens the name prompt with the folder name"
    );
    let created = state.handle_input_bytes(b"\r");
    assert!(matches!(
        sent_method(&created),
        crate::api::schema::Method::WorkspaceCreate(params)
            if params.focus
                && params.cwd.as_deref() == Some("/repo")
                && params.source_workspace_id.as_deref() == Some("ws_1")
    ));
}

/// `single_agent_space`, then a second space `notes` after it.
fn two_single_agent_spaces() -> ClientShellSnapshot {
    let mut two = single_agent_space();
    let mut second = two.workspaces[0].clone();
    second.workspace_id = "ws_2".into();
    second.label = "notes".into();
    second.focused = false;
    two.workspaces.push(second);
    two
}

#[test]
fn tree_spaces_follow_one_another_when_row_gap_is_zero() {
    let mut config = ClientShellConfig::from_config(&Config::default());
    config.spaces.row_gap = 0;
    let render = render_tree(&single_agent_space(), &config, tall(200));
    let render_two = render_tree(&two_single_agent_spaces(), &config, tall(200));
    assert_eq!(render.rows[2], render_two.rows[2]);
    assert_eq!(
        render_two.rows[3].trim_end(),
        "▎   notes",
        "the next space on the very next row: {:#?}",
        render_two.rows
    );
}

#[test]
fn tree_spaces_keep_one_blank_row_between_them_when_row_gap_is_one() {
    let mut config = ClientShellConfig::from_config(&Config::default());
    config.spaces.row_gap = 1;
    let mut two = two_single_agent_spaces();
    two.workspaces[0].focused = false;
    let render = render_tree(&two, &config, tall(200));
    assert_eq!(
        [render.rows[3].trim_end(), render.rows[4].trim_end()],
        ["", "▎   notes"],
        "one blank row and no line, then the next space: {:#?}",
        render.rows
    );
    assert_eq!(
        render.hits.workspaces[1].rect.y, 4,
        "a click on the next space lands on its row after the gap"
    );
    assert_eq!(
        render.rows[7].trim_end(),
        "",
        "nothing drawn past the last space's gap row: {:#?}",
        render.rows
    );
}

#[test]
fn tree_spaces_are_open_unless_folded_by_hand() {
    let config = ClientShellConfig::from_config(&Config::default());
    let open = render_tree(&tree_snapshot(), &config, tall(200));
    assert_eq!(
        open.rows[5..7],
        ["▎ ! dotworks ▾", "▎ ╰╴✓ claude"],
        "the space you are not in is open too"
    );
    let folded = render_tree(
        &tree_snapshot(),
        &config,
        TreeRenderInput {
            folded: &["ws_1"],
            ..tall(200)
        },
    );
    assert_eq!(
        folded.rows[2..5],
        ["▎ ⠹ herdr ▸", "▎ ! dotworks ▾", "▎ ╰╴✓ claude"]
    );
}

#[test]
fn tree_reveal_of_a_pane_in_a_folded_space_shows_the_space_row() {
    let mut snapshot = tree_snapshot();
    for workspace in &mut snapshot.workspaces {
        workspace.focused = workspace.workspace_id == "ws_2";
    }
    for pane in &mut snapshot.panes {
        pane.focused = pane.pane_id == "pane_5";
    }
    // Without the box, so these six rows are all the tree's.
    let mut config = ClientShellConfig::from_config(&Config::default());
    config.sidebar_tree_panel = false;
    let render = render_tree(
        &snapshot,
        &config,
        TreeRenderInput {
            height: 6,
            spinner_ms: 0,
            scroll: 0,
            reveal_focused: true,
            folded: &["ws_2"],
            working_for: &[],
            done_for: &[],
            waiting_for: &[],
            width: 26,
            hovered: None,
            hovered_row: None,
            dragged: None,
            home: None,
            filter: TreeFilter::All,
            board: Default::default(),
        },
    );
    let visible = render.rows[2..5]
        .iter()
        .map(|row| row.trim_end_matches('▕').trim_end())
        .collect::<Vec<_>>();
    assert_eq!(visible.last(), Some(&"▎ ! dotworks ▸"), "{visible:?}");
    assert!(render
        .targets
        .iter()
        .all(|(_, target)| !matches!(target, TreeTarget::Pane(id) if id == "pane_5")));
}

#[test]
fn tree_fold_arrow_toggles_and_the_rest_of_the_row_still_focuses_the_main_lead() {
    let mut state = tree_state(true);
    state.compose(106, 30).expect("tree frame");
    let arrow = state.hits.workspaces[0]
        .group_toggle
        .clone()
        .expect("space rows carry a fold arrow")
        .0;
    let click_arrow = |state: &mut ClientShellState| {
        let at = |kind| {
            RawInputEvent::Mouse(crossterm::event::MouseEvent {
                kind,
                column: arrow.x,
                row: arrow.y,
                modifiers: KeyModifiers::empty(),
            })
        };
        let mut outcome =
            state.handle_raw_events(vec![at(MouseEventKind::Down(MouseButton::Left))]);
        outcome.actions.extend(
            state
                .handle_raw_events(vec![at(MouseEventKind::Up(MouseButton::Left))])
                .actions,
        );
        outcome
    };
    let outcome = click_arrow(&mut state);
    assert!(
        outcome.actions.is_empty(),
        "the arrow sends nothing to the server"
    );
    assert!(state.collapsed_groups.contains(&tree_fold_key("ws_1")));
    let text = frame_text(&state.compose(106, 30).expect("folded frame"));
    assert!(text.contains(" ⠋ herdr ▸"), "{text}");
    assert!(!text.contains("├╴  main ▾"), "{text}");

    click_arrow(&mut state);
    assert!(!state.collapsed_groups.contains(&tree_fold_key("ws_1")));
    state.compose(106, 30).expect("unfolded frame");

    let space = state.hits.workspaces[1].rect;
    assert!(matches!(
        sent_method(&click(&mut state, space)),
        // Its main lead, the agent in its first tab's first pane.
        crate::api::schema::Method::PaneFocus(target) if target.pane_id == "pane_4"
    ));
}

#[test]
fn tree_nested_rows_spin_the_main_sets_partner_by_default_in_either_icon_mode() {
    // The lead and its worker both working, frame 2; braille's partner is dot orbit.
    let mut projected = lead_space(&[("herdr-lead", None), ("w1", Some("pane_1"))]);
    for agent in &mut projected.agents {
        agent.agent_status = AgentStatus::Working;
    }
    let nerd = render_tree(&projected, &nerd_config(), tall(200));
    assert_eq!(
        nerd.rows[2..4],
        ["▎ ⠹ herdr \u{f47c}", "▎ ╰╴⠄ w1"],
        "the lead keeps braille; its worker dot orbit's third frame"
    );
    let plain = ClientShellConfig::from_config(&Config::default());
    let plain = render_tree(&projected, &plain, tall(200));
    assert_eq!(
        plain.rows[2..4],
        ["▎ ⠹ herdr ▾", "▎ ╰╴⠄ w1"],
        "plain icons: the same pair"
    );
    // Dot orbit has eight frames; frame 7 is its last.
    let later = render_tree(&projected, &nerd_config(), tall(700));
    assert_eq!(later.rows[3], "▎ ╰╴⠈ w1");
}

#[test]
fn tree_a_worker_drawn_flat_keeps_the_lead_spinner_in_nerd_mode() {
    // w1 names a lead pane that is gone; w2 names a pane in another space. Both are drawn at
    // the top level, not under a lead, so neither spins the nested Nerd spinner.
    let mut projected = lead_space(&[
        ("herdr-lead", None),
        ("w1", Some("pane_9")),
        ("w2", Some("pane_x")),
    ]);
    let mut other = tree_workspace("ws_2", 2, "acme", AgentStatus::Idle);
    other.focused = false;
    projected.workspaces.push(other);
    let mut far = tree_pane("pane_x", "ws_2", "tab_x");
    far.focused = false;
    projected
        .tabs
        .push(tree_tab("tab_x", "ws_2", 1, Some("far")));
    projected.panes.push(far);
    for agent in &mut projected.agents {
        agent.agent_status = AgentStatus::Working;
    }
    let render = render_tree(&projected, &nerd_config(), tall(200));
    for name in ["w1", "w2"] {
        let y = render
            .rows
            .iter()
            .position(|row| row.ends_with(name))
            .unwrap_or_else(|| panic!("{name} drawn: {:#?}", render.rows));
        assert_eq!(
            render.buffer[(4, y as u16)].symbol(),
            "⠹",
            "{name} is not nested: {:?}",
            render.rows[y]
        );
    }
}

#[test]
fn tree_working_spinner_setting_drives_the_lead_and_nested_rows_follow_it_in_plain() {
    use crate::config::WorkingSpinnerStyle;
    let mut projected = lead_space(&[("herdr-lead", None), ("w1", Some("pane_1"))]);
    for agent in &mut projected.agents {
        agent.agent_status = AgentStatus::Working;
    }
    let marks = |style, nerd: bool| {
        let mut config = if nerd {
            nerd_config()
        } else {
            ClientShellConfig::from_config(&Config::default())
        };
        config.working_spinner = style;
        let render = render_tree(&projected, &config, tall(200));
        let cell = |x: u16, y: u16| render.buffer[(x, y)].symbol().to_owned();
        (cell(2, 2), cell(4, 3))
    };
    assert_eq!(
        marks(WorkingSpinnerStyle::HalfCircles, false),
        ("◓".into(), "⠹".into()),
        "plain: the chosen set on the lead; the worker's Nerd spinner falls back to braille"
    );
    assert_eq!(
        marks(WorkingSpinnerStyle::Arc, true),
        ("◝".into(), "⠄".into()),
        "nerd: the chosen set on the lead, its partner dot orbit on its worker"
    );
    assert_eq!(
        marks(WorkingSpinnerStyle::CircleSlices, true),
        ("\u{f0aa0}".into(), "\u{ee08}".into()),
        "a Nerd Font set with nerd icons; its partner the Nerd spinner"
    );
    assert_eq!(
        marks(WorkingSpinnerStyle::CircleSlices, false),
        ("⠹".into(), "⠄".into()),
        "a Nerd Font set with plain icons falls back to braille; its partner would too, so \
         the worker spins braille's partner"
    );
    assert_eq!(
        marks(WorkingSpinnerStyle::MoonPhases, true),
        ("\u{e391}".into(), "◝".into()),
        "moon phases pairs with arc"
    );
    assert_eq!(
        marks(WorkingSpinnerStyle::MoonPhases, false),
        ("⠹".into(), "◝".into()),
        "plain icons: moon phases falls back to braille, its plain partner arc stays"
    );
    // 200 ms in: a 100 ms set's third frame, half circles' second.
    for (style, at_200ms) in [
        (WorkingSpinnerStyle::Braille, "⠹"),
        (WorkingSpinnerStyle::BrailleChase, "⠊"),
        (WorkingSpinnerStyle::DotOrbit, "⠄"),
        (WorkingSpinnerStyle::NerdSpinner, "\u{ee08}"),
        (WorkingSpinnerStyle::CircleSlices, "\u{f0aa0}"),
        (WorkingSpinnerStyle::HalfCircles, "◓"),
    ] {
        assert_eq!(marks(style, true).0, at_200ms, "{style:?}");
    }
}

#[test]
fn tree_sub_agent_spinner_pick_drives_nested_rows_and_falls_back_to_braille_in_plain() {
    use crate::config::{SubAgentSpinnerStyle, WorkingSpinnerStyle};
    let mut projected = lead_space(&[("herdr-lead", None), ("w1", Some("pane_1"))]);
    for agent in &mut projected.agents {
        agent.agent_status = AgentStatus::Working;
    }
    let marks = |main, sub, nerd: bool| {
        let mut config = if nerd {
            nerd_config()
        } else {
            ClientShellConfig::from_config(&Config::default())
        };
        config.working_spinner = main;
        config.sub_agent_spinner = sub;
        let render = render_tree(&projected, &config, tall(200));
        let cell = |x: u16, y: u16| render.buffer[(x, y)].symbol().to_owned();
        (cell(2, 2), cell(4, 3))
    };
    assert_eq!(
        marks(
            WorkingSpinnerStyle::HalfCircles,
            SubAgentSpinnerStyle::Arc,
            true
        ),
        ("◓".into(), "◝".into()),
        "each row spins its own set"
    );
    assert_eq!(
        marks(
            WorkingSpinnerStyle::MoonPhases,
            SubAgentSpinnerStyle::CircleSlices,
            true
        ),
        ("\u{e391}".into(), "\u{f0aa0}".into()),
        "Nerd Font sets on both with nerd icons"
    );
    assert_eq!(
        marks(
            WorkingSpinnerStyle::HalfCircles,
            SubAgentSpinnerStyle::CircleSlices,
            false
        ),
        ("◓".into(), "⠹".into()),
        "a Nerd Font sub-agent set with plain icons falls back to braille"
    );
    assert_eq!(
        marks(
            WorkingSpinnerStyle::Arc,
            SubAgentSpinnerStyle::BrailleChase,
            false
        ),
        ("◝".into(), "⠊".into()),
        "plain sets keep their own frames with plain icons"
    );
    // Auto follows the main pick through its partner, then the plain-icons fallback.
    for (main, nerd, worker) in [
        (WorkingSpinnerStyle::HalfCircles, true, "\u{ee08}"),
        (WorkingSpinnerStyle::HalfCircles, false, "⠹"),
        (WorkingSpinnerStyle::Arc, true, "⠄"),
        (WorkingSpinnerStyle::Line, false, "⠊"),
    ] {
        assert_eq!(
            marks(main, SubAgentSpinnerStyle::Auto, nerd).1,
            worker,
            "auto under {main:?}, nerd {nerd}"
        );
    }
}

#[test]
fn tree_auto_sub_agent_spinner_never_spins_the_main_sets_frames_with_plain_icons() {
    use crate::config::{SubAgentSpinnerStyle, WorkingSpinnerStyle};
    // Circle slices falls back to braille with plain icons, and so does its partner, the
    // Nerd spinner: auto then gives the worker braille's own plain partner.
    let every = [
        WorkingSpinnerStyle::Braille,
        WorkingSpinnerStyle::BrailleChase,
        WorkingSpinnerStyle::HalfCircles,
        WorkingSpinnerStyle::NerdSpinner,
        WorkingSpinnerStyle::CircleSlices,
        WorkingSpinnerStyle::Arc,
        WorkingSpinnerStyle::DotOrbit,
        WorkingSpinnerStyle::Pulse,
        WorkingSpinnerStyle::Line,
        WorkingSpinnerStyle::Star,
        WorkingSpinnerStyle::MoonPhases,
        WorkingSpinnerStyle::Diamond,
    ];
    for nerd in [false, true] {
        for main in every {
            let mut config = if nerd {
                nerd_config()
            } else {
                ClientShellConfig::from_config(&Config::default())
            };
            config.working_spinner = main;
            config.sub_agent_spinner = SubAgentSpinnerStyle::Auto;
            let (lead, worker) = (spinner_set(&config, false), spinner_set(&config, true));
            assert_ne!(lead, worker, "auto under {main:?}, nerd {nerd}");
            assert!(
                nerd || !worker.needs_nerd_font(),
                "plain icons draw a plain set: {main:?} gave {worker:?}"
            );
        }
    }
    let mut config = ClientShellConfig::from_config(&Config::default());
    config.working_spinner = WorkingSpinnerStyle::CircleSlices;
    assert_eq!(spinner_set(&config, true), WorkingSpinnerStyle::DotOrbit);
    // A set picked by name stays the user's pick, even when it matches.
    config.sub_agent_spinner = SubAgentSpinnerStyle::NerdSpinner;
    assert_eq!(spinner_set(&config, true), WorkingSpinnerStyle::Braille);
}

#[test]
fn tree_spinner_new_sets_spin_their_frames_and_moon_phases_needs_nerd_icons() {
    use crate::config::WorkingSpinnerStyle;
    let frames = |style, nerd: bool, count: usize| {
        let mut config = if nerd {
            nerd_config()
        } else {
            ClientShellConfig::from_config(&Config::default())
        };
        config.working_spinner = style;
        let pace = spinner_pace_ms(spinner_set(&config, false));
        (0..count as u64)
            .map(|frame| spinner_glyph(frame * pace, &config, false))
            .collect::<Vec<_>>()
            .concat()
    };
    for (style, want) in [
        (WorkingSpinnerStyle::Pulse, "·•●•·"),
        (WorkingSpinnerStyle::Line, "|/─\\|"),
        (WorkingSpinnerStyle::Star, "✶✸✹✺✹✷✶"),
        (WorkingSpinnerStyle::Diamond, "◇◈◆◈◇"),
    ] {
        let count = want.chars().count();
        assert_eq!(frames(style, false, count), want, "{style:?} plain");
        assert_eq!(frames(style, true, count), want, "{style:?} nerd");
        assert!(!style.needs_nerd_font(), "{style:?}");
    }
    // All 28 Weather Icons moon phases in order, then round again.
    let phases = (0xe38d..=0xe3a8)
        .chain([0xe38d])
        .filter_map(char::from_u32)
        .collect::<String>();
    assert_eq!(frames(WorkingSpinnerStyle::MoonPhases, true, 29), phases);
    assert!(WorkingSpinnerStyle::MoonPhases.needs_nerd_font());
    assert_eq!(
        frames(WorkingSpinnerStyle::MoonPhases, false, 3),
        "⠋⠙⠹",
        "plain icons fall back to braille"
    );
}

#[test]
fn tree_spinner_arc_and_dot_orbit_spin_plain_unicode_frames() {
    for (name, want) in [("arc", "◜◠◝◞◡◟◜"), ("dot_orbit", "⠁⠂⠄⡀⢀⠠⠐⠈⠁")]
    {
        let parsed: Config =
            toml::from_str(&format!("[ui]\nworking_spinner = \"{name}\"\n")).expect(name);
        assert!(!parsed.ui.working_spinner.needs_nerd_font(), "{name}");
        let config = ClientShellConfig::from_config(&parsed);
        let pace = spinner_pace_ms(parsed.ui.working_spinner);
        let spun = (0..want.chars().count() as u64)
            .map(|frame| spinner_glyph(frame * pace, &config, false))
            .collect::<String>();
        assert_eq!(spun, want, "{name}");
    }
}

#[test]
fn tree_spinner_glyph_comes_from_time_at_each_sets_own_pace() {
    use crate::config::WorkingSpinnerStyle;
    let config = |main| {
        let mut config = nerd_config();
        config.working_spinner = main;
        config
    };
    // Braille turns every 100 ms, whatever the redraw rate.
    let braille = config(WorkingSpinnerStyle::Braille);
    assert_eq!(spinner_glyph(99, &braille, false), "⠋");
    assert_eq!(spinner_glyph(100, &braille, false), "⠙");
    assert_eq!(
        spinner_glyph(1_050, &braille, false),
        "⠋",
        "ten frames round"
    );
    // Half circles hold each frame 150 ms, moon phases 50 ms.
    let halves = config(WorkingSpinnerStyle::HalfCircles);
    assert_eq!(spinner_glyph(149, &halves, false), "◐");
    assert_eq!(spinner_glyph(150, &halves, false), "◓");
    let moon = config(WorkingSpinnerStyle::MoonPhases);
    assert_eq!(spinner_glyph(49, &moon, false), "\u{e38d}");
    assert_eq!(spinner_glyph(50, &moon, false), "\u{e38e}");
    // The lead and its worker each keep their own pace: at 50 ms moon phases has moved on
    // and its partner arc has not.
    assert_eq!(spinner_glyph(50, &moon, true), "◜");
    assert_eq!(spinner_glyph(100, &moon, true), "◠");
    // A set that falls back to braille for plain icons takes braille's pace too.
    let mut plain_moon = config(WorkingSpinnerStyle::MoonPhases);
    plain_moon.sidebar_tree_icons = crate::config::SidebarTreeIconsConfig::Plain;
    assert_eq!(spinner_glyph(50, &plain_moon, false), "⠋");
    assert_eq!(spinner_glyph(100, &plain_moon, false), "⠙");
}

#[test]
fn tree_spinner_marks_only_working_pane_rows_and_advances_with_the_frame() {
    let config = ClientShellConfig::from_config(&Config::default());
    assert_eq!(spinner_glyph(0, &config, false), "⠋");
    assert_eq!(spinner_glyph(100, &config, false), "⠙");
    assert_eq!(spinner_glyph(1_000, &config, false), "⠋");
    assert_eq!(tree_mark(AgentStatus::Working, 300, &config), "⠸");
    assert_eq!(tree_mark(AgentStatus::Idle, 300, &config), "○");

    let snapshot = tree_snapshot();
    let first = render_tree(&snapshot, &config, tall(0));
    let next = render_tree(&snapshot, &config, tall(100));
    let changed = first
        .rows
        .iter()
        .zip(&next.rows)
        .filter(|(left, right)| left != right)
        .map(|(left, right)| format!("{left} -> {right}"))
        .collect::<Vec<_>>();
    assert_eq!(
        changed,
        ["▎ ⠋ herdr ▾ -> ▎ ⠙ herdr ▾"],
        "only the main lead's mark on its space row"
    );
}

#[test]
fn tree_sidebar_hits_map_space_tab_and_pane_rows() {
    let snapshot = tree_snapshot();
    let mut config = ClientShellConfig::from_config(&Config::default());
    config.spaces.row_gap = 1;
    let render = render_tree(&snapshot, &config, tall(0));
    let spaces = render
        .hits
        .workspaces
        .iter()
        .map(|hit| (hit.rect.y, hit.workspace_id.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(spaces, [(2, "ws_1"), (6, "ws_2")]);
    let targets = render
        .targets
        .iter()
        .map(|(rect, target)| (rect.y, target.clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        targets,
        [
            (3, TreeTarget::Pane("pane_2".into())),
            (4, TreeTarget::Tab("tab_2".into())),
            (7, TreeTarget::Pane("pane_5".into())),
        ]
    );
    assert!(render
        .targets
        .iter()
        .all(|(rect, _)| rect.x == 0 && rect.width == 25 && rect.height == 1));
    assert_eq!(
        render.hits.workspace_body,
        Rect::new(0, 2, 25, 11),
        "the box takes six rows: its rule, ACTION REQUIRED and its entry, COMPLETED and its entry, a blank row"
    );
    assert_eq!(
        render.hits.sidebar_toggle,
        Rect::new(23, 19, 1, 1),
        "under the header's last icon"
    );
    assert_eq!(render.hits.new_workspace, Rect::default(), "no footer new");
    assert_eq!(render.hits.sidebar_divider, Rect::new(25, 0, 1, 20));
}

#[test]
fn tree_sidebar_scrolls_rows_that_overflow_and_reveals_focus() {
    let mut snapshot = tree_snapshot();
    for pane in &mut snapshot.panes {
        pane.focused = false;
    }
    snapshot.panes[4].focused = true;
    let mut config = ClientShellConfig::from_config(&Config::default());
    config.spaces.row_gap = 1;
    // Without the box, so these rows are all the tree's.
    config.sidebar_tree_panel = false;
    let short = |scroll, reveal_focused| TreeRenderInput {
        height: 8,
        spinner_ms: 0,
        scroll,
        reveal_focused,
        folded: &[],
        working_for: &[],
        done_for: &[],
        waiting_for: &[],
        width: 26,
        hovered: None,
        hovered_row: None,
        dragged: None,
        home: None,
        filter: TreeFilter::All,
        board: Default::default(),
    };

    let visible =
        |render: &TreeRender, y: usize| render.rows[y].trim_end_matches('▕').trim_end().to_owned();

    let top = render_tree(&snapshot, &config, short(0, false));
    let metrics = top
        .hits
        .workspace_scroll_metrics
        .expect("tree scroll metrics");
    assert_eq!(metrics.max_offset_from_bottom, 2);
    assert_eq!(top.hits.workspace_max_scroll, 2);
    assert_eq!(
        visible(&top, 1),
        "",
        "the header's own blank row, no gap row under it"
    );
    assert_eq!(visible(&top, 2), "▎ ⠋ herdr ▾");
    assert_eq!(top.hits.workspace_scrollbar, Rect::new(24, 2, 1, 5));
    assert_eq!(top.buffer[(24, 2)].symbol(), "▕");

    let scrolled = render_tree(&snapshot, &config, short(1, false));
    assert_eq!(scrolled.scroll, 1);
    assert_eq!(visible(&scrolled, 2), "▎ ├╴· repo");
    assert_eq!(
        scrolled.targets[0],
        (Rect::new(0, 2, 24, 1), TreeTarget::Pane("pane_2".into()))
    );
    assert_eq!(scrolled.hits.workspaces[0].rect.y, 5, "after the blank row");

    let clamped = render_tree(&snapshot, &config, short(99, false));
    assert_eq!(clamped.scroll, 2);

    let revealed = render_tree(&snapshot, &config, short(0, true));
    assert_eq!(revealed.scroll, 1);
    assert_eq!(visible(&revealed, 6), "▎ ╰╴✓ claude");
}

fn tree_config(tree: bool) -> ClientShellConfig {
    let mut config = Config::default();
    config.ui.sidebar_tree = tree;
    ClientShellConfig::from_config(&config)
}

fn tree_state(tree: bool) -> ClientShellState {
    let mut state = ClientShellState::new(tree_config(tree));
    state.set_snapshot(Box::new(tree_snapshot()));
    state.set_pane_surface(surface());
    state
}

fn frame_text(frame: &FrameData) -> String {
    frame_rows(frame).join("\n")
}

fn click(state: &mut ClientShellState, rect: Rect) -> ClientShellInput {
    let at = |kind| {
        RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind,
            column: rect.x + 2,
            row: rect.y,
            modifiers: KeyModifiers::empty(),
        })
    };
    let mut outcome = state.handle_raw_events(vec![at(MouseEventKind::Down(MouseButton::Left))]);
    let up = state.handle_raw_events(vec![at(MouseEventKind::Up(MouseButton::Left))]);
    outcome.actions.extend(up.actions);
    outcome
}

fn tree_row(state: &ClientShellState, target: &TreeTarget) -> Rect {
    state
        .hits
        .tree_rows
        .iter()
        .find(|(_, candidate)| candidate == target)
        .unwrap_or_else(|| panic!("{target:?} not drawn"))
        .0
}

fn sent_method(outcome: &ClientShellInput) -> &crate::api::schema::Method {
    let [ClientShellAction::Endpoint { request, .. }] = &outcome.actions[..] else {
        panic!(
            "expected one endpoint request, got {:?}",
            outcome.actions.len()
        );
    };
    &request.method
}

#[test]
fn tree_setting_off_keeps_the_spaces_and_agents_sections() {
    let mut state = tree_state(false);
    let frame = state.compose(106, 30).expect("sections frame");
    let text = frame_text(&frame);
    assert!(text.contains(" spaces"), "{text}");
    assert!(text.contains(" agents"), "{text}");
    assert!(!text.contains("├╴  main ▾"), "{text}");
    assert!(state.hits.tree_rows.is_empty());
}

#[test]
fn tree_setting_on_draws_the_tree_in_the_expanded_sidebar() {
    let mut state = tree_state(true);
    let frame = state.compose(106, 30).expect("tree frame");
    let text = frame_text(&frame);
    assert!(text.contains("▎ ├╴· repo"), "{text}");
    assert!(text.contains(" ⠋ herdr ▾"), "{text}");
    assert!(!text.contains(" agents"), "{text}");
    assert!(!text.contains("shell"), "{text}");
    assert_eq!(state.hits.tree_rows.len(), 3, "both spaces are open");
}

#[test]
fn tree_rows_focus_their_tab_pane_or_main_lead_on_click() {
    let mut state = tree_state(true);
    state.compose(106, 30).expect("tree frame");

    let tab = tree_row(&state, &TreeTarget::Tab("tab_2".into()));
    assert!(matches!(
        sent_method(&click(&mut state, tab)),
        crate::api::schema::Method::TabFocus(target) if target.tab_id == "tab_2"
    ));

    let pane = tree_row(&state, &TreeTarget::Pane("pane_5".into()));
    assert!(matches!(
        sent_method(&click(&mut state, pane)),
        crate::api::schema::Method::PaneFocus(target) if target.pane_id == "pane_5"
    ));

    let space = state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.workspace_id == "ws_2")
        .expect("space row")
        .rect;
    assert!(matches!(
        sent_method(&click(&mut state, space)),
        // Its main lead, the agent in its first tab's first pane.
        crate::api::schema::Method::PaneFocus(target) if target.pane_id == "pane_4"
    ));
}

/// Nadine's target look: lone shell tabs (named and unnamed), a lone agent tab
/// whose own name is hidden, and a split tab holding an agent and a shell.
fn branch_snapshot() -> ClientShellSnapshot {
    let mut projected = snapshot();
    let mut herdr = tree_workspace("ws_1", 1, "herdr", AgentStatus::Working);
    herdr.focused = true;
    projected.workspaces = vec![herdr];
    projected.tabs = vec![
        tree_tab("tab_1", "ws_1", 1, Some("main")),
        tree_tab("tab_2", "ws_1", 2, None),
        tree_tab("tab_3", "ws_1", 3, Some("agents")),
        tree_tab("tab_4", "ws_1", 4, Some("review")),
    ];
    projected.panes = vec![
        tree_pane("pane_1", "ws_1", "tab_1"),
        tree_pane("pane_2", "ws_1", "tab_2"),
        tree_pane("pane_3", "ws_1", "tab_3"),
        tree_pane("pane_4", "ws_1", "tab_4"),
        tree_pane("pane_5", "ws_1", "tab_4"),
    ];
    let mut claude = tree_agent(&projected.panes[2], "claude", AgentStatus::Working);
    claude.name = Some("herdr-w7".into());
    projected.agents = vec![
        claude,
        tree_agent(&projected.panes[3], "codex", AgentStatus::Blocked),
    ];
    projected.focused_workspace_id = Some("ws_1".into());
    projected.focused_tab_id = Some("tab_1".into());
    projected.focused_pane_id = Some("pane_1".into());
    projected
}

#[test]
fn tree_tabs_are_branches_lone_panes_fold_into_their_tab_row() {
    let snapshot = branch_snapshot();
    let config = ClientShellConfig::from_config(&Config::default());
    let render = render_tree(&snapshot, &config, tall(200));
    assert_eq!(
        render.rows[2..9],
        [
            "▎ · herdr ▾",
            "▎ ├╴· tab 2",
            "▎ ├╴⠹ herdr-w7",
            "▎ ╰╴! codex ▾",
            "▎     ╰╴· repo",
            "",
            ""
        ]
    );
    let text = render.rows.join("\n");
    assert!(!text.contains("shell"), "{text}");
    assert!(
        !text.contains("agents"),
        "lone agent tab hides its name: {text}"
    );

    let targets = render
        .targets
        .iter()
        .map(|(rect, target)| (rect.y, target.clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        targets,
        [
            (3, TreeTarget::Tab("tab_2".into())),
            (
                4,
                TreeTarget::AgentTab {
                    tab_id: "tab_3".into(),
                    pane_id: "pane_3".into(),
                },
            ),
            (
                5,
                TreeTarget::AgentTab {
                    tab_id: "tab_4".into(),
                    pane_id: "pane_4".into(),
                },
            ),
            (6, TreeTarget::Pane("pane_5".into())),
        ]
    );

    let palette = &config.palette;
    assert_eq!(
        render.buffer[(4, 4)].fg,
        working_colour(palette),
        "lone agent mark"
    );
    assert_eq!(
        render.buffer[(6, 4)].fg,
        palette.subtext0,
        "lone agent name"
    );
    assert_eq!(
        render.buffer[(10, 6)].fg,
        palette.subtext0,
        "split shell name"
    );
}

#[test]
fn tree_lone_agent_row_highlights_when_its_pane_is_focused() {
    let mut snapshot = branch_snapshot();
    for pane in &mut snapshot.panes {
        pane.focused = pane.pane_id == "pane_3";
    }
    let config = ClientShellConfig::from_config(&Config::default());
    let render = render_tree(&snapshot, &config, tall(0));
    let name = &render.buffer[(7, 4)];
    assert_eq!(name.fg, config.palette.text);
    assert!(name.modifier.contains(Modifier::BOLD));
}

#[test]
fn tree_branch_rows_focus_the_tab_or_the_pane_on_click() {
    let mut state = ClientShellState::new(tree_config(true));
    state.set_snapshot(Box::new(branch_snapshot()));
    state.set_pane_surface(surface());
    state.compose(106, 30).expect("branch frame");
    assert!(
        !state
            .hits
            .tree_rows
            .iter()
            .any(|(_, target)| *target == TreeTarget::Tab("tab_3".into())),
        "a lone agent row targets its pane, not its tab"
    );

    let row = tree_row(&state, &TreeTarget::Tab("tab_2".into()));
    assert!(matches!(
        sent_method(&click(&mut state, row)),
        crate::api::schema::Method::TabFocus(target) if target.tab_id == "tab_2"
    ));
    let lone_agent = TreeTarget::AgentTab {
        tab_id: "tab_3".into(),
        pane_id: "pane_3".into(),
    };
    // The split tab's one agent stands where its tab row was: a click focuses the agent.
    let split_agent = TreeTarget::AgentTab {
        tab_id: "tab_4".into(),
        pane_id: "pane_4".into(),
    };
    for (target, pane_id) in [
        (lone_agent, "pane_3"),
        (split_agent, "pane_4"),
        (TreeTarget::Pane("pane_5".into()), "pane_5"),
    ] {
        let row = tree_row(&state, &target);
        assert!(matches!(
            sent_method(&click(&mut state, row)),
            crate::api::schema::Method::PaneFocus(target) if target.pane_id == pane_id
        ));
    }
}

fn right_click(state: &mut ClientShellState, rect: Rect) -> Option<ClientContextMenuTarget> {
    state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Right),
        column: rect.x + 2,
        row: rect.y,
        modifiers: KeyModifiers::empty(),
    })]);
    match state.overlay.take() {
        Some(ClientShellOverlay::ContextMenu(menu)) => Some(menu.target),
        _ => None,
    }
}

#[test]
fn tree_panel_items_and_the_space_plus_open_their_menu_on_right_click() {
    let mut state = panel_state(panel_snapshot(), 45);
    hover_space(&mut state, "ws_1", 45);
    let (plus, _) = state
        .hits
        .tree_new_tab
        .iter()
        .find(|(_, target)| *target == TreeTarget::NewTab("ws_1".into()))
        .cloned()
        .expect("the current space's `+`");
    // `right_click` presses two columns right of the rect's start.
    let plus = Rect::new(plus.x.saturating_sub(2), plus.y, 1, 1);
    let target = right_click(&mut state, plus);
    assert!(
        matches!(
            target,
            Some(ClientContextMenuTarget::Workspace { ref workspace_id, .. }) if workspace_id == "ws_1"
        ),
        "space `+`: {target:?}"
    );
    let asking = panel_row(&state, &TreeTarget::Pane("pane_3".into()));
    let target = right_click(&mut state, asking);
    assert!(
        matches!(
            target,
            Some(ClientContextMenuTarget::Pane { ref pane_id, .. }) if pane_id == "pane_3"
        ),
        "ask: {target:?}"
    );
}

#[test]
fn tree_rows_open_the_rename_menu_of_the_tab_or_pane_they_stand_for_on_right_click() {
    let mut state = ClientShellState::new(tree_config(true));
    state.set_snapshot(Box::new(branch_snapshot()));
    state.set_pane_surface(surface());
    state.compose(106, 30).expect("branch frame");

    let space = state.hits.workspaces[0].rect;
    assert!(
        matches!(
            right_click(&mut state, space),
            Some(ClientContextMenuTarget::Pane {
                ref pane_id,
                row: Some(ClientRowMenu { space: Some(_) }),
                ..
            }) if pane_id == "pane_1"
        ),
        "control: the space row stands for its first shell and opens its row menu"
    );
    let lone_agent = TreeTarget::AgentTab {
        tab_id: "tab_3".into(),
        pane_id: "pane_3".into(),
    };
    let rect = tree_row(&state, &TreeTarget::Tab("tab_2".into()));
    let target = right_click(&mut state, rect);
    assert!(
        matches!(
            target,
            Some(ClientContextMenuTarget::Pane { ref pane_id, row: Some(_), .. }) if pane_id == "pane_2"
        ),
        "lone shell tab: its one pane's row menu: {target:?}"
    );
    // An agent's row in its tab's place shows the agent, not the tab's name: the pane
    // menu fits it, and its Rename pane shows on the row.
    let split_agent = TreeTarget::AgentTab {
        tab_id: "tab_4".into(),
        pane_id: "pane_4".into(),
    };
    for (row, pane) in [(lone_agent, "pane_3"), (split_agent, "pane_4")] {
        let rect = tree_row(&state, &row);
        let target = right_click(&mut state, rect);
        assert!(
            matches!(
                target,
                Some(ClientContextMenuTarget::Pane { ref pane_id, .. }) if pane_id == pane
            ),
            "{row:?}: {target:?}"
        );
    }
    let pane = tree_row(&state, &TreeTarget::Pane("pane_5".into()));
    let target = right_click(&mut state, pane);
    assert!(
        matches!(
            target,
            Some(ClientContextMenuTarget::Pane { ref pane_id, .. }) if pane_id == "pane_5"
        ),
        "split pane row: {target:?}"
    );
}

fn drag_second_space_onto_first(state: &mut ClientShellState) {
    state.compose(106, 30).expect("sidebar frame");
    let first = state.hits.workspaces[0].rect;
    let second = state.hits.workspaces[1].rect;
    let event = |kind, row| {
        RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind,
            column: second.x + 2,
            row,
            modifiers: KeyModifiers::empty(),
        })
    };
    state.handle_raw_events(vec![event(
        MouseEventKind::Down(MouseButton::Left),
        second.y,
    )]);
    state.handle_raw_events(vec![event(
        MouseEventKind::Drag(MouseButton::Left),
        first.y,
    )]);
}

/// The space a live drag carries, if a space is being dragged.
fn dragged_space(state: &ClientShellState) -> Option<&str> {
    match state.chrome_drag.as_ref() {
        Some(ClientChromeDrag::Workspace {
            source_workspace_id,
            ..
        }) => Some(source_workspace_id.as_str()),
        _ => None,
    }
}

/// A left press on `rect`'s row, then the mouse moved with the button held to `to_row`.
fn press_and_move(state: &mut ClientShellState, rect: Rect, to_row: u16) -> ClientShellInput {
    let event = |kind, row| {
        RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind,
            column: rect.x + 2,
            row,
            modifiers: KeyModifiers::empty(),
        })
    };
    let mut outcome =
        state.handle_raw_events(vec![event(MouseEventKind::Down(MouseButton::Left), rect.y)]);
    let moved =
        state.handle_raw_events(vec![event(MouseEventKind::Drag(MouseButton::Left), to_row)]);
    outcome.actions.extend(moved.actions);
    outcome
}

#[test]
fn tree_space_rows_start_a_drag() {
    let mut sections = tree_state(false);
    drag_second_space_onto_first(&mut sections);
    assert!(
        sections.chrome_drag.is_some(),
        "control: sections sidebar drags"
    );

    // Press on space row `dotworks` and move one row: the space is dragged.
    let mut tree = tree_state(true);
    tree.compose(106, 30).expect("tree frame");
    let dotworks = tree.hits.workspaces[1].rect;
    press_and_move(&mut tree, dotworks, dotworks.y - 1);
    assert_eq!(dragged_space(&tree), Some("ws_2"));
}

#[test]
fn tree_tab_and_agent_rows_do_not_drag_a_space() {
    let mut state = tree_state(true);
    state.compose(106, 30).expect("tree frame");
    let rows = state.hits.tree_rows.clone();
    assert!(
        rows.iter()
            .any(|(_, target)| matches!(target, TreeTarget::Tab(_)))
            && rows
                .iter()
                .any(|(_, target)| !matches!(target, TreeTarget::Tab(_))),
        "control: tab rows and agent rows drawn: {rows:?}"
    );
    for (rect, target) in rows {
        let mut state = tree_state(true);
        state.compose(106, 30).expect("tree frame");
        press_and_move(&mut state, rect, rect.y + 1);
        assert_eq!(dragged_space(&state), None, "{target:?}");
    }
}

#[test]
fn tree_linked_worktree_space_stays_fixed() {
    let mut projected = tree_snapshot();
    projected.workspaces[1].worktree = Some(ClientShellWorktree {
        key: "repo".into(),
        label: "repo".into(),
        is_linked_worktree: true,
    });
    let mut state = ClientShellState::new(tree_config(true));
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.compose(106, 30).expect("tree frame");
    let linked = state.hits.workspaces[1].rect;
    press_and_move(&mut state, linked, linked.y - 1);
    assert_eq!(dragged_space(&state), None);
}

/// Spaces A (open: its row, tab `logs` and a claude agent tab), B (folded) and C (open: its
/// row and tab `notes`), each space row standing for a shell.
fn three_spaces_state() -> ClientShellState {
    let mut projected = tree_snapshot();
    let mut alpha = tree_workspace("ws_1", 1, "alpha", AgentStatus::Working);
    alpha.focused = true;
    projected.workspaces = vec![
        alpha,
        tree_workspace("ws_2", 2, "beta", AgentStatus::Idle),
        tree_workspace("ws_3", 3, "gamma", AgentStatus::Idle),
    ];
    projected.tabs.clear();
    projected.panes.clear();
    for (workspace_id, id, label) in [
        ("ws_1", 1, "main"),
        ("ws_1", 2, "logs"),
        ("ws_1", 3, "agent"),
        ("ws_2", 4, "main"),
        ("ws_2", 5, "docs"),
        ("ws_3", 6, "main"),
        ("ws_3", 7, "notes"),
    ] {
        push_shell_tab(&mut projected, workspace_id, id, label);
    }
    projected.agents = vec![tree_agent(
        &projected.panes[2],
        "claude",
        AgentStatus::Working,
    )];
    projected.focused_tab_id = Some("tab_1".into());
    projected.focused_pane_id = Some("pane_1".into());
    let mut state = ClientShellState::new(tree_config(true));
    state.collapsed_groups.insert(tree_fold_key("ws_2"));
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.compose(106, 30).expect("tree frame");
    state
}

fn space_rect(state: &ClientShellState, workspace_id: &str) -> Rect {
    state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.workspace_id == workspace_id)
        .unwrap_or_else(|| panic!("{workspace_id} not drawn"))
        .rect
}

/// The last tree row drawn for space `workspace_id`, its tabs and agents included.
fn space_last_row(state: &ClientShellState, workspace_id: &str) -> u16 {
    let snapshot = state.snapshot.as_deref().expect("snapshot");
    let belongs = |target: &TreeTarget| match target {
        TreeTarget::Tab(tab_id) | TreeTarget::AgentTab { tab_id, .. } => snapshot
            .tabs
            .iter()
            .any(|tab| tab.tab_id == *tab_id && tab.workspace_id == workspace_id),
        TreeTarget::Pane(pane_id) => snapshot
            .panes
            .iter()
            .any(|pane| pane.pane_id == *pane_id && pane.workspace_id == workspace_id),
        TreeTarget::NewTab(_) => false,
    };
    state
        .hits
        .tree_rows
        .iter()
        .filter(|(_, target)| belongs(target))
        .map(|(rect, _)| rect.bottom() - 1)
        .chain(std::iter::once(
            space_rect(state, workspace_id).bottom() - 1,
        ))
        .max()
        .expect("a row")
}

/// Drags space `workspace_id` by its row to `to` and lets go there; returns the drop row the
/// drag showed last and the space move the release sent, if any.
fn drop_space(
    state: &mut ClientShellState,
    workspace_id: &str,
    to: (u16, u16),
) -> (Option<u16>, Option<crate::api::schema::Method>) {
    let from = space_rect(state, workspace_id);
    let event = |kind, (column, row)| {
        RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::empty(),
        })
    };
    state.handle_raw_events(vec![event(
        MouseEventKind::Down(MouseButton::Left),
        (from.x + 2, from.y),
    )]);
    state.handle_raw_events(vec![event(MouseEventKind::Drag(MouseButton::Left), to)]);
    let row = match state.chrome_drag.as_ref() {
        Some(ClientChromeDrag::Workspace { target, .. }) => target.as_ref().map(|(_, row)| *row),
        _ => None,
    };
    let up = state.handle_raw_events(vec![event(MouseEventKind::Up(MouseButton::Left), to)]);
    let moved = up.actions.iter().find_map(|action| match action {
        ClientShellAction::Endpoint { request, .. } => matches!(
            request.method,
            crate::api::schema::Method::WorkspaceMove(_)
                | crate::api::schema::Method::WorkspaceMoveBlock(_)
        )
        .then(|| request.method.clone()),
        _ => None,
    });
    (row, moved)
}

fn moved_to(method: Option<crate::api::schema::Method>) -> Option<(String, usize)> {
    match method? {
        crate::api::schema::Method::WorkspaceMove(params) => {
            Some((params.workspace_id, params.insert_index))
        }
        other => panic!("unexpected move {other:?}"),
    }
}

#[test]
fn tree_space_drops_between_whole_spaces() {
    let state = three_spaces_state();
    let (a, b, c) = (
        space_rect(&state, "ws_1"),
        space_rect(&state, "ws_2"),
        space_rect(&state, "ws_3"),
    );
    // The layout the cases rest on: A is three rows, B one, C two.
    assert_eq!(space_last_row(&state, "ws_1"), a.y + 2);
    assert_eq!((b.y, b.height), (a.y + 3, 1));
    assert_eq!(space_last_row(&state, "ws_3"), c.y + 1);
    let column = a.x + 2;

    // C onto the A/B boundary: C moves before B (A, C, B).
    let mut state = three_spaces_state();
    assert_eq!(
        drop_space(&mut state, "ws_3", (column, b.y - 1)),
        (
            Some(b.y - 1),
            Some(crate::api::schema::Method::WorkspaceMove(
                crate::api::schema::WorkspaceMoveParams {
                    workspace_id: "ws_3".into(),
                    insert_index: 1,
                }
            ))
        )
    );

    // C over A's `logs` tab row snaps to the nearest boundary, the same A/B one.
    let mut state = three_spaces_state();
    let (row, method) = drop_space(&mut state, "ws_3", (column, a.y + 1));
    assert_eq!(
        (row, moved_to(method)),
        (Some(b.y - 1), Some(("ws_3".into(), 1)))
    );

    // A below C's last row: A goes last (B, C, A).
    let mut state = three_spaces_state();
    let end = space_last_row(&state, "ws_3") + 1;
    let (row, method) = drop_space(&mut state, "ws_1", (column, end));
    assert_eq!(
        (row, moved_to(method)),
        (Some(end), Some(("ws_1".into(), 3)))
    );

    // B onto its own slot: the order stays, so nothing is sent.
    let mut state = three_spaces_state();
    let (row, method) = drop_space(&mut state, "ws_2", (column, b.y - 1));
    assert_eq!((row, method), (Some(b.y - 1), None));

    // Outside the tree body, in the panes: no drop.
    let mut state = three_spaces_state();
    let body = state.hits.workspace_body;
    let (row, method) = drop_space(&mut state, "ws_3", (body.right() + 10, b.y - 1));
    assert_eq!((row, method), (None, None));
}

#[test]
fn tree_space_drop_has_no_last_slot_while_the_list_end_is_hidden() {
    let mut state = three_spaces_state();
    state.compose(106, 7).expect("short tree frame");
    let metrics = state.hits.workspace_scroll_metrics.expect("tree metrics");
    assert!(metrics.offset_from_bottom > 0, "control: C is cut off");
    let body = state.hits.workspace_body;
    let b = space_rect(&state, "ws_2");
    assert_eq!(b.y, body.bottom() - 1, "control: B is the body's last row");
    // On B's row, the nearest slot is the A/B boundary, A's own place: none shows below B
    // yet, so A cannot go last.
    let (row, method) = drop_space(&mut state, "ws_1", (body.x + 2, b.y));
    assert_eq!((row, method), (Some(b.y - 1), None));
}

/// Presses space `workspace_id` and holds it over `to` without letting go.
fn hold_space_over(state: &mut ClientShellState, workspace_id: &str, to: u16) {
    let from = space_rect(state, workspace_id);
    press_and_move(state, from, to);
    assert_eq!(
        dragged_space(state),
        Some(workspace_id),
        "control: dragging"
    );
}

/// Rows of `frame` whose cells across `body` are all the drop line.
fn drop_line_rows(frame: &FrameData, body: Rect) -> Vec<u16> {
    (0..frame.height)
        .filter(|y| {
            (body.x..body.right()).all(|x| {
                frame.cells[usize::from(*y) * usize::from(frame.width) + usize::from(x)].symbol
                    == "─"
            })
        })
        .collect()
}

#[test]
fn tree_draws_the_drop_line_while_a_space_is_dragged() {
    let mut state = three_spaces_state();
    let frame = state.compose(106, 30).expect("tree frame");
    let body = state.hits.workspace_body;
    assert_eq!(drop_line_rows(&frame, body), Vec::<u16>::new(), "no drag");

    // C over the A/B boundary: that row reads `─` in accent across the body.
    let b = space_rect(&state, "ws_2");
    hold_space_over(&mut state, "ws_3", b.y - 1);
    let frame = state.compose(106, 30).expect("dragging frame");
    assert_eq!(drop_line_rows(&frame, body), vec![b.y - 1]);
    let palette = &state.config.palette;
    for x in body.x..body.right() {
        let cell = &frame.cells[usize::from(b.y - 1) * 106 + usize::from(x)];
        assert_eq!(
            cell.fg,
            crate::protocol::color_to_u32(palette.accent),
            "column {x}"
        );
    }
    // The dragged space row takes the old sidebar's drag fill.
    let c = space_rect(&state, "ws_3");
    assert_eq!(
        frame.cells[usize::from(c.y) * 106 + usize::from(c.x) + 2].bg,
        crate::protocol::color_to_u32(palette.surface1)
    );
}

#[test]
fn tree_drop_line_spares_the_header_and_the_box() {
    // The top slot sits on the header's blank row, never on its word.
    let mut state = three_spaces_state();
    let header = frame_rows(&state.compose(106, 30).expect("tree frame"))[0].clone();
    let a = space_rect(&state, "ws_1");
    hold_space_over(&mut state, "ws_3", a.y - 1);
    let frame = state.compose(106, 30).expect("dragging frame");
    let body = state.hits.workspace_body;
    assert_eq!(drop_line_rows(&frame, body), vec![a.y - 1]);
    assert_eq!(frame_rows(&frame)[0], header);

    // Below the last space, with the ACTION REQUIRED box under the body and no room left:
    // the line takes the body's last row and the box stays whole.
    let mut state = tree_state(true);
    let still = frame_rows(&state.compose(106, 12).expect("short tree frame"));
    let body = state.hits.workspace_body;
    assert!(
        still[usize::from(body.bottom())..]
            .iter()
            .any(|row| row.contains("ACTION REQUIRED")),
        "control: the box shows under the body"
    );
    let dotworks = space_rect(&state, "ws_2");
    state.workspace_scroll = state.hits.workspace_max_scroll;
    state.compose(106, 12).expect("scrolled tree frame");
    hold_space_over(&mut state, "ws_1", body.bottom() - 1);
    let frame = state.compose(106, 12).expect("dragging frame");
    assert_eq!(drop_line_rows(&frame, body), vec![body.bottom() - 1]);
    let frame = frame_rows(&frame);
    assert_eq!(
        frame[usize::from(body.bottom())..],
        still[usize::from(body.bottom())..]
    );
    assert!(dotworks.y < body.bottom(), "control: {dotworks:?}");
}

#[test]
fn tree_space_drop_skips_a_linked_worktree_boundary() {
    // B is A's linked worktree: nothing can go just before it, so no slot shows there.
    let mut state = three_spaces_state();
    let mut projected = state.snapshot.as_deref().expect("snapshot").clone();
    projected.workspaces[0].worktree = Some(ClientShellWorktree {
        key: "repo".into(),
        label: "repo".into(),
        is_linked_worktree: false,
    });
    projected.workspaces[1].worktree = Some(ClientShellWorktree {
        key: "repo".into(),
        label: "repo".into(),
        is_linked_worktree: true,
    });
    state.set_snapshot(Box::new(projected));
    state.compose(106, 30).expect("tree frame");
    let (a, b) = (space_rect(&state, "ws_1"), space_rect(&state, "ws_2"));
    let (row, method) = drop_space(&mut state, "ws_3", (a.x + 2, b.y - 1));
    assert_ne!(row, Some(b.y - 1));
    assert_eq!(method, None);
}

#[test]
fn tree_space_row_release_sends_nothing_after_the_rows_move() {
    let mut state = tree_state(true);
    state.compose(106, 30).expect("tree frame");
    let dotworks = state.hits.workspaces[1].rect;
    let at = |kind| {
        RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind,
            column: dotworks.x + 2,
            row: dotworks.y,
            modifiers: KeyModifiers::empty(),
        })
    };
    let press = state.handle_raw_events(vec![at(MouseEventKind::Down(MouseButton::Left))]);
    assert_eq!(
        press.actions.len(),
        1,
        "control: the press focuses the pane"
    );
    // A frame between press and release moves another row under the press point.
    state.hits.tree_space_agents.clear();
    state.hits.tree_space_shells.clear();
    let release = state.handle_raw_events(vec![at(MouseEventKind::Up(MouseButton::Left))]);
    assert_eq!(release.actions.len(), 0);
}

#[test]
fn tree_space_hits_and_their_pane_hits_cover_the_same_rows() {
    // The release skips the space's own focus by `space_row_pane`, trusting that a press
    // anywhere on a space's hit focused that pane: the two hit lists must agree.
    for (projected, height, scroll) in [
        (tree_snapshot(), 30, 0),
        (git_snapshot(), 30, 0),
        (git_snapshot(), 6, 1),
    ] {
        let mut state = ClientShellState::new(tree_config(true));
        state.set_snapshot(Box::new(projected));
        state.set_pane_surface(surface());
        state.workspace_scroll = scroll;
        state.compose(106, height).expect("tree frame");
        let snapshot = state.snapshot.as_deref().expect("snapshot");
        let pane_hits = state
            .hits
            .tree_space_agents
            .iter()
            .chain(&state.hits.tree_space_shells)
            .collect::<Vec<_>>();
        assert!(!state.hits.workspaces.is_empty(), "control: spaces drawn");
        for hit in &state.hits.workspaces {
            let pane = super::super::tree_sidebar::space_row_pane(snapshot, &hit.workspace_id);
            assert_eq!(
                pane_hits
                    .iter()
                    .find(|(rect, _)| *rect == hit.rect)
                    .map(|(_, pane_id)| pane_id.as_str()),
                pane.map(|pane| pane.pane_id.as_str()),
                "{} at {:?}",
                hit.workspace_id,
                hit.rect
            );
        }
    }
}

#[test]
fn tree_space_row_click_still_focuses_its_pane_once() {
    let mut state = tree_state(true);
    state.compose(106, 30).expect("tree frame");
    let dotworks = state.hits.workspaces[1].rect;
    let outcome = click(&mut state, dotworks);
    assert!(
        matches!(
            sent_method(&outcome),
            crate::api::schema::Method::PaneFocus(target) if target.pane_id == "pane_4"
        ),
        "{:?}",
        sent_method(&outcome)
    );
}

#[test]
fn tree_wheel_scrolls_the_tree() {
    // 12 rows: the box takes four (its two rules, ACTION REQUIRED and codex), leaving five tree
    // rows, one short of the six the tree has with a `notes` tab.
    let mut projected = tree_snapshot();
    push_shell_tab(&mut projected, "ws_2", 6, "notes");
    let mut state = ClientShellState::new(tree_config(true));
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.compose(106, 12).expect("short tree frame");
    assert_eq!(
        state
            .hits
            .tree_rows
            .first()
            .map(|(_, target)| target.clone()),
        Some(TreeTarget::Pane("pane_2".into()))
    );
    let body = state.hits.workspace_body;
    assert_eq!(state.hits.workspaces[0].rect.y, body.y);
    state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: body.x + 1,
        row: body.y,
        modifiers: KeyModifiers::empty(),
    })]);
    state.compose(106, 12).expect("scrolled tree frame");
    assert_eq!(
        state
            .hits
            .tree_rows
            .first()
            .map(|(rect, target)| (rect.y, target.clone())),
        Some((body.y, TreeTarget::Pane("pane_2".into())))
    );
    assert_eq!(state.hits.workspaces[0].workspace_id, "ws_2");
}

#[test]
fn tree_reveals_a_newly_focused_pane_in_the_same_space() {
    // pane_3 is the only pane of tab_2, so the tab row stands for it.
    let logs = TreeTarget::Tab("tab_2".into());
    let mut state = tree_state(true);
    state.compose(106, 5).expect("short tree frame");
    assert!(!state
        .hits
        .tree_rows
        .iter()
        .any(|(_, target)| *target == logs));
    let mut moved = tree_snapshot();
    moved.revision = 2;
    for pane in &mut moved.panes {
        pane.focused = pane.pane_id == "pane_3";
    }
    moved.focused_pane_id = Some("pane_3".into());
    state.set_snapshot(Box::new(moved));
    let mut surface = surface();
    surface.projection_revision = 2;
    state.set_pane_surface(surface);
    state.compose(106, 5).expect("revealed tree frame");
    assert!(state
        .hits
        .tree_rows
        .iter()
        .any(|(_, target)| *target == logs));
    assert!(!state
        .hits
        .tree_rows
        .iter()
        .any(|(_, target)| *target == TreeTarget::Pane("pane_3".into())));
}

#[test]
fn tree_spinner_tick_repaints_only_while_a_working_agent_is_shown_in_tree_mode() {
    let start = std::time::Instant::now();
    let later = |ms| start + std::time::Duration::from_millis(ms);

    let mut state = tree_state(true);
    state.compose(106, 30).expect("tree frame");
    assert!(state.tick_sidebar_spinner(start), "the first tick draws");
    assert!(
        !state.tick_sidebar_spinner(later(50)),
        "waits for the interval"
    );
    assert!(state.tick_sidebar_spinner(later(100)));
    let text = frame_text(&state.compose(106, 30).expect("next frame"));
    assert!(text.contains("▎ ⠙ herdr ▾"), "{text}");

    state.sidebar_collapsed = true;
    state.compose(106, 30).expect("collapsed frame");
    assert!(state.tick_sidebar_spinner(later(200)), "the rail spins too");

    let mut off = tree_state(false);
    off.compose(106, 30).expect("sections frame");
    assert!(!off.tick_sidebar_spinner(start), "setting off");

    let mut idle = ClientShellState::new(tree_config(true));
    let mut snapshot = tree_snapshot();
    for agent in &mut snapshot.agents {
        agent.agent_status = AgentStatus::Idle;
    }
    idle.set_snapshot(Box::new(snapshot));
    idle.set_pane_surface(surface());
    idle.compose(106, 30).expect("idle frame");
    assert!(!idle.tick_sidebar_spinner(start), "nothing working");

    let mut narrow = tree_state(true);
    narrow.compose(40, 30).expect("mobile frame");
    assert!(
        !narrow.tick_sidebar_spinner(start),
        "mobile layout has no sidebar"
    );
}

#[test]
fn tree_spinner_tick_lands_on_the_frame_its_time_says_and_waits_for_the_next_one() {
    let start = std::time::Instant::now();
    let later = |ms| start + std::time::Duration::from_millis(ms);
    let ms = std::time::Duration::from_millis;

    let mut state = tree_state(true);
    state.compose(106, 30).expect("tree frame");
    assert!(state.tick_sidebar_spinner(start), "the first tick draws");
    assert_eq!(
        state.timer_delay(start),
        ms(100),
        "braille and its partner dot orbit both turn every 100 ms"
    );
    // A late tick is not a slow spinner: 350 ms in is braille's fourth frame.
    assert!(state.tick_sidebar_spinner(later(350)));
    let text = frame_text(&state.compose(106, 30).expect("late frame"));
    assert!(text.contains("▎ ⠸ herdr ▾"), "{text}");
    assert_eq!(
        state.timer_delay(later(350)),
        ms(50),
        "the next turn, at 400 ms"
    );
    assert!(
        !state.tick_sidebar_spinner(later(399)),
        "no repaint before a frame changes"
    );
    assert!(state.tick_sidebar_spinner(later(400)));

    // Moon phases turns every 50 ms, so the redraw does too.
    let mut moon = tree_state(true);
    moon.config.working_spinner = crate::config::WorkingSpinnerStyle::MoonPhases;
    moon.config.sidebar_tree_icons = crate::config::SidebarTreeIconsConfig::Nerd;
    moon.compose(106, 30).expect("moon frame");
    assert!(moon.tick_sidebar_spinner(start));
    assert_eq!(moon.timer_delay(start), ms(50));
    assert!(moon.tick_sidebar_spinner(later(50)));

    // Nothing working: the timer keeps its idle pace.
    let mut idle = ClientShellState::new(tree_config(true));
    let mut snapshot = tree_snapshot();
    for agent in &mut snapshot.agents {
        agent.agent_status = AgentStatus::Idle;
    }
    idle.set_snapshot(Box::new(snapshot));
    idle.set_pane_surface(surface());
    idle.compose(106, 30).expect("idle frame");
    assert!(!idle.tick_sidebar_spinner(start));
    assert_eq!(idle.timer_delay(start), ms(100));
}

#[test]
fn tree_spinner_timer_goes_back_to_its_idle_pace_when_the_last_agent_stops_working() {
    let start = std::time::Instant::now();
    let later = |ms| start + std::time::Duration::from_millis(ms);
    let mut state = tree_state(true);
    state.compose(106, 30).expect("tree frame");
    assert!(state.tick_sidebar_spinner(start));
    assert_eq!(
        state.timer_delay(start),
        std::time::Duration::from_millis(100)
    );

    let mut idle = tree_snapshot();
    idle.revision = 2;
    for agent in &mut idle.agents {
        agent.agent_status = AgentStatus::Idle;
    }
    state.set_snapshot(Box::new(idle));
    let mut surface = surface();
    surface.projection_revision = 2;
    state.set_pane_surface(surface);
    state.compose(106, 30).expect("idle frame");
    assert!(
        !state.tick_sidebar_spinner(later(150)),
        "nothing left to spin"
    );
    // A spinner deadline left behind would sit in the past and make every wait zero.
    assert_eq!(
        state.timer_delay(later(1_000)),
        std::time::Duration::from_millis(100),
        "the timer is back to its idle pace"
    );
}

#[test]
fn tree_mode_collapsed_rail_spins_only_working_marks() {
    let mut state = tree_state(true);
    state.sidebar_collapsed = true;
    state.compose(106, 30).expect("first collapsed frame");
    let start = std::time::Instant::now();
    assert!(state.tick_sidebar_spinner(start));
    assert!(state.tick_sidebar_spinner(start + std::time::Duration::from_millis(100)));
    let frame = state.compose(106, 30).expect("collapsed tree frame");
    let buffer = frame.to_ratatui_buffer().expect("frame should reconstruct");
    // A space line starts with its mark; an agent line's mark follows its connector.
    let mark = |x: u16, y: u16| buffer[(x, y)].symbol().to_owned();
    let spaces = state
        .hits
        .workspaces
        .iter()
        .map(|hit| (hit.workspace_id.clone(), mark(hit.rect.x, hit.rect.y)))
        .collect::<Vec<_>>();
    assert_eq!(
        spaces,
        [
            ("ws_1".to_owned(), "⠙".to_owned()),
            ("ws_2".to_owned(), "?".to_owned())
        ]
    );
    // The main leads (pane_1, pane_4) are their spaces' lines.
    let agents = state
        .hits
        .agents
        .iter()
        .map(|(rect, pane_id)| (pane_id.clone(), mark(rect.x + 1, rect.y)))
        .collect::<Vec<_>>();
    assert_eq!(agents, [("pane_5".to_owned(), "○".to_owned())]);

    let mut off = tree_state(false);
    off.sidebar_collapsed = true;
    off.compose(106, 30)
        .expect("first collapsed sections frame");
    assert!(!off.tick_sidebar_spinner(std::time::Instant::now()));
    let frame = off.compose(106, 30).expect("collapsed sections frame");
    let buffer = frame.to_ratatui_buffer().expect("frame should reconstruct");
    let working = off.hits.workspaces[0].rect;
    assert_eq!(buffer[(working.x + 2, working.y)].symbol(), "●");
}

/// The rail's sample: `herdr` (main lead, a blocked and a working worker), `dotworks`
/// (a main lead that asks), `notes` (a done main lead and an idle worker, folded).
fn rail_snapshot() -> ClientShellSnapshot {
    let mut projected = snapshot();
    let mut herdr = tree_workspace("ws_1", 1, "herdr", AgentStatus::Blocked);
    herdr.focused = true;
    projected.workspaces = vec![
        herdr,
        tree_workspace("ws_2", 2, "dotworks", AgentStatus::Idle),
        tree_workspace("ws_3", 3, "notes", AgentStatus::Done),
    ];
    projected.tabs = Vec::new();
    projected.panes = Vec::new();
    projected.agents = Vec::new();
    // (space, name, state, tokens)
    type RailAgent = (
        &'static str,
        &'static str,
        AgentStatus,
        &'static [(&'static str, &'static str)],
    );
    let agents: [RailAgent; 6] = [
        ("ws_1", "herdr", AgentStatus::Idle, &[]),
        (
            "ws_1",
            "herdr-w1",
            AgentStatus::Blocked,
            &[("lead", "pane_1")],
        ),
        (
            "ws_1",
            "herdr-w2",
            AgentStatus::Working,
            &[("lead", "pane_1")],
        ),
        (
            "ws_2",
            "dotworks",
            AgentStatus::Idle,
            &[("ask", "merge now?")],
        ),
        ("ws_3", "notes", AgentStatus::Done, &[]),
        ("ws_3", "notes-w1", AgentStatus::Idle, &[("lead", "pane_5")]),
    ];
    for (index, (workspace_id, name, status, tokens)) in agents.into_iter().enumerate() {
        let tab_id = format!("tab_{}", index + 1);
        projected
            .tabs
            .push(tree_tab(&tab_id, workspace_id, index + 1, Some(name)));
        let pane = tree_pane(&format!("pane_{}", index + 1), workspace_id, &tab_id);
        let mut agent = tree_agent(&pane, "claude", status);
        agent.name = Some(name.into());
        agent.tokens = tokens
            .iter()
            .map(|(key, value)| ((*key).into(), (*value).into()))
            .collect();
        projected.panes.push(pane);
        projected.agents.push(agent);
    }
    projected.focused_workspace_id = Some("ws_1".into());
    projected
}

/// The collapsed tree sidebar over `rail_snapshot`, `notes` folded.
fn rail_state() -> ClientShellState {
    let mut state = panel_state(rail_snapshot(), 12);
    state.sidebar_collapsed = true;
    state.collapsed_groups.insert(tree_fold_key("ws_3"));
    state
}

/// The rail's three columns left of its divider, per row, and the frame.
fn rail_rows(state: &mut ClientShellState, height: u16) -> (Vec<String>, FrameData) {
    let frame = state.compose(106, height).expect("collapsed tree frame");
    let rows = frame_rows(&frame)
        .iter()
        .map(|row| {
            row.chars()
                .take(3)
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect();
    (rows, frame.frame)
}

#[test]
fn the_rail_never_lists_more_lines_than_its_rows() {
    use crate::client::shell::tree_rail::rail_lines;
    let snapshot = rail_snapshot();
    let folds = HashSet::from([tree_fold_key("ws_3")]);
    for rows in 0..16 {
        let lines = rail_lines(&snapshot, &folds, rows);
        assert!(lines.len() <= rows, "{} lines in {rows} rows", lines.len());
    }
}

#[test]
fn tree_mode_collapsed_rail_lists_spaces_and_their_agents_in_tree_order() {
    let mut state = rail_state();
    let (rows, frame) = rail_rows(&mut state, 12);
    let working = format!(
        "╰{}",
        spinner_glyph(state.sidebar_spinner_ms, &state.config, true)
    );
    assert_eq!(
        rows[..7],
        ["○1", "├?", working.as_str(), "", "?2", "", "✓3"],
        "a space's lead's mark and its number, its other agents under it; a folded space is \
         one line: {rows:#?}"
    );
    let buffer = frame.to_ratatui_buffer().expect("frame should reconstruct");
    let palette = &state.config.palette;
    assert_eq!(buffer[(0, 0)].fg, palette.overlay0, "the idle lead's mark");
    assert_eq!(buffer[(1, 1)].fg, palette.yellow, "blocked mark");
    assert_eq!(buffer[(0, 4)].fg, palette.yellow, "a question");
    assert_eq!(buffer[(0, 6)].fg, palette.green, "done");
    let spaces = state
        .hits
        .workspaces
        .iter()
        .map(|hit| (hit.rect.y, hit.workspace_id.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(spaces, [(0, "ws_1"), (4, "ws_2"), (6, "ws_3")]);
    let agents = state
        .hits
        .agents
        .iter()
        .map(|(rect, pane_id)| (rect.y, pane_id.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(agents, [(1, "pane_2"), (2, "pane_3")]);
}

#[test]
fn tree_mode_collapsed_rail_counts_needs_you_and_done_above_its_toggle() {
    let mut state = rail_state();
    let (rows, frame) = rail_rows(&mut state, 12);
    assert_eq!(rows[8..], ["───", "?2", "✓1", " »"], "{rows:#?}");
    let buffer = frame.to_ratatui_buffer().expect("frame should reconstruct");
    let palette = &state.config.palette;
    assert_eq!(buffer[(0, 9)].fg, palette.yellow, "ACTION REQUIRED");
    assert_eq!(buffer[(0, 10)].fg, palette.green, "COMPLETED");
    // Taller than the rail: the list is cut, the rule, the counts and the toggle stay.
    let (rows, _) = rail_rows(&mut state, 7);
    let working = format!(
        "╰{}",
        spinner_glyph(state.sidebar_spinner_ms, &state.config, true)
    );
    assert_eq!(
        rows,
        ["○1", "├?", working.as_str(), "───", "?2", "✓1", " »"],
        "{rows:#?}"
    );
    assert_eq!(state.hits.workspaces.len(), 1, "only the lines drawn");
    // Cut among a space's agents: the last one drawn keeps `├`, more follow.
    let (rows, _) = rail_rows(&mut state, 6);
    assert_eq!(rows, ["○1", "├?", "───", "?2", "✓1", " »"], "{rows:#?}");
}

#[test]
fn the_rail_shows_each_leads_mark_then_its_number_its_workers_under_it_and_totals_under_a_rule() {
    let mut state = rail_state();
    let (rows, frame) = rail_rows(&mut state, 12);
    let sub = spinner_glyph(state.sidebar_spinner_ms, &state.config, true);
    let working = format!("╰{sub}");
    assert_eq!(
        rows,
        [
            "○1",
            "├?",
            working.as_str(),
            "",
            "?2",
            "",
            "✓3",
            "",
            "───",
            "?2",
            "✓1",
            " »"
        ],
        "each space its lead's mark and number, its workers under it, a blank line between \
         spaces, then a rule, the totals and the toggle: {rows:#?}"
    );
    let buffer = frame.to_ratatui_buffer().expect("frame should reconstruct");
    let palette = &state.config.palette;
    let cell = |x: u16, y: u16| &buffer[(x, y)];
    assert_eq!(cell(0, 0).fg, palette.overlay0, "the idle lead's dim mark");
    assert_eq!(cell(1, 0).fg, palette.text, "the active space's number");
    for x in 0..3 {
        assert_eq!(
            cell(x, 0).bg,
            crate::client::shell::sidebar::workspace_selection_background(palette),
            "the active space on the selection band across the rail"
        );
    }
    assert_eq!(cell(0, 1).fg, palette.overlay0, "a dim connector");
    assert_eq!(cell(1, 1).fg, palette.yellow, "a worker waiting on you");
    assert_eq!(cell(1, 2).fg, working_colour(palette), "a working worker");
    assert_eq!(cell(0, 4).fg, palette.yellow, "a lead that asks");
    assert_eq!(
        cell(1, 4).fg,
        palette.overlay0,
        "another space's number dim"
    );
    assert_eq!(cell(0, 6).fg, palette.green, "a done lead");
    assert_eq!(cell(0, 8).fg, palette.overlay0, "a dim rule");
    assert_eq!(
        (cell(0, 9).fg, cell(1, 9).fg),
        (palette.yellow, palette.overlay1)
    );
    assert_eq!(
        (cell(0, 10).fg, cell(1, 10).fg),
        (palette.green, palette.overlay1)
    );
    let spaces = state
        .hits
        .workspaces
        .iter()
        .map(|hit| (hit.rect.y, hit.workspace_id.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(spaces, [(0, "ws_1"), (4, "ws_2"), (6, "ws_3")]);
}

#[test]
fn tree_mode_collapsed_rail_has_no_counts_while_the_box_is_off() {
    let mut state = rail_state();
    state.config.sidebar_tree_panel = false;
    let (rows, _) = rail_rows(&mut state, 12);
    assert_eq!(
        rows[6..],
        ["✓3", "", "", "", "", " »"],
        "the counts are the box's; no box, no rule and no counts: {rows:#?}"
    );
}

#[test]
fn tree_rows_draw_no_control_characters_from_names() {
    const ESCAPES: &str = "\x1b]0;x\x07\x1b[31m";
    // A tab label (a lone shell tab after the first), an agent's name, and a shell pane's
    // label beside that agent in its split tab, each holding a title sequence and a colour.
    let mut projected = with_first_shell(lead_space(&[(ESCAPES, None)]));
    push_shell_tab(&mut projected, "ws_1", 8, "main");
    projected.tabs[2].label = format!("main{ESCAPES}");
    let mut shell = tree_pane("pane_9", "ws_1", "tab_1");
    shell.label = Some(format!("sh{ESCAPES}"));
    projected.panes.push(shell);
    projected.agents[0].name = Some(format!("lead{ESCAPES}"));
    let config = ClientShellConfig::from_config(&Config::default());
    let render = render_tree(&projected, &config, tall(0));
    for name in ["main", "lead", "sh"] {
        assert!(
            render.rows.iter().any(|row| row.contains(name)),
            "{name} drawn: {:#?}",
            render.rows
        );
    }
    let area = render.buffer.area;
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let symbol = render.buffer[(x, y)].symbol();
            assert!(
                !symbol.contains(char::is_control),
                "a control character at ({x}, {y}) {symbol:?}: {:#?}",
                render.rows
            );
        }
    }
}

fn tree_state_with_remote_machine() -> ClientShellState {
    let mut state = tree_state(true);
    let profile = crate::client::endpoint::SavedSshEndpoint {
        id: crate::client::endpoint::ProfileId::parse("0123456789abcdef0123456789abcdef")
            .expect("profile id"),
        label: "Build".into(),
        target: "dev@build.example".into(),
        session: "agents".into(),
        enabled: true,
    };
    let endpoint_id = ClientEndpointId::Ssh(profile.id.clone());
    state.set_endpoint_catalog(&[profile]);
    state.set_endpoint_status(&endpoint_id, ClientEndpointStatus::Online);
    let mut remote = snapshot();
    remote.boot_id = "remote-boot".into();
    state.set_endpoint_snapshot(&endpoint_id, Box::new(remote));
    state
}

/// The top row of the frame right of the sidebar, where a top tab bar draws.
fn top_row_past_sidebar(state: &mut ClientShellState) -> String {
    let frame = state.compose(106, 30).expect("frame");
    let sidebar = usize::from(state.layout(106, 30).sidebar.width);
    frame.cells[sidebar..usize::from(frame.width)]
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect()
}

#[test]
fn tree_mode_hides_the_tab_bar_and_panes_start_at_the_top() {
    let mut sections = tree_state(false);
    let top = top_row_past_sidebar(&mut sections);
    assert!(
        top.contains("logs"),
        "control: the tab bar lists tabs: {top}"
    );
    let layout = sections.layout(106, 30);
    assert_eq!((layout.tab_bar.height, layout.pane_surface.y), (1, 1));

    let mut tree = tree_state(true);
    let top = top_row_past_sidebar(&mut tree);
    assert!(!top.contains("logs"), "no tab bar row: {top}");
    let layout = tree.layout(106, 30);
    assert_eq!(layout.tab_bar.height, 0);
    assert_eq!(
        (layout.pane_surface.y, layout.pane_surface.height),
        (0, 30),
        "panes start at the top"
    );
    assert_eq!(
        tree.config.initial_surface_size(106, 30),
        tree.surface_size(106, 30),
        "the first surface size already leaves out the tab bar"
    );

    // The collapsed rail lists no tabs, so the tab bar comes back; the toggle resends the
    // surface size, which loses the tab bar's row.
    let mut toggle = ClientShellInput::default();
    tree.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::ToggleSidebar),
        &mut toggle,
    );
    assert!(tree.sidebar_collapsed);
    assert!(toggle.resize, "collapsing resends the surface size");
    assert_eq!(tree.surface_size(106, 30).rows, 29);
    // The server answers the resize with a fresh surface.
    tree.set_pane_surface(surface());
    let top = top_row_past_sidebar(&mut tree);
    assert!(
        top.contains("logs"),
        "collapsed tree sidebar keeps the tab bar: {top}"
    );
    let layout = tree.layout(106, 30);
    assert_eq!((layout.tab_bar.height, layout.pane_surface.y), (1, 1));

    let mut federated = tree_state_with_remote_machine();
    let top = top_row_past_sidebar(&mut federated);
    assert!(
        top.contains("logs"),
        "the machine sidebar keeps the tab bar: {top}"
    );
}

#[test]
fn tree_mode_falls_back_to_the_machine_sidebar_with_several_endpoints() {
    let mut state = tree_state_with_remote_machine();
    let text = frame_text(&state.compose(106, 30).expect("federated frame"));
    assert!(text.contains("Build"), "{text}");
    assert!(!text.contains("├╴  main ▾"), "{text}");
    assert!(state.hits.tree_rows.is_empty());
    assert!(!state.tick_sidebar_spinner(std::time::Instant::now()));
}

#[test]
fn tree_mode_collapsed_rail_with_several_machines_keeps_static_working_marks() {
    let mut state = tree_state_with_remote_machine();
    state.sidebar_collapsed = true;
    state
        .compose(106, 30)
        .expect("first collapsed federated frame");
    assert!(!state.tick_sidebar_spinner(std::time::Instant::now()));
    let frame = state.compose(106, 30).expect("collapsed federated frame");
    let buffer = frame.to_ratatui_buffer().expect("frame should reconstruct");

    // The machine rail ran: machine rows and machine agent rows, no one-machine agent rows.
    assert_eq!(state.hits.machines.len(), 2);
    assert!(state.hits.agents.is_empty());
    assert!(state.hits.tree_rows.is_empty());

    let snapshot_of = |endpoint_id: &ClientEndpointId| {
        if endpoint_id.is_local() {
            tree_snapshot()
        } else {
            snapshot()
        }
    };
    let working_mark = status_icon(AgentStatus::Working, state.config.status_indicators);
    let spinner = (0..10)
        .map(|frame| spinner_glyph(frame, &state.config, false))
        .collect::<Vec<_>>();
    let mut marks = state
        .hits
        .workspaces
        .iter()
        .map(|hit| {
            let status = snapshot_of(&hit.endpoint_id)
                .workspaces
                .iter()
                .find(|workspace| workspace.workspace_id == hit.workspace_id)
                .expect("drawn space")
                .agent_status;
            (status, buffer[(hit.rect.x + 2, hit.rect.y)].symbol())
        })
        .collect::<Vec<_>>();
    marks.extend(
        state
            .hits
            .endpoint_agents
            .iter()
            .map(|(rect, endpoint_id, pane_id)| {
                let status = snapshot_of(endpoint_id)
                    .agents
                    .iter()
                    .find(|agent| agent.pane_id == *pane_id)
                    .expect("drawn agent")
                    .agent_status;
                (status, buffer[(rect.x + 1, rect.y)].symbol())
            }),
    );
    assert!(
        marks.iter().all(|(_, mark)| !spinner.contains(mark)),
        "{marks:?}"
    );
    let working = marks
        .iter()
        .filter(|(status, _)| *status == AgentStatus::Working)
        .map(|(_, mark)| *mark)
        .collect::<Vec<_>>();
    assert!(working.len() >= 2, "a Working space and agent: {marks:?}");
    assert!(
        working.iter().all(|mark| *mark == working_mark),
        "{marks:?}"
    );
}

#[test]
fn hidden_tree_does_not_reveal_a_pane_focus_change() {
    let mut state = tree_state_with_remote_machine();
    state.compose(106, 30).expect("federated frame");
    assert!(!state.reveal_focused_workspace);
    let mut moved = tree_snapshot();
    moved.revision = 2;
    for pane in &mut moved.panes {
        pane.focused = pane.pane_id == "pane_2";
    }
    moved.focused_pane_id = Some("pane_2".into());
    state.set_snapshot(Box::new(moved));
    assert!(!state.reveal_focused_workspace);
}

/// Six one-tab, one-pane spaces; `focused` picks the focused space.
fn six_space_snapshot(focused: usize, revision: u64) -> ClientShellSnapshot {
    let mut projected = snapshot();
    projected.revision = revision;
    projected.workspaces = (1..=6)
        .map(|index| {
            let mut workspace = tree_workspace(
                &format!("ws_{index}"),
                index,
                &format!("space-{index}"),
                AgentStatus::Idle,
            );
            workspace.active_tab_id = format!("tab_{index}");
            workspace.focused = index == focused;
            workspace
        })
        .collect();
    projected.tabs = (1..=6)
        .map(|index| {
            let mut tab = tree_tab(&format!("tab_{index}"), &format!("ws_{index}"), 1, None);
            tab.focused = index == focused;
            tab
        })
        .collect();
    projected.panes = (1..=6)
        .map(|index| {
            let mut pane = tree_pane(
                &format!("pane_{index}"),
                &format!("ws_{index}"),
                &format!("tab_{index}"),
            );
            pane.focused = index == focused;
            pane
        })
        .collect();
    projected.agents = Vec::new();
    projected.focused_workspace_id = Some(format!("ws_{focused}"));
    projected.focused_tab_id = Some(format!("tab_{focused}"));
    projected.focused_pane_id = Some(format!("pane_{focused}"));
    projected
}

fn jump_and_follow(
    state: &mut ClientShellState,
    action: crate::input::KeybindAction,
    target: usize,
    revision: u64,
) {
    let mut outcome = ClientShellInput::default();
    state.record_binding(crate::input::KeybindMatch::Action(action), &mut outcome);
    let expected = format!("ws_{target}");
    assert!(
        matches!(
            sent_method(&outcome),
            crate::api::schema::Method::WorkspaceFocus(focus) if focus.workspace_id == expected
        ),
        "{action:?} should focus {expected}"
    );
    // The server answers the focus request with a snapshot that moves focus there.
    state.set_snapshot(Box::new(six_space_snapshot(target, revision)));
    let mut surface = surface();
    surface.projection_revision = revision;
    state.set_pane_surface(surface);
    state.compose(106, 8).expect("followed frame");
    assert!(
        state
            .hits
            .workspaces
            .iter()
            .any(|hit| hit.workspace_id == expected),
        "space row {expected} scrolled into view"
    );
    // The focused pane is its space's only shell, so the space row stands for it.
    let pane = format!("pane_{target}");
    assert!(
        state
            .hits
            .tree_space_shells
            .iter()
            .any(|(_, pane_id)| *pane_id == pane),
        "focused pane's space row of {expected} scrolled into view"
    );
}

#[test]
fn tree_scrolls_to_a_space_reached_by_workspace_keys() {
    let mut state = ClientShellState::new(tree_config(true));
    state.set_snapshot(Box::new(six_space_snapshot(1, 1)));
    state.set_pane_surface(surface());
    state.compose(106, 8).expect("short tree frame");
    assert!(!state
        .hits
        .workspaces
        .iter()
        .any(|hit| hit.workspace_id == "ws_6"));

    jump_and_follow(
        &mut state,
        crate::input::KeybindAction::SwitchWorkspace(5),
        6,
        2,
    );
    assert!(!state
        .hits
        .workspaces
        .iter()
        .any(|hit| hit.workspace_id == "ws_1"));
    jump_and_follow(&mut state, crate::input::KeybindAction::NextWorkspace, 1, 3);
    jump_and_follow(
        &mut state,
        crate::input::KeybindAction::PreviousWorkspace,
        6,
        4,
    );
}

/// One focused space `herdr` of lone agent tabs, `(agent name, lead token)` each, in
/// order: tab `tab_N` holds pane `pane_N`. Every agent is Idle.
fn lead_space(tabs: &[(&str, Option<&str>)]) -> ClientShellSnapshot {
    let mut projected = snapshot();
    let mut herdr = tree_workspace("ws_1", 1, "herdr", AgentStatus::Idle);
    herdr.focused = true;
    projected.workspaces = vec![herdr];
    projected.tabs = Vec::new();
    projected.panes = Vec::new();
    projected.agents = Vec::new();
    for (index, (name, lead)) in tabs.iter().enumerate() {
        let tab_id = format!("tab_{}", index + 1);
        projected
            .tabs
            .push(tree_tab(&tab_id, "ws_1", index + 1, Some(name)));
        let pane = tree_pane(&format!("pane_{}", index + 1), "ws_1", &tab_id);
        let mut agent = tree_agent(&pane, "claude", AgentStatus::Idle);
        agent.name = Some((*name).into());
        agent.tokens = lead
            .map(|lead| vec![("lead".into(), lead.into())])
            .unwrap_or_default();
        projected.panes.push(pane);
        projected.agents.push(agent);
    }
    projected.focused_workspace_id = Some("ws_1".into());
    projected.focused_tab_id = None;
    projected.focused_pane_id = None;
    projected
}

/// Appends a lone shell tab named `label` to space `workspace_id`.
fn push_shell_tab(projected: &mut ClientShellSnapshot, workspace_id: &str, id: usize, label: &str) {
    let tab_id = format!("tab_{id}");
    projected
        .tabs
        .push(tree_tab(&tab_id, workspace_id, id, Some(label)));
    projected
        .panes
        .push(tree_pane(&format!("pane_{id}"), workspace_id, &tab_id));
}

/// `projected` with a shell tab `main` first in space `ws_1`, so no agent is its main lead
/// and every lead keeps its own row; the space row stands for that shell.
fn with_first_shell(mut projected: ClientShellSnapshot) -> ClientShellSnapshot {
    projected
        .tabs
        .insert(0, tree_tab("tab_0", "ws_1", 0, Some("main")));
    projected
        .panes
        .insert(0, tree_pane("pane_0", "ws_1", "tab_0"));
    projected
}

fn space_rows(projected: &ClientShellSnapshot, count: usize) -> Vec<String> {
    let config = ClientShellConfig::from_config(&Config::default());
    render_tree(projected, &config, tall(0)).rows[2..2 + count].to_vec()
}

#[test]
fn tree_nests_a_tab_under_the_tab_of_its_lead_pane() {
    let mut projected = with_first_shell(lead_space(&[
        ("herdr-lead", None),
        ("herdr-w7", Some("pane_1")),
        ("herdr-w2", Some("pane_1")),
    ]));
    // Tab order puts logs before the workers; the workers still follow their lead.
    push_shell_tab(&mut projected, "ws_1", 4, "logs");
    let logs = projected.tabs.pop().expect("logs tab");
    projected.tabs.insert(2, logs);
    projected.panes[2].git_branch = Some("fix".into());
    let config = ClientShellConfig::from_config(&Config::default());
    let render = render_tree(&projected, &config, tall(0));
    assert_eq!(
        render.rows[2..9],
        [
            "▎ · herdr ▾",
            "▎ ├╴○ herdr-lead",
            "▎ │   ├╴○ herdr-w7",
            "▎ │   │   fix",
            "▎ │   ╰╴○ herdr-w2",
            "▎ ╰╴· logs",
            ""
        ]
    );
    let targets = render
        .targets
        .iter()
        .map(|(rect, target)| (rect.y, target.clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        targets[1],
        (
            4,
            TreeTarget::AgentTab {
                tab_id: "tab_2".into(),
                pane_id: "pane_2".into(),
            }
        ),
        "a nested tab keeps its own click target"
    );
    assert_eq!(
        render.buffer[(8, 4)].fg,
        idle_colour(&config.palette),
        "idle mark"
    );
}

/// A lead (no `lead` token, or other tabs nest under it) nested under `top-lead`, its
/// tab split with a shell so it folds; a worker with a long name under it; and a split
/// tab under `top-lead` whose second pane is a lead with no `lead` token.
fn long_lead_space() -> ClientShellSnapshot {
    let mut projected = with_first_shell(lead_space(&[
        ("top-lead", None),
        ("oakwood-ws-lead", Some("pane_1")),
        ("oakwood-worker-long", Some("pane_2")),
        ("w", Some("pane_1")),
    ]));
    // `oakwood-ws-lead`'s tab gets a shell, so its row has a fold arrow.
    projected.panes.push(tree_pane("pane_9", "ws_1", "tab_2"));
    // `w`'s tab gets a second agent: a lead's pane in a split tab.
    let pane = tree_pane("pane_8", "ws_1", "tab_4");
    let mut agent = tree_agent(&pane, "claude", AgentStatus::Idle);
    agent.name = Some("oakwood-ws-lead".into());
    projected.panes.push(pane);
    projected.agents.push(agent);
    projected
}

#[test]
fn tree_cuts_a_long_lead_name_in_the_middle_and_keeps_its_last_part() {
    let config = ClientShellConfig::from_config(&Config::default());
    for (width, want) in [
        (
            26,
            [
                "▎     ├╴○ oakwoo…-lead ▾",
                "▎     │   ╰╴○ oakwood-w…",
                "▎         ╰╴○ oakw…-lead",
            ],
        ),
        (
            30,
            [
                "▎     ├╴○ oakwood-ws-lead ▾",
                "▎     │   ╰╴○ oakwood-worke…",
                "▎         ╰╴○ oakwood-…-lead",
            ],
        ),
    ] {
        let input = TreeRenderInput { width, ..tall(0) };
        let render = render_tree(&long_lead_space(), &config, input);
        let got = render
            .rows
            .iter()
            .filter(|row| row.contains("○ oak"))
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(
            got, want,
            "a lead keeps `-lead`, the fold arrow stays; a worker is cut at its end, at \
             {width} columns: {:#?}",
            render.rows
        );
    }
}

#[test]
fn tree_nests_two_levels_with_a_guide_for_each_open_ancestor() {
    let mut projected = with_first_shell(lead_space(&[
        ("lead", None),
        ("a", Some("pane_1")),
        ("b", Some("pane_2")),
        ("c", Some("pane_1")),
    ]));
    push_shell_tab(&mut projected, "ws_1", 5, "logs");
    assert_eq!(
        space_rows(&projected, 7),
        [
            "▎ · herdr ▾",
            "▎ ├╴○ lead",
            "▎ │   ├╴○ a",
            "▎ │   │   ╰╴○ b",
            "▎ │   ╰╴○ c",
            "▎ ╰╴· logs",
            "",
        ]
    );
}

#[test]
fn tree_draws_nesting_deeper_than_three_levels_at_the_third() {
    let projected = with_first_shell(lead_space(&[
        ("lead", None),
        ("d1", Some("pane_1")),
        ("d2", Some("pane_2")),
        ("d3", Some("pane_3")),
        ("d4", Some("pane_4")),
    ]));
    assert_eq!(
        space_rows(&projected, 7),
        [
            "▎ · herdr ▾",
            "▎ ╰╴○ lead",
            "▎     ╰╴○ d1",
            "▎         ╰╴○ d2",
            "▎             ├╴○ d3",
            "▎             ╰╴○ d4",
            "",
        ]
    );
}

#[test]
fn tree_nested_split_tab_lists_its_panes_under_it() {
    let mut projected = with_first_shell(lead_space(&[("lead", None), ("review", Some("pane_1"))]));
    projected.panes.push(tree_pane("pane_3", "ws_1", "tab_2"));
    push_shell_tab(&mut projected, "ws_1", 4, "logs");
    projected.agents[1].name = Some("codex".into());
    assert_eq!(
        space_rows(&projected, 7),
        [
            "▎ · herdr ▾",
            "▎ ├╴○ lead",
            "▎ │   ╰╴○ codex ▾",
            "▎ │       ╰╴· repo",
            "▎ ╰╴· logs",
            "",
            ""
        ]
    );
}

#[test]
fn tree_ignores_a_lead_in_another_space() {
    let mut projected = with_first_shell(lead_space(&[
        ("lead", None),
        ("x", Some("pane_9")),
        ("child", Some("pane_1")),
    ]));
    projected
        .workspaces
        .push(tree_workspace("ws_2", 2, "dotworks", AgentStatus::Idle));
    push_shell_tab(&mut projected, "ws_2", 9, "other");
    assert_eq!(
        space_rows(&projected, 5),
        [
            "▎ · herdr ▾",
            "▎ ├╴○ lead",
            "▎ │   ╰╴○ child",
            "▎ ╰╴○ x",
            "▎ · dotworks",
        ]
    );
}

#[test]
fn tree_ignores_a_lead_pane_that_is_gone() {
    let projected = with_first_shell(lead_space(&[
        ("lead", None),
        ("x", Some("w9:p9")),
        ("child", Some("pane_1")),
    ]));
    assert_eq!(
        space_rows(&projected, 5),
        [
            "▎ · herdr ▾",
            "▎ ├╴○ lead",
            "▎ │   ╰╴○ child",
            "▎ ╰╴○ x",
            ""
        ]
    );
}

#[test]
fn tree_draws_tabs_whose_leads_form_a_cycle_flat() {
    // a and b lead each other, self leads itself; c follows a, which the cycle leaves a root.
    let projected = with_first_shell(lead_space(&[
        ("a", Some("pane_2")),
        ("b", Some("pane_1")),
        ("c", Some("pane_1")),
        ("self", Some("pane_4")),
    ]));
    assert_eq!(
        space_rows(&projected, 6),
        [
            "▎ · herdr ▾",
            "▎ ├╴○ a",
            "▎ │   ╰╴○ c",
            "▎ ├╴○ b",
            "▎ ╰╴○ self",
            ""
        ]
    );
}

/// One space with a lone agent tab per state, a lone shell tab, and a split of a working
/// agent and a shell.
fn states_snapshot() -> ClientShellSnapshot {
    let mut projected = lead_space(&[
        ("work", None),
        ("idle", None),
        ("done", None),
        ("stuck", None),
    ]);
    for (agent, status) in projected.agents.iter_mut().zip([
        AgentStatus::Working,
        AgentStatus::Idle,
        AgentStatus::Done,
        AgentStatus::Blocked,
    ]) {
        agent.agent_status = status;
    }
    push_shell_tab(&mut projected, "ws_1", 5, "logs");
    push_shell_tab(&mut projected, "ws_1", 6, "split");
    projected.panes.push(tree_pane("pane_7", "ws_1", "tab_6"));
    let mut claude = tree_agent(&projected.panes[5], "claude", AgentStatus::Working);
    claude.name = Some("pair".into());
    projected.agents.push(claude);
    projected
}

#[test]
fn tree_marks_each_agent_state_and_shells_left_of_the_name() {
    let projected = with_first_shell(states_snapshot());
    let config = ClientShellConfig::from_config(&Config::default());
    let render = render_tree(&projected, &config, tall(200));
    assert_eq!(
        render.rows[2..12],
        [
            "▎ · herdr ▾",
            "▎ ├╴⠹ work",
            "▎ ├╴○ idle",
            "▎ ├╴✓ done",
            "▎ ├╴! stuck",
            "▎ ├╴· logs",
            "▎ ╰╴⠹ pair ▾",
            "▎     ╰╴· repo",
            "",
            ""
        ],
        "a split tab's one agent stands in the tab's place, its shell under it"
    );
    let palette = &config.palette;
    let mark = |y: u16| &render.buffer[(4, y)];
    for (y, color, state) in [
        (3, working_colour(palette), "working"),
        (4, idle_colour(palette), "idle"),
        (5, done_colour(palette), "done"),
        (6, approval_colour(palette), "blocked"),
        (7, shell_mark_colour(palette), "shell"),
    ] {
        assert_eq!(mark(y).fg, color, "{state}");
    }
    assert_eq!(
        render.buffer[(8, 9)].fg,
        shell_mark_colour(palette),
        "split shell"
    );
    let next = render_tree(&projected, &config, tall(300));
    assert_eq!(next.rows[3], "▎ ├╴⠸ work", "the spinner advances");
}

#[test]
fn tree_marks_on_vesper_read_working_yellow_idle_mint_and_done_teal() {
    use ratatui::style::Color::Rgb;
    let projected = with_first_shell(states_snapshot());
    let mut theme = Config::default();
    theme.theme.name = Some("vesper".into());
    let config = ClientShellConfig::from_config(&theme);
    let render = render_tree(&projected, &config, tall(200));
    let mark = |y: u16| render.buffer[(4, y)].fg;
    assert_eq!(mark(3), Rgb(255, 199, 153), "working: yellow");
    assert_eq!(mark(4), Rgb(153, 255, 228), "idle: mint, not grey");
    assert_eq!(mark(5), Rgb(102, 221, 204), "done: teal");
    assert_eq!(mark(6), Rgb(255, 128, 128), "blocked: red, the yes/no mark");
}

#[test]
fn tree_marks_follow_the_status_indicator_style() {
    let marks = |style| {
        let mut config = Config::default();
        config.ui.status_indicators = style;
        let config = ClientShellConfig::from_config(&config);
        let render = render_tree(&tree_snapshot(), &config, tall(200));
        [2, 5, 6].map(|y| render.rows[y].clone())
    };
    assert_eq!(
        marks(crate::config::StatusIndicatorStyle::Dots),
        ["▎ ⠹ herdr ▾", "▎ ! dotworks ▾", "▎ ╰╴✓ claude"],
        "dots: needs you is `!`, done a dot; working keeps the spinner"
    );
    assert_eq!(
        marks(crate::config::StatusIndicatorStyle::Symbols),
        ["▎ ⠹ herdr ▾", "▎ ! dotworks ▾", "▎ ╰╴✓ claude"]
    );
}

/// `tree_snapshot` with nothing blocked, focus on `workspace_id`.
fn calm_snapshot(workspace_id: &str, revision: u64) -> ClientShellSnapshot {
    let mut projected = tree_snapshot();
    projected.revision = revision;
    for agent in &mut projected.agents {
        if agent.agent_status == AgentStatus::Blocked {
            agent.agent_status = AgentStatus::Idle;
        }
    }
    for workspace in &mut projected.workspaces {
        workspace.focused = workspace.workspace_id == workspace_id;
    }
    projected.focused_workspace_id = Some(workspace_id.into());
    projected
}

fn show(state: &mut ClientShellState, projected: ClientShellSnapshot) {
    let revision = projected.revision;
    state.set_snapshot(Box::new(projected));
    let mut surface = surface();
    surface.projection_revision = revision;
    state.set_pane_surface(surface);
    state.compose(106, 30).expect("tree frame");
}

fn click_arrow(state: &mut ClientShellState, workspace_id: &str) {
    let arrow = state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.workspace_id == workspace_id)
        .and_then(|hit| hit.group_toggle.clone())
        .expect("fold arrow")
        .0;
    let at = |kind| {
        RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind,
            column: arrow.x,
            row: arrow.y,
            modifiers: KeyModifiers::empty(),
        })
    };
    state.handle_raw_events(vec![at(MouseEventKind::Down(MouseButton::Left))]);
    state.handle_raw_events(vec![at(MouseEventKind::Up(MouseButton::Left))]);
    state.compose(106, 30).expect("toggled frame");
}

#[test]
fn tree_spaces_stay_as_folded_by_hand_across_focus_changes() {
    let mut state = ClientShellState::new(tree_config(true));
    show(&mut state, calm_snapshot("ws_1", 1));
    assert!(
        drawn_pane(&state, "pane_5"),
        "a space you are not in is open"
    );

    click_arrow(&mut state, "ws_1");
    assert!(state.collapsed_groups.contains(&tree_fold_key("ws_1")));
    show(&mut state, calm_snapshot("ws_2", 2));
    assert!(
        !drawn_tab(&state, "tab_1") && drawn_pane(&state, "pane_5"),
        "focus leaves herdr folded and dotworks open"
    );
    show(&mut state, calm_snapshot("ws_1", 3));
    assert!(
        !drawn_tab(&state, "tab_1"),
        "focus coming back does not open it"
    );
}

#[test]
fn tree_fold_arrow_toggles_the_space_fold_key_of_any_space() {
    let mut state = ClientShellState::new(tree_config(true));
    show(&mut state, calm_snapshot("ws_1", 1));
    click_arrow(&mut state, "ws_2");
    assert_eq!(
        state.collapsed_groups,
        HashSet::from([tree_fold_key("ws_2")])
    );
    assert!(!drawn_pane(&state, "pane_5"));
    click_arrow(&mut state, "ws_2");
    assert!(state.collapsed_groups.is_empty());
    assert!(drawn_pane(&state, "pane_5"));
}

fn fold_preferences_path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "herdr-shell-tree-fold-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    ))
}

fn saved_groups(path: &std::path::Path) -> HashSet<String> {
    let mut config = Config::default();
    config.ui.sidebar_tree = true;
    let config = ClientShellConfig::from_config(&config).with_preferences_path(path.to_owned());
    ClientShellState::new(config).collapsed_groups
}

#[test]
fn tree_blocked_space_unfolds_on_the_next_snapshot_and_forgets_its_fold() {
    let path = fold_preferences_path();
    let mut config = Config::default();
    config.ui.sidebar_tree = true;
    let config = ClientShellConfig::from_config(&config).with_preferences_path(path.clone());
    let mut state = ClientShellState::new(config);
    show(&mut state, calm_snapshot("ws_1", 1));
    click_arrow(&mut state, "ws_2");
    assert!(saved_groups(&path).contains(&tree_fold_key("ws_2")));

    let mut blocked = tree_snapshot();
    blocked.revision = 2;
    state.set_snapshot(Box::new(blocked));
    assert!(
        !state.collapsed_groups.contains(&tree_fold_key("ws_2")),
        "the snapshot, not a render, drops the fold"
    );
    assert!(saved_groups(&path).is_empty(), "and saves that");
    let mut surface = surface();
    surface.projection_revision = 2;
    state.set_pane_surface(surface);
    state.compose(106, 30).expect("blocked frame");
    assert!(drawn_pane(&state, "pane_5"));
    std::fs::remove_file(path).expect("remove tree fold preferences");
}

#[test]
fn tree_blocked_unfold_shows_a_failed_save_in_the_next_frame() {
    // A file where a folder should be: the save cannot create its file.
    let blocker = fold_preferences_path();
    std::fs::write(&blocker, "").expect("write blocker file");
    let mut config = Config::default();
    config.ui.sidebar_tree = true;
    let config = ClientShellConfig::from_config(&config)
        .with_preferences_path(blocker.join("preferences.json"));
    let mut state = ClientShellState::new(config);
    state.collapsed_groups.insert(tree_fold_key("ws_2"));
    show(&mut state, tree_snapshot());
    assert!(!state.collapsed_groups.contains(&tree_fold_key("ws_2")));
    let error = state
        .endpoint_error
        .clone()
        .expect("the failed save is reported");
    let text = frame_text(&state.compose(106, 30).expect("frame"));
    std::fs::remove_file(&blocker).expect("remove blocker file");
    let shown = error.lines().next().unwrap_or_default();
    assert!(
        text.contains(&shown[..shown.len().min(20)]),
        "{error:?} not in\n{text}"
    );
}

#[test]
fn tree_fold_by_hand_after_the_block_clears_sticks() {
    let mut state = ClientShellState::new(tree_config(true));
    show(&mut state, tree_snapshot());
    assert!(drawn_pane(&state, "pane_5"), "blocked dotworks is open");
    show(&mut state, calm_snapshot("ws_1", 2));
    assert!(
        drawn_pane(&state, "pane_5"),
        "stays open after the block clears"
    );
    click_arrow(&mut state, "ws_2");
    show(&mut state, calm_snapshot("ws_1", 3));
    assert!(state.collapsed_groups.contains(&tree_fold_key("ws_2")));
    assert!(!drawn_pane(&state, "pane_5"), "the hand fold sticks");
}

// Weight and names: bold leads, normal-weight workers, whole worker names, the dim `·` shell
// mark; then the box ignoring a check stage and the space row's `+`.

/// Moves the mouse over space `workspace_id`'s row, so its `+` shows, and draws again.
fn hover_space(state: &mut ClientShellState, workspace_id: &str, height: u16) {
    let row = state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.workspace_id == workspace_id)
        .expect("space row")
        .rect;
    mouse_at(state, MouseEventKind::Moved, row.x + 4, row.y);
    state.compose(106, height).expect("hover frame");
}

fn mouse_at(
    state: &mut ClientShellState,
    kind: MouseEventKind,
    column: u16,
    row: u16,
) -> ClientShellInput {
    state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::empty(),
    })])
}

#[test]
fn tree_lead_rows_are_bold_and_worker_rows_normal_weight() {
    let mut projected = with_first_shell(lead_space(&[
        ("herdr-lead", None),
        ("herdr-w7", Some("pane_1")),
        ("solo", None),
    ]));
    // A focused worker stays normal weight; the band marks its focus.
    projected.panes[2].focused = true;
    projected.agents[1].focused = true;
    projected.panes[3].focused = true;
    let config = ClientShellConfig::from_config(&Config::default());
    let render = render_tree(&projected, &config, tall(0));
    assert_eq!(
        render.rows[2..7],
        [
            "▎ · herdr ▾",
            "▎ ├╴○ herdr-lead",
            "▎ │   ╰╴○ herdr-w7",
            "▎ ╰╴○ solo",
            ""
        ]
    );
    let cell = |x: u16, y: u16| &render.buffer[(x, y)];
    assert!(cell(6, 3).modifier.contains(Modifier::BOLD), "lead bold");
    assert_eq!(cell(6, 3).fg, config.palette.subtext0, "no role color");
    assert!(
        !cell(10, 4).modifier.contains(Modifier::BOLD),
        "worker normal"
    );
    assert!(
        cell(6, 5).modifier.contains(Modifier::BOLD),
        "a focused row outside a team keeps its bold"
    );
}

#[test]
fn tree_workers_keep_their_full_names() {
    let mut projected = lead_space(&[
        ("herdr-lead", None),
        ("herdr-w7", Some("pane_1")),
        ("dotworks-w1", Some("pane_1")),
        ("herdr-", Some("pane_1")),
        ("boss", None),
        ("boss-w2", Some("pane_5")),
    ]);
    projected.agents[1].agent_status = AgentStatus::Done;
    let config = ClientShellConfig::from_config(&Config::default());
    let render = render_tree(&projected, &config, tall_panel(200));
    assert_eq!(
        render.rows[2..9],
        [
            "▎ ○ herdr ▾",
            "▎ ├╴✓ herdr-w7",
            "▎ ├╴○ dotworks-w1",
            "▎ ├╴○ herdr-",
            "▎ ╰╴○ boss",
            "▎     ╰╴○ boss-w2",
            "",
        ],
        "whole names: under the main lead, folded into the space row, and under any \
         other lead"
    );
    assert_eq!(
        panel(&render),
        ["  COMPLETED 1", "  ✓ herdr/herdr-w7"],
        "the space, then the worker's whole name"
    );
}

#[test]
fn tree_plain_terminal_tabs_use_a_dot_plain_and_a_terminal_glyph_nerd() {
    let mut config = ClientShellConfig::from_config(&Config::default());
    for (icons, mark) in [
        (crate::config::SidebarTreeIconsConfig::Plain, '·'),
        (crate::config::SidebarTreeIconsConfig::Nerd, '\u{e795}'),
    ] {
        config.sidebar_tree_icons = icons;
        let render = render_tree(&branch_snapshot(), &config, tall(200));
        let marked = format!("{mark} ");
        let shells = render
            .rows
            .iter()
            .filter(|row| row.contains(&marked))
            .count();
        assert!(shells >= 2, "{icons:?}: {:#?}", render.rows);
        assert!(
            !render.rows.iter().any(|row| row.contains("› ")),
            "{icons:?}: no chevron"
        );
        let (y, row) = render
            .rows
            .iter()
            .enumerate()
            .find(|(_, row)| row.contains(&marked))
            .expect("a shell row");
        let x = row.chars().position(|ch| ch == mark).expect("mark") as u16;
        assert_eq!(
            render.buffer[(x, y as u16)].fg,
            shell_mark_colour(&config.palette)
        );
    }
}

#[test]
fn tree_panel_ignores_a_check_stage() {
    let mut projected = agents_space(&[("i", AgentStatus::Idle)]);
    projected.agents[0].tokens = vec![("stage".into(), "check · try the run".into())];
    let config = ClientShellConfig::from_config(&Config::default());
    assert_no_panel(&render_tree(&projected, &config, tall(0)));
}

#[test]
fn tree_space_row_plus_shows_only_on_the_hovered_space() {
    let mut state = ClientShellState::new(tree_config(true));
    let mut projected = tree_snapshot();
    projected.workspaces[0].git_ahead_behind = Some((6, 0));
    show(&mut state, projected);
    let rows = |state: &mut ClientShellState| {
        let frame = state.compose(106, 30).expect("frame");
        frame_rows(&frame)
            .into_iter()
            .map(|row| {
                row.chars()
                    .take(25)
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect::<Vec<_>>()
    };
    let before = rows(&mut state);
    assert_eq!(
        before[2], "▎ ⠋ herdr ▾        ↑6",
        "the current space shows no `+` without the mouse"
    );
    let dotworks = before
        .iter()
        .position(|row| row.contains("dotworks"))
        .expect("dotworks row");
    assert_eq!(
        before[dotworks], "▎ ! dotworks ▾",
        "no `+` on another space"
    );

    let outcome = mouse_at(&mut state, MouseEventKind::Moved, 5, dotworks as u16);
    assert!(outcome.repaint, "hover repaints");
    let hovered = rows(&mut state);
    assert_eq!(hovered[dotworks], "▎ ! dotworks ▾        +");
    assert_eq!(hovered[2], "▎ ⠋ herdr ▾        ↑6");

    let outcome = mouse_at(&mut state, MouseEventKind::Moved, 60, 5);
    assert!(outcome.repaint, "leaving repaints");
    assert_eq!(rows(&mut state)[dotworks], "▎ ! dotworks ▾");

    mouse_at(&mut state, MouseEventKind::Moved, 5, 2);
    assert_eq!(
        rows(&mut state)[2],
        "▎ ⠋ herdr ▾        ↑6 +",
        "the current space shows it under the mouse"
    );
}

#[test]
fn the_tree_row_under_the_mouse_takes_the_focused_rows_grey_until_the_mouse_leaves() {
    let mut state = ClientShellState::new(tree_config(true));
    show(&mut state, tree_snapshot());
    let grey = crate::protocol::color_to_u32(super::super::tree_sidebar::focus_band(
        &state.config.palette,
        false,
    ));
    let frame = |state: &mut ClientShellState| state.compose(106, 30).expect("frame");
    let bg = |frame: &FrameData, y: u16| frame.cells[usize::from(y) * 106 + 12].bg;
    let first = frame(&mut state);
    let rows = frame_rows(&first);
    let row_of = |needle: &str| {
        rows.iter()
            .position(|row| row.chars().take(25).collect::<String>().contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} in {rows:#?}")) as u16
    };
    let (dotworks, logs) = (row_of("dotworks"), row_of("logs"));
    let panel = state
        .hits
        .tree_panel
        .first()
        .expect("a ACTION REQUIRED row")
        .0
        .y;
    for y in [dotworks, logs, panel] {
        assert_ne!(bg(&first, y), grey, "row {y} without the mouse");
    }
    // A space row, a tab row and a ACTION REQUIRED box row: each alone while the mouse is on it.
    for y in [dotworks, logs, panel] {
        let outcome = mouse_at(&mut state, MouseEventKind::Moved, 5, y);
        assert!(outcome.repaint, "row {y}: the hover moved");
        let hovered = frame(&mut state);
        assert_eq!(bg(&hovered, y), grey, "row {y} under the mouse");
        for other in [dotworks, logs, panel]
            .into_iter()
            .filter(|other| *other != y)
        {
            assert_eq!(
                bg(&hovered, other),
                bg(&first, other),
                "row {other} is not hovered"
            );
        }
        let again = mouse_at(&mut state, MouseEventKind::Moved, 7, y);
        assert!(!again.repaint, "row {y}: the same row asks no repaint");
    }
    // Off the sidebar the grey goes.
    let outcome = mouse_at(&mut state, MouseEventKind::Moved, 60, 5);
    assert!(outcome.repaint, "leaving repaints");
    assert_eq!(bg(&frame(&mut state), panel), bg(&first, panel));

    // The rail's rows take it too.
    let mut state = rail_state();
    let (rows, frame) = rail_rows(&mut state, 12);
    let grey = crate::protocol::color_to_u32(super::super::tree_sidebar::focus_band(
        &state.config.palette,
        false,
    ));
    let agent = state.hits.agents.first().expect("a rail agent row").0;
    let before = frame.cells[usize::from(agent.y) * 106 + 1].bg;
    assert_ne!(before, grey, "{rows:#?}");
    let outcome = mouse_at(&mut state, MouseEventKind::Moved, agent.x, agent.y);
    assert!(outcome.repaint);
    let (_, frame) = rail_rows(&mut state, 12);
    assert_eq!(frame.cells[usize::from(agent.y) * 106 + 1].bg, grey);
}

#[test]
fn a_blank_rail_row_and_a_box_heading_stay_ungreyed_under_the_mouse() {
    // The box's heading has no hit, so the mouse on it greys nothing; even a hover rect on
    // it (as a stale frame could leave) draws no grey.
    let mut state = ClientShellState::new(tree_config(true));
    show(&mut state, tree_snapshot());
    let frame = |state: &mut ClientShellState| state.compose(106, 30).expect("frame");
    let bg = |frame: &FrameData, y: u16| frame.cells[usize::from(y) * 106 + 12].bg;
    let first = frame(&mut state);
    let heading = state
        .hits
        .tree_panel
        .first()
        .expect("a ACTION REQUIRED row")
        .0
        .y
        - 1;
    assert!(
        frame_rows(&first)[usize::from(heading)].contains("ACTION REQUIRED"),
        "{:#?}",
        frame_rows(&first)
    );
    let outcome = mouse_at(&mut state, MouseEventKind::Moved, 5, heading);
    assert!(!outcome.repaint, "the heading is no row to hover");
    state.hovered_tree_row = Some(Rect::new(0, heading, 30, 1));
    assert_eq!(bg(&frame(&mut state), heading), bg(&first, heading));

    // A blank rail row, between two groups, likewise.
    let mut state = rail_state();
    let (rows, first) = rail_rows(&mut state, 12);
    let blank = (1..rows.len() - 1)
        .find(|&y| rows[y].is_empty() && !rows[y - 1].is_empty() && !rows[y + 1].is_empty())
        .unwrap_or_else(|| panic!("a blank row between groups in {rows:#?}"))
        as u16;
    let outcome = mouse_at(&mut state, MouseEventKind::Moved, 1, blank);
    assert!(!outcome.repaint, "a blank rail row is no row to hover");
    state.hovered_tree_row = Some(Rect::new(0, blank, 3, 1));
    let (_, hovered) = rail_rows(&mut state, 12);
    let cell = |frame: &FrameData| frame.cells[usize::from(blank) * 106 + 1].bg;
    assert_eq!(cell(&hovered), cell(&first), "{rows:#?}");
}

#[test]
fn tree_space_row_plus_opens_a_focused_plain_tab_without_asking() {
    let mut config = Config::default();
    config.ui.sidebar_tree = true;
    config.ui.prompt_new_tab_name = true;
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    show(&mut state, tree_snapshot());
    let dotworks = state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.workspace_id == "ws_2")
        .expect("dotworks row")
        .rect;
    mouse_at(
        &mut state,
        MouseEventKind::Moved,
        dotworks.x + 4,
        dotworks.y,
    );
    state.compose(106, 30).expect("hover frame");
    let plus = (dotworks.right() - 3, dotworks.y);
    let mut outcome = mouse_at(
        &mut state,
        MouseEventKind::Down(MouseButton::Left),
        plus.0,
        plus.1,
    );
    let up = mouse_at(
        &mut state,
        MouseEventKind::Up(MouseButton::Left),
        plus.0,
        plus.1,
    );
    outcome.actions.extend(up.actions);
    assert!(state.overlay.is_none(), "no name prompt");
    match sent_method(&outcome) {
        crate::api::schema::Method::TabCreate(params) => {
            assert_eq!(params.workspace_id.as_deref(), Some("ws_2"));
            assert!(params.focus);
            assert_eq!(params.label, None);
        }
        method => panic!("{method:?}"),
    }
}

// Round 16: menu audit (a pane's own name shows) and tab folding.

#[test]
fn tree_pane_rows_show_the_name_given_by_rename_pane() {
    let mut projected = branch_snapshot();
    // pane_3: the lone agent herdr-w7; pane_4: codex in the split; pane_5: a split shell.
    projected.panes[2].label = Some("scout".into());
    projected.panes[3].label = Some("checker".into());
    projected.panes[4].label = Some("logs".into());
    let config = ClientShellConfig::from_config(&Config::default());
    let rows = render_tree(&projected, &config, tall(200)).rows;
    assert_eq!(
        rows[4..7],
        ["▎ ├╴⠹ scout", "▎ ╰╴! checker ▾", "▎     ╰╴· logs"],
        "{rows:#?}"
    );
}

/// `branch_snapshot` with the split `review` tab (codex blocked, a shell) folded or not.
/// `branch_snapshot` with a second agent, `pi`, in the `review` split tab beside codex: a
/// tab with two agents keeps its own row.
fn two_agent_review() -> ClientShellSnapshot {
    let mut projected = branch_snapshot();
    let pi = tree_agent(&projected.panes[4], "pi", AgentStatus::Idle);
    projected.agents.push(pi);
    projected
}

fn fold_render_of(projected: ClientShellSnapshot, folded: &[&str]) -> Vec<String> {
    let mut state = ClientShellState::new(tree_config(true));
    for tab in folded {
        state.collapsed_groups.insert(tree_tab_fold_key(tab));
    }
    show(&mut state, projected);
    let frame = state.compose(106, 30).expect("frame");
    frame_rows(&frame)
        .into_iter()
        .map(|row| {
            row.chars()
                .take(25)
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect()
}

#[test]
fn tree_split_tabs_fold_like_spaces_and_show_their_state_folded() {
    let open = fold_render_of(two_agent_review(), &[]);
    assert_eq!(
        open[3..9],
        [
            "▎ ├╴· tab 2",
            "▎ ├╴⠋ herdr-w7",
            "▎ ╰╴  review ▾",
            "▎     ├╴! codex",
            "▎     ╰╴○ pi",
            ""
        ],
        "only the split tab gets an arrow"
    );
    let folded = fold_render_of(two_agent_review(), &["tab_4"]);
    assert_eq!(
        folded[5], "▎ ╰╴! review ▸",
        "folded: arrow, then the tab's state"
    );
    assert_eq!(
        folded[6], "",
        "no pane rows after the folded tab: {folded:#?}"
    );

    // A folded tab holding the focused pane gets the focused row's band.
    let mut projected = two_agent_review();
    for pane in &mut projected.panes {
        pane.focused = pane.pane_id == "pane_5";
    }
    for tab in &mut projected.tabs {
        tab.focused = tab.tab_id == "tab_4";
    }
    // The state the server gives the tab: codex, its blocked pane.
    projected.tabs[3].agent_status = AgentStatus::Blocked;
    projected.focused_tab_id = Some("tab_4".into());
    projected.focused_pane_id = Some("pane_5".into());
    let config = ClientShellConfig::from_config(&Config::default());
    let focus_band = config.palette.selection_bg;
    let open = render_tree(&projected, &config, tall(0));
    assert_eq!(open.rows[7], "▎     ╰╴○ pi");
    assert_eq!(open.buffer[(19, 7)].bg, focus_band, "control: the pane row");
    assert_ne!(
        open.buffer[(19, 5)].bg,
        focus_band,
        "the open tab row is not"
    );
    let folded = render_tree(
        &projected,
        &config,
        TreeRenderInput {
            folded: &["tab_4"],
            ..tall(0)
        },
    );
    assert_eq!(folded.rows[5], "▎ ╰╴! review ▸");
    assert_eq!(
        folded.buffer[(19, 5)].bg,
        focus_band,
        "the folded tab row is"
    );
}

#[test]
fn tree_tab_arrow_folds_by_hand_and_is_saved() {
    let path = fold_preferences_path();
    let mut config = Config::default();
    config.ui.sidebar_tree = true;
    let config = ClientShellConfig::from_config(&config).with_preferences_path(path.clone());
    let mut state = ClientShellState::new(config);
    show(&mut state, branch_snapshot());
    let codex = TreeTarget::AgentTab {
        tab_id: "tab_4".into(),
        pane_id: "pane_4".into(),
    };
    let review = tree_row(&state, &codex);
    // The tab's one agent stands in its place; the arrow follows its name: `    ╰╴! codex ▾`.
    let arrow = state
        .hits
        .tree_tab_folds
        .iter()
        .find(|(_, tab_id)| tab_id == "tab_4")
        .map(|(cell, _)| (cell.x, cell.y))
        .expect("the tab's arrow");
    assert_eq!(arrow, (review.x + 12, review.y));
    let outcome = mouse_at(
        &mut state,
        MouseEventKind::Down(MouseButton::Left),
        arrow.0,
        arrow.1,
    );
    assert!(
        outcome.actions.is_empty(),
        "the arrow folds; it focuses nothing"
    );
    assert!(state.collapsed_groups.contains(&tree_tab_fold_key("tab_4")));
    assert!(saved_groups(&path).contains(&tree_tab_fold_key("tab_4")));
    state.compose(106, 30).expect("folded frame");
    assert!(
        !state
            .hits
            .tree_rows
            .iter()
            .any(|(_, target)| *target == TreeTarget::Pane("pane_5".into())),
        "folded panes are gone from the tree"
    );
    // The folded row still focuses its agent.
    let review = tree_row(&state, &codex);
    assert!(matches!(
        sent_method(&click(&mut state, Rect::new(review.x + 6, review.y, 5, 1))),
        crate::api::schema::Method::PaneFocus(target) if target.pane_id == "pane_4"
    ));
    mouse_at(
        &mut state,
        MouseEventKind::Down(MouseButton::Left),
        arrow.0,
        arrow.1,
    );
    assert!(!state.collapsed_groups.contains(&tree_tab_fold_key("tab_4")));
    assert!(saved_groups(&path).is_empty());
    std::fs::remove_file(path).expect("remove tab fold preferences");
}

#[test]
fn tree_space_menu_collapse_folds_the_space_row() {
    let mut projected = tree_snapshot();
    // herdr (ws_1) holds a worktree group with dotworks (ws_2) as its linked checkout.
    projected.workspaces[0].worktree = Some(ClientShellWorktree {
        key: "repo".into(),
        label: "repo".into(),
        is_linked_worktree: false,
    });
    projected.workspaces[1].worktree = Some(ClientShellWorktree {
        key: "repo".into(),
        label: "repo".into(),
        is_linked_worktree: true,
    });
    let mut state = ClientShellState::new(tree_config(true));
    show(&mut state, projected);
    state.open_workspace_context_menu("ws_1".into(), 3, 2);
    let Some(ClientShellOverlay::ContextMenu(menu)) = state.overlay.as_ref() else {
        panic!("no menu");
    };
    let items = menu.items();
    let collapse = items
        .iter()
        .position(|item| item.label == "Collapse")
        .expect("the group's Collapse entry");
    let mut outcome = ClientShellInput::default();
    state.activate_context_menu_item(collapse, &mut outcome);
    assert!(
        state.collapsed_groups.contains(&tree_fold_key("ws_1")),
        "Collapse folds the space row the tree shows: {:?}",
        state.collapsed_groups
    );
    state.open_workspace_context_menu("ws_1".into(), 3, 2);
    let Some(ClientShellOverlay::ContextMenu(menu)) = state.overlay.as_ref() else {
        panic!("no menu");
    };
    assert!(menu.items().iter().any(|item| item.label == "Expand"));
}

/// Space `herdr` with one lone agent tab, `herdr-lead`, working on a build.
fn single_agent_space() -> ClientShellSnapshot {
    let mut projected = agents_space(&[("herdr-lead", AgentStatus::Working)]);
    projected.agents[0].tokens = vec![("stage".into(), "build · fixes".into())];
    projected
}

#[test]
fn tree_space_with_one_agent_is_one_row() {
    let config = ClientShellConfig::from_config(&Config::default());
    let render = render_tree(&single_agent_space(), &config, tall(200));
    assert_eq!(
        render.rows[2..4],
        ["▎ ⠹ herdr       Build", ""],
        "no fold arrow, its column kept: mark, space name, stage word; no agent row under it"
    );
    assert_eq!(
        render.buffer[(2, 2)].fg,
        working_colour(&config.palette),
        "working mark"
    );
    assert_eq!(
        render.hits.workspaces[0].group_toggle, None,
        "nothing to fold, so no fold toggle"
    );
    let folded = render_tree(
        &single_agent_space(),
        &config,
        TreeRenderInput {
            folded: &["ws_1"],
            ..tall(200)
        },
    );
    assert_eq!(
        folded.rows[2], "▎ ⠹ herdr       Build",
        "a fold left from before draws the same row"
    );

    let mut with_shell = single_agent_space();
    push_shell_tab(&mut with_shell, "ws_1", 2, "logs");
    let render = render_tree(&with_shell, &config, tall(200));
    assert_eq!(
        render.rows[2..4],
        ["▎ ⠹ herdr ▾     Build", "▎ ╰╴· logs"],
        "a shell tab under it keeps the arrow"
    );
    assert!(render.hits.workspaces[0].group_toggle.is_some());

    let mut two = agents_space(&[
        ("herdr-lead", AgentStatus::Idle),
        ("herdr-w1", AgentStatus::Working),
    ]);
    two.agents[1].tokens = vec![("lead".into(), "pane_1".into())];
    let render = render_tree(&two, &config, tall(200));
    assert_eq!(
        render.rows[2..5],
        ["▎ ○ herdr ▾", "▎ ╰╴⠄ herdr-w1", ""],
        "more agents: the main lead stays on the space row, its workers under it"
    );
}

#[test]
fn tree_single_agent_space_row_keeps_its_clicks_fold_and_opens_the_agents_menu() {
    let mut state = ClientShellState::new(tree_config(true));
    let mut projected = single_agent_space();
    push_shell_tab(&mut projected, "ws_1", 2, "logs");
    show(&mut state, projected);
    let row = state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.workspace_id == "ws_1")
        .expect("space row")
        .rect;
    let target = right_click(&mut state, row);
    assert!(
        matches!(
            target,
            Some(ClientContextMenuTarget::Pane { ref pane_id, .. }) if pane_id == "pane_1"
        ),
        "the merged row's menu is its agent's: {target:?}"
    );
    state.compose(106, 30).expect("frame");
    hover_space(&mut state, "ws_1", 30);
    let (plus, _) = state
        .hits
        .tree_new_tab
        .iter()
        .find(|(_, target)| *target == TreeTarget::NewTab("ws_1".into()))
        .cloned()
        .expect("the current space's `+`");
    let target = right_click(
        &mut state,
        Rect::new(plus.x.saturating_sub(2), plus.y, 1, 1),
    );
    assert!(
        matches!(
            target,
            Some(ClientContextMenuTarget::Workspace { ref workspace_id, .. }) if workspace_id == "ws_1"
        ),
        "the `+` keeps the space's menu: {target:?}"
    );
    state.compose(106, 30).expect("frame");
    assert!(
        state
            .hits
            .tree_rows
            .iter()
            .any(|(_, target)| *target == TreeTarget::Tab("tab_2".into())),
        "the shell tab stays under the merged row"
    );
    click_arrow(&mut state, "ws_1");
    state.compose(106, 30).expect("frame");
    assert!(
        state.hits.tree_rows.is_empty(),
        "folding hides the shell tab"
    );
}

/// A new space as `+` makes it: `oakwood`, one tab holding one shell, focused.
fn one_shell_space() -> ClientShellSnapshot {
    let mut projected = snapshot();
    let mut oakwood = tree_workspace("ws_1", 1, "oakwood", AgentStatus::Unknown);
    oakwood.focused = true;
    projected.workspaces = vec![oakwood];
    let mut tab = tree_tab("tab_1", "ws_1", 1, None);
    tab.focused = true;
    projected.tabs = vec![tab];
    let mut pane = tree_pane("pane_1", "ws_1", "tab_1");
    pane.focused = true;
    projected.panes = vec![pane];
    projected.agents = Vec::new();
    projected.focused_workspace_id = Some("ws_1".into());
    projected.focused_tab_id = Some("tab_1".into());
    projected.focused_pane_id = Some("pane_1".into());
    projected
}

#[test]
fn a_space_row_always_stands_for_its_first_tabs_first_pane() {
    let config = ClientShellConfig::from_config(&Config::default());
    let shell = one_shell_space();
    let render = render_tree(&shell, &config, tall(200));
    assert_eq!(
        render.rows[2..4],
        ["▎ · oakwood", ""],
        "a new one-shell space is one row with the shell's mark; no child row"
    );
    assert_eq!(
        render.buffer[(2, 2)].fg,
        shell_mark_colour(&config.palette),
        "the shell mark in the shell's colour"
    );
    assert_eq!(
        render.hits.workspaces[0].group_toggle, None,
        "nothing to fold"
    );

    let mut claude = one_shell_space();
    claude.agents = vec![tree_agent(&claude.panes[0], "claude", AgentStatus::Working)];
    claude.agents[0].tokens = vec![("stage".into(), "build · fixes".into())];
    let render = render_tree(&claude, &config, tall(200));
    assert_eq!(
        render.rows[2..4],
        ["▎ ⠹ oakwood     Build", ""],
        "Claude starting in the pane: the same row, its mark in place of the shell's"
    );

    // Claude exits: the snapshot is the one-shell space again, so one row again.
    let render = render_tree(&shell, &config, tall(200));
    assert_eq!(render.rows[2..4], ["▎ · oakwood", ""], "Claude exited");

    let mut tabs = one_shell_space();
    push_shell_tab(&mut tabs, "ws_1", 2, "scratch");
    let render = render_tree(&tabs, &config, tall(200));
    assert_eq!(
        render.rows[2..5],
        ["▎ · oakwood ▾", "▎ ╰╴· scratch", ""],
        "a second tab is the only child row"
    );

    let mut split = one_shell_space();
    split.panes.push(tree_pane("pane_2", "ws_1", "tab_1"));
    let render = render_tree(&split, &config, tall(200));
    assert_eq!(
        render.rows[2..5],
        ["▎ · oakwood ▾", "▎ ╰╴· repo", ""],
        "a second pane in the first tab hangs under the space row"
    );

    let mut git = one_shell_space();
    git.workspaces[0].branch = Some("main".into());
    let render = render_tree(&git, &config, tall(200));
    assert_eq!(
        render.rows[2..5],
        ["▎ · oakwood", "▎   main", ""],
        "a git folder: the branch row under the space row"
    );
}

#[test]
fn a_shell_space_row_focuses_its_pane_and_its_menu_closes_the_whole_space() {
    let mut state = ClientShellState::new(tree_config(true));
    let mut projected = one_shell_space();
    push_shell_tab(&mut projected, "ws_1", 2, "scratch");
    projected.panes[0].focused = false;
    projected.tabs[0].focused = false;
    show(&mut state, projected);
    let row = state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.workspace_id == "ws_1")
        .expect("space row")
        .rect;
    let clicked = click(&mut state, row);
    let method = sent_method(&clicked);
    assert!(
        matches!(
            method,
            crate::api::schema::Method::PaneFocus(ref target) if target.pane_id == "pane_1"
        ),
        "a click on the row goes to the first tab's first pane: {method:?}"
    );
    state.compose(106, 30).expect("frame");
    let target = right_click(&mut state, row);
    assert!(
        matches!(
            target,
            Some(ClientContextMenuTarget::Pane {
                ref pane_id,
                row: Some(ClientRowMenu { space: Some(_) }),
                ..
            }) if pane_id == "pane_1"
        ),
        "the row's menu is its pane's with the space's items, so Close space closes it all: {target:?}"
    );
}

#[test]
fn tree_panel_shows_only_while_something_needs_you() {
    let config = ClientShellConfig::from_config(&Config::default());
    let working = agents_space(&[
        ("herdr-lead", AgentStatus::Working),
        ("herdr-w1", AgentStatus::Working),
    ]);
    assert_no_panel(&render_tree(&working, &config, tall_panel(200)));

    let mut blocked = working.clone();
    blocked.agents[1].agent_status = AgentStatus::Blocked;
    assert_eq!(
        panel(&render_tree(&blocked, &config, tall_panel(200))),
        ["  ACTION REQUIRED 1", "  ! herdr/herdr-w1"],
        "the working lead is in the tree, not the panel"
    );

    let mut done = working;
    done.agents[1].agent_status = AgentStatus::Done;
    done.agents[1].tokens = vec![("task".into(), "fix fold".into())];
    assert_eq!(
        panel(&render_tree(&done, &config, tall_panel(200))),
        ["  COMPLETED 1", "  ✓ herdr/herdr-w1"],
        "done and not seen yet: look at it"
    );
}

#[test]
fn tree_working_agents_show_how_long_they_have_worked() {
    let config = ClientShellConfig::from_config(&Config::default());
    let mut two = agents_space(&[
        ("herdr-lead", AgentStatus::Idle),
        ("herdr-w1", AgentStatus::Working),
    ]);
    two.agents[1].tokens = vec![("lead".into(), "pane_1".into())];
    let at = |working_for| TreeRenderInput {
        working_for,
        ..tall(200)
    };
    let render = render_tree(&two, &config, at(&[("pane_2", 270)]));
    assert_eq!(
        render.rows[3], "▎ ╰╴⠄ herdr-w1     4m",
        "whole minutes, at the right"
    );
    assert_eq!(render.buffer[(19, 3)].fg, config.palette.overlay0, "dim");
    assert_eq!(
        render_tree(&two, &config, at(&[("pane_2", 4500)])).rows[3],
        "▎ ╰╴⠄ herdr-w1  1h15m"
    );
    assert_eq!(
        render_tree(&two, &config, at(&[("pane_2", 30)])).rows[3],
        "▎ ╰╴⠄ herdr-w1",
        "under a minute: no time"
    );
    assert_eq!(
        render_tree(&two, &config, at(&[("pane_2", 60)])).rows[3],
        "▎ ╰╴⠄ herdr-w1     1m",
        "a time once one minute has passed"
    );
    assert_eq!(
        render_tree(&two, &config, at(&[("pane_1", 270)])).rows[2],
        "▎ ○ herdr ▾",
        "only while it works"
    );
    assert_eq!(
        render_tree(&single_agent_space(), &config, at(&[("pane_1", 270)])).rows[2],
        "▎ ⠹ herdr       Build",
        "a lead's stage word, not its time"
    );
}

#[test]
fn tree_client_notes_when_it_first_sees_an_agent_work() {
    let mut state = ClientShellState::new(tree_config(true));
    let mut projected = agents_space(&[
        ("herdr-lead", AgentStatus::Working),
        ("herdr-w1", AgentStatus::Idle),
    ]);
    state.set_snapshot(Box::new(projected.clone()));
    let since = |state: &ClientShellState| state.endpoints[0].working_since.clone();
    let first = since(&state);
    assert_eq!(first.keys().collect::<Vec<_>>(), ["pane_1"]);

    projected.revision += 1;
    state.set_snapshot(Box::new(projected.clone()));
    assert_eq!(since(&state), first, "still working: the start stays");

    projected.revision += 1;
    projected.agents[0].agent_status = AgentStatus::Idle;
    projected.agents[0].state_change_seq += 1;
    state.set_snapshot(Box::new(projected));
    assert!(since(&state).is_empty(), "stopped: forgotten");
}

#[test]
fn tree_off_keeps_no_first_seen_times() {
    let mut state = ClientShellState::new(tree_config(false));
    let mut projected = agents_space(&[
        ("herdr-lead", AgentStatus::Working),
        ("herdr-w1", AgentStatus::Blocked),
    ]);
    state.set_snapshot(Box::new(projected.clone()));
    projected.revision += 1;
    projected.agents[0].agent_status = AgentStatus::Done;
    projected.agents[0].state_change_seq += 1;
    state.set_snapshot(Box::new(projected));
    let endpoint = &state.endpoints[0];
    assert!(
        endpoint.working_since.is_empty()
            && endpoint.done_since.is_empty()
            && endpoint.waiting_since.is_empty(),
        "the original sidebar shows no times: nothing to keep per snapshot"
    );
}

#[test]
fn tree_working_time_comes_from_the_machine_whose_panes_it_draws() {
    let mut state = ClientShellState::new(tree_config(true));
    let mut local = agents_space(&[
        ("herdr-lead", AgentStatus::Idle),
        ("herdr-w1", AgentStatus::Working),
    ]);
    local.agents[1].tokens = vec![("lead".into(), "pane_1".into())];
    state.set_snapshot(Box::new(local.clone()));
    state.set_pane_surface(surface());
    let ago =
        |minutes: u64| std::time::Instant::now() - std::time::Duration::from_secs(minutes * 60);
    state.endpoints[0]
        .working_since
        .insert("pane_2".into(), ago(10));
    let text = |state: &mut ClientShellState| frame_text(&state.compose(106, 30).expect("frame"));
    // The word after `herdr-w1` on its row: the time, right-aligned.
    let w1_time = |text: &str| {
        text.lines()
            .find_map(|line| line.split_once(" herdr-w1 "))
            .and_then(|(_, rest)| rest.split_whitespace().next().map(str::to_owned))
    };
    assert_eq!(
        w1_time(&text(&mut state)).as_deref(),
        Some("10m"),
        "control: one machine draws its time"
    );

    // A second machine whose same pane id started working at another time.
    let profile = crate::client::endpoint::SavedSshEndpoint {
        id: crate::client::endpoint::ProfileId::parse("0123456789abcdef0123456789abcdef")
            .expect("profile id"),
        label: "Build".into(),
        target: "dev@build.example".into(),
        session: "agents".into(),
        enabled: true,
    };
    let remote_id = ClientEndpointId::Ssh(profile.id.clone());
    state.set_endpoint_catalog(&[profile]);
    state.set_endpoint_status(&remote_id, ClientEndpointStatus::Online);
    let mut remote = local;
    remote.boot_id = "remote-boot".into();
    state.set_endpoint_snapshot(&remote_id, Box::new(remote));
    let remote_index = state
        .endpoints
        .iter()
        .position(|endpoint| endpoint.endpoint_id == remote_id)
        .expect("remote endpoint");
    state.endpoints[remote_index]
        .working_since
        .insert("pane_2".into(), ago(4));

    // Why it holds: the tree draws only with one machine, which is always the active one.
    let local_active = text(&mut state);
    assert!(
        state.hits.tree_rows.is_empty(),
        "two machines: no tree rows"
    );
    assert!(
        w1_time(&local_active).as_deref() != Some("4m"),
        "Local active: never the other machine's time:\n{local_active}"
    );
    assert!(state.activate_endpoint_projection(&remote_id));
    let remote_active = text(&mut state);
    assert!(
        state.hits.tree_rows.is_empty(),
        "two machines: no tree rows"
    );
    assert!(
        w1_time(&remote_active).as_deref() != Some("10m"),
        "Build active: the Local panes never get Build's time, nor Build's panes Local's:\n{remote_active}"
    );
}

/// `tree_snapshot` with branches on both spaces and the done agent's pane focused.
fn spaces_with_branches() -> ClientShellSnapshot {
    let mut projected = tree_snapshot();
    projected.workspaces[0].branch = Some("main".into());
    projected.workspaces[1].branch = Some("fix".into());
    projected
}

#[test]
fn tree_space_shows_its_branch_on_a_dim_line_under_it() {
    let config = ClientShellConfig::from_config(&Config::default());
    let render = render_tree(&git_snapshot(), &config, tall(200));
    assert_eq!(
        render.rows[2..5],
        ["▎ ⠹ herdr ▾", "▎ │ feat/sidebar-… ↑6", "▎ ├╴· repo"],
        "under the name; ahead/behind at its end"
    );
    assert_eq!(render.buffer[(4, 3)].fg, config.palette.overlay0, "dim");
    let dotworks = render
        .rows
        .iter()
        .position(|row| row.starts_with("▎ ! dotworks ▾"))
        .expect("dotworks row");
    assert_eq!(
        render.rows[dotworks + 1],
        "▎ ╰╴✓ claude",
        "a space without a branch has no line"
    );

    let mut projected = git_snapshot();
    projected.workspaces[0].branch = Some("ma\u{1b}in".into());
    assert_eq!(
        render_tree(&projected, &config, tall(200)).rows[3],
        "▎ │ main           ↑6",
        "control characters dropped"
    );
}

#[test]
fn tree_branch_line_follows_the_spaces_rows_setting() {
    use crate::config::SpaceSidebarToken;
    let mut config = ClientShellConfig::from_config(&Config::default());
    config.spaces.rows = vec![vec![
        SpaceSidebarToken::StateIcon,
        SpaceSidebarToken::Workspace,
    ]];
    let render = render_tree(&git_snapshot(), &config, tall(200));
    assert_eq!(
        render.rows[3], "▎ ├╴· repo",
        "no `branch` in the rows: no line"
    );

    config.spaces.rows = vec![vec![SpaceSidebarToken::Styled {
        token: Box::new(SpaceSidebarToken::Branch),
        style: Default::default(),
        rules: Vec::new(),
    }]];
    let render = render_tree(&git_snapshot(), &config, tall(200));
    assert_eq!(
        render.rows[3], "▎ │ feat/sidebar-… ↑6",
        "a styled `branch` counts"
    );
}

#[test]
fn tree_folded_and_one_row_spaces_keep_their_branch_line() {
    let config = ClientShellConfig::from_config(&Config::default());
    let folded = render_tree(
        &spaces_with_branches(),
        &config,
        TreeRenderInput {
            folded: &["ws_1"],
            ..tall(200)
        },
    );
    assert_eq!(
        folded.rows[2..6],
        ["▎ ⠹ herdr ▸", "▎   main", "▎ ! dotworks ▾", "▎ │ fix"],
        "{:#?}",
        folded.rows
    );

    let mut single = single_agent_space();
    single.workspaces[0].branch = Some("master".into());
    let render = render_tree(&single, &config, tall(200));
    assert_eq!(
        render.rows[2..5],
        ["▎ ⠹ herdr       Build", "▎   master", ""],
        "under the name; the line brings no fold arrow back"
    );
    assert_eq!(render.hits.workspaces[0].group_toggle, None);
}

/// The dotworks case: space `dotworks` whose first pane is a shell in `~` (no repo, no
/// folder branch), then one Claude tab per entry of `branches`, its pane on that branch
/// (`""` for none).
fn dotworks_space(branches: &[&str]) -> ClientShellSnapshot {
    let mut projected = snapshot();
    let mut dotworks = tree_workspace("ws_1", 1, "dotworks", AgentStatus::Working);
    dotworks.focused = true;
    projected.workspaces = vec![dotworks];
    projected.tabs = vec![tree_tab("tab_0", "ws_1", 0, Some("main"))];
    let mut home = tree_pane("pane_0", "ws_1", "tab_0");
    home.cwd = Some("~".into());
    projected.panes = vec![home];
    for (index, branch) in branches.iter().enumerate() {
        let id = index + 1;
        push_shell_tab(&mut projected, "ws_1", id, &format!("w{id}"));
        let pane = projected.panes.last_mut().expect("a pane was pushed");
        pane.git_branch = (!branch.is_empty()).then(|| (*branch).to_owned());
        let agent = tree_agent(pane, "claude", AgentStatus::Working);
        projected.agents.push(agent);
    }
    projected
}

fn rows_with(render: &TreeRender, needle: &str) -> usize {
    render
        .rows
        .iter()
        .filter(|row| row.contains(needle))
        .count()
}

#[test]
fn tree_a_branch_every_child_shares_shows_once_on_the_space_line() {
    let config = ClientShellConfig::from_config(&Config::default());
    let render = render_tree(&dotworks_space(&["master", "master"]), &config, tall(0));
    assert!(render.rows[2].contains("dotworks"), "{:#?}", render.rows);
    assert!(
        render.rows[3].trim_end().ends_with(" master"),
        "the space's line: {:#?}",
        render.rows
    );
    assert_eq!(
        rows_with(&render, "master"),
        1,
        "the children show none: {:#?}",
        render.rows
    );
}

#[test]
fn tree_a_branch_not_every_child_shares_stays_on_the_children() {
    let config = ClientShellConfig::from_config(&Config::default());
    for (branches, why) in [
        (&["master", "fix/x"][..], "they disagree"),
        (&["master", ""][..], "one child has no branch"),
        (&["master"][..], "one child shares with no one"),
    ] {
        let render = render_tree(&dotworks_space(branches), &config, tall(0));
        assert!(
            !render.rows[3].contains("master"),
            "{why}: nothing lifted: {:#?}",
            render.rows
        );
        for branch in branches.iter().filter(|branch| !branch.is_empty()) {
            assert_eq!(
                rows_with(&render, branch),
                1,
                "{why}: {branch} keeps its own line: {:#?}",
                render.rows
            );
        }
    }
}

#[test]
fn tree_a_space_with_its_own_branch_lifts_nothing() {
    let config = ClientShellConfig::from_config(&Config::default());
    let mut projected = dotworks_space(&["master", "master"]);
    projected.panes[0].git_branch = Some("main".into());
    let render = render_tree(&projected, &config, tall(0));
    assert!(
        render.rows[3].trim_end().ends_with(" main"),
        "{:#?}",
        render.rows
    );
    assert_eq!(rows_with(&render, "master"), 2, "{:#?}", render.rows);

    let mut projected = dotworks_space(&["master", "master"]);
    projected.workspaces[0].branch = Some("main".into());
    let render = render_tree(&projected, &config, tall(0));
    assert!(
        render.rows[3].trim_end().ends_with(" main"),
        "{:#?}",
        render.rows
    );
    assert_eq!(
        rows_with(&render, "master"),
        2,
        "the folder's branch: {:#?}",
        render.rows
    );
}

#[test]
fn tree_a_lifted_branch_stays_when_folded_and_never_shows_ahead_behind() {
    let config = ClientShellConfig::from_config(&Config::default());
    let folded = render_tree(
        &dotworks_space(&["master", "master"]),
        &config,
        TreeRenderInput {
            folded: &["ws_1"],
            ..tall(0)
        },
    );
    assert!(folded.rows[2].contains("dotworks ▸"), "{:#?}", folded.rows);
    assert!(
        folded.rows[3].trim_end().ends_with(" master"),
        "{:#?}",
        folded.rows
    );

    let mut projected = dotworks_space(&["master", "master"]);
    projected.workspaces[0].git_ahead_behind = Some((6, 1));
    let render = render_tree(&projected, &config, tall(0));
    assert!(render.rows[3].contains("master"), "{:#?}", render.rows);
    assert_eq!(rows_with(&render, "↑"), 0, "{:#?}", render.rows);
    assert_eq!(rows_with(&render, "↓"), 0, "{:#?}", render.rows);
}

#[test]
fn tree_a_shared_branch_shows_nowhere_without_branch_in_the_spaces_rows() {
    use crate::config::SpaceSidebarToken;
    let mut config = ClientShellConfig::from_config(&Config::default());
    config.spaces.rows = vec![vec![
        SpaceSidebarToken::StateIcon,
        SpaceSidebarToken::Workspace,
    ]];
    let render = render_tree(&dotworks_space(&["master", "master"]), &config, tall(0));
    assert_eq!(rows_with(&render, "master"), 0, "{:#?}", render.rows);
}

#[test]
fn tree_branch_line_clicks_act_as_its_space_row() {
    let mut state = ClientShellState::new(tree_config(true));
    let mut projected = single_agent_space();
    projected.workspaces[0].branch = Some("master".into());
    show(&mut state, projected);
    let hit = state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.workspace_id == "ws_1")
        .expect("space hit")
        .rect;
    assert_eq!(hit.height, 2, "one hit over the space row and its line");
    assert_eq!(
        state
            .hits
            .workspaces
            .iter()
            .filter(|hit| hit.workspace_id == "ws_1")
            .count(),
        1,
        "one drop slot per space"
    );
    let line = Rect::new(hit.x, hit.y + 1, 1, 1);
    let target = right_click(&mut state, line);
    assert!(
        matches!(
            target,
            Some(ClientContextMenuTarget::Pane { ref pane_id, .. }) if pane_id == "pane_1"
        ),
        "the line's menu is its space row's, here the agent's: {target:?}"
    );
}

#[test]
fn tree_space_row_click_focuses_its_main_lead() {
    let mut state = ClientShellState::new(tree_config(true));
    let mut projected = single_agent_space();
    projected.workspaces[0].branch = Some("master".into());
    push_shell_tab(&mut projected, "ws_1", 2, "logs");
    show(&mut state, projected);
    let hit = state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.workspace_id == "ws_1")
        .expect("space hit")
        .rect;
    for (rect, what) in [
        (Rect::new(hit.x, hit.y, 1, 1), "space row"),
        (Rect::new(hit.x, hit.y + 1, 1, 1), "branch line"),
    ] {
        assert!(
            matches!(
                sent_method(&click(&mut state, rect)),
                crate::api::schema::Method::PaneFocus(ref target) if target.pane_id == "pane_1"
            ),
            "the {what} focuses the main lead's pane"
        );
    }

    let mut shells_only = snapshot();
    shells_only.workspaces[0].focused = true;
    push_shell_tab(&mut shells_only, "ws_1", 2, "logs");
    show(&mut state, shells_only);
    let hit = state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.workspace_id == "ws_1")
        .expect("space hit")
        .rect;
    assert!(
        matches!(
            sent_method(&click(&mut state, hit)),
            crate::api::schema::Method::PaneFocus(ref target) if target.pane_id == "pane_1"
        ),
        "no main lead: the space row focuses its first pane, the shell"
    );
}

#[test]
fn tree_branch_lines_count_in_scrolling_and_reveal() {
    let mut snapshot = spaces_with_branches();
    for pane in &mut snapshot.panes {
        pane.focused = false;
    }
    snapshot.panes[4].focused = true;
    let mut config = ClientShellConfig::from_config(&Config::default());
    config.spaces.row_gap = 1;
    // Without the box, so these rows are all the tree's.
    config.sidebar_tree_panel = false;
    let short = |scroll, reveal_focused| TreeRenderInput {
        // Room for dotworks, its branch line and claude.
        height: 8,
        spinner_ms: 0,
        scroll,
        reveal_focused,
        folded: &[],
        working_for: &[],
        done_for: &[],
        waiting_for: &[],
        width: 26,
        hovered: None,
        hovered_row: None,
        dragged: None,
        home: None,
        filter: TreeFilter::All,
        board: Default::default(),
    };
    let visible =
        |render: &TreeRender, y: usize| render.rows[y].trim_end_matches('▕').trim_end().to_owned();

    let top = render_tree(&snapshot, &config, short(0, false));
    assert_eq!(
        top.hits.workspace_max_scroll, 4,
        "branch lines count as rows to scroll past"
    );

    let scrolled = render_tree(&snapshot, &config, short(1, false));
    assert_eq!(
        visible(&scrolled, 2),
        "▎ │ main",
        "the line alone at the top"
    );
    assert_eq!(scrolled.hits.workspaces[0].workspace_id, "ws_1");
    assert_eq!(
        scrolled.hits.workspaces[0].rect,
        Rect::new(0, 2, 24, 1),
        "its own hit, over the line, while its space row is scrolled away"
    );

    let revealed = render_tree(&snapshot, &config, short(0, true));
    assert_eq!(revealed.scroll, 3);
    assert_eq!(
        visible(&revealed, 4),
        "▎ ! dotworks ▾",
        "space row kept above"
    );
    assert_eq!(visible(&revealed, 6), "▎ ╰╴✓ claude");
}

#[test]
fn tree_wheel_scrolls_past_a_branch_line() {
    // 13 rows: the box takes four (its two rules, ACTION REQUIRED and codex); the tree does not
    // fit.
    let mut state = ClientShellState::new(tree_config(true));
    state.set_snapshot(Box::new(spaces_with_branches()));
    state.set_pane_surface(surface());
    state.compose(106, 13).expect("short tree frame");
    let body = state.hits.workspace_body;
    assert_eq!(
        state.hits.workspaces[0].rect,
        Rect::new(body.x, body.y, state.hits.workspaces[0].rect.width, 2)
    );
    state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: body.x + 1,
        row: body.y,
        modifiers: KeyModifiers::empty(),
    })]);
    state.compose(106, 13).expect("scrolled tree frame");
    assert_eq!(state.hits.workspaces[0].workspace_id, "ws_1");
    assert_eq!(state.hits.workspaces[0].rect.y, body.y);
    assert_eq!(state.hits.workspaces[0].rect.height, 1, "the line");
    assert_eq!(
        state
            .hits
            .tree_rows
            .first()
            .map(|(rect, target)| (rect.y, target.clone())),
        Some((body.y + 1, TreeTarget::Pane("pane_2".into())))
    );
}

#[test]
fn tree_space_names_start_in_one_column() {
    let mut projected = single_agent_space();
    projected.agents[0].tokens.clear();
    projected.workspaces[0].focused = false;
    let mut shells = tree_workspace("ws_2", 2, "notes", AgentStatus::Unknown);
    shells.focused = false;
    projected.workspaces.push(shells);
    push_shell_tab(&mut projected, "ws_2", 2, "logs");
    let config = ClientShellConfig::from_config(&Config::default());
    let render = render_tree(&projected, &config, tall(200));
    assert_eq!(
        render.rows[2..6],
        ["▎ ⠹ herdr", "▎ · notes", "", ""],
        "every name after its mark: the main lead's, else the shell's; one shell has nothing \
         to fold, so no arrow"
    );
}

/// The approved picture's `herdr` and `acme` spaces: each main lead in its space's first
/// tab; herdr's workers are the main lead's, acme's `w1` is `acme-api-lead`'s.
fn picture_spaces() -> ClientShellSnapshot {
    let mut projected = snapshot();
    projected.workspaces = vec![
        tree_workspace("ws_1", 1, "herdr", AgentStatus::Working),
        tree_workspace("ws_2", 2, "acme", AgentStatus::Blocked),
    ];
    projected.tabs = Vec::new();
    projected.panes = Vec::new();
    projected.agents = Vec::new();
    // (space, agent name, state, lead pane)
    let agents: [(&str, &str, AgentStatus, Option<&str>); 7] = [
        ("ws_1", "herdr-lead", AgentStatus::Working, None),
        ("ws_1", "herdr-w1", AgentStatus::Working, Some("pane_1")),
        ("ws_1", "herdr-w2", AgentStatus::Working, Some("pane_1")),
        ("ws_2", "acme-lead", AgentStatus::Idle, None),
        ("ws_2", "acme-api-lead", AgentStatus::Blocked, None),
        ("ws_2", "acme-api-w1", AgentStatus::Working, Some("pane_5")),
        ("ws_2", "acme-web-lead", AgentStatus::Idle, None),
    ];
    for (index, (workspace_id, name, status, lead)) in agents.into_iter().enumerate() {
        let tab_id = format!("tab_{}", index + 1);
        projected
            .tabs
            .push(tree_tab(&tab_id, workspace_id, index + 1, Some(name)));
        let pane = tree_pane(&format!("pane_{}", index + 1), workspace_id, &tab_id);
        let mut agent = tree_agent(&pane, "claude", status);
        agent.name = Some(name.into());
        agent.tokens = lead
            .map(|lead| vec![("lead".into(), lead.into())])
            .unwrap_or_default();
        projected.panes.push(pane);
        projected.agents.push(agent);
    }
    projected.focused_workspace_id = None;
    projected.focused_tab_id = None;
    projected.focused_pane_id = None;
    projected
}

#[test]
fn tree_space_row_shows_its_leads_own_glyph_and_counts_every_other_agent() {
    let config = ClientShellConfig::from_config(&Config::default());
    let palette = &config.palette;
    // The lead idle, its worker working for five minutes.
    let mut projected = lead_space(&[("herdr-lead", None), ("w1", Some("pane_1"))]);
    projected.agents[1].agent_status = AgentStatus::Working;
    let folded = || TreeRenderInput {
        folded: &["ws_1"],
        working_for: &[("pane_2", 300)],
        ..tall(200)
    };
    let render = render_tree(&projected, &config, folded());
    let sub = spinner_glyph(200, &config, true);
    assert_eq!(
        render.rows[2],
        format!("▎ ○ herdr ▸ {sub}1"),
        "the idle lead's own mark; the working worker a count after the arrow, glyph and \
         number together"
    );
    assert_eq!(
        render.buffer[(2, 2)].fg,
        idle_colour(palette),
        "the lead's idle mark"
    );
    assert_eq!(
        render.buffer[(12, 2)].fg,
        working_colour(palette),
        "the worker's spinner"
    );

    // No main lead (its first pane runs no agent): a blank mark, every agent counted.
    projected.agents.remove(0);
    let render = render_tree(&projected, &config, folded());
    assert_eq!(render.rows[2], format!("▎ · herdr ▸ {sub}1"));
}

#[test]
fn tree_folded_space_shows_its_leads_mark_and_counts_every_other_agent_once() {
    let config = ClientShellConfig::from_config(&Config::default());
    // The lead idle, its worker blocked: the row's mark is the lead's, the `!` a count.
    let mut projected = lead_space(&[("herdr-lead", None), ("w1", Some("pane_1"))]);
    projected.agents[1].agent_status = AgentStatus::Blocked;
    let folded = TreeRenderInput {
        folded: &["ws_1"],
        ..tall(200)
    };
    let render = render_tree(&projected, &config, folded);
    let row = &render.rows[2];
    assert!(row.starts_with("▎ ○ herdr ▸"), "{row:?}");
    assert!(row.contains("?1"), "the blocked worker is counted: {row:?}");
    assert!(
        !row.contains("○1"),
        "the idle lead is the mark, not also a count: {row:?}"
    );

    // The lead blocked: the counts leave the lead out, as before.
    projected.agents[0].agent_status = AgentStatus::Blocked;
    projected.agents[1].agent_status = AgentStatus::Idle;
    let folded = TreeRenderInput {
        folded: &["ws_1"],
        ..tall(200)
    };
    let row = render_tree(&projected, &config, folded).rows[2].clone();
    assert!(row.starts_with("▎ ! herdr ▸"), "{row:?}");
    assert!(!row.contains("?1"), "{row:?}");
}

#[test]
fn a_folded_space_counts_its_hidden_tabs_after_the_arrow_needs_you_then_working() {
    let config = ClientShellConfig::from_config(&Config::default());
    let palette = &config.palette;
    // The lead working at build; its workers: w1 asks, w2 and w3 work, w4 idle, w5 done.
    let lead = Some("pane_1");
    let mut projected = lead_space(&[
        ("herdr-lead", None),
        ("w1", lead),
        ("w2", lead),
        ("w3", lead),
        ("w4", lead),
        ("w5", lead),
    ]);
    projected.agents[0].agent_status = AgentStatus::Working;
    projected.agents[0].tokens = vec![("stage".into(), "build · 13b".into())];
    projected.agents[1]
        .tokens
        .push(("ask".into(), "which db?".into()));
    projected.agents[2].agent_status = AgentStatus::Working;
    projected.agents[3].agent_status = AgentStatus::Working;
    projected.agents[5].agent_status = AgentStatus::Done;
    let render = render_tree(
        &projected,
        &config,
        TreeRenderInput {
            folded: &["ws_1"],
            width: 36,
            ..tall(200)
        },
    );
    let sub = spinner_glyph(200, &config, true);
    let main = spinner_glyph(200, &config, false);
    assert_ne!(sub, main, "the sub-agent spinner is its own set");
    let row = &render.rows[2];
    assert!(
        row.starts_with(&format!("▎ {main} herdr ▸ ?1 {sub}2")),
        "{row:?}"
    );
    assert!(
        row.ends_with(" Build"),
        "the stage word alone on the right: {row:?}"
    );
    assert!(
        !row.contains('○') && !row.contains('✓'),
        "idle and done are not counted: {row:?}"
    );
    let x = |text: &str| row[..row.find(text).expect("drawn")].chars().count() as u16;
    assert_eq!(render.buffer[(x("?1"), 2)].fg, palette.yellow);
    assert_eq!(render.buffer[(x("?1") + 1, 2)].fg, palette.overlay0);
    // Working in the colour of the worker rows it stands for.
    assert_eq!(render.buffer[(x(sub), 2)].fg, working_colour(palette));
    assert_eq!(render.buffer[(x(sub) + 1, 2)].fg, palette.overlay0);
}

/// `herdr-lead` idle; its workers: `w1` asks, `w2` blocked, `w3` done, `w4` working.
fn legend_space() -> ClientShellSnapshot {
    let lead = Some("pane_1");
    let mut projected = lead_space(&[
        ("herdr-lead", None),
        ("w1", lead),
        ("w2", lead),
        ("w3", lead),
        ("w4", lead),
    ]);
    projected.agents[1]
        .tokens
        .push(("ask".into(), "which db?".into()));
    projected.agents[2].agent_status = AgentStatus::Blocked;
    projected.agents[3].agent_status = AgentStatus::Done;
    projected.agents[4].agent_status = AgentStatus::Working;
    projected
}

/// The mark cell of the first row that holds `name`.
fn mark_of<'a>(render: &'a TreeRender, name: &str) -> (&'a str, ratatui::style::Color) {
    let y = render
        .rows
        .iter()
        .position(|row| row.contains(name))
        .unwrap_or_else(|| panic!("no row with {name:?}: {:#?}", render.rows));
    let x = if render.rows[y].contains('╴') { 4 } else { 2 };
    let cell = &render.buffer[(x, y as u16)];
    (cell.symbol(), cell.fg)
}

#[test]
fn tree_and_box_share_one_legend_and_a_space_shows_its_leads_own_mark() {
    use crate::config::StatusIndicatorStyle;
    for style in [StatusIndicatorStyle::Dots, StatusIndicatorStyle::Symbols] {
        let mut config = Config::default();
        config.ui.status_indicators = style;
        let config = ClientShellConfig::from_config(&config);
        let palette = &config.palette;
        let render = render_tree(&legend_space(), &config, tall(200));
        assert_eq!(
            mark_of(&render, "w1"),
            ("?", palette.yellow),
            "asks, {style:?}"
        );
        assert_eq!(
            mark_of(&render, "w2"),
            ("!", approval_colour(palette)),
            "blocked, {style:?}"
        );
        assert_eq!(
            mark_of(&render, "w3"),
            ("✓", done_colour(palette)),
            "done, {style:?}"
        );
        assert_eq!(
            mark_of(&render, "w4"),
            ("⠄", working_colour(palette)),
            "working, braille's partner"
        );
        assert_eq!(
            mark_of(&render, "herdr ▾"),
            ("○", idle_colour(palette)),
            "the space: its idle lead's own mark, {style:?}"
        );
    }

    let config = ClientShellConfig::from_config(&Config::default());
    let palette = &config.palette;
    // Whatever its workers do, the space shows its lead's own state.
    let mut projected = legend_space();
    let space = |projected: &ClientShellSnapshot| {
        let render = render_tree(projected, &config, tall(200));
        let (mark, color) = mark_of(&render, "herdr ▾");
        (mark.to_owned(), color)
    };
    for step in 1..5 {
        projected.agents[step].agent_status = AgentStatus::Idle;
        projected.agents[step].tokens.clear();
        assert_eq!(
            space(&projected),
            ("○".into(), idle_colour(palette)),
            "step {step}"
        );
    }
    projected.agents[0].agent_status = AgentStatus::Working;
    assert_eq!(
        space(&projected),
        ("⠹".into(), working_colour(palette)),
        "working"
    );
    projected.agents[0]
        .tokens
        .push(("ask".into(), "which db?".into()));
    projected.agents[0].agent_status = AgentStatus::Idle;
    assert_eq!(
        space(&projected),
        ("?".into(), palette.yellow),
        "a question"
    );
    projected.agents[0].tokens.clear();
    projected.agents[0].agent_status = AgentStatus::Done;
    assert_eq!(
        space(&projected),
        ("✓".into(), done_colour(palette)),
        "a done lead's own mark"
    );

    // The row's time is its lead's: a working lead keeps it when a worker is blocked.
    let mut projected = lead_space(&[("herdr-lead", None), ("w1", Some("pane_1"))]);
    projected.agents[0].agent_status = AgentStatus::Working;
    let timed = TreeRenderInput {
        working_for: &[("pane_1", 300)],
        ..tall(200)
    };
    let render = render_tree(&projected, &config, timed);
    assert_eq!(
        render.rows[2], "▎ ⠹ herdr ▾        5m",
        "control: the lead's time"
    );
    projected.agents[1].agent_status = AgentStatus::Blocked;
    let timed = TreeRenderInput {
        working_for: &[("pane_1", 300)],
        ..tall(200)
    };
    let render = render_tree(&projected, &config, timed);
    assert_eq!(
        render.rows[2], "▎ ⠹ herdr ▾        5m",
        "the lead's mark and time stay beside a blocked worker"
    );

    // The box marks its entries the same way.
    let render = render_tree(&panel_snapshot(), &config, tall_panel(200));
    let top = render
        .rows
        .iter()
        .rposition(|row| row.contains("ACTION REQUIRED"))
        .expect("box");
    let boxed = |name: &str| {
        let y = (top..render.rows.len())
            .find(|&y| render.rows[y].contains(name))
            .unwrap_or_else(|| panic!("no box row with {name:?}: {:#?}", render.rows));
        let cell = &render.buffer[(2, y as u16)];
        (cell.symbol().to_owned(), cell.fg)
    };
    assert_eq!(boxed("web/rev"), ("?".into(), palette.yellow), "box: asks");
    assert_eq!(
        boxed("web/cx"),
        ("!".into(), approval_colour(palette)),
        "box: blocked"
    );
    assert_eq!(
        boxed("pair"),
        ("✓".into(), done_colour(palette)),
        "box: done"
    );
}

#[test]
fn tree_main_lead_folds_into_its_space_row_with_its_workers_under_the_space() {
    let config = ClientShellConfig::from_config(&Config::default());
    let render = render_tree(&picture_spaces(), &config, tall(200));
    assert_eq!(
        render.rows[2..11],
        [
            "▎ ⠹ herdr ▾",
            "▎ ├╴⠄ herdr-w1",
            "▎ ╰╴⠄ herdr-w2",
            "▎ ○ acme ▾",
            "▎ ├╴! acme-api-lead",
            "▎ │   ╰╴⠄ acme-api-w1",
            "▎ ╰╴○ acme-web-lead",
            "",
            "",
        ],
        "the main lead's mark on the space row; its workers directly under the space, \
         any other lead's under that lead, each connector under its parent's name"
    );
    assert_eq!(
        render.buffer[(2, 2)].fg,
        working_colour(&config.palette),
        "main lead's mark"
    );
}

#[test]
fn tree_main_leads_tab_has_no_row_when_another_agent_shares_it() {
    // tab_1 (`main`) holds the main lead pane_1 and a second agent pane_2.
    let mut projected = tree_snapshot();
    let second = tree_agent(&projected.panes[2], "codex", AgentStatus::Idle);
    projected.agents.push(second);
    let config = ClientShellConfig::from_config(&Config::default());
    let render = render_tree(&projected, &config, tall(200));
    assert!(
        !render.rows.iter().any(|row| row.contains("main")),
        "no row for the main lead's tab: {:#?}",
        render.rows
    );
    assert!(
        !render
            .targets
            .iter()
            .any(|(_, target)| *target == TreeTarget::Tab("tab_1".into())),
        "no tab hit for the main lead's tab: {:?}",
        render.targets
    );
    let (second_row, _) = render
        .targets
        .iter()
        .find(|(_, target)| *target == TreeTarget::Pane("pane_2".into()))
        .expect("the second agent's own row");
    assert!(
        render.rows[usize::from(second_row.y)].starts_with("▎ ├╴"),
        "the second agent hangs under the space row: {:#?}",
        render.rows
    );

    // Its right-click menu is the pane's: Close there closes only that pane.
    let mut state = ClientShellState::new(tree_config(true));
    show(&mut state, projected);
    let rect = tree_row(&state, &TreeTarget::Pane("pane_2".into()));
    let target = right_click(&mut state, rect);
    assert!(
        matches!(
            target,
            Some(ClientContextMenuTarget::Pane { ref pane_id, .. }) if pane_id == "pane_2"
        ),
        "{target:?}"
    );
}

#[test]
fn tree_reads_each_leads_stage_from_its_own_pane_and_shows_it_instead_of_its_time() {
    let config = ClientShellConfig::from_config(&Config::default());
    let mut projected = picture_spaces();
    // The space's own stage tokens are no longer read.
    projected.workspaces[0].tokens = vec![
        ("stage".into(), "ship · old".into()),
        ("stage_by".into(), "pane_9".into()),
    ];
    projected.agents[0].tokens = vec![("stage".into(), "build · w1 on panel box".into())];
    projected.agents[4].tokens = vec![("stage".into(), "plan".into())];
    // A worker's stage never shows: workers show their time.
    projected.agents[5]
        .tokens
        .push(("stage".into(), "review".into()));
    let render = render_tree(&projected, &config, input_with_times());
    assert_eq!(
        [&render.rows[2], &render.rows[6], &render.rows[7]],
        [
            "▎ ⠹ herdr ▾         Build",
            "▎ ├╴! acme-api-lead  Plan",
            "▎ │   ╰╴⠄ acme-api-w1  1m",
        ],
        "the main lead's stage on the space row, instead of its 4m; another lead's on its \
         own row; the worker's time"
    );
    assert_eq!(render.buffer[(20, 2)].fg, config.palette.yellow, "build");
    assert_eq!(render.buffer[(21, 6)].fg, config.palette.mauve, "plan");

    projected.agents[0].tokens.clear();
    let render = render_tree(&projected, &config, input_with_times());
    assert_eq!(
        render.rows[2], "▎ ⠹ herdr ▾            4m",
        "no stage: the working time"
    );
}

#[test]
fn tree_draws_the_stage_word_verify_in_the_same_color_as_check() {
    let config = ClientShellConfig::from_config(&Config::default());
    let mut projected = picture_spaces();
    projected.agents[0].tokens = vec![("stage".into(), "check".into())];
    projected.agents[4].tokens = vec![("stage".into(), "verify · w2 on tests".into())];
    // Two columns wider than `input_with_times`, so `Verify` fits beside acme's lead.
    let input = TreeRenderInput {
        width: 32,
        ..input_with_times()
    };
    let render = render_tree(&projected, &config, input);
    for (row, word) in [(2, "Verify"), (6, "Verify")] {
        let column = render.rows[row]
            .chars()
            .count()
            .checked_sub(word.chars().count())
            .expect("the stage word ends the row");
        assert!(render.rows[row].ends_with(word), "{:?}", render.rows[row]);
        assert_eq!(
            render.buffer[(column as u16, row as u16)].fg,
            config.palette.blue,
            "{word}"
        );
    }
}

/// herdr's main lead working for 4m30s and acme's `w1` for 1m30s, at 30 columns.
fn input_with_times() -> TreeRenderInput {
    TreeRenderInput {
        width: 30,
        working_for: &[("pane_1", 270), ("pane_6", 90)],
        ..tall(200)
    }
}

/// `picture_spaces` with the picture's branches: herdr's `w2` and acme's web lead on their
/// own; everyone else on their space's.
fn picture_branches() -> ClientShellSnapshot {
    let mut projected = picture_spaces();
    projected.workspaces[0].branch = Some("feat/sidebar-tree".into());
    projected.workspaces[1].branch = Some("main".into());
    for (pane, branch) in projected.panes.iter_mut().zip([
        "feat/sidebar-tree",
        "feat/sidebar-tree",
        "pane-branch",
        "main",
        "main",
        "main",
        "feature/login",
    ]) {
        pane.git_branch = Some(branch.into());
    }
    projected
}

#[test]
fn tree_shows_a_branch_line_only_where_the_branch_changes() {
    let config = ClientShellConfig::from_config(&Config::default());
    let wide = || TreeRenderInput {
        width: 30,
        ..tall_panel(200)
    };
    let render = render_tree(&picture_branches(), &config, wide());
    assert_eq!(
        render.rows[2..15],
        [
            "▎ ⠹ herdr ▾",
            "▎ │ feat/sidebar-tree",
            "▎ ├╴⠄ herdr-w1",
            "▎ ╰╴⠄ herdr-w2",
            "▎     pane-branch",
            "▎ ○ acme ▾",
            "▎ │ main",
            "▎ ├╴! acme-api-lead",
            "▎ │   ╰╴⠄ acme-api-w1",
            "▎ ╰╴○ acme-web-lead",
            "▎     feature/login",
            "",
            ""
        ],
        "the space's line is its main lead's branch; a row on another one gets its own \
         line, dim, under its name; `│` keeps the tree joined past a line"
    );
    for (x, y) in [(4, 3), (6, 6), (6, 12)] {
        assert_eq!(render.buffer[(x, y)].fg, config.palette.overlay0, "dim");
    }
    assert!(
        !render.rows.iter().any(|row| row.contains('⎇')),
        "no branch marks at the row ends"
    );
    let w2 = TreeTarget::AgentTab {
        tab_id: "tab_3".into(),
        pane_id: "pane_3".into(),
    };
    let (w2_hit, _) = render
        .targets
        .iter()
        .find(|(_, target)| *target == w2)
        .expect("w2 row");
    assert_eq!(w2_hit.height, 2, "its line clicks as its row");

    // A lead on its own branch: the line keeps its workers joined; a worker on its lead's
    // branch shows nothing.
    let mut projected = picture_branches();
    projected.panes[4].git_branch = Some("api".into());
    projected.panes[5].git_branch = Some("api".into());
    let render = render_tree(&projected, &config, wide());
    assert_eq!(
        render.rows[8..12],
        [
            "▎ │ main",
            "▎ ├╴! acme-api-lead",
            "▎ │   │ api",
            "▎ │   ╰╴⠄ acme-api-w1",
        ]
    );
}

#[test]
fn tree_ahead_behind_sits_at_the_right_end_of_the_branch_line() {
    let mut config = ClientShellConfig::from_config(&Config::default());
    let wide = || TreeRenderInput {
        width: 30,
        ..tall(200)
    };
    let render = render_tree(&git_snapshot(), &config, wide());
    assert_eq!(
        render.rows[2..4],
        ["▎ ⠹ herdr ▾", "▎ │ feat/sidebar-tree  ↑6"],
        "moved from the space row to its branch line"
    );
    assert_eq!(render.buffer[(23, 3)].fg, config.palette.overlay0, "dim");

    let mut projected = git_snapshot();
    projected.workspaces[0].git_ahead_behind = Some((0, 0));
    assert_eq!(
        render_tree(&projected, &config, wide()).rows[3],
        "▎ │ feat/sidebar-tree",
        "only when not zero"
    );

    // The main lead on another branch than the folder: the folder's counts do not fit it.
    let mut projected = git_snapshot();
    projected.panes[0].git_branch = Some("fix".into());
    assert_eq!(render_tree(&projected, &config, wide()).rows[3], "▎ │ fix");

    config.spaces.rows = vec![vec![crate::config::SpaceSidebarToken::Workspace]];
    assert_eq!(
        render_tree(&git_snapshot(), &config, wide()).rows[2],
        "▎ ⠹ herdr ▾            ↑6",
        "no branch line: it stays on the space row"
    );
}

#[test]
fn tree_marks_an_agent_that_needs_you_as_the_box_does() {
    let config = ClientShellConfig::from_config(&Config::default());
    let mut projected = picture_spaces();
    // acme's main lead asks; acme-web-lead's ask is marks alone, so it asks nothing.
    projected.agents[3].tokens = vec![("ask".into(), "merge it?".into())];
    projected.agents[6].tokens = vec![("ask".into(), "**".into())];
    let render = render_tree(&projected, &config, tall_panel(200));
    assert_eq!(
        render.rows[5..10],
        [
            "▎ ? acme ▾",
            "▎ ├╴! acme-api-lead",
            "▎ │   ╰╴⠄ acme-api-w1",
            "▎ ╰╴○ acme-web-lead",
            "",
        ],
        "Herdr's blocked state: `!`, in red; the space shows its main lead's own `?`"
    );
    assert_eq!(render.buffer[(2, 5)].fg, config.palette.yellow, "asks");
    assert_eq!(
        render.buffer[(4, 6)].fg,
        approval_colour(&config.palette),
        "needs you"
    );
    let boxed = panel(&render);
    assert!(boxed[0].starts_with("  ACTION REQUIRED "), "{boxed:?}");
    assert_eq!(
        boxed[1..3],
        ["  ? acme", "  ! ac…/acme-api-lead"],
        "the same two in the box"
    );
}

#[test]
fn tree_working_rows_show_no_time_in_their_first_minute() {
    let config = ClientShellConfig::from_config(&Config::default());
    // A client just started (or the agent just went back to work): seen working for 5s.
    let input = TreeRenderInput {
        working_for: &[("pane_1", 5), ("pane_2", 5), ("pane_3", 5)],
        ..tall(200)
    };
    let render = render_tree(&picture_spaces(), &config, input);
    assert_eq!(
        render.rows[2..5],
        ["▎ ⠹ herdr ▾", "▎ ├╴⠄ herdr-w1", "▎ ╰╴⠄ herdr-w2",],
        "under a minute: no time, on the space row and the rows under it"
    );
}

#[test]
fn tree_stage_and_time_show_only_when_they_fit_beside_the_whole_name() {
    let config = ClientShellConfig::from_config(&Config::default());
    let mut projected = single_agent_space();
    projected.workspaces[0].label = "oakwood-workspace".into();
    projected.workspaces[0].focused = false;
    let row = |projected: &ClientShellSnapshot, width| {
        let input = TreeRenderInput { width, ..tall(200) };
        render_tree(projected, &config, input).rows[2].clone()
    };
    assert_eq!(
        row(&projected, 27),
        "▎ ⠹ oakwood-workspace",
        "no room for the stage beside the whole name: the name wins"
    );
    assert_eq!(row(&projected, 33), "▎ ⠹ oakwood-workspace  Build");

    // A worker's time the same way.
    let mut two = picture_spaces();
    two.agents[1].name = Some("a-very-long-worker".into());
    let input = TreeRenderInput {
        working_for: &[("pane_2", 270)],
        width: 27,
        ..tall(200)
    };
    assert_eq!(
        render_tree(&two, &config, input).rows[3],
        "▎ ├╴⠄ a-very-long-worker",
        "the time drops before the name is cut"
    );
}

/// The last drawn column of the row holding `text`.
fn last_column(render: &TreeRender, text: &str) -> u16 {
    let y = render
        .rows
        .iter()
        .position(|row| row.contains(text))
        .unwrap_or_else(|| panic!("no row with {text:?}: {:#?}", render.rows));
    // Every row is ASCII or one column per char here.
    (render.rows[y].chars().count() - 1) as u16
}

#[test]
fn tree_and_box_facts_all_end_in_one_column() {
    let config = ClientShellConfig::from_config(&Config::default());
    let mut projected = picture_branches();
    projected.workspaces[0].git_ahead_behind = Some((3, 0));
    projected.agents[0].tokens = vec![("stage".into(), "build".into())];
    projected.agents[4].tokens = vec![("stage".into(), "plan".into())];
    projected.agents[6].agent_status = AgentStatus::Done;
    for width in [30, 36] {
        let input = TreeRenderInput {
            width,
            working_for: &[("pane_2", 120), ("pane_3", 300), ("pane_6", 60)],
            waiting_for: &[("pane_5", 60)],
            done_for: &[("pane_7", 180)],
            ..tall_panel(200)
        };
        let render = render_tree(&projected, &config, input);
        // Three columns left of the header's last icon: the rows' own `+` sits one column
        // in from it, and the facts end two columns before that.
        let column = render.hits.tree_fold_all.x - 3;
        for text in [
            "herdr ",
            "feat/sidebar-tree",
            "⠄ herdr-w1",
            "⠄ herdr-w2",
            "acme-api-lead ",
            "╰╴⠄ acme-api-w1",
            "! acme/acme-api-lead",
        ] {
            assert_eq!(
                last_column(&render, text),
                column,
                "{text:?} at {width} columns: {:#?}",
                render.rows
            );
        }
        // A lead's whole name before its time: at 30 columns the done lead has no time.
        match width {
            30 => assert!(render
                .rows
                .iter()
                .any(|row| row == "  ✓ acme/acme-web-lead")),
            _ => assert_eq!(last_column(&render, "acme/acme-web-lead"), column),
        }
    }
}

/// The column where `text` starts on the first row holding it.
fn first_column(render: &TreeRender, text: &str) -> u16 {
    let row = render
        .rows
        .iter()
        .find(|row| row.contains(text))
        .unwrap_or_else(|| panic!("no row with {text:?}: {:#?}", render.rows));
    let at = row.find(text).expect("text in its row");
    // Columns, not bytes: marks and connectors are wider in UTF-8.
    row[..at].chars().count() as u16
}

#[test]
fn tree_box_sits_on_the_trees_mark_and_name_columns() {
    let config = ClientShellConfig::from_config(&Config::default());
    for width in [30, 36] {
        let input = TreeRenderInput {
            width,
            ..box_input()
        };
        let render = render_tree(&box_snapshot(), &config, input);
        // The tree's own columns, from its rows: a worker's mark and a space's name.
        let mark = first_column(&render, "├");
        let name = first_column(&render, "herdr");
        assert_eq!(
            (mark, name),
            (2, 4),
            "the tree's columns, past the left padding"
        );
        for (text, column) in [
            ("ACTION REQUIRED", mark),
            ("? oakwood", mark),
            ("oakwood-workspace ", name),
            ("+1 more", name),
            ("COMPLETED", mark),
            ("acme/acme-we", name),
        ] {
            assert_eq!(
                first_column(&render, text),
                column,
                "{text:?} at {width} columns: {:#?}",
                render.rows
            );
        }
    }
}

/// `projected` with a plugin's pane (the herdr-sidebar explorer) split into `tab_id`,
/// docked left: first in the tab's layout order.
fn with_plugin_pane(mut projected: ClientShellSnapshot, tab_id: &str) -> ClientShellSnapshot {
    let mut explorer = tree_pane("pane_90", "ws_1", tab_id);
    explorer.label = Some("Sidebar".into());
    explorer.plugin_id = Some("herdr-sidebar".into());
    let at = projected
        .panes
        .iter()
        .position(|pane| pane.tab_id == tab_id)
        .expect("the tab has a pane");
    projected.panes.insert(at, explorer);
    projected
}

#[test]
fn tree_plugin_pane_beside_the_main_lead_is_no_row_and_the_space_stays_one_row() {
    let config = ClientShellConfig::from_config(&Config::default());
    let alone = lead_space(&[("herdr-lead", None)]);
    let with_explorer = with_plugin_pane(alone.clone(), "tab_1");
    let before = render_tree(&alone, &config, tall(200));
    let after = render_tree(&with_explorer, &config, tall(200));
    assert_eq!(
        after.rows, before.rows,
        "the explorer is not a row and the lead still folds into the space row"
    );
    assert_eq!(
        after
            .hits
            .tree_space_agents
            .iter()
            .map(|(_, pane_id)| pane_id.as_str())
            .collect::<Vec<_>>(),
        ["pane_1"],
        "a click on the space row goes to the lead, not the explorer"
    );
}

#[test]
fn tree_plugin_pane_in_a_workers_tab_is_no_row() {
    let config = ClientShellConfig::from_config(&Config::default());
    let workers = lead_space(&[("herdr-lead", None), ("herdr-w1", Some("pane_1"))]);
    let with_explorer = with_plugin_pane(workers.clone(), "tab_2");
    let before = render_tree(&workers, &config, tall(200));
    let after = render_tree(&with_explorer, &config, tall(200));
    assert_eq!(
        after.rows, before.rows,
        "the worker stays one row, without the explorer under it"
    );
}

#[test]
fn tree_shell_rows_show_a_blue_dot_mark() {
    let config = ClientShellConfig::from_config(&Config::default());
    let render = render_tree(&tree_snapshot(), &config, tall(200));
    assert_eq!(
        render.rows[3..5],
        ["▎ ├╴· repo", "▎ ╰╴· logs"],
        "a shell's mark is a dot, where an agent shows its state"
    );
    assert_eq!(
        render.buffer[(4, 3)].fg,
        shell_mark_colour(&config.palette),
        "shell mark"
    );
    assert_eq!(
        render.buffer[(4, 4)].fg,
        shell_mark_colour(&config.palette),
        "shell mark"
    );
}

#[test]
fn tree_workers_of_the_main_lead_stay_normal_weight_when_focused() {
    let mut projected = lead_space(&[("herdr-lead", None), ("herdr-w1", Some("pane_1"))]);
    projected.panes[1].focused = true;
    projected.agents[1].focused = true;
    let config = ClientShellConfig::from_config(&Config::default());
    let render = render_tree(&projected, &config, tall(0));
    assert_eq!(render.rows[3], "▎ ╰╴○ herdr-w1");
    assert!(
        !render.buffer[(6, 3)].modifier.contains(Modifier::BOLD),
        "a worker is normal weight, focused or not"
    );
    assert!(
        render.buffer[(4, 2)].modifier.contains(Modifier::BOLD),
        "the lead, on the space row, is bold"
    );
}

#[test]
fn tree_fold_arrows_follow_the_name() {
    let config = ClientShellConfig::from_config(&Config::default());
    let open = render_tree(&tree_snapshot(), &config, tall(200));
    assert_eq!(
        open.rows[2..9],
        [
            "▎ ⠹ herdr ▾",
            "▎ ├╴· repo",
            "▎ ╰╴· logs",
            "▎ ! dotworks ▾",
            "▎ ╰╴✓ claude",
            "",
            ""
        ],
        "the arrow one space after the name"
    );
    assert_eq!(
        open.buffer[(10, 2)].fg,
        config.palette.overlay1,
        "arrow color"
    );
    assert_eq!(
        open.hits.workspaces[0]
            .group_toggle
            .as_ref()
            .map(|(rect, _)| rect.x),
        Some(10),
        "the space's arrow cell folds it"
    );

    // A tab of two shells keeps its row; its arrow keeps the mark column.
    let mut split = tree_snapshot();
    push_shell_tab(&mut split, "ws_2", 6, "notes");
    split.panes.push(tree_pane("pane_7", "ws_2", "tab_6"));
    let open = render_tree(&split, &config, tall(200));
    assert_eq!(open.rows[7], "▎ ╰╴  notes ▾");
    assert_eq!(
        open.buffer[(12, 7)].fg,
        config.palette.overlay1,
        "arrow color"
    );
    assert_eq!(
        open.hits.tree_tab_folds[0].0.x, 12,
        "the tab's arrow cell folds it"
    );

    let tab_folded = TreeRenderInput {
        folded: &["tab_6"],
        ..tall(200)
    };
    let tab_folded = render_tree(&split, &config, tab_folded);
    assert_eq!(tab_folded.rows[7], "▎ ╰╴  notes ▸");
    let space_folded = TreeRenderInput {
        folded: &["ws_2"],
        ..tall(200)
    };
    let space_folded = render_tree(&tree_snapshot(), &config, space_folded);
    assert!(
        space_folded.rows[5].starts_with("▎ ! dotworks ▸"),
        "{:?}",
        space_folded.rows[5]
    );

    let lone = render_tree(&single_agent_space(), &config, tall(200));
    assert!(
        !lone.rows[2].contains('▾') && !lone.rows[2].contains('▸'),
        "no arrow where nothing folds: {:?}",
        lone.rows[2]
    );
}

#[test]
fn tree_branch_lines_are_thick_and_keep_names_aligned() {
    let config = ClientShellConfig::from_config(&Config::default());
    let render = render_tree(&tree_snapshot(), &config, tall(200));
    assert_eq!(
        render.rows[3..9],
        [
            "▎ ├╴· repo",
            "▎ ╰╴· logs",
            "▎ ! dotworks ▾",
            "▎ ╰╴✓ claude",
            "",
            ""
        ],
        "`│ ├╴ ╰╴` in the columns `│ ├ └` and the space after them took"
    );
    assert_eq!(render.buffer[(3, 3)].fg, config.palette.overlay0, "dim");
}

#[test]
fn tree_tab_with_one_agent_has_no_tab_row_and_its_other_panes_nest_under_the_agent() {
    let config = ClientShellConfig::from_config(&Config::default());
    let mut workers = lead_space(&[("herdr-lead", None), ("herdr-w1", Some("pane_1"))]);
    workers.panes.push(tree_pane("pane_3", "ws_1", "tab_2"));
    let render = render_tree(&workers, &config, tall(200));
    assert_eq!(
        render.rows[3..5],
        ["▎ ╰╴○ herdr-w1 ▾", "▎     ╰╴· repo"],
        "the worker where its tab row was, its shell under it"
    );
    assert!(
        render.targets.iter().any(|(rect, target)| rect.y == 3
            && *target
                == TreeTarget::AgentTab {
                    tab_id: "tab_2".into(),
                    pane_id: "pane_2".into(),
                }),
        "a click on the row focuses the agent: {:?}",
        render.targets
    );
    let folded = TreeRenderInput {
        folded: &["tab_2"],
        ..tall(200)
    };
    assert_eq!(
        render_tree(&workers, &config, folded).rows[3..5],
        ["▎ ╰╴○ herdr-w1 ▸", ""],
        "its arrow folds the panes under it"
    );

    // The main lead is the space row: its tab's other panes hang under the space.
    let main = render_tree(&tree_snapshot(), &config, tall(200));
    assert_eq!(main.rows[2..5], ["▎ ⠹ herdr ▾", "▎ ├╴· repo", "▎ ╰╴· logs"],);
}

#[test]
fn tree_box_names_the_main_lead_by_its_space_with_a_plugin_pane_beside_it() {
    let config = ClientShellConfig::from_config(&Config::default());
    let mut alone = lead_space(&[("herdr-lead", None)]);
    alone.agents[0].agent_status = AgentStatus::Blocked;
    let with_explorer = with_plugin_pane(alone.clone(), "tab_1");
    assert_eq!(
        panel(&render_tree(&with_explorer, &config, tall_panel(200))),
        panel(&render_tree(&alone, &config, tall_panel(200))),
        "the explorer docked left does not make the lead a worker in the box"
    );
}

#[test]
fn tree_rows_have_one_column_of_padding_on_each_side() {
    let config = ClientShellConfig::from_config(&Config::default());
    let render = render_tree(&tree_snapshot(), &config, tall(200));
    assert_eq!(
        render.rows[2..5],
        ["▎ ⠹ herdr ▾", "▎ ├╴· repo", "▎ ╰╴· logs"],
        "the mark two columns in; the `+` two columns before the divider"
    );
    let bg = |x: u16, y: u16| render.buffer[(x, y)].bg;
    let band = bg(12, 2);
    assert_eq!(
        [bg(1, 2), bg(23, 2)],
        [band, band],
        "the band's own columns"
    );
    assert_ne!(bg(0, 2), band, "one column of padding on the left");
    assert_ne!(bg(24, 2), band, "and one on the right");
}

#[test]
fn tree_gap_rows_next_to_the_current_space_stay_plain_at_every_gap() {
    let mut config = ClientShellConfig::from_config(&Config::default());
    config.spaces.row_gap = 1;
    let render = render_tree(&three_spaces(1), &config, tall(200));
    assert_eq!(
        render.rows[2..7]
            .iter()
            .map(|row| row.trim_end())
            .collect::<Vec<_>>(),
        ["▎ ⠹ herdr       Build", "", "▎   notes", "", "▎   docs"],
        "plain gap rows beside the current space; each space's bar on its own rows only"
    );
    let cell = |x: u16, y: u16| &render.buffer[(x, y)];
    for y in [3, 5] {
        for x in [0, 1, 12, 23] {
            assert_eq!(cell(x, y).bg, cell(12, 2).bg, "row {y} col {x}: plain");
        }
    }
    assert_eq!(render.hits.workspaces[1].rect.y, 4);

    // Two blank rows between spaces, or none.
    config.spaces.row_gap = 2;
    let wide = render_tree(&three_spaces(1), &config, tall(200));
    assert_eq!(
        wide.rows[2..9]
            .iter()
            .map(|row| row.trim_end())
            .collect::<Vec<_>>(),
        [
            "▎ ⠹ herdr       Build",
            "",
            "",
            "▎   notes",
            "",
            "",
            "▎   docs"
        ],
    );
    config.spaces.row_gap = 0;
    let tight = render_tree(&three_spaces(1), &config, tall(200));
    assert_eq!(
        tight.rows[2..5]
            .iter()
            .map(|row| row.trim_end())
            .collect::<Vec<_>>(),
        ["▎ ⠹ herdr       Build", "▎   notes", "▎   docs"],
        "row_gap 0: no gap rows"
    );
}

/// `two_single_agent_spaces` plus `docs`, with `focused` the current space, gap one row.
fn three_spaces(focused: usize) -> ClientShellSnapshot {
    let mut three = two_single_agent_spaces();
    let mut docs = three.workspaces[0].clone();
    docs.workspace_id = "ws_3".into();
    docs.label = "docs".into();
    three.workspaces.push(docs);
    for (index, workspace) in three.workspaces.iter_mut().enumerate() {
        workspace.focused = index == focused;
    }
    three
}

#[test]
fn tree_first_and_last_space_sit_one_plain_row_under_the_header_and_above_the_end() {
    let mut config = ClientShellConfig::from_config(&Config::default());
    config.spaces.row_gap = 1;
    let rows = |focused| {
        render_tree(&three_spaces(focused), &config, tall(200)).rows[1..9]
            .iter()
            .map(|row| row.trim_end().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        rows(0),
        [
            "",
            "▎ ⠹ herdr       Build",
            "",
            "▎   notes",
            "",
            "▎   docs",
            "",
            "",
        ],
        "the first space current: one plain row under the header"
    );
    assert_eq!(
        rows(2),
        [
            "",
            "▎ ⠹ herdr       Build",
            "",
            "▎   notes",
            "",
            "▎   docs",
            "",
            "",
        ],
        "the last space current: a plain gap row under it too"
    );
}

fn nerd_config() -> ClientShellConfig {
    let mut config = ClientShellConfig::from_config(&Config::default());
    config.sidebar_tree_icons = crate::config::SidebarTreeIconsConfig::Nerd;
    config
}

#[test]
fn tree_branch_line_has_no_blank_row_under_it() {
    let config = ClientShellConfig::from_config(&Config::default());
    let open = render_tree(&git_snapshot(), &config, tall(200));
    assert_eq!(open.rows[3], "▎ │ feat/sidebar-… ↑6");
    assert!(
        open.rows[4].starts_with("▎ ├╴"),
        "the rows follow the line at once: {:#?}",
        open.rows
    );

    let folded = TreeRenderInput {
        folded: &["ws_1"],
        ..tall(200)
    };
    let folded = render_tree(&git_snapshot(), &config, folded);
    assert!(
        folded.rows[4].starts_with("▎ ! dotworks"),
        "folded: no extra row: {:#?}",
        folded.rows
    );

    let mut lone = single_agent_space();
    lone.workspaces[0].branch = Some("main".into());
    let lone = render_tree(&lone, &config, tall(200));
    assert_eq!(
        [lone.rows[3].as_str(), lone.rows[4].as_str()],
        ["▎   main", ""],
        "nothing under the branch: no extra row"
    );
}

#[test]
fn tree_lines_are_thin_with_a_rounded_last_corner_in_both_icon_sets() {
    let projected = branch_marks_snapshot();
    for (config, shell) in [
        (ClientShellConfig::from_config(&Config::default()), "·"),
        (nerd_config(), "\u{e795}"),
    ] {
        let render = render_tree(&projected, &config, tall(200));
        let text = render.rows[2..].join("\n");
        assert!(
            !text.contains(['┃', '┣', '┗', '━']),
            "no thick lines:\n{text}"
        );
        assert_eq!(
            render.rows[4],
            format!("▎ ├╴{shell} tab 2"),
            "a row with rows after it"
        );
        assert!(
            render.rows[7].starts_with("▎ ╰╴! codex "),
            "the last row's rounded corner: {:?}",
            render.rows[7]
        );
        assert!(
            render.rows[8].starts_with("▎     │ ") && render.rows[8].ends_with("fix"),
            "a nested branch line: {:?}",
            render.rows[8]
        );
        assert_eq!(
            render.rows[9],
            format!("▎     ╰╴{shell} repo"),
            "nested, names in place"
        );
        assert!(render.rows[3].starts_with("▎ │ "), "the space line's guide");
        assert_eq!(
            render.buffer[(2, 4)].fg,
            config.palette.overlay0,
            "colour unchanged"
        );
    }
}

#[test]
fn tree_branch_lines_show_the_git_branch_glyph_and_an_upright_name_in_the_nerd_set() {
    let nerd = nerd_config();
    let render = render_tree(&git_snapshot(), &nerd, tall(200));
    assert!(
        render.rows[3].starts_with("▎ │ \u{f418} feat/"),
        "{:?}",
        render.rows[3]
    );
    for (x, what) in [(4, "glyph"), (6, "name")] {
        let cell = &render.buffer[(x, 3)];
        assert_eq!(cell.fg, nerd.palette.overlay0, "dim {what}");
        assert!(!cell.modifier.contains(Modifier::ITALIC), "upright {what}");
    }
    // A pane's own branch line too.
    let render = render_tree(&branch_marks_snapshot(), &nerd, tall(200));
    assert!(
        render.rows.iter().any(|row| row == "▎     │ \u{f418} fix"),
        "{:#?}",
        render.rows
    );

    let plain = ClientShellConfig::from_config(&Config::default());
    let render = render_tree(&git_snapshot(), &plain, tall(200));
    assert!(
        render.rows[3].starts_with("▎ │ feat/"),
        "{:?}",
        render.rows[3]
    );
    assert!(
        !render.buffer[(4, 3)].modifier.contains(Modifier::ITALIC),
        "plain keeps its upright name"
    );
}

#[test]
fn tree_fold_arrows_are_octicon_chevrons_in_the_nerd_set() {
    let config = nerd_config();
    let open = render_tree(&tree_snapshot(), &config, tall(200));
    assert!(
        open.rows[2].contains("herdr \u{f47c}"),
        "{:?}",
        open.rows[2]
    );
    let folded = TreeRenderInput {
        folded: &["ws_2"],
        ..tall(200)
    };
    let folded = render_tree(&tree_snapshot(), &config, folded);
    assert!(
        folded.rows[5].contains("dotworks \u{f460}"),
        "{:?}",
        folded.rows[5]
    );
    let x = folded.rows[5]
        .chars()
        .position(|ch| ch == '\u{f460}')
        .expect("chevron") as u16;
    assert_eq!(folded.buffer[(x, 5)].fg, config.palette.overlay1, "grey");
    assert_eq!(
        folded.hits.workspaces[1]
            .group_toggle
            .as_ref()
            .map(|(cell, _)| cell.x),
        Some(x),
        "the chevron's cell folds"
    );
}

#[test]
fn tree_space_plus_shows_only_under_the_mouse() {
    let config = ClientShellConfig::from_config(&Config::default());
    let calm = render_tree(&tree_snapshot(), &config, tall(200));
    assert!(
        !calm.rows[2].contains('+'),
        "the current space shows no + without the mouse: {:?}",
        calm.rows[2]
    );
    assert!(calm.hits.tree_new_tab.is_empty(), "and no click cell");
    let hovered = TreeRenderInput {
        hovered: Some("ws_2"),
        ..tall(200)
    };
    let hovered = render_tree(&tree_snapshot(), &config, hovered);
    assert!(hovered.rows[5].ends_with('+'), "{:?}", hovered.rows[5]);
    assert!(!hovered.rows[2].contains('+'), "{:?}", hovered.rows[2]);
    assert_eq!(
        hovered
            .hits
            .tree_new_tab
            .iter()
            .map(|(_, target)| target.clone())
            .collect::<Vec<_>>(),
        [TreeTarget::NewTab("ws_2".into())]
    );
}

#[test]
fn tree_shell_mark_is_a_terminal_glyph_in_the_nerd_set() {
    let config = nerd_config();
    let render = render_tree(&tree_snapshot(), &config, tall(200));
    let (y, row) = render
        .rows
        .iter()
        .enumerate()
        .find(|(_, row)| row.starts_with("▎ ├╴"))
        .expect("a shell row");
    let x = row
        .chars()
        .position(|ch| ch == '\u{e795}')
        .expect("terminal glyph") as u16;
    assert_eq!(
        render.buffer[(x, y as u16)].fg,
        shell_mark_colour(&config.palette),
        "shell mark"
    );
    assert!(!row.contains('·'), "{row:?}");
}

#[test]
fn tree_shells_are_named_by_their_folder() {
    let config = ClientShellConfig::from_config(&Config::default());
    let name = |pane: fn(&mut ClientShellPane), home: Option<&'static str>| {
        let mut projected = tree_snapshot();
        pane(&mut projected.panes[2]);
        let input = TreeRenderInput { home, ..tall(200) };
        render_tree(&projected, &config, input).rows[3].clone()
    };
    assert_eq!(
        name(|_| {}, None),
        "▎ ├╴· repo",
        "the last part of its folder"
    );
    assert_eq!(
        name(|pane| pane.foreground_cwd = Some("/repo/src".into()), None),
        "▎ ├╴· src",
        "where its program runs, first"
    );
    assert_eq!(
        name(|pane| pane.cwd = Some("/home/me".into()), Some("/home/me")),
        "▎ ├╴· ~",
        "home"
    );
    assert_eq!(
        name(|pane| pane.cwd = Some("/".into()), None),
        "▎ ├╴· /",
        "root"
    );
    assert_eq!(
        name(|pane| pane.label = Some("web".into()), None),
        "▎ ├╴· web",
        "a Rename pane name wins"
    );
    assert_eq!(
        name(|pane| pane.cwd = None, None),
        "▎ ├╴· pane 2",
        "no folder: its place in the split"
    );
}

#[test]
fn tree_space_row_takes_the_focus_fill_when_its_main_lead_is_focused() {
    let config = ClientShellConfig::from_config(&Config::default());
    let focus = config.palette.selection_bg;
    // The current space's other rows sit on its block, not the focus fill.
    let block = ratatui::style::Color::Rgb(30, 30, 44);
    // tree_snapshot focuses pane_1, herdr's main lead.
    let render = render_tree(&tree_snapshot(), &config, tall(0));
    assert_eq!(
        render.buffer[(12, 2)].bg,
        focus,
        "the space row stands for it"
    );
    assert_eq!(
        render.buffer[(12, 3)].bg,
        block,
        "the rest has only the block"
    );

    let mut shell_focused = tree_snapshot();
    for pane in &mut shell_focused.panes {
        pane.focused = pane.pane_id == "pane_2";
    }
    shell_focused.agents[0].focused = false;
    let render = render_tree(&shell_focused, &config, tall(0));
    assert_eq!(
        render.buffer[(12, 2)].bg,
        block,
        "control: another pane focused"
    );
    assert_eq!(render.buffer[(12, 3)].bg, focus);
}

#[test]
fn tree_space_name_is_bright_only_while_the_pane_its_row_stands_for_is_focused() {
    let config = ClientShellConfig::from_config(&Config::default());
    let palette = &config.palette;
    let block = ratatui::style::Color::Rgb(30, 30, 44);
    // A space row's name starts in column 4, after the bar and its mark.
    let name = |render: &TreeRender, y: u16| {
        let cell = &render.buffer[(4, y)];
        (cell.fg, cell.modifier.contains(Modifier::BOLD))
    };
    // tree_snapshot focuses pane_1, the pane herdr's space row stands for.
    let render = render_tree(&tree_snapshot(), &config, tall(0));
    assert!(render.rows[2].contains("herdr"), "{:#?}", render.rows);
    assert_eq!(name(&render, 2), (palette.text, true), "its own pane");

    let mut child = tree_snapshot();
    for pane in &mut child.panes {
        pane.focused = pane.pane_id == "pane_2";
    }
    child.agents[0].focused = false;
    let render = render_tree(&child, &config, tall(0));
    assert_eq!(
        name(&render, 2),
        (palette.subtext0, true),
        "a child focused"
    );
    assert_eq!(render.buffer[(12, 2)].bg, block, "the tint stays");
    assert_eq!(render.buffer[(0, 2)].symbol(), "▎", "the bar stays");
    // Another space's name, whatever is focused.
    let other = (0..render.rows.len() as u16)
        .skip(3)
        .find(|y| render.rows[usize::from(*y)].contains("dotworks"))
        .unwrap_or_else(|| panic!("{:#?}", render.rows));
    assert_eq!(
        name(&render, other),
        (palette.subtext0, true),
        "another space"
    );

    // A space whose row stands for a shell follows the same rule.
    let mut shell = shell_space(false);
    let first = shell.panes[0].pane_id.clone();
    for (own, pane) in [(true, first.as_str()), (false, "")] {
        for each in &mut shell.panes {
            each.focused = if own {
                each.pane_id == pane
            } else {
                each.pane_id != first
            };
        }
        let render = render_tree(&shell, &config, tall(0));
        let colour = if own { palette.text } else { palette.subtext0 };
        assert_eq!(
            name(&render, 2),
            (colour, true),
            "shell, own {own}: {:#?}",
            render.rows
        );
    }
}

#[test]
fn tree_space_branch_line_fills_with_the_space_row_when_its_pane_is_focused() {
    let config = ClientShellConfig::from_config(&Config::default());
    let focus = config.palette.selection_bg;
    let block = ratatui::style::Color::Rgb(30, 30, 44);
    let fills = |render: &TreeRender, rows: std::ops::Range<u16>| {
        rows.map(|y| render.buffer[(12, y)].bg).collect::<Vec<_>>()
    };
    // git_snapshot focuses pane_1, the pane herdr's space row stands for.
    let render = render_tree(&git_snapshot(), &config, tall(0));
    assert!(
        render.rows[3].contains("feat/sidebar"),
        "{:#?}",
        render.rows
    );
    assert_eq!(
        fills(&render, 2..5),
        [focus, focus, block],
        "the space row and its branch line, then the block"
    );

    let mut child = git_snapshot();
    for pane in &mut child.panes {
        pane.focused = pane.pane_id == "pane_2";
    }
    child.agents[0].focused = false;
    let render = render_tree(&child, &config, tall(0));
    assert_eq!(
        fills(&render, 2..5),
        [block, block, focus],
        "a child focused: the branch line stays on the block"
    );

    let folded = render_tree(
        &git_snapshot(),
        &config,
        TreeRenderInput {
            folded: &["ws_1"],
            ..tall(0)
        },
    );
    assert!(folded.rows[2].contains('▸'), "{:#?}", folded.rows);
    assert_eq!(fills(&folded, 2..4), [focus, focus], "folded");

    let mut single = single_agent_space();
    single.workspaces[0].branch = Some("master".into());
    for pane in &mut single.panes {
        pane.focused = true;
    }
    let render = render_tree(&single, &config, tall(0));
    assert!(render.rows[3].contains("master"), "{:#?}", render.rows);
    assert_eq!(fills(&render, 2..4), [focus, focus], "a one-row space");

    let mut dotworks = dotworks_space(&["master", "master"]);
    dotworks.panes[0].focused = true;
    let render = render_tree(&dotworks, &config, tall(0));
    assert!(render.rows[3].contains("master"), "{:#?}", render.rows);
    assert_eq!(
        fills(&render, 2..5),
        [focus, focus, block],
        "a lifted branch line"
    );
}

#[test]
fn tree_space_branch_line_takes_a_key_pick_with_its_space_row() {
    // A child focused, so only the pick fills the space row and its branch line.
    let mut projected = git_snapshot();
    for pane in &mut projected.panes {
        pane.focused = pane.pane_id == "pane_2";
    }
    projected.agents[0].focused = false;
    let mut state = ClientShellState::new(tree_config(true));
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.compose(106, 30).expect("first frame");
    let row = state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.workspace_id == "ws_1")
        .expect("space row")
        .rect;
    let endpoint = state.active_endpoint_id.clone();
    state.mode = ClientShellMode::Navigate;
    state.navigate_workspace_id = state.navigation_target(&endpoint, "ws_1");
    let palette = state.config.palette.clone();
    let frame = state.compose(106, 30).expect("picked frame");
    let buffer = frame.to_ratatui_buffer().expect("frame should reconstruct");
    let pick = crate::client::shell::sidebar::workspace_selection_background(&palette);
    for y in [row.y, row.y + 1] {
        assert_eq!(buffer[(row.x + 10, y)].bg, pick, "row {y}");
    }
}

#[test]
fn tree_shell_folder_names_draw_no_control_characters() {
    let config = ClientShellConfig::from_config(&Config::default());
    let mut projected = tree_snapshot();
    projected.panes[2].cwd = Some("/srv/we\u{1b}]0;x\u{7}b".into());
    let render = render_tree(&projected, &config, tall(200));
    assert_eq!(
        render.rows[3], "▎ ├╴· we]0;xb",
        "a remote folder name cannot send escape sequences"
    );
}

#[test]
fn tree_header_leaves_out_the_board_icon_below_its_width_but_keeps_the_others() {
    let config = ClientShellConfig::from_config(&Config::default());
    // The sidebar's last column is its border, so the content is one column narrower.
    for content_width in 15..=18 {
        let render = render_tree(
            &tree_snapshot(),
            &config,
            TreeRenderInput {
                width: content_width + 1,
                ..tall(200)
            },
        );
        assert!(render.hits.tree_task_board.is_empty(), "{content_width}");
        assert!(!render.hits.tree_search.is_empty(), "{content_width}");
        assert!(!render.hits.tree_fold_all.is_empty(), "{content_width}");
        assert!(
            !render.rows[0].contains('☑'),
            "{content_width}: {:?}",
            render.rows[0]
        );
        assert!(
            render.rows[0].starts_with("  SPACES "),
            "{:?}",
            render.rows[0]
        );
    }
    let render = render_tree(
        &tree_snapshot(),
        &config,
        TreeRenderInput {
            width: 20,
            ..tall(200)
        },
    );
    assert_eq!(render.rows[0], "  SPACES ▽ + ☑ / ▾");
    // One column narrower than search and fold all need, `+` alone keeps a gap after the
    // word.
    let render = render_tree(
        &tree_snapshot(),
        &config,
        TreeRenderInput {
            width: 15,
            ..tall(200)
        },
    );
    assert!(render.hits.tree_search.is_empty());
    assert_eq!(render.rows[0], "  SPACES    +");
}

/// The sidebar's colours: yellow works and waits on you, teal is done, green is idle.
fn working_colour(palette: &Palette) -> ratatui::style::Color {
    palette.yellow
}

fn needs_you_colour(palette: &Palette) -> ratatui::style::Color {
    palette.yellow
}

/// The yes/no `!` (an agent blocked with no ask) and a folded blocked tab.
fn approval_colour(palette: &Palette) -> ratatui::style::Color {
    palette.red
}

fn done_colour(palette: &Palette) -> ratatui::style::Color {
    palette.teal
}

fn idle_colour(palette: &Palette) -> ratatui::style::Color {
    palette.green
}

/// A shell's mark is a kind, not a state, so in the shell's name colour.
fn shell_mark_colour(palette: &Palette) -> ratatui::style::Color {
    palette.subtext0
}

#[test]
fn tree_sidebar_colours_say_one_meaning_each() {
    let config = ClientShellConfig::from_config(&Config::default());
    let palette = &config.palette;
    let mut projected = tree_snapshot();
    // herdr: its main lead builds with a working `w1`; an idle agent in its second pane;
    // an unnamed split tab of two shells.
    projected.agents[0].tokens = vec![("stage".into(), "build · w1".into())];
    projected.tabs.push(tree_tab("tab_4", "ws_1", 3, None));
    projected
        .tabs
        .push(tree_tab("tab_5", "ws_1", 4, Some("notes")));
    projected.panes.extend([
        tree_pane("pane_6", "ws_1", "tab_4"),
        tree_pane("pane_7", "ws_1", "tab_4"),
        tree_pane("pane_8", "ws_1", "tab_5"),
        tree_pane("pane_9", "ws_1", "tab_5"),
    ]);
    let mut worker = tree_agent(&projected.panes[3], "claude", AgentStatus::Working);
    worker.name = Some("w1".into());
    let idle = tree_agent(&projected.panes[2], "codex", AgentStatus::Idle);
    projected.agents.extend([worker, idle]);
    let render = render_tree(
        &projected,
        &config,
        TreeRenderInput {
            height: 30,
            width: 34,
            ..tall(200)
        },
    );
    let at = |text: &str| {
        let y = render
            .rows
            .iter()
            .position(|row| row.contains(text))
            .unwrap_or_else(|| panic!("{text} drawn: {:#?}", render.rows));
        (column_of(&render.rows[y], text), y as u16)
    };
    let cell = |(x, y): (u16, u16)| &render.buffer[(x, y)];

    let (x, y) = at("herdr");
    assert_eq!(
        cell((x - 2, y)).fg,
        working_colour(palette),
        "working mark: {:#?}",
        render.rows
    );
    let build = cell(at("Build"));
    assert_eq!(
        build.fg, palette.yellow,
        "phase word keeps its stage colour"
    );
    assert!(
        !render.rows[y as usize].contains("· w1"),
        "no agent name after the word: {:#?}",
        render.rows
    );
    let (x, y) = at("codex");
    let idle_row = &render.rows[y as usize];
    assert!(
        idle_row.contains('╴'),
        "the idle agent's own row: {idle_row}"
    );
    assert_eq!(cell((x - 2, y)).fg, idle_colour(palette), "idle mark");
    assert_eq!(cell(at("tab 3")).fg, palette.overlay0, "tab N label");
    assert_eq!(cell(at("notes")).fg, palette.overlay1, "a tab's own label");
    let (x, y) = at("dotworks");
    assert_eq!(cell((x - 2, y)).fg, approval_colour(palette), "approval !");
    let (x, y) = at("✓ claude");
    assert_eq!(cell((x, y)).fg, done_colour(palette), "done mark");

    let heading = cell(at("ACTION REQUIRED"));
    assert_eq!(heading.fg, needs_you_colour(palette), "ACTION REQUIRED");
    assert!(heading.modifier.contains(Modifier::BOLD));
    assert_eq!(cell(at("COMPLETED")).fg, done_colour(palette), "COMPLETED");
    let (_, panel) = at("ACTION REQUIRED");
    let bang = column_of(&render.rows[panel as usize + 1], "!");
    assert_eq!(
        cell((bang, panel + 1)).fg,
        approval_colour(palette),
        "the panel's approval !"
    );
    // The lower lists' marks carry the state; their names read as a worker row's does.
    let worker_name = cell(at("codex"));
    assert_eq!(worker_name.fg, palette.subtext0, "a worker row's name");
    assert!(!worker_name.modifier.contains(Modifier::BOLD));
    let (_, completed) = at("COMPLETED");
    for (y, mark, colour) in [
        (panel + 1, "!", approval_colour(palette)),
        (completed + 1, "✓", done_colour(palette)),
    ] {
        let row = &render.rows[usize::from(y)];
        let x = column_of(row, mark);
        assert_eq!(cell((x, y)).fg, colour, "{row}: its mark");
        let name = cell((x + 2, y));
        assert_eq!(
            (name.fg, name.modifier),
            (worker_name.fg, worker_name.modifier),
            "{row}: its name"
        );
    }

    // A shell's mark, in the split tab.
    assert_eq!(
        cell(at("· repo")).fg,
        shell_mark_colour(palette),
        "shell mark"
    );
}

/// A right click on `rect`, the menu left open; its labels.
fn menu_labels(state: &mut ClientShellState, rect: Rect) -> Vec<&'static str> {
    state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Right),
        column: rect.x + 2,
        row: rect.y,
        modifiers: KeyModifiers::empty(),
    })]);
    match state.overlay.as_ref() {
        Some(ClientShellOverlay::ContextMenu(menu)) => {
            menu.items().iter().map(|item| item.label).collect()
        }
        other => panic!("no menu: {other:?}"),
    }
}

/// The open menu's item `label`, chosen.
fn choose(state: &mut ClientShellState, label: &str) -> ClientShellInput {
    let Some(ClientShellOverlay::ContextMenu(menu)) = state.overlay.as_ref() else {
        panic!("no menu");
    };
    let index = menu
        .items()
        .iter()
        .position(|item| item.label == label)
        .unwrap_or_else(|| panic!("no {label:?}"));
    let mut outcome = ClientShellInput::default();
    state.activate_context_menu_item(index, &mut outcome);
    outcome
}

fn sent_methods(outcome: &ClientShellInput) -> Vec<&crate::api::schema::Method> {
    outcome
        .actions
        .iter()
        .filter_map(|action| match action {
            ClientShellAction::Endpoint { request, .. } => Some(&request.method),
            _ => None,
        })
        .collect()
}

/// A row menu's first group: what it adds.
const ADDS: [&str; 3] = ["New tab", "Split right", "Split down"];
/// A pane's own items on a row menu, for a pane other than the focused one.
const OTHER_PANE: [&str; 5] = [
    "Rename pane",
    "Swap with focused pane",
    "Zoom",
    "Send right-clicks to pane",
    "Close pane",
];
/// The same for the focused pane: no swap.
const FOCUSED_PANE: [&str; 4] = [
    "Rename pane",
    "Zoom",
    "Send right-clicks to pane",
    "Close pane",
];

fn row_menu(groups: &[&[&'static str]]) -> Vec<&'static str> {
    groups
        .iter()
        .flat_map(|group| group.iter().copied())
        .collect()
}

/// `one_shell_space` with a second tab, its space git or not.
fn shell_space(git: bool) -> ClientShellSnapshot {
    let mut projected = one_shell_space();
    projected.workspaces[0].branch = git.then(|| "main".to_owned());
    projected.workspaces[0].worktree = None;
    push_shell_tab(&mut projected, "ws_1", 2, "scratch");
    projected
}

fn space_row_rect(state: &ClientShellState, workspace_id: &str) -> Rect {
    state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.workspace_id == workspace_id)
        .expect("space row")
        .rect
}

#[test]
fn a_shell_space_rows_menu_adds_then_holds_its_pane_then_its_space() {
    let mut state = ClientShellState::new(tree_config(true));
    show(&mut state, shell_space(false));
    let row = space_row_rect(&state, "ws_1");
    assert_eq!(
        menu_labels(&mut state, row),
        row_menu(&[&ADDS, &FOCUSED_PANE, &["Rename space", "Close space"]])
    );
    state.overlay = None;
    show(&mut state, shell_space(true));
    let row = space_row_rect(&state, "ws_1");
    assert_eq!(
        menu_labels(&mut state, row),
        row_menu(&[
            &ADDS,
            &FOCUSED_PANE,
            &[
                "Rename space",
                "Close space",
                "New worktree",
                "Open worktree..."
            ]
        ]),
        "a git space keeps its worktree items"
    );
}

#[test]
fn an_agent_space_rows_menu_is_the_same_three_groups() {
    let mut state = ClientShellState::new(tree_config(true));
    let mut projected = single_agent_space();
    push_shell_tab(&mut projected, "ws_1", 2, "logs");
    show(&mut state, projected);
    let row = space_row_rect(&state, "ws_1");
    let labels = menu_labels(&mut state, row);
    assert_eq!(&labels[..3], ADDS, "{labels:?}");
    assert!(labels[3..].starts_with(&["Rename pane"]), "{labels:?}");
    assert!(
        labels.ends_with(&["Close pane", "Rename space", "Close space"]),
        "{labels:?}"
    );
}

#[test]
fn a_space_rows_menu_acts_on_its_pane_and_its_space() {
    let mut state = ClientShellState::new(tree_config(true));
    show(&mut state, shell_space(false));
    let row = space_row_rect(&state, "ws_1");
    menu_labels(&mut state, row);
    let outcome = choose(&mut state, "Split right");
    assert!(
        matches!(
            sent_method(&outcome),
            crate::api::schema::Method::PaneSplit(params)
                if params.target_pane_id.as_deref() == Some("pane_1")
                    && params.direction == crate::api::schema::SplitDirection::Right
        ),
        "{:?}",
        sent_method(&outcome)
    );
    menu_labels(&mut state, row);
    choose(&mut state, "Rename space");
    assert!(
        matches!(
            state.overlay,
            Some(ClientShellOverlay::Rename(ClientRenameOverlay {
                target: ClientRenameTarget::Workspace { ref workspace_id },
                ..
            })) if workspace_id == "ws_1"
        ),
        "{:?}",
        state.overlay
    );
    state.overlay = None;
    menu_labels(&mut state, row);
    choose(&mut state, "Rename pane");
    assert!(
        matches!(
            state.overlay,
            Some(ClientShellOverlay::Rename(ClientRenameOverlay {
                target: ClientRenameTarget::Pane { ref pane_id },
                ..
            })) if pane_id == "pane_1"
        ),
        "{:?}",
        state.overlay
    );
}

#[test]
fn closing_from_a_row_menu_still_asks_first() {
    let mut state = ClientShellState::new(tree_config(true));
    state.config.confirm_close = true;
    show(&mut state, shell_space(false));
    let row = space_row_rect(&state, "ws_1");
    menu_labels(&mut state, row);
    let outcome = choose(&mut state, "Close space");
    assert!(sent_methods(&outcome).is_empty());
    assert!(
        matches!(state.overlay, Some(ClientShellOverlay::ConfirmClose(_))),
        "{:?}",
        state.overlay
    );
}

/// `branch_snapshot`, its space not the focused one.
fn unfocused_branch_state(prompt: bool) -> ClientShellState {
    let mut config = Config::default();
    config.ui.sidebar_tree = true;
    config.ui.prompt_new_tab_name = prompt;
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    let mut projected = branch_snapshot();
    projected.focused_workspace_id = Some("ws_9".into());
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    state.compose(106, 30).expect("branch frame");
    state
}

#[test]
fn tab_pane_and_agent_rows_open_the_row_menu_of_the_pane_they_stand_for() {
    let mut state = unfocused_branch_state(false);
    let rows = [
        (TreeTarget::Tab("tab_2".into()), "pane_2"),
        (
            TreeTarget::AgentTab {
                tab_id: "tab_3".into(),
                pane_id: "pane_3".into(),
            },
            "pane_3",
        ),
        (
            TreeTarget::AgentTab {
                tab_id: "tab_4".into(),
                pane_id: "pane_4".into(),
            },
            "pane_4",
        ),
        (TreeTarget::Pane("pane_5".into()), "pane_5"),
    ];
    for (row, pane) in rows {
        let rect = tree_row(&state, &row);
        assert_eq!(
            menu_labels(&mut state, rect),
            row_menu(&[&ADDS, &OTHER_PANE]),
            "{row:?}"
        );
        assert!(
            matches!(
                state.overlay,
                Some(ClientShellOverlay::ContextMenu(ClientContextMenuOverlay {
                    target: ClientContextMenuTarget::Pane { ref pane_id, .. },
                    ..
                })) if pane_id == pane
            ),
            "{row:?}: {:?}",
            state.overlay
        );
        state.overlay = None;
    }
}

#[test]
fn a_rows_new_tab_adds_to_the_rows_space_not_the_focused_one() {
    let mut state = unfocused_branch_state(false);
    let rect = tree_row(&state, &TreeTarget::Pane("pane_5".into()));
    menu_labels(&mut state, rect);
    let outcome = choose(&mut state, "New tab");
    assert!(
        matches!(
            sent_method(&outcome),
            crate::api::schema::Method::TabCreate(params)
                if params.workspace_id.as_deref() == Some("ws_1")
        ),
        "{:?}",
        sent_method(&outcome)
    );
    let mut state = unfocused_branch_state(true);
    let rect = tree_row(&state, &TreeTarget::Pane("pane_5".into()));
    menu_labels(&mut state, rect);
    choose(&mut state, "New tab");
    assert!(
        matches!(
            state.overlay,
            Some(ClientShellOverlay::Rename(ClientRenameOverlay {
                target: ClientRenameTarget::NewTab { ref workspace_id, .. },
                ..
            })) if workspace_id == "ws_1"
        ),
        "{:?}",
        state.overlay
    );
}

#[test]
fn a_split_tabs_row_menu_adds_splits_of_its_focused_pane() {
    let mut projected = branch_snapshot();
    projected.panes.push(tree_pane("pane_6", "ws_1", "tab_2"));
    let mut state = ClientShellState::new(tree_config(true));
    show(&mut state, projected);
    let rect = tree_row(&state, &TreeTarget::Tab("tab_2".into()));
    assert_eq!(
        menu_labels(&mut state, rect),
        ["New tab", "Split right", "Split down", "Rename", "Close"]
    );
    let outcome = choose(&mut state, "Split down");
    let methods = sent_methods(&outcome);
    assert!(
        matches!(
            methods[..],
            [
                crate::api::schema::Method::TabFocus(ref tab),
                crate::api::schema::Method::PaneSplit(ref split),
            ] if tab.tab_id == "tab_2"
                && split.workspace_id.as_deref() == Some("ws_1")
                && split.target_pane_id.is_none()
                && split.direction == crate::api::schema::SplitDirection::Down
        ),
        "the tab comes forward, then its focused pane splits: {methods:?}"
    );
}

#[test]
fn panel_items_and_worker_rows_open_the_row_menu() {
    let mut state = panel_state(panel_snapshot(), 45);
    let asking = panel_row(&state, &TreeTarget::Pane("pane_3".into()));
    let labels = menu_labels(&mut state, asking);
    assert_eq!(&labels[..3], ADDS, "panel item: {labels:?}");
    assert!(!labels.contains(&"Rename space"), "{labels:?}");
    state.overlay = None;

    let mut projected = agents_space(&[
        ("herdr-lead", AgentStatus::Working),
        ("herdr-w1", AgentStatus::Working),
    ]);
    projected.agents[1].tokens = vec![("lead".into(), "pane_1".into())];
    let mut state = ClientShellState::new(tree_config(true));
    show(&mut state, projected);
    let (worker, _) = state
        .hits
        .tree_rows
        .iter()
        .find(|(_, target)| {
            matches!(target, TreeTarget::Pane(pane) | TreeTarget::AgentTab { pane_id: pane, .. } if pane == "pane_2")
        })
        .cloned()
        .expect("the worker's row");
    let labels = menu_labels(&mut state, worker);
    assert_eq!(&labels[..3], ADDS, "worker row: {labels:?}");
}

#[test]
fn a_worktree_group_space_row_menu_closes_the_group_and_folds() {
    let mut state = ClientShellState::new(tree_config(true));
    let mut projected = shell_space(true);
    let mut linked = projected.workspaces[0].clone();
    linked.workspace_id = "ws_2".into();
    linked.focused = false;
    projected.workspaces[0].worktree = Some(ClientShellWorktree {
        key: "repo".into(),
        label: "repo".into(),
        is_linked_worktree: false,
    });
    linked.worktree = Some(ClientShellWorktree {
        key: "repo".into(),
        label: "repo".into(),
        is_linked_worktree: true,
    });
    projected.workspaces.push(linked);
    show(&mut state, projected);
    let row = space_row_rect(&state, "ws_1");
    let labels = menu_labels(&mut state, row);
    assert!(
        labels.ends_with(&[
            "Rename space",
            "Close group",
            "New worktree",
            "Open worktree...",
            "Collapse"
        ]),
        "{labels:?}"
    );
    choose(&mut state, "Collapse");
    assert!(state.collapsed_groups.contains(&tree_fold_key("ws_1")));
    state.compose(106, 30).expect("folded frame");
    let row = space_row_rect(&state, "ws_1");
    let labels = menu_labels(&mut state, row);
    assert_eq!(&labels[..3], ADDS, "a folded space row: {labels:?}");
    assert!(labels.ends_with(&["Expand"]), "{labels:?}");
}

/// U2: a space row's Close closes the worktree group only when the space is the group's one
/// repository checkout; beside a duplicate checkout it closes the space alone.
#[test]
fn a_space_row_menu_closes_the_group_only_for_its_one_repository_checkout() {
    for duplicate in [false, true] {
        let mut state = ClientShellState::new(tree_config(true));
        state.config.confirm_close = false;
        let mut projected = shell_space(true);
        let repo = |is_linked_worktree| ClientShellWorktree {
            key: "repo".into(),
            label: "repo".into(),
            is_linked_worktree,
        };
        projected.workspaces[0].worktree = Some(repo(false));
        let mut linked = projected.workspaces[0].clone();
        linked.workspace_id = "ws_2".into();
        linked.focused = false;
        linked.worktree = Some(repo(true));
        projected.workspaces.push(linked.clone());
        if duplicate {
            let mut copy = linked;
            copy.workspace_id = "ws_3".into();
            copy.worktree = Some(repo(false));
            projected.workspaces.push(copy);
        }
        show(&mut state, projected);
        let row = space_row_rect(&state, "ws_1");
        let labels = menu_labels(&mut state, row);
        let close = if duplicate {
            "Close space"
        } else {
            "Close group"
        };
        assert!(labels.contains(&close), "{labels:?}");
        let outcome = choose(&mut state, close);
        assert!(
            matches!(sent_methods(&outcome)[..],
                [crate::api::schema::Method::WorkspaceClose(params)]
                    if params.workspace_id == "ws_1" && params.close_group != duplicate),
            "duplicate {duplicate}: {:?}",
            sent_methods(&outcome)
        );
    }
}

/// B5c: a box heading's count is a plain grey number, one space after its title, no fill.
#[test]
fn a_tree_box_headings_count_is_a_plain_grey_number_one_space_after_it() {
    let config = ClientShellConfig::from_config(&Config::default());
    let render = render_tree(&box_snapshot(), &config, box_input());
    let palette = &config.palette;
    for (title, count) in [("ACTION REQUIRED", "4"), ("COMPLETED", "1")] {
        let y = render
            .rows
            .iter()
            .position(|row| row.starts_with(&format!("  {title} {count}")))
            .unwrap_or_else(|| panic!("{title}: {:#?}", render.rows)) as u16;
        let x = (title.len() + 3) as u16;
        let badge = &render.buffer[(x, y)];
        assert_eq!(
            (badge.symbol(), badge.fg, badge.bg),
            (count, palette.overlay1, render.buffer[(2, y)].bg)
        );
        assert!(!badge.modifier.contains(Modifier::BOLD));
        assert_eq!(render.buffer[(x - 1, y)].bg, render.buffer[(2, y)].bg);
    }
}

#[test]
fn the_sidebars_chrome_is_grey_and_plain_while_its_marks_keep_their_colour() {
    for palette in [Palette::catppuccin(), Palette::vesper()] {
        let mut config = ClientShellConfig::from_config(&Config::default());
        config.palette = palette.clone();
        let mut projected = git_snapshot();
        for pane in &mut projected.panes {
            pane.focused = pane.pane_id == "pane_2";
        }
        projected.integration_updates_available = true;
        let render = render_tree(
            &projected,
            &config,
            TreeRenderInput {
                board: super::super::task_board::BoardIcon::Open,
                ..tall(0)
            },
        );
        let filtered = render_tree(
            &projected,
            &config,
            TreeRenderInput {
                filter: TreeFilter::NeedsYou,
                ..tall(0)
            },
        );
        let cell = |x: u16, y: u16| &render.buffer[(x, y)];
        let plain = |x: u16, y: u16| !cell(x, y).modifier.contains(Modifier::BOLD);
        for y in 2..6 {
            assert_eq!(cell(0, y).symbol(), "▎", "row {y}");
            // The space bar is no chrome: it keeps the accent.
            assert_eq!(cell(0, y).fg, palette.accent, "the space bar, row {y}");
        }
        let arrow = (2..25)
            .find(|&x| cell(x, 2).symbol() == "▾")
            .expect("the open space's fold arrow");
        assert_eq!(cell(arrow, 2).fg, palette.overlay1, "the fold arrow");
        assert!(
            render.rows[0].starts_with("  SPACES"),
            "{:?}",
            render.rows[0]
        );
        for x in 2..8 {
            assert_eq!(cell(x, 0).fg, palette.overlay0, "SPACES at {x}");
            assert!(plain(x, 0), "SPACES bold at {x}");
        }
        let filter = filtered.hits.tree_filter;
        let icon = &filtered.buffer[(filter.x, filter.y)];
        assert_eq!(icon.fg, palette.yellow, "the filter icon");
        assert!(
            !icon.modifier.contains(Modifier::BOLD),
            "the filter icon bold"
        );
        let board = render.hits.tree_task_board;
        assert_eq!(cell(board.x, board.y).symbol(), "☑");
        assert_eq!(
            cell(board.x, board.y).fg,
            palette.yellow,
            "the open board icon"
        );
        assert!(plain(board.x, board.y), "the open board icon bold");
        let launcher = render.hits.global_launcher;
        let dot = (launcher.x..launcher.right())
            .find(|&x| cell(x, launcher.y).symbol() == "●")
            .expect("the attention dot");
        assert_eq!(
            cell(dot, launcher.y).fg,
            palette.accent,
            "the attention dot"
        );
        assert!(plain(dot, launcher.y), "the attention dot bold");
    }
}

#[test]
fn the_rails_totals_colour_their_mark_only_and_a_focused_agent_takes_the_spaces_fill() {
    for palette in [
        Palette::catppuccin(),
        Palette::vesper(),
        Palette::terminal(),
    ] {
        let mut projected = rail_snapshot();
        projected.agents[1].focused = true;
        projected.integration_updates_available = true;
        let mut state = panel_state(projected, 12);
        state.sidebar_collapsed = true;
        state.collapsed_groups.insert(tree_fold_key("ws_3"));
        state.config.palette = palette.clone();
        let (rows, frame) = rail_rows(&mut state, 12);
        assert_eq!(rows[9..], ["?2", "✓1", " »"], "{rows:#?}");
        let buffer = frame.to_ratatui_buffer().expect("frame should reconstruct");
        let cell = |x: u16, y: u16| &buffer[(x, y)];
        assert_eq!(
            (cell(0, 9).fg, cell(1, 9).fg),
            (palette.yellow, palette.overlay1),
            "ACTION REQUIRED: a yellow mark, a grey count"
        );
        assert_eq!(
            (cell(0, 10).fg, cell(1, 10).fg),
            (palette.green, palette.overlay1),
            "COMPLETED: a green mark, a grey count"
        );
        assert_eq!(rows[1], "├?", "{rows:#?}");
        for x in 0..3 {
            assert_eq!(
                cell(x, 1).bg,
                cell(x, 0).bg,
                "the focused agent on the focused space's fill, column {x}"
            );
        }
        assert_eq!(cell(1, 11).symbol(), "»");
        assert_eq!(cell(1, 11).fg, palette.accent, "the attention toggle");
        assert!(!cell(1, 11).modifier.contains(Modifier::BOLD), "not bold");
    }
}

#[test]
fn plan_in_the_sidebar_is_the_boards_lilac_apart_from_build() {
    use super::super::tree_sidebar::stage_color;
    for palette in [Palette::catppuccin(), Palette::vesper()] {
        let plan = stage_color("Plan", &palette);
        assert_eq!(plan, super::super::task_board::style::lilac(&palette));
        assert_ne!(plan, stage_color("Build", &palette), "Plan and Build");
    }
}

#[test]
fn a_yes_or_no_mark_is_red_on_its_row_and_its_folded_tab_and_a_question_stays_yellow() {
    let column = |row: &str, glyph: char| {
        row.chars()
            .position(|ch| ch == glyph)
            .unwrap_or_else(|| panic!("{glyph} in {row:?}")) as u16
    };
    for palette in [Palette::catppuccin(), Palette::vesper()] {
        let mut config = ClientShellConfig::from_config(&Config::default());
        config.palette = palette.clone();
        let mut projected = two_agent_review();
        projected.tabs[3].agent_status = AgentStatus::Blocked;
        let open = render_tree(&projected, &config, tall(0));
        assert_eq!(open.rows[6], "▎     ├╴! codex");
        let bang = column(&open.rows[6], '!');
        assert_eq!(open.buffer[(bang, 6)].fg, palette.red, "the agent row");
        let folded = render_tree(
            &projected,
            &config,
            TreeRenderInput {
                folded: &["tab_4"],
                ..tall(0)
            },
        );
        assert_eq!(folded.rows[5], "▎ ╰╴! review ▸");
        let bang = column(&folded.rows[5], '!');
        assert_eq!(folded.buffer[(bang, 5)].fg, palette.red, "the folded tab");
    }
}

#[test]
fn the_sidebars_stages_are_the_boards_five_colours_each_apart() {
    use super::super::task_board::style::{lilac, rose};
    use super::super::tree_sidebar::stage_color;
    let mut vesper = Palette::vesper();
    vesper.blue = ratatui::style::Color::Rgb(0x7f, 0xb4, 0xca);
    for palette in [
        Palette::catppuccin(),
        Palette::vesper(),
        vesper,
        Palette::terminal(),
    ] {
        let colours = [
            ("Plan", lilac(&palette)),
            ("Build", palette.yellow),
            ("Review", rose(&palette)),
            ("Verify", palette.blue),
            ("Ship", palette.green),
        ];
        for (word, colour) in colours {
            assert_eq!(stage_color(word, &palette), colour, "{word}");
        }
        for (at, (word, colour)) in colours.iter().enumerate() {
            for (other, other_colour) in &colours[at + 1..] {
                assert_ne!(colour, other_colour, "{word} and {other}");
            }
        }
    }
}
