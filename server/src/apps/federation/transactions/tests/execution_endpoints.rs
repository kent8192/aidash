//! Atomic completion retains the producing execution's live read dependencies.
use crate::endpoint::assert_json;
use crate::execution_fixtures::{ExecutionFixture, execution};
use aidash_server::apps::{
	federation::transactions::models::{
		AtomicHistory, AtomicParticipant,
		states::{AtomicHistoryRole, AtomicParticipantPhase},
	},
	identity::models::{AuthorizationRunOutput, AuthorizationRunRead},
	workspaces::models::{Artifact, Message, Task},
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
	let workspace = app
		.runtime
		.store
		.task(execution.task)
		.await
		.unwrap()
		.workspace_id;
	let sent = assert_json(
		execution
			.subject
			.post(
				&format!("/api/workspaces/{workspace}/messages"),
				&json!({"content":"Protected source for the atomic result"}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	assert_eq!(sent, json!({"sent":true}));
	let source_id = Message::objects()
		.filter(Message::field_workspace_id().eq(workspace))
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
	let run = app.runtime.store.runs().await.unwrap().remove(0);
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
		"id":id,"coordinator":app.runtime.config.node_id,"isolation":"serializable",
		"deadline":Utc::now()+Duration::minutes(5),
		"participants":[{"node_id":app.runtime.config.node_id,"mutations":[
			{"kind":"complete_task","task_id":task.id,"expected_revision":task.revision,
			 "artifact":{"kind":"text","name":"Atomic output","content":"derived protected result"}},
			{"kind":"finish_run","run_id":run.id,"task_id":task.id,"expected_revision":run.revision}
		]}]
	}))
	.unwrap();

	// Preparation validates the complete write set without publishing any output.
	assert_json(
		app.operator
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
		Artifact::objects()
			.filter(Artifact::field_task_id().eq(task.id))
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
		app.runtime.store.task(task.id).await.unwrap().status,
		aidash_server::domain::TaskStatus::Running
	);
	assert_eq!(
		app.runtime
			.store
			.run(run.id)
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

	// Commit and replay must persist exactly one artifact and its provenance.
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
			participant::finish(&app.runtime, &app.runtime.config.node_id, &manifest)
				.await
				.unwrap()
				.phase,
			"COMMITTED"
		);
	}
	let artifacts = Artifact::objects()
		.filter(Artifact::field_task_id().eq(task.id))
		.all_with_db(&mut app.database.lease.handle())
		.await
		.unwrap();
	assert_eq!(artifacts.len(), 1);
	let outputs = AuthorizationRunOutput::objects()
		.filter(AuthorizationRunOutput::field_run_id().eq(run.id))
		.all_with_db(&mut app.database.lease.handle())
		.await
		.unwrap();
	assert_eq!(outputs.len(), 1);
	assert_eq!(outputs[0].resource_id, artifacts[0].id);
	let reads = AuthorizationRunRead::objects()
		.filter(AuthorizationRunRead::field_run_id().eq(run.id))
		.filter(AuthorizationRunRead::field_resource_id().eq(artifacts[0].id))
		.all_with_db(&mut app.database.lease.handle())
		.await
		.unwrap();
	assert_eq!(reads.len(), 1);
	assert_eq!(
		app.runtime.store.task(task.id).await.unwrap().status,
		aidash_server::domain::TaskStatus::Completed
	);
	assert_eq!(
		app.runtime
			.store
			.run(run.id)
			.await
			.unwrap()
			.phase()
			.as_str(),
		"COMPLETED"
	);
	let path = format!("/api/workspaces/{}", task.workspace_id);
	let visible = assert_json(execution.subject.get(&path).await.unwrap(), 200);
	assert_eq!(visible["artifacts"].as_array().unwrap().len(), 1);
	assert_eq!(visible["artifacts"][0]["id"], json!(artifacts[0].id));

	// Revoking a source hides its derived output even while artifact.read is allowed.
	let mut policy = execution.policy.clone();
	policy["policies"].as_array_mut().unwrap().push(json!({
		"id":"revoke-source","effect":"deny","subjects":{"ids":["alice"]},
		"actions":["message.read"],"resources":{"kinds":["message"],"ids":[source_id]}
	}));
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
	let hidden = assert_json(execution.subject.get(&path).await.unwrap(), 200);
	assert_eq!(hidden["artifacts"], json!([]));
	let operator = assert_json(app.operator.get(&path).await.unwrap(), 200);
	assert_eq!(operator["artifacts"].as_array().unwrap().len(), 1);
}
