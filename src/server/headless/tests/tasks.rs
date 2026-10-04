use super::*;

fn fixture(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/app/api/tasks/fixtures")
        .join(name)
}

fn send_api_request(
    server: &mut HeadlessServer,
    id: &str,
    method: api::schema::Method,
) -> std::sync::mpsc::Receiver<String> {
    let (respond_to, response_rx) = std::sync::mpsc::channel();
    server.handle_api_request_with_shutdown_check(api::ApiRequestMessage {
        request: api::schema::Request {
            id: id.into(),
            method,
        },
        respond_to,
        response_write_complete: None,
    });
    response_rx
}

#[cfg(unix)]
#[tokio::test]
async fn server_keeps_answering_while_a_slow_tasks_list_runs() {
    let mut server = test_headless_server();
    server.app.tasks_command = Some(fixture("list-slow.sh"));

    let tasks_rx = send_api_request(
        &mut server,
        "slow-tasks",
        api::schema::Method::TasksList(api::schema::EmptyParams::default()),
    );
    let panes_rx = send_api_request(
        &mut server,
        "list-during-tasks",
        api::schema::Method::PaneList(api::schema::PaneListParams::default()),
    );

    let panes: api::schema::SuccessResponse = serde_json::from_str(
        &panes_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("pane.list answered while tasks.list runs"),
    )
    .unwrap();
    assert_eq!(panes.id, "list-during-tasks");
    assert!(
        tasks_rx.try_recv().is_err(),
        "the slow tasks.list must still be running"
    );
}

/// A client shell with one workspace, connected and past its first snapshot: its control
/// and render channels, both kept open so the client stays connected.
fn connected_shell(
    server: &mut HeadlessServer,
    client_id: u64,
) -> (
    std::sync::mpsc::Receiver<Vec<u8>>,
    std::sync::mpsc::Receiver<Vec<u8>>,
) {
    server.app.state.workspaces = vec![crate::workspace::Workspace::test_new("tasks")];
    server.app.state.ensure_test_terminals();
    server.app.state.active = Some(0);
    connect_shell(server, client_id)
}

/// Connects one more client shell to the server's current workspaces; it becomes the
/// foreground client.
fn connect_shell(
    server: &mut HeadlessServer,
    client_id: u64,
) -> (
    std::sync::mpsc::Receiver<Vec<u8>>,
    std::sync::mpsc::Receiver<Vec<u8>>,
) {
    let (writer, control_rx, render_rx) = test_client_writer();
    server.handle_server_event(ServerEvent::ClientShellConnected {
        surface_reuse: false,
        surface_delta: false,
        surface_scroll: false,
        client_id,
        surface_cols: 80,
        surface_rows: 23,
        cell_width_px: 0,
        cell_height_px: 0,
        pixel_mouse: false,
        direct_graphics: false,
        endpoint_keybindings: false,
        mouse_capture: false,
        surface_active: true,
        writer,
    });
    let _initial_snapshot = client_shell_snapshot(&control_rx);
    (control_rx, render_rx)
}

fn shell_request(
    server: &HeadlessServer,
    client_id: u64,
    id: &str,
    method: api::schema::Method,
) -> ServerEvent {
    ServerEvent::ClientShellEndpointRequest {
        client_id,
        boot_id: server.client_shell_boot_id.clone(),
        request: Box::new(api::schema::Request {
            id: id.into(),
            method,
        }),
    }
}

/// Runs server events until the client gets the whole reply to `request_id`.
async fn shell_reply(
    server: &mut HeadlessServer,
    control_rx: &std::sync::mpsc::Receiver<Vec<u8>>,
    request_id: &str,
) -> String {
    let mut reply = Vec::new();
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            while let Ok(bytes) = control_rx.try_recv() {
                if let ServerMessage::ClientShellEndpointResponseChunk {
                    request_id: id,
                    final_chunk,
                    data,
                    ..
                } = read_server_message(bytes)
                {
                    if id == request_id {
                        reply.extend_from_slice(&data);
                        if final_chunk {
                            return;
                        }
                    }
                }
            }
            tokio::select! {
                Some(event) = server.server_event_rx.recv() => {
                    server.handle_server_event(event);
                }
                Some(event) = server.app.event_rx.recv() => {
                    server.handle_internal_event_with_forwarding(event);
                }
                // The client writer delivers on its own thread; look again shortly.
                () = tokio::time::sleep(Duration::from_millis(10)) => {}
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("a reply to {request_id}"));
    String::from_utf8(reply).unwrap()
}

