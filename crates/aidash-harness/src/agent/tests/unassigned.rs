//! A final answer given while the only open children are Unassigned Tasks
//! (#772): the model is told once, then those its Run created are abandoned as
//! the parent completes, and a person is asked about the rest.
use super::*;
use aidash_domain::unassigned::{self, UnassignedTask};

fn task(n: u128, abandonable: bool, handed_over: bool) -> UnassignedTask {
	UnassignedTask {
		id: Uuid::from_u128(n),
		title: format!("Task {n}"),
		abandonable,
		handed_over,
	}
}

fn children(open: u64, failed: bool, unassigned: Vec<UnassignedTask>) -> ChildTaskSummary {
	ChildTaskSummary {
		has_pending: open > 0,
		has_failed: failed,
		open_tasks: open,
		unassigned,
		terminal_fingerprint: [0; 16],
	}
}

fn set_children(fixture: &Fixture, summary: ChildTaskSummary) {
	*fixture.backend.0.child_summary.lock().unwrap() = Some(summary);
}

fn answer(run: &mut Run, text: &str) {
	run.state = RunState::ToolCall(Box::new(ToolCallState {
		response: ModelResponse {
			text: text.into(),
			usage_complete: true,
			..Default::default()
		},
		..Default::default()
	}));
}

fn use_native_projection(fixture: &mut Fixture) {
	Arc::get_mut(&mut fixture.backend.0).unwrap().projection = ProjectionVersion::Native;
}

fn calls(fixture: &Fixture, name: &str) -> usize {
	fixture
		.backend
		.0
		.calls
		.lock()
		.unwrap()
		.iter()
		.filter(|&&call| call == name)
		.count()
}

fn told(fixture: &Fixture) -> Vec<Uuid> {
	fixture
		.run
		.context
		.events()
		.filter_map(|event| match event {
			ContextEvent::UnassignedTasksTold { tasks, .. } => Some(tasks.iter().map(|t| t.id)),
			_ => None,
		})
		.flatten()
		.collect()
}

#[rstest]
#[tokio::test]
async fn a_final_answer_beside_an_unassigned_task_is_held_and_the_model_is_told(
	mut fixture: Fixture,
) {
	set_children(&fixture, children(1, false, vec![task(1, true, false)]));
	answer(&mut fixture.run, "All done.");
	let step = fixture.run.step;
	advance_sources(&mut fixture).await.unwrap();
	let (event, saved) = last_write(&fixture);
	assert_eq!(event, "run.unassigned_tasks_told");
	assert_eq!(saved.phase(), RunPhase::Thinking);
	// The notice uses one step, publishes nothing and completes nothing.
	assert_eq!(saved.step, step + 1);
	assert_eq!(calls(&fixture, "home.output"), 0);
	assert_eq!(calls(&fixture, "home.complete"), 0);
	assert!(fixture.backend.0.published.lock().unwrap().is_empty());
	assert!(matches!(
		saved.context.events().last(),
		Some(ContextEvent::UnassignedTasksTold { tasks, instruction })
			if tasks.len() == 1
				&& tasks[0].id == Uuid::from_u128(1)
				&& instruction == unassigned::TOLD_INSTRUCTION
	));
}

async fn the_next_request_carries_the_notice(mut fixture: Fixture) {
	set_children(&fixture, children(1, false, vec![task(1, true, false)]));
	answer(&mut fixture.run, "All done.");
	advance_sources(&mut fixture).await.unwrap();
	fixture.run = last_write(&fixture).1;
	let before = fixture.backend.0.requests.lock().unwrap().len();
	// The Run thinks again; the scripted model's reply does not matter here.
	let _ = advance_sources(&mut fixture).await;
	let requests = fixture.backend.0.requests.lock().unwrap();
	assert_eq!(requests.len(), before + 1);
	let body = requests.last().unwrap().input_body().to_string();
	assert!(body.contains("Task 1"), "{body}");
	assert!(body.contains("task_delegate"), "{body}");
	assert!(body.contains("abandoned"), "{body}");
}

#[rstest]
#[tokio::test]
async fn the_next_request_carries_the_notice_on_the_legacy_projection(fixture: Fixture) {
	the_next_request_carries_the_notice(fixture).await;
}

#[rstest]
#[tokio::test]
async fn the_next_request_carries_the_notice_on_the_native_projection(mut fixture: Fixture) {
	use_native_projection(&mut fixture);
	the_next_request_carries_the_notice(fixture).await;
}

