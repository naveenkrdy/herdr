use super::*;

#[test]
fn shell_new_controls_use_the_same_client_action_routes_as_keybinds() {
    let mut config = Config::default();
    config.ui.prompt_new_workspace_name = false;
    config.ui.prompt_new_tab_name = true;
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.compose(106, 20).expect("composed frame");

    let new_workspace = state.hits.new_workspace;
    let create_workspace =
        state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: new_workspace.x + 1,
            row: new_workspace.y,
            modifiers: KeyModifiers::empty(),
        })]);
    let [ClientShellAction::Endpoint { request, .. }] = &create_workspace.actions[..] else {
        panic!("new workspace click should use the endpoint API");
    };
    assert!(matches!(
        request.method,
        crate::api::schema::Method::WorkspaceCreate(_)
    ));

    let new_tab = state.hits.new_tab;
    let open_new_tab =
        state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: new_tab.x + 1,
            row: new_tab.y,
            modifiers: KeyModifiers::empty(),
        })]);
    assert!(open_new_tab.actions.is_empty());
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Rename(ClientRenameOverlay {
            target: ClientRenameTarget::NewTab { .. },
            ..
        }))
    ));
}

#[test]
fn manual_client_chrome_preferences_round_trip_per_endpoint() {
    let path = std::env::temp_dir().join(format!(
        "herdr-client-shell-prefs-{}.json",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    let config =
        ClientShellConfig::from_config(&Config::default()).with_preferences_path(path.clone());
    let mut state = ClientShellState::new(config);
    state.sidebar_width = 31;
    state.sidebar_width_manual = true;
    state.sidebar_section_split = 0.7;
    state.sidebar_section_split_manual = true;
    state.sidebar_collapsed = true;
    state.sidebar_collapsed_manual = true;
    state.collapsed_groups.insert("repo-two".into());
    state.collapsed_groups.insert("repo-one".into());
    let profile =
        SavedSshEndpoint::new("Build", "dev@build.example", "agents").expect("saved SSH profile");
    let remote_id = ClientEndpointId::Ssh(profile.id.clone());
    state
        .remote_collapsed_groups
        .insert(remote_id.clone(), HashSet::from(["/repo".to_owned()]));
    state.persist_chrome_preferences(&mut ClientShellInput::default());
    let stored = std::fs::read_to_string(&path).expect("stored client chrome preferences");
    assert!(stored.contains(profile.id.as_str()));
    assert!(stored.contains("repo-one"));
    assert!(!stored.contains(&profile.label));
    assert!(!stored.contains(&profile.target));

    let reloaded_config =
        ClientShellConfig::from_config(&Config::default()).with_preferences_path(path.clone());
    let reloaded = ClientShellState::new(reloaded_config);
    assert_eq!(reloaded.sidebar_width, 31);
    assert!(reloaded.sidebar_width_manual);
    assert_eq!(reloaded.sidebar_section_split, 0.7);
    assert!(reloaded.sidebar_section_split_manual);
    assert!(reloaded.sidebar_collapsed);
    assert!(reloaded.sidebar_collapsed_manual);
    assert_eq!(
        reloaded.collapsed_groups,
        HashSet::from(["repo-one".to_string(), "repo-two".to_string()])
    );
    assert_eq!(
        reloaded.remote_collapsed_groups.get(&remote_id),
        Some(&HashSet::from(["/repo".to_owned()]))
    );
    let mut reloaded = reloaded;
    reloaded.persist_chrome_preferences(&mut ClientShellInput::default());
    let stored_again = std::fs::read_to_string(&path).expect("restored client chrome preferences");
    assert!(stored_again.contains(profile.id.as_str()));
    assert!(stored_again.contains("/repo"));
    std::fs::remove_file(path).expect("remove client chrome preferences");
}

#[test]
fn tab_bar_renders_endpoint_status_ellipses_and_clamps_to_useful_scroll() {
    let mut projected = snapshot();
    projected.tab_bar_right = vec![
        crate::protocol::ClientShellTabStatusSegment {
            text: "ZOOM".into(),
            accent: true,
        },
        crate::protocol::ClientShellTabStatusSegment {
            text: "host".into(),
            accent: false,
        },
    ];
    projected.tab_bar_right_separator = " · ".into();
    for number in 2..=8 {
        projected.tabs.push(ClientShellTab {
            tab_id: format!("tab_{number}"),
            workspace_id: "ws_1".into(),
            number,
            label: number.to_string(),
            custom_label: false,
            zoomed: false,
            focused: false,
            agent_status: AgentStatus::Idle,
        });
    }
    let mut config = ClientShellConfig::from_config(&Config::default());
    config.mobile_width_threshold = 0;
    let mut state = ClientShellState::new(config);
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    let frame = state.compose(106, 20).expect("status and overflow tabs");
    let top = frame.cells[..frame.width as usize]
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect::<String>();
    assert!(top.contains("ZOOM · host"));
    assert!(top.contains('…'));

    state.tab_scroll = usize::MAX;
    state.reveal_focused_tab = false;
    state.compose(106, 20).expect("clamped tab scroll");
    assert!(state.tab_scroll < 7);
    let manual_scroll = state.tab_scroll;
    let mut replacement = (**state.snapshot.as_ref().expect("snapshot")).clone();
    replacement.revision = 2;
    replacement.tab_bar_right[1].text = "tick".into();
    let mut replacement_surface = surface();
    replacement_surface.projection_revision = 2;
    state.set_snapshot(Box::new(replacement));
    state.set_pane_surface(replacement_surface);
    assert!(!state.reveal_focused_tab);
    state.compose(106, 20).expect("same-width status update");
    assert_eq!(state.tab_scroll, manual_scroll);

    state.compose(45, 20).expect("narrow tabs win over status");
    let narrow = state.compose(45, 20).expect("narrow tab frame");
    let top = narrow.cells[..narrow.width as usize]
        .iter()
        .map(|cell| cell.symbol.as_str())
        .collect::<String>();
    assert!(!top.contains("ZOOM · host"));
}

#[test]
fn inactive_auto_named_tab_label_does_not_stack_terminal_faint() {
    let mut projected = snapshot();
    projected.tabs.push(ClientShellTab {
        tab_id: "tab_2".into(),
        workspace_id: "ws_1".into(),
        number: 2,
        label: "beta".into(),
        custom_label: false,
        zoomed: false,
        focused: false,
        agent_status: AgentStatus::Idle,
    });
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(projected));
    state.set_pane_surface(surface());
    let frame = state.compose(106, 20).expect("tab bar frame");
    let rect = state
        .hits
        .tabs
        .iter()
        .find(|(_, tab_id)| tab_id == "tab_2")
        .expect("inactive tab hit")
        .0;
    let buffer = frame.to_ratatui_buffer().expect("tab bar buffer");
    let (x, y) = cell_symbol_position(&frame, rect, "beta");
    let cell = buffer.cell((x, y)).expect("inactive tab cell");
    assert!(
        !cell.modifier.contains(Modifier::DIM),
        "inactive tab label at ({x},{y}) should not stack terminal faint: {cell:?}"
    );
}

#[test]
fn configured_prefix_is_client_owned_and_renders_its_bar() {
    let config = toml::from_str::<Config>(
        r#"
[keys]
prefix = "ctrl+a"
detach = "prefix+x"
"#,
    )
    .expect("configured keybinds");
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());

    let old_default = state.handle_input_bytes(&[0x02]);
    assert_eq!(
        old_default.requests.len(),
        1,
        "ctrl-b should reach the pane"
    );

    let prefix = state.handle_input_bytes(&[0x01]);
    assert!(prefix.requests.is_empty());
    assert!(prefix.repaint);
    let frame = state.compose(106, 20).expect("prefix frame");
    let text = frame
        .cells
        .chunks(frame.width as usize)
        .map(|row| {
            row.iter()
                .map(|cell| cell.symbol.as_str())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("PREFIX"), "frame: {text:?}");
    assert!(text.contains("ctrl+a"), "frame: {text:?}");

    let detach = state.handle_input_bytes(b"x");
    assert!(detach.detach);
    assert!(detach.requests.is_empty());
}

#[test]
fn prefix_endpoint_action_uses_public_api_with_stable_ids() {
    let mut config = Config::default();
    config.ui.prompt_new_tab_name = false;
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(snapshot()));

    assert!(state.handle_input_bytes(&[0x02]).actions.is_empty());
    let create = state.handle_input_bytes(b"c");
    let [ClientShellAction::Endpoint {
        boot_id, request, ..
    }] = &create.actions[..]
    else {
        panic!("expected one endpoint action: {:?}", create.actions);
    };
    assert_eq!(boot_id, "boot-1");
    match &request.method {
        crate::api::schema::Method::TabCreate(params) => {
            assert_eq!(params.workspace_id.as_deref(), Some("ws_1"));
            assert!(params.focus);
        }
        other => panic!("expected tab.create, got {other:?}"),
    }
    assert!(state.pending_requests.contains_key(&request.id));
}

