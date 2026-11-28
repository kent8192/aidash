use super::*;

fn goal_fixture() -> Fixture {
	let mut fixture = fixture();
	{
		let mut task = fixture.backend.0.task.lock().unwrap();
		task.status = TaskStatus::Running;
		task.goal = Some("Verify that the report is readable".into());
	}
	fixture.run.state = RunState::Thinking(ThinkingState::default());
	fixture
}

#[tokio::test]
async fn goal_must_be_judged_before_a_task_completes() {
	let mut fixture = goal_fixture();
	fixture.backend.0.goal_replies.lock().unwrap().extend([
		r#"{"verdict":"not_yet","reason":"Read the report"}"#.into(),
		r#"{"verdict":"met","reason":"Transcript shows a successful read"}"#.into(),
	]);

	for _ in 0..4 {
		advance_sources(&mut fixture).await.unwrap();
	}

	assert_eq!(fixture.run.phase(), RunPhase::Completed);
	let verdicts = fixture.backend.0.goal_verdicts.lock().unwrap();
	assert_eq!(
		verdicts
			.iter()
			.map(|record| record.judgment.verdict)
			.collect::<Vec<_>>(),
		[
			aidash_domain::goal::Verdict::NotYet,
			aidash_domain::goal::Verdict::Met
		],
	);
	assert!(
		verdicts
			.iter()
			.all(|record| record.cost_nanocredits == Some(123) && record.model.id == "model")
	);
	let requests = fixture.backend.0.requests.lock().unwrap();
	assert_eq!(requests.len(), 4);
	assert!(requests[1].tools.is_empty() && requests[1].provider_tools.is_empty());
	let judge = serde_json::to_string(&requests[1].context).unwrap();
	assert!(
		judge.contains("Verify that the report is readable") && judge.contains("Completed task")
	);
	assert!(
		serde_json::to_string(&requests[2].context)
			.unwrap()
			.contains("Read the report")
	);
	assert_eq!(fixture.backend.0.published.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn invalid_judge_responses_retry_before_asking_the_requester() {
	let mut fixture = goal_fixture();
	fixture.backend.0.goal_replies.lock().unwrap().extend([
		"looks good".into(),
		r#"{"verdict":"success","reason":"Done"}"#.into(),
	]);
	for _ in 0..2 {
		advance_sources(&mut fixture).await.unwrap();
	}
	assert!(
		matches!(fixture.run.state, RunState::Waiting(ref wait) if matches!(**wait, WaitingState::Human { .. }))
	);
	assert!(fixture.backend.0.published.lock().unwrap().is_empty());
	assert_eq!(fixture.backend.0.requests.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn three_uninterrupted_not_yet_verdicts_ask_for_guidance() {
	let mut fixture = goal_fixture();
	fixture
		.backend
		.0
		.goal_replies
		.lock()
		.unwrap()
		.extend((0..3).map(|_| r#"{"verdict":"not_yet","reason":"Read the report"}"#.into()));
	for _ in 0..6 {
		advance_sources(&mut fixture).await.unwrap();
	}
	assert!(
		matches!(fixture.run.state, RunState::Waiting(ref wait) if matches!(**wait, WaitingState::Human { .. }))
	);
	assert_eq!(fixture.backend.0.goal_verdicts.lock().unwrap().len(), 3);
	assert!(fixture.backend.0.published.lock().unwrap().is_empty());
}

#[tokio::test]
async fn impossible_goal_settles_with_a_failed_task_and_one_message() {
	let mut fixture = goal_fixture();
	fixture
		.backend
		.0
		.goal_replies
		.lock()
		.unwrap()
		.push_back(r#"{"verdict":"impossible","reason":"The report was deleted"}"#.into());
	for _ in 0..2 {
		advance_sources(&mut fixture).await.unwrap();
	}
	assert_eq!(fixture.run.phase(), RunPhase::Failed);
	assert_eq!(fixture.backend.task_value().status, TaskStatus::Failed);
	let completion = fixture.run.context.final_completion.as_ref().unwrap();
	assert_eq!(
		completion.verdict.as_ref().unwrap().reason,
		"The report was deleted"
	);
	assert_eq!(fixture.backend.0.published.lock().unwrap().len(), 1);
}
