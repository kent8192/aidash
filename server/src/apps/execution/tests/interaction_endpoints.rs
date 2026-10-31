//! Run control and human answers through the registered HTTP routes.
use crate::endpoint::{assert_json, assert_json_rejection, subject};
use crate::execution_fixtures::{ExecutionFixture, execution};
use aidash_application::tenancy::TEST_FIXTURE;
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
		.runs(&TEST_FIXTURE)
		.await
		.unwrap()
		.into_iter()
		.find(|run| run.task_id == execution.task)
		.unwrap()
}

#[rstest]
#[tokio::test]
async fn human_request_list_defaults_to_pending_for_the_requested_run(
	#[future] execution: ExecutionFixture,
) {
	let execution = Box::pin(execution).await;
	let run = claim(&execution).await;
	let request = execution
		.app
		.runtime
		.store
		.human_request(
			&TEST_FIXTURE,
			&run,
			"QUESTION",
			"Which destination?",
			"list-question",
		)
		.await
		.unwrap();
	let page = assert_json(
		execution
			.subject
			.get(&format!("/api/human-requests?run={}", run.id))
			.await
			.unwrap(),
		200,
	);
	assert_eq!(page["items"].as_array().unwrap().len(), 1);
	assert_eq!(page["items"][0]["id"], request.id.to_string());
	assert!(page["next_cursor"].is_null());
	assert_json(
		execution
			.subject
			.get(&format!(
				"/api/human-requests?run={}&cursor=invalid",
				run.id
			))
			.await
			.unwrap(),
		400,
	);
	assert_json(
		execution
			.subject
			.get(&format!(
				"/api/human-requests?run={}&cursor={}",
				run.id,
				Uuid::new_v4()
			))
			.await
			.unwrap(),
		400,
	);
	assert_json(
		execution
			.subject
			.get(&format!("/api/human-requests?run={}", Uuid::new_v4()))
			.await
			.unwrap(),
		403,
	);
	assert_json(
		execution
			.app
			.deployment_administrator
			.get("/api/human-requests")
			.await
			.unwrap(),
		403,
	);
}

#[rstest]
#[tokio::test]
async fn task_run_list_returns_the_claimed_run_and_forbids_missing_tasks(
	#[future] execution: ExecutionFixture,
) {
	let execution = Box::pin(execution).await;
	let run = claim(&execution).await;
	let page = assert_json(
		execution
			.subject
			.get(&format!("/api/tasks/{}/runs", execution.task))
			.await
			.unwrap(),
		200,
	);
	assert_eq!(page["items"].as_array().unwrap().len(), 1);
	assert_eq!(page["items"][0]["id"], run.id.to_string());
	assert!(page["next_cursor"].is_null());
	assert_json(
		execution
			.subject
			.get(&format!("/api/tasks/{}/runs", Uuid::new_v4()))
			.await
			.unwrap(),
		403,
	);
	assert_json(
		execution
			.app
			.deployment_administrator
			.get(&format!("/api/tasks/{}/runs", execution.task))
			.await
			.unwrap(),
		403,
	);
}