#[test]
fn remote_keybinding_sources_keep_local_commands_off_endpoints_and_apply_server_profiles() {
    let local: Config = toml::from_str(
        r#"
[keys]
prefix = "ctrl+a"
new_tab = "prefix+c"

[[keys.command]]
key = "prefix+c"
command = "local-only"
"#,
    )
    .unwrap();
    let remote_local = ClientShellConfig::from_config(&local)
        .with_keybinding_source(ClientShellKeybindingSource::RemoteLocal);
    assert_eq!(remote_local.keybinds.prefix[0].0, KeyCode::Char('a'));
    assert!(remote_local.keybinds.keybinds.custom_commands.is_empty());
    assert_eq!(
        remote_local.keybinds.keybinds.new_tab.label().as_deref(),
        Some("prefix+c")
    );

    let mut local_state = ClientShellState::new(
        ClientShellConfig::from_config(&local)
            .with_keybinding_source(ClientShellKeybindingSource::Local),
    );
    let mut local_projection = snapshot();
    local_projection
        .commands
        .push(crate::protocol::ClientShellCommand {
            command_id: "cmd_loaded_endpoint".into(),
            binding_label: "prefix+c / prefix+y".into(),
            binding_labels: vec!["prefix+c".into(), "prefix+y".into()],
            action: crate::protocol::ClientShellCommandAction::Shell,
            description: Some("loaded endpoint command".into()),
        });
    local_state.set_snapshot(Box::new(local_projection));
    assert_eq!(
        local_state.config.keybinds.keybinds.custom_commands[0].label,
        "prefix+y"
    );
    assert_eq!(
        local_state
            .config
            .keybinds
            .keybinds
            .new_tab
            .label()
            .as_deref(),
        Some("prefix+c")
    );
    let mut command_outcome = ClientShellInput::default();
    local_state.record_binding(
        crate::input::KeybindMatch::Command(
            local_state.config.keybinds.keybinds.custom_commands[0].clone(),
        ),
        &mut command_outcome,
    );
    let [ClientShellAction::Endpoint { request, .. }] = &command_outcome.actions[..] else {
        panic!("expected surviving endpoint command binding");
    };
    let crate::api::schema::Method::CommandInvoke(params) = &request.method else {
        panic!("expected command invocation");
    };
    assert_eq!(params.command_id, "cmd_loaded_endpoint");

    let mut id_only_projection = snapshot();
    id_only_projection.revision = 2;
    id_only_projection
        .commands
        .push(crate::protocol::ClientShellCommand {
            command_id: "cmd_reloaded_endpoint".into(),
            binding_label: "prefix+c / prefix+y".into(),
            binding_labels: vec!["prefix+c".into(), "prefix+y".into()],
            action: crate::protocol::ClientShellCommandAction::Shell,
            description: Some("loaded endpoint command".into()),
        });
    local_state.mode = ClientShellMode::Prefix;
    local_state.set_snapshot(Box::new(id_only_projection));
    assert_eq!(local_state.mode, ClientShellMode::Prefix);
    assert_eq!(
        local_state.config.keybinds.keybinds.custom_commands[0].command,
        "cmd_reloaded_endpoint"
    );

    let endpoint: Config = toml::from_str(
        r#"
[keys]
prefix = "ctrl+x"
new_tab = "prefix+n"
"#,
    )
    .unwrap();
    let mut state = ClientShellState::new(
        ClientShellConfig::from_config(&local)
            .with_keybinding_source(ClientShellKeybindingSource::Endpoint),
    );
    let mut projection = snapshot();
    projection.server_keybindings_toml = endpoint.local_keybindings_profile_toml().ok();
    projection
        .commands
        .push(crate::protocol::ClientShellCommand {
            command_id: "cmd_remote".into(),
            binding_label: "prefix+z".into(),
            binding_labels: vec!["prefix+z".into()],
            action: crate::protocol::ClientShellCommandAction::Shell,
            description: Some("remote command".into()),
        });
    state.set_snapshot(Box::new(projection));

    assert_eq!(state.config.keybinds.prefix[0].0, KeyCode::Char('x'));
    assert_eq!(
        state.config.keybinds.keybinds.new_tab.label().as_deref(),
        Some("prefix+n")
    );
    assert_eq!(
        state.config.keybinds.keybinds.custom_commands[0].label,
        "prefix+z"
    );
    assert_eq!(
        state.config.keybinds.keybinds.custom_commands[0]
            .description
            .as_deref(),
        Some("remote command")
    );
    assert_eq!(
        state.config.keybinds.keybinds.custom_commands[0].command,
        "cmd_remote"
    );
}