#[rstest]
#[tokio::test]
async fn the_next_final_answer_abandons_what_the_run_created_and_says_so(mut fixture: Fixture) {
	set_children(&fixture, children(1, false, vec![task(1, true, false)]));
	answer(&mut fixture.run, "All done.");
	advance_sources(&mut fixture).await.unwrap();
	fixture.run = last_write(&fixture).1;
	answer(&mut fixture.run, "All done, twice.");
	advance_sources(&mut fixture).await.unwrap();
	assert_eq!(fixture.run.phase(), RunPhase::Completed);
	assert_eq!(
		*fixture.backend.0.completed_abandoning.lock().unwrap(),
		[vec![Uuid::from_u128(1)]]
	);
	let expected = unassigned::answer_with_abandoned("All done, twice.", &[task(1, true, false)]);
	assert!(expected.contains("Task 1"));
	// The artifact and the published message carry the sentence.
	assert_eq!(
		*fixture.backend.0.artifacts.lock().unwrap(),
		[json!(expected)]
	);
	assert_eq!(*fixture.backend.0.published.lock().unwrap(), [expected]);
}

#[rstest]
#[tokio::test]
async fn the_model_turn_keeps_the_models_own_words(mut fixture: Fixture) {
	use_native_projection(&mut fixture);
	set_children(&fixture, children(1, false, vec![task(1, true, false)]));
	answer(&mut fixture.run, "All done.");
	advance_sources(&mut fixture).await.unwrap();
	fixture.run = last_write(&fixture).1;
	answer(&mut fixture.run, "All done, twice.");
	advance_sources(&mut fixture).await.unwrap();
	let turn = fixture
		.run
		.context
		.events()
		.filter_map(|event| match event {
			ContextEvent::Assistant { text, .. } => Some(text.clone()),
			_ => None,
		})
		.next_back()
		.unwrap();
	assert_eq!(turn, "All done, twice.");
}

#[rstest]
#[tokio::test]
async fn delegating_after_the_notice_abandons_nothing_and_the_parent_waits(mut fixture: Fixture) {
	set_children(&fixture, children(1, false, vec![task(1, true, false)]));
	answer(&mut fixture.run, "All done.");
	advance_sources(&mut fixture).await.unwrap();
	fixture.run = last_write(&fixture).1;
	// The model handed it over: it is running now.
	set_children(&fixture, children(1, false, vec![]));
	answer(&mut fixture.run, "Waiting for it.");
	advance_sources(&mut fixture).await.unwrap();
	let (event, saved) = last_write(&fixture);
	assert_eq!(event, "run.waiting");
	assert!(matches!(
		saved.state,
		RunState::Waiting(ref waiting) if matches!(**waiting, WaitingState::Workspace { .. })
	));
	assert_eq!(calls(&fixture, "home.complete"), 0);
	assert!(fixture.backend.0.abandoned.lock().unwrap().is_empty());
}

#[rstest]
#[tokio::test]
async fn a_task_created_after_the_notice_gets_its_own_notice(mut fixture: Fixture) {
	set_children(&fixture, children(1, false, vec![task(1, true, false)]));
	answer(&mut fixture.run, "All done.");
	advance_sources(&mut fixture).await.unwrap();
	fixture.run = last_write(&fixture).1;
	set_children(
		&fixture,
		children(2, false, vec![task(1, true, false), task(2, true, false)]),
	);
	answer(&mut fixture.run, "All done again.");
	advance_sources(&mut fixture).await.unwrap();
	let (event, _) = last_write(&fixture);
	assert_eq!(event, "run.unassigned_tasks_told");
	assert_eq!(told(&fixture), [Uuid::from_u128(1), Uuid::from_u128(2)]);
	assert_eq!(calls(&fixture, "home.complete"), 0);
}

#[rstest]
#[tokio::test]
async fn with_no_step_left_there_is_no_notice_and_the_run_still_completes(mut fixture: Fixture) {
	set_children(&fixture, children(1, false, vec![task(1, true, false)]));
	// The fixture Agent allows 64 steps.
	fixture.run.step = 63;
	answer(&mut fixture.run, "All done.");
	advance_sources(&mut fixture).await.unwrap();
	assert_eq!(fixture.run.phase(), RunPhase::Completed);
	assert!(told(&fixture).is_empty());
	assert_eq!(
		*fixture.backend.0.completed_abandoning.lock().unwrap(),
		[vec![Uuid::from_u128(1)]]
	);
}

