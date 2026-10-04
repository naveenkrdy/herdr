use super::*;

impl ClientContextMenuOverlay {
    pub(super) fn items(&self) -> Vec<ClientContextMenuItem> {
        use ClientContextMenuAction as Action;

        match &self.target {
            ClientContextMenuTarget::Workspace { space, .. } => space_items(*space, false),
            ClientContextMenuTarget::Tab { splits, .. } => {
                let mut items = vec![item("New tab", Action::NewTab)];
                if *splits {
                    items.extend([
                        item("Split right", Action::SplitRight),
                        item("Split down", Action::SplitDown),
                    ]);
                }
                items.extend([item("Rename", Action::Rename), item("Close", Action::Close)]);
                items
            }
            ClientContextMenuTarget::Pane {
                source_pane_id,
                has_manual_label,
                right_click_passthrough,
                row,
                ..
            } => {
                let splits = || {
                    [
                        item("Split right", Action::SplitRight),
                        item("Split down", Action::SplitDown),
                    ]
                };
                let mut items = Vec::new();
                // A row's menu starts with what it adds, splits included.
                if row.is_some() {
                    items.push(item("New tab", Action::NewTab));
                    items.extend(splits());
                }
                items.push(item("Rename pane", Action::RenamePane));
                if *has_manual_label {
                    items.push(item("Clear pane name", Action::ClearPaneName));
                }
                if source_pane_id.is_some() {
                    items.push(item("Swap with focused pane", Action::SwapWithFocusedPane));
                }
                if row.is_none() {
                    items.extend(splits());
                }
                items.extend([
                    item("Zoom", Action::Zoom),
                    item(
                        if *right_click_passthrough {
                            "Use Herdr right-click menu"
                        } else {
                            "Send right-clicks to pane"
                        },
                        Action::ToggleRightClickPassthrough,
                    ),
                    item("Close pane", Action::ClosePane),
                ]);
                if let Some(space) = row.and_then(|row| row.space) {
                    items.extend(space_items(space, true));
                }
                items
            }
        }
    }
}

fn item(label: &'static str, action: ClientContextMenuAction) -> ClientContextMenuItem {
    ClientContextMenuItem { label, action }
}

/// A space's items. Beside a pane's items, on a space row's menu, Rename and Close say they
/// are the space's.
fn space_items(space: ClientSpaceMenu, beside_pane: bool) -> Vec<ClientContextMenuItem> {
    use ClientContextMenuAction as Action;

    let (rename, close) = if beside_pane {
        ("Rename space", "Close space")
    } else {
        ("Rename", "Close")
    };
    match space {
        ClientSpaceMenu { is_git: false, .. } => {
            vec![item(rename, Action::Rename), item(close, Action::Close)]
        }
        ClientSpaceMenu {
            is_linked_worktree: false,
            has_worktree_children: false,
            ..
        } => vec![
            item(rename, Action::Rename),
            item(close, Action::Close),
            item("New worktree", Action::NewWorktree),
            item("Open worktree...", Action::OpenWorktree),
        ],
        ClientSpaceMenu {
            is_linked_worktree: true,
            ..
        } => vec![
            item(rename, Action::Rename),
            item(close, Action::Close),
            item("Delete worktree checkout...", Action::RemoveWorktree),
        ],
        ClientSpaceMenu {
            close_group,
            collapsed,
            ..
        } => vec![
            item(rename, Action::Rename),
            item(
                if close_group { "Close group" } else { close },
                Action::Close,
            ),
            item("New worktree", Action::NewWorktree),
            item("Open worktree...", Action::OpenWorktree),
            item(
                if collapsed { "Expand" } else { "Collapse" },
                Action::ToggleGroup,
            ),
        ],
    }
}

impl ClientShellState {
    pub(super) fn open_workspace_context_menu(&mut self, workspace_id: String, x: u16, y: u16) {
        let Some(space) = self.space_menu(&workspace_id) else {
            return;
        };
        self.overlay = Some(ClientShellOverlay::ContextMenu(ClientContextMenuOverlay {
            target: ClientContextMenuTarget::Workspace {
                workspace_id,
                space,
            },
            x,
            y,
            highlighted: 0,
        }));
    }

