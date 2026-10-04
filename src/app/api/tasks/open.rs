//! `tasks.open`: opens a file the kept task list names, read-only, in a popup filling the
//! board's area running `less` (its colours from bat when bat is found), whose prompt line
//! says `q` goes back and `v` edits in `$EDITOR`; without `less` on `PATH`, `$EDITOR` itself.
//! The file is the popup's last argument, never part of a shell string Herdr builds; with bat,
//! `less` puts it in its own filter command quoted by itself, which a test with a hostile
//! name pins; a `less` too old to quote it opens the file without bat. A file the list does
//! not name is refused, so a client cannot open an arbitrary path.

use std::path::Path;

use ratatui::style::Color;

use crate::api::schema::{ResponseResult, TasksOpenParams};
use crate::app::state::Palette;
use crate::app::App;

use super::super::responses::{encode_error, encode_success};

impl App {
    pub(crate) fn handle_tasks_open(
        &mut self,
        request_id: String,
        params: TasksOpenParams,
    ) -> String {
        let editor = std::env::var("EDITOR").ok();
        let bat = crate::integration::command_available("bat");
        let less = crate::integration::command_available("less");
        let readers = Readers {
            bat,
            less,
            less_quotes: installed_less_quotes_lessopen,
        };
        self.open_listed_task_file(request_id, &params.file, readers, editor.as_deref())
    }

    /// Whether `tasks.open` may open `file`: the kept list names it. Otherwise the refusal's
    /// message, which `tasks.open` replies with the code `tasks_not_listed`.
    pub(crate) fn check_task_file_listed(&self, file: &str) -> Result<(), String> {
        let Some(list) = self.tasks_list.kept_list() else {
            return Err("no task list is kept; read it with tasks.list first".to_owned());
        };
        if !super::listed_files(list).any(|listed| listed == file) {
            return Err(format!("{file} is not a file in the task list"));
        }
        Ok(())
    }

    /// Opens `file` in a popup in the focused space when the kept list names it: in `less`,
    /// coloured by bat when both are found, else in `editor`.
    pub(super) fn open_listed_task_file(
        &mut self,
        request_id: String,
        file: &str,
        readers: Readers,
        editor: Option<&str>,
    ) -> String {
        if let Err(message) = self.check_task_file_listed(file) {
            return encode_error(request_id, "tasks_not_listed", message);
        }
        let folder = Path::new(file)
            .parent()
            .filter(|folder| !folder.as_os_str().is_empty())
            .map(Path::to_path_buf);
        let launch = open_command(readers, editor, file, &self.state.palette);
        let mut env = self.custom_command_env().0;
        env.extend(launch.env);
        if let Err(err) = self.spawn_popup_argv_command(
            &launch.argv,
            folder,
            env,
            // The whole board area, so a notes or intent file reads like a page.
            crate::app::popup::PopupGeometry {
                width: Some(crate::popup_size::PopupSize::Percent(100)),
                height: Some(crate::popup_size::PopupSize::Percent(100)),
            },
        ) {
            return encode_error(request_id, "tasks_open_failed", err.to_string());
        }
        encode_success(request_id, ResponseResult::Ok {})
    }
}

/// The programs found on `PATH` that `tasks.open` reads a file with.
#[derive(Debug, Clone, Copy)]
pub(super) struct Readers {
    pub(super) bat: bool,
    pub(super) less: bool,
    /// Whether the `less` found quotes the file name it puts for LESSOPEN's `%s`: asked only
    /// when bat is found too, for a file the list names, as it may run `less`.
    pub(super) less_quotes: fn() -> bool,
}

/// `less`'s prompt line: `(END)` at the end, then the way back and the keys a reader needs,
/// spaced as the board's footer shows its keys (each key as its block, one space, its words,
/// four columns to the next block). `less` itself runs `$EDITOR` on `v`; a lone Esc cannot
/// quit it, because its own commands start with Esc.
const LESS_PROMPT: &str =
    "-Ps?e(END) . q  back to board     v  edit     /  search     Space  next page ";

