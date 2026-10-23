//! Registry-pinned source policy and bounded data envelopes. None grants authority.
use crate::{
	Error, Result,
	decision::{Mode, SourcePin},
	registry::{bindings::QualifiedRef, rules::digest},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use uuid::Uuid;

pub const MAX_CANDIDATES: usize = 64;
pub const MAX_SOURCE_BYTES: usize = 1_048_576;
pub const MAX_PREVIEW_BYTES: usize = 2_048;

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceTools {
	pub read: Vec<QualifiedRef>,
	pub write: Vec<QualifiedRef>,
	pub detail: Option<QualifiedRef>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "policy", rename_all = "snake_case", deny_unknown_fields)]
pub enum PinPolicy {
	Always,
	OnDemand,
	Scored { mode: Mode, decider: QualifiedRef },
}
impl Default for PinPolicy {
	fn default() -> Self {
		Self::Always
	}
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Volatility {
	Run,
	#[default]
	Step,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub enum QueryPolicy {
	#[default]
	#[serde(rename = "task-inputs@1")]
	TaskInputsV1,
	#[serde(rename = "accepted-inputs-first@1")]
	AcceptedInputsFirstV1,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub enum RefreshPolicy {
	#[default]
	#[serde(rename = "revision-bound@1")]
	RevisionBoundV1,
}
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceDisclosure {
	/// A declaration of permitted classification material, subject to live authority.
	pub previews: bool,
	pub query: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceDeclaration {
	pub provider: String,
	pub operation: String,
	pub tools: SourceTools,
	#[serde(default)]
	pub pin: PinPolicy,
	#[serde(default)]
	pub volatility: Volatility,
	#[serde(default)]
	pub query: QueryPolicy,
	#[serde(default)]
	pub refresh: RefreshPolicy,
	#[serde(default)]
	pub disclosure: SourceDisclosure,
	#[serde(default)]
	pub required_home: bool,
}
impl SourceDeclaration {
	pub fn validate(&self) -> Result<()> {
		if self.provider.trim().is_empty()
			|| self.provider.len() > 128
			|| self.operation.trim().is_empty()
			|| self.operation.len() > 128
			|| self.tools.read.len() + self.tools.write.len() > 32
		{
			return Err(Error::Invalid(
				"invalid context source provider operation or tool bound".into(),
			));
		}
		let mut tools = BTreeSet::new();
		for tool in self
			.tools
			.read
			.iter()
			.chain(&self.tools.write)
			.chain(self.tools.detail.iter())
		{
			tool.validate()?;
			// A read Tool may also be the detail route; other duplicates add no authority.
			tools.insert(tool);
		}
		if let PinPolicy::Scored { mode, decider } = &self.pin {
			decider.validate()?;
			if self.required_home && *mode == Mode::Enforce {
				return Err(Error::Invalid(
					"required-Home source cannot enforce Scored selection".into(),
				));
			}
			if !self.disclosure.previews || !self.disclosure.query {
				return Err(Error::Invalid(
					"Scored source requires explicit preview and query disclosure".into(),
				));
			}
		}
		Ok(())
	}
	pub fn references(&self) -> Vec<(QualifiedRef, String)> {
		let mut refs = self
			.tools
			.read
			.iter()
			.chain(&self.tools.write)
			.chain(self.tools.detail.iter())
			.cloned()
			.map(|r| (r, "tool".into()))
			.collect::<Vec<_>>();
		if let PinPolicy::Scored { decider, .. } = &self.pin {
			refs.push((decider.clone(), "decider".into()));
		}
		refs.sort();
		refs.dedup();
		refs
	}
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CandidateIdentity {
	pub source: QualifiedRef,
	pub item: String,
	pub revision: String,
	/// Byte range within this revision; absent means the complete item.
	pub range: Option<[usize; 2]>,
}
impl CandidateIdentity {
	pub fn key(&self) -> String {
		digest(&json!(self))
	}
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Completeness {
	Complete,
	Partial,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DetailRoute {
	pub tool: QualifiedRef,
	pub arguments: Value,
	pub revision: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceCandidate {
	pub identity: CandidateIdentity,
	pub content: Value,
	pub preview: String,
	pub provenance: Vec<SourcePin>,
	pub dependencies: Vec<SourcePin>,
	pub completeness: Completeness,
	pub detail: Option<DetailRoute>,
	pub mandatory: bool,
}
impl SourceCandidate {
	pub fn validate(&self, source: &QualifiedRef, declaration: &SourceDeclaration) -> Result<()> {
		if &self.identity.source != source
			|| self.identity.item.is_empty()
			|| self.identity.item.len() > 512
			|| self.identity.revision.is_empty()
			|| self.identity.revision.len() > 256
			|| self.preview.len() > MAX_PREVIEW_BYTES
			|| serde_json::to_vec(&self.content)?.len() > MAX_SOURCE_BYTES
			|| self.identity.range.is_some_and(|[start, end]| start >= end)
			|| self.provenance.is_empty()
			|| self.dependencies.is_empty()
			|| self.provenance.len() > 128
			|| self.dependencies.len() > 128
		{
			return Err(Error::Invalid("invalid bounded source candidate".into()));
		}
		for pin in self.provenance.iter().chain(&self.dependencies) {
			if pin.resource.is_empty() {
				return Err(Error::Invalid("source dependency lacks resource".into()));
			}
			crate::decision::validate_digest(&pin.revision_digest)?;
		}
		if let Some(detail) = &self.detail {
			if declaration.tools.detail.as_ref() != Some(&detail.tool)
				|| detail.revision != self.identity.revision
				|| serde_json::to_vec(&detail.arguments)?.len() > 8192
			{
				return Err(Error::Invalid(
					"source detail route differs from its pinned Tool or revision".into(),
				));
			}
		}
		if declaration.required_home && !self.mandatory {
			return Err(Error::Invalid(
				"required-Home candidate must retain its mandatory receipt".into(),
			));
		}
		Ok(())
	}
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceOutcome {
	Empty,
	Excluded,
	Truncated,
	Deferred,
	Unavailable,
	Fetched,
	Selected,
	Fitted,
	Delivered,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceBatch {
	pub candidates: Vec<SourceCandidate>,
	pub outcome: SourceOutcome,
}
impl SourceBatch {
	pub fn validate(
		&self,
		source: &QualifiedRef,
		declaration: &SourceDeclaration,
		max_bytes: usize,
	) -> Result<()> {
		declaration.validate()?;
		if self.candidates.len() > MAX_CANDIDATES
			|| serde_json::to_vec(self)?.len() > max_bytes.min(MAX_SOURCE_BYTES)
			|| (self.candidates.is_empty() && self.outcome == SourceOutcome::Fetched)
			|| (!self.candidates.is_empty()
				&& matches!(
					self.outcome,
					SourceOutcome::Empty
						| SourceOutcome::Unavailable
						| SourceOutcome::Excluded
						| SourceOutcome::Deferred
				)) {
			return Err(Error::Invalid(
				"source batch violates its bounded fetch outcome".into(),
			));
		}
		for candidate in &self.candidates {
			candidate.validate(source, declaration)?;
		}
		Ok(())
	}
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AcceptedQueryInput {
	pub sequence: i64,
	pub text: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QueryMaterial {
	pub policy: QueryPolicy,
	pub text: String,
	pub input_digest: String,
	pub query_digest: String,
}
/// Accepted inputs must already be authorized; sequence order is canonical.
pub fn query_material(
	policy: QueryPolicy,
	seed: &str,
	inputs: &[AcceptedQueryInput],
	max_bytes: usize,
) -> QueryMaterial {
	let mut ordered = inputs.to_vec();
	ordered.sort_by_key(|i| i.sequence);
	let input_digest = digest(&json!(ordered));
	let text = match policy {
		QueryPolicy::TaskInputsV1 => std::iter::once(seed)
			.chain(ordered.iter().map(|i| i.text.as_str()))
			.collect::<Vec<_>>()
			.join("\n"),
		QueryPolicy::AcceptedInputsFirstV1 => ordered
			.iter()
			.rev()
			.map(|i| i.text.as_str())
			.chain(std::iter::once(seed))
			.collect::<Vec<_>>()
			.join("\n"),
	};
	let text = utf8_prefix(&text, max_bytes.min(MAX_SOURCE_BYTES)).to_owned();
	let query_digest = digest(&json!({"policy":policy,"text":text,"inputs":input_digest}));
	QueryMaterial {
		policy,
		text,
		input_digest,
		query_digest,
	}
}
pub fn utf8_prefix(value: &str, max_bytes: usize) -> &str {
	let mut end = value.len().min(max_bytes);
	while !value.is_char_boundary(end) {
		end -= 1;
	}
	&value[..end]
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceRequest {
	pub run_id: Uuid,
	pub source: QualifiedRef,
	pub declaration: SourceDeclaration,
	pub boundary: String,
	pub query: QueryMaterial,
	pub max_bytes: usize,
}
impl SourceRequest {
	pub fn refresh_key(&self) -> String {
		digest(
			&json!({"run":self.run_id,"source":self.source,"declaration":self.declaration,"query":self.query.query_digest,"inputs":self.query.input_digest,"boundary":if self.declaration.volatility==Volatility::Run {"run"} else {&self.boundary},"max_bytes":self.max_bytes}),
		)
	}
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DetailRequest {
	pub request: SourceRequest,
	pub identity: CandidateIdentity,
	pub route: DetailRoute,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceContribution {
	pub identity: CandidateIdentity,
	pub content: Value,
	pub provenance: Vec<SourcePin>,
	pub dependencies: Vec<SourcePin>,
	pub completeness: Completeness,
	pub detail: Option<DetailRoute>,
	/// Commits to the fetched bytes, independent of a lossy fitted variant.
	pub fetched_digest: String,
	pub content_digest: String,
}
impl From<SourceCandidate> for SourceContribution {
	fn from(c: SourceCandidate) -> Self {
		let content_digest = digest(&c.content);
		Self {
			identity: c.identity,
			content: c.content,
			provenance: c.provenance,
			dependencies: c.dependencies,
			completeness: c.completeness,
			detail: c.detail,
			fetched_digest: content_digest.clone(),
			content_digest,
		}
	}
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceDelivery {
	pub identity: CandidateIdentity,
	pub request_digest: String,
	pub content_digest: String,
	pub completeness: Completeness,
	pub accepted_inference: Uuid,
}
/// Only the accepted-inference path may call this, after comparing fitted bytes.
pub fn accepted_delivery(
	contribution: &SourceContribution,
	request_digest: String,
	accepted_inference: Uuid,
) -> Result<SourceDelivery> {
	crate::decision::validate_digest(&request_digest)?;
	if accepted_inference.is_nil() || contribution.content_digest != digest(&contribution.content) {
		return Err(Error::Invalid(
			"source delivery lacks accepted inference or fitted-byte integrity".into(),
		));
	}
	Ok(SourceDelivery {
		identity: contribution.identity.clone(),
		request_digest,
		content_digest: contribution.content_digest.clone(),
		completeness: contribution.completeness,
		accepted_inference,
	})
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SelectionFailure {
	ForbiddenDisclosure,
	Unapproved,
	BudgetExhausted,
	StateTooLarge,
	ProviderUnavailable,
	InvalidAnswers,
	AuthorityUnavailable,
	JournalUnavailable,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectionEvidence {
	pub boundary: crate::decision::Boundary,
	pub decider: crate::decision::DeciderPin,
	pub mode: Mode,
	pub query_digest: String,
	pub candidate_digest: String,
	pub state_digest: String,
	pub route: Option<u32>,
	pub threshold: Option<crate::decision::Probability>,
	pub questions: crate::decision::Questions,
	pub answers: std::collections::BTreeMap<String, crate::decision::Probability>,
	pub attempts: Vec<crate::decision::AttemptEvidence>,
	pub proposed: Vec<String>,
	pub selected: Vec<String>,
	pub failure: Option<SelectionFailure>,
}
#[derive(Debug, Clone)]
pub struct SourceSelection {
	/// Candidate identity keys retained for the request. Shadow always retains the original set.
	pub selected: Vec<String>,
	pub evidence: SelectionEvidence,
}

impl SelectionFailure {
	pub fn pause_reason(self) -> super::recovery::Failure {
		use super::recovery::Failure;
		match self {
			Self::ForbiddenDisclosure => Failure::SourceSelectionForbidden,
			Self::Unapproved => Failure::SourceSelectionUnapproved,
			Self::BudgetExhausted => Failure::SourceSelectionCallsExhausted,
			_ => Failure::SourceSelectionUnavailable,
		}
	}
}