#[cfg(unix)]
#[tokio::test]
async fn a_waiting_tasks_list_leaves_the_client_lane_to_other_commands() {
    let mut server = test_headless_server();
    let (control_rx, _render_rx) = connected_shell(&mut server, 61);
    server.app.tasks_command = Some(fixture("list-slow.sh"));
    let root = server.app.state.workspaces[0].tabs[0].root_pane;
    let pane_id = server.app.public_pane_id(0, root).unwrap();

    let tasks = shell_request(
        &server,
        61,
        "client-shell:tasks",
        api::schema::Method::TasksList(api::schema::EmptyParams::default()),
    );
    server.handle_server_event(tasks);
    assert!(
        !server.clients[&61].shell_endpoint_command_in_flight,
        "tasks.list does not hold the ordered lane"
    );
    let focus = shell_request(
        &server,
        61,
        "client-shell:focus",
        api::schema::Method::PaneFocus(api::schema::PaneTarget { pane_id }),
    );
    server.handle_server_event(focus);

    let reply = shell_reply(&mut server, &control_rx, "client-shell:focus").await;
    let success: api::schema::SuccessResponse = serde_json::from_str(&reply)
        .unwrap_or_else(|_| panic!("pane.focus succeeds while tasks.list waits: {reply}"));
    assert_eq!(success.id, "client-shell:focus");

    let again = shell_request(
        &server,
        61,
        "client-shell:tasks-again",
        api::schema::Method::TasksList(api::schema::EmptyParams::default()),
    );
    server.handle_server_event(again);
    let reply = shell_reply(&mut server, &control_rx, "client-shell:tasks-again").await;
    let error: api::schema::ErrorResponse = serde_json::from_str(&reply).unwrap();
    assert_eq!(
        error.error.code, "endpoint_busy",
        "one tasks.list at a time per client"
    );
    shutdown_test_runtimes(&mut server);
}

#[cfg(unix)]
#[tokio::test]
async fn a_client_tasks_list_reply_reaches_the_client() {
    let mut server = test_headless_server();
    let (control_rx, _render_rx) = connected_shell(&mut server, 62);
    server.app.tasks_command = Some(fixture("list-example.sh"));

    let tasks = shell_request(
        &server,
        62,
        "client-shell:tasks",
        api::schema::Method::TasksList(api::schema::EmptyParams::default()),
    );
    server.handle_server_event(tasks);
    let reply = shell_reply(&mut server, &control_rx, "client-shell:tasks").await;

    let success: api::schema::SuccessResponse = serde_json::from_str(&reply).unwrap();
    assert!(matches!(
        success.result,
        api::schema::ResponseResult::TasksList { .. }
    ));
    let next = shell_request(
        &server,
        62,
        "client-shell:tasks-next",
        api::schema::Method::TasksList(api::schema::EmptyParams::default()),
    );
    server.handle_server_event(next);
    let reply = shell_reply(&mut server, &control_rx, "client-shell:tasks-next").await;
    assert!(
        serde_json::from_str::<api::schema::SuccessResponse>(&reply).is_ok(),
        "the finished tasks.list freed its place: {reply}"
    );
    shutdown_test_runtimes(&mut server);
}

#[cfg(unix)]
#[tokio::test]
async fn a_waiting_tasks_act_leaves_the_client_lane_to_other_commands() {
    let mut server = test_headless_server();
    let (control_rx, _render_rx) = connected_shell(&mut server, 63);
    server.app.tasks_command = Some(fixture("list-slow.sh"));
    let root = server.app.state.workspaces[0].tabs[0].root_pane;
    let pane_id = server.app.public_pane_id(0, root).unwrap();

    let act = shell_request(
        &server,
        63,
        "client-shell:act",
        api::schema::Method::TasksAct(api::schema::TasksActParams {
            action: "done".into(),
            copies: vec![api::schema::TaskCopy {
                file: "/nonexistent/STATUS.md".into(),
                row: "| a | me |".into(),
            }],
            ..Default::default()
        }),
    );
    server.handle_server_event(act);
    assert!(
        !server.clients[&63].shell_endpoint_command_in_flight,
        "tasks.act does not hold the ordered lane"
    );
    let focus = shell_request(
        &server,
        63,
        "client-shell:focus",
        api::schema::Method::PaneFocus(api::schema::PaneTarget { pane_id }),
    );
    server.handle_server_event(focus);

    let reply = shell_reply(&mut server, &control_rx, "client-shell:focus").await;
    assert!(
        serde_json::from_str::<api::schema::SuccessResponse>(&reply).is_ok(),
        "pane.focus succeeds while tasks.act waits: {reply}"
    );
    shutdown_test_runtimes(&mut server);
}