/// The command that prints the popup's size as `rows cols`: `less` gives its filter no
/// `COLUMNS`, but the filter runs on the popup's own terminal.
const TERMINAL_SIZE: &str = "stty size </dev/tty 2>/dev/null";

/// bat as `less`'s input filter: `less` opens the file itself, so `v` edits it (through bat's
/// own pager `less` reads a pipe and cannot), and bat only colours what `less` shows. `less`
/// quotes the file name it puts for `%s`. bat has no terminal of its own (its output is
/// `less`'s pipe), so it wraps at the popup's width, read from the terminal it runs on: a long
/// line goes on under its own text, clear of the line numbers. Without a size it never wraps,
/// as 80 columns would cut a colour code in its header in two. DarkNeon is the installed bat
/// theme closest to Vesper's colours.
fn bat_lessopen() -> String {
    format!(
        "|cols=$({TERMINAL_SIZE}); cols=${{cols##* }}; bat --color=always --paging=never \
         --wrap=character --terminal-width=${{cols:-1000}} --style=numbers,header \
         --theme=DarkNeon -- %s"
    )
}

/// The first `less` release that quotes the file name it puts for LESSOPEN's `%s`: less's
/// NEWS lists "Fix some bugs handling filenames containing shell metacharacters" among the
/// changes from 487 to 530, which is commit 918c307 "Fix bug using LESSOPEN with filenames
/// containing shell metachars" (beta v516, released in 530). An older one puts the name in
/// the filter's shell command bare.
const LESS_QUOTES_LESSOPEN: u32 = 530;

/// Whether `less --version`'s output names a `less` that quotes the file name for `%s`: its
/// first line is `less <version> ...`; anything else is not known to.
fn quotes_lessopen_names(version: &str) -> bool {
    let mut words = version
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace();
    let number = match (words.next(), words.next()) {
        (Some("less"), Some(number)) => number,
        _ => return false,
    };
    let digits = number
        .find(|c: char| !c.is_ascii_digit())
        .map_or(number, |end| &number[..end]);
    digits
        .parse::<u32>()
        .is_ok_and(|version| version >= LESS_QUOTES_LESSOPEN)
}

/// Whether the `less` on `PATH` quotes the file name for LESSOPEN, read from `less --version`
/// once per server process; a `less` whose version cannot be read is taken as one that does
/// not.
fn installed_less_quotes_lessopen() -> bool {
    #[cfg(test)]
    tests::VERSION_ASKS.with(|asks| asks.set(asks.get() + 1));
    static QUOTES: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *QUOTES.get_or_init(|| {
        let output = std::process::Command::new("less")
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output();
        let version = match output {
            Ok(output) if output.status.success() => {
                String::from_utf8_lossy(&output.stdout).into_owned()
            }
            Ok(output) => {
                tracing::warn!(
                    status = %output.status,
                    "less --version failed; files open without bat"
                );
                return false;
            }
            Err(err) => {
                tracing::warn!(%err, "less --version did not run; files open without bat");
                return false;
            }
        };
        let quotes = quotes_lessopen_names(&version);
        if !quotes {
            tracing::info!(
                version = version.lines().next().unwrap_or_default(),
                "this less does not quote file names for LESSOPEN; files open without bat"
            );
        }
        quotes
    })
}

/// What the popup runs: its argv, and the environment it adds to the popup's own.
#[derive(Debug, PartialEq, Eq)]
struct Launch {
    argv: Vec<String>,
    env: Vec<(String, String)>,
}

/// The popup's argv and the environment it adds. With `less` found: `less` showing colours,
/// with the mouse wheel and the prompt line in the board's colours, then the file after
/// `--`; coloured by bat when bat is found too and that `less` quotes the file name for
/// bat's command. Without: the editor's words (split on spaces, with no shell), then the
/// file; `less` when `EDITOR` is unset or blank.
fn open_command(readers: Readers, editor: Option<&str>, file: &str, palette: &Palette) -> Launch {
    if readers.less {
        let bar = prompt_colors(palette);
        let argv = [
            "less",
            "-R",
            "--mouse",
            "--use-color",
            &format!("-DP{bar}"),
            &format!("-DE{bar}"),
            LESS_PROMPT,
            "--",
            file,
        ]
        .map(str::to_owned)
        .to_vec();
        let env = (readers.bat && (readers.less_quotes)())
            .then(|| ("LESSOPEN".to_owned(), bat_lessopen()))
            .into_iter()
            .collect();
        return Launch { argv, env };
    }
    let mut argv: Vec<String> = editor
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    if argv.is_empty() {
        argv.push("less".to_owned());
    }
    argv.push(file.to_owned());
    Launch {
        argv,
        env: Vec::new(),
    }
}

