//! The Completion Gate (#112) completes a Run only with passing Verification
//! Evidence for the exact mounted content, or when no Check applies.
use super::*;
use aidash_application::ports::execution::completion::{
	CheckRequest, CheckStart, CheckStatus, CompletionChecks, ContentState, MountedContent,
};
use aidash_domain::completion::{CheckOutcome, CompletionBasis, CompletionPolicy};

/// What one fake Check operation reports once it finishes.
#[derive(Clone)]
pub(super) enum Outcome {
	Pass,
	Fail(&'static str),
	Lost,
}

/// Completion Checks over an in-memory Working Area.
#[derive(Default)]
pub(super) struct FakeChecks {
	/// Mounted content digest; `None` means the Run has no Working Area.
	content: Mutex<Option<String>>,
	busy: AtomicBool,
	/// Why a Check cannot start; `None` lets it start.
	unavailable: Mutex<Option<String>>,
	/// Outcomes of the next started Checks, in order.
	outcomes: Mutex<VecDeque<Outcome>>,
	/// Started operations by idempotency key: id, polls left before finishing, outcome.
	operations: Mutex<BTreeMap<Uuid, (Uuid, u32, Outcome)>>,
	/// Every start request: key, command and the revision it ran at.
	started: Mutex<Vec<(Uuid, String, i64)>>,
	/// Each start request's own timeout and remaining verification time.
	timeouts: Mutex<Vec<(Option<u64>, u64)>>,
	/// Status polls answered `Running` before an operation finishes.
	polls: u32,
}

impl FakeChecks {
	fn with(content: &str, outcomes: impl IntoIterator<Item = Outcome>) -> Self {
		Self {
			content: Mutex::new(Some(content.into())),
			outcomes: Mutex::new(outcomes.into_iter().collect()),
			..Default::default()
		}
	}
	fn set_content(&self, content: &str) {
		*self.content.lock().unwrap() = Some(content.into());
	}
	fn started(&self) -> usize {
		self.started.lock().unwrap().len()
	}
}

#[async_trait]
impl CompletionChecks for FakeChecks {
	async fn content(&self, run: &Run) -> Result<Option<MountedContent>> {
		let _ = run;
		Ok(self
			.content
			.lock()
			.unwrap()
			.clone()
			.map(|digest| MountedContent {
				digest,
				revision: 7,
				state: if self.busy.load(Ordering::SeqCst) {
					ContentState::Busy
				} else {
					ContentState::Ready
				},
			}))
	}
	async fn start(&self, run: &Run, request: &CheckRequest<'_>) -> Result<CheckStart> {
		let _ = run;
		if let Some(detail) = self.unavailable.lock().unwrap().clone() {
			return Ok(CheckStart::Unavailable(detail));
		}
		self.started
			.lock()
			.unwrap()
			.push((request.key, request.command.into(), request.revision));
		self.timeouts
			.lock()
			.unwrap()
			.push((request.timeout_seconds, request.remaining_seconds));
		let mut operations = self.operations.lock().unwrap();
		let (operation_id, ..) = operations
			.entry(request.key)
			.or_insert_with(|| {
				let outcome = self
					.outcomes
					.lock()
					.unwrap()
					.pop_front()
					.expect("a scripted Check outcome");
				(Uuid::new_v4(), self.polls, outcome)
			})
			.clone();
		Ok(CheckStart::Started {
			operation_id,
			timeout_seconds: 60,
		})
	}
	async fn status(&self, run: &Run, operation: Uuid) -> Result<CheckStatus> {
		let _ = run;
		let mut operations = self.operations.lock().unwrap();
		let (_, polls, outcome) = operations
			.values_mut()
			.find(|(id, ..)| *id == operation)
			.expect("a started Check");
		if *polls > 0 {
			*polls -= 1;
			return Ok(CheckStatus::Running);
		}
		Ok(match outcome {
			Outcome::Pass => CheckStatus::Finished {
				passed: true,
				exit_code: Some(0),
				diagnostics: "test result: ok".into(),
			},
			Outcome::Fail(output) => CheckStatus::Finished {
				passed: false,
				exit_code: Some(101),
				diagnostics: (*output).into(),
			},
			Outcome::Lost => CheckStatus::Unavailable("runner lost the operation".into()),
		})
	}
}

fn tests_policy(extra: Value) -> CompletionPolicy {
	let mut policy = json!({
		"version": "checks/1",
		"checks": [{"name": "tests", "capability": "shell", "command": "cargo test"}],
	});
	policy
		.as_object_mut()
		.unwrap()
		.extend(extra.as_object().unwrap().clone());
	let policy: CompletionPolicy = serde_json::from_value(policy).unwrap();
	policy.validate().unwrap();
	policy
}

/// An Agent with a Completion Policy and a bound shell, on a node whose
/// Working Area currently holds `content`.
fn gated(fixture: &mut Fixture, policy: CompletionPolicy, checks: Option<FakeChecks>) {
	let mut snapshot = DeferredSnapshot::default();
	snapshot.bind(
		QualifiedRef::builtin(NODE, "shell"),
		"tool",
		"shell",
		json!(core_descriptor(NODE, "shell").unwrap()),
		Some("shell"),
		BindingOrigin::Explicit,
	);
	fixture.run.context.binding_snapshot = Some(Box::new(snapshot.snapshot()));
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.completion = Some(policy);
	state.checks = checks;
	state.task.lock().unwrap().status = TaskStatus::Running;
}

fn checks(fixture: &Fixture) -> &FakeChecks {
	fixture.backend.0.checks.as_ref().unwrap()
}

/// The model's final answer: an Accepted Response without tool calls.
fn answer(fixture: &mut Fixture, text: &str) {
	fixture.run.state = RunState::ToolCall(Box::new(ToolCallState {
		response: ModelResponse {
			text: text.into(),
			usage_complete: true,
			..Default::default()
		},
		included_input_seq: fixture.run.observed_input_seq,
		..Default::default()
	}));
}

/// Advance until the Run leaves the gate: it completed, went back to the
/// model or waits for a person. Timer waits of a running Check are resumed.
async fn settle(fixture: &mut Fixture) {
	for _ in 0..12 {
		match &fixture.run.state {
			RunState::ToolCall(_) => {}
			RunState::Waiting(wait) if matches!(**wait, WaitingState::Timer { .. }) => {}
			_ => return,
		}
		advance_sources(fixture).await.unwrap();
	}
	panic!("the Completion Gate did not settle");
}

fn repair_notes(run: &Run) -> Vec<aidash_domain::completion::RepairNote> {
	run.context
		.events()
		.filter_map(|event| match event {
			ContextEvent::CompletionRepairRequired { checks, .. } => Some(checks.clone()),
			_ => None,
		})
		.flatten()
		.collect()
}

fn completed(fixture: &Fixture) -> bool {
	fixture.backend.0.task.lock().unwrap().status == TaskStatus::Completed
}

fn review_prompt(fixture: &Fixture) -> String {
	let request = fixture.backend.0.human.lock().unwrap().clone().unwrap();
	assert_eq!(request.kind, "INFORMATION_REQUEST");
	request.prompt
}

fn answer_review(fixture: &Fixture, response: Value) {
	fixture
		.backend
		.0
		.human
		.lock()
		.unwrap()
		.as_mut()
		.unwrap()
		.response = Some(response);
}

#[rstest]
#[tokio::test]
async fn a_failing_check_sends_the_answer_back_and_only_passing_evidence_completes(
	mut fixture: Fixture,
) {
	gated(
		&mut fixture,
		tests_policy(json!({})),
		Some(FakeChecks::with(
			"sha256:edited",
			[
				Outcome::Fail("test parser::rejects_empty ... FAILED"),
				Outcome::Pass,
			],
		)),
	);
	fixture.run.context.completion.start_content = Some("sha256:start".into());
	// A fabricated success claim is only text; the Check decides.
	answer(&mut fixture, "All tests pass. ✅");
	settle(&mut fixture).await;
	assert_eq!(fixture.run.phase(), RunPhase::Thinking);
	assert!(!completed(&fixture));
	assert!(fixture.backend.0.artifacts.lock().unwrap().is_empty());
	let notes = repair_notes(&fixture.run);
	assert_eq!(notes.len(), 1);
	assert_eq!(notes[0].outcome, CheckOutcome::Failed);
	assert_eq!(notes[0].exit_code, Some(101));
	assert!(notes[0].diagnostics.contains("rejects_empty"));
	assert_eq!(fixture.run.context.completion.repairs, 1);

	// The model repairs the files and answers again; the Check runs on them.
	checks(&fixture).set_content("sha256:repaired");
	answer(&mut fixture, "Fixed the parser.");
	settle(&mut fixture).await;
	assert_eq!(fixture.run.phase(), RunPhase::Completed);
	assert_eq!(
		*fixture.backend.0.artifacts.lock().unwrap(),
		[json!("Fixed the parser.")]
	);
	assert_eq!(
		fixture.run.context.completion.basis,
		Some(CompletionBasis::Verified)
	);
	let started = checks(&fixture).started.lock().unwrap().clone();
	assert_eq!(started.len(), 2);
	assert!(
		started
			.iter()
			.all(|(_, command, revision)| command == "cargo test" && *revision == 7)
	);
	// The repair consumed the delivered failure; only the pass remains.
	let evidence = &fixture.run.context.completion.evidence;
	assert_eq!(evidence.len(), 1);
	assert_eq!(evidence[0].subject.content_digest, "sha256:repaired");
	assert_eq!(evidence[0].outcome, CheckOutcome::Passed);
	// The basis was durable at the commit fence, before the Task completed.
	assert_eq!(
		*fixture.backend.0.fenced.lock().unwrap(),
		[Some(CompletionBasis::Verified)]
	);
}

/// A repair turn that leaves the files unchanged, such as a retry of a flaky
/// Check, runs the Check again instead of reusing the delivered failure.
#[rstest]
#[tokio::test]
async fn a_repair_without_edits_runs_the_check_again(mut fixture: Fixture) {
	gated(
		&mut fixture,
		tests_policy(
			json!({"checks":[{"name":"tests","capability":"shell","command":"cargo test","when":"always"}]}),
		),
		Some(FakeChecks::with(
			"sha256:edited",
			[Outcome::Fail("flaky timeout"), Outcome::Pass],
		)),
	);
	answer(&mut fixture, "Done.");
	settle(&mut fixture).await;
	assert_eq!(repair_notes(&fixture.run).len(), 1);
	answer(&mut fixture, "Done; the failure was flaky.");
	settle(&mut fixture).await;
	assert!(completed(&fixture));
	assert_eq!(checks(&fixture).started(), 2);
	assert_eq!(fixture.run.context.completion.repairs, 1);
}

/// A Check still running past its timeout and grace keeps the Area busy; the
/// gate asks a person instead of waiting for the Area forever.
#[rstest]
#[tokio::test]
async fn an_overdue_check_pauses_for_review_while_it_keeps_the_area_busy(mut fixture: Fixture) {
	let mut fake = FakeChecks::with("sha256:edited", [Outcome::Pass]);
	fake.polls = u32::MAX;
	gated(
		&mut fixture,
		tests_policy(
			json!({"checks":[{"name":"tests","capability":"shell","command":"cargo test","when":"always"}]}),
		),
		Some(fake),
	);
	answer(&mut fixture, "Done.");
	advance_sources(&mut fixture).await.unwrap();
	let pending = fixture.run.context.completion.pending.as_mut().unwrap();
	pending.started_at -= chrono::Duration::seconds(3600);
	checks(&fixture).busy.store(true, Ordering::SeqCst);
	settle(&mut fixture).await;
	assert!(
		matches!(&fixture.run.state, RunState::Waiting(wait) if matches!(**wait, WaitingState::Human { .. }))
	);
	assert!(review_prompt(&fixture).contains("Check tests could not run"));
	let progress = &fixture.run.context.completion;
	assert_eq!(progress.pending, None);
	assert_eq!(progress.evidence[0].outcome, CheckOutcome::Unavailable);
	assert!(!completed(&fixture));
}

/// A repairable failure at the step limit names the step limit, not a spent
/// Repair Allowance that new input could renew.
#[rstest]
#[tokio::test]
async fn a_failure_at_the_step_limit_is_not_reported_as_a_spent_allowance(mut fixture: Fixture) {
	gated(
		&mut fixture,
		tests_policy(
			json!({"checks":[{"name":"tests","capability":"shell","command":"cargo test","when":"always"}]}),
		),
		Some(FakeChecks::with(
			"sha256:edited",
			[Outcome::Fail("assertion failed")],
		)),
	);
	fixture.run.step = 63;
	answer(&mut fixture, "Done.");
	settle(&mut fixture).await;
	let prompt = review_prompt(&fixture);
	assert!(prompt.contains("no step left"), "{prompt}");
	assert!(!prompt.contains("Repair Allowance"));
	assert_eq!(fixture.run.context.completion.repairs, 0);
	assert!(repair_notes(&fixture.run).is_empty());
}

/// Newer input ends a Completion Review without an answer; its request is
/// closed for the Run so it never waits for an answer the Run would ignore.
#[rstest]
#[tokio::test]
async fn new_input_closes_the_superseded_review_request(mut fixture: Fixture) {
	let fake = FakeChecks::with("sha256:edited", []);
	*fake.unavailable.lock().unwrap() = Some("runner is not healthy".into());
	gated(
		&mut fixture,
		tests_policy(
			json!({"checks":[{"name":"tests","capability":"shell","command":"cargo test","when":"always"}]}),
		),
		Some(fake),
	);
	answer(&mut fixture, "Done.");
	settle(&mut fixture).await;
	assert!(fixture.run.context.completion.review.is_some());
	fixture.backend.0.inputs.lock().unwrap().push(RunInput {
		seq: 1,
		sender: "human".into(),
		content: "Skip the runner; just explain the change.".into(),
		idempotency_key: "correction".into(),
		message_id: None,
		reference_only: false,
	});
	advance_sources(&mut fixture).await.unwrap();
	assert_eq!(fixture.run.context.completion.review, None);
	let request = fixture.backend.0.human.lock().unwrap().clone().unwrap();
	assert_eq!(request.response, Some(json!({"superseded": true})));
	assert_eq!(request.answered_by.as_deref(), Some("system"));
	assert!(!completed(&fixture));
}

#[rstest]
#[tokio::test]
async fn evidence_for_earlier_content_never_completes_a_later_edit(mut fixture: Fixture) {
	gated(
		&mut fixture,
		tests_policy(
			json!({"checks":[{"name":"tests","capability":"shell","command":"cargo test","when":"always"}]}),
		),
		Some(FakeChecks::with(
			"sha256:before",
			[Outcome::Pass, Outcome::Pass],
		)),
	);
	Arc::get_mut(&mut fixture.backend.0)
		.unwrap()
		.permit_completion = false;
	answer(&mut fixture, "Done.");
	settle(&mut fixture).await;
	// The commit fence saw a newer input after the Check passed: no completion.
	assert!(!completed(&fixture));
	assert_eq!(checks(&fixture).started(), 1);

	// The files change before the next answer; the passing evidence is stale.
	Arc::get_mut(&mut fixture.backend.0)
		.unwrap()
		.permit_completion = true;
	checks(&fixture).set_content("sha256:after");
	answer(&mut fixture, "Done again.");
	settle(&mut fixture).await;
	assert_eq!(checks(&fixture).started(), 2);
	assert!(completed(&fixture));
}

#[rstest]
#[tokio::test]
async fn unchanged_content_completes_without_running_checks(mut fixture: Fixture) {
	gated(
		&mut fixture,
		tests_policy(json!({})),
		Some(FakeChecks::with("sha256:unchanged", [])),
	);
	fixture.run.state = RunState::Ready(ReadyState::default());
	*fixture.backend.0.task.lock().unwrap() = Task {
		status: TaskStatus::Open,
		..fixture.backend.task_value()
	};
	advance_sources(&mut fixture).await.unwrap();
	assert_eq!(
		fixture.run.context.completion.start_content.as_deref(),
		Some("sha256:unchanged")
	);
	answer(&mut fixture, "Here is the explanation you asked for.");
	settle(&mut fixture).await;
	assert!(completed(&fixture));
	assert_eq!(checks(&fixture).started(), 0);
	assert_eq!(
		fixture.run.context.completion.basis,
		Some(CompletionBasis::NotRequired)
	);
}

#[rstest]
#[tokio::test]
async fn new_input_during_a_check_discards_the_candidate_and_keeps_its_evidence(
	mut fixture: Fixture,
) {
	let mut fake = FakeChecks::with("sha256:edited", [Outcome::Pass]);
	fake.polls = 1;
	gated(
		&mut fixture,
		tests_policy(
			json!({"checks":[{"name":"tests","capability":"shell","command":"cargo test","when":"always"}]}),
		),
		Some(fake),
	);
	answer(&mut fixture, "Done.");
	advance_sources(&mut fixture).await.unwrap();
	assert!(fixture.run.context.completion.pending.is_some());
	assert!(
		matches!(&fixture.run.state, RunState::Waiting(wait) if matches!(**wait, WaitingState::Timer { .. }))
	);

	// A correction arrives while the Check runs.
	fixture.backend.0.inputs.lock().unwrap().push(RunInput {
		seq: 1,
		sender: "human".into(),
		content: "Also update the changelog.".into(),
		idempotency_key: "correction".into(),
		message_id: None,
		reference_only: false,
	});
	settle(&mut fixture).await;
	assert_eq!(fixture.run.phase(), RunPhase::Thinking);
	assert!(!completed(&fixture));
	assert!(fixture.run.context.completion.pending.is_some());

	// The next answer over the same files reuses the finished Check.
	fixture.run.observed_input_seq = 1;
	answer(&mut fixture, "Done, including the correction.");
	settle(&mut fixture).await;
	assert!(completed(&fixture));
	assert_eq!(checks(&fixture).started(), 1);
}

#[rstest]
#[tokio::test]
async fn an_unavailable_check_pauses_for_review_and_a_retry_runs_it_again(mut fixture: Fixture) {
	let fake = FakeChecks::with("sha256:edited", [Outcome::Pass]);
	*fake.unavailable.lock().unwrap() = Some("runner is not healthy".into());
	gated(
		&mut fixture,
		tests_policy(
			json!({"checks":[{"name":"tests","capability":"shell","command":"cargo test","when":"always"}]}),
		),
		Some(fake),
	);
	answer(&mut fixture, "Done.");
	settle(&mut fixture).await;
	assert!(
		matches!(&fixture.run.state, RunState::Waiting(wait) if matches!(**wait, WaitingState::Human { resume: ResumeState::ToolCall(_), .. }))
	);
	assert!(review_prompt(&fixture).contains("Check tests could not run"));
	let evidence = &fixture.run.context.completion.evidence;
	assert_eq!(evidence[0].outcome, CheckOutcome::Unavailable);
	assert_eq!(evidence[0].operation_id, None);
	assert_eq!(fixture.run.context.completion.repairs, 0);

	*checks(&fixture).unavailable.lock().unwrap() = None;
	// The answer form sends plain text unless it parses as JSON.
	answer_review(&fixture, json!("Retry"));
	advance_sources(&mut fixture).await.unwrap();
	settle(&mut fixture).await;
	assert!(completed(&fixture));
	assert_eq!(fixture.run.context.completion.review, None);
}

#[rstest]
#[tokio::test]
async fn failure_loss_and_exhaustion_stay_distinguishable(mut fixture: Fixture) {
	gated(
		&mut fixture,
		tests_policy(
			json!({"repair_allowance":0,"checks":[{"name":"tests","capability":"shell","command":"cargo test","when":"always"}]}),
		),
		Some(FakeChecks::with(
			"sha256:edited",
			[Outcome::Fail("assertion failed"), Outcome::Lost],
		)),
	);
	answer(&mut fixture, "Done.");
	settle(&mut fixture).await;
	assert!(review_prompt(&fixture).contains("Repair Allowance is spent"));
	assert_eq!(
		fixture.run.context.completion.evidence[0].outcome,
		CheckOutcome::Failed
	);
	assert!(repair_notes(&fixture.run).is_empty());

	// A retry runs the Check again; a lost operation is unavailable, not failed.
	answer_review(&fixture, json!({"retry": true}));
	advance_sources(&mut fixture).await.unwrap();
	settle(&mut fixture).await;
	assert!(review_prompt(&fixture).contains("could not run"));
	assert_eq!(
		fixture.run.context.completion.evidence[0].outcome,
		CheckOutcome::Unavailable
	);
	assert!(!completed(&fixture));

	// An answer without a retry asks again rather than completing.
	answer_review(&fixture, json!({}));
	advance_sources(&mut fixture).await.unwrap();
	settle(&mut fixture).await;
	assert!(
		matches!(&fixture.run.state, RunState::Waiting(wait) if matches!(**wait, WaitingState::Human { .. }))
	);
	assert_eq!(fixture.run.context.completion.reviews, 3);
	assert!(!completed(&fixture));
}

#[rstest]
#[tokio::test]
async fn a_node_without_checks_never_completes_a_gated_run(mut fixture: Fixture) {
	// Not even `content_changed` can be decided without reading the content.
	gated(&mut fixture, tests_policy(json!({})), None);
	fixture.run.state = RunState::Ready(ReadyState::default());
	fixture.backend.0.task.lock().unwrap().status = TaskStatus::Open;
	advance_sources(&mut fixture).await.unwrap();
	assert_eq!(fixture.run.context.completion.start_content, None);
	answer(&mut fixture, "Done.");
	settle(&mut fixture).await;
	assert!(!completed(&fixture));
	assert!(review_prompt(&fixture).contains("could not run"));
}

#[rstest]
#[tokio::test]
async fn the_gate_waits_while_another_operation_runs_in_the_area(mut fixture: Fixture) {
	let fake = FakeChecks::with("sha256:edited", [Outcome::Pass]);
	fake.busy.store(true, Ordering::SeqCst);
	gated(
		&mut fixture,
		tests_policy(
			json!({"checks":[{"name":"tests","capability":"shell","command":"cargo test","when":"always"}]}),
		),
		Some(fake),
	);
	answer(&mut fixture, "Done.");
	advance_sources(&mut fixture).await.unwrap();
	assert!(
		matches!(&fixture.run.state, RunState::Waiting(wait) if matches!(**wait, WaitingState::Timer { .. }))
	);
	assert_eq!(checks(&fixture).started(), 0);
	checks(&fixture).busy.store(false, Ordering::SeqCst);
	settle(&mut fixture).await;
	assert!(completed(&fixture));
	assert_eq!(checks(&fixture).started(), 1);
}

#[rstest]
#[tokio::test]
async fn an_agent_without_a_policy_completes_as_before(mut fixture: Fixture) {
	fixture.backend.0.task.lock().unwrap().status = TaskStatus::Running;
	answer(&mut fixture, "Done.");
	advance_sources(&mut fixture).await.unwrap();
	assert!(completed(&fixture));
	assert!(fixture.run.context.completion.is_empty());
}

/// No Check outlasts the Run's remaining verification time, even when its
/// own timeout, or the node default it would otherwise use, is longer.
#[rstest]
#[tokio::test]
async fn checks_never_outlast_the_remaining_verification_time(mut fixture: Fixture) {
	gated(
		&mut fixture,
		tests_policy(json!({"verification_seconds": 45})),
		Some(FakeChecks::with("sha256:edited", [Outcome::Pass])),
	);
	fixture.run.context.completion.start_content = Some("sha256:start".into());
	fixture.run.context.completion.seconds = 30;
	answer(&mut fixture, "Done.");
	settle(&mut fixture).await;
	assert_eq!(*checks(&fixture).timeouts.lock().unwrap(), [(None, 15)]);
}
