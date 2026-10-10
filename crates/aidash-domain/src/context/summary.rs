//! Execution Summary: the bounded, structured, merge-only Summary Stage output.
use super::{ContextEvent, HistoryEntry};
use crate::registry::EntityRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub const SUMMARY_VERSION: u8 = 1;
const MAX_ITEM_ID: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SummaryItem {
	pub id: String,
	pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResolvedItem {
	pub id: String,
	pub resolved_by: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtifactRef {
	/// Path, artifact ID or record reference exactly as it appeared in history.
	pub reference: String,
	/// Revision or digest when history stated one; otherwise empty.
	pub revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VerificationRef {
	/// Tool call ID or evidence reference from history. Never a model claim.
	pub reference: String,
	pub outcome: String,
}

/// Model-generated part of an Execution Summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SummaryContent {
	pub goal: String,
	pub constraints: Vec<SummaryItem>,
	pub decisions: Vec<String>,
	pub unresolved: Vec<SummaryItem>,
	/// Previous constraints or unresolved items explicitly closed by this merge.
	pub resolved: Vec<ResolvedItem>,
	pub artifacts: Vec<ArtifactRef>,
	pub verification: Vec<VerificationRef>,
}

/// Exact Context Journal range folded into the summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SummarySource {
	pub from_seq: u64,
	pub through_seq: u64,
	/// Inclusive journal sequence ranges absorbed by this summary and every
	/// summary it merged. Entries pruned between them were never absorbed.
	pub absorbed: Vec<[u64; 2]>,
	/// Digest of the projection entries absorbed by the latest merge.
	pub entries_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SummaryLink {
	pub through_seq: u64,
	pub digest: String,
}

impl SummarySource {
	/// Whether the journal entry at `seq` was folded into the summary.
	pub fn absorbs(&self, seq: u64) -> bool {
		self.absorbed
			.iter()
			.any(|[from, through]| (*from..=*through).contains(&seq))
	}
}

/// Sources whose authority the summarized text depends on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SummaryDependencies {
	pub message_ids: BTreeSet<uuid::Uuid>,
	pub tool_call_ids: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SummaryProvider {
	pub model: EntityRef,
	pub definition_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecutionSummary {
	pub version: u8,
	pub content: SummaryContent,
	pub source: SummarySource,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub previous: Option<SummaryLink>,
	pub dependencies: SummaryDependencies,
	pub policy_version: String,
	pub provider: SummaryProvider,
	pub digest: String,
}

/// Why a summary candidate was not adopted.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Rejection {
	#[error("summary output is not valid structured JSON")]
	Malformed,
	#[error("summary goal is empty")]
	EmptyGoal,
	#[error("summary item IDs must be unique, non-empty and at most 64 bytes")]
	InvalidItem,
	#[error("summary dropped previous item {0} without resolving it")]
	DroppedItem(String),
	#[error("summary changed previous item {0} without resolving it")]
	ChangedItem(String),
	#[error("summary resolved item {0} without a tool call from this merge as evidence")]
	UnprovenResolution(String),
	#[error("summary resolved item {0} but kept it active")]
	ResolvedItemActive(String),
	#[error("summary verification {0} names no tool call from its history")]
	UnprovenVerification(String),
	#[error("summary dropped or changed previous verification {0}")]
	DroppedVerification(String),
	#[error("summary exceeds its {0}-token bound")]
	Oversized(u32),
	#[error("summary did not reduce the complete request")]
	NotReduced,
}

impl Rejection {
	pub fn label(&self) -> &'static str {
		match self {
			Self::Malformed => "malformed",
			Self::EmptyGoal => "empty_goal",
			Self::InvalidItem => "invalid_item",
			Self::DroppedItem(_) => "dropped_item",
			Self::ChangedItem(_) => "changed_item",
			Self::UnprovenResolution(_) => "unproven_resolution",
			Self::ResolvedItemActive(_) => "resolved_item_active",
			Self::UnprovenVerification(_) => "unproven_verification",
			Self::DroppedVerification(_) => "dropped_verification",
			Self::Oversized(_) => "oversized",
			Self::NotReduced => "not_reduced",
		}
	}
}

impl SummaryContent {
	/// Parse provider text and enforce the merge contract against the previous
	/// summary and the `absorbed` history. Size reduction of the complete
	/// request is checked by the caller.
	pub fn parse(
		text: &str,
		previous: Option<&ExecutionSummary>,
		absorbed: &[HistoryEntry],
		max_tokens: u32,
	) -> Result<Self, Rejection> {
		let content: Self = serde_json::from_str(text.trim()).map_err(|_| Rejection::Malformed)?;
		if content.goal.trim().is_empty() {
			return Err(Rejection::EmptyGoal);
		}
		let mut ids = BTreeSet::new();
		for item in content.constraints.iter().chain(&content.unresolved) {
			if item.id.trim().is_empty()
				|| item.id.len() > MAX_ITEM_ID
				|| item.text.trim().is_empty()
				|| !ids.insert(item.id.as_str())
			{
				return Err(Rejection::InvalidItem);
			}
		}
		// Only tool calls this merge absorbs are evidence; model text never is.
		let evidence: BTreeSet<&str> = absorbed
			.iter()
			.filter_map(|entry| match &entry.event {
				ContextEvent::Tool { call, .. } => Some(call.id.as_str()),
				_ => None,
			})
			.collect();
		let resolved = content
			.resolved
			.iter()
			.map(|item| {
				if item.resolved_by.trim().is_empty() {
					Err(Rejection::InvalidItem)
				} else if !evidence.contains(item.resolved_by.as_str()) {
					Err(Rejection::UnprovenResolution(item.id.clone()))
				} else {
					Ok(item.id.as_str())
				}
			})
			.collect::<Result<BTreeSet<_>, _>>()?;
		// A resolution closes its item. The model view omits `resolved`, so a
		// resolved ID left active could carry rewritten requirement text.
		if let Some(id) = resolved.iter().find(|id| ids.contains(**id)) {
			return Err(Rejection::ResolvedItemActive((*id).to_owned()));
		}
		// A verification names an absorbed tool call, or is carried unchanged
		// from the previous summary, which validated it when it was merged.
		for verification in &content.verification {
			if !evidence.contains(verification.reference.as_str())
				&& !previous
					.is_some_and(|previous| previous.content.verification.contains(verification))
			{
				return Err(Rejection::UnprovenVerification(
					verification.reference.clone(),
				));
			}
		}
		// The checked events left the projection with the earlier merge, so a
		// previous verification is its only model-visible record: keep it.
		if let Some(previous) = previous
			&& let Some(dropped) = previous
				.content
				.verification
				.iter()
				.find(|verification| !content.verification.contains(verification))
		{
			return Err(Rejection::DroppedVerification(dropped.reference.clone()));
		}
		if let Some(previous) = previous {
			// A retained item keeps its list and exact text; only an explicit
			// resolution, which removes the item, may close it.
			for (current, items) in [
				(&content.constraints, &previous.content.constraints),
				(&content.unresolved, &previous.content.unresolved),
			] {
				for item in items {
					if resolved.contains(item.id.as_str()) {
						continue;
					}
					if !ids.contains(item.id.as_str()) {
						return Err(Rejection::DroppedItem(item.id.clone()));
					}
					if !current.contains(item) {
						return Err(Rejection::ChangedItem(item.id.clone()));
					}
				}
			}
		}
		if super::estimated_tokens(
			&serde_json::to_string(&content).map_err(|_| Rejection::Malformed)?,
		) > max_tokens as usize
		{
			return Err(Rejection::Oversized(max_tokens));
		}
		Ok(content)
	}

	/// OpenRouter structured-output request for the content schema.
	pub fn response_format() -> Value {
		let item = json!({"type":"object","additionalProperties":false,"required":["id","text"],"properties":{"id":{"type":"string"},"text":{"type":"string"}}});
		json!({"type":"json_schema","json_schema":{"name":"execution_summary","strict":true,"schema":{
			"type":"object","additionalProperties":false,
			"required":["goal","constraints","decisions","unresolved","resolved","artifacts","verification"],
			"properties":{
				"goal":{"type":"string"},
				"constraints":{"type":"array","items":item},
				"decisions":{"type":"array","items":{"type":"string"}},
				"unresolved":{"type":"array","items":item},
				"resolved":{"type":"array","items":{"type":"object","additionalProperties":false,"required":["id","resolved_by"],"properties":{"id":{"type":"string"},"resolved_by":{"type":"string"}}}},
				"artifacts":{"type":"array","items":{"type":"object","additionalProperties":false,"required":["reference","revision"],"properties":{"reference":{"type":"string"},"revision":{"type":"string"}}}},
				"verification":{"type":"array","items":{"type":"object","additionalProperties":false,"required":["reference","outcome"],"properties":{"reference":{"type":"string"},"outcome":{"type":"string"}}}}
			}
		}}})
	}
}

/// Events the Summary Stage may absorb: tool results, whose sources the
/// summary tracks as dependencies. Human answers, corrections, continuation
/// markers and media observations, whose source messages a summary could not
/// recheck, always stay verbatim in the projection.
pub fn absorbable(event: &ContextEvent) -> bool {
	matches!(event, ContextEvent::Tool { .. })
}

pub fn entries_digest(entries: &[HistoryEntry]) -> String {
	crate::registry::rules::digest(&json!(entries))
}

impl ExecutionSummary {
	/// Compose an adopted summary from validated content. `absorbed` must be
	/// non-empty and strictly ordered; the merged range starts at the previous
	/// summary.
	pub fn merge(
		content: SummaryContent,
		previous: Option<&ExecutionSummary>,
		absorbed: &[HistoryEntry],
		mut dependencies: SummaryDependencies,
		policy_version: &str,
		provider: SummaryProvider,
	) -> crate::Result<Self> {
		let (Some(first), Some(last)) = (absorbed.first(), absorbed.last()) else {
			return Err(crate::Error::Invalid(
				"summary merge requires absorbed history".into(),
			));
		};
		if absorbed.windows(2).any(|pair| pair[0].seq >= pair[1].seq) {
			return Err(crate::Error::Invalid(
				"summary merge requires strictly ordered history".into(),
			));
		}
		if let Some(previous) = previous {
			if first.seq <= previous.source.through_seq {
				return Err(crate::Error::Invalid(
					"summary merge overlaps the previous summary range".into(),
				));
			}
			dependencies
				.message_ids
				.extend(previous.dependencies.message_ids.iter().copied());
			dependencies
				.tool_call_ids
				.extend(previous.dependencies.tool_call_ids.iter().cloned());
		}
		let mut ranges = previous.map_or_else(Vec::new, |p| p.source.absorbed.clone());
		for entry in absorbed {
			match ranges.last_mut() {
				Some(range) if range[1] + 1 == entry.seq => range[1] = entry.seq,
				_ => ranges.push([entry.seq, entry.seq]),
			}
		}
		let mut summary = Self {
			version: SUMMARY_VERSION,
			content,
			source: SummarySource {
				from_seq: previous.map_or(first.seq, |p| p.source.from_seq),
				through_seq: last.seq,
				absorbed: ranges,
				entries_digest: entries_digest(absorbed),
			},
			previous: previous.map(|p| SummaryLink {
				through_seq: p.source.through_seq,
				digest: p.digest.clone(),
			}),
			dependencies,
			policy_version: policy_version.into(),
			provider,
			digest: String::new(),
		};
		summary.digest = crate::registry::rules::digest(&serde_json::to_value(&summary)?);
		Ok(summary)
	}

	/// Provider-visible form. Provenance and authority data stay out of it.
	pub fn model_view(&self) -> Value {
		let content = &self.content;
		json!({
			"goal": content.goal,
			"constraints": content.constraints,
			"decisions": content.decisions,
			"unresolved": content.unresolved,
			"artifacts": content.artifacts,
			"verification": content.verification,
			"covers_history_seq": [self.source.from_seq, self.source.through_seq],
		})
	}
}

#[cfg(test)]
mod tests;