/// less's colours for its prompt bar and its messages, `fg.bg`: the board footer's grey words
/// on the theme's dark band. less takes 256-colour numbers, not RGB, so each is the nearest
/// one; `-` keeps less's own for a colour the theme leaves to the terminal.
fn prompt_colors(palette: &Palette) -> String {
    let number = |color| color_number(color).map_or_else(|| "-".to_owned(), |n| n.to_string());
    format!("{}.{}", number(palette.overlay1), number(palette.surface0))
}

/// `color`'s number among the terminal's 256 colours: an RGB colour's nearest of the
/// 6x6x6 cube and the grey ramp (the first 16 are the terminal theme's own, so never
/// matched), a numbered or named colour its own number, none for the terminal's default.
fn color_number(color: Color) -> Option<u8> {
    let named = match color {
        Color::Reset => return None,
        Color::Indexed(number) => return Some(number),
        Color::Rgb(r, g, b) => {
            let fixed = crate::ghostty::default_palette();
            let distance = |index: usize| {
                let c = fixed[index];
                let d = |a: u8, b: u8| (i32::from(a) - i32::from(b)).pow(2);
                d(c.r, r) + d(c.g, g) + d(c.b, b)
            };
            return (16..256)
                .min_by_key(|index| distance(*index))
                .map(|index| index as u8);
        }
        Color::Black => 0,
        Color::Red => 1,
        Color::Green => 2,
        Color::Yellow => 3,
        Color::Blue => 4,
        Color::Magenta => 5,
        Color::Cyan => 6,
        Color::Gray => 7,
        Color::DarkGray => 8,
        Color::LightRed => 9,
        Color::LightGreen => 10,
        Color::LightYellow => 11,
        Color::LightBlue => 12,
        Color::LightMagenta => 13,
        Color::LightCyan => 14,
        Color::White => 15,
    };
    Some(named)
}

#[cfg(test)]
mod tests {
    use crate::api::schema::{ErrorResponse, Method, Request, SuccessResponse, TasksOpenParams};
    use crate::app::App;

    fn test_app() -> App {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &crate::config::Config::default(),
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        app.state.workspaces = vec![crate::workspace::Workspace::test_new("tasks")];
        app.state.ensure_test_terminals();
        app.state.active = Some(0);
        app
    }

    fn fixture(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/app/api/tasks/fixtures")
            .join(name)
    }

    /// A task list naming `work_file` as a work item and `copy_file` as a task's copy.
    fn list(work_file: &str, copy_file: &str) -> serde_json::Value {
        serde_json::json!({
            "version": 1,
            "tasks": [{"task": "t", "copies": [{"file": copy_file, "line": 3, "row": "| t | me |"}]}],
            "work": [{"title": "w", "file": work_file}],
            "problems": [],
        })
    }

    fn open_request(file: &str) -> Request {
        Request {
            id: "open".into(),
            method: Method::TasksOpen(TasksOpenParams { file: file.into() }),
        }
    }

    fn error_of(response: &str) -> (String, String) {
        let response: ErrorResponse = serde_json::from_str(response)
            .unwrap_or_else(|err| panic!("not an error response ({err}): {response}"));
        (response.error.code, response.error.message)
    }

    #[test]
    fn tasks_open_request_has_the_documented_shape() {
        let request: Request = serde_json::from_value(serde_json::json!({
            "id": "1",
            "method": "tasks.open",
            "params": {"file": "/notes/STATUS.md"},
        }))
        .unwrap();
        assert_eq!(request.method, open_request("/notes/STATUS.md").method);
    }

