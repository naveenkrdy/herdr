//! `tasks.act`: one of the five task actions, run by `tasks.command` off the server loop.
//! The shape is checked before anything runs, and every file must be one the kept task list
//! names, as for `tasks.open`; values go to the command as `--flag=value` arguments with no
//! shell, so a value that starts with `-` stays a value.

use std::path::Path;
use std::time::Duration;

use crate::api::schema::{ResponseResult, TasksActParams};
use crate::app::App;
use crate::events::AppEvent;

use super::super::responses::{encode_error, encode_success};
use super::{last_line, run_command, TasksError, TASKS_COMMAND_TIMEOUT};

/// An action's reply is one short JSON line; anything longer is not a reply.
const MAX_ACT_OUTPUT_BYTES: usize = 64 * 1024;

const ACTIONS: [&str; 5] = ["done", "delete", "move", "reorder", "new"];

#[derive(Debug)]
pub(crate) struct TasksActFinished {
    request_id: String,
    respond_to: std::sync::mpsc::Sender<String>,
    result: Result<(bool, Option<String>), TasksError>,
}

impl App {
    /// Checks the action's shape and that the kept list names its files, then runs it off the
    /// server loop; the reply is sent when the command finishes.
    pub(super) fn start_tasks_act(
        &mut self,
        request_id: String,
        params: TasksActParams,
        respond_to: std::sync::mpsc::Sender<String>,
    ) {
        let args = match tasks_act_args(&params) {
            Ok(args) => args,
            Err(message) => {
                let _ = respond_to.send(encode_error(request_id, "invalid_params", message));
                return;
            }
        };
        let Some(program) = self.tasks_command.clone() else {
            let _ = respond_to.send(encode_error(
                request_id,
                "tasks_unavailable",
                "tasks.command is not set",
            ));
            return;
        };
        if let Err(message) =
            action_files(&params).try_for_each(|file| self.check_task_file_listed(file))
        {
            let _ = respond_to.send(encode_error(request_id, "tasks_not_listed", message));
            return;
        }
        let event_tx = self.event_tx.clone();
        tokio::spawn(async move {
            let result = run_tasks_act(&program, &args, TASKS_COMMAND_TIMEOUT).await;
            let _ = event_tx
                .send(AppEvent::TasksActFinished(Box::new(TasksActFinished {
                    request_id,
                    respond_to,
                    result,
                })))
                .await;
        });
    }

    /// Replies to the action, and drops the kept list whatever the outcome, so the next
    /// `tasks.list` reads the notes files again.
    pub(crate) fn handle_api_tasks_act_finished(&mut self, finished: TasksActFinished) {
        self.tasks_list.action_finished();
        let response = match finished.result {
            Ok((ok, reason)) => {
                encode_success(finished.request_id, ResponseResult::TasksAct { ok, reason })
            }
            Err(error) => encode_error(finished.request_id, error.code, error.message),
        };
        let _ = finished.respond_to.send(response);
    }
}

/// The command's arguments for a well-shaped action, or why the shape is wrong.
fn tasks_act_args(params: &TasksActParams) -> Result<Vec<String>, String> {
    let action = params.action.as_str();
    if !ACTIONS.contains(&action) {
        return Err(format!("action must be one of {}", ACTIONS.join(", ")));
    }
    let mut args = vec![action.to_owned()];
    if action == "new" {
        if !params.copies.is_empty() || params.to.is_some() || params.direction.is_some() {
            return Err("new takes only file, task and who".into());
        }
        let (Some(file), Some(task), Some(who)) = (
            non_empty(&params.file),
            non_empty(&params.task),
            non_empty(&params.who),
        ) else {
            return Err("new needs file, task and who".into());
        };
        args.extend([
            format!("--file={file}"),
            format!("--task={task}"),
            format!("--who={who}"),
        ]);
        return Ok(args);
    }
    let extra_to = params.to.is_some() && action != "move";
    let extra_direction = params.direction.is_some() && action != "reorder";
    if params.file.is_some()
        || params.task.is_some()
        || params.who.is_some()
        || extra_to
        || extra_direction
    {
        return Err(match action {
            "move" => "move takes only copies and to".into(),
            "reorder" => "reorder takes only copies and direction".into(),
            _ => format!("{action} takes only copies"),
        });
    }
    if params.copies.is_empty() {
        return Err(format!("{action} needs at least one copy"));
    }
    for copy in &params.copies {
        if copy.file.is_empty() || copy.row.is_empty() {
            return Err("each copy needs a file and a row".into());
        }
        args.extend([
            format!("--file={}", copy.file),
            format!("--row={}", copy.row),
        ]);
    }
    match action {
        "move" => {
            let Some(to) = non_empty(&params.to) else {
                return Err("move needs to".into());
            };
            args.push(format!("--to={to}"));
        }
        "reorder" => match params.direction.as_deref() {
            Some("up") => args.push("--up".into()),
            Some("down") => args.push("--down".into()),
            _ => return Err("reorder needs direction up or down".into()),
        },
        _ => {}
    }
    Ok(args)
}

