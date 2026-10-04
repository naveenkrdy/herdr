use super::*;

/// A dialog's key as the board's footer draws it: the key in its block, then the action.
fn key_text(key: &str, action: &str) -> String {
    format!(" {key}  {action}")
}

/// A dialog's keys, left to right, two columns apart.
fn keys_row(keys: &[(&str, &str)]) -> String {
    keys.iter()
        .map(|(key, action)| key_text(key, action))
        .collect::<Vec<_>>()
        .join("  ")
}

fn dialog_state() -> ClientShellState {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state
}

fn rename() -> ClientShellOverlay {
    ClientShellOverlay::Rename(ClientRenameOverlay {
        title: "rename",
        input: "name".into(),
        target: ClientRenameTarget::Pane {
            pane_id: "pane_1".into(),
        },
    })
}

fn confirm_close() -> ClientShellOverlay {
    ClientShellOverlay::ConfirmClose(ClientConfirmCloseOverlay {
        workspace_id: "ws_1".into(),
        close_group: false,
        tab_target: None,
        title: "close".into(),
        detail: "confirm".into(),
    })
}

fn worktree_remove(force_confirmation: bool) -> ClientShellOverlay {
    ClientShellOverlay::WorktreeRemove(ClientWorktreeRemoveOverlay {
        workspace_id: "ws_1".into(),
        path: "path".into(),
        error: None,
        removing: false,
        force_confirmation,
    })
}

/// A dialog's name, the overlay that opens it, the width it draws in full, and its keys.
type Dialog = (
    &'static str,
    ClientShellOverlay,
    u16,
    Vec<(&'static str, &'static str)>,
);

/// Every dialog with an Enter key.
fn dialogs() -> Vec<Dialog> {
    vec![
        (
            "onboarding",
            ClientShellOverlay::Onboarding,
            68,
            vec![("Enter", "continue")],
        ),
        (
            "name",
            rename(),
            60,
            vec![("Enter", "save"), ("^c", "clear"), ("esc", "cancel")],
        ),
        (
            "confirm close",
            confirm_close(),
            68,
            vec![("Enter", "confirm"), ("esc", "cancel")],
        ),
        (
            "worktree create",
            ClientShellOverlay::WorktreeCreate(ClientWorktreeCreateOverlay {
                source_workspace_id: "ws_1".into(),
                repo_name: "repo".into(),
                branch: "branch".into(),
                checkout_path: "path".into(),
                error: None,
                creating: false,
            }),
            72,
            vec![("Enter", "create and open"), ("esc", "cancel")],
        ),
        (
            "worktree open",
            ClientShellOverlay::WorktreeOpen(ClientWorktreeOpenOverlay {
                source_workspace_id: "ws_1".into(),
                entries: Vec::new(),
                selected: 0,
                query: TextEditor::default(),
                search_focused: false,
                error: None,
                opening: false,
            }),
            100,
            vec![("Enter", "open"), ("esc", "cancel")],
        ),
        (
            "worktree remove",
            worktree_remove(false),
            72,
            vec![("Enter", "remove"), ("esc", "cancel")],
        ),
        (
            "worktree delete anyway",
            worktree_remove(true),
            72,
            vec![("Enter", "delete anyway"), ("esc", "cancel")],
        ),
    ]
}

#[test]
fn dialog_keys_say_enter_and_draw_as_the_boards_footer_keys() {
    let mut cases = dialogs();
    cases.push(("settings", ClientShellOverlay::Onboarding, 80, Vec::new()));
    for (name, overlay, width, keys) in cases {
        let mut state = dialog_state();
        if name == "settings" {
            state.open_settings_overlay();
        } else {
            state.overlay = Some(overlay);
        }
        let keys = if name == "settings" {
            vec![("Enter", "apply"), ("esc", "close")]
        } else {
            keys
        };
        let frame = state.compose(width, 40).unwrap();
        let rows = frame_rows(&frame);
        let text = rows.join("\n");
        assert!(!text.contains('↵'), "{name}: no ↵ glyph:\n{text}");
        let row = keys_row(&keys);
        let y = rows
            .iter()
            .position(|line| line.contains(&row))
            .unwrap_or_else(|| panic!("{name}: `{row}` drawn:\n{text}"));
        let line = &rows[y];
        let x = line[..line.find(&row).expect("found")].chars().count() as u16;

        // The primary key's hit area is its block and its action, no wider.
        let primary = state.hits.overlay_primary;
        let (key, action) = keys[0];
        assert_eq!(
            (primary.x, primary.y, primary.width),
            (x, y as u16, key_text(key, action).chars().count() as u16),
            "{name}: the primary hit area"
        );

        // The key in the board's key block, the action in the hint grey.
        let palette = &state.config.palette;
        let key_style = crate::client::shell::task_board::style::key(palette);
        let cell = |x: u16| &frame.cells[y * width as usize + x as usize];
        let block = cell(x + 1);
        assert_eq!(block.symbol, "E", "{name}");
        let (text_colour, fill) = if is_danger(name) {
            (danger_text(palette), palette.red)
        } else {
            (
                key_style.fg.expect("a key colour"),
                key_style.bg.expect("a key fill"),
            )
        };
        assert_eq!(
            block.fg,
            crate::protocol::color_to_u32(text_colour),
            "{name}: key text"
        );
        assert_eq!(
            block.bg,
            crate::protocol::color_to_u32(fill),
            "{name}: key block"
        );
        assert_eq!(
            block.modifier & Modifier::BOLD.bits(),
            0,
            "{name}: plain key"
        );
        let words = cell(x + key.len() as u16 + 3);
        assert_eq!(words.symbol, &action[..1], "{name}");
        assert_eq!(
            words.fg,
            crate::protocol::color_to_u32(palette.overlay0),
            "{name}: action in the hint grey"
        );
        assert_eq!(
            words.modifier & Modifier::BOLD.bits(),
            0,
            "{name}: plain action"
        );
    }
}

