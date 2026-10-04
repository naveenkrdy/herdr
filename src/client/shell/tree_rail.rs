//! The collapsed tree sidebar: a rail of three columns in the open tree's order. Each
//! space is one line, its main lead's mark and its number, standing for its main lead as
//! the space row does; under an open space, one line per other agent the tree shows, a dim
//! connector and its mark in the sub-agent spinner while it works; a blank line between
//! spaces. At the bottom, under a dim rule and above the `»` toggle, the box's ACTION
//! REQUIRED and COMPLETED totals. Built from the tree's rows, once per compose.

use std::collections::HashSet;

use super::render::sidebar::workspace_selection_background;
use super::render::{put_text, render_sidebar_background, ShellRenderState};
use super::tree_sidebar::{agent_mark, tree_rows, TreeRow, DONE_MARK};
use super::*;
use crate::api::schema::AgentStatus;
use crate::protocol::{ClientShellAgent, ClientShellPane};

/// One line of the rail.
pub(super) enum RailLine<'a> {
    /// A space, its 1-based place and its main lead.
    Space {
        workspace: &'a ClientShellWorkspace,
        number: usize,
        lead: Option<&'a ClientShellAgent>,
    },
    /// An agent row of the tree; `last` under its space.
    Agent {
        pane: &'a ClientShellPane,
        agent: &'a ClientShellAgent,
        last: bool,
    },
    /// Between two spaces.
    Blank,
}

/// The rail's first `rows` lines: the tree's space rows and agent rows, flat. Shells,
/// plugin panes, tab rows and branch lines are left out.
pub(super) fn rail_lines<'a>(
    snapshot: &'a ClientShellSnapshot,
    folds: &HashSet<String>,
    rows: usize,
) -> Vec<RailLine<'a>> {
    let mut lines = Vec::with_capacity(rows.min(snapshot.workspaces.len() + snapshot.agents.len()));
    let mut number = 0;
    for row in tree_rows(snapshot, folds, false) {
        let line = match row {
            TreeRow::Space {
                workspace, agent, ..
            } => {
                number += 1;
                end_space(&mut lines);
                if !lines.is_empty() {
                    if lines.len() == rows {
                        return lines;
                    }
                    lines.push(RailLine::Blank);
                }
                RailLine::Space {
                    workspace,
                    number,
                    lead: agent.map(|(_, agent)| agent),
                }
            }
            TreeRow::AgentTab { pane, agent, .. }
            | TreeRow::Pane {
                pane,
                agent: Some(agent),
                ..
            } => RailLine::Agent {
                pane,
                agent,
                last: false,
            },
            _ => continue,
        };
        // Full: the next line only told whether the last agent line ends its space.
        if lines.len() == rows {
            return lines;
        }
        lines.push(line);
    }
    end_space(&mut lines);
    lines
}

