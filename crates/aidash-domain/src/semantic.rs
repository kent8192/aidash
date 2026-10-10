use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Public reasons contain no query, source ID, upstream body, or credential.
#[derive(
	Debug,
	Clone,
	Copy,
	PartialEq,
	Eq,
	Serialize,
	Deserialize,
	schemars::JsonSchema,
	thiserror::Error,
)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "RemoteSemanticFailure")]
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
	pub fn transient(self) -> bool {
		matches!(self, Self::Unavailable)
	}
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "SemanticEmbeddingConfig")]
#[serde(deny_unknown_fields)]
pub struct EmbeddingConfig {
	pub provider: String,
	pub endpoint: String,
	pub credential_env: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub provider_credential: Option<String>,
	pub model: String,
	pub model_version: String,
	pub dimensions: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "SemanticVectorConfig")]
#[serde(deny_unknown_fields)]
pub struct VectorConfig {
	pub provider: String,
	pub endpoint: String,
	pub credential_env: Option<String>,
}

/// Result of one approved embedding request.
#[derive(Debug)]
pub struct Embedding {
	pub vector: Vec<f32>,
	pub tokens: Option<u64>,
}

/// Candidate data remains untrusted until the use case checks database authority.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct Point {
	pub id: Uuid,
	pub score: f32,
	pub payload: Value,
}

/// Scope comes from current PostgreSQL authority, never from vector payloads.
pub struct VectorFilter<'a> {
	pub allowed: &'a [Uuid],
	pub workspace: Uuid,
	pub tenant: &'a str,
}

impl EmbeddingConfig {
	/// Provider definition invariants do not read credentials or contact a provider.
	pub fn validate_parameters(&self) -> crate::Result<()> {
		crate::provider_credentials::validate_source(
			&self.endpoint,
			&self.provider,
			self.credential_env.as_deref(),
			self.provider_credential.as_deref(),
		)?;
		if !matches!(self.provider.as_str(), "openai" | "openrouter")
			|| self.model.trim().is_empty()
			|| self.model.len() > 256
			|| self.model_version.trim().is_empty()
			|| self.model_version.len() > 128
			|| !(1..=8192).contains(&self.dimensions)
		{
			return Err(crate::Error::Invalid(
				"invalid embedding provider, model, version or dimensions".into(),
			));
		}
		Ok(())
	}
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputRead {
	pub id: Uuid,
	pub sequence: i64,
	pub digest: String,
}

pub mod remote;
pub mod results;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "SemanticSource")]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Source {
	/// Canonical text and authority are resolved from a native memory unit.
	Unit {
		id: Uuid,
	},
	Memory {
		text: String,
	},
	Artifact {
		id: Uuid,
	},
	Message {
		id: Uuid,
	},
}

impl Source {
	pub fn same_origin(&self, other: &Self) -> bool {
		match (self, other) {
			(Self::Memory { .. }, Self::Memory { .. }) => true,
			_ => self == other,
		}
	}
}

pub mod indexing;

pub mod mutations;

pub mod retrieval;