    /// What the space's menu offers for it, while it exists.
    fn space_menu(&self, workspace_id: &str) -> Option<ClientSpaceMenu> {
        let snapshot = self.snapshot.as_deref()?;
        let workspace = snapshot
            .workspaces
            .iter()
            .find(|workspace| workspace.workspace_id == workspace_id)?;
        let worktree = workspace.worktree.as_ref();
        let has_worktree_children = worktree.is_some_and(|worktree| {
            !worktree.is_linked_worktree
                && snapshot.workspaces.iter().any(|candidate| {
                    candidate.worktree.as_ref().is_some_and(|candidate| {
                        candidate.key == worktree.key && candidate.is_linked_worktree
                    })
                })
        });
        // The tree shows no worktree groups; there Collapse folds the space's own row.
        let collapsed = if self.tree_sidebar_mode() {
            self.collapsed_groups
                .contains(&super::tree_sidebar::tree_fold_key(workspace_id))
        } else {
            worktree.is_some_and(|worktree| {
                self.group_is_collapsed(&self.active_endpoint_id, &worktree.key)
            })
        };
        Some(ClientSpaceMenu {
            is_git: worktree.is_some() || workspace.branch.is_some(),
            is_linked_worktree: worktree.is_some_and(|worktree| worktree.is_linked_worktree),
            has_worktree_children,
            close_group: super::sidebar::workspace_close_is_group(snapshot, workspace),
            collapsed,
        })
    }

    pub(super) fn open_tab_context_menu(&mut self, tab_id: String, x: u16, y: u16) {
        self.open_tab_menu(tab_id, false, x, y);
    }

    /// A sidebar tab row's menu: a lone pane's row menu when the tab holds one pane, else the
    /// tab's with splits of its focused pane.
    pub(super) fn open_tab_row_menu(&mut self, tab_id: String, x: u16, y: u16) {
        let lone_pane = self.snapshot.as_deref().and_then(|snapshot| {
            let mut panes = snapshot.panes.iter().filter(|pane| pane.tab_id == tab_id);
            match (panes.next(), panes.next()) {
                (Some(pane), None) => Some(pane.pane_id.clone()),
                _ => None,
            }
        });
        match lone_pane {
            Some(pane_id) => self.open_row_menu(pane_id, false, x, y),
            None => self.open_tab_menu(tab_id, true, x, y),
        }
    }

    fn open_tab_menu(&mut self, tab_id: String, splits: bool, x: u16, y: u16) {
        let Some(tab) = self
            .snapshot
            .as_deref()
            .and_then(|snapshot| snapshot.tabs.iter().find(|tab| tab.tab_id == tab_id))
        else {
            return;
        };
        self.overlay = Some(ClientShellOverlay::ContextMenu(ClientContextMenuOverlay {
            target: ClientContextMenuTarget::Tab {
                tab_id,
                workspace_id: tab.workspace_id.clone(),
                splits,
            },
            x,
            y,
            highlighted: 0,
        }));
    }

    pub(super) fn open_pane_context_menu(&mut self, pane_id: String, x: u16, y: u16) {
        self.open_pane_menu(pane_id, None, x, y);
    }

    /// The menu of a sidebar row standing for `pane_id`: what it adds, the pane's items, and
    /// the space's too when the row is its space's.
    pub(super) fn open_row_menu(&mut self, pane_id: String, space_row: bool, x: u16, y: u16) {
        let space = self
            .snapshot
            .as_deref()
            .and_then(|snapshot| snapshot.panes.iter().find(|pane| pane.pane_id == pane_id))
            .map(|pane| pane.workspace_id.clone())
            .filter(|_| space_row)
            .and_then(|workspace_id| self.space_menu(&workspace_id));
        self.open_pane_menu(pane_id, Some(ClientRowMenu { space }), x, y);
    }