#[rstest]
#[tokio::test]
async fn asking_a_person_at_the_step_limit_leaves_room_to_resume(mut fixture: Fixture) {
	set_children(&fixture, children(1, false, vec![task(1, false, false)]));
	fixture.run.step = 63;
	answer(&mut fixture.run, "All done.");
	advance_sources(&mut fixture).await.unwrap();
	let (_, saved) = last_write(&fixture);
	assert!(matches!(saved.state, RunState::Waiting(_)));
	fixture.run = saved;
	fixture
		.backend
		.0
		.human
		.lock()
		.unwrap()
		.as_mut()
		.unwrap()
		.response = Some(json!({"answer": "Done"}));
	advance_sources(&mut fixture).await.unwrap();
	assert_eq!(fixture.run.phase(), RunPhase::Thinking);
	set_children(&fixture, children(0, false, vec![]));
	// The answer allows one final model turn and completion.
	advance_sources(&mut fixture).await.unwrap();
	advance_sources(&mut fixture).await.unwrap();
	assert_eq!(fixture.run.phase(), RunPhase::Completed);
}

#[rstest]
#[tokio::test]
async fn a_running_child_beside_an_unassigned_task_means_waiting_without_a_notice(
	mut fixture: Fixture,
) {
	set_children(&fixture, children(2, false, vec![task(1, true, false)]));
	answer(&mut fixture.run, "Partial.");
	advance_sources(&mut fixture).await.unwrap();
	let (event, _) = last_write(&fixture);
	assert_eq!(event, "run.waiting");
	assert!(told(&fixture).is_empty());
	// A text answer given while children are running is published as before.
	assert_eq!(*fixture.backend.0.published.lock().unwrap(), ["Partial."]);
}

#[rstest]
#[tokio::test]
async fn a_failed_child_beside_an_unassigned_task_goes_to_a_person_without_a_notice(
	mut fixture: Fixture,
) {
	set_children(&fixture, children(2, true, vec![task(1, true, false)]));
	answer(&mut fixture.run, "Partial.");
	advance_sources(&mut fixture).await.unwrap();
	assert!(told(&fixture).is_empty());
	assert!(matches!(
		fixture.run.state,
		RunState::Waiting(ref waiting) if matches!(**waiting, WaitingState::Human { answer_by: None, .. })
	));
}

#[rstest]
#[tokio::test]
async fn a_task_made_elsewhere_raises_a_request_without_a_deadline_and_abandons_nothing(
	mut fixture: Fixture,
) {
	let people = task(2, false, false);
	set_children(&fixture, children(1, false, vec![people.clone()]));
	answer(&mut fixture.run, "All done.");
	advance_sources(&mut fixture).await.unwrap();
	fixture.run = last_write(&fixture).1;
	answer(&mut fixture.run, "All done, twice.");
	advance_sources(&mut fixture).await.unwrap();
	assert!(matches!(
		fixture.run.state,
		RunState::Waiting(ref waiting) if matches!(**waiting, WaitingState::Human { answer_by: None, .. })
	));
	assert_eq!(calls(&fixture, "home.complete"), 0);
	let prompt = fixture
		.backend
		.0
		.human
		.lock()
		.unwrap()
		.clone()
		.unwrap()
		.prompt;
	assert!(prompt.contains("Task 2"));
	assert_eq!(
		fixture.backend.0.human_key.lock().unwrap().as_deref(),
		Some(format!("{}:{}:unassigned", fixture.run.id, 0).as_str())
	);
}

#[rstest]
#[tokio::test]
async fn a_handed_over_child_makes_the_parent_abandon_the_root_and_wait(mut fixture: Fixture) {
	set_children(
		&fixture,
		children(2, false, vec![task(1, true, false), task(2, true, true)]),
	);
	answer(&mut fixture.run, "All done.");
	advance_sources(&mut fixture).await.unwrap();
	fixture.run = last_write(&fixture).1;
	answer(&mut fixture.run, "All done, twice.");
	advance_sources(&mut fixture).await.unwrap();
	assert_eq!(
		*fixture.backend.0.abandoned.lock().unwrap(),
		[vec![Uuid::from_u128(1)]]
	);
	assert_eq!(calls(&fixture, "home.complete"), 0);
	assert!(matches!(
		fixture.run.state,
		RunState::Waiting(ref waiting) if matches!(**waiting, WaitingState::Workspace { .. })
	));
}

