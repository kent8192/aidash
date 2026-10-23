use super::*;
use aidash_domain::registry::bindings::QualifiedRef;
use std::sync::{
	Mutex,
	atomic::{AtomicBool, AtomicUsize, Ordering},
};
fn config() -> DeciderConfig {
	DeciderConfig {
		hook: Hook::ApprovalObservation,
		answer_type: AnswerType::Noul,
		description: "Independent observer".into(),
		provider_credential: "openrouter".into(),
		routes: vec![DeciderRoute::jev("typesafe/jev-1.14")],
		builder: BUILDER.into(),
		option_source: aidash_domain::decision::observation::OPTION_SOURCE.into(),
		rule: aidash_domain::decision::observation::RULE.into(),
		mode: Mode::Shadow,
		max_calls_per_run: 10,
		state_retention: Default::default(),
	}
}
fn definition() -> aidash_domain::registry::Entry {
	serde_json::from_value(json!({"id":"observer","version":"1.0.0","kind":"decider","name":{"en":"Observer"},"description":{"en":"Observe human approvals"},"config":config()})).unwrap()
}
fn frame(kind: ApprovalKind) -> (ObservationFrame, ObservationInput) {
	let definition = definition();
	let content = json!("Read the documentation, ask before creating an agent");
	let input = InputMaterial {
		reference: FrameReference {
			resource: "human/3".into(),
			revision_digest: digest(&content),
		},
		sequence: Some(3),
		content,
	};
	let request = Uuid::new_v4();
	let task = Uuid::new_v4();
	let pending = match kind {
		ApprovalKind::ToolCall => PendingAction::ToolCall {
			name: "read".into(),
			arguments: json!({"path":"README.md"}),
			definition: json!({"version":"1.0.0"}),
		},
		ApprovalKind::Outbound => PendingAction::Outbound {
			name: "notify".into(),
			arguments: json!({"destination":"https://example.com"}),
			definition: json!({"version":"1.0.0"}),
		},
		ApprovalKind::WebSearch => PendingAction::WebSearch {
			name: "web_search".into(),
			arguments: json!({"query":"documentation"}),
			definition: json!({"version":"1.0.0"}),
		},
		ApprovalKind::PageReading => PendingAction::PageReading {
			name: "page_read".into(),
			arguments: json!({"url":"https://example.com"}),
			definition: json!({"version":"1.0.0"}),
		},
		ApprovalKind::Generation => PendingAction::Generation {
			request: PendingGeneration {
				request,
				task,
				policy_id: "generation-policy".into(),
				policy_revision: 7,
				requested_agent_id: "generated".into(),
				requested_agent_version: "1.0.0".into(),
				definition: json!({"instructions":"Read docs"}),
				token_limit: 1000,
				depth: 1,
			},
		},
	};
	let mut frame = ObservationFrame {
		version: 1,
		id: Uuid::nil(),
		tenant: "tenant".into(),
		approval: Uuid::new_v4(),
		node: "aidash://node".into(),
		created_at: Utc::now(),
		kind,
		origin: if kind == ApprovalKind::Generation {
			ObservationOrigin::Generation {
				request,
				task,
				policy_id: "generation-policy".into(),
				policy_revision: 7,
			}
		} else {
			ObservationOrigin::Run {
				run: Uuid::new_v4(),
				step: 2,
				input_revision: 3,
				cycle: 0,
			}
		},
		pending_digest: pending.digest().unwrap(),
		settings: ObserverSettings {
			revision: 4,
			decider: Some(DeciderPin {
				identity: QualifiedRef {
					registry_deployment: "aidash://node".into(),
					id: definition.id.clone(),
					version: definition.version.clone(),
				},
				definition_digest: digest(&serde_json::to_value(&definition).unwrap()),
				configuration_digest: config().digest().unwrap(),
				provider_implementation: "observer-adapter".into(),
			}),
			config: Some(config()),
			restrictions: Restrictions::default(),
		},
		input_cutoff: 3,
		input_refs: vec![input.reference.clone()],
		instruction_ref: None,
		action_ref: FrameReference {
			resource: "pending/action".into(),
			revision_digest: pending.digest().unwrap(),
		},
		policy_refs: vec![],
		prior_context_refs: vec![],
		operation_class: Some("read".into()),
		effect_class: None,
	};
	frame.id = frame.identity();
	(
		frame,
		ObservationInput {
			inputs: vec![input],
			instructions: None,
			policies: vec![],
			prior_calls: vec![],
			pending,
		},
	)
}
#[derive(Default)]
struct Saved {
	permits: Vec<DispatchPermit>,
	finished: BTreeMap<Uuid, (AttemptStatus, Option<BTreeMap<String, Probability>>)>,
	evidence: Option<ObservationEvidence>,
}
struct Rig {
	saved: Mutex<Saved>,
	calls: AtomicUsize,
	fail_commit: AtomicBool,
	fail_transport: bool,
	allowance: usize,
}
impl Rig {
	fn new() -> Self {
		Self {
			saved: Mutex::new(Saved::default()),
			calls: AtomicUsize::new(0),
			fail_commit: AtomicBool::new(false),
			fail_transport: false,
			allowance: 10,
		}
	}
	fn gate(&self) -> DecisionGate<'_> {
		DecisionGate {
			authority: self,
			journal: self,
			provider: self,
		}
	}
}
#[async_trait]
impl DecisionAuthority for Rig {
	async fn check(&self, _: &Boundary, _: &DeciderPin, _: &[SourcePin]) -> Result<Approval> {
		Ok(Approval {
			restrictions: Restrictions::default(),
			state_retention: Default::default(),
		})
	}
	async fn read(&self, _: &Evidence, _: bool) -> Result<()> {
		unreachable!()
	}
}
#[async_trait]
impl ObservationJournal for Rig {
	async fn completed(
		&self,
		_: &ObservationFrame,
		_: &ObservationLease,
	) -> Result<Option<ObservationEvidence>> {
		Ok(self.saved.lock().unwrap().evidence.clone())
	}
	async fn commit_observation(
		&self,
		frame: &ObservationFrame,
		_: &ObservationLease,
		evidence: &ObservationEvidence,
	) -> Result<()> {
		evidence.validate(frame)?;
		if self.fail_commit.load(Ordering::SeqCst) {
			return Err(Error::Conflict("observer lease lost".into()));
		}
		self.saved.lock().unwrap().evidence = Some(evidence.clone());
		Ok(())
	}
}
#[async_trait]
impl DecisionJournal for Rig {
	async fn recover(&self, decision: Uuid, _: &Boundary) -> Result<Vec<RecoveredAttempt>> {
		let saved = self.saved.lock().unwrap();
		Ok(saved
			.permits
			.iter()
			.filter(|p| p.record.decision == decision)
			.map(|permit| {
				let (status, answers) = saved
					.finished
					.get(&permit.record.attempt)
					.cloned()
					.unwrap_or((AttemptStatus::Uncertain, None));
				RecoveredAttempt {
					permit: permit.clone(),
					status,
					answers,
				}
			})
			.collect())
	}
	async fn reserve(&self, record: &DispatchRecord) -> Result<Reservation> {
		let mut saved = self.saved.lock().unwrap();
		if saved.permits.len() >= self.allowance {
			return Err(Error::Context(
				aidash_domain::context::recovery::Failure::DeciderCallsExhausted,
			));
		}
		let permit = DispatchPermit {
			record: record.clone(),
			owner_receipts: BTreeMap::from([("observer-owner".into(), Uuid::new_v4())]),
		};
		saved.permits.push(permit.clone());
		Ok(Reservation::Fresh(permit))
	}
	async fn finish_attempt(
		&self,
		permit: &DispatchPermit,
		answers: Option<&BTreeMap<String, Probability>>,
		status: AttemptStatus,
	) -> Result<()> {
		self.saved
			.lock()
			.unwrap()
			.finished
			.insert(permit.record.attempt, (status, answers.cloned()));
		Ok(())
	}
	async fn commit(
		&self,
		_: &Evidence,
		_: Option<&Context>,
		_: Option<&RetainedState>,
	) -> Result<()> {
		panic!("observer cannot mutate a Run")
	}
	async fn commit_approval(&self, _: &ApprovalEvidence) -> Result<()> {
		panic!("observer cannot settle approval")
	}
	async fn expire_states(&self, _: DateTime<Utc>) -> Result<usize> {
		unreachable!()
	}
	async fn append_outcome(&self, _: &OutcomeLink) -> Result<()> {
		unreachable!()
	}
}
#[async_trait]
impl DecisionProvider for Rig {
	fn implementation_id(&self) -> &str {
		"observer-adapter"
	}
	fn configuration_digest(&self) -> Result<String> {
		Ok(config().digest()?)
	}
	fn plan(&self, _: usize, state: &Value, questions: &Questions) -> Result<Vec<PreparedRequest>> {
		Ok(vec![PreparedRequest {
			body: serde_json::to_vec(&json!({"state":state,"questions":questions}))?,
			questions: questions.clone(),
		}])
	}
	async fn prepare(
		&self,
		_: usize,
		request: &PreparedRequest,
		_: Uuid,
	) -> Result<Box<dyn PreparedDispatch + '_>> {
		Ok(Box::new(Transport {
			rig: self,
			questions: request.questions.clone(),
		}))
	}
}
struct Transport<'a> {
	rig: &'a Rig,
	questions: Questions,
}
#[async_trait]
impl PreparedDispatch for Transport<'_> {
	async fn dispatch(
		self: Box<Self>,
	) -> std::result::Result<BTreeMap<String, Probability>, DispatchError> {
		self.rig.calls.fetch_add(1, Ordering::SeqCst);
		if self.rig.fail_transport {
			return Err(DispatchError::ProviderFailure(Error::External(
				"timeout".into(),
			)));
		}
		Ok(self
			.questions
			.keys()
			.map(|q| {
				(
					q.clone(),
					Probability::new(if q == "boundary" { 0.8 } else { 0.1 }).unwrap(),
				)
			})
			.collect())
	}
}
#[tokio::test]
async fn all_five_typed_actions_are_judged_without_approval_settlement_or_live_run() {
	for kind in [
		ApprovalKind::ToolCall,
		ApprovalKind::Outbound,
		ApprovalKind::WebSearch,
		ApprovalKind::PageReading,
		ApprovalKind::Generation,
	] {
		let rig = Rig::new();
		let (frame, input) = frame(kind);
		let lease = ObservationLease {
			worker: Uuid::new_v4(),
			revision: 1,
		};
		let definition = definition();
		let evidence = rig
			.gate()
			.observe_approval(
				&ObservationRequest {
					frame: &frame,
					lease: &lease,
					input: Some(&input),
					definition: Some(&definition),
					now: Utc::now(),
				},
				&rig,
			)
			.await
			.unwrap();
		assert_eq!(
			evidence.outcome,
			ObservationOutcome::Available {
				verdict: Verdict::Blocked(BlockReason::Boundary)
			}
		);
		assert_eq!(rig.calls.load(Ordering::SeqCst), 1);
		assert_eq!(
			rig.saved.lock().unwrap().permits[0].record.boundary.run,
			frame.id
		);
		let state = build_observation_state(&frame, &input, &config().routes[0].state_budget)
			.unwrap()
			.state;
		assert_eq!(
			state["pending"]["kind"],
			serde_json::to_value(kind).unwrap()
		);
		if kind == ApprovalKind::Generation {
			assert!(state["pending"]["request"].get("request").is_some());
			assert!(state["pending"].get("name").is_none());
		}
	}
}
#[tokio::test]
async fn answered_attempt_survives_lost_observer_lease_without_second_charge_or_provider_call() {
	let rig = Rig::new();
	let (frame, input) = frame(ApprovalKind::ToolCall);
	let mut lease = ObservationLease {
		worker: Uuid::new_v4(),
		revision: 1,
	};
	let definition = definition();
	rig.fail_commit.store(true, Ordering::SeqCst);
	let request = ObservationRequest {
		frame: &frame,
		lease: &lease,
		input: Some(&input),
		definition: Some(&definition),
		now: Utc::now(),
	};
	assert!(rig.gate().observe_approval(&request, &rig).await.is_err());
	assert_eq!(rig.calls.load(Ordering::SeqCst), 1);
	rig.fail_commit.store(false, Ordering::SeqCst);
	lease.worker = Uuid::new_v4();
	lease.revision += 1;
	let request = ObservationRequest {
		frame: &frame,
		lease: &lease,
		input: Some(&input),
		definition: Some(&definition),
		now: Utc::now(),
	};
	let evidence = rig.gate().observe_approval(&request, &rig).await.unwrap();
	assert_eq!(
		evidence.outcome,
		ObservationOutcome::Available {
			verdict: Verdict::Blocked(BlockReason::Boundary)
		}
	);
	assert_eq!(rig.calls.load(Ordering::SeqCst), 1);
	assert_eq!(rig.saved.lock().unwrap().permits.len(), 1);
	assert_eq!(
		rig.gate().observe_approval(&request, &rig).await.unwrap(),
		evidence
	);
	assert_eq!(rig.calls.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn uncertain_attempt_cannot_be_dispatched_again_and_exhaustion_is_no_verdict() {
	for (transport, allowance, reason, calls) in [
		(true, 10, UnavailableReason::JournalFailure, 1),
		(false, 0, UnavailableReason::CallsExhausted, 0),
	] {
		let rig = Rig {
			fail_transport: transport,
			allowance,
			..Rig::new()
		};
		let (frame, input) = frame(ApprovalKind::Generation);
		let lease = ObservationLease {
			worker: Uuid::new_v4(),
			revision: 1,
		};
		let definition = definition();
		rig.fail_commit.store(true, Ordering::SeqCst);
		let request = ObservationRequest {
			frame: &frame,
			lease: &lease,
			input: Some(&input),
			definition: Some(&definition),
			now: Utc::now(),
		};
		assert!(rig.gate().observe_approval(&request, &rig).await.is_err());
		rig.fail_commit.store(false, Ordering::SeqCst);
		let recovered = rig.gate().observe_approval(&request, &rig).await.unwrap();
		assert_eq!(
			recovered.outcome,
			ObservationOutcome::Unavailable { reason }
		);
		assert_eq!(rig.calls.load(Ordering::SeqCst), calls);
	}
}
#[tokio::test]
async fn future_or_changed_inputs_cannot_trigger_retrospective_provider_calls() {
	let rig = Rig::new();
	let (frame, mut input) = frame(ApprovalKind::WebSearch);
	let lease = ObservationLease {
		worker: Uuid::new_v4(),
		revision: 1,
	};
	let definition = definition();
	input.inputs[0].sequence = Some(frame.input_cutoff + 1);
	let evidence = rig
		.gate()
		.observe_approval(
			&ObservationRequest {
				frame: &frame,
				lease: &lease,
				input: Some(&input),
				definition: Some(&definition),
				now: Utc::now(),
			},
			&rig,
		)
		.await
		.unwrap();
	assert_eq!(
		evidence.outcome,
		ObservationOutcome::Unavailable {
			reason: UnavailableReason::MissingState
		}
	);
	assert_eq!(rig.calls.load(Ordering::SeqCst), 0);
	let original = frame.id;
	let mut changed = frame.clone();
	changed.settings.revision += 1;
	assert_ne!(changed.identity(), original);
	assert!(changed.validate().is_err());
}

#[test]
fn frozen_reference_commitments_reject_changed_content_even_if_ids_are_unchanged() {
	let (mut frame, mut input) = frame(ApprovalKind::ToolCall);
	let instructions = json!("Only read documentation");
	let policy = json!({"outbound":"ask"});
	let call = PriorCallMaterial {
		reference: FrameReference {
			resource: "context/2".into(),
			revision_digest: digest(&json!({"name":"read","arguments":{"path":"docs.md"}})),
		},
		name: "read".into(),
		arguments: json!({"path":"docs.md"}),
	};
	input.instructions = Some(ReferenceMaterial {
		reference: FrameReference {
			resource: "instructions/1".into(),
			revision_digest: digest(&instructions),
		},
		content: instructions,
	});
	input.policies.push(ReferenceMaterial {
		reference: FrameReference {
			resource: "policy/1".into(),
			revision_digest: digest(&policy),
		},
		content: policy,
	});
	input.prior_calls.push(call);
	frame.instruction_ref = input.instructions.as_ref().map(|i| i.reference.clone());
	frame.policy_refs = input.policies.iter().map(|p| p.reference.clone()).collect();
	frame.prior_context_refs = input
		.prior_calls
		.iter()
		.map(|p| p.reference.clone())
		.collect();
	frame.id = frame.identity();
	input.validate(&frame).unwrap();
	for kind in 0..4 {
		let mut changed = input.clone();
		match kind {
			0 => changed.inputs[0].content = json!("new human input"),
			1 => changed.instructions.as_mut().unwrap().content = json!("new instructions"),
			2 => changed.policies[0].content = json!({"outbound":"allow"}),
			_ => changed.prior_calls[0].arguments = json!({"path":"secrets"}),
		}
		assert!(changed.validate(&frame).is_err());
	}
}
