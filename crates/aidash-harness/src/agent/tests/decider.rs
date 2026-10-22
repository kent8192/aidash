//! A fixture compaction Decider: Jev answers every question with the Backend's
//! `jev_retention`, and an applied decision saves the Run as the native journal does.
use super::*;
use aidash_application::ports::decision::{
	Approval, DecisionAuthority, DecisionJournal, DecisionPorts, DecisionProvider, DispatchError,
	DispatchPermit, DispatchRecord, PreparedDispatch, PreparedRequest, RecoveredAttempt,
	Reservation, RetainedState,
};
use aidash_domain::decision::{
	APPROVAL_BUILDER, APPROVAL_OPTION_SOURCE, APPROVAL_RULE, AnswerType, ApprovalEvidence,
	AttemptStatus, BUILDER, BlockReason, BoundDecider, Boundary, DeciderConfig, DeciderPin,
	DeciderRoute, Evidence, Hook, Mode, OPTION_SOURCE, OutcomeLink, Probability, Questions, RULE,
	Restrictions, SourcePin,
};

const IMPLEMENTATION: &str = "fixture-decider";

fn config() -> DeciderConfig {
	DeciderConfig {
		hook: Hook::Compaction,
		answer_type: AnswerType::Noul,
		description: "Keep needed history".into(),
		provider_credential: "openrouter".into(),
		routes: vec![DeciderRoute::jev("typesafe/jev-1.14")],
		builder: BUILDER.into(),
		option_source: OPTION_SOURCE.into(),
		rule: RULE.into(),
		mode: Mode::Enforce,
		max_calls_per_run: 100,
		state_retention: Default::default(),
	}
}

fn definition() -> Entry {
	serde_json::from_value(
		json!({"id":"compaction-decider","version":"1.0.0","kind":"decider",
		"name":{"en":"Compaction"},"description":{"en":"Retain history"},"config":config()}),
	)
	.unwrap()
}

/// How the fixture approval Decider answers the five block questions.
#[derive(Clone, Copy)]
pub(super) enum Reply {
	/// The probability of each category, in the rule's order.
	Answers([f64; 5]),
	/// The provider fails.
	Outage,
	/// The Run has spent the Decider's call allowance.
	AllowanceSpent,
}
/// The Tenant's approval Decider, in the mode it runs in.
#[derive(Clone, Copy)]
pub(super) struct ApprovalDecider {
	pub(super) mode: Mode,
	pub(super) reply: Reply,
}
impl ApprovalDecider {
	pub(super) fn allowing(mode: Mode) -> Self {
		Self {
			mode,
			reply: Reply::Answers([0.0; 5]),
		}
	}
	pub(super) fn flagging(reason: BlockReason) -> Self {
		let mut answers = [0.0; 5];
		let place = BlockReason::CATEGORIES
			.iter()
			.position(|category| *category == reason)
			.unwrap();
		answers[place] = 0.9;
		Self {
			mode: Mode::Enforce,
			reply: Reply::Answers(answers),
		}
	}
	pub(super) fn unavailable() -> Self {
		Self {
			mode: Mode::Enforce,
			reply: Reply::Outage,
		}
	}
	pub(super) fn allowance_spent() -> Self {
		Self {
			mode: Mode::Enforce,
			reply: Reply::AllowanceSpent,
		}
	}
}
fn approval_config(mode: Mode) -> DeciderConfig {
	DeciderConfig {
		hook: Hook::Approval,
		description: "Block risky calls".into(),
		builder: APPROVAL_BUILDER.into(),
		option_source: APPROVAL_OPTION_SOURCE.into(),
		rule: APPROVAL_RULE.into(),
		mode,
		max_calls_per_run: 200,
		..config()
	}
}
fn approval_definition(mode: Mode) -> Entry {
	serde_json::from_value(
		json!({"id":"approval-decider","version":"1.0.0","kind":"decider",
		"name":{"en":"Approval"},"description":{"en":"Judge calls"},"config":approval_config(mode)}),
	)
	.unwrap()
}

/// The fixture Run's compaction Decider, or `None` when the Backend expects no
/// compaction.
pub(super) fn ports<'a>(backend: &Backend, node: &str, run: &Run) -> Option<DecisionPorts<'a>> {
	backend.0.jev_retention?;
	Some(bind(backend, node, run, config(), definition(), false))
}

/// The fixture Run's approval Decider, or `None` when Auto is not available.
pub(super) fn approval_ports<'a>(
	backend: &Backend,
	node: &str,
	run: &Run,
) -> Option<DecisionPorts<'a>> {
	let mode = backend.0.approval_decider?.mode;
	Some(bind(
		backend,
		node,
		run,
		approval_config(mode),
		approval_definition(mode),
		true,
	))
}

fn bind<'a>(
	backend: &Backend,
	node: &str,
	run: &Run,
	config: DeciderConfig,
	definition: Entry,
	approval: bool,
) -> DecisionPorts<'a> {
	let decider = BoundDecider {
		pin: DeciderPin {
			identity: QualifiedRef {
				registry_deployment: node.into(),
				id: definition.id.clone(),
				version: definition.version.clone(),
			},
			definition_digest: rules::digest(&serde_json::to_value(&definition).unwrap()),
			configuration_digest: config.digest().unwrap(),
			provider_implementation: IMPLEMENTATION.into(),
		},
		config,
		restrictions: Restrictions::default(),
	};
	let fixture = || {
		Box::new(FixtureDecision {
			backend: backend.clone(),
			run: run.clone(),
			approval,
		})
	};
	DecisionPorts {
		decider,
		definition,
		authority: fixture(),
		journal: fixture(),
		provider: fixture(),
	}
}