#[rstest]
#[tokio::test]
async fn early_abandonment_is_named_in_the_final_answer(mut fixture: Fixture) {
	set_children(
		&fixture,
		children(2, false, vec![task(1, true, false), task(2, true, true)]),
	);
	answer(&mut fixture.run, "First answer.");
	advance_sources(&mut fixture).await.unwrap();
	fixture.run = last_write(&fixture).1;
	answer(&mut fixture.run, "Second answer.");
	advance_sources(&mut fixture).await.unwrap();
	fixture.run = last_write(&fixture).1;
	// Simulate a worker restart using the persisted state and recovery payload.
	let pending =
		aidash_domain::run_state::encode(&fixture.run.state, &fixture.run.recovery).unwrap();
	let (state, recovery) = aidash_domain::run_state::decode(fixture.run.phase(), pending).unwrap();
	fixture.run.state = state;
	fixture.run.recovery = recovery;
	set_children(&fixture, children(0, false, vec![]));
	answer(&mut fixture.run, "Finished.");
	advance_sources(&mut fixture).await.unwrap();
	let artifact = fixture.backend.0.artifacts.lock().unwrap();
	let text = artifact.last().unwrap().as_str().unwrap();
	assert!(text.contains("Task 1"), "{text}");
	assert_eq!(
		fixture.backend.0.published.lock().unwrap().last().unwrap(),
		text
	);
	assert_eq!(
		*fixture.backend.0.completed_abandoning.lock().unwrap(),
		[Vec::<Uuid>::new()]
	);
}

#[rstest]
#[tokio::test]
async fn a_task_handed_over_before_abandonment_is_not_named_as_abandoned(mut fixture: Fixture) {
	set_children(
		&fixture,
		children(
			3,
			false,
			vec![
				task(1, true, false),
				task(2, true, false),
				task(3, true, true),
			],
		),
	);
	answer(&mut fixture.run, "First answer.");
	advance_sources(&mut fixture).await.unwrap();
	fixture.run = last_write(&fixture).1;
	// The Home observes Task 2 handed over after the summary was read.
	fixture
		.backend
		.0
		.abandon_skipped
		.lock()
		.unwrap()
		.push(Uuid::from_u128(2));
	answer(&mut fixture.run, "Second answer.");
	advance_sources(&mut fixture).await.unwrap();
	fixture.run = last_write(&fixture).1;
	set_children(&fixture, children(0, false, vec![]));
	answer(&mut fixture.run, "Finished.");
	advance_sources(&mut fixture).await.unwrap();
	let artifacts = fixture.backend.0.artifacts.lock().unwrap();
	let text = artifacts.last().unwrap().as_str().unwrap();
	assert!(text.contains("Task 1"), "{text}");
	assert!(!text.contains("Task 2"), "{text}");
}

#[rstest]
#[tokio::test]
async fn a_conflict_from_complete_reaches_the_same_decision(mut fixture: Fixture) {
	// The check saw nothing pending; a child was created before the commit.
	answer(&mut fixture.run, "All done.");
	*fixture.backend.0.complete_error.lock().unwrap() =
		Some(Error::Conflict("task has unresolved children".into()));
	// Read once at the check, then again after the conflict.
	fixture.backend.0.child_summaries.lock().unwrap().extend([
		children(0, false, vec![]),
		children(1, false, vec![task(7, true, false)]),
	]);
	advance_sources(&mut fixture).await.unwrap();
	let (event, saved) = last_write(&fixture);
	assert_eq!(event, "run.unassigned_tasks_told");
	assert_eq!(saved.phase(), RunPhase::Thinking);
}

#[rstest]
#[tokio::test]
async fn a_completion_gate_send_back_abandons_nothing(mut fixture: Fixture) {
	set_children(&fixture, children(1, false, vec![task(1, true, false)]));
	answer(&mut fixture.run, "All done.");
	advance_sources(&mut fixture).await.unwrap();
	fixture.run = last_write(&fixture).1;
	// A citation correction stands in for any send-back before `complete`.
	answer(&mut fixture.run, "Claim [[cite:ev_invented]].");
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.known_evidence = vec![];
	state.custom.insert(
		"lookup".into(),
		Scripted::research(aidash_domain::research::Operation::Find),
	);
	advance_sources(&mut fixture).await.unwrap();
	assert_eq!(last_write(&fixture).0, "run.citation_correction_required");
	assert_eq!(calls(&fixture, "home.complete"), 0);
	assert!(fixture.backend.0.abandoned.lock().unwrap().is_empty());
}

