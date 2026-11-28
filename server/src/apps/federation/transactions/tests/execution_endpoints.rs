//! Atomic completion retains the producing execution's live read dependencies.
use crate::endpoint::assert_json;
use crate::execution_fixtures::{ExecutionFixture, execution};
use aidash_application::tenancy::TEST_FIXTURE;
use aidash_server::apps::{
	federation::transactions::models::{
		AtomicHistory, AtomicParticipant,
		states::{AtomicHistoryRole, AtomicParticipantPhase},
	},
	identity::models::{AuthorizationRunOutput, AuthorizationRunRead},
	workspaces::models::{Message, Task},
};
use aidash_server::{
	harness::Harness,
	transactions::{Manifest, coordinator, participant},
};
use chrono::{Duration, Utc};
use reinhardt::db::orm::Model;
use rstest::rstest;
use serde_json::json;
use uuid::Uuid;

/// The Tenant's default Node address, which a manifest names as coordinator and participant.
async fn node(runtime: &aidash_server::federation::Federation) -> String {
	crate::endpoint::default_node(&runtime.store, "acme")
		.await
		.to_string()
}

#[rstest]
#[tokio::test]
async fn prepared_completion_rolls_back_outputs_and_commit_retains_source_revocation(
	#[future] execution: ExecutionFixture,
) {
	// Arrange an admitted execution with a protected message in its input context.
	let execution = Box::pin(execution).await;
	let app = &execution.app;
	assert_json(
		execution
			.subject
			.post(
				&format!("/api/tasks/{}/claim", execution.task),
				&json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	let thread = app
		.runtime
		.store
		.task(&TEST_FIXTURE, execution.task)
		.await
		.unwrap()
		.thread_id;
	let sent = assert_json(
		execution
			.subject
			.post(
				&format!("/api/threads/{thread}/messages"),
				&json!({"content":"Protected source for the atomic result"}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	assert_eq!(sent, json!({"sent":true}));
	let source_id = Message::objects()
		.filter(Message::field_thread_id().eq(thread))
		.filter(Message::field_content().eq("Protected source for the atomic result"))
		.get_with_db(&mut app.database.lease.handle())
		.await
		.unwrap()
		.id;
	let harness = Harness {
		federation: app.runtime.clone(),
	};
	assert!(harness.worker_once().await.unwrap());
	assert!(harness.worker_once().await.unwrap());
	let run = app
		.runtime
		.store
		.runs(&TEST_FIXTURE)
		.await
		.unwrap()
		.remove(0);
	assert_eq!(run.phase().as_str(), "TOOL_CALL", "{:?}", run.error);
	let task = Task::objects()
		.filter(Task::field_id().eq(execution.task))
		.get_with_db(&mut app.database.lease.handle())
		.await
		.unwrap();
	let source_reads = AuthorizationRunRead::objects()
		.filter(AuthorizationRunRead::field_run_id().eq(run.id))
		.filter(AuthorizationRunRead::field_resource_id().eq(source_id))
		.all_with_db(&mut app.database.lease.handle())
		.await
		.unwrap();
	assert_eq!(
		source_reads.len(),
		1,
		"inference must record its message dependency"
	);
	let id = Uuid::new_v4();
	let manifest: Manifest = serde_json::from_value(json!({
		"id":id,"coordinator":node(&app.runtime).await,"isolation":"serializable",
		"deadline":Utc::now()+Duration::minutes(5),
		"participants":[{"node_id":node(&app.runtime).await,"mutations":[
			{"kind":"complete_task","task_id":task.id,"expected_revision":task.revision,
			 "text":"derived protected result"},
			{"kind":"finish_run","run_id":run.id,"task_id":task.id,"expected_revision":run.revision}
		]}]
	}))
	.unwrap();

	// Preparation validates the complete write set without publishing any output.
	assert_json(
		execution
			.subject
			.post("/api/transactions", &manifest, "json")
			.await
			.unwrap(),
		202,
	);
	for _ in 0..2 {
		coordinator::advance(&app.runtime, id).await.unwrap();
	}
	let prepared = AtomicParticipant::objects()
		.filter(AtomicParticipant::field_id().eq(id))
		.get_with_db(&mut app.database.lease.handle())
		.await
		.unwrap();
	assert_eq!(prepared.phase, AtomicParticipantPhase::Prepared);
	assert!(
		Message::objects()
			.filter(
				Message::field_idempotency_key()
					.eq(Some(format!("atomic:{}:task:{}", manifest.id, task.id)))
			)
			.all_with_db(&mut app.database.lease.handle())
			.await
			.unwrap()
			.is_empty()
	);
	assert!(
		AuthorizationRunOutput::objects()
			.filter(AuthorizationRunOutput::field_run_id().eq(run.id))
			.all_with_db(&mut app.database.lease.handle())
			.await
			.unwrap()
			.is_empty()
	);
	assert_eq!(
		app.runtime
			.store
			.task(&TEST_FIXTURE, task.id)
			.await
			.unwrap()
			.status,
		aidash_server::domain::TaskStatus::Running
	);
	assert_eq!(
		app.runtime
			.store
			.run(&TEST_FIXTURE, run.id)
			.await
			.unwrap()
			.phase()
			.as_str(),
		"TOOL_CALL"
	);
	let history = AtomicHistory::objects()
		.filter(AtomicHistory::field_transaction_id().eq(id))
		.order_by(&["sequence"])
		.all_with_db(&mut app.database.lease.handle())
		.await
		.unwrap();
	assert_eq!(
		history
			.iter()
			.filter(|row| row.role == AtomicHistoryRole::Participant)
			.map(|row| row.phase.as_str())
			.collect::<Vec<_>>(),
		["RESERVED", "PREPARED"]
	);

	// Commit and replay must persist exactly one Completion Message and its provenance.
	let mut completed = false;
	for _ in 0..8 {
		if coordinator::advance(&app.runtime, id)
			.await
			.unwrap()
			.complete
		{
			completed = true;
			break;
		}
	}
	assert!(completed);
	for _ in 0..2 {
		assert_eq!(
			participant::finish(
				&app.runtime,
				None,
				node(&app.runtime).await.as_str(),
				&share_for(&app.runtime, &manifest, &node(&app.runtime).await).await
			)
			.await
			.unwrap()
			.phase,
			"COMMITTED"
		);
	}
	let completions = Message::objects()
		.filter(
			Message::field_idempotency_key()
				.eq(Some(format!("atomic:{}:task:{}", manifest.id, task.id))),
		)
		.all_with_db(&mut app.database.lease.handle())
		.await
		.unwrap();
	assert_eq!(completions.len(), 1);
	let outputs = AuthorizationRunOutput::objects()
		.filter(AuthorizationRunOutput::field_run_id().eq(run.id))
		.all_with_db(&mut app.database.lease.handle())
		.await
		.unwrap();
	assert_eq!(outputs.len(), 1);
	assert_eq!(outputs[0].resource_id, completions[0].id);
	let reads = AuthorizationRunRead::objects()
		.filter(AuthorizationRunRead::field_run_id().eq(run.id))
		.filter(AuthorizationRunRead::field_resource_id().eq(completions[0].id))
		.all_with_db(&mut app.database.lease.handle())
		.await
		.unwrap();
	assert_eq!(reads.len(), 1);
	assert_eq!(
		app.runtime
			.store
			.task(&TEST_FIXTURE, task.id)
			.await
			.unwrap()
			.status,
		aidash_server::domain::TaskStatus::Completed
	);
	assert_eq!(
		app.runtime
			.store
			.run(&TEST_FIXTURE, run.id)
			.await
			.unwrap()
			.phase()
			.as_str(),
		"COMPLETED"
	);
	let path = format!("/api/threads/{}", task.thread_id);
	let visible = assert_json(execution.subject.get(&path).await.unwrap(), 200);
	assert_eq!(
		visible["messages"]
			.as_array()
			.unwrap()
			.iter()
			.filter(|item| item["completion"].is_object())
			.count(),
		1
	);
	assert!(
		visible["messages"]
			.as_array()
			.unwrap()
			.iter()
			.any(|item| item["id"] == json!(completions[0].id))
	);

	// Revoking a source hides its derived output even while the Completion Message has no direct deny.
	let mut policy = execution.policy.clone();
	policy["policies"].as_array_mut().unwrap().push(json!({
		"id":"revoke-source","effect":"deny","subjects":{"ids":["alice"]},
		"actions":["message.read"],"resources":{"kinds":["message"],"ids":[source_id]}
	}));
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
	let hidden = assert_json(execution.subject.get(&path).await.unwrap(), 200);
	assert!(
		!hidden["messages"]
			.as_array()
			.unwrap()
			.iter()
			.any(|item| item["id"] == json!(completions[0].id))
	);
	// A Deployment Administrator holds no Tenant content authority (#484).
	let refused = assert_json(app.deployment_administrator.get(&path).await.unwrap(), 403);
	assert!(refused.get("messages").is_none(), "{refused}");
}

/// What the participant at `node` receives (ADR 0146): sealed with the seed
/// the coordinator stored, or with a fixed seed when none admitted it.
async fn share_for(
	coordinator: &aidash_server::federation::Federation,
	manifest: &aidash_server::transactions::Manifest,
	node: &str,
) -> aidash_server::transactions::Share {
	let seed = aidash_server::transactions::coordinator::status(coordinator, manifest.id)
		.await
		.map(|status| status.seed)
		.unwrap_or_else(|_| "5e".repeat(32));
	manifest.share(&seed, node).unwrap()
}
