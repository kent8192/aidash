//! A `task_delegate` whose delegated Agent cannot read an attached Reference pauses the
//! delegating Run and names that Agent (#754).
use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};

/// Upload a Reference, and mark it ready as the extraction worker would: this fixture has no
/// isolated runner, and admission reads only the committed original.
async fn ready_reference(c: &CoreFixture) -> (Uuid, String) {
	let bytes = b"private original";
	let digest = aidash_server::capabilities::objects::digest(bytes);
	let (status, upload) = request(&c.app,&c.token,"POST","/api/references/uploads",json!({"idempotency_key":Uuid::new_v4(),"name":"attached.txt","media_type":"text/plain","size":bytes.len(),"digest":digest})).await;
	assert_eq!(status, 200, "{upload}");
	let id: Uuid = serde_json::from_value(upload["reference_id"].clone()).unwrap();
	let path = format!("/api/references/{id}");
	for (route, body) in [
		("chunks", json!({"offset":0,"data":STANDARD.encode(bytes)})),
		("commit", Value::Null),
	] {
		let (status, reply) =
			request(&c.app, &c.token, "POST", &format!("{path}/{route}"), body).await;
		assert_eq!(status, 200, "{reply}");
	}
	sqlx::query("UPDATE core_records SET state = 'ready' WHERE id = $1 AND kind = 'reference'")
		.bind(id)
		.execute(c.f.store.pool.fixture_driver())
		.await
		.unwrap();
	(id, digest)
}

/// Register `research@1.2.0`, which attaches the Reference, and approve it on the default Node.
async fn register_child(c: &CoreFixture, reference: Uuid, digest: &str) {
	let source = json!({"id":"attached-reference","version":"1.0.0","kind":"source","name":{"en":"attached"},"description":{"en":"Immutable reference set"},"schema":{},"config":{"schema_version":1,"source":{"adapter":"reference_attachments","references":[{"reference_id":reference,"digest":digest}]}}});
	let (status, result) = request(&c.app, &c.token, "POST", "/api/registry", source).await;
	assert_eq!(status, 200, "{result}");
	let node = aidash_domain::nodes::NodeName::derived("acme");
	let (status, result) = request(&c.app,&c.token,"POST","/api/authorization/acme/catalog",json!({"node":node,"entry":{"id":"attached-reference","version":"1.0.0"},"expected_revision":0,"enabled":true})).await;
	assert_eq!(status, 200, "{result}");
	let mut agent =
		c.f.registry
			.get(
				&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
				"research",
				"1.1.0",
			)
			.await
			.unwrap();
	agent.version = "1.2.0".into();
	agent.binding_normalization = None;
	agent.config["bindings"].as_array_mut().unwrap().push(json!({"kind":"source","target":{"registry_deployment":c.f.config.deployment,"id":"attached-reference","version":"1.0.0"},"narrow":{}}));
	let (status, result) = request(&c.app, &c.token, "POST", "/api/registry", json!(agent)).await;
	assert_eq!(status, 200, "{result}");
	let (status, result) = request(&c.app,&c.token,"POST","/api/authorization/acme/catalog",json!({"node":node,"entry":{"id":"research","version":"1.2.0"},"expected_revision":0,"enabled":true})).await;
	assert_eq!(status, 200, "{result}");
}

#[rstest::rstest]
#[tokio::test]
async fn a_delegated_agent_without_reference_read_pauses_the_delegating_run_naming_it(
	#[future] capability_fixture: CoreFixture,
) {
	// Arrange: the child attaches a Reference the policy lets its owner and the parent Agent
	// read, but not the child.
	let mut c = Box::pin(capability_fixture).await;
	let child = aidash_server::domain::qualified_agent(
		&aidash_domain::nodes::NodeAddress::derived(c.f.config.deployment.as_str(), "acme"),
		"research",
		"1.2.0",
	);
	c.policy["subjects"][child.clone()] =
		json!({"kind":"agent", "node": aidash_domain::nodes::NodeName::derived("acme")});
	crate::common::replace_policy(&c.f.store, "acme", c.policy.clone()).await;
	let (reference, digest) = ready_reference(&c).await;
	register_child(&c, reference, &digest).await;
	let granted = c.policy.clone();
	let mut denied = c.policy.clone();
	denied["policies"].as_array_mut().unwrap().push(json!({"id":"child-cannot-read","effect":"deny","subjects":{"ids":[child]},"actions":["reference.read"],"resources":{"kinds":["reference"]}}));
	crate::common::replace_policy(&c.f.store, "acme", denied).await;
	let parent = admit(&c).await;
	let workspace =
		c.f.store
			.task(&TEST_FIXTURE, c.task)
			.await
			.unwrap()
			.workspace_id;
	let (status, delegated_task) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/workspaces/{workspace}/tasks"),
		json!({"title":"Child work","description":"Use the attached Reference"}),
	)
	.await;
	assert_eq!(status, 200, "{delegated_task}");
	let delegated_task: Uuid = serde_json::from_value(delegated_task["id"].clone()).unwrap();
	let response = aidash_server::provider::ModelResponse {
		tool_calls: vec![aidash_server::provider::ToolCall {
			id: "delegate-with-reference".into(),
			name: "task_delegate".into(),
			arguments: json!({"task_id":delegated_task,"node_id":c.f.config.deployment,"agent":{"id":"research","version":"1.2.0"}}),
		}],
		..Default::default()
	};
	let pending = crate::common::tool_pending(
		json!({"response":response,"cursor":0,"response_epoch":1,"request_window":120000,"request_tokens":0}),
	);
	sqlx::query("UPDATE runs SET phase = 'TOOL_CALL', pending = $1 WHERE id = $2")
		.bind(pending)
		.bind(parent.id)
		.execute(c.f.store.pool.fixture_driver())
		.await
		.unwrap();
	let harness = aidash_server::harness::Harness {
		federation: c.f.clone(),
	};
	// Act
	harness.worker_once().await.unwrap();
	// Assert: the parent pauses and says whose authority refused.
	let paused = c.f.store.run(&TEST_FIXTURE, parent.id).await.unwrap();
	assert_eq!(
		paused.control.as_str(),
		"PAUSED",
		"error={:?}",
		paused.error
	);
	assert_eq!(
		paused.error.as_deref(),
		Some(format!("task_delegate: reference.read was denied for {child}").as_str())
	);
	let status: String = sqlx::query_scalar(
		"SELECT status FROM invocations WHERE run_id = $1 AND tool = 'task_delegate'",
	)
	.bind(parent.id)
	.fetch_one(c.f.store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(status, "REFUSED");
	let denials: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM authorization_decisions WHERE action = 'reference.read' AND decision ->> 'allowed' = 'false'")
		.fetch_one(c.f.store.pool.fixture_driver())
		.await
		.unwrap();
	assert!(denials >= 1, "the denied reference.read must be audited");
	// Act: once the policy grants it, resuming goes through.
	crate::common::replace_policy(&c.f.store, "acme", granted).await;
	let (status, resumed) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/runs/{}/control", parent.id),
		json!({"action":"resume"}),
	)
	.await;
	assert_eq!(status, 200, "{resumed}");
	harness.worker_once().await.unwrap();
	// Assert: the delegation went through.
	let runs = c.f.store.runs(&TEST_FIXTURE).await.unwrap();
	assert!(
		runs.iter().any(|run| run.task_id == delegated_task),
		"the delegated task was claimed after the grant"
	);
	let again = c.f.store.run(&TEST_FIXTURE, parent.id).await.unwrap();
	assert_eq!(again.control.as_str(), "ACTIVE", "error={:?}", again.error);
	c.close().await;
}
