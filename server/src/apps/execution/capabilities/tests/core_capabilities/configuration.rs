use super::*;
#[rstest::rstest]
#[tokio::test]
async fn subject_configuration_creates_an_immutable_version_without_catalog_grants(
	#[future] capability_fixture: CoreFixture,
) {
	let c = Box::pin(capability_fixture).await;
	let before = c.f.registry.get("research", "1.1.0").await.unwrap();
	let input = json!({"idempotency_key":Uuid::new_v4(),"source_version":"1.1.0","new_version":"1.2.0","core_capabilities":{"files":true,"python":true},"skill_attachments":before.config["skill_attachments"],"skill_roots":[],"reference_attachments":[]});
	let path = "/api/agents/research/capabilities";
	// A configuration cannot claim direct Skills while disabling their tool.
	assert_eq!(
		request(&c.app, &c.token, "POST", path, input.clone())
			.await
			.0,
		400
	);
	let mut input = input;
	input["core_capabilities"]["skills"] = json!(true);
	let mut missing = input.clone();
	missing["reference_attachments"] =
		json!([{"reference_id":Uuid::new_v4(),"digest":"0".repeat(64)}]);
	assert_eq!(
		request(&c.app, &c.token, "POST", path, missing).await.0,
		404
	);
	let (status, saved) = request(&c.app, &c.token, "POST", path, input.clone()).await;
	assert_eq!(status, 200, "{saved}");
	assert_eq!(saved["catalog_approval_required"], true);
	assert_eq!(
		request(&c.app, &c.token, "POST", path, input.clone()).await,
		(200, saved.clone())
	);
	assert_eq!(c.f.registry.get("research", "1.1.0").await.unwrap(), before);
	assert_eq!(saved["entry"]["config"]["model"], before.config["model"]);
	let mut changed = input.clone();
	changed["core_capabilities"]["shell"] = json!(true);
	assert_eq!(
		request(&c.app, &c.token, "POST", path, changed).await.0,
		409
	);
	let mut policy = c.policy.clone();
	policy["policies"].as_array_mut().unwrap().push(json!({"id":"deny-bob-configuration","effect":"deny","subjects":{"ids":["bob"]},"actions":["agent.configure"],"resources":{"kinds":["agent"]}}));
	let (status, result) = request(
		&c.app,
		&c.f.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":2,"bundle":policy}),
	)
	.await;
	assert_eq!(status, 200, "{result}");
	let (_, bob) = request(
		&c.app,
		&c.f.config.api_token,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"bob"}),
	)
	.await;
	assert_eq!(
		request(&c.app, bob["token"].as_str().unwrap(), "POST", path, input)
			.await
			.0,
		403
	);
	let (status, denied) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/tasks/{}/delegate", c.task),
		json!({"node_id":c.f.config.node_id,"agent":{"id":"research","version":"1.2.0"}}),
	)
	.await;
	assert_eq!(
		status, 403,
		"new fields must not create an execution grant: {denied}"
	);
	c.close().await;
}
