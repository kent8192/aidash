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
	#[error("Semantic retrieval configuration is unavailable or changed")]
	Configuration,
	#[error("Home semantic retrieval requires current authority at both nodes")]
	Authority,
	#[error("A consumed semantic source changed; create a follow-up Task")]
	Invalidated,
	#[error("The semantic provider violated its approved response contract")]
	ProviderContract,
	#[error("The embedding provider cannot find the configured model")]
	EmbeddingModelNotFound,
	#[error("Semantic context does not fit the available context budget")]
	ContextBudget,
	#[error(
		"The native-memory operation's durable model budget is exhausted; create a follow-up Task"
	)]
	DurableMemoryBudget,
	#[error("The generated lineage has insufficient provider allowance")]
	Allowance,
	#[error("The semantic backend is temporarily unavailable")]
	Unavailable,
	#[error("Semantic retries are exhausted; an authorized manual retry is required")]
	RetriesExhausted,
	#[error("The semantic operation is already in progress")]
	Pending,
	/// A reused retrieval no longer matches the Home corpus; the Run retrieves
	/// it again instead of pausing.
	#[error("The reused semantic retrieval is out of date; retrieve it again")]
	Stale,
}

/// Delay between repeated replays of a semantic operation that is still
/// `Pending`. Each replay repeats the charged workspace embedding, so only the
/// first one comes after a second. None of them spends an outage retry (#233).
pub const PENDING_REPLAY_SECONDS: i64 = 30;

/// Retries of a semantic outage before a Run pauses, or degrades an opted-in
/// workspace slot (ADR 0059), after delays of 2, 4, 8, 16 and 32 s.
pub const OUTAGE_RETRIES: u32 = 5;

impl Failure {
	/// Established peers must still forbid retrying an irrecoverably spent operation.
	pub fn compatible(self) -> Self {
		match self {
			Self::DurableMemoryBudget => Self::ContextBudget,
			Self::EmbeddingModelNotFound => Self::ProviderContract,
			other => other,
		}
	}

	pub fn transient(self) -> bool {
		matches!(self, Self::Unavailable)
	}
}

#[derive(
	Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingInputKind {
	Text,
	Image,
}

fn default_embedding_inputs() -> std::collections::BTreeSet<EmbeddingInputKind> {
	[EmbeddingInputKind::Text].into_iter().collect()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "SemanticEmbeddingConfig")]
#[serde(deny_unknown_fields)]
pub struct EmbeddingConfig {
	pub provider: String,
	pub endpoint: String,
	/// As for models: a Tenant Provider Credential, or no key (ADR 0006).
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub provider_credential: Option<String>,
	pub model: String,
	pub model_version: String,
	pub dimensions: usize,
	#[serde(default = "default_embedding_inputs")]
	pub inputs: std::collections::BTreeSet<EmbeddingInputKind>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "SemanticVectorConfig")]
#[serde(deny_unknown_fields)]
pub struct VectorConfig {
	pub provider: String,
	pub endpoint: String,
	pub credential_env: Option<String>,
}

/// A single input, borrowed so image bytes never enter durable point payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddingInput<'a> {
	Text(&'a str),
	Image {
		media_type: &'a str,
		bytes: &'a [u8],
	},
}

impl EmbeddingInput<'_> {
	pub fn validate(&self, config: &EmbeddingConfig) -> crate::Result<()> {
		config.validate_parameters()?;
		let kind = match self {
			Self::Text(_) => EmbeddingInputKind::Text,
			Self::Image { .. } => EmbeddingInputKind::Image,
		};
		if !config.inputs.contains(&kind) {
			return Err(crate::Error::Invalid(
				"embedding input modality is not supported".into(),
			));
		}
		if let Self::Image { media_type, bytes } = self {
			crate::provider::validate_image(media_type, bytes)?;
		}
		Ok(())
	}
	pub fn byte_len(&self) -> usize {
		match self {
			Self::Text(text) => text.len(),
			Self::Image { bytes, .. } => bytes.len(),
		}
	}
	pub fn reservation_amount(&self) -> crate::Result<i64> {
		match self {
			Self::Text(text) => crate::generation::embedding::reservation_amount(text.len()),
			Self::Image { media_type, bytes } => {
				let size = crate::provider::image_token_allowance(bytes.len())
					.and_then(|v| v.checked_add(media_type.len()))
					.and_then(|v| v.checked_add(1024))
					.and_then(|v| i64::try_from(v).ok())
					.ok_or_else(|| {
						crate::Error::Invalid("embedding input reservation overflow".into())
					})?;
				Ok(size)
			}
		}
	}
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
			self.provider_credential.as_deref(),
		)?;
		if !matches!(self.provider.as_str(), "openai" | "openrouter")
			|| self.model.trim().is_empty()
			|| self.model.len() > 256
			|| self.model_version.trim().is_empty()
			|| self.model_version.len() > 128
			|| !(1..=8192).contains(&self.dimensions)
			|| !self.inputs.contains(&EmbeddingInputKind::Text)
		{
			return Err(crate::Error::Invalid(
				"invalid embedding provider, model, version, dimensions or inputs".into(),
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
	/// An immutable File version; source content remains in its Working Area.
	WorkingImage {
		area_id: Uuid,
		generation: i64,
		file_id: Uuid,
		path: String,
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

pub mod degraded;
pub mod indexing;

pub mod mutations;

pub mod recovery;

pub mod retrieval;
