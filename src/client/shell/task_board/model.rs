//! The task list `claude-tasks list --json` prints, contract version 1
//! (`docs/claude-tasks-format.md` in the tool's repo). Every field has a default and unknown fields
//! are ignored, so a newer reader's additions never reject the list.

use serde::Deserialize;

/// The only contract version this client reads.
pub(in crate::client::shell) const TASK_LIST_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub(in crate::client::shell) struct TaskList {
    /// Missing reads as 0, which is never a version this client reads.
    pub(in crate::client::shell) version: u32,
    pub(in crate::client::shell) generated: String,
    pub(in crate::client::shell) tasks: Vec<Task>,
    pub(in crate::client::shell) work: Vec<WorkItem>,
    /// Sources the reader could not read, each naming its file; never fatal.
    pub(in crate::client::shell) problems: Vec<String>,
}

impl TaskList {
    /// Whether the board draws `other` as it draws this list: the same but for the seconds
    /// of `generated`, which it shows to the minute.
    pub(in crate::client::shell) fn draws_like(&self, other: &TaskList) -> bool {
        fn minute(generated: &str) -> &str {
            generated.get(..16).unwrap_or(generated)
        }
        self.version == other.version
            && minute(&self.generated) == minute(&other.generated)
            && self.tasks == other.tasks
            && self.work == other.work
            && self.problems == other.problems
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub(in crate::client::shell) struct Task {
    /// The reader's number, counted through the sections in the file's order. The board does
    /// not show it: it numbers its rows as they show.
    pub(in crate::client::shell) n: u32,
    /// The Herdr space's label.
    pub(in crate::client::shell) space: String,
    pub(in crate::client::shell) project: String,
    pub(in crate::client::shell) task: String,
    pub(in crate::client::shell) group: TaskGroup,
    /// The who-must-act cell without its tags.
    pub(in crate::client::shell) state: String,
    pub(in crate::client::shell) since: Option<String>,
    pub(in crate::client::shell) first_seen: Option<String>,
    pub(in crate::client::shell) age_days: u32,
    /// Seven days or older.
    pub(in crate::client::shell) stale: bool,
    pub(in crate::client::shell) done_on: Option<String>,
    /// The agent on this task, only while its pane is working.
    pub(in crate::client::shell) agent: Option<TaskAgent>,
    /// Every file row with this task's text in its space.
    pub(in crate::client::shell) copies: Vec<TaskCopy>,
    /// Its deadline, `YYYY-MM-DD`. This and the fields below come from a newer reader; an
    /// older one sends none, and the board draws `state` and `age_days` as before.
    pub(in crate::client::shell) due: Option<String>,
    /// The Due column's words: the deadline (`Today`, `Thu`, `Overdue 3d`), or the day a done
    /// task was done.
    pub(in crate::client::shell) due_text: Option<String>,
    #[serde(deserialize_with = "known_level")]
    pub(in crate::client::shell) due_level: Option<DueLevel>,
    /// The Age column's words (`new`, `3d`, `2w`); empty for a done task.
    pub(in crate::client::shell) age_text: Option<String>,
    /// When a task that starts later starts (`Starts Tue`), shown under Due when it has none.
    pub(in crate::client::shell) starts_text: Option<String>,
    /// The Next action column's words.
    pub(in crate::client::shell) action: Option<String>,
    /// The Owner cell as written (`You`, `me`, a person, a tab). This and the fields below
    /// come from a reader of the one notes format; an older one sends none, and the board
    /// takes the section from `group` (`Task::section`).
    pub(in crate::client::shell) owner: Option<String>,
    #[serde(deserialize_with = "known_section")]
    pub(in crate::client::shell) section: Option<TaskSection>,
    /// What waits on this task, from its Blocking cell.
    pub(in crate::client::shell) blocking: Option<String>,
    /// The day it starts, `YYYY-MM-DD`.
    pub(in crate::client::shell) starts: Option<String>,
    /// It starts after today: drawn dim at the end of its section.
    pub(in crate::client::shell) scheduled: bool,
    /// The slug of the intent card it belongs to.
    pub(in crate::client::shell) card: Option<String>,
}

impl Task {
    /// The section it shows in: the reader's, else from an older reader its group, in
    /// progress while an agent is on it (the reader names one only while it works).
    pub(in crate::client::shell) fn section(&self) -> TaskSection {
        match self.section {
            Some(section) => section,
            None if self.agent.is_some() && self.group != TaskGroup::Done => {
                TaskSection::InProgress
            }
            None => self.group_section(),
        }
    }

    /// The section its group puts it in, whatever agent is on it.
    /// A group a newer reader added waits on others, so it still shows.
    pub(in crate::client::shell) fn group_section(&self) -> TaskSection {
        TaskSection::of_group(self.group)
    }
}

/// The board's sections, in the order they show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::client::shell) enum TaskSection {
    /// IN PROGRESS: an agent is at work on it.
    InProgress,
    /// ACTION REQUIRED: the user must act.
    ActionRequired,
    /// QUEUED: the lead's own, or a worker's that is not at work.
    Queued,
    /// WAITING ON OTHERS: a person or another tab must act.
    Waiting,
    /// COMPLETED, the last seven days.
    Completed,
    /// A section a newer reader added; it reads as none.
    #[serde(other)]
    Unknown,
}

impl TaskSection {
    /// The section `group` puts a task in; a group a newer reader added waits on others.
    pub(in crate::client::shell) fn of_group(group: TaskGroup) -> Self {
        match group {
            TaskGroup::You => Self::ActionRequired,
            TaskGroup::Claude => Self::Queued,
            TaskGroup::Waiting | TaskGroup::Unknown => Self::Waiting,
            TaskGroup::Done => Self::Completed,
        }
    }
}

/// A section, a section this client does not know read as none.
fn known_section<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<TaskSection>, D::Error> {
    let section = Option::<TaskSection>::deserialize(deserializer)?;
    Ok(section.filter(|section| *section != TaskSection::Unknown))
}

/// How near a task's deadline is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(in crate::client::shell) enum DueLevel {
    Overdue,
    Today,
    Soon,
    Later,
    /// A level a newer reader added; it reads as none.
    #[serde(other)]
    Unknown,
}

