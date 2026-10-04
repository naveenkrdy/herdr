use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use tokio::io::{AsyncRead, AsyncReadExt};

use crate::api::schema::{Method, Request, ResponseResult};
use crate::app::App;
use crate::events::AppEvent;

use super::responses::{encode_error, encode_success};

mod act;
mod open;
pub(crate) use act::TasksActFinished;

/// How long `<tasks.command> list --json` may run before it and its children are killed.
pub(crate) const TASKS_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_TASKS_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const MAX_TASKS_STDERR_TAIL_BYTES: usize = 4 * 1024;

/// A kept list older than this is refreshed, which picks up files the list does not name yet.
pub(crate) const TASKS_LIST_MAX_AGE: Duration = Duration::from_secs(30);

/// Within this long of the last check, a request reuses the kept list without a new one.
pub(crate) const TASKS_LIST_CHECK_INTERVAL: Duration = Duration::from_secs(1);

/// Paths watched at most; changes past them wait for the `TASKS_LIST_MAX_AGE` rerun.
const MAX_WATCHED_PATHS: usize = 2000;

/// The `tasks.list` requests waiting on a run, whether one is in flight, and the last good
/// list.
#[derive(Debug, Default)]
pub(crate) struct TasksListState {
    kept: Option<Arc<KeptTasksList>>,
    waiting: Vec<TasksListWaiter>,
    in_flight: bool,
    /// When the kept list was last confirmed: made by a run, or its files found unchanged.
    checked_at: Option<Instant>,
    /// Counts finished `tasks.act` runs: a list read while one finished may predate it.
    actions_finished: u64,
}

/// A request waiting for a run of the command that was configured when it was asked, started
/// after the last action that had finished by then.
#[derive(Debug)]
struct TasksListWaiter {
    program: PathBuf,
    /// `TasksListState::actions_finished` when the request was asked: a run started at this
    /// count or later answers it.
    actions_finished: u64,
    request_id: String,
    respond_to: std::sync::mpsc::Sender<String>,
}

impl TasksListState {
    /// An action changed (or may have changed) the notes files: the next list runs again.
    pub(super) fn action_finished(&mut self) {
        self.kept = None;
        self.checked_at = None;
        self.actions_finished = self.actions_finished.wrapping_add(1);
    }

    /// The last good list, whatever its age; `None` after an action or a failed run.
    fn kept_list(&self) -> Option<&serde_json::Value> {
        self.kept.as_ref().map(|kept| &kept.list)
    }

    #[cfg(test)]
    pub(crate) fn keep_list_for_test(&mut self, list: serde_json::Value) {
        self.kept = Some(Arc::new(KeptTasksList {
            program: PathBuf::from("tasks-command"),
            list,
            watched: Some(Vec::new()),
            started: SystemTime::now(),
            ran_at: Instant::now(),
        }));
    }

    #[cfg(test)]
    pub(crate) fn age_last_check_for_test(&mut self, age: Duration) {
        self.checked_at = self
            .checked_at
            .map(|checked_at| checked_at.checked_sub(age).unwrap_or(checked_at));
    }

    #[cfg(test)]
    pub(crate) fn age_last_run_for_test(&mut self, age: Duration) {
        if let Some(kept) = self.kept.as_mut() {
            let kept = Arc::make_mut(kept);
            kept.ran_at = kept.ran_at.checked_sub(age).unwrap_or(kept.ran_at);
        }
    }
}

/// A list kept with the modification times of the files and folders it names, so a later
/// request can reuse it after a stat instead of running the command again.
#[derive(Debug, Clone)]
struct KeptTasksList {
    program: PathBuf,
    list: serde_json::Value,
    /// `None` when the file check failed: the list is then never reused.
    watched: Option<Vec<(PathBuf, Option<SystemTime>)>>,
    started: SystemTime,
    ran_at: Instant,
}

impl KeptTasksList {
    fn is_current(&self, program: &Path) -> bool {
        self.watched.is_some()
            && self.program == program
            && self.ran_at.elapsed() < TASKS_LIST_MAX_AGE
    }

    /// True when a watched path changed, or was written while the command was reading it.
    fn watched_changed(&self) -> bool {
        let Some(watched) = &self.watched else {
            return true;
        };
        watched.iter().any(|(path, modified)| {
            *modified != modified_time(path) || modified.is_some_and(|time| time >= self.started)
        })
    }
}

#[derive(Debug)]
pub(crate) struct TasksListFinished {
    program: PathBuf,
    /// `TasksListState::actions_finished` when the run started.
    actions_finished: u64,
    outcome: TasksListOutcome,
}

