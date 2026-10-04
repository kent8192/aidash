//! Private document admission and provider execution through native endpoints.
#[path = "../../execution/tests/support/endpoint.rs"]
mod endpoint;
#[path = "../../execution/tests/support/execution.rs"]
mod execution_fixtures;
#[path = "../../execution/tests/support/native_database.rs"]
mod native_database;
// The adapter suite also uses delayed and unavailable provider variants.
#[allow(dead_code)]
#[path = "../../execution/tests/provider_fixtures.rs"]
mod provider_fixtures;

use aidash_server::apps::registry::models::AgentKnowledge;
use aidash_server::{
	context,
	domain::NewTask,
	harness::Harness,
	knowledge,
	registry::{Entry, Package},
	tool,
};
use endpoint::assert_json;
use execution_fixtures::{ExecutionFixture, execution};
use reinhardt::db::orm::Model;
use reinhardt::test::APIClient;
use serde_json::{Value, json};
use uuid::Uuid;

async fn personal(client: &APIClient, key: Uuid, value: Value) -> (u16, Value) {
	client
		.set_header("Idempotency-Key", &key.to_string())
		.await
		.unwrap();
	let response = client
		.post("/api/agents/personal", &value, "json")
		.await
		.unwrap();
	(response.status_code(), response.json_value().unwrap())
}

#[rstest::rstest]
#[tokio::test]
async fn private_documents_are_atomic_idempotent_and_absent_from_registry(
	#[future] execution: ExecutionFixture,
) {
	personal_agent_roundtrip(Box::pin(execution).await, false).await;
}

#[rstest::rstest]
#[tokio::test]
async fn admitted_large_private_documents_fit_the_execution_soft_window(
	#[future] execution: ExecutionFixture,
) {
	personal_agent_roundtrip(Box::pin(execution).await, true).await;
}

async fn personal_agent_roundtrip(mut execution: ExecutionFixture, large_documents: bool) {
	let app = &execution.app;
	let f = &app.runtime;
	let skill = json!({"id":"writing","version":"1.0.0","kind":"skill","name":{"en":"Writing"},"description":{"en":"Writing"},"config":{"instructions":"Use the provided reference documents."}});
	assert_eq!(
		app.operator
			.post("/api/registry", &skill, "json")
			.await
			.unwrap()
			.status_code(),
		200
	);
	let mut input = json!({"entry":{"id":"","version":"1.0.0","kind":"agent","name":{"en":"Personal"},"description":{"en":"Skills first"},"config":{"model":{"id":"model","version":"1.0.0"},"skills":[{"id":"writing","version":"1.0.0"}]}},"documents":[{"name":"private.pdf","media_type":"application/pdf","text":"PRIVATE-REFERENCE-123"}]});
	if large_documents {
		let mut model = f.registry.get("model", "1.0.0").await.unwrap();
		model.id = "small-model".into();
		model.config["context_window"] = json!(32768);
		f.registry.register(model).await.unwrap();
		input["entry"]["config"]["model"]["id"] = json!("small-model");
		let tools = tool::builtins()
			.values()
			.map(|tool| tool.specification())
			.collect::<Vec<_>>();
		let budget = context::RequestBudget {
			window: 32768,
			instructions: "",
			tools: &tools,
			max_output_tokens: 4096,
		};
		let private = json!({"reference_documents":input["documents"]});
		// Leave 6,000 units for the system prompt and workspace: admission fits,
		// but reserving 8,192 units without accounting for documents cannot.
		let padding = budget.remaining(&context::Context::default(), &private) - 6000;
		input["documents"][0]["text"] =
			json!(format!("PRIVATE-REFERENCE-123{}", "x".repeat(padding)));
	}
	let expected_text = input["documents"][0]["text"].clone();
	for field in ["name", "text"] {
		let mut invalid = input.clone();
		invalid["documents"][0][field] = json!("invalid\0document");
		let rejected = personal(&app.operator, Uuid::new_v4(), invalid).await;
		assert_eq!(rejected.0, 400, "{rejected:?}");
		assert!(rejected.1.to_string().contains("NUL"));
	}
	let key = Uuid::new_v4();
	assert_eq!(
		personal(&execution.subject, key, input.clone()).await.0,
		403
	);
	let first = personal(&app.operator, key, input.clone()).await;
	assert_eq!(first.0, 200, "{first:?}");
	assert!(!first.1.to_string().contains("PRIVATE-REFERENCE"));
	assert_eq!(first, personal(&app.operator, key, input.clone()).await);
	let entry: Entry = serde_json::from_value(first.1.clone()).unwrap();
	let documents = AgentKnowledge::objects()
		.filter(AgentKnowledge::field_agent_key().eq(entry.id.clone()))
		.filter(AgentKnowledge::field_agent_version().eq(entry.version.clone()))
		.get_with_db(&mut app.database.lease.handle())
		.await
		.unwrap();
	assert_eq!(documents.documents[0]["text"], expected_text);
	let mut changed = input.clone();
	changed["documents"][0]["text"] = json!("Different");
	assert_eq!(personal(&app.operator, key, changed).await.0, 409);
	let state = assert_json(app.operator.get("/api/state").await.unwrap(), 200);
	assert!(!state.to_string().contains("PRIVATE-REFERENCE"));
	assert!(!state.to_string().contains("private.pdf"));
	let package = Package {
		entity: entry.clone(),
		author: "fixture".into(),
		permissions: vec![],
		dependencies: vec![],
	};
	let published = f.registry.publish(package).await.unwrap();
	f.registry
		.install(
			&published.id,
			&published.version,
			&published.digest,
			json!({}),
		)
		.await
		.unwrap();
	let mut cloned = entry.clone();
	cloned.id = "cloned-personal".into();
	f.registry.register(cloned.clone()).await.unwrap();
	assert!(knowledge::load(&f.registry.db, &cloned).await.is_err());
	let mut invalid = input;
	invalid["documents"][0]["text"] = json!(" ");
	assert_eq!(
		personal(&app.operator, Uuid::new_v4(), invalid).await.0,
		400
	);
	let workspace = f
		.store
		.create_workspace("Personal", "Use documents")
		.await
		.unwrap();
	let task = f
		.store
		.create_task(
			workspace.id,
			&NewTask {
				title: "Read documents".into(),
				description: "Summarize references".into(),
				requirements: json!({}),
				dependencies: vec![],
				parent_id: None,
			},
			"human",
			None,
		)
		.await
		.unwrap();
	let claimed = app
		.operator
		.post(
			&format!("/api/tasks/{}/claim", task.id),
			&json!({"revision":0,"agent":{"id":entry.id,"version":entry.version}}),
			"json",
		)
		.await
		.unwrap();
	assert_json(claimed, 200);
	let harness = Harness {
		federation: f.clone(),
	};
	for _ in 0..12 {
		if !harness.worker_once().await.unwrap() {
			break;
		}
	}
	let diagnostic_state = assert_json(app.operator.get("/api/state").await.unwrap(), 200);
	let body = execution
		.provider
		.received
		.try_recv()
		.unwrap_or_else(|error| panic!("no inference ({error}): {}", diagnostic_state["runs"]));
	let context: Value =
		serde_json::from_str(body["messages"][1]["content"].as_str().unwrap()).unwrap();
	assert_eq!(
		context["current"]["reference_documents"][0]["text"],
		expected_text
	);
	let instructions = body["messages"][0]["content"].as_str().unwrap();
	assert!(instructions.contains("Use the provided reference documents."));
	assert!(!instructions.contains("PRIVATE-REFERENCE-123"));
}
