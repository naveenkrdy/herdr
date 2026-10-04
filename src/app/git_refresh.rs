use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use super::{App, GIT_REMOTE_STATUS_REFRESH_INTERVAL, GIT_REPO_DISCOVERY_REFRESH_INTERVAL};
use crate::events::AppEvent;
use crate::layout::PaneId;
use crate::workspace::{
    GitStatusCacheEntry, GitStatusRefreshDemand, PaneGitStatus, WorkspaceGitStatus,
};

#[derive(Clone, Debug, PartialEq, Eq)]
struct WorkspaceGitRefreshItem {
    workspace_id: String,
    resolved_identity_cwd: PathBuf,
    cache_key_hint: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PaneGitRefreshItem {
    pane_id: PaneId,
    cwd: PathBuf,
    cache_key_hint: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct WorkspaceGitRefreshTarget {
    workspace_id: String,
    resolved_identity_cwd: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PaneGitRefreshTarget {
    pane_id: PaneId,
    cwd: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct GitRefreshJob {
    cache_key: PathBuf,
    cached: Option<GitStatusCacheEntry>,
    targets: Vec<WorkspaceGitRefreshTarget>,
    panes: Vec<PaneGitRefreshTarget>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct GitRefreshOutput {
    results: Vec<WorkspaceGitStatus>,
    pane_results: Vec<PaneGitStatus>,
    cache_updates: Vec<(PathBuf, GitStatusCacheEntry)>,
}

impl App {
    pub(crate) fn start_git_status_refresh_if_due(&mut self, now: Instant) {
        let Some(deadline) = self.git_refresh_deadline() else {
            return;
        };

        if now < deadline {
            return;
        }

        let refresh_repo_discovery = self.git_identity_refresh_requested
            || now.saturating_duration_since(self.last_git_repo_discovery_refresh)
                >= GIT_REPO_DISCOVERY_REFRESH_INTERVAL;
        let workspaces = self.workspace_git_refresh_items(refresh_repo_discovery);

        if workspaces.is_empty() {
            self.last_git_remote_status_refresh = now;
            self.git_identity_refresh_requested = false;
            return;
        }

        self.git_refresh_in_flight = true;
        let event_tx = self.event_tx.clone();
        let cache = self.git_status_cache.clone();
        let mut demand = self.git_refresh_demand();
        if self.git_identity_refresh_requested {
            demand.branch = true;
        }
        let panes = if demand.branch {
            self.pane_git_refresh_items(refresh_repo_discovery)
        } else {
            Vec::new()
        };
        self.git_identity_refresh_requested = false;
        if refresh_repo_discovery {
            self.last_git_repo_discovery_refresh = now;
        }
        std::thread::spawn(move || {
            let output =
                refresh_git_statuses_with_cache_and_demand(workspaces, panes, &cache, demand);
            let _ = event_tx.blocking_send(AppEvent::GitStatusRefreshed {
                results: output.results,
                pane_results: output.pane_results,
                cache_updates: output.cache_updates,
            });
        });
    }

    pub(crate) fn request_git_identity_refresh(&mut self, now: Instant) {
        self.git_identity_refresh_requested = true;
        self.mark_git_status_refresh_due(now);
    }

    pub(crate) fn mark_git_status_refresh_due(&mut self, now: Instant) {
        self.git_status_cache
            .retain(|_, entry| entry.fingerprint.is_some());
        if self.git_refresh_in_flight {
            self.git_refresh_due_after_in_flight = true;
            return;
        }
        self.last_git_remote_status_refresh = now
            .checked_sub(GIT_REMOTE_STATUS_REFRESH_INTERVAL)
            .unwrap_or(now);
        self.git_refresh_due_after_in_flight = false;
    }

    pub(crate) fn git_refresh_deadline(&self) -> Option<Instant> {
        (!self.git_refresh_in_flight
            && !self.state.workspaces.is_empty()
            && (self.git_identity_refresh_requested || !self.git_refresh_demand().is_empty()))
        .then_some(self.last_git_remote_status_refresh + GIT_REMOTE_STATUS_REFRESH_INTERVAL)
    }

    fn git_refresh_demand(&self) -> GitStatusRefreshDemand {
        let mut demand = GitStatusRefreshDemand::default();
        for token in self.state.sidebar_spaces.rows.iter().flatten() {
            match token.parts().0 {
                crate::config::SpaceSidebarToken::Branch => demand.branch = true,
                crate::config::SpaceSidebarToken::GitStatus => demand.ahead_behind = true,
                _ => {}
            }
        }
        demand
    }

    fn workspace_git_refresh_items(
        &self,
        refresh_repo_discovery: bool,
    ) -> Vec<WorkspaceGitRefreshItem> {
        self.state
            .workspaces
            .iter()
            .filter_map(|ws| {
                let cwd =
                    ws.resolved_identity_cwd_from(&self.state.terminals, &self.terminal_runtimes)?;
                let cache_key_hint = reusable_cache_key(
                    refresh_repo_discovery,
                    &cwd,
                    Some((&ws.cached_identity_cwd, &ws.cached_git_status_key)),
                );
                Some(WorkspaceGitRefreshItem {
                    workspace_id: ws.id.clone(),
                    resolved_identity_cwd: cwd,
                    cache_key_hint,
                })
            })
            .collect()
    }

    fn pane_git_refresh_items(&self, refresh_repo_discovery: bool) -> Vec<PaneGitRefreshItem> {
        let terminals = &self.state.terminals;
        self.state
            .workspaces
            .iter()
            .flat_map(|ws| &ws.tabs)
            .flat_map(|tab| {
                tab.panes.keys().filter_map(move |&pane_id| {
                    let cwd = tab.cwd_for_pane(pane_id, terminals, &self.terminal_runtimes)?;
                    let terminal = tab
                        .terminal_id(pane_id)
                        .and_then(|terminal_id| terminals.get(terminal_id));
                    let cache_key_hint = reusable_cache_key(
                        refresh_repo_discovery,
                        &cwd,
                        terminal.and_then(|terminal| {
                            Some((
                                terminal.git_cwd.as_deref()?,
                                terminal.git_status_key.as_deref()?,
                            ))
                        }),
                    );
                    Some(PaneGitRefreshItem {
                        pane_id,
                        cwd,
                        cache_key_hint,
                    })
                })
            })
            .collect()
    }
}

/// Reuses the last `(folder, cache key)` lookup only while the folder is unchanged and no
/// repo rediscovery is due.
fn reusable_cache_key(
    refresh_repo_discovery: bool,
    cwd: &Path,
    last_lookup: Option<(&Path, &Path)>,
) -> Option<PathBuf> {
    last_lookup
        .filter(|(last_cwd, _)| !refresh_repo_discovery && *last_cwd == cwd)
        .map(|(_, key)| key.to_path_buf())
}

fn deduplicate_git_refresh_items(
    items: Vec<WorkspaceGitRefreshItem>,
    panes: Vec<PaneGitRefreshItem>,
    cache: &HashMap<PathBuf, GitStatusCacheEntry>,
) -> Vec<GitRefreshJob> {
    let mut indexes = HashMap::<PathBuf, usize>::new();
    let mut jobs = Vec::<GitRefreshJob>::new();

    for item in items {
        let index = git_refresh_job_index(
            &mut jobs,
            &mut indexes,
            cache,
            &item.resolved_identity_cwd,
            item.cache_key_hint,
        );
        jobs[index].targets.push(WorkspaceGitRefreshTarget {
            workspace_id: item.workspace_id,
            resolved_identity_cwd: item.resolved_identity_cwd,
        });
    }
    for pane in panes {
        let index = git_refresh_job_index(
            &mut jobs,
            &mut indexes,
            cache,
            &pane.cwd,
            pane.cache_key_hint,
        );
        jobs[index].panes.push(PaneGitRefreshTarget {
            pane_id: pane.pane_id,
            cwd: pane.cwd,
        });
    }

    jobs
}

fn git_refresh_job_index(
    jobs: &mut Vec<GitRefreshJob>,
    indexes: &mut HashMap<PathBuf, usize>,
    cache: &HashMap<PathBuf, GitStatusCacheEntry>,
    cwd: &Path,
    cache_key_hint: Option<PathBuf>,
) -> usize {
    let reconcile = cache_key_hint.is_none();
    let cache_key = cache_key_hint.unwrap_or_else(|| {
        crate::workspace::git_status_cache_key(cwd).unwrap_or_else(|| cwd.to_path_buf())
    });
    if let Some(&index) = indexes.get(&cache_key) {
        jobs[index].cached = jobs[index].cached.take().filter(|_| !reconcile);
        return index;
    }

    let cached = cache.get(&cache_key).filter(|_| !reconcile).cloned();
    indexes.insert(cache_key.clone(), jobs.len());
    jobs.push(GitRefreshJob {
        cache_key,
        cached,
        targets: Vec::new(),
        panes: Vec::new(),
    });
    jobs.len() - 1
}

fn refresh_git_statuses_with_cache_and_demand(
    items: Vec<WorkspaceGitRefreshItem>,
    panes: Vec<PaneGitRefreshItem>,
    cache: &HashMap<PathBuf, GitStatusCacheEntry>,
    demand: GitStatusRefreshDemand,
) -> GitRefreshOutput {
    let mut results = Vec::new();
    let mut pane_results = Vec::new();
    let mut cache_updates = Vec::new();

    for job in deduplicate_git_refresh_items(items, panes, cache) {
        // Panes only need the branch; skip ahead/behind work for repos no space uses.
        let job_demand = if job.targets.is_empty() {
            GitStatusRefreshDemand {
                branch: true,
                ahead_behind: false,
            }
        } else {
            demand
        };
        let (snapshot, cache_entry) = crate::workspace::git_status_snapshot_for_cwd_with_demand(
            &job.cache_key,
            job.cached.as_ref(),
            job_demand,
        );
        // A branch-only fingerprint has no upstream: a space that later shares a tracked repo
        // sees a changed fingerprint and computes ahead/behind instead of reusing this entry.
        if let Some(cache_entry) = cache_entry {
            cache_updates.push((job.cache_key.clone(), cache_entry));
        }
        pane_results.extend(job.panes.into_iter().map(|pane| PaneGitStatus {
            pane_id: pane.pane_id,
            cwd: pane.cwd,
            status_cache_key: job.cache_key.clone(),
            branch: snapshot.branch.clone(),
        }));
        results.extend(job.targets.into_iter().map(move |target| {
            snapshot.clone().into_workspace_status(
                target.workspace_id,
                target.resolved_identity_cwd,
                job.cache_key.clone(),
                demand,
            )
        }));
    }

    GitRefreshOutput {
        results,
        pane_results,
        cache_updates,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::Workspace;

    #[test]
    fn git_refresh_deduplicates_workspaces_with_same_cache_key() {
        let repo =
            std::env::temp_dir().join(format!("herdr-git-refresh-dedupe-{}", std::process::id()));
        let nested = repo.join("nested");
        let other = repo.join("other");
        std::fs::create_dir_all(&nested).expect("create nested dir");
        std::fs::create_dir_all(&other).expect("create other dir");
        std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .arg("init")
            .output()
            .expect("run git init");

        let output = refresh_git_statuses_with_cache_and_demand(
            vec![
                WorkspaceGitRefreshItem {
                    workspace_id: "one".into(),
                    resolved_identity_cwd: nested.clone(),
                    cache_key_hint: None,
                },
                WorkspaceGitRefreshItem {
                    workspace_id: "two".into(),
                    resolved_identity_cwd: other.clone(),
                    cache_key_hint: None,
                },
            ],
            Vec::new(),
            &HashMap::new(),
            GitStatusRefreshDemand::ALL,
        );

        assert_eq!(output.cache_updates.len(), 1);
        assert_eq!(
            output.cache_updates[0].0,
            std::fs::canonicalize(&repo).expect("canonical repo path")
        );
        assert_eq!(output.results.len(), 2);
        assert_eq!(output.results[0].workspace_id, "one");
        assert_eq!(output.results[0].resolved_identity_cwd, nested);
        assert_eq!(output.results[1].workspace_id, "two");
        assert_eq!(output.results[1].resolved_identity_cwd, other);

        let _ = std::fs::remove_dir_all(repo);
    }

    #[test]
    fn shared_root_repo_refresh_keeps_workspace_specific_fallback_labels() {
        let cache_key = PathBuf::from("/");
        let cached = GitStatusCacheEntry {
            fingerprint: None,
            retry_after: Some(Instant::now() + std::time::Duration::from_secs(30)),
            snapshot: crate::workspace::WorkspaceGitStatusSnapshot {
                auto_label: "/".into(),
                branch: Some("main".into()),
                ahead_behind: None,
                space: Some(crate::workspace::GitSpaceMetadata {
                    key: "/.git".into(),
                    checkout_key: "/".into(),
                    repo_name: "repo".into(),
                    repo_root: cache_key.clone(),
                    is_linked_worktree: false,
                }),
            },
        };
        let items = ["alpha", "beta"]
            .into_iter()
            .map(|name| WorkspaceGitRefreshItem {
                workspace_id: name.into(),
                resolved_identity_cwd: cache_key.join(name),
                cache_key_hint: Some(cache_key.clone()),
            })
            .collect();

        let output = refresh_git_statuses_with_cache_and_demand(
            items,
            Vec::new(),
            &HashMap::from([(cache_key, cached)]),
            GitStatusRefreshDemand::ALL,
        );

        assert_eq!(output.cache_updates.len(), 1);
        assert_eq!(output.results.len(), 2);
        assert_eq!(output.results[0].auto_label, "alpha");
        assert_eq!(output.results[1].auto_label, "beta");
        assert_eq!(output.results[0].branch.as_deref(), Some("main"));
        assert_eq!(output.results[1].branch.as_deref(), Some("main"));
    }

    #[test]
    fn git_refresh_item_collection_does_not_discover_uncached_cwd() {
        let mut app = test_app(&crate::config::Config::default());
        let cwd = std::env::temp_dir().join(format!("herdr-uncached-cwd-{}", std::process::id()));
        let mut ws = Workspace::test_new("test");
        ws.identity_cwd = cwd.clone();
        ws.tabs.clear();
        app.state.workspaces.push(ws);

        let items = app.workspace_git_refresh_items(false);

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].resolved_identity_cwd, cwd);
        assert_eq!(items[0].cache_key_hint, None);
    }

    #[test]
    fn git_refresh_item_collection_reuses_matching_cached_key() {
        let mut app = test_app(&crate::config::Config::default());
        let cwd = PathBuf::from("/repo/deep/nested");
        let cache_key = PathBuf::from("/repo");
        let mut ws = Workspace::test_new("test");
        ws.identity_cwd = cwd.clone();
        ws.cached_identity_cwd = cwd;
        ws.cached_git_status_key = cache_key.clone();
        ws.tabs.clear();
        app.state.workspaces.push(ws);

        let items = app.workspace_git_refresh_items(false);

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].cache_key_hint, Some(cache_key));
    }

    #[test]
    fn periodic_repo_discovery_ignores_cached_key_hints() {
        let mut app = test_app(&crate::config::Config::default());
        let cwd = PathBuf::from("/repo/deep/nested");
        let mut ws = Workspace::test_new("test");
        ws.identity_cwd = cwd.clone();
        ws.cached_identity_cwd = cwd;
        ws.cached_git_status_key = PathBuf::from("/repo");
        ws.tabs.clear();
        app.state.workspaces.push(ws);

        let items = app.workspace_git_refresh_items(true);

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].cache_key_hint, None);
        let cache_key = items[0].resolved_identity_cwd.clone();
        let cached = GitStatusCacheEntry {
            fingerprint: None,
            retry_after: None,
            snapshot: crate::workspace::WorkspaceGitStatusSnapshot {
                auto_label: "stale".into(),
                branch: None,
                ahead_behind: None,
                space: None,
            },
        };
        let jobs =
            deduplicate_git_refresh_items(items, Vec::new(), &HashMap::from([(cache_key, cached)]));
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].cached, None);
    }

    #[test]
    fn cwd_identity_refresh_runs_once_without_sidebar_git_tokens() {
        let mut config = crate::config::Config::default();
        config.ui.sidebar.spaces.rows = vec![vec![crate::config::SpaceSidebarToken::Workspace]];
        let mut app = test_app(&config);
        app.state.workspaces.push(Workspace::test_new("test"));
        let now = Instant::now();

        app.request_git_identity_refresh(now);

        assert!(app.git_refresh_deadline().is_some());
        app.start_git_status_refresh_if_due(now);
        assert!(app.git_refresh_in_flight);
        assert!(!app.git_identity_refresh_requested);
    }

    #[test]
    fn due_git_refresh_does_not_start_without_sidebar_consumer() {
        let mut config = crate::config::Config::default();
        config.ui.sidebar.spaces.rows = vec![vec![crate::config::SpaceSidebarToken::Workspace]];
        let mut app = test_app(&config);
        app.state.workspaces.push(Workspace::test_new("test"));
        let now = Instant::now();
        app.last_git_remote_status_refresh = now - GIT_REMOTE_STATUS_REFRESH_INTERVAL;

        app.start_git_status_refresh_if_due(now);

        assert!(!app.git_refresh_in_flight);
        assert!(app.event_rx.try_recv().is_err());
    }

    #[test]
    fn git_refresh_demand_matches_sidebar_rows() {
        let cases = [
            (
                crate::config::SpaceSidebarToken::Workspace,
                GitStatusRefreshDemand::default(),
            ),
            (
                crate::config::SpaceSidebarToken::Branch,
                GitStatusRefreshDemand {
                    branch: true,
                    ahead_behind: false,
                },
            ),
            (
                crate::config::SpaceSidebarToken::GitStatus,
                GitStatusRefreshDemand {
                    branch: false,
                    ahead_behind: true,
                },
            ),
        ];

        for (token, expected) in cases {
            let mut config = crate::config::Config::default();
            config.ui.sidebar.spaces.rows = vec![vec![token.clone()]];
            let mut app = test_app(&config);
            app.state.workspaces.push(Workspace::test_new("test"));

            assert_eq!(app.git_refresh_demand(), expected, "token: {token:?}");
            assert_eq!(
                app.git_refresh_deadline().is_some(),
                !expected.is_empty(),
                "token: {token:?}"
            );
        }
    }

    #[test]
    fn unnamed_linked_worktree_does_not_force_periodic_branch_refresh() {
        let mut config = crate::config::Config::default();
        config.ui.sidebar.spaces.rows = vec![vec![crate::config::SpaceSidebarToken::Workspace]];
        let mut app = test_app(&config);
        let mut child = Workspace::test_new("test");
        child.custom_name = None;
        child.worktree_space = Some(crate::workspace::WorktreeSpaceMembership {
            key: "repo".into(),
            label: "repo".into(),
            repo_root: "/repo".into(),
            checkout_path: "/repo-worktree".into(),
            is_linked_worktree: true,
        });
        app.state.workspaces.push(child);

        assert_eq!(app.git_refresh_deadline(), None);
    }

    #[test]
    fn custom_named_linked_worktree_does_not_require_branch_refresh() {
        let mut config = crate::config::Config::default();
        config.ui.sidebar.spaces.rows = vec![vec![crate::config::SpaceSidebarToken::Workspace]];
        let mut app = test_app(&config);
        let mut child = Workspace::test_new("custom");
        child.worktree_space = Some(crate::workspace::WorktreeSpaceMembership {
            key: "repo".into(),
            label: "repo".into(),
            repo_root: "/repo".into(),
            checkout_path: "/repo-worktree".into(),
            is_linked_worktree: true,
        });
        app.state.workspaces.push(child);

        assert_eq!(app.git_refresh_deadline(), None);
    }

    #[test]
    fn headless_deadline_can_suppress_git_refresh_timer() {
        let mut app = test_app(&crate::config::Config::default());
        app.state.workspaces.push(Workspace::test_new("test"));
        let now = Instant::now();
        app.last_git_remote_status_refresh = now - GIT_REMOTE_STATUS_REFRESH_INTERVAL;

        assert_eq!(
            app.next_headless_loop_deadline_with_git_refresh(now, false, false),
            None
        );
        assert_eq!(
            app.next_headless_loop_deadline_with_git_refresh(now, false, true),
            Some(now)
        );
    }

    #[test]
    fn explicit_git_refresh_invalidates_cached_non_git_results() {
        let mut app = test_app(&crate::config::Config::default());
        let cwd = std::env::temp_dir().join(format!("herdr-git-miss-{}", std::process::id()));
        std::fs::create_dir_all(&cwd).unwrap();
        let (_, entry) = crate::workspace::git_status_snapshot_for_cwd_with_demand(
            &cwd,
            None,
            GitStatusRefreshDemand::ALL,
        );
        app.git_status_cache
            .insert(cwd.clone(), entry.expect("non-Git cache entry"));

        app.mark_git_status_refresh_due(Instant::now());

        assert!(app.git_status_cache.is_empty());
        std::fs::remove_dir_all(cwd).unwrap();
    }

    #[test]
    fn git_refresh_due_request_survives_in_flight_refresh() {
        let mut app = test_app(&crate::config::Config::default());
        let now = Instant::now();
        app.git_refresh_in_flight = true;

        app.mark_git_status_refresh_due(now);
        assert!(app.git_refresh_due_after_in_flight);

        app.handle_internal_event(AppEvent::GitStatusRefreshed {
            results: Vec::new(),
            pane_results: Vec::new(),
            cache_updates: Vec::new(),
        });

        assert!(!app.git_refresh_in_flight);
        assert!(!app.git_refresh_due_after_in_flight);
        assert_eq!(app.git_refresh_deadline(), None);

        app.state.workspaces.push(Workspace::test_new("test"));
        let deadline = app
            .git_refresh_deadline()
            .expect("refresh should be due once a workspace exists");
        assert!(deadline <= Instant::now());
    }

    fn init_repo(path: &std::path::Path, branch: &str) {
        std::fs::create_dir_all(path).expect("create repo dir");
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(path)
            .args(["init", "-q", "-b", branch])
            .output()
            .expect("run git init");
        assert!(output.status.success(), "git init failed: {output:?}");
    }

    #[test]
    fn pane_git_refresh_resolves_each_pane_with_one_lookup_per_folder_root() {
        let root = std::env::temp_dir().join(format!("herdr-pane-branch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let space_repo = root.join("space");
        let tab_repo = root.join("tab");
        let plain = root.join("plain");
        init_repo(&space_repo, "feat/space");
        init_repo(&tab_repo, "feat/tab");
        std::fs::create_dir_all(&plain).expect("create plain dir");
        let mut panes = (0..15u32)
            .map(|index| {
                let cwd = space_repo.join(format!("dir{index}"));
                std::fs::create_dir_all(&cwd).expect("create pane dir");
                PaneGitRefreshItem {
                    pane_id: PaneId::from_raw(index + 1),
                    cwd,
                    cache_key_hint: None,
                }
            })
            .collect::<Vec<_>>();
        panes.push(PaneGitRefreshItem {
            pane_id: PaneId::from_raw(100),
            cwd: tab_repo.clone(),
            cache_key_hint: None,
        });
        panes.push(PaneGitRefreshItem {
            pane_id: PaneId::from_raw(101),
            cwd: plain.clone(),
            cache_key_hint: None,
        });
        let workspaces = vec![WorkspaceGitRefreshItem {
            workspace_id: "space".into(),
            resolved_identity_cwd: space_repo.clone(),
            cache_key_hint: None,
        }];

        let jobs =
            deduplicate_git_refresh_items(workspaces.clone(), panes.clone(), &HashMap::new());
        let output = refresh_git_statuses_with_cache_and_demand(
            workspaces,
            panes,
            &HashMap::new(),
            GitStatusRefreshDemand {
                branch: true,
                ahead_behind: false,
            },
        );

        assert_eq!(jobs.len(), 3, "one lookup per repo or plain folder");
        assert_eq!(output.results.len(), 1);
        assert_eq!(output.results[0].branch.as_deref(), Some("feat/space"));
        assert_eq!(output.pane_results.len(), 17);
        let branch_of = |raw: u32| {
            output
                .pane_results
                .iter()
                .find(|result| result.pane_id == PaneId::from_raw(raw))
                .map(|result| result.branch.clone())
        };
        for raw in 1..=15 {
            assert_eq!(
                branch_of(raw),
                Some(Some("feat/space".into())),
                "pane {raw}"
            );
        }
        assert_eq!(branch_of(100), Some(Some("feat/tab".into())));
        assert_eq!(branch_of(101), Some(None));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn pane_only_repo_reuses_its_cache_entry_on_the_next_refresh() {
        let root = std::env::temp_dir().join(format!("herdr-pane-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        init_repo(&root, "feat/tab");
        let key = std::fs::canonicalize(&root).expect("canonical repo");
        let pane = |cache_key_hint| PaneGitRefreshItem {
            pane_id: PaneId::from_raw(1),
            cwd: root.clone(),
            cache_key_hint,
        };
        let branch_only = GitStatusRefreshDemand {
            branch: true,
            ahead_behind: false,
        };

        let first = refresh_git_statuses_with_cache_and_demand(
            Vec::new(),
            vec![pane(None)],
            &HashMap::new(),
            branch_only,
        );
        let cache: HashMap<_, _> = first.cache_updates.into_iter().collect();
        assert!(cache.contains_key(&key), "first refresh caches the repo");
        let hint = first.pane_results[0].status_cache_key.clone();
        let jobs = deduplicate_git_refresh_items(Vec::new(), vec![pane(Some(hint))], &cache);

        assert_eq!(jobs.len(), 1);
        assert!(
            jobs[0].cached.is_some(),
            "second refresh starts from the cached repo context"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn pane_only_repo_skips_ahead_behind_even_when_spaces_demand_it() {
        let root = std::env::temp_dir().join(format!("herdr-pane-upstream-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        init_repo(&root, "feat/tab");
        for args in [
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "init",
            ][..],
            &["update-ref", "refs/remotes/origin/feat/tab", "HEAD"],
            &["config", "branch.feat/tab.remote", "origin"],
            &["config", "branch.feat/tab.merge", "refs/heads/feat/tab"],
        ] {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .output()
                .expect("run git");
            assert!(output.status.success(), "git {args:?} failed: {output:?}");
        }
        let pane = || PaneGitRefreshItem {
            pane_id: PaneId::from_raw(1),
            cwd: root.clone(),
            cache_key_hint: None,
        };
        let space = WorkspaceGitRefreshItem {
            workspace_id: "space".into(),
            resolved_identity_cwd: root.clone(),
            cache_key_hint: None,
        };

        let shared = refresh_git_statuses_with_cache_and_demand(
            vec![space],
            vec![pane()],
            &HashMap::new(),
            GitStatusRefreshDemand::ALL,
        );
        let pane_only = refresh_git_statuses_with_cache_and_demand(
            Vec::new(),
            vec![pane()],
            &HashMap::new(),
            GitStatusRefreshDemand::ALL,
        );

        assert_eq!(shared.results[0].ahead_behind, Some((0, 0)));
        assert_eq!(
            pane_only.pane_results[0].branch.as_deref(),
            Some("feat/tab")
        );
        let (_, entry) = &pane_only.cache_updates[0];
        assert_eq!(entry.snapshot.ahead_behind, None);
        assert_eq!(
            entry
                .fingerprint
                .as_ref()
                .map(|fingerprint| &fingerprint.upstream),
            Some(&None),
            "a pane-only lookup must not read the upstream"
        );
        let cache: HashMap<_, _> = pane_only.cache_updates.into_iter().collect();
        let key = pane_only.pane_results[0].status_cache_key.clone();
        let later_space = refresh_git_statuses_with_cache_and_demand(
            vec![WorkspaceGitRefreshItem {
                workspace_id: "space".into(),
                resolved_identity_cwd: root.clone(),
                cache_key_hint: Some(key),
            }],
            Vec::new(),
            &cache,
            GitStatusRefreshDemand::ALL,
        );
        assert_eq!(
            later_space.results[0].ahead_behind,
            Some((0, 0)),
            "a space sharing the repo later must not inherit the pane's missing ahead/behind"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn pane_git_refresh_items_reuse_cached_key_only_for_unchanged_cwd() {
        let mut app = test_app(&crate::config::Config::default());
        let ws = Workspace::test_new("test");
        let pane_id = ws.tabs[0].root_pane;
        let cwd = ws.identity_cwd.clone();
        app.state.workspaces.push(ws);
        app.state.ensure_test_terminals();
        let terminal_id = app.state.workspaces[0].tabs[0]
            .terminal_id(pane_id)
            .expect("root terminal")
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).expect("terminal");
        terminal.git_cwd = Some(cwd.clone());
        terminal.git_status_key = Some(PathBuf::from("/repo"));

        let items = app.pane_git_refresh_items(false);
        assert_eq!(
            items,
            vec![PaneGitRefreshItem {
                pane_id,
                cwd: cwd.clone(),
                cache_key_hint: Some(PathBuf::from("/repo")),
            }]
        );
        assert_eq!(app.pane_git_refresh_items(true)[0].cache_key_hint, None);

        app.state
            .terminals
            .get_mut(&terminal_id)
            .expect("terminal")
            .git_cwd = Some(PathBuf::from("/elsewhere"));
        assert_eq!(app.pane_git_refresh_items(false)[0].cache_key_hint, None);
    }

    #[test]
    fn pane_branches_refresh_only_when_branch_is_demanded() {
        for (token, expect_panes) in [
            (crate::config::SpaceSidebarToken::GitStatus, false),
            (crate::config::SpaceSidebarToken::Branch, true),
        ] {
            let mut config = crate::config::Config::default();
            config.ui.sidebar.spaces.rows = vec![vec![token.clone()]];
            let mut app = test_app(&config);
            app.state.workspaces.push(Workspace::test_new("test"));
            app.state.ensure_test_terminals();
            let now = Instant::now();
            app.last_git_remote_status_refresh = now - GIT_REMOTE_STATUS_REFRESH_INTERVAL;

            app.start_git_status_refresh_if_due(now);

            let Some(AppEvent::GitStatusRefreshed { pane_results, .. }) =
                app.event_rx.blocking_recv()
            else {
                panic!("expected git status refresh for {token:?}");
            };
            assert_eq!(!pane_results.is_empty(), expect_panes, "token: {token:?}");
        }
    }

    fn test_app(config: &crate::config::Config) -> super::super::App {
        super::super::App::new(
            config,
            crate::app::AppPolicy::TEST,
            None,
            tokio::sync::mpsc::unbounded_channel().1,
            crate::api::EventHub::default(),
        )
    }
}
