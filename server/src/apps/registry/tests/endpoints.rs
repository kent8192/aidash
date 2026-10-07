use crate::endpoint::{EndpointFixture, assert_json, endpoint};
use aidash_server::apps::execution::models::Event;
use aidash_server::apps::registry::models::{
	AgentKnowledge, Definition, Installation, RegistryRequest,
};
use reinhardt::db::orm::Model;
use rstest::{fixture, rstest};
use serde_json::{Value, json};
use uuid::Uuid;

#[fixture]
pub fn skill() -> Value {
	json!({"id":"endpoint-skill","version":"1.0.0","kind":"skill","name":{"en":"Endpoint skill"},"description":{"en":"Reusable instructions"},"languages":["en"],"schema":{"type":"object"},"config":{"instructions":"Perform the requested task"}})
}

#[rstest]
#[tokio::test]
async fn registry_validation_and_query_filters_are_applied_before_persistence(
	#[future] endpoint: EndpointFixture,
	skill: Value,
) {
	// Arrange
	let app = endpoint.await;
	let seeded_definitions = app
		.runtime
		.registry
		.list(&Default::default())
		.await
		.unwrap();
	let mut invalid = skill.clone();
	invalid["schema"] = json!({"type":"unknown-json-schema-type"});
	// Act
	let rejected = app
		.operator
		.post("/api/registry", &invalid, "json")
		.await
		.unwrap();
	let created = app
		.operator
		.post("/api/registry", &skill, "json")
		.await
		.unwrap();
	let found = app
		.operator
		.get("/api/registry/endpoint-skill/1.0.0")
		.await
		.unwrap();
	let excluded = app.operator.get("/api/registry?kind=agent").await.unwrap();
	// Assert
	assert_json(rejected, 400);
	assert_eq!(assert_json(created, 200)["id"], skill["id"]);
	assert_eq!(assert_json(found, 200)["config"], skill["config"]);
	assert_eq!(assert_json(excluded, 200), json!([]));
	let records = Definition::objects()
		.all()
		.all_with_db(&mut app.database.lease.handle())
		.await
		.unwrap();
	assert_eq!(records.len(), seeded_definitions.len() + 1);
}

#[rstest]
#[tokio::test]
async fn package_install_checks_overrides_digest_and_immutable_versions(
	#[future] endpoint: EndpointFixture,
	skill: Value,
) {
	// Arrange
	let app = endpoint.await;
	let package =
		json!({"entity":skill,"author":"Endpoint test","permissions":[],"dependencies":[]});
	let published = assert_json(
		app.operator
			.post("/api/marketplace", &package, "json")
			.await
			.unwrap(),
		200,
	);
	let path = "/api/marketplace/endpoint-skill/1.0.0/install";
	// Act
	let invalid_override = app
		.operator
		.post(
			path,
			&json!({"digest":published["digest"],"config":{"unknown":true}}),
			"json",
		)
		.await
		.unwrap();
	let invalid_digest = app
		.operator
		.post(
			path,
			&json!({"digest":"sha256:incorrect","config":{}}),
			"json",
		)
		.await
		.unwrap();
	// Assert
	assert_json(invalid_override, 400);
	assert_json(invalid_digest, 409);
	assert!(
		Installation::objects()
			.all()
			.all_with_db(&mut app.database.lease.handle())
			.await
			.unwrap()
			.is_empty()
	);
	// Act
	let install =
		json!({"digest":published["digest"],"config":{"instructions":"Local instructions"}});
	let installed = app.operator.post(path, &install, "json").await.unwrap();
	let repeated = app.operator.post(path, &install, "json").await.unwrap();
	let mut altered = package;
	altered["entity"]["config"]["instructions"] = json!("Changed published version");
	let conflict = app
		.operator
		.post("/api/marketplace", &altered, "json")
		.await
		.unwrap();
	// Assert
	assert_eq!(
		assert_json(installed, 200)["config"]["instructions"],
		"Local instructions"
	);
	assert_json(repeated, 200);
	assert_json(conflict, 409);
	assert_eq!(
		Installation::objects()
			.all()
			.all_with_db(&mut app.database.lease.handle())
			.await
			.unwrap()
			.len(),
		1
	);
}