/// The dialogs whose Enter destroys something: their key keeps its red fill.
fn is_danger(name: &str) -> bool {
    matches!(
        name,
        "confirm close" | "worktree remove" | "worktree delete anyway"
    )
}

/// The text on a red key: the panel's own colour, as the red buttons had it.
fn danger_text(palette: &crate::app::state::Palette) -> ratatui::style::Color {
    match palette.panel_bg {
        ratatui::style::Color::Reset => palette.surface_dim,
        colour => colour,
    }
}

#[test]
fn a_dangerous_dialog_key_keeps_its_red_fill() {
    let dangers: Vec<Dialog> = dialogs()
        .into_iter()
        .filter(|(name, ..)| is_danger(name))
        .collect();
    assert_eq!(dangers.len(), 3, "confirm close, remove and delete anyway");
    for (name, overlay, width, keys) in dangers {
        let mut state = dialog_state();
        state.overlay = Some(overlay);
        let frame = state.compose(width, 40).unwrap();
        let primary = state.hits.overlay_primary;
        let palette = &state.config.palette;
        let cell = |x: u16| &frame.cells[primary.y as usize * width as usize + x as usize];
        let (key, action) = keys[0];
        // Every column of the key block is red, the action after it is not.
        for x in primary.x..primary.x + key.len() as u16 + 2 {
            assert_eq!(
                cell(x).bg,
                crate::protocol::color_to_u32(palette.red),
                "{name}: the `{key}` block is red at column {x}"
            );
            assert_eq!(
                cell(x).fg,
                crate::protocol::color_to_u32(danger_text(palette)),
                "{name}: the `{key}` text on red at column {x}"
            );
        }
        let words = cell(primary.x + key.len() as u16 + 3);
        assert_eq!(words.symbol, &action[..1], "{name}");
        assert_eq!(
            words.fg,
            crate::protocol::color_to_u32(palette.overlay0),
            "{name}: action in the hint grey"
        );
        assert_ne!(
            words.bg,
            crate::protocol::color_to_u32(palette.red),
            "{name}: the action is not red"
        );
    }
}

fn click(state: &mut ClientShellState, x: u16, y: u16) -> ClientShellInput {
    state.handle_raw_events(vec![crate::raw_input::RawInputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: x,
        row: y,
        modifiers: KeyModifiers::empty(),
    })])
}

/// Whether a click asked a server for something.
fn sends(outcome: &ClientShellInput) -> bool {
    !outcome.requests.is_empty()
        || outcome
            .actions
            .iter()
            .any(|action| matches!(action, ClientShellAction::Endpoint { .. }))
}

#[test]
fn a_click_on_a_dialog_keys_last_column_still_acts() {
    // The name dialog: clear empties the name, cancel closes it.
    let mut state = dialog_state();
    state.overlay = Some(rename());
    state.compose(60, 40).unwrap();
    let clear = state.hits.overlay_clear;
    click(&mut state, clear.right() - 1, clear.y);
    let Some(ClientShellOverlay::Rename(dialog)) = &state.overlay else {
        panic!("clear keeps the dialog open");
    };
    assert!(dialog.input.is_empty(), "clear empties the name");
    state.compose(60, 40).unwrap();
    let cancel = state.hits.overlay_cancel;
    click(&mut state, cancel.right() - 1, cancel.y);
    assert!(state.overlay.is_none(), "cancel closes the dialog");

    // Its save key, at its last column.
    let mut state = dialog_state();
    state.overlay = Some(rename());
    state.compose(60, 40).unwrap();
    let save = state.hits.overlay_primary;
    let outcome = click(&mut state, save.right() - 1, save.y);
    assert!(state.overlay.is_none(), "save closes the dialog");
    assert!(sends(&outcome), "save sends the rename");

    // The close confirmation's key, at its last column.
    let mut state = dialog_state();
    state.overlay = Some(confirm_close());
    state.compose(68, 40).unwrap();
    let confirm = state.hits.overlay_primary;
    let outcome = click(&mut state, confirm.right() - 1, confirm.y);
    assert!(state.overlay.is_none(), "confirm closes the dialog");
    assert!(sends(&outcome), "confirm sends the close");
}

#[test]
fn dialog_keys_stay_inside_a_narrow_dialog() {
    for (name, overlay, _, _) in dialogs() {
        let mut state = dialog_state();
        state.overlay = Some(overlay);
        state.compose(30, 20).unwrap();
        let area = Rect::new(0, 0, 30, 20);
        for (label, rect) in [
            ("primary", state.hits.overlay_primary),
            ("clear", state.hits.overlay_clear),
            ("cancel", state.hits.overlay_cancel),
        ] {
            assert_eq!(
                rect.intersection(area),
                rect,
                "{name} {label}: its hit area stays on screen"
            );
        }
    }
}