#[test]
fn custom_binding_invokes_only_the_endpoint_manifest_id() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    let binding = crate::config::CustomCommandKeybind {
        bindings: crate::config::ActionKeybinds::prefix("z"),
        label: "prefix+z".into(),
        command: "secret-command --token hidden".into(),
        action: crate::config::CustomCommandAction::Shell,
        description: None,
        width: None,
        height: None,
    };
    let mut projection = snapshot();
    projection
        .commands
        .push(crate::protocol::ClientShellCommand {
            command_id: "cmd_0123456789abcdef0123456789abcdef".into(),
            binding_label: binding.label.clone(),
            binding_labels: binding.bindings.labels(),
            action: crate::protocol::ClientShellCommandAction::Shell,
            description: None,
        });
    state.set_snapshot(Box::new(projection));

    let mut outcome = ClientShellInput::default();
    state.record_binding(crate::input::KeybindMatch::Command(binding), &mut outcome);

    let [ClientShellAction::Endpoint { request, .. }] = &outcome.actions[..] else {
        panic!("expected endpoint command invocation");
    };
    let crate::api::schema::Method::CommandInvoke(params) = &request.method else {
        panic!("expected command.invoke");
    };
    assert_eq!(params.command_id, "cmd_0123456789abcdef0123456789abcdef");
    assert_eq!(params.workspace_id.as_deref(), Some("ws_1"));
    assert_eq!(params.tab_id.as_deref(), Some("tab_1"));
    assert_eq!(params.pane_id.as_deref(), Some("pane_1"));
    assert_eq!(params.selection, None);
    assert!(!serde_json::to_string(request)
        .unwrap()
        .contains("secret-command"));
}

