use super::*;

#[rstest::rstest]
#[tokio::test]
async fn configuration_carries_an_opted_in_unchanged_approval(
	#[future] capability_fixture: CoreFixture,
) {
	let c = Box::pin(capability_fixture).await;
	let node = aidash_domain::nodes::NodeName::derived("acme");
	let (status, approval) = request(
		&c.app,
		&c.token,
		"POST",
		"/api/authorization/acme/catalog",
		json!({
			"node":node,"entry":{"id":"research","version":"1.1.0"},
			"expected_revision":1,"enabled":true,"carry_over":true,"scope":"node"
		}),
	)
	.await;
	assert_eq!(status, 200, "{approval}");
	let source =
		c.f.registry
			.get(
				&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
				"research",
				"1.1.0",
			)
			.await
			.unwrap();
	let (status, saved) = request(
		&c.app,
		&c.token,
		"POST",
		"/api/agents/research/capabilities",
		json!({
			"idempotency_key":Uuid::new_v4(),"source_version":"1.1.0","new_version":"1.2.0",
			"bindings":source.config["bindings"],"remove_default":source.config["remove_default"]
		}),
	)
	.await;
	assert_eq!(status, 200, "{saved}");
	assert_eq!(saved["catalog_approval_required"], false);
	assert!(
		saved["carry_results"]
			.as_array()
			.unwrap()
			.iter()
			.any(|result| result["node"] == node.as_str() && result["carried"] == true)
	);
}
#[rstest::rstest]
#[tokio::test]
async fn subject_configuration_creates_an_immutable_version_without_catalog_grants(
	#[future] capability_fixture: CoreFixture,
) {
	let c = Box::pin(capability_fixture).await;
	let before =
		c.f.registry
			.get(
				&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
				"research",
				"1.1.0",
			)
			.await
			.unwrap();
	let input = json!({"idempotency_key":Uuid::new_v4(),"source_version":"1.1.0","new_version":"1.2.0","bindings":before.config["bindings"],"remove_default":[]});
	let path = "/api/agents/research/capabilities";
	// A configured Skill Source cannot remove its required support operations.
	let mut invalid = input.clone();
	invalid["remove_default"] = json!(["skill_load"]);
	assert_eq!(
		request(&c.app, &c.token, "POST", path, invalid).await.0,
		400
	);
	let mut missing = input.clone();
	missing["bindings"].as_array_mut().unwrap().push(json!({"kind":"source","target":{"registry_deployment":c.f.config.deployment,"id":"missing-source","version":"1.0.0"},"narrow":{}}));
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
	assert_eq!(
		c.f.registry
			.get(
				&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
				"research",
				"1.1.0"
			)
			.await
			.unwrap(),
		before
	);
	assert_eq!(saved["entry"]["config"]["model"], before.config["model"]);
	let mut changed = input.clone();
	changed["remove_default"] = json!(["file_search"]);
	assert_eq!(
		request(&c.app, &c.token, "POST", path, changed).await.0,
		409
	);
	let mut policy = c.policy.clone();
	policy["policies"].as_array_mut().unwrap().push(json!({"id":"deny-bob-configuration","effect":"deny","subjects":{"ids":["bob"]},"actions":["agent.configure"],"resources":{"kinds":["agent"]}}));
	crate::common::replace_policy(&c.f.store, "acme", policy.clone()).await;
	let (_, bob) = async {
		(
			200_u16,
			common::issued_credential(&c.f.store, "acme", "bob").await,
		)
	}
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
		json!({"node_id":c.f.config.deployment,"agent":{"id":"research","version":"1.2.0"}}),
	)
	.await;
	assert_eq!(
		status, 403,
		"new fields must not create an execution grant: {denied}"
	);
	c.close().await;
}
