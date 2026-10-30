//! Private document admission and provider execution through native endpoints.
#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
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

use aidash_server::apps::registry::models::{AgentKnowledge, AgentKnowledgeImage};
use aidash_server::{context, harness::Harness, knowledge, registry::Entry, tool};
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

#[rstest::rstest]
#[tokio::test]
async fn reference_image_replay_uses_saved_binary_without_another_provider_call(
	#[future] execution: ExecutionFixture,
) {
	use provider_fixtures::upstream_fixtures::{FixedServerGuard, reply};
	use std::sync::{
		Arc,
		atomic::{AtomicUsize, Ordering},
	};
	let execution = Box::pin(execution).await;
	let app = &execution.app;
	let count = Arc::new(AtomicUsize::new(0));
	let observed = count.clone();
	let router = reinhardt::test::stub::StubRouter::new().route("/v1/embeddings", http::Method::POST, reply(move |request: reinhardt::Request| {
		let observed = observed.clone();
		let body = request.json::<Value>().unwrap();
		async move { observed.fetch_add(1, Ordering::SeqCst); reinhardt::Response::ok().with_json(&json!({"model":body["model"],"data":[{"index":0,"embedding":[1.0,0.0]}],"usage":{"prompt_tokens":1}})).unwrap() }
	})).into_server_router();
	let listener = Arc::new(tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap());
	let server = FixedServerGuard::spawn(listener, Arc::new(router), None);
	let endpoint = server.url.clone();
	let runtime = &app.runtime;
	runtime.store.keyless.permit_origin_of(&endpoint).unwrap();
	let tenant = aidash_domain::tenancy::TenantName::new("acme").unwrap();
	let embedding: Entry = serde_json::from_value(json!({"id":"reference-image-test","version":"1.0.0","kind":"embedding","name":{"en":"Reference embedding"},"description":{"en":"Fixture"},"config":{"provider":"openai","endpoint":format!("{endpoint}/v1"),"model":"image-test","model_version":"1","dimensions":2,"inputs":["text","image"]}})).unwrap();
	runtime.registry.register(&tenant, embedding).await.unwrap();
	let authority = aidash_server::authorization::Authorization {
		pool: runtime.store.pool.clone(),
	};
	authority
		.set_catalog(
			"acme",
			&aidash_domain::nodes::NodeName::derived("acme"),
			&aidash_server::registry::EntityRef {
				id: "reference-image-test".into(),
				version: "1.0.0".into(),
			},
			0,
			true,
			"deployment_administrator",
		)
		.await
		.unwrap();
	let input = json!({"entry":{"id":"","version":"1.0.0","kind":"agent","name":{"en":"Image agent"},"description":{"en":"Fixture"},"config":{"model":{"id":"model","version":"1.0.0"},"schema_version":1,"instructions":"Use the reference images.","reference_embedding":{"id":"reference-image-test","version":"1.0.0"},"remove_default":["compaction"]}},"documents":[{"name":"chart.png","media_type":"image/png","data_base64":"iVBORw0KGgo="}]});
	let key = Uuid::new_v4();
	let first = personal(&execution.subject, key, input.clone()).await;
	assert_eq!(first.0, 200, "{first:?}");
	assert_eq!(count.load(Ordering::SeqCst), 1);
	let source = first.1["config"]["bindings"]
		.as_array()
		.unwrap()
		.iter()
		.find(|binding| binding["kind"] == "source")
		.unwrap()["target"]
		.clone();
	let record = AgentKnowledgeImage::objects()
		.filter(AgentKnowledgeImage::field_agent_key().eq(source["id"].as_str().unwrap()))
		.filter(AgentKnowledgeImage::field_agent_version().eq(source["version"].as_str().unwrap()))
		.get_with_db(&mut app.database.lease.handle())
		.await
		.unwrap();
	assert_eq!(record.bytes.as_slice(), b"\x89PNG\r\n\x1a\n".as_slice());
	assert_eq!(record.document_index, 0);
	assert_eq!(record.media_type, "image/png");
	assert_eq!(record.size, 8);
	assert_eq!(
		record.sha256,
		aidash_domain::semantic::indexing::content_digest_bytes(&record.bytes)
	);
	server.abort();
	assert_eq!(personal(&execution.subject, key, input).await, first);
	assert_eq!(count.load(Ordering::SeqCst), 1);
}