#[test]
fn plugin_command_carries_client_owned_selection_coordinates() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    let binding = crate::config::CustomCommandKeybind {
        bindings: crate::config::ActionKeybinds::prefix("p"),
        label: "prefix+p".into(),
        command: "plugin.action".into(),
        action: crate::config::CustomCommandAction::PluginAction,
        description: None,
        width: None,
        height: None,
    };
    let mut projection = snapshot();
    projection
        .commands
        .push(crate::protocol::ClientShellCommand {
            command_id: "cmd_plugin".into(),
            binding_label: binding.label.clone(),
            binding_labels: binding.bindings.labels(),
            action: crate::protocol::ClientShellCommandAction::PluginAction,
            description: None,
        });
    state.set_snapshot(Box::new(projection));
    let mut pane_surface = surface();
    pane_surface.panes[0].content_revision = 42;
    state.set_pane_surface(pane_surface);
    let mut selection =
        crate::selection::Selection::absolute_range("pane_1".to_owned(), (2, 3), (4, 5));
    assert!(selection.finish());
    state.selection = Some(selection);

    let mut outcome = ClientShellInput::default();
    state.record_binding(crate::input::KeybindMatch::Command(binding), &mut outcome);

    let [ClientShellAction::Endpoint { request, .. }] = &outcome.actions[..] else {
        panic!("expected endpoint command invocation");
    };
    let crate::api::schema::Method::CommandInvoke(params) = &request.method else {
        panic!("expected command.invoke");
    };
    assert_eq!(
        params.selection,
        Some(crate::api::schema::PaneSelectionReadParams {
            pane_id: "pane_1".into(),
            anchor: crate::api::schema::PaneTextPoint { row: 2, col: 3 },
            cursor: crate::api::schema::PaneTextPoint { row: 4, col: 5 },
            content_revision: Some(42),
        })
    );
}

#[test]
fn unavailable_endpoint_method_is_disabled_without_disconnect() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.set_endpoint_methods(Some(vec!["pane.focus".into()]));
    let mut outcome = ClientShellInput::default();

    state.push_endpoint_method(
        crate::api::schema::Method::WorkspaceFocus(crate::api::schema::WorkspaceTarget {
            workspace_id: "missing".into(),
        }),
        &mut outcome,
    );

    assert!(outcome.actions.is_empty());
    assert!(outcome.repaint);
    let notice = state
        .visible_endpoint_notice
        .as_ref()
        .expect("unsupported action notice");
    assert_eq!(notice.key.kind, ClientEndpointNoticeKind::Unsupported);
    assert_eq!(notice.key.code, "workspace.focus");
    assert!(notice.body.contains("This server"));
    assert!(state.endpoint_error.is_none());

    let mut repeated = ClientShellInput::default();
    state.push_endpoint_method(
        crate::api::schema::Method::WorkspaceFocus(crate::api::schema::WorkspaceTarget {
            workspace_id: "missing".into(),
        }),
        &mut repeated,
    );
    assert!(repeated.actions.is_empty());
    assert!(!repeated.repaint);

    state.compose(106, 20).expect("endpoint notice frame");
    assert!(!state.hits.notification_toast.is_empty());

    state.handle_input_bytes(b"x");
    assert!(state.visible_endpoint_notice.is_some());

    let hit = state.hits.notification_toast;
    let dismissed =
        state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: hit.x,
            row: hit.y,
            modifiers: KeyModifiers::empty(),
        })]);
    assert!(dismissed.repaint);
    assert!(state.visible_endpoint_notice.is_none());
}

#[test]
fn generic_endpoint_failures_and_control_errors_are_visible() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    let mut outcome = ClientShellInput::default();
    state.push_endpoint_method(
        crate::api::schema::Method::WorkspaceFocus(crate::api::schema::WorkspaceTarget {
            workspace_id: "missing".into(),
        }),
        &mut outcome,
    );
    let request_id = match &outcome.actions[..] {
        [ClientShellAction::Endpoint { request, .. }] => request.id.clone(),
        other => panic!("expected generic endpoint request, got {other:?}"),
    };
    let rejected_at = std::time::Instant::now();
    let (repaint, actions) = state.handle_endpoint_result(
        "boot-1",
        &request_id,
        Err(ClientShellEndpointError {
            code: Some("not_found".into()),
            message: "workspace no longer exists".into(),
        }),
    );
    assert!(repaint);
    assert!(actions.is_empty());
    let notice = state
        .visible_endpoint_notice
        .as_ref()
        .expect("rejected action notice");
    assert_eq!(notice.key.kind, ClientEndpointNoticeKind::Rejected);
    assert_eq!(notice.key.code, "workspace.focus:not_found");
    assert_eq!(notice.body, "workspace no longer exists");
    let observed_at = std::time::Instant::now();
    assert!(notice.deadline >= rejected_at + std::time::Duration::from_secs(3));
    assert!(notice.deadline <= observed_at + std::time::Duration::from_secs(3));
    assert!(state.endpoint_error.is_none());

    assert!(state.receive_endpoint_error("Paste rejected: too large".into()));
    let notice = state
        .visible_endpoint_notice
        .as_ref()
        .expect("paste rejection notice");
    assert_eq!(notice.key.kind, ClientEndpointNoticeKind::Rejected);
    assert_eq!(notice.key.code, "paste_rejected");
    assert_eq!(notice.body, "Paste rejected: too large");
    assert!(state.receive_endpoint_error("Paste rejected again".into()));
    assert_eq!(
        state
            .visible_endpoint_notice
            .as_ref()
            .map(|notice| notice.body.as_str()),
        Some("Paste rejected again")
    );
}