    #[test]
    fn tasks_open_without_a_kept_list_is_refused() {
        let mut app = test_app();
        let response = app.handle_api_request(open_request("/notes/STATUS.md"));
        assert_eq!(
            error_of(&response),
            (
                "tasks_not_listed".to_owned(),
                "no task list is kept; read it with tasks.list first".to_owned()
            )
        );
        assert!(app.state.popup_pane.is_none());
        assert_eq!(version_asks(), 0, "a refused open runs no less");
    }

    #[test]
    fn tasks_open_refuses_a_file_the_list_does_not_name() {
        let mut app = test_app();
        app.tasks_list
            .keep_list_for_test(list("/notes/intent.md", "/notes/STATUS.md"));
        for file in [
            "/etc/passwd",
            "/notes",
            "/notes/STATUS.md/",
            "notes/STATUS.md",
        ] {
            let response = app.handle_api_request(open_request(file));
            assert_eq!(
                error_of(&response),
                (
                    "tasks_not_listed".to_owned(),
                    format!("{file} is not a file in the task list")
                )
            );
        }
        assert!(app.state.popup_pane.is_none());
        assert_eq!(version_asks(), 0, "a refused open runs no less");
    }

    use super::Readers;

    thread_local! {
        /// How often this test's thread asked for the installed `less`'s version.
        pub(super) static VERSION_ASKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }

    fn version_asks() -> usize {
        VERSION_ASKS.with(std::cell::Cell::get)
    }

    /// A `less` that quotes the file name for LESSOPEN, and one too old to.
    fn a_new_less() -> bool {
        true
    }

    fn an_old_less() -> bool {
        false
    }

    /// Where the version must not be read: without bat, or without `less`.
    fn not_asked() -> bool {
        panic!("the less version is read only when bat colours the file")
    }

    const BOTH: Readers = Readers {
        bat: true,
        less: true,
        less_quotes: a_new_less,
    };

    /// His theme, which the prompt bar's colours come from.
    fn vesper() -> crate::app::state::Palette {
        crate::app::state::Palette::vesper()
    }

    #[test]
    fn tasks_open_reads_the_file_in_less_with_the_way_back_on_its_prompt_line() {
        let file = "/notes/-it's a \"file\".md";
        for readers in [
            BOTH,
            Readers {
                bat: false,
                less: true,
                less_quotes: not_asked,
            },
        ] {
            let argv = super::open_command(readers, Some("vim"), file, &vesper()).argv;
            assert_eq!(argv[0], "less", "{argv:?}");
            assert!(argv.iter().any(|arg| arg == "--mouse"), "{argv:?}");
            let prompt = argv
                .iter()
                .find(|arg| arg.starts_with("-Ps"))
                .unwrap_or_else(|| panic!("a prompt line: {argv:?}"));
            // The board footer's text: each key as its block shows it, one space, its words,
            // and four columns to the next key's block; a space each end of the bar.
            assert!(
                prompt.ends_with(
                    ". q  back to board     v  edit     /  search     Space  next page "
                ),
                "{prompt:?}"
            );
            assert_eq!(
                argv[argv.len() - 2..],
                ["--", file],
                "the file is never an option"
            );
        }
    }

    #[test]
    fn tasks_open_colours_the_file_with_bat_inside_less_so_v_still_edits() {
        // less opens the file itself, so `v` edits it; bat only colours what less shows.
        let file = "/notes/STATUS.md";
        let super::Launch { argv, env } = super::open_command(BOTH, None, file, &vesper());
        assert!(
            argv.iter().any(|arg| arg == "-R"),
            "less shows bat's colours: {argv:?}"
        );
        assert_eq!(argv.last().map(String::as_str), Some(file));
        assert_eq!(
            env,
            [(
                "LESSOPEN".to_owned(),
                "|cols=$(stty size </dev/tty 2>/dev/null); cols=${cols##* }; bat --color=always \
                 --paging=never --wrap=character --terminal-width=${cols:-1000} \
                 --style=numbers,header --theme=DarkNeon -- %s"
                    .to_owned()
            )]
        );
        let env = super::open_command(
            Readers {
                bat: false,
                less: true,
                less_quotes: not_asked,
            },
            None,
            file,
            &vesper(),
        )
        .env;
        assert!(env.is_empty(), "without bat, plain less: {env:?}");
    }

