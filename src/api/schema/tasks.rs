use serde::{Deserialize, Serialize};

/// One action on the task list, run by `tasks.command` against the notes files.
///
/// `action` is `done`, `delete`, `move`, `reorder` or `new`. The first four name the task by
/// `copies` (every file row of a merged task); `move` also takes `to` and `reorder` takes
/// `direction` (`up` or `down`). `new` takes only `file`, `task` and `who`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, Default)]
pub struct TasksActParams {
    pub action: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub copies: Vec<TaskCopy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub who: Option<String>,
}

/// Opens one file the task list names, in an editor popup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, Default)]
pub struct TasksOpenParams {
    /// Exactly as the task list gave it, in `work[].file` or `tasks[].copies[].file`.
    pub file: String,
}

/// One file row of a task, exactly as the task list gave it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, Default)]
pub struct TaskCopy {
    pub file: String,
    pub row: String,
}