#[rstest]
#[tokio::test]
async fn bounded_lists_enforce_current_workspace_task_and_run_visibility(
	#[future] execution: ExecutionFixture,
) {
	let execution = Box::pin(execution).await;
	let run = claim(&execution).await;
	let private_request = execution
		.app
		.runtime
		.store
		.human_request(
			&TEST_FIXTURE,
			&run,
			"QUESTION",
			"Private?",
			"private-request",
		)
		.await
		.unwrap();
	for index in 0..50 {
		execution
			.app
			.runtime
			.store
			.human_request(
				&TEST_FIXTURE,
				&run,
				"QUESTION",
				"Private?",
				&format!("private-{index}"),
			)
			.await
			.unwrap();
	}
	let mut policy = execution.policy.clone();
	policy["policies"].as_array_mut().unwrap().push(json!({"id":"deny-run-read","effect":"deny","subjects":{"any":true},"actions":["run.read"],"resources":{"kinds":["*"]}}));
	assert_json(
		execution
			.app
			.deployment_administrator
			.post(
				"/api/authorization/acme",
				&json!({"expected_revision":1,"bundle":policy.clone()}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	let task_path = format!("/api/tasks/{}/runs", execution.task);
	let runs = assert_json(execution.subject.get(&task_path).await.unwrap(), 200);
	assert!(runs["items"].as_array().unwrap().is_empty());
	let requests = assert_json(
		execution.subject.get("/api/human-requests").await.unwrap(),
		200,
	);
	assert!(requests["items"].as_array().unwrap().is_empty());
	assert!(requests["next_cursor"].is_string());
	assert_json(
		execution
			.subject
			.get(&format!("/api/human-requests?run={}", run.id))
			.await
			.unwrap(),
		403,
	);
	policy["policies"].as_array_mut().unwrap().push(json!({"id":"deny-task-read","effect":"deny","subjects":{"any":true},"actions":["task.read"],"resources":{"kinds":["*"]}}));
	assert_json(
		execution
			.app
			.deployment_administrator
			.post(
				"/api/authorization/acme",
				&json!({"expected_revision":2,"bundle":policy.clone()}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	assert_json(execution.subject.get(&task_path).await.unwrap(), 403);
	policy["policies"].as_array_mut().unwrap().push(json!({"id":"deny-workspace-read","effect":"deny","subjects":{"any":true},"actions":["workspace.read"],"resources":{"kinds":["*"]}}));
	assert_json(
		execution
			.app
			.deployment_administrator
			.post(
				"/api/authorization/acme",
				&json!({"expected_revision":3,"bundle":policy}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	let requests = assert_json(
		execution.subject.get("/api/human-requests").await.unwrap(),
		200,
	);
	assert!(requests["items"].as_array().unwrap().is_empty());
	assert!(requests["next_cursor"].is_null());
	assert_json(
		execution
			.subject
			.get(&format!(
				"/api/human-requests?cursor={}",
				private_request.id
			))
			.await
			.unwrap(),
		400,
	);
}

#[rstest]
#[tokio::test]
async fn human_request_list_pages_past_fifty_requests(#[future] execution: ExecutionFixture) {
	let execution = Box::pin(execution).await;
	let run = claim(&execution).await;
	for index in 0..51 {
		execution
			.app
			.runtime
			.store
			.human_request(
				&TEST_FIXTURE,
				&run,
				"QUESTION",
				"Question",
				&format!("page-{index}"),
			)
			.await
			.unwrap();
	}
	let path = format!("/api/human-requests?run={}", run.id);
	let first = assert_json(execution.subject.get(&path).await.unwrap(), 200);
	let items = first["items"].as_array().unwrap();
	assert_eq!(items.len(), 50);
	let cursor = first["next_cursor"].as_str().unwrap();
	let second = assert_json(
		execution
			.subject
			.get(&format!("{path}&cursor={cursor}"))
			.await
			.unwrap(),
		200,
	);
	assert_eq!(second["items"].as_array().unwrap().len(), 1);
	assert!(second["next_cursor"].is_null());
	assert!(
		items
			.iter()
			.all(|item| item["id"] != second["items"][0]["id"])
	);
	let keys: Vec<_> = items
		.iter()
		.chain(second["items"].as_array().unwrap())
		.map(|item| {
			(
				chrono::DateTime::parse_from_rfc3339(item["created_at"].as_str().unwrap()).unwrap(),
				Uuid::parse_str(item["id"].as_str().unwrap()).unwrap(),
			)
		})
		.collect();
	assert!(keys.windows(2).all(|pair| pair[0] > pair[1]));
}

#[rstest]
#[tokio::test]
async fn human_requests_hidden_by_current_human_read_policy_are_not_listed(
	#[future] execution: ExecutionFixture,
) {
	let execution = Box::pin(execution).await;
	let run = claim(&execution).await;
	execution
		.app
		.runtime
		.store
		.human_request(&TEST_FIXTURE, &run, "QUESTION", "Private", "human-policy")
		.await
		.unwrap();
	let mut policy = execution.policy.clone();
	policy["policies"].as_array_mut().unwrap().push(json!({"id":"deny-human-read","effect":"deny","subjects":{"any":true},"actions":["human.read"],"resources":{"kinds":["*"]}}));
	assert_json(
		execution
			.app
			.deployment_administrator
			.post(
				"/api/authorization/acme",
				&json!({"expected_revision":1,"bundle":policy}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	let page = assert_json(
		execution.subject.get("/api/human-requests").await.unwrap(),
		200,
	);
	assert!(page["items"].as_array().unwrap().is_empty());
	assert_json(
		execution
			.subject
			.get(&format!("/api/human-requests?run={}", run.id))
			.await
			.unwrap(),
		403,
	);
}

#[rstest]
#[tokio::test]
async fn human_request_list_filters_answered_and_reports_waiting_deadline(
	#[future] execution: ExecutionFixture,
) {
	use aidash_server::domain::{ReadyState, ResumeState, RunState, WaitingState};
	use reinhardt::db::orm::Json as DbJson;
	let execution = Box::pin(execution).await;
	let run = claim(&execution).await;
	let request = execution
		.app
		.runtime
		.store
		.human_request(
			&TEST_FIXTURE,
			&run,
			"QUESTION",
			"Question",
			"deadline-question",
		)
		.await
		.unwrap();
	let deadline = chrono::Utc::now() + chrono::Duration::hours(1);
	let state = RunState::Waiting(Box::new(WaitingState::Human {
		request_id: request.id,
		answer_by: Some(deadline),
		resume: ResumeState::Ready(ReadyState {}),
	}));
	execution.app.database.fixture(async |tx| {
		let mut row = aidash_server::apps::execution::models::Run::objects()
			.filter(aidash_server::apps::execution::models::Run::field_id().eq(run.id))
			.get_with_db(tx).await?;
			row.phase = aidash_server::apps::execution::models::states::RunPhase::Waiting;
		row.pending = DbJson(json!({"state_version": 1, "data": json!(state)["data"], "recovery": {"retry": null, "lease_recovered": false}}));
		aidash_server::apps::execution::models::Run::objects().update_with_conn(tx, &row).await?;
		Ok(())
	}).await;
	let path = format!("/api/human-requests?run={}", run.id);
	let pending = assert_json(execution.subject.get(&path).await.unwrap(), 200);
	let actual =
		chrono::DateTime::parse_from_rfc3339(pending["items"][0]["answer_by"].as_str().unwrap())
			.unwrap();
	assert_eq!(actual, deadline);
	execution
		.app
		.database
		.fixture(async |tx| {
			let mut row = HumanRequest::objects()
				.filter(HumanRequest::field_id().eq(request.id))
				.get_with_db(tx)
				.await?;
			row.response = Some(DbJson(json!({"answer": "there"})));
			row.answered_by = Some("alice".to_owned());
			HumanRequest::objects().update_with_conn(tx, &row).await?;
			Ok(())
		})
		.await;
	let pending = assert_json(execution.subject.get(&path).await.unwrap(), 200);
	assert!(pending["items"].as_array().unwrap().is_empty());
	let answered = assert_json(
		execution
			.subject
			.get(&format!("{path}&state=answered"))
			.await
			.unwrap(),
		200,
	);
	assert_eq!(answered["items"][0]["id"], request.id.to_string());
	assert!(answered["items"][0].get("answer_by").is_none());
	let all = assert_json(
		execution
			.subject
			.get(&format!("{path}&state=all"))
			.await
			.unwrap(),
		200,
	);
	assert_eq!(all["items"].as_array().unwrap().len(), 1);
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
		app.runtime
			.store
			.run(&TEST_FIXTURE, run.id)
			.await
			.unwrap()
			.revision,
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
		let credential =
			crate::endpoint::issued_credential(&app.runtime.store, "acme", "alice").await;
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
		let grant = app
			.database
			.fixture(async |tx| {
				Ok(AuthorizationExecution::objects()
					.filter(AuthorizationExecution::field_run_id().eq(run.id))
					.get_with_db(tx)
					.await?)
			})
			.await;
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
		.events(&TEST_FIXTURE, 0, Some(run.workspace_id), 500)
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
			.run(&TEST_FIXTURE, run.id)
			.await
			.unwrap()
			.phase()
			.as_str(),
		"CANCELLED"
	);
	assert_eq!(
		app.runtime
			.store
			.task(&TEST_FIXTURE, run.task_id)
			.await
			.unwrap()
			.status,
		aidash_server::domain::TaskStatus::Cancelled
	);
}

#[rstest]
#[tokio::test]
async fn human_answers_replay_once_and_enforce_live_read_and_write_authority(
	#[from(crate::endpoint::endpoint)] _endpoint: crate::endpoint::EndpointFuture,
	#[future]
	#[with("aidash://endpoint-test", _endpoint.clone())]
	execution: ExecutionFixture,
	#[from(crate::endpoint::anonymous_client)]
	#[with(_endpoint.clone())]
	_credential_client_0: crate::endpoint::ClientFuture,
) {
	// Arrange a durable human request in a scoped execution.
	let execution = Box::pin(execution).await;
	let app = &execution.app;
	let run = claim(&execution).await;
	let request = app
		.runtime
		.store
		.human_request(
			&TEST_FIXTURE,
			&run,
			"QUESTION",
			"Which destination?",
			"endpoint-question",
		)
		.await
		.unwrap();
	let replay = app
		.runtime
		.store
		.human_request(
			&TEST_FIXTURE,
			&run,
			"QUESTION",
			"Which destination?",
			"endpoint-question",
		)
		.await
		.unwrap();
	assert_eq!(replay.id, request.id);
	assert!(
		app.runtime
			.store
			.human_request(
				&TEST_FIXTURE,
				&run,
				"QUESTION",
				"Changed question",
				"endpoint-question"
			)
			.await
			.is_err()
	);
	let url = format!("/api/human-requests/{}/answer", request.id);
	let foreign = subject(app, "outsider", _credential_client_0.await).await;
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
		.events(&TEST_FIXTURE, 0, Some(run.workspace_id), 500)
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
		.human_request(
			&TEST_FIXTURE,
			&run,
			"QUESTION",
			"Still allowed?",
			"denied-question",
		)
		.await
		.unwrap();
	let mut policy = execution.policy.clone();
	policy["policies"].as_array_mut().unwrap().push(json!({"id":"deny-answers","effect":"deny","subjects":{"any":true},"actions":["human.answer"],"resources":{"kinds":["*"]}}));
	assert_json(
		app.deployment_administrator
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
