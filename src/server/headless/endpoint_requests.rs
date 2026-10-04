use super::*;

impl HeadlessServer {
    pub(super) fn handle_client_shell_endpoint_request(
        &mut self,
        client_id: u64,
        boot_id: String,
        mut request: Box<api::schema::Request>,
    ) -> bool {
        let Some(client) = self.clients.get(&client_id) else {
            return false;
        };
        if !matches!(client.mode, ClientConnectionMode::ClientShell) {
            self.remove_client_and_resize_if_needed(client_id);
            return true;
        }
        let request_id = request.id.clone();
        if !crate::server::client_commands::supports_client_shell_method(&request.method) {
            let message = crate::server::client_commands::error_message(
                boot_id,
                request_id,
                "unsupported_endpoint_command",
                "this method is not available through the client shell command lane",
            );
            self.send_to_client(client_id, message);
            return false;
        }
        if boot_id != self.client_shell_boot_id {
            let message = crate::server::client_commands::error_message(
                boot_id,
                request_id,
                "stale_boot",
                "endpoint command targeted an earlier server boot",
            );
            self.send_to_client(client_id, message);
            return false;
        }
        let surface_active = client.shell_surface_active;
        if let api::schema::Method::ClientShellSurfaceSet(params) = &request.method {
            let Some((changed, projection_revision)) =
                self.set_client_shell_surface_active(client_id, params.active)
            else {
                return false;
            };
            self.send_to_client(
                client_id,
                crate::server::client_commands::success_message_with_result(
                    boot_id,
                    request_id,
                    api::schema::ResponseResult::ClientShellSurfaceSet {
                        active: params.active,
                        projection_revision,
                    },
                ),
            );
            return changed;
        }
        // `tasks.list`, `tasks.act` and `tasks.open` run beside the ordered lane: one of each at
        // a time per shell, and they may wait together.
        let side_slot = match &request.method {
            api::schema::Method::TasksList(_) => Some(SideSlot::List),
            api::schema::Method::TasksAct(_) => Some(SideSlot::Act),
            api::schema::Method::TasksOpen(_) => Some(SideSlot::Open),
            _ => None,
        };
        let side = side_slot.is_some();
        let busy = match side_slot {
            Some(slot) => slot.waiting(client).is_some(),
            None => client.shell_endpoint_command_in_flight,
        };
        if busy {
            let message = crate::server::client_commands::error_message(
                boot_id,
                request_id,
                "endpoint_busy",
                "this endpoint is still processing another command",
            );
            self.send_to_client(client_id, message);
            return false;
        }
        if !surface_active {
            let message = crate::server::client_commands::error_message(
                boot_id,
                request_id,
                "surface_inactive",
                "this method requires an active client shell surface",
            );
            self.send_to_client(client_id, message);
            return false;
        }

        let api_request_id = format!(
            "endpoint:{}:{client_id}:{request_id}",
            self.client_shell_boot_id
        );
        request.id = api_request_id.clone();
        let (respond_to, response_rx) = std::sync::mpsc::channel();
        if let Err(err) = crate::server::client_commands::spawn_response_waiter(
            client_id,
            boot_id.clone(),
            request_id.clone(),
            response_rx,
            self.server_event_tx.clone(),
            !side,
        ) {
            let message = crate::server::client_commands::error_message(
                boot_id,
                request_id,
                "server_unavailable",
                format!("failed to start endpoint response bridge: {err}"),
            );
            self.send_to_client(client_id, message);
            return false;
        }
        if let Some(slot) = side_slot {
            if let Some(client) = self.clients.get_mut(&client_id) {
                *slot.waiting_mut(client) = Some(request_id);
            }
            let msg = api::ApiRequestMessage {
                request: *request,
                respond_to,
                response_write_complete: None,
            };
            let opens = match &msg.request.method {
                api::schema::Method::TasksOpen(params) => {
                    self.app.check_task_file_listed(&params.file).is_ok()
                }
                _ => false,
            };
            if opens {
                // The popup opens in this shell's space and takes its input. A refused open
                // is answered below and moves nothing.
                let foreground_changed = self.promote_client_to_foreground(client_id);
                return foreground_changed | self.handle_client_shell_api_request(client_id, msg);
            }
            // Reading the task list, an action or a refused open neither focuses this shell nor
            // moves anything.
            return self.handle_api_request_with_shutdown_check_inner(msg, false, true);
        }
        if let Some(client) = self.clients.get_mut(&client_id) {
            client.shell_endpoint_command_in_flight = true;
            // A later source restore has a new projection revision. Keep this request's lease
            // so a delayed worktree response cannot focus a pane after endpoint switching.
            client.shell_endpoint_command_surface_revision = Some(client.shell_projection_revision);
            let deferred_worktree = matches!(
                &request.method,
                api::schema::Method::WorktreeCreate(_)
                    | api::schema::Method::WorktreeRemove(_)
                    | api::schema::Method::WorktreeList(_)
                    | api::schema::Method::WorktreeOpen(_)
            );
            let deferred_navigation = match &request.method {
                api::schema::Method::WorktreeCreate(params) => params.focus,
                api::schema::Method::WorktreeOpen(params) => params.focus,
                _ => false,
            };
            client.shell_deferred_navigation_request_id =
                deferred_worktree.then(|| api_request_id.clone());
            client.shell_deferred_navigation_response = deferred_navigation.then(Vec::new);
        }
        let foreground_changed = self.promote_client_to_foreground(client_id);
        foreground_changed
            | self.handle_client_shell_api_request(
                client_id,
                api::ApiRequestMessage {
                    request: *request,
                    respond_to,
                    response_write_complete: None,
                },
            )
    }
}

/// Which of a shell's side-lane waiting slots a request uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SideSlot {
    List,
    Act,
    Open,
}

impl SideSlot {
    fn waiting(self, client: &ClientConnection) -> Option<&String> {
        match self {
            Self::List => client.shell_side_list_request_id.as_ref(),
            Self::Act => client.shell_side_act_request_id.as_ref(),
            Self::Open => client.shell_side_open_request_id.as_ref(),
        }
    }

    fn waiting_mut(self, client: &mut ClientConnection) -> &mut Option<String> {
        match self {
            Self::List => &mut client.shell_side_list_request_id,
            Self::Act => &mut client.shell_side_act_request_id,
            Self::Open => &mut client.shell_side_open_request_id,
        }
    }
}