#[test]
fn endpoint_timeout_is_a_deduplicated_server_notice() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    let mut outcome = ClientShellInput::default();
    state.push_endpoint_method(
        crate::api::schema::Method::WorkspaceFocus(crate::api::schema::WorkspaceTarget {
            workspace_id: "work".into(),
        }),
        &mut outcome,
    );
    let request_id = match &outcome.actions[..] {
        [ClientShellAction::Endpoint { request, .. }] => request.id.clone(),
        other => panic!("expected endpoint request, got {other:?}"),
    };

    let (repaint, actions) = state.handle_endpoint_result(
        "boot-1",
        &request_id,
        Err(ClientShellEndpointError {
            code: Some("endpoint_timeout".into()),
            message: "transport timeout".into(),
        }),
    );

    assert!(repaint);
    assert!(actions.is_empty());
    let notice = state
        .visible_endpoint_notice
        .as_ref()
        .expect("timeout notice");
    assert_eq!(notice.key.kind, ClientEndpointNoticeKind::Timeout);
    assert_eq!(notice.key.code, "workspace.focus");
    assert!(notice.body.contains("workspace.focus"));
    assert!(state.endpoint_error.is_none());
}

#[test]
fn endpoint_notice_dedupe_resets_for_a_new_server_boot() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_endpoint_methods(Some(Vec::new()));
    let method = || {
        crate::api::schema::Method::WorkspaceFocus(crate::api::schema::WorkspaceTarget {
            workspace_id: "work".into(),
        })
    };
    let mut first = ClientShellInput::default();
    state.push_endpoint_method(method(), &mut first);
    assert!(first.repaint);

    let mut next_snapshot = snapshot();
    next_snapshot.boot_id = "boot-2".into();
    state.set_snapshot(Box::new(next_snapshot));
    let mut second = ClientShellInput::default();
    state.push_endpoint_method(method(), &mut second);

    assert!(second.repaint);
    assert_eq!(
        state
            .visible_endpoint_notice
            .as_ref()
            .map(|notice| notice.key.boot_id.as_str()),
        Some("boot-2")
    );
}

#[test]
fn custom_binding_missing_from_endpoint_manifest_is_not_forwarded() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    let binding = crate::config::CustomCommandKeybind {
        bindings: crate::config::ActionKeybinds::prefix("z"),
        label: "prefix+z".into(),
        command: "secret-command".into(),
        action: crate::config::CustomCommandAction::Shell,
        description: None,
        width: None,
        height: None,
    };

    let mut outcome = ClientShellInput::default();
    state.record_binding(crate::input::KeybindMatch::Command(binding), &mut outcome);

    assert!(outcome.actions.is_empty());
    assert!(outcome.repaint);
    assert!(state
        .endpoint_error
        .as_deref()
        .is_some_and(|error| error.contains("not available")));
}

#[test]
fn help_overlay_restores_released_search_scroll_and_custom_binding_behavior() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    let mut projection = snapshot();
    projection
        .commands
        .push(crate::protocol::ClientShellCommand {
            command_id: "plugin-action".into(),
            binding_label: "prefix+z".into(),
            binding_labels: vec!["prefix+z".into()],
            action: crate::protocol::ClientShellCommandAction::PluginAction,
            description: Some("run plugin action".into()),
        });
    state.set_snapshot(Box::new(projection));
    state.set_pane_surface(surface());
    let mut open = ClientShellInput::default();
    state.record_binding(
        crate::input::KeybindMatch::Action(crate::input::KeybindAction::Help),
        &mut open,
    );
    let initial = state.compose(106, 30).expect("help overlay");
    let text = initial
        .cells
        .chunks(initial.width as usize)
        .map(|row| {
            row.iter()
                .map(|cell| cell.symbol.as_str())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("global"));
    assert!(state.hits.help_max_scroll > 0);
    assert_ne!(state.hits.help_scrollbar, Rect::default());

    state.handle_input_bytes(b"/");
    state.handle_input_bytes(b"plugin");
    let custom = state.compose(106, 30).expect("custom help search");
    let text = custom
        .cells
        .chunks(custom.width as usize)
        .map(|row| {
            row.iter()
                .map(|cell| cell.symbol.as_str())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("custom"));
    assert!(text.contains("run plugin action"));
    state.handle_input_bytes(b"\x1b");

    state.handle_input_bytes(b"/");
    state.handle_input_bytes(b"does-not-exist");
    let empty = state.compose(106, 30).expect("empty help search");
    let text = empty
        .cells
        .chunks(empty.width as usize)
        .map(|row| {
            row.iter()
                .map(|cell| cell.symbol.as_str())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("no matching keybinds"));

    state.handle_input_bytes(b"\x1b");
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Help(ClientHelpOverlay {
            search_focused: false,
            ref query,
            scroll: 0,
        })) if query.is_empty()
    ));
    state.compose(106, 30).expect("restored help");
    state.handle_input_bytes(b"\x1b[F");
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Help(ClientHelpOverlay { scroll, .. }))
            if scroll == state.hits.help_max_scroll
    ));
    state.handle_input_bytes(b"\x1b[H");
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Help(ClientHelpOverlay {
            scroll: 0,
            ..
        }))
    ));
    state.handle_input_bytes(b"?");
    assert!(state.overlay.is_none());
}