#[rstest]
#[case::a_task_of_its_own(true, 3600)]
#[case::only_other_childrens_tasks(false, 300)]
#[tokio::test]
async fn the_ceiling_of_the_internal_wait_rechecks_children_without_a_model_call(
	mut fixture: Fixture,
	#[case] owned: bool,
	#[case] seconds: i64,
) {
	// The next look comes as late as #644's ceiling allows for what can wake it.
	Arc::get_mut(&mut fixture.backend.0)
		.unwrap()
		.owned_tasks_pending
		.store(owned, Ordering::Release);
	set_children(&fixture, children(1, false, vec![]));
	let past = chrono::Utc::now() - chrono::Duration::minutes(1);
	fixture.run.state = RunState::Waiting(Box::new(WaitingState::Workspace {
		wake_at: past,
		cursor: 0,
		resume: ResumeState::Thinking(ThinkingState::default()),
	}));
	let step = fixture.run.step;
	let inferred = calls(&fixture, "provider.infer");
	advance_sources(&mut fixture).await.unwrap();
	let (event, saved) = last_write(&fixture);
	assert_eq!(event, "run.waiting_for_workspace");
	assert_eq!(saved.step, step);
	assert_eq!(calls(&fixture, "provider.infer"), inferred);
	let RunState::Waiting(waiting) = &saved.state else {
		panic!("expected the Run to keep waiting");
	};
	let WaitingState::Workspace { wake_at, .. } = **waiting else {
		panic!("expected a workspace wait");
	};
	let before = chrono::Utc::now();
	assert!(wake_at > before + chrono::Duration::seconds(seconds - 60));
	assert!(wake_at <= before + chrono::Duration::seconds(seconds));
}

#[rstest]
#[tokio::test]
async fn the_ceiling_wakes_the_run_once_the_decision_is_no_longer_to_wait(mut fixture: Fixture) {
	// Every pending child is unassigned now: the answer must be reconsidered.
	set_children(&fixture, children(1, false, vec![task(1, true, false)]));
	let past = chrono::Utc::now() - chrono::Duration::minutes(1);
	fixture.run.state = RunState::Waiting(Box::new(WaitingState::Workspace {
		wake_at: past,
		cursor: 0,
		resume: ResumeState::Thinking(ThinkingState::default()),
	}));
	advance_sources(&mut fixture).await.unwrap();
	let (event, saved) = last_write(&fixture);
	assert_eq!(event, "run.resumed");
	assert_eq!(saved.phase(), RunPhase::Thinking);
}

#[rstest]
#[tokio::test]
async fn abandoning_needs_task_abandon_for_each_task_and_a_refusal_changes_nothing(
	mut fixture: Fixture,
) {
	set_children(&fixture, children(1, false, vec![task(1, true, false)]));
	answer(&mut fixture.run, "All done.");
	advance_sources(&mut fixture).await.unwrap();
	fixture.run = last_write(&fixture).1;
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.scoped = true;
	state.denied_action = Some("task.abandon");
	answer(&mut fixture.run, "All done, twice.");
	let refused = advance_sources(&mut fixture).await;
	assert!(matches!(refused, Err(Error::Forbidden)));
	assert!(
		fixture
			.backend
			.0
			.actions
			.lock()
			.unwrap()
			.iter()
			.any(|action| action == "task.abandon")
	);
	assert_eq!(calls(&fixture, "home.complete"), 0);
	assert!(fixture.backend.0.published.lock().unwrap().is_empty());
}

#[rstest]
#[tokio::test]
async fn a_handed_over_dependent_also_needs_task_abandon_before_the_home_is_called(
	mut fixture: Fixture,
) {
	set_children(
		&fixture,
		children(2, false, vec![task(1, true, false), task(2, true, true)]),
	);
	answer(&mut fixture.run, "All done.");
	advance_sources(&mut fixture).await.unwrap();
	fixture.run = last_write(&fixture).1;
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.scoped = true;
	state.denied_action = Some("task.abandon");
	answer(&mut fixture.run, "All done, twice.");
	assert!(advance_sources(&mut fixture).await.is_err());
	assert!(fixture.backend.0.abandoned.lock().unwrap().is_empty());
}