#[derive(Debug)]
enum TasksListOutcome {
    Unchanged(Arc<KeptTasksList>),
    Ran(Result<KeptTasksList, TasksError>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TasksError {
    code: &'static str,
    message: String,
}

impl TasksError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl App {
    /// Answers `tasks.list` off the server loop. The kept list is reused while nothing it names
    /// changed; otherwise the command runs, at most once at a time, and every request for the
    /// same command that arrived meanwhile gets that run's reply.
    pub(crate) fn handle_deferred_tasks_api_request(
        &mut self,
        request: Request,
        respond_to: std::sync::mpsc::Sender<String>,
    ) -> bool {
        match request.method {
            Method::TasksList(_) => {}
            Method::TasksAct(params) => {
                self.start_tasks_act(request.id, params, respond_to);
                return true;
            }
            _ => return false,
        }
        let Some(program) = self.tasks_command.clone() else {
            let _ = respond_to.send(encode_error(
                request.id,
                "tasks_unavailable",
                "tasks.command is not set",
            ));
            return true;
        };
        let recently_checked = self
            .tasks_list
            .checked_at
            .is_some_and(|checked_at| checked_at.elapsed() < TASKS_LIST_CHECK_INTERVAL);
        if let Some(kept) = self
            .tasks_list
            .kept
            .as_ref()
            .filter(|kept| recently_checked && kept.is_current(&program))
        {
            let _ = respond_to.send(encode_success(
                request.id,
                ResponseResult::TasksList {
                    list: kept.list.clone(),
                },
            ));
            return true;
        }
        self.tasks_list.waiting.push(TasksListWaiter {
            program: program.clone(),
            actions_finished: self.tasks_list.actions_finished,
            request_id: request.id,
            respond_to,
        });
        if !self.tasks_list.in_flight {
            self.start_tasks_list_run(program);
        }
        true
    }

    fn start_tasks_list_run(&mut self, program: PathBuf) {
        self.tasks_list.in_flight = true;
        let actions_finished = self.tasks_list.actions_finished;
        let kept = self
            .tasks_list
            .kept
            .clone()
            .filter(|kept| kept.is_current(&program));
        let event_tx = self.event_tx.clone();
        tokio::spawn(async move {
            let outcome = match kept {
                Some(kept) if !watched_changed(Arc::clone(&kept)).await => {
                    TasksListOutcome::Unchanged(kept)
                }
                _ => TasksListOutcome::Ran(run_and_keep(program.clone()).await),
            };
            let _ = event_tx
                .send(AppEvent::TasksListFinished(Box::new(TasksListFinished {
                    program,
                    actions_finished,
                    outcome,
                })))
                .await;
        });
    }

    /// Replies to the requests for the command that ran that were asked before any action
    /// finished during the run; the others (asked after a change of `tasks.command` or after
    /// an action) get a new run of the command configured now.
    pub(crate) fn handle_api_tasks_list_finished(&mut self, finished: TasksListFinished) {
        self.tasks_list.in_flight = false;
        let result = match finished.outcome {
            TasksListOutcome::Unchanged(kept) => {
                self.tasks_list.checked_at = Some(Instant::now());
                Ok(kept)
            }
            TasksListOutcome::Ran(Ok(kept)) => {
                let kept = Arc::new(kept);
                let keep = self.tasks_command.as_deref() == Some(&kept.program)
                    && self.tasks_list.actions_finished == finished.actions_finished;
                self.tasks_list.kept = keep.then(|| Arc::clone(&kept));
                self.tasks_list.checked_at = Some(Instant::now());
                Ok(kept)
            }
            TasksListOutcome::Ran(Err(error)) => {
                self.tasks_list.kept = None;
                self.tasks_list.checked_at = None;
                Err(error)
            }
        };
        let (answered, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.tasks_list.waiting)
            .into_iter()
            .partition(|waiter| {
                waiter.program == finished.program
                    && waiter.actions_finished <= finished.actions_finished
            });
        for waiter in answered {
            let response = match &result {
                Ok(kept) => encode_success(
                    waiter.request_id,
                    ResponseResult::TasksList {
                        list: kept.list.clone(),
                    },
                ),
                Err(error) => encode_error(waiter.request_id, error.code, error.message.clone()),
            };
            let _ = waiter.respond_to.send(response);
        }
        if later.is_empty() {
            return;
        }
        let Some(program) = self.tasks_command.clone() else {
            for waiter in later {
                let _ = waiter.respond_to.send(encode_error(
                    waiter.request_id,
                    "tasks_unavailable",
                    "tasks.command is not set",
                ));
            }
            return;
        };
        self.tasks_list.waiting = later
            .into_iter()
            .map(|waiter| TasksListWaiter {
                program: program.clone(),
                ..waiter
            })
            .collect();
        self.start_tasks_list_run(program);
    }
}

async fn watched_changed(kept: Arc<KeptTasksList>) -> bool {
    tokio::task::spawn_blocking(move || kept.watched_changed())
        .await
        .unwrap_or_else(|error| {
            tracing::warn!(%error, "task list file check failed; running the command again");
            true
        })
}

type Watched = Vec<(PathBuf, Option<SystemTime>)>;

/// The file check's result, or `None` when its task failed, so the list is never kept with
/// an empty watch list that would never change.
fn joined_watched(joined: Result<Watched, tokio::task::JoinError>) -> Option<Watched> {
    joined
        .inspect_err(|error| {
            tracing::warn!(%error, "task list file check failed; the list will not be reused");
        })
        .ok()
}

async fn run_and_keep(program: PathBuf) -> Result<KeptTasksList, TasksError> {
    let started = SystemTime::now();
    let ran_at = Instant::now();
    let list = list_tasks(&program, TASKS_COMMAND_TIMEOUT).await?;
    let paths = watched_paths(&list);
    let watched = joined_watched(
        tokio::task::spawn_blocking(move || {
            paths
                .into_iter()
                .map(|path| {
                    let modified = modified_time(&path);
                    (path, modified)
                })
                .collect()
        })
        .await,
    );
    Ok(KeptTasksList {
        program,
        list,
        watched,
        started,
        ran_at,
    })
}

/// Every file the list names: `tasks[].copies[].file`, then `work[].file`.
fn listed_files(list: &serde_json::Value) -> impl Iterator<Item = &str> {
    let items = |key: &str| {
        list.get(key)
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
    };
    let copies = items("tasks")
        .filter_map(|task| task.get("copies").and_then(serde_json::Value::as_array))
        .flatten();
    copies
        .chain(items("work"))
        .filter_map(|item| item.get("file").and_then(serde_json::Value::as_str))
}

/// Every file the list names and each one's folder, each once and at most
/// `MAX_WATCHED_PATHS` of them.
fn watched_paths(list: &serde_json::Value) -> BTreeSet<PathBuf> {
    let mut paths = BTreeSet::new();
    for file in listed_files(list) {
        let file = PathBuf::from(file);
        let folder = file
            .parent()
            .filter(|folder| !folder.as_os_str().is_empty())
            .map(Path::to_path_buf);
        for path in folder.into_iter().chain([file]) {
            if paths.len() == MAX_WATCHED_PATHS && !paths.contains(&path) {
                tracing::debug!(
                    max = MAX_WATCHED_PATHS,
                    "task list names more paths than are watched"
                );
                return paths;
            }
            paths.insert(path);
        }
    }
    paths
}

fn modified_time(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
}

async fn list_tasks(program: &Path, timeout: Duration) -> Result<serde_json::Value, TasksError> {
    let stdout = run_tasks_command(
        program,
        &["list", "--json"],
        timeout,
        MAX_TASKS_OUTPUT_BYTES,
    )
    .await?;
    let list: serde_json::Value = serde_json::from_slice(&stdout)
        .map_err(|err| TasksError::new("tasks_bad_output", format!("output is not JSON: {err}")))?;
    match list.get("version") {
        Some(version) if version == 1 => Ok(list),
        Some(version) => Err(TasksError::new(
            "tasks_bad_output",
            format!("output has version {version}; only version 1 is understood"),
        )),
        None => Err(TasksError::new(
            "tasks_bad_output",
            "output has no version; only version 1 is understood",
        )),
    }
}

/// Runs the tasks command in its own process group and returns its stdout, at most
/// `max_output` bytes of it; a non-zero exit fails with stderr's last line.
async fn run_tasks_command(
    program: &Path,
    args: &[&str],
    timeout: Duration,
    max_output: usize,
) -> Result<Vec<u8>, TasksError> {
    let output = run_command(program, args, timeout, max_output).await?;
    if !output.status.success() {
        let message =
            last_line(&output.stderr).unwrap_or_else(|| format!("exited with {}", output.status));
        return Err(TasksError::new("tasks_failed", message));
    }
    if output.stdout_cut {
        return Err(TasksError::new(
            "tasks_bad_output",
            format!("output is larger than {max_output} bytes"),
        ));
    }
    Ok(output.stdout)
}

/// What a finished tasks command printed and how it exited.
struct CommandOutput {
    status: std::process::ExitStatus,
    /// At most the first `max_output` bytes.
    stdout: Vec<u8>,
    /// Whether stdout had more than that.
    stdout_cut: bool,
    /// The last `MAX_TASKS_STDERR_TAIL_BYTES` bytes.
    stderr: Vec<u8>,
}

/// Runs the tasks command with `args` and no shell, in its own process group, which is killed
/// when it finishes or after `timeout`.
async fn run_command(
    program: &Path,
    args: &[impl AsRef<std::ffi::OsStr>],
    timeout: Duration,
    max_output: usize,
) -> Result<CommandOutput, TasksError> {
    let mut process = std::process::Command::new(program);
    process
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::platform::configure_status_command(&mut process);
    let mut process = tokio::process::Command::from(process);
    process.kill_on_drop(true);
    let mut child = process.spawn().map_err(|err| {
        TasksError::new(
            "tasks_unavailable",
            format!("cannot run {}: {err}", program.display()),
        )
    })?;
    // Dropping the guard kills the whole process group, including children left behind.
    let _process_group = crate::platform::StatusCommandGuard::new(&child)
        .map_err(|err| TasksError::new("tasks_failed", err.to_string()))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let operation = async {
        let (status, stdout, stderr) = tokio::join!(
            child.wait(),
            read_head(stdout, max_output),
            read_tail(stderr, MAX_TASKS_STDERR_TAIL_BYTES),
        );
        let failed = |err: std::io::Error| TasksError::new("tasks_failed", err.to_string());
        let (stdout, stdout_cut) = stdout.map_err(failed)?;
        Ok(CommandOutput {
            status: status.map_err(failed)?,
            stdout,
            stdout_cut,
            stderr: stderr.unwrap_or_default(),
        })
    };
    match tokio::time::timeout(timeout, operation).await {
        Ok(result) => result,
        Err(_) => Err(TasksError::new(
            "tasks_timeout",
            format!(
                "tasks.command did not finish within {} ms",
                timeout.as_millis()
            ),
        )),
    }
}

/// The last non-blank line of a command's stderr.
fn last_line(output: &[u8]) -> Option<String> {
    String::from_utf8_lossy(output)
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .map(str::to_owned)
}

/// Reads a stream to its end, keeping its first `cap` bytes; also says whether it had more.
async fn read_head(
    reader: Option<impl AsyncRead + Unpin>,
    cap: usize,
) -> std::io::Result<(Vec<u8>, bool)> {
    let mut kept = Vec::new();
    let mut dropped = false;
    read_chunks(reader, |chunk| {
        let room = cap.saturating_sub(kept.len());
        kept.extend_from_slice(&chunk[..chunk.len().min(room)]);
        dropped |= chunk.len() > room;
    })
    .await?;
    Ok((kept, dropped))
}

/// Reads a stream to its end, keeping its last `cap` bytes.
async fn read_tail(reader: Option<impl AsyncRead + Unpin>, cap: usize) -> std::io::Result<Vec<u8>> {
    let mut kept = Vec::new();
    read_chunks(reader, |chunk| {
        kept.extend_from_slice(chunk);
        kept.drain(..kept.len().saturating_sub(cap));
    })
    .await?;
    Ok(kept)
}

/// Reads a stream to its end, handing each chunk read to `take`.
async fn read_chunks(
    reader: Option<impl AsyncRead + Unpin>,
    mut take: impl FnMut(&[u8]),
) -> std::io::Result<()> {
    let Some(mut reader) = reader else {
        return Ok(());
    };
    let mut buffer = [0_u8; 8192];
    loop {
        let count = reader.read(&mut buffer).await?;
        if count == 0 {
            return Ok(());
        }
        take(&buffer[..count]);
    }
}

/// A temporary folder of its own for one test, removed when the test drops it.
#[cfg(test)]
struct TestFolder(PathBuf);

#[cfg(test)]
impl TestFolder {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "herdr-tasks-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("test folder");
        Self(dir)
    }

    /// The path of `name` in this folder, as a string.
    fn file(&self, name: &str) -> String {
        self.0.join(name).to_string_lossy().into_owned()
    }
}

#[cfg(test)]
impl Drop for TestFolder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use crate::api::schema::{EmptyParams, Method, Request, ResponseResult, SuccessResponse};
    use crate::app::App;

