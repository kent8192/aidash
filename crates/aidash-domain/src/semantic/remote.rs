//! Home-owned semantic wire contracts and pure operation invariants.
use super::Failure;
use crate::registry::EntityRef;

/// Portable validation keeps malformed contracts, JSON failures and semantic
/// recovery reasons distinct at every adapter boundary.
#[derive(Debug, thiserror::Error)]
pub enum ContractError {
	#[error(transparent)]
	Domain(#[from] crate::Error),
	#[error(transparent)]
	Json(#[from] serde_json::Error),
	#[error(transparent)]
	Semantic(Failure),
}
pub type Result<T> = std::result::Result<T, ContractError>;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(rename = "RemoteSemanticRequest")]
pub enum Request {
	Disabled {},
	RequiredHome {
		embedding: EntityRef,
		/// An explicitly selected Home logical participant. Executor-local banks
		/// and similarly named Registry entries never substitute for this mapping.
		#[serde(default, skip_serializing_if = "Option::is_none")]
		native: Option<Box<NativeRequest>>,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		compactor: Option<EntityRef>,
		/// Exact Summary Stage model pinned by the Agent's Context Policy.
		/// Omitted for peers and Agents without a Summary Stage.
		#[serde(default, skip_serializing_if = "Option::is_none")]
		summarizer: Option<EntityRef>,
	},
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeRequest {
	pub participant: Uuid,
	pub expected_revision: i64,
	pub provider: EntityRef,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeBank {
	pub bank: crate::memory::Bank,
	pub provider: Provider,
	pub roles: Vec<Provider>,
	pub max_model_tokens: usize,
	pub max_context_tokens: usize,
	/// Home's pinned retention for disposable receiver quotations.
	pub cache_max_age_seconds: u64,
	pub cache_max_attempts: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeBinding {
	pub selection: NativeRequest,
	pub generation: Option<NativeOrigin>,
	pub participant: crate::memory::Binding,
	pub agent: Provider,
	pub banks: Vec<NativeBank>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeOrigin {
	pub node_id: String,
	pub intent_id: Uuid,
}
impl Default for Request {
	fn default() -> Self {
		Self::Disabled {}
	}
}
impl Request {
	pub fn compactor(&self) -> Option<&EntityRef> {
		match self {
			Self::Disabled {} => None,
			Self::RequiredHome { compactor, .. } => compactor.as_ref(),
		}
	}
	pub fn summarizer(&self) -> Option<&EntityRef> {
		match self {
			Self::Disabled {} => None,
			Self::RequiredHome { summarizer, .. } => summarizer.as_ref(),
		}
	}
}

/// The configuration digest includes the credential *reference*, never its value.
/// Only the owning node resolves that reference and executes the provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "RemoteSemanticProvider")]
pub struct Provider {
	pub node_id: String,
	pub entry: EntityRef,
	pub digest: String,
	pub configuration_digest: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(rename = "RemoteSemanticBinding")]
pub enum Binding {
	Disabled {},
	RequiredHome {
		home_lineage: Vec<crate::generation::remote::Ancestor>,
		execution_lineage: Vec<crate::generation::remote::Ancestor>,
		version: u32,
		/// Zero for native memory: index generations never identify canonical Unit reads.
		index_revision: i64,
		index_digest: String,
		embedding: Box<Provider>,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		native: Option<Box<NativeBinding>>,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		compactor: Option<Box<Provider>>,
		/// Home-disclosed Summary Stage model pin; absent means no Summary
		/// Stage may run for this remote execution.
		#[serde(default, skip_serializing_if = "Option::is_none")]
		summarizer: Option<Box<Provider>>,
	},
}
impl Default for Binding {
	fn default() -> Self {
		Self::Disabled {}
	}
}
impl Binding {
	pub fn disabled(&self) -> bool {
		matches!(self, Self::Disabled {})
	}
	pub fn request(&self) -> Request {
		match self {
			Self::Disabled {} => Request::Disabled {},
			Self::RequiredHome {
				embedding,
				compactor,
				summarizer,
				native,
				..
			} => Request::RequiredHome {
				embedding: embedding.entry.clone(),
				native: native
					.as_ref()
					.map(|binding| Box::new(binding.selection.clone())),
				compactor: compactor.as_ref().map(|p| p.entry.clone()),
				summarizer: summarizer.as_ref().map(|p| p.entry.clone()),
			},
		}
	}
}

impl Binding {
	pub fn summarizer(&self) -> Option<&Provider> {
		match self {
			Self::RequiredHome { summarizer, .. } => summarizer.as_deref(),
			Self::Disabled {} => None,
		}
	}
	pub fn native(&self) -> Option<&NativeBinding> {
		match self {
			Self::RequiredHome { native, .. } => native.as_deref(),
			Self::Disabled {} => None,
		}
	}
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Boundary {
	pub step: i32,
	pub input_sequence: i64,
	pub task_revision: i64,
	pub inputs_digest: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Operation {
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
	pub fn validate(&self) -> Result<()> {
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
			return Err(crate::Error::Invalid("invalid remote semantic operation".into()).into());
		}
		crate::configuration::validate_node_id(&self.home_node).map_err(Into::into)
	}
	pub fn digest(&self) -> Result<String> {
		Ok(crate::registry::rules::digest(&serde_json::to_value(self)?))
	}
	pub fn set_id(&mut self) -> Result<()> {
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
pub struct SourceRead {
	pub entry_id: Uuid,
	pub revision: i64,
	pub content_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
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
	pub result: super::results::SearchResult,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub memory: Option<NativeContext>,
	pub estimated_tokens: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeContext {
	pub banks: Vec<NativeRecall>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeRecall {
	pub bank: crate::memory::Bank,
	pub provider: EntityRef,
	pub recall: crate::memory::Recall,
}

/// Deterministic truncation is applied to the bytes actually sent and charged.
pub fn bounded_query(query: &str, max_bytes: usize) -> (String, bool) {
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

pub use super::InputRead;

#[cfg(test)]
mod operation_tests;

pub mod journal;

pub mod status;

pub mod disclosure;