#[rstest]
#[tokio::test]
async fn registration_rolls_back_invalid_requests_and_replays_concurrent_retries(
	#[future] endpoint: EndpointFixture,
	mut skill: Value,
) {
	// Arrange: one key survives a failed attempt, then two concurrent retries.
	let app = endpoint.await;
	let seeded_definitions = app
		.runtime
		.registry
		.list(&Default::default())
		.await
		.unwrap();
	let key = Uuid::new_v4();
	app.operator
		.set_header("Idempotency-Key", &key.to_string())
		.await
		.unwrap();
	skill["id"] = json!("");
	let mut invalid = skill.clone();
	invalid["schema"] = json!({"type":"invalid-schema-type"});
	assert_json(
		app.operator
			.post("/api/registry", &invalid, "json")
			.await
			.unwrap(),
		400,
	);
	assert!(
		RegistryRequest::objects()
			.all()
			.all_with_db(&mut app.database.lease.handle())
			.await
			.unwrap()
			.is_empty()
	);
	// Act
	let (first, second) = tokio::join!(
		app.operator.post("/api/registry", &skill, "json"),
		app.operator.post("/api/registry", &skill, "json"),
	);
	let first = assert_json(first.unwrap(), 200);
	let second = assert_json(second.unwrap(), 200);
	let mut conflict = skill;
	conflict["config"]["instructions"] = json!("Different instructions");
	let rejected = app
		.operator
		.post("/api/registry", &conflict, "json")
		.await
		.unwrap();
	// Assert: identity, definition and event commit exactly once.
	assert_eq!(first, second);
	Uuid::parse_str(first["id"].as_str().unwrap()).unwrap();
	assert_json(rejected, 409);
	assert_eq!(
		Definition::objects()
			.all()
			.all_with_db(&mut app.database.lease.handle())
			.await
			.unwrap()
			.len(),
		seeded_definitions.len() + 1
	);
	let events = Event::objects()
		.filter(Event::field_kind().eq("registry.registered".to_owned()))
		.all_with_db(&mut app.database.lease.handle())
		.await
		.unwrap();
	assert_eq!(events.len(), 1);
	assert_eq!(events[0].data["id"], first["id"]);
}

#[fixture]
fn model() -> Value {
	json!({"id":"personal-model","version":"1.0.0","kind":"model","name":{"en":"Fixture model"},"description":{"en":"No provider request is made"},"config":{"provider":"openrouter","model_id":"fixture","endpoint":"http://localhost","context_window":32768,"max_output_tokens":4096,"modalities":["text"],"cost":{}}})
}

#[rstest]
#[tokio::test]
async fn personal_documents_commit_once_and_remain_private_on_public_registry_reads(
	#[future] endpoint: EndpointFixture,
	model: Value,
) {
	// Arrange
	let app = endpoint.await;
	assert_json(
		app.operator
			.post("/api/registry", &model, "json")
			.await
			.unwrap(),
		200,
	);
	app.operator
		.set_header("Idempotency-Key", &Uuid::new_v4().to_string())
		.await
		.unwrap();
	let input = json!({"entry":{"id":"","version":"1.0.0","kind":"agent","name":{"en":"Personal"},"description":{"en":"Private reference test"},"config":{"model":{"id":"personal-model","version":"1.0.0"},"instructions":"Use the attached reference documents."}},"documents":[{"name":"private.txt","media_type":"text/plain","text":"PRIVATE-REFERENCE-ONLY"}]});
	// Act
	let created = assert_json(
		app.operator
			.post("/api/agents/personal", &input, "json")
			.await
			.unwrap(),
		200,
	);
	let repeated = assert_json(
		app.operator
			.post("/api/agents/personal", &input, "json")
			.await
			.unwrap(),
		200,
	);
	let found = assert_json(
		app.operator
			.get(&format!(
				"/api/registry/{}/1.0.0",
				created["id"].as_str().unwrap()
			))
			.await
			.unwrap(),
		200,
	);
	let mut changed = input;
	changed["documents"][0]["text"] = json!("Changed private reference");
	let conflict = app
		.operator
		.post("/api/agents/personal", &changed, "json")
		.await
		.unwrap();
	// Assert
	assert_eq!(created, repeated);
	assert_eq!(created, found);
	assert!(!found.to_string().contains("PRIVATE-REFERENCE-ONLY"));
	assert!(!found.to_string().contains("private.txt"));
	assert_json(conflict, 409);
	let documents = AgentKnowledge::objects()
		.all()
		.all_with_db(&mut app.database.lease.handle())
		.await
		.unwrap();
	assert_eq!(documents.len(), 1);
	assert_eq!(documents[0].documents[0]["text"], "PRIVATE-REFERENCE-ONLY");
	let events = Event::objects()
		.filter(Event::field_kind().eq("registry.registered".to_owned()))
		.all_with_db(&mut app.database.lease.handle())
		.await
		.unwrap();
	assert_eq!(events.len(), 2);
}