#[test]
fn resize_mode_reuses_endpoint_resize_and_stays_active_until_done() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());

    assert!(state.handle_input_bytes(&[0x02]).actions.is_empty());
    assert!(state.handle_input_bytes(b"r").actions.is_empty());
    assert_eq!(state.mode, ClientShellMode::Resize);

    let modified = state.handle_input_bytes(b"\x1b[1;2D");
    assert!(matches!(
        &modified.actions[..],
        [ClientShellAction::Endpoint { request, .. }]
            if matches!(
                &request.method,
                crate::api::schema::Method::PaneResize(params)
                    if params.direction == crate::api::schema::PaneDirection::Left
            )
    ));
    assert_eq!(state.mode, ClientShellMode::Resize);

    let resize = state.handle_input_bytes(b"h");
    let [ClientShellAction::Endpoint { request, .. }] = &resize.actions[..] else {
        panic!("resize should use endpoint API");
    };
    assert!(matches!(
        &request.method,
        crate::api::schema::Method::PaneResize(params)
            if params.pane_id.as_deref() == Some("pane_1")
                && params.direction == crate::api::schema::PaneDirection::Left
    ));
    assert_eq!(state.mode, ClientShellMode::Resize);

    assert!(state.handle_input_bytes(b"\r").actions.is_empty());
    assert_eq!(state.mode, ClientShellMode::Terminal);
}

#[test]
fn live_client_config_reloads_sidebar_tree() {
    let mut shell_config = ClientShellConfig::from_config(&Config::default());
    assert!(!shell_config.sidebar_tree);

    let mut config = Config::default();
    config.ui.sidebar_tree = true;
    shell_config.apply_live_config(&config, &[], &[]);
    assert!(shell_config.sidebar_tree);

    config.ui.sidebar_tree = false;
    shell_config.apply_live_config(&config, &[], &[]);
    assert!(!shell_config.sidebar_tree);
}

#[test]
fn live_client_config_reloads_tree_panel_switch() {
    let mut shell_config = ClientShellConfig::from_config(&Config::default());
    assert!(shell_config.sidebar_tree_panel);

    let mut config = Config::default();
    config.ui.sidebar_tree_panel = false;
    shell_config.apply_live_config(&config, &[], &[]);
    assert!(!shell_config.sidebar_tree_panel);

    shell_config.apply_live_config(&Config::default(), &[], &[]);
    assert!(shell_config.sidebar_tree_panel);
}

#[test]
fn live_client_config_reloads_tree_icons() {
    use crate::config::SidebarTreeIconsConfig;
    let mut shell_config = ClientShellConfig::from_config(&Config::default());
    assert_eq!(
        shell_config.sidebar_tree_icons,
        SidebarTreeIconsConfig::Plain
    );
    let mut config = Config::default();
    config.ui.sidebar_tree_icons = SidebarTreeIconsConfig::Nerd;
    shell_config.apply_live_config(&config, &[], &[]);
    assert_eq!(
        shell_config.sidebar_tree_icons,
        SidebarTreeIconsConfig::Nerd
    );
    shell_config.apply_live_config(&Config::default(), &[], &[]);
    assert_eq!(
        shell_config.sidebar_tree_icons,
        SidebarTreeIconsConfig::Plain
    );
}