    fn open_pane_menu(&mut self, pane_id: String, row: Option<ClientRowMenu>, x: u16, y: u16) {
        let Some(snapshot) = self.snapshot.as_deref() else {
            return;
        };
        let Some(pane) = snapshot.panes.iter().find(|pane| pane.pane_id == pane_id) else {
            return;
        };
        let source_pane_id = snapshot
            .focused_pane_id
            .clone()
            .filter(|focused| focused != &pane_id);
        self.overlay = Some(ClientShellOverlay::ContextMenu(ClientContextMenuOverlay {
            target: ClientContextMenuTarget::Pane {
                pane_id,
                workspace_id: pane.workspace_id.clone(),
                source_pane_id,
                has_manual_label: pane.label.is_some(),
                right_click_passthrough: pane.right_click_passthrough,
                row,
            },
            x,
            y,
            highlighted: 0,
        }));
    }

    pub(super) fn move_context_menu_selection(&mut self, delta: isize) {
        let Some(ClientShellOverlay::ContextMenu(menu)) = self.overlay.as_mut() else {
            return;
        };
        let item_count = menu.items().len();
        if item_count == 0 {
            return;
        }
        menu.highlighted = (menu.highlighted as isize + delta)
            .clamp(0, item_count.saturating_sub(1) as isize) as usize;
    }

    pub(super) fn activate_context_menu_item(
        &mut self,
        index: usize,
        outcome: &mut ClientShellInput,
    ) {
        let Some(ClientShellOverlay::ContextMenu(menu)) = self.overlay.take() else {
            return;
        };
        let Some(action) = menu.items().get(index).map(|item| item.action) else {
            outcome.repaint = true;
            return;
        };
        match menu.target {
            ClientContextMenuTarget::Workspace {
                workspace_id,
                space,
            } => self.activate_workspace_context_action(
                workspace_id,
                space.close_group,
                action,
                outcome,
            ),
            ClientContextMenuTarget::Tab {
                tab_id,
                workspace_id,
                ..
            } => self.activate_tab_context_action(tab_id, workspace_id, action, outcome),
            // A row's menu adds a tab to the row's space, and its space items act on the space.
            ClientContextMenuTarget::Pane {
                workspace_id,
                row: Some(_),
                ..
            } if action == ClientContextMenuAction::NewTab => {
                self.new_tab_in(workspace_id, outcome)
            }
            ClientContextMenuTarget::Pane {
                workspace_id,
                row: Some(row),
                ..
            } if matches!(
                action,
                ClientContextMenuAction::Rename
                    | ClientContextMenuAction::Close
                    | ClientContextMenuAction::NewWorktree
                    | ClientContextMenuAction::OpenWorktree
                    | ClientContextMenuAction::RemoveWorktree
                    | ClientContextMenuAction::ToggleGroup
            ) =>
            {
                let close_group = row.space.is_some_and(|space| space.close_group);
                self.activate_workspace_context_action(workspace_id, close_group, action, outcome)
            }
            ClientContextMenuTarget::Pane {
                pane_id,
                workspace_id,
                source_pane_id,
                right_click_passthrough,
                ..
            } => self.activate_pane_context_action(
                pane_id,
                workspace_id,
                source_pane_id,
                right_click_passthrough,
                action,
                outcome,
            ),
        }
        outcome.repaint = true;
    }

