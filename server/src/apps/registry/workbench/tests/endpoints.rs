use crate::endpoint::{EndpointFixture, assert_json, endpoint, subject};
use aidash_server::apps::registry::workbench::models::AgentDraft;
use aidash_server::apps::registry::workbench::models::AgentTestLimit;
use reinhardt::db::orm::Model;
use rstest::{fixture, rstest};
use serde_json::{Value, json};
use uuid::Uuid;

#[fixture]
fn draft_entry() -> Value {
	json!({"id":"","version":"1.0.0","kind":"agent","name":{"en":"Draft"},"description":{"en":"Editable draft"},"schema":{},"config":{"model":{"id":"model","version":"1.0.0"},"instructions":"Work on the task","tools":[],"skills":[]}})
}

#[rstest]
#[tokio::test]
async fn draft_create_save_and_stale_write_preserve_owner_and_revision(
	#[future] endpoint: EndpointFixture,
	draft_entry: Value,
) {
	// Arrange
	let app = endpoint.await;
	let alice = subject(&app, "alice").await;
	let draft = assert_json(
		alice
			.post(
				"/api/workbench/drafts",
				&json!({"entry":draft_entry}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	let id = serde_json::from_value::<Uuid>(draft["id"].clone()).unwrap();
	let path = format!("/api/workbench/drafts/{id}");
	let mut edited = draft["entry"].clone();
	edited["config"]["instructions"] = json!("Updated instructions");
	let input = json!({"expected_revision":1,"entry":edited,"release_notes":"Ready to review"});
	// Act
	let saved = alice.put(&path, &input, "json").await.unwrap();
	let stale = alice.put(&path, &input, "json").await.unwrap();
	let loaded = alice.get(&path).await.unwrap();
	// Assert
	assert_eq!(assert_json(saved, 200)["revision"], 2);
	assert_json(stale, 409);
	let loaded = assert_json(loaded, 200);
	assert_eq!(loaded["owner"], "alice");
	assert_eq!(
		loaded["entry"]["config"]["instructions"],
		"Updated instructions"
	);
	let persisted = AgentDraft::objects()
		.filter(AgentDraft::field_id().eq(id))
		.get_with_db(&mut app.database.lease.handle())
		.await
		.unwrap();
	assert_eq!(persisted.revision, 2);
}

#[rstest]
#[tokio::test]
async fn invalid_test_limits_leave_the_saved_tenant_policy_unchanged(
	#[future] endpoint: EndpointFixture,
) {
	let app = endpoint.await;
	let _alice = subject(&app, "alice").await;
	let path = "/api/workbench/test-limits/endpoint";
	let valid = json!({"tenant":"endpoint","max_input_bytes":4096,"max_output_tokens":128,
		"max_total_tokens":512,"max_steps":4,"max_duration_secs":30,"max_concurrent":1,
		"payload_days":7,"incident_evidence_days":14});
	assert_json(app.operator.put(path, &valid, "json").await.unwrap(), 200);
	for (field, rejected) in [
		("max_input_bytes", 1023),
		("max_steps", 65),
		("max_concurrent", 0),
	] {
		let mut invalid = valid.clone();
		invalid[field] = json!(rejected);
		assert_json(app.operator.put(path, &invalid, "json").await.unwrap(), 400);
	}
	let mut invalid = valid;
	invalid["max_output_tokens"] = json!(1024);
	assert_json(app.operator.put(path, &invalid, "json").await.unwrap(), 400);
	let saved = AgentTestLimit::objects()
		.filter(AgentTestLimit::field_tenant().eq("endpoint"))
		.get_with_db(&mut app.database.lease.handle())
		.await
		.unwrap();
	assert_eq!(saved.max_input_bytes, 4096);
	assert_eq!(saved.max_steps, 4);
	assert_eq!(saved.max_concurrent, 1);
	assert_eq!(saved.max_output_tokens, 128);
	assert_eq!(saved.max_total_tokens, 512);
}
