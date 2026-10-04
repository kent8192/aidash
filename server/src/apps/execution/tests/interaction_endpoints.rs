//! Run control and human answers through the registered HTTP routes.
use crate::endpoint::{assert_json, assert_json_rejection, subject};
use crate::execution_fixtures::{ExecutionFixture, execution};
use aidash_server::apps::execution::models::HumanRequest;
use aidash_server::apps::identity::models::AuthorizationExecution;
use aidash_server::{domain::Run, harness::Harness};
use reinhardt::db::orm::Model;
use rstest::rstest;
use serde_json::json;
use uuid::Uuid;

async fn claim(execution: &ExecutionFixture) -> Run {
	assert_json(
		execution
			.subject
			.post(
				&format!("/api/tasks/{}/claim", execution.task),
				&json!({"revision": 0, "agent": {"id": "research", "version": "1.0.0"}}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	execution
		.app
		.runtime
		.store
		.runs()
		.await
		.unwrap()
		.into_iter()
		.find(|run| run.task_id == execution.task)
		.unwrap()
}

#[rstest]
#[tokio::test]
async fn control_rotates_credentials_atomically_and_cancellation_remains_terminal(
	#[future] execution: ExecutionFixture,
) {
	// Arrange an admitted scoped execution without starting inference.
	let execution = Box::pin(execution).await;
	let app = &execution.app;
	let run = claim(&execution).await;
	let url = format!("/api/runs/{}/control", run.id);
	assert_json_rejection(
		execution
			.subject
			.post(&url, &json!({"action":"invalid"}), "json")
			.await
			.unwrap(),
		422,
	);
	assert_eq!(
		app.runtime.store.run(run.id).await.unwrap().revision,
		run.revision
	);
	let paused = assert_json(
		execution
			.subject
			.post(&url, &json!({"action":"pause"}), "json")
			.await
			.unwrap(),
		200,
	);
	assert_eq!(paused["control"], "PAUSED");

	// A fresh credential becomes the durable grant only with successful resume.
	let mut accepted = Uuid::nil();
	for (action, status) in [("resume", 200), ("resume", 409)] {
		let credential = assert_json(
			app.operator
				.post(
					"/api/authorization/acme/credentials",
					&json!({"subject":"alice"}),
					"json",
				)
				.await
				.unwrap(),
			200,
		);
		execution
			.subject
			.set_header(
				"Authorization",
				&format!("Bearer {}", credential["token"].as_str().unwrap()),
			)
			.await
			.unwrap();
		let result = assert_json(
			execution
				.subject
				.post(&url, &json!({"action": action}), "json")
				.await
				.unwrap(),
			status,
		);
		if status == 200 {
			assert_eq!(result["control"], "ACTIVE");
			accepted = Uuid::parse_str(credential["credential"]["id"].as_str().unwrap()).unwrap();
		}
		let grant = AuthorizationExecution::objects()
			.filter(AuthorizationExecution::field_run_id().eq(run.id))
			.get_with_db(&mut app.database.lease.handle())
			.await
			.unwrap();
		assert_eq!(
			grant.credential_id(),
			accepted,
			"failed resume must roll back credential rotation"
		);
		if status == 200 {
			let cancelled = assert_json(
				execution
					.subject
					.post(&url, &json!({"action":"cancel"}), "json")
					.await
					.unwrap(),
				200,
			);
			assert_eq!(cancelled["control"], "CANCELLED");
		}
	}
	let events = app
		.runtime
		.store
		.events(0, Some(run.workspace_id), 500)
		.await
		.unwrap();
	let actions: Vec<_> = events
		.iter()
		.filter(|event| event.kind == "run.control")
		.map(|event| event.data["action"].as_str().unwrap())
		.collect();
	assert_eq!(actions, ["pause", "resume", "cancel"]);
	let harness = Harness {
		federation: app.runtime.clone(),
	};
	assert!(harness.worker_once().await.unwrap());
	assert_eq!(
		app.runtime
			.store
			.run(run.id)
			.await
			.unwrap()
			.phase()
			.as_str(),
		"CANCELLED"
	);
	assert_eq!(
		app.runtime.store.task(run.task_id).await.unwrap().status,
		aidash_server::domain::TaskStatus::Cancelled
	);
}

#[rstest]
#[tokio::test]
async fn human_answers_replay_once_and_enforce_live_read_and_write_authority(
	#[future] execution: ExecutionFixture,
) {
	// Arrange a durable human request in a scoped execution.
	let execution = Box::pin(execution).await;
	let app = &execution.app;
	let run = claim(&execution).await;
	let request = app
		.runtime
		.store
		.human_request(&run, "QUESTION", "Which destination?", "endpoint-question")
		.await
		.unwrap();
	let replay = app
		.runtime
		.store
		.human_request(&run, "QUESTION", "Which destination?", "endpoint-question")
		.await
		.unwrap();
	assert_eq!(replay.id, request.id);
	assert!(
		app.runtime
			.store
			.human_request(&run, "QUESTION", "Changed question", "endpoint-question")
			.await
			.is_err()
	);
	let url = format!("/api/human-requests/{}/answer", request.id);
	let foreign = subject(app, "outsider").await;
	assert_json(
		foreign
			.post(&url, &json!({"destination":"private"}), "json")
			.await
			.unwrap(),
		403,
	);
	assert_json(
		execution
			.subject
			.post(&url, &json!(null), "json")
			.await
			.unwrap(),
		400,
	);

	// Concurrent retries store one answer and one corresponding event.
	let response = json!({"destination":"selected"});
	let (first, replay) = tokio::join!(
		execution.subject.post(&url, &response, "json"),
		execution.subject.post(&url, &response, "json"),
	);
	let first = assert_json(first.unwrap(), 200);
	assert_eq!(assert_json(replay.unwrap(), 200), first);
	assert_eq!(first["response"], response);
	assert_eq!(first["answered_by"], "alice");
	assert_json(
		execution
			.subject
			.post(&url, &json!({"destination":"changed"}), "json")
			.await
			.unwrap(),
		409,
	);
	let events = app
		.runtime
		.store
		.events(0, Some(run.workspace_id), 500)
		.await
		.unwrap();
	assert_eq!(
		events
			.iter()
			.filter(|event| event.kind == "human.requested")
			.count(),
		1
	);
	assert_eq!(
		events
			.iter()
			.filter(|event| event.kind == "human.answered")
			.count(),
		1
	);

	// A new policy denial blocks the next answer without changing saved state.
	let pending = app
		.runtime
		.store
		.human_request(&run, "QUESTION", "Still allowed?", "denied-question")
		.await
		.unwrap();
	let mut policy = execution.policy.clone();
	policy["policies"].as_array_mut().unwrap().push(json!({"id":"deny-answers","effect":"deny","subjects":{"any":true},"actions":["human.answer"],"resources":{"kinds":["*"]}}));
	assert_json(
		app.operator
			.post(
				"/api/authorization/acme",
				&json!({"expected_revision":1,"bundle":policy}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	assert_json(
		execution
			.subject
			.post(
				&format!("/api/human-requests/{}/answer", pending.id),
				&json!("denied"),
				"json",
			)
			.await
			.unwrap(),
		403,
	);
	let persisted = HumanRequest::objects()
		.filter(HumanRequest::field_id().eq(pending.id))
		.get_with_db(&mut app.database.lease.handle())
		.await
		.unwrap();
	assert!(persisted.response.is_none());
	assert!(persisted.answered_by.is_none());
}
