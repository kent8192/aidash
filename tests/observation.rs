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
