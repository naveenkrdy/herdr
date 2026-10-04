//! The board's filters: the text filter, where a task or a card shows while every typed word
//! is in the text the board draws for it, in any order and any case; and the Phases view's
//! Filter menu, whose picks keep the cards in a status, a kind and an owner, each read from
//! the card's own fields. Client state only; the server never sees either.

use ratatui::style::Color;

use super::model::{DueLevel, Task, WorkItem, WorkKind, WorkStage};
use super::table::{state_shown, where_text};
use super::work::{holder, kind_mark, shown_title, waits_on_you, who_text, Holder};
use super::{style, AgentState, BoardAgent};
use crate::app::state::Palette;

/// The filter's words as they are matched: split on spaces, lower case; none when empty.
pub(super) fn words(filter: &str) -> Vec<String> {
    filter.split_whitespace().map(str::to_lowercase).collect()
}

/// Every word is in one of `fields`, each word in any of them.
fn all_in(fields: &[String], words: &[String]) -> bool {
    let fields: Vec<String> = fields.iter().map(|field| field.to_lowercase()).collect();
    words
        .iter()
        .all(|word| fields.iter().any(|field| field.contains(word.as_str())))
}

/// A task shows while every word is in its task, its Where, its Next action, its Due or when
/// it starts.
pub(super) fn task_matches(task: &Task, words: &[String]) -> bool {
    words.is_empty()
        || all_in(
            &[
                task.task.clone(),
                where_text(task, false),
                state_shown(task).to_owned(),
                task.due_text.clone().unwrap_or_default(),
                task.starts_text.clone().unwrap_or_default(),
            ],
            words,
        )
}

/// A live agent's tab shows while every word is in its name, its space, what it does or
/// asks, or the `task` its row shows.
pub(super) fn agent_matches(agent: &BoardAgent, task: Option<&str>, words: &[String]) -> bool {
    let says = match &agent.state {
        AgentState::Working(Some(text)) | AgentState::Waiting(Some(text)) => text.clone(),
        AgentState::Working(None) | AgentState::Waiting(None) | AgentState::Resting => {
            String::new()
        }
    };
    words.is_empty()
        || all_in(
            &[
                agent.tab.clone(),
                agent.space.clone(),
                says,
                task.unwrap_or_default().to_owned(),
            ],
            words,
        )
}

/// A card shows while every word is in its title, kind, project or who line (what the card
/// shows, never the space it does not), or in one of its steps' codes, which its panel no
/// longer draws but a plan names it by.
pub(super) fn card_matches(
    card: &WorkItem,
    (tasks, agents): (&[Task], &[BoardAgent]),
    words: &[String],
) -> bool {
    if words.is_empty() {
        return true;
    }
    let mut fields = vec![
        shown_title(&card.title).to_owned(),
        kind_mark(card.kind).1.to_owned(),
        card.project.clone(),
        who_text(card, (tasks, agents)).unwrap_or_default(),
    ];
    fields.extend(card.steps.iter().filter_map(|step| step.code.clone()));
    all_in(&fields, words)
}

/// Where a card's work stands, as the Filter menu's STATUS group sorts it: each card in one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::client::shell) enum CardStatus {
    /// Its phase waits on the user (a plan to approve, a build to check), or they hold it.
    NeedsYou,
    /// Due today or overdue.
    AtRisk,
    /// Its status says `blocked`, in any case.
    Blocked,
    /// Open, and none of the others.
    OnTrack,
    /// Closed, or shipped.
    Done,
}

/// Who holds a card, as the Filter menu's OWNER group sorts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::client::shell) enum CardOwner {
    /// The user.
    Me,
    /// An agent at work on it.
    Agents,
    /// A named owner, or its space's lead.
    Others,
}

/// One item of the Filter menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::client::shell) enum FilterPick {
    Status(CardStatus),
    Kind(WorkKind),
    Owner(CardOwner),
}

impl FilterPick {
    /// Every item, in the menu's order: STATUS, KIND, then OWNER.
    pub(in crate::client::shell) const ALL: [Self; 11] = [
        Self::Status(CardStatus::NeedsYou),
        Self::Status(CardStatus::AtRisk),
        Self::Status(CardStatus::Blocked),
        Self::Status(CardStatus::OnTrack),
        Self::Status(CardStatus::Done),
        Self::Kind(WorkKind::Feature),
        Self::Kind(WorkKind::Bug),
        Self::Kind(WorkKind::Chore),
        Self::Owner(CardOwner::Me),
        Self::Owner(CardOwner::Agents),
        Self::Owner(CardOwner::Others),
    ];

