//! Unassigned Tasks: open child Tasks nobody is working on or can start, and
//! what a Run does about them when it gives its final answer (#772).
use crate::{ChildTaskSummary, TaskStatus};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

/// The most Tasks one notice, one sentence or one list names.
pub const LIST_LIMIT: usize = 32;

/// The reason recorded on a Task a finishing Run abandons.
pub const ABANDON_REASON: &str =
	"Abandoned: nobody was given this subtask when the Run that created it finished.";

/// What the model is told about Tasks nobody will run.
pub const TOLD_INSTRUCTION: &str = "The subtasks listed have nobody to run them. Hand them over with task_delegate. If they are no longer needed, give your final answer again: they will be recorded as abandoned, and say so in your answer.";

/// An open child Task that nobody is working on or can start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UnassignedTask {
	pub id: Uuid,
	pub title: String,
	/// A Run of the parent Task created it and nothing open sits beneath it.
	#[serde(default)]
	pub abandonable: bool,
	/// Someone has it, but it waits on another Unassigned Task.
	#[serde(default)]
	pub handed_over: bool,
}

/// One pending child as the summary builders read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingChild {
	pub id: Uuid,
	pub title: String,
	pub status: TaskStatus,
	pub dependencies: Vec<Uuid>,
	pub abandonable: bool,
	/// A Run is under way, another Node took it, or a generated specialist
	/// awaits approval for it.
	pub started: bool,
}

/// Which pending children are Unassigned Tasks, in id order.
pub fn classify(pending: &[PendingChild]) -> Vec<UnassignedTask> {
	let mut unassigned: BTreeMap<Uuid, bool> = pending
		.iter()
		.filter(|child| child.status == TaskStatus::Open && !child.started)
		.map(|child| (child.id, false))
		.collect();
	// A child someone has can still be stuck on a sibling nobody has. Repeat
	// until nothing changes, so a chain of siblings is covered whatever its
	// order. A dependency outside the pending children never sticks a child:
	// that work is still moving.
	loop {
		let stuck: Vec<Uuid> = pending
			.iter()
			.filter(|child| {
				matches!(child.status, TaskStatus::Open | TaskStatus::Claimed)
					&& !unassigned.contains_key(&child.id)
					&& child
						.dependencies
						.iter()
						.any(|dependency| unassigned.contains_key(dependency))
			})
			.map(|child| child.id)
			.collect();
		if stuck.is_empty() {
			break;
		}
		unassigned.extend(stuck.into_iter().map(|id| (id, true)));
	}
	let mut found: Vec<UnassignedTask> = pending
		.iter()
		.filter_map(|child| {
			unassigned
				.get(&child.id)
				.map(|&handed_over| UnassignedTask {
					id: child.id,
					title: child.title.clone(),
					abandonable: child.abandonable,
					handed_over,
				})
		})
		.collect();
	found.sort_by_key(|task| task.id);
	found
}

/// What a Run does with its pending children when the model answers finally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildrenDecision {
	/// Nothing pending: go on to the final-answer checks.
	Finish,
	/// A failed child: ask a person, as before.
	AskAboutFailed,
	/// Another child can still change: wait, as before.
	Wait,
	/// Tell the model about these.
	Tell(Vec<UnassignedTask>),
	/// Ask a person about these.
	AskAboutUnassigned(Vec<UnassignedTask>),
	/// Abandon these in the transaction that completes the parent.
	FinishAbandoning(Vec<UnassignedTask>),
	/// Abandon these now and wait: a handed-over child depends on them, so the
	/// parent cannot complete yet.
	AbandonAndWait(Vec<UnassignedTask>),
}

pub fn decide_children(
	summary: &ChildTaskSummary,
	told: &BTreeSet<Uuid>,
	no_step_left: bool,
) -> ChildrenDecision {
	if !summary.has_pending {
		return ChildrenDecision::Finish;
	}
	if summary.has_failed {
		return ChildrenDecision::AskAboutFailed;
	}
	// Another child can still change: the Run waits for it.
	let all = &summary.unassigned;
	if summary.open_tasks > all.len() as u64 {
		return ChildrenDecision::Wait;
	}
	// With no step left there is no notice, so the Run does not fail on its
	// step limit; it goes on to the rules below.
	if !no_step_left {
		let untold: Vec<UnassignedTask> = all
			.iter()
			.filter(|task| !told.contains(&task.id))
			.take(LIST_LIMIT)
			.cloned()
			.collect();
		if !untold.is_empty() {
			return ChildrenDecision::Tell(untold);
		}
	}
	let ask: Vec<UnassignedTask> = all
		.iter()
		.filter(|task| !task.handed_over && !task.abandonable)
		.cloned()
		.collect();
	if !ask.is_empty() {
		return ChildrenDecision::AskAboutUnassigned(ask);
	}
	let abandon: Vec<UnassignedTask> = all
		.iter()
		.filter(|task| !task.handed_over && task.abandonable)
		.cloned()
		.collect();
	if !all.iter().any(|task| task.handed_over) {
		return ChildrenDecision::FinishAbandoning(abandon);
	}
	if abandon.is_empty() {
		// Not reachable from real data: the root of a handed-over chain is
		// either abandonable or asked about above.
		return ChildrenDecision::Wait;
	}
	ChildrenDecision::AbandonAndWait(abandon)
}

/// Render the completion answer with earlier and newly abandoned Tasks, once per ID.
pub fn completion_answer(
	answer: &str,
	early: &[UnassignedTask],
	abandoning: &[UnassignedTask],
) -> String {
	let mut named = early.to_vec();
	for task in abandoning {
		if !named.iter().any(|prior| prior.id == task.id) {
			named.push(task.clone());
		}
	}
	answer_with_abandoned(answer, &named)
}

/// The answer followed by the fixed sentence naming the abandoned Tasks.
pub fn answer_with_abandoned(answer: &str, abandoned: &[UnassignedTask]) -> String {
	if abandoned.is_empty() {
		return answer.to_owned();
	}
	format!(
		"{answer}\n\nNobody was given these subtasks, so they were abandoned during this Run: {}.",
		title_list(abandoned)
	)
}

/// At most [`LIST_LIMIT`] quoted titles, then a count of the rest.
fn title_list(tasks: &[UnassignedTask]) -> String {
	let mut text = tasks
		.iter()
		.take(LIST_LIMIT)
		.map(|task| format!("\"{}\"", task.title))
		.collect::<Vec<_>>()
		.join(", ");
	if tasks.len() > LIST_LIMIT {
		text.push_str(&format!(" and {} more", tasks.len() - LIST_LIMIT));
	}
	text
}

/// The Human request text for Unassigned Tasks created elsewhere.
pub fn request_prompt(tasks: &[UnassignedTask]) -> String {
	format!(
		"These subtasks have nobody to run them: {}. A person can hand them over or abandon them in their task details, giving a reason. Then answer this request to continue, or cancel this parent.",
		title_list(tasks)
	)
}

#[cfg(test)]
mod tests;
