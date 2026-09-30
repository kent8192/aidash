//! Wire bindings for Home-owned semantic context. IDs in this protocol identify
//! durable records; they never substitute for live peer and subject authority.
pub(crate) mod journal;
pub mod status;

use crate::{Error, Result, registry::EntityRef};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub(crate) const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
#[schema(as = RemoteSemanticRequest)]
pub enum Request {
	Disabled {},
	RequiredHome {
		embedding: EntityRef,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		compactor: Option<EntityRef>,
	},
}
impl Default for Request {
	fn default() -> Self {
		Self::Disabled {}
	}
}
impl Request {
	pub(crate) fn compactor(&self) -> Option<&EntityRef> {
		match self {
			Self::Disabled {} => None,
			Self::RequiredHome { compactor, .. } => compactor.as_ref(),
		}
	}
}

/// The configuration digest includes the credential *reference*, never its value.
/// Only the owning node resolves that reference and executes the provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = RemoteSemanticProvider)]
pub struct Provider {
	pub node_id: String,
	pub entry: EntityRef,
	pub digest: String,
	pub configuration_digest: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
#[schema(as = RemoteSemanticBinding)]
pub enum Binding {
	Disabled {},
	RequiredHome {
		home_lineage: Vec<crate::generation::remote::Ancestor>,
		execution_lineage: Vec<crate::generation::remote::Ancestor>,
		version: u32,
		index_revision: i64,
		index_digest: String,
		embedding: Box<Provider>,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		compactor: Option<Box<Provider>>,
	},
}
impl Default for Binding {
	fn default() -> Self {
		Self::Disabled {}
	}
}
impl Binding {
	pub(crate) fn disabled(&self) -> bool {
		matches!(self, Self::Disabled {})
	}
	pub(crate) fn request(&self) -> Request {
		match self {
			Self::Disabled {} => Request::Disabled {},
			Self::RequiredHome {
				embedding,
				compactor,
				..
			} => Request::RequiredHome {
				embedding: embedding.entry.clone(),
				compactor: compactor.as_ref().map(|p| p.entry.clone()),
			},
		}
	}
}

/// Public reasons contain no query, source ID, upstream body, or credential.
#[derive(
	Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema, thiserror::Error,
)]
#[serde(rename_all = "snake_case")]
#[schema(as = RemoteSemanticFailure)]
pub enum Failure {
	#[error("Home semantic retrieval configuration is unavailable or changed")]
	Configuration,
	#[error("Home semantic retrieval requires current authority at both nodes")]
	Authority,
	#[error("A consumed semantic source changed; create a follow-up Task")]
	Invalidated,
	#[error("The semantic provider violated its approved response contract")]
	ProviderContract,
	#[error("Semantic context does not fit the available context budget")]
	ContextBudget,
	#[error("The generated lineage has insufficient provider allowance")]
	Allowance,
	#[error("The Home semantic backend is temporarily unavailable")]
	Unavailable,
	#[error("Semantic retries are exhausted; an authorized manual retry is required")]
	RetriesExhausted,
	#[error("The semantic operation is already in progress")]
	Pending,
}
impl Failure {
	pub(crate) fn transient(self) -> bool {
		matches!(self, Self::Unavailable)
	}
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Boundary {
	pub step: i32,
	pub input_sequence: i64,
	pub task_revision: i64,
	pub inputs_digest: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InputRead {
	pub id: Uuid,
	pub sequence: i64,
	pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Operation {
	pub id: Uuid,
	pub home_node: String,
	pub grant_id: Uuid,
	pub admission_id: Uuid,
	pub boundary: Boundary,
	pub inputs: Vec<InputRead>,
	pub query: String,
	pub max_tokens: usize,
	pub metadata: Value,
}
impl Operation {
	pub(crate) fn validate(&self) -> Result<()> {
		if self.id.is_nil()
			|| self.grant_id.is_nil()
			|| self.admission_id.is_nil()
			|| self.query.trim().is_empty()
			|| self.query.len() > 32768
			|| !(1..=32768).contains(&self.max_tokens)
			|| self.boundary.step < 0
			|| self.boundary.input_sequence < 0
			|| self.inputs.len() > 200
			|| self
				.inputs
				.windows(2)
				.any(|pair| pair[0].sequence >= pair[1].sequence)
			|| self
				.inputs
				.iter()
				.any(|input| input.sequence <= 0 || input.sequence > self.boundary.input_sequence)
			|| !self.metadata.is_object()
			|| serde_json::to_vec(&self.metadata)?.len() > 4096
		{
			return Err(Error::Invalid("invalid remote semantic operation".into()));
		}
		crate::config::validate_node_id(&self.home_node)
	}
	pub(crate) fn digest(&self) -> Result<String> {
		Ok(crate::registry::digest(&serde_json::to_value(self)?))
	}
	pub(crate) fn set_id(&mut self) -> Result<()> {
		use sha2::{Digest, Sha256};
		self.id = Uuid::nil();
		let hash = Sha256::digest(serde_json::to_vec(self)?);
		let mut bytes = [0; 16];
		bytes.copy_from_slice(&hash[..16]);
		bytes[6] = (bytes[6] & 0x0f) | 0x80;
		bytes[8] = (bytes[8] & 0x3f) | 0x80;
		self.id = Uuid::from_bytes(bytes);
		Ok(())
	}
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SourceRead {
	pub entry_id: Uuid,
	pub revision: i64,
	pub content_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Receipt {
	pub operation_id: Uuid,
	pub operation_digest: String,
	pub home_node: String,
	pub tenant: String,
	pub workspace_id: Uuid,
	pub grant_id: Uuid,
	pub admission_id: Uuid,
	pub executor: String,
	pub binding: Binding,
	pub retrieved_at: DateTime<Utc>,
	pub query_truncated: bool,
	pub candidate_digest: String,
	pub sources: Vec<SourceRead>,
	pub result: super::SearchResult,
	pub estimated_tokens: usize,
}

/// Deterministic truncation is applied to the bytes actually sent and charged.
pub(crate) fn bounded_query(query: &str, max_bytes: usize) -> (String, bool) {
	let mut end = query.len().min(max_bytes);
	while !query.is_char_boundary(end) {
		end -= 1;
	}
	(query[..end].to_owned(), end < query.len())
}

#[cfg(test)]
mod tests {
	use super::*;
	#[rstest::rstest]
	#[case(0, "", true)]
	#[case(1, "a", true)]
	#[case(3, "a", true)]
	#[case(4, "a日", true)]
	#[case(7, "a日本", false)]
	fn query_limits_count_bytes_without_splitting_utf8(
		#[case] limit: usize,
		#[case] text: &str,
		#[case] truncated: bool,
	) {
		assert_eq!(bounded_query("a日本", limit), (text.into(), truncated));
	}
	#[test]
	fn disabled_is_the_only_implicit_mode() {
		assert!(
			serde_json::from_value::<Binding>(
				serde_json::json!({"mode":"disabled","embedding":{}})
			)
			.is_err()
		);
		assert_eq!(
			serde_json::to_value(Binding::default()).unwrap(),
			serde_json::json!({"mode":"disabled"})
		);
		assert_eq!(Request::default(), Request::Disabled {});
		assert_eq!(Binding::default(), Binding::Disabled {});
		assert!(
			serde_json::from_value::<Request>(serde_json::json!({"mode":"required_home"})).is_err()
		);
		assert!(
			serde_json::from_value::<Request>(
				serde_json::json!({"mode":"disabled","embedding":{}})
			)
			.is_err()
		);
	}
}
