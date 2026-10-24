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
	/// The Run's resume cycle (`RecoveryState::decision_cycle`).
	#[serde(default)]
	pub cycle: u32,
	/// The outage retry of this step (`RecoveryState::decider_retry`). Every
	/// Route failing schedules one, which starts a new decision (ADR 0066).
	#[serde(default)]
	pub retry: u32,
}
impl Boundary {
	pub fn validate(&self) -> Result<()> {
		crate::nodes::validate_node_ref(&self.node)?;
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
	/// The Model Turn of the event on the Native Projection Version, if any.
	pub turn: Option<usize>,
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
	/// Charged reservation stopped before any physical request.
	NotDispatched,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AttemptEvidence {
	pub id: Uuid,
	pub request_digest: String,
	pub questions: Vec<String>,
	/// Commits to the exact question definitions the attempt asked.
	pub question_digest: String,
	pub status: AttemptStatus,
	/// Canonical owner receipts are retained by their owners; these exact IDs link them.
	pub owner_receipts: BTreeMap<String, Uuid>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
	Applied,
	/// Pruning that was not enough on its own, completed by the Summary Stage
	/// and recorded together with the adopted summary (ADR 0056).
	Staged,
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
	AuthorityFailure,
	JournalFailure,
	/// The provider returned answers that failed validation.
	InvalidAnswers,
	/// The durable answers of every attempt do not cover the questions.
	IncompleteAnswers,
	NoCandidates,
}
impl Reason {
	/// Reasons that only arise once candidates and their questions exist.
	fn after_candidates(self) -> bool {
		matches!(
			self,
			Self::ProviderFailure
				| Self::JournalFailure
				| Self::InvalidAnswers
				| Self::IncompleteAnswers
		)
	}
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

/// Immutable witness preserved when cleanup removes the classification state bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum StateRetentionWitness {
	Disabled,
	Enabled { id: Uuid, expires_at: DateTime<Utc> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FitMetrics {
	pub before: usize,
	pub proposed: usize,
	pub window: usize,
	pub retained: usize,
	/// History entries the projection no longer holds.
	pub removed: usize,
	/// Candidates whose branch is `drop`.
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
	/// The Decider Route this decision asked, by its place in the version.
	pub route: u32,
	/// The model of every Route in order; with the Route thresholds, the mode
	/// and the parameters digest they complete the pinned configuration
	/// commitment.
	pub route_models: Vec<String>,
	/// The keep threshold of every Route in order. Replay holds `restrictions`
	/// to the threshold of the Route asked.
	pub route_thresholds: Vec<Probability>,
	/// The contract and model of the Route asked.
	pub provider_contract: String,
	pub model: String,
	/// Completes the pinned configuration commitment with the Route models.
	pub configuration_parameters_digest: String,
	pub builder: String,
	pub option_source: String,
	pub rule: String,
	pub state_digest: String,
	pub history_length: usize,
	/// Model Turns in the history on the Native Projection Version; `None` on
	/// Legacy, where `preserve_recent` counts events.
	pub turns: Option<usize>,
	pub sources: Vec<SourcePin>,
	pub candidates: Vec<Candidate>,
	pub questions: Questions,
	pub answers: BTreeMap<String, Probability>,
	pub restrictions: Restrictions,
	/// The Binding restrictions admitted with the Run, which the decision
	/// identity commits to; `restrictions` may only be stricter.
	pub binding_restrictions: Restrictions,
	pub mode: Mode,
	pub branches: BTreeMap<String, Branch>,
	pub attempts: Vec<AttemptEvidence>,
	pub outcome: Outcome,
	pub reason: Reason,
	pub fit: FitMetrics,
	/// Digest of the history an applied or staged decision produced.
	pub projection_digest: Option<String>,
	pub state: StateReference,
	pub state_retention: StateRetentionWitness,
}
impl Evidence {
	/// No provider, current policy, mutable definition or executable state is consulted.
	pub fn replay(&self) -> Result<BTreeMap<String, Branch>> {
		self.validate_identity()?;
		let proposed_reason = if self.restrictions.forbid_apply {
			Reason::Forbidden
		} else if self.fit.proposed > self.fit.window {
			Reason::Insufficient
		} else {
			Reason::Fits
		};
		let changes = matches!(self.outcome, Outcome::Applied | Outcome::Staged);
		if self.reason != proposed_reason
			|| self.outcome == Outcome::Rejected
				&& (self.mode != Mode::Enforce || self.reason == Reason::Fits)
			|| self.fit.retained.checked_add(self.fit.removed) != Some(self.history_length)
			|| self.turns.is_none() && self.fit.removed != self.fit.dropped
			|| self.fit.truncated > self.fit.retained
			|| self.fit.before <= self.fit.window
			|| self.outcome == Outcome::Applied
				&& (self.mode != Mode::Enforce
					|| self.reason != Reason::Fits
					|| self.fit.proposed > self.fit.window
					|| self.restrictions.forbid_apply)
			|| self.outcome == Outcome::Staged
				&& (self.mode != Mode::Enforce || self.reason != Reason::Insufficient)
			|| self.outcome == Outcome::Shadow && self.mode != Mode::Shadow
			|| changes != self.projection_digest.is_some()
		{
			return Err(Error::Invalid(
				"corrupt historical application outcome".into(),
			));
		}
		self.validate_contract()?;
		if self.id.is_nil() || self.candidates.is_empty() {
			return Err(Error::Invalid(
				"unsupported or incomplete historical decision evidence".into(),
			));
		}
		if let Some(digest) = &self.projection_digest {
			validate_digest(digest)?;
		}
		validate_answers(&self.questions, &self.answers)?;
		self.validate_candidates()?;
		let mut replay = BTreeMap::new();
		for candidate in &self.candidates {
			let call = *self
				.answers
				.get(&candidate.keep_call)
				.ok_or_else(|| Error::Invalid("missing call answer".into()))?;
			let result = *self
				.answers
				.get(&candidate.keep_result)
				.ok_or_else(|| Error::Invalid("missing result answer".into()))?;
			let protected = super::pruning::protected(
				candidate.history_index,
				candidate.turn,
				self.history_length,
				self.turns,
				self.restrictions.preserve_recent,
			);
			let chosen = if protected {
				Branch::Keep
			} else {
				branch(call, result, self.restrictions.threshold)
			};
			if replay.insert(candidate.id.clone(), chosen).is_some() {
				return Err(Error::Invalid(
					"duplicate historical decision candidate".into(),
				));
			}
		}
		if replay != self.branches
			|| self.fit.dropped
				!= replay
					.values()
					.filter(|branch| **branch == Branch::Drop)
					.count()
			|| self.fit.truncated
				> replay
					.values()
					.filter(|branch| **branch == Branch::TruncateResult)
					.count()
		{
			return Err(Error::Invalid(
				"historical decision branch or question coverage changed".into(),
			));
		}
		let coverage = self.validate_attempts()?;
		if coverage != self.questions.len() {
			return Err(Error::Invalid(
				"historical answers lack durable attempt evidence".into(),
			));
		}
		Ok(replay)
	}
	/// A rejection may follow an early failure, before any answer, branch or
	/// candidate exists, so it cannot replay. It still keeps every invariant
	/// that holds for a rejection at any stage: a reason other than fitting, a
	/// supported contract, the pinned Decider and model, a consistent retained
	/// state reference and fit counts of the context that did not fit. A
	/// rejection that followed a complete evaluation (`insufficient`, or
	/// `forbidden` with selected branches) replays in full. Every other one
	/// stopped before branch selection, so it reports no branch and the
	/// unchanged context, and without candidates nothing was asked, answered
	/// or charged. Candidates it does report are as the builder made them.
	pub fn validate_rejection(&self) -> Result<()> {
		if self.outcome != Outcome::Rejected || self.reason == Reason::Fits {
			return Err(Error::Invalid(
				"corrupt historical application outcome".into(),
			));
		}
		// Only a complete evaluation finds the proposed context too large, so
		// that rejection must replay in full, fit metrics included.
		if self.reason == Reason::Insufficient
			|| self.reason == Reason::Forbidden && !self.branches.is_empty()
		{
			return self.replay().map(|_| ());
		}
		self.validate_identity()?;
		if self.fit.retained != self.history_length
			|| self.fit.before <= self.fit.window
			|| !self.branches.is_empty()
			|| self.fit.proposed != self.fit.before
			|| self.fit.removed != 0
			|| self.fit.dropped != 0
			|| self.fit.truncated != 0
			|| self.projection_digest.is_some()
			|| self.reason == Reason::NoCandidates
				&& (!self.candidates.is_empty()
					|| !self.questions.is_empty()
					|| !self.answers.is_empty()
					|| !self.attempts.is_empty())
			|| self.reason.after_candidates() && self.candidates.is_empty()
		{
			return Err(Error::Invalid(
				"corrupt historical application outcome".into(),
			));
		}
		self.validate_candidates()?;
		self.validate_attempts()?;
		// Only an attempt whose answers failed validation records invalid answers.
		if self.reason == Reason::InvalidAnswers
			&& !self
				.attempts
				.iter()
				.any(|attempt| attempt.status == AttemptStatus::Failed)
		{
			return Err(Error::Invalid(
				"invalid answers require the attempt that returned them".into(),
			));
		}
		self.validate_contract()
	}
	/// Each attempt is well formed and asks questions of this decision that no
	/// other attempt asks; returns how many questions an answered attempt covers.
	fn validate_attempts(&self) -> Result<usize> {
		validate_attempt_set(&self.attempts, &self.questions)
	}
	/// Each candidate names an event of the history under the ID its boundary
	/// derives, with a call and a result question of its own, and every
	/// question belongs to a candidate. The builder makes candidates and
	/// questions together, so this holds before any branch is selected.
	fn validate_candidates(&self) -> Result<()> {
		let mut question_ids = std::collections::BTreeSet::new();
		let mut indexes = std::collections::BTreeSet::new();
		for candidate in &self.candidates {
			let turn_valid = match (candidate.turn, self.turns) {
				(None, _) => true,
				(Some(turn), Some(turns)) => turn < turns,
				(Some(_), None) => false,
			};
			if candidate.history_index >= self.history_length
				|| !indexes.insert(candidate.history_index)
				|| !turn_valid
				|| candidate.id != self.boundary.candidate_id(candidate.history_index)
				|| candidate.description.trim().is_empty()
				|| candidate.keep_call.is_empty()
				|| candidate.keep_result.is_empty()
				|| !question_ids.insert(&candidate.keep_call)
				|| !question_ids.insert(&candidate.keep_result)
			{
				return Err(Error::Invalid(
					"corrupt historical decision candidate".into(),
				));
			}
		}
		if question_ids.len() != self.questions.len()
			|| question_ids
				.iter()
				.any(|question| !self.questions.contains_key(*question))
			|| self
				.questions
				.values()
				.any(|question| question.description.trim().is_empty())
		{
			return Err(Error::Invalid(
				"historical decision questions differ from its candidates".into(),
			));
		}
		Ok(())
	}
	fn validate_identity(&self) -> Result<()> {
		self.boundary.validate()?;
		self.decider.validate()?;
		if self.decider.identity.registry_deployment != self.boundary.node {
			return Err(Error::Invalid(
				"historical Decider differs from the execution Node".into(),
			));
		}
		validate_digest(&self.state_digest)?;
		match self.provider_contract.as_str() {
			PROVIDER => validate_model(&self.model)?,
			_ => validate_decision_model(&self.model)?,
		}
		validate_digest(&self.configuration_parameters_digest)?;
		let route = usize::try_from(self.route).unwrap_or(usize::MAX);
		if self.route_models.get(route) != Some(&self.model)
			|| self.route_models.len() > MAX_ROUTES
			|| self.route_thresholds.len() != self.route_models.len()
			|| configuration_digest(
				&self.route_models,
				&self.route_thresholds,
				self.mode,
				&self.configuration_parameters_digest,
			) != self.decider.configuration_digest
		{
			return Err(Error::Invalid(
				"historical model or mode differs from the pinned Decider configuration".into(),
			));
		}
		if self.restrictions.threshold.value() > self.route_thresholds[route].value() {
			return Err(Error::Invalid(
				"historical restrictions are looser than the Route asked".into(),
			));
		}
		self.binding_restrictions.validate_floor()?;
		if !self
			.restrictions
			.at_least_as_strict(&self.binding_restrictions)
		{
			return Err(Error::Invalid(
				"historical restrictions are looser than the admitted Binding".into(),
			));
		}
		for source in &self.sources {
			validate_digest(&source.revision_digest)?;
			if source.resource.trim().is_empty() {
				return Err(Error::Invalid("corrupt historical source reference".into()));
			}
		}
		let valid_state = match (&self.state, &self.state_retention) {
			(StateReference::Disabled, StateRetentionWitness::Disabled) => true,
			(
				StateReference::Retained {
					id,
					digest,
					expires_at,
				},
				StateRetentionWitness::Enabled {
					id: retained_id,
					expires_at: deadline,
				},
			) => {
				!id.is_nil()
					&& id == retained_id
					&& digest == &self.state_digest
					&& expires_at == deadline
			}
			(
				StateReference::Expired {
					id,
					digest,
					expired_at,
				},
				StateRetentionWitness::Enabled {
					id: retained_id,
					expires_at: deadline,
				},
			) => {
				!id.is_nil()
					&& id == retained_id
					&& digest == &self.state_digest
					&& expired_at == deadline
			}
			_ => false,
		};
		if !valid_state {
			return Err(Error::Invalid(
				"historical retained-state reference changed".into(),
			));
		}
		Ok(())
	}
	fn validate_contract(&self) -> Result<()> {
		if self.version != EVIDENCE_VERSION
			|| self.rule != RULE
			|| !matches!(self.provider_contract.as_str(), PROVIDER | DECISIONS)
			|| self.builder != BUILDER
			|| self.option_source != OPTION_SOURCE
			|| self.restrictions.validate_floor().is_err()
		{
			return Err(Error::Invalid(
				"unsupported or incomplete historical decision evidence".into(),
			));
		}
		Ok(())
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

/// Each attempt is well formed and asks questions of the decision that no other
/// attempt asks; returns how many questions an answered attempt covers.
pub(super) fn validate_attempt_set(
	attempts: &[AttemptEvidence],
	questions: &Questions,
) -> Result<usize> {
	let mut coverage = std::collections::BTreeSet::new();
	let mut attempted_questions = std::collections::BTreeSet::new();
	let mut ids = std::collections::BTreeSet::new();
	for attempt in attempts {
		validate_digest(&attempt.request_digest)?;
		validate_digest(&attempt.question_digest)?;
		if attempt.id.is_nil()
			|| !ids.insert(attempt.id)
			|| attempt.questions.is_empty()
			|| attempt.owner_receipts.is_empty()
			|| attempt.owner_receipts.values().any(Uuid::is_nil)
		{
			return Err(Error::Invalid("corrupt historical attempt evidence".into()));
		}
		for question in &attempt.questions {
			if !questions.contains_key(question) || !attempted_questions.insert(question) {
				return Err(Error::Invalid(
					"historical attempts have conflicting question coverage".into(),
				));
			}
			if attempt.status == AttemptStatus::Answered {
				coverage.insert(question);
			}
		}
		if question_digest(questions, &attempt.questions)? != attempt.question_digest {
			return Err(Error::Invalid(
				"historical attempt asked other questions than its evidence names".into(),
			));
		}
	}
	Ok(coverage.len())
}

/// The commitment an attempt keeps to the definitions of the questions it asked.
pub fn question_digest(questions: &Questions, asked: &[String]) -> Result<String> {
	let mut selected = BTreeMap::new();
	for id in asked {
		let question = questions.get(id).ok_or_else(|| {
			Error::Invalid("decision attempt asks a question its catalog lacks".into())
		})?;
		selected.insert(id, question);
	}
	Ok(digest(&serde_json::to_value(selected)?))
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
