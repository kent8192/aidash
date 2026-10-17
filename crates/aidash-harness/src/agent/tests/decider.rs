//! A fixture compaction Decider: Jev answers every question with the Backend's
//! `jev_retention`, and an applied decision saves the Run as the native journal does.
use super::*;
use aidash_application::ports::decision::{
	Approval, DecisionAuthority, DecisionJournal, DecisionPorts, DecisionProvider, DispatchError,
	DispatchPermit, DispatchRecord, PreparedDispatch, PreparedRequest, RecoveredAttempt,
	Reservation, RetainedState,
};
use aidash_domain::decision::{
	AnswerType, AttemptStatus, BUILDER, BoundDecider, Boundary, DeciderConfig, DeciderPin,
	Evidence, Hook, Mode, OPTION_SOURCE, OutcomeLink, PROVIDER, Probability, Questions, RULE,
	Restrictions, SourcePin, StateBudget,
};

const IMPLEMENTATION: &str = "fixture-decider";

fn config() -> DeciderConfig {
	DeciderConfig {
		hook: Hook::Compaction,
		answer_type: AnswerType::Noul,
		description: "Keep needed history".into(),
		provider_contract: PROVIDER.into(),
		provider_credential: "openrouter".into(),
		model: "typesafe/jev-1.14".into(),
		builder: BUILDER.into(),
		option_source: OPTION_SOURCE.into(),
		rule: RULE.into(),
		keep_threshold: Probability::half(),
		mode: Mode::Enforce,
		max_calls_per_run: 100,
		state_retention: Default::default(),
		state_budget: StateBudget::default(),
	}
}

fn definition() -> Entry {
	serde_json::from_value(
		json!({"id":"compaction-decider","version":"1.0.0","kind":"decider",
		"name":{"en":"Compaction"},"description":{"en":"Retain history"},"config":config()}),
	)
	.unwrap()
}

/// The fixture Run's compaction Decider, or `None` when the Backend expects no
/// compaction.
pub(super) fn ports<'a>(backend: &Backend, node: &str, run: &Run) -> Option<DecisionPorts<'a>> {
	backend.0.jev_retention?;
	let definition = definition();
	let decider = BoundDecider {
		pin: DeciderPin {
			identity: QualifiedRef {
				registry_node: node.into(),
				id: definition.id.clone(),
				version: definition.version.clone(),
			},
			definition_digest: rules::digest(&serde_json::to_value(&definition).unwrap()),
			configuration_digest: config().digest().unwrap(),
			provider_implementation: IMPLEMENTATION.into(),
		},
		config: config(),
		restrictions: Restrictions::default(),
	};
	let fixture = || {
		Box::new(FixtureDecision {
			backend: backend.clone(),
			run: run.clone(),
		})
	};
	Some(DecisionPorts {
		decider,
		definition,
		authority: fixture(),
		journal: fixture(),
		provider: fixture(),
	})
}

struct FixtureDecision {
	backend: Backend,
	run: Run,
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
		Ok(config().digest()?)
	}
	fn plan(&self, _: &Value, questions: &Questions) -> Result<Vec<PreparedRequest>> {
		Ok(vec![PreparedRequest {
			body: serde_json::to_vec(&json!({"questions": questions.keys().collect::<Vec<_>>()}))?,
			questions: questions.clone(),
		}])
	}
	async fn prepare(
		&self,
		request: &PreparedRequest,
		_: Uuid,
	) -> Result<Box<dyn PreparedDispatch + '_>> {
		Ok(Box::new(FixtureDispatch {
			backend: self.backend.clone(),
			questions: request.questions.keys().cloned().collect(),
		}))
	}
}

struct FixtureDispatch {
	backend: Backend,
	questions: Vec<String>,
}
#[async_trait]
impl PreparedDispatch for FixtureDispatch {
	async fn dispatch(
		self: Box<Self>,
	) -> std::result::Result<BTreeMap<String, Probability>, DispatchError> {
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
