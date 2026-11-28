//! Observation and paged record reads against the native migration schema.
#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
#[path = "support/endpoint.rs"]
mod endpoint_fixtures;
#[path = "support/native_database.rs"]
mod native_database;

use aidash_application::tenancy::TEST_FIXTURE;
use aidash_server::apps::workspaces::models::Message;
use aidash_server::{
	domain::{NewTask, Task, Thread},
	federation::Home,
	tool::{ToolContext, builtins},
};
use endpoint_fixtures::{EndpointFixture, endpoint};
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
	endpoint_fixtures::register_fixture_agent(&app, "lead").await;
	let f = &app.runtime;
	let scope = endpoint_fixtures::tenant_scope(&f.store, "acme").await;
	let thread: Thread = f
		.store
		.create_thread(&scope, "alice", "Airline")
		.await
		.unwrap();
	let task: Task = f
		.store
		.create_task(
			&scope,
			thread.id,
			&NewTask {
				title: "Plan".into(),
				description: "Coordinate specialists".into(),
				goal: None,
				requirements: json!({}),
				dependencies: vec![],
				parent_id: None,
			},
			"alice",
			None,
		)
		.await
		.unwrap();
	let lead = f
		.registry
		.get(scope.tenant_name().unwrap(), "lead", "1.0.0")
		.await
		.unwrap();
	endpoint_fixtures::local_run(&f.store, &scope, &task, &lead).await;
	let worker = Uuid::new_v4();
	let run = f.store.lease_run(worker, 120).await.unwrap().unwrap();
	let ctx = ToolContext {
		home: Home::new(f.clone(), run.clone()),
		store: f.store.clone(),
		run: run.clone(),
		scope: scope.clone(),
	};
	let tools = builtins();
	let mut first_size = 0;
	// Act
	for i in 0..8 {
		let key = format!("observe-{i}");
		f.store
			.invocation_start(
				&scope,
				&run,
				worker,
				&key,
				"workspace_observe",
				&json!({}),
				true,
			)
			.await
			.unwrap();
		let result = tools["workspace_observe"]
			.invoke(&ctx, json!({}), &key)
			.await
			.unwrap();
		f.store
			.invocation_finish(&scope, &run, worker, &key, &result)
			.await
			.unwrap();
		// Assert
		let audit = f.store.snapshot(&scope, thread.id).await.unwrap();
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
	let snapshot = f.store.snapshot(&scope, thread.id).await.unwrap();
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
				&scope,
				thread.id,
				&NewTask {
					title: format!("Child {index}"),
					description: "large child description ".repeat(2_500),
					goal: None,
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
					&scope,
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
		.child_task_summary(&scope, thread.id, task.id)
		.await
		.unwrap();
	assert!(children.has_pending && children.has_failed);
	// The summary names each Unassigned child by id and title (#772), but
	// never carries a description, so it grows only by a short entry per child.
	assert_eq!(children.unassigned.len(), 69);
	let encoded = serde_json::to_string(&children).unwrap();
	assert!(!encoded.contains("large child description"));
	assert!(encoded.len() < 128 + children.unassigned.len() * 160);
	let old_event = f
		.store
		.emit(
			&scope,
			Some(thread.id),
			"old-record-regression",
			json!({"marker":"beyond the recent event snapshot"}),
		)
		.await
		.unwrap();
	f.store
		.message(
			&scope,
			thread.id,
			"review-test",
			"old message",
			Some("observation-old-message"),
		)
		.await
		.unwrap();
	let old_message = f
		.store
		.pool
		.atomic(&TEST_FIXTURE, async |db| {
			let old_message = Message::objects()
				.filter(Message::field_thread_id().eq(thread.id))
				.filter(
					Message::field_idempotency_key().eq(Some("observation-old-message".to_owned())),
				)
				.get_with_db(db)
				.await?
				.id;
			Ok(old_message)
		})
		.await
		.unwrap();

	for (kind, id, expected) in [
		("task", task.id, "Coordinate specialists"),
		("message", old_message, "old message"),
		("event", old_event.id, "beyond the recent event snapshot"),
	] {
		let record = f
			.store
			.thread_record(&scope, thread.id, kind, id)
			.await
			.unwrap();
		assert!(record.to_string().contains(expected));
	}
	for index in 0..105 {
		f.store
			.emit(
				&scope,
				Some(thread.id),
				"newer-record",
				json!({"index":index}),
			)
			.await
			.unwrap();
		f.store
			.message(
				&scope,
				thread.id,
				"review-test",
				&format!("new message {index}"),
				Some(&format!("observation-new-message-{index}")),
			)
			.await
			.unwrap();
	}
	let recent = f.store.snapshot(&scope, thread.id).await.unwrap();
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
			"thread_read_plan":{"step":prepared_run.step,"cursor":0,"call":{"id":"read-1","name":"workspace_read","arguments":{"max_chars":97}},"result":{"content":"stable chunk"}}
		}),
	)));
	let prepared_input = json!({"kind":"message","id":Uuid::new_v4(),"max_chars":97});
	f.store
		.invocation_start(
			&scope,
			&prepared_run,
			worker,
			"thread-read-plan-durability",
			"workspace_read",
			&prepared_input,
			true,
		)
		.await
		.unwrap();
	let recovered = f.store.run(&scope, run.id).await.unwrap();
	assert_eq!(
		serde_json::to_value(&recovered.state).unwrap()["data"],
		serde_json::to_value(&prepared_run.state).unwrap()["data"]
	);
	let other = f
		.store
		.create_thread(&scope, "alice", "Private")
		.await
		.unwrap();
	assert!(
		tools["workspace_read"]
			.invoke(&ctx, json!({"kind":"thread","id":other.id}), "other")
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