#[rstest]
#[case::only_unassigned(1, 1, 300)]
#[case::something_still_running(2, 1, 3600)]
#[tokio::test]
async fn the_default_wait_on_a_remote_home_ignores_unassigned_tasks(
	mut fixture: Fixture,
	#[case] open: u64,
	#[case] unassigned_count: u64,
	#[case] seconds: i64,
) {
	let wait = Scripted::new(
		ToolSpec {
			name: "workspace_wait".into(),
			description: "Wait".into(),
			parameters: json!({"type":"object"}),
		},
		"workspace_wait",
	);
	*wait.outputs.lock().unwrap() = [json!({"wait_seconds":300})].into();
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.remote_home = true;
	state.custom.insert("workspace_wait".into(), wait);
	*state.child_summary.lock().unwrap() = Some(children(
		open,
		false,
		(1..=u128::from(unassigned_count))
			.map(|n| task(n, true, false))
			.collect(),
	));
	respond(
		&mut fixture.run,
		vec![call("call-1", "workspace_wait", json!({}))],
	);
	let before = chrono::Utc::now();
	advance_sources(&mut fixture).await.unwrap();
	let RunState::Waiting(waiting) = &fixture.run.state else {
		panic!("Run did not wait")
	};
	let WaitingState::Workspace { wake_at, .. } = waiting.as_ref() else {
		panic!("wrong wait")
	};
	assert!(*wake_at >= before + chrono::Duration::seconds(seconds));
	assert!(*wake_at <= chrono::Utc::now() + chrono::Duration::seconds(seconds));
}

async fn wake_result(fixture: &mut Fixture, summary: ChildTaskSummary) -> Value {
	// A remote Home keeps the children's fingerprint, so one read serves the wake.
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.remote_home = true;
	*state.child_summary.lock().unwrap() = Some(summary);
	fixture.run.state = RunState::Waiting(Box::new(WaitingState::Workspace {
		wake_at: chrono::Utc::now() - chrono::Duration::seconds(1),
		cursor: 0,
		resume: ResumeState::ToolCall(Box::default()),
	}));
	advance_sources(fixture).await.unwrap();
	let RunState::ToolCall(state) = &fixture.run.state else {
		panic!("the wait did not return to its call")
	};
	state.wait_result.clone().expect("a wake result")
}

#[rstest]
#[tokio::test]
async fn the_wake_result_lists_unassigned_tasks_when_there_are_any(mut fixture: Fixture) {
	let result = wake_result(&mut fixture, children(2, false, vec![task(1, true, false)])).await;
	assert_eq!(result["reason"], "ceiling");
	// One Task is running and one is Unassigned: only the first is open work,
	// as the local count has it.
	assert_eq!(result["open_tasks"], 1);
	assert_eq!(
		result["unassigned_tasks"],
		json!([{"id": Uuid::from_u128(1), "title": "Task 1"}])
	);
}

#[rstest]
#[tokio::test]
async fn the_wake_result_leaves_the_field_out_when_nothing_is_unassigned(mut fixture: Fixture) {
	let result = wake_result(&mut fixture, children(1, false, vec![])).await;
	assert!(result.get("unassigned_tasks").is_none());
}

#[rstest]
#[tokio::test]
async fn the_wake_result_names_at_most_32_unassigned_tasks(mut fixture: Fixture) {
	let many: Vec<_> = (1..=40).map(|n| task(n, true, false)).collect();
	let result = wake_result(&mut fixture, children(40, false, many)).await;
	assert_eq!(result["unassigned_tasks"].as_array().unwrap().len(), 32);
}

#[rstest]
#[tokio::test]
async fn a_run_on_another_node_leaves_the_task_abandon_check_to_its_home(mut fixture: Fixture) {
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.scoped = true;
	state.remote_home = true;
	state.denied_action = Some("task.abandon");
	set_children(&fixture, children(1, false, vec![task(1, true, false)]));
	answer(&mut fixture.run, "All done.");
	advance_sources(&mut fixture).await.unwrap();
	fixture.run = last_write(&fixture).1;
	answer(&mut fixture.run, "All done, twice.");
	advance_sources(&mut fixture).await.unwrap();
	assert!(
		!fixture
			.backend
			.0
			.actions
			.lock()
			.unwrap()
			.iter()
			.any(|action| action == "task.abandon")
	);
	assert_eq!(
		*fixture.backend.0.completed_abandoning.lock().unwrap(),
		[vec![Uuid::from_u128(1)]]
	);
}