async fn personal_agent_roundtrip(mut execution: ExecutionFixture, large_documents: bool) {
	let app = &execution.app;
	let f = &app.runtime;
	let subject = &execution.subject;
	let tenant = aidash_domain::tenancy::TenantName::new("acme").unwrap();
	let skill = json!({"id":"writing","version":"1.0.0","kind":"skill","name":{"en":"Writing"},"description":{"en":"Writing"},"config":{"instructions":"Use the provided reference documents."}});
	assert_eq!(
		subject
			.post("/api/registry", &skill, "json")
			.await
			.unwrap()
			.status_code(),
		200
	);
	let mut input = json!({"entry":{"id":"","version":"1.0.0","kind":"agent","name":{"en":"Personal"},"description":{"en":"Skills first"},"config":{"model":{"id":"model","version":"1.0.0"},"schema_version":1,"bindings":[{"kind":"skill","target":{"registry_deployment":f.config.deployment,"id":"writing","version":"1.0.0"},"narrow":{}}],"remove_default":["compaction"]}},"documents":[{"name":"private.pdf","media_type":"application/pdf","text":"PRIVATE-REFERENCE-123"}]});
	if large_documents {
		let mut model = f.registry.get(&tenant, "model", "1.0.0").await.unwrap();
		model.id = "small-model".into();
		model.config["context_window"] = json!(32768);
		f.registry.register(&tenant, model).await.unwrap();
		input["entry"]["config"]["model"]["id"] = json!("small-model");
		let tools = tool::builtins()
			.values()
			.map(|tool| tool.specification())
			.collect::<Vec<_>>();
		let budget = context::RequestBudget {
			input_format: Default::default(),
			window: 32768,
			instructions: "",
			tools: &tools,
			max_output_tokens: 4096,
			projection: &aidash_domain::context::RequestProjection::Legacy,
			estimate: Default::default(),
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
		let rejected = personal(subject, Uuid::new_v4(), invalid).await;
		assert_eq!(rejected.0, 400, "{rejected:?}");
		assert!(rejected.1.to_string().contains("NUL"));
	}
	let key = Uuid::new_v4();
	assert_eq!(
		personal(&app.deployment_administrator, key, input.clone())
			.await
			.0,
		403
	);
	let first = personal(subject, key, input.clone()).await;
	assert_eq!(first.0, 200, "{first:?}");
	assert!(!first.1.to_string().contains("PRIVATE-REFERENCE"));
	assert_eq!(first, personal(subject, key, input.clone()).await);
	let entry: Entry = serde_json::from_value(first.1.clone()).unwrap();
	let source = entry.config["bindings"]
		.as_array()
		.unwrap()
		.iter()
		.find(|b| b["kind"] == "source")
		.unwrap()["target"]
		.clone();
	let documents = AgentKnowledge::objects()
		.filter(AgentKnowledge::field_agent_key().eq(source["id"].as_str().unwrap()))
		.filter(AgentKnowledge::field_agent_version().eq(source["version"].as_str().unwrap()))
		.get_with_db(&mut app.database.lease.handle())
		.await
		.unwrap();
	assert_eq!(documents.documents[0]["text"], expected_text);
	let mut changed = input.clone();
	changed["documents"][0]["text"] = json!("Different");
	assert_eq!(personal(subject, key, changed).await.0, 409);
	let state = assert_json(
		app.deployment_administrator
			.get("/api/state")
			.await
			.unwrap(),
		200,
	);
	assert!(!state.to_string().contains("PRIVATE-REFERENCE"));
	assert!(!state.to_string().contains("private.pdf"));
	let mut cloned = entry.clone();
	cloned.id = "cloned-personal".into();
	cloned.binding_normalization = None;
	let cloned = f.registry.register(&tenant, cloned).await.unwrap();
	assert_eq!(
		knowledge::load(&f.registry, &tenant, &cloned)
			.await
			.unwrap(),
		knowledge::load(&f.registry, &tenant, &entry).await.unwrap()
	);
	let mut invalid = input;
	invalid["documents"][0]["text"] = json!(" ");
	assert_eq!(personal(subject, Uuid::new_v4(), invalid).await.0, 400);
	let model = &entry.config["model"];
	let approvals: Vec<Value> = (large_documents.then(|| model.clone()))
		.into_iter()
		.chain([
		json!({"id":"writing","version":"1.0.0"}),
		source.clone(),
		json!({"id":entry.id,"version":entry.version}),
	])
	.map(|reference| json!({"entry":{"id":reference["id"],"version":reference["version"]},"enabled":true,"expected_revision":0}))
	.collect();
	assert_json(
		subject
			.post(
				"/api/authorization/acme/catalog/approvals",
				&json!({"node":aidash_domain::nodes::NodeName::derived("acme"), "approvals": approvals }),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	let claimed = subject
		.post(
			&format!("/api/tasks/{}/claim", execution.task),
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
	let diagnostic_state = assert_json(subject.get("/api/state").await.unwrap(), 200);
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
