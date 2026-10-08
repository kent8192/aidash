//! Native Hindsight contracts. Memory content is a typed unit, never an arbitrary JSON bank.
//!
//! The upstream MIT notice is in `memory/LICENSE.hindsight`.
use crate::{Error, Result, registry::EntityRef};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub mod consolidation;
pub mod extraction;
pub mod graph;
pub mod recall;
pub mod recovery;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
	pub engine: EngineKind,
	pub policy: Policy,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EngineKind {
	HindsightRust,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceConfig {
	pub scope: SourceScope,
	pub memory: EntityRef,
	pub max_tokens: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceScope {
	Participant,
	Workspace,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "provider", rename_all = "snake_case", deny_unknown_fields)]
pub enum RerankerConfig {
	Model {
		model: EntityRef,
	},
	/// Explicitly registered RRF passthrough, never a transport failure fallback.
	Rrf,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "provider", rename_all = "snake_case", deny_unknown_fields)]
pub enum TokenizerConfig {
	/// Conservative UTF-8 upper bound; the entire serialized envelope is counted.
	Utf8UpperBound,
}

/// Issued by the Home authority for a participant, independently of a definition version.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Bank {
	pub home: String,
	pub tenant: String,
	pub workspace: Uuid,
	/// `None` is the explicitly published Workspace bank.
	pub participant: Option<Uuid>,
}
impl Bank {
	pub fn validate(&self) -> Result<()> {
		if self.home.trim().is_empty()
			|| self.tenant.trim().is_empty()
			|| self.workspace.is_nil()
			|| self.participant.is_some_and(|id| id.is_nil())
		{
			return Err(Error::Invalid("invalid memory bank identity".into()));
		}
		Ok(())
	}
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Binding {
	pub bank: Bank,
	pub participant_revision: i64,
	pub agent: EntityRef,
	pub provider: EntityRef,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
	World,
	Experience,
	Observation,
	MentalModel,
}
impl Kind {
	pub fn derived(self) -> bool {
		matches!(self, Self::Observation | Self::MentalModel)
	}
}

/// Verification is separate from admission and model confidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Verification {
	Unverified,
	Supported,
	Contradicted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Learning {
	Fact,
	Preference,
	Procedure,
	Failure,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Evidence {
	/// Exact selected disclosure grant; source lineage is resolved at the Home.
	Publication {
		id: Uuid,
		revision: i64,
	},
	Unit {
		bank: Bank,
		id: Uuid,
		revision: i64,
	},
	Message {
		id: Uuid,
		revision: i64,
		digest: String,
	},
	Artifact {
		id: Uuid,
		revision: i64,
		digest: String,
	},
	Run {
		id: Uuid,
		revision: i64,
		digest: String,
	},
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Entity {
	pub name: String,
	pub category: String,
	/// Explicit source/model-resolved names, including cross-language aliases.
	#[serde(default)]
	pub aliases: Vec<String>,
}
impl Entity {
	/// Unicode letters are retained; equality does not depend on an English tokenizer.
	pub fn key(&self) -> String {
		format!(
			"{}:{}",
			self.category.trim().to_lowercase(),
			self.name
				.split_whitespace()
				.collect::<Vec<_>>()
				.join(" ")
				.to_lowercase()
		)
	}
	pub fn keys(&self) -> std::collections::BTreeSet<String> {
		std::iter::once(&self.name)
			.chain(&self.aliases)
			.map(|name| {
				format!(
					"{}:{}",
					self.category.trim().to_lowercase(),
					name.split_whitespace()
						.collect::<Vec<_>>()
						.join(" ")
						.to_lowercase()
				)
			})
			.collect()
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
	Entity,
	Semantic,
	Temporal,
	Causes,
	CausedBy,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Link {
	pub target: Uuid,
	pub revision: i64,
	pub kind: LinkKind,
	pub weight: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TimeRange {
	pub start: DateTime<Utc>,
	pub end: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Content {
	/// A recurring question, retained independently of the current generated answer.
	#[serde(default)]
	pub mental_model: Option<MentalModel>,
	pub text: String,
	pub kind: Kind,
	pub learning: Learning,
	pub verification: Verification,
	pub occurred: Option<TimeRange>,
	pub entities: Vec<Entity>,
	pub evidence: Vec<Evidence>,
	pub links: Vec<Link>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MentalModel {
	pub question: String,
	pub automatic_refresh: bool,
}

impl Content {
	pub fn validate(&self, bounds: &Bounds) -> Result<()> {
		if (self.kind == Kind::MentalModel) != self.mental_model.is_some()
			|| self.mental_model.as_ref().is_some_and(|m| {
				m.question.trim().is_empty() || m.question.len() > bounds.max_input_bytes
			}) {
			return Err(Error::Invalid(
				"mental models require a bounded recurring question".into(),
			));
		}
		if self.text.trim().is_empty()
			|| serde_json::to_vec(self)?.len() > bounds.max_unit_bytes
			|| self.entities.len() > bounds.max_entities
			|| self.evidence.len() > bounds.max_evidence
			|| self.links.len() > bounds.max_links
		{
			return Err(Error::Invalid(
				"memory content exceeds its declared bounds".into(),
			));
		}
		if self
			.occurred
			.as_ref()
			.is_some_and(|time| time.start > time.end)
			|| self.entities.iter().any(|entity| {
				entity.name.trim().is_empty()
					|| entity.category.trim().is_empty()
					|| entity.aliases.len() > bounds.max_entities
					|| entity.aliases.iter().any(|alias| alias.trim().is_empty())
					|| entity.keys().len() != entity.aliases.len() + 1
			}) || self.links.iter().any(|link| {
			link.target.is_nil()
				|| link.revision < 1
				|| !link.weight.is_finite()
				|| !(0.0..=1.0).contains(&link.weight)
		}) {
			return Err(Error::Invalid(
				"invalid memory time, entity, or link".into(),
			));
		}
		for evidence in &self.evidence {
			let valid = match evidence {
				Evidence::Publication { id, revision } => !id.is_nil() && *revision > 0,
				Evidence::Unit { bank, id, revision } => {
					bank.validate().is_ok() && !id.is_nil() && *revision > 0
				}
				Evidence::Message {
					id,
					revision,
					digest,
				} => !id.is_nil() && *revision > 0 && !digest.is_empty(),
				Evidence::Artifact {
					id,
					revision,
					digest,
				}
				| Evidence::Run {
					id,
					revision,
					digest,
				} => !id.is_nil() && *revision > 0 && !digest.is_empty(),
			};
			if !valid {
				return Err(Error::Invalid("invalid memory evidence revision".into()));
			}
		}
		if self.kind.derived()
			&& (self.verification != Verification::Unverified
				|| self.evidence.is_empty()
				|| self
					.evidence
					.iter()
					.any(|e| !matches!(e, Evidence::Unit { .. } | Evidence::Publication { .. })))
		{
			return Err(Error::Invalid(
				"derived memory must be unverified and supported by admitted units".into(),
			));
		}
		Ok(())
	}
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Unit {
	pub id: Uuid,
	pub bank: Bank,
	pub revision: i64,
	pub content: Content,
	pub learned_at: DateTime<Utc>,
	pub updated_at: DateTime<Utc>,
	pub deleted: bool,
	pub stale: bool,
}
impl Unit {
	pub fn evidence(&self) -> Evidence {
		Evidence::Unit {
			bank: self.bank.clone(),
			id: self.id,
			revision: self.revision,
		}
	}
	pub fn visible(&self) -> bool {
		!self.deleted && !self.stale && self.content.verification != Verification::Contradicted
	}
}

/// These are operational caps, not unapproved product latency/quality targets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Bounds {
	pub max_unit_bytes: usize,
	pub max_input_bytes: usize,
	pub max_units: usize,
	pub max_candidates: usize,
	pub max_entities: usize,
	pub max_evidence: usize,
	pub max_links: usize,
	pub max_graph_hops: usize,
	pub max_graph_visits: usize,
	pub max_results: usize,
	pub max_context_tokens: usize,
	pub max_model_calls: usize,
	pub max_model_tokens: usize,
	pub max_cost_micros: u64,
	pub max_retries: usize,
	pub max_call_seconds: u32,
}
impl Bounds {
	pub fn validate(&self) -> Result<()> {
		let finite = [
			self.max_unit_bytes,
			self.max_input_bytes,
			self.max_units,
			self.max_candidates,
			self.max_entities,
			self.max_evidence,
			self.max_links,
			self.max_graph_hops,
			self.max_graph_visits,
			self.max_results,
			self.max_context_tokens,
			self.max_model_calls,
			self.max_model_tokens,
			self.max_retries,
			self.max_call_seconds as usize,
		];
		if finite.contains(&0)
			|| finite.iter().any(|bound| *bound > i32::MAX as usize)
			|| self.max_cost_micros == 0
			|| self.max_cost_micros > i64::MAX as u64
			|| self.max_retries == 0
			|| self.max_candidates > self.max_units
			|| self.max_results > self.max_candidates
		{
			return Err(Error::Invalid(
				"memory bounds must be finite and internally consistent".into(),
			));
		}
		Ok(())
	}
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Policy {
	pub extraction: EntityRef,
	pub derivation: EntityRef,
	pub reflection: EntityRef,
	pub embedding: EntityRef,
	pub reranker: EntityRef,
	pub tokenizer: EntityRef,
	/// Explicit cosine cutoff for derived graph edges, in millionths (1..=1_000_000).
	pub semantic_link_min_similarity_millionths: u32,
	/// Explicit rates for the pinned role versions, in microcurrency per million tokens.
	pub prices: Prices,
	pub retention: Retention,
	pub bounds: Bounds,
	pub learn_from_runs: bool,
	/// Automatic work is explicit and pinned to this Registry policy version.
	pub maintain_observations: bool,
	pub refresh_mental_models: bool,
}

/// Explicit finite limits for memory-owned records. Tombstone identities remain
/// inside max_unit_records so expiry cannot reopen a deleted identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Retention {
	pub unit_max_age_days: Option<u32>,
	pub candidate_days: u32,
	pub history_days: u32,
	pub history_versions: usize,
	pub model_result_days: u32,
	pub backup_days: u32,
	pub purge_after_seconds: u32,
	pub purge_batch: usize,
	pub max_unit_records: usize,
	pub max_model_operations: usize,
}
impl Retention {
	/// Logical expiry is checked at delivery, independently of asynchronous cleanup.
	pub fn unit_expired(&self, learned_at: DateTime<Utc>, now: DateTime<Utc>) -> bool {
		self.unit_max_age_days.is_some_and(|days| {
			learned_at
				.checked_add_signed(chrono::Duration::days(i64::from(days)))
				.is_none_or(|expires_at| expires_at <= now)
		})
	}
	pub fn validate(&self, bounds: &Bounds) -> Result<()> {
		if [
			self.candidate_days,
			self.history_days,
			self.model_result_days,
			self.backup_days,
		]
		.iter()
		.any(|v| !(1..=3650).contains(v))
			|| self
				.unit_max_age_days
				.is_some_and(|v| !(1..=3650).contains(&v))
			|| !(1..=86_400).contains(&self.purge_after_seconds)
			|| !(1..=1024).contains(&self.purge_batch)
			|| !(1..=1024).contains(&self.history_versions)
			|| self.max_unit_records < bounds.max_units
			|| self.max_unit_records > i32::MAX as usize
			|| !(1..=i32::MAX as usize).contains(&self.max_model_operations)
		{
			return Err(Error::Invalid("memory retention requires finite, consistent record, history, purge and backup limits".into()));
		}
		Ok(())
	}
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Prices {
	pub extraction: Rate,
	pub derivation: Rate,
	pub reflection: Rate,
	pub embedding: Rate,
	pub reranker: Rate,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Rate {
	pub input_per_million: u64,
	pub output_per_million: u64,
}
impl Rate {
	pub fn charge(self, input: u64, output: u64) -> Result<u64> {
		let numerator = (u128::from(input) * u128::from(self.input_per_million))
			.checked_add(u128::from(output) * u128::from(self.output_per_million))
			.ok_or_else(|| Error::Invalid("memory price overflow".into()))?;
		u64::try_from(numerator.div_ceil(1_000_000))
			.map_err(|_| Error::Invalid("memory price overflow".into()))
	}
}
impl Policy {
	pub fn validate(&self) -> Result<()> {
		self.bounds.validate()?;
		if !(1..=1_000_000).contains(&self.semantic_link_min_similarity_millionths) {
			return Err(Error::Invalid(
				"memory semantic graph cutoff must be explicit and positive".into(),
			));
		}
		self.retention.validate(&self.bounds)?;
		for role in [
			&self.extraction,
			&self.derivation,
			&self.reflection,
			&self.embedding,
			&self.reranker,
			&self.tokenizer,
		] {
			if role.id.trim().is_empty() || role.version.trim().is_empty() {
				return Err(Error::Invalid(
					"every memory role needs a versioned Registry reference".into(),
				));
			}
		}
		Ok(())
	}
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Change {
	Add {
		id: Uuid,
		content: Content,
	},
	Correct {
		id: Uuid,
		expected_revision: i64,
		content: Content,
	},
	Delete {
		id: Uuid,
		expected_revision: i64,
	},
}
impl Change {
	pub fn id(&self) -> Uuid {
		match self {
			Self::Add { id, .. } | Self::Correct { id, .. } | Self::Delete { id, .. } => *id,
		}
	}
	pub fn expected_revision(&self) -> i64 {
		match self {
			Self::Add { .. } => 0,
			Self::Correct {
				expected_revision, ..
			}
			| Self::Delete {
				expected_revision, ..
			} => *expected_revision,
		}
	}
	pub fn validate(&self, bounds: &Bounds) -> Result<()> {
		if self.id().is_nil()
			|| self.expected_revision() == i64::MAX
			|| (!matches!(self, Self::Add { .. }) && self.expected_revision() < 1)
		{
			return Err(Error::Invalid(
				"memory mutation requires an observed unit revision".into(),
			));
		}
		if let Self::Add { content, .. } | Self::Correct { content, .. } = self {
			content.validate(bounds)?;
		}
		Ok(())
	}
}

/// The complete request is the idempotency identity; keys cannot replay different content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Mutation {
	pub operation_id: Uuid,
	pub provider: EntityRef,
	pub bank: Bank,
	pub changes: Vec<Change>,
}
impl Mutation {
	pub fn validate(&self, policy: &Policy) -> Result<()> {
		self.validate_bounds(&policy.bounds)
	}
	pub fn validate_bounds(&self, bounds: &Bounds) -> Result<()> {
		bounds.validate()?;
		self.bank.validate()?;
		if self.operation_id.is_nil()
			|| self.changes.is_empty()
			|| self.changes.len() > bounds.max_candidates
			|| self.provider.id.trim().is_empty()
			|| self.provider.version.trim().is_empty()
		{
			return Err(Error::Invalid("invalid memory mutation".into()));
		}
		let mut ids = std::collections::BTreeSet::new();
		for change in &self.changes {
			change.validate(bounds)?;
			if !ids.insert(change.id()) {
				return Err(Error::Invalid("duplicate memory unit in mutation".into()));
			}
		}
		Ok(())
	}
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
	pub id: Uuid,
	pub bank: Bank,
	pub revision: i64,
	pub run: Evidence,
	pub content: Content,
	pub state: CandidateState,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CandidateState {
	Pending,
	Admitted,
	Rejected,
	Invalidated,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Recall {
	Disabled,
	Empty,
	NoSpace,
	Ready { units: Vec<Unit> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecallQuery {
	pub text: String,
	pub time: Option<TimeRange>,
	pub kinds: Vec<Kind>,
	pub max_tokens: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Reflection {
	pub text: String,
	pub evidence: Vec<Evidence>,
}

#[cfg(test)]
mod tests;
