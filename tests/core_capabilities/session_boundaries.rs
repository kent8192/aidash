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
#[case(false)]
#[case(true)]
#[tokio::test]
async fn designated_approver_is_rechecked_and_preserves_the_original_requester(
	#[case] revoke: bool,
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
	if revoke {
		c.policy["policies"].as_array_mut().unwrap().push(json!({"id":"deny-designated-approver","effect":"deny","subjects":{"ids":["bob"]},"actions":["capability.approve"],"resources":{"kinds":["outbound"]}}));
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