    fn activate_workspace_context_action(
        &mut self,
        workspace_id: String,
        close_group: bool,
        action: ClientContextMenuAction,
        outcome: &mut ClientShellInput,
    ) {
        use crate::input::KeybindAction;

        match action {
            ClientContextMenuAction::Rename => {
                let label = self
                    .snapshot
                    .as_deref()
                    .and_then(|snapshot| {
                        snapshot
                            .workspaces
                            .iter()
                            .find(|workspace| workspace.workspace_id == workspace_id)
                    })
                    .map(|workspace| workspace.label.clone());
                if let Some(label) = label {
                    self.overlay = Some(ClientShellOverlay::Rename(ClientRenameOverlay {
                        title: "rename workspace",
                        input: TextEditor::new(&label, false),
                        target: ClientRenameTarget::Workspace { workspace_id },
                    }));
                }
            }
            ClientContextMenuAction::Close => {
                self.request_workspace_close(workspace_id, Some(close_group), outcome);
            }
            ClientContextMenuAction::NewWorktree => {
                self.begin_worktree_action_for(KeybindAction::NewWorktree, workspace_id, outcome)
            }
            ClientContextMenuAction::OpenWorktree => {
                self.begin_worktree_action_for(KeybindAction::OpenWorktree, workspace_id, outcome)
            }
            ClientContextMenuAction::RemoveWorktree => {
                self.begin_worktree_action_for(KeybindAction::RemoveWorktree, workspace_id, outcome)
            }
            ClientContextMenuAction::ToggleGroup if self.tree_sidebar_mode() => {
                let endpoint_id = self.active_endpoint_id.clone();
                self.toggle_collapsed_group(
                    &endpoint_id,
                    super::tree_sidebar::tree_fold_key(&workspace_id),
                );
                self.persist_chrome_preferences(outcome);
            }
            ClientContextMenuAction::ToggleGroup => {
                let key = self.snapshot.as_deref().and_then(|snapshot| {
                    snapshot
                        .workspaces
                        .iter()
                        .find(|workspace| workspace.workspace_id == workspace_id)
                        .and_then(|workspace| workspace.worktree.as_ref())
                        .map(|worktree| worktree.key.clone())
                });
                if let Some(key) = key {
                    let endpoint_id = self.active_endpoint_id.clone();
                    self.toggle_collapsed_group(&endpoint_id, key);
                    self.persist_chrome_preferences(outcome);
                }
            }
            _ => {}
        }
    }

    fn activate_tab_context_action(
        &mut self,
        tab_id: String,
        workspace_id: String,
        action: ClientContextMenuAction,
        outcome: &mut ClientShellInput,
    ) {
        use crate::api::schema::{Method, TabTarget};

        self.push_endpoint_method(
            Method::TabFocus(TabTarget {
                tab_id: tab_id.clone(),
            }),
            outcome,
        );
        match action {
            ClientContextMenuAction::NewTab => self.new_tab_in(workspace_id, outcome),
            // With the tab in front, the space's focused pane is the tab's.
            ClientContextMenuAction::SplitRight | ClientContextMenuAction::SplitDown => {
                self.split_pane(workspace_id, None, action, outcome)
            }
            ClientContextMenuAction::Rename => {
                let tab = self
                    .snapshot
                    .as_deref()
                    .and_then(|snapshot| snapshot.tabs.iter().find(|tab| tab.tab_id == tab_id));
                if let Some(tab) = tab {
                    self.overlay = Some(ClientShellOverlay::Rename(ClientRenameOverlay {
                        title: "rename tab",
                        input: TextEditor::new(&tab.label, false),
                        target: ClientRenameTarget::Tab {
                            tab_id,
                            auto_name: !tab.custom_label,
                            original_name: tab.label.clone(),
                        },
                    }));
                }
            }
            ClientContextMenuAction::Close => {
                self.request_tab_close(tab_id, outcome);
            }
            _ => {}
        }
    }

    /// A new tab in `workspace_id`, named first when the config asks for a name.
    fn new_tab_in(&mut self, workspace_id: String, outcome: &mut ClientShellInput) {
        if self.config.prompt_new_tab_name {
            let default_name = (self
                .snapshot
                .as_deref()
                .map(|snapshot| {
                    snapshot
                        .tabs
                        .iter()
                        .filter(|tab| tab.workspace_id == workspace_id)
                        .count()
                })
                .unwrap_or(0)
                + 1)
            .to_string();
            self.overlay = Some(ClientShellOverlay::Rename(ClientRenameOverlay {
                title: "new tab",
                input: TextEditor::new(&default_name, true),
                target: ClientRenameTarget::NewTab {
                    workspace_id,
                    default_name,
                },
            }));
        } else {
            self.push_endpoint_method(
                crate::api::schema::Method::TabCreate(crate::api::schema::TabCreateParams {
                    workspace_id: Some(workspace_id),
                    cwd: None,
                    focus: true,
                    label: None,
                    env: Default::default(),
                }),
                outcome,
            );
        }
    }

