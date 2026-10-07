//! Reader-controlled evidence is distinct from safe ordinary journal summaries.
use super::*;
use chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Boundary {
	pub node: String,
	pub run: Uuid,
	pub step: i32,
	pub run_revision: i64,
	pub input_revision: i64,
	pub worker: Uuid,
	pub input_digest: String,
}
impl Boundary {
	pub fn validate(&self) -> Result<()> {
		crate::configuration::validate_node_id(&self.node)?;
		validate_digest(&self.input_digest)?;
		if self.step < 0
			|| self.run_revision < 0
			|| self.input_revision < 0
			|| self.worker.is_nil()
			|| self.run.is_nil()
		{
			return Err(Error::Invalid("invalid decision execution boundary".into()));
		}
		Ok(())
	}
	pub fn candidate_id(&self, index: usize) -> String {
		format!(
			"{}/runs/{}/input/{}/{}/history/{index}",
			self.node, self.run, self.input_revision, self.input_digest
		)
	}
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
	pub id: String,
	pub history_index: usize,
	pub description: String,
	pub keep_call: String,
	pub keep_result: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourcePin {
	pub resource: String,
	pub revision_digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttemptStatus {
	Answered,
	Failed,
	Uncertain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AttemptEvidence {
	pub id: Uuid,
	pub request_digest: String,
	pub questions: Vec<String>,
	pub status: AttemptStatus,
	/// Canonical owner receipts are retained by their owners; these exact IDs link them.
	pub owner_receipts: BTreeMap<String, Uuid>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
	Applied,
	Shadow,
	Rejected,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
	Fits,
	Insufficient,
	Forbidden,
	ProviderFailure,
	InvalidAnswers,
	NoCandidates,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum StateReference {
	Disabled,
	Retained {
		id: Uuid,
		digest: String,
		expires_at: DateTime<Utc>,
	},
	Expired {
		id: Uuid,
		digest: String,
		expired_at: DateTime<Utc>,
	},
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FitMetrics {
	pub before: usize,
	pub proposed: usize,
	pub window: usize,
	pub retained: usize,
	pub dropped: usize,
	pub truncated: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
	pub version: u8,
	pub id: Uuid,
	pub boundary: Boundary,
	pub hook: Hook,
	pub decider: DeciderPin,
	pub provider_contract: String,
	pub model: String,
	pub builder: String,
	pub option_source: String,
	pub rule: String,
	pub state_digest: String,
	pub history_length: usize,
	pub sources: Vec<SourcePin>,
	pub candidates: Vec<Candidate>,
	pub questions: Questions,
	pub answers: BTreeMap<String, Probability>,
	pub restrictions: Restrictions,
	pub mode: Mode,
	pub branches: BTreeMap<String, Branch>,
	pub attempts: Vec<AttemptEvidence>,
	pub outcome: Outcome,
	pub reason: Reason,
	pub fit: FitMetrics,
	pub state: StateReference,
}
impl Evidence {
	/// No provider, current policy, mutable definition or executable state is consulted.
	pub fn replay(&self) -> Result<BTreeMap<String, Branch>> {
		self.boundary.validate()?;
		self.decider.validate()?;
		validate_digest(&self.state_digest)?;
		validate_model(&self.model)?;
		for source in &self.sources {
			validate_digest(&source.revision_digest)?;
			if source.resource.trim().is_empty() {
				return Err(Error::Invalid("corrupt historical source reference".into()));
			}
		}
		match &self.state {
			StateReference::Retained { id, digest, .. }
			| StateReference::Expired { id, digest, .. }
				if id.is_nil() || digest != &self.state_digest =>
			{
				return Err(Error::Invalid(
					"historical retained-state reference changed".into(),
				));
			}
			_ => {}
		}
		let proposed_reason = if self.restrictions.forbid_apply {
			Reason::Forbidden
		} else if self.fit.proposed > self.fit.window {
			Reason::Insufficient
		} else {
			Reason::Fits
		};
		if self.reason != proposed_reason
			|| self.outcome == Outcome::Rejected
				&& (self.mode != Mode::Enforce || self.reason == Reason::Fits)
			|| self.fit.retained.checked_add(self.fit.dropped) != Some(self.history_length)
			|| self.fit.truncated > self.fit.retained
			|| self.fit.before <= self.fit.window
			|| self.outcome == Outcome::Applied
				&& (self.mode != Mode::Enforce
					|| self.reason != Reason::Fits
					|| self.fit.proposed > self.fit.window
					|| self.restrictions.forbid_apply)
			|| self.outcome == Outcome::Shadow && self.mode != Mode::Shadow
		{
			return Err(Error::Invalid(
				"corrupt historical application outcome".into(),
			));
		}
		if self.version != EVIDENCE_VERSION
			|| self.rule != RULE
			|| self.provider_contract != PROVIDER
			|| self.builder != BUILDER
			|| self.option_source != OPTION_SOURCE
			|| self.restrictions.keep_threshold.value() > 0.5
			|| self.restrictions.preserve_recent < 6
			|| self.id.is_nil()
			|| self.candidates.is_empty()
		{
			return Err(Error::Invalid(
				"unsupported or incomplete historical decision evidence".into(),
			));
		}
		validate_answers(&self.questions, &self.answers)?;
		let mut replay = BTreeMap::new();
		let mut question_ids = std::collections::BTreeSet::new();
		for candidate in &self.candidates {
			if candidate.history_index >= self.history_length
				|| candidate.id != self.boundary.candidate_id(candidate.history_index)
				|| candidate.description.trim().is_empty()
				|| !question_ids.insert(&candidate.keep_call)
				|| !question_ids.insert(&candidate.keep_result)
			{
				return Err(Error::Invalid(
					"corrupt historical decision candidate".into(),
				));
			}
			let call = *self
				.answers
				.get(&candidate.keep_call)
				.ok_or_else(|| Error::Invalid("missing call answer".into()))?;
			let result = *self
				.answers
				.get(&candidate.keep_result)
				.ok_or_else(|| Error::Invalid("missing result answer".into()))?;
			let protected = candidate.history_index == 0
				|| candidate.history_index
					>= self
						.history_length
						.saturating_sub(self.restrictions.preserve_recent);
			let chosen = if protected {
				Branch::Keep
			} else {
				branch(call, result, self.restrictions.keep_threshold)
			};
			if replay.insert(candidate.id.clone(), chosen).is_some() {
				return Err(Error::Invalid(
					"duplicate historical decision candidate".into(),
				));
			}
		}
		if question_ids.len() != self.questions.len() || replay != self.branches {
			return Err(Error::Invalid(
				"historical decision branch or question coverage changed".into(),
			));
		}
		let mut coverage = std::collections::BTreeSet::new();
		let mut attempts = std::collections::BTreeSet::new();
		for attempt in &self.attempts {
			validate_digest(&attempt.request_digest)?;
			if attempt.id.is_nil()
				|| !attempts.insert(attempt.id)
				|| attempt.questions.is_empty()
				|| attempt.owner_receipts.is_empty()
				|| attempt.owner_receipts.values().any(Uuid::is_nil)
			{
				return Err(Error::Invalid("corrupt historical attempt evidence".into()));
			}
			if attempt.status == AttemptStatus::Answered {
				for question in &attempt.questions {
					if !self.questions.contains_key(question) || !coverage.insert(question) {
						return Err(Error::Invalid(
							"historical attempts have conflicting question coverage".into(),
						));
					}
				}
			}
		}
		if coverage.len() != self.questions.len() {
			return Err(Error::Invalid(
				"historical answers lack durable attempt evidence".into(),
			));
		}
		Ok(replay)
	}
	pub fn summary(&self) -> Summary {
		Summary {
			id: self.id,
			hook: self.hook,
			outcome: self.outcome,
			reason: self.reason,
			attempts: self.attempts.len(),
			fit: self.fit.clone(),
		}
	}
}

pub fn validate_answers(
	questions: &Questions,
	answers: &BTreeMap<String, Probability>,
) -> Result<()> {
	if questions.is_empty()
		|| questions.len() != answers.len()
		|| questions.iter().any(|(id, q)| {
			id.is_empty() || q.description.trim().is_empty() || !answers.contains_key(id)
		}) {
		return Err(Error::Invalid(
			"decision answers must cover every expected Noul question exactly".into(),
		));
	}
	Ok(())
}

/// Contains no candidate descriptions, source text, provider bodies or retained state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Summary {
	pub id: Uuid,
	pub hook: Hook,
	pub outcome: Outcome,
	pub reason: Reason,
	pub attempts: usize,
	pub fit: FitMetrics,
}

/// Appended measurement, never a causal claim about the model or downstream success.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OutcomeLink {
	pub decision: Uuid,
	pub next_inference: Option<String>,
	pub observed_status: String,
}