/// Marks the agent line that ends the space above as its last.
fn end_space(lines: &mut [RailLine<'_>]) {
    if let Some(RailLine::Agent { last, .. }) = lines.last_mut() {
        *last = true;
    }
}

/// An agent's rail mark: the tree's [`agent_mark`] while it works (the sub-agent spinner for
/// a `worker`), quieter where the rail is only a glance: every agent waiting on you one
/// yellow `?`, a done one a green `✓`, any other a dim `○`.
fn rail_mark(
    agent: &ClientShellAgent,
    spinner_ms: u64,
    config: &ClientShellConfig,
    worker: bool,
) -> (&'static str, ratatui::style::Color) {
    let palette = &config.palette;
    if super::tree_panel::needs_you(agent).is_some() {
        return super::tree_panel::reason_mark(super::tree_panel::Reason::Answer, palette);
    }
    match agent.agent_status {
        AgentStatus::Working => agent_mark(agent, spinner_ms, config, worker),
        AgentStatus::Done => (DONE_MARK, palette.green),
        _ => ("○", palette.overlay0),
    }
}

pub(super) fn render_tree_rail(
    buffer: &mut Buffer,
    area: Rect,
    snapshot: &ClientShellSnapshot,
    config: &ClientShellConfig,
    state: &ShellRenderState<'_>,
    hits: &mut ShellHitMap,
) {
    let selected_workspace_id = state
        .selected_workspace_id
        .map(|target| target.workspace_id.as_str());
    let spinner_ms = state.spinner_ms;
    let palette = &config.palette;
    render_sidebar_background(buffer, area, palette);
    let content = Rect::new(area.x, area.y, area.width.saturating_sub(1), area.height);
    if content.is_empty() {
        hits.sidebar_toggle = Rect::default();
        return;
    }
    let selection_background = workspace_selection_background(palette);
    // The box's totals, each only above zero, under a dim rule; they sit right above the
    // toggle.
    let (waiting, done) = if config.sidebar_tree_panel {
        super::tree_panel::totals(snapshot)
    } else {
        (0, 0)
    };
    let (ask, yellow) = super::tree_panel::reason_mark(super::tree_panel::Reason::Answer, palette);
    let counts = [(waiting, ask, yellow), (done, DONE_MARK, palette.green)];
    let shown_counts = counts.iter().filter(|(count, ..)| *count > 0).count() as u16;
    let toggle_y = content.bottom().saturating_sub(1);
    let rule_rows = u16::from(shown_counts > 0);
    let counts_y = toggle_y.saturating_sub(shown_counts).max(content.y);
    let rule_y = counts_y.saturating_sub(rule_rows).max(content.y);
    let lines = rail_lines(
        snapshot,
        state.collapsed_groups,
        usize::from(rule_y - content.y),
    );
    // The row under the mouse draws on the focused tree row's grey, unless it has a fill.
    let hover = super::tree_sidebar::focus_band(palette, selected_workspace_id.is_some());
    for (y, line) in (content.y..).zip(lines) {
        let rect = Rect::new(content.x, y, content.width, 1);
        if state.hovered_row.is_some_and(|row| row.intersects(rect))
            && !matches!(line, RailLine::Blank)
        {
            buffer.set_style(rect, Style::default().bg(hover));
        }
        match line {
            RailLine::Blank => {}
            RailLine::Space {
                workspace,
                number,
                lead,
            } => {
                // The active space's number in the text colour on the selection band across
                // the rail; a space the keys select keeps its band; the rest dim.
                let selected = selected_workspace_id == Some(workspace.workspace_id.as_str());
                let number_style = if workspace.focused {
                    buffer.set_style(rect, Style::default().bg(selection_background));
                    Style::default().fg(palette.text)
                } else if selected {
                    buffer.set_style(rect, Style::default().bg(selection_background));
                    Style::default().fg(palette.overlay1)
                } else {
                    Style::default().fg(palette.overlay0)
                };
                let (mark, color) = lead.map_or((" ", palette.overlay0), |agent| {
                    rail_mark(agent, spinner_ms, config, false)
                });
                put_text(buffer, rect.x, y, 1, mark, Style::default().fg(color));
                put_text(
                    buffer,
                    rect.x.saturating_add(1),
                    y,
                    rect.width.saturating_sub(1),
                    &number.to_string(),
                    number_style,
                );
                hits.workspaces.push(WorkspaceHit {
                    rect,
                    endpoint_id: ClientEndpointId::Local,
                    workspace_id: workspace.workspace_id.clone(),
                    indented: false,
                    group_toggle: None,
                });
            }
            RailLine::Agent { pane, agent, last } => {
                if agent.focused {
                    buffer.set_style(rect, Style::default().bg(selection_background));
                }
                put_text(
                    buffer,
                    rect.x,
                    y,
                    1,
                    if last { "╰" } else { "├" },
                    Style::default().fg(palette.overlay0),
                );
                let (mark, color) = rail_mark(agent, spinner_ms, config, true);
                put_text(
                    buffer,
                    rect.x.saturating_add(1),
                    y,
                    rect.width.saturating_sub(1),
                    mark,
                    Style::default().fg(color),
                );
                hits.agents.push((rect, pane.pane_id.clone()));
            }
        }
    }
    if rule_rows > 0 {
        let rule = "─".repeat(usize::from(content.width));
        put_text(
            buffer,
            content.x,
            rule_y,
            content.width,
            &rule,
            Style::default().fg(palette.overlay0),
        );
    }
    let shown = counts.iter().filter(|(count, ..)| *count > 0);
    for (y, (count, mark, color)) in (counts_y..toggle_y).zip(shown) {
        put_text(buffer, content.x, y, 1, mark, Style::default().fg(*color));
        put_text(
            buffer,
            content.x.saturating_add(1),
            y,
            content.width.saturating_sub(1),
            &count.to_string(),
            Style::default().fg(palette.overlay1),
        );
    }
    hits.sidebar_toggle = Rect::new(content.x + content.width / 2, toggle_y, 1, 1);
    put_text(
        buffer,
        hits.sidebar_toggle.x,
        hits.sidebar_toggle.y,
        hits.sidebar_toggle.width,
        "»",
        if super::global_menu::global_menu_attention(snapshot) {
            Style::default().fg(palette.accent)
        } else {
            Style::default().fg(palette.overlay0)
        },
    );
}
