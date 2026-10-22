//! A model's question waits for its person up to the person's Answer Deadline,
//! then the system answers it as unanswered and the Run continues (#413).
use super::approvals::human_request_tool;
use super::*;
use aidash_domain::approval_rules::ApprovalMode;
use chrono::{DateTime, Duration, Utc};

const UNANSWERED: &str = r#"{"answered":false,"reason":"answer_deadline"}"#;

/// The Run's model asks a question, which its person's Home journals as `request`.
fn asks(fixture: &mut Fixture, deadline: Option<i64>) -> Uuid {
	let tool = human_request_tool(fixture, Some(ApprovalMode::Manual));
	let request = Uuid::from_u128(0x413);
	*tool.outputs.lock().unwrap() = [json!({"human_request_id": request})].into();
	Arc::get_mut(&mut fixture.backend.0)
		.unwrap()
		.answer_deadline = deadline;
	respond(
		&mut fixture.run,
		vec![call(
			"call-0",
			"human_request",
			json!({"kind":"QUESTION","prompt":"Which one?"}),
		)],
	);
	request
}

/// The question `request` of `kind`, unanswered, as the Run's Home holds it.
fn journal(fixture: &Fixture, request: Uuid, kind: &str, response: Option<Value>) {
	*fixture.backend.0.human.lock().unwrap() = Some(HumanRequest {
		id: request,
		workspace_id: fixture.run.workspace_id,
		run_id: fixture.run.id,
		kind: kind.into(),
		prompt: "Which one?".into(),
		answered_by: response.as_ref().map(|_| "alice".into()),
		response,
		created_at: fixture.run.updated_at,
		answer_by: None,
	});
}

/// The Run waits on `request`, to continue without an answer at `answer_by`.
fn waits(fixture: &mut Fixture, request: Uuid, answer_by: Option<DateTime<Utc>>) {
	fixture.run.state = RunState::Waiting(Box::new(WaitingState::Human {
		request_id: request,
		answer_by,
		resume: ResumeState::Thinking(ThinkingState::default()),
	}));
}

fn answer_by(run: &Run) -> Option<DateTime<Utc>> {
	let RunState::Waiting(waiting) = &run.state else {
		panic!("the Run waits, not {:?}", run.phase());
	};
	let WaitingState::Human { answer_by, .. } = waiting.as_ref() else {
		panic!("the Run waits for a human, not {waiting:?}");
	};
	*answer_by
}

/// What the model reads of the question's answer once the Run continues.
fn heard(run: &Run) -> Value {
	let Some(ContextEvent::Human { response, .. }) = run.context.events().last() else {
		panic!("the answer is in the Context");
	};
	response.clone()
}

fn closed(fixture: &Fixture) -> usize {
	fixture
		.backend
		.calls()
		.iter()
		.filter(|call| **call == "home.human_close")
		.count()
}

#[rstest]
#[tokio::test]
async fn a_question_of_a_person_with_a_deadline_wakes_the_run_that_many_seconds_later(
	mut fixture: Fixture,
) {
	// Arrange
	asks(&mut fixture, Some(120));
	let before = Utc::now();

	// Act
	advance_sources(&mut fixture).await.unwrap();

	// Assert
	let by = answer_by(&fixture.run).expect("the question has an Answer Deadline");
	assert!(by >= before + Duration::seconds(120));
	assert!(by <= Utc::now() + Duration::seconds(120));
	let RunState::Waiting(waiting) = &fixture.run.state else {
		unreachable!("checked by answer_by");
	};
	assert_eq!(waiting.deadline(), Some(by));
	assert_eq!(last_write(&fixture).0, "run.tool_recorded");
}

#[rstest]
#[tokio::test]
async fn a_person_without_a_deadline_is_waited_for(mut fixture: Fixture) {
	// Arrange
	asks(&mut fixture, None);

	// Act
	advance_sources(&mut fixture).await.unwrap();

	// Assert
	assert_eq!(answer_by(&fixture.run), None);
	assert!(
		fixture
			.backend
			.calls()
			.contains(&"authority.answer_deadline")
	);
}

#[rstest]
#[tokio::test]
async fn another_nodes_person_has_no_deadline_here(mut fixture: Fixture) {
	// Arrange
	asks(&mut fixture, Some(120));
	Arc::get_mut(&mut fixture.backend.0).unwrap().remote_home = true;

	// Act
	advance_sources(&mut fixture).await.unwrap();

	// Assert
	assert_eq!(answer_by(&fixture.run), None);
	assert!(
		!fixture
			.backend
			.calls()
			.contains(&"authority.answer_deadline")
	);
}

