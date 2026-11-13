//! Items a response left out because a Node holding one of their Read
//! dependencies did not answer (#770). A real refusal is never counted here.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

/// What an Agent is told beside a non-empty `unconfirmed` list.
pub const NOTE: &str = "Some items could not be confirmed with the Nodes listed here and are left out, so this result may be incomplete.";

/// Add `unconfirmed` and its note to a tool result, only when something was
/// left out, so an ordinary result does not change.
pub fn attach(result: &mut serde_json::Value, entries: Vec<Unconfirmed>) {
	if entries.is_empty() {
		return;
	}
	result["unconfirmed"] = serde_json::json!(entries);
	result["unconfirmed_note"] = serde_json::json!(NOTE);
}

/// A tool result built from an HTTP-shaped answer: `unconfirmed` is always
/// present there, but an Agent sees it, with its note, only when something was
/// left out.
pub fn for_tool(result: &mut serde_json::Value) {
	let Some(entries) = result
		.as_object_mut()
		.and_then(|object| object.remove("unconfirmed"))
		.and_then(|entries| serde_json::from_value::<Vec<Unconfirmed>>(entries).ok())
	else {
		return;
	};
	attach(result, entries);
}

/// What kind of item was left out, so a view that mixes kinds (`/api/state`)
/// can narrow the entry to the list it sits above.
#[derive(
	Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "UnconfirmedKind")]
pub enum Kind {
	Task,
	Run,
	Artifact,
	Event,
	Message,
	HumanRequest,
	SemanticEntry,
	MemoryUnit,
}

/// How many rows one response scanned and could not confirm because `node` did
/// not answer. It is not a total, and it is not "you should see these": some
/// of the rows can still be refused once the Node answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Unconfirmed {
	/// The address of the Node that did not answer.
	pub node: String,
	pub kind: Kind,
	pub workspace_id: Uuid,
	pub count: u64,
}

/// Rows of one response that could not be confirmed, recorded one row at a
/// time. A row counts once, under the first silent Node its check reached.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Tally {
	counts: BTreeMap<(String, Kind, Uuid), u64>,
}

impl Tally {
	pub fn new() -> Self {
		Self::default()
	}

	/// One row of `kind` in `workspace` that `node`'s silence left unconfirmed.
	pub fn record(&mut self, node: &str, kind: Kind, workspace: Uuid) {
		let count = self
			.counts
			.entry((node.to_owned(), kind, workspace))
			.or_default();
		*count = count.saturating_add(1);
	}

	/// Settle one row. `silence` is the Node the row's check gave up on, taken
	/// from the authority after the check. A visible row is never counted, and
	/// a row without a silent Node was refused, which is not counted either.
	/// Returns whether the row is visible, so the loop keeps its own shape.
	pub fn settle(
		&mut self,
		silence: Option<String>,
		kind: Kind,
		workspace: Uuid,
		visible: bool,
	) -> bool {
		if !visible && let Some(node) = silence {
			self.record(&node, kind, workspace);
		}
		visible
	}

	/// A tally that already holds `entries`, to add rows to a result.
	pub fn from_entries(entries: Vec<Unconfirmed>) -> Self {
		let mut tally = Self::new();
		for entry in entries {
			let total = tally
				.counts
				.entry((entry.node, entry.kind, entry.workspace_id))
				.or_default();
			*total = total.saturating_add(entry.count);
		}
		tally
	}

	/// [`Self::settle`] for an item that may sit in no Workspace, such as a
	/// Tenant-level event. Such an item depends on no Node, so it is never
	/// recorded (#770).
	pub fn settle_in(
		&mut self,
		silence: Option<String>,
		kind: Kind,
		workspace: Option<Uuid>,
		visible: bool,
	) -> bool {
		match workspace {
			Some(workspace) => self.settle(silence, kind, workspace, visible),
			None => visible,
		}
	}

	pub fn is_empty(&self) -> bool {
		self.counts.is_empty()
	}

	/// Add the rows another tally recorded.
	pub fn merge(&mut self, other: Tally) {
		for ((node, kind, workspace), count) in other.counts {
			let total = self.counts.entry((node, kind, workspace)).or_default();
			*total = total.saturating_add(count);
		}
	}