    #[test]
    fn only_a_less_that_quotes_the_file_name_gets_bat_as_its_filter() {
        // less 530 is the first release that quotes the name it puts for LESSOPEN's `%s`.
        for (version, quotes) in [
            (
                "less 710 (PCRE2 regular expressions)\nCopyright (C) 1984-2026",
                true,
            ),
            ("less 668 (POSIX regular expressions)\n", true),
            ("less 530 (POSIX regular expressions)", true),
            ("less 581.2 (PCRE regular expressions)", true),
            ("less 529", false),
            ("less 487 (POSIX regular expressions)", false),
            ("BusyBox v1.36.1 (2023-11-07) multi-call binary.", false),
            ("less", false),
            ("", false),
        ] {
            assert_eq!(super::quotes_lessopen_names(version), quotes, "{version:?}");
        }
        // An older or unreadable less opens the file plain, still itself, so `v` edits it.
        let file = "/notes/STATUS.md";
        let old_less = Readers {
            less_quotes: an_old_less,
            ..BOTH
        };
        let launch = super::open_command(old_less, None, file, &vesper());
        assert!(launch.env.is_empty(), "no bat filter: {:?}", launch.env);
        assert_eq!(launch.argv[0], "less");
        assert_eq!(launch.argv[launch.argv.len() - 2..], ["--", file]);
    }

    #[test]
    fn tasks_open_without_less_uses_the_editor_words_then_the_file() {
        let file = "/notes/it's a \"file\".md";
        // bat pages through less, so without less bat is no help either.
        for bat in [false, true] {
            let readers = Readers {
                bat,
                less: false,
                less_quotes: not_asked,
            };
            let argv = |editor| super::open_command(readers, editor, file, &vesper());
            assert_eq!(
                argv(None),
                super::Launch {
                    argv: vec!["less".to_owned(), file.to_owned()],
                    env: vec![]
                }
            );
            assert_eq!(argv(Some("  ")).argv, ["less", file]);
            assert_eq!(argv(Some("nvim")).argv, ["nvim", file]);
            assert_eq!(argv(Some("code --wait")).argv, ["code", "--wait", file]);
            assert!(argv(Some("nvim")).env.is_empty());
        }
    }

    use super::super::TestFolder as Notes;

    #[cfg(unix)]
    #[test]
    fn tasks_open_hands_bat_a_hostile_file_name_as_one_argument_through_less() {
        // `less` puts the file name for `%s` in a shell command; its own quoting must keep a
        // name with quotes, `$( )` and `;` one argument. The fake editor stands in for bat.
        if !crate::integration::command_available("less") {
            return;
        }
        // An older less puts the name in bare, and `tasks.open` gives it no bat filter then.
        if !super::installed_less_quotes_lessopen() {
            eprintln!("skipped: the installed less does not quote file names for LESSOPEN");
            return;
        }
        let notes = Notes::new("open-lessopen");
        let file = notes
            .0
            .join("it's a \"task\" $(touch made-by-a-shell) ;.md")
            .to_str()
            .unwrap()
            .to_owned();
        std::fs::write(&file, "# notes\n").unwrap();
        let recorder = fixture("open-records.sh").to_str().unwrap().to_owned();
        let lessopen = super::bat_lessopen().replacen("bat", &recorder, 1);
        let launch = super::open_command(BOTH, None, &file, &vesper());
        let status = std::process::Command::new(&launch.argv[0])
            .args(&launch.argv[1..])
            .current_dir(&notes.0)
            .env("LESSOPEN", &lessopen)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .status()
            .expect("less runs");
        assert!(status.success(), "{status:?}");
        let opened = std::fs::read_to_string(notes.0.join("opened")).expect("the filter ran");
        assert_eq!(opened.lines().last(), Some(file.as_str()), "{opened}");
        assert!(
            !notes.0.join("made-by-a-shell").exists(),
            "no shell read the file name"
        );
    }