#[rstest]
#[tokio::test]
async fn a_failed_subtask_intervention_waits_for_its_answer_whatever_the_deadline(
	mut fixture: Fixture,
) {
	// Arrange
	let state = Arc::get_mut(&mut fixture.backend.0).unwrap();
	state.failed_children = true;
	state.answer_deadline = Some(60);
	state.scoped = true;
	state.permit_tools = true;
	fixture.run.state = RunState::ToolCall(Box::new(ToolCallState {
		response: ModelResponse {
			text: "Done.".into(),
			usage_complete: true,
			..Default::default()
		},
		..Default::default()
	}));

	// Act
	advance_sources(&mut fixture).await.unwrap();

	// Assert
	assert_eq!(answer_by(&fixture.run), None);
	assert!(fixture.backend.calls().contains(&"home.human_request"));
	assert!(
		!fixture
			.backend
			.calls()
			.contains(&"authority.answer_deadline")
	);
}

#[rstest]
#[tokio::test]
async fn the_system_answers_a_question_nobody_answered_by_the_deadline(mut fixture: Fixture) {
	// Arrange
	let request = Uuid::from_u128(0x413);
	journal(&fixture, request, "QUESTION", None);
	waits(
		&mut fixture,
		request,
		Some(Utc::now() - Duration::seconds(1)),
	);

	// Act
	advance_sources(&mut fixture).await.unwrap();

	// Assert
	let stored = fixture.backend.0.human.lock().unwrap().clone().unwrap();
	assert_eq!(
		stored.response,
		Some(serde_json::from_str(UNANSWERED).unwrap())
	);
	assert_eq!(stored.answered_by.as_deref(), Some("system"));
	assert_eq!(fixture.run.phase(), RunPhase::Thinking);
	assert_eq!(last_write(&fixture).0, "run.resumed");
	let heard = heard(&fixture.run);
	assert_eq!(heard["answered"], false);
	assert_eq!(heard["reason"], "answer_deadline");
	assert!(
		heard["message"]
			.as_str()
			.is_some_and(|message| message.contains("continue on your own judgment")),
		"the model is told to go on: {heard}"
	);
}

#[rstest]
#[tokio::test]
async fn an_answer_that_came_before_the_deadline_wins(mut fixture: Fixture) {
	// Arrange
	let request = Uuid::from_u128(0x413);
	let answer = json!({"answer":"the second"});
	journal(&fixture, request, "QUESTION", Some(answer.clone()));
	waits(
		&mut fixture,
		request,
		Some(Utc::now() - Duration::seconds(1)),
	);

	// Act
	advance_sources(&mut fixture).await.unwrap();

	// Assert
	assert_eq!(closed(&fixture), 0);
	assert_eq!(heard(&fixture.run), answer);
	assert_eq!(fixture.run.phase(), RunPhase::Thinking);
}

#[rstest]
#[tokio::test]
async fn an_answer_that_lands_as_the_system_closes_the_question_wins(mut fixture: Fixture) {
	// Arrange
	let request = Uuid::from_u128(0x413);
	let answer = json!({"answer":"the second"});
	journal(&fixture, request, "QUESTION", None);
	waits(
		&mut fixture,
		request,
		Some(Utc::now() - Duration::seconds(1)),
	);
	*fixture.backend.0.answered_just_before_close.lock().unwrap() = Some(answer.clone());

	// Act
	advance_sources(&mut fixture).await.unwrap();

	// Assert
	assert_eq!(closed(&fixture), 1);
	assert_eq!(heard(&fixture.run), answer);
	let stored = fixture.backend.0.human.lock().unwrap().clone().unwrap();
	assert_eq!(stored.answered_by.as_deref(), Some("alice"));
}

#[rstest]
#[tokio::test]
async fn the_system_answer_given_before_a_crash_is_what_the_model_reads_after_it(
	mut fixture: Fixture,
) {
	// Arrange: the worker closed the question, then died before it saved the Run.
	let request = Uuid::from_u128(0x413);
	journal(
		&fixture,
		request,
		"QUESTION",
		Some(serde_json::from_str(UNANSWERED).unwrap()),
	);
	waits(
		&mut fixture,
		request,
		Some(Utc::now() - Duration::seconds(1)),
	);

	// Act
	advance_sources(&mut fixture).await.unwrap();

	// Assert
	assert_eq!(closed(&fixture), 0, "it is not answered twice");
	let heard = heard(&fixture.run);
	assert_eq!(
		(&heard["answered"], &heard["reason"]),
		(&json!(false), &json!("answer_deadline"))
	);
	assert!(heard["message"].is_string());
}

#[rstest]
#[tokio::test]
async fn a_question_stays_open_until_its_deadline(mut fixture: Fixture) {
	// Arrange: the scheduler's clock is ahead of this worker's.
	let request = Uuid::from_u128(0x413);
	let by = Utc::now() + Duration::seconds(30);
	journal(&fixture, request, "QUESTION", None);
	waits(&mut fixture, request, Some(by));

	// Act
	advance_sources(&mut fixture).await.unwrap();

	// Assert
	assert_eq!(closed(&fixture), 0);
	assert_eq!(answer_by(&fixture.run), Some(by));
	assert_eq!(last_write(&fixture).0, "run.waiting");
}
