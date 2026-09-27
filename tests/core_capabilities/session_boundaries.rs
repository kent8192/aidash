use super::*;

#[rstest::rstest]
#[tokio::test]
async fn same_agent_child_cannot_deadlock_parent_and_another_thread_remains_runnable(
	#[future] capability_fixture: CoreFixture,
) {
	let c = Box::pin(capability_fixture).await;
	let parent = admit(&c).await;
	let (status,patch)=request(&c.app,&c.token,"POST",&path_for_patch(parent.id),json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"preconditions":{"private.txt":null},"patch":"*** Begin Patch\n*** Add File: private.txt\n+parent-only\n*** End Patch"})).await;
	assert_eq!(status, 200, "{patch}");
	let (_, area) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/runs/{}/working-area", parent.id),
		Value::Null,
	)
	.await;
	let (_, child) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/workspaces/{}/tasks", parent.workspace_id),
		json!({"title":"Child","description":"Dependent work","parent_id":parent.task_id}),
	)
	.await;
	let (status, denied) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/tasks/{}/delegate", child["id"].as_str().unwrap()),
		json!({"node_id":c.f.config.node_id,"agent":{"id":"research","version":"1.1.0"}}),
	)
	.await;
	assert_eq!(status, 409, "{denied}");
	assert!(denied.to_string().contains("SESSION_DEPENDENCY_CYCLE"));
	let first =
		c.f.store
			.lease_run(Uuid::new_v4(), 30)
			.await
			.unwrap()
			.unwrap();
	assert_eq!(first.id, parent.id);
	let (_, other) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/workspaces/{}/tasks", parent.workspace_id),
		json!({"title":"Independent","description":"A separate conversation"}),
	)
	.await;
	let (status, result) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/tasks/{}/delegate", other["id"].as_str().unwrap()),
		json!({"node_id":c.f.config.node_id,"agent":{"id":"research","version":"1.1.0"}}),
	)
	.await;
	assert_eq!(status, 200, "{result}");
	let second =
		c.f.store
			.lease_run(Uuid::new_v4(), 30)
			.await
			.unwrap()
			.unwrap();
	assert_eq!(second.task_id.to_string(), other["id"].as_str().unwrap());
	assert_ne!(second.id, parent.id);
	let (_, own) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/runs/{}/working-area", second.id),
		Value::Null,
	)
	.await;
	assert_ne!(own["id"], area["id"]);
	assert_eq!(own["manifest"], json!([]));
	let (status, denied) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/runs/{}/files/read", second.id),
		json!({"file_id":area["manifest"][0]["file_id"],"representation":"text"}),
	)
	.await;
	assert_eq!(status, 404, "{denied}");
	assert!(!denied.to_string().contains("parent-only"));
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn predecessor_run_cannot_read_files_after_its_successor_becomes_active(
	#[future] capability_fixture: CoreFixture,
) {
	use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
	let c = Box::pin(capability_fixture).await;
	let predecessor = admit(&c).await;
	let message = source(
		&c,
		predecessor.workspace_id,
		"Successor-only current file\n",
	)
	.await;
	let (status, materialized) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/runs/{}/files/materialize", predecessor.id),
		json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"path":"current.txt","source":{"kind":"message","message_id":message}}),
	)
	.await;
	assert_eq!(status, 200, "{materialized}");
	let (_, area) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/runs/{}/working-area", predecessor.id),
		Value::Null,
	)
	.await;
	let file_id = area["manifest"][0]["file_id"].clone();
	sqlx::query(
		&Query::update()
			.table(Alias::new("runs"))
			.value(Alias::new("phase"), "COMPLETED")
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(predecessor.id)
	.execute(&c.f.store.pool)
	.await
	.unwrap();
	let input = json!({"file_id":file_id,"representation":"text"});
	let (status, latest) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/runs/{}/files/read", predecessor.id),
		input.clone(),
	)
	.await;
	assert_eq!(
		status, 200,
		"the latest completed run remains readable: {latest}"
	);
	let (status, queued) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/working-areas/{}/queue", area["id"].as_str().unwrap()),
		json!({"idempotency_key":Uuid::new_v4(),"agent_version":"1.1.0","title":"Successor","description":"Read the current session"}),
	)
	.await;
	assert_eq!(status, 200, "{queued}");
	let successor: Uuid = serde_json::from_value(
		queued["queue"].as_array().unwrap().last().unwrap()["run_id"].clone(),
	)
	.unwrap();
	let (status, denied) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/runs/{}/files/read", predecessor.id),
		input.clone(),
	)
	.await;
	assert_eq!(status, 404, "{denied}");
	let (status, current) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/runs/{successor}/files/read"),
		input,
	)
	.await;
	assert_eq!(status, 200, "{current}");
	assert_eq!(current["content"], "Successor-only current file\n");
	c.close().await;
}