struct FixtureDecision {
	backend: Backend,
	run: Run,
	/// Serves the approval Decider instead of the compaction Decider.
	approval: bool,
}

#[async_trait]
impl DecisionAuthority for FixtureDecision {
	async fn check(&self, _: &Boundary, _: &DeciderPin, _: &[SourcePin]) -> Result<Approval> {
		Ok(Approval {
			restrictions: Restrictions::default(),
			state_retention: Default::default(),
		})
	}
	async fn read(&self, _: &Evidence, _: bool) -> Result<()> {
		Ok(())
	}
}

#[async_trait]
impl DecisionJournal for FixtureDecision {
	async fn recover(&self, _: Uuid, _: &Boundary) -> Result<Vec<RecoveredAttempt>> {
		Ok(vec![])
	}
	async fn reserve(&self, record: &DispatchRecord) -> Result<Reservation> {
		if self.approval
			&& self
				.backend
				.0
				.approval_decider
				.is_some_and(|decider| matches!(decider.reply, Reply::AllowanceSpent))
		{
			return Err(Error::Context(
				aidash_domain::context::recovery::Failure::DeciderCallsExhausted,
			));
		}
		Ok(Reservation::Fresh(DispatchPermit {
			record: record.clone(),
			owner_receipts: BTreeMap::from([("run".into(), Uuid::new_v4())]),
		}))
	}
	async fn finish_attempt(
		&self,
		_: &DispatchPermit,
		_: Option<&BTreeMap<String, Probability>>,
		_: AttemptStatus,
	) -> Result<()> {
		Ok(())
	}
	async fn commit(
		&self,
		_: &Evidence,
		context: Option<&Context>,
		_: Option<&RetainedState>,
	) -> Result<()> {
		self.backend.record("decision.commit");
		if let Some(context) = context {
			let mut run = self.run.clone();
			run.context = context.clone();
			self.backend.persist(&run, "context.compacted", None)?;
		}
		Ok(())
	}
	async fn commit_approval(&self, evidence: &ApprovalEvidence) -> Result<()> {
		self.backend.record("decision.approval");
		self.backend
			.0
			.approval_evidence
			.lock()
			.unwrap()
			.push(evidence.clone());
		Ok(())
	}
	async fn expire_states(&self, _: chrono::DateTime<chrono::Utc>) -> Result<usize> {
		Ok(0)
	}
	async fn append_outcome(&self, _: &OutcomeLink) -> Result<()> {
		Ok(())
	}
}

#[async_trait]
impl DecisionProvider for FixtureDecision {
	fn implementation_id(&self) -> &str {
		IMPLEMENTATION
	}
	fn configuration_digest(&self) -> Result<String> {
		let config = match self.backend.0.approval_decider {
			Some(decider) if self.approval => approval_config(decider.mode),
			_ => config(),
		};
		Ok(config.digest()?)
	}
	fn plan(&self, _: usize, state: &Value, questions: &Questions) -> Result<Vec<PreparedRequest>> {
		if self.approval {
			self.backend
				.0
				.judged_states
				.lock()
				.unwrap()
				.push(state.clone());
		}
		Ok(vec![PreparedRequest {
			body: serde_json::to_vec(&json!({"questions": questions.keys().collect::<Vec<_>>()}))?,
			questions: questions.clone(),
		}])
	}
	async fn prepare(
		&self,
		_: usize,
		request: &PreparedRequest,
		_: Uuid,
	) -> Result<Box<dyn PreparedDispatch + '_>> {
		Ok(Box::new(FixtureDispatch {
			backend: self.backend.clone(),
			questions: request.questions.keys().cloned().collect(),
			approval: self.approval,
		}))
	}
}

struct FixtureDispatch {
	backend: Backend,
	questions: Vec<String>,
	approval: bool,
}
#[async_trait]
impl PreparedDispatch for FixtureDispatch {
	async fn dispatch(
		self: Box<Self>,
	) -> std::result::Result<BTreeMap<String, Probability>, DispatchError> {
		if self.approval {
			let Some(decider) = self.backend.0.approval_decider else {
				unexpected("approval decision")
			};
			self.backend.record("jev.judge");
			let Reply::Answers(values) = decider.reply else {
				return Err(Error::External("route outage".into()).into());
			};
			return Ok(BlockReason::CATEGORIES
				.into_iter()
				.zip(values)
				.map(|(reason, value)| (reason.id().to_owned(), Probability::new(value).unwrap()))
				.collect());
		}
		let Some(retention) = self.backend.0.jev_retention else {
			unexpected("compaction")
		};
		self.backend.record("jev.ask");
		let retention = Probability::new(retention).unwrap();
		Ok(self
			.questions
			.into_iter()
			.map(|question| (question, retention))
			.collect())
	}
}