    fn test_app() -> App {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        App::new(
            &crate::config::Config::default(),
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        )
    }

    fn fixture(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/app/api/tasks/fixtures")
            .join(name)
    }

    fn tasks_list(id: &str) -> Request {
        Request {
            id: id.into(),
            method: Method::TasksList(EmptyParams::default()),
        }
    }

    /// Sends one `tasks.list` and drives the app until its reply arrives.
    async fn list_once(app: &mut App, id: &str) -> String {
        let (respond_to, response_rx) = std::sync::mpsc::channel();
        assert!(app.handle_deferred_tasks_api_request(tasks_list(id), respond_to));
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            loop {
                if let Ok(response) = response_rx.try_recv() {
                    return response;
                }
                let event = app.event_rx.recv().await.expect("app event");
                app.handle_internal_event_with_render_impact(event);
            }
        })
        .await
        .expect("tasks.list replied")
    }

    fn error_of(response: &str) -> (String, String) {
        let error: crate::api::schema::ErrorResponse = serde_json::from_str(response).unwrap();
        (error.error.code, error.error.message)
    }

    #[cfg(unix)]
    async fn list_error(program: &str, timeout: std::time::Duration) -> super::TasksError {
        super::list_tasks(&fixture(program), timeout)
            .await
            .expect_err("the fake command fails")
    }

    #[tokio::test]
    async fn tasks_list_without_a_command_is_unavailable() {
        let mut app = test_app();
        assert_eq!(app.tasks_command, None);

        let response = list_once(&mut app, "req_unset").await;

        assert_eq!(
            error_of(&response),
            (
                "tasks_unavailable".to_owned(),
                "tasks.command is not set".to_owned()
            )
        );
    }

    #[tokio::test]
    async fn tasks_list_with_a_missing_program_is_unavailable() {
        let mut app = test_app();
        let missing = fixture("no-such-tasks-command");
        app.tasks_command = Some(missing.clone());

        let response = list_once(&mut app, "req_missing").await;

        let (code, message) = error_of(&response);
        assert_eq!(code, "tasks_unavailable");
        assert!(
            message.starts_with(&format!("cannot run {}: ", missing.display())),
            "{message}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tasks_list_failure_reports_the_last_stderr_line() {
        assert_eq!(
            list_error("list-fails.sh", super::TASKS_COMMAND_TIMEOUT).await,
            super::TasksError::new("tasks_failed", "claude-tasks: cannot read resume pointers")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tasks_list_times_out_and_stops_the_command() {
        let started = std::time::Instant::now();

        let error = list_error("list-slow.sh", std::time::Duration::from_millis(300)).await;

        assert_eq!(
            error,
            super::TasksError::new(
                "tasks_timeout",
                "tasks.command did not finish within 300 ms"
            )
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tasks_list_rejects_output_that_is_not_json() {
        let error = list_error("list-bad-json.sh", super::TASKS_COMMAND_TIMEOUT).await;
        assert_eq!(error.code, "tasks_bad_output");
        assert!(
            error.message.starts_with("output is not JSON: "),
            "{}",
            error.message
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tasks_list_rejects_another_contract_version() {
        assert_eq!(
            list_error("list-other-version.sh", super::TASKS_COMMAND_TIMEOUT).await,
            super::TasksError::new(
                "tasks_bad_output",
                "output has version 2; only version 1 is understood"
            )
        );
    }

    /// A fake tasks command in its own folder that counts its list runs and names
    /// `notes/STATUS.md` and `intent/feature.md` in its list. Any other first argument is an
    /// action: it answers `{"ok": true}`, or refuses with exit 3 once `act-exit` holds 3. A
    /// `list-sleep` file makes a list run take a second after it read `list.json`.
    #[cfg(unix)]
    struct CountingCommand {
        dir: std::path::PathBuf,
        /// Removes `dir` when the command is dropped.
        _folder: super::TestFolder,
    }

    #[cfg(unix)]
    impl CountingCommand {
        fn new(name: &str, exit_code: i32) -> Self {
            use std::os::unix::fs::PermissionsExt;

            let folder = super::TestFolder::new(name);
            let dir = folder.0.clone();
            std::fs::create_dir_all(dir.join("notes")).unwrap();
            std::fs::create_dir_all(dir.join("intent")).unwrap();
            std::fs::write(dir.join("notes/STATUS.md"), "| Task | Waiting on |\n").unwrap();
            std::fs::write(dir.join("intent/feature.md"), "# Feature\n").unwrap();
            let list = serde_json::json!({
                "version": 1,
                "tasks": [{"copies": [{"file": dir.join("notes/STATUS.md")}]}],
                "work": [{"file": dir.join("intent/feature.md")}],
                "problems": [],
            });
            std::fs::write(dir.join("list.json"), list.to_string()).unwrap();
            let script = dir.join("tasks-command");
            std::fs::write(
                &script,
                format!(
                    "#!/bin/sh\n\
                     if [ \"$1\" != list ]; then\n\
                     code=$(cat '{0}/act-exit' 2>/dev/null || echo 0)\n\
                     if [ \"$code\" = 0 ]; then echo '{{\"ok\": true}}'; \
                     else echo '{{\"ok\": false, \"reason\": \"row changed\"}}'; fi\n\
                     exit $code\n\
                     fi\n\
                     echo run >> '{0}/runs'\n\
                     list=$(cat '{0}/list.json')\n\
                     [ -f '{0}/list-sleep' ] && sleep 1\n\
                     printf '%s\\n' \"$list\"\n\
                     exit {exit_code}\n",
                    dir.display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
            Self {
                dir,
                _folder: folder,
            }
        }

        fn program(&self) -> std::path::PathBuf {
            self.dir.join("tasks-command")
        }

        fn runs(&self) -> usize {
            std::fs::read_to_string(self.dir.join("runs"))
                .map(|runs| runs.lines().count())
                .unwrap_or(0)
        }

        fn set_modified(&self, relative: &str, seconds: u64) {
            std::fs::File::open(self.dir.join(relative))
                .unwrap()
                .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds))
                .unwrap();
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tasks_list_reuses_the_last_list_until_a_named_file_changes() {
        let command = CountingCommand::new("reuse", 0);
        let mut app = test_app();
        app.tasks_command = Some(command.program());

        let first = list_once(&mut app, "first").await;
        let second = list_once(&mut app, "second").await;
        assert_eq!(
            command.runs(),
            1,
            "no file changed, so the command ran once"
        );
        let list = |response: &str| {
            serde_json::from_str::<SuccessResponse>(response)
                .unwrap()
                .result
        };
        assert_eq!(list(&first), list(&second));

        command.set_modified("notes/STATUS.md", 1_000);
        app.tasks_list
            .age_last_check_for_test(super::TASKS_LIST_CHECK_INTERVAL);
        list_once(&mut app, "after-notes-change").await;
        assert_eq!(command.runs(), 2, "a named file changed");

        command.set_modified("intent/feature.md", 1_000);
        app.tasks_list
            .age_last_check_for_test(super::TASKS_LIST_CHECK_INTERVAL);
        list_once(&mut app, "after-intent-change").await;
        assert_eq!(command.runs(), 3, "a named work file changed");

        list_once(&mut app, "unchanged-again").await;
        assert_eq!(command.runs(), 3);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tasks_list_reruns_when_a_named_folder_gains_a_file() {
        let command = CountingCommand::new("folder", 0);
        let mut app = test_app();
        app.tasks_command = Some(command.program());

        list_once(&mut app, "first").await;
        std::fs::write(command.dir.join("notes/NEW-STATUS.md"), "new\n").unwrap();
        command.set_modified("notes", 1_000);
        app.tasks_list
            .age_last_check_for_test(super::TASKS_LIST_CHECK_INTERVAL);
        list_once(&mut app, "after-new-file").await;

        assert_eq!(command.runs(), 2);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tasks_list_reruns_once_the_last_list_is_thirty_seconds_old() {
        let command = CountingCommand::new("age", 0);
        let mut app = test_app();
        app.tasks_command = Some(command.program());

        list_once(&mut app, "first").await;
        app.tasks_list
            .age_last_run_for_test(super::TASKS_LIST_MAX_AGE);
        list_once(&mut app, "old").await;

        assert_eq!(command.runs(), 2);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tasks_list_reruns_after_a_failed_run() {
        let command = CountingCommand::new("failed", 1);
        let mut app = test_app();
        app.tasks_command = Some(command.program());

        let first = list_once(&mut app, "first").await;
        let second = list_once(&mut app, "second").await;

        assert_eq!(error_of(&first).0, "tasks_failed");
        assert_eq!(error_of(&second).0, "tasks_failed");
        assert_eq!(command.runs(), 2, "a failed run is never reused");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tasks_list_requests_during_a_run_share_its_result() {
        let command = CountingCommand::new("shared", 0);
        let mut app = test_app();
        app.tasks_command = Some(command.program());

        let (first_tx, first_rx) = std::sync::mpsc::channel();
        let (second_tx, second_rx) = std::sync::mpsc::channel();
        assert!(app.handle_deferred_tasks_api_request(tasks_list("first"), first_tx));
        assert!(app.handle_deferred_tasks_api_request(tasks_list("second"), second_tx));
        let event = tokio::time::timeout(std::time::Duration::from_secs(15), app.event_rx.recv())
            .await
            .unwrap()
            .unwrap();
        app.handle_internal_event_with_render_impact(event);

        let first: SuccessResponse = serde_json::from_str(&first_rx.try_recv().unwrap()).unwrap();
        let second: SuccessResponse = serde_json::from_str(&second_rx.try_recv().unwrap()).unwrap();
        assert_eq!((first.id.as_str(), second.id.as_str()), ("first", "second"));
        assert_eq!(first.result, second.result);
        assert_eq!(command.runs(), 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tasks_list_runs_the_new_command_after_a_config_change() {
        let first_command = CountingCommand::new("config-first", 0);
        let second_command = CountingCommand::new("config-second", 0);
        let mut app = test_app();
        app.tasks_command = Some(first_command.program());
        list_once(&mut app, "first").await;

        app.tasks_command = Some(second_command.program());
        list_once(&mut app, "second").await;

        assert_eq!((first_command.runs(), second_command.runs()), (1, 1));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tasks_list_asked_after_a_command_change_gets_the_new_commands_list() {
        let old_command = CountingCommand::new("change-old", 0);
        let new_command = CountingCommand::new("change-new", 0);
        let mut app = test_app();
        app.tasks_command = Some(old_command.program());

        let (before_tx, before_rx) = std::sync::mpsc::channel();
        let (after_tx, after_rx) = std::sync::mpsc::channel();
        assert!(app.handle_deferred_tasks_api_request(tasks_list("before"), before_tx));
        app.tasks_command = Some(new_command.program());
        assert!(app.handle_deferred_tasks_api_request(tasks_list("after"), after_tx));
        let after = tokio::time::timeout(std::time::Duration::from_secs(15), async {
            loop {
                if let Ok(response) = after_rx.try_recv() {
                    return response;
                }
                let event = app.event_rx.recv().await.expect("app event");
                app.handle_internal_event_with_render_impact(event);
            }
        })
        .await
        .expect("the later request replied");

        let work_file = |response: &str| {
            let SuccessResponse {
                result: ResponseResult::TasksList { list },
                ..
            } = serde_json::from_str(response).unwrap()
            else {
                panic!("a task list: {response}");
            };
            list["work"][0]["file"].as_str().unwrap().to_owned()
        };
        let before = before_rx
            .try_recv()
            .expect("the earlier request replied first");
        assert!(work_file(&before).starts_with(old_command.dir.to_str().unwrap()));
        assert!(
            work_file(&after).starts_with(new_command.dir.to_str().unwrap()),
            "asked after the change, answered by the new command"
        );
        assert_eq!((old_command.runs(), new_command.runs()), (1, 1));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tasks_list_checks_the_named_files_at_most_once_a_second() {
        let command = CountingCommand::new("once-a-second", 0);
        let mut app = test_app();
        app.tasks_command = Some(command.program());

        let first = list_once(&mut app, "first").await;
        command.set_modified("notes/STATUS.md", 1_000);
        let soon = list_once(&mut app, "within-a-second").await;
        assert_eq!(
            command.runs(),
            1,
            "no check within a second of the last one"
        );
        let list = |response: &str| {
            serde_json::from_str::<SuccessResponse>(response)
                .unwrap()
                .result
        };
        assert_eq!(list(&soon), list(&first));

        app.tasks_list
            .age_last_check_for_test(super::TASKS_LIST_CHECK_INTERVAL);
        list_once(&mut app, "a-second-later").await;
        assert_eq!(command.runs(), 2, "a second later the change is seen");
    }

    #[test]
    fn tasks_list_watches_each_path_once_and_at_most_the_cap() {
        let file = |n: usize| serde_json::json!({"file": format!("/notes/{n}/STATUS.md")});
        let twice = serde_json::json!({
            "version": 1,
            "tasks": [{"copies": [file(1), file(1)]}],
            "work": [file(1)],
        });
        assert_eq!(
            super::watched_paths(&twice).len(),
            2,
            "the file and its folder, once each"
        );

        let many = serde_json::json!({
            "version": 1,
            "tasks": [{"copies": (0..super::MAX_WATCHED_PATHS).map(file).collect::<Vec<_>>()}],
            "work": [],
        });
        assert_eq!(
            super::watched_paths(&many).len(),
            super::MAX_WATCHED_PATHS,
            "past the cap the 30 s rerun catches changes"
        );
    }

    #[tokio::test]
    async fn a_failed_file_check_forces_the_next_run() {
        let joined = tokio::task::spawn_blocking(|| -> Vec<(std::path::PathBuf, _)> {
            panic!("the file check failed")
        })
        .await;
        let watched = super::joined_watched(joined);
        assert_eq!(watched, None, "no empty watch list that would never change");

        let kept = super::KeptTasksList {
            program: "claude-tasks".into(),
            list: serde_json::json!({"version": 1}),
            watched,
            started: std::time::SystemTime::now(),
            ran_at: std::time::Instant::now(),
        };
        assert!(!kept.is_current(std::path::Path::new("claude-tasks")));
        assert!(kept.watched_changed());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tasks_list_rejects_output_over_the_limit() {
        let error = super::run_tasks_command(
            &fixture("list-example.sh"),
            &["list", "--json"],
            super::TASKS_COMMAND_TIMEOUT,
            100,
        )
        .await
        .expect_err("the example list is over 100 bytes");
        assert_eq!(
            error,
            super::TasksError::new("tasks_bad_output", "output is larger than 100 bytes")
        );
    }

    /// Sends one `tasks.act` and drives the app until its reply arrives; the list it keeps
    /// first names the file, as `tasks.act` needs.
    async fn act_once(app: &mut App, row: &str) -> String {
        app.tasks_list.keep_list_for_test(serde_json::json!({
            "version": 1,
            "tasks": [{"copies": [{"file": "/nonexistent/STATUS.md"}]}],
            "work": [],
            "problems": [],
        }));
        let (respond_to, response_rx) = std::sync::mpsc::channel();
        let request = Request {
            id: "act".into(),
            method: Method::TasksAct(crate::api::schema::TasksActParams {
                action: "done".into(),
                copies: vec![crate::api::schema::TaskCopy {
                    file: "/nonexistent/STATUS.md".into(),
                    row: row.into(),
                }],
                ..Default::default()
            }),
        };
        assert!(app.handle_deferred_tasks_api_request(request, respond_to));
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            loop {
                if let Ok(response) = response_rx.try_recv() {
                    return response;
                }
                let event = app.event_rx.recv().await.expect("app event");
                app.handle_internal_event_with_render_impact(event);
            }
        })
        .await
        .expect("tasks.act replied")
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_finished_action_makes_the_next_tasks_list_run_again() {
        let command = CountingCommand::new("after-act", 0);
        let mut app = test_app();
        app.tasks_command = Some(command.program());

        list_once(&mut app, "first").await;
        list_once(&mut app, "kept").await;
        assert_eq!(command.runs(), 1, "the kept list is reused");

        let done = act_once(&mut app, "| a | me |").await;
        assert!(done.contains("\"ok\":true"), "{done}");
        list_once(&mut app, "after-done").await;
        assert_eq!(command.runs(), 2, "a done action drops the kept list");

        std::fs::write(command.dir.join("act-exit"), "3").unwrap();
        let refused = act_once(&mut app, "| a | me |").await;
        assert!(refused.contains("row changed"), "{refused}");
        list_once(&mut app, "after-refusal").await;
        assert_eq!(
            command.runs(),
            3,
            "a refused action drops the kept list too"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_list_read_before_an_action_finished_is_not_kept() {
        let command = CountingCommand::new("act-during-list", 0);
        std::fs::write(command.dir.join("list-sleep"), "").unwrap();
        let mut app = test_app();
        app.tasks_command = Some(command.program());

        // The list run starts first and sleeps; the action finishes while it runs.
        let (list_tx, list_rx) = std::sync::mpsc::channel();
        assert!(app.handle_deferred_tasks_api_request(tasks_list("during"), list_tx));
        act_once(&mut app, "| a | me |").await;
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            while list_rx.try_recv().is_err() {
                let event = app.event_rx.recv().await.expect("app event");
                app.handle_internal_event_with_render_impact(event);
            }
        })
        .await
        .expect("the list replied");
        std::fs::remove_file(command.dir.join("list-sleep")).unwrap();

        list_once(&mut app, "after").await;
        assert_eq!(
            command.runs(),
            2,
            "the list read while the action ran may predate it, so it is not reused"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_tasks_list_asked_after_an_action_gets_a_list_read_after_it() {
        let command = CountingCommand::new("ask-after-act", 0);
        std::fs::write(command.dir.join("list-sleep"), "").unwrap();
        let mut app = test_app();
        app.tasks_command = Some(command.program());

        // The first run reads the list, then sleeps; the action changes the list and finishes
        // meanwhile, and only then is the second request asked.
        let (before_tx, before_rx) = std::sync::mpsc::channel();
        assert!(app.handle_deferred_tasks_api_request(tasks_list("before"), before_tx));
        while command.runs() == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        // The run reads the list right after it counts itself, well within its one-second sleep.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        std::fs::write(
            command.dir.join("list.json"),
            r#"{"version": 1, "tasks": [], "work": [], "problems": ["after the action"]}"#,
        )
        .unwrap();
        act_once(&mut app, "| a | me |").await;
        std::fs::remove_file(command.dir.join("list-sleep")).unwrap();
        let after = list_once(&mut app, "after").await;

        let problems = |response: &str| {
            let SuccessResponse {
                result: ResponseResult::TasksList { list },
                ..
            } = serde_json::from_str(response).unwrap()
            else {
                panic!("a task list: {response}");
            };
            list["problems"].clone()
        };
        let before = before_rx.try_recv().expect("the earlier request replied");
        assert_eq!(
            problems(&before),
            serde_json::json!([]),
            "read before the action"
        );
        assert_eq!(
            problems(&after),
            serde_json::json!(["after the action"]),
            "asked after the action, answered by a run after it"
        );
        assert_eq!(command.runs(), 2);
    }

    #[test]
    fn tasks_list_request_uses_the_tasks_list_method_name() {
        let json = serde_json::to_value(tasks_list("req_tasks")).unwrap();
        assert_eq!(json["method"], "tasks.list");
        assert_eq!(
            serde_json::from_value::<Request>(json).unwrap(),
            tasks_list("req_tasks")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tasks_list_replies_with_the_commands_json() {
        let mut app = test_app();
        app.tasks_command = Some(fixture("list-example.sh"));

        let response = list_once(&mut app, "req_tasks").await;

        let expected: serde_json::Value =
            serde_json::from_str(include_str!("tasks/fixtures/list-example.json")).unwrap();
        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(success.id, "req_tasks");
        assert_eq!(success.result, ResponseResult::TasksList { list: expected });
        let wire: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(wire["result"]["type"], "tasks_list");
        assert_eq!(wire["result"]["list"]["version"], 1);
    }
}
