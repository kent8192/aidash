//! Immutable Research journal references and their ephemeral, model-visible value.
use super::{ContextEvent, HistoryEntry};
use crate::research::{Operation, expiry::ExpiredResults};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

pub const CONTRACT: &str = "aidash.research.reference/1";

/// The exact delivered variant, with the provenance needed to remove every
/// variant when its Provider result expires. No provider content is retained.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchReference {
	pub contract: String,
	pub invocation: String,
	pub digest: String,
	pub operation: Operation,
	pub sources: Vec<Uuid>,
	pub documents: Vec<Uuid>,
}

impl ResearchReference {
	pub fn from_result(invocation: &str, value: &Value) -> Option<Self> {
		if value["contract"] != crate::research::RESULT_CONTRACT {
			return None;
		}
		let operation = Operation::parse(value["operation"].as_str()?)?;
		let uuid = |value: &Value| value.as_str().and_then(|id| id.parse().ok());
		Some(Self {
			contract: CONTRACT.into(),
			invocation: invocation.into(),
			digest: crate::registry::rules::digest(value),
			operation,
			sources: if operation == Operation::Search {
				value["data"]["sources"]
					.as_array()
					.into_iter()
					.flatten()
					.filter_map(|source| uuid(&source["source_id"]))
					.collect()
			} else {
				Vec::new()
			},
			documents: if operation == Operation::Search {
				Vec::new()
			} else {
				uuid(&value["data"]["document_id"]).into_iter().collect()
			},
		})
	}
	pub fn parse(value: &Value) -> Option<Self> {
		if value["contract"] != CONTRACT {
			return None;
		}
		let reference: Self = serde_json::from_value(value.clone()).ok()?;
		(reference
			.digest
			.strip_prefix("sha256:")
			.is_some_and(|hash| {
				hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
			}) && !reference.invocation.is_empty())
		.then_some(reference)
	}
	pub fn covered_by(&self, expired: &ExpiredResults) -> bool {
		self.sources.iter().any(|id| expired.sources.contains(id))
			|| self
				.documents
				.iter()
				.any(|id| expired.documents.contains(id))
	}
	pub fn unavailable(&self) -> Value {
		json!({"contract":"aidash.research.unavailable/1","invocation":self.invocation,
			"digest":self.digest,"explanation":"The referenced Research result is unavailable; its contents cannot be read or cited."})
	}
}

impl HistoryEntry {
	pub fn new(seq: u64, event: ContextEvent) -> Self {
		let research = match &event {
			ContextEvent::Tool { call, result } => ResearchReference::parse(result).or_else(|| {
				ResearchReference::from_result(&format!("journal:{seq}:{}", call.id), result)
			}),
			_ => None,
		};
		Self {
			seq,
			event,
			research,
		}
	}
	/// Give a newly delivered result the durable invocation identity used by
	/// its Research attempt. Find and View have the same identity even though
	/// they do not charge a provider attempt.
	pub fn with_invocation(mut self, invocation: &str) -> Self {
		if let Some(reference) = &mut self.research {
			reference.invocation = invocation.into();
		}
		self
	}
	pub fn research_reference(&self) -> Option<ResearchReference> {
		let ContextEvent::Tool { call, result } = &self.event else {
			return None;
		};
		let mut reference = self
			.research
			.clone()
			.or_else(|| ResearchReference::parse(result))
			.or_else(|| {
				ResearchReference::from_result(&format!("journal:{}:{}", self.seq, call.id), result)
			})?;
		// Placeholders never change journal identity. Pruning a retained value
		// creates a separately owned, content-addressed variant with the same
		// provenance, so a purge reaches the shortened copy too.
		if ResearchReference::parse(result).is_none()
			&& !ContextEvent::is_expired_result(result)
			&& !ContextEvent::is_revoked_result(result)
			&& result["contract"] != "aidash.research.unavailable/1"
		{
			reference.digest = crate::registry::rules::digest(result);
		}
		Some(reference)
	}
	/// Canonical durable bytes; model and summary requests use `event`
	/// directly, after the owner resolves the reference.
	pub fn durable_event(&self) -> Value {
		let mut event = serde_json::to_value(&self.event).expect("ContextEvent is JSON");
		if let Some(reference) = self.research_reference() {
			event["result"] = json!(reference);
		}
		event
	}
}

impl Serialize for HistoryEntry {
	fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
		use serde::ser::SerializeStruct;
		let event = self.durable_event();
		// Tool JSON may happen to use our contract marker without being an
		// internal reference. Escape that ordinary value explicitly rather
		// than relaxing validation of damaged durable references.
		let inline_result = event["result"]["contract"] == CONTRACT
			&& ResearchReference::parse(&event["result"]).is_none();
		let mut entry =
			serializer.serialize_struct("HistoryEntry", 2 + usize::from(inline_result))?;
		entry.serialize_field("seq", &self.seq)?;
		entry.serialize_field("event", &event)?;
		if inline_result {
			entry.serialize_field("inline_result", &true)?;
		}
		entry.end()
	}
}
impl<'de> Deserialize<'de> for HistoryEntry {
	fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		#[derive(Deserialize)]
		#[serde(deny_unknown_fields)]
		struct Stored {
			seq: u64,
			event: ContextEvent,
			#[serde(default)]
			inline_result: bool,
		}
		let stored = Stored::deserialize(deserializer)?;
		let inline_result = matches!(&stored.event, ContextEvent::Tool { result, .. }
			if result["contract"] == CONTRACT && ResearchReference::parse(result).is_none());
		if inline_result && !stored.inline_result {
			return Err(serde::de::Error::custom(
				"invalid Research journal reference",
			));
		}
		if stored.inline_result && !inline_result {
			return Err(serde::de::Error::custom(
				"invalid inline Research result marker",
			));
		}
		Ok(Self::new(stored.seq, stored.event))
	}
}

#[cfg(test)]
mod tests;
