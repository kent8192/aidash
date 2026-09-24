#[allow(dead_code)]
mod common;

use aidash::{
	domain::NewTask,
	federation::Home,
	tool::{ToolContext, builtins},
};
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires disposable PostgreSQL; see scripts/check.sh"]
async fn observations_do_not_recursively_embed_the_invocation_journal() {
	let (f, url, schema) = common::setup().await;
	let workspace = f
		.store
		.create_workspace("Airline", "航空会社の新規事業計画")
		.await
		.unwrap();
	let task = f
		.store
		.create_task(
			workspace.id,
			&NewTask {
				title: "Plan".into(),
				description: "Coordinate specialists".into(),
				requirements: json!({}),
				dependencies: vec![],
				parent_id: None,
			},
			"human",
			None,
		)
		.await
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
				.transition(child.id, child.revision, "human", "FAILED")
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
	let old_message: Uuid = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.column(sea_orm::sea_query::Alias::new("id"))
			.from(sea_orm::sea_query::Alias::new("messages"))
			.and_where(sea_orm::sea_query::Expr::cust(
				"workspace_id = $1 AND sender = 'review-test' AND content = 'old message'",
			))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(workspace.id)
	.fetch_one(&f.store.pool)
	.await
	.unwrap();
	let artifact_id = Uuid::new_v4();
	sqlx::query(
		&sea_orm::sea_query::Query::insert()
			.into_table(sea_orm::sea_query::Alias::new("artifacts"))
			.columns([
				sea_orm::sea_query::Alias::new("id"),
				sea_orm::sea_query::Alias::new("workspace_id"),
				sea_orm::sea_query::Alias::new("task_id"),
				sea_orm::sea_query::Alias::new("kind"),
				sea_orm::sea_query::Alias::new("name"),
				sea_orm::sea_query::Alias::new("content"),
				sea_orm::sea_query::Alias::new("created_by"),
				sea_orm::sea_query::Alias::new("idempotency_key"),
			])
			.values_panic([
				sea_orm::sea_query::Expr::cust("$1"),
				sea_orm::sea_query::Expr::cust("$2"),
				sea_orm::sea_query::Expr::cust("$3"),
				sea_orm::sea_query::Expr::cust("$4"),
				sea_orm::sea_query::Expr::cust("$5"),
				sea_orm::sea_query::Expr::cust("$6"),
				sea_orm::sea_query::Expr::cust("$7"),
				sea_orm::sea_query::Expr::cust("$8"),
			])
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(artifact_id)
	.bind(workspace.id)
	.bind(task.id)
	.bind("text")
	.bind("review artifact")
	.bind(json!("record query regression"))
	.bind("reviewer")
	.bind("review-artifact")
	.execute(&f.store.pool)
	.await
	.unwrap();
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
	prepared_run.pending = json!({
		"response":{"tool_calls":[{"id":"read-1","name":"workspace_read","arguments":{"max_chars":97}}]},
		"workspace_read_plan":{"step":prepared_run.step,"cursor":0,"result":{"content":"stable chunk"}}
	});
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
	assert_eq!(recovered.pending, prepared_run.pending);
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
	common::cleanup(f, &url, &schema).await;
}
