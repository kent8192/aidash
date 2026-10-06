//! Observation and paged record reads against the native migration schema.
#[path = "support/endpoint.rs"]
mod endpoint_fixtures;
#[path = "support/native_database.rs"]
mod native_database;

use aidash_server::apps::workspaces::models::states::ArtifactKind;
use aidash_server::apps::workspaces::models::{Artifact, Message};
use aidash_server::{
	domain::{NewTask, Task, Workspace},
	federation::Home,
	tool::{ToolContext, builtins},
};
use endpoint_fixtures::{EndpointFixture, assert_json, endpoint};
use reinhardt::db::orm::Model;
use rstest::rstest;
use serde_json::json;
use uuid::Uuid;

#[rstest]
#[tokio::test]
async fn observations_do_not_recursively_embed_the_invocation_journal(
	#[future] endpoint: EndpointFixture,
) {
	// Arrange
	let app = endpoint.await;
	let f = &app.runtime;
	let workspace: Workspace = serde_json::from_value(assert_json(
		app.operator
			.post(
				"/api/workspaces",
				&json!({"title":"Airline","goal":"航空会社の新規事業計画"}),
				"json",
			)
			.await
			.unwrap(),
		200,
	))
	.unwrap();
	let task: Task = serde_json::from_value(assert_json(
		app.operator
			.post(
				&format!("/api/workspaces/{}/tasks", workspace.id),
				&json!({"title":"Plan","description":"Coordinate specialists"}),
				"json",
			)
			.await
			.unwrap(),
		200,
	))
	.unwrap();
	f.store
		.accept_run(&task, &f.config.node_id, "lead", "1.0.0")
		.await
		.unwrap();
	let worker = Uuid::new_v4();
	let run = f.store.lease_run(worker, 120).await.unwrap().unwrap();
	let ctx = ToolContext {
		home: Home::new(f.clone(), run.clone()),
		store: f.store.clone(),
		run: run.clone(),
	};
	let tools = builtins();
	let mut first_size = 0;
	// Act
	for i in 0..8 {
		let key = format!("observe-{i}");
		f.store
			.invocation_start(&run, worker, &key, "workspace_observe", &json!({}), true)
			.await
			.unwrap();
		let result = tools["workspace_observe"]
			.invoke(&ctx, json!({}), &key)
			.await
			.unwrap();
		f.store
			.invocation_finish(&run, worker, &key, &result)
			.await
			.unwrap();
		// Assert
		let audit = f.store.snapshot(workspace.id).await.unwrap();
		assert!(
			audit
				.events
				.iter()
				.any(|e| e.kind == "tool.completed" && e.data["result"] == result)
		);
		assert!(
			result["events"]
				.as_array()
				.unwrap()
				.iter()
				.all(|event| event.get("data").is_none()),
			"observation must omit audit payloads, including prior observation results"
		);
		let size = result.to_string().len();
		if i == 0 {
			first_size = size;
		}
		assert!(
			size < first_size + 16000,
			"observation grew recursively: {size}"
		);
	}
	let snapshot = f.store.snapshot(workspace.id).await.unwrap();
	let event = snapshot
		.events
		.iter()
		.find(|e| e.kind == "tool.completed")
		.unwrap();
	let mut recovered = String::new();
	let mut offset = 0;
	loop {
		let result = tools["workspace_read"]
			.invoke(
				&ctx,
				json!({"kind":"event","id":event.id,"offset":offset,"max_chars":97}),
				"read",
			)
			.await
			.unwrap();
		let content = result["content"].as_str().unwrap();
		assert!(content.chars().count() <= 97);
		recovered.push_str(content);
		let Some(next) = result["next_offset"].as_u64() else {
			break;
		};
		assert!(next > offset);
		offset = next;
	}
	assert_eq!(
		serde_json::from_str::<serde_json::Value>(&recovered).unwrap(),
		json!(event)
	);
	let mut ids = Vec::new();
	let mut offset = 0;
	loop {
		let page = tools["workspace_observe"]
			.invoke(&ctx, json!({"offset":offset,"limit":3}), "page")
			.await
			.unwrap();
		ids.extend(
			page["events"]
				.as_array()
				.unwrap()
				.iter()
				.map(|e| e["id"].clone()),
		);
		let Some(next) = page["pages"]["events"]["next_offset"].as_u64() else {
			break;
		};
		offset = next;
	}
	assert_eq!(
		ids,
		snapshot
			.events
			.iter()
			.rev()
			.map(|e| json!(e.id))
			.collect::<Vec<_>>()
	);
	for index in 0..70 {
		let child = f
			.store
			.create_task(
				workspace.id,
				&NewTask {
					title: format!("Child {index}"),
					description: "large child description ".repeat(2_500),
					requirements: json!({}),
					dependencies: vec![],
					parent_id: Some(task.id),
				},
				"human",
				None,
			)
			.await
			.unwrap();
		if index == 0 {
			f.store
				.transition(
					child.id,
					child.revision,
					"human",
					aidash_server::domain::TaskStatus::Failed,
				)
				.await
				.unwrap();
		}
	}
	let children = f
		.store
		.child_task_summary(workspace.id, task.id)
		.await
		.unwrap();
	assert!(children.has_pending && children.has_failed);
	assert!(serde_json::to_vec(&children).unwrap().len() < 128);
	let old_event = f
		.store
		.emit(
			Some(workspace.id),
			"old-record-regression",
			json!({"marker":"beyond the recent event snapshot"}),
		)
		.await
		.unwrap();
	f.store
		.message(
			workspace.id,
			"review-test",
			"old message",
			Some("observation-old-message"),
		)
		.await
		.unwrap();
	let mut db = app.database.lease.handle();
	let old_message = Message::objects()
		.filter(Message::field_workspace_id().eq(workspace.id))
		.filter(Message::field_idempotency_key().eq(Some("observation-old-message".to_owned())))
		.get_with_db(&mut db)
		.await
		.unwrap()
		.id;
	let artifact = Artifact::build()
		.workspace_id(workspace.id)
		.task_id(task.id)
		.kind(ArtifactKind::Text)
		.name("review artifact")
		.content(json!("record query regression").into())
		.created_by("reviewer")
		.idempotency_key("review-artifact")
		.finish();
	let artifact_id = Artifact::objects()
		.create_with_conn(&mut db, &artifact)
		.await
		.unwrap()
		.id;

	for (kind, id, expected) in [
		("task", task.id, "Coordinate specialists"),
		("artifact", artifact_id, "record query regression"),
		("message", old_message, "old message"),
		("event", old_event.id, "beyond the recent event snapshot"),
	] {
		let record = f
			.store
			.workspace_record(workspace.id, kind, id)
			.await
			.unwrap();
		assert!(record.to_string().contains(expected));
	}
	for index in 0..105 {
		f.store
			.emit(Some(workspace.id), "newer-record", json!({"index":index}))
			.await
			.unwrap();
		f.store
			.message(
				workspace.id,
				"review-test",
				&format!("new message {index}"),
				Some(&format!("observation-new-message-{index}")),
			)
			.await
			.unwrap();
	}
	let recent = f.store.snapshot(workspace.id).await.unwrap();
	assert!(!recent.events.iter().any(|event| event.id == old_event.id));
	assert!(
		!recent
			.messages
			.iter()
			.any(|message| message.id == old_message)
	);
	for (kind, id, expected) in [
		("event", old_event.id, "beyond the recent event snapshot"),
		("message", old_message, "old message"),
	] {
		let result = tools["workspace_read"]
			.invoke(
				&ctx,
				json!({"kind":kind,"id":id,"max_chars":2000}),
				"old-record",
			)
			.await
			.unwrap();
		let record: serde_json::Value =
			serde_json::from_str(result["content"].as_str().unwrap()).unwrap();
		assert!(record.to_string().contains(expected));
	}
	let mut prepared_run = run.clone();
	prepared_run.state = aidash_server::domain::RunState::ToolCall(Box::new(state::tool_call(
		json!({
			"response":{"tool_calls":[{"id":"read-1","name":"workspace_read","arguments":{"max_chars":97}}]},
			"workspace_read_plan":{"step":prepared_run.step,"cursor":0,"call":{"id":"read-1","name":"workspace_read","arguments":{"max_chars":97}},"result":{"content":"stable chunk"}}
		}),
	)));
	let prepared_input = json!({"kind":"artifact","id":Uuid::new_v4(),"max_chars":97});
	f.store
		.invocation_start(
			&prepared_run,
			worker,
			"workspace-read-plan-durability",
			"workspace_read",
			&prepared_input,
			true,
		)
		.await
		.unwrap();
	let recovered = f.store.run(run.id).await.unwrap();
	assert_eq!(
		serde_json::to_value(&recovered.state).unwrap()["data"],
		serde_json::to_value(&prepared_run.state).unwrap()["data"]
	);
	let other = f
		.store
		.create_workspace("Private", "Another workspace")
		.await
		.unwrap();
	assert!(
		tools["workspace_read"]
			.invoke(&ctx, json!({"kind":"workspace","id":other.id}), "other")
			.await
			.is_err()
	);
	assert!(
		tools["workspace_read"]
			.invoke(
				&ctx,
				json!({"kind":"event","id":event.id,"offset":u64::MAX}),
				"range"
			)
			.await
			.is_err()
	);
	assert!(
		tools["workspace_read"]
			.invoke(
				&ctx,
				json!({"kind":"event","id":event.id,"max_chars":16001}),
				"limit"
			)
			.await
			.is_err()
	);
}

#[path = "support/state.rs"]
mod state;