/// The files a well-shaped action changes: `file` for `new`, each copy's file otherwise.
fn action_files(params: &TasksActParams) -> impl Iterator<Item = &str> {
    params
        .file
        .as_deref()
        .into_iter()
        .chain(params.copies.iter().map(|copy| copy.file.as_str()))
}

fn non_empty(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|value| !value.is_empty())
}

/// The reader's reply line: `{"ok": true}` or `{"ok": false, "reason": "..."}`.
#[derive(serde::Deserialize)]
struct ActReply {
    ok: bool,
    #[serde(default)]
    reason: Option<String>,
}

/// Runs one action: exit 0 with an ok line is done, exit 3 with a refusal line is refused
/// with its reason; anything else fails with the reader's reason or stderr's last line.
async fn run_tasks_act(
    program: &Path,
    args: &[String],
    timeout: Duration,
) -> Result<(bool, Option<String>), TasksError> {
    let output = run_command(program, args, timeout, MAX_ACT_OUTPUT_BYTES).await?;
    let reply = (!output.stdout_cut)
        .then(|| last_line(&output.stdout))
        .flatten()
        .and_then(|line| serde_json::from_str::<ActReply>(&line).ok());
    match (output.status.code(), reply) {
        (Some(0), Some(ActReply { ok: true, .. })) => Ok((true, None)),
        (Some(3), Some(ActReply { ok: false, reason })) => Ok((
            false,
            Some(reason.unwrap_or_else(|| "refused without a reason".into())),
        )),
        (Some(0 | 3), None) => Err(TasksError::new(
            "tasks_failed",
            format!(
                "tasks.command exited with {} without its reply line",
                output.status
            ),
        )),
        (Some(0 | 3), Some(reply)) => Err(TasksError::new(
            "tasks_failed",
            match reply.reason {
                Some(reason) => format!(
                    "tasks.command exited with {} but replied ok: {}: {reason}",
                    output.status, reply.ok
                ),
                None => format!(
                    "tasks.command exited with {} but replied ok: {}",
                    output.status, reply.ok
                ),
            },
        )),
        (_, reply) => Err(TasksError::new(
            "tasks_failed",
            reply
                .and_then(|reply| reply.reason)
                .or_else(|| last_line(&output.stderr))
                .unwrap_or_else(|| format!("exited with {}", output.status)),
        )),
    }
}

#[cfg(test)]
mod tests {
    use crate::api::schema::{
        ErrorResponse, Method, Request, ResponseResult, SuccessResponse, TaskCopy, TasksActParams,
    };
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

    use super::super::TestFolder as Notes;

    /// The arguments the fake command wrote to `args` in `notes`, or none when it did not run.
    fn recorded_args(notes: &Notes) -> Option<Vec<String>> {
        std::fs::read_to_string(notes.0.join("args"))
            .ok()
            .map(|args| args.lines().map(str::to_owned).collect())
    }

    fn copy(file: &str, row: &str) -> TaskCopy {
        TaskCopy {
            file: file.into(),
            row: row.into(),
        }
    }

    fn params(action: &str) -> TasksActParams {
        TasksActParams {
            action: action.into(),
            ..TasksActParams::default()
        }
    }

