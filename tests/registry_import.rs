mod common;
use aidash::api;
use common::{cleanup, request, setup};
use serde_json::{Value, json};

fn entry(id: &str, kind: &str, config: Value) -> Value {
	json!({"id":id,"version":"1.0.0","kind":kind,"name":{"en":id},"description":{"en":"import fixture"},"config":config})
}
fn skill(id: &str) -> Value {
	entry(id, "skill", json!({"instructions":"Read carefully."}))
}
fn model() -> Value {
	entry(
		"model",
		"model",
		json!({"provider":"openrouter","model_id":"fixture","endpoint":"http://127.0.0.1:9/v1","context_window":128000,"modalities":["text"],"cost":{}}),
	)
}
fn agent() -> Value {
	entry(
		"agent",
		"agent",
		json!({"model":{"id":"model","version":"1.0.0"},"instructions":"Follow the skill.","skills":[{"id":"research","version":"1.0.0"}]}),
	)
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn imports_dependencies_before_dependents_and_retries_without_duplicate_events() {
	let (f, url, schema) = setup().await;
	let app = api::router(f.clone());
	let cluster = entry(
		"cluster",
		"cluster",
		json!({"coordinator":{"id":"agent","version":"1.0.0"}}),
	);
	let tool = entry(
		"delegate",
		"tool",
		json!({"transport":"agent","node_id":f.config.node_id,"agent":{"id":"agent","version":"1.0.0"}}),
	);
	let input = json!({"entries":[cluster, tool, agent(), skill("research"), model()]});
	let first = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/registry/import",
		input.clone(),
	)
	.await;
	assert_eq!(first, (200, json!({"imported":5,"unchanged":0})));
	let retry = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/registry/import",
		input,
	)
	.await;
	assert_eq!(retry, (200, json!({"imported":0,"unchanged":5})));
	assert_eq!(f.registry.list(&Default::default()).await.unwrap().len(), 5);
	assert_eq!(
		f.store
			.events(0, None, 100)
			.await
			.unwrap()
			.iter()
			.filter(|event| event.kind == "registry.registered")
			.count(),
		5
	);
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn conflicts_and_missing_references_roll_back_entries_and_events() {
	let (f, url, schema) = setup().await;
	let app = api::router(f.clone());
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/registry/import",
			json!({"entries":[skill("existing")]})
		)
		.await
		.0,
		200
	);
	let mut changed = skill("existing");
	changed["config"]["instructions"] = json!("Changed instructions");
	for (entries, status) in [
		(json!([skill("should-rollback"), changed]), 409),
		(json!([skill("should-rollback"), agent()]), 400),
	] {
		let response = request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/registry/import",
			json!({"entries":entries}),
		)
		.await;
		assert_eq!(response.0, status, "{response:?}");
		assert_eq!(f.registry.list(&Default::default()).await.unwrap().len(), 1);
		assert_eq!(
			f.store
				.events(0, None, 100)
				.await
				.unwrap()
				.iter()
				.filter(|event| event.kind == "registry.registered")
				.count(),
			1
		);
	}
	assert_eq!(
		f.registry.get("existing", "1.0.0").await.unwrap().config["instructions"],
		"Read carefully."
	);
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn rejects_invalid_duplicate_cyclic_and_oversized_batches() {
	let (f, url, schema) = setup().await;
	let app = api::router(f.clone());
	let mut invalid = skill("invalid");
	invalid["version"] = json!("latest");
	let mut cyclic = agent();
	cyclic["config"]["cluster"] = json!({"id":"cluster","version":"1.0.0"});
	let cluster = entry(
		"cluster",
		"cluster",
		json!({"coordinator":{"id":"agent","version":"1.0.0"}}),
	);
	for entries in [
		json!([]),
		json!([skill("duplicate"), skill("duplicate")]),
		json!([skill("valid"), invalid]),
		json!([cyclic, cluster, model(), skill("research")]),
		Value::Array(
			(0..101)
				.map(|index| skill(&format!("skill-{index}")))
				.collect(),
		),
	] {
		let response = request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/registry/import",
			json!({"entries":entries}),
		)
		.await;
		assert_eq!(response.0, 400, "{response:?}");
	}
	let mut unknown = skill("unknown");
	unknown["unexpected"] = json!(true);
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/registry/import",
			json!({"entries":[unknown]})
		)
		.await
		.0,
		422
	);
	assert!(
		f.registry
			.list(&Default::default())
			.await
			.unwrap()
			.is_empty()
	);
	assert!(f.store.events(0, None, 100).await.unwrap().is_empty());
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn import_requires_operator_access() {
	let (f, url, schema) = setup().await;
	let app = api::router(f.clone());
	let input = json!({"entries":[skill("private")]});
	assert_eq!(
		request(
			&app,
			"invalid",
			"POST",
			"/api/registry/import",
			input.clone()
		)
		.await
		.0,
		401
	);
	let (_, token, _) = common::bootstrap(&f, &app, "http://127.0.0.1:9").await;
	assert_eq!(
		request(&app, &token, "POST", "/api/registry/import", input)
			.await
			.0,
		403
	);
	assert!(f.registry.get("private", "1.0.0").await.is_err());
	cleanup(f, &url, &schema).await;
}