/// A due level, a level this client does not know read as none.
fn known_level<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<DueLevel>, D::Error> {
    let level = Option::<DueLevel>::deserialize(deserializer)?;
    Ok(level.filter(|level| *level != DueLevel::Unknown))
}

/// A card field's text, none for null or only spaces.
fn some_text<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    let text = Option::<String>::deserialize(deserializer)?;
    Ok(text.filter(|text| !text.trim().is_empty()))
}

/// A card field, its default for null.
fn null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// Who must act, which is the task's section.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(in crate::client::shell) enum TaskGroup {
    /// NEEDS YOU.
    You,
    /// CLAUDE.
    Claude,
    /// WAITING ON PEOPLE.
    Waiting,
    /// DONE.
    Done,
    /// A group a newer reader added.
    #[default]
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub(in crate::client::shell) struct TaskAgent {
    /// The Herdr pane ID.
    pub(in crate::client::shell) pane: String,
    pub(in crate::client::shell) name: String,
    /// How long it has been on the task; null when the agent has no brief to date it by.
    pub(in crate::client::shell) minutes: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub(in crate::client::shell) struct TaskCopy {
    pub(in crate::client::shell) file: String,
    pub(in crate::client::shell) line: u32,
    /// The exact raw line, the row's only identity.
    pub(in crate::client::shell) row: String,
}

/// A card of the Work view: one intent file.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub(in crate::client::shell) struct WorkItem {
    pub(in crate::client::shell) space: String,
    pub(in crate::client::shell) project: String,
    pub(in crate::client::shell) kind: WorkKind,
    pub(in crate::client::shell) title: String,
    pub(in crate::client::shell) stage: WorkStage,
    pub(in crate::client::shell) status: String,
    pub(in crate::client::shell) file: String,
    pub(in crate::client::shell) branch: Option<String>,
    pub(in crate::client::shell) commits: u32,
    pub(in crate::client::shell) steps: Vec<WorkStep>,
    pub(in crate::client::shell) shipped_on: Option<String>,
    pub(in crate::client::shell) agent: Option<TaskAgent>,
    /// The day of the newest commit on its branch, `YYYY-MM-DD`; none before the first.
    pub(in crate::client::shell) last_commit: Option<String>,
    /// Its `Objective:` line. This and the fields below come from a newer reader; an older
    /// one sends none, and each reads as none (as does null, and an empty string).
    #[serde(deserialize_with = "some_text")]
    pub(in crate::client::shell) objective: Option<String>,
    /// Its `Approval:` line: `pending`, `approved 2026-09-28`.
    #[serde(deserialize_with = "some_text")]
    pub(in crate::client::shell) approval: Option<String>,
    /// Its deadline, `YYYY-MM-DD`.
    #[serde(deserialize_with = "some_text")]
    pub(in crate::client::shell) due: Option<String>,
    /// The deadline's words as a task's (`Today`, `Thu`, `Overdue 3d`).
    #[serde(deserialize_with = "some_text")]
    pub(in crate::client::shell) due_text: Option<String>,
    #[serde(deserialize_with = "known_level")]
    pub(in crate::client::shell) due_level: Option<DueLevel>,
    /// The day it was written, `YYYY-MM-DD`.
    #[serde(deserialize_with = "some_text")]
    pub(in crate::client::shell) started: Option<String>,
    /// Its `Phase:` line, when it names a phase; its stage already follows it.
    #[serde(deserialize_with = "some_text")]
    pub(in crate::client::shell) phase: Option<String>,
    /// Its file's name without `.md`: what a task's `card` names.
    #[serde(deserialize_with = "some_text")]
    pub(in crate::client::shell) slug: Option<String>,
    #[serde(deserialize_with = "null_default")]
    pub(in crate::client::shell) findings: Vec<WorkFinding>,
    /// The text of its `## Verification` and `## Release`.
    #[serde(deserialize_with = "some_text")]
    pub(in crate::client::shell) verification: Option<String>,
    #[serde(deserialize_with = "some_text")]
    pub(in crate::client::shell) release: Option<String>,
    /// The newer of its last commit and its file's change, `YYYY-MM-DD`.
    #[serde(deserialize_with = "some_text")]
    pub(in crate::client::shell) last_activity: Option<String>,
}