    /// Sends one `tasks.act` and drives the app until its reply arrives.
    async fn act_once(app: &mut App, params: TasksActParams) -> String {
        let (respond_to, response_rx) = std::sync::mpsc::channel();
        let request = Request {
            id: "act".into(),
            method: Method::TasksAct(params),
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

    fn act_result(response: &str) -> (bool, Option<String>) {
        let success: SuccessResponse = serde_json::from_str(response)
            .unwrap_or_else(|_| panic!("a tasks_act reply: {response}"));
        let ResponseResult::TasksAct { ok, reason } = success.result else {
            panic!("a tasks_act reply: {response}");
        };
        (ok, reason)
    }

    fn error_of(response: &str) -> (String, String) {
        let error: ErrorResponse =
            serde_json::from_str(response).unwrap_or_else(|_| panic!("an error reply: {response}"));
        (error.error.code, error.error.message)
    }

    /// Keeps a task list whose one task has a copy in each of `files`.
    fn keep_list_of(app: &mut App, files: &[&str]) {
        let copies = files
            .iter()
            .map(|file| serde_json::json!({"file": file, "line": 3, "row": "| t | me |"}))
            .collect::<Vec<_>>();
        app.tasks_list.keep_list_for_test(serde_json::json!({
            "version": 1,
            "tasks": [{"task": "t", "copies": copies}],
            "work": [],
            "problems": [],
        }));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tasks_act_refuses_a_file_the_kept_list_does_not_name_without_running_anything() {
        let notes = Notes::new("act-unlisted");
        let (listed, other) = (notes.file("listed.md"), notes.file("other.md"));
        let refused = [
            (
                "a copy the list does not name",
                true,
                TasksActParams {
                    copies: vec![copy(&listed, "| a | me |"), copy(&other, "| a | me |")],
                    ..params("done")
                },
                format!("{other} is not a file in the task list"),
            ),
            (
                "a new task in a file the list does not name",
                true,
                TasksActParams {
                    file: Some(other.clone()),
                    task: Some("t".into()),
                    who: Some("me".into()),
                    ..params("new")
                },
                format!("{other} is not a file in the task list"),
            ),
            (
                "no kept list",
                false,
                TasksActParams {
                    copies: vec![copy(&listed, "| a | me |")],
                    ..params("delete")
                },
                "no task list is kept; read it with tasks.list first".to_owned(),
            ),
        ];
        for (case, keep, params, message) in refused {
            let mut app = test_app();
            app.tasks_command = Some(fixture("act-ok.sh"));
            if keep {
                keep_list_of(&mut app, &[&listed]);
            }
            let response = act_once(&mut app, params).await;
            assert_eq!(
                error_of(&response),
                ("tasks_not_listed".to_owned(), message),
                "{case}"
            );
            assert_eq!(recorded_args(&notes), None, "{case}: the command never ran");
        }

        let mut app = test_app();
        app.tasks_command = Some(fixture("act-ok.sh"));
        keep_list_of(&mut app, &[&listed]);
        let response = act_once(
            &mut app,
            TasksActParams {
                copies: vec![copy(&listed, "| a | me |")],
                ..params("done")
            },
        )
        .await;
        assert_eq!(act_result(&response), (true, None), "a listed file acts");
        assert_eq!(
            recorded_args(&notes),
            Some(vec![
                "done".to_owned(),
                format!("--file={listed}"),
                "--row=| a | me |".to_owned()
            ])
        );
    }

    #[test]
    fn tasks_act_request_has_the_documented_shape() {
        let request = Request {
            id: "req_act".into(),
            method: Method::TasksAct(TasksActParams {
                action: "move".into(),
                copies: vec![copy("/notes/STATUS.md", "| a | Nadine |")],
                to: Some("me".into()),
                ..TasksActParams::default()
            }),
        };
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "id": "req_act",
                "method": "tasks.act",
                "params": {
                    "action": "move",
                    "copies": [{"file": "/notes/STATUS.md", "row": "| a | Nadine |"}],
                    "to": "me",
                },
            })
        );
        assert_eq!(serde_json::from_value::<Request>(json).unwrap(), request);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tasks_act_runs_each_action_with_its_arguments_and_no_shell() {
        let notes = Notes::new("act-args");
        let (a, b) = (notes.file("a.md"), notes.file("b.md"));
        let row = "| Chrome | stuck at pairing | Nadine: go |";
        let cases = [
            (
                TasksActParams {
                    copies: vec![copy(&a, row), copy(&b, "| x | me |")],
                    ..params("done")
                },
                vec![
                    "done".to_owned(),
                    format!("--file={a}"),
                    format!("--row={row}"),
                    format!("--file={b}"),
                    "--row=| x | me |".to_owned(),
                ],
            ),
            (
                TasksActParams {
                    copies: vec![copy(&a, row)],
                    ..params("delete")
                },
                vec![
                    "delete".into(),
                    format!("--file={a}"),
                    format!("--row={row}"),
                ],
            ),
            (
                TasksActParams {
                    copies: vec![copy(&a, row)],
                    to: Some("me".into()),
                    ..params("move")
                },
                vec![
                    "move".into(),
                    format!("--file={a}"),
                    format!("--row={row}"),
                    "--to=me".into(),
                ],
            ),
            (
                TasksActParams {
                    copies: vec![copy(&a, row)],
                    direction: Some("down".into()),
                    ..params("reorder")
                },
                vec![
                    "reorder".into(),
                    format!("--file={a}"),
                    format!("--row={row}"),
                    "--down".into(),
                ],
            ),
            (
                TasksActParams {
                    file: Some(a.clone()),
                    task: Some("-v looks like a flag; $(not run)".into()),
                    who: Some("Nadine".into()),
                    ..params("new")
                },
                vec![
                    "new".into(),
                    format!("--file={a}"),
                    "--task=-v looks like a flag; $(not run)".into(),
                    "--who=Nadine".into(),
                ],
            ),
        ];
        let mut app = test_app();
        app.tasks_command = Some(fixture("act-ok.sh"));
        for (params, expected) in cases {
            keep_list_of(&mut app, &[&a, &b]);
            let action = params.action.clone();
            let response = act_once(&mut app, params).await;
            assert_eq!(act_result(&response), (true, None), "{action}");
            assert_eq!(recorded_args(&notes), Some(expected), "{action}");
        }
        keep_list_of(&mut app, &[&a]);
        let response = act_once(
            &mut app,
            TasksActParams {
                copies: vec![copy(&a, row)],
                ..params("done")
            },
        )
        .await;
        let wire: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(
            wire["result"],
            serde_json::json!({"type": "tasks_act", "ok": true, "reason": null})
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tasks_act_refusal_is_a_reply_with_the_readers_reason() {
        let notes = Notes::new("act-refused");
        let mut app = test_app();
        app.tasks_command = Some(fixture("act-refuses.sh"));
        keep_list_of(&mut app, &[&notes.file("a.md")]);

        let response = act_once(
            &mut app,
            TasksActParams {
                copies: vec![copy(&notes.file("a.md"), "| a | me |")],
                ..params("delete")
            },
        )
        .await;

        assert_eq!(
            act_result(&response),
            (false, Some("row changed since the list was read".into()))
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tasks_act_failures_are_errors() {
        let notes = Notes::new("act-failed");
        let done = || TasksActParams {
            copies: vec![copy(&notes.file("a.md"), "| a | me |")],
            ..params("done")
        };
        let mut app = test_app();

        app.tasks_command = Some(fixture("act-bad-args.sh"));
        keep_list_of(&mut app, &[&notes.file("a.md")]);
        assert_eq!(
            error_of(&act_once(&mut app, done()).await),
            (
                "tasks_failed".to_owned(),
                "bad arguments: one --row for each --file".to_owned()
            ),
            "exit 2 gives the reader's reason"
        );

        app.tasks_command = Some(fixture("list-fails.sh"));
        keep_list_of(&mut app, &[&notes.file("a.md")]);
        assert_eq!(
            error_of(&act_once(&mut app, done()).await),
            (
                "tasks_failed".to_owned(),
                "claude-tasks: cannot read resume pointers".to_owned()
            ),
            "another exit gives stderr's last line"
        );

        app.tasks_command = Some(fixture("list-bad-json.sh"));
        keep_list_of(&mut app, &[&notes.file("a.md")]);
        assert_eq!(
            error_of(&act_once(&mut app, done()).await).0,
            "tasks_failed",
            "exit 0 without an ok line"
        );

        app.tasks_command = None;
        keep_list_of(&mut app, &[&notes.file("a.md")]);
        assert_eq!(
            error_of(&act_once(&mut app, done()).await),
            (
                "tasks_unavailable".to_owned(),
                "tasks.command is not set".to_owned()
            )
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tasks_act_reply_that_disagrees_with_the_exit_keeps_its_reason() {
        let program = std::path::Path::new("/bin/sh");
        let run = |script: &str| {
            let args = vec!["-c".to_owned(), script.to_owned()];
            async move {
                super::run_tasks_act(program, &args, std::time::Duration::from_secs(5))
                    .await
                    .map_err(|error| (error.code, error.message))
            }
        };

        assert_eq!(
            run(r#"echo '{"ok": false, "reason": "no such row"}'; exit 0"#).await,
            Err((
                "tasks_failed",
                "tasks.command exited with exit status: 0 but replied ok: false: no such row"
                    .to_owned()
            ))
        );
        assert_eq!(
            run(r#"echo '{"ok": true}'; exit 3"#).await,
            Err((
                "tasks_failed",
                "tasks.command exited with exit status: 3 but replied ok: true".to_owned()
            ))
        );
    }

    #[tokio::test]
    async fn tasks_act_without_a_runnable_command_is_unavailable() {
        let notes = Notes::new("act-unset");
        let done = || TasksActParams {
            copies: vec![copy(&notes.file("a.md"), "| a | me |")],
            ..params("done")
        };
        let mut app = test_app();
        app.tasks_command = None;
        keep_list_of(&mut app, &[&notes.file("a.md")]);
        assert_eq!(
            error_of(&act_once(&mut app, done()).await),
            (
                "tasks_unavailable".to_owned(),
                "tasks.command is not set".to_owned()
            )
        );

        app.tasks_command = Some(fixture("no-such-tasks-command"));
        keep_list_of(&mut app, &[&notes.file("a.md")]);
        assert_eq!(
            error_of(&act_once(&mut app, done()).await).0,
            "tasks_unavailable"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tasks_act_times_out_and_stops_the_command() {
        let started = std::time::Instant::now();
        let error = super::run_tasks_act(
            &fixture("list-slow.sh"),
            &["done".to_owned()],
            std::time::Duration::from_millis(300),
        )
        .await
        .expect_err("the slow command times out");
        assert_eq!(error.code, "tasks_timeout");
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tasks_act_rejects_a_bad_shape_without_running_anything() {
        let notes = Notes::new("act-shape");
        let a = notes.file("a.md");
        let one = || vec![copy(&a, "| a | me |")];
        let bad = [
            (
                params("archive"),
                "action must be one of done, delete, move, reorder, new",
            ),
            (params("done"), "done needs at least one copy"),
            (
                TasksActParams {
                    copies: vec![copy(&a, "")],
                    ..params("done")
                },
                "each copy needs a file and a row",
            ),
            (
                TasksActParams {
                    copies: one(),
                    ..params("move")
                },
                "move needs to",
            ),
            (
                TasksActParams {
                    copies: one(),
                    direction: Some("sideways".into()),
                    ..params("reorder")
                },
                "reorder needs direction up or down",
            ),
            (
                TasksActParams {
                    copies: one(),
                    task: Some("t".into()),
                    ..params("delete")
                },
                "delete takes only copies",
            ),
            (
                TasksActParams {
                    file: Some(a.clone()),
                    task: Some("t".into()),
                    ..params("new")
                },
                "new needs file, task and who",
            ),
            (
                TasksActParams {
                    file: Some(a.clone()),
                    task: Some("t".into()),
                    who: Some("me".into()),
                    copies: one(),
                    ..params("new")
                },
                "new takes only file, task and who",
            ),
        ];
        let mut app = test_app();
        app.tasks_command = Some(fixture("act-ok.sh"));
        for (params, message) in bad {
            let response = act_once(&mut app, params).await;
            assert_eq!(
                error_of(&response),
                ("invalid_params".to_owned(), message.to_owned())
            );
        }
        assert_eq!(recorded_args(&notes), None, "the command never ran");
    }
}