	/// The entries, ordered by Node, kind and Workspace so a response is stable.
	pub fn into_entries(self) -> Vec<Unconfirmed> {
		self.counts
			.into_iter()
			.map(|((node, kind, workspace_id), count)| Unconfirmed {
				node,
				kind,
				workspace_id,
				count,
			})
			.collect()
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	const NODE: &str = "aidash://local/brave-otter-k3m9q2xa";

	#[test]
	fn only_rows_a_silent_node_left_unconfirmed_are_counted() {
		let workspace = Uuid::from_u128(1);
		let mut tally = Tally::new();
		for _ in 0..3 {
			assert!(!tally.settle(Some(NODE.into()), Kind::Task, workspace, false));
		}
		for _ in 0..5 {
			assert!(!tally.settle(None, Kind::Task, workspace, false));
		}
		assert!(tally.settle(None, Kind::Task, workspace, true));
		assert!(tally.settle(Some(NODE.into()), Kind::Task, workspace, true));
		assert_eq!(
			tally.into_entries(),
			vec![Unconfirmed {
				node: NODE.into(),
				kind: Kind::Task,
				workspace_id: workspace,
				count: 3,
			}]
		);
	}

	#[test]
	fn entries_are_sorted_by_node_kind_and_workspace() {
		let mut tally = Tally::new();
		tally.record("aidash://local/b", Kind::Run, Uuid::from_u128(2));
		tally.record("aidash://local/a", Kind::Task, Uuid::from_u128(2));
		tally.record("aidash://local/a", Kind::Task, Uuid::from_u128(1));
		tally.record("aidash://local/a", Kind::Event, Uuid::from_u128(3));
		let order: Vec<_> = tally
			.into_entries()
			.into_iter()
			.map(|entry| (entry.node, entry.kind, entry.workspace_id.as_u128()))
			.collect();
		assert_eq!(
			order,
			vec![
				("aidash://local/a".to_owned(), Kind::Task, 1),
				("aidash://local/a".to_owned(), Kind::Task, 2),
				("aidash://local/a".to_owned(), Kind::Event, 3),
				("aidash://local/b".to_owned(), Kind::Run, 2),
			]
		);
	}

	#[test]
	fn a_tool_result_carries_the_field_and_its_note_only_when_something_was_left_out() {
		let mut ordinary = serde_json::json!({"matches": [], "unconfirmed": []});
		for_tool(&mut ordinary);
		assert_eq!(ordinary, serde_json::json!({"matches": []}));

		let mut recall = serde_json::json!({"result": "recall", "value": "empty"});
		attach(&mut recall, vec![]);
		assert_eq!(
			recall,
			serde_json::json!({"result": "recall", "value": "empty"})
		);

		let entry = Unconfirmed {
			node: NODE.into(),
			kind: Kind::MemoryUnit,
			workspace_id: Uuid::from_u128(1),
			count: 2,
		};
		let mut left_out = serde_json::json!({"matches": [], "unconfirmed": [entry.clone()]});
		for_tool(&mut left_out);
		assert_eq!(left_out["unconfirmed"][0]["count"], 2);
		assert_eq!(left_out["unconfirmed"][0]["kind"], "memory_unit");
		assert_eq!(left_out["unconfirmed_note"], NOTE);
		let mut recalled = serde_json::json!({"result": "recall"});
		attach(&mut recalled, vec![entry]);
		assert_eq!(recalled["unconfirmed_note"], NOTE);
	}

	#[test]
	fn entries_added_to_a_tally_merge_with_its_rows() {
		let mut tally = Tally::from_entries(vec![Unconfirmed {
			node: NODE.into(),
			kind: Kind::SemanticEntry,
			workspace_id: Uuid::from_u128(1),
			count: 2,
		}]);
		tally.record(NODE, Kind::SemanticEntry, Uuid::from_u128(1));
		let mut other = Tally::new();
		other.record(NODE, Kind::SemanticEntry, Uuid::from_u128(1));
		tally.merge(other);
		assert_eq!(tally.into_entries()[0].count, 4);
	}

	#[test]
	fn kinds_serialize_in_snake_case() {
		assert_eq!(
			serde_json::to_value(Kind::HumanRequest).unwrap(),
			serde_json::json!("human_request")
		);
		assert_eq!(
			serde_json::to_value(Kind::SemanticEntry).unwrap(),
			serde_json::json!("semantic_entry")
		);
	}
}