/// A review finding: `HIGH`, `MEDIUM` or `LOW` (none when the bullet names none), its words,
/// and whether it is still open.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub(in crate::client::shell) struct WorkFinding {
    #[serde(deserialize_with = "some_text")]
    pub(in crate::client::shell) severity: Option<String>,
    #[serde(deserialize_with = "null_default")]
    pub(in crate::client::shell) text: String,
    #[serde(deserialize_with = "null_default")]
    pub(in crate::client::shell) open: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(in crate::client::shell) enum WorkKind {
    /// Also a card without a `Kind:` line.
    #[default]
    Feature,
    Bug,
    Chore,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(in crate::client::shell) enum WorkStage {
    Plan,
    Build,
    Review,
    /// `verify` is the stage's newer name; the column keeps its label.
    #[serde(alias = "verify")]
    Check,
    Shipped,
    #[default]
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub(in crate::client::shell) struct WorkStep {
    /// The short form: its first sentence, cut at a word.
    pub(in crate::client::shell) text: String,
    /// The whole line; empty from a reader that sends only `text`.
    pub(in crate::client::shell) full: String,
    pub(in crate::client::shell) done: bool,
    /// Its code (`A2`, `B2b`); none for a step without one.
    #[serde(deserialize_with = "some_text")]
    pub(in crate::client::shell) code: Option<String>,
}

/// Parses the reader's output, refusing any contract version but this client's. The client
/// reads the list from the server as JSON already (`parse_task_list_value`); tests read text.
#[cfg(test)]
pub(in crate::client::shell) fn parse_task_list(json: &str) -> Result<TaskList, String> {
    let list: TaskList =
        serde_json::from_str(json).map_err(|error| format!("unreadable task list: {error}"))?;
    checked_version(list)
}

/// The list as the server's `tasks.list` returns it, already JSON.
pub(in crate::client::shell) fn parse_task_list_value(
    value: serde_json::Value,
) -> Result<TaskList, String> {
    let list: TaskList =
        serde_json::from_value(value).map_err(|error| format!("unreadable task list: {error}"))?;
    checked_version(list)
}

fn checked_version(list: TaskList) -> Result<TaskList, String> {
    if list.version != TASK_LIST_VERSION {
        return Err(format!(
            "task list version {} (this Herdr reads version {TASK_LIST_VERSION})",
            list.version
        ));
    }
    Ok(list)
}