    /// Splits `target`, or the space's focused pane without one, right or down as `action`
    /// says.
    fn split_pane(
        &mut self,
        workspace_id: String,
        target: Option<String>,
        action: ClientContextMenuAction,
        outcome: &mut ClientShellInput,
    ) {
        use crate::api::schema::{Method, PaneSplitParams, SplitDirection};

        self.push_endpoint_method(
            Method::PaneSplit(PaneSplitParams {
                workspace_id: Some(workspace_id),
                target_pane_id: target,
                direction: if action == ClientContextMenuAction::SplitRight {
                    SplitDirection::Right
                } else {
                    SplitDirection::Down
                },
                ratio: None,
                cwd: None,
                focus: true,
                right_click: Default::default(),
                env: Default::default(),
            }),
            outcome,
        );
    }

    fn activate_pane_context_action(
        &mut self,
        pane_id: String,
        workspace_id: String,
        source_pane_id: Option<String>,
        right_click_passthrough: bool,
        action: ClientContextMenuAction,
        outcome: &mut ClientShellInput,
    ) {
        use crate::api::schema::{
            Method, PaneInputSetParams, PaneRenameParams, PaneRightClickTarget, PaneSwapParams,
            PaneTarget, PaneZoomMode, PaneZoomParams,
        };

        match action {
            ClientContextMenuAction::RenamePane => {
                let label = self.snapshot.as_deref().and_then(|snapshot| {
                    snapshot
                        .panes
                        .iter()
                        .find(|pane| pane.pane_id == pane_id)
                        .and_then(|pane| pane.label.clone())
                });
                self.overlay = Some(ClientShellOverlay::Rename(ClientRenameOverlay {
                    title: "rename pane",
                    input: TextEditor::new(label.as_deref().unwrap_or_default(), label.is_none()),
                    target: ClientRenameTarget::Pane { pane_id },
                }));
            }
            ClientContextMenuAction::ClearPaneName => self.push_endpoint_method(
                Method::PaneRename(PaneRenameParams {
                    pane_id,
                    label: None,
                }),
                outcome,
            ),
            ClientContextMenuAction::SwapWithFocusedPane => {
                if let Some(source_pane_id) = source_pane_id {
                    self.push_endpoint_method(
                        Method::PaneSwap(PaneSwapParams {
                            pane_id: None,
                            direction: None,
                            source_pane_id: Some(source_pane_id.clone()),
                            target_pane_id: Some(pane_id),
                        }),
                        outcome,
                    );
                    self.push_endpoint_method(
                        Method::PaneFocus(PaneTarget {
                            pane_id: source_pane_id,
                        }),
                        outcome,
                    );
                }
            }
            ClientContextMenuAction::SplitRight | ClientContextMenuAction::SplitDown => {
                self.split_pane(workspace_id, Some(pane_id), action, outcome)
            }
            ClientContextMenuAction::Zoom => self.push_endpoint_method(
                Method::PaneZoom(PaneZoomParams {
                    pane_id: Some(pane_id),
                    mode: PaneZoomMode::Toggle,
                }),
                outcome,
            ),
            ClientContextMenuAction::ToggleRightClickPassthrough => self.push_endpoint_method(
                Method::PaneInputSet(PaneInputSetParams {
                    pane_id,
                    right_click: if right_click_passthrough {
                        PaneRightClickTarget::Herdr
                    } else {
                        PaneRightClickTarget::Pane
                    },
                }),
                outcome,
            ),
            ClientContextMenuAction::ClosePane => {
                self.push_endpoint_method(Method::PaneClose(PaneTarget { pane_id }), outcome)
            }
            _ => {}
        }
    }
}