    /// `bat_lessopen()` with the popup's terminal size given as `rows cols`: a test has no
    /// terminal for `stty` to read.
    fn lessopen_at_size(rows: u16, cols: u16) -> String {
        let lessopen =
            super::bat_lessopen().replacen(super::TERMINAL_SIZE, &format!("echo {rows} {cols}"), 1);
        assert_ne!(lessopen, super::bat_lessopen(), "the filter reads the size");
        lessopen
    }

    #[cfg(unix)]
    #[test]
    fn tasks_open_has_bat_wrap_lines_at_the_width_of_the_popups_terminal() {
        // less gives its filter no `COLUMNS`; the filter reads the size of the terminal it runs
        // on, the popup's, and bat wraps its lines to that width.
        if !crate::integration::command_available("less") {
            return;
        }
        let notes = Notes::new("open-wrap-width");
        let file = notes.0.join("STATUS.md").to_str().unwrap().to_owned();
        std::fs::write(&file, "# notes\n").unwrap();
        let recorder = fixture("open-records.sh").to_str().unwrap().to_owned();
        let lessopen = lessopen_at_size(11, 37).replacen("bat", &recorder, 1);
        let launch = super::open_command(BOTH, None, &file, &vesper());
        let status = std::process::Command::new(&launch.argv[0])
            .args(&launch.argv[1..])
            .current_dir(&notes.0)
            .env("LESSOPEN", &lessopen)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .status()
            .expect("less runs");
        assert!(status.success(), "{status:?}");
        let opened = std::fs::read_to_string(notes.0.join("opened")).expect("the filter ran");
        let args: Vec<&str> = opened.lines().skip(1).collect();
        for option in ["--wrap=character", "--terminal-width=37"] {
            assert!(args.contains(&option), "{option} in {args:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_line_longer_than_the_popup_wraps_under_its_own_text_clear_of_the_line_numbers() {
        if !crate::integration::command_available("bat") {
            return;
        }
        let notes = Notes::new("open-wrap-gutter");
        let file = notes.0.join("long.md");
        let long = "a long line ".repeat(10);
        std::fs::write(&file, format!("short\n{long}end\n")).unwrap();
        // The filter as less runs it, on a terminal 40 columns wide.
        let filter = lessopen_at_size(11, 40)
            .trim_start_matches('|')
            .replace("%s", file.to_str().unwrap());
        let output = std::process::Command::new("sh")
            .args(["-c", &filter])
            .output()
            .expect("the filter runs");
        let shown = String::from_utf8_lossy(&output.stdout);
        let lines: Vec<String> = shown.lines().map(strip_colours).collect();
        let first = lines
            .iter()
            .position(|line| line.contains("a long line"))
            .unwrap_or_else(|| panic!("the long line: {lines:#?}"));
        // `   2 a long line …` then the rest under the text: the number's gutter stays clear.
        let gutter = lines[first].find('a').expect("its text");
        assert!(lines[first][..gutter].trim() == "2", "{lines:#?}");
        let rest = &lines[first + 1..];
        assert!(
            rest.last().is_some_and(|line| line.ends_with("end")),
            "the line wrapped onto the rows after it: {lines:#?}"
        );
        for line in &lines[first..] {
            assert!(line.len() <= 40, "no row is wider: {line:?}");
        }
        for line in rest {
            assert!(
                line.get(..gutter)
                    .is_some_and(|lead| lead.trim().is_empty()),
                "a wrapped row starts under the text: {line:?} in {lines:#?}"
            );
        }
    }

    /// `line` without its SGR colour codes.
    fn strip_colours(line: &str) -> String {
        let mut plain = String::new();
        let mut chars = line.chars();
        while let Some(ch) = chars.next() {
            if ch == '\x1b' {
                for code in chars.by_ref() {
                    if code == 'm' {
                        break;
                    }
                }
            } else {
                plain.push(ch);
            }
        }
        plain
    }

    #[test]
    fn tasks_open_colours_less_prompt_bar_with_the_boards_grey_words_on_its_dark_band() {
        // less takes 256-colour numbers, not RGB: Vesper's overlay1 (126, 126, 126) is 244,
        // its surface0 (35, 35, 35) is 235. Messages (the LESSOPEN note) share the bar's look.
        let argv = super::open_command(BOTH, None, "/notes/STATUS.md", &vesper()).argv;
        for option in ["--use-color", "-DP244.235", "-DE244.235"] {
            assert!(argv.iter().any(|arg| arg == option), "{option} in {argv:?}");
        }
        // A theme left to the terminal's own colours keeps less's default for that side.
        let mut palette = vesper();
        palette.surface0 = ratatui::style::Color::Reset;
        let argv = super::open_command(BOTH, None, "/notes/STATUS.md", &palette).argv;
        assert!(argv.iter().any(|arg| arg == "-DP244.-"), "{argv:?}");
    }

    /// The folder the fake editor ran in, then its arguments; waits up to 10 s for them.
    async fn opened(notes: &Notes) -> Vec<String> {
        let path = notes.0.join("opened");
        for _ in 0..200 {
            if let Ok(text) = std::fs::read_to_string(&path) {
                if text.ends_with('\n') {
                    return text.lines().map(str::to_owned).collect();
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("the fake editor never wrote {}", path.display());
    }

    /// Opens `file` from a list naming it in `work` or in a task's copies, with the fake editor,
    /// and returns what it recorded and the popup terminal's launch argv.
    async fn open_listed(notes: &Notes, file: &str, as_work: bool) -> (Vec<String>, Vec<String>) {
        let mut app = test_app();
        let other = notes.0.join("other.md").to_str().unwrap().to_owned();
        app.tasks_list.keep_list_for_test(if as_work {
            list(file, &other)
        } else {
            list(&other, file)
        });
        let editor = fixture("open-records.sh").to_str().unwrap().to_owned();
        let response = app.open_listed_task_file(
            "open".into(),
            file,
            Readers {
                bat: false,
                less: false,
                less_quotes: not_asked,
            },
            Some(&editor),
        );
        let success: SuccessResponse = serde_json::from_str(&response)
            .unwrap_or_else(|err| panic!("not a success ({err}): {response}"));
        assert_eq!(success.result, crate::api::schema::ResponseResult::Ok {});
        let popup = app.state.popup_pane.as_ref().expect("a popup opened");
        let whole = Some(crate::popup_size::PopupSize::Percent(100));
        assert_eq!(
            (popup.width, popup.height),
            (whole, whole),
            "the whole board area, not a box inside it"
        );
        let launch_argv = app.state.terminals[&popup.terminal_id]
            .launch_argv
            .clone()
            .expect("the popup runs an argv, not a shell string");
        let opened = opened(notes).await;
        assert!(app.close_popup_pane());
        (opened, launch_argv)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tasks_open_runs_the_editor_on_a_listed_file_in_its_folder() {
        let notes = Notes::new("open-listed");
        let file = notes.0.join("STATUS.md").to_str().unwrap().to_owned();
        let editor = fixture("open-records.sh").to_str().unwrap().to_owned();
        let folder = std::fs::canonicalize(&notes.0).unwrap();
        let folder = folder.to_str().unwrap();

        for as_work in [true, false] {
            let _ = std::fs::remove_file(notes.0.join("opened"));
            let (opened, launch_argv) = open_listed(&notes, &file, as_work).await;
            assert_eq!(launch_argv, [editor.clone(), file.clone()]);
            assert_eq!(opened, [folder, &file], "as_work: {as_work}");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tasks_open_passes_a_file_name_with_spaces_and_quotes_as_one_argument() {
        let notes = Notes::new("open-quoted");
        let file = notes
            .0
            .join("it's a \"task\" $(touch made-by-a-shell) ;.md")
            .to_str()
            .unwrap()
            .to_owned();

        let (opened, _) = open_listed(&notes, &file, true).await;

        assert_eq!(opened.len(), 2, "one argument: {opened:?}");
        assert_eq!(opened[1], file);
        assert!(
            !notes.0.join("made-by-a-shell").exists(),
            "no shell read the file name"
        );
    }
}
