//! With `ui.sidebar_tree` off the sidebar, its header, the tab bar, the footer and the
//! bottom panel must draw and click exactly as on `master`, apart from the agent status
//! marks, whose colours every sidebar shares (working and needs you yellow, done teal, idle
//! green). The fixtures under `fixtures/sidebar_off/` were written by this file's
//! `dump` on a `master` checkout, then only those marks' colours were updated; the test
//! renders the same snapshots here and compares.

use super::*;

fn workspace(id: &str, number: usize, label: &str, status: AgentStatus) -> ClientShellWorkspace {
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

fn tab(id: &str, workspace_id: &str, number: usize, status: AgentStatus) -> ClientShellTab {
    ClientShellTab {
        tab_id: id.into(),
        workspace_id: workspace_id.into(),
        number,
        label: number.to_string(),
        custom_label: false,
        zoomed: false,
        focused: false,
        agent_status: status,
    }
}

fn pane(id: &str, workspace_id: &str, tab_id: &str) -> ClientShellPane {
    let mut pane = snapshot().panes[0].clone();
    pane.pane_id = id.into();
    pane.workspace_id = workspace_id.into();
    pane.tab_id = tab_id.into();
    pane.focused = false;
    pane
}

fn agent(pane: &ClientShellPane, name: &str, status: AgentStatus) -> ClientShellAgent {
    ClientShellAgent {
        pane_id: pane.pane_id.clone(),
        workspace_id: pane.workspace_id.clone(),
        tab_id: pane.tab_id.clone(),
        name: Some(name.into()),
        display_agent: None,
        agent: Some("claude".into()),
        title: Some(format!("{name} title")),
        terminal_title: None,
        terminal_title_stripped: None,
        agent_status: status,
        state_change_seq: 0,
        state_labels: Vec::new(),
        tokens: Vec::new(),
        focused: pane.focused,
    }
}

/// Three spaces: `herdr` (focused) with a working lead and a blocked worker in two tabs,
/// `dotworks` with an agent that was working, and `notes` with only a shell. `done`
/// turns the dotworks agent Done, which the client shows only after it saw it work.
fn spaces(done: bool) -> ClientShellSnapshot {
    let mut projected = snapshot();
    let mut herdr = workspace("ws_1", 1, "herdr", AgentStatus::Blocked);
    herdr.focused = true;
    herdr.branch = Some("feat/sidebar-tree".into());
    herdr.git_ahead_behind = Some((3, 1));
    let dotworks_status = if done {
        AgentStatus::Done
    } else {
        AgentStatus::Working
    };
    projected.workspaces = vec![
        herdr,
        workspace("ws_2", 2, "dotworks", dotworks_status),
        workspace("ws_3", 3, "notes", AgentStatus::Unknown),
    ];
    let mut first = tab("tab_1", "ws_1", 1, AgentStatus::Working);
    first.focused = true;
    projected.tabs = vec![
        first,
        tab("tab_2", "ws_1", 2, AgentStatus::Blocked),
        tab("tab_3", "ws_2", 1, dotworks_status),
        tab("tab_4", "ws_3", 1, AgentStatus::Unknown),
    ];
    let mut lead = pane("pane_1", "ws_1", "tab_1");
    lead.focused = true;
    projected.panes = vec![
        lead,
        pane("pane_2", "ws_1", "tab_2"),
        pane("pane_3", "ws_2", "tab_3"),
        pane("pane_4", "ws_3", "tab_4"),
    ];
    let mut dotworks = agent(&projected.panes[2], "dotworks-lead", dotworks_status);
    dotworks.state_change_seq = u64::from(done);
    let mut worker = agent(&projected.panes[1], "herdr-w1", AgentStatus::Blocked);
    worker.tokens = vec![("lead".into(), "pane_1".into())];
    projected.agents = vec![
        agent(&projected.panes[0], "herdr-lead", AgentStatus::Working),
        worker,
        dotworks,
    ];
    projected.focused_workspace_id = Some("ws_1".into());
    projected.focused_tab_id = Some("tab_1".into());
    projected.focused_pane_id = Some("pane_1".into());
    projected.revision = 1 + u64::from(done);
    projected
}

fn show(state: &mut ClientShellState, projected: ClientShellSnapshot) {
    let revision = projected.revision;
    state.set_snapshot(Box::new(projected));
    let mut surface = surface();
    surface.projection_revision = revision;
    state.set_pane_surface(surface);
}

/// Every cell's symbol and style, row by row, then the click areas `master` knows.
fn dump(state: &mut ClientShellState, cols: u16, rows: u16) -> String {
    let frame = state.compose(cols, rows).expect("frame");
    let mut out = String::new();
    for (y, row) in frame.cells.chunks(frame.width as usize).enumerate() {
        let text = row
            .iter()
            .map(|cell| cell.symbol.as_str())
            .collect::<String>();
        out.push_str(&format!("{y:2} |{text}|\n"));
        let mut runs = Vec::new();
        let mut start = 0;
        for x in 1..=row.len() {
            let same = x < row.len() && {
                let (a, b) = (&row[x], &row[start]);
                (a.fg, a.bg, a.modifier) == (b.fg, b.bg, b.modifier)
            };
            if !same {
                let cell = &row[start];
                runs.push(format!(
                    "{start}-{} {:x}/{:x}/{:x}",
                    x - 1,
                    cell.fg,
                    cell.bg,
                    cell.modifier
                ));
                start = x;
            }
        }
        out.push_str(&format!("   {}\n", runs.join(" ")));
    }
    let hits = &state.hits;
    let workspaces = hits
        .workspaces
        .iter()
        .map(|hit| {
            format!(
                "{:?} {} indented={} toggle={:?}",
                hit.rect, hit.workspace_id, hit.indented, hit.group_toggle
            )
        })
        .collect::<Vec<_>>();
    out.push_str(&format!("workspaces: {workspaces:?}\n"));
    out.push_str(&format!("tabs: {:?}\n", hits.tabs));
    out.push_str(&format!("agents: {:?}\n", hits.agents));
    out.push_str(&format!("endpoint_agents: {:?}\n", hits.endpoint_agents));
    for (name, rect) in [
        ("workspace_body", hits.workspace_body),
        ("workspace_scrollbar", hits.workspace_scrollbar),
        ("agent_body", hits.agent_body),
        ("agent_scrollbar", hits.agent_scrollbar),
        ("agent_sort_toggle", hits.agent_sort_toggle),
        ("sidebar_divider", hits.sidebar_divider),
        ("sidebar_section_divider", hits.sidebar_section_divider),
        ("sidebar_toggle", hits.sidebar_toggle),
        ("new_workspace", hits.new_workspace),
        ("new_tab", hits.new_tab),
        ("tab_scroll_left", hits.tab_scroll_left),
        ("tab_scroll_right", hits.tab_scroll_right),
        ("global_launcher", hits.global_launcher),
    ] {
        out.push_str(&format!("{name}: {rect:?}\n"));
    }
    out.push_str(&format!(
        "max scroll: workspaces {} agents {}\n",
        hits.workspace_max_scroll, hits.agent_max_scroll
    ));
    out
}

/// The fixture scenarios by name: one space, several spaces with agents, the collapsed
/// rail and a narrow window.
fn scenarios() -> Vec<(&'static str, String)> {
    // The default: the tree is off.
    let off = Config::default;
    let mut out = Vec::new();

    let mut state = ClientShellState::new(ClientShellConfig::from_config(&off()));
    show(&mut state, snapshot());
    out.push(("one_space", dump(&mut state, 106, 30)));

    let mut state = ClientShellState::new(ClientShellConfig::from_config(&off()));
    show(&mut state, spaces(false));
    state.compose(106, 30).expect("working frame");
    show(&mut state, spaces(true));
    out.push(("spaces", dump(&mut state, 106, 30)));

    state.sidebar_collapsed = true;
    out.push(("collapsed", dump(&mut state, 106, 30)));

    let mut narrow = off();
    narrow.ui.sidebar_width = 18;
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&narrow));
    show(&mut state, spaces(false));
    state.compose(72, 16).expect("working frame");
    show(&mut state, spaces(true));
    out.push(("narrow", dump(&mut state, 72, 16)));
    out
}

#[test]
fn sidebar_with_the_tree_off_draws_and_clicks_as_on_master() {
    assert!(
        !Config::default().ui.sidebar_tree,
        "the scenarios render the default"
    );
    let fixture = |name: &str| match name {
        "one_space" => include_str!("../../../../tests/fixtures/sidebar_off/one_space.txt"),
        "spaces" => include_str!("../../../../tests/fixtures/sidebar_off/spaces.txt"),
        "collapsed" => include_str!("../../../../tests/fixtures/sidebar_off/collapsed.txt"),
        "narrow" => include_str!("../../../../tests/fixtures/sidebar_off/narrow.txt"),
        other => panic!("no fixture for {other}"),
    };
    for (name, rendered) in scenarios() {
        let expected = fixture(name);
        if rendered != expected {
            let first = rendered
                .lines()
                .zip(expected.lines())
                .position(|(got, want)| got != want)
                .unwrap_or(0);
            panic!(
                "{name} differs from master first at dump line {first}:\n  got:  {:?}\n  want: {:?}",
                rendered.lines().nth(first),
                expected.lines().nth(first)
            );
        }
    }
}
