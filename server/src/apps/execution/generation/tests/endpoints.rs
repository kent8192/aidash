use crate::endpoint::{assert_json, endpoint, subject};
use crate::execution_fixtures::{ExecutionFixture, execution};
use aidash_server::apps::execution::generation::models::{GenerationBudget, GenerationUsage};
use aidash_server::generation::provision;
use aidash_server::harness::Harness;
use reinhardt::db::orm::Model;
use rstest::rstest;
use serde_json::json;

#[rstest]
#[tokio::test]
async fn generation_reads_apply_tenant_authorization(
	endpoint: crate::endpoint::EndpointFuture,
	#[from(crate::endpoint::anonymous_client)]
	#[with(endpoint.clone())]
	_credential_client_0: crate::endpoint::ClientFuture,
) {
	// Arrange
	let app = endpoint.await;
	let alice = subject(&app, "alice", _credential_client_0.await).await;
	// Act
	let own = alice
		.get("/api/generation/endpoint/policies")
		.await
		.unwrap();
	let other = alice.get("/api/generation/another/policies").await.unwrap();
	let anonymous = app
		.anonymous
		.get("/api/generation/endpoint/requests")
		.await
		.unwrap();
	// Assert
	assert_eq!(assert_json(own, 200), json!([]));
	assert_json(other, 403);
	assert_json(anonymous, 401);
}

#[rstest]
#[tokio::test]
async fn generated_execution_reports_charged_usage_and_releases_unused_quota_over_http(
	#[future] execution: ExecutionFixture,
) {
	// Arrange a missing capability backed by an approved generation template.
	let mut fixture = execution.await;
	let app = &fixture.app;
	let mut template = assert_json(
		app.operator
			.get("/api/registry/research/1.0.0")
			.await
			.unwrap(),
		200,
	);
	template["id"] = json!("generated-template");
	template["capabilities"] = json!(["generation.fixture"]);
	let spec = json!({"enabled":true,"template":template,
		"permissions":{"roles":[],"groups":[],"attributes":{}},"approval_required":false,
		"limits":{"max_agents":2,"max_concurrent":2,"max_depth":2,"token_budget":400000,
			"tokens_per_agent":200000,"lifetime_seconds":3600}});
	assert_json(
		app.operator
			.post(
				"/api/generation/acme/policies/research",
				&json!({"expected_revision":0,"spec":spec}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	let workspace = app
		.runtime
		.store
		.task(fixture.task)
		.await
		.unwrap()
		.workspace_id;
	let task = assert_json(
		fixture
			.subject
			.post(
				&format!("/api/workspaces/{workspace}/tasks"),
				&json!({"title":"Generated specialist","description":"Use approved inference",
			"requirements":{"capability":"generation.fixture"}}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	let assignment = assert_json(
		fixture
			.subject
			.post(
				&format!(
					"/api/generation/acme/tasks/{}/assign",
					task["id"].as_str().unwrap()
				),
				&json!({"policy_id":"research","reason":"missing capability"}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	assert_eq!(assignment["kind"], "generated");
	let request = assignment["generation"]["id"].as_str().unwrap();
	let usage_path = format!("/api/generation/acme/requests/{request}/usage");
	let unused = assert_json(fixture.subject.get(&usage_path).await.unwrap(), 200);
	assert_eq!(unused["used_tokens"], 0);
	assert_eq!(unused["inference_attempts"], 0);
	assert!(fixture.provider.received.try_recv().is_err());

	// Act: complete one admitted inference, then reconcile its generation quota twice.
	provision::reconcile(&app.runtime).await.unwrap();
	let worker = Harness {
		federation: app.runtime.clone(),
	};
	for _ in 0..8 {
		worker.worker_once().await.unwrap();
	}
	provision::reconcile(&app.runtime).await.unwrap();
	provision::reconcile(&app.runtime).await.unwrap();

	// Assert the HTTP projection and native records agree on the bounded refund.
	let usage = assert_json(fixture.subject.get(&usage_path).await.unwrap(), 200);
	assert_eq!(
		usage,
		json!({"token_limit":200000,"used_tokens":19,"inference_attempts":1,
		"compaction_call_limit":0,"compaction_calls":0,"embedding_call_limit":0,"embedding_calls":0})
	);
	let requests = assert_json(
		fixture
			.subject
			.get("/api/generation/acme/requests")
			.await
			.unwrap(),
		200,
	);
	assert_eq!(requests[0]["id"], request);
	assert_eq!(requests[0]["status"], "COMPLETED");
	let policies = assert_json(
		fixture
			.subject
			.get("/api/generation/acme/policies")
			.await
			.unwrap(),
		200,
	);
	assert_eq!(policies[0]["allocated_tokens"], 19);
	let mut db = app.database.lease.handle();
	let attempts = GenerationUsage::objects()
		.all()
		.all_with_db(&mut db)
		.await
		.unwrap();
	assert_eq!(attempts.len(), 1);
	assert_eq!(attempts[0].reserved_tokens, 132096);
	assert_eq!(attempts[0].reported_tokens, Some(19));
	let budgets = GenerationBudget::objects()
		.all()
		.all_with_db(&mut db)
		.await
		.unwrap();
	assert_eq!(budgets.len(), 1);
	assert_eq!(budgets[0].used_tokens, 19);
	assert_eq!(
		fixture.provider.received.try_recv().unwrap()["model"],
		"fixture"
	);
	assert!(fixture.provider.received.try_recv().is_err());
}