#[cfg(unix)]
#[tokio::test]
async fn a_client_may_wait_on_one_action_and_one_list_at_once() {
    let mut server = test_headless_server();
    let (control_rx, _render_rx) = connected_shell(&mut server, 64);
    server.app.tasks_command = Some(fixture("list-slow.sh"));
    let act = |id: &str| {
        api::schema::Method::TasksAct(api::schema::TasksActParams {
            action: "done".into(),
            copies: vec![api::schema::TaskCopy {
                file: "/nonexistent/STATUS.md".into(),
                row: format!("| {id} | me |"),
            }],
            ..Default::default()
        })
    };

    let first = shell_request(&server, 64, "client-shell:act", act("a"));
    server.handle_server_event(first);
    let list = shell_request(
        &server,
        64,
        "client-shell:tasks",
        api::schema::Method::TasksList(api::schema::EmptyParams::default()),
    );
    server.handle_server_event(list);
    let client = &server.clients[&64];
    assert_eq!(
        (
            client.shell_side_act_request_id.as_deref(),
            client.shell_side_list_request_id.as_deref()
        ),
        (Some("client-shell:act"), Some("client-shell:tasks")),
        "an action and a list wait at the same time"
    );

    let second = shell_request(&server, 64, "client-shell:act-again", act("b"));
    server.handle_server_event(second);
    let reply = shell_reply(&mut server, &control_rx, "client-shell:act-again").await;
    let error: api::schema::ErrorResponse = serde_json::from_str(&reply).unwrap();
    assert_eq!(
        error.error.code, "endpoint_busy",
        "one action at a time per client"
    );
    shutdown_test_runtimes(&mut server);
}

#[cfg(unix)]
#[tokio::test]
async fn a_client_tasks_open_is_answered_beside_the_client_lane() {
    let mut server = test_headless_server();
    let (control_rx, _render_rx) = connected_shell(&mut server, 64);
    let open = || {
        api::schema::Method::TasksOpen(api::schema::TasksOpenParams {
            file: "/nonexistent/STATUS.md".into(),
        })
    };

    server.handle_server_event(shell_request(&server, 64, "client-shell:open", open()));
    assert!(
        !server.clients[&64].shell_endpoint_command_in_flight,
        "tasks.open does not hold the ordered lane"
    );
    let reply = shell_reply(&mut server, &control_rx, "client-shell:open").await;
    let error: api::schema::ErrorResponse = serde_json::from_str(&reply).unwrap();
    assert_eq!(
        error.error.code, "tasks_not_listed",
        "no kept list, so nothing opens: {reply}"
    );
    assert!(server.app.state.popup_pane.is_none());
    assert_eq!(server.clients[&64].shell_side_open_request_id, None);

    server.handle_server_event(shell_request(
        &server,
        64,
        "client-shell:open-again",
        open(),
    ));
    let reply = shell_reply(&mut server, &control_rx, "client-shell:open-again").await;
    assert!(
        reply.contains("tasks_not_listed"),
        "a finished tasks.open frees its slot: {reply}"
    );
    shutdown_test_runtimes(&mut server);
}

#[cfg(unix)]
#[tokio::test]
async fn only_a_tasks_open_that_opens_moves_its_client_to_the_foreground() {
    let mut server = test_headless_server();
    let (asker_rx, _asker_render) = connected_shell(&mut server, 65);
    let (_other_rx, _other_render) = connect_shell(&mut server, 66);
    assert_eq!(server.foreground_client_id, Some(66));
    /// Removes the test's notes folder even when an assertion fails.
    struct RemoveOnDrop(std::path::PathBuf);
    impl Drop for RemoveOnDrop {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let notes = RemoveOnDrop(std::env::temp_dir().join(format!(
        "herdr-tasks-open-foreground-{}",
        std::process::id()
    )));
    std::fs::create_dir_all(&notes.0).unwrap();
    let file = notes.0.join("STATUS.md").to_str().unwrap().to_owned();
    std::fs::write(&file, "| Task | Waiting on |\n").unwrap();
    let open = |file: &str| {
        api::schema::Method::TasksOpen(api::schema::TasksOpenParams { file: file.into() })
    };

    server.handle_server_event(shell_request(&server, 65, "no-list", open(&file)));
    let reply = shell_reply(&mut server, &asker_rx, "no-list").await;
    assert!(reply.contains("tasks_not_listed"), "{reply}");
    assert_eq!(
        server.foreground_client_id,
        Some(66),
        "a refused open (no kept list) leaves the foreground client"
    );

    server.app.tasks_list.keep_list_for_test(serde_json::json!({
        "version": 1, "tasks": [], "work": [{"file": file}], "problems": [],
    }));
    server.handle_server_event(shell_request(&server, 65, "unlisted", open("/etc/hosts")));
    let reply = shell_reply(&mut server, &asker_rx, "unlisted").await;
    assert!(reply.contains("tasks_not_listed"), "{reply}");
    assert_eq!(
        server.foreground_client_id,
        Some(66),
        "a refused open (unlisted file) leaves the foreground client"
    );

    server.handle_server_event(shell_request(&server, 65, "listed", open(&file)));
    let reply = shell_reply(&mut server, &asker_rx, "listed").await;
    assert!(
        serde_json::from_str::<api::schema::SuccessResponse>(&reply).is_ok(),
        "{reply}"
    );
    assert_eq!(
        server.foreground_client_id,
        Some(65),
        "the client whose popup opens takes the foreground"
    );
    assert!(server.app.close_popup_pane());
    shutdown_test_runtimes(&mut server);
}
