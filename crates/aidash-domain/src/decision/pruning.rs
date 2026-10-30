//! Which history a compaction decision may classify, and how its branches
//! change the Context Projection. The gate and replay share these rules, so a
//! replayed decision protects exactly what the original one did.
use super::Branch;
use crate::context::{ContextEvent, HistoryEntry};
use crate::projection::native::model_turns;
use serde_json::json;
use std::collections::BTreeMap;

/// A tool result longer than this many characters is truncated to its head.
const TRUNCATE_ABOVE: usize = 420;
pub const TRUNCATED_HEAD: usize = 300;
/// How a decision marks a tool result it cut to its head.
const TRUNCATION_MARK: &str = "[decision compaction truncated ";
const TRUNCATION_END: &str = "full result remains in the execution journal]";

/// Whether a decision cut `result` to its head. Such a result is a string
/// whatever its original shape; the Context Journal keeps the original.
pub fn truncated_result(result: &serde_json::Value) -> bool {
	result
		.as_str()
		.is_some_and(|text| text.contains(TRUNCATION_MARK) && text.ends_with(TRUNCATION_END))
}

/// The protection units of a history: each event's Model Turn and their number
/// on the Native Projection Version, or none for Legacy, which counts events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Units {
	pub turn_of: Vec<Option<usize>>,
	pub turns: Option<usize>,
}
impl Units {
	pub fn of(history: &[HistoryEntry], native: bool) -> Self {
		if !native {
			return Self {
				turn_of: vec![None; history.len()],
				turns: None,
			};
		}
		let turn_of = model_turns(history);
		let turns = turn_of.iter().flatten().max().map_or(0, |last| last + 1);
		Self {
			turn_of,
			turns: Some(turns),
		}
	}
}

/// Whether the event at `index` is never classified: the first and the latest
/// `preserve_recent` events, or on the Native Projection Version every event of
/// the first and the latest `preserve_recent` Model Turns. Native events outside
/// any turn are protected by their position, as on Legacy.
pub fn protected(
	index: usize,
	turn: Option<usize>,
	history_length: usize,
	turns: Option<usize>,
	preserve_recent: usize,
) -> bool {
	match (turn, turns) {
		(Some(turn), Some(total)) => turn == 0 || turn >= total.saturating_sub(preserve_recent),
		_ => index == 0 || index >= history_length.saturating_sub(preserve_recent),
	}
}

/// The projection a decision's branches produce, with what it removed.
#[derive(Debug, Clone, PartialEq)]
pub struct Pruned {
	pub history: Vec<HistoryEntry>,
	/// History entries no longer in the projection.
	pub removed: usize,
	/// Tool results shortened to their head.
	pub truncated: usize,
}

/// Apply the branch selected for each classified history index. On Legacy a
/// dropped event leaves the projection. On the Native Projection Version a
/// dropped result is elided under its call, and a Model Turn leaves only as a
/// whole, when every call it ran is dropped and its response has no text of
/// its own (ADR 0015).
pub fn apply(
	history: &[HistoryEntry],
	selected: &BTreeMap<usize, Branch>,
	units: &Units,
) -> Pruned {
	let chosen = |index: usize| selected.get(&index).copied().unwrap_or(Branch::Keep);
	let mut removable: BTreeMap<usize, bool> = BTreeMap::new();
	if units.turns.is_some() {
		for (index, turn) in units.turn_of.iter().enumerate() {
			let Some(turn) = turn else { continue };
			let keep = match &history[index].event {
				ContextEvent::Assistant { text, calls, .. } => {
					!text.trim().is_empty() || calls.is_empty()
				}
				_ => chosen(index) != Branch::Drop,
			};
			*removable.entry(*turn).or_insert(true) &= !keep;
		}
	}
	let mut pruned = Pruned {
		history: Vec::with_capacity(history.len()),
		removed: 0,
		truncated: 0,
	};
	for (index, entry) in history.iter().enumerate() {
		if units.turn_of[index].is_some_and(|turn| removable[&turn]) {
			pruned.removed += 1;
			continue;
		}
		let ContextEvent::Tool { call, result } = &entry.event else {
			pruned.history.push(entry.clone());
			continue;
		};
		match chosen(index) {
			Branch::Keep => pruned.history.push(entry.clone()),
			Branch::Drop if units.turn_of[index].is_some() => {
				// The call stays in its Model Turn; only its result is elided.
				let mut kept = entry.clone();
				kept.event = ContextEvent::Tool {
					call: call.clone(),
					result: ContextEvent::elided_result(&call.id),
				};
				pruned.history.push(kept);
			}
			Branch::Drop => pruned.removed += 1,
			Branch::TruncateResult => {
				let text = result
					.as_str()
					.map(str::to_owned)
					.unwrap_or_else(|| result.to_string());
				let length = text.chars().count();
				if length <= TRUNCATE_ABOVE {
					pruned.history.push(entry.clone());
					continue;
				}
				// The head is the historical action rule, not a limit of the
				// classification state or its evidence.
				let head: String = text.chars().take(TRUNCATED_HEAD).collect();
				pruned.truncated += 1;
				pruned.history.push(HistoryEntry {
					research: entry.research_reference(),
					seq: entry.seq,
					event: ContextEvent::Tool {
						call: call.clone(),
						result: json!(format!(
							"{head}\n{TRUNCATION_MARK}{} chars of this tool result; {TRUNCATION_END}",
							length - TRUNCATED_HEAD
						)),
					},
				});
			}
		}
	}
	pruned
}

/// The commitment Decision Evidence keeps to the projection it produced.
pub fn projection_digest(history: &[HistoryEntry]) -> crate::Result<String> {
	Ok(crate::registry::rules::digest(&serde_json::to_value(
		history,
	)?))
}