#[test]
fn settings_overlay_saves_and_applies_tree_detail_switches() {
    struct ConfigPathGuard(Option<std::ffi::OsString>, std::path::PathBuf);
    impl Drop for ConfigPathGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.1);
            match self.0.take() {
                Some(previous) => std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, previous),
                None => std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR),
            }
        }
    }
    let dir = std::env::temp_dir().join(format!(
        "herdr-client-shell-tree-details-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    std::fs::write(&path, "[ui]\nconfirm_close = false\n").unwrap();
    let _guard = ConfigPathGuard(
        std::env::var_os(crate::config::CONFIG_PATH_ENV_VAR),
        dir.clone(),
    );
    std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

    let frame_text = |state: &mut ClientShellState| {
        let frame = state.compose(106, 30).expect("settings overlay");
        frame
            .cells
            .chunks(frame.width as usize)
            .map(|row| {
                row.iter()
                    .map(|cell| cell.symbol.as_str())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.open_settings_overlay();
    for _ in 0..5 {
        state.handle_input_bytes(b"\t");
    }
    let text = frame_text(&mut state);
    for line in ["sidebar tree: off", "needs-you panel: on"] {
        assert!(text.contains(line), "{line} missing:\n{text}");
    }
    assert!(
        !text.contains("agent tasks"),
        "the tasks line is gone:\n{text}"
    );

    state.handle_input_bytes(b"j");
    state.handle_input_bytes(b"\r");
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(
        written.contains("sidebar_tree_panel = false"),
        "unexpected {written:?}"
    );
    assert!(!state.config.sidebar_tree_panel);

    state.handle_input_bytes(b"\r");
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(
        written.contains("sidebar_tree_panel = true"),
        "unexpected {written:?}"
    );
    assert!(state.config.sidebar_tree_panel);
    assert!(!state.config.sidebar_tree, "other switches untouched");
    assert!(written.contains("confirm_close = false"));
}

#[test]
fn settings_indicators_scroll_to_keep_the_selected_choice_drawn_in_a_short_window() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.open_settings_overlay();
    state.handle_input_bytes(b"\t");
    // Two status styles, twelve spinner sets for the main agent, and auto then twelve
    // sets for its sub-agents; the cursor starts on the first.
    for step in 0..27 {
        state.compose(80, 24).expect("settings overlay");
        let selected = match state.overlay.as_ref() {
            Some(ClientShellOverlay::Settings(settings)) => settings.selected,
            _ => panic!("settings open"),
        };
        assert_eq!(selected, step);
        assert!(
            state
                .hits
                .settings_choices
                .iter()
                .any(|(_, index)| *index == selected),
            "choice {selected} is drawn at 80x24: {:?}",
            state.hits.settings_choices
        );
        state.handle_input_bytes(b"j");
    }
}

#[test]
fn settings_indicators_draw_every_choice_at_once_in_a_tall_window() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.open_settings_overlay();
    state.handle_input_bytes(b"\t");
    state.compose(106, 80).expect("settings overlay");
    let mut drawn = state
        .hits
        .settings_choices
        .iter()
        .map(|(_, index)| *index)
        .collect::<Vec<_>>();
    drawn.sort_unstable();
    assert_eq!(drawn, (0..IndicatorChoices::LINES).collect::<Vec<_>>());
}

#[test]
fn settings_overlay_lists_and_saves_the_working_spinner() {
    struct ConfigPathGuard(Option<std::ffi::OsString>, std::path::PathBuf);
    impl Drop for ConfigPathGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.1);
            match self.0.take() {
                Some(previous) => std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, previous),
                None => std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR),
            }
        }
    }
    let dir = std::env::temp_dir().join(format!(
        "herdr-client-shell-working-spinner-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    std::fs::write(&path, "[ui]\nstatus_indicators = \"symbols\"\n").unwrap();
    let _guard = ConfigPathGuard(
        std::env::var_os(crate::config::CONFIG_PATH_ENV_VAR),
        dir.clone(),
    );
    std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

    let mut config = Config::default();
    config.ui.status_indicators = crate::config::StatusIndicatorStyle::Symbols;
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.open_settings_overlay();
    state.handle_input_bytes(b"\t");
    let frame = state.compose(106, 40).expect("settings overlay");
    let text = frame
        .cells
        .chunks(frame.width as usize)
        .map(|row| {
            row.iter()
                .map(|cell| cell.symbol.as_str())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    for label in [
        "agent status indicators",
        "main agent spinner",
        "nerd spinner",
        "braille ✓",
        "circle slices",
        "moon phases",
        "arc",
        "half circles",
        "braille chase",
        "dot orbit",
        "pulse",
        "line",
        "star",
        "diamond",
    ] {
        assert!(text.contains(label), "{label:?} missing:\n{text}");
    }
    for dropped in ["braille full", "clock", "blocks"] {
        assert!(!text.contains(dropped), "{dropped:?} is listed:\n{text}");
    }

    // The cursor starts on the status style in use (symbols, the second line); the
    // spinner sets follow, so six down is half circles, the sixth set.
    for _ in 0..6 {
        state.handle_input_bytes(b"j");
    }
    state.handle_input_bytes(b"\r");
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(
        written.contains("working_spinner = \"half_circles\""),
        "unexpected {written:?}"
    );
    assert!(written.contains("status_indicators = \"symbols\""));
    assert_eq!(
        state.config.working_spinner,
        crate::config::WorkingSpinnerStyle::HalfCircles
    );
    assert_eq!(
        state.config.status_indicators,
        crate::config::StatusIndicatorStyle::Symbols,
        "the status style is untouched"
    );
}

#[test]
fn settings_indicators_list_select_and_save_both_spinners_at_80x24() {
    struct ConfigPathGuard(Option<std::ffi::OsString>, std::path::PathBuf);
    impl Drop for ConfigPathGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.1);
            match self.0.take() {
                Some(previous) => std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, previous),
                None => std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR),
            }
        }
    }
    let dir = std::env::temp_dir().join(format!(
        "herdr-client-shell-sub-agent-spinner-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    std::fs::write(&path, "[ui]\nworking_spinner = \"moon_phases\"\n").unwrap();
    let _guard = ConfigPathGuard(
        std::env::var_os(crate::config::CONFIG_PATH_ENV_VAR),
        dir.clone(),
    );
    std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

    let mut config = Config::default();
    config.ui.working_spinner = crate::config::WorkingSpinnerStyle::MoonPhases;
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.open_settings_overlay();
    state.handle_input_bytes(b"\t");
    let screen = |state: &mut ClientShellState| {
        let frame = state.compose(80, 24).expect("settings overlay");
        frame
            .cells
            .chunks(frame.width as usize)
            .map(|row| {
                row.iter()
                    .map(|cell| cell.symbol.as_str())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    // Walk every choice: the status styles, the main agent's twelve sets, then the
    // sub-agents' auto and twelve sets.
    let mut seen = String::new();
    let mut at_auto = String::new();
    for step in 0..27 {
        let text = screen(&mut state);
        if step == 14 {
            at_auto.clone_from(&text);
        }
        seen.push_str(&text);
        assert!(
            state
                .hits
                .settings_choices
                .iter()
                .any(|(_, index)| *index == step),
            "choice {step} is drawn at 80x24"
        );
        state.handle_input_bytes(b"j");
    }
    for label in ["main agent spinner", "sub-agent spinner"] {
        assert!(seen.contains(label), "{label:?} missing");
    }
    // The sub-agents' list marks its own choice in use: auto, showing the set it resolves
    // to, moon phases' partner arc.
    assert!(at_auto.contains("sub-agent spinner"), "{at_auto}");
    assert!(
        at_auto.contains("auto (goes with main: arc) ✓"),
        "{at_auto}"
    );

    // The last choice is the sub-agents' diamond; apply saves only the sub-agent key.
    state.handle_input_bytes(b"\r");
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(
        written.contains("sub_agent_spinner = \"diamond\""),
        "unexpected {written:?}"
    );
    assert!(
        written.contains("working_spinner = \"moon_phases\""),
        "{written:?}"
    );
    assert_eq!(
        state.config.sub_agent_spinner,
        crate::config::SubAgentSpinnerStyle::Diamond
    );
    assert_eq!(
        state.config.working_spinner,
        crate::config::WorkingSpinnerStyle::MoonPhases,
        "the main agent's spinner is untouched"
    );
    let text = screen(&mut state);
    assert!(text.contains("diamond ✓"), "{text}");

    // Back up to the main agent's arc and apply: only the main key changes, and the
    // sub-agents' pick survives it.
    for _ in 0..(26 - 6) {
        state.handle_input_bytes(b"k");
    }
    state.handle_input_bytes(b"\r");
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(
        written.contains("working_spinner = \"arc\""),
        "unexpected {written:?}"
    );
    assert!(written.contains("sub_agent_spinner = \"diamond\""));
    assert_eq!(
        state.config.sub_agent_spinner,
        crate::config::SubAgentSpinnerStyle::Diamond
    );

    // Auto again: the row names arc's partner, dot orbit.
    for _ in 0..(14 - 6) {
        state.handle_input_bytes(b"j");
    }
    state.handle_input_bytes(b"\r");
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(
        written.contains("sub_agent_spinner = \"auto\""),
        "unexpected {written:?}"
    );
    let text = screen(&mut state);
    assert!(
        text.contains("auto (goes with main: dot orbit) ✓"),
        "{text}"
    );
}

#[test]
fn settings_overlay_saves_and_applies_sidebar_tree() {
    struct ConfigPathGuard(Option<std::ffi::OsString>, std::path::PathBuf);
    impl Drop for ConfigPathGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.1);
            match self.0.take() {
                Some(previous) => std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, previous),
                None => std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR),
            }
        }
    }
    let dir = std::env::temp_dir().join(format!(
        "herdr-client-shell-sidebar-tree-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    std::fs::write(&path, "[ui]\nconfirm_close = false\n").unwrap();
    let _guard = ConfigPathGuard(
        std::env::var_os(crate::config::CONFIG_PATH_ENV_VAR),
        dir.clone(),
    );
    std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

    let frame_text = |state: &mut ClientShellState| {
        let frame = state.compose(106, 30).expect("settings overlay");
        frame
            .cells
            .chunks(frame.width as usize)
            .map(|row| {
                row.iter()
                    .map(|cell| cell.symbol.as_str())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let reloads_server_config = |outcome: &ClientShellInput| {
        outcome.actions.iter().any(|action| {
            matches!(
                action,
                ClientShellAction::Endpoint { request, .. }
                    if matches!(
                        request.method,
                        crate::api::schema::Method::ServerReloadConfig(_)
                    )
            )
        })
    };

    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.open_settings_overlay();
    assert!(frame_text(&mut state).contains(" sidebar "));

    for _ in 0..5 {
        state.handle_input_bytes(b"\t");
    }
    let text = frame_text(&mut state);
    assert!(
        text.contains("sidebar tree"),
        "sidebar section missing:\n{text}"
    );
    assert!(!state.config.sidebar_tree);

    state.handle_input_bytes(b"k");
    let enable = state.handle_input_bytes(b"\r");
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(
        written.contains("sidebar_tree = true"),
        "unexpected {written:?}"
    );
    assert!(written.contains("confirm_close = false"));
    assert!(state.config.sidebar_tree);
    assert!(reloads_server_config(&enable));
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Settings(_))
    ));

    // Enter switches the selected line, so a second Enter turns the tree off again.
    let disable = state.handle_input_bytes(b"\r");
    let written = std::fs::read_to_string(&path).unwrap();
    assert!(
        written.contains("sidebar_tree = false"),
        "unexpected {written:?}"
    );
    assert!(!state.config.sidebar_tree);
    assert!(reloads_server_config(&disable));
}
