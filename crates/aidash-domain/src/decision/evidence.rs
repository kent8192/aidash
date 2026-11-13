//! Reader-controlled evidence is distinct from safe ordinary journal summaries.
use super::*;
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
	#[serde(default)]
	pub provider_report: AttemptProviderReport,
	pub id: Uuid,
	pub request_digest: String,
	pub questions: Vec<String>,
	/// Commits to the exact question definitions the attempt asked.
	pub question_digest: String,
	pub status: AttemptStatus,
	/// Canonical owner receipts are retained by their owners; these exact IDs link them.
	pub owner_receipts: BTreeMap<String, Uuid>,
}

/// Each attempt is well formed and asks questions of the decision that no other
/// attempt asks; returns how many questions an answered attempt covers.
pub(crate) fn validate_attempt_set(
	attempts: &[AttemptEvidence],
	questions: &Questions,
) -> Result<usize> {
	let mut coverage = std::collections::BTreeSet::new();
	let mut attempted_questions = std::collections::BTreeSet::new();
	let mut ids = std::collections::BTreeSet::new();
	for attempt in attempts {
		attempt.provider_report.validate()?;
		if attempt.status == AttemptStatus::NotDispatched
			&& attempt.provider_report != AttemptProviderReport::Unknown
		{
			return Err(Error::Invalid(
				"undispatched attempt cannot report provider facts".into(),
			));
		}
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

/// Appended measurement, never a causal claim about the model or downstream success.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OutcomeLink {
	pub decision: Uuid,
	pub next_inference: Option<String>,
	pub observed_status: String,
}

fn charged_stable_id(value: &serde_json::Value) -> Uuid {
	Uuid::parse_str(&digest(value)[7..39]).expect("canonical SHA256 contains 32 UUID hex digits")
}
pub fn charged_decision_identity(
	boundary: &crate::decision::Boundary,
	decider: &crate::decision::DeciderPin,
	binding_restrictions: &crate::decision::Restrictions,
	state: &str,
	sources: &[crate::decision::SourcePin],
	window: usize,
	route: u32,
) -> Uuid {
	charged_stable_id(
		&serde_json::json!({"contract":"aidash.decision/2","node":boundary.node,"run":boundary.run,"step":boundary.step,"cycle":boundary.cycle,"retry":boundary.retry,"input_revision":boundary.input_revision,"input_digest":boundary.input_digest,"decider":decider,"route":route,"binding_restrictions":binding_restrictions,"state":state,"sources":sources,"window":window}),
	)
}

pub fn charged_attempt_identity(
	decision: Uuid,
	request_digest: &str,
	questions: &[String],
) -> Uuid {
	charged_stable_id(
		&serde_json::json!({"decision":decision,"request":request_digest,"questions":questions}),
	)
}
