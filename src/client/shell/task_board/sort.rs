//! The order of the board's task rows: by priority unless the header's sort menu picks
//! another. Client state only, kept for the client's life.

use std::cmp::Ordering;

use super::model::{DueLevel, Task, TaskGroup};
use super::table::{done_day, state_shown};

/// How the task rows of every section are ordered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::client::shell) enum TaskSort {
    /// Overdue first, then what blocks others, then the nearest due date, then the oldest.
    #[default]
    Priority,
    /// Overdue first, then the nearest due date, then the oldest; ties in the file's order.
    Due,
    /// Smallest age first.
    Newest,
    /// Largest age first.
    Oldest,
    /// Project name A to Z, then newest.
    Project,
    /// The last column's text as drawn A to Z, then newest.
    State,
    /// The list's own order, as the file keeps it.
    File,
}

impl TaskSort {
    /// The menu's orders, top down.
    pub(super) const ALL: [Self; 7] = [
        Self::Priority,
        Self::Due,
        Self::Newest,
        Self::Oldest,
        Self::Project,
        Self::State,
        Self::File,
    ];

    /// Its word on the header's button.
    pub(super) fn word(self) -> &'static str {
        match self {
            Self::Priority => "Priority",
            Self::Due => "Due",
            Self::Newest => "Newest",
            Self::Oldest => "Oldest",
            Self::Project => "Project",
            Self::State => "State",
            Self::File => "File",
        }
    }

    /// Its line in the menu.
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Priority => "By priority",
            Self::Due => "By due date",
            Self::Newest => "Newest first",
            Self::Oldest => "Oldest first",
            Self::Project => "By project",
            Self::State => "By state",
            Self::File => "File order",
        }
    }

    /// The order one step down the menu (`down`) or up, stopping at its ends.
    pub(super) fn step(self, down: bool) -> Self {
        // `ALL` lists the orders as they are declared.
        let index = self as usize;
        let next = if down {
            (index + 1).min(Self::ALL.len() - 1)
        } else {
            index.saturating_sub(1)
        };
        Self::ALL[next]
    }

    /// Whether `a` shows before `b`: done tasks after the rest, newest done first (oldest
    /// first under `Oldest`, a task without its date as done `today`); the rest by this
    /// order. Ties keep the list's order, as the sort that uses it is stable.
    pub(super) fn order(self, a: &Task, b: &Task, today: &str) -> Ordering {
        match (a.group == TaskGroup::Done, b.group == TaskGroup::Done) {
            (true, true) => {
                let newest = done_day(b, today).cmp(done_day(a, today));
                match self {
                    Self::Oldest => newest.reverse(),
                    _ => newest,
                }
            }
            (false, false) => {
                let newest = a.age_days.cmp(&b.age_days);
                match self {
                    Self::Priority => priority_order(a, b).then(newest.reverse()),
                    Self::Due => due_order(a, b).then(newest.reverse()),
                    Self::Newest => newest,
                    Self::Oldest => newest.reverse(),
                    Self::Project => caseless(&a.project, &b.project).then(newest),
                    Self::State => caseless(state_shown(a), state_shown(b)).then(newest),
                    Self::File => Ordering::Equal,
                }
            }
            (a_done, b_done) => a_done.cmp(&b_done),
        }
    }
}

/// `a` against `b` by priority: overdue first, then what blocks others, then as [`due_order`].
fn priority_order(a: &Task, b: &Task) -> Ordering {
    let (a_due, b_due) = (due_key(a), due_key(b));
    a_due
        .0
        .cmp(&b_due.0)
        .then(a.blocking.is_none().cmp(&b.blocking.is_none()))
        .then(a_due.cmp(&b_due))
}

/// `a` against `b` by due: overdue first, then the nearest `due` date, then tasks with none.
fn due_order(a: &Task, b: &Task) -> Ordering {
    due_key(a).cmp(&due_key(b))
}

/// What [`due_order`] compares: not overdue, no due date, then the date, as ISO dates order
/// as text.
fn due_key(task: &Task) -> (bool, bool, Option<&str>) {
    (
        task.due_level != Some(DueLevel::Overdue),
        task.due.is_none(),
        task.due.as_deref(),
    )
}

/// `a` against `b`, A to Z whatever their case, with no copy made.
fn caseless(a: &str, b: &str) -> Ordering {
    a.chars()
        .flat_map(char::to_lowercase)
        .cmp(b.chars().flat_map(char::to_lowercase))
}
