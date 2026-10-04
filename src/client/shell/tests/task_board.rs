use super::*;
use crate::client::shell::task_board::model::{
    parse_task_list, DueLevel, TaskGroup, TaskSection, WorkKind, WorkStage,
};
use crate::client::shell::task_board::{
    footer_keys, render_task_board, AgentLabels, AgentState, BoardAgent, BoardOpener, BoardView,
    CardOwner, CardStatus, Category, FilterPick, Fold, RowKey, TaskBoard, TaskBoardHits,
    TaskBoardStore, TaskKey,
};
use crate::input::TerminalKey;

const LIST_V1: &str = include_str!("../../../../tests/fixtures/task-board/list-v1.json");

#[test]
fn task_list_v1_reads_every_field_the_contract_names() {
    let list = parse_task_list(LIST_V1).expect("the fixture is contract v1");
    assert_eq!(list.version, 1);
    assert_eq!(list.generated, "2026-09-25T08:05:12+05:30");
    assert_eq!(list.tasks.len(), 13);
    let groups = list.tasks.iter().map(|task| task.group).collect::<Vec<_>>();
    for group in [
        TaskGroup::You,
        TaskGroup::Claude,
        TaskGroup::Waiting,
        TaskGroup::Done,
    ] {
        assert!(groups.contains(&group), "{group:?} in {groups:?}");
    }
    let first = &list.tasks[0];
    assert_eq!(first.n, 1);
    assert_eq!(first.space, "oakwood");
    assert_eq!(first.project, "Chrome");
    assert_eq!(first.task, "Per-client Chrome is stuck at pairing");
    assert_eq!(first.state, "go on the pairing broadcast");
    assert_eq!(first.since.as_deref(), Some("2026-09-23"));
    assert_eq!(first.first_seen.as_deref(), Some("2026-09-23"));
    assert_eq!(first.age_days, 2);
    assert!(!first.stale);
    assert_eq!(first.done_on, None);
    assert_eq!(first.agent, None);
    assert_eq!(first.copies.len(), 1);
    assert_eq!(first.copies[0].line, 85);
    assert!(first.copies[0].file.ends_with("WORKSPACE-STATUS.md"));
    assert!(first.copies[0].row.starts_with("| Chrome |"));

    let stale = &list.tasks[2];
    assert!(stale.stale && stale.age_days == 9);
    let agent = list.tasks[5].agent.as_ref().expect("task 6 has an agent");
    assert_eq!(
        (agent.pane.as_str(), agent.name.as_str(), agent.minutes),
        ("w3:p1", "oakwood-ws-lead", Some(4))
    );
    let done = &list.tasks[12];
    assert_eq!(done.group, TaskGroup::Done);
    assert_eq!(done.done_on.as_deref(), Some("2026-09-25"));
    assert_eq!(done.copies.len(), 2, "a merged task keeps every copy");

    assert_eq!(list.work.len(), 2);
    let card = &list.work[1];
    assert_eq!(card.kind, WorkKind::Feature);
    assert_eq!(card.stage, WorkStage::Build);
    assert_eq!(card.title, "Task board");
    assert_eq!(card.status, "open");
    assert_eq!(card.branch.as_deref(), Some("feat/task-board"));
    assert_eq!(card.commits, 1);
    assert_eq!(card.steps.len(), 2);
    assert!(card.steps[0].done && !card.steps[1].done);
    assert_eq!(card.shipped_on, None);
    assert_eq!(
        card.agent.as_ref().map(|agent| agent.name.as_str()),
        Some("herdr-w1")
    );
    assert_eq!(list.problems.len(), 1);
}

#[test]
fn task_list_ignores_unknown_fields_and_defaults_missing_ones() {
    let list = parse_task_list(
        r#"{"version": 1, "extra": {"a": 1},
            "tasks": [{"n": 4, "task": "t", "group": "you", "new_field": true}]}"#,
    )
    .expect("unknown fields are ignored");
    assert_eq!(list.tasks.len(), 1);
    let task = &list.tasks[0];
    assert_eq!((task.n, task.task.as_str()), (4, "t"));
    assert_eq!(task.space, "");
    assert_eq!(task.copies.len(), 0);
    assert!(list.work.is_empty() && list.problems.is_empty());
}

#[test]
fn task_list_keeps_a_task_of_a_group_or_stage_it_does_not_know() {
    let list = parse_task_list(
        r#"{"version": 1,
            "tasks": [{"n": 1, "task": "t", "group": "someday"}],
            "work": [{"title": "w", "kind": "spike", "stage": "parked"}, {"title": "x"}]}"#,
    )
    .expect("new enum values do not reject the list");
    assert_eq!(list.tasks[0].group, TaskGroup::Unknown);
    assert_eq!(list.work[0].kind, WorkKind::Unknown);
    assert_eq!(list.work[0].stage, WorkStage::Unknown);
    assert_eq!(
        list.work[1].kind,
        WorkKind::Feature,
        "missing means feature"
    );
}

#[test]
fn task_list_reads_the_stage_verify_as_check() {
    let list = parse_task_list(
        r#"{"version": 1,
            "work": [{"title": "v", "stage": "verify"}, {"title": "c", "stage": "check"}]}"#,
    )
    .expect("a verify card parses");
    assert_eq!(
        list.work[0].stage,
        WorkStage::Check,
        "verify lands in CHECK"
    );
    assert_eq!(list.work[1].stage, WorkStage::Check, "check still does");
}

#[test]
fn task_list_rejects_another_version_and_bad_json() {
    let error = parse_task_list(r#"{"version": 2, "tasks": []}"#).expect_err("version 2");
    assert!(error.contains("version 2"), "{error}");
    let error = parse_task_list(r#"{"tasks": []}"#).expect_err("no version");
    assert!(error.contains("version"), "{error}");
    assert!(parse_task_list("not json").is_err());
}

#[test]
fn task_board_store_keeps_the_last_list_its_time_and_the_last_error() {
    let mut store = TaskBoardStore::default();
    assert!(store.list().is_none() && store.error().is_none() && store.read_at().is_none());

    let at = std::time::Instant::now();
    store.receive_json(LIST_V1, at);
    assert_eq!(store.list().map(|list| list.tasks.len()), Some(13));
    assert_eq!(store.read_at(), Some(at));
    assert_eq!(store.error(), None);

    let later = at + std::time::Duration::from_secs(2);
    store.receive_json("not json", later);
    assert_eq!(
        store.list().map(|list| list.tasks.len()),
        Some(13),
        "a failed read keeps the last list"
    );
    assert_eq!(store.read_at(), Some(at), "the time is the last list's");
    assert!(store
        .error()
        .is_some_and(|error| error.contains("unreadable")));

    store.receive(
        parse_task_list(r#"{"version": 1, "tasks": [{"n": 1, "task": "t", "group": "claude"}]}"#),
        later,
    );
    assert_eq!(store.error(), None, "a good read clears the error");
}

#[test]
fn task_board_store_shows_an_error_before_any_list() {
    let mut store = TaskBoardStore::default();
    store.receive(Err("tasks_unavailable".into()), std::time::Instant::now());
    assert!(store.list().is_none());
    assert_eq!(store.error(), Some("tasks_unavailable"));
}

#[test]
fn task_list_reads_an_agent_without_minutes() {
    let list = parse_task_list(
        r#"{"version": 1, "tasks": [{"n": 1, "task": "t", "group": "claude",
            "agent": {"pane": "w1:p1", "name": "w1", "minutes": null}}]}"#,
    )
    .expect("minutes may be null");
    let agent = list.tasks[0].agent.as_ref().expect("agent");
    assert_eq!((agent.name.as_str(), agent.minutes), ("w1", None));
}

const LIST_DUE: &str = include_str!("../../../../tests/fixtures/task-board/list-v1-due.json");

#[test]
fn task_list_reads_the_due_age_start_and_action_fields() {
    let list = parse_task_list(LIST_DUE).expect("the due fixture is contract v1");
    let today = &list.tasks[0];
    assert_eq!(today.due.as_deref(), Some("2026-09-28"));
    assert_eq!(today.due_text.as_deref(), Some("Today"));
    assert_eq!(today.due_level, Some(DueLevel::Today));
    assert_eq!(today.age_text.as_deref(), Some("new"));
    assert_eq!(today.starts_text, None);
    assert_eq!(today.action.as_deref(), Some("Keep off or turn on"));
    assert_eq!(list.tasks[1].due_level, Some(DueLevel::Soon));
    assert_eq!(
        list.tasks[3].starts_text.as_deref(),
        Some("Starts tomorrow")
    );
    assert_eq!(list.tasks[5].due_level, Some(DueLevel::Overdue));
    let done = &list.tasks[6];
    assert_eq!(
        (done.due_text.as_deref(), done.age_text.as_deref()),
        (Some("Today"), Some(""))
    );
    // A level a newer reader added reads as none, as null does.
    let list = parse_task_list(
        r#"{"version": 1, "tasks": [{"n": 1, "task": "t", "group": "you", "due_level": "someday"},
            {"n": 2, "task": "u", "group": "you", "due_level": "later"}]}"#,
    )
    .expect("an unknown level is not an error");
    assert_eq!(list.tasks[0].due_level, None);
    assert_eq!(list.tasks[1].due_level, Some(DueLevel::Later));
}

#[test]
fn task_list_from_an_older_reader_has_none_of_the_due_fields() {
    let list = parse_task_list(LIST_V1).expect("the fixture is contract v1");
    for task in &list.tasks {
        assert_eq!(
            (
                &task.due,
                &task.due_text,
                task.due_level,
                &task.age_text,
                &task.starts_text,
                &task.action
            ),
            (&None, &None, None, &None, &None, &None),
            "{}",
            task.task
        );
    }
}

/// A newer reader's list, shaped as `claude-tasks list --json` prints it: every section, an
/// overdue, a blocking and a scheduled task, owners and a card. The tasks are made up.
const LIST_SECTIONS: &str =
    include_str!("../../../../tests/fixtures/task-board/list-v1-sections.json");

#[test]
fn task_list_reads_the_section_owner_blocking_start_and_card_fields() {
    use TaskSection::{ActionRequired, Completed, InProgress, Queued, Waiting};
    let list = parse_task_list(LIST_SECTIONS).expect("the sections fixture is contract v1");
    let sections: Vec<_> = list.tasks.iter().map(|task| task.section).collect();
    let want = [
        InProgress,
        ActionRequired,
        ActionRequired,
        ActionRequired,
        ActionRequired,
        ActionRequired,
        Queued,
        Queued,
        Queued,
        Waiting,
        Waiting,
        Completed,
        Completed,
    ];
    assert_eq!(sections, want.map(Some));
    let working = &list.tasks[0];
    assert_eq!(working.owner.as_deref(), Some("herdr-w1"));
    assert_eq!(working.card.as_deref(), Some("2026-09-28-one-board"));
    assert_eq!(
        list.tasks[2].blocking.as_deref(),
        Some("Chrome pairing round")
    );
    let scheduled = &list.tasks[5];
    assert_eq!(
        (scheduled.starts.as_deref(), scheduled.scheduled),
        (Some("2026-10-01"), true)
    );
    assert!(!list.tasks[1].scheduled);
    assert_eq!(list.tasks[1].blocking, None);
    // The reader's section is the one the task shows in.
    let shown: Vec<_> = list.tasks.iter().map(|task| task.section()).collect();
    assert_eq!(shown, want);
}

#[test]
fn an_older_readers_task_takes_its_section_from_its_group_and_a_working_agent() {
    use TaskSection::{ActionRequired, Completed, InProgress, Queued, Waiting};
    let list = parse_task_list(LIST_V1).expect("the fixture is contract v1");
    let shown: Vec<_> = list
        .tasks
        .iter()
        .map(|task| (task.section, task.section()))
        .collect();
    // No `section` field: `you` needs action, `claude` is queued unless an agent works on it,
    // `waiting` waits on others and `done` is completed.
    let want = [
        ActionRequired,
        ActionRequired,
        ActionRequired,
        ActionRequired,
        ActionRequired,
        InProgress,
        InProgress,
        Queued,
        Queued,
        Waiting,
        Waiting,
        Completed,
        Completed,
    ];
    assert_eq!(shown, want.map(|section| (None, section)));
    // Without its agent a task in progress goes back to its group's section.
    assert_eq!(list.tasks[5].group_section(), Queued);
    // A section a newer reader added reads as none, so the task falls back the same way; a
    // group it does not know waits on others, as before.
    let list = parse_task_list(
        r#"{"version": 1, "tasks": [
            {"task": "t", "group": "you", "section": "someday"},
            {"task": "u", "group": "someday", "section": "later"},
            {"task": "v", "group": "done", "section": null, "agent": {"pane": "p", "name": "w"}}]}"#,
    )
    .expect("an unknown section is not an error");
    let shown: Vec<_> = list
        .tasks
        .iter()
        .map(|task| (task.section, task.section()))
        .collect();
    assert_eq!(
        shown,
        [(None, ActionRequired), (None, Waiting), (None, Completed)]
    );
}

#[test]
fn task_rows_draw_the_readers_action_and_age_text_and_an_older_readers_state_and_days() {
    let mut board = TaskBoard::default();
    board
        .store
        .receive_json(LIST_DUE, std::time::Instant::now());
    board.open = true;
    let render = render_board(&board, WIDE, 40);
    let row = &render.rows[render.row_with("VS Code Workspace Trust")];
    assert!(row.contains("Keep off or turn on"), "{row:?}");
    assert!(row.contains("new"), "{row:?}");
    let row = &render.rows[render.row_with("RFC: security answers")];
    assert!(row.contains("12d"), "{row:?}");
    // An older reader's list still draws, from its state and its days.
    let render = render_board(&fixture_board(), WIDE, 60);
    let row = &render.rows[render.row_with("Per-client Chrome")];
    assert!(
        row.contains("go on the pairing broadcast") && row.contains("2d"),
        "{row:?}"
    );
}

/// The due fixture's board with each `(from, to)` swapped in its JSON.
fn due_board_swapped(swaps: &[(&str, &str)]) -> TaskBoard {
    let list = swaps.iter().fold(LIST_DUE.to_owned(), |list, (from, to)| {
        list.replace(from, to)
    });
    let mut board = TaskBoard::default();
    board.store.receive_json(&list, std::time::Instant::now());
    board.open = true;
    board
}

#[test]
fn an_action_that_only_says_needs_you_or_is_only_spaces_draws_blank_not_a_dash() {
    let mut board = due_board_swapped(&[
        (
            r#""action": "Sign in to Oakwood""#,
            r#""action": "needs you""#,
        ),
        (r#""action": "Keep off or turn on""#, r#""action": "   ""#),
    ]);
    let render = render_board(&board, WIDE, 40);
    for task in ["Fern: next revenue refresh", "VS Code Workspace Trust"] {
        let row = &render.rows[render.row_with(task)];
        assert!(!split_cells(row).iter().any(|cell| cell == "-"), "{row:?}");
    }
    // Open, its detail leaves the blank action out, and the due day its row shows whole.
    let list = parse_task_list(LIST_DUE).expect("fixture");
    board.selected = Some(RowKey::Task(TaskKey::of(&list.tasks[0])));
    board.open_row = board.selected.clone();
    let render = render_board(&board, WIDE, 40);
    let row = render.row_with("VS Code Workspace Trust");
    let detail = &render.rows[row + 1..row + 3];
    assert!(
        !detail.iter().any(|line| line.contains("Due")),
        "{detail:#?}"
    );
    assert!(
        !detail.iter().any(|line| line.contains("Action")),
        "{detail:#?}"
    );
}

/// Each cell's width on `row` between its lines, in screen columns.
fn cell_widths(row: &str) -> Vec<usize> {
    let mut widths = Vec::new();
    let mut width = None;
    for ch in row.chars() {
        if ch == '│' {
            widths.extend(width);
            width = Some(0);
        } else if let Some(width) = &mut width {
            *width += UnicodeWidthStr::width(ch.encode_utf8(&mut [0; 4]) as &str);
        }
    }
    widths
}

#[test]
fn the_project_column_is_as_wide_as_its_longest_name_up_to_24_and_the_task_gives_way() {
    let long_task = "A long task name the board cuts before it cuts the project";
    let board = |project: &str| {
        due_board_swapped(&[
            (
                r#""project": "vscode""#,
                &format!(r#""project": "{project}""#),
            ),
            (
                r#""task": "VS Code Workspace Trust""#,
                &format!(r#""task": "{long_task}""#),
            ),
        ])
    };
    let project_cell = |project: &str, width: u16| {
        let render = render_board(&board(project), width, 40);
        let row = render.rows[render.row_with("A long task")].clone();
        (split_cells(&row)[1].clone(), cell_widths(&row), row)
    };
    let (shown, _, row) = project_cell("oakwood-workspace", WIDE);
    assert_eq!(shown, "oakwood-workspace", "{row:?}");
    assert!(!row.contains(long_task), "the task is cut instead: {row:?}");

    let exactly = "abcdefghijklmnopqrstuvwx";
    assert_eq!(exactly.len(), 24);
    let (shown, _, row) = project_cell(exactly, 160);
    assert_eq!(shown, exactly, "{row:?}");
    let over = "abcdefghijklmnopqrstuvwxy";
    let (shown, _, row) = project_cell(over, 160);
    assert_eq!(
        (UnicodeWidthStr::width(shown.as_str()), shown.ends_with('…')),
        (24, true),
        "{row:?}"
    );

    // Narrow, beside WAITING's owner and blocking columns, the project gives way before the
    // task drops below its least.
    let mut board = TaskBoard::default();
    let list = LIST_SECTIONS.replace(
        r#""project": "oak-cli""#,
        &format!(r#""project": "{over}""#),
    );
    board.store.receive_json(&list, std::time::Instant::now());
    board.open = true;
    let render = render_board(&board, 80, 40);
    let row = &render.rows[render.row_with("Ship the oak")];
    let widths = cell_widths(row);
    assert!(widths[2] >= 16, "{widths:?} {row:?}");
    assert!(split_cells(row)[1].ends_with('…'), "{row:?}");
}

/// What an agent or task at work draws in, as the board plan's one meaning per colour sets it.
fn working_colour(palette: &Palette) -> ratatui::style::Color {
    palette.yellow
}

/// Whether the fill `fill` leans to `to` more than to `other`, by RGB distance.
fn leans_to(
    fill: ratatui::style::Color,
    to: ratatui::style::Color,
    other: ratatui::style::Color,
) -> bool {
    use ratatui::style::Color::Rgb;
    let distance = |a, b| match (a, b) {
        (Rgb(r1, g1, b1), Rgb(r2, g2, b2)) => [(r1, r2), (g1, g2), (b1, b2)]
            .into_iter()
            .map(|(x, y)| (i32::from(x) - i32::from(y)).pow(2))
            .sum::<i32>(),
        _ => i32::MAX,
    };
    distance(fill, to) < distance(fill, other)
}

#[test]
fn the_tasks_views_notice_and_filter_read_in_text_its_matches_accent_and_its_read_errors_red() {
    let mut board = fixture_board();
    board.notice = Some("Select a task first.".into());
    board.filter = "Chrome".into();
    let render = render_board(&board, WIDE, 40);
    let palette = &render.palette;
    let notice = render.row_with("Select a task first.");
    assert_eq!(render.cell(notice, "Select").fg, palette.text);
    let clear = render.hits.filter_clear;
    let clear = &render.buffer[(clear.x, clear.y)];
    assert_eq!(clear.fg, palette.text, "{clear:?}");
    assert!(
        leans_to(clear.bg, palette.accent, palette.yellow),
        "{clear:?}"
    );
    let hit = render.cell(render.row_with("Per-client Chrome"), "Chrome");
    assert!(leans_to(hit.bg, palette.accent, palette.yellow), "{hit:?}");
    let problems = render.row_with("1 notes file has a problem");
    assert_eq!(render.cell(problems, "notes file").fg, palette.red);

    let mut board = fixture_board();
    board
        .store
        .receive_json("not json", std::time::Instant::now());
    let render = render_board(&board, WIDE, 40);
    let failed = render.row_with("Last read failed: ");
    assert_eq!(render.cell(failed, "Last read").fg, render.palette.red);
    assert_eq!(render.cell(failed, "unreadable").fg, render.palette.text);
}

/// The foot's notice draws clean and inside the board: no control character, no line past
/// its margin, and a text longer than three lines ends in `…`.
#[test]
fn the_foots_lines_are_clean_fitted_and_cut_with_an_ellipsis() {
    let mut board = fixture_board();
    let long = "x".repeat(400);
    board.notice = Some(format!("Moved\u{1b}[31m\ttask\u{7} to Done {long}"));
    let width = 80;
    let render = render_board(&board, width, 40);
    let notice = render.row_with("Moved");
    assert!(
        render.rows[notice].contains("Moved[31mtask to Done"),
        "cleaned: {:?}",
        render.rows[notice]
    );
    let lines = &render.rows[notice..notice + 3];
    assert!(lines[2].trim_end().ends_with('…'), "{lines:#?}");
    for text in lines {
        assert!(!text.chars().any(char::is_control), "{text:?}");
        // The margin column before the right edge stays blank.
        let drawn = text.trim_end().chars().count();
        assert!(drawn < usize::from(width), "past the margin: {text:?}");
    }
}

#[test]
fn the_tasks_views_acting_line_is_blue_and_its_project_and_agent_menu_doing_read_as_text() {
    let mut board = fixture_board();
    board.acting = Some(crate::client::shell::task_board::Acting {
        request_id: "client-shell-tasks-act:1".into(),
        task: task_key(1),
        label: "done".into(),
    });
    let render = render_board(&board, WIDE, 40);
    let acting = render.row_with("◐ done…");
    assert_eq!(
        render.cell(acting, "◐ done…").fg,
        working_colour(&render.palette)
    );
    let row = render.row_with("Per-client Chrome");
    let project = split_cells(&render.rows[row])[1].clone();
    assert_eq!(render.cell(row, &project).fg, render.palette.overlay1);

    // Claude's own action in ACTION REQUIRED is read too.
    let mut board = TaskBoard::default();
    let list = LIST_SECTIONS.replace(
        "\"task\": \"Ship the oak CLI login fix\",\n   \"group\": \"you\"",
        "\"task\": \"Ship the oak CLI login fix\",\n   \"group\": \"claude\"",
    );
    assert_ne!(list, LIST_SECTIONS);
    board.store.receive_json(&list, std::time::Instant::now());
    board.open = true;
    let render = render_board(&board, WIDE, 40);
    let row = render.row_with("Ship the oak");
    assert_eq!(render.cell(row, "say go on").fg, render.palette.text);

    let mut board = tasks_and_work_board();
    board.space = Some("herdr".into());
    board.agent_menu = true;
    let render = render_board_with_agents(&board, 186, 46, &lead_and_workers());
    let worker = usize::from(render.hits.agent_items[2].0.y);
    assert_eq!(render.cell(worker, "Round 13a").fg, render.palette.text);
}

/// The due fixture's board, DONE open, sorted by due as it was written for.
fn due_board() -> TaskBoard {
    let mut board = TaskBoard::default();
    board
        .store
        .receive_json(LIST_DUE, std::time::Instant::now());
    board.open = true;
    board.done_expanded = true;
    board.sort = crate::client::shell::task_board::TaskSort::Due;
    board
}

#[test]
fn search_finds_a_task_by_its_due_and_its_next_action() {
    let mut board = due_board();
    for (words, kept, dropped) in [
        ("overdue", "Sundial: Sheet12 file", "RFC: security answers"),
        ("tomorrow", "Judge design lens", "Editors follow the theme"),
        ("morgan", "RFC: security answers", "Sundial: Sheet12 file"),
    ] {
        board.filter = words.into();
        let render = render_board(&board, WIDE, 60);
        assert!(
            render.rows.iter().any(|row| row.contains(kept)),
            "{words}: {:#?}",
            render.rows
        );
        assert!(
            !render.rows.iter().any(|row| row.contains(dropped)),
            "{words}: {:#?}",
            render.rows
        );
    }
}

#[test]
fn every_sections_rows_line_up_under_its_own_header_row_and_show_its_cells() {
    let render = render_board(&due_board(), WIDE, 60);
    // Each box's column-name row sits under its top rule, and its rows share its lines.
    let sections = [
        (
            "  ACTION REQUIRED",
            &["#", "Project", "Task", "Action", "Due", "Age"][..],
            &["VS Code Workspace Trust", "Fern: next revenue"][..],
        ),
        (
            "  QUEUED",
            &["#", "Project", "Task", "Next step", "Due", "Age"][..],
            &["Editors follow the theme", "Judge design lens"][..],
        ),
        (
            "  WAITING ON OTHERS",
            &["#", "Project", "Task", "Owner", "Requested", "Waiting"][..],
            &["RFC: security answers", "Sundial: Sheet12 file"][..],
        ),
        (
            "  COMPLETED 1 ▼",
            &["#", "Project", "Task", "Completed"][..],
            &["Theme generators in repo"][..],
        ),
    ];
    for (title, names, tasks) in sections {
        let head = render.row_with(title) + 2;
        assert_eq!(
            split_cells(&render.rows[head]),
            names,
            "{title}: {:#?}",
            render.rows
        );
        for task in tasks {
            let row = &render.rows[render.row_with(task)];
            assert_eq!(
                bar_columns(row),
                bar_columns(&render.rows[head]),
                "{row:?}\n{:?}",
                render.rows[head]
            );
        }
    }
    let cells = |task: &str| -> Vec<String> {
        render.rows[render.row_with(task)]
            .split('│')
            .map(|cell| cell.trim().to_owned())
            .collect()
    };
    // The project in its own cell, before the task.
    assert_eq!(
        cells("VS Code")[2..4],
        ["vscode", "VS Code Workspace Trust"]
    );
    assert_eq!(
        cells("VS Code")[4..7],
        ["? Keep off or turn on", "Today", "new"]
    );
    // Waiting on others: who, what they were asked, and how long it has waited.
    assert_eq!(cells("Sundial")[4..7], ["… Carmen", "reply to both", "3d"]);
    // Not started yet: its next step, and when it starts under Due.
    assert_eq!(
        cells("Judge design lens")[4..7],
        ["write the lens", "Starts tomorrow", "3d"]
    );
    // Done: the day it was done, and no age: nothing after it but the border.
    assert_eq!(cells("Theme generators")[4..], ["✓ Today", ""]);
}

#[test]
fn an_older_readers_done_task_shows_its_done_day_under_completed_and_no_age() {
    let mut board = fixture_board();
    board.done_expanded = true;
    let render = render_board(&board, WIDE, 60);
    let list = parse_task_list(LIST_V1).expect("fixture");
    let done = list
        .tasks
        .iter()
        .find(|task| task.group == TaskGroup::Done)
        .expect("a done task");
    let row = &render.rows[render.row_with(&done.task[..20])];
    let cells: Vec<&str> = row.split('│').map(str::trim).collect();
    let day = done.done_on.as_deref().expect("done day");
    let month_day = format!("Sep {}", day[8..].trim_start_matches('0'));
    // The day under Completed after its tick, the last cell: no age, and no `✓ done` beside
    // it.
    let ticked = format!("✓ {month_day}");
    assert_eq!(cells[4..], [ticked.as_str(), ""], "{row:?}");
    assert!(!row.contains("✓ done"), "{row:?}");
}

#[test]
fn the_small_grid_drops_age_first_and_the_stacked_list_puts_due_age_and_action_under_the_task() {
    let render = render_board(&due_board(), NARROW, 60);
    let head = &render.rows[render.row_with("│ Action")];
    assert!(head.contains("│ Due") && !head.contains("Age"), "{head:?}");
    let row = &render.rows[render.row_with("VS Code")];
    assert!(row.contains("│ Today"), "{row:?}");
    let render = render_board(&due_board(), 50, 60);
    let y = render.row_with("Sundial");
    assert!(
        render.rows[y + 1].contains("Overdue 3d · 3d · … Carmen: reply to both"),
        "{:?}",
        render.rows[y + 1]
    );
    let y = render.row_with("Theme generators");
    assert!(
        render.rows[y + 1].contains("Today · Pushed e17c30f"),
        "{:?}",
        render.rows[y + 1]
    );
}

#[test]
fn overdue_is_red_today_and_a_stale_age_bold_and_a_task_not_started_dim() {
    // WAITING ON OTHERS has no Due column, so the overdue task asks you instead.
    let json = LIST_DUE.replace(
        r#""group": "waiting", "state": "Carmen"#,
        r#""group": "you", "state": "Carmen"#,
    );
    let list = parse_task_list(&json).expect("the due fixture is contract v1");
    let mut board = due_board();
    board.store.receive_json(&json, std::time::Instant::now());
    let render = render_board(&board, WIDE, 60);
    let palette = &render.palette;
    let overdue = render.row_with("Sundial: Sheet12 file");
    assert_eq!(render.cell(overdue, "Overdue 3d").fg, palette.red);
    // Due today in text, not bold: the rest of Due is dim.
    let today = render.cell(render.row_with("VS Code Workspace Trust"), "Today");
    assert!(!today.modifier.contains(Modifier::BOLD));
    assert_eq!(today.fg, palette.text);
    let later = render.cell(render.row_with("Fern: next revenue"), "Thu");
    assert!(!later.modifier.contains(Modifier::BOLD));
    assert_ne!(later.fg, palette.red);
    let stale = render.row_with("RFC: security answers");
    assert_eq!(render.cell(stale, "12d").fg, palette.text);
    // Not started: every cell dim, its number too.
    let waits = render.row_with("Judge design lens");
    for text in ["Judge design lens", "Starts tomorrow", "3d"] {
        assert_eq!(render.cell(waits, text).fg, palette.overlay0, "{text}");
    }
    assert_eq!(render.buffer[(3, waits as u16)].fg, palette.overlay0, "#");
    // Done keeps its strike over its task, its day under Completed in the section's colour;
    // selected, a styled row keeps the band.
    let done = render.row_with("Theme generators in repo");
    assert!(render
        .cell(done, "Theme generators")
        .modifier
        .contains(Modifier::CROSSED_OUT));
    assert_eq!(render.cell(done, "✓ Today").fg, palette.green);
    let day = render.cell(done, "Today");
    assert_eq!(day.fg, palette.overlay1);
    assert!(!day.modifier.contains(Modifier::CROSSED_OUT));
    board.selected = Some(RowKey::Task(TaskKey::of(&list.tasks[5])));
    let render = render_board(&board, WIDE, 60);
    let overdue = render.cell(render.row_with("Sundial: Sheet12 file"), "Overdue 3d");
    assert_eq!(overdue.bg, render.palette.selection_bg);
    assert_eq!(overdue.fg, render.palette.red);
}

#[test]
fn task_rows_sort_by_due_by_default_overdue_then_nearest_due_then_oldest() {
    use crate::client::shell::task_board::TaskSort;
    let mut board = due_board();
    assert_eq!(board.sort, TaskSort::Due);
    let render = render_board(&board, WIDE, 60);
    assert!(
        render.rows[0].contains(" Sort: Due ▾ "),
        "{:?}",
        render.rows[0]
    );
    let before = |a: &str, b: &str| {
        assert!(
            render.row_with(a) < render.row_with(b),
            "{a} before {b}: {:#?}",
            render.rows
        );
    };
    // Overdue before an older task with no due.
    before("Sundial: Sheet12 file", "RFC: security answers");
    // With no due, oldest first: Judge started, so it is not held to the end as scheduled.
    board.store.receive_json(
        &LIST_DUE.replacen(
            r#""starts_text": "Starts tomorrow""#,
            r#""starts_text": null"#,
            1,
        ),
        std::time::Instant::now(),
    );
    let render = render_board(&board, WIDE, 60);
    assert!(
        render.row_with("Judge design lens") < render.row_with("Editors follow the theme"),
        "{:#?}",
        render.rows
    );
    // The nearest due first, whatever the file's order.
    let json = LIST_DUE
        .replacen(r#""due": "2026-09-28""#, r#""due": "2026-10-05""#, 1)
        .replacen(r#""due_level": "today""#, r#""due_level": "later""#, 1);
    board.store.receive_json(&json, std::time::Instant::now());
    let render = render_board(&board, WIDE, 60);
    assert!(render.row_with("Fern: next revenue") < render.row_with("VS Code Workspace Trust"));
    board.sort_menu = true;
    let render = render_board(&board, WIDE, 60);
    let first = render.row_with("By due date");
    assert!(
        render.rows[first].contains("By due date ✓"),
        "{:?}",
        render.rows[first]
    );
    assert!(render.rows[first + 1].contains("Newest first"));
}

/// The heading row that starts with `heading`, followed by nothing or a space.
fn heading_row<'a>(render: &'a BoardRender, heading: &str) -> &'a str {
    render
        .rows
        .iter()
        .find(|row| row.as_str() == heading || row.starts_with(&format!("{heading} ")))
        .unwrap_or_else(|| panic!("{heading:?} not drawn: {:#?}", render.rows))
}

#[test]
fn section_titles_count_their_rows_then_how_many_are_overdue_and_due_today() {
    let render = render_board(&due_board(), WIDE, 60);
    for (heading, detail) in [
        ("  ACTION REQUIRED 2", "1 due today"),
        ("  QUEUED 1", "+1 scheduled"),
        ("  WAITING ON OTHERS 2", "1 overdue"),
        ("  COMPLETED 1 ▼", "last 7 days"),
    ] {
        let row = heading_row(&render, heading);
        assert!(row.ends_with(&format!("  {detail}")), "{row:?}");
    }
    // Overdue before due today, joined by a dot; DONE only ever counts.
    let json = LIST_DUE.replacen(r#""due_level": "soon""#, r#""due_level": "overdue""#, 1);
    let mut board = due_board();
    board.store.receive_json(&json, std::time::Instant::now());
    let render = render_board(&board, WIDE, 60);
    let row = heading_row(&render, "  ACTION REQUIRED 2");
    assert!(row.ends_with("  1 overdue · 1 due today"), "{row:?}");
    board.done_expanded = false;
    let render = render_board(&board, WIDE, 60);
    assert!(heading_row(&render, "  COMPLETED 1 ▶").ends_with("  last 7 days"));
    // Counted as the rows are filtered: the search keeps one of two, the space one section.
    let mut board = due_board();
    board.filter = "rfc".into();
    let render = render_board(&board, WIDE, 60);
    assert_eq!(
        heading_row(&render, "  WAITING ON OTHERS 1 of 2"),
        "  WAITING ON OTHERS 1 of 2"
    );
    board.filter = "sundial".into();
    let render = render_board(&board, WIDE, 60);
    assert!(heading_row(&render, "  WAITING ON OTHERS 1 of 2").ends_with("  1 overdue"));
    let mut board = due_board();
    board.space = Some("dot-works".into());
    let render = render_board(&board, WIDE, 60);
    assert!(heading_row(&render, "  QUEUED 1").ends_with("  +1 scheduled"));
    // Nothing there needs Nadine: ACTION REQUIRED says so, and no other empty section shows.
    assert!(heading_row(&render, "  ACTION REQUIRED 0").ends_with("  nothing needs you"));
}

#[test]
fn a_heading_takes_its_colour_its_count_grey_and_its_grey_detail_ends_at_the_box_edge() {
    let json = LIST_DUE.replacen(r#""due_level": "soon""#, r#""due_level": "overdue""#, 1);
    let mut board = due_board();
    board.store.receive_json(&json, std::time::Instant::now());
    let render = render_board(&board, WIDE, 60);
    let y = render.row_with("  ACTION REQUIRED 2");
    let role = render.palette.yellow;
    let title = render.cell(y, "ACTION");
    assert_eq!((title.fg, title.bg), (role, ratatui::style::Color::Reset));
    assert!(title.modifier.contains(Modifier::BOLD));
    // The count: a plain grey number one space after the title, no fill.
    let count = render.cell(y, "2");
    assert_eq!(
        (count.fg, count.bg),
        (render.palette.overlay1, ratatui::style::Color::Reset)
    );
    // The detail is grey, as the fold arrow is, and ends where the box below ends.
    assert_eq!(render.cell(y, "1 overdue").fg, render.palette.overlay0);
    assert_eq!(render.cell(y, "·").fg, render.palette.overlay0);
    let done = render.row_with("  COMPLETED 1 ▼");
    assert_eq!(render.cell(done, "▼").fg, render.palette.overlay0);
    let edge = |row: &str| UnicodeWidthStr::width(row);
    assert_eq!(edge(&render.rows[y]), edge(&render.rows[y + 1]));
    // Narrow, the grey detail drops before the title or the badge cut short.
    let render = render_board(&board, 40, 60);
    assert_eq!(
        heading_row(&render, "  ACTION REQUIRED 2"),
        "  ACTION REQUIRED 2"
    );
    // A heading with nothing more to count draws nothing at its right.
    let mut board = due_board();
    board.filter = "rfc".into();
    let render = render_board(&board, WIDE, 60);
    let y = render.row_with("  WAITING ON OTHERS 1 of 2");
    assert_eq!(render.rows[y].trim_end(), "  WAITING ON OTHERS 1 of 2");
}

/// The design's normal main-area width, and a narrow one.
const WIDE: u16 = 118;
const NARROW: u16 = 60;

/// The older reader's board, newest first: its list has no due dates, and its tests place
/// rows by age.
fn fixture_board() -> TaskBoard {
    let mut board = TaskBoard::default();
    board.store.receive_json(LIST_V1, std::time::Instant::now());
    board.open = true;
    board.can_act = true;
    board.sort = crate::client::shell::task_board::TaskSort::Newest;
    board
}

/// The older reader's list with task `list_n` in `section`, as a newer reader may send a
/// task an agent works on that it does not put in progress.
fn list_v1_in_section(list_n: usize, section: &str) -> String {
    LIST_V1.replacen(
        &format!("\"n\": {list_n},"),
        &format!("\"n\": {list_n}, \"section\": \"{section}\","),
        1,
    )
}

/// `fixture_board` with task `list_n` in `section`.
fn fixture_board_in_section(list_n: usize, section: &str) -> TaskBoard {
    let mut board = fixture_board();
    board.store.receive_json(
        &list_v1_in_section(list_n, section),
        std::time::Instant::now(),
    );
    board
}

fn key(list_n: usize) -> RowKey {
    RowKey::Task(task_key(list_n))
}

fn task_key(list_n: usize) -> TaskKey {
    let list = parse_task_list(LIST_V1).expect("fixture");
    TaskKey::of(&list.tasks[list_n - 1])
}

struct BoardRender {
    rows: Vec<String>,
    buffer: Buffer,
    hits: TaskBoardHits,
    palette: Palette,
}

impl BoardRender {
    fn row_with(&self, needle: &str) -> usize {
        self.rows
            .iter()
            .position(|row| row.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} not drawn: {:#?}", self.rows))
    }

    /// The screen column where `needle` starts on row `y`.
    fn column(&self, y: usize, needle: &str) -> u16 {
        let row = &self.rows[y];
        let byte = row
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} not on row {y}: {row:?}"));
        UnicodeWidthStr::width(&row[..byte]) as u16
    }

    /// The cell where `needle` starts on row `y`.
    fn cell(&self, y: usize, needle: &str) -> &ratatui::buffer::Cell {
        let row = &self.rows[y];
        let byte = row
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} not on row {y}: {row:?}"));
        let x = UnicodeWidthStr::width(&row[..byte]) as u16;
        &self.buffer[(x, y as u16)]
    }
}

/// The sub-agent spinner's glyph the test renders draw, and the lead's.
const WORKER_SPINNER: &str = "◜";
const LEAD_SPINNER: &str = "⠋";

fn render_board(board: &TaskBoard, width: u16, height: u16) -> BoardRender {
    render_board_with_agents(board, width, height, &[])
}

/// The board drawn with `agents` working or waiting on Nadine.
fn render_board_with_agents(
    board: &TaskBoard,
    width: u16,
    height: u16,
    agents: &[BoardAgent],
) -> BoardRender {
    render_board_on(board, width, height, agents, Palette::catppuccin())
}

/// The board drawn with `agents` in `palette`.
fn render_board_on(
    board: &TaskBoard,
    width: u16,
    height: u16,
    agents: &[BoardAgent],
    palette: Palette,
) -> BoardRender {
    let area = Rect::new(0, 0, width, height);
    let mut buffer = Buffer::empty(area);
    let hits = render_task_board(
        &mut buffer,
        area,
        board,
        &palette,
        Some("/home/user/devbox"),
        (agents, (WORKER_SPINNER, palette.yellow)),
    );
    let rows = (0..height)
        .map(|y| {
            let mut row = String::new();
            let mut x = 0;
            while x < width {
                let symbol = buffer[(x, y)].symbol();
                row.push_str(symbol);
                x += UnicodeWidthStr::width(symbol).max(1) as u16;
            }
            row.trim_end().to_owned()
        })
        .collect();
    BoardRender {
        rows,
        buffer,
        hits,
        palette,
    }
}

#[test]
fn task_board_header_counts_every_space_and_ends_with_its_close() {
    let render = render_board(&fixture_board(), HEADER_WIDE, 40);
    let header = &render.rows[0];
    for part in [" Tasks ", " Phases ", "Space: All ▾"] {
        assert!(header.contains(part), "{part:?} in {header:?}");
    }
    assert!(header.ends_with('×'), "{header:?}");
    assert_eq!(render.hits.close, Rect::new(HEADER_WIDE - 4, 0, 3, 1));
    // The counts sit on the chip row, on row 2.
    let chips = &render.rows[2];
    for part in [
        "! Action required 5",
        "◐ In progress 0",
        "… Waiting on others 2",
    ] {
        assert!(chips.contains(part), "{part:?} in {chips:?}");
    }
    // Its glyph is in the section's colour, then its words and its count grey.
    assert_eq!(render.cell(2, "!").fg, render.palette.yellow);
    let count = render.column(2, "! Action required 5") + "! Action required ".len() as u16;
    assert_eq!(render.buffer[(count, 2)].symbol(), "5");
    assert_eq!(render.buffer[(count, 2)].fg, render.palette.overlay1);
    assert_eq!(render.cell(2, "Action").fg, render.palette.overlay1);
    assert_eq!(render.cell(2, "◐").fg, working_colour(&render.palette));
}

/// The sections fixture's board, every section shown.
fn sections_board() -> TaskBoard {
    let mut board = TaskBoard::default();
    board
        .store
        .receive_json(LIST_SECTIONS, std::time::Instant::now());
    board.open = true;
    board.can_act = true;
    board.can_open = true;
    board.done_expanded = true;
    board
}

/// The sections fixture's worker at work on its task.
fn sections_worker() -> BoardAgent {
    working("w1:p1", "herdr", "herdr-w1", Some("build"))
}

/// Where each of `titles` is drawn, asserting they come in that order.
fn in_order(render: &BoardRender, titles: &[&str]) -> Vec<usize> {
    let rows: Vec<usize> = titles.iter().map(|title| render.row_with(title)).collect();
    assert!(rows.windows(2).all(|pair| pair[0] < pair[1]), "{rows:?}");
    rows
}

#[test]
fn the_sections_are_in_progress_action_required_queued_waiting_and_completed() {
    let agents = [sections_worker()];
    let render = render_board_with_agents(&sections_board(), 186, 80, &agents);
    let rows = in_order(
        &render,
        &[
            " IN PROGRESS 1",
            " ACTION REQUIRED 4",
            " QUEUED 2",
            " WAITING ON OTHERS 2",
            " COMPLETED 2",
        ],
    );
    // The task its agent works on shows once, in the agent's IN PROGRESS row.
    let task_rows: Vec<usize> = (0..render.rows.len())
        .filter(|row| render.rows[*row].contains("Board design B1"))
        .collect();
    assert_eq!(task_rows.len(), 1, "{:#?}", render.rows);
    assert!(rows[0] < task_rows[0] && task_rows[0] < rows[1]);
    assert!(render.rows[task_rows[0]].contains("herdr-w1"));
    assert_eq!(
        render.cell(rows[1], "ACTION REQUIRED").fg,
        render.palette.yellow
    );
    assert_eq!(render.cell(rows[2], "QUEUED").fg, render.palette.teal);
    assert_eq!(render.cell(rows[4], "COMPLETED").fg, render.palette.green);
    // With its agent not at work the task goes back to its group's section, QUEUED, and no
    // IN PROGRESS shows.
    let render = render_board(&sections_board(), 186, 80);
    assert!(
        render.rows.iter().all(|row| !row.contains("IN PROGRESS")),
        "{:#?}",
        render.rows
    );
    let rows = in_order(&render, &[" QUEUED 3", " WAITING ON OTHERS 2"]);
    let at = render.row_with("Board design B1");
    assert!(rows[0] < at && at < rows[1]);
    // An older reader's list, with no section, draws in the same sections from its groups.
    let render = render_board(&fixture_board(), WIDE, 60);
    in_order(
        &render,
        &[
            " ACTION REQUIRED 5",
            " QUEUED 4",
            " WAITING ON OTHERS 2",
            " COMPLETED 2",
        ],
    );
}

/// The sections fixture's row for its task `list_n`.
fn sections_key(list_n: usize) -> RowKey {
    let list = parse_task_list(LIST_SECTIONS).expect("the sections fixture is contract v1");
    RowKey::Task(TaskKey::of(&list.tasks[list_n - 1]))
}

/// Each `label  value` field of an open row's block, its value to the box's right edge.
fn block_fields(lines: &[String]) -> Vec<(String, String)> {
    lines
        .iter()
        .filter_map(|line| {
            let inner = line.trim_start_matches(" │").trim_end_matches('│').trim();
            let (label, value) = inner.split_once("  ")?;
            Some((label.to_owned(), value.trim().to_owned()))
        })
        .collect()
}

#[test]
fn an_open_row_shows_only_what_helps_act_on_it_in_its_section() {
    // Task 10 waits on Carmen and holds up another task.
    let list = LIST_SECTIONS.replacen(
        "\"owner\": \"Carmen\",\n   \"section\": \"waiting\",\n   \"blocking\": null",
        "\"owner\": \"Carmen\",\n   \"section\": \"waiting\",\n   \"blocking\": \"Causeway go-live\"",
        1,
    );
    assert_ne!(list, LIST_SECTIONS, "task 10 blocks");
    let agents = [
        sections_worker(),
        resting("w2:p1", "oakwood", "oakwood-ws-lead"),
    ];
    // Every section's row shows its action, next step, owner, request, due day and what it
    // blocks whole, so the block holds only its file, then its keys and `← close`; with no
    // agent working for another here, no lead.
    let keys = |act: bool| {
        let done = if act { "     d  done" } else { "" };
        format!("go to space     o  open file{done}     ←  close")
    };
    let cases = [(3, true), (2, true), (8, true), (10, true), (13, false)];
    for (n, act) in cases {
        let mut board = sections_board();
        board.store.receive_json(&list, std::time::Instant::now());
        board.selected = Some(sections_key(n));
        board.open_row = board.selected.clone();
        let render = render_board_with_agents(&board, 186, 80, &agents);
        let lines = lines_of(&render, &sections_key(n));
        let block = block_fields(&lines[1..]);
        let labels: Vec<&str> = block.iter().map(|(label, _)| label.as_str()).collect();
        assert_eq!(labels, ["File", "Enter"], "task {n}: {lines:#?}");
        assert_eq!(block[1].1, keys(act), "task {n}");
        // The footer leaves the row's keys to the block.
        let footer = &render.rows[render.rows.len() - 1];
        assert!(!footer.contains("←  close"), "task {n}: {footer:?}");
    }
    // Its worker at rest, task 1 shows in QUEUED, and its Enter goes to that agent.
    let mut board = sections_board();
    board.selected = Some(sections_key(1));
    board.open_row = board.selected.clone();
    let agents = [resting("w1:p1", "herdr", "herdr-w1")];
    let render = render_board_with_agents(&board, 186, 80, &agents);
    let keys = &render.rows[render.row_with("←  close")];
    assert!(keys.contains("Enter  go to agent"), "{keys:?}");
}

// The toolbar counts what Nadine reads first: tasks he must act on, agents at work and tasks
// waiting on others, each in its section's colour and a dim dot apart; the tabs read Tasks
// and Phases.
/// Each toolbar chip's drawn text from its click area's first column to its last.
fn chip_texts(render: &BoardRender) -> Vec<String> {
    render
        .hits
        .categories
        .iter()
        .map(|(area, _)| {
            (area.x..area.right())
                .map(|x| render.buffer[(x, area.y)].symbol().to_string())
                .collect()
        })
        .collect()
}

#[test]
fn toolbar_chips_name_their_section_in_full_then_fall_back_to_icon_and_count() {
    let agents = [sections_worker()];
    let wide = render_board_with_agents(&sections_board(), 186, 80, &agents);
    // Its icon, its words, then its count one space on; the click area is exactly what is
    // drawn, fill to fill.
    assert_eq!(
        chip_texts(&wide),
        [
            " ◐ In progress 1 ",
            " ! Action required 4 ",
            " ◌ Queued 2 ",
            " … Waiting on others 2 "
        ]
    );
    // Only its glyph takes the section's colour; the words stay grey.
    let palette = &wide.palette;
    for (glyph, words, role) in [
        ("!", "Action required", palette.yellow),
        ("◐", "In progress", working_colour(palette)),
        ("…", "Waiting on others", palette.mauve),
    ] {
        assert_eq!(wide.cell(2, glyph).fg, role, "{glyph}");
        assert_eq!(wide.cell(2, words).fg, palette.overlay1, "{words}");
    }
    // The chips no longer share the top bar, so they stay whole down to a narrow board.
    for width in [150, 118, 90] {
        let render = render_board_with_agents(&sections_board(), width, 80, &agents);
        assert_eq!(chip_texts(&render), chip_texts(&wide), "{width}");
    }
    // Narrow, each falls back to its icon and count; the click areas follow.
    for width in [70, 60, 50] {
        let render = render_board_with_agents(&sections_board(), width, 80, &agents);
        let texts = chip_texts(&render);
        assert!(
            texts.iter().all(|text| !text.contains("Action required")),
            "{width}: {texts:?}"
        );
        let full = [" ◐ 1 ", " ! 4 ", " ◌ 2 ", " … 2 "];
        assert_eq!(texts, full[..texts.len()], "{width}");
    }
    // Filtering keeps `n of m`, in either form.
    let mut board = sections_board();
    board.filter = "rfc".into();
    let render = render_board_with_agents(&board, 186, 80, &agents);
    assert!(
        chip_texts(&render)[3].starts_with(" … Waiting on others ")
            && chip_texts(&render)[3].ends_with(" of 2 "),
        "{:?}",
        chip_texts(&render)
    );
}

#[test]
fn the_toolbar_counts_action_in_progress_and_waiting_and_the_tabs_read_tasks_and_phases() {
    let agents = [sections_worker()];
    let render = render_board_with_agents(&sections_board(), 186, 80, &agents);
    let header = &render.rows[0];
    let chips = &render.rows[2];
    // Scheduled rows are not counted, as the section titles count them apart.
    assert!(
        header.starts_with("  Tasks   Phases  │  ")
            && chips.contains(
                "◐ In progress 1   ! Action required 4   ◌ Queued 2   … Waiting on others 2"
            ),
        "{header:?} {chips:?}"
    );
    assert!(!header.contains("Work"), "{header:?}");
    let title_colour = |title: &str| {
        render
            .cell(render.row_with(&format!(" {title} ")), title)
            .fg
    };
    // A chip's glyph reads its section's colour; its words stay grey.
    for (glyph, title) in [
        ("!", "ACTION REQUIRED"),
        ("◐", "IN PROGRESS"),
        ("…", "WAITING ON OTHERS"),
    ] {
        assert_eq!(render.cell(2, glyph).fg, title_colour(title), "{glyph}");
    }
    assert!(!chips.contains('·'), "no dot between the buttons");
    let buttons: Vec<Category> = render.hits.categories.iter().map(|(_, c)| *c).collect();
    assert_eq!(
        buttons,
        [
            Category::Section(TaskSection::InProgress),
            Category::Section(TaskSection::ActionRequired),
            Category::Section(TaskSection::Queued),
            Category::Section(TaskSection::Waiting),
        ]
    );
    // In progress shown alone: only the working agents' rows.
    let mut board = sections_board();
    board.only_section = Some(TaskSection::InProgress);
    let render = render_board_with_agents(&board, 186, 80, &agents);
    assert!(render.rows.iter().any(|row| row.contains(" IN PROGRESS 1")));
    for other in [
        " ACTION REQUIRED ",
        " QUEUED ",
        " WAITING ON OTHERS ",
        " COMPLETED",
    ] {
        assert!(
            !render.rows.iter().any(|row| row.contains(other)),
            "{other}: {:#?}",
            render.rows
        );
    }
    // Action shown alone: no IN PROGRESS.
    board.only_section = Some(TaskSection::ActionRequired);
    let render = render_board_with_agents(&board, 186, 80, &agents);
    assert!(!render.rows.iter().any(|row| row.contains(" IN PROGRESS ")));
    assert!(render
        .rows
        .iter()
        .any(|row| row.contains(" ACTION REQUIRED 4")));
}

#[test]
fn a_working_agents_step_is_its_phase_word_never_its_raw_stage() {
    use crate::api::schema::AgentStatus;
    use crate::client::shell::task_board::board_agents;
    let mut snap = snapshot();
    snap.workspaces = vec![workspace("ws_h", "herdr", "/h")];
    let stages = [
        ("waiting · Nadine: 2 picks", None),
        ("review · 2 findings", Some("Review")),
        ("build", Some("Build")),
        ("check", Some("Verify")),
        ("verify · tests", Some("Verify")),
        ("plan", Some("Plan")),
        ("ship · push", Some("Ship")),
    ];
    snap.agents = stages
        .iter()
        .enumerate()
        .map(|(index, (stage, _))| {
            live_agent(
                &format!("p{index}"),
                "ws_h",
                "herdr-lead",
                AgentStatus::Working,
                &[("stage", stage)],
            )
        })
        .collect();
    let config = ClientShellConfig::from_config(&Config::default());
    let got: Vec<AgentState> = board_agents(Some(&snap), &config, None, 0)
        .into_iter()
        .map(|agent| agent.state)
        .collect();
    let want: Vec<AgentState> = stages
        .iter()
        .map(|(_, word)| AgentState::Working(word.map(Into::into)))
        .collect();
    assert_eq!(got, want);
}

#[test]
fn in_progress_lists_only_working_agents_each_on_its_tasks_step() {
    // A worker at work under a lead at rest: the lead has no row, the worker is a top row.
    let agents = [
        resting("w1:p9", "herdr", "herdr-lead"),
        worker("w1:p1", "herdr", "herdr-w1", "w1:p9", Some("Review")),
        waiting("w1:p7", "herdr", "herdr-w7", Some("pick A or B")),
    ];
    let render = render_board_with_agents(&sections_board(), 186, 90, &agents);
    let heading = render.row_with(" IN PROGRESS");
    let bottom = (heading..render.rows.len())
        .find(|y| render.rows[*y].starts_with(" ╰"))
        .unwrap_or_else(|| panic!("{:#?}", render.rows));
    let block = &render.rows[heading..bottom];
    assert!(
        render.rows[heading].ends_with("IN PROGRESS 1 ▼"),
        "{:?}",
        render.rows[heading]
    );
    assert!(
        block
            .iter()
            .all(|row| !row.contains("herdr-lead") && !row.contains("herdr-w7")),
        "{block:#?}"
    );
    let row = render.row_with("herdr-w1");
    assert!(
        !render.rows[row].contains('╰') && !render.rows[row].contains('├'),
        "{:?}",
        render.rows[row]
    );
    // With no card its Step now is its task's next action, and Doing what it does.
    let head = heading + 2;
    assert_eq!(
        render.column(row, "build round 19"),
        render.column(head, "Step now")
    );
    assert_eq!(render.column(row, "Review"), render.column(head, "Doing"));
}

#[test]
fn each_section_draws_its_own_columns_under_its_own_header_row() {
    let agents = [sections_worker()];
    // From 80 to 109 Age and Waiting give way first.
    for (width, age) in [(186, true), (89, false)] {
        let render = render_board_with_agents(&sections_board(), width, 90, &agents);
        // Each box's column-name row, the first row with `names` under its heading.
        let head = |title: &str, names: &[&str], hidden: &[&str]| {
            let at = render.row_with(title);
            let y = (at + 1..render.rows.len())
                .find(|y| render.rows[*y].contains(names[0]))
                .unwrap_or_else(|| panic!("{title} has no head: {:#?}", render.rows));
            assert_eq!(y, at + 2, "{title}: {:#?}", render.rows);
            let columns: Vec<u16> = names.iter().map(|name| render.column(y, name)).collect();
            assert!(
                columns.windows(2).all(|pair| pair[0] < pair[1]),
                "{title}: {columns:?}"
            );
            for name in hidden {
                assert!(
                    !render.rows[y].contains(name),
                    "{title} shows {name}: {:?}",
                    render.rows[y]
                );
            }
            y
        };
        let live = head(
            " IN PROGRESS",
            &["Agent", "Working on", "Stage", "Step now", "Doing", "For"],
            &[],
        );
        // Age or Waiting last where they show, and among the hidden where they give way.
        let last = |names: &[&'static str], last: &'static str| {
            let mut names = names.to_vec();
            if age {
                names.push(last);
            }
            names
        };
        let gone = |names: &[&'static str], last: &'static str| {
            let mut names = names.to_vec();
            if !age {
                names.push(last);
            }
            names
        };
        let action = head(
            " ACTION REQUIRED",
            &last(
                &["#", "Project", "Task", "Action", "Due", "Blocking"],
                "Age",
            ),
            &gone(&[], "Age"),
        );
        // No queued row blocks anything, so QUEUED has no Blocking column.
        let queued = head(
            " QUEUED",
            &last(&["#", "Project", "Task", "Next step", "Due"], "Age"),
            &gone(&["Blocking"], "Age"),
        );
        let waiting = head(
            " WAITING ON OTHERS",
            &last(&["#", "Project", "Task", "Owner", "Requested"], "Waiting"),
            &gone(&["Blocking", "Due"], "Waiting"),
        );
        let completed = head(
            " COMPLETED",
            &["#", "Project", "Task", "Completed"],
            &["Due", "Age"],
        );
        // `#`, Project and Task line up in every task section, and every box is one width.
        for name in ["Project", "Task"] {
            let at: Vec<u16> = [action, queued, waiting, completed]
                .iter()
                .map(|y| render.column(*y, name))
                .collect();
            assert!(
                at.windows(2).all(|pair| pair[0] == pair[1]),
                "{name} {width}: {at:?}"
            );
        }
        let right: Vec<usize> = [live, action, queued, waiting, completed]
            .iter()
            .map(|y| UnicodeWidthStr::width(render.rows[*y].as_str()))
            .collect();
        assert!(
            right.windows(2).all(|pair| pair[0] == pair[1]),
            "{width}: {right:?}"
        );
        // Project is a column of its own, not a prefix in Task.
        let row = render.row_with("Ship the oak");
        assert_eq!(
            render.column(row, "oak-cli"),
            render.column(action, "Project")
        );
        assert!(
            !render.rows[row].contains("oak-cli ·"),
            "{:?}",
            render.rows[row]
        );
        assert_eq!(
            render.column(row, "Ship the oak"),
            render.column(action, "Task")
        );
        // A person's request splits into its owner and what is asked.
        let row = render.row_with("Settle the");
        assert_eq!(
            render.column(row, "… Carmen"),
            render.column(waiting, "Owner")
        );
        assert_eq!(
            render.column(row, "finish the review"),
            render.column(waiting, "Requested")
        );
        // A blocking row names what waits on it; a done row its day.
        let row = render.row_with("Pick the");
        assert_eq!(
            render.column(row, if age { "Chrome pairing" } else { "Chrome…" }),
            render.column(action, "Blocking")
        );
        let row = render.row_with("Keep pane");
        assert_eq!(
            render.column(row, "✓ Sep 27"),
            render.column(completed, "Completed")
        );
    }
    // From 56 to 79 Project folds back into Task and Blocking, Age and Requested give way.
    let render = render_board_with_agents(&sections_board(), 70, 90, &agents);
    let at = render.row_with(" ACTION REQUIRED");
    let y = at + 2;
    for name in ["#", "Task", "Action", "Due"] {
        render.column(y, name);
    }
    for name in ["Project", "Blocking", "Age"] {
        assert!(
            !render.rows[y].contains(name),
            "{name}: {:?}",
            render.rows[y]
        );
    }
    render.row_with("oak-cli · Ship");
}

#[test]
fn task_board_sections_come_in_order_with_their_counts() {
    let render = render_board(&fixture_board(), WIDE, 60);
    let headings = [
        " ACTION REQUIRED 5",
        " QUEUED 4",
        " WAITING ON OTHERS 2",
        " COMPLETED 2",
    ]
    .map(|heading| render.row_with(heading));
    assert!(
        headings.windows(2).all(|pair| pair[0] < pair[1]),
        "{headings:?}"
    );
    assert_eq!(render.rows[1], "", "a blank line under the header");
    assert!(render.rows[2].contains("Queued"), "the chip row is row 2");
    assert_eq!(headings[0], 3);
    assert!(
        render.rows[5].contains("│ Task"),
        "the section's own column row"
    );
    assert_eq!(
        render.cell(headings[0], "ACTION REQUIRED").fg,
        render.palette.yellow
    );
    assert!(render
        .cell(headings[0], "ACTION REQUIRED")
        .modifier
        .contains(Modifier::BOLD));
    assert_eq!(render.cell(headings[1], "QUEUED").fg, render.palette.teal);
    assert_eq!(
        render.cell(headings[3], "COMPLETED").fg,
        render.palette.green
    );
    assert_eq!(
        render.cell(headings[0], "5").fg,
        render.palette.overlay1,
        "the count is a grey number after the heading"
    );
    // The first row is numbered 1: rows count down the list as shown.
    assert!(render.rows[headings[0] + 4].starts_with(" │ 1 "));
}

#[test]
fn task_board_sections_are_rounded_boxes_under_their_heading() {
    let render = render_board(&fixture_board(), 132, 40);
    let rows = &render.rows;
    let heading = render.row_with("ACTION REQUIRED");
    let top = heading + 1;
    assert!(
        rows[heading].starts_with("  ACTION REQUIRED 5")
            && rows[top].starts_with(" ╭─")
            && rows[top].ends_with("─╮"),
        "the heading above the top border: {:#?}",
        rows
    );
    assert_eq!(
        UnicodeWidthStr::width(rows[top].as_str()),
        131,
        "the box ends one column before the edge"
    );
    let head = &rows[top + 1];
    assert!(head.starts_with(" │ #"), "{head:?}");
    for name in ["│ Project", "│ Task", "│ Action", "│ Age"] {
        assert!(head.contains(name), "{name:?} in {head:?}");
    }
    assert!(head.ends_with('│'), "{head:?}");
    let rule = &rows[top + 2];
    assert!(
        rule.starts_with(" ├─") && rule.contains('┼') && rule.ends_with("─┤"),
        "{rule:?}"
    );
    let first = &rows[top + 3];
    assert!(first.starts_with(" │ 1 "), "numbered as shown: {first:?}");
    assert!(
        first.contains("│ sidebar ") && first.contains("│ Say final"),
        "the project in its own cell before the task: {first:?}"
    );
    assert_eq!(
        render.column(top + 3, "sidebar"),
        render.column(top + 1, "Project")
    );
    assert!(first.ends_with('│'), "{first:?}");
    let bottom = rows
        .iter()
        .skip(top)
        .position(|row| row.starts_with(" ╰"))
        .map(|at| top + at)
        .expect("the box's bottom");
    assert!(
        rows[bottom].contains('┴') && rows[bottom].ends_with("─╯"),
        "{:?}",
        rows[bottom]
    );
    assert!(
        rows[bottom - 1].contains("│ rfc-tool ") && rows[bottom - 1].contains("│ Approve"),
        "the last ACTION REQUIRED task, the oldest"
    );
    assert_eq!(rows[bottom + 1], "", "a blank row between sections");

    // The boxes below start with their own column names, `#`, Project and Task in the first
    // box's columns, each box in its own colour.
    let claude = render.row_with("QUEUED");
    let waiting = render.row_with("WAITING ON OTHERS");
    for (y, name) in [(claude + 2, "│ Next step"), (waiting + 2, "│ Requested")] {
        assert!(
            rows[y].starts_with(" │ #") && rows[y].contains(name),
            "{name:?} in {:?}",
            rows[y]
        );
        assert_eq!(
            bar_columns(&rows[y])[..4],
            bar_columns(head)[..4],
            "{:?}",
            rows[y]
        );
    }
    let border = |y: usize| render.buffer[(1, y as u16)].fg;
    assert_eq!(border(top), border(top + 3), "one colour per box");
    assert_ne!(
        border(top),
        border(claude + 1),
        "ACTION REQUIRED and QUEUED apart"
    );
    assert_ne!(border(claude + 1), border(waiting + 1));
    assert_eq!(
        render.cell(heading, "ACTION REQUIRED").fg,
        render.palette.yellow,
        "the heading in its section's colour"
    );
    let inner_line = rows[top + 3].find("│ sidebar ").expect("task") as u16;
    let inner_line = UnicodeWidthStr::width(&rows[top + 3][..inner_line as usize]) as u16;
    assert_eq!(
        render.buffer[(inner_line, (top + 3) as u16)].fg,
        border(top),
        "the lines inside in the section's colour"
    );

    // DONE, last 7 days stays one folded line.
    let done = render.row_with("last 7 days");
    assert!(
        rows[done].contains("▶") && rows[done].contains(" COMPLETED 2"),
        "{:?}",
        rows[done]
    );
    assert!(
        !rows[done].contains('╭') && !rows[done + 1].contains('╭'),
        "folded: no box: {:?}",
        rows[done]
    );
    assert!(
        rows[done + 1..]
            .iter()
            .all(|row| !row.contains("Box lines on the")),
        "its tasks stay hidden"
    );
}

/// The task rows' numbers as drawn, top down.
fn row_numbers(render: &BoardRender) -> Vec<u32> {
    render
        .rows
        .iter()
        .filter_map(|row| split_cells(row).first()?.parse().ok())
        .collect()
}

#[test]
fn task_rows_are_numbered_down_the_list_as_shown_whatever_the_sort_filter_or_section() {
    use crate::client::shell::task_board::TaskSort;
    let mut board = fixture_board();
    board.done_expanded = true;
    for sort in [
        TaskSort::Newest,
        TaskSort::Oldest,
        TaskSort::Project,
        TaskSort::State,
        TaskSort::File,
    ] {
        board.sort = sort;
        let render = render_board(&board, 186, 80);
        assert_eq!(
            row_numbers(&render),
            (1..=13).collect::<Vec<_>>(),
            "{sort:?}"
        );
    }
    // Newest first, the first row is the newest task, not the file's first.
    board.sort = TaskSort::Newest;
    let render = render_board(&board, 186, 80);
    let first = render.row_with("Say final, then decide");
    assert!(
        render.rows[first].starts_with(" │ 1 "),
        "{:?}",
        render.rows[first]
    );
    // A filter or one section shown alone numbers what is left from 1.
    board.filter = "Chrome".into();
    let render = render_board(&board, 186, 80);
    assert_eq!(row_numbers(&render), [1]);
    board.filter.clear();
    board.only_section = Some(TaskSection::Queued);
    let render = render_board(&board, 186, 80);
    assert_eq!(row_numbers(&render), [1, 2, 3, 4]);
    // The card panel numbers a task as its Tasks row does.
    let mut board = work_board();
    board.work_selected = Some(card("2026-09-24-tree-sidebar.md"));
    let panel = panel_rows(&render_board(&board, 186, 50)).join("\n");
    for part in ["#1  say final (you)", "#2  push the fork (claude)"] {
        assert!(panel.contains(part), "{part:?} in {panel}");
    }
    board.view = BoardView::Tasks;
    let render = render_board(&board, 186, 46);
    assert!(render.rows[render.row_with("say final")].starts_with(" │ 1 "));
    assert!(render.rows[render.row_with("push the fork")].starts_with(" │ 2 "));
}

#[test]
fn the_card_panel_numbers_a_done_task_only_while_done_is_open() {
    // A done task has a Tasks row only while DONE is open: folded, the panel gives it `·`.
    let mut list = parse_task_list(WORK_V1).expect("work fixture");
    let mut done = list.tasks[1].clone();
    done.group = TaskGroup::Done;
    done.task = "tidy the rail".into();
    done.done_on = Some("2026-09-24".into());
    list.tasks.push(done);
    let mut board = work_board();
    board.store.receive(Ok(list), std::time::Instant::now());
    board.work_selected = Some(card("2026-09-24-tree-sidebar.md"));
    let panel = panel_rows(&render_board(&board, 186, 52)).join("\n");
    assert!(panel.contains("·  tidy the rail"), "{panel}");
    assert!(panel.contains("#2  push the fork"), "{panel}");
    board.done_expanded = true;
    let panel = panel_rows(&render_board(&board, 186, 52)).join("\n");
    assert!(panel.contains("#4  tidy the rail"), "{panel}");
}

#[test]
fn task_board_rows_show_number_task_age_and_state() {
    let mut board = fixture_board();
    board.done_expanded = true;
    let render = render_board(&board, WIDE, 60);
    let chrome = render.row_with("Per-client Chrome is stuck at pairing");
    let row = &render.rows[chrome];
    // Third newest in NEEDS YOU, so numbered 3.
    assert!(row.starts_with(" │ 3 "), "{row:?}");
    for part in ["│ Chrome ", "│ Per-client", "2d", "go on the pairing"] {
        assert!(row.contains(part), "{part:?} in {row:?}");
    }
    // A task first seen today says so; `now` is only a waiting tab's live ask.
    let today = &render.rows[render.row_with("Say final, then decide")];
    assert_eq!(
        split_cells(today).last().map(String::as_str),
        Some("today"),
        "{today:?}"
    );
    let blue = render.row_with("Blue terminal icon in Vesper");
    assert!(render.rows[blue].starts_with(" │ 13"));

    assert_eq!(render.cell(chrome, "3 ").fg, render.palette.overlay0);
}

#[test]
fn a_task_with_an_agent_shows_its_plain_state_and_names_the_agent_only_in_its_detail() {
    let mut board = fixture_board();
    let render = render_board(&board, WIDE, 60);
    let lead = render.row_with("Never cut the lead's name");
    let cells = split_cells(&render.rows[lead]);
    assert_eq!(cells[3], "to do", "{cells:?}");
    assert!(!render.rows[lead].contains('◐'), "{:?}", render.rows[lead]);
    assert_eq!(render.cell(lead, "to do").fg, render.palette.text);
    board.selected = Some(key(7));
    board.open_row = board.selected.clone();
    let render = render_board(&board, WIDE, 60);
    // Open in QUEUED, the row keeps its state; the block names its agent, not the next step
    // its row shows whole.
    let lead = render.row_with("Never cut the lead's name");
    let cells = split_cells(&render.rows[lead]);
    assert_eq!(cells[3], "to do", "{cells:?}");
    assert!(
        render.rows[lead + 1].starts_with(" │     Agent  herdr-w1 · 12m"),
        "{:#?}",
        render.rows
    );
    assert!(
        !render.rows[lead + 1..lead + 5]
            .iter()
            .any(|row| row.contains("Next step")),
        "{:#?}",
        render.rows
    );
    assert!(
        !render
            .rows
            .iter()
            .any(|row| row.contains("Waiting on  to do")),
        "{:#?}",
        render.rows
    );
    // Completed, the block names the agent.
    let mut board = fixture_board_in_section(7, "completed");
    board.done_expanded = true;
    board.selected = Some(key(7));
    board.open_row = board.selected.clone();
    let render = render_board(&board, WIDE, 60);
    let row = render.row_with("Never cut the lead's name");
    assert!(
        !render.rows[row].contains("herdr-w1"),
        "{:?}",
        render.rows[row]
    );
    assert!(
        render
            .rows
            .iter()
            .any(|row| row.contains("Agent  herdr-w1 · 12m")),
        "{:#?}",
        render.rows
    );
}

#[test]
fn a_task_row_an_agent_is_on_starts_with_its_spinner_and_its_detail_says_what_it_does() {
    // A task with a row of its own: one the reader does not put in progress.
    let mut board = fixture_board_in_section(7, "queued");
    let agents = [worker(
        "w2:p2",
        "herdr",
        "herdr-w1",
        "w2:p1",
        Some("Writing report.md"),
    )];
    let render = render_board_with_agents(&board, 186, 60, &agents);
    // Its own row, not LIVE's, which names it too: the spinner leads its Task cell.
    let row = render.row_with(&format!("│ {WORKER_SPINNER} Never cut the lead's name"));
    assert!(
        render.rows[row].contains("│ sidebar "),
        "{:?}",
        render.rows[row]
    );
    assert_eq!(
        render.cell(row, WORKER_SPINNER).fg,
        render.palette.yellow,
        "the tree's working colour"
    );
    // A task no agent is on has none.
    let plain = render.row_with("Stamp the build sha");
    assert!(
        render.rows[plain].contains("│ oak-cli ") && render.rows[plain].contains("│ Stamp"),
        "{:?}",
        render.rows[plain]
    );
    // Open in QUEUED, its block names the agent its spinner stands for and what it does, not
    // the next step its row shows whole.
    board.selected = Some(key(7));
    board.open_row = board.selected.clone();
    let render = render_board_with_agents(&board, 186, 60, &agents);
    let row = render.row_with(&format!("│ {WORKER_SPINNER} Never cut the lead's name"));
    assert!(
        render.rows[row + 1].starts_with(" │     Agent  herdr-w1 · 12m · Writing report.md"),
        "{:#?}",
        render.rows
    );
    assert!(
        !render.rows[row + 1..row + 4]
            .iter()
            .any(|row| row.contains("Next step ")),
        "{:#?}",
        render.rows
    );
    // Completed, its block says what the agent does.
    let mut board = fixture_board_in_section(7, "completed");
    board.done_expanded = true;
    board.selected = Some(key(7));
    board.open_row = board.selected.clone();
    let render = render_board_with_agents(&board, 186, 60, &agents);
    render.row_with("Agent  herdr-w1 · 12m · Writing report.md");
    // An agent the board does not list as working: its name and time alone.
    let mut board = fixture_board_in_section(6, "completed");
    board.done_expanded = true;
    board.selected = Some(key(6));
    board.open_row = board.selected.clone();
    let render = render_board_with_agents(&board, 186, 60, &agents);
    let agent = render.row_with("Agent  oakwood-ws-lead · 4m");
    assert!(
        render.rows[agent]
            .trim_end_matches('│')
            .trim_end()
            .ends_with("· 4m"),
        "{:?}",
        render.rows[agent]
    );
}

#[test]
fn done_holds_the_last_seven_days_newest_first_each_with_the_day_it_was_done() {
    // Task 12 was done two days before the list was read; task 13 has no date, so it reads
    // as done the day the list was read.
    let json = LIST_V1
        .replacen(
            r#""done_on": "2026-09-25""#,
            r#""done_on": "2026-09-23""#,
            1,
        )
        .replacen(r#""done_on": "2026-09-25""#, r#""done_on": null"#, 1);
    let mut board = TaskBoard {
        open: true,
        ..TaskBoard::default()
    };
    board.store.receive_json(&json, std::time::Instant::now());
    let render = render_board(&board, WIDE, 60);
    let folded = render.row_with("last 7 days");
    assert!(
        render.rows[folded].starts_with("  COMPLETED 2 ▶"),
        "{:?}",
        render.rows[folded]
    );
    board.done_expanded = true;
    let render = render_board(&board, WIDE, 60);
    let heading = render.row_with("COMPLETED 2");
    // COMPLETED's box names its own columns, with no Due or Age, then its rows.
    assert_eq!(
        split_cells(&render.rows[heading + 2]),
        ["#", "Project", "Task", "Completed"],
        "{:?}",
        render.rows[heading + 2]
    );
    assert_eq!(
        split_cells(&render.rows[heading + 4])[1],
        "sidebar",
        "{:?}",
        render.rows[heading + 4]
    );
    let newest = render.row_with("Blue terminal icon");
    let older = render.row_with("Box lines on the");
    assert!(newest < older, "newest first");
    // The day it was done is under Completed, the last cell.
    let completed = |y: usize| -> Vec<String> {
        render.rows[y]
            .split('│')
            .map(|cell| cell.trim().to_owned())
            .skip(4)
            .collect()
    };
    assert_eq!(completed(newest), ["✓ Sep 25", ""]);
    assert_eq!(completed(older), ["✓ Sep 23", ""]);
    let top = render.row_with("ACTION REQUIRED") + 1;
    assert_eq!(split_cells(&render.rows[top + 1])[3..], ["Action", "Age"]);
}

#[test]
fn task_board_age_is_in_text_once_a_task_is_stale_and_dim_before() {
    let render = render_board(&fixture_board(), WIDE, 40);
    let palette = &render.palette;
    let stale = render.row_with("Approve the polished wording");
    let stale = render.cell(stale, "9d");
    assert_eq!(
        (stale.fg, stale.modifier.contains(Modifier::BOLD)),
        (palette.text, false)
    );
    let fresh = render.row_with("Per-client Chrome");
    assert_eq!(render.cell(fresh, "2d").fg, palette.overlay0);
}

#[test]
fn task_board_done_is_crossed_out_and_dim_with_its_day_in_green() {
    let mut board = fixture_board();
    board.done_expanded = true;
    let render = render_board(&board, WIDE, 60);
    let done = render.row_with("Blue terminal icon in Vesper");
    for text in ["13", "sidebar", "Blue terminal"] {
        let cell = render.cell(done, text);
        assert!(cell.modifier.contains(Modifier::CROSSED_OUT), "{text}");
        assert_eq!(cell.fg, render.palette.overlay0, "{text}");
    }
    // COMPLETED's colour is its tick's; the day is grey.
    assert_eq!(render.cell(done, "✓ Sep 25").fg, render.palette.green);
    let day = render.cell(done, "Sep 25");
    assert_eq!(day.fg, render.palette.overlay1);
    assert!(!day.modifier.contains(Modifier::CROSSED_OUT));
    assert!(
        !render.rows[done].contains("✓ done"),
        "{:?}",
        render.rows[done]
    );
}

/// Every word of `text` shows, in order, across the drawn rows.
fn shows_whole(render: &BoardRender, text: &str) -> bool {
    let words = render.rows.join(" ");
    let mut rest = words.as_str();
    text.split_whitespace().all(|word| match rest.find(word) {
        Some(at) => {
            rest = &rest[at + word.len()..];
            true
        }
        None => false,
    })
}

#[test]
fn task_board_long_text_is_cut_on_its_row_and_shown_whole_once_open() {
    let list = parse_task_list(LIST_V1).expect("fixture");
    for width in [WIDE, NARROW] {
        for (n, task) in list.tasks.iter().enumerate() {
            let mut board = fixture_board();
            board.done_expanded = true;
            board.selected = Some(key(n + 1));
            board.open_row = board.selected.clone();
            let render = render_board(&board, width, 80);
            let rect = render
                .hits
                .rows
                .iter()
                .find(|(_, target)| *target == key(n + 1))
                .expect("a row for every task")
                .0;
            // Open, the row's own lines hold its text and state whole, never cut.
            let detail = render.rows[usize::from(rect.y)..usize::from(rect.bottom())].join(" ");
            assert!(!detail.contains('…'), "{width}: {detail:?}");
            let shows = |text: &str| text.split_whitespace().all(|word| detail.contains(word));
            assert!(shows(&task.task), "{width}: {:?} in {detail:?}", task.task);
            // NEEDS YOU says "needs you" itself; a done task says it is done.
            let says_state = !(task.group == TaskGroup::You && task.state == "needs you")
                && task.group != TaskGroup::Done;
            if says_state {
                assert!(
                    shows(&task.state),
                    "{width}: {:?} in {detail:?}",
                    task.state
                );
            }
        }
    }
}

#[test]
fn every_task_row_is_one_line_its_long_text_cut_at_a_word_with_an_ellipsis() {
    let list = parse_task_list(LIST_V1).expect("fixture");
    for width in [WIDE, NARROW] {
        let mut board = fixture_board();
        board.done_expanded = true;
        let render = render_board(&board, width, 80);
        for n in 1..=list.tasks.len() {
            let rect = render
                .hits
                .rows
                .iter()
                .find(|(_, target)| *target == key(n))
                .expect("a row for every task")
                .0;
            assert_eq!(rect.height, 1, "{width}: task {n} takes one line");
        }
    }
    // The long task (9) at the narrow width ends at a whole word, then `…`.
    let mut board = fixture_board();
    board.done_expanded = true;
    let narrow = render_board(&board, NARROW, 80);
    let cells = cells_of(&narrow, "onboard · Run");
    let task = cells[1].as_str();
    let text = task
        .strip_prefix("onboard · ")
        .and_then(|task| task.strip_suffix('…'))
        .unwrap_or_else(|| panic!("the project first, the cut last: {task:?}"));
    let whole = &list.tasks[8].task;
    // A word boundary is before a space or after a hyphen, as `cut` says.
    assert!(
        whole.starts_with(text) && (whole[text.len()..].starts_with(' ') || text.ends_with('-')),
        "cut before a space, never inside a word: {task:?}"
    );
}

#[test]
fn the_project_is_a_text_column_from_80_wide_else_a_dim_task_prefix_and_never_a_where_column() {
    for width in [132, 92, 70] {
        let render = render_board(&fixture_board(), width, 40);
        let top = render.row_with("ACTION REQUIRED") + 1;
        let head = split_cells(&render.rows[top + 1]);
        // Age gives way below 110 wide, and Project below 80, where it leads the task.
        let names: &[&str] = match width {
            110.. => &["#", "Project", "Task", "Action", "Age"],
            80.. => &["#", "Project", "Task", "Action"],
            _ => &["#", "Task", "Action"],
        };
        assert_eq!(head, names, "{width}");
        let row = render.row_with("Per-client Chrome");
        let project = if width < 80 { "Chrome · " } else { "Chrome " };
        assert!(
            render.rows[row].contains(project),
            "{width}: {:?}",
            render.rows[row]
        );
        // A column is grey; folded before the task it is a dim lead.
        let colour = if width < 80 {
            render.palette.overlay0
        } else {
            render.palette.overlay1
        };
        assert_eq!(render.cell(row, project).fg, colour, "{width}");
        assert_eq!(render.cell(row, "Per-client").fg, render.palette.text);
        assert_eq!(render.cell(row, "3 ").fg, render.palette.overlay0, "# dim");
    }
}

#[test]
fn the_selected_row_shows_a_labelled_detail_block_and_the_footer_drops_its_keys() {
    let mut board = fixture_board();
    board.selected = Some(key(1));
    board.open_row = board.selected.clone();
    let render = render_board(&board, 132, 50);
    let row = render.row_with("Per-client Chrome is stuck at pairing");
    // The row keeps its task and its action whole. The block holds only what the row does not
    // show: its file, and no action, place or agent.
    // Labels in one dim column as wide as the longest and two more, values beside them.
    assert!(render.rows[row].contains("go on the pairing broadcast"));
    assert_eq!(render.cell(row, "go on").bg, render.palette.selection_bg);
    let field = |width: usize, label: &str, value: &str| format!(" │     {label:<width$}{value}");
    let got = &render.rows[row + 1];
    let want = field(
        6,
        "File",
        "~/Workdir/oakwood-workspace/WORKSPACE-STATUS.md:85",
    );
    assert!(got.starts_with(want.as_str()), "{want:?}\n{got:?}");
    assert!(got.ends_with('│'), "inside the box: {got:?}");
    assert_eq!(render.cell(row + 1, "File").fg, render.palette.overlay0);
    assert_eq!(render.cell(row + 1, "~/Workdir").fg, render.palette.text);
    assert_eq!(
        render.cell(row + 1, "~/Workdir").bg,
        render.palette.selection_bg
    );
    for left_out in ["Where", "Action", "Agent"] {
        assert!(
            !render.rows[row + 1..row + 4]
                .iter()
                .any(|line| line.contains(left_out)),
            "no {left_out:?}: {:#?}",
            &render.rows[row + 1..row + 4]
        );
    }
    assert_eq!(
        split_cells(&render.rows[row + 2]),
        Vec::<String>::new(),
        "a blank line"
    );
    let keys = &render.rows[row + 3];
    assert!(
        keys.starts_with(" │      Enter  go to space     d  done     ←  close "),
        "the row's keys as the footer draws them: {keys:?}"
    );
    assert!(!render
        .cell(row + 3, "Enter")
        .modifier
        .contains(Modifier::BOLD));
    assert!(render.rows[row + 4].starts_with(" │ 4 "), "the next row");
    // The footer keeps the keys that move around and leaves the row's own to the block.
    let footer = &render.rows[49];
    for kept in ["↑ ↓  select", "x  delete", "/  search", "?  all keys"] {
        assert!(footer.contains(kept), "{kept:?} in {footer:?}");
    }
    for dropped in ["Enter", "d  done", "←  close"] {
        assert!(!footer.contains(dropped), "{dropped:?} in {footer:?}");
    }

    // A completed task's block says its file; an agent's task names it;
    // a task in two files says so; a long value wraps under its value column.
    board.selected = Some(key(13));
    board.open_row = board.selected.clone();
    board.done_expanded = true;
    let render = render_board(&board, 132, 60);
    let row = render.row_with("Blue terminal icon in Vesper");
    let expected = [field(
        6,
        "File",
        "~/projects/repos/herdr/.lead-notes/STATUS.md:15 (+1 more file)",
    )];
    for (offset, want) in expected.iter().enumerate() {
        let got = &render.rows[row + 1 + offset];
        assert!(got.starts_with(want.as_str()), "{want:?}\n{got:?}");
        assert!(got.ends_with('│'), "inside the box: {got:?}");
    }
    assert_eq!(render.cell(row + 1, "File").fg, render.palette.overlay0);
    assert_eq!(render.cell(row + 1, "~/projects").fg, render.palette.text);
    assert_eq!(
        render.cell(row + 1, "~/projects").bg,
        render.palette.selection_bg
    );
    assert!(
        !render.rows.iter().any(|row| row.contains("Agent ")),
        "no agent"
    );
    let mut board = fixture_board_in_section(6, "completed");
    board.selected = Some(key(6));
    board.open_row = board.selected.clone();
    board.done_expanded = true;
    let render = render_board(&board, 132, 60);
    assert!(
        render
            .rows
            .iter()
            .any(|row| row.starts_with(" │     Agent  oakwood-ws-lead · 4m")),
        "{:#?}",
        render.rows
    );
    let mut board = fixture_board_in_section(9, "completed");
    board.selected = Some(key(9));
    board.open_row = board.selected.clone();
    board.done_expanded = true;
    let render = render_board(&board, NARROW, 60);
    let row = render.row_with(" │     File  ~/Workdir/");
    assert!(
        render.rows[row + 1].starts_with(" │           ") && render.rows[row + 1].contains(":91"),
        "the rest of the text under its value: {:?}",
        render.rows[row + 1]
    );
}

#[test]
fn task_board_selected_row_and_its_detail_block_are_banded_and_one_target() {
    // Completed, its block keeps every field and its keys.
    let mut board = fixture_board_in_section(6, "completed");
    board.done_expanded = true;
    board.selected = Some(key(6));
    board.open_row = board.selected.clone();
    let render = render_board(&board, WIDE, 60);
    let row = render.row_with("Compare Carmen's 7 rows");
    assert_eq!(
        render.cell(row, "Compare").bg,
        render.palette.selection_bg,
        "the selected row is banded"
    );
    assert_eq!(
        render.cell(row + 1, "Agent").bg,
        render.palette.selection_bg
    );
    let hit = render
        .hits
        .rows
        .iter()
        .find(|(_, target)| *target == key(6))
        .expect("the row is a target")
        .0;
    // The row, two fields (agent, file), a blank line and the keys.
    assert_eq!(
        (hit.y, hit.height),
        (row as u16, 5),
        "the row and its detail"
    );
    assert!(
        render.rows[row + 4].contains(" Enter  go to agent     d  done     ←  close"),
        "Enter goes to the agent on it: {:?}",
        render.rows[row + 4]
    );
}

#[test]
fn task_board_foot_shows_problems_then_the_keys() {
    // Each problem with its path, as a click on the problem line shows it.
    let mut board = fixture_board();
    board.problems_shown = true;
    let render = render_board(&board, WIDE, 40);
    let keys = &render.rows[39];
    for part in [
        "↑ ↓  select",
        "Enter  go to space",
        "d  done",
        "?  all keys",
    ] {
        assert!(keys.contains(part), "{part:?} in {keys:?}");
    }
    assert!(render.rows[38].trim().chars().all(|ch| ch == '─'));
    let problems = &render.rows[37];
    assert!(
        problems.contains("Problems: /home/user/devbox/Workdir/oakwood-workspace/OLD-NOTES.md"),
        "{problems:?}"
    );
    assert_eq!(render.cell(37, "Problems").fg, render.palette.red);
}

#[test]
fn task_board_narrowed_to_a_space_shows_only_its_tasks_and_projects() {
    let mut board = fixture_board();
    board.space = Some("oakwood".into());
    let render = render_board(&board, WIDE, 40);
    let header = &render.rows[0];
    assert!(header.contains(" oakwood ▾   × "), "{header:?}");
    let chips = &render.rows[2];
    for part in [
        "! Action required 3",
        "◐ In progress 0",
        "… Waiting on others 2",
    ] {
        assert!(chips.contains(part), "{part:?} in {chips:?}");
    }
    assert!(!header.contains("Space: All"));
    let chrome = render.row_with("Per-client Chrome");
    assert!(render.rows[chrome].contains(" Chrome "));
    assert!(!render.rows.iter().any(|row| row.contains("oakwood/")));
    assert!(!render.rows.iter().any(|row| row.contains("Never cut")));
    assert!(!render.rows.iter().any(|row| row.contains("last 7 days")));
    let chip = render.hits.back_to_all;
    assert_eq!((chip.y, chip.width), (0, 3));
    assert_eq!(render.buffer[(chip.x + 1, 0)].symbol(), "×");
}

/// The header has no counts: with no list read, zeros would read as real.
fn assert_no_counts(header: &str) {
    for count in ["Action", "In progress", "Waiting", "·", "Updated "] {
        assert!(!header.contains(count), "{count:?} in {header:?}");
    }
    assert!(
        header.contains(" Tasks ") && header.ends_with('×'),
        "{header:?}"
    );
}

#[test]
fn task_board_says_when_it_is_reading_failed_or_empty() {
    let mut board = TaskBoard {
        open: true,
        ..TaskBoard::default()
    };
    let render = render_board(&board, WIDE, 20);
    assert!(
        render.rows[3].contains("Reading tasks"),
        "{:#?}",
        render.rows
    );
    assert_no_counts(&render.rows[0]);

    board
        .store
        .receive(Err("tasks_unavailable".into()), std::time::Instant::now());
    let render = render_board(&board, WIDE, 20);
    assert!(render.rows[3].contains("Tasks unavailable: tasks_unavailable"));
    assert_no_counts(&render.rows[0]);

    board
        .store
        .receive_json(r#"{"version": 1, "tasks": []}"#, std::time::Instant::now());
    let render = render_board(&board, WIDE, 20);
    assert!(render.rows[3].contains("No tasks"), "{:#?}", render.rows);
    assert!(
        render.rows[2]
            .contains("◐ In progress 0   ! Action required 0   ◌ Queued 0   … Waiting on others 0"),
        "a list read with no tasks has real zeros: {:?}",
        render.rows[2]
    );

    let mut board = fixture_board();
    board
        .store
        .receive_json("not json", std::time::Instant::now());
    let render = render_board(&board, WIDE, 40);
    assert!(render
        .rows
        .iter()
        .any(|row| row.contains("Per-client Chrome")));
    assert!(
        render.rows[36].contains("Last read failed: unreadable task list"),
        "{:#?}",
        render.rows
    );
}

#[test]
fn task_board_scrolls_the_selected_task_into_view() {
    let mut board = fixture_board();
    board.selected = Some(key(13));
    board.reveal_selected = true;
    board.done_expanded = true;
    let render = render_board(&board, WIDE, 16);
    assert!(render.hits.scroll > 0);
    assert!(render.hits.scroll <= render.hits.max_scroll);
    assert!(render
        .rows
        .iter()
        .any(|row| row.contains("Blue terminal icon")));
    assert!(render
        .hits
        .rows
        .iter()
        .any(|(_, target)| *target == key(13)));
    let (_, start, end) = render
        .hits
        .spans
        .iter()
        .find(|(target, _, _)| *target == key(13))
        .cloned()
        .expect("span");
    assert!(start >= render.hits.scroll && end <= render.hits.scroll + render.hits.viewport);

    board.reveal_selected = false;
    board.scroll = 0;
    let render = render_board(&board, WIDE, 16);
    assert_eq!(render.hits.scroll, 0, "only a reveal moves the scroll");
}

/// A client over the older reader's board, newest first as [`fixture_board`].
fn board_state() -> ClientShellState {
    let mut config = Config::default();
    config.ui.sidebar_tree = true;
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state
        .task_board
        .store
        .receive_json(LIST_V1, std::time::Instant::now());
    state.task_board.sort = crate::client::shell::task_board::TaskSort::Newest;
    state
}

#[test]
fn task_board_open_draws_in_the_main_area_instead_of_the_panes() {
    let mut state = board_state();
    let closed = frame_rows(&state.compose(106, 30).expect("frame"));
    assert!(closed.iter().any(|row| row.contains("LIVE")));
    assert!(!closed.iter().any(|row| row.contains(" Tasks ")));

    state.task_board.open = true;
    let frame = state.compose(106, 30).expect("frame");
    let rows = frame_rows(&frame);
    assert!(!rows.iter().any(|row| row.contains("LIVE")), "{rows:#?}");
    assert!(
        rows[0].contains(" SPACES"),
        "the sidebar stays: {:?}",
        rows[0]
    );
    let area = state.layout(106, 30).pane_surface;
    let header: String = rows[area.y as usize]
        .chars()
        .skip(area.x as usize)
        .collect();
    assert!(
        header.starts_with("  Tasks   Phases  │  Space: All ▾ "),
        "{header:?}"
    );
    assert!(
        state.hits.panes.is_empty(),
        "no pane takes clicks under the board"
    );
    assert!(frame.cursor.is_none(), "the pane cursor is under the board");
    assert_eq!(state.hits.task_board.area, area);

    state.task_board.open = false;
    let reopened = frame_rows(&state.compose(106, 30).expect("frame"));
    assert_eq!(
        reopened, closed,
        "closing gives the panes back as they were"
    );
}

#[test]
fn task_board_open_keeps_pane_output_without_drawing_it() {
    let mut state = board_state();
    state.task_board.open = true;
    state.compose(106, 30).expect("frame");
    let mut pane = surface().panes[0].clone();
    pane.content_revision = 2;
    let patch = crate::protocol::PaneSurfacePatch {
        boot_id: "boot-1".into(),
        projection_revision: 1,
        base_surface_revision: 1,
        surface_revision: 2,
        rows: vec![crate::protocol::PaneSurfacePatchRow {
            x: 0,
            y: 0,
            cells: vec![
                crate::protocol::CellData {
                    symbol: "N".into(),
                    fg: 0,
                    bg: 0,
                    modifier: 0,
                    skip: false,
                    hyperlink: None,
                };
                4
            ],
        }],
        panes: vec![pane],
        cursor: None,
    };
    assert!(
        matches!(
            state.apply_pane_surface_patch(patch),
            ClientPaneSurfacePatchOutcome::Retained
        ),
        "nothing to draw while the board covers the panes"
    );
    state.task_board.open = false;
    let rows = frame_rows(&state.compose(106, 30).expect("frame"));
    assert!(
        rows.iter().any(|row| row.contains("NNNN")),
        "the kept output shows once the board closes: {rows:#?}"
    );
}

fn press_key(state: &mut ClientShellState, code: KeyCode) -> ClientShellInput {
    state.handle_raw_events(vec![RawInputEvent::Key(TerminalKey::new(
        code,
        KeyModifiers::NONE,
    ))])
}

/// The default prefix, `ctrl+b`, then `key`.
fn prefix_key(state: &mut ClientShellState, key: char) -> ClientShellInput {
    state.handle_raw_events(vec![RawInputEvent::Key(TerminalKey::new(
        KeyCode::Char('b'),
        KeyModifiers::CONTROL,
    ))]);
    press_key(state, KeyCode::Char(key))
}

fn board_click(state: &mut ClientShellState, rect: Rect) -> ClientShellInput {
    assert!(!rect.is_empty(), "drawn");
    let at = |kind| {
        RawInputEvent::Mouse(crossterm::event::MouseEvent {
            kind,
            column: rect.x,
            row: rect.y,
            modifiers: KeyModifiers::empty(),
        })
    };
    let mut outcome = state.handle_raw_events(vec![at(MouseEventKind::Down(MouseButton::Left))]);
    let up = state.handle_raw_events(vec![at(MouseEventKind::Up(MouseButton::Left))]);
    outcome.actions.extend(up.actions);
    outcome
}

fn focused_pane(outcome: &ClientShellInput) -> Option<&str> {
    outcome.actions.iter().find_map(|action| match action {
        ClientShellAction::Endpoint { request, .. } => match &request.method {
            crate::api::schema::Method::PaneFocus(target) => Some(target.pane_id.as_str()),
            _ => None,
        },
        _ => None,
    })
}

fn row_of(state: &ClientShellState, target: &RowKey) -> Rect {
    state
        .hits
        .task_board
        .rows
        .iter()
        .find(|(_, key)| key == target)
        .unwrap_or_else(|| panic!("{target:?} not drawn"))
        .0
}

#[test]
fn prefix_a_opens_and_closes_the_task_board() {
    let mut state = board_state();
    state.compose(106, 30).expect("frame");
    let outcome = prefix_key(&mut state, 'a');
    assert!(state.task_board.open && outcome.repaint);
    assert!(
        outcome.requests.is_empty(),
        "the key never reaches the pane"
    );
    state.compose(106, 30).expect("frame");
    prefix_key(&mut state, 'a');
    assert!(!state.task_board.open);
}

#[test]
fn task_board_key_is_in_the_help_screen() {
    let keybinds = crate::config::Keybinds::default();
    let groups = crate::input::keybind_help_groups(
        &keybinds,
        &[(KeyCode::Char('b'), KeyModifiers::CONTROL)],
    );
    let entry = groups
        .iter()
        .flat_map(|(_, entries)| entries)
        .find(|(_, label)| label == "task board")
        .expect("listed");
    assert_eq!(entry.0, "prefix+a");
    let others = groups
        .iter()
        .flat_map(|(_, entries)| entries)
        .filter(|(key, _)| key == "prefix+a")
        .count();
    assert_eq!(others, 1, "no other default key is prefix+a");
}

#[test]
fn task_board_keys_select_rows_and_close_without_reaching_the_pane() {
    let mut state = board_state();
    state.task_board.open = true;
    state.compose(106, 30).expect("frame");
    let outcome = press_key(&mut state, KeyCode::Down);
    assert!(outcome.requests.is_empty(), "{:?}", outcome.requests);
    assert_eq!(
        state.task_board.selected,
        Some(key(4)),
        "down starts at the top, the newest task"
    );
    assert!(state.task_board.reveal_selected);
    state.compose(106, 30).expect("frame");
    press_key(&mut state, KeyCode::Down);
    assert_eq!(state.task_board.selected, Some(key(5)));
    press_key(&mut state, KeyCode::Up);
    assert_eq!(state.task_board.selected, Some(key(4)));
    press_key(&mut state, KeyCode::Up);
    assert_eq!(state.task_board.selected, Some(key(4)), "the top stays");
    let outcome = press_key(&mut state, KeyCode::Char('x'));
    assert!(outcome.requests.is_empty(), "other keys stay on the board");
    press_key(&mut state, KeyCode::Esc);
    assert!(state.task_board.open, "esc first answers x's question");
    press_key(&mut state, KeyCode::Esc);
    assert!(!state.task_board.open);
}

#[test]
fn task_board_up_from_nothing_selects_the_last_task() {
    let mut state = board_state();
    state.task_board.open = true;
    state.compose(106, 30).expect("frame");
    press_key(&mut state, KeyCode::Up);
    // DONE, last 7 days starts folded, so the last task shown is the last of WAITING ON PEOPLE,
    // not a DONE one.
    assert_eq!(state.task_board.selected, Some(key(11)));
}

#[test]
fn task_board_enter_goes_to_the_tasks_agent_and_closes() {
    let mut state = board_state();
    state.task_board.open = true;
    state.task_board.selected = Some(key(1));
    state.compose(106, 30).expect("frame");
    let outcome = press_key(&mut state, KeyCode::Enter);
    assert_eq!(focused_pane(&outcome), None, "task 1 has no agent");
    assert!(state.task_board.open);

    state.task_board.selected = Some(key(6));
    let outcome = press_key(&mut state, KeyCode::Enter);
    assert_eq!(focused_pane(&outcome), Some("w3:p1"));
    assert!(!state.task_board.open, "the agent's pane shows");
}

#[test]
fn task_board_rows_select_on_click_and_go_to_the_agent_on_a_second() {
    let mut state = board_state();
    state.task_board.open = true;
    state.compose(106, 40).expect("frame");
    let row = row_of(&state, &key(6));
    let outcome = board_click(&mut state, row);
    assert_eq!(state.task_board.selected, Some(key(6)));
    assert_eq!(focused_pane(&outcome), None);
    state.compose(106, 40).expect("frame");
    let row = row_of(&state, &key(6));
    let outcome = board_click(&mut state, row);
    assert_eq!(focused_pane(&outcome), Some("w3:p1"));
    assert!(!state.task_board.open);
}

/// How many lines `target`'s row takes in a fresh frame: one while closed, more while open.
fn row_height(state: &mut ClientShellState, target: &RowKey) -> u16 {
    state.compose(106, 40).expect("frame");
    row_of(state, target).height
}

#[test]
fn a_selected_row_stays_one_line_until_right_opens_it_and_left_closes_it() {
    let mut state = board_state();
    state.task_board.open = true;
    state.compose(106, 40).expect("frame");
    press_key(&mut state, KeyCode::Down);
    assert_eq!(state.task_board.selected, Some(key(4)));
    assert_eq!(
        row_height(&mut state, &key(4)),
        1,
        "selecting moves the band"
    );
    press_key(&mut state, KeyCode::Right);
    assert!(row_height(&mut state, &key(4)) > 1, "right opens it");
    press_key(&mut state, KeyCode::Left);
    assert_eq!(row_height(&mut state, &key(4)), 1, "left closes it");
    press_key(&mut state, KeyCode::Char('l'));
    assert!(row_height(&mut state, &key(4)) > 1, "l opens it");
    press_key(&mut state, KeyCode::Char('h'));
    assert_eq!(row_height(&mut state, &key(4)), 1, "h closes it");
    // One open row at a time: moving on closes it, and coming back finds it closed.
    press_key(&mut state, KeyCode::Right);
    press_key(&mut state, KeyCode::Down);
    assert_eq!(state.task_board.selected, Some(key(5)));
    assert_eq!(row_height(&mut state, &key(4)), 1, "the row left closes");
    assert_eq!(row_height(&mut state, &key(5)), 1, "the next stays closed");
    press_key(&mut state, KeyCode::Up);
    assert_eq!(row_height(&mut state, &key(4)), 1, "closed on coming back");
    // While the search box takes the keys, the arrows stay its own.
    press_key(&mut state, KeyCode::Char('/'));
    press_key(&mut state, KeyCode::Right);
    press_key(&mut state, KeyCode::Enter);
    assert_eq!(row_height(&mut state, &key(4)), 1, "the box kept the arrow");
}

#[test]
fn right_opens_a_live_row_and_a_workers_row_too() {
    // A lead's row at the full width shows every fact in its cells: open, it has its keys.
    let mut state = worker_state("w2:p1");
    state.task_board.open = true;
    for pane in ["w2:p1", "w2:p2"] {
        let target = RowKey::Agent(pane.into());
        state.task_board.selected = Some(target.clone());
        assert_eq!(row_height(&mut state, &target), 1, "{pane} closed");
        press_key(&mut state, KeyCode::Right);
        assert!(row_height(&mut state, &target) > 1, "{pane} open");
        press_key(&mut state, KeyCode::Left);
    }
}

#[test]
fn a_click_on_the_selected_row_opens_it_and_another_closes_it() {
    let mut state = board_state();
    state.task_board.open = true;
    state.compose(106, 40).expect("frame");
    let row = row_of(&state, &key(6));
    board_click(&mut state, row);
    assert_eq!(state.task_board.selected, Some(key(6)));
    assert_eq!(row_height(&mut state, &key(6)), 1, "a first click selects");
    age_last_click(&mut state);
    let row = row_of(&state, &key(6));
    let outcome = board_click(&mut state, row);
    assert_eq!(focused_pane(&outcome), None, "not a double click");
    assert!(
        state.task_board.reveal_selected,
        "opening scrolls it into view"
    );
    assert!(
        row_height(&mut state, &key(6)) > 1,
        "a click on it opens it"
    );
    age_last_click(&mut state);
    let row = row_of(&state, &key(6));
    board_click(&mut state, row);
    assert_eq!(row_height(&mut state, &key(6)), 1, "another closes it");
}

#[test]
fn an_open_row_that_leaves_the_list_comes_back_closed_once_selected_again() {
    // A refresh drops the open row (as a delete does); the keys move on, and the row comes
    // back closed when it returns and is selected again.
    let mut state = board_state();
    state.task_board.open = true;
    state.compose(106, 40).expect("frame");
    let row = row_of(&state, &key(9));
    board_click(&mut state, row);
    press_key(&mut state, KeyCode::Right);
    assert!(row_height(&mut state, &key(9)) > 1, "open");
    let without = LIST_V1.replace("Run client-onboard live once", "Gone for now");
    let now = std::time::Instant::now();
    state.task_board.store.receive_json(&without, now);
    state.compose(106, 40).expect("frame");
    press_key(&mut state, KeyCode::Down);
    assert_ne!(state.task_board.selected, Some(key(9)), "the keys moved on");
    state.task_board.store.receive_json(LIST_V1, now);
    state.compose(106, 40).expect("frame");
    age_last_click(&mut state);
    let row = row_of(&state, &key(9));
    board_click(&mut state, row);
    assert_eq!(state.task_board.selected, Some(key(9)));
    assert_eq!(row_height(&mut state, &key(9)), 1, "back closed");
}

#[test]
fn the_footer_says_right_opens_a_selected_row_and_left_closes_it() {
    let mut state = board_state();
    state.task_board.open = true;
    let footer = |state: &mut ClientShellState| {
        let rows = frame_rows(&state.compose(132, 50).expect("frame"));
        rows.last().cloned().unwrap_or_default()
    };
    // With nothing selected the footer has no arrow; `?` lists them.
    assert!(!footer(&mut state).contains('→'), "nothing selected");
    press_key(&mut state, KeyCode::Char('?'));
    let rows = frame_rows(&state.compose(132, 50).expect("frame"));
    assert!(
        rows.iter().any(|row| row.contains("→ ←  details")),
        "? lists the arrows"
    );
    press_key(&mut state, KeyCode::Char('?'));
    state.task_board.selected = Some(key(1));
    let closed = footer(&mut state);
    assert!(closed.contains("→  details"), "{closed:?}");
    // Closed, the footer holds the row's own keys, as its block would.
    assert!(closed.contains("Enter  go to space"), "{closed:?}");
    assert!(closed.contains("d  done"), "{closed:?}");
    // A click on the hint does its key.
    let hint = state
        .hits
        .task_board
        .footer_keys
        .iter()
        .find(|(_, key)| *key == "→")
        .expect("the hint is a target")
        .0;
    board_click(&mut state, hint);
    let open = footer(&mut state);
    assert!(!open.contains("→  details"), "{open:?}");
    for key in ["Enter", "←  close"] {
        assert!(
            !open.contains(key),
            "the block holds the row's keys and close: {open:?}"
        );
    }
    assert_eq!(state.task_board.open_row, Some(key(1)));
    // The block's `← close` is a target too: a click closes the row.
    let close = state
        .hits
        .task_board
        .footer_keys
        .iter()
        .find(|(_, key)| *key == "←")
        .expect("close is a target")
        .0;
    board_click(&mut state, close);
    assert_eq!(state.task_board.open_row, None);
}

/// Task 9's action in the fixture, made as long as its text.
const LONG_ACTION: &str =
    "wait for the next new client to sign, then book a sandbox tenant and a free afternoon";

/// The fixture board with task 9 waiting on `LONG_ACTION`.
fn long_action_board() -> TaskBoard {
    let mut board = fixture_board();
    let list = LIST_V1.replace("\"next new client\"", &format!("{LONG_ACTION:?}"));
    board.store.receive_json(&list, std::time::Instant::now());
    board
}

#[test]
fn an_open_task_with_a_space_and_no_project_shows_its_space_once() {
    // With no project the row leads with the space, so the block needs no `Where`.
    let mut board = fixture_board();
    let list = LIST_V1.replace("\"project\": \"onboard\"", "\"project\": \"\"");
    board.store.receive_json(&list, std::time::Instant::now());
    board.selected = Some(key(9));
    board.open_row = board.selected.clone();
    for width in [186, 89, NARROW, 50] {
        let render = render_board(&board, width, 80);
        let lines = lines_of(&render, &key(9));
        let all = lines.join("\n");
        // The file path holds `oakwood-workspace`; the space shows once apart from it.
        let space_lines = lines
            .iter()
            .filter(|line| !line.contains("File") && line.contains("oakwood"))
            .count();
        assert_eq!(space_lines, 1, "{width}: {lines:#?}");
        // From 80 wide in the Project column, narrower before the task, dim.
        let leads = if width >= 80 {
            lines[0].contains("│ oakwood ") && lines[0].contains("│ Run")
        } else {
            lines[0].contains("oakwood · Run")
        };
        assert!(leads, "{width}: {lines:#?}");
        assert!(!all.contains("Where"), "{width}: {lines:#?}");
    }
}

#[test]
fn an_open_row_split_inside_a_word_wider_than_its_column_keeps_each_pieces_colour() {
    // The project is one word wider than any Task column, so where it leads the task (below
    // 80 wide) the wrap splits it.
    let long = "Q".repeat(140);
    let mut board = fixture_board();
    let list = LIST_V1.replace(
        "\"project\": \"onboard\"",
        &format!("\"project\": \"{long}\""),
    );
    board.store.receive_json(&list, std::time::Instant::now());
    board.selected = Some(key(9));
    board.open_row = board.selected.clone();
    for width in [186, 89, NARROW, 50] {
        let render = render_board(&board, width, 80);
        let rect = render
            .hits
            .rows
            .iter()
            .find(|(_, target)| *target == key(9))
            .expect("row 9")
            .0;
        let mut split = 0;
        let mut run = None;
        // The row's own lines: its block starts at the project it cuts, else its file.
        let own = (rect.y..rect.bottom()).take_while(|y| {
            let line = &render.rows[usize::from(*y)];
            !line.contains("Project  ") && !line.contains("File  ")
        });
        for y in own {
            let cells = (0..width).map(|x| &render.buffer[(x, y)]);
            // The Project column is grey; folded before the task, a dim lead.
            let colour = if width < 80 {
                render.palette.overlay0
            } else {
                render.palette.overlay1
            };
            let kept = cells
                .filter(|cell| cell.symbol() == "Q")
                .all(|cell| cell.fg == colour);
            assert!(kept, "{width}: row {y} {:?}", render.rows[usize::from(y)]);
            split += usize::from(render.rows[usize::from(y)].contains('Q'));
            if run.is_none() && render.rows[usize::from(y)].contains(" Run ") {
                run = Some(usize::from(y));
            }
        }
        let run = run.unwrap_or_else(|| panic!("{width}: {:#?}", render.rows));
        assert_eq!(render.cell(run, "Run").fg, render.palette.text, "{width}");
        if width >= 80 {
            // In its own Project column the project is cut to one line, whole in the block.
            assert_eq!(split, 1, "{width}: cut, not split: {:#?}", render.rows);
            continue;
        }
        assert!(split > 1, "{width}: the word splits: {:#?}", render.rows);
        assert_eq!(
            render.cell(run, "· Run").fg,
            render.palette.overlay0,
            "{width}"
        );
    }
}

/// The columns of `row`'s lines, `│`, on screen.
fn bar_columns(row: &str) -> Vec<usize> {
    let mut column = 0;
    let mut bars = Vec::new();
    for ch in row.chars() {
        if ch == '│' {
            bars.push(column);
        }
        column += unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
    }
    bars
}

/// The lines `target`'s row and its block take in `render`.
fn lines_of<'a>(render: &'a BoardRender, target: &RowKey) -> &'a [String] {
    let rect = render
        .hits
        .rows
        .iter()
        .find(|(_, key)| key == target)
        .unwrap_or_else(|| panic!("{target:?} not drawn"))
        .0;
    &render.rows[usize::from(rect.y)..usize::from(rect.bottom())]
}

#[test]
fn an_open_row_wraps_its_task_and_action_in_its_own_cells_and_never_repeats_its_task() {
    let list = parse_task_list(LIST_V1).expect("fixture");
    let task = &list.tasks[8].task;
    for width in [186, 89, NARROW, 50] {
        let mut board = long_action_board();
        board.selected = Some(key(9));
        board.open_row = board.selected.clone();
        let render = render_board(&board, width, 80);
        let lines = lines_of(&render, &key(9));
        // In a grid the row's own lines keep its cells' lines; stacked, only the borders. Its
        // QUEUED block starts with its file.
        let cells = bar_columns(&lines[0]);
        let row_lines = lines
            .iter()
            .take_while(|line| bar_columns(line) == cells && !line.contains("File "))
            .count();
        let row = lines[..row_lines].join(" ");
        assert!(row_lines > 1, "{width}: the row wraps: {lines:#?}");
        for word in task
            .split_whitespace()
            .chain(LONG_ACTION.split_whitespace())
        {
            assert!(
                row.contains(word),
                "{width}: {word:?} in its row: {lines:#?}"
            );
        }
        let all = lines.join(" ");
        assert!(!all.contains('…'), "{width}: nothing cut: {lines:#?}");
        assert_eq!(
            all.matches("client-onboard").count(),
            1,
            "{width}: the task once: {lines:#?}"
        );
        // The action once: its row's cell wraps it whole, so the block leaves it out.
        assert_eq!(all.matches("afternoon").count(), 1, "{width}: {lines:#?}");
        for label in ["Task ", "Waiting on "] {
            assert!(!all.contains(label), "{width}: no {label:?}: {lines:#?}");
        }
        for line in lines {
            assert!(
                line.starts_with(" │") && line.ends_with('│'),
                "{width}: inside the box: {line:?}"
            );
        }
        // QUEUED's block leaves the next step to its row and holds its file, then its keys
        // and close; the footer leaves them out.
        let block = &lines[row_lines..];
        assert!(
            block[0].contains("File "),
            "{width}: the block starts with it: {block:#?}"
        );
        assert!(
            block.iter().any(|line| line.contains("Enter  go to space")),
            "{width}: {block:#?}"
        );
        let footer = &render.rows[render.rows.len() - 1];
        assert!(
            !footer.contains("Enter  go to space"),
            "{width}: {footer:?}"
        );
    }
}

#[test]
fn an_open_rows_block_leaves_out_what_its_row_shows_and_may_hold_only_keys() {
    // Narrowed to its space, a completed row's project is all the place it needs.
    let mut board = fixture_board_in_section(9, "completed");
    board.done_expanded = true;
    board.space = Some("oakwood".into());
    board.selected = Some(key(9));
    board.open_row = board.selected.clone();
    let render = render_board(&board, 186, 60);
    let block = lines_of(&render, &key(9)).join("\n");
    assert!(!block.contains("Where"), "{block}");
    assert!(block.contains("File"), "{block}");

    // A lead's row, at the full width and narrower: its row shows what it works on, so its
    // block holds only its keys and close.
    let mut board = tasks_and_work_board();
    let agents = lead_and_workers();
    let lead = RowKey::Agent("w2:p1".into());
    board.selected = Some(lead.clone());
    board.open_row = board.selected.clone();
    for width in [186, NARROW] {
        let render = render_board_with_agents(&board, width, 46, &agents);
        let lines = lines_of(&render, &lead);
        let last = lines.last().expect("its lines");
        assert!(last.contains("Enter  go to agent"), "{width}: {lines:#?}");
        for left_out in ["Space", "Lead", "Task", "Card"] {
            assert!(
                !lines.iter().any(|line| line.contains(left_out)),
                "{width}: no {left_out:?}: {lines:#?}"
            );
        }
    }

    // A worker's block: the lead it works for; its task, what it does and for how long are
    // its row's, and no card links its task.
    let worker = RowKey::Agent("w2:p2".into());
    board.selected = Some(worker.clone());
    board.open_row = board.selected.clone();
    let render = render_board_with_agents(&board, 186, 46, &agents);
    let block = lines_of(&render, &worker)[1..].join("\n");
    assert!(
        block.contains("Lead") && block.contains("herdr-lead"),
        "{block}"
    );
    for left_out in ["Doing", "For ", "Task", "Card", "Space", "t  show its task"] {
        assert!(!block.contains(left_out), "no {left_out:?} in {block}");
    }

    // A tab waiting on you: its ask is its row's, so its block is its keys alone.
    let mut board = fixture_board();
    let asking = BoardAgent {
        state: AgentState::Waiting(Some("Which sandbox tenant should the run use?".into())),
        ..working("w9:p1", "oakwood", "oak-ask", None)
    };
    let tab = RowKey::Agent("w9:p1".into());
    board.selected = Some(tab.clone());
    board.open_row = board.selected.clone();
    for space in [None, Some("oakwood".to_owned())] {
        board.space = space;
        let render = render_board_with_agents(&board, 186, 60, std::slice::from_ref(&asking));
        let lines = lines_of(&render, &tab);
        assert_eq!(lines.len(), 2, "keys alone: {lines:#?}");
        assert!(!lines[1].contains("Ask"), "{lines:#?}");
    }
}

#[test]
fn task_board_close_and_wheel_work_by_mouse() {
    let mut state = board_state();
    state.task_board.open = true;
    state.compose(106, 16).expect("frame");
    assert!(state.hits.task_board.max_scroll > 0);
    let area = state.hits.task_board.area;
    state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: area.x + 10,
        row: area.y + 5,
        modifiers: KeyModifiers::empty(),
    })]);
    assert!(state.task_board.scroll > 0);
    state.compose(106, 16).expect("frame");
    let close = state.hits.task_board.close;
    board_click(&mut state, close);
    assert!(!state.task_board.open);
}

fn oakwood_state() -> ClientShellState {
    let mut state = board_state();
    let mut projected = snapshot();
    projected.workspaces[0].label = "oakwood".into();
    projected.workspaces[0].custom_label = true;
    state.set_snapshot(Box::new(projected));
    state
}

#[test]
fn clicking_a_space_while_the_board_is_open_shows_only_its_tasks() {
    let mut state = oakwood_state();
    state.task_board.open = true;
    state.compose(106, 30).expect("frame");
    let space = state.hits.workspaces[0].rect;
    let outcome = board_click(&mut state, space);
    assert_eq!(state.task_board.space.as_deref(), Some("oakwood"));
    assert!(outcome.actions.is_empty(), "the board narrows; focus stays");
    assert!(state.task_board.open);
    state.compose(106, 30).expect("frame");
    let back = state.hits.task_board.back_to_all;
    board_click(&mut state, back);
    assert_eq!(state.task_board.space, None);

    state.task_board.open = false;
    state.compose(106, 30).expect("frame");
    let space = state.hits.workspaces[0].rect;
    board_click(&mut state, space);
    assert_eq!(
        state.task_board.space, None,
        "a closed board is not narrowed"
    );
}

#[test]
fn clicking_a_one_shell_space_row_while_the_board_is_open_narrows_it_and_keeps_focus() {
    let mut state = oakwood_state();
    state.task_board.open = true;
    state.compose(106, 30).expect("frame");
    let (row, _) = state
        .hits
        .tree_space_shells
        .first()
        .cloned()
        .expect("a one-shell space row stands for its pane");
    // Right of the name, away from the fold arrow and the `+`.
    let point = Rect::new(row.x + row.width / 2, row.y, 1, 1);
    let outcome = board_click(&mut state, point);
    assert_eq!(state.task_board.space.as_deref(), Some("oakwood"));
    assert!(
        outcome.actions.is_empty(),
        "the board narrows; no pane is focused"
    );
    assert!(state.task_board.open);
}

#[test]
fn clicking_a_one_agent_space_row_while_the_board_is_open_narrows_it_and_keeps_focus() {
    // oakwood's one pane is an agent: its space row stands for that agent's pane.
    let mut state = oakwood_state();
    let mut snap = state.snapshot.as_deref().cloned().expect("snapshot");
    let (pane_id, workspace_id) = (
        snap.panes[0].pane_id.clone(),
        snap.panes[0].workspace_id.clone(),
    );
    snap.agents = vec![live_agent(
        &pane_id,
        &workspace_id,
        "oak-lead",
        crate::api::schema::AgentStatus::Idle,
        &[],
    )];
    state.set_snapshot(Box::new(snap));
    state.task_board.open = true;
    state.compose(106, 30).expect("frame");
    let (row, _) = state
        .hits
        .tree_space_agents
        .first()
        .cloned()
        .expect("a one-agent space row stands for its pane");
    let point = Rect::new(row.x + row.width / 2, row.y, 1, 1);
    let outcome = board_click(&mut state, point);
    assert_eq!(state.task_board.space.as_deref(), Some("oakwood"));
    assert!(
        outcome.actions.is_empty(),
        "the board narrows; no pane is focused"
    );
    assert!(state.task_board.open);
}

#[test]
fn clicking_a_pane_in_the_tree_while_the_board_is_open_closes_it() {
    let mut state = board_state();
    // A second tab: the space row stands for the first tab's pane, so this is a tree row.
    let mut projected = snapshot();
    let mut tab = projected.tabs[0].clone();
    tab.tab_id = "tab_2".into();
    tab.number = 2;
    tab.focused = false;
    let mut pane = projected.panes[0].clone();
    pane.pane_id = "pane_2".into();
    pane.tab_id = "tab_2".into();
    pane.focused = false;
    projected.tabs.push(tab);
    projected.panes.push(pane);
    state.set_snapshot(Box::new(projected));
    state.task_board.open = true;
    state.compose(106, 30).expect("frame");
    let (row, _) = state.hits.tree_rows.first().cloned().expect("a tree row");
    let outcome = board_click(&mut state, row);
    assert!(!outcome.actions.is_empty(), "the row still focuses");
    assert!(!state.task_board.open, "so the focused pane shows");
}

#[test]
fn tree_header_board_icon_is_yellow_only_while_the_board_is_open() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&{
        let mut config = Config::default();
        config.ui.sidebar_tree = true;
        config
    }));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    let palette = state.config.palette.clone();
    let icon = |state: &mut ClientShellState| {
        let frame = state.compose(106, 30).expect("frame");
        let rows = frame_rows(&frame);
        let cell = state.hits.tree_task_board;
        assert_eq!(cell.width, 1, "drawn");
        let symbol: String = rows[cell.y as usize]
            .chars()
            .nth(cell.x as usize)
            .into_iter()
            .collect();
        assert_eq!(symbol, "☑");
        frame.to_ratatui_buffer().expect("buffer")[(cell.x, cell.y)].clone()
    };
    let grey = icon(&mut state);
    assert_eq!(grey.fg, palette.overlay1, "grey until a list is read");

    state
        .task_board
        .store
        .receive_json(LIST_V1, std::time::Instant::now());
    let closed = icon(&mut state);
    assert_eq!(
        closed.fg, palette.overlay1,
        "grey while closed, even with a `you` task: closed, the list is not read"
    );

    let cell = state.hits.tree_task_board;
    board_click(&mut state, cell);
    assert!(state.task_board.open, "the icon opens the board");
    let open = icon(&mut state);
    assert_eq!(open.fg, palette.yellow, "yellow while open");
    assert!(!open.modifier.contains(Modifier::BOLD));
    assert_eq!(open.bg, grey.bg, "no cell fill, like the other icons");
    let cell = state.hits.tree_task_board;
    board_click(&mut state, cell);
    assert!(!state.task_board.open, "and closes it");
    let closed = icon(&mut state);
    assert_eq!(closed.fg, palette.overlay1, "grey again once closed");
    assert!(!closed.modifier.contains(Modifier::BOLD));
}

/// The `tasks.list` requests a timer tick at `now` sends beside the command lane.
fn tick_asks(state: &mut ClientShellState, now: std::time::Instant) -> Vec<String> {
    let mut outcome = ClientShellInput::default();
    state.tick_task_board(now, &mut outcome);
    outcome
        .actions
        .iter()
        .filter_map(|action| match action {
            ClientShellAction::EndpointSide { request, .. } => {
                assert!(
                    matches!(request.method, crate::api::schema::Method::TasksList(_)),
                    "only tasks.list goes beside the lane"
                );
                Some(request.id.clone())
            }
            ClientShellAction::Endpoint { .. } => panic!("tasks.list never joins the lane"),
            _ => None,
        })
        .collect()
}

fn empty_board_state() -> ClientShellState {
    let mut config = Config::default();
    config.ui.sidebar_tree = true;
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&config));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state
}

fn fixture_reply() -> Result<crate::api::schema::ResponseResult, ClientShellEndpointError> {
    Ok(crate::api::schema::ResponseResult::TasksList {
        list: serde_json::from_str(LIST_V1).expect("fixture json"),
    })
}

#[test]
fn task_board_asks_for_tasks_only_while_open_one_at_a_time_every_two_seconds() {
    let mut state = empty_board_state();
    let start = std::time::Instant::now();
    assert!(
        tick_asks(&mut state, start).is_empty(),
        "closed: nothing asked"
    );

    state.task_board.open = true;
    let first = tick_asks(&mut state, start);
    assert_eq!(first.len(), 1, "open: asked at once");
    let second = std::time::Duration::from_secs(1);
    assert!(
        tick_asks(&mut state, start + second * 3).is_empty(),
        "one request in flight, however long it takes"
    );
    let (repaint, actions) = state.handle_endpoint_result("boot-1", &first[0], fixture_reply());
    assert!(repaint && actions.is_empty());
    assert_eq!(
        state.task_board.store.list().map(|list| list.tasks.len()),
        Some(13)
    );
    let mut state = empty_board_state();
    state.task_board.open = true;
    let first = tick_asks(&mut state, start);
    state.handle_endpoint_result("boot-1", &first[0], fixture_reply());
    let almost = start + second * 2 - std::time::Duration::from_millis(1);
    assert!(
        tick_asks(&mut state, almost).is_empty(),
        "every 2 s from the last ask"
    );
    let next = start + second * 2;
    assert_eq!(tick_asks(&mut state, next).len(), 1, "asked again");

    state.task_board.open = false;
    let (_, _) = state.handle_endpoint_result("boot-1", "unknown", fixture_reply());
    assert!(
        tick_asks(&mut state, next + second * 10).is_empty(),
        "closed again"
    );
}

#[test]
fn task_board_shows_the_servers_tasks_or_why_it_has_none() {
    let mut state = empty_board_state();
    state.task_board.open = true;
    let asked = tick_asks(&mut state, std::time::Instant::now());
    state.handle_endpoint_result("boot-1", &asked[0], fixture_reply());
    let rows = frame_rows(&state.compose(140, 40).expect("frame"));
    assert!(
        rows.iter().any(|row| row.contains("Per-client Chrome")),
        "{rows:#?}"
    );

    let mut state = empty_board_state();
    state.task_board.open = true;
    let asked = tick_asks(&mut state, std::time::Instant::now());
    state.handle_endpoint_result(
        "boot-1",
        &asked[0],
        Err(ClientShellEndpointError {
            code: Some("tasks_unavailable".into()),
            message: "tasks.command is not set".into(),
        }),
    );
    let rows = frame_rows(&state.compose(140, 40).expect("frame"));
    assert!(
        rows.iter()
            .any(|row| row.contains("Tasks unavailable: tasks.command is not set")),
        "{rows:#?}"
    );
}

#[test]
fn task_board_never_asks_a_server_that_does_not_advertise_tasks_list() {
    let mut state = empty_board_state();
    state.set_endpoint_methods(Some(vec!["pane.focus".into()]));
    state.task_board.open = true;
    let now = std::time::Instant::now();
    assert!(tick_asks(&mut state, now).is_empty());
    assert!(
        state
            .task_board
            .store
            .error()
            .is_some_and(|error| error.contains("does not support tasks.list")),
        "{:?}",
        state.task_board.store.error()
    );
    assert!(tick_asks(&mut state, now + std::time::Duration::from_secs(60)).is_empty());
}

#[test]
fn task_board_cancelled_ask_is_asked_again_and_a_new_endpoint_starts_empty() {
    let mut state = empty_board_state();
    state.task_board.open = true;
    let now = std::time::Instant::now();
    let asked = tick_asks(&mut state, now);
    state.cancel_endpoint_request(&asked[0]);
    let later = now + std::time::Duration::from_millis(100);
    let again = tick_asks(&mut state, later);
    assert_eq!(
        again.len(),
        1,
        "a cancelled or unsent ask is asked again on the next tick"
    );
    state.handle_endpoint_result("boot-1", &again[0], fixture_reply());
    assert!(state.task_board.store.list().is_some());

    state.reset_endpoint_projection();
    assert!(
        state.task_board.store.list().is_none(),
        "another server's tasks"
    );
    assert!(state.task_board.store.error().is_none());
}

const WORK_V1: &str = include_str!("../../../../tests/fixtures/task-board/work-v1.json");

fn work_board() -> TaskBoard {
    let mut board = TaskBoard::default();
    board.store.receive_json(WORK_V1, std::time::Instant::now());
    board.open = true;
    board.view = BoardView::Work;
    board.can_act = true;
    board.can_open = true;
    board
}

/// A card's identity: its intent file, here by its name.
fn card(name: &str) -> String {
    let list = parse_task_list(WORK_V1).expect("work fixture");
    list.work
        .iter()
        .find(|card| card.file.ends_with(name))
        .unwrap_or_else(|| panic!("{name}"))
        .file
        .clone()
}

const WORK_PHASES: &str = include_str!("../../../../tests/fixtures/task-board/work-v1-phases.json");

fn phases_board() -> TaskBoard {
    let mut board = work_board();
    board
        .store
        .receive_json(WORK_PHASES, std::time::Instant::now());
    board
}

/// A card of the phases fixture, by its name.
fn phases_card(name: &str) -> String {
    format!("/home/user/devbox/projects/repos/herdr/docs/intent/2026-09-29-{name}.md")
}

#[test]
fn work_card_fields_default_when_the_reader_sends_none() {
    let list = parse_task_list(WORK_V1).expect("work fixture");
    for card in &list.work {
        assert_eq!(card.objective, None, "{}", card.file);
        assert_eq!(card.approval, None);
        assert_eq!(
            (card.due.as_deref(), card.due_text.as_deref()),
            (None, None)
        );
        assert_eq!(card.due_level, None);
        assert_eq!(
            (card.started.as_deref(), card.phase.as_deref()),
            (None, None)
        );
        assert_eq!(card.slug, None);
        assert!(card.findings.is_empty());
        assert_eq!(
            (card.verification.as_deref(), card.release.as_deref()),
            (None, None)
        );
        assert_eq!(card.last_activity, None);
        assert!(card.steps.iter().all(|step| step.code.is_none()));
    }
}

#[test]
fn work_card_fields_read_from_the_phases_fixture() {
    let list = parse_task_list(WORK_PHASES).expect("phases fixture");
    let by = |name: &str| {
        let file = phases_card(name);
        list.work
            .iter()
            .find(|card| card.file == file)
            .unwrap_or_else(|| panic!("{name}"))
    };
    let review = by("review-open");
    assert_eq!(
        review.objective.as_deref(),
        Some("One board for tasks and phases, readable at a glance on a laptop screen.")
    );
    assert_eq!(review.phase.as_deref(), Some("review"));
    assert_eq!(review.slug.as_deref(), Some("2026-09-29-review-open"));
    assert_eq!(
        (review.started.as_deref(), review.last_activity.as_deref()),
        (Some("2026-09-26"), Some("2026-09-29"))
    );
    assert_eq!(
        (review.due.as_deref(), review.due_text.as_deref()),
        (Some("2026-10-01"), Some("Thu"))
    );
    assert_eq!(review.due_level, Some(DueLevel::Soon));
    assert_eq!(review.findings.len(), 4);
    assert_eq!(review.findings[1].severity.as_deref(), Some("HIGH"));
    assert_eq!(review.findings[1].text, "Panel cuts long titles.");
    assert!(review.findings[1].open && !review.findings[0].open);
    assert_eq!(review.findings[2].severity, None);
    assert_eq!(
        by("plan-approved").approval.as_deref(),
        Some("approved 2026-09-28")
    );
    assert_eq!(
        by("verify-done").verification.as_deref(),
        Some("Ran the suite.\nAll 12 green.")
    );
    assert_eq!(by("ship").release.as_deref(), Some("PR #12 merged."));
    assert!(by("review-none").findings.is_empty());
    let overdue = by("build-overdue");
    assert_eq!(overdue.due_level, Some(DueLevel::Overdue));
    let codes: Vec<_> = overdue
        .steps
        .iter()
        .map(|step| step.code.as_deref())
        .collect();
    assert_eq!(codes, [Some("B2a"), Some("B2b"), None]);
    assert_eq!(overdue.stage, WorkStage::Build);
}

#[test]
fn work_card_fields_read_null_as_none() {
    let list = parse_task_list(WORK_PHASES).expect("phases fixture");
    let card = list
        .work
        .iter()
        .find(|card| card.file == phases_card("all-null"))
        .expect("all-null card");
    assert_eq!(card.objective, None);
    assert_eq!(
        (card.due_text.as_deref(), card.slug.as_deref()),
        (None, None)
    );
    assert_eq!(card.due_level, None);
    assert!(card.findings.is_empty(), "findings: null reads as none");
    assert_eq!(card.steps[0].code, None);
    assert_eq!(card.steps[0].text, "No code here.");
}

#[test]
fn phase_status_line_says_each_phase_state() {
    let render = render_board(&phases_board(), 186, 90);
    let status = |name: &str| {
        card_text(&render, &phases_card(name))
            .get(6)
            .cloned()
            .unwrap_or_default()
    };
    for (name, words) in [
        ("plan-pending", "? Awaiting your approval"),
        ("plan-approved", "✓ Approved"),
        ("build-on-track", "· On track"),
        ("all-null", "· On track"),
        ("review-open", "· 2 open findings"),
        ("review-passed", "✓ Review passed"),
        ("review-none", "· Awaiting review"),
        ("verify-done", "✓ Verified"),
        ("verify-waiting", "? Awaiting your check"),
        ("ship", "✓ Shipped 28 Sep"),
    ] {
        assert_eq!(status(name), words, "{name}: {:#?}", render.rows);
    }
    let one_open = work_board_with(|list| {
        for card in &mut list.work {
            if card.title == "Herdr task board" {
                card.stage = WorkStage::Review;
                card.findings = vec![crate::client::shell::task_board::model::WorkFinding {
                    severity: None,
                    text: "No severity word.".into(),
                    open: true,
                }];
            }
        }
    });
    let render = render_board(&one_open, 186, 90);
    assert_eq!(
        card_text(&render, &card("2026-09-25-task-board.md"))[6],
        "· 1 open finding"
    );
}

#[test]
fn only_an_approval_starting_with_approved_reads_approved() {
    for (approval, words) in [
        (None, "? Awaiting your approval"),
        (Some("pending"), "? Awaiting your approval"),
        (Some("rejected"), "? Awaiting your approval"),
        (Some("not approved"), "? Awaiting your approval"),
        (Some("Approved 2026-09-28"), "✓ Approved"),
        (Some("approved 2026-09-28"), "✓ Approved"),
    ] {
        let board = work_board_with(|list| {
            for card in &mut list.work {
                if card.file.ends_with("2026-09-25-lead-name.md") {
                    card.approval = approval.map(str::to_owned);
                }
            }
        });
        let render = render_board(&board, 186, 90);
        assert_eq!(
            card_text(&render, &card("2026-09-25-lead-name.md"))[6],
            words,
            "{approval:?}"
        );
    }
}

#[test]
fn a_cards_status_from_the_notes_file_draws_without_its_control_characters() {
    let board = phases_board_with(|list| {
        for card in &mut list.work {
            if card.file == phases_card("build-overdue") {
                card.due_text = Some("Overdue\u{7} 3d\u{1b}[31m".into());
            }
        }
    });
    let render = render_board(&board, 186, 90);
    assert_eq!(
        card_text(&render, &phases_card("build-overdue"))[6],
        "! Overdue 3d[31m"
    );
    let mut board = board;
    board.work_selected = Some(phases_card("build-overdue"));
    let render = render_board(&board, 186, 46);
    assert!(
        render
            .buffer
            .content()
            .iter()
            .all(|cell| !cell.symbol().chars().any(char::is_control)),
        "a control character reached the buffer"
    );
    render.row_with("Overdue 3d[31m");
}

#[test]
fn build_card_due_line_is_red_overdue_and_yellow_today_never_bold() {
    let render = render_board(&phases_board(), 186, 90);
    let line = |name: &str| {
        let (rect, _) = render
            .hits
            .cards
            .iter()
            .find(|(_, card)| *card == phases_card(name))
            .unwrap_or_else(|| panic!("{name} drawn"));
        usize::from(rect.y + 7)
    };
    let overdue = line("build-overdue");
    assert!(
        render.rows[overdue].contains("Overdue 3d"),
        "{:?}",
        render.rows[overdue]
    );
    assert_drawn(
        &render,
        (overdue, "! Overdue 3d"),
        render.palette.red,
        false,
    );
    let today = line("build-today");
    assert!(
        render.rows[today].contains("Due today"),
        "{:?}",
        render.rows[today]
    );
    assert_drawn(
        &render,
        (today, "! Due today"),
        render.palette.yellow,
        false,
    );
    assert_drawn(
        &render,
        (line("build-on-track"), "· On track"),
        render.palette.overlay1,
        false,
    );
    let soon = work_board_with(|list| {
        for card in &mut list.work {
            if card.title == "Herdr task board" {
                card.due_text = Some("Wed".into());
                card.due_level = Some(DueLevel::Soon);
            }
        }
    });
    let render = render_board(&soon, 186, 90);
    assert_eq!(
        card_text(&render, &card("2026-09-25-task-board.md"))[6],
        "· Due Wed"
    );
}

#[test]
fn phase_status_line_is_cut_at_a_narrow_card() {
    // Five columns 14 wide: a card's inside is 10 columns.
    let render = render_board(&phases_board(), 80, 90);
    let words = &card_text(&render, &phases_card("verify-waiting"))[6];
    assert!(
        words.starts_with("? Awaitin") && words.ends_with('…'),
        "{words:?}"
    );
}

/// The open panel of the phases fixture's card `name`, `width` wide and 80 high: each row's
/// screen row and its words inside the box, under the heading's rule.
fn phases_panel(name: &str, width: u16) -> (BoardRender, Vec<(usize, String)>) {
    let mut board = phases_board();
    board.work_selected = Some(phases_card(name));
    let render = render_board(&board, width, 80);
    let panel = render.hits.work_panel;
    let inside = (panel.y + 3..panel.bottom() - 1)
        .map(|y| {
            let words: String = (panel.x + 2..panel.right() - 2)
                .map(|x| render.buffer[(x, y)].symbol().to_owned())
                .collect();
            (usize::from(y), words.trim_end().to_owned())
        })
        .collect();
    (render, inside)
}

fn panel_words(inside: &[(usize, String)]) -> Vec<&str> {
    inside.iter().map(|(_, words)| words.as_str()).collect()
}

#[test]
fn review_card_panel_shows_its_fields_in_order() {
    let (_, inside) = phases_panel("review-open", 186);
    let words = panel_words(&inside);
    assert_eq!(
        words[..17],
        [
            "Objective     One board for tasks and phases, readable at a glance on a laptop screen.",
            "Status        · 2 open findings",
            "Owner         You",
            "Blocked by    You · pick a time",
            "Findings      HIGH Panel cuts long titles.",
            "              MEDIUM Timeline drops the year.",
            "              LOW Old spacing nit.",
            "              A finding with no severity.",
            "Timeline      started 26 Sep · due Thu · last activity 29 Sep",
            "",
            "STEPS  2 of 2 done",
            "✓ Board columns.",
            "✓ Panel fields.",
            "",
            "LINKED TASKS  3",
            "#1  choose the phases colours (you)",
            "#2  update the format doc (claude)",
        ],
        "{words:#?}"
    );
    assert_eq!(words[17], "·  an old done one (done)", "{words:#?}");
    let foot = &words[words.len() - 4..words.len() - 1];
    assert_eq!(
        foot,
        [
            "Spec          ~/projects/repos/herdr/docs/intent/2026-09-29-review-open.md",
            "Branch        feat/phases · 9 commits · last commit Sep 28",
            "",
        ],
        "{words:#?}"
    );
}

#[test]
fn review_panel_findings_open_first_coloured_by_severity() {
    let (render, inside) = phases_panel("review-open", 186);
    let row = |needle: &str| {
        inside
            .iter()
            .find(|(_, words)| words.contains(needle))
            .unwrap_or_else(|| panic!("{needle}: {inside:#?}"))
            .0
    };
    let palette = &render.palette;
    let high = row("HIGH Panel");
    assert_eq!(render.cell(high, "HIGH").fg, palette.red);
    assert_eq!(render.cell(high, "Panel cuts").fg, palette.text);
    // The reader's lower-case `medium` draws upper case, peach: a warning, not a wait on you.
    let medium = row("MEDIUM Timeline");
    assert_eq!(render.cell(medium, "MEDIUM").fg, palette.peach);
    // Fixed ones after the open ones, dim, their severity too.
    let low = row("LOW Old");
    assert_eq!(render.cell(low, "LOW").fg, palette.overlay0);
    assert_eq!(render.cell(low, "Old spacing").fg, palette.overlay0);
    let none = row("A finding with no severity.");
    assert_eq!(render.cell(none, "A finding").fg, palette.overlay0);
    assert!(high < medium && medium < low && low < none);
    assert_eq!(render.cell(row("Status"), "2 open").fg, palette.overlay1);
}

#[test]
fn a_finding_with_no_words_keeps_its_severity_and_label() {
    let mut board = work_board_with(|list| {
        for card in &mut list.work {
            if card.title == "Tree sidebar" {
                card.findings = vec![crate::client::shell::task_board::model::WorkFinding {
                    severity: Some("HIGH".into()),
                    text: String::new(),
                    open: true,
                }];
            }
        }
    });
    board.work_selected = Some(card("2026-09-24-tree-sidebar.md"));
    let render = render_board(&board, 186, 60);
    let row = render.row_with("│ Status        · 1 open finding");
    assert!(
        render.rows[row + 1].contains("│ Owner         herdr")
            && render.rows[row + 2].contains("│ Findings      HIGH"),
        "{:#?}",
        &render.rows[row..row + 3]
    );
}

#[test]
fn build_card_panel_shows_current_step_and_no_step_codes() {
    let (render, inside) = phases_panel("build-overdue", 186);
    let words = panel_words(&inside);
    assert_eq!(
        words[..11],
        [
            "Objective     Draw each card's phase status and a details panel.",
            "Status        ! Overdue 3d",
            "Owner         Carmen",
            "Current step  Phases view: card status line and the details panel from card fields.",
            "Timeline      started 26 Sep · overdue 3d · last activity 29 Sep",
            "",
            "STEPS  1 of 3 done",
            "✓ Phases view columns.",
            "▸ Phases view: card status line and the details panel from card fields.",
            "○ Tidy the docs.",
            "",
        ],
        "{words:#?}"
    );
    let status = inside[1].0;
    assert_eq!(render.cell(status, "Overdue").fg, render.palette.red);
    let current = inside[3].0;
    assert_eq!(render.cell(current, "Phases").fg, render.palette.text);
    // The current step is the first one not done, its title bold; no code is drawn.
    let step = inside[8].0;
    assert_drawn(
        &render,
        (
            step,
            "Phases view: card status line and the details panel from card fields.",
        ),
        render.palette.text,
        true,
    );
    for (_, words) in &inside {
        assert!(
            !words.contains("B2b") && !words.contains("B2a"),
            "{inside:#?}"
        );
    }
    // No one who must act is You: nothing blocks it.
    assert!(!words.iter().any(|line| line.starts_with("Blocked by")));
    // The working agent owns a card, before any task's owner.
    let (_, today) = phases_panel("build-today", 186);
    assert!(
        panel_words(&today).contains(&"Owner         herdr-w1"),
        "{today:#?}"
    );
}

/// Asserts every cell of `needle` on row `y` is drawn in `fg`, bold or not.
fn assert_drawn(
    render: &BoardRender,
    (y, needle): (usize, &str),
    fg: ratatui::style::Color,
    bold: bool,
) {
    let start = render.column(y, needle);
    for x in start..start + UnicodeWidthStr::width(needle) as u16 {
        let cell = &render.buffer[(x, y as u16)];
        assert_eq!(
            (cell.fg, cell.modifier.contains(Modifier::BOLD)),
            (fg, bold),
            "{needle:?} at column {x}: {:?}",
            render.rows[y]
        );
    }
}

/// A panel's label: grey, not bold, as a column's name.
fn assert_label(render: &BoardRender, y: usize, needle: &str) {
    assert_drawn(render, (y, needle), render.palette.overlay1, false);
}

/// A panel's value: plain text.
fn assert_value(render: &BoardRender, y: usize, needle: &str) {
    assert_drawn(render, (y, needle), render.palette.text, false);
}

#[test]
fn the_details_panel_draws_labels_grey_and_values_in_text() {
    let (render, inside) = phases_panel("build-overdue", WIDE);
    let row = |needle: &str| {
        inside
            .iter()
            .find(|(_, words)| words.starts_with(needle))
            .unwrap_or_else(|| panic!("{needle}: {inside:#?}"))
            .0
    };
    for label in ["Objective", "Status", "Owner", "Current step", "Timeline"] {
        assert_label(&render, row(label), label);
    }
    assert_value(&render, row("Objective"), "Draw each card");
    // The owner is a who: grey, as a card's who line is.
    assert_drawn(
        &render,
        (row("Owner"), "Carmen"),
        render.palette.overlay1,
        false,
    );
    assert_value(&render, row("Timeline"), "started 26 Sep");
    let steps = row("STEPS");
    assert_label(&render, steps, "STEPS");
    assert_value(&render, steps, "1 of 3 done");
    let tasks = row("LINKED TASKS");
    assert_label(&render, tasks, "LINKED TASKS");
    assert_value(&render, tasks, "1");
    assert_value(&render, tasks + 1, "#");
    // The foot: its labels as the fields' are, the file and the branch in text.
    let spec = render.row_with("Spec          ~/");
    assert_label(&render, spec, "Spec");
    assert_value(&render, spec, "~/projects/repos/herdr");
    assert_label(&render, spec + 1, "Branch");
    assert_value(&render, spec + 1, "feat/task-board-phases · 4 commits");
    // Their values line up under the fields' values.
    let objective = row("Objective");
    assert_eq!(
        render.column(spec, "~/"),
        render.column(objective, "Draw each")
    );
    assert_eq!(
        render.column(spec + 1, "feat/"),
        render.column(objective, "Draw each")
    );
}

#[test]
fn a_done_step_reads_in_text_after_a_teal_tick_and_no_step_code_is_drawn() {
    let (render, inside) = phases_panel("build-overdue", WIDE);
    let row = |needle: &str| {
        inside
            .iter()
            .find(|(_, words)| words.contains(needle))
            .unwrap_or_else(|| panic!("{needle}: {inside:#?}"))
            .0
    };
    let done = row("✓ Phases view columns.");
    assert_drawn(&render, (done, "✓"), render.palette.teal, false);
    assert_value(&render, done, "Phases view columns.");
    let open = row("○ Tidy");
    assert_drawn(&render, (open, "○"), render.palette.text, false);
    assert_value(&render, open, "Tidy the docs.");
    for (_, words) in &inside {
        assert!(!words.contains("B2a"), "a step code is drawn: {inside:#?}");
    }
}

#[test]
fn a_panel_with_no_branch_says_its_commits_in_text() {
    let (render, _) = phases_panel("build-today", WIDE);
    let spec = render.row_with("Spec          ~/");
    assert!(
        render.rows[spec + 1].contains("│ 0 commits"),
        "{:?}",
        render.rows[spec + 1]
    );
    assert_value(&render, spec + 1, "0 commits");
}

#[test]
fn a_phase_status_reads_in_its_marks_colour_yellow_when_it_waits_on_you_green_once_passed_and_grey_when_neutral(
) {
    let render = render_board(&phases_board(), 186, 90);
    for (name, words, colour) in [
        (
            "plan-pending",
            "? Awaiting your approval",
            render.palette.yellow,
        ),
        (
            "verify-waiting",
            "? Awaiting your check",
            render.palette.yellow,
        ),
        ("review-open", "· 2 open findings", render.palette.overlay1),
        ("review-none", "· Awaiting review", render.palette.overlay1),
        ("plan-approved", "✓ Approved", render.palette.green),
        ("review-passed", "✓ Review passed", render.palette.green),
        ("verify-done", "✓ Verified", render.palette.green),
        ("ship", "✓ Shipped 28 Sep", render.palette.green),
        ("build-on-track", "· On track", render.palette.overlay1),
    ] {
        assert_card_words(&render, (name, 6), words, colour);
    }
    let (panel, inside) = phases_panel("verify-waiting", WIDE);
    let status = inside
        .iter()
        .find(|(_, words)| words.starts_with("Status"))
        .unwrap_or_else(|| panic!("{inside:#?}"))
        .0;
    assert_drawn(
        &panel,
        (status, "? Awaiting your check"),
        panel.palette.yellow,
        false,
    );
    let (panel, inside) = phases_panel("verify-done", WIDE);
    let status = inside
        .iter()
        .find(|(_, words)| words.starts_with("Status"))
        .unwrap_or_else(|| panic!("{inside:#?}"))
        .0;
    assert_drawn(&panel, (status, "✓ Verified"), panel.palette.green, false);
}

#[test]
fn a_card_reads_its_title_in_text_and_its_count_grey_under_its_one_muted_line() {
    let render = render_board(&phases_board(), 186, 90);
    let (rect, _) = render
        .hits
        .cards
        .iter()
        .find(|(_, card)| *card == phases_card("build-overdue"))
        .unwrap_or_else(|| panic!("{:#?}", render.rows));
    let top = usize::from(rect.y);
    // Line 1 is the muted one past its kind: the mark and word in the kind's colour (B5b),
    // the project grey.
    assert_drawn(&render, (top + 1, "◆ feature"), render.palette.mauve, false);
    assert_drawn(
        &render,
        (top + 1, "· herdr"),
        render.palette.overlay0,
        false,
    );
    assert_value(&render, top + 3, "build overdue");
    assert_drawn(&render, (top + 5, "1/3"), render.palette.overlay1, false);
    // Every kind's mark says its kind by shape and by the kind's colour.
    let render = render_board(&work_board(), WIDE, 40);
    for (mark, fg) in [
        ("◆ feature", render.palette.mauve),
        ("✱ bug", render.palette.red),
        ("▣ chore", render.palette.teal),
    ] {
        let row = render.row_with(mark);
        assert_drawn(&render, (row, mark), fg, false);
    }
}

#[test]
fn every_stage_heading_is_bold_in_its_stage_colour_on_a_tint_of_it() {
    let render = render_board(&phases_board(), 186, 90);
    let row = render.row_with(" PLAN ");
    let palette = render.palette.clone();
    let styles: Vec<_> = [
        ("PLAN", palette.mauve),
        ("BUILD", palette.yellow),
        (
            "REVIEW",
            crate::client::shell::task_board::style::rose(&palette),
        ),
        ("VERIFY", palette.blue),
        ("SHIP", palette.green),
    ]
    .into_iter()
    .map(|(heading, fg)| {
        let cell = render.cell(row, heading);
        assert_drawn(&render, (row, heading), fg, true);
        cell.bg
    })
    .collect();
    // Each label's fill is a tint of its own colour, so no two stages share one.
    let mut fills = styles.clone();
    fills.dedup();
    assert_eq!(fills.len(), 5, "a fill per stage: {styles:?}");
}

#[test]
fn a_working_cards_spinner_is_yellow_the_colour_for_working() {
    let render = render_board_with_agents(
        &work_board(),
        WIDE,
        40,
        &[working("w36:pV", "herdr", "herdr-w1", None)],
    );
    let who = render.row_with(&format!("{WORKER_SPINNER} herdr-w1"));
    assert_drawn(
        &render,
        (who, WORKER_SPINNER),
        working_colour(&render.palette),
        false,
    );
}

/// `phases_board` with its list changed by `change`.
fn phases_board_with(
    change: impl FnOnce(&mut crate::client::shell::task_board::model::TaskList),
) -> TaskBoard {
    let mut list = parse_task_list(WORK_PHASES).expect("phases fixture");
    change(&mut list);
    let mut board = work_board();
    board.store.receive(Ok(list), std::time::Instant::now());
    board
}

/// The row of a card's who line, its sixth (B5b: after the rule, the two title lines and the progress line).
fn who_row(render: &BoardRender, name: &str) -> usize {
    let (rect, _) = render
        .hits
        .cards
        .iter()
        .find(|(_, card)| *card == phases_card(name))
        .unwrap_or_else(|| panic!("{name} drawn: {:#?}", render.rows));
    usize::from(rect.y + 6)
}

#[test]
fn a_cards_who_line_is_its_agent_else_its_open_tasks_owner_else_its_spaces_lead_or_space() {
    // Carmen's task waits on the overdue build; the agent on today's build wins over it.
    let board = phases_board_with(|list| {
        list.tasks.push(list.tasks[4].clone());
        if let Some(task) = list.tasks.last_mut() {
            task.card = Some("2026-09-29-build-today".into());
        }
    });
    let render = render_board(&board, 186, 90);
    let who = |name: &str| card_text(&render, &phases_card(name))[5].clone();
    assert_eq!(who("build-overdue"), "Carmen");
    let grey = render.palette.overlay1;
    assert_drawn(
        &render,
        (who_row(&render, "build-overdue"), "Carmen"),
        grey,
        false,
    );
    assert_eq!(who("build-today"), "herdr-w1 · 12m");
    assert_drawn(
        &render,
        (who_row(&render, "build-today"), "herdr-w1 · 12m"),
        grey,
        false,
    );
    // No task and no agent: the space itself, never the branch or "No owner".
    assert_eq!(who("plan-pending"), "herdr");
    assert_drawn(
        &render,
        (who_row(&render, "plan-pending"), "herdr"),
        grey,
        false,
    );
    // A lead known in the space: the lead.
    let render = render_board_with_agents(
        &board,
        186,
        90,
        &[working("w1:lead", "herdr", "herdr-lead", None)],
    );
    assert_eq!(
        card_text(&render, &phases_card("plan-pending"))[5],
        "herdr-lead"
    );
    // A done task is not who has it: the open one beside it is.
    let board = phases_board_with(|list| {
        list.tasks[0].group = TaskGroup::Done;
        list.tasks[0].owner = Some("Morgan".into());
    });
    let render = render_board(&board, 186, 90);
    assert_eq!(
        card_text(&render, &phases_card("review-open"))[5],
        "dotworks-lead"
    );
}

#[test]
fn a_card_waiting_on_you_says_you_with_no_mark_before_it() {
    for owner in ["You", "you", "YOU", " you "] {
        let board = phases_board_with(|list| list.tasks[0].owner = Some(owner.into()));
        let render = render_board(&board, 186, 90);
        assert_eq!(
            card_text(&render, &phases_card("review-open"))[5],
            "You",
            "{owner:?}"
        );
        let row = who_row(&render, "review-open");
        assert_drawn(&render, (row, "You"), render.palette.overlay1, false);
    }
}

#[test]
fn a_long_owner_is_cut_inside_a_narrow_card() {
    let board = phases_board_with(|list| {
        list.tasks[4].owner = Some("a-very-long-owner-name".into());
    });
    // Five columns 14 wide: a card's inside is 10 columns.
    let render = render_board(&board, 80, 90);
    let who = &card_text(&render, &phases_card("build-overdue"))[5];
    assert!(who.starts_with("a-very") && who.ends_with('…'), "{who:?}");
    let row = who_row(&render, "build-overdue");
    let (rect, _) = render
        .hits
        .cards
        .iter()
        .find(|(_, card)| *card == phases_card("build-overdue"))
        .unwrap_or_else(|| panic!("drawn"));
    assert_eq!(render.buffer[(rect.right() - 1, row as u16)].symbol(), "│");
}

#[test]
fn the_panels_owner_is_the_cards_who() {
    let mut board = phases_board();
    board.work_selected = Some(phases_card("plan-pending"));
    let render = render_board_with_agents(
        &board,
        WIDE,
        80,
        &[working("w1:lead", "herdr", "herdr-lead", None)],
    );
    let row = render.row_with("Owner         herdr-lead");
    assert_label(&render, row, "Owner");
}

#[test]
fn the_filter_finds_a_card_by_who_has_it() {
    let mut board = phases_board();
    board.filter = "carmen".into();
    let render = render_board(&board, 186, 90);
    let shown: Vec<&String> = render.hits.cards.iter().map(|(_, card)| card).collect();
    assert_eq!(shown, [&phases_card("build-overdue")], "{:#?}", render.rows);
}

#[test]
fn a_card_without_a_slug_keeps_its_project_tasks() {
    let (_, inside) = phases_panel("all-null", 186);
    let words = panel_words(&inside);
    assert_eq!(
        words[..8],
        [
            "Status        · On track",
            "Owner         herdr",
            "Current step  No code here.",
            "",
            "STEPS  0 of 1 done",
            "▸ No code here.",
            "",
            "TASKS  4",
        ],
        "{words:#?}"
    );
    assert!(
        words.contains(&"#3  an unlinked herdr task (claude)"),
        "{words:#?}"
    );
}

#[test]
fn panel_phase_block_for_plan_verify_and_ship() {
    for (name, want) in [
        ("plan-approved", vec!["Approval      approved 2026-09-28"]),
        (
            "verify-done",
            vec![
                "Verification  Ran the suite.",
                "              All 12 green.",
            ],
        ),
        ("ship", vec!["Release       PR #12 merged."]),
    ] {
        let (_, inside) = phases_panel(name, 186);
        let words = panel_words(&inside);
        let at = words
            .iter()
            .position(|line| *line == want[0])
            .unwrap_or_else(|| panic!("{name}: {words:#?}"));
        assert_eq!(words[at..at + want.len()], want[..], "{name}");
        assert_eq!(at, 2, "{name}: after Status and Owner: {words:#?}");
    }
    let (_, pending) = phases_panel("plan-pending", 186);
    assert!(panel_words(&pending).contains(&"Approval      pending"));
}

#[test]
fn panel_long_objective_wraps_inside_the_panel() {
    let (render, inside) = phases_panel("review-open", 60);
    let words = panel_words(&inside);
    assert_eq!(
        words[..2],
        [
            "Objective     One board for tasks and phases, readable",
            "              at a glance on a laptop screen.",
        ],
        "{words:#?}"
    );
    let panel = render.hits.work_panel;
    for (y, _) in &inside {
        assert_eq!(render.buffer[(panel.right() - 1, *y as u16)].symbol(), "│");
    }
}

#[test]
fn an_open_boards_reread_repaints_only_when_what_it_draws_changed() {
    let mut state = empty_board_state();
    state.task_board.open = true;
    let start = std::time::Instant::now();
    let asks = tick_asks(&mut state, start);
    let (repaint, _) = state.handle_endpoint_result("boot-1", &asks[0], fixture_reply());
    assert!(repaint, "the first list draws");
    // The same list two seconds later: nothing to draw again.
    let later = start + std::time::Duration::from_secs(3);
    let asks = tick_asks(&mut state, later);
    assert_eq!(asks.len(), 1, "asked again");
    let (repaint, _) = state.handle_endpoint_result("boot-1", &asks[0], fixture_reply());
    assert!(!repaint, "an unchanged list is not drawn again");
    // A changed list draws.
    let mut changed: serde_json::Value = serde_json::from_str(LIST_V1).expect("fixture json");
    changed["problems"] = serde_json::json!(["/notes/x.md: no Task column"]);
    let asks = tick_asks(&mut state, later + std::time::Duration::from_secs(3));
    let (repaint, _) = state.handle_endpoint_result(
        "boot-1",
        &asks[0],
        Ok(crate::api::schema::ResponseResult::TasksList { list: changed }),
    );
    assert!(repaint, "a changed list draws");
}

#[test]
fn work_view_header_counts_the_kinds_shown_this_week() {
    let render = render_board(&work_board(), HEADER_WIDE, 40);
    let header = &render.rows[0];
    // The kind chips are gone: the top bar and the chip row name no kind, and row 2 holds
    // only the Filter menu.
    for kind in ["Features", "Bugs", "Chores", "◆", "✱", "▣", "○"] {
        assert!(!header.contains(kind), "{kind:?} in {header:?}");
        assert!(
            !render.rows[2].contains(kind),
            "{kind:?} in {:?}",
            render.rows[2]
        );
    }
    assert_eq!(render.rows[2].trim(), "Filter ▾", "{:?}", render.rows[2]);
    // The Tasks view's buttons are not in the Work view's header.
    for tasks_part in ["Action", "In progress", "Waiting"] {
        assert!(!header.contains(tasks_part), "{tasks_part:?} in {header:?}");
    }
    assert_eq!(
        render.cell(0, " Phases ").bg,
        render.palette.selection_bg,
        "Phases is on"
    );
    assert_ne!(render.cell(0, " Tasks ").bg, render.palette.selection_bg);
}

#[test]
fn work_view_has_five_stage_columns_with_counts() {
    let render = render_board(&work_board(), WIDE, 40);
    let order = |row: &str, parts: &[&str]| {
        let at = parts
            .iter()
            .map(|part| {
                row.find(part)
                    .unwrap_or_else(|| panic!("{part:?} in {row:?}"))
            })
            .collect::<Vec<_>>();
        assert!(at.windows(2).all(|pair| pair[0] < pair[1]), "{row:?}");
    };
    order(
        &render.rows[3],
        &["PLAN  2", "BUILD  2", "REVIEW  1", "VERIFY  1", "SHIP  10"],
    );
    assert!(render.cell(3, "PLAN").modifier.contains(Modifier::BOLD));
}

#[test]
fn work_cards_show_kind_project_title_progress_and_agent() {
    // herdr-w1 works: its card's who line starts with the spinner.
    let render = render_board_with_agents(
        &work_board(),
        WIDE,
        40,
        &[working("w36:pV", "herdr", "herdr-w1", None)],
    );
    let board_card = render.row_with("Herdr task board");
    let kind = render.row_with("◆ feature");
    assert!(
        render.rows[kind].contains("herdr"),
        "{:?}",
        render.rows[kind]
    );
    // The progress line is the title's line and its blank second line below.
    assert!(
        render.rows[board_card + 2].contains("2/3"),
        "{:#?}",
        render.rows
    );
    assert!(
        render.rows[board_card + 3].contains(&format!("{WORKER_SPINNER} herdr-w1 · 25m")),
        "the agent on it in the sub-agent spinner: {:#?}",
        &render.rows[kind..board_card + 4]
    );
    assert_eq!(
        render.cell(board_card + 3, WORKER_SPINNER).fg,
        working_colour(&render.palette)
    );
    assert_eq!(
        render.cell(board_card + 3, "herdr-w1").fg,
        render.palette.overlay1
    );
    // Each mark in its kind's colour (B5b).
    assert_eq!(render.cell(kind, "◆").fg, render.palette.mauve);
    let bug = render.row_with("✱ bug");
    assert_eq!(render.cell(bug, "✱").fg, render.palette.red);
    let chore = render.row_with("▣ chore");
    assert_eq!(render.cell(chore, "▣").fg, render.palette.teal);
    let lead = render.row_with("Never cut the");
    assert!(
        render.rows[lead + 1].contains('…') && render.rows[lead + 2].contains("0/2"),
        "a card's title stops at two lines: {:#?}",
        render.rows
    );
}

#[test]
fn a_cards_panel_says_what_its_agent_does_now_above_its_steps() {
    let mut board = work_board();
    board.work_selected = Some(card("2026-09-25-task-board.md"));
    let agents = [worker(
        "w36:pV",
        "herdr",
        "herdr-w1",
        "w36:p1",
        Some("Writing report.md"),
    )];
    let render = render_board_with_agents(&board, 186, 46, &agents);
    let now = render.row_with("│ Owner         herdr-w1: Writing report.md ");
    assert!(
        render.rows[now + 1].contains("│ Current step  work view")
            && render.rows[now + 2]
                .chars()
                .all(|ch| ch == ' ' || ch == '│')
            && render.rows[now + 3].contains("│ STEPS"),
        "{:#?}",
        &render.rows[now..now + 4]
    );
    assert_eq!(render.cell(now, "Owner").fg, render.palette.overlay1);
    assert_eq!(render.cell(now, "herdr-w1:").fg, render.palette.overlay1);
    // An agent the board does not list at work: its name alone.
    let render = render_board_with_agents(&board, 186, 46, &[]);
    let now = render.row_with("│ Owner         herdr-w1 ");
    assert!(
        !render.rows[now].contains("herdr-w1:"),
        "{:?}",
        render.rows[now]
    );
    // A card with no agent, no linked task and no space lead is its space's, as its card says.
    board.work_selected = Some(card("2026-09-24-tree-sidebar.md"));
    let render = render_board_with_agents(&board, 186, 46, &agents);
    render.row_with("│ Owner         herdr ");
}

#[test]
fn a_cards_now_line_drops_control_characters_from_what_its_agent_does() {
    use crate::api::schema::AgentStatus;
    use crate::client::shell::task_board::board_agents;
    let mut board = work_board();
    board.work_selected = Some(card("2026-09-25-task-board.md"));
    let mut snap = snapshot();
    snap.workspaces = vec![workspace("ws_h", "herdr", "/h")];
    snap.agents = vec![live_agent(
        "w36:pV",
        "ws_h",
        "herdr-w1",
        AgentStatus::Working,
        &[("task", "Writing \u{1b}[31mreport\u{1b}[0m.md\u{7}")],
    )];
    let config = ClientShellConfig::from_config(&Config::default());
    let agents = board_agents(Some(&snap), &config, None, 0);
    assert_eq!(
        agents[0].state,
        AgentState::Working(Some("Writing [31mreport[0m.md".into()))
    );
    let render = render_board_with_agents(&board, 186, 46, &agents);
    render.row_with("│ Owner         herdr-w1: Writing [31mreport[0m.md ");
}

/// The work board with `change` made to its list first.
fn work_board_with(
    change: impl FnOnce(&mut crate::client::shell::task_board::model::TaskList),
) -> TaskBoard {
    let mut list = parse_task_list(WORK_V1).expect("work fixture");
    change(&mut list);
    let mut board = work_board();
    board.store.receive(Ok(list), std::time::Instant::now());
    board
}

#[test]
fn a_work_card_title_takes_up_to_two_lines_cut_at_a_word_and_its_panel_shows_it_whole() {
    let lead_name = card("2026-09-25-lead-name.md");
    let mut board = work_board();
    board.work_selected = Some(lead_name);
    let render = render_board(&board, 186, 46);
    let title = render.row_with("Never cut the");
    let inside = |row: usize| {
        render.rows[row]
            .split('│')
            .nth(1)
            .unwrap_or("")
            .trim()
            .to_owned()
    };
    let (first, second) = (inside(title), inside(title + 1));
    assert!(!first.ends_with('…'), "the first line is whole: {first:?}");
    assert!(second.ends_with('…'), "the second line is cut: {second:?}");
    assert!(
        !second.trim_end_matches('…').ends_with(' ')
            && "Never cut the lead's name, even in a narrow sidebar where every column counts"
                .starts_with(&format!("{first} {}", second.trim_end_matches('…'))),
        "cut at a word: {first:?} {second:?}"
    );
    assert!(
        render.rows[title + 2].contains("0/2"),
        "the progress right under the two title lines: {:#?}",
        &render.rows[title - 2..title + 4]
    );
    let panel = panel_rows(&render).join(" ");
    assert!(
        shows_whole(
            &BoardRender {
                rows: vec![panel],
                ..render
            },
            "Never cut the lead's name, even in a narrow sidebar"
        ),
        "the panel shows the title whole"
    );
}

#[test]
fn a_work_card_shows_its_title_without_a_conventional_prefix_the_kind_mark_already_says() {
    let mut board = work_board_with(|list| {
        list.work[2].title = "feat(board): Herdr task board".into();
        list.work[5].title = "fix: Grey icon (Vesper)".into();
        list.work[1].title = "chore:Build sha in version".into();
    });
    board.work_selected = Some(card("2026-09-25-task-board.md"));
    let render = render_board(&board, 186, 47);
    let rows = render.rows.join("\n");
    for prefix in ["feat(board):", "fix:", "chore:"] {
        assert!(!rows.contains(prefix), "{prefix:?} drawn: {rows}");
    }
    for title in [
        "│ Herdr task board",
        "│ Grey icon (Vesper)",
        "│ Build sha in version",
    ] {
        assert!(rows.contains(title), "{title:?} on its card: {rows}");
    }
    let panel = panel_rows(&render);
    assert!(
        panel[1].contains(" Feature    Herdr task board "),
        "{:?}",
        panel[1]
    );
    // The filter matches what the card shows, so the prefix no longer finds it.
    board.filter = "fix:".into();
    let render = render_board(&board, 186, 47);
    assert!(
        !render.rows.iter().any(|row| row.contains("Grey icon")),
        "{:#?}",
        render.rows
    );
    // The data keeps the raw title.
    let list = board.store.list().expect("read");
    assert_eq!(list.work[5].title, "fix: Grey icon (Vesper)");
}

#[test]
fn a_work_card_with_no_agent_says_its_space_never_its_branch_or_last_commit_day() {
    let board = work_board_with(|list| {
        for card in &mut list.work {
            card.last_commit = Some("2026-09-27".into());
        }
    });
    // herdr-w1 works, so the card it is on keeps its spinner.
    let render = render_board_with_agents(
        &board,
        186,
        46,
        &[working("w36:pV", "herdr", "herdr-w1", None)],
    );
    // Nobody in oakwood: the card is its space's, and the branch stays in the panel.
    let per_client = render.row_with("Chrome per client");
    assert!(
        render.rows[per_client + 3].contains("│ oakwood ")
            && !render
                .rows
                .iter()
                .any(|row| row.contains("feat/per-client")),
        "{:#?}",
        &render.rows[per_client - 2..per_client + 5]
    );
    assert_eq!(
        render.cell(per_client + 3, "oakwood").fg,
        render.palette.overlay1
    );
    let task_board = render.row_with("Herdr task board");
    assert!(
        render.rows[task_board + 3].contains(&format!("{WORKER_SPINNER} herdr-w1 · 25m")),
        "the agent stays: {:#?}",
        &render.rows[task_board - 2..task_board + 5]
    );
}

/// The lines inside the card of `file` as drawn: each row between its box's sides, trimmed.
fn card_text(render: &BoardRender, file: &str) -> Vec<String> {
    let (rect, _) = render
        .hits
        .cards
        .iter()
        .find(|(_, card)| card == file)
        .unwrap_or_else(|| panic!("{file} drawn"));
    (rect.y + 1..rect.bottom() - 1)
        .map(|y| {
            (rect.x + 1..rect.right() - 1)
                .map(|x| render.buffer[(x, y)].symbol().to_owned())
                .collect::<String>()
                .trim()
                .to_owned()
        })
        .collect()
}

#[test]
fn every_card_has_seven_lines_kind_and_project_rule_title_progress_who_and_phase_status() {
    let board = work_board_with(|list| {
        for card in &mut list.work {
            if card.title == "Box lines on /clear" {
                card.shipped_on = Some("2026-09-28".into());
            } else if card.title == "Numbered, wrapped rows" {
                // Shipped in another year than the list's: the year shows.
                card.shipped_on = Some("2027-01-05".into());
            } else if card.title == "Ghostty config moved" {
                card.shipped_on = None;
            } else if card.title == "Blank /clear screen" {
                // One step, still open: `0/1`.
                card.steps[0].done = false;
            }
        }
    });
    let render = render_board_with_agents(
        &board,
        186,
        90,
        &[working("w36:pV", "herdr", "herdr-w1", None)],
    );
    // A progress line is its bar and, after it, the steps done over all of them.
    let counts = |line: &str, count: &str| {
        line.ends_with(&format!(" {count}")) && line.starts_with(['━', '─'])
    };
    // Build: its agent at work, with the spinner, and no deadline: on track.
    let build = card_text(&render, &card("2026-09-25-task-board.md"));
    assert_eq!(
        [&build[..1], &build[2..4], &build[5..]].concat(),
        [
            "◆ feature · herdr",
            "Herdr task board",
            "",
            &format!("{WORKER_SPINNER} herdr-w1 · 25m"),
            "· On track",
        ],
        "{:#?}",
        render.rows
    );
    // The rule between the kind and the title runs the card's inside, its box's `├` and `┤`
    // at the sides.
    assert!(
        !build[1].is_empty() && build[1].chars().all(|c| c == '─'),
        "{:#?}",
        render.rows
    );
    assert!(counts(&build[4], "2/3"), "{:#?}", render.rows);
    // Plan, no steps: a blank progress line, and with nobody in oakwood its space has it.
    let plan = card_text(&render, &card("2026-09-24-build-sha.md"));
    assert_eq!(plan[0], "▣ chore · oak-cli");
    assert_eq!(plan[2], "Build sha in version");
    assert_eq!(plan[4], "", "{plan:?}");
    assert_eq!(plan[5], "oakwood", "{plan:?}");
    assert_eq!(plan[6], "? Awaiting your approval");
    // Ship: the day it shipped on the last line, and its space as who has it.
    let shipped = card_text(&render, &card("shipped-1.md"));
    assert!(counts(&shipped[4], "1/1"), "{shipped:?}");
    assert_eq!(
        shipped[5..],
        ["dotworks", "✓ Shipped 28 Sep"],
        "{shipped:?}"
    );
    let open_one = card_text(&render, &card("shipped-0.md"));
    assert!(counts(&open_one[4], "0/1"), "{open_one:?}");
    let next_year = card_text(&render, &card("shipped-2.md"));
    assert_eq!(next_year[6], "✓ Shipped 5 Jan 2027");
    // No `shipped_on`: nothing to say.
    let undated = card_text(&render, &card("shipped-3.md"));
    assert_eq!(undated[6], "", "{undated:?}");
    // Every card is the same seven lines, in every column.
    assert!(render.hits.cards.iter().all(|(rect, _)| rect.height == 9));
}

/// The tree sidebar card's steps: one done, then two open, the first open one three lines
/// long at the panel's width, each with its short text and its whole line.
fn tree_steps(list: &mut crate::client::shell::task_board::model::TaskList) {
    let step =
        |text: &str, full: &str, done: bool| crate::client::shell::task_board::model::WorkStep {
            text: text.into(),
            full: full.into(),
            done,
            ..Default::default()
        };
    let tree = list
        .work
        .iter_mut()
        .find(|card| card.file.ends_with("2026-09-24-tree-sidebar.md"))
        .expect("tree card");
    tree.last_commit = Some("2026-09-27".into());
    tree.steps = vec![
        step("Tree rows.", "Tree rows. Each folder a row.", true),
        step(
            "Folded counts.",
            &format!(
                "Folded counts. {} end of the long step.",
                "A folded space shows what its hidden tabs do right after its arrow, the needs you count first and then the working count with the sub-agent spinner.".repeat(3)
            ),
            false,
        ),
        step("Rail cleanup.", "Rail cleanup.", false),
    ];
}

#[test]
fn a_work_panel_step_shows_its_whole_title_cut_only_at_the_panel_width() {
    let step = |text: &str, full: &str| crate::client::shell::task_board::model::WorkStep {
        text: text.into(),
        full: full.into(),
        done: false,
        ..Default::default()
    };
    let long = format!(
        "{}the end",
        "Rail cleanup with every mark in its own column ".repeat(5)
    );
    let mut board = work_board_with(|list| {
        let tree = list
            .work
            .iter_mut()
            .find(|card| card.file.ends_with("2026-09-24-tree-sidebar.md"))
            .expect("tree card");
        tree.steps = vec![
            step(
                "B2 herdr-lead: spinners share one frame clock",
                "- [ ] B2 herdr-lead: spinners share one frame clock",
            ),
            // No short text: the whole line is the source.
            step("", "Folded counts (B) in the sidebar"),
            // No space after its colon: kept as written.
            step("B2:spinners", "B2:spinners"),
            step(&long, &long),
            step("(B) tidy the rail", "(B) tidy the rail"),
        ];
    });
    board.work_selected = Some(card("2026-09-24-tree-sidebar.md"));
    let render = render_board(&board, 186, 46);
    let steps = render.row_with("STEPS");
    let inside = |render: &BoardRender, offset: usize| {
        render.rows[steps + offset]
            .trim()
            .trim_matches('│')
            .trim()
            .to_owned()
    };
    assert_eq!(
        inside(&render, 1),
        "▸ B2 herdr-lead: spinners share one frame clock"
    );
    assert_eq!(inside(&render, 2), "○ Folded counts (B) in the sidebar");
    assert_eq!(inside(&render, 3), "○ B2:spinners");
    // Wider than the panel: one line, cut at its width with `…`.
    let cut = inside(&render, 4);
    let panel = render.hits.work_panel;
    assert!(cut.ends_with('…') && !cut.contains("the end"), "{cut:?}");
    assert!(
        long.starts_with(
            cut.trim_start_matches("○ ")
                .trim_end_matches('…')
                .trim_end()
        ),
        "{cut:?}"
    );
    assert!(
        UnicodeWidthStr::width(cut.as_str()) + 4 >= usize::from(panel.width) - 6,
        "it takes the panel's width: {cut:?}"
    );
    assert_eq!(inside(&render, 5), "○ (B) tidy the rail", "one line");

    // The selected step shows in full, wrapped under its text.
    board.work_step = Some(3);
    let render = render_board(&board, 186, 46);
    assert!(
        (4..8).any(|offset| inside(&render, offset).ends_with("the end")),
        "{:#?}",
        &render.rows[steps..steps + 8]
    );
}

#[test]
fn the_work_panel_lists_done_steps_first_then_the_current_one_each_by_its_short_title_then_file_branch_and_keys(
) {
    let mut board = work_board_with(tree_steps);
    board.work_selected = Some(card("2026-09-24-tree-sidebar.md"));
    let render = render_board(&board, 186, 46);
    let panel = render.hits.work_panel;
    let steps = render.row_with("STEPS");
    let at = |offset: usize| render.rows[steps + offset].as_str();
    // Each step its short text on one line; the whole line is in the file.
    assert!(
        at(1).contains("│ ✓ Tree rows.") && !at(1).contains("Each folder"),
        "{:?}",
        at(1)
    );
    assert!(
        at(2).contains("│ ▸ Folded counts.") && !at(2).contains("A folded space"),
        "{:#?}",
        render.rows
    );
    assert!(at(3).contains("│ ○ Rail cleanup."), "{:?}", at(3));
    assert_eq!(render.cell(steps + 1, "✓").fg, render.palette.teal);
    assert_eq!(render.cell(steps + 1, "Tree rows").fg, render.palette.text);
    assert_eq!(render.cell(steps + 2, "▸").fg, render.palette.text);
    assert_eq!(render.cell(steps + 2, "Folded").fg, render.palette.text);
    assert_eq!(render.cell(steps + 3, "○").fg, render.palette.text);
    // Its foot: the spec file and the branch, then its keys.
    let keys = usize::from(panel.bottom()) - 2;
    assert!(
        render.rows[keys - 3].contains(
            "│ Spec          ~/projects/repos/herdr/docs/intent/2026-09-24-tree-sidebar.md"
        ),
        "{:?}",
        render.rows[keys - 3]
    );
    assert!(
        render.rows[keys - 2]
            .contains("│ Branch        feat/sidebar-tree · 4 commits · last commit Sep 27"),
        "{:?}",
        render.rows[keys - 2]
    );
    assert!(
        render.rows[keys - 1]
            .trim_matches(|c| c == '│' || c == ' ')
            .is_empty(),
        "a blank line between the foot and the keys: {:?}",
        render.rows[keys - 1]
    );
    assert!(
        render.rows[keys].contains("│  ↑↓  step     Enter  open file "),
        "{:?}",
        render.rows[keys]
    );
    // Selected, a step keeps its short title, on the selection band.
    board.work_step = Some(1);
    let render = render_board(&board, 186, 46);
    let first = render.row_with("▸ Folded counts.");
    assert!(
        render.rows[first + 1].contains("○ Rail cleanup."),
        "{:#?}",
        render.rows
    );
    assert_eq!(render.cell(first, "Folded").bg, render.palette.selection_bg);
    assert_ne!(
        render.cell(render.row_with("○ Rail"), "Rail").bg,
        render.palette.selection_bg
    );

    // A card with an agent offers its lead's tab.
    board.work_step = None;
    board.work_selected = Some(card("2026-09-25-task-board.md"));
    let render = render_board(&board, 186, 46);
    let keys = usize::from(render.hits.work_panel.bottom()) - 2;
    assert!(
        render.rows[keys].contains("│  ↑↓  step     Enter  go to lead's tab     o  open file "),
        "{:?}",
        render.rows[keys]
    );
}

#[test]
fn a_short_work_panel_keeps_the_branch_and_commits_in_its_heading() {
    let mut board = work_board_with(tree_steps);
    board.work_selected = Some(card("2026-09-24-tree-sidebar.md"));
    // A card is nine rows: the panel keeps what is left under a whole one and the stage's
    // label and underline (B5b: two rows more than a seven-row card, so two rows taller windows).
    for height in [23, 24, 25] {
        let render = render_board(&board, 186, height);
        let panel = render.hits.work_panel;
        assert!(
            (5..9).contains(&panel.height),
            "a panel with no foot at {height} rows: {panel:?}"
        );
        let heading = &render.rows[usize::from(panel.y) + 1];
        assert!(
            heading.contains("feat/sidebar-tree · 4 commits · last commit Sep 27"),
            "{height} rows: {heading:?}"
        );
    }
    // A panel with its foot has them there, not twice.
    let render = render_board(&board, 186, 46);
    let panel = render.hits.work_panel;
    assert!(!render.rows[usize::from(panel.y) + 1].contains("4 commits"));
}

#[test]
fn a_card_with_commits_and_no_branch_says_so_without_the_branch_label() {
    let mut board = work_board_with(|list| {
        tree_steps(list);
        let tree = list
            .work
            .iter_mut()
            .find(|card| card.file.ends_with("2026-09-24-tree-sidebar.md"))
            .expect("tree card");
        tree.branch = None;
        tree.commits = 3;
    });
    board.work_selected = Some(card("2026-09-24-tree-sidebar.md"));
    let render = render_board(&board, 186, 46);
    let line = render.row_with("3 commits · last commit Sep 27");
    assert!(
        !render.rows[line].contains("branch"),
        "{:?}",
        render.rows[line]
    );
}

#[test]
fn the_work_panel_opens_only_at_the_bottom_with_no_button_to_move_it() {
    for (width, height) in [(186, 46), (100, 40), (89, 37)] {
        let mut state = work_selected_state("2026-09-24-tree-sidebar.md", width, height);
        let rows = frame_rows(&state.compose(width, height).expect("frame"));
        let hits = &state.hits.task_board;
        let (area, panel) = (hits.area, hits.work_panel);
        assert_eq!(
            (panel.x, panel.width),
            (area.x + 1, area.width - 2),
            "{width}: at the bottom: {panel:?}"
        );
        let columns_bottom = hits
            .work_column_areas
            .iter()
            .map(|area| area.bottom())
            .max()
            .unwrap_or(0);
        assert!(columns_bottom < panel.y, "{width}: under the columns");
        // The heading ends with its close button alone.
        let heading = &rows[usize::from(panel.y) + 1];
        assert!(
            heading.trim_end().ends_with("×  │")
                && !heading.contains('⇥')
                && !heading.contains('⇩'),
            "{width}: {heading:?}"
        );
        assert!(
            !rows
                .iter()
                .any(|row| row.contains(" side ") || row.contains(" bottom ")),
            "{width}: {rows:#?}"
        );
        // The close button still closes it.
        let close = hits.panel_close;
        board_click(&mut state, close);
        assert!(state.task_board.work_panel_closed, "{width}");
    }
}

#[test]
fn a_selected_step_is_banded_and_the_arrows_move_between_steps() {
    let mut state = work_state();
    let mut list = parse_task_list(WORK_V1).expect("work fixture");
    tree_steps(&mut list);
    state
        .task_board
        .store
        .receive(Ok(list), std::time::Instant::now());
    state.task_board.view = BoardView::Work;
    state.task_board.work_selected = Some(card("2026-09-24-tree-sidebar.md"));
    state.compose(186, 46).expect("frame");
    // REVIEW holds this card alone: down past it goes into the panel's first step.
    press_key(&mut state, KeyCode::Down);
    assert_eq!(state.task_board.work_step, Some(0));
    assert_eq!(
        state.task_board.work_selected,
        Some(card("2026-09-24-tree-sidebar.md"))
    );
    let rows = frame_rows(&state.compose(186, 46).expect("frame"));
    let first = rows
        .iter()
        .position(|row| row.contains("▸ Folded counts."))
        .expect("step");
    assert!(
        rows[first + 1].contains("○ Rail cleanup."),
        "its short title alone: {:#?}",
        &rows[first..first + 3]
    );
    press_key(&mut state, KeyCode::Down);
    assert_eq!(state.task_board.work_step, Some(1));
    press_key(&mut state, KeyCode::Down);
    press_key(&mut state, KeyCode::Down);
    assert_eq!(state.task_board.work_step, Some(2), "the last step stays");
    // Esc leaves the panel before it closes it.
    press_key(&mut state, KeyCode::Esc);
    assert_eq!(state.task_board.work_step, None);
    assert!(!state.task_board.work_panel_closed);
    // A click on a step selects it; up from the first leaves the panel.
    state.compose(186, 46).expect("frame");
    let (rect, _) = state
        .hits
        .task_board
        .work_step_rows
        .iter()
        .find(|(_, step)| *step == 1)
        .copied()
        .expect("step 1 drawn");
    board_click(&mut state, rect);
    assert_eq!(state.task_board.work_step, Some(1));
    press_key(&mut state, KeyCode::Up);
    press_key(&mut state, KeyCode::Up);
    assert_eq!(state.task_board.work_step, None);
    press_key(&mut state, KeyCode::Up);
    assert_eq!(
        state.task_board.work_step, None,
        "up again moves among the cards"
    );
}

#[test]
fn right_from_the_last_column_stays_on_the_cards_and_down_from_its_last_card_enters_the_steps() {
    let mut state = work_state();
    state.task_board.view = BoardView::Work;
    state.task_board.work_selected = Some(card("shipped-0.md"));
    state.compose(186, 46).expect("frame");
    // SHIP is the last column: right has nowhere to go, and the panel is not beside it.
    press_key(&mut state, KeyCode::Right);
    assert_eq!(state.task_board.work_step, None);
    assert_eq!(state.task_board.work_selected, Some(card("shipped-0.md")));
    // From a column with more after it, right moves to the next column.
    state.task_board.work_selected = Some(card("2026-09-24-tree-sidebar.md"));
    state.compose(186, 46).expect("frame");
    press_key(&mut state, KeyCode::Right);
    assert_eq!(state.task_board.work_step, None);
    assert_eq!(
        state.task_board.work_selected,
        Some(card("2026-09-24-grey-icon.md"))
    );
    // Down from a column's last card goes into the panel under it.
    state.compose(186, 46).expect("frame");
    press_key(&mut state, KeyCode::Down);
    assert_eq!(state.task_board.work_step, Some(0));
}

#[test]
fn work_shipped_keeps_this_week_and_a_tall_column_scrolls_its_full_cards() {
    let render = render_board(&work_board(), WIDE, 40);
    assert!(!render
        .rows
        .iter()
        .any(|row| row.contains("Old shipped card")));

    // Short enough that SHIP's ten cards do not all fit: a whole card, and the count of
    // the rest in the label.
    let tall = render_board(&work_board(), WIDE, 17);
    assert!(!tall.rows.iter().any(|row| row.contains("Old shipped card")));
    let first = tall.row_with("Blank /clear");
    assert!(
        tall.rows[first - 2].contains("✱ bug") && tall.rows[first - 3].contains('╭'),
        "a full card: {:#?}",
        tall.rows
    );
    assert!(tall.rows[3].contains("SHIP  10") && tall.rows[3].ends_with("▼ 9"));
    assert!(
        !tall.rows.iter().any(|row| row.contains("Hooks tidy")),
        "the last cards are below: {:#?}",
        tall.rows
    );
}

#[test]
fn work_selected_card_is_boxed_and_shown_in_full_below() {
    let mut board = work_board();
    board.work_selected = Some(card("2026-09-24-tree-sidebar.md"));
    let render = render_board(&board, WIDE, 50);
    let title = render.row_with("Tree sidebar");
    assert!(
        render.rows[title - 3].contains('╭') && render.rows[title].contains('│'),
        "boxed: {:#?}",
        &render.rows[title - 3..title + 4]
    );
    let panel = panel_rows(&render).join("\n");
    for part in [
        "◆ Feature    Tree sidebar",
        "herdr",
        "feat/sidebar-tree",
        "4 commits",
        "✓ tree rows",
        "✓ double count fixed",
        "#1  say final (you)",
        "#2  push the fork (claude)",
    ] {
        assert!(panel.contains(part), "{part:?} in {panel}");
    }

    let mut board = work_board();
    board.work_selected = Some(card("shipped-5.md"));
    let tall = render_board(&board, WIDE, 23);
    assert!(
        tall.rows
            .iter()
            .any(|row| row.contains("Double count of unseen")),
        "a one-line column shows the selected card whole: {:#?}",
        tall.rows
    );
}

#[test]
fn work_view_says_when_there_is_no_work_yet() {
    let mut board = TaskBoard {
        open: true,
        view: BoardView::Work,
        ..TaskBoard::default()
    };
    board.store.receive_json(
        LIST_V1.replace("\"work\": [", "\"old_work\": [").as_str(),
        std::time::Instant::now(),
    );
    let render = render_board(&board, WIDE, 20);
    assert!(
        render.rows[3].contains(
            "No features, bugs or chores yet: they appear when a lead writes an intent file."
        ),
        "{:#?}",
        render.rows
    );
}

#[test]
fn work_view_footer_lists_the_keys_that_work() {
    // Every key, as `?` shows them.
    let mut board = work_board();
    board.keys_shown = true;
    let render = render_board(&board, WIDE, 40);
    let keys = &render.rows[36..].join("\n");
    for part in [
        "← ↑ ↓ →  select",
        "Double-click  open",
        "Enter  go to lead's tab",
        "t  its tasks",
        "Tab  Tasks",
        "click a space  only its work",
        "Esc  close",
    ] {
        assert!(keys.contains(part), "{part:?} in {keys:?}");
    }
    assert!(keys.contains("o  open file"), "{keys:?}");
    assert!(!keys.contains(" n "), "no key starts new work: {keys:?}");

    board.can_open = false;
    let keys = render_board(&board, WIDE, 40).rows[36..].join("\n");
    assert!(
        !keys.contains("o  open") && !keys.contains("Double-click"),
        "a server without tasks.open cannot open the file: {keys:?}"
    );
    assert!(keys.contains("Enter  go to lead's tab"), "{keys:?}");
}

/// The footer's rows under its rule, and the footer keys a click there does.
fn footer_under_rule(render: &BoardRender) -> (String, Vec<&'static str>) {
    let rule = render
        .rows
        .iter()
        .rposition(|row| is_rule(row))
        .expect("footer rule");
    let keys = render.rows[rule + 1..].join("\n");
    let clicks = render
        .hits
        .footer_keys
        .iter()
        .filter(|(rect, _)| usize::from(rect.y) > rule)
        .map(|(_, key)| *key)
        .collect();
    (keys, clicks)
}

#[test]
fn work_view_footer_leaves_out_the_keys_the_card_panel_shows_as_buttons() {
    let mut board = work_board();
    board.keys_shown = true;
    board.work_selected = Some(card("2026-09-25-task-board.md"));
    let render = render_board(&board, 132, 40);
    assert!(render.hits.work_panel.height > 0, "{:#?}", render.rows);
    let buttons = usize::from(render.hits.work_panel.bottom()) - 2;
    assert!(
        render.rows[buttons].contains(" ↑↓  step     Enter  go to lead's tab     o  open file"),
        "{:?}",
        render.rows[buttons]
    );
    let (keys, clicks) = footer_under_rule(&render);
    for key in ["o  open file", "Enter  go to lead's tab"] {
        assert!(!keys.contains(key), "the panel shows {key:?}: {keys:?}");
    }
    for key in [
        "Double-click  open",
        "t  its tasks",
        "/  search",
        "Tab  Tasks",
        "Esc  close",
    ] {
        assert!(keys.contains(key), "{key:?} in {keys:?}");
    }
    for key in ["Enter", "o"] {
        assert!(
            !clicks.contains(&key),
            "no footer click does {key:?}: {clicks:?}"
        );
    }
    assert!(
        clicks.contains(&"Esc") && clicks.contains(&"t"),
        "{clicks:?}"
    );

    // A card with no agent opens its file on `Enter`, so neither `Enter` nor `o` is left for
    // `?` to list.
    board.work_selected = Some(card("2026-09-23-per-client.md"));
    let (keys, _) = footer_under_rule(&render_board(&board, 132, 40));
    assert!(!keys.contains("Enter  "), "{keys:?}");
    assert!(!keys.contains("o  open file"), "{keys:?}");

    // A body too short for the panel keeps every key in the `?` list.
    board.work_selected = Some(card("2026-09-25-task-board.md"));
    let short = render_board(&board, 132, 14);
    assert_eq!(short.hits.work_panel.height, 0, "{:#?}", short.rows);
    let (keys, clicks) = footer_under_rule(&short);
    for key in ["o  open file", "Enter  go to lead's tab", "t  its tasks"] {
        assert!(keys.contains(key), "{key:?} in {keys:?}");
    }
    assert!(
        clicks.contains(&"o") && clicks.contains(&"Enter"),
        "{clicks:?}"
    );
}

#[test]
fn a_filter_pick_that_hides_the_selected_card_hides_its_panel_and_gives_back_its_keys() {
    // The grid, the panel and the footer agree: a card a Filter pick leaves out of its
    // column shows no panel, and the footer lists the panel's keys again.
    let task_board = card("2026-09-25-task-board.md");
    let mut state = work_state();
    state.task_board.view = BoardView::Work;
    state.task_board.keys_shown = true;
    state.task_board.work_selected = Some(task_board.clone());
    state.compose(132, 40).expect("frame");
    let hits = &state.hits.task_board;
    assert!(hits.cards.iter().any(|(_, file)| *file == task_board));
    assert!(
        hits.work_panel.height > 0,
        "a feature's panel before the pick"
    );

    state
        .task_board
        .work_filter
        .toggle(FilterPick::Kind(WorkKind::Bug));
    state.compose(132, 40).expect("frame");
    let hits = &state.hits.task_board;
    let drawn = hits.cards.iter().any(|(_, file)| *file == task_board);
    let panel = hits.work_panel.height > 0;
    assert_eq!(drawn, panel, "drawn {drawn}, panel {panel}");
    let keys: Vec<&str> = hits.footer_keys.iter().map(|(_, key)| *key).collect();
    for key in ["Enter", "o"] {
        assert!(
            keys.contains(&key),
            "one {key:?} button, in the panel or the footer: {keys:?}"
        );
    }
}

#[test]
fn work_view_at_a_narrow_width_keeps_every_card_title_whole() {
    // Narrow, one stage shows at a time: the selected card's.
    for (file, title) in [
        ("2026-09-25-task-board.md", "Herdr task board"),
        ("2026-09-23-per-client.md", "Chrome per client"),
        ("2026-09-24-tree-sidebar.md", "Tree sidebar"),
        ("2026-09-24-grey-icon.md", "Grey icon (Vesper)"),
    ] {
        let mut board = work_board();
        board.work_selected = Some(card(file));
        let render = render_board(&board, NARROW, 80);
        let columns = render.rows[..render.rows.len() - 12].join(" ");
        assert!(
            shows_whole(&render, title) && columns.contains(title.split(' ').next().unwrap_or("")),
            "{title}: {:#?}",
            render.rows
        );
    }
}

fn work_state() -> ClientShellState {
    let mut state = empty_board_state();
    state
        .task_board
        .store
        .receive_json(WORK_V1, std::time::Instant::now());
    state.task_board.open = true;
    state.compose(160, 40).expect("frame");
    state
}

#[test]
fn tab_and_the_view_tabs_switch_between_tasks_and_work() {
    let mut state = work_state();
    let footer = frame_rows(&state.compose(200, 40).expect("frame"))[39].clone();
    assert!(footer.contains("Tab  Phases"), "{footer:?}");
    press_key(&mut state, KeyCode::Tab);
    assert_eq!(state.task_board.view, BoardView::Work);
    state.compose(160, 40).expect("frame");
    press_key(&mut state, KeyCode::Tab);
    assert_eq!(state.task_board.view, BoardView::Tasks);
    state.compose(160, 40).expect("frame");
    let work = state.hits.task_board.work_tab;
    board_click(&mut state, work);
    assert_eq!(state.task_board.view, BoardView::Work);
    state.compose(160, 40).expect("frame");
    let tasks = state.hits.task_board.tasks_tab;
    board_click(&mut state, tasks);
    assert_eq!(state.task_board.view, BoardView::Tasks);
}

#[test]
fn work_view_keys_move_between_cards_go_to_the_agent_and_show_its_tasks() {
    let mut state = work_state();
    state.task_board.view = BoardView::Work;
    state.compose(160, 40).expect("frame");
    press_key(&mut state, KeyCode::Down);
    assert_eq!(
        state.task_board.work_selected,
        Some(card("2026-09-25-lead-name.md")),
        "the first card of the first column"
    );
    state.compose(160, 40).expect("frame");
    press_key(&mut state, KeyCode::Char('j'));
    assert_eq!(
        state.task_board.work_selected,
        Some(card("2026-09-24-build-sha.md"))
    );
    press_key(&mut state, KeyCode::Right);
    assert_eq!(
        state.task_board.work_selected,
        Some(card("2026-09-23-per-client.md")),
        "the same row of the next column"
    );
    press_key(&mut state, KeyCode::Char('k'));
    assert_eq!(
        state.task_board.work_selected,
        Some(card("2026-09-25-task-board.md"))
    );
    let outcome = press_key(&mut state, KeyCode::Enter);
    assert_eq!(focused_pane(&outcome), Some("w36:pV"));
    assert!(!state.task_board.open, "the agent's pane shows");

    state.task_board.open = true;
    state.task_board.work_selected = Some(card("2026-09-23-per-client.md"));
    state.compose(160, 40).expect("frame");
    let outcome = press_key(&mut state, KeyCode::Enter);
    assert_eq!(focused_pane(&outcome), None, "no agent on it");
    press_key(&mut state, KeyCode::Char('t'));
    assert_eq!(state.task_board.view, BoardView::Tasks);
    assert_eq!(state.task_board.space.as_deref(), Some("oakwood"));
}

#[test]
fn work_cards_select_on_click() {
    let mut state = work_state();
    state.task_board.view = BoardView::Work;
    state.compose(160, 40).expect("frame");
    let (rect, _) = state
        .hits
        .task_board
        .cards
        .iter()
        .find(|(_, file)| *file == card("2026-09-24-grey-icon.md"))
        .cloned()
        .expect("drawn");
    board_click(&mut state, rect);
    assert_eq!(
        state.task_board.work_selected,
        Some(card("2026-09-24-grey-icon.md"))
    );
}

#[test]
fn every_key_a_board_footer_names_has_a_handler_in_its_view() {
    for view in [BoardView::Tasks, BoardView::Work] {
        for (key, what) in footer_keys(view, true, true).iter() {
            // Mouse hints name no key.
            if key.starts_with("click") || *key == "Double-click" {
                continue;
            }
            let code = match *key {
                "↑ ↓" => KeyCode::Down,
                "← ↑ ↓ →" => KeyCode::Right,
                "Enter" => KeyCode::Enter,
                "Tab" => KeyCode::Tab,
                "J/K" => KeyCode::Char('J'),
                "→ ←" => KeyCode::Right,
                "Esc" => KeyCode::Esc,
                key if key.chars().count() == 1 => {
                    KeyCode::Char(key.chars().next().expect("one character"))
                }
                key => panic!("the footer names {key:?}; map it to its key here"),
            };
            let mut state = work_state();
            state.task_board.view = view;
            state.compose(160, 40).expect("frame");
            // The arrows open a selected row: select the first one.
            if *key == "→ ←" {
                press_key(&mut state, KeyCode::Down);
                state.compose(160, 40).expect("frame");
            }
            let outcome = press_key(&mut state, code);
            assert!(
                outcome.repaint,
                "{view:?}: the footer names {key:?}{what:?} but pressing it does nothing"
            );
        }
    }
}

#[test]
fn work_view_says_where_new_work_comes_from_only_while_there_is_none() {
    let render = render_board(&work_board(), WIDE, 40);
    assert!(
        !render
            .rows
            .iter()
            .any(|row| row.contains("new work starts when")),
        "the cards show where work comes from: {:#?}",
        render.rows
    );
    let mut board = work_board();
    board.store.receive_json(
        r#"{"version": 1, "tasks": [], "work": []}"#,
        std::time::Instant::now(),
    );
    let render = render_board(&board, WIDE, 40);
    let empty = render.row_with("they appear when a lead writes an intent file");
    assert_eq!(
        render.cell(empty, "No features").fg,
        render.palette.overlay1
    );
}

#[test]
fn wrapped_detail_and_panel_lines_never_end_in_a_separator() {
    let mut board = work_board();
    board.work_selected = Some(card("2026-09-24-tree-sidebar.md"));
    let mut tasks = fixture_board();
    tasks.selected = Some(key(6));
    for render in [
        render_board(&board, WIDE, 40),
        render_board(&tasks, WIDE, 40),
    ] {
        for row in &render.rows {
            let text = row.trim_end().trim_end_matches('│').trim_end();
            assert!(!text.ends_with('·'), "{row:?}");
        }
    }
}

#[test]
fn work_panel_says_how_many_lines_it_could_not_show() {
    let mut list = parse_task_list(WORK_V1).expect("work fixture");
    let file = card("2026-09-24-tree-sidebar.md");
    let tree = list
        .work
        .iter_mut()
        .find(|card| card.file == file)
        .expect("tree card");
    // Thirty steps, each one line cut at the panel's width: the head, thirty steps and the
    // tasks line are more lines than the window has.
    tree.steps = (1..=30)
        .map(|step| crate::client::shell::task_board::model::WorkStep {
            full: String::new(),
            text: format!("step {step:02} {}", "wrap ".repeat(10)),
            done: false,
            ..Default::default()
        })
        .collect();
    let mut board = work_board();
    board.store.receive(Ok(list), std::time::Instant::now());
    board.work_selected = Some(file);
    // The count is every item not shown: the steps after the last one shown, and the card's
    // two tasks.
    let render = render_board(&board, WIDE, 40);
    let more = render.row_with(" more steps");
    assert!(
        render.rows[more].contains(" ↑↓  step     Enter  open file")
            && render.rows[more + 1].contains('╰')
            && render.rows[more - 1].trim_matches(|c| c == '│' || c == ' ').is_empty()
            && render.rows[more - 2].contains("│ Branch ")
            && render.rows[more - 3].contains("│ Spec ")
            && render.rows[more - 4].contains("step "),
        "the count is on the keys' line, the panel's last, under a blank line, its spec file and branch: {:#?}",
        render.rows
    );
    let last = render.rows[more - 4]
        .split("step ")
        .nth(1)
        .and_then(|rest| rest.get(..2))
        .and_then(|number| number.parse::<usize>().ok())
        .unwrap_or_else(|| panic!("a step above the count: {:#?}", render.rows));
    assert!(
        render.rows[more].contains(&format!("▼ {} more steps and 2 tasks · scroll", 30 - last)),
        "{:#?}",
        render.rows
    );
    // Narrow, the count sits in the same place.
    let render = render_board(&board, NARROW, 40);
    let rule = render.rows.len() - 2;
    assert!(
        render.rows[rule - 3].contains(" ↑↓  step     Enter  open file")
            // Each step not selected takes one line, so more show than when they wrapped.
            && render.rows[rule - 3].ends_with("▼ 27 │"),
        "in marks where the words do not fit: {:#?}",
        render.rows
    );
}

// Row actions (round B4c): each key sends one `tasks.act` beside the command lane, naming
// the selected task by every copy it has. The tests are the server: they read each request
// the board sent and answer it ok, refused, or with an error.

/// The `tasks.act` requests `outcome` sends, each by its id; none may join the lane.
fn act_requests(outcome: &ClientShellInput) -> Vec<(String, crate::api::schema::TasksActParams)> {
    outcome
        .actions
        .iter()
        .filter_map(|action| match action {
            ClientShellAction::EndpointSide { request, .. } => match &request.method {
                crate::api::schema::Method::TasksAct(params) => {
                    Some((request.id.clone(), params.clone()))
                }
                _ => None,
            },
            ClientShellAction::Endpoint { request, .. } => {
                assert!(
                    !matches!(request.method, crate::api::schema::Method::TasksAct(_)),
                    "tasks.act never joins the lane"
                );
                None
            }
            _ => None,
        })
        .collect()
}

/// The one `tasks.act` `outcome` sends.
fn one_act(outcome: &ClientShellInput) -> (String, crate::api::schema::TasksActParams) {
    let mut sent = act_requests(outcome);
    assert_eq!(sent.len(), 1, "{sent:?}");
    sent.remove(0)
}

fn copies_of(list_n: usize) -> Vec<crate::api::schema::TaskCopy> {
    let list = parse_task_list(LIST_V1).expect("fixture");
    list.tasks[list_n - 1]
        .copies
        .iter()
        .map(|copy| crate::api::schema::TaskCopy {
            file: copy.file.clone(),
            row: copy.row.clone(),
        })
        .collect()
}

fn act_reply(
    ok: bool,
    reason: Option<&str>,
) -> Result<crate::api::schema::ResponseResult, ClientShellEndpointError> {
    Ok(crate::api::schema::ResponseResult::TasksAct {
        ok,
        reason: reason.map(str::to_owned),
    })
}

/// Answers the action `id` ok, then lands the list read after it, so the next action can go.
fn act_done_and_list_read(state: &mut ClientShellState, id: &str) {
    state.handle_endpoint_result("boot-1", id, act_reply(true, None));
    let asked = tick_asks(state, std::time::Instant::now());
    state.handle_endpoint_result("boot-1", &asked[0], fixture_reply());
}

/// The board open on the Tasks view with task `list_n` selected, drawn once.
fn acting_state(list_n: usize) -> ClientShellState {
    let mut state = board_state();
    state.task_board.open = true;
    state.task_board.selected = Some(key(list_n));
    state.compose(WIDE + 26, 40).expect("frame");
    state
}

fn typed(state: &mut ClientShellState, text: &str) {
    for ch in text.chars() {
        let modifiers = if ch.is_uppercase() {
            KeyModifiers::SHIFT
        } else {
            KeyModifiers::NONE
        };
        state.handle_raw_events(vec![RawInputEvent::Key(TerminalKey::new(
            KeyCode::Char(ch),
            modifiers,
        ))]);
    }
}

fn board_rows(state: &mut ClientShellState) -> Vec<String> {
    frame_rows(&state.compose(WIDE + 26, 40).expect("frame"))
}

#[test]
fn d_marks_the_selected_task_done_naming_every_copy() {
    let mut state = acting_state(13);
    state.task_board.done_expanded = true;
    board_rows(&mut state);
    let outcome = press_key(&mut state, KeyCode::Char('d'));
    let (_, params) = one_act(&outcome);
    assert_eq!(params.action, "done");
    assert_eq!(
        params.copies,
        copies_of(13),
        "a merged task sends both copies"
    );
    assert_eq!(params.copies.len(), 2);
    assert_eq!(
        (
            &params.to,
            &params.direction,
            &params.file,
            &params.task,
            &params.who
        ),
        (&None, &None, &None, &None, &None)
    );
}

#[test]
fn x_asks_on_the_row_and_only_y_deletes() {
    let mut state = acting_state(1);
    let outcome = press_key(&mut state, KeyCode::Char('x'));
    assert!(act_requests(&outcome).is_empty(), "asks first");
    let rows = board_rows(&mut state);
    let task = rows
        .iter()
        .position(|row| row.contains("Per-client Chrome"))
        .expect("drawn");
    assert!(
        rows[task + 1..task + 3]
            .iter()
            .any(|row| row.contains("y  delete     n  keep")),
        "on the selected row: {rows:#?}"
    );
    for keep in [KeyCode::Char('n'), KeyCode::Esc, KeyCode::Char('q')] {
        press_key(&mut state, KeyCode::Char('x'));
        let outcome = press_key(&mut state, keep);
        assert!(act_requests(&outcome).is_empty(), "{keep:?} keeps it");
        assert!(state.task_board.open, "{keep:?} only answers the question");
        assert!(
            !board_rows(&mut state)
                .iter()
                .any(|row| row.contains("y  delete")),
            "{keep:?} closes the question"
        );
    }
    press_key(&mut state, KeyCode::Char('x'));
    let (_, params) = one_act(&press_key(&mut state, KeyCode::Char('y')));
    assert_eq!(params.action, "delete");
    assert_eq!(params.copies, copies_of(1));
}

#[test]
fn m_chooses_who_the_task_moves_to_or_takes_a_typed_name() {
    let mut state = acting_state(1);
    press_key(&mut state, KeyCode::Char('m'));
    assert!(
        board_rows(&mut state)
            .iter()
            .any(|row| row.contains("1  you     2  me     3  other…")),
        "the chooser shows on the row"
    );
    let (id, params) = one_act(&press_key(&mut state, KeyCode::Char('1')));
    assert_eq!(
        (params.action.as_str(), params.to.as_deref()),
        ("move", Some("you"))
    );
    assert_eq!(params.copies, copies_of(1));
    act_done_and_list_read(&mut state, &id);

    press_key(&mut state, KeyCode::Char('m'));
    let (id, params) = one_act(&press_key(&mut state, KeyCode::Char('2')));
    assert_eq!(params.to.as_deref(), Some("me"));
    act_done_and_list_read(&mut state, &id);

    press_key(&mut state, KeyCode::Char('m'));
    assert!(act_requests(&press_key(&mut state, KeyCode::Char('3'))).is_empty());
    typed(&mut state, "Priya K");
    assert!(
        board_rows(&mut state)
            .iter()
            .any(|row| row.contains("Move to: Priya K")),
        "the typed name shows on the row"
    );
    press_key(&mut state, KeyCode::Backspace);
    typed(&mut state, "R");
    let (id, params) = one_act(&press_key(&mut state, KeyCode::Enter));
    assert_eq!(params.to.as_deref(), Some("Priya R"));
    act_done_and_list_read(&mut state, &id);

    press_key(&mut state, KeyCode::Char('m'));
    press_key(&mut state, KeyCode::Char('3'));
    typed(&mut state, "Sam");
    assert!(act_requests(&press_key(&mut state, KeyCode::Esc)).is_empty());
    assert!(state.task_board.open, "escape cancels the name only");
    press_key(&mut state, KeyCode::Char('m'));
    assert!(act_requests(&press_key(&mut state, KeyCode::Esc)).is_empty());
}

#[test]
fn shift_j_and_k_move_the_task_down_and_up() {
    let mut state = acting_state(2);
    let shifted = |state: &mut ClientShellState, ch| {
        state.handle_raw_events(vec![RawInputEvent::Key(TerminalKey::new(
            KeyCode::Char(ch),
            KeyModifiers::SHIFT,
        ))])
    };
    let (id, params) = one_act(&shifted(&mut state, 'J'));
    assert_eq!(
        (params.action.as_str(), params.direction.as_deref()),
        ("reorder", Some("down"))
    );
    assert_eq!(params.copies, copies_of(2));
    act_done_and_list_read(&mut state, &id);
    let (_, params) = one_act(&shifted(&mut state, 'K'));
    assert_eq!(params.direction.as_deref(), Some("up"));
    assert_eq!(
        state.task_board.selected,
        Some(key(2)),
        "j and k still select; J and K move"
    );
}

#[test]
fn n_types_a_new_task_under_the_selected_one_into_its_file() {
    let mut state = board_state();
    state.task_board.open = true;
    state.compose(WIDE + 26, 40).expect("frame");
    let outcome = press_key(&mut state, KeyCode::Char('n'));
    assert!(act_requests(&outcome).is_empty());
    assert!(
        board_rows(&mut state)
            .iter()
            .any(|row| row.contains("Select a task first")),
        "with nothing selected, a notice says so"
    );

    let mut state = acting_state(4);
    press_key(&mut state, KeyCode::Char('n'));
    typed(&mut state, "Ship it");
    let rows = board_rows(&mut state);
    let task = rows
        .iter()
        .position(|row| row.contains("Say final"))
        .expect("drawn");
    assert!(
        rows[task + 1..task + 3]
            .iter()
            .any(|row| row.contains("New task: Ship it")),
        "under the selected task: {rows:#?}"
    );
    let (_, params) = one_act(&press_key(&mut state, KeyCode::Enter));
    assert_eq!(params.action, "new");
    assert_eq!(params.file.as_deref(), Some(copies_of(4)[0].file.as_str()));
    assert_eq!(params.who.as_deref(), Some("me"));
    assert_eq!(params.task.as_deref(), Some("Ship it"));
    assert!(params.copies.is_empty(), "new takes no copies");

    let mut state = acting_state(4);
    press_key(&mut state, KeyCode::Char('n'));
    assert!(
        act_requests(&press_key(&mut state, KeyCode::Enter)).is_empty(),
        "an empty task sends nothing"
    );
    press_key(&mut state, KeyCode::Char('n'));
    typed(&mut state, "never mind");
    assert!(act_requests(&press_key(&mut state, KeyCode::Esc)).is_empty());
    assert!(state.task_board.open);
    assert!(!board_rows(&mut state)
        .iter()
        .any(|row| row.contains("New task:")));
}

#[test]
fn a_waiting_action_shows_on_its_row_and_a_second_key_does_nothing() {
    let mut state = acting_state(1);
    let (id, _) = one_act(&press_key(&mut state, KeyCode::Char('d')));
    let rows = board_rows(&mut state);
    let task = rows
        .iter()
        .position(|row| row.contains("Per-client Chrome"))
        .expect("drawn");
    assert!(
        rows[task + 1..task + 3]
            .iter()
            .any(|row| row.contains("◐ done…")),
        "the row waits: {rows:#?}"
    );
    for code in [
        KeyCode::Char('d'),
        KeyCode::Char('x'),
        KeyCode::Char('m'),
        KeyCode::Char('n'),
    ] {
        let outcome = press_key(&mut state, code);
        assert!(act_requests(&outcome).is_empty(), "{code:?} while waiting");
    }
    assert!(
        !board_rows(&mut state)
            .iter()
            .any(|row| row.contains("y  delete") || row.contains("New task:")),
        "no question opens while one action waits"
    );
    state.handle_endpoint_result("boot-1", &id, act_reply(true, None));
    assert!(!board_rows(&mut state)
        .iter()
        .any(|row| row.contains("◐ done…")));
    let asked = tick_asks(&mut state, std::time::Instant::now());
    state.handle_endpoint_result("boot-1", &asked[0], fixture_reply());
    assert_eq!(
        act_requests(&press_key(&mut state, KeyCode::Char('d'))).len(),
        1
    );
}

// The server drops its kept list after each action and refuses an action on a file its
// list does not name, so a second action waits for the list read after the first.
#[test]
fn a_second_action_waits_for_the_list_read_after_the_first() {
    let start = std::time::Instant::now();
    let pressed_d =
        |state: &mut ClientShellState| act_requests(&press_key(state, KeyCode::Char('d'))).len();

    let mut state = acting_state(1);
    let (id, _) = one_act(&press_key(&mut state, KeyCode::Char('d')));
    state.handle_endpoint_result("boot-1", &id, act_reply(true, None));
    assert_eq!(
        pressed_d(&mut state),
        0,
        "answered, list not read again yet"
    );
    let asked = tick_asks(&mut state, start);
    assert_eq!(pressed_d(&mut state), 0, "the list is asked, not landed");
    state.handle_endpoint_result("boot-1", &asked[0], fixture_reply());
    assert_eq!(
        pressed_d(&mut state),
        1,
        "the list read after the action landed"
    );

    // A list asked before the action finished may predate it: its reply is not enough.
    let mut state = acting_state(1);
    let (id, _) = one_act(&press_key(&mut state, KeyCode::Char('d')));
    let crossed = tick_asks(&mut state, start);
    state.handle_endpoint_result("boot-1", &id, act_reply(false, Some("no such row")));
    state.handle_endpoint_result("boot-1", &crossed[0], fixture_reply());
    assert_eq!(
        pressed_d(&mut state),
        0,
        "the crossed list may predate the action"
    );
    let asked = tick_asks(&mut state, start);
    state.handle_endpoint_result("boot-1", &asked[0], fixture_reply());
    assert_eq!(pressed_d(&mut state), 1);

    // A failed read keeps the old list on screen, but it is still the old list.
    let mut state = acting_state(1);
    let (id, _) = one_act(&press_key(&mut state, KeyCode::Char('d')));
    state.handle_endpoint_result("boot-1", &id, act_reply(true, None));
    let asked = tick_asks(&mut state, start);
    let failed = Err(ClientShellEndpointError {
        code: Some("tasks_failed".into()),
        message: "claude-tasks: cannot read resume pointers".into(),
    });
    state.handle_endpoint_result("boot-1", &asked[0], failed);
    assert_eq!(pressed_d(&mut state), 0, "a failed read is not a list");
    let asked = tick_asks(&mut state, start + std::time::Duration::from_secs(3));
    state.handle_endpoint_result("boot-1", &asked[0], fixture_reply());
    assert_eq!(pressed_d(&mut state), 1);

    // An interrupted action may still have run: the same wait.
    let mut state = acting_state(1);
    let (id, _) = one_act(&press_key(&mut state, KeyCode::Char('d')));
    assert!(state.cancel_endpoint_request(&id));
    assert_eq!(
        pressed_d(&mut state),
        0,
        "interrupted, list not read again yet"
    );
    let asked = tick_asks(&mut state, start);
    state.handle_endpoint_result("boot-1", &asked[0], fixture_reply());
    assert_eq!(pressed_d(&mut state), 1);
}

#[test]
fn an_answered_action_reads_the_list_again_at_once() {
    let mut state = acting_state(1);
    let start = std::time::Instant::now();
    let asked = tick_asks(&mut state, start);
    state.handle_endpoint_result("boot-1", &asked[0], fixture_reply());
    let soon = start + std::time::Duration::from_millis(100);
    assert!(tick_asks(&mut state, soon).is_empty(), "read 100 ms ago");

    let (id, _) = one_act(&press_key(&mut state, KeyCode::Char('d')));
    state.handle_endpoint_result("boot-1", &id, act_reply(true, None));
    assert_eq!(
        tick_asks(&mut state, soon).len(),
        1,
        "ok: read again at once"
    );

    // A list asked before the action finished may predate it: once it lands, ask again.
    let mut state = acting_state(1);
    let (id, _) = one_act(&press_key(&mut state, KeyCode::Char('d')));
    let asked = tick_asks(&mut state, start);
    state.handle_endpoint_result("boot-1", &id, act_reply(false, Some("no such row")));
    assert!(tick_asks(&mut state, soon).is_empty(), "one list at a time");
    state.handle_endpoint_result("boot-1", &asked[0], fixture_reply());
    assert_eq!(
        tick_asks(&mut state, soon).len(),
        1,
        "the list that crossed the action is read again"
    );
}

#[test]
fn a_refused_or_failed_action_shows_why_in_the_notice_line() {
    let cases: [(
        Result<crate::api::schema::ResponseResult, ClientShellEndpointError>,
        &str,
    ); 5] = [
        (
            act_reply(false, Some("row changed since the list was read")),
            "row changed since the list was read",
        ),
        (
            Err(ClientShellEndpointError {
                code: Some("tasks_failed".into()),
                message: "bad arguments: one --row for each --file".into(),
            }),
            "bad arguments: one --row for each --file",
        ),
        (
            Err(ClientShellEndpointError {
                code: Some("tasks_timeout".into()),
                message: "tasks.command did not finish within 5000 ms".into(),
            }),
            "tasks.command did not finish within 5000 ms",
        ),
        (
            Err(ClientShellEndpointError {
                code: Some("endpoint_busy".into()),
                message: "this endpoint is still processing another command".into(),
            }),
            "this endpoint is still processing another command",
        ),
        (
            Err(ClientShellEndpointError {
                code: Some("invalid_params".into()),
                message: "move needs to".into(),
            }),
            "move needs to",
        ),
    ];
    for (reply, shown) in cases {
        let mut state = acting_state(1);
        let start = std::time::Instant::now();
        let asked = tick_asks(&mut state, start);
        state.handle_endpoint_result("boot-1", &asked[0], fixture_reply());
        let (id, _) = one_act(&press_key(&mut state, KeyCode::Char('d')));
        let (repaint, _) = state.handle_endpoint_result("boot-1", &id, reply);
        assert!(repaint);
        let rows = board_rows(&mut state);
        let line = rows
            .iter()
            .position(|row| row.contains(shown))
            .unwrap_or_else(|| panic!("{shown:?} not shown: {rows:#?}"));
        assert!(
            rows[line + 1].contains("notes file has a problem") && rows[line + 2].contains('─'),
            "the first line of the foot, above the problems and the rule: {rows:#?}"
        );
        assert_eq!(
            tick_asks(&mut state, start + std::time::Duration::from_millis(100)).len(),
            1,
            "{shown}: read again"
        );
    }
}

#[test]
fn a_server_without_tasks_act_hides_the_action_keys_and_ignores_them() {
    let mut state = acting_state(1);
    state.task_board.keys_shown = true;
    // The footer and the selected row's detail block between them.
    let shown = board_rows(&mut state).join("\n");
    for part in ["d  done", "x  delete", "m  move", "J/K  reorder", "n  new"] {
        assert!(shown.contains(part), "{part:?} in {shown:?}");
    }
    state.set_endpoint_methods(Some(vec!["tasks.list".into(), "pane.focus".into()]));
    let rows = board_rows(&mut state);
    for part in ["d  done", "x  delete", "m  move", "reorder", "n  new"] {
        assert!(
            !rows.iter().any(|row| row.contains(part)),
            "{part:?} hidden: {rows:#?}"
        );
    }
    for code in ['d', 'x', 'm', 'J', 'K', 'n'] {
        let outcome = press_key(&mut state, KeyCode::Char(code));
        assert!(act_requests(&outcome).is_empty(), "{code}");
        assert!(!outcome.repaint, "{code} does nothing");
    }
}

#[test]
fn action_keys_act_only_in_the_tasks_view() {
    let mut state = acting_state(1);
    state.task_board.view = BoardView::Work;
    state.compose(160, 40).expect("frame");
    for code in ['d', 'x', 'm', 'J', 'K', 'n'] {
        let outcome = press_key(&mut state, KeyCode::Char(code));
        assert!(act_requests(&outcome).is_empty(), "{code}");
    }
}

#[test]
fn a_question_left_open_closes_with_the_board() {
    // A double click on the selected row goes to its agent and closes the board.
    let mut state = acting_state(6);
    press_key(&mut state, KeyCode::Char('x'));
    let row = row_of(&state, &key(6));
    board_click(&mut state, row);
    board_click(&mut state, row);
    assert!(!state.task_board.open, "task 6 has an agent");
    state.toggle_task_board(BoardOpener::Key);
    state.compose(WIDE + 26, 40).expect("frame");
    press_key(&mut state, KeyCode::Down);
    assert_eq!(
        state.task_board.selected,
        Some(key(7)),
        "the next key moves the selection, not x's old question"
    );

    // The board's own key closes it too.
    let mut state = acting_state(1);
    press_key(&mut state, KeyCode::Char('m'));
    state.toggle_task_board(BoardOpener::Key);
    state.toggle_task_board(BoardOpener::Key);
    assert!(!board_rows(&mut state)
        .iter()
        .any(|row| row.contains("1 Nadine")));
}

#[test]
fn the_board_keeps_what_the_server_takes_and_draws_the_footer_from_it() {
    let mut board = fixture_board();
    assert!(board.can_act, "the fixture board's server takes tasks.act");
    let render = render_board(&board, WIDE, 40);
    assert!(render.rows[39].contains("d  done"), "{:#?}", render.rows);
    board.can_act = false;
    let render = render_board(&board, WIDE, 40);
    assert!(
        !render.rows.iter().any(|row| row.contains("d  done")),
        "{:#?}",
        render.rows
    );

    // The shell sets it from the server's methods each time it draws the open board.
    let mut state = acting_state(1);
    assert!(state.task_board.can_act);
    state.set_endpoint_methods(Some(vec!["tasks.list".into()]));
    state.compose(WIDE + 26, 40).expect("frame");
    assert!(!state.task_board.can_act);
}

// `enter` on a Work card asks the server to open its intent file in a popup, beside the
// command lane like `tasks.act`; the tests are the server.

/// The `tasks.open` requests `outcome` sends, each by its id; none may join the lane.
fn open_requests(outcome: &ClientShellInput) -> Vec<(String, String)> {
    outcome
        .actions
        .iter()
        .filter_map(|action| match action {
            ClientShellAction::EndpointSide { request, .. } => match &request.method {
                crate::api::schema::Method::TasksOpen(params) => {
                    Some((request.id.clone(), params.file.clone()))
                }
                _ => None,
            },
            ClientShellAction::Endpoint { request, .. } => {
                assert!(
                    !matches!(request.method, crate::api::schema::Method::TasksOpen(_)),
                    "tasks.open never joins the lane"
                );
                None
            }
            _ => None,
        })
        .collect()
}

/// The board open on the Work view with its first card selected, drawn once.
fn opening_state() -> ClientShellState {
    let mut state = work_state();
    press_key(&mut state, KeyCode::Tab);
    state.compose(160, 40).expect("frame");
    press_key(&mut state, KeyCode::Down);
    state.compose(160, 40).expect("frame");
    assert!(state.task_board.work_selected.is_some());
    state
}

fn not_listed(
    message: &str,
) -> Result<crate::api::schema::ResponseResult, ClientShellEndpointError> {
    Err(ClientShellEndpointError {
        code: Some("tasks_not_listed".into()),
        message: message.into(),
    })
}

#[test]
fn o_opens_the_selected_cards_file_and_the_board_closes_once_it_is_open() {
    let mut state = opening_state();
    let file = state.task_board.work_selected.clone().expect("selected");
    let sent = open_requests(&press_key(&mut state, KeyCode::Char('o')));
    assert_eq!(sent.len(), 1, "{sent:?}");
    let (id, sent_file) = sent[0].clone();
    assert_eq!(sent_file, file);
    assert!(
        open_requests(&press_key(&mut state, KeyCode::Char('o'))).is_empty(),
        "one open at a time"
    );
    assert!(state.task_board.open, "the board waits for the answer");

    let (repaint, actions) =
        state.handle_endpoint_result("boot-1", &id, Ok(crate::api::schema::ResponseResult::Ok {}));
    assert!(repaint);
    assert!(actions.is_empty());
    assert!(!state.task_board.open, "the popup shows over the panes");
    assert!(state.task_board.opening.is_none());
}

#[test]
fn enter_on_a_card_without_an_agent_opens_its_file_as_o_does() {
    let mut state = opening_state();
    let file = card("2026-09-23-per-client.md");
    state.task_board.work_selected = Some(file.clone());
    state.compose(160, 40).expect("frame");
    let outcome = press_key(&mut state, KeyCode::Enter);
    assert_eq!(focused_pane(&outcome), None, "no agent on it");
    let sent = open_requests(&outcome);
    assert_eq!(
        sent.iter().map(|(_, sent)| sent).collect::<Vec<_>>(),
        [&file]
    );
    assert!(state.task_board.open, "the board waits for the answer");
    // While that open waits, a second Enter sends nothing.
    assert!(
        open_requests(&press_key(&mut state, KeyCode::Enter)).is_empty(),
        "one open at a time"
    );
    // A card with an agent still goes to its tab.
    state.task_board.work_selected = Some(card("2026-09-25-task-board.md"));
    let outcome = press_key(&mut state, KeyCode::Enter);
    assert_eq!(focused_pane(&outcome), Some("w36:pV"));
    assert!(open_requests(&outcome).is_empty());
}

#[test]
fn enter_on_a_card_without_an_agent_does_nothing_on_a_server_without_tasks_open() {
    let mut state = opening_state();
    state.set_endpoint_methods(Some(vec!["tasks.list".into(), "tasks.act".into()]));
    state.task_board.work_selected = Some(card("2026-09-23-per-client.md"));
    state.compose(160, 40).expect("frame");
    let outcome = press_key(&mut state, KeyCode::Enter);
    assert!(outcome.actions.is_empty(), "{:?}", outcome.actions);
    // As `o`: a key that does nothing asks for no redraw.
    assert!(!outcome.repaint);
    assert!(state.task_board.open);
    assert!(state.task_board.notice.is_none());
}

#[test]
fn the_card_panel_says_what_enter_does_and_shows_open_file_once() {
    let mut board = work_board();
    let buttons = |board: &TaskBoard| {
        let render = render_board(board, 132, 40);
        assert!(render.hits.work_panel.height > 0, "{:#?}", render.rows);
        render.rows[usize::from(render.hits.work_panel.bottom()) - 2].clone()
    };
    board.work_selected = Some(card("2026-09-25-task-board.md"));
    let row = buttons(&board);
    assert!(
        row.contains("Enter  go to lead's tab     o  open file"),
        "{row:?}"
    );
    board.work_selected = Some(card("2026-09-23-per-client.md"));
    let row = buttons(&board);
    assert!(row.contains("Enter  open file"), "{row:?}");
    assert!(!row.contains("o  open file"), "{row:?}");
    board.can_open = false;
    let row = buttons(&board);
    assert!(
        !row.contains("Enter") && !row.contains("open file"),
        "{row:?}"
    );
}

#[test]
fn o_does_nothing_on_a_server_without_tasks_open() {
    let mut state = opening_state();
    state.set_endpoint_methods(Some(vec!["tasks.list".into(), "tasks.act".into()]));
    state.compose(160, 40).expect("frame");
    let outcome = press_key(&mut state, KeyCode::Char('o'));
    assert!(outcome.actions.is_empty(), "{:?}", outcome.actions);
    assert!(state.task_board.open);
    assert!(state.task_board.notice.is_none());
}

#[test]
fn a_file_the_list_does_not_name_shows_why_and_reads_the_list_again() {
    let mut state = opening_state();
    let start = std::time::Instant::now();
    let asked = tick_asks(&mut state, start);
    let work = Ok(crate::api::schema::ResponseResult::TasksList {
        list: serde_json::from_str(WORK_V1).expect("work fixture json"),
    });
    state.handle_endpoint_result("boot-1", &asked[0], work);
    let soon = start + std::time::Duration::from_millis(100);
    assert!(
        tick_asks(&mut state, soon).is_empty(),
        "read two seconds apart"
    );
    let (id, _) = open_requests(&press_key(&mut state, KeyCode::Char('o')))[0].clone();
    let message = "no task list is kept; read it with tasks.list first";
    let (repaint, _) = state.handle_endpoint_result("boot-1", &id, not_listed(message));
    assert!(repaint);
    assert!(state.task_board.open);
    let rows = frame_rows(&state.compose(160, 40).expect("frame"));
    assert!(rows.iter().any(|row| row.contains(message)), "{rows:#?}");
    assert_eq!(
        tick_asks(&mut state, soon).len(),
        1,
        "the list is read again at once"
    );

    // Any other error shows its message and leaves the list alone.
    let (id, _) = open_requests(&press_key(&mut state, KeyCode::Char('o')))[0].clone();
    state.handle_endpoint_result(
        "boot-1",
        &id,
        Err(ClientShellEndpointError {
            code: Some("tasks_open_failed".into()),
            message: "popup already open".into(),
        }),
    );
    assert_eq!(
        state.task_board.notice.as_deref(),
        Some("popup already open")
    );

    // A server error with no message is named by its code.
    let (id, _) = open_requests(&press_key(&mut state, KeyCode::Char('o')))[0].clone();
    state.handle_endpoint_result(
        "boot-1",
        &id,
        Err(ClientShellEndpointError {
            code: Some("tasks_open_failed".into()),
            message: String::new(),
        }),
    );
    assert_eq!(
        state.task_board.notice.as_deref(),
        Some("tasks_open_failed")
    );
}

#[test]
fn a_cancelled_open_lets_o_ask_again() {
    let mut state = opening_state();
    let (id, _) = open_requests(&press_key(&mut state, KeyCode::Char('o')))[0].clone();
    assert!(state.cancel_endpoint_request(&id));
    assert!(state.task_board.opening.is_none());
    assert_eq!(
        open_requests(&press_key(&mut state, KeyCode::Char('o'))).len(),
        1
    );
}

// Design pass (2026-09-26): fixes and window-size layout.

fn wheel(state: &mut ClientShellState, column: u16, row: u16, down: bool) -> ClientShellInput {
    state.handle_raw_events(vec![RawInputEvent::Mouse(crossterm::event::MouseEvent {
        kind: if down {
            MouseEventKind::ScrollDown
        } else {
            MouseEventKind::ScrollUp
        },
        column,
        row,
        modifiers: KeyModifiers::empty(),
    })])
}

/// The Work view with `file` selected by a click, drawn at `width` x `height`.
fn work_selected_state(file: &str, width: u16, height: u16) -> ClientShellState {
    let mut state = work_state();
    state.task_board.view = BoardView::Work;
    state.task_board.work_selected = Some(card(file));
    state.task_board.reveal_selected = true;
    state.compose(width, height).expect("frame");
    state
}

#[test]
fn wheel_in_the_selected_cards_column_stays_where_it_scrolled() {
    // B5b: 50 rows, so the column still holds two nine-row cards under the panel and the
    // selected card stays in view at the place the wheel left it.
    let mut state = work_selected_state("shipped-1.md", 160, 50);
    let shipped = state.hits.task_board.work_column_areas[4];
    for _ in 0..3 {
        wheel(&mut state, shipped.x + 1, shipped.y + 1, true);
    }
    state.compose(160, 50).expect("frame");
    state.compose(160, 50).expect("frame");
    assert_eq!(
        state.task_board.work_scroll[4], 3,
        "the wheel keeps its place: {:?}",
        state.task_board.work_scroll
    );
}

#[test]
fn needs_you_rows_leave_the_action_blank_where_the_state_only_says_needs_you() {
    let render = render_board(&fixture_board(), WIDE, 40);
    let row = render.row_with("Ask eng for an OIDC");
    assert!(
        !render.rows[row].contains("needs you"),
        "the section says it: {:?}",
        render.rows[row]
    );
    let action = split_cells(&render.rows[render.row_with("│ #")])
        .iter()
        .position(|name| name == "Action")
        .expect("an Action column");
    let blank = render.rows[row]
        .split('│')
        .nth(action + 1)
        .expect("its cell");
    assert!(blank.trim().is_empty(), "{:?}", render.rows[row]);
    // Any other State shows as written.
    let chrome = render.row_with("Per-client Chrome");
    assert!(render.rows[chrome].contains("go on the pairing"));
}

/// Wide enough for every header count, the read time, the filter box and the sort button.
const HEADER_WIDE: u16 = 201;

#[test]
fn the_action_chip_reads_its_label_then_its_count_and_n_of_m_under_a_filter() {
    let mut board = fixture_board();
    board.filter = "Chrome".into();
    let chips = render_board(&board, HEADER_WIDE + 8, 40).rows[2].clone();
    assert!(chips.contains(" ! Action required 1 of 5 "), "{chips:?}");
    board.space = Some("herdr".into());
    board.filter.clear();
    let chips = render_board(&board, HEADER_WIDE, 40).rows[2].clone();
    // The label stays the same whatever the count.
    assert!(chips.contains(" ! Action required 1 "), "{chips:?}");
    // The sidebar's tab count keeps its own words; only the board's chip counts tasks.
    assert!(!chips.contains("Needs you"), "{chips:?}");
}

#[test]
fn task_board_header_counts_action_and_says_when_the_list_was_read() {
    let render = render_board(&fixture_board(), HEADER_WIDE, 40);
    let header = &render.rows[0];
    assert!(
        render.rows[2].contains("! Action required 5"),
        "{:?}",
        render.rows[2]
    );
    assert!(header.contains("Updated 8:05 am"), "{header:?}");
    assert_eq!(
        render.cell(0, "Updated 8:05 am").fg,
        render.palette.overlay0
    );
    for (generated, shown) in [
        ("2026-09-25T00:00:59+05:30", "Updated 12:00 am"),
        ("2026-09-25T12:30:00Z", "Updated 12:30 pm"),
        ("2026-09-25T14:00:00+05:30", "Updated 2:00 pm"),
        ("2026-09-25T23:59:00+05:30", "Updated 11:59 pm"),
    ] {
        let mut board = TaskBoard::default();
        board.store.receive_json(
            &LIST_V1.replace("2026-09-25T08:05:12+05:30", generated),
            std::time::Instant::now(),
        );
        let header = render_board(&board, HEADER_WIDE, 40).rows[0].clone();
        assert!(header.contains(shown), "{generated}: {header:?}");
    }
    let mut board = TaskBoard::default();
    board.store.receive_json(
        &LIST_V1.replace("2026-09-25T08:05:12+05:30", "soon"),
        std::time::Instant::now(),
    );
    let header = render_board(&board, HEADER_WIDE, 40).rows[0].clone();
    assert!(!header.contains("Updated "), "no time to show: {header:?}");

    let mut board = fixture_board();
    board.space = Some("herdr".into());
    let chips = render_board(&board, HEADER_WIDE, 40).rows[2].clone();
    assert!(chips.contains("! Action required 1"), "{chips:?}");
}

#[test]
fn a_long_project_is_cut_before_the_task_and_shown_whole_in_the_detail() {
    let mut board = TaskBoard {
        open: true,
        sort: crate::client::shell::task_board::TaskSort::Newest,
        ..TaskBoard::default()
    };
    let long = |list: &str| {
        list.replace(r#""space": "oakwood""#, r#""space": "oakwood-workspace""#)
            .replace(
                r#""project": "rfc-tool""#,
                r#""project": "rfc-tool-and-its-long-name""#,
            )
    };
    board
        .store
        .receive_json(&long(LIST_V1), std::time::Instant::now());
    let render = render_board(&board, NARROW, 60);
    let row = render.row_with("· Approve the");
    assert!(
        render.rows[row].contains("│ rfc-tool-… · Approve the"),
        "the project takes at most half the cell, cut at a word: {:?}",
        render.rows[row]
    );
    assert!(
        render.rows[row + 1].starts_with(" ╰"),
        "one line, no wrap: {:#?}",
        &render.rows[row..row + 2]
    );
    // Completed, its block shows the project its row cuts, whole.
    board.store.receive_json(
        &long(&list_v1_in_section(3, "completed")),
        std::time::Instant::now(),
    );
    board.done_expanded = true;
    board.selected = Some(RowKey::Task(TaskKey {
        space: "oakwood-workspace".into(),
        task: "Approve the polished wording".into(),
    }));
    board.open_row = board.selected.clone();
    let render = render_board(&board, WIDE, 60);
    assert!(
        render
            .rows
            .iter()
            .any(|row| row.contains("Project  rfc-tool-and-its-long-name")),
        "the detail shows it whole: {:#?}",
        render.rows
    );
}

#[test]
fn a_selected_card_in_a_crowded_column_is_drawn_whole_and_never_hidden() {
    for (width, height) in [(WIDE, 23), (64, 38), (WIDE, 17)] {
        let mut board = work_board();
        board.work_selected = Some(card("shipped-5.md"));
        board.reveal_selected = true;
        let render = render_board(&board, width, height);
        let (rect, _) = render
            .hits
            .cards
            .iter()
            .find(|(_, file)| *file == card("shipped-5.md"))
            .cloned()
            .unwrap_or_else(|| panic!("{width}x{height}: drawn: {:#?}", render.rows));
        assert_eq!(rect.height, 9, "{width}x{height}: a whole card");
        assert!(
            render.rows[rect.y as usize + 3].contains("Double count of"),
            "{width}x{height}: {:#?}",
            render.rows
        );
        let corner = &render.buffer[(rect.x, rect.y)];
        assert_eq!(
            (corner.symbol(), corner.fg),
            ("╭", render.palette.accent),
            "{width}x{height}: the selected card's box"
        );
    }
}

#[test]
fn a_scrolled_column_says_how_many_cards_are_above_and_its_markers_page_by_click() {
    let mut state = work_state();
    state.task_board.view = BoardView::Work;
    state.compose(160, 40).expect("frame");
    let shipped = state.hits.task_board.work_column_areas[4];
    let marker = |state: &ClientShellState, down: bool| {
        state
            .hits
            .task_board
            .work_markers
            .iter()
            .find(|marker| marker.column == 4 && marker.down == down)
            .map(|marker| marker.rect)
    };
    assert!(marker(&state, false).is_none(), "nothing above yet");
    let more = marker(&state, true).expect("▼ N more");
    let page = state
        .hits
        .task_board
        .cards
        .iter()
        .filter(|(rect, _)| shipped.contains(rect.as_position()))
        .count();
    assert!(page > 1, "{page}");

    board_click(&mut state, more);
    let rows = frame_rows(&state.compose(160, 40).expect("frame"));
    assert_eq!(state.task_board.work_scroll[4], page, "one page down");
    let above = marker(&state, false).expect("▲ N above");
    // The label row sits over the stage's underline (B5b).
    assert_eq!(above.y, shipped.y - 2, "in the column's label row");
    assert!(
        rows[above.y as usize].contains(&format!("▲ {page}")),
        "{:?}",
        rows[above.y as usize]
    );

    board_click(&mut state, above);
    state.compose(160, 40).expect("frame");
    assert_eq!(state.task_board.work_scroll[4], 0, "one page up");

    wheel(&mut state, shipped.x + 1, shipped.y + 1, true);
    let rows = frame_rows(&state.compose(160, 40).expect("frame"));
    assert!(
        rows[shipped.y as usize - 2].contains("▲ 1"),
        "the wheel too: {:?}",
        rows[shipped.y as usize - 2]
    );
}

#[test]
fn a_second_click_on_the_selected_card_opens_its_file_as_enter_does() {
    let mut state = work_state();
    state.task_board.view = BoardView::Work;
    state.compose(160, 40).expect("frame");
    let file = card("2026-09-24-grey-icon.md");
    let rect = |state: &ClientShellState| {
        state
            .hits
            .task_board
            .cards
            .iter()
            .find(|(_, drawn)| *drawn == file)
            .map(|(rect, _)| *rect)
            .expect("drawn")
    };
    let at = rect(&state);
    let first = board_click(&mut state, at);
    assert!(open_requests(&first).is_empty(), "the first click selects");
    assert_eq!(state.task_board.work_selected.as_ref(), Some(&file));
    state.compose(160, 40).expect("frame");
    let at = rect(&state);
    let second = board_click(&mut state, at);
    assert_eq!(
        open_requests(&second)
            .into_iter()
            .map(|(_, sent)| sent)
            .collect::<Vec<_>>(),
        [file]
    );
}

#[test]
fn enter_on_a_task_with_no_matching_space_opens_its_notes_file_and_on_one_with_an_agent_goes_there()
{
    let mut state = board_state();
    state.task_board.open = true;
    state.compose(106, 40).expect("frame");
    let file = "/home/user/devbox/Workdir/oakwood-workspace/WORKSPACE-STATUS.md";

    // The default snapshot has no "oakwood" workspace, so Enter falls back to the file.
    state.task_board.selected = Some(key(1));
    let rows = frame_rows(&state.compose(106, 40).expect("frame"));
    assert!(
        rows.iter().any(|row| row.contains("Enter  go to space")),
        "the detail says what enter does: {rows:#?}"
    );
    let outcome = press_key(&mut state, KeyCode::Enter);
    assert_eq!(focused_pane(&outcome), None);
    let sent = open_requests(&outcome);
    assert_eq!(
        sent.iter()
            .map(|(_, sent)| sent.as_str())
            .collect::<Vec<_>>(),
        [file]
    );
    assert!(state.task_board.open, "the board waits for the answer");
    let (repaint, _) = state.handle_endpoint_result(
        "boot-1",
        &sent[0].0,
        Ok(crate::api::schema::ResponseResult::Ok {}),
    );
    assert!(repaint && !state.task_board.open, "the popup shows");
    assert_eq!(
        state.task_board.selected,
        Some(key(1)),
        "the board keeps its place"
    );

    // A double click on the selected row acts as enter does.
    state.task_board.open = true;
    state.compose(106, 40).expect("frame");
    let row = row_of(&state, &key(1));
    board_click(&mut state, row);
    let outcome = board_click(&mut state, row);
    assert_eq!(open_requests(&outcome).len(), 1, "{:?}", outcome.actions);

    // A task with an agent goes to it, and opens nothing.
    let mut state = board_state();
    state.task_board.open = true;
    state.task_board.selected = Some(key(6));
    state.compose(106, 40).expect("frame");
    let outcome = press_key(&mut state, KeyCode::Enter);
    assert_eq!(focused_pane(&outcome), Some("w3:p1"));
    assert!(open_requests(&outcome).is_empty());
}

/// The `workspace.focus` requests `outcome` sends, by workspace id.
fn workspace_focus_requests(outcome: &ClientShellInput) -> Vec<String> {
    outcome
        .actions
        .iter()
        .filter_map(|action| match action {
            ClientShellAction::Endpoint { request, .. } => match &request.method {
                crate::api::schema::Method::WorkspaceFocus(target) => {
                    Some(target.workspace_id.clone())
                }
                _ => None,
            },
            _ => None,
        })
        .collect()
}

/// A snapshot with one workspace labelled `label`, its directory `cwd`.
fn workspace(workspace_id: &str, label: &str, cwd: &str) -> ClientShellWorkspace {
    ClientShellWorkspace {
        workspace_id: workspace_id.into(),
        active_tab_id: "tab_1".into(),
        new_workspace_cwd: cwd.into(),
        number: 1,
        label: label.into(),
        custom_label: true,
        branch: None,
        git_ahead_behind: None,
        tokens: Vec::new(),
        worktree: None,
        focused: false,
        agent_status: crate::api::schema::AgentStatus::Idle,
    }
}

/// An agent's pane in `workspace_id`, its tab named `name`, in `status` with `tokens`.
fn live_agent(
    pane_id: &str,
    workspace_id: &str,
    name: &str,
    status: crate::api::schema::AgentStatus,
    tokens: &[(&str, &str)],
) -> crate::protocol::ClientShellAgent {
    crate::protocol::ClientShellAgent {
        pane_id: pane_id.into(),
        workspace_id: workspace_id.into(),
        tab_id: "tab_1".into(),
        name: Some(name.into()),
        display_agent: None,
        agent: Some("claude".into()),
        title: None,
        terminal_title: None,
        terminal_title_stripped: None,
        agent_status: status,
        state_change_seq: 0,
        state_labels: Vec::new(),
        tokens: tokens
            .iter()
            .map(|(key, value)| ((*key).into(), (*value).into()))
            .collect(),
        focused: false,
    }
}

/// A tab at work in `space`, on `doing` when it says.
fn working(pane_id: &str, space: &str, tab: &str, doing: Option<&str>) -> BoardAgent {
    BoardAgent {
        pane_id: pane_id.into(),
        space: space.into(),
        tab: tab.into(),
        state: AgentState::Working(doing.map(Into::into)),
        lead: None,
        mark: (LEAD_SPINNER, Palette::catppuccin().yellow),
        // What it says it does is its `doing` label too.
        labels: AgentLabels {
            doing: doing.map(Into::into),
            ..AgentLabels::default()
        },
    }
}

#[test]
fn board_agents_reads_every_agent_its_state_and_its_lead_from_the_snapshot() {
    use crate::api::schema::AgentStatus;
    use crate::client::shell::task_board::board_agents;
    let mut snap = snapshot();
    snap.workspaces = vec![
        workspace("ws_h", "herdr", "/h"),
        workspace("ws_z", "oakwood", "/z"),
    ];
    snap.agents = vec![
        live_agent(
            "p1",
            "ws_h",
            "herdr-w1",
            AgentStatus::Working,
            &[("stage", "build"), ("task", "round 13a")],
        ),
        live_agent(
            "p2",
            "ws_h",
            "herdr-lead",
            AgentStatus::Working,
            &[("stage", "build · 13a")],
        ),
        live_agent(
            "p3",
            "ws_z",
            "oak-w2",
            AgentStatus::Working,
            &[("task", "  ")],
        ),
        live_agent(
            "p4",
            "ws_z",
            "oak-lead",
            AgentStatus::Idle,
            &[("ask", "pick A or B")],
        ),
        live_agent("p5", "ws_z", "oak-w3", AgentStatus::Blocked, &[]),
        live_agent("p6", "ws_z", "oak-w4", AgentStatus::Idle, &[]),
        live_agent(
            "p7",
            "ws_z",
            "oak-w5",
            AgentStatus::Done,
            &[("ask", "`**`")],
        ),
    ];
    let config = ClientShellConfig::from_config(&Config::default());
    let got: Vec<(String, String, String, AgentState)> =
        board_agents(Some(&snap), &config, None, 0)
            .into_iter()
            .map(|agent| (agent.pane_id, agent.space, agent.tab, agent.state))
            .collect();
    let want = |pane: &str, space: &str, tab: &str, state| {
        (pane.to_owned(), space.to_owned(), tab.to_owned(), state)
    };
    assert_eq!(
        got,
        [
            want(
                "p1",
                "herdr",
                "herdr-w1",
                AgentState::Working(Some("round 13a".into()))
            ),
            want(
                "p2",
                "herdr",
                "herdr-lead",
                AgentState::Working(Some("Build".into()))
            ),
            want("p3", "oakwood", "oak-w2", AgentState::Working(None)),
            want(
                "p4",
                "oakwood",
                "oak-lead",
                AgentState::Waiting(Some("pick A or B".into()))
            ),
            want("p5", "oakwood", "oak-w3", AgentState::Waiting(None)),
            want("p6", "oakwood", "oak-w4", AgentState::Resting),
            want("p7", "oakwood", "oak-w5", AgentState::Resting),
        ]
    );
    assert!(board_agents(None, &config, None, 0).is_empty());
    // A worker names its lead's pane in its space; a lead elsewhere or none is no link.
    snap.agents[0].tokens.push(("lead".into(), "p2".into()));
    snap.agents[2].tokens.push(("lead".into(), "p2".into()));
    snap.agents[4].tokens.push(("lead".into(), "p9".into()));
    let leads: Vec<Option<String>> = board_agents(Some(&snap), &config, None, 0)
        .into_iter()
        .map(|agent| agent.lead)
        .collect();
    assert_eq!(
        leads,
        [Some("p2".to_owned()), None, None, None, None, None, None]
    );
}

/// `lead`'s worker at work in `space`, on `doing` when it says, in the sub-agent spinner.
fn worker(pane_id: &str, space: &str, tab: &str, lead: &str, doing: Option<&str>) -> BoardAgent {
    BoardAgent {
        lead: Some(lead.into()),
        mark: (WORKER_SPINNER, Palette::catppuccin().yellow),
        ..working(pane_id, space, tab, doing)
    }
}

/// An agent in `space` neither working nor waiting on Nadine.
fn resting(pane_id: &str, space: &str, tab: &str) -> BoardAgent {
    BoardAgent {
        state: AgentState::Resting,
        mark: ("○", Palette::catppuccin().green),
        ..working(pane_id, space, tab, None)
    }
}

/// herdr's lead at work with two workers, and oakwood's lead at rest with one at work.
fn lead_and_workers() -> Vec<BoardAgent> {
    vec![
        working("w2:p1", "herdr", "herdr-lead", Some("build · w1 on 13a")),
        worker(
            "w2:p2",
            "herdr",
            "herdr-w1",
            "w2:p1",
            Some("Round 13a on feat/task-board"),
        ),
        resting("w3:p1", "oakwood", "oak-lead"),
        worker("w2:p3", "herdr", "herdr-w2", "w2:p1", Some("tidy the rail")),
        worker("w3:p2", "oakwood", "oak-w1", "w3:p1", None),
    ]
}

/// The fixture's tasks with the Work fixture's cards.
fn tasks_and_work_board() -> TaskBoard {
    let mut list = parse_task_list(LIST_V1).expect("fixture");
    list.work = parse_task_list(WORK_V1).expect("work fixture").work;
    let mut board = fixture_board();
    board.can_open = true;
    board.store.receive(Ok(list), std::time::Instant::now());
    board
}

#[test]
fn live_shows_each_lead_with_its_working_workers_under_it() {
    let mut board = tasks_and_work_board();
    let agents = lead_and_workers();
    let render = render_board_with_agents(&board, 186, 46, &agents);
    let heading = render.row_with("IN PROGRESS ");
    assert!(
        render.rows[heading].ends_with("IN PROGRESS 4 ▼"),
        "the lead at rest is not counted: {:?}",
        render.rows[heading]
    );
    let row = |name: &str| render.row_with(&format!(" {name} "));
    let lead = row("herdr-lead");
    assert_eq!(
        [row("herdr-w1"), row("herdr-w2"), row("oak-w1")],
        [lead + 1, lead + 2, lead + 3],
        "{:#?}",
        render.rows
    );
    // No tree: every spinner in one straight column, lead and worker alike, no connector.
    assert!(render.rows[lead].contains(&format!("│ {LEAD_SPINNER} ")));
    let spinner_column = |y: usize, spinner: &str| render.column(y, spinner);
    let column = spinner_column(lead, LEAD_SPINNER);
    for y in [lead + 1, lead + 2] {
        assert!(
            render.rows[y].contains(&format!("│ {WORKER_SPINNER} ")),
            "{:?}",
            render.rows[y]
        );
        assert_eq!(
            spinner_column(y, WORKER_SPINNER),
            column,
            "{:#?}",
            render.rows
        );
    }
    assert!(
        !render
            .rows
            .iter()
            .any(|row| row.contains("│ ├") || row.contains("│ ╰")),
        "{:#?}",
        render.rows
    );
    // A lead at rest has no row; its working worker is a top row, with no connector.
    assert!(
        !render.rows.iter().any(|row| row.contains(" oak-lead ")),
        "{:#?}",
        render.rows
    );
    assert!(
        render.rows[lead + 3].contains(&format!("│ {WORKER_SPINNER} ")),
        "{:?}",
        render.rows[lead + 3]
    );
    let palette = &render.palette;
    assert_eq!(render.cell(lead + 1, WORKER_SPINNER).fg, palette.yellow);
    assert_eq!(render.cell(lead + 3, "oak-w1").fg, palette.overlay1);
    // The lead at rest is no stop for the selection.
    let at_rest = RowKey::Agent("w3:p1".into());
    assert!(!render.hits.spans.iter().any(|(key, _, _)| *key == at_rest));
    assert!(!render.hits.rows.iter().any(|(_, key)| *key == at_rest));

    // A working lead the search keeps out has no row; its worker the search keeps is a top row.
    board.filter = "rail".into();
    let render = render_board_with_agents(&board, 186, 46, &agents);
    assert!(
        !render.rows.iter().any(|row| row.contains(" herdr-lead ")),
        "{:#?}",
        render.rows
    );
    let worker = render.row_with(" herdr-w2 ");
    assert!(
        render.rows[worker].contains(&format!("│ {WORKER_SPINNER} ")),
        "{:?}",
        render.rows[worker]
    );
    board.filter.clear();

    // An open worker's row: its task, what it does and how long, whole in their cells; its
    // block: its card and its space.
    board.selected = Some(RowKey::Agent("w2:p2".into()));
    board.open_row = board.selected.clone();
    let render = render_board_with_agents(&board, 186, 46, &agents);
    let at = render.row_with(" herdr-w1 ");
    let cells = split_cells(&render.rows[at]);
    assert_eq!(
        cells[1..],
        [
            "herdr-w1",
            "Never cut the lead's name",
            "working",
            "Round 13a on feat/task-board",
            "12m"
        ],
        "{cells:?}"
    );
    let block: Vec<&str> = render.rows[at + 1..at + 5]
        .iter()
        .map(String::as_str)
        .collect();
    let field = |line: &str, label: &str, value: &str| {
        let rest = line
            .split_once(&format!("│     {label} "))
            .map(|(_, rest)| rest.trim_start())
            .unwrap_or_else(|| panic!("{label}: {block:#?}"));
        assert!(rest.starts_with(value), "{label}: {line:?}");
    };
    // No task of its links a card: its block names its lead, then its keys.
    field(block[0], "Lead", "herdr-lead");
    assert!(
        block[2].contains("Enter  go to agent     ←  close"),
        "{block:#?}"
    );
}

/// The board's state with herdr's lead at work and its worker `herdr-w1` on task 7 and the
/// task board card; `focused` names the pane the snapshot focuses.
fn worker_state(focused: &str) -> ClientShellState {
    use crate::api::schema::AgentStatus;
    let mut state = board_state();
    let mut snap = snapshot();
    snap.workspaces = vec![
        workspace("ws_z", "oakwood", "/z"),
        workspace("ws_h", "herdr", "/h"),
    ];
    snap.agents = vec![
        live_agent("w2:p1", "ws_h", "herdr-lead", AgentStatus::Working, &[]),
        live_agent(
            "w2:p2",
            "ws_h",
            "herdr-w1",
            AgentStatus::Working,
            &[("lead", "w2:p1"), ("task", "Writing report.md")],
        ),
        live_agent("w3:p1", "ws_z", "oak-lead", AgentStatus::Idle, &[]),
    ];
    snap.focused_workspace_id = snap
        .agents
        .iter()
        .find(|agent| agent.pane_id == focused)
        .map(|agent| agent.workspace_id.clone());
    snap.focused_pane_id = Some(focused.into());
    state.set_snapshot(Box::new(snap));
    let mut list = parse_task_list(LIST_V1).expect("fixture");
    list.work = parse_task_list(WORK_V1).expect("work fixture").work;
    state
        .task_board
        .store
        .receive(Ok(list), std::time::Instant::now());
    state
}

/// `worker_state` with a sidebar tree to click: herdr's first tab holds a plain shell (the
/// space row stands for it), herdr-w1 and another plain shell, its second herdr-lead alone;
/// oakwood's holds oak-lead.
fn tree_worker_state(focused: &str) -> ClientShellState {
    let mut state = worker_state(focused);
    let mut snap = state.snapshot.as_deref().cloned().expect("snapshot");
    let base_tab = snap.tabs[0].clone();
    let base_pane = snap.panes[0].clone();
    let tab = |tab_id: &str, workspace_id: &str| {
        let mut tab = base_tab.clone();
        tab.tab_id = tab_id.into();
        tab.workspace_id = workspace_id.into();
        tab.focused = false;
        tab
    };
    let pane = |pane_id: &str, workspace_id: &str, tab_id: &str| {
        let mut pane = base_pane.clone();
        pane.pane_id = pane_id.into();
        pane.workspace_id = workspace_id.into();
        pane.tab_id = tab_id.into();
        pane.focused = pane_id == focused;
        pane
    };
    for workspace in &mut snap.workspaces {
        workspace.active_tab_id = format!("tab_{}", workspace.workspace_id);
    }
    snap.tabs = vec![
        tab("tab_ws_z", "ws_z"),
        tab("tab_ws_h", "ws_h"),
        tab("tab_lead", "ws_h"),
    ];
    snap.tabs[2].number = 2;
    snap.panes = vec![
        pane("w3:p1", "ws_z", "tab_ws_z"),
        pane("shell_top", "ws_h", "tab_ws_h"),
        pane("w2:p2", "ws_h", "tab_ws_h"),
        pane("shell_h", "ws_h", "tab_ws_h"),
        pane("w2:p1", "ws_h", "tab_lead"),
    ];
    for agent in &mut snap.agents {
        agent.tab_id = snap
            .panes
            .iter()
            .find(|pane| pane.pane_id == agent.pane_id)
            .map_or_else(String::new, |pane| pane.tab_id.clone());
    }
    snap.focused_tab_id = snap
        .panes
        .iter()
        .find(|pane| pane.pane_id == focused)
        .map(|pane| pane.tab_id.clone());
    state.set_snapshot(Box::new(snap));
    state
}

/// The tree row that focuses `pane`, as the last frame drew it.
fn tree_pane_row(state: &ClientShellState, pane: &str) -> Rect {
    use crate::client::shell::tree_sidebar::TreeTarget;
    state
        .hits
        .tree_rows
        .iter()
        .find(|(_, target)| match target {
            TreeTarget::Pane(id) => id == pane,
            TreeTarget::AgentTab { pane_id, .. } => pane_id == pane,
            _ => false,
        })
        .map(|(rect, _)| *rect)
        .unwrap_or_else(|| panic!("no tree row for {pane}: {:?}", state.hits.tree_rows))
}

/// A click on the tree row that focuses `pane`.
fn click_tree_pane(state: &mut ClientShellState, pane: &str) -> ClientShellInput {
    let row = tree_pane_row(state, pane);
    board_click(state, row)
}

/// Links the `lead-name` card, in PLAN with its steps open, to task 7, herdr-w1's on `w2:p2`, by the slug `s1` only: the
/// card names no agent, as the reader sends it; `done` puts the task in DONE.
fn link_lead_name_to_task_7(store: &mut TaskBoardStore, done: bool) {
    let mut list = store.list().cloned().expect("a list");
    let card = list
        .work
        .iter_mut()
        .find(|card| card.file.ends_with("2026-09-25-lead-name.md"))
        .expect("the lead-name card");
    assert!(card.agent.is_none());
    card.slug = Some("s1".into());
    let task = list
        .tasks
        .iter_mut()
        .find(|task| task.n == 7)
        .expect("task 7");
    task.card = Some("s1".into());
    if done {
        task.group = TaskGroup::Done;
    }
    store.receive(Ok(list), std::time::Instant::now());
}

#[test]
fn the_board_key_opens_on_a_focused_agents_space_with_that_agent_alone() {
    // herdr-w1, a worker in herdr beside its lead: herdr, and herdr-w1 alone.
    let mut state = worker_state("w2:p2");
    prefix_key(&mut state, 'a');
    assert!(state.task_board.open);
    assert_eq!(state.task_board.space.as_deref(), Some("herdr"));
    assert_eq!(state.task_board.agent.as_deref(), Some("w2:p2"));
    let rows = frame_rows(&state.compose(186, 46).expect("frame"));
    assert!(
        rows.iter()
            .any(|row| row.contains("Showing herdr-w1's tasks · Esc shows all agents")),
        "{rows:#?}"
    );
    assert!(
        !rows.iter().any(|row| row.contains("Say final")),
        "{rows:#?}"
    );
    prefix_key(&mut state, 'a');
    // The lead, a worker's or not, is followed too.
    let mut state = worker_state("w2:p1");
    prefix_key(&mut state, 'a');
    assert_eq!(state.task_board.space.as_deref(), Some("herdr"));
    assert_eq!(state.task_board.agent.as_deref(), Some("w2:p1"));
    // oak-lead is oakwood's one agent: its space shows whole, as that is its own.
    let mut state = worker_state("w3:p1");
    prefix_key(&mut state, 'a');
    assert_eq!(state.task_board.space.as_deref(), Some("oakwood"));
    let rows = frame_rows(&state.compose(186, 46).expect("frame"));
    assert!(
        !rows.iter().any(|row| row.contains("Showing ")),
        "{rows:#?}"
    );
    // A plain shell focused: every space, no agent, whatever the board showed before.
    let mut state = tree_worker_state("shell_h");
    state.task_board.space = Some("herdr".into());
    state.task_board.agent = Some("w2:p2".into());
    prefix_key(&mut state, 'a');
    assert!(state.task_board.open);
    assert_eq!(state.task_board.space, None);
    assert_eq!(state.task_board.agent, None);
}

#[test]
fn the_sidebar_icon_opens_on_a_focused_agents_space_with_that_agent_alone() {
    // The lead: herdr and herdr-lead alone, no row picked, as it works for no one.
    let mut state = worker_state("w2:p1");
    state.compose(186, 46).expect("frame");
    let icon = state.hits.tree_task_board;
    board_click(&mut state, icon);
    assert!(state.task_board.open);
    assert_eq!(state.task_board.space.as_deref(), Some("herdr"));
    assert_eq!(state.task_board.agent.as_deref(), Some("w2:p1"));
    assert_eq!(state.task_board.selected, None);
    // A worker: its own IN PROGRESS row stays picked under the filter.
    let mut state = worker_state("w2:p2");
    state.compose(186, 46).expect("frame");
    let icon = state.hits.tree_task_board;
    board_click(&mut state, icon);
    assert_eq!(state.task_board.agent.as_deref(), Some("w2:p2"));
    assert_eq!(
        state.task_board.selected,
        Some(RowKey::Agent("w2:p2".into()))
    );
    let rows = frame_rows(&state.compose(186, 46).expect("frame"));
    assert!(
        rows.iter()
            .any(|row| row.contains("Showing herdr-w1's tasks")),
        "{rows:#?}"
    );
    // A plain shell: its space, no agent.
    let mut state = tree_worker_state("shell_h");
    let mut snap = state.snapshot.as_deref().cloned().expect("snapshot");
    snap.focused_workspace_id = Some("ws_h".into());
    state.set_snapshot(Box::new(snap));
    state.compose(186, 46).expect("frame");
    let icon = state.hits.tree_task_board;
    board_click(&mut state, icon);
    assert_eq!(state.task_board.space.as_deref(), Some("herdr"));
    assert_eq!(state.task_board.agent, None);
}

#[test]
fn clicking_an_agents_sidebar_row_while_the_board_is_open_shows_that_agent_alone() {
    let mut state = tree_worker_state("shell_h");
    state.task_board.open = true;
    state.compose(186, 46).expect("frame");
    // Every space: herdr-w1's row narrows to herdr and herdr-w1, and focuses nothing.
    let outcome = click_tree_pane(&mut state, "w2:p2");
    assert!(state.task_board.open, "the board stays");
    assert!(outcome.actions.is_empty(), "no pane focuses");
    assert_eq!(state.task_board.space.as_deref(), Some("herdr"));
    assert_eq!(state.task_board.agent.as_deref(), Some("w2:p2"));
    // The lead's own row, a lone agent's tab: herdr-lead instead.
    state.compose(186, 46).expect("frame");
    let outcome = click_tree_pane(&mut state, "w2:p1");
    assert!(state.task_board.open);
    assert!(outcome.actions.is_empty());
    assert_eq!(state.task_board.agent.as_deref(), Some("w2:p1"));
    // The same agent again changes nothing: the list keeps its scroll.
    state.compose(186, 46).expect("frame");
    state.task_board.scroll = 3;
    click_tree_pane(&mut state, "w2:p1");
    assert_eq!(state.task_board.agent.as_deref(), Some("w2:p1"));
    assert_eq!(state.task_board.scroll, 3);
    // A plain shell's row is no agent: the board closes and the pane focuses.
    state.compose(186, 46).expect("frame");
    let outcome = click_tree_pane(&mut state, "shell_h");
    assert!(!outcome.actions.is_empty(), "the row still focuses");
    assert!(!state.task_board.open);
    // A space row narrows to the space and shows every agent, as before.
    state.task_board.open = true;
    state.task_board.space = Some("herdr".into());
    state.task_board.agent = Some("w2:p2".into());
    state.compose(186, 46).expect("frame");
    let oakwood = state
        .hits
        .workspaces
        .iter()
        .find(|hit| hit.workspace_id == "ws_z")
        .map(|hit| hit.rect)
        .expect("oakwood's row");
    board_click(&mut state, oakwood);
    assert!(state.task_board.open);
    assert_eq!(state.task_board.space.as_deref(), Some("oakwood"));
    assert_eq!(state.task_board.agent, None);
}

#[test]
fn clicking_an_agent_on_the_folded_rail_while_the_board_is_open_shows_that_agent_alone() {
    let mut state = tree_worker_state("shell_h");
    state.sidebar_collapsed = true;
    state.task_board.open = true;
    state.compose(186, 46).expect("frame");
    let (rect, _) = state
        .hits
        .agents
        .iter()
        .find(|(_, pane)| pane == "w2:p2")
        .cloned()
        .expect("herdr-w1 on the rail");
    let outcome = board_click(&mut state, rect);
    assert!(state.task_board.open);
    assert!(outcome.actions.is_empty());
    assert_eq!(state.task_board.space.as_deref(), Some("herdr"));
    assert_eq!(state.task_board.agent.as_deref(), Some("w2:p2"));
}

#[test]
fn an_agent_shown_alone_keeps_a_card_its_open_task_links_by_slug() {
    let mut board = tasks_and_work_board();
    let agents = lead_and_workers();
    board.view = BoardView::Work;
    board.space = Some("herdr".into());
    board.agent = Some("w2:p2".into());
    link_lead_name_to_task_7(&mut board.store, false);
    let render = render_board_with_agents(&board, 186, 46, &agents);
    let cards: Vec<String> = render.hits.work_columns.iter().flatten().cloned().collect();
    assert_eq!(
        cards,
        [
            card("2026-09-25-lead-name.md"),
            card("2026-09-25-task-board.md")
        ]
    );
    // Task 7 done: the card is no longer its.
    link_lead_name_to_task_7(&mut board.store, true);
    let render = render_board_with_agents(&board, 186, 46, &agents);
    let cards: Vec<String> = render.hits.work_columns.iter().flatten().cloned().collect();
    assert_eq!(cards, [card("2026-09-25-task-board.md")]);
}

#[test]
fn a_new_agent_filter_drops_a_selected_row_or_card_it_hides_and_keeps_one_it_shows() {
    let mut state = tree_worker_state("shell_h");
    state.task_board.open = true;
    state.task_board.view = BoardView::Work;
    let task_board = card("2026-09-25-task-board.md");
    state.task_board.work_selected = Some(task_board.clone());
    state.compose(186, 46).expect("frame");
    // herdr-w1's card, herdr-w1 chosen: it stays, with its panel.
    click_tree_pane(&mut state, "w2:p2");
    assert_eq!(state.task_board.work_selected.as_ref(), Some(&task_board));
    let rows = frame_rows(&state.compose(186, 46).expect("frame"));
    assert!(!state.hits.task_board.work_panel.is_empty(), "{rows:#?}");
    // herdr-lead has no card: none stays picked, and no panel shows.
    click_tree_pane(&mut state, "w2:p1");
    assert_eq!(state.task_board.work_selected, None);
    state.compose(186, 46).expect("frame");
    assert!(state.hits.task_board.work_panel.is_empty());
    // The Tasks view: herdr-w1's task 7 and its IN PROGRESS row go for herdr-lead.
    let task_7 = {
        let list = state.task_board.store.list().expect("a list");
        let task = list.tasks.iter().find(|task| task.n == 7).expect("task 7");
        RowKey::Task(TaskKey::of(task))
    };
    state.task_board.view = BoardView::Tasks;
    for row in [task_7.clone(), RowKey::Agent("w2:p2".into())] {
        state.task_board.agent = None;
        state.task_board.selected = Some(row.clone());
        state.compose(186, 46).expect("frame");
        click_tree_pane(&mut state, "w2:p1");
        assert_eq!(state.task_board.selected, None, "{row:?}");
        // herdr-w1's own filter keeps it.
        state.task_board.selected = Some(row.clone());
        state.compose(186, 46).expect("frame");
        click_tree_pane(&mut state, "w2:p2");
        assert_eq!(state.task_board.selected, Some(row));
    }
}

#[test]
fn clicking_a_working_worker_spins_its_slug_linked_card_and_its_panel() {
    let mut state = tree_worker_state("shell_h");
    link_lead_name_to_task_7(&mut state.task_board.store, false);
    state.task_board.open = true;
    state.task_board.view = BoardView::Work;
    state.compose(186, 46).expect("frame");
    click_tree_pane(&mut state, "w2:p2");
    assert_eq!(state.task_board.agent.as_deref(), Some("w2:p2"));
    state.task_board.work_selected = Some(card("2026-09-25-lead-name.md"));
    state.task_board.work_panel_closed = false;
    let rows = frame_rows(&state.compose(186, 46).expect("frame"));
    // A spinner frame, a braille cell, right before `mark` and its space, on a row that says
    // `text`.
    let spins = |mark: &str, text: &str| {
        rows.iter().any(|row| {
            row.contains(text)
                && row.split(mark).next().is_some_and(|before| {
                    before
                        .trim_end()
                        .chars()
                        .last()
                        .is_some_and(|glyph| ('\u{2801}'..='\u{28FF}').contains(&glyph))
                })
        })
    };
    // Task 7's minutes: only the linked card's who line says 12m, the task board card's own
    // agent says 25m.
    assert!(
        spins("herdr-w1 · 12m", "herdr-w1 · 12m"),
        "its card spins: {rows:#?}"
    );
    // The panel spins on the card's first open step.
    assert!(
        spins("design", "herdr-w1 working on it · 12m"),
        "its panel spins: {rows:#?}"
    );
}

#[test]
fn the_sidebar_icon_opens_the_board_on_your_space_with_a_focused_workers_task_and_the_key_on_all() {
    // herdr-w1, a worker, is focused: the board opens on herdr with its task picked, which
    // is its own IN PROGRESS row while it works on it.
    let mut state = worker_state("w2:p2");
    state.compose(186, 46).expect("frame");
    let icon = state.hits.tree_task_board;
    board_click(&mut state, icon);
    assert!(state.task_board.open);
    assert_eq!(state.task_board.space.as_deref(), Some("herdr"));
    assert_eq!(
        state.task_board.selected,
        Some(RowKey::Agent("w2:p2".into()))
    );
    assert_eq!(state.task_board.view, BoardView::Tasks);
    assert!(state.task_board.reveal_selected);
    // Selected, not open: `→` shows its detail, the lead it works for.
    state.compose(186, 46).expect("frame");
    press_key(&mut state, KeyCode::Right);
    let rows = frame_rows(&state.compose(186, 46).expect("frame"));
    assert!(
        rows.iter()
            .any(|row| row.contains("Lead  ") && row.contains("herdr-lead")),
        "its detail shows: {rows:#?}"
    );
    board_click(&mut state, icon);
    assert!(!state.task_board.open);
    // The board's key opens on the focused agent too.
    prefix_key(&mut state, 'a');
    assert!(state.task_board.open);
    assert_eq!(state.task_board.space.as_deref(), Some("herdr"));
    assert_eq!(state.task_board.agent.as_deref(), Some("w2:p2"));
    prefix_key(&mut state, 'a');

    // A lead focused, already shown alone: its space, and the view and scroll as they were.
    let mut state = worker_state("w2:p1");
    state.task_board.view = BoardView::Work;
    state.task_board.space = Some("herdr".into());
    state.task_board.agent = Some("w2:p1".into());
    state.task_board.scroll = 4;
    state.compose(186, 46).expect("frame");
    let icon = state.hits.tree_task_board;
    board_click(&mut state, icon);
    assert_eq!(state.task_board.space.as_deref(), Some("herdr"));
    assert_eq!(state.task_board.view, BoardView::Work);
    assert_eq!(state.task_board.scroll, 4);
    assert_eq!(state.task_board.selected, None);
}

#[test]
fn the_agent_button_shows_only_on_one_space_with_several_agents_and_lists_them_as_a_tree() {
    let mut board = tasks_and_work_board();
    let agents = lead_and_workers();
    // Every space: no agent button.
    let render = render_board_with_agents(&board, 186, 46, &agents);
    assert!(!render.rows[0].contains("agents"), "{:?}", render.rows[0]);
    assert!(render.hits.agent_button.is_empty());
    // One space with one agent or none: none either.
    board.space = Some("dotworks".into());
    let render = render_board_with_agents(&board, 186, 46, &agents);
    assert!(!render.rows[0].contains("agents"), "{:?}", render.rows[0]);
    // herdr has three: the button after the space's, dim.
    board.space = Some("herdr".into());
    let render = render_board_with_agents(&board, 186, 46, &agents);
    let header = &render.rows[0];
    let (space, agent) = (
        header.find(" herdr ▾ ").expect("space button"),
        header
            .find(" ›  All agents ▾ ")
            .unwrap_or_else(|| panic!("{header:?}")),
    );
    assert!(space < agent, "{header:?}");
    assert_eq!(
        render.cell(0, "All agents").fg,
        render.palette.overlay1,
        "dim until an agent is chosen"
    );
    assert!(!render.hits.agent_button.is_empty());
    // Its menu: All agents, then the lead and its workers as a tree, each with its mark.
    board.agent_menu = true;
    let render = render_board_with_agents(&board, 186, 46, &agents);
    let items: Vec<Option<String>> = render
        .hits
        .agent_items
        .iter()
        .map(|(_, pane)| pane.clone())
        .collect();
    assert_eq!(
        items,
        [
            None,
            Some("w2:p1".to_owned()),
            Some("w2:p2".to_owned()),
            Some("w2:p3".to_owned())
        ]
    );
    let line = |index: usize| render.rows[usize::from(render.hits.agent_items[index].0.y)].clone();
    assert!(line(0).contains("│ All agents "), "{:?}", line(0));
    assert!(
        line(1).contains(&format!("│ {LEAD_SPINNER} herdr-lead ")),
        "{:?}",
        line(1)
    );
    assert!(
        line(2).contains(&format!(
            "│   ├{WORKER_SPINNER} herdr-w1 · Round 13a on feat/task-board"
        )),
        "{:?}",
        line(2)
    );
    assert!(
        line(3).contains(&format!("│   ╰{WORKER_SPINNER} herdr-w2 · tidy the rail")),
        "{:?}",
        line(3)
    );
}

#[test]
fn a_chosen_agent_narrows_tasks_live_and_work_to_it_with_a_note_under_the_header() {
    let mut board = tasks_and_work_board();
    let agents = lead_and_workers();
    board.space = Some("herdr".into());
    board.agent = Some("w2:p2".into());
    let render = render_board_with_agents(&board, 186, 46, &agents);
    let header = &render.rows[0];
    assert!(
        header.contains(&format!(" ›  {WORKER_SPINNER} herdr-w1 ▾ ")),
        "{header:?}"
    );
    assert_eq!(render.cell(0, "herdr-w1").bg, render.palette.selection_bg);
    assert!(
        render.rows[1].starts_with("  Showing herdr-w1's tasks · Esc shows all agents"),
        "{:?}",
        render.rows[1]
    );
    assert!(
        // Only the chosen agent counts as at work, not the lead.
        render.rows[2].contains("! Action required 0")
            && render.rows[2].contains("◐ In progress 1"),
        "{:?}",
        render.rows[2]
    );
    let live = render.row_with("IN PROGRESS ");
    assert!(
        render.rows[live].ends_with("IN PROGRESS 1 ▼"),
        "{:?}",
        render.rows[live]
    );
    assert!(!render.rows.iter().any(|row| row.contains("herdr-lead")));
    render.row_with(" herdr-w1 ");
    render.row_with("Never cut the lead's name");
    assert!(!render.rows.iter().any(|row| row.contains("Say final")));
    // The Work view: only the card it is on.
    board.view = BoardView::Work;
    let render = render_board_with_agents(&board, 186, 46, &agents);
    assert!(
        render.rows[1].starts_with("  Showing herdr-w1's cards · Esc shows all agents"),
        "{:?}",
        render.rows[1]
    );
    let cards: Vec<String> = render.hits.work_columns.iter().flatten().cloned().collect();
    assert_eq!(cards, [card("2026-09-25-task-board.md")]);
}

#[test]
fn the_agent_menu_picks_an_agent_and_esc_or_a_new_space_shows_all_agents_again() {
    let mut state = worker_state("w2:p1");
    state.task_board.open = true;
    state.task_board.space = Some("herdr".into());
    board_rows(&mut state);
    let button = state.hits.task_board.agent_button;
    board_click(&mut state, button);
    assert!(state.task_board.agent_menu);
    board_rows(&mut state);
    let (worker, _) = state
        .hits
        .task_board
        .agent_items
        .iter()
        .find(|(_, pane)| pane.as_deref() == Some("w2:p2"))
        .cloned()
        .expect("the worker's item");
    board_click(&mut state, worker);
    assert_eq!(state.task_board.agent.as_deref(), Some("w2:p2"));
    assert!(!state.task_board.agent_menu);
    // Esc shows every agent again, and leaves the board open.
    press_key(&mut state, KeyCode::Esc);
    assert_eq!(state.task_board.agent, None);
    assert!(state.task_board.open);
    // Any key closes the open menu; another space shows every agent.
    state.task_board.agent = Some("w2:p2".into());
    board_rows(&mut state);
    let button = state.hits.task_board.agent_button;
    board_click(&mut state, button);
    press_key(&mut state, KeyCode::Esc);
    assert!(!state.task_board.agent_menu);
    assert_eq!(
        state.task_board.agent.as_deref(),
        Some("w2:p2"),
        "Esc only closed the menu"
    );
    board_rows(&mut state);
    let space = state.hits.task_board.space_button;
    board_click(&mut state, space);
    board_rows(&mut state);
    let (oakwood, _) = state
        .hits
        .task_board
        .space_items
        .iter()
        .find(|(_, space)| space.as_deref() == Some("oakwood"))
        .cloned()
        .expect("oakwood");
    board_click(&mut state, oakwood);
    assert_eq!(state.task_board.agent, None);
}

#[test]
fn lives_count_and_more_line_leave_out_a_resting_lead_over_its_workers() {
    let board = fixture_board();
    // herdr's lead at work with five workers fills LIVE's six rows; oakwood's resting lead
    // and its two workers come after them.
    let mut agents = vec![working("w2:p1", "herdr", "herdr-lead", None)];
    agents.extend((1..=5).map(|n| {
        let pane = format!("w2:p{}", n + 1);
        worker(&pane, "herdr", &format!("herdr-w{n}"), "w2:p1", None)
    }));
    agents.push(resting("w3:p1", "oakwood", "oak-lead"));
    agents.extend((1..=2).map(|n| {
        let pane = format!("w3:p{}", n + 1);
        worker(&pane, "oakwood", &format!("oak-w{n}"), "w3:p1", None)
    }));
    let render = render_board_with_agents(&board, 186, 60, &agents);
    let heading = render.row_with("IN PROGRESS ");
    assert!(
        render.rows[heading].ends_with("IN PROGRESS 8 ▼"),
        "{:?}",
        render.rows[heading]
    );
    render.row_with(" herdr-w5 ");
    assert!(!render.rows.iter().any(|row| row.contains(" oak-lead ")));
    render.row_with("+2 more");
}

#[test]
fn an_agent_finds_its_task_by_its_pane_first_then_its_name_in_its_space_never_done_and_its_card_by_that_tasks_slug(
) {
    use crate::client::shell::task_board::model::{
        Task, TaskAgent, TaskGroup, WorkItem, WorkStage,
    };
    use crate::client::shell::task_board::{agent_card, doing_of, worker_task};
    let tag = |pane: &str, name: &str| {
        Some(TaskAgent {
            pane: pane.into(),
            name: name.into(),
            minutes: None,
        })
    };
    // Each task is on the card its own text names.
    let task = |text: &str, space: &str, group, agent| Task {
        task: text.into(),
        space: space.into(),
        group,
        agent,
        card: Some(text.into()),
        ..Task::default()
    };
    // A card's own agent tag links nothing: real cards never set it.
    let card = |title: &str, space: &str, stage, agent| WorkItem {
        title: title.into(),
        space: space.into(),
        stage,
        agent,
        slug: Some(title.into()),
        ..WorkItem::default()
    };
    let me = worker("w2:p2", "herdr", "herdr-w1", "w2:p1", Some("on it"));
    let mut list = parse_task_list(WORK_V1).expect("work fixture");
    list.tasks = vec![
        task("done by pane", "herdr", TaskGroup::Done, tag("w2:p2", "")),
        task(
            "same name elsewhere",
            "oakwood",
            TaskGroup::Claude,
            tag("w9:p9", "herdr-w1"),
        ),
        task(
            "by name",
            "herdr",
            TaskGroup::Claude,
            tag("w0:old", "herdr-w1"),
        ),
        task("by pane", "herdr", TaskGroup::You, tag("w2:p2", "renamed")),
    ];
    list.work = vec![
        card(
            "shipped by pane",
            "herdr",
            WorkStage::Shipped,
            tag("w2:p2", ""),
        ),
        card(
            "same name elsewhere",
            "oakwood",
            WorkStage::Build,
            tag("w9:p9", "herdr-w1"),
        ),
        card(
            "by name",
            "herdr",
            WorkStage::Build,
            tag("w0:old", "herdr-w1"),
        ),
        card(
            "by pane",
            "herdr",
            WorkStage::Review,
            tag("w2:p2", "renamed"),
        ),
    ];
    let found = |list: &crate::client::shell::task_board::model::TaskList| {
        (
            worker_task(&me, list).map(|task| task.task.clone()),
            agent_card(&me, list).map(|card| card.title.clone()),
        )
    };
    assert_eq!(
        found(&list),
        (Some("by pane".into()), Some("by pane".into()))
    );
    list.tasks.pop();
    list.work.pop();
    assert_eq!(
        found(&list),
        (Some("by name".into()), Some("by name".into()))
    );
    list.tasks.pop();
    list.work.pop();
    assert_eq!(found(&list), (None, None));
    // A tag finds the agent on its pane before one of the same name, and a name only in the
    // tag's own space.
    let other = working("w7:p1", "oakwood", "herdr-w1", Some("elsewhere"));
    let named = TaskAgent {
        pane: "w2:p2".into(),
        name: "herdr-w1".into(),
        minutes: None,
    };
    let agents = [other.clone(), me.clone()];
    assert_eq!(doing_of(&named, "oakwood", &agents), Some("on it"));
    let by_name = TaskAgent {
        pane: "w0:old".into(),
        ..named
    };
    assert_eq!(doing_of(&by_name, "herdr", &agents), Some("on it"));
    assert_eq!(doing_of(&by_name, "oakwood", &agents), Some("elsewhere"));
    assert_eq!(doing_of(&by_name, "other", &agents), None);
}

#[test]
fn t_and_w_act_only_on_a_row_drawn_as_a_worker() {
    use crate::api::schema::AgentStatus;
    // herdr-w1's lead works for a lead itself: LIVE draws herdr-w1 as a top row, which offers
    // only the go-to-tab key.
    let mut state = worker_state("w2:p2");
    let mut snap = state.snapshot.as_deref().cloned().expect("snapshot");
    snap.agents[0].tokens = vec![("lead".into(), "w2:p9".into())];
    snap.agents.push(live_agent(
        "w2:p9",
        "ws_h",
        "herdr-boss",
        AgentStatus::Idle,
        &[],
    ));
    state.set_snapshot(Box::new(snap));
    state.task_board.open = true;
    state.task_board.selected = Some(RowKey::Agent("w2:p2".into()));
    let rows = frame_rows(&state.compose(186, 46).expect("frame"));
    assert!(
        !rows.iter().any(|row| row.contains("show its task")),
        "a top row: {rows:#?}"
    );
    press_key(&mut state, KeyCode::Char('t'));
    press_key(&mut state, KeyCode::Char('w'));
    assert_eq!(
        state.task_board.selected,
        Some(RowKey::Agent("w2:p2".into()))
    );
    assert_eq!(state.task_board.view, BoardView::Tasks);
    // Shown alone by the agent button, a worker is a top row too.
    let mut state = worker_state("w2:p2");
    state.task_board.open = true;
    state.task_board.space = Some("herdr".into());
    state.task_board.agent = Some("w2:p2".into());
    state.task_board.selected = Some(RowKey::Agent("w2:p2".into()));
    let rows = frame_rows(&state.compose(186, 46).expect("frame"));
    assert!(
        !rows.iter().any(|row| row.contains("show its task")),
        "a top row: {rows:#?}"
    );
    press_key(&mut state, KeyCode::Char('t'));
    press_key(&mut state, KeyCode::Char('w'));
    assert_eq!(
        state.task_board.selected,
        Some(RowKey::Agent("w2:p2".into()))
    );
    assert_eq!(state.task_board.view, BoardView::Tasks);
}

#[test]
fn t_and_w_keep_their_work_view_and_search_meanings() {
    // In Work, t still shows a card's tasks.
    let mut state = worker_state("w2:p2");
    state.task_board.open = true;
    state.task_board.view = BoardView::Work;
    state.task_board.work_selected = Some(card("2026-09-25-task-board.md"));
    state.compose(186, 46).expect("frame");
    press_key(&mut state, KeyCode::Char('t'));
    assert_eq!(state.task_board.view, BoardView::Tasks);
    assert_eq!(state.task_board.space.as_deref(), Some("herdr"));
    // With a worker selected, typing in the search box takes t and w as letters.
    state.task_board.space = None;
    state.task_board.selected = Some(RowKey::Agent("w2:p2".into()));
    state.compose(186, 46).expect("frame");
    press_key(&mut state, KeyCode::Char('/'));
    press_key(&mut state, KeyCode::Char('t'));
    press_key(&mut state, KeyCode::Char('w'));
    assert_eq!(state.task_board.filter, "tw");
    assert_eq!(state.task_board.view, BoardView::Tasks);
}

#[test]
fn board_spinners_turn_with_the_sidebar_hidden_while_the_board_shows_a_working_agent() {
    let start = std::time::Instant::now();
    let later = |ms| start + std::time::Duration::from_millis(ms);
    let mut state = worker_state("w2:p2");
    state.config.sidebar_collapsed_mode = crate::config::SidebarCollapsedModeConfig::Hidden;
    state.sidebar_collapsed = true;
    state.compose(186, 46).expect("frame");
    assert!(
        !state.tick_sidebar_spinner(start),
        "no sidebar and no board: nothing spins"
    );
    state.task_board.open = true;
    let first = frame_rows(&state.compose(186, 46).expect("frame"));
    assert!(state.tick_sidebar_spinner(start), "the first tick draws");
    assert!(!state.tick_sidebar_spinner(later(50)), "waits for a turn");
    assert!(state.tick_sidebar_spinner(later(100)));
    let turned = frame_rows(&state.compose(186, 46).expect("frame"));
    let live = |rows: &[String]| {
        rows.iter()
            .find(|row| row.contains("herdr-lead"))
            .cloned()
            .unwrap_or_default()
    };
    assert_ne!(live(&first), live(&turned), "LIVE's spinner turned");
    // A space with nobody at work shows no spinner, so the clock rests.
    state.task_board.space = Some("oakwood".into());
    state.compose(186, 46).expect("frame");
    assert!(!state.tick_sidebar_spinner(later(200)), "oakwood rests");
    assert_eq!(state.sidebar_spinner_next_at, None);
}

/// The fixture's own number (`n`, its place in the file) of the task a row draws, found by its
/// text: the drawn numbers count the rows as shown, so they cannot say which task it is.
fn file_number(row: &str) -> Option<String> {
    let list = parse_task_list(LIST_V1).expect("fixture");
    list.tasks
        .iter()
        .find(|task| row.contains(task.task.as_str()))
        .map(|task| task.n.to_string())
}

/// The file numbers of NEEDS YOU's rows as drawn, top down, a waiting tab's row as its tab
/// name.
fn needs_you_rows(render: &BoardRender) -> Vec<String> {
    let heading = render.row_with("ACTION REQUIRED");
    render.rows[heading + 1..]
        .iter()
        .take_while(|row| !row.contains("QUEUED"))
        .filter_map(|row| {
            let cells = split_cells(row);
            if let Some(number) = file_number(row) {
                return Some(number);
            }
            cells
                .iter()
                .any(|cell| cell.starts_with("oak-lead"))
                .then(|| "oak-lead".to_owned())
        })
        .collect()
}

#[test]
fn task_rows_sort_newest_first_by_default_and_by_each_choice_with_waiting_tabs_on_top() {
    use crate::client::shell::task_board::TaskSort;
    let agents = [waiting("w9:p1", "oakwood", "oak-lead", Some("pick A or B"))];
    let mut board = fixture_board();
    assert_eq!(board.sort, TaskSort::Newest);
    for (sort, want) in [
        (TaskSort::Newest, ["4", "5", "1", "2", "3"]),
        (TaskSort::Oldest, ["3", "2", "1", "4", "5"]),
        (TaskSort::Project, ["1", "5", "2", "3", "4"]),
        // By the last column as drawn: `-` for the tasks that only need you, first.
        (TaskSort::State, ["4", "2", "3", "1", "5"]),
    ] {
        board.sort = sort;
        let render = render_board_with_agents(&board, 186, 60, &agents);
        let rows = needs_you_rows(&render);
        assert_eq!(rows[0], "oak-lead", "{sort:?}: {rows:?}");
        assert_eq!(rows[1..], want, "{sort:?}");
    }
}

#[test]
fn done_stays_newest_done_first_under_every_sort_but_oldest_first() {
    use crate::client::shell::task_board::TaskSort;
    let mut list = parse_task_list(LIST_V1).expect("fixture");
    // Task 12 was done the day before task 13, and is older.
    list.tasks[11].done_on = Some("2026-09-24".into());
    list.tasks[11].age_days = 1;
    list.tasks[12].project = "zz".into();
    let mut board = fixture_board();
    board.store.receive(Ok(list), std::time::Instant::now());
    board.done_expanded = true;
    let done_rows = |board: &TaskBoard| {
        let render = render_board(board, 186, 60);
        let heading = render.row_with("COMPLETED");
        render.rows[heading + 1..]
            .iter()
            .filter_map(|row| file_number(row))
            .collect::<Vec<_>>()
    };
    for sort in [TaskSort::Newest, TaskSort::Project, TaskSort::State] {
        board.sort = sort;
        assert_eq!(done_rows(&board), ["13", "12"], "{sort:?}");
    }
    board.sort = TaskSort::Oldest;
    assert_eq!(done_rows(&board), ["12", "13"]);
}

#[test]
fn the_sort_button_follows_search_in_the_space_buttons_style_and_names_the_choice() {
    use crate::client::shell::task_board::TaskSort;
    let mut board = fixture_board();
    let render = render_board(&board, 186, 46);
    let header = &render.rows[0];
    assert!(header.contains(" Sort: Newest ▾ "), "{header:?}");
    let (search, sort) = (render.hits.filter, render.hits.sort_button);
    assert_eq!(
        sort.x,
        search.right() + 1,
        "one column after the search box"
    );
    assert_eq!(sort.width, 16);
    assert!(render.hits.work_tab.right() < sort.x);
    let button = render.cell(0, "Sort:");
    let space = render.cell(0, "Space: All");
    assert_eq!((button.fg, button.bg), (space.fg, space.bg));
    for (sort, word) in [
        (TaskSort::Oldest, " Sort: Oldest ▾ "),
        (TaskSort::Project, " Sort: Project ▾ "),
        (TaskSort::State, " Sort: State ▾ "),
    ] {
        board.sort = sort;
        let render = render_board(&board, 186, 46);
        assert!(render.rows[0].contains(word), "{:?}", render.rows[0]);
    }
    // Work has no task rows to sort.
    board.view = BoardView::Work;
    let render = render_board(&board, 186, 46);
    assert!(!render.rows[0].contains("Sort:"), "{:?}", render.rows[0]);
    assert!(render.hits.sort_button.is_empty());
}

#[test]
fn the_sort_menu_lists_the_seven_orders_under_its_button_with_the_chosen_one_checked() {
    let mut board = fixture_board();
    board.sort_menu = true;
    let render = render_board(&board, 186, 46);
    let x = render.hits.sort_button.x;
    let item = |y: usize| {
        let row = &render.rows[y];
        let byte = row
            .char_indices()
            .nth(usize::from(x))
            .map_or(row.len(), |(byte, _)| byte);
        row[byte..].to_owned()
    };
    assert!(item(1).starts_with("┌────────────────┐"), "{:?}", item(1));
    assert!(item(2).starts_with("│ By priority    │"), "{:?}", item(2));
    assert!(item(3).starts_with("│ By due date    │"), "{:?}", item(3));
    assert!(item(4).starts_with("│ Newest first ✓ │"), "{:?}", item(4));
    assert!(item(5).starts_with("│ Oldest first   │"), "{:?}", item(5));
    assert!(item(6).starts_with("│ By project     │"), "{:?}", item(6));
    assert!(item(7).starts_with("│ By state       │"), "{:?}", item(7));
    assert!(item(8).starts_with("│ File order     │"), "{:?}", item(8));
    assert!(item(9).starts_with("└────────────────┘"), "{:?}", item(9));
    assert_eq!(render.cell(4, "Newest first").fg, render.palette.text);
    assert_eq!(
        render.cell(4, "Newest first").bg,
        render.palette.selection_bg
    );
    assert_eq!(render.cell(5, "Oldest first").fg, render.palette.text);
    assert_eq!(render.hits.sort_items.len(), 7);
}

#[test]
fn a_narrowing_header_drops_the_sort_button_before_the_search_box_shrinks() {
    let board = fixture_board();
    let mut dropped_first = false;
    for width in (60..=186).rev() {
        let render = render_board(&board, width, 46);
        let header = &render.rows[0];
        let sort = header.contains("Sort:");
        let full_search = header.contains("⌕ Search");
        assert!(!sort || full_search, "{width}: {header:?}");
        dropped_first |= !sort && full_search;
    }
    assert!(
        dropped_first,
        "some width keeps the whole search box without the sort button"
    );
}

#[test]
fn s_and_a_click_open_the_sort_menu_and_its_choice_lasts_for_the_clients_life() {
    use crate::client::shell::task_board::TaskSort;
    let mut state = worker_state("w2:p2");
    state.task_board.open = true;
    state.compose(186, 46).expect("frame");
    // The key: s opens it, ↓ and ↑ move the check and sort at once, Enter closes it.
    press_key(&mut state, KeyCode::Char('s'));
    assert!(state.task_board.sort_menu);
    press_key(&mut state, KeyCode::Down);
    press_key(&mut state, KeyCode::Down);
    press_key(&mut state, KeyCode::Up);
    assert_eq!(state.task_board.sort, TaskSort::Oldest);
    assert!(state.task_board.sort_menu, "still open");
    press_key(&mut state, KeyCode::Enter);
    assert!(!state.task_board.sort_menu);
    assert_eq!(state.task_board.sort, TaskSort::Oldest);
    // The mouse: a click on the button opens it, one on an item picks it.
    state.compose(186, 46).expect("frame");
    let button = state.hits.task_board.sort_button;
    board_click(&mut state, button);
    assert!(state.task_board.sort_menu);
    state.compose(186, 46).expect("frame");
    let by_state = state.hits.task_board.sort_items[5].0;
    board_click(&mut state, by_state);
    assert!(!state.task_board.sort_menu);
    assert_eq!(state.task_board.sort, TaskSort::State);
    // Closing the board, or another server, keeps it.
    press_key(&mut state, KeyCode::Esc);
    prefix_key(&mut state, 'a');
    assert!(state.task_board.open);
    state.task_board.forget_endpoint();
    assert_eq!(state.task_board.sort, TaskSort::State);
    // `?` names the key.
    state.task_board.keys_shown = true;
    let rows = frame_rows(&state.compose(186, 46).expect("frame"));
    assert!(rows.iter().any(|row| row.contains(" s  sort")), "{rows:#?}");
}

#[test]
fn the_sort_menu_opens_only_in_tasks_and_closes_when_the_view_leaves_it() {
    use crate::client::shell::task_board::TaskSort;
    // In Work, s opens nothing, so ↑ ↓ j k and a click keep their Work meaning.
    let mut state = worker_state("w2:p2");
    state.task_board.open = true;
    state.task_board.view = BoardView::Work;
    state.compose(186, 46).expect("frame");
    press_key(&mut state, KeyCode::Char('s'));
    assert!(!state.task_board.sort_menu, "no menu in Work");
    for code in [
        KeyCode::Down,
        KeyCode::Char('j'),
        KeyCode::Up,
        KeyCode::Char('k'),
    ] {
        press_key(&mut state, code);
    }
    assert_eq!(
        state.task_board.sort,
        TaskSort::Newest,
        "the Tasks sort stays"
    );
    // Open in Tasks, Tab to Work closes it; so does a click on the Work tab.
    state.task_board.view = BoardView::Tasks;
    state.compose(186, 46).expect("frame");
    press_key(&mut state, KeyCode::Char('s'));
    assert!(state.task_board.sort_menu);
    press_key(&mut state, KeyCode::Tab);
    assert_eq!(state.task_board.view, BoardView::Work);
    assert!(!state.task_board.sort_menu, "Tab closes it");
    press_key(&mut state, KeyCode::Tab);
    state.compose(186, 46).expect("frame");
    press_key(&mut state, KeyCode::Char('s'));
    let work_tab = state.hits.task_board.work_tab;
    board_click(&mut state, work_tab);
    assert!(!state.task_board.sort_menu, "a click closes it");
    press_key(&mut state, KeyCode::Down);
    assert_eq!(state.task_board.sort, TaskSort::Newest);
}

/// The text of `row` from `start` up to its `…`, which must end a whole word of `whole`.
fn cut_at_a_word(row: &str, start: &str, whole: &str) {
    let from = row
        .find(start)
        .unwrap_or_else(|| panic!("{start:?} in {row:?}"));
    let rest = &row[from..];
    let kept = rest.split('…').next().unwrap_or(rest);
    assert!(rest.contains('…'), "cut: {row:?}");
    assert!(
        whole.starts_with(kept) && whole[kept.len()..].starts_with(' '),
        "{kept:?} ends a word of {whole:?}"
    );
}

#[test]
fn the_agent_menu_and_a_cards_who_line_cut_at_a_word() {
    let doing = "Writing the report on the sidebar filter and its spinner clock";
    let mut board = tasks_and_work_board();
    board.space = Some("herdr".into());
    board.agent_menu = true;
    let mut agents = lead_and_workers();
    agents[1].state = AgentState::Working(Some(doing.into()));
    let render = render_board_with_agents(&board, 186, 46, &agents);
    let row = render.row_with("herdr-w1 · ");
    cut_at_a_word(&render.rows[row], "Writing", doing);

    let name = "herdr-w1 on the sidebar filter and its spinner clock";
    let board = work_board_with(|list| {
        for card in &mut list.work {
            if let Some(agent) = card.agent.as_mut() {
                agent.name = name.into();
                agent.minutes = None;
            }
        }
    });
    let render = render_board_with_agents(&board, 186, 46, &[]);
    let row = render.row_with("herdr-w1 on");
    cut_at_a_word(&render.rows[row], "herdr-w1", name);
}

#[test]
fn a_task_or_card_spins_only_while_its_tagged_agent_works() {
    // A task's own row: its Task cell starts with its text or the spinner, as LIVE names the
    // task too, after its number.
    let spins = |render: &BoardRender, text: &str| {
        let plain = format!("│ {text}");
        let spinning = format!("│ {WORKER_SPINNER} {text}");
        let row = render
            .rows
            .iter()
            .find(|row| row.contains(&plain) || row.contains(&spinning))
            .unwrap_or_else(|| panic!("{text:?} has no row: {:#?}", render.rows));
        row.contains(&spinning)
    };
    // Task 7 names herdr-w1 on w2:p2, task 6 oakwood's lead on w3:p1; each has a row of its
    // own as the reader does not put it in progress.
    let board = fixture_board_in_section(7, "queued");
    let board = {
        let mut board = board;
        let json = list_v1_in_section(7, "queued");
        let json = json.replacen("\"n\": 6,", "\"n\": 6, \"section\": \"queued\",", 1);
        board.store.receive_json(&json, std::time::Instant::now());
        board
    };
    let render = render_board_with_agents(&board, 186, 60, &[]);
    assert!(
        !spins(&render, "Never cut the lead's name"),
        "herdr-w1 is gone"
    );
    assert!(!spins(&render, "Compare Carmen"), "the lead is gone");
    let agents = [
        working("w2:p2", "herdr", "herdr-w1", None),
        resting("w3:p1", "oakwood", "oakwood-ws-lead"),
    ];
    let render = render_board_with_agents(&board, 186, 60, &agents);
    assert!(
        spins(&render, "Never cut the lead's name"),
        "herdr-w1 works"
    );
    assert!(!spins(&render, "Compare Carmen"), "the lead rests");
    // The same name in another space is not the tagged agent.
    let elsewhere = [working("w9:p9", "oakwood", "herdr-w1", None)];
    let render = render_board_with_agents(&board, 186, 60, &elsewhere);
    assert!(!spins(&render, "Never cut the lead's name"));
    // A card: its who line keeps the name, with the spinner only while its agent works.
    let mut board = work_board();
    board.work_selected = None;
    board.work_panel_closed = true;
    let render = render_board_with_agents(&board, 186, 46, &[]);
    let who = render.row_with("herdr-w1 · ");
    assert!(
        !render.rows[who].contains(WORKER_SPINNER),
        "{:?}",
        render.rows[who]
    );
    let agents = [working("w36:pV", "herdr", "herdr-w1", None)];
    let render = render_board_with_agents(&board, 186, 46, &agents);
    let who = render.row_with("herdr-w1 · ");
    assert!(
        render.rows[who].contains(WORKER_SPINNER),
        "{:?}",
        render.rows[who]
    );
}

#[test]
fn leaving_work_drops_its_selected_step_so_esc_in_tasks_closes_the_board() {
    let mut state = work_state();
    state.task_board.view = BoardView::Work;
    state.task_board.work_selected = Some(card("2026-09-25-task-board.md"));
    state.task_board.work_step = Some(0);
    state.compose(160, 40).expect("frame");
    press_key(&mut state, KeyCode::Tab);
    assert_eq!(state.task_board.view, BoardView::Tasks);
    assert_eq!(state.task_board.work_step, None);
    state.compose(160, 40).expect("frame");
    press_key(&mut state, KeyCode::Esc);
    assert!(!state.task_board.open, "the first Esc closes the board");
}

#[test]
fn the_spinner_clock_schedules_a_frame_only_while_a_working_agent_is_on_screen() {
    let start = std::time::Instant::now();
    let idle_wake = std::time::Duration::from_millis(100);
    let mut state = worker_state("w2:p2");
    state.config.sidebar_collapsed_mode = crate::config::SidebarCollapsedModeConfig::Hidden;
    state.sidebar_collapsed = true;
    state.compose(186, 46).expect("frame");
    // (b) The board closed and the sidebar hidden: no frame, the loop's idle wake only.
    assert!(!state.tick_sidebar_spinner(start), "(b) nothing drawn");
    assert_eq!(state.sidebar_spinner_next_at, None, "(b)");
    assert_eq!(state.timer_delay(start), idle_wake, "(b)");
    // (a) The board open on a space with nobody at work, in either view.
    state.task_board.open = true;
    state.task_board.space = Some("oakwood".into());
    for view in [BoardView::Tasks, BoardView::Work] {
        state.task_board.view = view;
        state.compose(186, 46).expect("frame");
        assert!(!state.tick_sidebar_spinner(start), "(a) {view:?}");
        assert_eq!(state.sidebar_spinner_next_at, None, "(a) {view:?}");
        assert_eq!(state.timer_delay(start), idle_wake, "(a) {view:?}");
    }
    // (c) A working agent's IN PROGRESS row on screen: a frame at its turn. A card's spinner
    // is in `a_task_or_card_spins_only_while_its_tagged_agent_works`.
    state.task_board.space = Some("herdr".into());
    state.task_board.view = BoardView::Tasks;
    let rows = frame_rows(&state.compose(186, 46).expect("frame"));
    assert!(
        rows.iter()
            .any(|row| row.contains(WORKER_SPINNER) || row.contains(LEAD_SPINNER)),
        "(c) a spinner drawn: {rows:#?}"
    );
    assert!(state.tick_sidebar_spinner(start), "(c)");
    assert!(
        state
            .sidebar_spinner_next_at
            .is_some_and(|at| at > start && at <= start + idle_wake),
        "(c) the next turn is scheduled: {:?}",
        state.sidebar_spinner_next_at
    );
    // (d) The same space in the Phases view, the working agent's card filtered out: its
    // agents work, but no card on screen is theirs, so no spinner is drawn and no frame is
    // due.
    state.task_board.view = BoardView::Work;
    let rows = frame_rows(&state.compose(186, 46).expect("frame"));
    assert!(
        rows.iter().any(|row| row.contains("herdr-w1 · 25m")),
        "(d) herdr-w1's card spins before the filter: {rows:#?}"
    );
    state.task_board.filter = "grey".into();
    let rows = frame_rows(&state.compose(186, 46).expect("frame"));
    assert!(
        rows.iter().any(|row| row.contains("grey"))
            && !rows.iter().any(|row| row.contains("herdr-w1")),
        "(d) only the grey card, no agent: {rows:#?}"
    );
    assert!(!state.tick_sidebar_spinner(start + idle_wake), "(d)");
    assert_eq!(state.sidebar_spinner_next_at, None, "(d)");
}

#[test]
fn the_board_spins_only_for_a_working_agent_it_draws() {
    // herdr: herdr-lead and herdr-w1 at work; oakwood: oak-lead idle.
    let mut state = worker_state("w2:p2");
    let spins = |state: &mut ClientShellState| {
        state.compose(186, 46).expect("frame");
        state.hits.task_board.spins
    };
    assert!(!spins(&mut state), "closed");
    state.task_board.open = true;
    assert!(spins(&mut state), "every space");
    state.task_board.space = Some("oakwood".into());
    assert!(!spins(&mut state), "oakwood rests");
    state.task_board.space = Some("herdr".into());
    assert!(spins(&mut state));
    // herdr-w1 shown alone, then resting: only its own state counts.
    state.task_board.agent = Some("w2:p2".into());
    assert!(spins(&mut state));
    let mut snap = state.snapshot.as_deref().cloned().expect("snapshot");
    snap.agents[1].agent_status = crate::api::schema::AgentStatus::Idle;
    state.set_snapshot(Box::new(snap));
    assert!(!spins(&mut state), "the lead works, but it is not shown");
    // A choice the header does not offer (every space) shows every agent again.
    state.task_board.space = None;
    assert!(spins(&mut state), "the lead works");
}

#[test]
fn j_k_and_n_switch_the_sort_to_file_order_so_the_move_shows_at_once() {
    use crate::client::shell::task_board::TaskSort;
    let shifted = |state: &mut ClientShellState, ch| {
        state.handle_raw_events(vec![RawInputEvent::Key(TerminalKey::new(
            KeyCode::Char(ch),
            KeyModifiers::SHIFT,
        ))])
    };
    let mut state = acting_state(2);
    assert_eq!(
        state.task_board.sort,
        TaskSort::Newest,
        "newest first at start"
    );
    shifted(&mut state, 'J');
    assert_eq!(state.task_board.sort, TaskSort::File);
    let mut state = acting_state(2);
    press_key(&mut state, KeyCode::Char('n'));
    assert_eq!(state.task_board.sort, TaskSort::File);
    // File order is the list's own, and the button says so.
    let mut board = fixture_board();
    board.sort = TaskSort::File;
    let agents = [waiting("w9:p1", "oakwood", "oak-lead", Some("pick A or B"))];
    let render = render_board_with_agents(&board, 186, 60, &agents);
    assert!(
        render.rows[0].contains(" Sort: File ▾ "),
        "{:?}",
        render.rows[0]
    );
    assert_eq!(
        needs_you_rows(&render),
        ["oak-lead", "1", "2", "3", "4", "5"]
    );
}

#[test]
fn a_waiting_worker_in_needs_you_offers_neither_t_nor_w() {
    use crate::api::schema::AgentStatus;
    // herdr-w1 works for herdr-lead but now asks Nadine: its row is a waiting tab.
    let mut state = worker_state("w2:p2");
    let mut snap = state.snapshot.as_deref().cloned().expect("snapshot");
    snap.agents[1].agent_status = AgentStatus::Idle;
    snap.agents[1]
        .tokens
        .push(("ask".into(), "pick A or B".into()));
    state.set_snapshot(Box::new(snap));
    state.task_board.open = true;
    state.task_board.selected = Some(RowKey::Agent("w2:p2".into()));
    let rows = frame_rows(&state.compose(186, 46).expect("frame"));
    assert!(
        rows.iter().any(|row| row.contains("pick A or B")),
        "a waiting tab: {rows:#?}"
    );
    press_key(&mut state, KeyCode::Char('t'));
    press_key(&mut state, KeyCode::Char('w'));
    assert_eq!(
        state.task_board.selected,
        Some(RowKey::Agent("w2:p2".into()))
    );
    assert_eq!(state.task_board.view, BoardView::Tasks);
}

#[test]
fn a_selected_workers_t_shows_its_task_and_w_its_card() {
    let mut state = worker_state("w2:p1");
    state.task_board.open = true;
    state.task_board.selected = Some(RowKey::Agent("w2:p2".into()));
    state.compose(186, 46).expect("frame");
    // Its task in progress is its own row's: `t` has no row to show.
    press_key(&mut state, KeyCode::Char('t'));
    assert_eq!(
        state.task_board.selected,
        Some(RowKey::Agent("w2:p2".into()))
    );
    // A task it is on that is not in progress has its own row; it is on the task board card.
    let mut list = parse_task_list(&list_v1_in_section(7, "queued")).expect("fixture");
    list.work = parse_task_list(WORK_V1).expect("work fixture").work;
    let board_card = card("2026-09-25-task-board.md");
    if let Some(found) = list.work.iter_mut().find(|found| found.file == board_card) {
        found.slug = Some("task-board".into());
    }
    list.tasks[6].card = Some("task-board".into());
    state
        .task_board
        .store
        .receive(Ok(list), std::time::Instant::now());
    state.compose(186, 46).expect("frame");
    press_key(&mut state, KeyCode::Char('t'));
    assert_eq!(
        state.task_board.selected,
        Some(key(7)),
        "its task, selected"
    );
    assert!(state.task_board.reveal_selected, "and scrolled into view");
    state.task_board.selected = Some(RowKey::Agent("w2:p2".into()));
    state.compose(186, 46).expect("frame");
    press_key(&mut state, KeyCode::Char('w'));
    assert_eq!(state.task_board.view, BoardView::Work);
    assert_eq!(
        state.task_board.work_selected,
        Some(card("2026-09-25-task-board.md"))
    );
    // A lead's row has neither.
    state.task_board.view = BoardView::Tasks;
    state.task_board.selected = Some(RowKey::Agent("w2:p1".into()));
    state.compose(186, 46).expect("frame");
    press_key(&mut state, KeyCode::Char('t'));
    assert_eq!(
        state.task_board.selected,
        Some(RowKey::Agent("w2:p1".into()))
    );
}

#[test]
fn live_lists_each_working_tab_above_needs_you_with_what_it_is_doing() {
    let board = fixture_board();
    let agents = [
        // Task 7 names this pane's agent, 12 minutes on it.
        working("w2:p2", "herdr", "herdr-w1", Some("round 13a: LIVE")),
        working("w8:p1", "oakwood", "oak-w2", None),
    ];
    let render = render_board_with_agents(&board, WIDE, 60, &agents);
    let heading = render.row_with("IN PROGRESS 2");
    assert_eq!(heading, 3, "first under the chip row: {:#?}", render.rows);
    assert!(
        render.rows[heading].starts_with("  IN PROGRESS 2 ▼"),
        "{:?}",
        render.rows[heading]
    );
    assert_eq!(
        render.cell(heading, "IN PROGRESS").fg,
        working_colour(&render.palette)
    );
    assert!(render.rows[heading + 1].starts_with(" ╭─"));
    assert_eq!(
        split_cells(&render.rows[heading + 2]),
        ["Agent", "Working on", "Stage", "Step now", "Doing", "For"]
    );
    let first = heading + 4;
    let cells = split_cells(&render.rows[first]);
    assert_eq!(
        [cells[0].as_str(), &cells[1], &cells[4], &cells[5]],
        [LEAD_SPINNER, "herdr-w1", "round 13a: LIVE", "12m"]
    );
    assert_eq!(render.cell(first, LEAD_SPINNER).fg, render.palette.yellow);
    assert_eq!(render.cell(first, "herdr-w1").fg, render.palette.overlay1);
    assert_eq!(render.cell(first, "round").fg, render.palette.text);
    assert_eq!(render.cell(first, "12m").fg, render.palette.overlay0);
    assert_eq!(
        split_cells(&render.rows[first + 1]),
        [LEAD_SPINNER, "oak-w2", "working", "working"]
    );
    assert!(render.rows[first + 2].starts_with(" ╰"));
    assert!(
        blank(&render.rows[first + 3]) && render.rows[first + 4].contains("ACTION REQUIRED 5"),
        "{:#?}",
        render.rows
    );
    assert_eq!(
        render.hits.spans[0].0,
        RowKey::Agent("w2:p2".into()),
        "the keys reach LIVE first"
    );

    // Open, a LIVE row shows what it does in full in its Doing column, then its key alone.
    let mut board = fixture_board();
    board.selected = Some(RowKey::Agent("w2:p2".into()));
    board.open_row = board.selected.clone();
    let render = render_board_with_agents(&board, WIDE, 60, &agents);
    let row = render.row_with("herdr-w1");
    // The screen column where `needle` starts on row `y`.
    let column = |y: usize, needle: &str| {
        let text = &render.rows[y];
        text.find(needle)
            .map(|at| UnicodeWidthStr::width(&text[..at]))
            .unwrap_or_else(|| panic!("{needle:?} not on {text:?}"))
    };
    assert_eq!(column(row, "round 13a: LIVE"), column(row, "round 13a"));
    assert!(render.rows[row].contains(" 12m "), "{:?}", render.rows[row]);
    // Its row shows all it has: its block is its keys alone.
    assert!(
        render.rows[row + 1].starts_with(" │      Enter  go to agent     ←  close "),
        "{:?}",
        render.rows[row + 1]
    );
    let footer = &render.rows[59];
    for dropped in ["Enter", "d  done", "x  delete", "m  move"] {
        assert!(
            !footer.contains(dropped),
            "a tab takes no {dropped:?}: {footer:?}"
        );
    }

    // With no tab working there is no LIVE.
    let render = render_board_with_agents(&fixture_board(), WIDE, 60, &[]);
    assert!(!render.rows.iter().any(|row| row.contains("IN PROGRESS")));

    // Folded, the heading alone; with the keyboard's focus on it, drawn as a selected row.
    let mut board = fixture_board();
    board.live_folded = true;
    board.focused_heading = Some(Fold::Live);
    let render = render_board_with_agents(&board, WIDE, 60, &agents);
    assert!(
        render.rows[3].starts_with("  IN PROGRESS 2 ▶"),
        "{:?}",
        render.rows[3]
    );
    assert!(blank(&render.rows[4]) && render.rows[5].contains("ACTION REQUIRED"));
    assert_eq!(
        render.cell(3, "▶").bg,
        render.palette.selection_bg,
        "the label keeps its fill; the band is under the rest"
    );
    assert!(render.hits.live_folded);
    assert_eq!(render.hits.live_heading.y, 3);
}

#[test]
fn live_shows_agent_working_on_stage_step_now_doing_and_for_under_one_header_row_in_aligned_columns(
) {
    let agents = [
        // Task 7 names this pane's agent, 12 minutes on it.
        working("w2:p2", "herdr", "herdr-w1", Some("round 13a: LIVE")),
        working("w8:p1", "oakwood", "oak-w2", None),
    ];
    let render = render_board_with_agents(&fixture_board(), WIDE, 60, &agents);
    let heading = render.row_with("IN PROGRESS 2");
    let head = heading + 2;
    assert_eq!(
        split_cells(&render.rows[head]),
        ["Agent", "Working on", "Stage", "Step now", "Doing", "For"]
    );
    // Its task, which has no row of its own while the tab works on it.
    assert_eq!(
        render
            .rows
            .iter()
            .filter(|row| row.contains("Never cut the lead's name"))
            .count(),
        1
    );
    let first = heading + 4;
    assert_eq!(
        split_cells(&render.rows[first]),
        [
            LEAD_SPINNER.to_owned(),
            "herdr-w1".to_owned(),
            "Never cut the lead's name".to_owned(),
            "working".to_owned(),
            "round 13a: LIVE".to_owned(),
            "12m".to_owned(),
        ]
    );
    assert_eq!(render.cell(first, "Never cut").fg, render.palette.text);
    assert_eq!(render.cell(first, "12m").fg, render.palette.overlay0);
    // A working tab with nothing to say: a dim `working`; its For blank.
    assert_eq!(
        split_cells(&render.rows[first + 1]),
        [LEAD_SPINNER, "oak-w2", "working", "working"]
    );
    assert_eq!(
        render.cell(first + 1, "working").fg,
        render.palette.overlay0
    );
    // One set of columns: each name over its text.
    let column = |y: usize, needle: &str| {
        let text = &render.rows[y];
        text.find(needle)
            .map(|at| UnicodeWidthStr::width(&text[..at]))
            .unwrap_or_else(|| panic!("{needle:?} not on {text:?}"))
    };
    assert_eq!(column(head, "Agent"), column(first, "herdr-w1"));
    assert_eq!(column(head, "Agent"), column(first + 1, "oak-w2"));
    assert_eq!(column(head, "Working on"), column(first, "Never cut"));
    assert_eq!(column(head, "Step now"), column(first, "working"));
    assert_eq!(column(head, "Step now"), column(first + 1, "working"));
    assert_eq!(column(head, "Doing"), column(first, "round 13a"));
    assert_eq!(column(head, "For"), column(first, "12m"));
    assert_eq!(
        render
            .rows
            .iter()
            .filter(|row| row.contains("│ Agent"))
            .count(),
        1
    );

    // Open, the block does not repeat the task the row shows.
    let mut board = fixture_board();
    board.selected = Some(RowKey::Agent("w2:p2".into()));
    board.open_row = board.selected.clone();
    let render = render_board_with_agents(&board, WIDE, 60, &agents);
    let row = render.row_with("herdr-w1");
    assert!(
        !render.rows[row + 1..row + 4]
            .iter()
            .any(|line| line.contains("Task ")),
        "{:#?}",
        &render.rows[row..row + 4]
    );

    // Search keeps a LIVE row by the task it shows.
    let mut board = fixture_board();
    board.filter = "lead's".into();
    let render = render_board_with_agents(&board, WIDE, 60, &agents);
    let heading = render.row_with("IN PROGRESS 1");
    assert!(
        render.rows[heading + 4].contains("herdr-w1"),
        "{:#?}",
        render.rows
    );

    // 56 to 79: Stage and Step now give way; Working on, Doing and For keep their room.
    let render = render_board_with_agents(&fixture_board(), 60, 40, &agents);
    let heading = render.row_with("IN PROGRESS 2");
    assert_eq!(
        split_cells(&render.rows[heading + 2]),
        ["Agent", "Working on", "Doing", "For"]
    );
    let cells = split_cells(&render.rows[heading + 4]);
    // The two share the room, each cut at a word.
    assert_eq!([cells[1].as_str(), &cells[4]], ["herdr-w1", "12m"]);
    assert!(cells[2].starts_with("Never cut"), "{cells:?}");
    assert!(cells[3].starts_with("round 13a"), "{cells:?}");
}

#[test]
fn live_drops_stage_and_step_now_then_stacks_as_the_task_grid_does() {
    let agents = [working(
        "w2:p2",
        "herdr",
        "herdr-w1",
        Some("round 13a: LIVE"),
    )];
    // 56 to 79: no Stage or Step now; the tab, what it is on, what it does and how long
    // keep their room.
    let render = render_board_with_agents(&fixture_board(), 60, 40, &agents);
    let heading = render.row_with("IN PROGRESS 1");
    assert_eq!(
        split_cells(&render.rows[heading + 2]),
        ["Agent", "Working on", "Doing", "For"]
    );
    let cells = split_cells(&render.rows[heading + 4]);
    assert_eq!(
        [cells[0].as_str(), &cells[1], &cells[4]],
        [LEAD_SPINNER, "herdr-w1", "12m"]
    );
    assert!(cells[3].starts_with("round 13a"), "{cells:?}");

    // Below 56, as the tasks stack: the mark and the tab, then what it does under them.
    let render = render_board_with_agents(&fixture_board(), 50, 40, &agents);
    let heading = render.row_with("IN PROGRESS 1");
    assert!(render.rows[heading + 1].starts_with(" ╭─"));
    assert!(
        render.rows[heading + 2].starts_with(&format!(" │ {LEAD_SPINNER}  herdr-w1 ")),
        "{:#?}",
        render.rows
    );
    assert!(render.rows[heading + 3].starts_with(" │    round 13a: LIVE · 12m "));
    assert!(render.rows[heading + 4].starts_with(" ╰─"));
    assert_eq!(
        render.cell(heading + 2, "herdr-w1").fg,
        render.palette.overlay1
    );
    assert_eq!(
        render.cell(heading + 3, "· 12m").fg,
        render.palette.overlay0
    );
}

/// The fixture list with `edit` made to it.
fn fixture_board_with(
    edit: impl FnOnce(&mut crate::client::shell::task_board::model::TaskList),
) -> TaskBoard {
    let mut list = parse_task_list(LIST_V1).expect("fixture");
    edit(&mut list);
    let mut board = fixture_board();
    board.store.receive(Ok(list), std::time::Instant::now());
    board
}

/// `agent` with `labels` from its pane and its tab.
fn labelled(agent: BoardAgent, labels: AgentLabels) -> BoardAgent {
    BoardAgent { labels, ..agent }
}

/// Card `slug` titled `title` at `stage`, its steps `(text, done)`.
fn card_of(
    slug: &str,
    title: &str,
    stage: WorkStage,
    steps: &[(&str, bool)],
) -> crate::client::shell::task_board::model::WorkItem {
    crate::client::shell::task_board::model::WorkItem {
        space: "herdr".into(),
        project: "herdr".into(),
        title: title.into(),
        stage,
        slug: Some(slug.into()),
        steps: steps
            .iter()
            .map(
                |(text, done)| crate::client::shell::task_board::model::WorkStep {
                    text: (*text).into(),
                    done: *done,
                    ..Default::default()
                },
            )
            .collect(),
        ..Default::default()
    }
}

/// The fixture with task 7, which herdr-w1 works on, linked to `card` by its slug.
fn board_with_card(card: crate::client::shell::task_board::model::WorkItem) -> TaskBoard {
    fixture_board_with(|list| {
        if let Some(task) = list.tasks.iter_mut().find(|task| task.n == 7) {
            task.card = card.slug.clone();
        }
        list.work.push(card);
    })
}

/// The text under column `name` on IN PROGRESS's row for `tab`, and that text's cell.
fn live_cell<'a>(
    render: &'a BoardRender,
    tab: &str,
    name: &str,
) -> (String, &'a ratatui::buffer::Cell) {
    let cells = |y: usize| -> Vec<String> {
        render.rows[y]
            .split('│')
            .map(|cell| cell.trim().to_owned())
            .collect()
    };
    let head = render.row_with("│ Agent ");
    let at = cells(head)
        .iter()
        .position(|cell| cell == name)
        .unwrap_or_else(|| panic!("no {name:?} column: {:?}", render.rows[head]));
    let y = render.row_with(&format!(" {tab} "));
    let text = cells(y)[at].clone();
    assert!(
        !text.is_empty(),
        "{name} of {tab} blank: {:?}",
        render.rows[y]
    );
    let x = render.column(head, name);
    (text, &render.buffer[(x, y as u16)])
}

#[test]
fn live_columns_are_agent_working_on_stage_step_now_doing_and_for() {
    let lead = labelled(
        working("w2:p1", "herdr", "herdr-lead", None),
        AgentLabels {
            stage: Some("build · herdr-w1 on B4c".into()),
            ..AgentLabels::default()
        },
    );
    let render = render_board_with_agents(&fixture_board(), WIDE, 60, &[lead]);
    let heading = render.row_with("IN PROGRESS 1");
    assert_eq!(
        split_cells(&render.rows[heading + 2]),
        ["Agent", "Working on", "Stage", "Step now", "Doing", "For"]
    );
    // A lead with no card and no `doing`: its stage text says what it is on and does.
    assert_eq!(
        split_cells(&render.rows[heading + 4]),
        [
            LEAD_SPINNER,
            "herdr-lead",
            "build · herdr-w1 on B4c",
            "build",
            "build",
            "build · herdr-w1 on B4c"
        ]
    );
    let (_, stage) = live_cell(&render, "herdr-lead", "Stage");
    assert_eq!(stage.fg, render.palette.yellow);
}

#[test]
fn live_working_on_takes_the_card_then_the_task_then_its_labels_then_the_tab_then_the_folder() {
    let w1 = || working("w2:p2", "herdr", "herdr-w1", None);
    // A card by the task's `card`: its title.
    let board = board_with_card(card_of(
        "b4c",
        "Board B4c",
        WorkStage::Build,
        &[("tasks view", true), ("work view", false)],
    ));
    let render = render_board_with_agents(&board, WIDE, 60, &[w1()]);
    assert_eq!(live_cell(&render, "herdr-w1", "Working on").0, "Board B4c");
    assert_eq!(live_cell(&render, "herdr-w1", "For").0, "12m");
    // No card: the task it is on.
    let render = render_board_with_agents(&fixture_board(), WIDE, 60, &[w1()]);
    let (text, _) = live_cell(&render, "herdr-w1", "Working on");
    assert!(text.ends_with("Never cut the lead's name"), "{text:?}");
    // A `card` no card has: the task, as with none.
    let board = fixture_board_with(|list| {
        if let Some(task) = list.tasks.iter_mut().find(|task| task.n == 7) {
            task.card = Some("gone".into());
        }
    });
    let render = render_board_with_agents(&board, WIDE, 60, &[w1()]);
    let (text, _) = live_cell(&render, "herdr-w1", "Working on");
    assert!(text.ends_with("Never cut the lead's name"), "{text:?}");
    // On no task: its `task` label, its stage, its tab's own name, its folder, in turn.
    let steps = [
        AgentLabels {
            task: Some("fix the rail".into()),
            stage: Some("review · 2 findings".into()),
            tab_name: Some("notes pass".into()),
            folder: Some("dot-works".into()),
            ..AgentLabels::default()
        },
        AgentLabels {
            stage: Some("review · 2 findings".into()),
            tab_name: Some("notes pass".into()),
            folder: Some("dot-works".into()),
            ..AgentLabels::default()
        },
        AgentLabels {
            tab_name: Some("notes pass".into()),
            folder: Some("dot-works".into()),
            ..AgentLabels::default()
        },
        AgentLabels {
            folder: Some("dot-works".into()),
            ..AgentLabels::default()
        },
    ];
    for (labels, want) in steps.into_iter().zip([
        "fix the rail",
        "review · 2 findings",
        "notes pass",
        "dot-works",
    ]) {
        let agent = labelled(working("w8:p1", "oakwood", "oak-w2", None), labels);
        let render = render_board_with_agents(&fixture_board(), WIDE, 60, &[agent]);
        assert_eq!(live_cell(&render, "oak-w2", "Working on").0, want);
    }
}

#[test]
fn live_stage_is_the_cards_stage_in_its_colour_and_a_waiting_stage_is_plain() {
    let build = AgentLabels {
        stage: Some("build · x".into()),
        ..AgentLabels::default()
    };
    // The card's stage wins over the pane's.
    let board = board_with_card(card_of("b4c", "Board B4c", WorkStage::Review, &[]));
    let agent = labelled(working("w2:p2", "herdr", "herdr-w1", None), build.clone());
    let render = render_board_with_agents(&board, WIDE, 60, &[agent]);
    let (text, cell) = live_cell(&render, "herdr-w1", "Stage");
    assert_eq!(
        (text.as_str(), cell.fg),
        (
            "review",
            crate::client::shell::task_board::style::rose(&render.palette)
        )
    );
    let board = board_with_card(card_of("b4c", "Board B4c", WorkStage::Check, &[]));
    let agent = labelled(working("w2:p2", "herdr", "herdr-w1", None), build);
    let render = render_board_with_agents(&board, WIDE, 60, &[agent]);
    let (text, cell) = live_cell(&render, "herdr-w1", "Stage");
    assert_eq!((text.as_str(), cell.fg), ("verify", render.palette.blue));
    // No card: the pane stage's first word, coloured when it is a phase.
    for (stage, want, colour) in [
        ("plan · B5", "plan", render.palette.mauve),
        ("waiting · Nadine: 2 picks", "waiting", render.palette.text),
    ] {
        let agent = labelled(
            working("w8:p1", "oakwood", "oak-lead", None),
            AgentLabels {
                stage: Some(stage.into()),
                ..AgentLabels::default()
            },
        );
        let render = render_board_with_agents(&fixture_board(), WIDE, 60, &[agent]);
        let (text, cell) = live_cell(&render, "oak-lead", "Stage");
        assert_eq!((text.as_str(), cell.fg), (want, colour));
    }
}

#[test]
fn live_step_now_is_the_cards_first_undone_step_else_the_stage_else_working() {
    let w1 = || working("w2:p2", "herdr", "herdr-w1", None);
    let board = board_with_card(card_of(
        "b4c",
        "Board B4c",
        WorkStage::Build,
        &[
            ("tasks view", true),
            ("write tests", false),
            ("ship", false),
        ],
    ));
    let render = render_board_with_agents(&board, WIDE, 60, &[w1()]);
    assert_eq!(live_cell(&render, "herdr-w1", "Step now").0, "write tests");
    let board = board_with_card(card_of(
        "b4c",
        "Board B4c",
        WorkStage::Build,
        &[("tasks view", true)],
    ));
    let render = render_board_with_agents(&board, WIDE, 60, &[w1()]);
    assert_eq!(live_cell(&render, "herdr-w1", "Step now").0, "build");
    // No card, no stage.
    let render = render_board_with_agents(&fixture_board(), WIDE, 60, &[w1()]);
    let (text, cell) = live_cell(&render, "herdr-w1", "Step now");
    assert_eq!(
        (text.as_str(), cell.fg),
        ("working", render.palette.overlay0)
    );
}

#[test]
fn live_doing_is_the_doing_label_else_the_stage_text_else_working() {
    let with = |doing: Option<&str>, stage: Option<&str>| {
        labelled(
            working("w8:p1", "oakwood", "oak-w2", None),
            AgentLabels {
                doing: doing.map(Into::into),
                stage: stage.map(Into::into),
                ..AgentLabels::default()
            },
        )
    };
    let doing = |agent: BoardAgent| {
        let render = render_board_with_agents(&fixture_board(), WIDE, 60, &[agent]);
        let (text, cell) = live_cell(&render, "oak-w2", "Doing");
        (text, cell.fg == render.palette.overlay0)
    };
    assert_eq!(
        doing(with(Some("editing table.rs"), Some("build · x"))),
        ("editing table.rs".to_owned(), false)
    );
    assert_eq!(
        doing(with(None, Some("build · x"))),
        ("build · x".to_owned(), false)
    );
    assert_eq!(doing(with(None, None)), ("working".to_owned(), true));
}

#[test]
fn live_doing_on_a_board_task_falls_back_to_the_task_label_the_row_does_not_show() {
    // herdr-w1 is on task 7, so Working on names the task and its `task` label shows nowhere
    // else.
    let agent = labelled(
        working("w2:p2", "herdr", "herdr-w1", None),
        AgentLabels {
            task: Some("round 13a".into()),
            ..AgentLabels::default()
        },
    );
    let render = render_board_with_agents(&fixture_board(), WIDE, 60, &[agent]);
    let (text, _) = live_cell(&render, "herdr-w1", "Working on");
    assert!(text.ends_with("Never cut the lead's name"), "{text:?}");
    assert_eq!(live_cell(&render, "herdr-w1", "Doing").0, "round 13a");
}

#[test]
fn board_agents_read_each_agents_stage_doing_and_task_labels_its_tab_name_and_folder() {
    use crate::api::schema::AgentStatus;
    use crate::client::shell::task_board::board_agents;
    let mut snap = snapshot();
    let mut named = snap.tabs[0].clone();
    named.tab_id = "tab_named".into();
    named.label = "notes pass".into();
    named.custom_label = true;
    snap.tabs.push(named);
    let mut pane = snap.panes[0].clone();
    pane.pane_id = "p2".into();
    pane.tab_id = "tab_named".into();
    pane.cwd = Some("/home/user/devbox".into());
    pane.foreground_cwd = None;
    snap.panes.push(pane);
    let mut first = live_agent(
        "pane_1",
        "ws_1",
        "herdr-w1",
        AgentStatus::Working,
        &[
            ("stage", "build · x"),
            ("doing", "running tests"),
            ("task", "fix the rail"),
        ],
    );
    first.tab_id = "tab_1".into();
    let mut second = live_agent(
        "p2",
        "ws_1",
        "oak-w2",
        AgentStatus::Working,
        &[("doing", " ")],
    );
    second.tab_id = "tab_named".into();
    snap.agents = vec![first, second];
    let config = ClientShellConfig::from_config(&Config::default());
    let got: Vec<AgentLabels> = board_agents(Some(&snap), &config, Some("/home/user/devbox"), 0)
        .into_iter()
        .map(|agent| agent.labels)
        .collect();
    assert_eq!(
        got,
        [
            // A tab of a default name gives none; its pane's folder its last part.
            AgentLabels {
                stage: Some("build · x".into()),
                doing: Some("running tests".into()),
                task: Some("fix the rail".into()),
                tab_name: None,
                folder: Some("repo".into()),
            },
            // A blank `doing` is none; the home folder is `~`.
            AgentLabels {
                tab_name: Some("notes pass".into()),
                folder: Some("~".into()),
                ..AgentLabels::default()
            },
        ]
    );
}

#[test]
fn live_working_on_is_never_blank_for_a_lead_or_a_worker_on_no_task() {
    let board = tasks_and_work_board();
    let mut agents = lead_and_workers();
    agents[0].labels.stage = Some("build · w1 on 13a".into());
    agents[4].labels.folder = Some("oakwood-cli".into());
    for width in [186, 89] {
        let render = render_board_with_agents(&board, width, 46, &agents);
        let row = |name: &str| render.row_with(&format!(" {name} "));
        let working_on = |name: &str| split_cells(&render.rows[row(name)])[2].clone();
        // A worker on no task of the board: its folder, in the text colour.
        assert_eq!(working_on("oak-w1"), "oakwood-cli", "{width} wide");
        assert_eq!(
            render.cell(row("oak-w1"), "oakwood-cli").fg,
            render.palette.text
        );
        // A lead over its workers: its stage text; a lead at rest has no row.
        assert_eq!(
            working_on("herdr-lead"),
            "build · w1 on 13a",
            "{width} wide"
        );
        assert!(
            !render.rows.iter().any(|row| row.contains(" oak-lead ")),
            "{width} wide: {:#?}",
            render.rows
        );
        assert!(
            !render
                .rows
                .iter()
                .any(|row| row.contains("no task on board")),
            "{width} wide: {:#?}",
            render.rows
        );
    }
}

#[test]
fn live_shows_six_tabs_and_folds_the_rest_into_a_more_row() {
    let agents: Vec<BoardAgent> = (1..=9)
        .map(|n| working(&format!("p{n}"), "sample", &format!("tab-{n}"), None))
        .collect();
    let render = render_board_with_agents(&fixture_board(), WIDE, 80, &agents);
    assert!(render.rows[3].contains("IN PROGRESS 9"));
    for n in 1..=6 {
        render.row_with(&format!("tab-{n} "));
    }
    assert!(!render.rows.iter().any(|row| row.contains("tab-7")));
    let more = render.row_with("+3 more");
    assert!(
        render.rows[more].starts_with(" │  +3 more") && render.rows[more].ends_with('│'),
        "{:?}",
        render.rows[more]
    );
    assert_eq!(
        split_cells(&render.rows[more]),
        ["+3 more"],
        "no column lines"
    );
    assert_eq!(render.cell(more, "+3 more").fg, render.palette.overlay0);
    assert!(render.rows[more + 1].starts_with(" ╰"));
    let live_rows = render
        .hits
        .spans
        .iter()
        .filter(|(key, _, _)| matches!(key, RowKey::Agent(_)))
        .count();
    assert_eq!(live_rows, 6);
}

/// A tab waiting on Nadine in `space`: asking `ask`, or blocked for an approval.
fn waiting(pane_id: &str, space: &str, tab: &str, ask: Option<&str>) -> BoardAgent {
    BoardAgent {
        pane_id: pane_id.into(),
        space: space.into(),
        tab: tab.into(),
        state: AgentState::Waiting(ask.map(Into::into)),
        lead: None,
        // As `agent_mark` gives it: a question's yellow `?`, a yes or no's red `!`.
        mark: match ask {
            Some(_) => ("?", Palette::catppuccin().yellow),
            None => ("!", Palette::catppuccin().red),
        },
        labels: AgentLabels::default(),
    }
}

/// An agent's ACTION REQUIRED row reads its own mark, as the sidebar draws it, in its `#` cell
/// and before its action: a yes or no a red `!`, a question a yellow `?`. A task's row keeps
/// the section's yellow `?`.
#[test]
fn waiting_rows_take_their_agents_mark() {
    use crate::api::schema::AgentStatus;
    use crate::client::shell::task_board::board_agents;
    // The agents as the board reads them from the server, so each row's mark is the one
    // `agent_mark` gives the sidebar.
    let mut snap = snapshot();
    snap.workspaces = vec![workspace("ws_h", "herdr", "/h")];
    snap.agents = vec![
        live_agent(
            "w5:p1",
            "ws_h",
            "oak-lead",
            AgentStatus::Blocked,
            &[("ask", "pick A or B")],
        ),
        live_agent("w6:p1", "ws_h", "herdr-w3", AgentStatus::Blocked, &[]),
    ];
    let mut config = ClientShellConfig::from_config(&Config::default());
    config.palette = Palette::catppuccin();
    let agents = board_agents(Some(&snap), &config, None, 0);
    let render = render_board_with_agents(&fixture_board(), WIDE, 60, &agents);
    let asks = render.row_with("pick A or B");
    let approves = render.row_with("asks for an approval");
    let task = approves + 1;
    assert!(
        render.rows[task].starts_with(" │ 1 "),
        "{:?}",
        render.rows[task]
    );
    for (row, glyph, color, action) in [
        (asks, "?", render.palette.yellow, "answer in tab"),
        (approves, "!", render.palette.red, "approve in tab"),
    ] {
        assert_eq!(
            split_cells(&render.rows[row])[0],
            glyph,
            "{:?}",
            render.rows[row]
        );
        assert_eq!(render.cell(row, glyph).fg, color);
        let before = format!("{glyph} {action}");
        assert_eq!(
            render.cell(row, &before).fg,
            color,
            "{:?}",
            render.rows[row]
        );
    }
    // The first task row of the section with an action.
    let (task, action) = (task..render.row_with("WAITING"))
        .find_map(|row| {
            let action = split_cells(&render.rows[row])
                .into_iter()
                .find(|cell| cell.starts_with("? "))?;
            Some((row, action))
        })
        .unwrap_or_else(|| panic!("no task row with an action: {:#?}", render.rows));
    assert!(
        render.rows[task].starts_with(" │ "),
        "{:?}",
        render.rows[task]
    );
    assert_eq!(render.cell(task, &action).fg, render.palette.yellow);
}

#[test]
fn only_the_users_actions_take_the_question_mark() {
    // Task 8 is Claude's next step, moved into ACTION REQUIRED; task 5 is Nadine's.
    let board = fixture_board_in_section(8, "action_required");
    for width in [WIDE, NARROW] {
        let render = render_board(&board, width, 60);
        let heading = render.row_with("ACTION REQUIRED");
        let claude = render.row_with("next oak-cli round");
        let yours = render.row_with("say commit");
        assert!(claude > heading && yours > heading, "{:#?}", render.rows);
        assert!(
            render.rows[yours].contains("? say commit"),
            "{:?}",
            render.rows[yours]
        );
        assert!(
            !render.rows[claude].contains("? next oak-cli round"),
            "{:?}",
            render.rows[claude]
        );
        assert_eq!(
            render.cell(claude, "next oak-cli round").fg,
            render.palette.text
        );
    }
}

#[test]
fn tabs_waiting_on_you_head_needs_you_and_count_in_it() {
    let agents = [
        working("w2:p2", "herdr", "herdr-w1", None),
        waiting(
            "w5:p1",
            "oakwood",
            "oak-lead",
            Some("pick A or B for the panel"),
        ),
        waiting("w6:p1", "herdr", "herdr-w3", None),
    ];
    let render = render_board_with_agents(&fixture_board(), WIDE, 60, &agents);
    // Action counts the tabs waiting on Nadine with the tasks; In progress, the one at work.
    assert!(
        render.rows[2].contains("◐ In progress 1   ! Action required 7"),
        "{:?}",
        render.rows[2]
    );
    let heading = render.row_with("ACTION REQUIRED 7");
    let first = heading + 4;
    assert_eq!(
        split_cells(&render.rows[first]),
        [
            "?",
            "oak-lead",
            "pick A or B for the panel",
            "? answer in tab",
            "now"
        ]
    );
    let mark = render.cell(first, "?");
    assert_eq!(mark.fg, render.palette.yellow);
    assert!(!mark.modifier.contains(Modifier::BOLD));
    assert_eq!(render.cell(first, "oak-lead ").fg, render.palette.overlay0);
    assert_eq!(render.cell(first, "pick A").fg, render.palette.text);
    assert_eq!(render.cell(first, "? answer").fg, render.palette.yellow);
    assert_eq!(render.cell(first, "answer in tab").fg, render.palette.text);
    assert_eq!(
        split_cells(&render.rows[first + 1]),
        [
            "!",
            "herdr-w3",
            "asks for an approval",
            "! approve in tab",
            "now"
        ]
    );
    assert!(
        render.rows[first + 2].starts_with(" │ 1 ") && render.rows[first + 2].contains("Say final"),
        "then the tasks, newest first, numbered from 1: the tabs have no number"
    );
    let order: Vec<&RowKey> = render
        .hits
        .spans
        .iter()
        .map(|(key, _, _)| key)
        .take(4)
        .collect();
    assert_eq!(
        order,
        [
            &RowKey::Agent("w2:p2".into()),
            &RowKey::Agent("w5:p1".into()),
            &RowKey::Agent("w6:p1".into()),
            &key(4)
        ]
    );

    // Open, its row says what it asks and its block, with no fact its row leaves out, offers
    // only Enter and close; the footer drops the keys that act on a task.
    let mut board = fixture_board();
    board.can_open = true;
    board.selected = Some(RowKey::Agent("w5:p1".into()));
    board.open_row = board.selected.clone();
    let render = render_board_with_agents(&board, WIDE, 60, &agents);
    let row = render.row_with("│ pick A or B");
    assert!(
        render.rows[row + 1].starts_with(" │      Enter  go to agent     ←  close "),
        "{:?}",
        render.rows[row + 1]
    );
    assert!(!render.rows[row + 1].contains("done"));
    let footer = &render.rows[59];
    for dropped in ["Enter", "o  open", "d  done", "x  delete", "m  move"] {
        assert!(!footer.contains(dropped), "{dropped:?} in {footer:?}");
    }
    assert!(footer.contains("↑ ↓  select") && footer.contains("?  all keys"));

    // Asked for NEEDS YOU alone, the waiting tabs stay and LIVE goes.
    let mut board = fixture_board();
    board.only_section = Some(TaskSection::ActionRequired);
    let render = render_board_with_agents(&board, WIDE, 60, &agents);
    assert!(render.rows.iter().any(|row| row.contains("│ pick A or B")));
    assert!(!render.rows.iter().any(|row| row.contains("IN PROGRESS")));
    board.only_section = Some(TaskSection::Queued);
    let render = render_board_with_agents(&board, WIDE, 60, &agents);
    assert!(!render.rows.iter().any(|row| row.contains("oak-lead")));
}

#[test]
fn a_waiting_tab_named_by_a_needs_you_task_shows_once_as_the_tab() {
    let mut list = parse_task_list(LIST_V1).expect("fixture");
    list.tasks[4].agent = Some(crate::client::shell::task_board::model::TaskAgent {
        pane: "w5:p1".into(),
        name: "oak-lead".into(),
        minutes: None,
    });
    let mut board = fixture_board();
    board.store.receive(Ok(list), std::time::Instant::now());
    let agents = [waiting("w5:p1", "oakwood", "oak-lead", Some("say commit"))];
    let render = render_board_with_agents(&board, WIDE, 60, &agents);
    assert!(render
        .rows
        .iter()
        .any(|row| row.contains("ACTION REQUIRED 5")));
    // The tab in the Project column, what it asks in the Task column.
    assert!(render
        .rows
        .iter()
        .any(|row| row.contains("│ oak-lead ") && row.contains("│ say commit")));
    assert!(
        !render
            .rows
            .iter()
            .any(|row| row.contains("Commit your theme")),
        "{:#?}",
        render.rows
    );

    // A filter that keeps the task but not its tab shows the task: the rule holds only
    // while the tab's row is shown.
    board.filter = "theme".into();
    let render = render_board_with_agents(&board, WIDE, 60, &agents);
    assert!(
        render
            .rows
            .iter()
            .any(|row| row.contains("Commit your theme")),
        "{:#?}",
        render.rows
    );
    assert!(!render.rows.iter().any(|row| row.contains("│ oak-lead ")));
}

#[test]
fn a_needs_you_task_hides_only_while_its_waiting_tab_shows_in_its_place() {
    // The rows and the counts take the waiting tabs from one place: in every case the tab
    // named by task 5 or the task shows, never both and never neither.
    let mut list = parse_task_list(LIST_V1).expect("fixture");
    list.tasks[4].agent = Some(crate::client::shell::task_board::model::TaskAgent {
        pane: "w5:p1".into(),
        name: "oak-lead".into(),
        minutes: None,
    });
    let asking = waiting("w5:p1", "oakwood", "oak-lead", Some("say commit"));
    let at_rest = BoardAgent {
        state: AgentState::Resting,
        ..asking.clone()
    };
    for (agents, filter, space, tab_shows) in [
        (&asking, "", None, true),
        (&asking, "theme", None, false),
        (&asking, "", Some("oakwood"), true),
        (&at_rest, "", None, false),
    ] {
        let mut board = fixture_board();
        board
            .store
            .receive(Ok(list.clone()), std::time::Instant::now());
        board.filter = filter.into();
        board.space = space.map(Into::into);
        let render = render_board_with_agents(&board, WIDE, 60, std::slice::from_ref(agents));
        let tab = render.rows.iter().any(|row| row.contains("│ oak-lead "));
        let task = render
            .rows
            .iter()
            .any(|row| row.contains("Commit your theme"));
        assert_eq!(
            (tab, task),
            (tab_shows, !tab_shows),
            "{filter:?} {space:?} {:?}: {:#?}",
            agents.state,
            render.rows
        );
    }
}

#[test]
fn keys_that_act_on_a_task_do_nothing_on_a_waiting_tab_and_enter_goes_to_it() {
    let mut state = board_state();
    let mut snap = snapshot();
    snap.workspaces = vec![workspace("ws_z", "oakwood", "/home/user/devbox/oakwood")];
    snap.agents = vec![live_agent(
        "p_ask",
        "ws_z",
        "oak-lead",
        crate::api::schema::AgentStatus::Idle,
        &[("ask", "pick A or B")],
    )];
    state.set_snapshot(Box::new(snap));
    state.task_board.open = true;
    board_rows(&mut state);
    let row = row_of(&state, &RowKey::Agent("p_ask".into()));
    board_click(&mut state, row);
    assert_eq!(
        state.task_board.selected,
        Some(RowKey::Agent("p_ask".into()))
    );
    board_rows(&mut state);
    for code in ['d', 'x', 'm', 'J', 'K', 'n', 'o'] {
        let outcome = press_key(&mut state, KeyCode::Char(code));
        assert!(act_requests(&outcome).is_empty(), "{code}");
        assert!(open_requests(&outcome).is_empty(), "{code}");
        assert_eq!(state.task_board.notice, None, "{code}");
        assert_eq!(state.task_board.prompt, None, "{code}");
    }
    let outcome = press_key(&mut state, KeyCode::Enter);
    assert_eq!(focused_pane(&outcome), Some("p_ask"));
    assert!(!state.task_board.open);
}

/// A snapshot with one space, `herdr`, and one tab at work in it, on pane `p_live`.
fn snap_with_live() -> ClientShellSnapshot {
    let mut snap = snapshot();
    snap.workspaces = vec![workspace("ws_herdr", "herdr", "/home/user/devbox/herdr")];
    snap.agents = vec![live_agent(
        "p_live",
        "ws_herdr",
        "herdr-w1",
        crate::api::schema::AgentStatus::Working,
        &[("task", "round 13a")],
    )];
    snap
}

#[test]
fn the_live_heading_folds_by_click_or_enter_and_a_live_row_goes_to_its_tab() {
    let mut state = board_state();
    state.set_snapshot(Box::new(snap_with_live()));
    state.task_board.open = true;
    let rows = board_rows(&mut state);
    assert!(
        rows.iter().any(|row| row.contains("IN PROGRESS 1 ▼")),
        "{rows:#?}"
    );

    // A click on its heading folds it to the heading alone.
    let heading = state.hits.task_board.live_heading;
    board_click(&mut state, heading);
    assert!(state.task_board.live_folded);
    let rows = board_rows(&mut state);
    let at = rows
        .iter()
        .position(|row| row.contains("IN PROGRESS 1 ▶"))
        .unwrap_or_else(|| panic!("folded: {rows:#?}"));
    assert!(!rows[at + 1].contains('╭'), "no box: {rows:#?}");
    assert!(!rows.iter().any(|row| row.contains("round 13a")));

    // Up from the first row reaches the folded heading; Down leaves it; Enter opens it.
    state.task_board.selected = Some(key(4));
    press_key(&mut state, KeyCode::Up);
    assert_eq!(state.task_board.focused_heading, Some(Fold::Live));
    assert_eq!(state.task_board.selected, None);
    board_rows(&mut state);
    // Right opens no row while the heading has the focus, even with a row selected.
    state.task_board.selected = Some(key(4));
    press_key(&mut state, KeyCode::Right);
    assert_eq!(state.task_board.open_row, None, "right on the heading");
    state.task_board.selected = None;
    press_key(&mut state, KeyCode::Down);
    assert_eq!(state.task_board.selected, Some(key(4)), "down leaves it");
    assert_eq!(state.task_board.focused_heading, None);
    press_key(&mut state, KeyCode::Up);
    board_rows(&mut state);
    press_key(&mut state, KeyCode::Enter);
    assert!(!state.task_board.live_folded, "Enter opens it");
    assert_eq!(state.task_board.focused_heading, None);

    // Its focus goes when LIVE does, so it does not come back focused with it.
    state.task_board.live_folded = true;
    board_rows(&mut state);
    state.task_board.focused_heading = Some(Fold::Live);
    let mut idle = state.snapshot.as_deref().expect("snapshot").clone();
    idle.agents.clear();
    state.set_snapshot(Box::new(idle));
    board_rows(&mut state);
    assert_eq!(state.task_board.focused_heading, None);
    state.set_snapshot(Box::new(snap_with_live()));
    state.task_board.live_folded = false;

    // A LIVE row selects like a task's; Enter goes to its tab and the board closes.
    board_rows(&mut state);
    let live_row = row_of(&state, &RowKey::Agent("p_live".into()));
    board_click(&mut state, live_row);
    assert_eq!(
        state.task_board.selected,
        Some(RowKey::Agent("p_live".into()))
    );
    board_rows(&mut state);
    let outcome = press_key(&mut state, KeyCode::Enter);
    assert_eq!(focused_pane(&outcome), Some("p_live"));
    assert!(!state.task_board.open);
}

#[test]
fn enter_on_a_task_with_no_agent_but_a_matching_space_focuses_that_workspace() {
    let mut state = board_state();
    let mut snap = snapshot();
    snap.workspaces = vec![workspace(
        "ws_oakwood",
        "oakwood",
        "/home/user/devbox/Workdir/oakwood-workspace",
    )];
    state.set_snapshot(Box::new(snap));
    state.task_board.open = true;
    state.task_board.selected = Some(key(1));
    state.compose(106, 40).expect("frame");

    let outcome = press_key(&mut state, KeyCode::Enter);
    assert_eq!(focused_pane(&outcome), None);
    assert!(
        open_requests(&outcome).is_empty(),
        "the space wins over opening the file: {:?}",
        outcome.actions
    );
    assert_eq!(workspace_focus_requests(&outcome), ["ws_oakwood"]);
    assert!(
        !state.task_board.open,
        "the board closes once it goes to the space"
    );
}

#[test]
fn enter_picks_the_workspace_holding_the_files_file_when_two_share_its_space_label() {
    let mut state = board_state();
    let mut snap = snapshot();
    let other = workspace("ws_other", "oakwood", "/home/user/devbox/elsewhere");
    let matching = workspace(
        "ws_oakwood",
        "oakwood",
        "/home/user/devbox/Workdir/oakwood-workspace",
    );
    snap.workspaces = vec![other, matching];
    state.set_snapshot(Box::new(snap));
    state.task_board.open = true;
    state.task_board.selected = Some(key(1));
    state.compose(106, 40).expect("frame");

    let outcome = press_key(&mut state, KeyCode::Enter);
    assert_eq!(
        workspace_focus_requests(&outcome),
        ["ws_oakwood"],
        "the one whose directory holds the task's file, not the first sharing the label"
    );
}

#[test]
fn enter_does_not_pick_a_workspace_whose_directory_only_shares_a_name_prefix() {
    let mut state = board_state();
    let mut snap = snapshot();
    // "oak" is a text-prefix of "oakwood-workspace", but not its parent directory: the
    // task's file lives under neither, so this decoy must lose to the real one below it.
    let decoy = workspace("ws_decoy", "oakwood", "/home/user/devbox/Workdir/oak");
    let matching = workspace(
        "ws_oakwood",
        "oakwood",
        "/home/user/devbox/Workdir/oakwood-workspace",
    );
    snap.workspaces = vec![decoy, matching];
    state.set_snapshot(Box::new(snap));
    state.task_board.open = true;
    state.task_board.selected = Some(key(1));
    state.compose(106, 40).expect("frame");

    let outcome = press_key(&mut state, KeyCode::Enter);
    assert_eq!(
        workspace_focus_requests(&outcome),
        ["ws_oakwood"],
        "a shared text prefix is not the same as holding the file: {:?}",
        outcome.actions
    );
}

#[test]
fn enter_falls_back_to_opening_the_file_when_two_workspaces_share_the_space_label() {
    let mut state = board_state();
    let mut snap = snapshot();
    snap.workspaces = vec![
        workspace("ws_a", "oakwood", "/home/user/devbox/elsewhere-a"),
        workspace("ws_b", "oakwood", "/home/user/devbox/elsewhere-b"),
    ];
    state.set_snapshot(Box::new(snap));
    state.task_board.open = true;
    state.task_board.selected = Some(key(1));
    state.compose(106, 40).expect("frame");

    let outcome = press_key(&mut state, KeyCode::Enter);
    assert!(workspace_focus_requests(&outcome).is_empty());
    assert_eq!(
        open_requests(&outcome)
            .iter()
            .map(|(_, file)| file.clone())
            .collect::<Vec<_>>(),
        ["/home/user/devbox/Workdir/oakwood-workspace/WORKSPACE-STATUS.md".to_owned()]
    );
}

#[test]
fn enter_falls_back_to_opening_the_file_when_two_same_label_workspaces_both_hold_it() {
    // Under the Home policy every workspace starts in $HOME, so both hold the file.
    let mut state = board_state();
    let mut snap = snapshot();
    snap.workspaces = vec![
        workspace("ws_a", "oakwood", "/home/user/devbox"),
        workspace("ws_b", "oakwood", "/home/user/devbox"),
    ];
    state.set_snapshot(Box::new(snap));
    state.task_board.open = true;
    state.task_board.selected = Some(key(1));
    state.compose(106, 40).expect("frame");

    let outcome = press_key(&mut state, KeyCode::Enter);
    assert!(
        workspace_focus_requests(&outcome).is_empty(),
        "neither is picked when both hold the file: {:?}",
        outcome.actions
    );
    assert_eq!(open_requests(&outcome).len(), 1);
}

#[test]
fn o_opens_the_selected_tasks_file_whether_or_not_it_has_an_agent() {
    let mut state = board_state();
    state.task_board.open = true;
    state.task_board.selected = Some(key(1));
    state.compose(106, 40).expect("frame");
    let sent = open_requests(&press_key(&mut state, KeyCode::Char('o')));
    assert_eq!(
        sent.iter()
            .map(|(_, file)| file.as_str())
            .collect::<Vec<_>>(),
        ["/home/user/devbox/Workdir/oakwood-workspace/WORKSPACE-STATUS.md"]
    );
    assert!(state.task_board.open, "the board waits for the answer");

    // A task with an agent: `o` still opens the file, and never goes to the agent.
    let mut state = board_state();
    state.task_board.open = true;
    state.task_board.selected = Some(key(6));
    state.compose(106, 40).expect("frame");
    let outcome = press_key(&mut state, KeyCode::Char('o'));
    assert_eq!(focused_pane(&outcome), None, "o never goes to the agent");
    assert_eq!(open_requests(&outcome).len(), 1);
}

/// The footer hint naming `key`, as the last frame drew it.
fn footer_hint(state: &ClientShellState, key: &str) -> Option<Rect> {
    state
        .hits
        .task_board
        .footer_keys
        .iter()
        .find(|(_, named)| *named == key)
        .map(|(rect, _)| *rect)
}

#[test]
fn a_click_on_a_footer_hint_does_its_key_on_the_selection() {
    let mut state = acting_state(1);
    state.task_board.keys_shown = true;
    state.compose(160, 40).expect("frame");
    for key in ["Enter", "d", "x", "m", "J", "K", "n", "Tab", "Esc"] {
        assert!(footer_hint(&state, key).is_some(), "{key} is clickable");
    }
    for key in ["↑ ↓", "click a space", "J/K"] {
        assert!(footer_hint(&state, key).is_none(), "{key} names no one key");
    }
    let hint = footer_hint(&state, "d").expect("d");
    let (_, params) = one_act(&board_click(&mut state, hint));
    assert_eq!(params.action, "done");
    assert_eq!(params.copies, copies_of(1));

    let mut state = acting_state(1);
    state.task_board.keys_shown = true;
    state.compose(160, 40).expect("frame");
    let hint = footer_hint(&state, "K").expect("K");
    let (_, params) = one_act(&board_click(&mut state, hint));
    assert_eq!(params.direction.as_deref(), Some("up"));

    let hint = footer_hint(&state, "Tab").expect("Tab");
    board_click(&mut state, hint);
    assert_eq!(state.task_board.view, BoardView::Work);
    state.compose(160, 40).expect("frame");
    for key in ["Enter", "o", "t", "Tab", "Esc"] {
        assert!(
            footer_hint(&state, key).is_some(),
            "Work: {key} is clickable"
        );
    }
    let hint = footer_hint(&state, "Esc").expect("Esc");
    board_click(&mut state, hint);
    assert!(!state.task_board.open);
}

#[test]
fn a_board_too_small_says_the_size_it_needs_on_one_line_with_its_close() {
    for (width, height) in [(39, 40), (WIDE, 9), (20, 5)] {
        let render = render_board(&fixture_board(), width, height);
        let line = &render.rows[0];
        assert!(
            line.starts_with(" Window too") && line.ends_with('×'),
            "{width}x{height}: {line:?}"
        );
        assert!(
            render.rows[1..].iter().all(|row| row.is_empty()),
            "{width}x{height}: one line: {:#?}",
            render.rows
        );
        assert_eq!(render.hits.close, Rect::new(width - 4, 0, 3, 1));
        assert!(render.hits.rows.is_empty() && render.hits.footer_keys.is_empty());
    }
    let render = render_board(&fixture_board(), 60, 9);
    assert!(render.rows[0].contains(" Window too small for the board: 40 x 10 needed"));
    let render = render_board(&fixture_board(), 40, 10);
    assert!(!render.rows[0].contains("Window too small"), "40 x 10 fits");
    assert!(render.rows[0].contains(" Tasks "), "{:?}", render.rows[0]);
}

/// The cells of the table row showing `needle`.
fn cells_of(render: &BoardRender, needle: &str) -> Vec<String> {
    split_cells(&render.rows[render.row_with(needle)])
}

/// Splits a rendered row into its cells at the lines between them and the box's borders.
fn split_cells(row: &str) -> Vec<String> {
    row.split('│')
        .map(str::trim)
        .filter(|cell| !cell.is_empty())
        .map(str::to_owned)
        .collect()
}

/// ACTION REQUIRED's column names, each with its cell's width and padding, from its
/// column-name row's lines.
fn action_required_widths(render: &BoardRender) -> Vec<(String, usize)> {
    let y = render.row_with("ACTION REQUIRED") + 2;
    let head = &render.rows[y];
    let lines = bar_columns(head);
    split_cells(head)
        .into_iter()
        .zip(lines.windows(2).map(|pair| pair[1] - pair[0] - 1))
        .collect()
}

#[test]
fn your_action_takes_spare_width_first_up_to_45_and_the_task_keeps_at_least_half() {
    let mut list = parse_task_list(LIST_V1).expect("fixture");
    let action = "sudo into the bastion, run rotate-db-password staging, paste it in 1Password";
    list.tasks[0].state = action.into();
    let mut board = fixture_board();
    board.store.receive(Ok(list), std::time::Instant::now());
    // ACTION REQUIRED alone, so the room Task shares is its own: Task, Action and the line
    // between them. Other sections' cells would take from it too.
    board.only_section = Some(TaskSection::ActionRequired);
    let width_of = |widths: &[(String, usize)], name: &str| {
        widths
            .iter()
            .find(|(named, _)| named == name)
            .map(|(_, width)| *width)
            .unwrap_or_else(|| panic!("{name}: {widths:?}"))
    };
    for width in [186, 110, 89, 70] {
        let render = render_board(&board, width, 60);
        let widths = action_required_widths(&render);
        let (task, state) = (width_of(&widths, "Task"), width_of(&widths, "Action"));
        // 45 columns of text and a column of padding each side, while the task keeps half
        // the room the two share.
        let room = task + state + 1;
        // Below 80 wide Action keeps its old most, its mark and 16 columns of text, so the
        // task reads.
        let most = if width < 80 { 20 } else { 49 };
        assert_eq!(
            state,
            most.min(room - room.div_ceil(2) - 1),
            "{width}: {widths:?}"
        );
        assert!(task >= 16, "{width}: {widths:?}");
        let at = widths
            .iter()
            .position(|(named, _)| named == "Action")
            .expect("Action");
        let shown = split_cells(&render.rows[render.row_with("Per-client Chrome")])[at].clone();
        let shown = shown.strip_prefix("? ").unwrap_or(&shown).to_owned();
        assert!(
            action.starts_with(shown.trim_end_matches('…')),
            "{width}: {shown:?}"
        );
    }
    let wide = action_required_widths(&render_board(&board, 186, 60));
    assert_eq!(
        width_of(&wide, "Action"),
        49,
        "its mark and the whole 45 at a wide board: {wide:?}"
    );
}

#[test]
fn task_board_columns_narrow_state_first_then_a_stacked_list() {
    // 110 and wider: #, Project, Task, Action up to 45 columns of text, and Age.
    let render = render_board(&fixture_board(), 110, 40);
    assert_eq!(
        cells_of(&render, "Per-client Chrome"),
        [
            "3",
            "Chrome",
            "Per-client Chrome is stuck at pairing",
            "? go on the pairing broadcast",
            "2d"
        ]
    );

    // 80 to 109: Age gives way first; Action takes its whole text and its mark, Task the
    // rest.
    let render = render_board(&fixture_board(), 92, 40);
    assert_eq!(
        cells_of(&render, "Per-client Chrome"),
        [
            "3",
            "Chrome",
            "Per-client Chrome is stuck at…",
            "? go on the pairing broadcast"
        ]
    );

    // 56 to 79: no Project, which leads the task again, and Action up to 16 columns of
    // text; open, the row wraps the action whole in its cell and the block leaves it out, and
    // a completed task's place leads its task, so no block line says where it lives.
    let mut board = fixture_board();
    board.selected = Some(key(1));
    let render = render_board(&board, 70, 40);
    assert_eq!(
        cells_of(&render, "Per-client Chrome"),
        ["3", "Chrome · Per-client Chrome is stuck…", "? go on the…"]
    );
    board.open_row = board.selected.clone();
    let render = render_board(&board, 70, 40);
    let row = render.row_with("Per-client Chrome");
    assert!(
        render.rows[row + 2].contains("│   broadcast "),
        "{:#?}",
        render.rows
    );
    assert!(
        !render.rows.iter().any(|row| row.contains("Action  go on")),
        "{:#?}",
        render.rows
    );
    board.selected = Some(key(13));
    board.open_row = board.selected.clone();
    board.done_expanded = true;
    let render = render_board(&board, 70, 60);
    assert!(
        !render.rows.iter().any(|row| row.contains("Where")),
        "{:#?}",
        render.rows
    );

    // 40 to 55: a list with no grid, inside its box: the number and task, then age and
    // state, dim, each on one line.
    let render = render_board(&fixture_board(), 50, 60);
    let first = render.row_with("Per-client Chrome");
    assert_eq!(
        render.rows[first],
        " │ 3  Chrome · Per-client Chrome is stuck at…   │"
    );
    assert_eq!(
        render.rows[first + 1],
        " │    2d · ? go on the pairing broadcast        │"
    );
    assert_eq!(render.cell(first + 1, "2d").fg, render.palette.overlay0);
    let body = &render.rows[2..render.rows.len() - 4];
    assert!(
        !body
            .iter()
            .any(|row| row.contains('┼') || row.contains('┴') || row.contains('├')),
        "no lines between cells: {body:#?}"
    );
    let row = render
        .hits
        .rows
        .iter()
        .find(|(_, target)| *target == key(1))
        .expect("a click target")
        .0;
    assert_eq!((row.y as usize, row.height), (first, 2));
}

#[test]
fn task_board_header_shortens_its_labels_then_drops_counts_from_the_right() {
    let rows = |width: u16| {
        let render = render_board(&fixture_board(), width, 40).rows;
        (render[0].clone(), render[2].clone())
    };
    let (wide, wide_chips) = rows(HEADER_WIDE);
    assert!(wide_chips.contains("! Action required 5") && wide.contains("Updated 8:05 am"));

    // The chips left the top bar, so it keeps its time and its `Search` word down to a much
    // narrower board; its words go first: the time, the sort, then `Search` for the filter
    // box's `⌕` alone.
    let (short, chips) = rows(62);
    for part in ["Space: All", " ⌕ "] {
        assert!(short.contains(part), "{part:?} in {short:?}");
    }
    assert!(
        !short.contains("Search") && !short.contains("Updated"),
        "{short:?}"
    );
    // The chips on their own row fall back to icon and count.
    assert!(chips.contains("◐ 0   ! 5   ◌ 4   … 2"), "{chips:?}");
    assert!(!chips.contains("Action"), "{chips:?}");

    // Then the space button's `▾` alone.
    let (compact, chips) = rows(44);
    assert!(!compact.contains("Space: All"), "{compact:?}");
    assert!(
        compact.contains(" ▾ ") && compact.contains(" ⌕ "),
        "{compact:?}"
    );
    for part in ["◐ 0", "! 5", "◌ 4", "… 2"] {
        assert!(chips.contains(part), "{part:?} in {chips:?}");
    }

    // The chips no longer share the top bar, so no chip drops from the right: at the
    // narrowest board all four still show whole, on their own row.
    let (narrow, chips) = rows(40);
    assert!(
        narrow.contains(" ▾ ") && narrow.contains(" ⌕ "),
        "{narrow:?}"
    );
    for part in ["◐ 0", "! 5", "◌ 4", "… 2"] {
        assert!(chips.contains(part), "{part:?} in {chips:?}");
    }

    for width in [HEADER_WIDE, WIDE, 80, 60, 56, 44, 42, 40] {
        let (row, chips) = rows(width);
        assert!(
            row.contains(" Tasks ") && row.ends_with('×'),
            "{width}: {row:?}"
        );
        assert!(
            !chips.contains('…')
                || chips.contains("… 2")
                || chips.contains("… Waiting on others 2"),
            "{width}: whole or not at all: {chips:?}"
        );
    }

    // The Phases view has no kind chips: its top bar and row 2 name none, at any width.
    let mut board = work_board();
    board.view = BoardView::Work;
    for width in [70, 44] {
        let render = render_board(&board, width, 40);
        for row in [&render.rows[0], &render.rows[2]] {
            for part in ["◆", "✱", "▣", "○", "Features"] {
                assert!(!row.contains(part), "{width}: {part:?} in {row:?}");
            }
        }
        assert_eq!(render.rows[2].trim(), "Filter ▾", "{width}");
    }
}

#[test]
fn task_board_footer_gives_way_to_one_line_of_keys_then_a_question_mark() {
    let rows = |board: &TaskBoard, width: u16, height: u16| render_board(board, width, height).rows;
    let board = fixture_board();

    let wide = rows(&board, WIDE, 40);
    assert!(wide[39].contains("d  done") && wide[39].contains("Enter  go to space"));
    assert!(
        wide[38].trim().chars().all(|ch| ch == '─'),
        "{:?}",
        wide[38]
    );

    // Narrower, the keys that fit before `? all keys`, the most used first.
    let one = rows(&board, 72, 40);
    let keys = &one[39];
    assert_eq!(
        keys.trim(),
        "↑ ↓  select     Enter  go to space     d  done     ?  all keys"
    );
    assert!(one[38].trim().chars().all(|ch| ch == '─'), "{:?}", one[38]);

    let short = rows(&board, WIDE, 15);
    assert!(
        short[14].contains("?  all keys"),
        "a short window takes one line: {short:#?}"
    );
    assert!(short[13].trim().chars().all(|ch| ch == '─'));

    let tiny = rows(&board, 40, 40);
    assert_eq!(tiny[39].trim(), "↑ ↓  select     ?  all keys", "{tiny:#?}");

    let mut work = work_board();
    work.view = BoardView::Work;
    let keys = rows(&work, 78, 40)[39].clone();
    assert!(
        keys.trim()
            .starts_with("← ↑ ↓ →  select     Double-click  open")
            && keys.ends_with("?  all keys"),
        "{keys:?}"
    );
}

#[test]
fn question_mark_or_a_click_on_it_shows_every_key_and_again_hides_them() {
    let mut state = board_state();
    state.task_board.open = true;
    state.compose(96, 40).expect("frame");
    let rows = frame_rows(&state.compose(96, 40).expect("frame"));
    assert!(rows[39].contains("?  all keys"), "{:?}", rows[39]);

    press_key(&mut state, KeyCode::Char('?'));
    assert!(state.task_board.keys_shown);
    let rows = frame_rows(&state.compose(96, 40).expect("frame"));
    let foot = rows[34..].join("\n");
    for part in [
        "d  done",
        "x  delete",
        "m  move",
        "J/K  reorder",
        "n  new task",
        "↑ ↓  select",
        "Enter  go to space",
        "o  open file",
        "Tab  Phases",
        "click a space  only its tasks",
        "Esc  close",
        "?  fewer keys",
    ] {
        assert!(foot.contains(part), "{part:?} in {foot}");
    }

    let hint = footer_hint(&state, "?").expect("? is clickable");
    board_click(&mut state, hint);
    assert!(!state.task_board.keys_shown, "a click hides them again");
}

/// The Work fixture with its REVIEW and VERIFY cards moved to PLAN, so two stages are empty.
fn two_empty_stages() -> TaskBoard {
    let mut board = work_board();
    board.store.receive_json(
        &WORK_V1
            .replace(r#""stage": "review""#, r#""stage": "plan""#)
            .replace(r#""stage": "check""#, r#""stage": "plan""#),
        std::time::Instant::now(),
    );
    board
}

/// The Work columns a render drew, left to right: each one's first column and width.
fn drawn_columns(render: &BoardRender) -> Vec<(u16, u16)> {
    render
        .hits
        .work_column_areas
        .iter()
        .filter(|area| !area.is_empty())
        .map(|area| (area.x, area.width))
        .collect()
}

#[test]
fn work_columns_stay_five_and_equal_down_to_14_wide_then_pick_one_stage() {
    // Five columns at every width where each gets 14: the columns left over go to the first
    // ones, one each. Empty stages keep their columns.
    for (board, width) in [
        (work_board(), 186),
        (two_empty_stages(), 186),
        (two_empty_stages(), 100),
        (work_board(), 80),
    ] {
        let render = render_board(&board, width, 40);
        let columns = drawn_columns(&render);
        assert_eq!(columns.len(), 5, "{width}: {columns:?}");
        let widths: Vec<u16> = columns.iter().map(|(_, width)| *width).collect();
        assert!(
            widths
                .windows(2)
                .all(|pair| pair[0] == pair[1] || pair[0] == pair[1] + 1),
            "{width}: equal, the wider first: {widths:?}"
        );
        assert!(
            widths.iter().all(|column| *column >= 14),
            "{width}: {widths:?}"
        );
        assert_eq!(
            widths.iter().sum::<u16>() + 4 * 2,
            width - 2,
            "{width}: the whole width inside the margins"
        );
    }
    // 186: 184 inside the margins, four gaps of 2, 176 over five: 36 then four of 35.
    let render = render_board(&two_empty_stages(), 186, 40);
    assert_eq!(
        drawn_columns(&render),
        [(1, 36), (39, 35), (76, 35), (113, 35), (150, 35)]
    );

    // An empty stage draws its label and `no items`, dim, on its first card line.
    for width in [186, 100] {
        let render = render_board(&two_empty_stages(), width, 40);
        let labels = &render.rows[3];
        for heading in ["PLAN  4", "BUILD  2", "REVIEW  0", "VERIFY  0", "SHIP  10"] {
            assert!(
                labels.contains(heading),
                "{width}: {heading:?} in {labels:?}"
            );
        }
        let review = drawn_columns(&render)[2];
        let verify = drawn_columns(&render)[3];
        // Row 4 is the stage's underline (B5b), so the first card line is row 6.
        for (x, _) in [review, verify] {
            let line = &render.rows[6];
            let text: String = (x + 2..x + 10)
                .map(|column| render.buffer[(column, 6)].symbol().to_owned())
                .collect();
            assert_eq!(text, "no items", "{width}: {line:?}");
            assert_eq!(render.buffer[(x + 2, 6)].fg, render.palette.overlay1);
        }
        assert!(
            !render.rows.iter().any(|row| row.starts_with(" REVIEW 0")),
            "{width}: no folded strip: {:#?}",
            &render.rows[3..7]
        );
    }

    // Under 14 a column: one column with a stage picker, in the new words.
    // (B5b: 61 rows, so PLAN's four nine-row cards all fit and no `▼ 1` follows the picker.)
    for (board, width) in [(work_board(), 79), (two_empty_stages(), 60)] {
        let render = render_board(&board, width, 61);
        assert!(
            render.rows[3].contains("◂  PLAN  ") && render.rows[3].trim_end().ends_with('▸'),
            "{width}: {:?}",
            render.rows[3]
        );
        assert_eq!(
            drawn_columns(&render),
            [(1, width - 2)],
            "{width}: the whole width"
        );
    }
}

#[test]
fn the_stage_picker_follows_the_selection_and_its_arrows_step_stages() {
    let mut state = work_state();
    state.task_board.view = BoardView::Work;
    // The main area at 70 columns is under 5 x 14.
    state.compose(70, 40).expect("frame");
    let rows = frame_rows(&state.compose(70, 40).expect("frame"));
    assert!(
        rows.iter().any(|row| row.contains("◂  PLAN  2  ▸")),
        "{rows:#?}"
    );

    press_key(&mut state, KeyCode::Right);
    let rows = frame_rows(&state.compose(70, 40).expect("frame"));
    assert!(
        rows.iter().any(|row| row.contains("◂  PLAN  2  ▸")),
        "the first card first"
    );
    press_key(&mut state, KeyCode::Right);
    let rows = frame_rows(&state.compose(70, 40).expect("frame"));
    assert!(
        rows.iter().any(|row| row.contains("◂  BUILD  2  ▸")),
        "{rows:#?}"
    );
    assert_eq!(
        state.task_board.work_selected,
        Some(card("2026-09-25-task-board.md"))
    );

    let arrow = |state: &ClientShellState, right: bool| {
        state
            .hits
            .task_board
            .work_stage_arrows
            .iter()
            .find(|(_, is_right)| *is_right == right)
            .map(|(rect, _)| *rect)
            .expect("drawn")
    };
    let next = arrow(&state, true);
    board_click(&mut state, next);
    let rows = frame_rows(&state.compose(70, 40).expect("frame"));
    assert!(
        rows.iter().any(|row| row.contains("◂  REVIEW  1  ▸")),
        "{rows:#?}"
    );
    let previous = arrow(&state, false);
    board_click(&mut state, previous);
    let rows = frame_rows(&state.compose(70, 40).expect("frame"));
    assert!(
        rows.iter().any(|row| row.contains("◂  BUILD  2  ▸")),
        "{rows:#?}"
    );
}

/// The Work panel's drawn lines, from its first row to its last line.
fn panel_rows(render: &BoardRender) -> Vec<String> {
    let panel = render.hits.work_panel;
    let mut rows = render.rows[usize::from(panel.y)..usize::from(panel.bottom())].to_vec();
    while rows.last().is_some_and(|row| row.trim().is_empty()) {
        rows.pop();
    }
    rows
}

#[test]
fn a_short_window_shrinks_the_work_panel_before_the_cards() {
    let mut board = work_board();
    board.work_selected = Some(card("2026-09-24-tree-sidebar.md"));
    let tall = panel_rows(&render_board(&board, WIDE, 50));
    assert!(
        tall.iter().any(|row| row.contains("#2  push the fork")),
        "{tall:#?}"
    );
    assert!(!tall.iter().any(|row| row.contains(" more")), "{tall:#?}");

    // B5b: 24 rows, two more than with a seven-row card, and one for the chip row.
    let short = render_board(&board, WIDE, 24);
    let shipped = short.hits.work_column_areas[4];
    assert!(
        shipped.height >= 9,
        "the columns keep a whole card: {shipped:?} {:#?}",
        short.rows
    );
    let panel = panel_rows(&short);
    assert!(
        panel
            .iter()
            .any(|row| row.contains("▼ 6 more steps and 2 tasks · scroll")),
        "its buttons and the count: {:#?}",
        short.rows
    );
}

#[test]
fn a_short_panel_keeps_a_body_line_before_the_blank_line_over_its_keys() {
    let mut board = phases_board();
    board.work_selected = Some(phases_card("review-open"));
    let mut heights = Vec::new();
    for window in 20..40 {
        let render = render_board(&board, WIDE, window);
        let panel = render.hits.work_panel;
        // Without its foot: two borders, the heading, its rule and the keys around the body.
        if panel.height < 6 || panel.height >= 10 {
            continue;
        }
        heights.push(panel.height);
        let row = |y: u16| -> String {
            (panel.x + 2..panel.right() - 2)
                .map(|x| render.buffer[(x, y)].symbol().to_owned())
                .collect::<String>()
                .trim_end()
                .to_owned()
        };
        let (top, keys) = (panel.y + 3, panel.bottom() - 2);
        assert!(
            !row(top).is_empty(),
            "{window} rows, a {}-row panel: {:#?}",
            panel.height,
            render.rows
        );
        // Room for more than one line: the blank line parts the body from the keys.
        if keys - top > 1 {
            assert_eq!(row(keys - 1), "", "{window} rows: {:#?}", render.rows);
        }
    }
    assert!(heights.contains(&6), "{heights:?}");
    assert!(heights.contains(&7), "{heights:?}");
}

#[test]
fn task_board_header_leaves_out_the_read_time_before_it_shortens_labels() {
    // Every full label and the filter box fit; the update time does not.
    // The chips are on their own row now, so the time goes at a much narrower board.
    let render = render_board(&fixture_board(), 98, 40);
    let header = &render.rows[0];
    assert!(header.contains("Space: All"), "{header:?}");
    for part in [
        "! Action required 5",
        "◐ In progress 0",
        "… Waiting on others 2",
    ] {
        assert!(
            render.rows[2].contains(part),
            "{part:?} in {:?}",
            render.rows[2]
        );
    }
    assert!(!header.contains("Updated "), "{header:?}");
    // The Phases view has no kind chips: its row 2 holds only the Filter menu.
    let mut board = work_board();
    board.view = BoardView::Work;
    let render = render_board(&board, 82, 40);
    let header = &render.rows[0];
    assert!(header.contains("Space: All"), "{header:?}");
    assert!(!header.contains("Updated "), "{header:?}");
    for part in ["◆", "✱", "▣", "Features", "Bugs", "Chores"] {
        assert!(!header.contains(part), "{part:?} in {header:?}");
    }
    assert_eq!(render.rows[2].trim(), "Filter ▾");
}

#[test]
fn every_key_shown_in_a_small_board_still_ends_with_the_way_back() {
    let mut board = fixture_board();
    board.keys_shown = true;
    let render = render_board(&board, 40, 10);
    assert!(
        render.rows.iter().any(|row| row.contains("?  fewer keys")),
        "{:#?}",
        render.rows
    );
    for joined in ["task↑", "close?"] {
        assert!(
            !render.rows.iter().any(|row| row.contains(joined)),
            "keys keep their gap: {:#?}",
            render.rows
        );
    }
}

// Nadine's first look (2026-09-26): a click never stops the board scrolling. The Tasks
// view's wheel scrolls the list; in the Work view the wheel scrolls the column under it, the
// panel under the panel, and the selected card's column anywhere else.

/// The Work view with the tree sidebar card selected and its panel far too long to show:
/// thirty long steps.
fn long_panel_state() -> ClientShellState {
    let mut list = parse_task_list(WORK_V1).expect("work fixture");
    let file = card("2026-09-24-tree-sidebar.md");
    let tree = list
        .work
        .iter_mut()
        .find(|card| card.file == file)
        .expect("tree card");
    tree.steps = (1..=30)
        .map(|step| crate::client::shell::task_board::model::WorkStep {
            full: String::new(),
            text: format!("step {step:02} {}", "x".repeat(60)),
            done: false,
            ..Default::default()
        })
        .collect();
    let mut state = work_state();
    state
        .task_board
        .store
        .receive(Ok(list), std::time::Instant::now());
    state.task_board.view = BoardView::Work;
    state.compose(160, 40).expect("frame");
    let at = state
        .hits
        .task_board
        .cards
        .iter()
        .find(|(_, drawn)| *drawn == file)
        .map(|(rect, _)| *rect)
        .expect("tree card drawn");
    board_click(&mut state, at);
    state
}

#[test]
fn a_clicked_task_row_still_scrolls_by_wheel_and_page_keys() {
    let mut state = board_state();
    state.task_board.open = true;
    state.compose(106, 16).expect("frame");
    let row = row_of(&state, &key(2));
    board_click(&mut state, row);
    state.compose(106, 16).expect("frame");
    let area = state.hits.task_board.area;
    wheel(&mut state, area.x + 10, area.y + 5, true);
    state.compose(106, 16).expect("frame");
    assert!(
        state.task_board.scroll > 0,
        "the wheel scrolls after a click"
    );
    state.task_board.scroll = 0;
    press_key(&mut state, KeyCode::PageDown);
    state.compose(106, 16).expect("frame");
    assert!(
        state.task_board.scroll > 0,
        "page down scrolls after a click"
    );
}

#[test]
fn work_wheel_over_the_panel_scrolls_it_and_a_new_card_starts_it_at_the_top() {
    let mut state = long_panel_state();
    let rows = frame_rows(&state.compose(160, 40).expect("frame"));
    let step = rows
        .iter()
        .position(|row| row.contains("step 01"))
        .unwrap_or_else(|| panic!("the panel starts at its top: {rows:#?}"));
    assert!(rows.iter().any(|row| row.contains(" more")), "{rows:#?}");

    // Two turns: the card's fields and the steps' heading are above the first step.
    wheel(&mut state, 60, step as u16, true);
    state.compose(160, 40).expect("frame");
    wheel(&mut state, 60, step as u16, true);
    let rows = frame_rows(&state.compose(160, 40).expect("frame"));
    assert!(
        !rows.iter().any(|row| row.contains("step 01")),
        "the wheel over the panel scrolls it: {rows:#?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("▲ 2 above")),
        "the panel says what is above: {rows:#?}"
    );
    wheel(&mut state, 60, step as u16, false);
    state.compose(160, 40).expect("frame");
    wheel(&mut state, 60, step as u16, false);
    let rows = frame_rows(&state.compose(160, 40).expect("frame"));
    assert!(rows.iter().any(|row| row.contains("step 01")), "{rows:#?}");

    // Scrolled, then another card and back: each card's panel starts at its top.
    wheel(&mut state, 60, step as u16, true);
    state.compose(160, 40).expect("frame");
    press_key(&mut state, KeyCode::Right);
    press_key(&mut state, KeyCode::Left);
    let rows = frame_rows(&state.compose(160, 40).expect("frame"));
    assert!(rows.iter().any(|row| row.contains("step 01")), "{rows:#?}");
}

#[test]
fn work_wheel_over_the_header_or_a_gap_scrolls_the_selected_cards_column() {
    // B5b: 50 rows, so the column still holds two nine-row cards under the panel.
    let mut state = work_selected_state("shipped-1.md", 160, 50);
    let shipped = state.hits.task_board.work_column_areas[4];
    let header = state.hits.task_board.area.y;
    wheel(&mut state, shipped.x + 2, header, true);
    state.compose(160, 50).expect("frame");
    assert_eq!(state.task_board.work_scroll[4], 1, "over the header");
    // The cell between two columns.
    wheel(&mut state, shipped.x - 1, shipped.y + 1, true);
    state.compose(160, 50).expect("frame");
    assert_eq!(
        state.task_board.work_scroll[4], 2,
        "over the gap between columns"
    );
}

#[test]
fn page_keys_page_the_selected_cards_column_in_the_work_view() {
    // B5b: 50 rows, so the column still holds two nine-row cards under the panel.
    let mut state = work_selected_state("shipped-1.md", 160, 50);
    let shipped = state.hits.task_board.work_column_areas[4];
    let page = state
        .hits
        .task_board
        .cards
        .iter()
        .filter(|(rect, _)| shipped.contains(rect.as_position()))
        .count();
    assert!(page > 1, "{page}");
    press_key(&mut state, KeyCode::PageDown);
    state.compose(160, 50).expect("frame");
    assert_eq!(state.task_board.work_scroll[4], page, "one page down");
    press_key(&mut state, KeyCode::PageUp);
    state.compose(160, 50).expect("frame");
    assert_eq!(state.task_board.work_scroll[4], 0, "one page up");
}

#[test]
fn a_stacked_needs_you_row_shows_its_age_alone_when_the_state_only_says_needs_you() {
    let render = render_board(&fixture_board(), 50, 60);
    let task = render.row_with("Ask eng for an OIDC");
    let age = render.rows[task + 1].trim_end();
    assert!(
        !age.ends_with(" · -") && !age.ends_with('·'),
        "the section says it: {age:?}"
    );
    assert_eq!(age.trim_matches([' ', '│']), "3d", "{age:?}");
}

// Cards look like cards at rest (Nadine, 2026-09-26): every card has a dim side in the
// grid's colour; the accent border is the only focus mark.

/// The colour of the cell a drawn card starts at, its symbol, and whether it is selected.
fn card_corners(
    render: &BoardRender,
    selected: &str,
) -> Vec<(String, ratatui::style::Color, bool, u16)> {
    render
        .hits
        .cards
        .iter()
        .map(|(rect, file)| {
            let cell = &render.buffer[(rect.x, rect.y)];
            (
                cell.symbol().to_owned(),
                cell.fg,
                *file == card(selected),
                rect.height,
            )
        })
        .collect()
}

#[test]
fn every_full_card_has_a_dim_box_at_rest_and_the_selected_one_an_accent_box() {
    let mut board = work_board();
    board.work_selected = Some(card("2026-09-24-tree-sidebar.md"));
    let render = render_board(&board, WIDE, 40);
    let full: Vec<_> = card_corners(&render, "2026-09-24-tree-sidebar.md")
        .into_iter()
        .filter(|(_, _, _, height)| *height > 1)
        .collect();
    assert!(full.len() > 2, "{:#?}", render.rows);
    for (symbol, fg, selected, _) in full {
        let want = if selected {
            render.palette.accent
        } else {
            render.palette.overlay0
        };
        assert_eq!(
            (symbol.as_str(), fg),
            ("╭", want),
            "selected {selected}: {:#?}",
            render.rows
        );
    }
}

#[test]
fn closing_the_file_popup_brings_the_board_back_on_the_same_row_and_scroll() {
    let mut state = board_state();
    state.task_board.open = true;
    state.task_board.selected = Some(key(1));
    state.compose(106, 16).expect("frame");
    let area = state.hits.task_board.area;
    wheel(&mut state, area.x + 10, area.y + 5, true);
    state.compose(106, 16).expect("frame");
    let scroll = state.task_board.scroll;
    assert!(scroll > 0);
    let sent = open_requests(&press_key(&mut state, KeyCode::Enter));
    assert_eq!(sent.len(), 1);
    state.handle_endpoint_result(
        "boot-1",
        &sent[0].0,
        Ok(crate::api::schema::ResponseResult::Ok {}),
    );
    assert!(!state.task_board.open, "the popup shows over the panes");

    let revision = |mut surface: PaneSurfaceFrame, revision: u64| {
        surface.surface_revision = revision;
        surface
    };
    state.set_pane_surface(revision(surface_with_popup(), 2));
    assert!(
        !state.task_board.open,
        "the board waits while the file shows"
    );
    state.set_pane_surface(revision(surface(), 3));
    assert!(
        state.task_board.open,
        "the board is back once the popup closes"
    );
    state.compose(106, 16).expect("frame");
    assert_eq!(state.task_board.selected, Some(key(1)));
    assert_eq!(state.task_board.scroll, scroll, "at the same scroll");

    // The popup may show before the answer comes.
    let sent = open_requests(&press_key(&mut state, KeyCode::Enter));
    state.set_pane_surface(revision(surface_with_popup(), 4));
    state.handle_endpoint_result(
        "boot-1",
        &sent[0].0,
        Ok(crate::api::schema::ResponseResult::Ok {}),
    );
    assert!(!state.task_board.open);
    state.set_pane_surface(revision(surface(), 5));
    assert!(state.task_board.open, "back after a popup that came first");

    // A popup the board did not open leaves a closed board closed.
    press_key(&mut state, KeyCode::Esc);
    state.set_pane_surface(revision(surface_with_popup(), 6));
    state.set_pane_surface(revision(surface(), 7));
    assert!(!state.task_board.open);
}

// Round G2 (2026-09-26): the panel takes the rows the tallest column does not need.

#[test]
fn a_click_or_wheel_on_the_grown_panels_first_row_reaches_the_panel_not_a_column() {
    // Short columns (no SHIP cards) and a crowded one (all of them): the panel grows in
    // both, and no column reaches into it.
    for keep_shipped in [false, true] {
        let mut state = long_panel_state();
        if !keep_shipped {
            let mut list = state.task_board.store.list().cloned().expect("list");
            list.work.retain(|card| card.stage != WorkStage::Shipped);
            state
                .task_board
                .store
                .receive(Ok(list), std::time::Instant::now());
        }
        state.compose(160, 40).expect("frame");
        let panel = state.hits.task_board.work_panel;
        assert!(
            panel.height > 7,
            "shipped {keep_shipped}: the panel grew past its old 7 lines: {panel:?}"
        );
        for area in &state.hits.task_board.work_column_areas {
            assert!(
                area.is_empty() || area.bottom() < panel.y,
                "shipped {keep_shipped}: column {area:?} reaches the panel {panel:?}"
            );
        }
        let first = Rect::new(panel.x + 4, panel.y, 1, 1);
        let selected = state.task_board.work_selected.clone();
        board_click(&mut state, first);
        assert_eq!(
            state.task_board.work_selected, selected,
            "shipped {keep_shipped}: a click on the panel selects no card"
        );
        let columns = state.task_board.work_scroll;
        wheel(&mut state, first.x, first.y, true);
        assert_eq!(
            state.task_board.work_panel_scroll, 3,
            "shipped {keep_shipped}: the wheel scrolls the panel"
        );
        assert_eq!(
            state.task_board.work_scroll, columns,
            "shipped {keep_shipped}"
        );
    }
}

// Design pass H (Nadine, 2026-09-26 6:2x pm): one spacing scale, one footer rule, hanging
// lists, a text filter, one card layout, and one margin, gap and padding everywhere.

/// Whether `row` is blank: nothing drawn on it.
fn blank(row: &str) -> bool {
    row.trim().is_empty()
}

/// Whether `row` is a rule: only `─` across it.
fn is_rule(row: &str) -> bool {
    !blank(row) && row.trim().chars().all(|ch| ch == '─')
}

#[test]
fn the_tasks_view_boxes_each_section_under_its_own_column_row_and_a_blank_between() {
    let render = render_board(&fixture_board(), WIDE, 60);
    assert!(blank(&render.rows[1]), "one blank row under the header");
    // Each heading sits on its own line above its box (Nadine, 2026-09-28), not in the border.
    assert!(
        render.rows[3].starts_with("  ACTION REQUIRED 5") && !render.rows[3].contains('─'),
        "{:?}",
        render.rows[3]
    );
    assert!(
        render.rows[4].starts_with(" ╭─")
            && render.rows[4].ends_with('╮')
            && !render.rows[4].contains("ACTION REQUIRED"),
        "a plain top border under the heading: {:?}",
        render.rows[4]
    );
    assert!(
        render.rows[5].contains("│ Task") && render.rows[5].contains("│ Action"),
        "the first section's column-name row: {:?}",
        render.rows[5]
    );
    assert_eq!(
        render.rows[5].find('│'),
        Some(1),
        "the box starts at the board's one-column margin: {:?}",
        render.rows[5]
    );
    for heading in ["QUEUED", "WAITING ON OTHERS"] {
        let at = render.row_with(heading);
        assert!(
            render.rows[at].starts_with(&format!("  {heading} "))
                && render.rows[at + 1].starts_with(" ╭─")
                && render.rows[at + 1].ends_with('╮'),
            "the heading above its box's top border: {:#?}",
            &render.rows[at..=at + 1]
        );
        assert!(
            render.rows[at + 2].starts_with(" │ #") && render.rows[at + 2].contains("│ Task"),
            "its own column-name row under the border: {:?}",
            render.rows[at + 2]
        );
        assert!(
            blank(&render.rows[at - 1]) && render.rows[at - 2].starts_with(" ╰"),
            "one blank row parts it from the box above: {:#?}",
            &render.rows[at - 2..=at]
        );
    }
    // DONE, last 7 days starts folded: its heading alone, no box.
    let done = render.row_with("last 7 days");
    assert!(
        render.rows[done].starts_with("  COMPLETED 2 ▶") && !render.rows[done].contains('─'),
        "{:?}",
        render.rows[done]
    );
    assert!(
        render.rows[done + 1..].iter().all(|row| !row.contains('╭')),
        "no box under it while it is folded: {:#?}",
        &render.rows[done..]
    );
    assert!(
        !render
            .rows
            .iter()
            .any(|row| row.contains("Box lines on the /clear screen")
                || row.contains("Blue terminal icon in Vesper")),
        "no rows show while it is folded: {:#?}",
        render.rows
    );

    // A stacked list has boxes without columns; one blank row parts its sections, never two.
    let stacked = render_board(&fixture_board(), 50, 60);
    let at = stacked.row_with("QUEUED");
    assert!(
        stacked.rows[at].starts_with("  QUEUED") && stacked.rows[at + 1].starts_with(" ╭─"),
        "{:#?}",
        &stacked.rows[at..=at + 1]
    );
    assert!(blank(&stacked.rows[at - 1]) && stacked.rows[at - 2].starts_with(" ╰"));
    assert!(
        !stacked.rows.iter().any(|row| row.contains("│ Task")),
        "no column names"
    );
}

#[test]
fn a_click_on_the_done_heading_opens_it_and_a_second_click_folds_it_again() {
    let mut state = board_state();
    state.task_board.open = true;
    // Tall enough that the opened section's tasks show without scrolling.
    let board_rows =
        |state: &mut ClientShellState| frame_rows(&state.compose(WIDE + 26, 48).expect("frame"));
    let rows = board_rows(&mut state);
    assert!(
        rows.iter()
            .any(|row| row.contains("COMPLETED") && row.contains("▶")),
        "starts folded: {rows:#?}"
    );
    assert!(
        !rows
            .iter()
            .any(|row| row.contains("Box lines on the /clear screen")),
        "its tasks are hidden while folded: {rows:#?}"
    );
    let heading = state.hits.task_board.done_heading;
    assert!(!heading.is_empty(), "the heading is a click target");

    board_click(&mut state, heading);
    let rows = board_rows(&mut state);
    assert!(rows
        .iter()
        .any(|row| row.contains("COMPLETED") && row.contains("▼")));
    assert!(rows
        .iter()
        .any(|row| row.contains("Box lines on the /clear screen")));

    let heading = state.hits.task_board.done_heading;
    board_click(&mut state, heading);
    let rows = board_rows(&mut state);
    assert!(rows
        .iter()
        .any(|row| row.contains("COMPLETED") && row.contains("▶")));
    assert!(!rows
        .iter()
        .any(|row| row.contains("Box lines on the /clear screen")));
}

#[test]
fn down_past_the_last_row_reaches_the_folded_done_heading_and_enter_opens_it() {
    let mut state = board_state();
    state.task_board.open = true;
    board_rows(&mut state);
    // The last visible row, with DONE folded, is the last (non-DONE) span.
    let last_span = state
        .hits
        .task_board
        .spans
        .last()
        .expect("some rows shown")
        .0
        .clone();
    state.task_board.selected = Some(last_span.clone());
    board_rows(&mut state);

    press_key(&mut state, KeyCode::Down);
    board_rows(&mut state);
    assert!(
        state.task_board.focused_heading == Some(Fold::Done),
        "reaches the heading"
    );
    assert_eq!(state.task_board.selected, None);

    // Up from the heading returns to the last row.
    press_key(&mut state, KeyCode::Up);
    board_rows(&mut state);
    assert!(state.task_board.focused_heading.is_none());
    assert_eq!(state.task_board.selected, Some(last_span));

    press_key(&mut state, KeyCode::Down);
    board_rows(&mut state);
    assert!(
        state.task_board.focused_heading == Some(Fold::Done),
        "reaches it again"
    );
    press_key(&mut state, KeyCode::Enter);
    assert!(state.task_board.done_expanded, "enter opens it");
    assert!(state.task_board.focused_heading.is_none());

    let rows = board_rows(&mut state);
    assert!(rows
        .iter()
        .any(|row| row.contains("COMPLETED") && row.contains("▼")));
}

#[test]
fn the_focused_done_heading_is_drawn_as_selected() {
    let mut board = fixture_board();
    let render = render_board(&board, WIDE, 40);
    let row = render.row_with("last 7 days");
    assert_ne!(
        render.cell(row, "COMPLETED").bg,
        render.palette.selection_bg,
        "not selected before it has focus"
    );

    board.focused_heading = Some(Fold::Done);
    let render = render_board(&board, WIDE, 40);
    let row = render.row_with("last 7 days");
    // The section's label and count keep their own chip colour on the band, as in every
    // state.
    assert_eq!(
        render.cell(row, "▶").bg,
        render.palette.selection_bg,
        "the arrow on the focused heading has the selection band: {:?}",
        render.rows[row]
    );
    assert_eq!(
        render.buffer[(WIDE - 2, row as u16)].bg,
        render.palette.selection_bg,
        "the band runs across the row"
    );
}

#[test]
fn the_focused_done_heading_scrolls_into_view() {
    let mut board = fixture_board();
    board.focused_heading = Some(Fold::Done);
    board.reveal_selected = true;
    let render = render_board(&board, WIDE, 12);
    assert!(
        render.rows.iter().any(|row| row.contains("last 7 days")),
        "the focused heading is scrolled into view: {:#?}",
        render.rows
    );
}

#[test]
fn a_row_click_after_the_done_heading_took_focus_makes_enter_go_to_that_rows_space() {
    let mut state = board_state();
    let mut snap = snapshot();
    snap.workspaces = vec![workspace(
        "ws_oakwood",
        "oakwood",
        "/home/user/devbox/Workdir/oakwood-workspace",
    )];
    state.set_snapshot(Box::new(snap));
    state.task_board.open = true;
    board_rows(&mut state);
    let last_span = state.hits.task_board.spans.last().expect("rows").0.clone();
    state.task_board.selected = Some(last_span);
    board_rows(&mut state);
    press_key(&mut state, KeyCode::Down);
    board_rows(&mut state);
    assert!(
        state.task_board.focused_heading == Some(Fold::Done),
        "reaches the heading"
    );

    let row = state
        .hits
        .task_board
        .rows
        .iter()
        .find(|(_, target)| *target == key(1))
        .map(|(rect, _)| *rect)
        .expect("task 1 drawn");
    board_click(&mut state, row);
    assert!(
        state.task_board.focused_heading.is_none(),
        "a row click takes the focus"
    );
    board_rows(&mut state);
    let outcome = press_key(&mut state, KeyCode::Enter);
    assert!(
        !state.task_board.done_expanded,
        "Enter does not open the fold"
    );
    assert_eq!(workspace_focus_requests(&outcome), ["ws_oakwood"]);
}

#[test]
fn the_done_heading_focus_does_not_outlive_a_close_filter_space_or_view_change() {
    fn focused_state() -> ClientShellState {
        let mut state = board_state();
        state.task_board.open = true;
        board_rows(&mut state);
        state.task_board.focused_heading = Some(Fold::Done);
        state
    }

    let mut state = focused_state();
    state.toggle_task_board(BoardOpener::Key);
    assert!(
        state.task_board.focused_heading.is_none(),
        "closing the board"
    );

    let mut state = focused_state();
    press_key(&mut state, KeyCode::Char('/'));
    press_key(&mut state, KeyCode::Char('z'));
    assert!(
        state.task_board.focused_heading.is_none(),
        "a filter change"
    );

    let mut state = focused_state();
    press_key(&mut state, KeyCode::Tab);
    assert!(state.task_board.focused_heading.is_none(), "a view change");

    let mut state = focused_state();
    let button = state.hits.task_board.space_button;
    board_click(&mut state, button);
    board_rows(&mut state);
    let item = state.hits.task_board.space_items[1].0;
    board_click(&mut state, item);
    assert!(state.task_board.space.is_some(), "a space was picked");
    assert!(state.task_board.focused_heading.is_none(), "a space change");

    let mut state = focused_state();
    let category = state.hits.task_board.categories[0].0;
    board_click(&mut state, category);
    assert!(
        state.task_board.focused_heading.is_none(),
        "a category change"
    );
}

#[test]
fn the_done_heading_focus_ends_when_new_data_removes_the_heading() {
    let mut state = board_state();
    state.task_board.open = true;
    board_rows(&mut state);
    let last_span = state.hits.task_board.spans.last().expect("rows").0.clone();
    state.task_board.selected = Some(last_span);
    board_rows(&mut state);
    press_key(&mut state, KeyCode::Down);
    board_rows(&mut state);
    assert!(
        state.task_board.focused_heading == Some(Fold::Done),
        "reaches the heading"
    );

    // The next read has nothing done today: the heading is gone.
    let mut list: serde_json::Value = serde_json::from_str(LIST_V1).expect("fixture json");
    list["tasks"]
        .as_array_mut()
        .expect("tasks")
        .retain(|task| task["group"] != "done");
    let asked = tick_asks(&mut state, std::time::Instant::now());
    state.handle_endpoint_result(
        "boot-1",
        &asked[0],
        Ok(crate::api::schema::ResponseResult::TasksList { list }),
    );
    let rows = board_rows(&mut state);
    assert!(
        !rows.iter().any(|row| row.contains("last 7 days")),
        "{rows:#?}"
    );

    press_key(&mut state, KeyCode::Enter);
    assert!(
        state.task_board.focused_heading.is_none(),
        "the focus went with its heading"
    );
    assert!(
        !state.task_board.done_expanded,
        "Enter does not open a fold that is not there"
    );
}

#[test]
fn the_done_heading_focus_ends_when_the_data_drops_it_even_if_it_comes_back_before_a_key() {
    let mut state = board_state();
    state.task_board.open = true;
    board_rows(&mut state);
    let last_span = state.hits.task_board.spans.last().expect("rows").0.clone();
    state.task_board.selected = Some(last_span);
    board_rows(&mut state);
    press_key(&mut state, KeyCode::Down);
    board_rows(&mut state);
    assert!(
        state.task_board.focused_heading == Some(Fold::Done),
        "reaches the heading"
    );

    // A read with nothing done today, then the full list again, with no key between.
    let mut list: serde_json::Value = serde_json::from_str(LIST_V1).expect("fixture json");
    list["tasks"]
        .as_array_mut()
        .expect("tasks")
        .retain(|task| task["group"] != "done");
    let asked = tick_asks(&mut state, std::time::Instant::now());
    state.handle_endpoint_result(
        "boot-1",
        &asked[0],
        Ok(crate::api::schema::ResponseResult::TasksList { list }),
    );
    assert!(
        state.task_board.focused_heading.is_none(),
        "the focus goes with the data that drops the heading"
    );
    let later = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let asked = tick_asks(&mut state, later);
    state.handle_endpoint_result("boot-1", &asked[0], fixture_reply());
    board_rows(&mut state);
    assert!(
        state.task_board.focused_heading.is_none(),
        "the heading comes back without its focus"
    );
}

#[test]
fn down_moves_into_the_done_rows_once_done_today_is_open() {
    let mut state = board_state();
    state.task_board.open = true;
    state.task_board.done_expanded = true;
    board_rows(&mut state);
    let last_span = state.hits.task_board.spans.last().expect("rows").0.clone();
    state.task_board.selected = Some(last_span.clone());
    board_rows(&mut state);

    press_key(&mut state, KeyCode::Down);
    assert!(
        state.task_board.focused_heading.is_none(),
        "the open heading sits above its rows, not after the last one"
    );
    assert_eq!(state.task_board.selected, Some(last_span));
}

#[test]
fn down_reaches_the_folded_done_heading_when_every_shown_task_is_done() {
    let mut state = board_state();
    state.task_board.open = true;
    state.task_board.filter = "Box lines".into();
    board_rows(&mut state);
    assert!(
        state.hits.task_board.spans.is_empty(),
        "only done tasks shown"
    );

    press_key(&mut state, KeyCode::Down);
    assert!(state.task_board.focused_heading == Some(Fold::Done));
    press_key(&mut state, KeyCode::Enter);
    assert!(state.task_board.done_expanded);
}

#[test]
fn work_cards_start_under_their_stage_and_stack_with_no_blank_rows_and_one_rule_on_screen() {
    let mut board = work_board();
    board.work_selected = Some(card("2026-09-24-tree-sidebar.md"));
    // B5b: 51 rows, so BUILD's two nine-row cards both fit above the panel.
    let render = render_board(&board, WIDE, 51);
    // The stage label and its underline (B5b), then the cards at once.
    assert!(
        render.rows[5].contains('╭'),
        "the first cards sit right under the label: {:#?}",
        &render.rows[..9]
    );
    // BUILD's two cards: the first one's bottom edge, then the second one's top.
    let build = render.hits.work_column_areas[1];
    let boxes: Vec<Rect> = render
        .hits
        .cards
        .iter()
        .map(|(rect, _)| *rect)
        .filter(|rect| build.contains(rect.as_position()))
        .collect();
    assert_eq!(boxes.len(), 2, "{:#?}", render.rows);
    assert_eq!(
        boxes[1].y,
        boxes[0].bottom(),
        "no blank row between stacked cards: {:#?}",
        render.rows
    );
    // The panel sits one blank row under the columns, and no rule but the footer's runs
    // across the board.
    let panel = render.hits.work_panel;
    let above = usize::from(panel.y);
    assert!(
        blank(&render.rows[above - 1]) && build.bottom() == panel.y - 1,
        "one blank row between the columns and the panel: {:#?}",
        render.rows
    );
    let rules: Vec<usize> = (0..render.rows.len())
        .filter(|y| is_rule(&render.rows[*y]))
        .collect();
    assert_eq!(rules.len(), 1, "only the footer's rule: {:#?}", render.rows);
    assert!(
        render.rows[panel.y as usize + 1].contains("◆ Feature    Tree sidebar"),
        "{:#?}",
        render.rows
    );
}

#[test]
fn footer_hints_keep_one_space_between_arrows_and_space_their_key_blocks_evenly() {
    // A key's block has a space each side, one space before its words, four before the
    // next block.
    let tasks = render_board(&fixture_board(), WIDE, 40);
    assert!(
        tasks.rows[39]
            .trim()
            .starts_with("↑ ↓  select     Enter  go to space     d  done     x  delete     "),
        "{:?}",
        tasks.rows[39]
    );
    let work = render_board(&work_board(), WIDE, 40);
    assert!(
        work.rows[39]
            .trim()
            .starts_with("← ↑ ↓ →  select     Double-click  open     /  search     "),
        "{:?}",
        work.rows[39]
    );
    // The clickable keys still land on their block.
    let tab = work
        .hits
        .footer_keys
        .iter()
        .find(|(rect, key)| *key == "Tab" && rect.y == 39)
        .map(|(rect, _)| *rect)
        .expect("Tab hint");
    let block: String = work.rows[39]
        .chars()
        .skip(usize::from(tab.x))
        .take(5)
        .collect();
    assert_eq!(block, " Tab ");
}

#[test]
fn a_row_question_spaces_its_keys_as_the_footer_does() {
    let mut state = acting_state(1);
    press_key(&mut state, KeyCode::Char('x'));
    let rows = board_rows(&mut state);
    assert!(
        rows.iter()
            .any(|row| row.contains("Delete this task?     y  delete     n  keep")),
        "{rows:#?}"
    );
    press_key(&mut state, KeyCode::Char('n'));
    press_key(&mut state, KeyCode::Char('m'));
    let rows = board_rows(&mut state);
    assert!(
        rows.iter()
            .any(|row| row.contains("Move to:     1  you     2  me     3  other…")),
        "{rows:#?}"
    );
}

/// The Work board with the tree sidebar card selected, its steps replaced by `steps`.
fn tree_board_with_steps(steps: Vec<(String, bool)>) -> TaskBoard {
    let mut list = parse_task_list(WORK_V1).expect("work fixture");
    let file = card("2026-09-24-tree-sidebar.md");
    let tree = list
        .work
        .iter_mut()
        .find(|card| card.file == file)
        .expect("tree card");
    tree.steps = steps
        .into_iter()
        .map(
            |(text, done)| crate::client::shell::task_board::model::WorkStep {
                text,
                done,
                ..Default::default()
            },
        )
        .collect();
    let mut board = work_board();
    board.store.receive(Ok(list), std::time::Instant::now());
    board.work_selected = Some(file);
    board
}

#[test]
fn the_work_panel_lists_one_step_or_task_per_line_and_wraps_the_selected_step_under_its_text() {
    let mut board = work_board();
    board.work_selected = Some(card("2026-09-24-tree-sidebar.md"));
    let render = render_board(&board, WIDE, 48);
    let panel = render.hits.work_panel;
    let lines: Vec<&str> = render.rows[panel.y as usize..panel.bottom() as usize]
        .iter()
        .map(|row| row.trim().trim_matches('│').trim())
        .collect();
    for line in ["✓ tree rows", "✓ double count fixed", "#1  say final (you)"] {
        assert!(lines.contains(&line), "{line:?} alone: {lines:#?}");
    }
    let tick = render.row_with("✓ tree rows");
    assert_eq!(render.cell(tick, "✓").fg, render.palette.teal);

    // The selected step, too long for a narrow panel's line, goes on under its text, not
    // under its mark.
    let long = format!("step 01 {}", "wrap ".repeat(10));
    let mut board = tree_board_with_steps(vec![(long, false)]);
    board.work_step = Some(0);
    let render = render_board(&board, NARROW, 41);
    let step = render.row_with("▸ step 01");
    assert!(
        render.rows[step].starts_with(" │ ▸ step 01"),
        "{:?}",
        render.rows[step]
    );
    assert!(
        render.rows[step + 1].starts_with(" │   wrap"),
        "hangs under the text: {:#?}",
        &render.rows[step..step + 3]
    );
}

#[test]
fn the_problems_line_wraps_under_its_text_instead_of_being_cut() {
    let mut board = fixture_board();
    let mut list = board.store.list().cloned().expect("list");
    list.problems = vec![format!(
        "/notes/{}.md: no Task column",
        "long-name-".repeat(6)
    )];
    board.store.receive(Ok(list), std::time::Instant::now());
    // Each problem with its path, as a click on the problem line shows it.
    board.problems_shown = true;
    let render = render_board(&board, NARROW, 40);
    let first = render.row_with("Problems: /notes/");
    let text = " Problems: ".len();
    let next = &render.rows[first + 1];
    let lead: String = next.chars().take(text).collect();
    assert!(
        blank(&lead)
            && next
                .chars()
                .nth(text)
                .is_some_and(|ch| ch != ' ' && ch != '─'),
        "the rest hangs under the text: {:#?}",
        &render.rows[first..first + 3]
    );
    assert!(
        shows_whole(&render, "no Task column"),
        "never cut: {:#?}",
        render.rows
    );
}

/// Presses each character of `text` as a key.
fn type_keys(state: &mut ClientShellState, text: &str) -> ClientShellInput {
    let mut outcome = ClientShellInput::default();
    for ch in text.chars() {
        let modifiers = if ch.is_uppercase() {
            KeyModifiers::SHIFT
        } else {
            KeyModifiers::NONE
        };
        let next = state.handle_raw_events(vec![RawInputEvent::Key(TerminalKey::new(
            KeyCode::Char(ch),
            modifiers,
        ))]);
        outcome.actions.extend(next.actions);
    }
    outcome
}

#[test]
fn slash_filters_the_tasks_by_text_and_the_header_counts_follow() {
    let mut state = board_state();
    state.task_board.open = true;
    let rows = board_rows(&mut state);
    assert!(rows[0].contains(" ⌕ Search"), "{:?}", rows[0]);

    press_key(&mut state, KeyCode::Char('/'));
    // Typed keys are the filter's, never the board's: `d` marks nothing done.
    let outcome = type_keys(&mut state, "Chrome");
    assert!(act_requests(&outcome).is_empty());
    let rows = board_rows(&mut state);
    assert!(rows[0].contains(" ⌕ Chrome▏"), "{:?}", rows[0]);
    assert!(rows.iter().any(|row| row.contains("Per-client Chrome")));
    assert!(
        !rows.iter().any(|row| row.contains("Ask eng for an OIDC")),
        "{rows:#?}"
    );
    assert!(
        rows[2].contains("! Action required 1 of 5"),
        "{:?}",
        rows[2]
    );

    press_key(&mut state, KeyCode::Enter);
    assert_eq!(state.task_board.filter, "Chrome");
    assert!(!state.task_board.filter_editing);
    let rows = board_rows(&mut state);
    assert!(
        rows[0].contains(" ⌕ Chrome ") && rows[2].contains("! Action required 1 of 5"),
        "{:?} {:?}",
        rows[0],
        rows[2]
    );
    // Keys act on the board again, over the rows the filter keeps.
    press_key(&mut state, KeyCode::Down);
    assert_eq!(state.task_board.selected, Some(key(1)));

    // Where and State match too, in any case.
    state.task_board.filter = "PAIRING broadcast".into();
    let rows = board_rows(&mut state);
    assert!(rows.iter().any(|row| row.contains("Per-client Chrome")));
    state.task_board.filter = "oakwood/oak-cli".into();
    let rows = board_rows(&mut state);
    assert!(rows.iter().any(|row| row.contains("Ask eng for an OIDC")));
    assert!(!rows.iter().any(|row| row.contains("Per-client Chrome")));

    // Esc clears the filter first; the next one closes the board.
    press_key(&mut state, KeyCode::Esc);
    assert!(state.task_board.filter.is_empty() && state.task_board.open);
    press_key(&mut state, KeyCode::Esc);
    assert!(!state.task_board.open);
}

#[test]
fn the_filter_narrows_work_cards_with_the_space_and_clears_by_its_close_or_esc() {
    let mut state = work_state();
    state.task_board.view = BoardView::Work;
    state.compose(160, 40).expect("frame");
    press_key(&mut state, KeyCode::Char('/'));
    type_keys(&mut state, "bug");
    press_key(&mut state, KeyCode::Enter);
    let rows = frame_rows(&state.compose(160, 40).expect("frame"));
    let drawn: Vec<String> = state
        .hits
        .task_board
        .cards
        .iter()
        .map(|(_, file)| file.clone())
        .collect();
    assert_eq!(
        drawn,
        [
            card("2026-09-24-grey-icon.md"),
            card("shipped-0.md"),
            card("shipped-5.md")
        ],
        "only bugs: {rows:#?}"
    );
    // The kind chips are gone: no kind, and no count, in the top bar; row 2 holds only the
    // Filter menu.
    for part in ["Features", "Bugs", "◆", "✱", "of 10"] {
        assert!(!rows[0].contains(part), "{part:?} in {:?}", rows[0]);
    }
    assert!(rows[2].trim_end().ends_with("Filter ▾"), "{:?}", rows[2]);
    for part in ["Features", "Bugs", "◆", "✱"] {
        assert!(!rows[2].contains(part), "{part:?} in {:?}", rows[2]);
    }

    // With a space as well, both narrow: the herdr bug leaves.
    state.task_board.space = Some("dotworks".into());
    state.compose(160, 40).expect("frame");
    let drawn: Vec<String> = state
        .hits
        .task_board
        .cards
        .iter()
        .map(|(_, file)| file.clone())
        .collect();
    assert_eq!(drawn, [card("shipped-0.md"), card("shipped-5.md")]);
    state.task_board.space = None;

    // The chip's `×` clears it; a click on its text types into it again.
    state.task_board.filter = "bug".into();
    state.compose(160, 40).expect("frame");
    let clear = state.hits.task_board.filter_clear;
    board_click(&mut state, clear);
    assert!(state.task_board.filter.is_empty());
    state.compose(160, 40).expect("frame");
    let chip = state.hits.task_board.filter;
    board_click(&mut state, chip);
    assert!(state.task_board.filter_editing);
    type_keys(&mut state, "zz");
    press_key(&mut state, KeyCode::Backspace);
    assert_eq!(state.task_board.filter, "z");
    press_key(&mut state, KeyCode::Esc);
    assert!(state.task_board.filter.is_empty() && !state.task_board.filter_editing);
    assert!(state.task_board.open, "esc in the input only clears it");
}

#[test]
fn every_full_card_shows_kind_and_project_title_progress_who_and_status_in_one_layout() {
    // herdr-w1 works: its card's who line starts with the spinner.
    let render = render_board_with_agents(
        &work_board(),
        160,
        40,
        &[working("w36:pV", "herdr", "herdr-w1", None)],
    );
    let inner = |rect: Rect, line: u16| -> String {
        let row = &render.rows[usize::from(rect.y + 1 + line)];
        row.chars()
            .skip(usize::from(rect.x) + 1)
            .take(usize::from(rect.width) - 2)
            .collect()
    };
    let full: Vec<(Rect, String)> = render
        .hits
        .cards
        .iter()
        .filter(|(rect, _)| rect.height > 1)
        .cloned()
        .collect();
    assert!(full.len() >= 5, "{:#?}", render.rows);
    let list = parse_task_list(WORK_V1).expect("work fixture");
    for (rect, file) in &full {
        let item = list
            .work
            .iter()
            .find(|item| item.file == *file)
            .expect("card");
        let first = inner(*rect, 0);
        // One space inside each side; the kind, then the project after a dot.
        assert!(
            first.starts_with(' ') && first.trim_end().ends_with(&format!(" · {}", item.project)),
            "{first:?}"
        );
        let lines = rect.height - 2;
        assert_eq!(lines, 7, "{file}");
        // Who has it on the sixth line, in grey: its agent, else herdr's lead (herdr-w1, the
        // one agent there, leads it), else its space; the phase's status on the seventh.
        let who_line = inner(*rect, 5);
        let who = match &item.agent {
            Some(agent) => format!("{WORKER_SPINNER} {}", agent.name),
            None if item.space == "herdr" => "herdr-w1".to_owned(),
            None => item.space.clone(),
        };
        let shown: String = who.chars().take(15).collect();
        assert!(
            who_line.trim().starts_with(&shown),
            "{file}: {who_line:?} wants {who:?}"
        );
        let who_y = rect.y + 6;
        let who_x = rect.x + 2;
        assert_eq!(
            render.buffer[(who_x + u16::from(item.agent.is_some()) * 2, who_y)].fg,
            render.palette.overlay1,
            "{file}: the who line is grey"
        );
        // The steps done over all of them end the progress line, before who; a card with no
        // steps leaves it blank.
        let progress = inner(*rect, 4);
        match item.steps.len() {
            0 => assert_eq!(progress.trim(), "", "{file}: no steps, no progress"),
            total => assert!(
                progress.trim().ends_with(&format!(
                    " {}/{total}",
                    item.steps.iter().filter(|step| step.done).count()
                )),
                "{file}: the progress before who: {progress:?}"
            ),
        }
        let status = inner(*rect, 6);
        // A card without the phase fields says its phase's first state.
        let want = match item.stage {
            WorkStage::Plan => "? Awaiting your approval",
            WorkStage::Build => "· On track",
            WorkStage::Review => "· Awaiting review",
            WorkStage::Check => "? Awaiting your check",
            _ => "✓ Shipped ",
        };
        assert!(status.trim().starts_with(want), "{file}: {status:?}");
    }
    // A title longer than its two lines is cut to them, ending in `…`.
    let (lead, _) = full
        .iter()
        .find(|(_, file)| *file == card("2026-09-25-lead-name.md"))
        .expect("lead name card");
    assert_eq!(lead.height, 2 + 7, "{:#?}", render.rows);
    assert!(
        inner(*lead, 3).trim_end().ends_with('…'),
        "{:#?}",
        render.rows
    );
}

#[test]
fn every_card_in_a_column_has_the_same_width() {
    let render = render_board(&work_board(), WIDE, 40);
    for area in render
        .hits
        .work_column_areas
        .iter()
        .filter(|area| !area.is_empty())
    {
        let cards: Vec<Rect> = render
            .hits
            .cards
            .iter()
            .map(|(rect, _)| *rect)
            .filter(|rect| area.contains(rect.as_position()))
            .collect();
        assert!(
            cards.iter().all(|rect| rect.width == area.width),
            "{cards:?}"
        );
    }
}

#[test]
fn work_columns_sit_two_apart_and_a_card_has_a_space_inside_its_side() {
    let render = render_board(&work_board(), WIDE, 40);
    let areas = &render.hits.work_column_areas;
    for pair in areas.windows(2) {
        assert_eq!(pair[1].x, pair[0].right() + 2, "{areas:?}");
    }
    let shipped = render.row_with("Blank /clear");
    assert!(
        render.rows[shipped].contains("│ Blank /clear"),
        "{:?}",
        render.rows[shipped]
    );
    assert!(
        render.rows[3].starts_with("  PLAN  2"),
        "the stage titles start at the margin: {:?}",
        render.rows[3]
    );
}

#[test]
fn the_board_comes_back_when_its_file_popup_closed_before_it_was_seen() {
    let mut state = board_state();
    state.task_board.open = true;
    state.task_board.selected = Some(key(1));
    state.compose(106, 16).expect("frame");
    let sent = open_requests(&press_key(&mut state, KeyCode::Enter));
    state.handle_endpoint_result(
        "boot-1",
        &sent[0].0,
        Ok(crate::api::schema::ResponseResult::Ok {}),
    );
    assert!(!state.task_board.open);
    // `less` came and went between two surface updates: no popup is ever seen.
    let now = std::time::Instant::now();
    tick_asks(&mut state, now + std::time::Duration::from_millis(500));
    assert!(!state.task_board.open, "still waiting for the popup");
    tick_asks(&mut state, now + std::time::Duration::from_secs(4));
    assert!(state.task_board.open, "the board is back");
    assert_eq!(state.task_board.selected, Some(key(1)));

    // A popup that shows late still closes the board, which comes back after it.
    let revision = |mut surface: PaneSurfaceFrame, revision: u64| {
        surface.surface_revision = revision;
        surface
    };
    state.set_pane_surface(revision(surface_with_popup(), 2));
    assert!(!state.task_board.open, "the late popup shows");
    state.set_pane_surface(revision(surface(), 3));
    assert!(state.task_board.open);
}

#[test]
fn a_popup_long_after_the_board_came_back_is_not_its_file() {
    let mut state = board_state();
    state.task_board.open = true;
    let long_ago = std::time::Instant::now()
        .checked_sub(std::time::Duration::from_secs(31))
        .expect("a clock 31 s old");
    state.task_board.file_popup = crate::client::shell::task_board::FilePopup::Lapsed(long_ago);
    let revision = |mut surface: PaneSurfaceFrame, revision: u64| {
        surface.surface_revision = revision;
        surface
    };
    state.set_pane_surface(revision(surface_with_popup(), 2));
    assert!(
        state.task_board.open,
        "another popup leaves the board as it is"
    );
    assert_eq!(
        state.task_board.file_popup,
        crate::client::shell::task_board::FilePopup::None,
        "and the board stops following"
    );
}

#[test]
fn a_long_notice_or_problems_line_stops_at_three_lines_with_an_ellipsis() {
    let mut board = fixture_board();
    let mut list = board.store.list().cloned().expect("list");
    list.problems = vec!["word ".repeat(80)];
    board.store.receive(Ok(list), std::time::Instant::now());
    // Each problem with its path, as a click on the problem line shows it.
    board.problems_shown = true;
    let render = render_board(&board, NARROW, 40);
    let first = render.row_with("Problems: word");
    let rule = (first..render.rows.len())
        .find(|y| is_rule(&render.rows[*y]))
        .expect("the footer rule");
    assert_eq!(rule - first, 3, "three lines: {:#?}", render.rows);
    assert!(
        render.rows[rule - 1].ends_with('…'),
        "{:?}",
        render.rows[rule - 1]
    );
}

#[test]
fn a_task_or_card_the_filter_hides_takes_no_action_key() {
    // Task 1 selected, then a filter that hides it: `d` asks for a selection instead.
    let mut state = acting_state(1);
    state.task_board.filter = "OIDC".into();
    board_rows(&mut state);
    let outcome = press_key(&mut state, KeyCode::Char('d'));
    assert!(
        act_requests(&outcome).is_empty(),
        "the hidden task is left alone"
    );
    assert!(
        board_rows(&mut state)
            .iter()
            .any(|row| row.contains("Select a task first")),
        "the notice says why"
    );

    // A card selected, then hidden: enter opens nothing and g goes nowhere.
    let mut state = work_selected_state("2026-09-25-task-board.md", 160, 40);
    state.task_board.filter = "Chrome".into();
    state.compose(160, 40).expect("frame");
    assert!(open_requests(&press_key(&mut state, KeyCode::Enter)).is_empty());
    assert_eq!(
        focused_pane(&press_key(&mut state, KeyCode::Char('g'))),
        None
    );
}

// Round six: the approved sample (2026-09-26).

/// The fill behind `needle` on row `y`, which a filled block (tab, button, key, label) has.
fn fill_of(render: &BoardRender, y: usize, needle: &str) -> ratatui::style::Color {
    render.cell(y, needle).bg
}

/// Whether a colour is a real fill, not the terminal's own background.
fn filled(color: ratatui::style::Color) -> bool {
    !matches!(color, ratatui::style::Color::Reset)
}

#[test]
fn the_filter_needs_every_word_in_any_order_across_task_where_and_action() {
    let mut board = fixture_board();
    // Words out of order, both in the task.
    board.filter = "push decide".into();
    let render = render_board(&board, WIDE, 40);
    assert!(shows_whole(&render, "Say final, then decide the push"));
    assert_eq!(render.hits.spans.len(), 1, "{:#?}", render.rows);

    // One word in Where, one in the task.
    board.filter = "PAIRING oakwood".into();
    let render = render_board(&board, WIDE, 40);
    assert_eq!(
        render
            .hits
            .spans
            .iter()
            .map(|(key, _, _)| key.clone())
            .collect::<Vec<_>>(),
        [key(1)]
    );

    // One in the action: `say commit`.
    board.filter = "commit say".into();
    let render = render_board(&board, WIDE, 40);
    assert_eq!(
        render
            .hits
            .spans
            .iter()
            .map(|(key, _, _)| key.clone())
            .collect::<Vec<_>>(),
        [key(5)]
    );

    // A word nowhere hides every row.
    board.filter = "push nowhere".into();
    let render = render_board(&board, WIDE, 40);
    assert!(render.hits.spans.is_empty());
}

#[test]
fn the_filter_never_matches_a_card_only_through_its_hidden_space() {
    let mut board = work_board();
    // The shipped cards' space is `dotworks`; nothing on them says so.
    board.filter = "dotworks".into();
    let render = render_board(&board, 160, 40);
    assert!(render.hits.cards.is_empty(), "{:#?}", render.rows);
    // Their project, which the card shows, does match, with a word of the title.
    board.filter = "clear dot-works".into();
    let render = render_board(&board, 160, 40);
    let drawn: Vec<String> = render
        .hits
        .cards
        .iter()
        .map(|(_, file)| file.clone())
        .collect();
    assert_eq!(
        drawn,
        [card("shipped-0.md"), card("shipped-1.md")],
        "{:#?}",
        render.rows
    );
    // The kind word and the who line match too, but no longer the branch the card left.
    board.filter = "bug fix/grey-icon".into();
    let render = render_board(&board, 160, 40);
    assert!(render.hits.cards.is_empty(), "{:#?}", render.rows);
    board.filter = "feature herdr-w1".into();
    let render = render_board(&board, 160, 40);
    let drawn: Vec<String> = render
        .hits
        .cards
        .iter()
        .map(|(_, file)| file.clone())
        .collect();
    assert_eq!(drawn, [card("2026-09-25-task-board.md")]);
}

#[test]
fn the_filter_marks_each_matched_word_in_rows_and_cards() {
    let mut board = fixture_board();
    board.filter = "push decide".into();
    let render = render_board(&board, WIDE, 40);
    let row = render.row_with("Say final, then decide the push");
    let mark = render.cell(row, "decide").bg;
    assert!(filled(mark), "a matched word has a fill");
    assert_eq!(
        render.cell(row, "push").bg,
        mark,
        "every matched word the same"
    );
    assert_eq!(render.cell(row, "push").fg, render.palette.text);
    assert_ne!(
        render.cell(row, "Say").bg,
        mark,
        "the rest of the task is not marked"
    );
    // Case never matters: `DECIDE` marks `decide`.
    board.filter = "DECIDE".into();
    let render = render_board(&board, WIDE, 40);
    assert_eq!(render.cell(row, "decide").bg, mark);

    let mut board = work_board();
    board.filter = "grey".into();
    let render = render_board(&board, 160, 40);
    let row = render.row_with("Grey icon");
    assert_eq!(
        render.cell(row, "Grey").bg,
        mark,
        "a card's title is marked the same way"
    );
}

#[test]
fn a_set_filter_counts_n_of_m_in_the_header_and_each_section_and_explains_itself_while_typing() {
    let mut board = fixture_board();
    let render = render_board(&board, WIDE, 40);
    let heading = render.row_with("ACTION REQUIRED");
    assert!(
        render.rows[heading].contains("ACTION REQUIRED 5"),
        "{:?}",
        render.rows[heading]
    );

    board.filter = "push decide".into();
    board.filter_editing = true;
    let render = render_board(&board, WIDE, 40);
    assert!(
        render.rows[2].contains("! Action required 1 of 5"),
        "{:?}",
        render.rows[2]
    );
    let heading = render.row_with("ACTION REQUIRED");
    assert!(
        render.rows[heading].contains("ACTION REQUIRED 1 of 5"),
        "{:?}",
        render.rows[heading]
    );
    let hint = &render.rows[1];
    assert!(
        hint.contains(" Enter  keep") && hint.contains(" Esc  clear"),
        "{hint:?}"
    );
    assert!(filled(fill_of(&render, 1, "Enter")));

    // Kept, the hint goes; the counts stay.
    board.filter_editing = false;
    let render = render_board(&board, WIDE, 40);
    assert!(blank(&render.rows[1]), "{:?}", render.rows[1]);
    assert!(
        render.rows[2].contains("! Action required 1 of 5"),
        "{:?}",
        render.rows[2]
    );

    // The Work view counts each stage the same way; it has no kind chips to count.
    let mut board = work_board();
    board.filter = "herdr".into();
    let render = render_board(&board, 160, 40);
    for part in ["◆ Features", "✱ Bugs", "▣ Chores"] {
        assert!(
            !render.rows[0].contains(part) && !render.rows[2].contains(part),
            "{part:?} in {:?} {:?}",
            render.rows[0],
            render.rows[2]
        );
    }
    assert!(
        render.rows[3].contains("BUILD  1 of 2"),
        "{:?}",
        render.rows[3]
    );
}

#[test]
fn the_search_box_sits_beside_the_space_button_and_says_search() {
    let render = render_board(&fixture_board(), HEADER_WIDE, 40);
    let header = &render.rows[0];
    assert!(
        header.starts_with("  Tasks   Phases  │  Space: All ▾   ⌕ Search "),
        "the space and the search, the two that narrow the list, together: {header:?}"
    );
    assert!(header.contains("Sort: Newest ▾ "), "{header:?}");
    assert_eq!(header.matches('│').count(), 1, "one rule: {header:?}");
    assert!(header.ends_with("Updated 8:05 am   ×"), "{header:?}");
    let button = render.hits.space_button;
    assert_eq!(render.hits.filter.x, button.right() + 1);
    assert!(render.hits.work_tab.right() < button.x);

    let mut board = work_board();
    board.view = BoardView::Work;
    let render = render_board(&board, HEADER_WIDE, 40);
    assert!(
        render.rows[0].contains(" ⌕ Search "),
        "{:?}",
        render.rows[0]
    );
    assert!(!render.rows[0].contains("Filter"), "{:?}", render.rows[0]);

    for view in [BoardView::Tasks, BoardView::Work] {
        let keys = footer_keys(view, true, true);
        assert!(keys.contains(&("/", "search")), "{view:?}: {keys:?}");
        // The search box is `/`'s; the only key called filter is the Phases view's Filter
        // menu, `f`, which replaced its kind buttons.
        let filters: Vec<_> = keys.iter().filter(|(_, what)| *what == "filter").collect();
        if view == BoardView::Work {
            assert_eq!(filters, [&("f", "filter")], "{view:?}: {keys:?}");
        } else {
            assert!(filters.is_empty(), "{view:?}: {keys:?}");
        }
    }
}

#[test]
fn the_header_has_tabs_a_space_button_counts_a_filter_box_and_a_close_button_on_no_band() {
    let render = render_board(&fixture_board(), WIDE, 40);
    let header = &render.rows[0];
    assert!(
        header.starts_with("  Tasks   Phases  │  Space: All ▾   ⌕ Search ")
            && header.contains("Sort: Newest ▾ ")
            && header.matches('│').count() == 1,
        "{header:?}"
    );
    assert!(
        render.rows[2].contains("! Action required 5"),
        "{:?}",
        render.rows[2]
    );
    assert!(header.contains(" ⌕ Search"), "{header:?}");
    assert!(header.ends_with(" ×"), "{header:?}");
    // No band: the margin and the gaps show the board's own ground.
    let ground = render.buffer[(0, 0)].bg;
    assert!(!filled(ground));
    assert!(!filled(render.buffer[(render.hits.work_tab.right(), 0)].bg));
    // The active tab takes the selection fill, the other none; the buttons are filled
    // blocks on the ground.
    assert_eq!(fill_of(&render, 0, " Tasks "), render.palette.selection_bg);
    assert_eq!(fill_of(&render, 0, " Phases "), ground);
    let button = fill_of(&render, 0, " Space: All ▾ ");
    assert!(button != ground && button != render.palette.selection_bg);
    assert_ne!(fill_of(&render, 0, " ⌕ Search"), ground);
    assert_eq!(render.hits.close, Rect::new(WIDE - 4, 0, 3, 1));
    assert_eq!(render.buffer[(WIDE - 3, 0)].symbol(), "×");
    assert_eq!(render.buffer[(WIDE - 3, 0)].bg, button);
    // One left edge: the tabs and the section boxes start at column 1.
    let heading = render.row_with("ACTION REQUIRED");
    assert_eq!(render.rows[heading + 1].find('╭'), Some(1));
    assert!(render.rows[heading + 4].starts_with(" │ 1"));

    let mut board = work_board();
    board.view = BoardView::Work;
    let render = render_board(&board, WIDE, 40);
    assert_eq!(fill_of(&render, 0, " Phases "), render.palette.selection_bg);
    assert_ne!(fill_of(&render, 0, " Tasks "), render.palette.selection_bg);
}

#[test]
fn the_header_groups_split_by_a_thin_rule_and_their_buttons_a_column_apart() {
    let render = render_board(&fixture_board(), WIDE, 40);
    let hits = &render.hits;
    let overlay0 = render.palette.overlay0;

    // Inside the tabs group and the category buttons: one column apart.
    assert_eq!(
        hits.work_tab.x,
        hits.tasks_tab.right() + 1,
        "Tasks and Phases sit one column apart"
    );
    assert_eq!(
        hits.categories.len(),
        4,
        "every category chip fits at {WIDE}"
    );
    for pair in hits.categories.windows(2) {
        assert_eq!(
            pair[1].0.x,
            pair[0].0.right() + 1,
            "category buttons sit one column apart"
        );
        // They sit on the chip row, not the top bar.
        assert_eq!(pair[0].0.y, 2);
        assert_eq!(render.buffer[(pair[0].0.right(), 2)].symbol(), " ");
    }

    // The space picker and the search box are one group, one column apart.
    assert_eq!(hits.filter.x, hits.space_button.right() + 1);

    // Between groups: a thin rule, one column each side.
    let tabs_sep = hits.work_tab.right() + 1;
    assert_eq!(
        hits.space_button.x,
        hits.work_tab.right() + 3,
        "a three-column separator zone between the tabs and the space button"
    );
    assert_eq!(render.buffer[(tabs_sep, 0)].symbol(), "│");
    assert_eq!(render.buffer[(tabs_sep, 0)].fg, overlay0);
    assert_eq!(
        hits.sort_button.x,
        hits.filter.right() + 1,
        "the sort button one column after the search box"
    );
    // The chips left the top bar, so no second rule parts the sort button from them.
    assert_eq!(
        render.rows[0].matches('│').count(),
        1,
        "one rule: {:?}",
        render.rows[0]
    );

    // The time: alone at the right, a button's gap before the close button. It shows at WIDE
    // and at a wider area.
    let wide_render = render_board(&fixture_board(), WIDE + 86, 40);
    assert!(
        wide_render.rows[0].ends_with("Updated 8:05 am   ×"),
        "the time shows at a wider width: {:?}",
        wide_render.rows[0]
    );

    // No separator hangs alone: narrow, the one rule stays between the tabs and the space
    // button, and nothing draws a second.
    let mut board = fixture_board();
    board.only_section = Some(TaskSection::ActionRequired);
    let narrow_render = render_board(&board, 74, 40);
    assert_eq!(
        narrow_render.rows[0].matches('│').count(),
        1,
        "no dangling separator: {:?}",
        narrow_render.rows[0]
    );
    assert_eq!(
        narrow_render.buffer[(narrow_render.hits.work_tab.right() + 1, 0)].symbol(),
        "│"
    );
}

#[test]
fn the_space_button_opens_a_menu_of_spaces_and_a_pick_narrows_the_board() {
    let mut state = board_state();
    state.task_board.open = true;
    board_rows(&mut state);
    let button = state.hits.task_board.space_button;
    board_click(&mut state, button);
    assert!(state.task_board.space_menu);
    let rows = board_rows(&mut state);
    let labels: Vec<Option<String>> = state
        .hits
        .task_board
        .space_items
        .iter()
        .map(|(_, space)| space.clone())
        .collect();
    assert_eq!(
        labels,
        [
            None,
            Some("oakwood".to_owned()),
            Some("herdr".to_owned()),
            Some("dotworks".to_owned())
        ],
        "All spaces, then each space in the list's order: {rows:#?}"
    );
    let (herdr, _) = state.hits.task_board.space_items[2].clone();
    assert!(rows[usize::from(herdr.y)].contains("herdr"), "{rows:#?}");
    board_click(&mut state, herdr);
    assert_eq!(state.task_board.space.as_deref(), Some("herdr"));
    assert!(!state.task_board.space_menu);
    let rows = board_rows(&mut state);
    assert!(rows[0].contains(" herdr ▾ "), "{:?}", rows[0]);

    // A click beside the open menu closes it and keeps the space.
    let button = state.hits.task_board.space_button;
    board_click(&mut state, button);
    board_rows(&mut state);
    let menu_bottom = state
        .hits
        .task_board
        .space_items
        .last()
        .map(|(rect, _)| rect.bottom())
        .expect("items");
    let beside = Rect::new(button.x, menu_bottom + 3, 1, 1);
    let outcome = board_click(&mut state, beside);
    assert!(!state.task_board.space_menu && outcome.actions.is_empty());
    assert_eq!(state.task_board.space.as_deref(), Some("herdr"));

    // Esc closes an open menu and nothing else.
    let button = state.hits.task_board.space_button;
    board_click(&mut state, button);
    board_rows(&mut state);
    let (all, _) = state.hits.task_board.space_items[0].clone();
    press_key(&mut state, KeyCode::Esc);
    assert!(!state.task_board.space_menu && state.task_board.open);
    // All spaces from the menu shows every space again.
    board_click(&mut state, button);
    board_rows(&mut state);
    board_click(&mut state, all);
    assert_eq!(state.task_board.space, None);
}

#[test]
fn keys_in_the_footer_detail_line_and_row_questions_are_filled_blocks() {
    let mut board = fixture_board();
    board.can_open = true;
    let render = render_board(&board, WIDE, 40);
    let keys = &render.rows[39];
    assert!(
        keys.contains(" ↑ ↓  select     Enter  go to space     o  open file     d  done"),
        "{keys:?}"
    );
    let block = fill_of(&render, 39, "Enter");
    assert!(filled(block));
    assert_eq!(fill_of(&render, 39, "↑"), block);
    assert!(
        !filled(render.cell(39, "select").bg),
        "the words are not in the block"
    );
    // The selected row's detail block: its keys are the same blocks.
    board.selected = Some(key(1));
    board.open_row = board.selected.clone();
    let render = render_board(&board, WIDE, 40);
    let row = render.row_with("Per-client Chrome");
    let detail = (row + 1..row + 8)
        .find(|at| render.rows[*at].contains("Enter"))
        .unwrap_or_else(|| panic!("the detail's keys: {:#?}", render.rows));
    assert!(
        render.rows[detail].contains(" Enter  go to space     o  open file"),
        "{:?}",
        render.rows[detail]
    );
    assert_eq!(fill_of(&render, detail, "Enter"), block);
    assert!(
        render.rows[detail].contains(" d  done"),
        "{:?}",
        render.rows[detail]
    );

    // A question on a row.
    let mut state = acting_state(1);
    press_key(&mut state, KeyCode::Char('x'));
    let area = Rect::new(0, 0, WIDE + 26, 40);
    let frame = state.compose(area.width, area.height).expect("frame");
    let rows = frame_rows(&frame);
    let at = rows
        .iter()
        .position(|row| row.contains("Delete this task?"))
        .expect("the question");
    assert!(
        rows[at].contains("Delete this task?     y  delete     n  keep"),
        "{:?}",
        rows[at]
    );
}

#[test]
fn sections_and_stages_are_coloured_labels_each_with_its_own_palette_role() {
    let render = render_board(&fixture_board(), WIDE, 80);
    let palette = &render.palette;
    for (title, role) in [
        ("ACTION REQUIRED", palette.yellow),
        ("QUEUED", palette.teal),
        ("WAITING ON OTHERS", palette.mauve),
        ("COMPLETED", palette.green),
    ] {
        let row = render.row_with(&format!("{title} "));
        let cell = render.cell(row, title);
        assert_eq!(cell.fg, role, "{title}");
        assert!(!filled(cell.bg), "{title} is bold text with no fill");
        assert!(cell.modifier.contains(Modifier::BOLD), "{title}");
        // Its count is a plain grey number one space after it.
        let x = render.rows[row].find(title).expect("title drawn") + title.len() + 1;
        let count = &render.buffer[(x as u16, row as u16)];
        assert_eq!(count.fg, render.palette.overlay1, "{title}'s count");
        assert!(!filled(count.bg), "{title}'s count has no fill");
    }
    let mut board = work_board();
    board.view = BoardView::Work;
    let render = render_board(&board, 160, 40);
    // Each stage in its own colour (B5b).
    for (stage, role) in [
        ("PLAN", palette.mauve),
        ("BUILD", palette.yellow),
        (
            "REVIEW",
            crate::client::shell::task_board::style::rose(palette),
        ),
        ("VERIFY", palette.blue),
        ("SHIP", palette.green),
    ] {
        let cell = render.cell(3, stage);
        assert_eq!(cell.fg, role, "{stage}");
        assert!(filled(cell.bg), "{stage} is a filled label");
    }
    assert!(render.rows[3].contains(" PLAN  2"), "{:?}", render.rows[3]);
}

#[test]
fn every_card_is_as_wide_as_its_column_and_nine_high_whatever_it_holds() {
    let mut board = work_board();
    board.work_selected = Some(card("2026-09-24-build-sha.md"));
    for width in [WIDE, 160] {
        let render = render_board(&board, width, 40);
        let columns = &render.hits.work_column_areas;
        assert!(render.hits.cards.len() >= 5, "{width}: {:#?}", render.rows);
        for (rect, _) in &render.hits.cards {
            let column = columns
                .iter()
                .find(|area| area.x == rect.x)
                .expect("in a column");
            assert_eq!(
                (rect.width, rect.height),
                (column.width, 9),
                "{width}: {rect:?}"
            );
        }
    }
    // A long title keeps two lines and the second ends in `…`; the panel has it whole.
    let render = render_board(&board, 160, 40);
    let lead = render
        .hits
        .cards
        .iter()
        .find(|(_, file)| *file == card("2026-09-25-lead-name.md"))
        .expect("drawn")
        .0;
    let line = |n: u16| {
        render.rows[usize::from(lead.y + n)]
            .chars()
            .skip(usize::from(lead.x))
            .take(usize::from(lead.width))
            .collect::<String>()
    };
    assert!(line(3).contains("Never cut the"), "{:?}", line(3));
    assert!(
        line(4).trim_end_matches(['│', ' ']).ends_with('…'),
        "{:?}",
        line(4)
    );
}

#[test]
fn a_selected_cards_panel_takes_half_the_body_and_ends_with_its_buttons() {
    let steps = (1..=24)
        .map(|n| (format!("step {n}"), n < 4))
        .collect::<Vec<_>>();
    let board = tree_board_with_steps(steps);
    let render = render_board(&board, 160, 40);
    let rule = render
        .rows
        .iter()
        .rposition(|row| is_rule(row))
        .expect("footer rule");
    // The body: from under the header's blank row to over the rule's.
    let body = rule - 1 - 2;
    let panel = render.hits.work_panel;
    assert_eq!(
        usize::from(panel.height),
        body / 2,
        "exactly half: {:#?}",
        render.rows
    );
    // Its last line inside the box holds its buttons, whatever it scrolled.
    let last = usize::from(panel.bottom()) - 2;
    assert!(
        render.rows[last].contains(" ↑↓  step     Enter  open file"),
        "{:?}",
        render.rows[last]
    );
    assert!(filled(render.cell(last, "Enter ").bg));
}

#[test]
fn a_click_on_a_panel_button_does_its_key() {
    let mut state = work_selected_state("2026-09-25-task-board.md", 160, 40);
    state.compose(160, 40).expect("frame");
    let button = footer_hint(&state, "Enter");
    let panel = state.hits.task_board.work_panel;
    let button = state
        .hits
        .task_board
        .footer_keys
        .iter()
        .find(|(rect, key)| *key == "Enter" && rect.y >= panel.y && rect.y < panel.bottom())
        .map(|(rect, _)| *rect)
        .unwrap_or_else(|| panic!("an Enter button in the panel: {button:?}"));
    let outcome = board_click(&mut state, button);
    assert_eq!(focused_pane(&outcome), Some("w36:pV"), "its lead's tab");

    // Without an agent the panel's `Enter` opens the card's file.
    let file = "2026-09-23-per-client.md";
    let mut state = work_selected_state(file, 160, 40);
    state.compose(160, 40).expect("frame");
    let panel = state.hits.task_board.work_panel;
    let button = state
        .hits
        .task_board
        .footer_keys
        .iter()
        .find(|(rect, key)| *key == "Enter" && rect.y >= panel.y && rect.y < panel.bottom())
        .map(|(rect, _)| *rect)
        .expect("an Enter button in the panel");
    let sent = open_requests(&board_click(&mut state, button));
    assert_eq!(
        sent.iter().map(|(_, sent)| sent).collect::<Vec<_>>(),
        [&card(file)]
    );
}

#[test]
fn every_section_box_has_a_row_naming_its_columns() {
    // Every open section's box names its own columns under its top border, then its rows.
    let render = render_board(&fixture_board(), WIDE, 80);
    let head = split_cells(&render.rows[5]);
    assert_eq!(
        head,
        ["#", "Project", "Task", "Action", "Age"],
        "{:?}",
        render.rows[5]
    );
    assert!(!render.cell(5, "Task").modifier.contains(Modifier::BOLD));
    assert_eq!(
        render
            .rows
            .iter()
            .filter(|row| row.contains("│ Task"))
            .count(),
        3,
        "once in each open box: {:#?}",
        render.rows
    );
    for heading in ["QUEUED", "WAITING ON OTHERS"] {
        let at = render.row_with(heading);
        assert_eq!(
            split_cells(&render.rows[at + 2])[0],
            "#",
            "{:?}",
            render.rows[at + 2]
        );
        assert!(
            split_cells(&render.rows[at + 4])[0].parse::<u32>().is_ok(),
            "{:?}",
            render.rows[at + 4]
        );
    }

    // A column whose tasks say little is still as wide as its name: Waiting over `9d`.
    let waiting = render.row_with("WAITING ON OTHERS") + 2;
    assert!(
        render.rows[waiting].ends_with("│ Waiting │"),
        "{:?}",
        render.rows[waiting]
    );

    // No Project or Age at a narrow width (and no Due, as no task has one), and no column
    // names at all in a stacked list.
    let render = render_board(&fixture_board(), NARROW, 80);
    let words = split_cells(&render.rows[5]);
    assert_eq!(words, ["#", "Task", "Action"], "{:?}", render.rows[5]);
    let render = render_board(&fixture_board(), 50, 80);
    assert!(!render.rows.iter().any(|row| row.contains("│ Task")));
}

// Round seven: Nadine's approved sample (space picker first, category buttons, a half-height
// card panel, full-width cards, double-click, one spacing system).

/// The drawn header button of `category`.
/// The Filter menu's item for `pick`, as the last frame drew it.
fn filter_item(state: &ClientShellState, pick: FilterPick) -> Rect {
    state
        .hits
        .task_board
        .work_filter_items
        .iter()
        .find(|(_, drawn)| *drawn == pick)
        .unwrap_or_else(|| panic!("{pick:?} not drawn"))
        .0
}

fn category_button(state: &ClientShellState, category: Category) -> Rect {
    state
        .hits
        .task_board
        .categories
        .iter()
        .find(|(_, drawn)| *drawn == category)
        .unwrap_or_else(|| panic!("{category:?} not drawn"))
        .0
}

#[test]
fn the_header_starts_with_the_tabs_then_the_space_picker_and_search_the_categories_and_the_time() {
    // Spacing: a column of margin, one space inside each button, one column between the
    // buttons of a group and a thin rule with a column each side between groups; the right
    // side ends a column from the edge.
    let search = format!("{:<23}", " ⌕ Search");
    // Tasks has the sort button after the search box; Work, with no rows to sort, has none.
    // The chips are not in the top bar: they sit on the chip row.
    let header = |sort: &str, time: &str| {
        let left = format!(" Tasks   Phases  │  Space: All ▾  {search} {sort}");
        let right = format!("Updated {time}   × ");
        let gap = 206 - 1 - UnicodeWidthStr::width(left.as_str()) - 1 - right.chars().count();
        format!(" {left}{}{}", " ".repeat(gap), right.trim_end())
    };
    let render = render_board(&fixture_board(), 206, 40);
    assert_eq!(render.rows[0], header(" Sort: Newest ▾ ", "8:05 am"));
    assert!(
        render.rows[2].ends_with(
            "◐ In progress 0   ! Action required 5   ◌ Queued 4   … Waiting on others 2"
        ),
        "{:?}",
        render.rows[2]
    );
    assert_eq!(render.hits.tasks_tab, Rect::new(1, 0, 7, 1));
    assert_eq!(render.hits.work_tab, Rect::new(9, 0, 8, 1));
    assert_eq!(render.hits.space_button, Rect::new(20, 0, 14, 1));
    assert_eq!(render.hits.filter, Rect::new(35, 0, 23, 1));
    assert_eq!(render.hits.sort_button, Rect::new(59, 0, 16, 1));
    assert_eq!(render.hits.close, Rect::new(202, 0, 3, 1));
    // A chip's glyph reads its section's colour.
    assert_eq!(render.cell(2, "!").fg, render.palette.yellow);
    assert_eq!(render.cell(2, "◐").fg, working_colour(&render.palette));

    // The Phases view has no kind chips: its top bar ends at the search box and the time, and
    // row 2 holds only the Filter menu.
    let render = render_board(&work_board(), 206, 40);
    assert_eq!(render.rows[0], header("", "10:30 am"));
    assert_eq!(render.rows[2].trim(), "Filter ▾");
}

#[test]
fn a_kind_pick_or_a_section_chip_shows_only_its_cards_or_tasks_and_a_second_click_all_again() {
    let bugs = [
        card("2026-09-24-grey-icon.md"),
        card("shipped-0.md"),
        card("shipped-5.md"),
    ];
    let drawn = |state: &ClientShellState| -> Vec<String> {
        state
            .hits
            .task_board
            .cards
            .iter()
            .map(|(_, file)| file.clone())
            .collect()
    };
    let mut state = work_state();
    state.task_board.view = BoardView::Work;
    state.compose(186, 46).expect("frame");
    // The Phases view's kinds are in its Filter menu: a click on the button, then on Bug.
    let button = state.hits.task_board.work_filter_button;
    board_click(&mut state, button);
    state.compose(186, 46).expect("frame");
    let bug = filter_item(&state, FilterPick::Kind(WorkKind::Bug));
    board_click(&mut state, bug);
    assert_eq!(state.task_board.work_filter.kind, Some(WorkKind::Bug));
    let frame = state.compose(186, 46).expect("frame");
    let shown = drawn(&state);
    assert!(
        !shown.is_empty() && shown.iter().all(|file| bugs.contains(file)),
        "only bugs: {shown:?}"
    );
    // The pick shows as a pill left of the button.
    let rows = frame_rows(&frame);
    let pill_row = &rows[usize::from(state.hits.task_board.work_filter_button.y)];
    assert!(pill_row.contains(" Kind: Bug × "), "{pill_row:?}");

    // With the filter's words too: bugs that name herdr.
    state.task_board.filter = "herdr".into();
    state.compose(186, 46).expect("frame");
    assert_eq!(drawn(&state), [card("2026-09-24-grey-icon.md")]);
    state.task_board.filter.clear();
    state.compose(186, 46).expect("frame");

    // Bug again, in the menu still open, shows all.
    let bug = filter_item(&state, FilterPick::Kind(WorkKind::Bug));
    board_click(&mut state, bug);
    assert_eq!(
        state.task_board.work_filter.kind, None,
        "a second click shows all"
    );
    state.compose(186, 46).expect("frame");
    assert!(drawn(&state).iter().any(|file| !bugs.contains(file)));

    // The Tasks view's buttons are its sections.
    let mut state = board_state();
    state.task_board.open = true;
    state.compose(186, 46).expect("frame");
    let waiting = category_button(&state, Category::Section(TaskSection::Waiting));
    board_click(&mut state, waiting);
    assert_eq!(state.task_board.only_section, Some(TaskSection::Waiting));
    let rows = frame_rows(&state.compose(186, 46).expect("frame"));
    let list = parse_task_list(LIST_V1).expect("fixture");
    let keys: Vec<RowKey> = state
        .hits
        .task_board
        .rows
        .iter()
        .map(|(_, key)| key.clone())
        .collect();
    assert_eq!(
        keys,
        list.tasks
            .iter()
            .filter(|task| task.group == TaskGroup::Waiting)
            .map(|task| RowKey::Task(TaskKey::of(task)))
            .collect::<Vec<_>>()
    );
    assert!(
        !rows.iter().any(|row| row.contains("ACTION REQUIRED")),
        "{rows:#?}"
    );
    assert!(
        !rows.iter().any(|row| row.contains("last 7 days")),
        "{rows:#?}"
    );
    let waiting = category_button(&state, Category::Section(TaskSection::Waiting));
    board_click(&mut state, waiting);
    assert_eq!(
        state.task_board.only_section, None,
        "a second click shows all"
    );
}

#[test]
fn a_narrow_panel_cuts_its_title_but_never_the_close() {
    let mut board = work_board();
    board.work_selected = Some(card("2026-09-25-lead-name.md"));
    for width in [186, 100, NARROW] {
        let render = render_board(&board, width, 46);
        let panel = render.hits.work_panel;
        let heading = &render.rows[usize::from(panel.y) + 1];
        assert!(heading.ends_with("×  │"), "{width}: {heading:?}");
        assert_eq!(render.hits.panel_close.y, panel.y + 1, "{width}");
        let title = heading
            .split("Feature ")
            .nth(1)
            .unwrap_or_default()
            .trim_start();
        assert_eq!(
            title.contains('…'),
            width < 186,
            "{width}: the title is cut first: {heading:?}"
        );
    }
}

#[test]
fn the_card_panel_is_a_box_half_the_body_with_a_heading_bar_labels_and_its_buttons_last() {
    let mut board = work_board();
    board.work_selected = Some(card("2026-09-24-tree-sidebar.md"));
    let render = render_board(&board, WIDE, 40);
    // Body rows 2 to 36 (35), a blank row, the rule and the keys: the panel takes 17.
    let (left, right) = (" │ ", " │");
    let rows = &render.rows;
    assert_eq!(rows[19], "", "a blank row above the panel");
    assert_eq!(rows[20], format!(" ╭{}╮", "─".repeat(114)));
    assert!(
        rows[21].starts_with(" │  ◆ Feature    Tree sidebar ")
            && rows[21].ends_with("herdr   ×  │"),
        "{:?}",
        rows[21]
    );
    assert_eq!(rows[22], format!(" ├{}┤", "─".repeat(114)));
    let inside: Vec<String> = rows[23..36]
        .iter()
        .map(|row| {
            assert!(
                row.starts_with(left) && row.ends_with(right),
                "inside the box: {row:?}"
            );
            row[left.len()..row.len() - right.len()]
                .trim_end()
                .to_owned()
        })
        .collect();
    assert_eq!(
        inside[..12],
        [
            "Status        · Awaiting review",
            "Owner         herdr",
            "",
            "STEPS  6 of 6 done",
            "✓ tree rows",
            "✓ fold",
            "✓ needs-you box",
            "✓ marks legend",
            "✓ icons",
            "Spec          ~/projects/repos/herdr/docs/intent/2026-09-24-tree-sidebar.md",
            "Branch        feat/sidebar-tree · 4 commits",
            "",
        ]
    );
    assert!(
        inside[12].starts_with(" ↑↓  step     Enter  open file")
            && inside[12].ends_with("▼ 1 more step and 2 tasks · scroll"),
        "{:?}",
        inside[12]
    );
    assert_eq!(rows[36], format!(" ╰{}╯", "─".repeat(114)));
    assert_eq!(rows[37], "", "a blank row above the footer's rule");
    assert!(render
        .cell(21, "Tree sidebar")
        .modifier
        .contains(Modifier::BOLD));
    assert!(!render.cell(26, "STEPS").modifier.contains(Modifier::BOLD));
    assert_eq!(render.cell(26, "STEPS").fg, render.palette.overlay1);
    assert_eq!(render.hits.panel_close, Rect::new(112, 21, 3, 1));

    // Half at another height too, and a long panel says what it cannot show on its last line.
    let mut list = parse_task_list(WORK_V1).expect("work fixture");
    list.work
        .iter_mut()
        .find(|item| item.file == card("2026-09-24-tree-sidebar.md"))
        .expect("tree card")
        .steps = (1..=30)
        .map(|step| crate::client::shell::task_board::model::WorkStep {
            full: String::new(),
            text: format!("step {step:02}"),
            done: step < 4,
            ..Default::default()
        })
        .collect();
    board.store.receive(Ok(list), std::time::Instant::now());
    let render = render_board(&board, WIDE, 46);
    // Body rows 2 to 42 (41): the panel takes 20, rows 23 to 42.
    assert!(render.rows[23].starts_with(" ╭") && render.rows[42].starts_with(" ╰"));
    let buttons = &render.rows[41];
    assert!(
        buttons.starts_with(" │  ↑↓  step     Enter  open file ")
            && buttons.ends_with("▼ 22 more steps and 2 tasks · scroll │"),
        "{buttons:?}"
    );
}

#[test]
fn the_panel_closes_by_its_button_or_esc_and_the_columns_take_the_whole_body() {
    let mut state = work_selected_state("2026-09-24-tree-sidebar.md", 186, 46);
    let whole_body = |state: &ClientShellState| state.hits.task_board.work_column_areas[2].height;
    let with_panel = whole_body(&state);
    let close = state.hits.task_board.panel_close;
    board_click(&mut state, close);
    state.compose(186, 46).expect("frame");
    assert_eq!(state.hits.task_board.panel_close, Rect::default(), "closed");
    assert_eq!(state.hits.task_board.work_panel, Rect::default());
    assert_eq!(
        state.task_board.work_selected,
        Some(card("2026-09-24-tree-sidebar.md")),
        "the card stays selected"
    );
    assert!(
        whole_body(&state) > with_panel * 2,
        "the columns take the body"
    );

    // Selecting a card shows it again; Esc closes the panel, a second Esc the board.
    press_key(&mut state, KeyCode::Down);
    state.compose(186, 46).expect("frame");
    assert_ne!(state.hits.task_board.panel_close, Rect::default());
    press_key(&mut state, KeyCode::Esc);
    assert!(state.task_board.open, "the first Esc closes the panel");
    state.compose(186, 46).expect("frame");
    assert_eq!(state.hits.task_board.panel_close, Rect::default());
    press_key(&mut state, KeyCode::Esc);
    assert!(!state.task_board.open, "the second closes the board");
}

#[test]
fn while_the_panel_shows_cards_keep_their_full_size_and_a_crowded_column_scrolls() {
    let mut state = work_selected_state("shipped-5.md", 186, 46);
    let board = &state.hits.task_board;
    assert!(
        board.cards.iter().all(|(rect, _)| rect.height == 9),
        "no one-line cards: {:?}",
        board.cards
    );
    assert!(board
        .cards
        .iter()
        .any(|(_, file)| *file == card("shipped-5.md")));
    assert!(
        board
            .work_markers
            .iter()
            .any(|marker| marker.column == 4 && marker.down),
        "SHIP says what is below"
    );
    // The selected card's column follows the arrows.
    for _ in 0..4 {
        press_key(&mut state, KeyCode::Down);
        state.compose(186, 46).expect("frame");
    }
    let selected = state.task_board.work_selected.clone().expect("selected");
    assert_eq!(selected, card("shipped-9.md"));
    assert!(state
        .hits
        .task_board
        .cards
        .iter()
        .any(|(rect, file)| *file == selected && rect.height == 9));
}

#[test]
fn the_five_columns_share_the_board_width_and_every_card_fills_its_column() {
    let columns = |width: u16| -> Vec<(u16, u16)> {
        let render = render_board(&work_board(), width, 60);
        let mut columns: Vec<(u16, u16)> = render
            .hits
            .cards
            .iter()
            .map(|(rect, _)| (rect.x, rect.width))
            .collect();
        columns.sort_unstable();
        columns.dedup();
        columns
    };
    // 160: 158 inside the margins, four gaps of 2, five columns of 30.
    assert_eq!(
        columns(160),
        [(1, 30), (33, 30), (65, 30), (97, 30), (129, 30)]
    );
    // 163: the three columns left over go to the first columns, one each; the last still
    // ends a column from the edge.
    assert_eq!(
        columns(163),
        [(1, 31), (34, 31), (67, 31), (100, 30), (132, 30)]
    );
    // A stage's label fills its column, and the gap after it is left bare.
    let render = render_board(&work_board(), 160, 60);
    let fill = render.buffer[(1, 3)].bg;
    assert!(filled(fill));
    assert_eq!(render.buffer[(30, 3)].bg, fill);
    assert!(!filled(render.buffer[(31, 3)].bg));
}

/// Moves the board's last click back a second, as if the next click came long after it.
fn age_last_click(state: &mut ClientShellState) {
    let click = state.task_board.last_click.as_mut().expect("a click");
    click.at = click
        .at
        .checked_sub(std::time::Duration::from_secs(1))
        .expect("an instant a second back");
}

#[test]
fn one_click_selects_and_a_double_click_opens_a_card_or_a_row() {
    let mut state = work_state();
    state.task_board.view = BoardView::Work;
    state.compose(160, 40).expect("frame");
    let file = card("2026-09-24-grey-icon.md");
    let rect = |state: &ClientShellState| {
        state
            .hits
            .task_board
            .cards
            .iter()
            .find(|(_, drawn)| *drawn == file)
            .map(|(rect, _)| *rect)
            .expect("drawn")
    };
    let at = rect(&state);
    assert!(open_requests(&board_click(&mut state, at)).is_empty());
    assert_eq!(state.task_board.work_selected.as_ref(), Some(&file));
    state.compose(160, 40).expect("frame");
    age_last_click(&mut state);
    let at = rect(&state);
    assert!(
        open_requests(&board_click(&mut state, at)).is_empty(),
        "a second single click later does nothing new"
    );
    state.compose(160, 40).expect("frame");
    let at = rect(&state);
    let sent = open_requests(&board_click(&mut state, at));
    assert_eq!(
        sent.into_iter().map(|(_, sent)| sent).collect::<Vec<_>>(),
        [file],
        "two clicks within the double-click time open it"
    );

    // A task row: a double click does what Enter does, a late second click nothing.
    let mut state = board_state();
    state.task_board.open = true;
    state.compose(106, 40).expect("frame");
    let row = row_of(&state, &key(6));
    board_click(&mut state, row);
    state.compose(106, 40).expect("frame");
    age_last_click(&mut state);
    let row = row_of(&state, &key(6));
    let outcome = board_click(&mut state, row);
    assert_eq!(focused_pane(&outcome), None, "a late click only selects");
    assert!(state.task_board.open);
    state.compose(106, 40).expect("frame");
    let row = row_of(&state, &key(6));
    let outcome = board_click(&mut state, row);
    assert_eq!(focused_pane(&outcome), Some("w3:p1"));
}

#[test]
fn stage_columns_have_no_subtitles_and_cards_start_under_their_label() {
    let render = render_board(&work_board(), WIDE, 40);
    for subtitle in [
        "/think done",
        "test first",
        "review, /check",
        "live check",
        "pushed",
    ] {
        assert!(
            !render.rows.iter().any(|row| row.contains(subtitle)),
            "{subtitle:?}: {:#?}",
            render.rows
        );
    }
    assert!(render.rows[3].contains("PLAN  2"));
    // The label, its stage's underline (B5b), then the cards.
    assert!(render.rows[4].trim_start().starts_with('━'));
    assert!(
        render.rows[5].trim_start().starts_with('╭'),
        "{:?}",
        render.rows[5]
    );
}

#[test]
fn typing_a_filter_puts_its_keys_under_the_box_and_draws_no_other_line() {
    let mut board = fixture_board();
    board.filter = "push".into();
    let kept = render_board(&board, 160, 40);
    board.filter_editing = true;
    let typing = render_board(&board, 160, 40);
    let x = usize::from(typing.hits.filter.x);
    assert_eq!(
        typing.rows[1],
        format!("{} Enter  keep     Esc  clear", " ".repeat(x)),
        "the keys start under the box"
    );
    assert_eq!(typing.rows[2..], kept.rows[2..], "no line moves");
}

/// The fixture's tasks with no problems line above the footer.
fn quiet_board() -> TaskBoard {
    let mut board = fixture_board();
    board.store.receive_json(
        &LIST_V1.replace("\"problems\": [", "\"old_problems\": ["),
        std::time::Instant::now(),
    );
    board
}

#[test]
fn keys_everywhere_are_a_block_a_space_their_words_and_four_columns_to_the_next() {
    let render = render_board(&quiet_board(), 160, 40);
    assert_eq!(
        render.rows[39],
        "  ↑ ↓  select     Enter  go to space     d  done     x  delete     m  move     /  search     \
         Tab  Phases     ?  all keys"
    );
    assert_eq!(render.cell(39, "Enter").bg, render.cell(39, "?").bg);
    assert!(is_rule(&render.rows[38]) && blank(&render.rows[37]));

    let render = render_board(&work_board(), 160, 40);
    assert_eq!(
        render.rows[39],
        "  ← ↑ ↓ →  select     Double-click  open     /  search     Tab  Tasks     ?  all keys"
    );

    // The detail under a selected row.
    let mut board = fixture_board();
    board.can_open = true;
    board.selected = Some(key(1));
    let render = render_board(&board, 160, 40);
    let detail = render.row_with(" Enter  go to space");
    assert!(
        render.rows[detail].contains(" Enter  go to space     o  open file     d  done"),
        "{:?}",
        render.rows[detail]
    );
}

#[test]
fn one_blank_row_parts_the_header_the_sections_and_the_footer_and_one_column_the_edges() {
    // Short enough that the sections fill the body.
    let mut board = quiet_board();
    board.done_expanded = true;
    let render = render_board(&board, 160, 28);
    let rows = &render.rows;
    let rule = rows.iter().rposition(|row| is_rule(row)).expect("rule");
    assert_eq!(rule, 26);
    assert!(
        blank(&rows[1]) && rows[3].contains("ACTION REQUIRED") && rows[5].contains("│ Task"),
        "{rows:#?}"
    );
    assert!(
        blank(&rows[rule - 1]) && !blank(&rows[rule - 2]),
        "{rows:#?}"
    );
    let claude = rows
        .iter()
        .position(|row| row.contains("QUEUED"))
        .expect("QUEUED heading");
    assert!(
        blank(&rows[claude - 1]) && !blank(&rows[claude - 2]),
        "{rows:#?}"
    );
    for (y, row) in rows.iter().enumerate().skip(1) {
        // A heading's rule runs to the edge by design (change 4); every other row keeps
        // a blank column on the right the way it always has.
        let runs_to_the_edge = row.trim_end().ends_with('─');
        assert!(
            row.is_empty()
                || (row.starts_with(' ')
                    && (runs_to_the_edge || UnicodeWidthStr::width(row.as_str()) < 160)),
            "row {y} keeps a column each side: {row:?}"
        );
    }

    // The Work view: its label, cards under it, a blank row, the panel, a blank row, the rule.
    let mut board = work_board();
    board.work_selected = Some(card("2026-09-24-tree-sidebar.md"));
    let render = render_board(&board, 160, 40);
    let rows = &render.rows;
    let panel = usize::from(render.hits.work_panel.y);
    assert!(rows[panel].starts_with(" ╭"), "{rows:#?}");
    let columns = render.hits.work_column_areas[4];
    assert!(
        blank(&rows[panel - 1]) && usize::from(columns.bottom()) == panel - 1,
        "{rows:#?}"
    );
    assert!(rows[37].is_empty() && rows[36].starts_with(" ╰"));
    // Inside a card, every line starts one space after its side.
    let kind = render.row_with("◆ feature");
    assert!(rows[kind].contains("│ ◆ feature"), "{:?}", rows[kind]);
}

#[test]
fn a_long_title_is_cut_in_the_panels_heading_and_shown_whole_above_its_steps() {
    // Where the card lives stays in the heading; the title gives way there, and the panel
    // shows it whole as its first lines.
    let long = "a very long card title that runs on and on past the heading bar's room, \
                as an intent file's first line sometimes does";
    let mut list = parse_task_list(WORK_V1).expect("work fixture");
    list.work
        .iter_mut()
        .find(|item| item.file == card("2026-09-24-tree-sidebar.md"))
        .expect("tree card")
        .title = long.to_owned();
    let mut board = work_board();
    board.store.receive(Ok(list), std::time::Instant::now());
    board.work_selected = Some(card("2026-09-24-tree-sidebar.md"));
    let render = render_board(&board, WIDE, 40);
    let heading = &render.rows[21];
    assert!(
        heading.contains("◆ Feature    a very long")
            && heading.contains("…")
            && heading.ends_with("herdr   ×  │"),
        "{heading:?}"
    );
    let inside: Vec<String> = render.rows[23..30]
        .iter()
        .map(|row| row.trim_matches(|ch| ch == ' ' || ch == '│').to_owned())
        .collect();
    assert!(
        shows_whole(&render, long) && inside[0].starts_with("a very long card title"),
        "{inside:#?}"
    );
    assert!(
        render
            .cell(23, "a very long")
            .modifier
            .contains(Modifier::BOLD),
        "the title as the heading shows it"
    );
    let steps = inside
        .iter()
        .position(|line| line.starts_with("STEPS"))
        .expect("the steps after it");
    assert!(
        inside[steps - 1].is_empty(),
        "a blank row before the steps: {inside:#?}"
    );
}

#[test]
fn rows_go_overdue_then_blocking_then_due_soonest_then_oldest_by_default() {
    use crate::client::shell::task_board::TaskSort;
    let mut board = sections_board();
    assert_eq!(board.sort, TaskSort::Priority);
    let render = render_board(&board, 186, 90);
    assert!(
        render.rows[0].contains(" Sort: Priority ▾ "),
        "{:?}",
        render.rows[0]
    );
    // Overdue, then blocking others, then the nearest due day, then the oldest; the
    // scheduled row last.
    in_order(
        &render,
        &[
            "Ship the oak CLI",
            "Pick the Chrome",
            "Approve the board",
            "Grant Screen",
            "Fern revenue",
        ],
    );
    in_order(
        &render,
        &[
            "Accuracy round 1",
            "Remove the board",
            "Board design B1",
            "Re-measure",
        ],
    );
    // The menu's by-due order still puts the due day before what blocks others.
    board.sort = TaskSort::Due;
    let render = render_board(&board, 186, 90);
    in_order(
        &render,
        &[
            "Ship the oak CLI",
            "Approve the board",
            "Grant Screen",
            "Pick the Chrome",
        ],
    );
}

#[test]
fn a_scheduled_row_stays_last_in_its_section_dim_with_when_it_starts_under_due() {
    use crate::client::shell::task_board::TaskSort;
    let mut board = sections_board();
    for sort in [TaskSort::Priority, TaskSort::Newest, TaskSort::Project] {
        board.sort = sort;
        let render = render_board(&board, 186, 90);
        let last = |title: &str, scheduled: &str| {
            let at = render.row_with(title);
            let bottom = (at..render.rows.len())
                .find(|y| render.rows[*y].starts_with(" ╰"))
                .unwrap_or_else(|| panic!("{title} has no bottom: {:#?}", render.rows));
            assert_eq!(
                render.row_with(scheduled),
                bottom - 1,
                "{sort:?}: {:#?}",
                render.rows
            );
        };
        last(" ACTION REQUIRED", "Fern revenue");
        last(" QUEUED", "Re-measure");
    }
    let render = render_board(&board, 186, 90);
    let head = render.row_with(" ACTION REQUIRED") + 2;
    let row = render.row_with("Fern revenue");
    // No due day, so Due says when it starts; its action shows as any row's.
    assert_eq!(render.column(row, "Starts Thu"), render.column(head, "Due"));
    assert_eq!(
        render.column(row, "? sign in to Oakwood"),
        render.column(head, "Action")
    );
    for text in ["Fern revenue", "Starts Thu", "sign in to Oakwood"] {
        assert_eq!(render.cell(row, text).fg, render.palette.overlay0, "{text}");
    }
    // Search still finds it by when it starts.
    board.filter = "starts thu".to_owned();
    let render = render_board(&board, 186, 90);
    render.row_with("Fern revenue");
}

/// The sections fixture with `edit` made to it, as `sections_board` reads it.
fn sections_board_with(
    edit: impl FnOnce(&mut crate::client::shell::task_board::model::TaskList),
) -> TaskBoard {
    let mut list = parse_task_list(LIST_SECTIONS).expect("the sections fixture is contract v1");
    edit(&mut list);
    let mut board = sections_board();
    board.store.receive(Ok(list), std::time::Instant::now());
    board
}

/// Card `chrome` for the sections fixture's task 3: a feature at build, one of its two steps
/// done, on a branch with two commits.
fn chrome_card() -> crate::client::shell::task_board::model::WorkItem {
    crate::client::shell::task_board::model::WorkItem {
        space: "oakwood".into(),
        kind: WorkKind::Feature,
        branch: Some("feat/chrome".into()),
        commits: 2,
        last_commit: Some("2026-09-28".into()),
        ..card_of(
            "chrome",
            "Chrome pairing",
            WorkStage::Build,
            &[("pick the profile", true), ("pair it", false)],
        )
    }
}

/// oakwood's lead at work, and its worker oak-w1 on the sections fixture's task 3.
fn oakwood_lead_and_worker() -> Vec<BoardAgent> {
    vec![
        working("w5:p1", "oakwood", "oak-lead", Some("build · oak-w1 on 3")),
        worker(
            "w5:p2",
            "oakwood",
            "oak-w1",
            "w5:p1",
            Some("reading the profile"),
        ),
    ]
}

/// The open block of task `n` of the sections fixture: its labelled fields and keys.
fn open_task_block(mut board: TaskBoard, n: usize, agents: &[BoardAgent]) -> Vec<(String, String)> {
    board.selected = Some(sections_key(n));
    board.open_row = board.selected.clone();
    let render = render_board_with_agents(&board, 186, 80, agents);
    block_fields(&lines_of(&render, &sections_key(n))[1..])
}

#[test]
fn an_open_task_row_names_its_card_agent_lead_progress_file_and_branch() {
    use crate::client::shell::task_board::model::TaskAgent;
    let field = |label: &str, value: &str| (label.to_owned(), value.to_owned());
    let on_task_3 = |card: Option<&str>, agent: bool| {
        let card = card.map(str::to_owned);
        sections_board_with(move |list| {
            let task = &mut list.tasks[2];
            task.card = card;
            task.agent = agent.then(|| TaskAgent {
                pane: "w5:p2".into(),
                name: "oak-w1".into(),
                minutes: Some(12),
            });
            list.work.push(chrome_card());
        })
    };
    let agents = oakwood_lead_and_worker();
    let file = "~/Workdir/oakwood-workspace/STATUS.md:31";
    // Linked by its card slug: the card, the agent on it, its lead, the card's steps, then the
    // file and the card's branch; the keys last, `w` among them.
    assert_eq!(
        open_task_block(on_task_3(Some("chrome"), true), 3, &agents),
        vec![
            field("Card", "◆ Feature · Chrome pairing · build · step 2 of 2"),
            field("Agent", "oak-w1 · 12m · reading the profile"),
            field("Lead", "oak-lead"),
            field("Progress", "━━━━━━━━━━────────── 1/2 steps · next: pair it"),
            field("File", file),
            field("Branch", "feat/chrome · 2 commits · last commit Sep 28"),
            field(
                "Enter",
                "go to agent     w  show its card     o  open file     d  done     ←  close"
            ),
        ]
    );
    // A slug no card has, and none: no Card, Progress or Branch line and no `w`.
    for card in [Some("gone"), None] {
        assert_eq!(
            open_task_block(on_task_3(card, true), 3, &agents),
            vec![
                field("Agent", "oak-w1 · 12m · reading the profile"),
                field("Lead", "oak-lead"),
                field("File", file),
                field(
                    "Enter",
                    "go to agent     o  open file     d  done     ←  close"
                ),
            ],
            "{card:?}"
        );
    }
    // No agent on it: Enter goes to its space, and the lead still shows.
    assert_eq!(
        open_task_block(on_task_3(None, false), 3, &agents),
        vec![
            field("Lead", "oak-lead"),
            field("File", file),
            field(
                "Enter",
                "go to space     o  open file     d  done     ←  close"
            ),
        ]
    );
    // The lead is the agent on it: named once, as the agent.
    let lead_on_it = sections_board_with(|list| {
        list.tasks[2].agent = Some(TaskAgent {
            pane: "w5:p1".into(),
            name: "oak-lead".into(),
            minutes: None,
        });
    });
    let block = open_task_block(lead_on_it, 3, &agents);
    assert!(block.iter().all(|(label, _)| label != "Lead"), "{block:?}");
    // An agent alone in its space leads no one: no Lead line.
    let alone = [working("w5:p1", "oakwood", "oak-lead", None)];
    let block = open_task_block(on_task_3(None, false), 3, &alone);
    assert!(block.iter().all(|(label, _)| label != "Lead"), "{block:?}");
}

#[test]
fn an_open_task_row_leaves_out_empty_card_facts() {
    // A card with no steps and no branch: its Card line, no Progress and no Branch.
    let board = || {
        sections_board_with(|list| {
            list.tasks[2].card = Some("chrome".into());
            list.work
                .push(crate::client::shell::task_board::model::WorkItem {
                    steps: Vec::new(),
                    branch: None,
                    ..chrome_card()
                });
        })
    };
    let labels: Vec<String> = open_task_block(board(), 3, &[])
        .into_iter()
        .map(|(label, _)| label)
        .collect();
    assert_eq!(labels, ["Card", "File", "Enter"]);
    let block = open_task_block(board(), 3, &[]);
    assert_eq!(block[0].1, "◆ Feature · Chrome pairing · build");
}

#[test]
fn an_open_task_row_shows_a_cell_its_row_cut_short_and_never_one_it_shows_whole() {
    let field = |label: &str, value: &str| (label.to_owned(), value.to_owned());
    // Task 3 blocks a round too long for its Blocking column: the block says it whole.
    let long = "Chrome pairing round and the Sundial customer id import";
    let board = sections_board_with(|list| list.tasks[2].blocking = Some(long.into()));
    let block = open_task_block(board, 3, &[]);
    assert_eq!(block[0], field("Blocking", long), "{block:?}");
    // Shown whole on the row (the action wraps open; the blocking and due fit): none of it.
    for n in [2, 3] {
        let block = open_task_block(sections_board(), n, &[]);
        for cell in ["Action", "Blocking", "Due"] {
            assert!(
                block.iter().all(|(label, _)| label != cell),
                "{n}: {block:?}"
            );
        }
    }
    // A waiting task's owner too long for its column shows whole; its request wraps open.
    let owner = "Josh, Carmen and the whole Oakwood data team";
    let board = sections_board_with(|list| list.tasks[10].owner = Some(owner.into()));
    let block = open_task_block(board, 11, &[]);
    assert_eq!(block[0], field("Owner", owner), "{block:?}");
    assert!(
        block.iter().all(|(label, _)| label != "Requested"),
        "{block:?}"
    );
}

#[test]
fn an_open_live_row_names_its_lead_task_progress_and_branch_but_not_what_its_row_shows() {
    let field = |label: &str, value: &str| (label.to_owned(), value.to_owned());
    let agents = lead_and_workers();
    let open = |mut board: TaskBoard, pane: &str| {
        board.selected = Some(RowKey::Agent(pane.into()));
        board.open_row = board.selected.clone();
        let render = render_board_with_agents(&board, 186, 60, &agents);
        block_fields(&lines_of(&render, &RowKey::Agent(pane.into()))[1..])
    };
    let card = crate::client::shell::task_board::model::WorkItem {
        branch: Some("feat/lead-name".into()),
        commits: 3,
        last_commit: Some("2026-09-28".into()),
        ..card_of(
            "lead-name",
            "Lead name kept whole",
            WorkStage::Build,
            &[("measure", true), ("cut nothing", false)],
        )
    };
    // herdr-w1's task 7 is on the card: its row shows the card's title, stage and step, so the
    // block names its lead, its task, the card's progress and its branch.
    let block = open(board_with_card(card), "w2:p2");
    let labels: Vec<&str> = block.iter().map(|(label, _)| label.as_str()).collect();
    assert_eq!(
        labels,
        ["Lead", "Task", "Progress", "Branch", "Enter"],
        "{block:?}"
    );
    assert_eq!(block[0], field("Lead", "herdr-lead"));
    assert!(
        block[1].1.starts_with("Never cut the lead's name"),
        "{block:?}"
    );
    assert_eq!(
        block[2],
        field(
            "Progress",
            "━━━━━━━━━━────────── 1/2 steps · next: cut nothing"
        )
    );
    assert_eq!(
        block[3],
        field("Branch", "feat/lead-name · 3 commits · last commit Sep 28")
    );
    assert_eq!(
        block[4],
        field("Enter", "go to agent     w  show its card     ←  close")
    );
    // No card: its row shows its task, so the block has only its lead and its keys.
    assert_eq!(
        open(tasks_and_work_board(), "w2:p2"),
        vec![
            field("Lead", "herdr-lead"),
            field("Enter", "go to agent     ←  close"),
        ]
    );
    // The lead itself: no Lead line, only its keys.
    assert_eq!(
        open(tasks_and_work_board(), "w2:p1"),
        vec![field("Enter", "go to agent     ←  close")]
    );
}

#[test]
fn a_selected_row_has_a_soft_band_down_its_lines_and_no_accent_bar() {
    // The column inside the box's left border: blank, on the band while the row is selected.
    let edge = |render: &BoardRender, y: usize| render.buffer[(2, y as u16)].clone();
    let banded = |render: &BoardRender, y: usize| {
        let cell = edge(render, y);
        cell.symbol() == " " && cell.bg == render.palette.selection_bg
    };
    let mut board = sections_board();
    board.selected = Some(sections_key(3));
    let render = render_board_with_agents(&board, 186, 80, &[sections_worker()]);
    let row = render.row_with("Pick the Chrome profile");
    assert!(banded(&render, row), "{:?}", render.rows[row]);
    let other = render.row_with("Ship the oak CLI login fix");
    assert_eq!(edge(&render, other).symbol(), " ");
    assert!(!banded(&render, other), "{:?}", render.rows[other]);
    // Open, the band runs down its block too, blank line and keys included.
    board.open_row = board.selected.clone();
    let render = render_board_with_agents(&board, 186, 80, &[sections_worker()]);
    let lines = lines_of(&render, &sections_key(3)).len();
    let row = render.row_with("Pick the Chrome profile");
    for y in row..row + lines {
        assert!(banded(&render, y), "{y}: {:?}", render.rows[y]);
    }
    assert!(!banded(&render, row + lines), "the next row");
    assert!(
        !render.rows.iter().any(|row| row.contains('▌')),
        "{:#?}",
        render.rows
    );
    // A selected IN PROGRESS row has it too.
    let mut board = tasks_and_work_board();
    board.selected = Some(RowKey::Agent("w2:p2".into()));
    let render = render_board_with_agents(&board, 186, 60, &lead_and_workers());
    let row = render.row_with(" herdr-w1 ");
    assert!(banded(&render, row), "{:?}", render.rows[row]);
    assert!(!banded(&render, render.row_with(" herdr-lead ")));
}

#[test]
fn w_shows_the_card_of_any_row_its_task_links_and_t_the_task_of_any_agent_row() {
    // Task 3 is on the task-board card by its slug; herdr-lead, a top row, is on task 3.
    let mut state = worker_state("w2:p2");
    let mut list = parse_task_list(LIST_V1).expect("fixture");
    list.work = parse_task_list(WORK_V1).expect("work fixture").work;
    let board_card = card("2026-09-25-task-board.md");
    if let Some(found) = list.work.iter_mut().find(|found| found.file == board_card) {
        found.slug = Some("task-board".into());
    }
    list.tasks[2].card = Some("task-board".into());
    list.tasks[2].section = Some(TaskSection::ActionRequired);
    list.tasks[2].agent = Some(crate::client::shell::task_board::model::TaskAgent {
        pane: "w2:p1".into(),
        name: "herdr-lead".into(),
        minutes: None,
    });
    state
        .task_board
        .store
        .receive(Ok(list.clone()), std::time::Instant::now());
    state.task_board.open = true;
    let task_3 = RowKey::Task(TaskKey::of(&list.tasks[2]));
    let task_4 = RowKey::Task(TaskKey::of(&list.tasks[3]));
    // A task row with no card: `w` does nothing.
    state.task_board.selected = Some(task_4.clone());
    state.compose(186, 46).expect("frame");
    press_key(&mut state, KeyCode::Char('w'));
    assert_eq!(state.task_board.view, BoardView::Tasks);
    assert_eq!(state.task_board.selected, Some(task_4));
    // The lead's top row: `t` selects the task it is on.
    state.task_board.selected = Some(RowKey::Agent("w2:p1".into()));
    state.compose(186, 46).expect("frame");
    press_key(&mut state, KeyCode::Char('t'));
    assert_eq!(state.task_board.selected, Some(task_3.clone()));
    // That task's row: `w` shows its card in the Work view.
    state.compose(186, 46).expect("frame");
    press_key(&mut state, KeyCode::Char('w'));
    assert_eq!(state.task_board.view, BoardView::Work);
    assert_eq!(state.task_board.work_selected, Some(board_card.clone()));
    // The lead's row reaches the same card through its task.
    state.task_board.view = BoardView::Tasks;
    state.task_board.selected = Some(RowKey::Agent("w2:p1".into()));
    state.compose(186, 46).expect("frame");
    press_key(&mut state, KeyCode::Char('w'));
    assert_eq!(state.task_board.view, BoardView::Work);
    assert_eq!(state.task_board.work_selected, Some(board_card));
}

/// The sections fixture with no task that needs Nadine.
fn nothing_needs_you_board() -> TaskBoard {
    sections_board_with(|list| {
        list.tasks
            .retain(|task| task.section() != TaskSection::ActionRequired);
    })
}

#[test]
fn an_empty_action_required_shows_its_heading_with_zero_and_nothing_needs_you() {
    let render = render_board(&nothing_needs_you_board(), 186, 80);
    let rows = in_order(&render, &[" ACTION REQUIRED 0", " QUEUED "]);
    let heading = &render.rows[rows[0]];
    assert!(heading.ends_with("nothing needs you"), "{heading:?}");
    // It does not fold, and its note is grey like any heading detail.
    assert!(
        !heading.contains('▼') && !heading.contains('▶'),
        "{heading:?}"
    );
    assert_eq!(
        render.cell(rows[0], "nothing needs you").fg,
        render.palette.overlay0
    );
    assert_eq!(
        render.cell(rows[0], "ACTION REQUIRED").fg,
        render.palette.yellow
    );
    // No box under it: a blank line, then the next section's heading.
    assert_eq!(render.rows[rows[0] + 1], "");
    assert_eq!(rows[1], rows[0] + 2, "{:#?}", render.rows);
}

#[test]
fn an_empty_action_required_under_a_filter_reads_zero_of_zero() {
    let mut board = nothing_needs_you_board();
    board.filter = "board".to_owned();
    let render = render_board(&board, 186, 80);
    let row = render.row_with(" ACTION REQUIRED 0 of 0");
    assert!(render.rows[row].ends_with("nothing needs you"));
}

#[test]
fn action_required_hidden_by_the_filter_alone_stays_hidden() {
    let mut board = sections_board();
    board.filter = "design b1".to_owned();
    let render = render_board(&board, 186, 80);
    render.row_with(" QUEUED 1 of");
    assert!(
        !render
            .rows
            .iter()
            .any(|row| row.contains("ACTION REQUIRED ")),
        "{:#?}",
        render.rows
    );
}

#[test]
fn a_tab_waiting_on_the_user_fills_action_required_so_it_says_nothing_else() {
    let agents = [waiting("w1:p2", "herdr", "herdr-w2", Some("pick A or B"))];
    let render = render_board_with_agents(&nothing_needs_you_board(), 186, 80, &agents);
    let row = render.row_with(" ACTION REQUIRED 1");
    assert!(!render.rows[row].contains("nothing needs you"));
}

#[test]
fn other_empty_sections_stay_hidden_beside_an_empty_action_required() {
    let board = sections_board_with(|list| {
        list.tasks.retain(|task| {
            !matches!(
                task.section(),
                TaskSection::ActionRequired | TaskSection::Waiting
            )
        });
    });
    let render = render_board(&board, 186, 80);
    render.row_with(" ACTION REQUIRED 0");
    assert!(
        !render
            .rows
            .iter()
            .any(|row| row.contains("WAITING ON OTHERS ")),
        "{:#?}",
        render.rows
    );
}

#[test]
fn another_section_shown_alone_has_no_empty_action_required() {
    let mut board = nothing_needs_you_board();
    board.only_section = Some(TaskSection::Queued);
    let render = render_board(&board, 186, 80);
    assert!(!render
        .rows
        .iter()
        .any(|row| row.contains("ACTION REQUIRED ")));
    render.row_with(" QUEUED ");
}

#[test]
fn an_all_empty_board_keeps_its_message_and_no_empty_action_required() {
    let board = sections_board_with(|list| list.tasks.clear());
    let render = render_board(&board, 186, 80);
    render.row_with("No tasks");
    assert!(!render
        .rows
        .iter()
        .any(|row| row.contains("ACTION REQUIRED ")));
}

#[test]
fn a_narrow_empty_action_required_keeps_its_note_whole_or_drops_it() {
    // B5c: with the count one space after the title the note fits at the board's smallest
    // width; narrower still, the board says the window is too small.
    for width in 30..=60 {
        let render = render_board(&nothing_needs_you_board(), width, 40);
        let Some(row) = render
            .rows
            .iter()
            .find(|row| row.contains(" ACTION REQUIRED 0"))
        else {
            continue;
        };
        assert!(
            row.contains("nothing needs you") || !row.contains("nothing"),
            "{width}: {row:?}"
        );
    }
}

#[test]
fn action_required_with_only_a_scheduled_row_counts_it_apart_and_says_nothing_else() {
    let board = sections_board_with(|list| {
        list.tasks.retain(|task| {
            task.section() != TaskSection::ActionRequired || task.task.contains("Fern")
        });
    });
    let render = render_board(&board, 186, 80);
    let row = render.row_with(" ACTION REQUIRED 0");
    let heading = &render.rows[row];
    assert!(heading.ends_with("+1 scheduled"), "{heading:?}");
    render.row_with("Fern revenue");
}

/// The fixture's problem: a notes file the read could not use.
const OLD_NOTES: &str = "/home/user/devbox/Workdir/oakwood-workspace/OLD-NOTES.md";

#[test]
fn a_read_problem_shows_as_one_short_red_line_without_its_path() {
    let render = render_board(&fixture_board(), WIDE, 40);
    let row = render.row_with("1 notes file has a problem");
    assert_eq!(render.rows[row].trim(), "1 notes file has a problem");
    assert_eq!(render.cell(row, "1 notes").fg, render.palette.red);
    assert!(!render.rows.iter().any(|row| row.contains(OLD_NOTES)));
    assert!(!render.rows.iter().any(|row| row.contains("Problems: ")));
}

#[test]
fn problems_count_notes_files_not_lines() {
    let board = fixture_board_with(|list| {
        list.problems
            .push(format!("{OLD_NOTES}: a row with no date"));
    });
    render_board(&board, WIDE, 40).row_with("1 notes file has a problem");
    let board = fixture_board_with(|list| {
        list.problems
            .push("/home/user/devbox/notes/b.md: no Task column".to_owned());
    });
    render_board(&board, WIDE, 40).row_with("2 notes files have a problem");
}

#[test]
fn no_problems_draw_no_problem_line() {
    let board = fixture_board_with(|list| list.problems.clear());
    let render = render_board(&board, WIDE, 40);
    assert!(!render.rows.iter().any(|row| row.contains("problem")));
    assert!(render.hits.problems.is_empty());
}

#[test]
fn every_key_shown_shows_each_problem_with_its_path() {
    let mut board = fixture_board();
    board.keys_shown = true;
    let render = render_board(&board, WIDE, 40);
    let row = render.row_with("Problems: ");
    assert!(
        render.rows[row].contains(OLD_NOTES),
        "{:?}",
        render.rows[row]
    );
    assert_eq!(render.cell(row, "Problems").fg, render.palette.red);
    assert!(!render.rows.iter().any(|row| row.contains("notes file has")));
    // A click would change nothing drawn, so the lines take none.
    assert!(render.hits.problems.is_empty());
}

#[test]
fn a_click_on_the_problem_line_shows_its_path_and_another_hides_it() {
    let mut state = board_state();
    state.task_board.open = true;
    let rows = frame_rows(&state.compose(WIDE, 40).expect("frame"));
    assert!(rows
        .iter()
        .any(|row| row.contains("1 notes file has a problem")));
    let line = state.hits.task_board.problems;
    board_click(&mut state, line);
    let rows = frame_rows(&state.compose(WIDE, 40).expect("frame"));
    assert!(rows.iter().any(|row| row.contains(OLD_NOTES)), "{rows:#?}");
    assert!(state.task_board.open, "the click stays on the board");
    let line = state.hits.task_board.problems;
    board_click(&mut state, line);
    let rows = frame_rows(&state.compose(WIDE, 40).expect("frame"));
    assert!(!rows.iter().any(|row| row.contains(OLD_NOTES)));
    assert!(rows
        .iter()
        .any(|row| row.contains("1 notes file has a problem")));
}

// B5b: the Phases view's kind and stage colours, its cards' progress line, and the details
// panel's steps in order.

/// Each kind's colour: features lilac, bugs red, chores teal.
fn kind_colour(kind: WorkKind, palette: &Palette) -> ratatui::style::Color {
    match kind {
        WorkKind::Feature => palette.mauve,
        WorkKind::Bug => palette.red,
        WorkKind::Chore => palette.teal,
        WorkKind::Unknown => palette.text,
    }
}

#[test]
fn the_kind_buttons_and_each_cards_kind_word_are_in_their_kinds_colour() {
    let render = render_board(&work_board(), 186, 90);
    let palette = &render.palette;
    // The kind buttons are gone: row 2 holds only the Filter menu, and the top bar names no
    // kind.
    assert_eq!(render.row_with("Filter ▾"), 2);
    for row in [&render.rows[0], &render.rows[2]] {
        for label in ["◆ Features", "✱ Bugs", "▣ Chores", "○ Chores"] {
            assert!(!row.contains(label), "{label:?} in {row:?}");
        }
    }
    for (word, kind) in [
        ("◆ feature", WorkKind::Feature),
        ("✱ bug", WorkKind::Bug),
        ("▣ chore", WorkKind::Chore),
    ] {
        let row = render.row_with(&format!("{word} · "));
        assert_drawn(&render, (row, word), kind_colour(kind, palette), false);
        // The project after it stays grey.
        let project = render.column(row, word) + UnicodeWidthStr::width(word) as u16;
        assert_eq!(
            render.buffer[(project + 1, row as u16)].fg,
            palette.overlay0,
            "{word}'s ` · `"
        );
    }
}

#[test]
fn a_card_of_a_kind_this_client_does_not_know_keeps_the_text_colour() {
    let board = phases_board_with(|list| {
        for card in &mut list.work {
            card.kind = WorkKind::Unknown;
        }
    });
    let render = render_board(&board, 186, 90);
    let row = render.row_with("· work · herdr");
    assert_drawn(&render, (row, "· work"), render.palette.text, false);
}

/// Each stage's colour, by its column label.
fn stage_colours(palette: &Palette) -> [(&'static str, ratatui::style::Color); 5] {
    [
        ("PLAN", palette.mauve),
        ("BUILD", palette.yellow),
        (
            "REVIEW",
            crate::client::shell::task_board::style::rose(palette),
        ),
        ("VERIFY", palette.blue),
        ("SHIP", palette.green),
    ]
}

#[test]
fn each_stage_heading_is_in_its_stage_colour_over_an_underline_of_it_even_with_no_cards() {
    // REVIEW and VERIFY hold no card here.
    let render = render_board(&two_empty_stages(), 186, 60);
    let palette = &render.palette;
    let row = render.row_with(" PLAN ");
    let columns = drawn_columns(&render);
    assert_eq!(columns.len(), 5);
    for ((heading, colour), (x, width)) in stage_colours(palette).into_iter().zip(columns) {
        assert_drawn(&render, (row, heading), colour, true);
        for column in x..x + width {
            let cell = &render.buffer[(column, row as u16 + 1)];
            assert_eq!(
                (cell.symbol(), cell.fg),
                ("━", colour),
                "{heading}'s underline at {column}: {:?}",
                render.rows[row + 1]
            );
        }
    }
    // An empty column says so under its underline.
    assert!(
        render.rows[row + 3].contains("no items"),
        "{:#?}",
        render.rows
    );
}

#[test]
fn the_one_column_pickers_heading_and_underline_are_in_the_picked_stages_colour() {
    let mut board = work_board();
    board.work_selected = Some(card("2026-09-24-grey-icon.md"));
    let render = render_board(&board, 70, 40);
    let row = render.row_with("◂  VERIFY");
    assert_drawn(&render, (row, "VERIFY"), render.palette.blue, true);
    let x = render.column(row, "◂");
    let cell = &render.buffer[(x, row as u16 + 1)];
    assert_eq!((cell.symbol(), cell.fg), ("━", render.palette.blue));
}

// B5b item 6: a card is its kind, a rule, its title on up to two lines, a progress line, who
// has it and its phase's status.

/// The box `file`'s card is drawn in.
fn card_box(render: &BoardRender, file: &str) -> Rect {
    render
        .hits
        .cards
        .iter()
        .find(|(_, card)| card == file)
        .map(|(rect, _)| *rect)
        .unwrap_or_else(|| panic!("{file}: {:#?}", render.rows))
}

/// What a card's row `y` says between its borders, trimmed.
fn card_row_text(render: &BoardRender, rect: Rect, y: u16) -> String {
    (rect.x + 1..rect.right() - 1)
        .map(|x| render.buffer[(x, y)].symbol().to_owned())
        .collect::<String>()
        .trim()
        .to_owned()
}

/// A card's progress line: its bar's colour, its filled and empty cells, and its count.
fn progress(render: &BoardRender, rect: Rect) -> (ratatui::style::Color, usize, usize, String) {
    let y = rect.y + 5;
    let line = card_row_text(render, rect, y);
    let (bar, count) = line.rsplit_once(' ').unwrap_or_else(|| panic!("{line:?}"));
    // Its first cell inside the box, not the same bar in another column.
    let x = (rect.x + 1..rect.right())
        .find(|x| render.buffer[(*x, y)].symbol() != " ")
        .unwrap_or(rect.x);
    (
        render.buffer[(x, y)].fg,
        bar.chars().filter(|c| *c == '━').count(),
        bar.chars().filter(|c| *c == '─').count(),
        count.to_owned(),
    )
}

#[test]
fn a_card_is_its_kind_a_rule_then_its_title_on_up_to_two_lines_nine_rows_high() {
    let render = render_board(&work_board(), WIDE, 40);
    let rect = card_box(&render, &card("2026-09-25-task-board.md"));
    assert_eq!(rect.height, 9, "{:#?}", render.rows);
    let rule = rect.y + 2;
    assert_eq!(render.buffer[(rect.x, rule)].symbol(), "├");
    assert_eq!(render.buffer[(rect.right() - 1, rule)].symbol(), "┤");
    let across = card_row_text(&render, rect, rule);
    assert!(
        !across.is_empty() && across.chars().all(|c| c == '─'),
        "{across:?}"
    );
    assert_eq!(
        render.buffer[(rect.x + 2, rule)].fg,
        render.palette.overlay0
    );
    assert_eq!(card_row_text(&render, rect, rect.y + 3), "Herdr task board");
    assert_eq!(
        card_row_text(&render, rect, rect.y + 4),
        "",
        "a short title leaves its second line blank"
    );

    // A long title takes both lines, cut with `…` only past the second.
    let file = card("2026-09-25-lead-name.md");
    let rect = card_box(&render, &file);
    let title = parse_task_list(WORK_V1)
        .expect("work fixture")
        .work
        .into_iter()
        .find(|item| item.file == file)
        .expect("lead-name card")
        .title;
    let (first, second) = (
        card_row_text(&render, rect, rect.y + 3),
        card_row_text(&render, rect, rect.y + 4),
    );
    assert!(
        !first.is_empty() && !second.is_empty(),
        "{first:?} {second:?}"
    );
    let shown = format!("{first} {second}");
    assert!(
        title.starts_with(shown.trim_end_matches('…')),
        "{shown:?} of {title:?}"
    );
    // The step count says itself on the progress line; no `of 3 steps` line.
    assert!(
        !render
            .rows
            .iter()
            .any(|row| row.contains(" steps") || row.contains("1 step")),
        "{:#?}",
        render.rows
    );
}

#[test]
fn a_progress_line_fills_done_over_all_steps_with_the_count_at_its_end() {
    let render = render_board(&work_board(), WIDE, 40);
    let rect = card_box(&render, &card("2026-09-23-per-client.md"));
    let (fg, filled, empty, count) = progress(&render, rect);
    assert_eq!(count, "1/2");
    assert_eq!(
        (fg, filled),
        (render.palette.text, empty),
        "half full, in text"
    );
    let y = rect.y + 5;
    let track = render.column(usize::from(y), "─");
    assert_eq!(render.buffer[(track, y)].fg, render.palette.surface1);
    assert_eq!(
        render.cell(usize::from(y), "1/2").fg,
        render.palette.overlay1
    );
}

#[test]
fn a_progress_line_is_yellow_while_its_agent_works_red_when_late_and_green_when_done() {
    let herdr_w1 = [working("w36:pV", "herdr", "herdr-w1", None)];
    let task_board = card("2026-09-25-task-board.md");
    let render = render_board_with_agents(&work_board(), WIDE, 40, &herdr_w1);
    let (fg, filled, empty, count) = progress(&render, card_box(&render, &task_board));
    assert_eq!(
        (fg, count.as_str()),
        (working_colour(&render.palette), "2/3")
    );
    assert!(filled > empty && empty > 0, "{filled} {empty}");
    // Its agent not at work: the text colour.
    let render = render_board(&work_board(), WIDE, 40);
    assert_eq!(
        progress(&render, card_box(&render, &task_board)).0,
        render.palette.text
    );

    let late = |list: &mut crate::client::shell::task_board::model::TaskList| {
        for item in &mut list.work {
            item.due_level = Some(DueLevel::Overdue);
        }
    };
    let render = render_board(&work_board_with(late), WIDE, 40);
    let rect = card_box(&render, &card("2026-09-23-per-client.md"));
    assert_eq!(progress(&render, rect).0, render.palette.red);
    // Working says more than late.
    let render = render_board_with_agents(&work_board_with(late), WIDE, 40, &herdr_w1);
    let rect = card_box(&render, &task_board);
    assert_eq!(progress(&render, rect).0, working_colour(&render.palette));
    // Due soon is not late.
    let render = render_board(
        &work_board_with(|list| {
            for item in &mut list.work {
                item.due_level = Some(DueLevel::Soon);
            }
        }),
        WIDE,
        40,
    );
    let rect = card_box(&render, &card("2026-09-23-per-client.md"));
    assert_eq!(progress(&render, rect).0, render.palette.text);

    // A closed card: green, all of it.
    let render = render_board(&work_board(), WIDE, 40);
    let (fg, filled, empty, count) = progress(&render, card_box(&render, &card("shipped-0.md")));
    assert_eq!(
        (fg, empty, count.as_str()),
        (render.palette.green, 0, "1/1")
    );
    assert!(filled > 0);
}

#[test]
fn a_card_with_no_steps_draws_no_progress_line_and_keeps_its_height() {
    let render = render_board(&work_board(), WIDE, 40);
    let rect = card_box(&render, &card("2026-09-24-build-sha.md"));
    assert_eq!(rect.height, 9);
    assert_eq!(
        card_row_text(&render, rect, rect.y + 5),
        "",
        "{:#?}",
        render.rows
    );
    assert!(!render
        .rows
        .iter()
        .any(|row| row.contains("0/0") || row.contains("no steps")));
}

#[test]
fn a_cards_agent_is_the_working_agent_on_an_open_task_that_links_it() {
    let linked = |group: TaskGroup| {
        move |list: &mut crate::client::shell::task_board::model::TaskList| {
            let item = list
                .work
                .iter_mut()
                .find(|item| item.file.ends_with("2026-09-23-per-client.md"))
                .expect("per-client card");
            item.slug = Some("2026-09-23-per-client".into());
            list.tasks[2].card = Some("2026-09-23-per-client".into());
            list.tasks[2].group = group;
            list.tasks[2].agent = Some(crate::client::shell::task_board::model::TaskAgent {
                pane: "w37:pX".into(),
                name: "oak-w2".into(),
                minutes: Some(12),
            });
        }
    };
    let oak_w2 = [working("w37:pX", "oakwood", "oak-w2", None)];
    let file = card("2026-09-23-per-client.md");
    let render = render_board_with_agents(
        &work_board_with(linked(TaskGroup::Waiting)),
        WIDE,
        40,
        &oak_w2,
    );
    let rect = card_box(&render, &file);
    assert_eq!(
        card_row_text(&render, rect, rect.y + 6),
        format!("{WORKER_SPINNER} oak-w2 · 12m"),
        "{:#?}",
        render.rows
    );
    assert_eq!(progress(&render, rect).0, working_colour(&render.palette));
    // Its agent idle: named, no spinner, the bar in text.
    let render = render_board(&work_board_with(linked(TaskGroup::Waiting)), WIDE, 40);
    let rect = card_box(&render, &file);
    assert_eq!(card_row_text(&render, rect, rect.y + 6), "oak-w2 · 12m");
    assert_eq!(progress(&render, rect).0, render.palette.text);
    // A done task, or no task linking it: not its agent, only its space's lead by name.
    for board in [work_board_with(linked(TaskGroup::Done)), work_board()] {
        let render = render_board_with_agents(&board, WIDE, 40, &oak_w2);
        let rect = card_box(&render, &file);
        assert_eq!(card_row_text(&render, rect, rect.y + 6), "oak-w2");
        assert_eq!(progress(&render, rect).0, render.palette.text);
    }
}

#[test]
fn a_column_fits_as_many_whole_nine_row_cards_as_its_rows_hold_and_counts_the_rest() {
    for (width, height) in [(WIDE, 40), (WIDE, 46), (60, 40)] {
        let mut board = work_board();
        if width == 60 {
            // The one-column picker, on SHIP.
            board.work_selected = Some(card("shipped-0.md"));
        }
        let render = render_board(&board, width, height);
        let ship = render.hits.work_column_areas[4];
        assert!(ship.height > 0, "{width}x{height}: {:#?}", render.rows);
        let fits = usize::from(ship.height) / 9;
        let drawn: Vec<Rect> = render
            .hits
            .cards
            .iter()
            .map(|(rect, _)| *rect)
            .filter(|rect| ship.contains(rect.as_position()))
            .collect();
        assert_eq!(drawn.len(), fits, "{width}x{height}: {:#?}", render.rows);
        assert!(drawn.iter().all(|rect| rect.height == 9));
        let label = &render.rows[usize::from(ship.y) - 2];
        assert!(
            label.contains(&format!("▼ {}", 10 - fits)),
            "{width}x{height}: {label:?}"
        );
    }
}

// B5b item 7: the details panel.

/// The build-overdue card's panel, its linked task's agent `herdr-w1` set, drawn beside
/// `agents`: the render and the words of each row inside it.
fn overdue_panel(agents: &[BoardAgent]) -> (BoardRender, Vec<(usize, String)>) {
    let mut board = phases_board_with(|list| {
        let task = list
            .tasks
            .iter_mut()
            .find(|task| task.card.as_deref() == Some("2026-09-29-build-overdue"))
            .expect("the task linking build-overdue");
        task.agent = Some(crate::client::shell::task_board::model::TaskAgent {
            pane: "w36:pV".into(),
            name: "herdr-w1".into(),
            minutes: Some(12),
        });
    });
    board.work_selected = Some(phases_card("build-overdue"));
    let render = render_board_with_agents(&board, 186, 80, agents);
    let panel = render.hits.work_panel;
    let inside = (panel.y + 3..panel.bottom() - 1)
        .map(|y| {
            let words: String = (panel.x + 2..panel.right() - 2)
                .map(|x| render.buffer[(x, y)].symbol().to_owned())
                .collect();
            (usize::from(y), words.trim_end().to_owned())
        })
        .collect();
    (render, inside)
}

#[test]
fn the_panels_title_bar_says_its_stage_in_the_stage_colour_and_its_phase_status() {
    let (render, _) = phases_panel("build-overdue", 186);
    let heading = usize::from(render.hits.work_panel.y) + 1;
    assert!(
        render.rows[heading].contains("build overdue   BUILD · ! Overdue 3d"),
        "{:?}",
        render.rows[heading]
    );
    assert_drawn(&render, (heading, "BUILD"), render.palette.yellow, true);
    assert_drawn(
        &render,
        (heading, "! Overdue 3d"),
        render.palette.red,
        false,
    );
    let (render, _) = phases_panel("verify-waiting", 186);
    let heading = usize::from(render.hits.work_panel.y) + 1;
    assert!(
        render.rows[heading].contains("VERIFY · ? Awaiting your check"),
        "{:?}",
        render.rows[heading]
    );
    assert_drawn(&render, (heading, "VERIFY"), render.palette.blue, true);
    assert_drawn(
        &render,
        (heading, "? Awaiting your check"),
        render.palette.yellow,
        false,
    );
    // A narrow panel keeps the title whole before the stage.
    let (render, _) = phases_panel("build-overdue", 40);
    let heading = usize::from(render.hits.work_panel.y) + 1;
    assert!(
        render.rows[heading].contains("build overdue"),
        "{:?}",
        render.rows[heading]
    );
}

#[test]
fn the_panel_lists_done_steps_then_the_current_one_bold_then_the_rest_with_no_codes() {
    let (render, inside) = overdue_panel(&[]);
    let words = panel_words(&inside);
    let steps = words
        .iter()
        .position(|line| line.starts_with("STEPS"))
        .unwrap_or_else(|| panic!("{words:#?}"));
    assert_eq!(
        words[steps..steps + 4],
        [
            "STEPS  1 of 3 done",
            "✓ Phases view columns.",
            "▸ Phases view: card status line and the details panel from card fields.",
            "○ Tidy the docs.",
        ],
        "{words:#?}"
    );
    let current = inside[steps + 2].0;
    assert_drawn(&render, (current, "▸"), render.palette.text, false);
    assert_drawn(
        &render,
        (current, "Phases view: card"),
        render.palette.text,
        true,
    );
    assert_drawn(
        &render,
        (inside[steps + 3].0, "Tidy"),
        render.palette.text,
        false,
    );
    // No step code anywhere in it, the Current step field too.
    assert!(
        !words
            .iter()
            .any(|line| line.contains("B2a") || line.contains("B2b")),
        "{words:#?}"
    );
    assert!(words.contains(
        &"Current step  Phases view: card status line and the details panel from card fields."
    ));
    // Its agent idle: a plain `▸`, no working words.
    assert!(!words.iter().any(|line| line.contains("working on it")));
}

#[test]
fn the_current_step_spins_with_its_card_agent_and_says_it_works_on_it() {
    let (render, inside) = overdue_panel(&[working("w36:pV", "herdr", "herdr-w1", None)]);
    let (row, line) = inside
        .iter()
        .find(|(_, words)| {
            words.contains("Phases view: card") && !words.starts_with("Current step")
        })
        .unwrap_or_else(|| panic!("{inside:#?}"));
    assert!(
        line.starts_with(&format!("{WORKER_SPINNER} Phases view: card"))
            && line.ends_with("   herdr-w1 working on it · 12m"),
        "{line:?}"
    );
    assert_eq!(
        render.cell(*row, WORKER_SPINNER).fg,
        working_colour(&render.palette)
    );
    assert_drawn(
        &render,
        (*row, "herdr-w1 working on it · 12m"),
        working_colour(&render.palette),
        false,
    );
    // Only the current step says so.
    assert_eq!(
        inside
            .iter()
            .filter(|(_, words)| words.contains("working on it"))
            .count(),
        1
    );
    // No task links the card: a plain `▸`, whoever works.
    let (_, inside) = phases_panel("build-overdue", 186);
    let words = panel_words(&inside);
    assert!(words
        .iter()
        .any(|line| line.starts_with("▸ Phases view: card")));
    assert!(!words.iter().any(|line| line.contains("working on it")));
}

#[test]
fn a_card_with_every_step_done_has_no_current_step_and_its_codes_still_filter() {
    let (_, inside) = phases_panel("review-open", 186);
    let words = panel_words(&inside);
    assert!(
        !words.iter().any(|line| line.starts_with('▸')),
        "{words:#?}"
    );
    assert!(words.contains(&"✓ Board columns."), "{words:#?}");
    // A step's code still finds its card, though no longer drawn.
    let mut board = phases_board();
    board.filter = "B2b".into();
    let render = render_board(&board, 186, 80);
    let shown: Vec<&String> = render.hits.cards.iter().map(|(_, card)| card).collect();
    assert_eq!(shown, [&phases_card("build-overdue")], "{:#?}", render.rows);
}

#[test]
fn the_panel_keys_sit_under_a_blank_line_and_a_footed_panel_keeps_two_rows_of_lines() {
    let (render, _) = phases_panel("build-overdue", 186);
    let panel = render.hits.work_panel;
    let keys = usize::from(panel.bottom()) - 2;
    let inner = |y: usize| {
        (panel.x + 1..panel.right() - 1)
            .map(|x| render.buffer[(x, y as u16)].symbol().to_owned())
            .collect::<String>()
            .trim()
            .to_owned()
    };
    assert!(inner(keys).starts_with("↑↓"), "{:?}", inner(keys));
    assert_eq!(inner(keys - 1), "", "a blank line before the keys");
    assert!(
        inner(keys - 2).starts_with("Branch"),
        "{:?}",
        inner(keys - 2)
    );
    assert!(inner(keys - 3).starts_with("Spec"), "{:?}", inner(keys - 3));
    // Whatever the height, a panel with its Spec and Branch lines keeps two rows above them.
    let mut board = phases_board();
    board.work_selected = Some(phases_card("build-overdue"));
    for height in 18..40 {
        let render = render_board(&board, 186, height);
        let panel = render.hits.work_panel;
        let Some(spec) =
            (panel.y..panel.bottom()).find(|y| render.rows[usize::from(*y)].contains("Spec "))
        else {
            continue;
        };
        assert!(spec >= panel.y + 5, "{height}: {:#?}", render.rows);
    }
}

#[test]
fn the_arrows_walk_the_steps_in_the_order_the_panel_shows_them() {
    let mut state = work_state();
    let mut list = parse_task_list(WORK_V1).expect("work fixture");
    tree_steps(&mut list);
    state
        .task_board
        .store
        .receive(Ok(list), std::time::Instant::now());
    state.task_board.view = BoardView::Work;
    state.task_board.work_selected = Some(card("2026-09-24-tree-sidebar.md"));
    state.compose(186, 46).expect("frame");
    let banded = |state: &mut ClientShellState| {
        let frame = state.compose(186, 46).expect("frame");
        let rows = frame_rows(&frame);
        let buffer = frame.to_ratatui_buffer().expect("buffer");
        let band = state.config.palette.selection_bg;
        let (y, _) = state
            .hits
            .task_board
            .work_step_rows
            .iter()
            .map(|(rect, step)| (rect.y, rect.x + 1, *step))
            .find(|(y, x, _)| buffer[(*x, *y)].bg == band)
            .map(|(y, _, step)| (y, step))
            .unwrap_or_else(|| panic!("{rows:#?}"));
        rows[usize::from(y)].clone()
    };
    press_key(&mut state, KeyCode::Down);
    assert!(banded(&mut state).contains("✓ Tree rows."));
    press_key(&mut state, KeyCode::Down);
    assert!(banded(&mut state).contains("▸ Folded counts."));
    press_key(&mut state, KeyCode::Down);
    assert!(banded(&mut state).contains("○ Rail cleanup."));
}

// B5c: the mark carries the state colour, names and words stay quiet, bold only for
// top-level names.

/// The sections board with its worker at work and a tab waiting on Nadine.
fn quiet_cells_render(width: u16) -> BoardRender {
    let agents = [
        sections_worker(),
        waiting("w2:p1", "herdr", "herdr-w2", Some("Pick the plan")),
    ];
    render_board_with_agents(&sections_board(), width, 60, &agents)
}

fn bold(cell: &ratatui::buffer::Cell) -> bool {
    cell.modifier.contains(Modifier::BOLD)
}

#[test]
fn an_action_cell_draws_a_yellow_question_mark_before_its_words_in_text() {
    let render = quiet_cells_render(WIDE);
    let palette = &render.palette;
    let y = render.row_with("Ship the oak CLI");
    let mark = render.cell(y, "? say go on");
    assert_eq!((mark.fg, bold(mark)), (palette.yellow, false), "{mark:?}");
    assert_eq!(render.cell(y, "say go on").fg, palette.text);
    // A task that only needs you says nothing there, and draws no mark.
    let y = render.row_with("Grant Screen Recording");
    assert!(!render.rows[y].contains('?'), "{:?}", render.rows[y]);
}

#[test]
fn a_tab_waiting_on_you_has_a_plain_question_mark_and_its_action_in_text() {
    let render = quiet_cells_render(WIDE);
    let palette = &render.palette;
    let y = render.row_with("Pick the plan");
    let number = render.cell(y, "?");
    assert_eq!(
        (number.fg, bold(number)),
        (palette.yellow, false),
        "{number:?}"
    );
    let mark = render.cell(y, "? answer in tab");
    assert_eq!((mark.fg, bold(mark)), (palette.yellow, false), "{mark:?}");
    assert_eq!(render.cell(y, "answer in tab").fg, palette.text);
}

#[test]
fn an_owner_cell_draws_a_lilac_mark_before_a_grey_name_the_filter_still_marks() {
    let render = quiet_cells_render(WIDE);
    let palette = &render.palette;
    let y = render.row_with("Settle the security");
    let mark = render.cell(y, "… Carmen");
    assert_eq!((mark.fg, bold(mark)), (palette.mauve, false), "{mark:?}");
    assert_eq!(render.cell(y, "Carmen").fg, palette.overlay1);
    // The longest owner still fits whole beside its mark.
    let y = render.row_with("Supply the missing");
    assert!(
        render.rows[y].contains("… Josh or Oakwood"),
        "{:?}",
        render.rows[y]
    );

    let mut board = sections_board();
    board.filter = "Carmen".into();
    let render = render_board(&board, WIDE, 60);
    let y = render.row_with("Settle the security");
    let mark = render.cell(y, "… Carmen");
    let name = render.cell(y, "Carmen");
    assert_eq!(
        mark.bg,
        render.buffer[(0, 1)].bg,
        "the mark is not a match: {mark:?}"
    );
    assert_ne!(name.bg, mark.bg, "the name is: {name:?}");
}

#[test]
fn a_completed_cell_draws_a_green_tick_before_a_grey_day_not_crossed_out() {
    let render = quiet_cells_render(WIDE);
    let palette = &render.palette;
    let y = render.row_with("A1 notes format");
    let mark = render.cell(y, "✓ Today");
    assert_eq!((mark.fg, bold(mark)), (palette.green, false), "{mark:?}");
    let day = render.cell(y, "Today");
    assert_eq!(day.fg, palette.overlay1);
    assert!(!day.modifier.contains(Modifier::CROSSED_OUT), "{day:?}");
}

#[test]
fn an_open_rows_mark_starts_its_first_line_only() {
    let list = parse_task_list(LIST_SECTIONS).expect("the sections fixture is contract v1");
    let n = 1 + list
        .tasks
        .iter()
        .position(|task| task.task.starts_with("Pick the Chrome"))
        .expect("the Chrome task");
    let mut board = sections_board();
    board.selected = Some(sections_key(n));
    board.open_row = board.selected.clone();
    let render = render_board(&board, 60, 60);
    let lines = lines_of(&render, &sections_key(n));
    let y = render.row_with(&lines[0]);
    assert_eq!(render.cell(y, "? pick").fg, render.palette.yellow);
    let marks = lines.iter().filter(|line| line.contains("? ")).count();
    assert_eq!(marks, 1, "{lines:#?}");
    let words: String = lines.iter().map(|line| line.as_str()).collect();
    assert!(words.contains("pick one of two"), "{lines:#?}");
}

#[test]
fn a_stacked_rows_state_carries_its_mark_and_a_done_row_no_second_tick() {
    let render = quiet_cells_render(50);
    let palette = &render.palette;
    let y = render.row_with("say go on");
    assert_eq!(render.cell(y, "? say go on").fg, palette.yellow);
    assert_eq!(render.cell(y, "say go on").fg, palette.text);
    assert!(
        render.rows.iter().all(|row| !row.contains("✓ ✓")),
        "{:#?}",
        render.rows
    );
}

#[test]
fn an_agents_name_in_progress_is_grey_and_its_spinner_yellow_not_bold() {
    for width in [WIDE, 50] {
        let render = quiet_cells_render(width);
        let palette = &render.palette;
        let y = render.row_with("herdr-w1");
        assert_eq!(render.cell(y, "herdr-w1").fg, palette.overlay1, "{width}");
        let spinner = render.cell(y, LEAD_SPINNER);
        assert_eq!(
            (spinner.fg, bold(spinner)),
            (palette.yellow, false),
            "{width}"
        );
    }
}

#[test]
fn due_today_stale_and_blocking_are_not_bold_and_still_read_apart_from_the_rest() {
    let json = LIST_DUE.replace(
        r#""group": "waiting", "state": "Carmen"#,
        r#""group": "you", "state": "Carmen"#,
    );
    let mut board = due_board();
    board.store.receive_json(&json, std::time::Instant::now());
    let render = render_board(&board, WIDE, 60);
    let palette = &render.palette;
    let today = render.cell(render.row_with("VS Code Workspace Trust"), "Today");
    let later = render.cell(render.row_with("Fern: next revenue"), "Thu");
    assert_eq!((today.fg, bold(today)), (palette.text, false), "{today:?}");
    assert_eq!(
        (later.fg, bold(later)),
        (palette.overlay0, false),
        "{later:?}"
    );
    let stale = render.cell(render.row_with("RFC: security answers"), "12d");
    assert_eq!((stale.fg, bold(stale)), (palette.text, false), "{stale:?}");

    let render = quiet_cells_render(WIDE);
    let palette = &render.palette;
    let y = render.row_with("Pick the Chrome");
    let blocking = render.cell(y, "Chrome pairing");
    assert_eq!(
        (blocking.fg, bold(blocking)),
        (palette.text, false),
        "{blocking:?}"
    );
    let age = render.cell(y, "3d");
    assert_eq!((age.fg, bold(age)), (palette.overlay0, false), "{age:?}");
}

#[test]
fn a_rows_project_is_grey_and_its_waiting_days_dim() {
    let render = quiet_cells_render(WIDE);
    let palette = &render.palette;
    let y = render.row_with("Settle the security");
    assert_eq!(render.cell(y, "Causeway").fg, palette.overlay1);
    assert_eq!(render.cell(y, "3d").fg, palette.overlay0);
}

#[test]
fn on_vesper_the_action_mark_and_the_owner_mark_differ() {
    let agents = [sections_worker()];
    let area = Rect::new(0, 0, WIDE, 60);
    let mut buffer = Buffer::empty(area);
    let palette = Palette::vesper();
    render_task_board(
        &mut buffer,
        area,
        &sections_board(),
        &palette,
        Some("/home/user/devbox"),
        (&agents, (WORKER_SPINNER, palette.yellow)),
    );
    let find = |needle: &str| {
        (0..60u16)
            .find_map(|y| {
                let row: String = (0..WIDE)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect();
                row.find(needle)
                    .map(|byte| (UnicodeWidthStr::width(&row[..byte]) as u16, y))
            })
            .unwrap_or_else(|| panic!("{needle:?} not drawn"))
    };
    let action = buffer[find("? say go on")].fg;
    let owner = buffer[find("… Carmen")].fg;
    assert_eq!(action, palette.yellow);
    assert_ne!(action, owner);
}

// B5c C2: the board's chrome is quiet: counts, heads and labels grey, one fill for what is
// chosen, a red label and not a red sentence.

/// A section heading's count on the board: its row, its first cell's column and the words
/// after the title.
fn heading_count(render: &BoardRender) -> (usize, u16, String) {
    const TITLES: [&str; 5] = [
        "ACTION REQUIRED",
        "IN PROGRESS",
        "QUEUED",
        "WAITING ON OTHERS",
        "COMPLETED",
    ];
    for (y, row) in render.rows.iter().enumerate() {
        for title in TITLES {
            let Some(byte) = row.find(&format!("{title} ")) else {
                continue;
            };
            let rest = &row[byte + title.len() + 1..];
            if rest.starts_with(|c: char| c.is_ascii_digit()) {
                let x = render.column(y, title) + title.len() as u16 + 1;
                return (y, x, rest.to_owned());
            }
        }
    }
    panic!(
        "no heading with its count one space after it: {:#?}",
        render.rows
    )
}

#[test]
fn a_section_headings_count_is_a_plain_grey_number_one_space_after_its_title() {
    let mut filtered = fixture_board();
    filtered.filter = "Chrome".into();
    for (board, of) in [(fixture_board(), false), (filtered, true)] {
        let render = render_board(&board, WIDE, 40);
        let palette = &render.palette;
        let (y, x, rest) = heading_count(&render);
        let count = &render.buffer[(x, y as u16)];
        let title = &render.buffer[(x - 2, y as u16)];
        assert_eq!(
            (count.fg, count.bg, bold(count)),
            (palette.overlay1, title.bg, false),
            "{count:?}"
        );
        assert_eq!(
            render.buffer[(x - 1, y as u16)].bg,
            title.bg,
            "no fill before it"
        );
        assert_eq!(rest.split_whitespace().nth(1) == Some("of"), of, "{rest:?}");
    }
}

#[test]
fn column_heads_and_field_labels_are_grey_and_not_bold() {
    let render = quiet_cells_render(WIDE);
    let y = render.row_with("ACTION REQUIRED") + 2;
    let first = render.rows[y]
        .char_indices()
        .find(|(_, c)| c.is_alphabetic())
        .map(|(byte, _)| UnicodeWidthStr::width(&render.rows[y][..byte]) as u16)
        .unwrap_or_else(|| panic!("no head: {:?}", render.rows[y]));
    let head = &render.buffer[(first, y as u16)];
    assert_eq!(
        (head.fg, bold(head)),
        (render.palette.overlay1, false),
        "{head:?}"
    );

    let (render, inside) = phases_panel("review-open", 186);
    let panel = render.hits.work_panel;
    let (y, words) = inside
        .iter()
        .find(|(_, words)| words.starts_with("Owner "))
        .unwrap_or_else(|| panic!("{inside:#?}"));
    let label = &render.buffer[(panel.x + 2, *y as u16)];
    assert_eq!(
        (label.fg, bold(label)),
        (render.palette.overlay1, false),
        "{label:?}"
    );
    let value_x =
        panel.x + 2 + UnicodeWidthStr::width(&words[..words.find("You").expect("You")]) as u16;
    // The owner is a who: grey, as a card's who line is.
    assert_eq!(
        render.buffer[(value_x, *y as u16)].fg,
        render.palette.overlay1
    );
}

#[test]
fn the_stage_pickers_arrows_are_grey_and_plain() {
    let render = render_board(&work_board(), 79, 60);
    let palette = &render.palette;
    let y = 3;
    let left = &render.buffer[(render.column(y, "◂"), y as u16)];
    let right = &render.buffer[(render.column(y, "▸"), y as u16)];
    for arrow in [left, right] {
        assert_eq!(
            (arrow.fg, bold(arrow)),
            (palette.overlay1, false),
            "{arrow:?}"
        );
    }
    // The stage's own label keeps its colour and weight.
    assert!(bold(render.cell(y, "PLAN")));
}

#[test]
fn a_chosen_menu_item_takes_the_selection_fill_only() {
    let mut board = tasks_and_work_board();
    board.space = Some("herdr".into());
    board.agent_menu = true;
    let render = render_board_with_agents(&board, 186, 46, &lead_and_workers());
    let palette = &render.palette;
    let row = |rect: Rect| {
        (rect.x..rect.right())
            .map(|x| render.buffer[(x, rect.y)].clone())
            .collect::<Vec<_>>()
    };
    // Every agent is the chosen item while no agent is.
    let (chosen, _) = &render.hits.agent_items[0];
    for cell in row(*chosen) {
        assert_eq!(cell.bg, palette.selection_bg, "{cell:?}");
        assert!(!bold(&cell), "{cell:?}");
        assert_ne!(cell.fg, palette.accent, "{cell:?}");
    }
    let (other, _) = &render.hits.agent_items[1];
    assert!(row(*other)
        .iter()
        .all(|cell| cell.bg != palette.selection_bg));

    let mut board = fixture_board();
    board.sort_menu = true;
    let render = render_board(&board, 186, 46);
    let palette = &render.palette;
    let row = |rect: Rect| {
        (rect.x..rect.right())
            .map(|x| render.buffer[(x, rect.y)].clone())
            .collect::<Vec<_>>()
    };
    let (chosen, _) = render
        .hits
        .sort_items
        .iter()
        .find(|(rect, _)| render.rows[usize::from(rect.y)].contains("Newest first"))
        .expect("the chosen order");
    let words = render.cell(usize::from(chosen.y), "Newest first");
    assert_eq!((words.fg, bold(words)), (palette.text, false), "{words:?}");
    for cell in row(*chosen) {
        assert_eq!(cell.bg, palette.selection_bg, "{cell:?}");
    }
    let (other, _) = render.hits.sort_items[0];
    assert!(row(other)
        .iter()
        .all(|cell| cell.bg != palette.selection_bg));
}

#[test]
fn the_acting_lines_spinner_is_yellow_and_its_words_grey() {
    let mut board = fixture_board();
    board.acting = Some(crate::client::shell::task_board::Acting {
        request_id: "client-shell-tasks-act:1".into(),
        task: task_key(1),
        label: "done".into(),
    });
    let render = render_board(&board, WIDE, 40);
    let palette = &render.palette;
    let y = render.row_with("◐ done…");
    let spinner = render.cell(y, "◐");
    assert_eq!((spinner.fg, bold(spinner)), (palette.yellow, false));
    assert_eq!(render.cell(y, "done…").fg, palette.overlay1);
}

#[test]
fn the_foots_notice_is_text_and_a_red_label_starts_an_error() {
    let mut board = fixture_board();
    board.notice = Some("Select a task first.".into());
    let render = render_board(&board, WIDE, 40);
    let palette = &render.palette;
    let y = render.row_with("Select a task first.");
    assert_eq!(render.cell(y, "Select").fg, palette.text);
    // The count alone has no label: it stays red.
    let y = render.row_with("1 notes file has a problem");
    assert_eq!(render.cell(y, "1 notes").fg, palette.red);

    let mut board = fixture_board();
    board.keys_shown = true;
    let render = render_board(&board, WIDE, 40);
    let y = render.row_with("Problems: ");
    assert_eq!(render.cell(y, "Problems").fg, render.palette.red);
    let after = render.column(y, "Problems: ") + 10;
    assert_eq!(render.buffer[(after, y as u16)].fg, render.palette.text);

    let mut board = fixture_board();
    board
        .store
        .receive_json("not json", std::time::Instant::now());
    let render = render_board(&board, WIDE, 40);
    let y = render.row_with("Last read failed: ");
    assert_eq!(render.cell(y, "Last read").fg, render.palette.red);
    assert_eq!(render.cell(y, "unreadable").fg, render.palette.text);
}

#[test]
fn the_active_view_tab_takes_the_selection_fill_and_the_other_none() {
    for palette in [
        Palette::catppuccin(),
        Palette::vesper(),
        Palette::terminal(),
    ] {
        let mut board = tasks_and_work_board();
        board.space = Some("herdr".into());
        let render = render_board_on(&board, 186, 46, &lead_and_workers(), palette);
        let palette = &render.palette;
        let at = |rect: Rect| &render.buffer[(rect.x + 1, rect.y)];
        let on = at(render.hits.tasks_tab);
        let off = at(render.hits.work_tab);
        let gap = &render.buffer[(render.hits.work_tab.x - 1, render.hits.work_tab.y)];
        assert_eq!(
            (on.fg, on.bg, bold(on)),
            (palette.text, palette.selection_bg, false)
        );
        assert_eq!(
            (off.fg, off.bg, bold(off)),
            (palette.overlay1, gap.bg, false)
        );
        assert_ne!((on.fg, on.bg), (off.fg, off.bg), "the tabs read apart");
        // The agent picker is a button as the sort button is, not a tab.
        let agent = at(render.hits.agent_button);
        let sort = at(render.hits.sort_button);
        assert_eq!((agent.fg, agent.bg), (palette.overlay1, sort.bg));
    }
}

#[test]
fn an_active_filter_reads_in_text_on_its_tint_and_its_clear_is_plain() {
    let mut board = fixture_board();
    board.filter = "Chrome".into();
    let render = render_board(&board, WIDE, 40);
    let palette = &render.palette;
    let filter = &render.buffer[(render.hits.filter.x + 1, render.hits.filter.y)];
    assert_eq!(filter.fg, palette.text, "{filter:?}");
    assert!(
        leans_to(filter.bg, palette.accent, palette.yellow),
        "{filter:?}"
    );
    let clear = render.hits.filter_clear;
    let clear = &render.buffer[(clear.x, clear.y)];
    assert_eq!(
        (clear.symbol(), clear.fg, bold(clear)),
        ("×", palette.text, false)
    );
    assert_eq!(clear.bg, filter.bg);
}

#[test]
fn a_footer_key_is_a_plain_button() {
    let render = render_board(&fixture_board(), WIDE, 40);
    let (rect, _) = render.hits.footer_keys[0];
    let block = &render.buffer[(rect.x, rect.y)];
    assert!(!bold(block), "{block:?}");
    assert_ne!(block.bg, ratatui::style::Color::Reset, "{block:?}");
}

#[test]
fn every_empty_message_is_the_same_grey() {
    let reading = TaskBoard {
        open: true,
        ..TaskBoard::default()
    };
    let render = render_board(&reading, WIDE, 40);
    let y = render.row_with("Reading tasks…");
    assert_eq!(render.cell(y, "Reading").fg, render.palette.overlay1);

    let mut empty = TaskBoard {
        open: true,
        view: BoardView::Work,
        ..TaskBoard::default()
    };
    empty.store.receive_json(
        LIST_V1.replace("\"work\": [", "\"old_work\": [").as_str(),
        std::time::Instant::now(),
    );
    let render = render_board(&empty, WIDE, 40);
    let y = render.row_with("No features, bugs or chores yet");
    assert_eq!(render.cell(y, "No features").fg, render.palette.overlay1);

    let render = render_board(&two_empty_stages(), WIDE, 60);
    let y = render.row_with("no items");
    assert_eq!(render.cell(y, "no items").fg, render.palette.overlay1);
}

/// The text drawn in `rect` on its row.
fn text_in(render: &BoardRender, rect: Rect) -> String {
    (rect.x..rect.right())
        .map(|x| render.buffer[(x, rect.y)].symbol())
        .collect()
}

#[test]
fn the_top_bar_reads_the_view_switch_then_space_search_and_sort_then_the_chips_on_no_band() {
    for palette in [Palette::catppuccin(), Palette::vesper()] {
        for view in [BoardView::Tasks, BoardView::Work] {
            let mut board = fixture_board();
            board.view = view;
            let render = render_board_on(&board, HEADER_WIDE, 40, &[], palette.clone());
            let header = &render.rows[0];
            let hits = &render.hits;
            assert!(
                header.starts_with("  Tasks   Phases  │  Space: All ▾   ⌕ Search"),
                "{view:?}: {header:?}"
            );
            assert!(!header.contains('·'), "no dot in the top bar: {header:?}");
            assert!(header.contains("Updated "), "{header:?}");
            // Each group's buttons one column apart; a `│` with one column each side
            // between the tabs and the space and search.
            assert_eq!(hits.tasks_tab.x, 1);
            assert_eq!(hits.work_tab.x, hits.tasks_tab.right() + 1);
            assert_eq!(render.buffer[(hits.work_tab.right() + 1, 0)].symbol(), "│");
            assert_eq!(hits.space_button.x, hits.work_tab.right() + 3);
            assert_eq!(hits.filter.x, hits.space_button.right() + 1);
            let last = if view == BoardView::Tasks {
                assert_eq!(hits.sort_button.x, hits.filter.right() + 1);
                hits.sort_button
            } else {
                assert!(hits.sort_button.is_empty());
                hits.filter
            };
            // The chips left the top bar: it holds the one rule, and the Tasks view's chips are
            // one column apart on row 2 (the Phases view has none).
            assert_eq!(header.matches('│').count(), 1, "{header:?}");
            let chips = &hits.categories;
            if view == BoardView::Tasks {
                assert!(!chips.is_empty());
                assert!(chips.iter().all(|(rect, _)| rect.y == 2), "{chips:?}");
                for pair in chips.windows(2) {
                    assert_eq!(pair[1].0.x, pair[0].0.right() + 1, "{header:?}");
                }
            } else {
                assert!(chips.is_empty(), "{chips:?}");
                assert_eq!(render.rows[2].trim(), "Filter ▾");
            }
            // The time, then the close button three blank columns on, as a button's gap.
            let close = hits.close;
            assert_eq!(close, Rect::new(HEADER_WIDE - 4, 0, 3, 1));
            assert_ne!(render.buffer[(close.x - 3, 0)].symbol(), " ", "{header:?}");
            assert!(header.ends_with("m   ×"), "{header:?}");
            // No band: the gaps and the margin show the board's own ground.
            let ground = render.buffer[(0, 1)].bg;
            for x in [
                0,
                hits.work_tab.right(),
                hits.work_tab.right() + 1,
                hits.space_button.right(),
                last.right(),
                close.x - 1,
                HEADER_WIDE - 1,
            ] {
                assert_eq!(render.buffer[(x, 0)].bg, ground, "{view:?} column {x}");
            }
            // Every button's hit covers its own label.
            assert_eq!(text_in(&render, hits.tasks_tab), " Tasks ");
            assert_eq!(text_in(&render, hits.work_tab), " Phases ");
            assert_eq!(text_in(&render, hits.space_button), " Space: All ▾ ");
            assert_eq!(text_in(&render, close), " × ");
            if view == BoardView::Tasks {
                assert_eq!(text_in(&render, hits.sort_button), " Sort: Newest ▾ ");
            }
            assert!(text_in(&render, hits.filter).contains("⌕ Search"));
        }
    }
}

#[test]
fn a_chosen_space_reads_space_and_its_name_with_its_clear_one_column_on() {
    let mut board = fixture_board();
    board.space = Some("herdr".to_owned());
    let render = render_board(&board, HEADER_WIDE, 40);
    let hits = &render.hits;
    assert_eq!(text_in(&render, hits.space_button), " Space: herdr ▾ ");
    assert_eq!(hits.back_to_all.x, hits.space_button.right() + 1);
    assert_eq!(text_in(&render, hits.back_to_all), " × ");
    assert_eq!(hits.filter.x, hits.back_to_all.right() + 1);
}

#[test]
fn a_narrow_top_bar_keeps_its_buttons_apart_and_in_order() {
    for width in [160, 118, 100, 98, 80, 70, 64, 60, 44, 40] {
        for view in [BoardView::Tasks, BoardView::Work] {
            let mut board = fixture_board();
            board.view = view;
            let render = render_board(&board, width, 40);
            let hits = &render.hits;
            let header = &render.rows[0];
            let mut rects = vec![
                hits.tasks_tab,
                hits.work_tab,
                hits.space_button,
                hits.filter,
            ];
            if !hits.sort_button.is_empty() {
                rects.push(hits.sort_button);
            }
            rects.push(hits.close);
            assert!(
                rects.iter().all(|rect| !rect.is_empty() && rect.y == 0),
                "{width} {view:?}: {header:?}"
            );
            for pair in rects.windows(2) {
                assert!(
                    pair[0].right() < pair[1].x,
                    "{width} {view:?}: {:?} then {:?} in {header:?}",
                    pair[0],
                    pair[1]
                );
            }
            // The chips, on row 2, keep apart and in order too, inside the margins.
            let chips: Vec<Rect> = hits.categories.iter().map(|(rect, _)| *rect).collect();
            assert!(chips.iter().all(|rect| rect.y == 2), "{width}: {chips:?}");
            assert_eq!(
                chips.is_empty(),
                view == BoardView::Work,
                "{width} {view:?}"
            );
            for pair in chips.windows(2) {
                assert!(pair[0].right() < pair[1].x, "{width}: {chips:?}");
            }
            if let (Some(first), Some(last)) = (chips.first(), chips.last()) {
                assert!(first.x >= 1 && last.right() < width, "{width}: {chips:?}");
            }
            assert_eq!(text_in(&render, hits.tasks_tab), " Tasks ");
            assert!(hits.close.right() < width, "{width}: {header:?}");
        }
    }
}

/// The chip row's chips as drawn: each hit rect's text and its section, left to right.
fn drawn_chips(render: &BoardRender) -> Vec<(String, TaskSection)> {
    render
        .hits
        .categories
        .iter()
        .map(|(rect, category)| {
            let Category::Section(section) = category;
            (text_in(render, *rect), *section)
        })
        .collect()
}

/// The column of the table's right border, on its top row.
fn table_right_border(render: &BoardRender) -> u16 {
    let top = render.row_with("╮");
    render.column(top, "╮")
}

#[test]
fn the_tasks_chips_sit_on_their_own_row_in_section_order_ending_on_the_tables_right_border() {
    let render = render_board(&fixture_board(), 186, 46);
    let header = &render.rows[0];
    for gone in ["In progress", "Action required", "Queued", "Waiting on"] {
        assert!(
            !header.contains(gone),
            "{gone:?} left the top bar: {header:?}"
        );
    }
    assert_eq!(header.matches('│').count(), 1, "one rule: {header:?}");
    assert_eq!(
        drawn_chips(&render),
        [
            (" ◐ In progress 0 ".to_owned(), TaskSection::InProgress),
            (
                " ! Action required 5 ".to_owned(),
                TaskSection::ActionRequired
            ),
            (" ◌ Queued 4 ".to_owned(), TaskSection::Queued),
            (" … Waiting on others 2 ".to_owned(), TaskSection::Waiting),
        ]
    );
    let rects: Vec<Rect> = render
        .hits
        .categories
        .iter()
        .map(|(rect, _)| *rect)
        .collect();
    assert!(rects.iter().all(|rect| rect.y == 2), "{rects:?}");
    for pair in rects.windows(2) {
        assert_eq!(
            pair[0].right() + 1,
            pair[1].x,
            "one column apart: {rects:?}"
        );
    }
    let last = rects.last().expect("chips");
    assert_eq!(
        last.right(),
        table_right_border(&render) + 1,
        "{:?}",
        render.rows[2]
    );
}

#[test]
fn a_chip_colours_only_its_glyph_and_takes_a_fill_only_when_on() {
    use crate::client::shell::task_board::style::lilac;
    for palette in [Palette::catppuccin(), Palette::vesper()] {
        let roles = [
            (TaskSection::InProgress, palette.yellow),
            (TaskSection::ActionRequired, palette.yellow),
            (TaskSection::Queued, palette.teal),
            (TaskSection::Waiting, lilac(&palette)),
        ];
        for only in [None, Some(TaskSection::Queued)] {
            let mut board = fixture_board();
            board.only_section = only;
            let render = render_board_on(&board, 186, 46, &[], palette.clone());
            for (rect, category) in &render.hits.categories {
                let Category::Section(section) = category;
                let (_, role) = roles
                    .iter()
                    .find(|(shown, _)| shown == section)
                    .expect("a role");
                let on = only == Some(*section);
                let mark = &render.buffer[(rect.x + 1, rect.y)];
                assert_eq!(mark.fg, *role, "{section:?}'s mark");
                for x in rect.x..rect.right() {
                    let cell = &render.buffer[(x, rect.y)];
                    assert!(!cell.modifier.contains(Modifier::BOLD), "{section:?} bold");
                    assert_eq!(
                        cell.bg != ratatui::style::Color::Reset,
                        on,
                        "{section:?} fill at {x}"
                    );
                    if x > rect.x + 1 && !cell.symbol().trim().is_empty() {
                        let words = if on { palette.text } else { palette.overlay1 };
                        assert_eq!(cell.fg, words, "{section:?}'s words at {x}");
                    }
                }
            }
        }
    }
}

#[test]
fn narrow_chip_rows_keep_icon_and_count_and_their_hits_follow_them() {
    let board = fixture_board();
    for width in [118u16, 80, 70, 60, 50, 40] {
        let render = render_board(&board, width, 46);
        let chips = drawn_chips(&render);
        let rects: Vec<Rect> = render
            .hits
            .categories
            .iter()
            .map(|(rect, _)| *rect)
            .collect();
        assert!(!chips.is_empty(), "{width}: some chip shows");
        for (text, _) in &chips {
            let mut parts = text.split_whitespace();
            let mark = parts.next().expect("a mark");
            assert!(["◐", "!", "◌", "…"].contains(&mark), "{width}: {text:?}");
            assert!(
                text.trim_end().ends_with(|c: char| c.is_ascii_digit()),
                "{width}: its count last: {text:?}"
            );
        }
        for pair in rects.windows(2) {
            assert!(pair[0].right() < pair[1].x, "{width}: apart {rects:?}");
        }
        assert!(rects[0].x >= 1, "{width}: inside the margin {rects:?}");
        assert!(
            rects.last().expect("chips").right() < width,
            "{width}: inside the margin {rects:?}"
        );
    }
    // At 80 all four show whole; at 40, icon and count.
    let words = |width| {
        drawn_chips(&render_board(&board, width, 46))
            .into_iter()
            .map(|(text, _)| text)
            .collect::<Vec<_>>()
    };
    assert_eq!(words(80).len(), 4);
    assert!(words(80)[2].contains("Queued"), "{:?}", words(80));
    assert_eq!(words(40), [" ◐ 0 ", " ! 5 ", " ◌ 4 ", " … 2 "]);
}

#[test]
fn the_queued_chip_shows_queued_alone_and_a_second_click_all() {
    let mut state = board_state();
    state.task_board.open = true;
    state.compose(186, 46).expect("frame");
    let queued = category_button(&state, Category::Section(TaskSection::Queued));
    board_click(&mut state, queued);
    assert_eq!(state.task_board.only_section, Some(TaskSection::Queued));
    let rows = frame_rows(&state.compose(186, 46).expect("frame"));
    assert!(rows.iter().any(|row| row.contains("QUEUED")), "{rows:#?}");
    assert!(
        !rows.iter().any(|row| row.contains("ACTION REQUIRED")),
        "{rows:#?}"
    );
    let queued = category_button(&state, Category::Section(TaskSection::Queued));
    board_click(&mut state, queued);
    assert_eq!(state.task_board.only_section, None);
}

#[test]
fn the_chip_row_sits_under_a_blank_row_and_the_body_starts_right_under_it() {
    let tasks = render_board(&fixture_board(), 186, 46);
    assert_eq!(tasks.rows[1].trim(), "", "a blank row under the top bar");
    assert!(
        tasks.rows[2].contains("◐ In progress"),
        "{:?}",
        tasks.rows[2]
    );
    assert!(
        tasks.rows[3].starts_with("  ACTION REQUIRED"),
        "{:?}",
        tasks.rows[3]
    );
    let phases = render_board(&phases_board(), 186, 46);
    assert_eq!(phases.rows[1].trim(), "");
    assert_eq!(phases.rows[2].trim(), "Filter ▾");
    assert!(phases.rows[3].contains("PLAN"), "{:?}", phases.rows[3]);
    // At 40 rows a column too tall for its cards counts those under it, and none is lost.
    let render = render_board(&phases_board(), 186, 40);
    for (index, column) in render.hits.work_column_areas.iter().enumerate() {
        let drawn = render
            .hits
            .cards
            .iter()
            .filter(|(rect, _)| rect.x >= column.x && rect.x < column.right())
            .count();
        let under = render
            .hits
            .work_markers
            .iter()
            .filter(|marker| marker.column == index && marker.down)
            .map(|marker| render.hits.work_columns[index].len() - marker.first)
            .max()
            .unwrap_or(0);
        let all = render.hits.work_columns[index].len();
        assert!(
            drawn + under >= all,
            "column {index}: {drawn} drawn and {under} under of {all}"
        );
    }
}

#[test]
fn the_phases_chip_row_holds_only_the_filter_button_on_the_columns_right_border() {
    for width in [186u16, 118, 80] {
        let render = render_board(&phases_board(), width, 46);
        for gone in ["Features", "Bugs", "Chores"] {
            assert!(
                !render.rows[0].contains(gone),
                "{width}: {:?}",
                render.rows[0]
            );
        }
        let button = render.hits.work_filter_button;
        assert_eq!(text_in(&render, button), " Filter ▾ ");
        assert_eq!(button.y, 2);
        let columns = render.hits.work_column_areas.last().expect("columns");
        assert_eq!(
            button.right(),
            columns.right(),
            "{width}: {:?}",
            render.rows[2]
        );
        assert_eq!(
            render.buffer[(button.x, button.y)].bg,
            render.cell(0, " Space: All ▾ ").bg,
            "a button's fill"
        );
    }
}

#[test]
fn the_filter_menu_box_is_never_bold_where_it_covers_a_stage_heading() {
    let mut board = phases_board();
    board.work_filter_menu = true;
    // At 118 the box's top border lies on the bold stage heading row.
    let render = render_board(&board, 118, 40);
    let button = render.hits.work_filter_button;
    let items = &render.hits.work_filter_items;
    let (last, _) = items.last().expect("items drawn");
    let left = items[0].0.x - 2;
    let top = button.y + 1;
    assert!(
        render.rows[usize::from(top)].contains('┌'),
        "{:?}",
        render.rows[usize::from(top)]
    );
    for y in top..=last.y + 1 {
        for x in left..button.right() {
            assert!(
                !render.buffer[(x, y)].modifier.contains(Modifier::BOLD),
                "bold at {x},{y}: {:?}",
                render.buffer[(x, y)].symbol()
            );
        }
    }
}

#[test]
fn a_filtered_stage_heading_drops_of_all_rather_than_cut_its_count() {
    let mut board = phases_board();
    board.work_filter.status = Some(CardStatus::NeedsYou);
    let render = render_board(&board, 80, 40);
    let headings = render
        .rows
        .iter()
        .find(|row| row.contains("PLAN"))
        .expect("stage headings drawn");
    assert!(!headings.contains('…'), "{headings:?}");
    for heading in ["REVIEW  1", "VERIFY  1"] {
        assert!(headings.contains(heading), "{heading}: {headings:?}");
    }
    // Wide enough, the heading keeps its whole count.
    let render = render_board(&board, 186, 40);
    let headings = render
        .rows
        .iter()
        .find(|row| row.contains("PLAN"))
        .expect("stage headings drawn");
    assert!(headings.contains("REVIEW  1 of 3"), "{headings:?}");
}

#[test]
fn the_filter_menu_lists_status_kind_and_owner_with_coloured_marks_and_grey_counts() {
    use crate::client::shell::task_board::style::lilac;
    for palette in [Palette::catppuccin(), Palette::vesper()] {
        let mut board = phases_board();
        board.work_filter_menu = true;
        let render = render_board_on(&board, 186, 46, &[], palette.clone());
        let button = render.hits.work_filter_button;
        let items = &render.hits.work_filter_items;
        assert_eq!(items.len(), 11);
        assert!(items
            .iter()
            .all(|(rect, _)| rect.right() + 1 == button.right()));
        let top = button.y + 1;
        assert_eq!(
            items[0].0.y,
            top + 2,
            "under its box's top and the STATUS heading"
        );
        let line = |pick: FilterPick| {
            let (rect, _) = items
                .iter()
                .find(|(_, drawn)| *drawn == pick)
                .expect("drawn");
            (*rect, text_in(&render, *rect))
        };
        let feature = lilac(&palette);
        let bug = kind_colour(WorkKind::Bug, &palette);
        let chore = kind_colour(WorkKind::Chore, &palette);
        for (pick, mark, words, count, role) in [
            (
                FilterPick::Status(CardStatus::NeedsYou),
                "?",
                "Needs you",
                "3",
                palette.yellow,
            ),
            (
                FilterPick::Status(CardStatus::AtRisk),
                "!",
                "At risk",
                "2",
                palette.red,
            ),
            (
                FilterPick::Status(CardStatus::Blocked),
                "✗",
                "Blocked",
                "0",
                palette.red,
            ),
            (
                FilterPick::Status(CardStatus::OnTrack),
                "·",
                "On track",
                "6",
                palette.overlay1,
            ),
            (
                FilterPick::Status(CardStatus::Done),
                "✓",
                "Done",
                "1",
                palette.green,
            ),
            (
                FilterPick::Kind(WorkKind::Feature),
                "◆",
                "Feature",
                "12",
                feature,
            ),
            (FilterPick::Kind(WorkKind::Bug), "✱", "Bug", "0", bug),
            (FilterPick::Kind(WorkKind::Chore), "▣", "Chore", "0", chore),
            (
                FilterPick::Owner(CardOwner::Me),
                "",
                "Me",
                "1",
                palette.text,
            ),
            (
                FilterPick::Owner(CardOwner::Agents),
                "",
                "Agents",
                "1",
                palette.text,
            ),
            (
                FilterPick::Owner(CardOwner::Others),
                "",
                "Others",
                "1",
                palette.text,
            ),
        ] {
            let (rect, text) = line(pick);
            let expected = format!("{mark:<1} {words}");
            assert!(
                text.starts_with(&format!(" {expected} ")),
                "{pick:?}: {text:?}"
            );
            assert!(
                text.trim_end().ends_with(&format!(" {count}")),
                "{pick:?}: {text:?}"
            );
            let at = |offset: usize| &render.buffer[(rect.x + offset as u16, rect.y)];
            if !mark.is_empty() {
                assert_eq!(at(1).fg, role, "{pick:?}'s mark");
            }
            assert_eq!(at(3).fg, palette.text, "{pick:?}'s words");
            let count_x = UnicodeWidthStr::width(text.trim_end()) - 1;
            assert_eq!(at(count_x).fg, palette.overlay1, "{pick:?}'s count");
        }
        let y = usize::from(top + 1);
        let heading = &render.rows[y];
        let status = render.column(y, "STATUS");
        assert_eq!(
            render.buffer[(status, top + 1)].fg,
            palette.overlay1,
            "{heading:?}"
        );
        assert!(render.rows.iter().any(|row| row.contains("│ KIND ")));
        assert!(render.rows.iter().any(|row| row.contains("│ OWNER ")));
        assert_eq!(
            render.buffer[(items[0].0.x + 2, items[0].0.y)].bg,
            palette.selection_bg,
            "the keyboard's item"
        );
    }
}

/// The phases fixture's card named `name`, with the list it came in.
fn phases_card_of<'a>(
    list: &'a crate::client::shell::task_board::model::TaskList,
    name: &str,
) -> &'a crate::client::shell::task_board::model::WorkItem {
    list.work
        .iter()
        .find(|card| card.file.ends_with(&format!("{name}.md")))
        .unwrap_or_else(|| panic!("{name}"))
}

#[test]
fn each_status_bucket_takes_its_cards_from_their_fields() {
    use crate::client::shell::task_board::card_status;
    let mut list = parse_task_list(WORK_PHASES).expect("phases fixture");
    // Blocked by its status in any case; closed is done at any stage; due today is at risk.
    for card in &mut list.work {
        if card.file.ends_with("build-on-track.md") {
            card.status = "Blocked".into();
        }
        if card.file.ends_with("review-none.md") {
            card.status = "closed".into();
        }
        if card.file.ends_with("plan-approved.md") {
            card.due_level = Some(DueLevel::Today);
        }
        if card.file.ends_with("verify-waiting.md") {
            card.due_level = Some(DueLevel::Overdue);
        }
    }
    let status = |name: &str| card_status(phases_card_of(&list, name), &list.tasks, &[]);
    for (name, expected) in [
        // Needs you: a plan not approved, a build in VERIFY unverified (even overdue), or
        // one he holds (its open task's owner is You).
        ("2026-09-29-plan-pending", CardStatus::NeedsYou),
        ("2026-09-29-verify-waiting", CardStatus::NeedsYou),
        ("2026-09-29-review-open", CardStatus::NeedsYou),
        // At risk: overdue, or due today.
        ("2026-09-29-build-overdue", CardStatus::AtRisk),
        ("2026-09-29-build-today", CardStatus::AtRisk),
        ("2026-09-29-plan-approved", CardStatus::AtRisk),
        ("2026-09-29-build-on-track", CardStatus::Blocked),
        ("2026-09-29-review-none", CardStatus::Done),
        ("2026-09-29-ship", CardStatus::Done),
        // On track: the rest.
        ("2026-09-29-review-passed", CardStatus::OnTrack),
        ("2026-09-29-verify-done", CardStatus::OnTrack),
        ("2026-09-29-all-null", CardStatus::OnTrack),
    ] {
        assert_eq!(status(name), expected, "{name}");
    }
    // Non-matches: an approval in any case is not waiting; `blocking` is not blocked.
    for card in &mut list.work {
        if card.file.ends_with("plan-pending.md") {
            card.approval = Some("APPROVED 2026-09-29".into());
        }
        if card.file.ends_with("build-on-track.md") {
            card.status = "blocking".into();
        }
    }
    let status = |name: &str| card_status(phases_card_of(&list, name), &list.tasks, &[]);
    assert_eq!(status("2026-09-29-plan-pending"), CardStatus::OnTrack);
    assert_eq!(status("2026-09-29-build-on-track"), CardStatus::OnTrack);
}

#[test]
fn each_kind_bucket_takes_its_cards_and_chores_read_a_square() {
    let mut board = phases_board_with(|list| {
        for card in &mut list.work {
            if card.file.ends_with("build-on-track.md") {
                card.kind = WorkKind::Chore;
            }
            if card.file.ends_with("review-none.md") {
                card.kind = WorkKind::Bug;
            }
        }
    });
    board.work_filter_menu = true;
    let render = render_board(&board, 186, 46);
    let count = |pick: FilterPick| {
        let (rect, _) = render
            .hits
            .work_filter_items
            .iter()
            .find(|(_, drawn)| *drawn == pick)
            .expect("drawn");
        text_in(&render, *rect)
            .split_whitespace()
            .last()
            .map(str::to_owned)
    };
    assert_eq!(
        count(FilterPick::Kind(WorkKind::Feature)).as_deref(),
        Some("10")
    );
    assert_eq!(count(FilterPick::Kind(WorkKind::Bug)).as_deref(), Some("1"));
    assert_eq!(
        count(FilterPick::Kind(WorkKind::Chore)).as_deref(),
        Some("1")
    );
    board.work_filter_menu = false;
    board.work_filter.toggle(FilterPick::Kind(WorkKind::Chore));
    let render = render_board(&board, 186, 46);
    assert_eq!(render.hits.cards.len(), 1, "{:#?}", render.rows);
    assert_eq!(render.hits.cards[0].1, phases_card("build-on-track"));
    render.row_with("▣ chore · ");
}

#[test]
fn each_owner_bucket_follows_the_cards_holder_and_a_card_with_none_is_in_none() {
    use crate::client::shell::task_board::card_owner;
    let list = parse_task_list(WORK_PHASES).expect("phases fixture");
    let owner = |name: &str, agents: &[BoardAgent]| {
        card_owner(phases_card_of(&list, name), &list.tasks, agents)
    };
    // Its open task's owner You is Me; an agent on it is Agents; a named owner is Others.
    assert_eq!(owner("2026-09-29-review-open", &[]), Some(CardOwner::Me));
    assert_eq!(
        owner("2026-09-29-build-today", &[]),
        Some(CardOwner::Agents)
    );
    assert_eq!(
        owner("2026-09-29-build-overdue", &[]),
        Some(CardOwner::Others)
    );
    // No linked task and no lead: in no bucket, never Others.
    assert_eq!(owner("2026-09-29-plan-pending", &[]), None);
    // Its space's lead holds it: Others.
    let lead = [working("w1:p1", "herdr", "herdr-lead", None)];
    assert_eq!(
        owner("2026-09-29-plan-pending", &lead),
        Some(CardOwner::Others)
    );
    // A pick of Others leaves out the card no one holds.
    let mut board = phases_board();
    board
        .work_filter
        .toggle(FilterPick::Owner(CardOwner::Others));
    let render = render_board(&board, 186, 46);
    let drawn: Vec<&String> = render.hits.cards.iter().map(|(_, file)| file).collect();
    assert_eq!(drawn, [&phases_card("build-overdue")]);
}

#[test]
fn picks_in_two_groups_and_the_search_combine() {
    // review-open is a feature waiting on Nadine: in Needs you and in Feature.
    let mut board = phases_board_with(|list| {
        for card in &mut list.work {
            if card.file.ends_with("plan-pending.md") {
                card.kind = WorkKind::Bug;
            }
        }
    });
    board
        .work_filter
        .toggle(FilterPick::Status(CardStatus::NeedsYou));
    board
        .work_filter
        .toggle(FilterPick::Kind(WorkKind::Feature));
    let render = render_board(&board, 186, 46);
    let mut drawn: Vec<String> = render
        .hits
        .cards
        .iter()
        .map(|(_, file)| file.clone())
        .collect();
    drawn.sort();
    assert_eq!(
        drawn,
        [phases_card("review-open"), phases_card("verify-waiting"),]
    );
    // Counts: each item counts what picking it shows, with the other groups' picks and the
    // search applied.
    board.work_filter_menu = true;
    board.filter = "review".into();
    let render = render_board(&board, 186, 46);
    let count = |pick: FilterPick| {
        let (rect, _) = render
            .hits
            .work_filter_items
            .iter()
            .find(|(_, drawn)| *drawn == pick)
            .expect("drawn");
        text_in(&render, *rect)
            .split_whitespace()
            .last()
            .map(str::to_owned)
    };
    // `review` keeps review-open, review-passed, review-none; of the features among them
    // one needs Nadine and two are on track; of those needing him, one is a feature.
    assert_eq!(
        count(FilterPick::Status(CardStatus::NeedsYou)).as_deref(),
        Some("1")
    );
    assert_eq!(
        count(FilterPick::Status(CardStatus::OnTrack)).as_deref(),
        Some("2")
    );
    assert_eq!(
        count(FilterPick::Kind(WorkKind::Feature)).as_deref(),
        Some("1")
    );
    assert_eq!(count(FilterPick::Kind(WorkKind::Bug)).as_deref(), Some("0"));
}

#[test]
fn a_pick_draws_its_pill_and_the_headings_count_of_how_many() {
    let mut board = phases_board();
    board
        .work_filter
        .toggle(FilterPick::Status(CardStatus::NeedsYou));
    let render = render_board(&board, 186, 46);
    let row = &render.rows[2];
    assert!(
        row.trim_end().ends_with("Status: Needs you ×   Filter ▾"),
        "{row:?}"
    );
    let (clear, pick) = render.hits.work_filter_pills[0];
    assert_eq!(text_in(&render, clear), "× ");
    assert_eq!(pick, Some(FilterPick::Status(CardStatus::NeedsYou)));
    assert_eq!(render.buffer[(clear.x, 2)].fg, render.palette.overlay1);
    let pill = render.column(2, " Status: ");
    assert_eq!(
        render.buffer[(pill, 2)].bg,
        render.buffer[(render.hits.work_filter_button.x, 2)].bg,
        "the button's fill"
    );
    assert_eq!(clear.right() + 1, render.hits.work_filter_button.x);
    let headings = &render.rows[3];
    for part in [
        "PLAN  1 of 2",
        "BUILD  0 of 4",
        "REVIEW  1 of 3",
        "VERIFY  1 of 2",
    ] {
        assert!(headings.contains(part), "{part:?} in {headings:?}");
    }
}

/// A board state showing the phases fixture's Phases view, drawn once at 186 by 46.
fn phases_state() -> ClientShellState {
    let mut state = work_state();
    state
        .task_board
        .store
        .receive_json(WORK_PHASES, std::time::Instant::now());
    state.task_board.view = BoardView::Work;
    state.compose(186, 46).expect("frame");
    state
}

#[test]
fn a_pill_clears_by_its_cross_by_its_pick_again_and_by_key() {
    let needs_you = FilterPick::Status(CardStatus::NeedsYou);
    // Its `×`.
    let mut state = phases_state();
    state.task_board.work_filter.toggle(needs_you);
    state.compose(186, 46).expect("frame");
    let (clear, _) = state.hits.task_board.work_filter_pills[0];
    board_click(&mut state, clear);
    assert!(state.task_board.work_filter.is_empty());
    assert!(!state.task_board.work_filter_menu, "a clear opens no menu");
    // The pick again, in the menu.
    state.task_board.work_filter.toggle(needs_you);
    state.task_board.work_filter_menu = true;
    state.compose(186, 46).expect("frame");
    let item = filter_item(&state, needs_you);
    board_click(&mut state, item);
    assert!(state.task_board.work_filter.is_empty());
    assert!(
        state.task_board.work_filter_menu,
        "the menu stays for another pick"
    );
    // Enter on it in the menu.
    state.task_board.work_filter.toggle(needs_you);
    state.task_board.work_filter_cursor = 0;
    press_key(&mut state, KeyCode::Enter);
    assert!(state.task_board.work_filter.is_empty());
    // Esc outside the menu clears every pick, and the next Esc closes the board.
    press_key(&mut state, KeyCode::Esc);
    assert!(!state.task_board.work_filter_menu);
    state.task_board.work_filter.toggle(needs_you);
    state
        .task_board
        .work_filter
        .toggle(FilterPick::Kind(WorkKind::Bug));
    press_key(&mut state, KeyCode::Esc);
    assert!(state.task_board.work_filter.is_empty());
    assert!(state.task_board.open);
    press_key(&mut state, KeyCode::Esc);
    assert!(!state.task_board.open);
}

#[test]
fn f_opens_the_filter_menu_and_its_keys_move_pick_and_close_it() {
    let mut state = phases_state();
    press_key(&mut state, KeyCode::Char('f'));
    assert!(state.task_board.work_filter_menu);
    press_key(&mut state, KeyCode::Down);
    press_key(&mut state, KeyCode::Char('j'));
    press_key(&mut state, KeyCode::Up);
    assert_eq!(state.task_board.work_filter_cursor, 1);
    for _ in 0..20 {
        press_key(&mut state, KeyCode::Down);
    }
    assert_eq!(
        state.task_board.work_filter_cursor,
        FilterPick::ALL.len() - 1,
        "stops at the last"
    );
    press_key(&mut state, KeyCode::Enter);
    assert_eq!(state.task_board.work_filter.owner, Some(CardOwner::Others));
    assert!(state.task_board.work_filter_menu, "Enter keeps it open");
    press_key(&mut state, KeyCode::Char(' '));
    assert_eq!(
        state.task_board.work_filter.owner, None,
        "Space picks as Enter"
    );
    press_key(&mut state, KeyCode::Esc);
    assert!(!state.task_board.work_filter_menu);
    assert!(state.task_board.open, "Esc only closed the menu");
    // `f` again closes it, as the key that opened it.
    press_key(&mut state, KeyCode::Char('f'));
    assert!(state.task_board.work_filter_menu);
    press_key(&mut state, KeyCode::Char('f'));
    assert!(!state.task_board.work_filter_menu, "f toggles it closed");
    assert!(state.task_board.open);
    // Any other key closes it and does its own: Tab switches the view.
    press_key(&mut state, KeyCode::Char('f'));
    press_key(&mut state, KeyCode::Tab);
    assert!(!state.task_board.work_filter_menu);
    assert_eq!(state.task_board.view, BoardView::Tasks);
    // In Tasks, f opens nothing.
    press_key(&mut state, KeyCode::Char('f'));
    assert!(!state.task_board.work_filter_menu);
    // A click on the button opens it; one outside closes it.
    press_key(&mut state, KeyCode::Tab);
    state.compose(186, 46).expect("frame");
    let button = state.hits.task_board.work_filter_button;
    board_click(&mut state, button);
    assert!(state.task_board.work_filter_menu);
    state.compose(186, 46).expect("frame");
    let area = state.hits.task_board.area;
    board_click(&mut state, Rect::new(area.x + 2, area.y + 20, 1, 1));
    assert!(!state.task_board.work_filter_menu);
    // The open menu takes a click on the Tasks tab as it takes any: it only closes.
    state.task_board.work_filter_menu = true;
    state.compose(186, 46).expect("frame");
    let tasks_tab = state.hits.task_board.tasks_tab;
    board_click(&mut state, tasks_tab);
    assert!(!state.task_board.work_filter_menu);
    assert_eq!(state.task_board.view, BoardView::Work);
    // `?` names the key in Phases.
    state.task_board.keys_shown = true;
    let rows = frame_rows(&state.compose(186, 46).expect("frame"));
    assert!(
        rows.iter().any(|row| row.contains(" f  filter")),
        "{rows:#?}"
    );
}

#[test]
fn a_pills_cross_clears_its_pick_while_the_menu_is_open() {
    let mut state = phases_state();
    state
        .task_board
        .work_filter
        .toggle(FilterPick::Status(CardStatus::NeedsYou));
    state.task_board.work_filter_menu = true;
    state.compose(186, 46).expect("frame");
    let (clear, _) = state.hits.task_board.work_filter_pills[0];
    board_click(&mut state, clear);
    assert!(state.task_board.work_filter.is_empty(), "the pick is gone");
    assert!(!state.task_board.work_filter_menu, "and the menu closed");
}

#[test]
fn f_opens_no_filter_menu_before_its_button_is_drawn() {
    // No list read yet: the chip row and its Filter button are not drawn.
    let mut state = empty_board_state();
    state.task_board.open = true;
    state.task_board.view = BoardView::Work;
    state.compose(186, 46).expect("frame");
    assert!(state.hits.task_board.work_filter_button.is_empty());
    press_key(&mut state, KeyCode::Char('f'));
    assert!(!state.task_board.work_filter_menu, "f opens nothing unseen");
    press_key(&mut state, KeyCode::Down);
    assert_eq!(state.task_board.work_filter_cursor, 0, "no menu takes ↓");
    // Once the list is read and the button drawn, f opens it.
    state
        .task_board
        .store
        .receive_json(WORK_PHASES, std::time::Instant::now());
    state.compose(186, 46).expect("frame");
    press_key(&mut state, KeyCode::Char('f'));
    assert!(state.task_board.work_filter_menu);
}

#[test]
fn a_narrow_phases_chip_row_shrinks_the_pills_before_it_hides_a_pick() {
    let mut board = phases_board();
    board
        .work_filter
        .toggle(FilterPick::Status(CardStatus::NeedsYou));
    board
        .work_filter
        .toggle(FilterPick::Kind(WorkKind::Feature));
    board
        .work_filter
        .toggle(FilterPick::Owner(CardOwner::Agents));
    let row = |width: u16| render_board(&board, width, 46).rows[2].trim().to_owned();
    assert_eq!(
        row(186),
        "Status: Needs you ×   Kind: Feature ×   Owner: Agents ×   Filter ▾"
    );
    assert_eq!(row(60), "Needs you ×   Feature ×   Agents ×   Filter ▾");
    assert_eq!(row(42), "Needs you ×   Feature ×   Agents ×   ▾");
    assert_eq!(row(40), "3 filters ×   ▾");
    let mut one = phases_board();
    one.work_filter
        .toggle(FilterPick::Status(CardStatus::NeedsYou));
    assert_eq!(
        render_board(&one, 40, 46).rows[2].trim(),
        "Status: Needs you ×   Filter ▾"
    );
    for width in (40u16..=186).step_by(3) {
        let render = render_board(&board, width, 46);
        let mut parts: Vec<Rect> = render
            .hits
            .work_filter_pills
            .iter()
            .map(|(rect, _)| *rect)
            .collect();
        parts.push(render.hits.work_filter_button);
        assert!(
            !render.hits.work_filter_pills.is_empty(),
            "{width}: a pick shows"
        );
        for pair in parts.windows(2) {
            assert!(pair[0].right() < pair[1].x, "{width}: {parts:?}");
        }
        assert_eq!(render.hits.work_filter_button.right(), width - 1, "{width}");
        let first = render.rows[2].find(|c: char| c != ' ').unwrap_or(0);
        assert!(first >= 1, "{width}: inside the margin");
    }
    // The summed pill clears every pick.
    let mut state = phases_state();
    state.task_board.work_filter = board.work_filter;
    state.compose(40, 46).expect("frame");
    let (clear, pick) = state.hits.task_board.work_filter_pills[0];
    assert_eq!(pick, None);
    board_click(&mut state, clear);
    assert!(state.task_board.work_filter.is_empty());
}

/// Asserts `needle` on line `line` of card `name` (0 is its kind line) is drawn in `fg`, not bold,
/// found inside the card's own box.
fn assert_card_words(
    render: &BoardRender,
    (name, line): (&str, u16),
    needle: &str,
    fg: ratatui::style::Color,
) {
    let (rect, _) = render
        .hits
        .cards
        .iter()
        .find(|(_, card)| *card == phases_card(name))
        .unwrap_or_else(|| panic!("{name} drawn"));
    let y = rect.y + 1 + line;
    let cells: Vec<(u16, String)> = (rect.x + 1..rect.right() - 1)
        .map(|x| (x, render.buffer[(x, y)].symbol().to_owned()))
        .collect();
    let row: String = cells.iter().map(|(_, symbol)| symbol.as_str()).collect();
    let at = row
        .find(needle)
        .unwrap_or_else(|| panic!("{name}: {needle:?} not in {row:?}"));
    let start = UnicodeWidthStr::width(&row[..at]);
    for (x, _) in cells
        .iter()
        .skip(start)
        .take(UnicodeWidthStr::width(needle))
    {
        let cell = &render.buffer[(*x, y)];
        assert_eq!(
            (cell.fg, cell.modifier.contains(Modifier::BOLD)),
            (fg, false),
            "{name}: {needle:?} at column {x}: {row:?}"
        );
    }
}

#[test]
fn a_card_status_line_is_its_mark_and_words_in_the_marks_colour_never_bold() {
    let board = phases_board_with(|list| {
        for card in &mut list.work {
            if card.file.ends_with("build-on-track.md") {
                card.due_text = Some("Thu".into());
                card.due_level = Some(DueLevel::Soon);
            }
            if card.file.ends_with("review-none.md") {
                card.status = "Blocked".into();
            }
        }
    });
    for palette in [Palette::catppuccin(), Palette::vesper()] {
        let render = render_board_on(&board, 186, 90, &[], palette.clone());
        let status = |name: &str| card_text(&render, &phases_card(name))[6].clone();
        for (name, line, colour) in [
            ("plan-pending", "? Awaiting your approval", palette.yellow),
            ("verify-waiting", "? Awaiting your check", palette.yellow),
            ("plan-approved", "✓ Approved", palette.green),
            ("review-passed", "✓ Review passed", palette.green),
            ("verify-done", "✓ Verified", palette.green),
            ("ship", "✓ Shipped 28 Sep", palette.green),
            ("build-overdue", "! Overdue 3d", palette.red),
            ("build-today", "! Due today", palette.yellow),
            ("review-none", "✗ Blocked", palette.red),
            ("all-null", "· On track", palette.overlay1),
            ("build-on-track", "· Due Thu", palette.overlay1),
            ("review-open", "· 2 open findings", palette.overlay1),
        ] {
            assert_eq!(status(name), line, "{name}");
            assert_card_words(&render, (name, 6), line, colour);
        }
    }
}

#[test]
fn a_blocked_card_says_blocked_in_any_case_and_only_from_its_status() {
    for word in ["blocked", "BLOCKED", " Blocked "] {
        let board = phases_board_with(|list| {
            for card in &mut list.work {
                if card.file.ends_with("build-overdue.md") {
                    card.status = word.into();
                }
            }
        });
        let render = render_board(&board, 186, 90);
        assert_eq!(
            card_text(&render, &phases_card("build-overdue"))[6],
            "✗ Blocked",
            "{word:?}"
        );
    }
    // Late but open: its deadline, not blocked.
    let render = render_board(&phases_board(), 186, 90);
    assert_eq!(
        card_text(&render, &phases_card("build-overdue"))[6],
        "! Overdue 3d"
    );
}

#[test]
fn a_status_line_is_cut_at_a_narrow_card_never_wrapped() {
    // Five columns 14 wide: a card's inside is 10 columns.
    let render = render_board(&phases_board(), 80, 90);
    let lines = card_text(&render, &phases_card("plan-pending"));
    assert!(
        lines[6].starts_with("? Await") && lines[6].ends_with('…'),
        "{lines:?}"
    );
    assert_eq!(lines.len(), 7, "{lines:?}");
    assert_card_words(
        &render,
        ("plan-pending", 6),
        "? Await",
        render.palette.yellow,
    );
}

#[test]
fn a_cards_who_line_is_grey_whoever_holds_it_and_only_the_spinner_colours() {
    let owned = |owner: &'static str| {
        phases_board_with(move |list| list.tasks[0].owner = Some(owner.into()))
    };
    for owner in ["You", "you", " YOU "] {
        let render = render_board(&owned(owner), 186, 90);
        assert_eq!(
            card_text(&render, &phases_card("review-open"))[5],
            "You",
            "{owner:?}: no ? on the who line"
        );
        assert_card_words(&render, ("review-open", 5), "You", render.palette.overlay1);
    }
    let render = render_board(&phases_board(), 186, 90);
    assert_card_words(
        &render,
        ("build-overdue", 5),
        "Carmen",
        render.palette.overlay1,
    );
    assert_card_words(
        &render,
        ("plan-pending", 5),
        "herdr",
        render.palette.overlay1,
    );
    let render = render_board_with_agents(
        &work_board(),
        WIDE,
        40,
        &[working("w36:pV", "herdr", "herdr-w1", None)],
    );
    let who = render.row_with(&format!("{WORKER_SPINNER} herdr-w1"));
    assert_drawn(
        &render,
        (who, WORKER_SPINNER),
        working_colour(&render.palette),
        false,
    );
    assert_drawn(&render, (who, "herdr-w1"), render.palette.overlay1, false);
}

#[test]
fn a_cards_progress_count_is_grey_and_its_title_text_not_bold() {
    for palette in [Palette::catppuccin(), Palette::vesper()] {
        let render = render_board_on(&phases_board(), 186, 90, &[], palette.clone());
        let lines = card_text(&render, &phases_card("build-overdue"));
        let count = lines[4]
            .rsplit(' ')
            .next()
            .unwrap_or_else(|| panic!("{lines:?}"))
            .to_owned();
        assert!(count.contains('/'), "{lines:?}");
        assert_card_words(&render, ("build-overdue", 4), &count, palette.overlay1);
        assert_card_words(&render, ("build-overdue", 4), "━", palette.red);
        assert_card_words(&render, ("build-overdue", 2), &lines[2], palette.text);
    }
}

#[test]
fn the_panel_says_the_cards_status_as_its_card_does_and_its_owner_grey() {
    let (panel, inside) = phases_panel("verify-waiting", WIDE);
    let row = |inside: &[(usize, String)], label: &str| {
        inside
            .iter()
            .find(|(_, words)| words.starts_with(label))
            .unwrap_or_else(|| panic!("{label}: {inside:#?}"))
            .0
    };
    let status = row(&inside, "Status");
    assert_drawn(
        &panel,
        (status, "? Awaiting your check"),
        panel.palette.yellow,
        false,
    );
    let heading = usize::from(panel.hits.work_panel.y) + 1;
    assert!(
        panel.rows[heading].contains("VERIFY · ? Awaiting your check"),
        "{:?}",
        panel.rows[heading]
    );
    assert_drawn(
        &panel,
        (heading, "? Awaiting your check"),
        panel.palette.yellow,
        false,
    );
    let (panel, inside) = phases_panel("build-overdue", WIDE);
    assert_drawn(
        &panel,
        (row(&inside, "Status"), "! Overdue 3d"),
        panel.palette.red,
        false,
    );
    assert_drawn(
        &panel,
        (row(&inside, "Owner"), "Carmen"),
        panel.palette.overlay1,
        false,
    );
}

#[test]
fn a_done_step_ticks_teal_the_colour_for_done_work() {
    let (panel, inside) = phases_panel("build-overdue", WIDE);
    let (row, words) = inside
        .iter()
        .find(|(_, words)| words.starts_with("✓"))
        .unwrap_or_else(|| panic!("a done step: {inside:#?}"));
    assert_drawn(&panel, (*row, "✓"), panel.palette.teal, false);
    assert!(!words.is_empty());
}

#[test]
fn a_bug_is_red_and_a_late_bug_still_reads_apart_by_its_glyph_word_and_line() {
    let board = phases_board_with(|list| {
        for card in &mut list.work {
            if card.file.ends_with("build-overdue.md") {
                card.kind = WorkKind::Bug;
            }
        }
    });
    for palette in [Palette::catppuccin(), Palette::vesper()] {
        let render = render_board_on(&board, 186, 90, &[], palette.clone());
        let lines = card_text(&render, &phases_card("build-overdue"));
        // The kind says what it is on its own line, the deadline what is late on the last.
        assert!(lines[0].starts_with("✱ bug"), "{lines:?}");
        assert_eq!(lines[6], "! Overdue 3d", "{lines:?}");
        assert_card_words(&render, ("build-overdue", 0), "✱ bug", palette.red);
        assert_card_words(&render, ("build-overdue", 6), "! Overdue 3d", palette.red);
        // A feature beside it keeps its lilac.
        assert_card_words(
            &render,
            ("build-today", 0),
            "◆ feature",
            crate::client::shell::task_board::style::lilac(&palette),
        );
    }
}

#[test]
fn every_phases_column_head_takes_its_stage_colour_review_rose() {
    use crate::client::shell::task_board::style::{lilac, rose};
    for palette in [Palette::catppuccin(), Palette::vesper()] {
        let render = render_board_on(&phases_board(), 186, 90, &[], palette.clone());
        for (label, colour) in [
            ("PLAN", lilac(&palette)),
            ("BUILD", palette.yellow),
            ("REVIEW", rose(&palette)),
            ("VERIFY", palette.blue),
            ("SHIP", palette.green),
        ] {
            let row = render.row_with(label);
            assert_eq!(render.cell(row, label).fg, colour, "{label}");
        }
    }
}

#[test]
fn a_board_agent_waiting_on_a_yes_or_no_draws_a_red_bang_and_one_asking_a_yellow_question() {
    use crate::api::schema::AgentStatus;
    use crate::client::shell::task_board::board_agents;
    let mut snap = snapshot();
    snap.workspaces = vec![workspace("ws_h", "herdr", "/h")];
    snap.agents = vec![
        live_agent("w1:p1", "ws_h", "herdr-yes", AgentStatus::Blocked, &[]),
        live_agent(
            "w1:p2",
            "ws_h",
            "herdr-ask",
            AgentStatus::Blocked,
            &[("ask", "Which branch?")],
        ),
    ];
    let config = ClientShellConfig::from_config(&Config::default());
    let agents = board_agents(Some(&snap), &config, None, 0);
    let palette = &config.palette;
    assert_eq!(agents[0].mark, ("!", palette.red), "a yes or no");
    assert_eq!(agents[1].mark, ("?", palette.yellow), "a question");
    let mut board = tasks_and_work_board();
    board.space = Some("herdr".into());
    board.agent_menu = true;
    let render = render_board_with_agents(&board, 186, 46, &agents);
    for (index, glyph, colour) in [(1, "!", palette.red), (2, "?", palette.yellow)] {
        let (rect, _) = &render.hits.agent_items[index];
        let y = rect.y;
        let x = (rect.x..rect.right())
            .find(|x| render.buffer[(*x, y)].symbol() == glyph)
            .unwrap_or_else(|| panic!("{glyph} in {:?}", render.rows[usize::from(y)]));
        assert_eq!(render.buffer[(x, y)].fg, colour, "{glyph}");
    }
}