#[rstest::fixture]
async fn approver_fixture(
	#[future(awt)] test_environment: Arc<TestEnvironment>,
) -> (CoreFixture, aidash::domain::Run, String) {
	let c = Box::pin(build_core_fixture(
		test_environment,
		"aidash://approver-test",
	))
	.await;
	let (mut c, run) = Box::pin(configure_approvals(c)).await;
	c.policy["subjects"]["bob"]["attributes"] = json!({"capability_approver":true});
	c.policy["policies"].as_array_mut().unwrap().push(json!({"id":"requester-not-an-approver","effect":"deny","subjects":{"ids":["alice"]},"actions":["capability.approve"],"resources":{"kinds":["outbound"]}}));
	let (status, result) = request(
		&c.app,
		&c.f.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":3,"bundle":c.policy}),
	)
	.await;
	assert_eq!(status, 200, "{result}");
	let (status, credential) = request(
		&c.app,
		&c.f.config.api_token,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"bob"}),
	)
	.await;
	assert_eq!(status, 200, "{credential}");
	(c, run, credential["token"].as_str().unwrap().to_owned())
}
#[rstest::rstest]
#[case("none", false)]
#[case("permission", false)]
#[case("attribute", false)]
#[case("permission", true)]
#[case("attribute", true)]
#[tokio::test]
async fn designated_approver_is_rechecked_and_preserves_the_original_requester(
	#[case] withdrawal: &str,
	#[case] grant: bool,
	#[future] approver_fixture: (CoreFixture, aidash::domain::Run, String),
) {
	let (mut c, run, bob) = Box::pin(approver_fixture).await;
	let (status, pending) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/runs/{}/outbound", run.id),
		json!({"idempotency_key":Uuid::new_v4(),"url":"https://example.com/"}),
	)
	.await;
	assert_eq!(status, 200, "{pending}");
	assert_eq!(pending["approver"], "bob");
	if grant {
		let (status,approved)=request(&c.app,&bob,"POST",&format!("/api/capabilities/approvals/{}/decide",pending["approval_id"].as_str().unwrap()),json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"choice":"allow_run","targets":["https://example.com"],"expires_at":chrono::Utc::now()+chrono::Duration::minutes(1)})).await;
		assert_eq!(status, 200, "{approved}");
		assert!(approved["grant_id"].is_string());
	}
	let (_, cards) = request(
		&c.app,
		&bob,
		"GET",
		"/api/capabilities/approvals",
		Value::Null,
	)
	.await;
	assert_eq!(cards["items"][0]["requester"], "alice");
	assert_eq!(cards["items"][0]["approver"], "bob");
	let revoke = withdrawal != "none";
	if revoke {
		if withdrawal == "attribute" {
			c.policy["subjects"]["bob"]["attributes"] = json!({});
		} else {
			c.policy["policies"].as_array_mut().unwrap().push(json!({"id":"deny-designated-approver","effect":"deny","subjects":{"ids":["bob"]},"actions":["capability.approve"],"resources":{"kinds":["outbound"]}}));
		}
		assert_eq!(
			request(
				&c.app,
				&c.f.config.api_token,
				"POST",
				"/api/authorization/acme",
				json!({"expected_revision":4,"bundle":c.policy})
			)
			.await
			.0,
			200
		);
	}
	let (status, cards) = request(
		&c.app,
		&bob,
		"GET",
		"/api/capabilities/approvals",
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{cards}");
	assert_eq!(
		cards["items"].as_array().unwrap().len(),
		if revoke { 0 } else { 1 + usize::from(grant) },
		"revoked designated approvers must not see stored cards: {cards}"
	);
	let (_, owner_cards) = request(
		&c.app,
		&c.token,
		"GET",
		"/api/capabilities/approvals",
		Value::Null,
	)
	.await;
	assert_eq!(
		owner_cards["items"].as_array().unwrap().len(),
		1 + usize::from(grant),
		"owners retain their cards"
	);
	let (status,result)=request(&c.app,&bob,"POST",&format!("/api/capabilities/approvals/{}/decide",pending["approval_id"].as_str().unwrap()),json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"choice":"allow_run","targets":["https://example.com"],"expires_at":chrono::Utc::now()+chrono::Duration::minutes(1)})).await;
	if revoke {
		assert_eq!(status, 404, "{result}");
		let (blocked_status, blocked) = request(
			&c.app,
			&c.token,
			"POST",
			&format!("/api/runs/{}/outbound", run.id),
			json!({"idempotency_key":Uuid::new_v4(),"url":"https://example.com/blocked"}),
		)
		.await;
		assert_eq!(blocked_status, 200, "{blocked}");
		assert_eq!(blocked["status"], "blocked", "{blocked}");
		assert!(blocked["approver"].is_null());
		let (status, cards) = request(
			&c.app,
			&c.token,
			"GET",
			"/api/capabilities/approvals",
			Value::Null,
		)
		.await;
		assert_eq!(status, 200, "{cards}");
		assert!(
			cards["items"]
				.as_array()
				.unwrap()
				.iter()
				.any(|card| card["state"] == "blocked" && card["approver"].is_null())
		);
	} else {
		assert_eq!(status, 200, "{result}");
		let (_, cards) = request(
			&c.app,
			&c.token,
			"GET",
			"/api/capabilities/approvals",
			Value::Null,
		)
		.await;
		let grant = cards["items"]
			.as_array()
			.unwrap()
			.iter()
			.find(|card| card["kind"] == "grant")
			.unwrap();
		assert_eq!(grant["requester"], "alice");
		assert_eq!(grant["approver"], "bob");
	}
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn approval_list_and_decision_restore_the_request_workspace_context(
	#[future] approver_fixture: (CoreFixture, aidash::domain::Run, String),
) {
	let (c, run, bob) = Box::pin(approver_fixture).await;
	let mut policy = c.policy.clone();
	let policies = policy["policies"].as_array_mut().unwrap();
	policies.retain(|rule| rule["id"] != "approvable-outbound");
	policies.push(json!({
		"id":"outbound-request",
		"effect":"allow",
		"subjects":{"any":true},
		"actions":["capability.request"],
		"resources":{"kinds":["outbound"]}
	}));
	policies.push(json!({
		"id":"workspace-bound-approver",
		"effect":"allow",
		"subjects":{"ids":["bob"]},
		"actions":["capability.approve"],
		"resources":{"kinds":["outbound"]},
		"condition":{"op":"eq","left":{"source":"resource","path":"/workspace_id"},"right":{"source":"literal","value":run.workspace_id}}
	}));
	let (status, updated) = request(
		&c.app,
		&c.f.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":4,"bundle":policy}),
	)
	.await;
	assert_eq!(status, 200, "{updated}");
	let (status, pending) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/runs/{}/outbound", run.id),
		json!({"idempotency_key":Uuid::new_v4(),"url":"https://example.com/"}),
	)
	.await;
	assert_eq!(status, 200, "{pending}");
	assert_eq!(pending["approver"], "bob");
	let (status, cards) = request(
		&c.app,
		&bob,
		"GET",
		"/api/capabilities/approvals",
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{cards}");
	assert_eq!(cards["items"][0]["requester"], "alice");
	let (status, decision) = request(
		&c.app,
		&bob,
		"POST",
		&format!(
			"/api/capabilities/approvals/{}/decide",
			pending["approval_id"].as_str().unwrap()
		),
		json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"choice":"allow_once"}),
	)
	.await;
	assert_eq!(status, 200, "{decision}");
	assert_eq!(decision["state"], "approved");
	c.close().await;
}
