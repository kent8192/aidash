//! Degraded context (ADR 0059): a local Run step whose optional Workspace
//! semantic context stayed unavailable after the bounded retry continues with a
//! typed marker in the workspace slot instead of pausing, when every source of
//! that slot opted in.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The outage that waiting can fix. Missing vector points are not one: the
/// index needs repair, so they pause the Run instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "SemanticOutageCause")]
pub enum OutageCause {
	EmbeddingProvider,
	VectorBackend,
	/// The index is still being built; only its READY part was read (ADR 0129).
	IndexIncomplete,
}

/// The marker that stands in the workspace slot of `semantic_memory`, in the
/// shape of a memory bank status. It names no endpoint, body or query.
pub fn marker(cause: OutageCause) -> Value {
	json!({"status":"unavailable","cause":cause})
}

/// The READY part of an incomplete index, marked as Degraded context: the
/// search result with `status` and `cause` beside it (ADR 0129).
pub fn partial(mut result: Value) -> Value {
	if let Some(object) = result.as_object_mut() {
		object.insert("status".into(), json!("incomplete"));
		object.insert("cause".into(), json!(OutageCause::IndexIncomplete));
	}
	result
}

/// The cause when `slot` is the marker or a [`partial`] result.
pub fn cause(slot: &Value) -> Option<OutageCause> {
	if !matches!(slot.get("status")?.as_str()?, "unavailable" | "incomplete") {
		return None;
	}
	serde_json::from_value(slot.get("cause")?.clone()).ok()
}

/// Whether a workspace slot holds reads a step must recheck before it adopts
/// an answer built on them: a search result, complete or [`partial`]. An absent
/// slot and the outage [`marker`] hold none.
pub fn holds_reads(slot: &Value) -> bool {
	match slot.get("status") {
		None => !slot.is_null(),
		Some(status) => status == "incomplete",
	}
}

/// The cause of a degraded workspace slot in `semantic_memory`: the envelope's
/// workspace slot, or the bare slot when no memory was combined with it.
pub fn degraded(semantic: &Value) -> Option<OutageCause> {
	cause(semantic.get("workspace").unwrap_or(semantic))
}

/// Steps kept in `recent`; older ones are only counted.
pub const MAX_RECENT_DEGRADED_STEPS: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DegradedStep {
	pub step: i32,
	pub cause: OutageCause,
}

/// What a Run remembers about its Degraded context.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DegradedContext {
	/// Every step that ran with Degraded context.
	pub count: u32,
	/// The latest of those steps, oldest first.
	pub recent: Vec<DegradedStep>,
	/// Set by a degraded step and cleared by a retrieval that succeeds. While
	/// it is set a step tries once and degrades at once if that try fails,
	/// instead of waiting for the bounded retry again.
	pub outage: bool,
}
impl DegradedContext {
	pub fn is_empty(&self) -> bool {
		*self == Self::default()
	}
	/// Record `step` as degraded. Returns whether the step is new; a recovery
	/// of the same step that degrades again is still one step.
	pub fn record(&mut self, step: i32, cause: OutageCause) -> bool {
		self.outage = true;
		if let Some(last) = self.recent.last_mut()
			&& last.step == step
		{
			last.cause = cause;
			return false;
		}
		self.count = self.count.saturating_add(1);
		self.recent.push(DegradedStep { step, cause });
		let excess = self.recent.len().saturating_sub(MAX_RECENT_DEGRADED_STEPS);
		self.recent.drain(..excess);
		true
	}
	pub fn recovered(&mut self) {
		self.outage = false;
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_marker_round_trips_and_other_statuses_are_not_degraded() {
		let envelope =
			json!({"workspace":marker(OutageCause::VectorBackend),"memory":{"status":"disabled"}});
		assert_eq!(
			envelope["workspace"],
			json!({"status":"unavailable","cause":"vector_backend"})
		);
		assert_eq!(degraded(&envelope), Some(OutageCause::VectorBackend));
		assert_eq!(
			degraded(&marker(OutageCause::EmbeddingProvider)),
			Some(OutageCause::EmbeddingProvider)
		);
		for workspace in [
			json!(null),
			json!({"status":"no_space"}),
			json!({"status":"unavailable","cause":"missing_points"}),
			json!({"matches":[]}),
		] {
			assert_eq!(degraded(&json!({"workspace":workspace})), None);
		}
	}

	#[test]
	fn a_partial_result_holds_reads_to_recheck_and_the_marker_holds_none() {
		let result = json!({"workspace_id":"w","index_revision":1,"matches":[{"text":"x"}]});
		assert!(holds_reads(&result));
		// The READY part of an incomplete index carries real matches (ADR 0129).
		assert!(holds_reads(&partial(result)));
		for slot in [
			json!(null),
			marker(OutageCause::VectorBackend),
			marker(OutageCause::IndexIncomplete),
			json!({"status":"unavailable","cause":"missing_points"}),
			json!({"status":"no_space"}),
		] {
			assert!(!holds_reads(&slot), "{slot}");
		}
	}

	#[test]
	fn recent_steps_are_bounded_while_the_count_keeps_every_step() {
		let mut context = DegradedContext::default();
		for step in 0..20 {
			assert!(context.record(step, OutageCause::EmbeddingProvider));
		}
		assert!(!context.record(19, OutageCause::VectorBackend));
		assert_eq!(context.count, 20);
		assert_eq!(context.recent.len(), MAX_RECENT_DEGRADED_STEPS);
		assert_eq!(context.recent.first().unwrap().step, 4);
		assert_eq!(
			context.recent.last().copied(),
			Some(DegradedStep {
				step: 19,
				cause: OutageCause::VectorBackend
			})
		);
		assert!(context.outage);
		context.recovered();
		assert!(!context.outage && !context.is_empty());
	}
}
