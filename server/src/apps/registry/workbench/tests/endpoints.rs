use crate::endpoint::{assert_json, endpoint, subject};
use aidash_server::apps::identity::models::AuthorizationDecision;
use aidash_server::apps::registry::workbench::models::AgentDraft;
use aidash_server::apps::registry::workbench::models::AgentTestLimit;
use reinhardt::db::orm::Model;
use rstest::{fixture, rstest};
use serde_json::{Value, json};
use uuid::Uuid;

#[fixture]
fn draft_entry() -> Value {
	json!({"id":"","version":"1.0.0","kind":"agent","name":{"en":"Draft"},"description":{"en":"Editable draft"},"schema":{},"config":{"model":{"id":"model","version":"1.0.0"},"instructions":"Work on the task","schema_version":1,"bindings":[],"remove_default":[]}})
}

#[rstest]
#[tokio::test]
async fn draft_create_save_and_stale_write_preserve_owner_and_revision(
	endpoint: crate::endpoint::EndpointFuture,
	draft_entry: Value,
	#[from(crate::endpoint::anonymous_client)]
	#[with(endpoint.clone())]
	_credential_client_0: crate::endpoint::ClientFuture,
) {
	// Arrange
	let app = endpoint.await;
	let alice = subject(&app, "alice", _credential_client_0.await).await;
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
	let persisted = app
		.database
		.fixture(async |tx| {
			Ok(AgentDraft::objects()
				.filter(AgentDraft::field_id().eq(id))
				.get_with_db(tx)
				.await?)
		})
		.await;
	assert_eq!(persisted.revision, 2);
}

#[rstest]
#[tokio::test]
async fn invalid_test_limits_leave_the_saved_tenant_policy_unchanged(
	endpoint: crate::endpoint::EndpointFuture,
	#[from(crate::endpoint::anonymous_client)]
	#[with(endpoint.clone())]
	_credential_client_0: crate::endpoint::ClientFuture,
) {
	let app = endpoint.await;
	let _alice = subject(&app, "alice", _credential_client_0.await).await;
	let path = "/api/workbench/test-limits/endpoint";
	let valid = json!({"tenant":"endpoint","max_input_bytes":4096,"max_output_tokens":128,
		"max_total_tokens":512,"max_steps":4,"max_duration_secs":30,"max_concurrent":1,
		"payload_days":7,"incident_evidence_days":14});
	assert_json(
		app.deployment_administrator
			.put(path, &valid, "json")
			.await
			.unwrap(),
		200,
	);
	for (field, rejected) in [
		("max_input_bytes", 1023),
		("max_steps", 65),
		("max_concurrent", 0),
	] {
		let mut invalid = valid.clone();
		invalid[field] = json!(rejected);
		assert_json(
			app.deployment_administrator
				.put(path, &invalid, "json")
				.await
				.unwrap(),
			400,
		);
	}
	let mut invalid = valid;
	invalid["max_output_tokens"] = json!(1024);
	assert_json(
		app.deployment_administrator
			.put(path, &invalid, "json")
			.await
			.unwrap(),
		400,
	);
	let saved = app
		.database
		.fixture(async |tx| {
			Ok(AgentTestLimit::objects()
				.filter(AgentTestLimit::field_tenant().eq("endpoint"))
				.get_with_db(tx)
				.await?)
		})
		.await;
	assert_eq!(saved.max_input_bytes, 4096);
	assert_eq!(saved.max_steps, 4);
	assert_eq!(saved.max_concurrent, 1);
	assert_eq!(saved.max_output_tokens, 128);
	assert_eq!(saved.max_total_tokens, 512);
}

#[rstest]
#[tokio::test]
async fn tenants_set_their_own_test_limits_at_the_revision_they_read(
	endpoint: crate::endpoint::EndpointFuture,
	#[from(crate::endpoint::anonymous_client)]
	#[with(endpoint.clone())]
	credential_client: crate::endpoint::ClientFuture,
) {
	let app = endpoint.await;
	let alice = subject(&app, "alice", credential_client.await).await;
	let path = "/api/workbench/test-limits/endpoint";
	let read = assert_json(alice.get(path).await.unwrap(), 200);
	assert_eq!(read["limits"]["revision"], 0);
	assert_eq!(read["ceilings"]["max_concurrent"], 32);
	let mut limits = read["limits"].clone();
	limits["max_concurrent"] = json!(3);
	let saved = assert_json(alice.put(path, &limits, "json").await.unwrap(), 200);
	assert_eq!(saved["limits"]["revision"], 1);
	assert_eq!(saved["limits"]["max_concurrent"], 3);
	// A write at the revision another write already advanced conflicts.
	assert_json(alice.put(path, &limits, "json").await.unwrap(), 409);
	limits["tenant"] = json!("other");
	assert_json(
		alice
			.put("/api/workbench/test-limits/other", &limits, "json")
			.await
			.unwrap(),
		403,
	);
	let policy = assert_json(
		app.deployment_administrator
			.get("/api/authorization/endpoint")
			.await
			.unwrap(),
		200,
	);
	let mut bundle = policy["bundle"].clone();
	// The evaluation names this Deployment's node, as every other API path does.
	bundle["policies"].as_array_mut().unwrap().push(json!({
		"id":"no-test-limits","effect":"deny","subjects":{"ids":["alice"]},
		"actions":["workbench.test_limits.update"],"resources":{"kinds":["*"]},
		"condition":{"op":"eq","left":{"source":"environment","path":"/node_id"},
			"right":{"source":"literal","value":app.runtime.config.node_id}}
	}));
	assert_json(
		app.deployment_administrator
			.post(
				"/api/authorization/endpoint",
				&json!({"expected_revision":policy["revision"],"bundle":bundle}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	assert_json(alice.get(path).await.unwrap(), 403);
	let denied = app
		.database
		.fixture(async |tx| {
			Ok(AuthorizationDecision::objects()
				.filter(AuthorizationDecision::field_tenant().eq("endpoint"))
				.filter(AuthorizationDecision::field_action().eq("workbench.test_limits.update"))
				.order_by(&["-sequence"])
				.first_with_db(tx)
				.await?)
		})
		.await
		.expect("the denial is audited after its transaction rolled back");
	assert_eq!(
		denied.decision.0["allowed"], false,
		"{:?}",
		denied.decision.0
	);
	let saved = app
		.database
		.fixture(async |tx| {
			Ok(AgentTestLimit::objects()
				.filter(AgentTestLimit::field_tenant().eq("endpoint"))
				.get_with_db(tx)
				.await?)
		})
		.await;
	assert_eq!((saved.max_concurrent, saved.revision), (3, 1));
}