    /// Its group's name, as its pill says it: `Status`, `Kind`, `Owner`.
    pub(super) fn group(self) -> &'static str {
        match self {
            Self::Status(_) => "Status",
            Self::Kind(_) => "Kind",
            Self::Owner(_) => "Owner",
        }
    }

    /// Its mark and its words; an owner has no mark.
    pub(super) fn words(self) -> (&'static str, &'static str) {
        match self {
            Self::Status(CardStatus::NeedsYou) => ("?", "Needs you"),
            Self::Status(CardStatus::AtRisk) => ("!", "At risk"),
            Self::Status(CardStatus::Blocked) => ("✗", "Blocked"),
            Self::Status(CardStatus::OnTrack) => ("·", "On track"),
            Self::Status(CardStatus::Done) => ("✓", "Done"),
            Self::Kind(WorkKind::Feature) => (kind_mark(WorkKind::Feature).0, "Feature"),
            Self::Kind(WorkKind::Bug) => (kind_mark(WorkKind::Bug).0, "Bug"),
            Self::Kind(WorkKind::Chore) => (kind_mark(WorkKind::Chore).0, "Chore"),
            Self::Kind(WorkKind::Unknown) => (kind_mark(WorkKind::Unknown).0, "Other"),
            Self::Owner(CardOwner::Me) => ("", "Me"),
            Self::Owner(CardOwner::Agents) => ("", "Agents"),
            Self::Owner(CardOwner::Others) => ("", "Others"),
        }
    }

    /// Its mark's colour: a status's own, a kind's.
    pub(super) fn role(self, palette: &Palette) -> Color {
        match self {
            Self::Status(CardStatus::NeedsYou) => palette.yellow,
            Self::Status(CardStatus::AtRisk | CardStatus::Blocked) => palette.red,
            Self::Status(CardStatus::OnTrack) => palette.overlay1,
            Self::Status(CardStatus::Done) => palette.green,
            Self::Kind(kind) => style::kind_role(kind, palette),
            Self::Owner(_) => palette.text,
        }
    }

    /// Whether `card` is in it.
    fn holds(self, card: &WorkItem, (tasks, agents): (&[Task], &[BoardAgent])) -> bool {
        match self {
            Self::Status(status) => card_status(card, tasks, agents) == status,
            Self::Kind(kind) => card.kind == kind,
            Self::Owner(owner) => card_owner(card, tasks, agents) == Some(owner),
        }
    }
}

/// A card's status, from its fields alone: done (closed, or shipped) first, then blocked, then
/// waiting on the user (its phase's `?`, or they hold it), then due today or overdue.
pub(in crate::client::shell) fn card_status(
    card: &WorkItem,
    tasks: &[Task],
    agents: &[BoardAgent],
) -> CardStatus {
    let status = card.status.trim();
    if status.eq_ignore_ascii_case("closed") || card.stage == WorkStage::Shipped {
        CardStatus::Done
    } else if status.eq_ignore_ascii_case("blocked") {
        CardStatus::Blocked
    } else if waits_on_you(card) || matches!(holder(card, tasks, agents), Some(Holder::You)) {
        CardStatus::NeedsYou
    } else if matches!(card.due_level, Some(DueLevel::Overdue | DueLevel::Today)) {
        CardStatus::AtRisk
    } else {
        CardStatus::OnTrack
    }
}

/// Who holds a card, as its who line says: none for a card only its space has (no linked task
/// and no lead), which no owner pick keeps.
pub(in crate::client::shell) fn card_owner(
    card: &WorkItem,
    tasks: &[Task],
    agents: &[BoardAgent],
) -> Option<CardOwner> {
    match holder(card, tasks, agents)? {
        Holder::You => Some(CardOwner::Me),
        Holder::Agent(..) => Some(CardOwner::Agents),
        Holder::Named(_) => Some(CardOwner::Others),
        Holder::Space(_) => None,
    }
}

/// The Phases view's Filter menu picks: at most one in each group; a card shows while it is in
/// every pick.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(in crate::client::shell) struct WorkFilter {
    pub(in crate::client::shell) status: Option<CardStatus>,
    pub(in crate::client::shell) kind: Option<WorkKind>,
    pub(in crate::client::shell) owner: Option<CardOwner>,
}

impl WorkFilter {
    /// The picks, in the menu's order.
    pub(in crate::client::shell) fn picks(self) -> Vec<FilterPick> {
        [
            self.status.map(FilterPick::Status),
            self.kind.map(FilterPick::Kind),
            self.owner.map(FilterPick::Owner),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    pub(in crate::client::shell) fn is_empty(self) -> bool {
        self == Self::default()
    }

    pub(in crate::client::shell) fn has(self, pick: FilterPick) -> bool {
        self.picks().contains(&pick)
    }

    /// Picks `pick`, in place of its group's other; picked already, clears it.
    pub(in crate::client::shell) fn toggle(&mut self, pick: FilterPick) {
        fn flip<T: PartialEq>(slot: &mut Option<T>, value: T) {
            *slot = if slot.as_ref() == Some(&value) {
                None
            } else {
                Some(value)
            };
        }
        match pick {
            FilterPick::Status(status) => flip(&mut self.status, status),
            FilterPick::Kind(kind) => flip(&mut self.kind, kind),
            FilterPick::Owner(owner) => flip(&mut self.owner, owner),
        }
    }

    /// Whether `card` is in every pick.
    pub(super) fn keeps(self, card: &WorkItem, context: (&[Task], &[BoardAgent])) -> bool {
        self.picks()
            .into_iter()
            .all(|pick| pick.holds(card, context))
    }

    /// How many of `cards` would show with `pick` in its group: the other groups' picks
    /// still apply, so each count says what picking it shows.
    pub(super) fn count_with(
        self,
        pick: FilterPick,
        cards: &[&WorkItem],
        context: (&[Task], &[BoardAgent]),
    ) -> usize {
        let mut with = self;
        if !with.has(pick) {
            with.toggle(pick);
        }
        cards
            .iter()
            .filter(|card| with.keeps(card, context))
            .count()
    }
}
