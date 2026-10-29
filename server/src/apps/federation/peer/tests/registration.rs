//! Node Registrations through native HTTP: pinning the other Deployment's key,
//! replacing and disabling a registration, and its revision history.
use crate::endpoint::{
	EndpointFixture, browser_mapping, default_node, endpoint, tenant_member, tenant_root,
	tenant_scope,
};
use crate::peering;
use aidash_server::routes;
use reinhardt::test::fixtures::server::test_server_guard;
use rstest::rstest;
use serde_json::{Value, json};

/// Gives `app` a Deployment Key, as `[deployment.key] file` does at startup,
/// and lets it reach the loopback origins of the other fixtures.
fn federate(app: &EndpointFixture, reachable: &[&str]) -> aidash_server::federation::Federation {
	let runtime = peering::enable(&app.runtime, &app.context);
	for origin in reachable {
		peering::permit(&runtime, origin);
	}
	runtime
}

async fn post(app: &EndpointFixture, node: &str, body: &Value, status: u16) -> Value {
	// Node registrations are administered by a Tenant Administrator Mapping (#484).
	let mapping = browser_mapping(app, "acme", "alice").await;
	let (code, response) = mapping
		.send(
			"POST",
			&peering::registration_path("acme", node),
			body.clone(),
		)
		.await;
	assert_eq!(code, status, "{body}: {response}");
	response
}

#[rstest]
#[tokio::test]
async fn a_registration_pins_the_key_and_is_replaced_disabled_and_enabled_by_revision(
	#[future]
	#[from(endpoint)]
	#[with("aidash://local")]
	local: EndpointFixture,
	#[future]
	#[from(endpoint)]
	#[with("aidash://remote")]
	remote: EndpointFixture,
) {
	// Arrange
	let local = Box::pin(local).await;
	let remote = Box::pin(remote).await;
	let relocated = test_server_guard(
		routes()
			.with_di_context(remote.context.clone())
			.into_server(),
	)
	.await;
	let remote_runtime = federate(&remote, &[]);
	let local_runtime = federate(&local, &[&remote.server.url, &relocated.url]);
	tenant_scope(&local_runtime.store, "acme").await;
	tenant_root(&remote_runtime, "other").await;
	let owner = default_node(&local_runtime.store, "acme").await.node;
	let source = default_node(&remote_runtime.store, "other")
		.await
		.to_string();
	let key_id = peering::key_id(&remote_runtime);
	let original = peering::registration(&source, Some((&remote.server.url, &key_id)));
	// Act
	let created = post(&local, owner.as_str(), &original, 200).await;
	let mut moved = original.clone();
	moved["endpoint"] = json!(relocated.url);
	moved["expected_revision"] = json!(1);
	let replaced = post(&local, owner.as_str(), &moved, 200).await;
	// Disabling keeps what the registration pinned and asks no one.
	let mut disabled = moved.clone();
	disabled["enabled"] = json!(false);
	disabled["expected_revision"] = json!(2);
	disabled["endpoint"] = json!("http://127.0.0.1:1");
	disabled["key_id"] = Value::Null;
	let stopped = post(&local, owner.as_str(), &disabled, 200).await;
	let mut enabled = moved.clone();
	enabled["expected_revision"] = json!(3);
	let restored = post(&local, owner.as_str(), &enabled, 200).await;
	// Assert
	assert_eq!(created["node"], owner.as_str());
	assert_eq!(created["source_node"], source);
	assert_eq!(created["endpoint"], remote.server.url);
	assert_eq!(created["key_id"], key_id);
	assert_eq!(created["revision"], 1);
	assert_ne!(created["actor"], "deployment_administrator");
	assert_eq!(replaced["endpoint"], relocated.url);
	assert_eq!(replaced["revision"], 2);
	assert_eq!(stopped["enabled"], false);
	assert_eq!(stopped["endpoint"], relocated.url);
	assert_eq!(stopped["key_id"], key_id);
	assert_eq!(restored["enabled"], true);
	assert_eq!(restored["revision"], 4);
	let member = browser_mapping(&local, "acme", "alice").await;
	let (status, listed) = member
		.send(
			"GET",
			&format!("/api/tenants/acme/nodes/{}/registrations", owner.as_str()),
			Value::Null,
		)
		.await;
	assert_eq!(status, 200, "{listed}");
	assert_eq!(listed.as_array().unwrap().len(), 1);
	assert_eq!(listed[0]["revision"], 4);
	let (status, history) = member
		.send(
			"GET",
			&format!(
				"/api/tenants/acme/nodes/{}/registration-history",
				owner.as_str()
			),
			Value::Null,
		)
		.await;
	assert_eq!(status, 200, "{history}");
	assert_eq!(
		history
			.as_array()
			.unwrap()
			.iter()
			.map(|row| (row["sequence"].clone(), row["enabled"].clone()))
			.collect::<Vec<_>>(),
		[
			(json!(1), json!(true)),
			(json!(2), json!(true)),
			(json!(3), json!(false)),
			(json!(4), json!(true))
		]
	);
}

#[rstest]
#[tokio::test]
async fn a_registration_refuses_a_stranger_a_stale_revision_and_a_self_registration(
	#[future]
	#[from(endpoint)]
	#[with("aidash://local")]
	local: EndpointFixture,
	#[future]
	#[from(endpoint)]
	#[with("aidash://remote")]
	remote: EndpointFixture,
) {
	// Arrange
	let local = Box::pin(local).await;
	let remote = Box::pin(remote).await;
	let remote_runtime = federate(&remote, &[]);
	let local_runtime = federate(&local, &[&remote.server.url]);
	tenant_scope(&local_runtime.store, "acme").await;
	tenant_root(&remote_runtime, "other").await;
	let owner = default_node(&local_runtime.store, "acme").await;
	let source = default_node(&remote_runtime.store, "other")
		.await
		.to_string();
	let key_id = peering::key_id(&remote_runtime);
	let valid = peering::registration(&source, Some((&remote.server.url, &key_id)));
	let name = owner.node.as_str();
	// Act and assert
	let mut wrong_key = valid.clone();
	wrong_key["key_id"] = json!(peering::key_id(&local_runtime));
	post(&local, name, &wrong_key, 422).await;
	let mut other_deployment = valid.clone();
	other_deployment["source_node"] = json!("aidash://different-identity/some-node");
	post(&local, name, &other_deployment, 422).await;
	let mut without_key = valid.clone();
	without_key["key_id"] = Value::Null;
	post(&local, name, &without_key, 400).await;
	post(
		&local,
		name,
		&peering::registration(&owner.to_string(), None),
		400,
	)
	.await;
	let nobody = aidash_domain::nodes::NodeName::derived("nobody");
	post(
		&local,
		name,
		&peering::registration(&format!("aidash://local/{nobody}"), None),
		422,
	)
	.await;
	let mut disabled_first = valid.clone();
	disabled_first["enabled"] = json!(false);
	post(&local, name, &disabled_first, 400).await;
	post(&local, nobody.as_str(), &valid, 404).await;
	post(&local, name, &valid, 200).await;
	let mut stale = valid.clone();
	stale["expected_revision"] = json!(7);
	post(&local, name, &stale, 409).await;
	post(&local, name, &valid, 409).await;
}

#[rstest]
#[tokio::test]
async fn concurrent_first_registrations_of_one_node_create_exactly_one_revision(
	#[future]
	#[from(endpoint)]
	#[with("aidash://local")]
	local: EndpointFixture,
	#[future]
	#[from(endpoint)]
	#[with("aidash://remote")]
	remote: EndpointFixture,
) {
	// Arrange
	let local = Box::pin(local).await;
	let remote = Box::pin(remote).await;
	let remote_runtime = federate(&remote, &[]);
	let local_runtime = federate(&local, &[&remote.server.url]);
	tenant_scope(&local_runtime.store, "acme").await;
	tenant_root(&remote_runtime, "other").await;
	let owner = default_node(&local_runtime.store, "acme").await.node;
	let source = default_node(&remote_runtime.store, "other")
		.await
		.to_string();
	let body = peering::registration(
		&source,
		Some((&remote.server.url, &peering::key_id(&remote_runtime))),
	);
	let path = peering::registration_path("acme", owner.as_str());
	// Act
	let member = browser_mapping(&local, "acme", "alice").await;
	let (first, second) = tokio::join!(
		member.send("POST", &path, body.clone()),
		member.send("POST", &path, body.clone()),
	);
	// Assert
	let mut statuses = [first.0, second.0];
	statuses.sort_unstable();
	assert_eq!(statuses, [200, 409]);
	let (status, history) = member
		.send(
			"GET",
			&format!(
				"/api/tenants/acme/nodes/{}/registration-history",
				owner.as_str()
			),
			Value::Null,
		)
		.await;
	assert_eq!(status, 200, "{history}");
	assert_eq!(history.as_array().unwrap().len(), 1);
}

#[rstest]
#[tokio::test]
async fn bearer_actors_cannot_register_a_node_and_nothing_is_recorded(
	#[future]
	#[from(endpoint)]
	#[with("aidash://local")]
	local: EndpointFixture,
	#[future]
	#[from(endpoint)]
	#[with("aidash://remote")]
	remote: EndpointFixture,
) {
	// Arrange
	let local = Box::pin(local).await;
	let remote = Box::pin(remote).await;
	let remote_runtime = federate(&remote, &[]);
	let local_runtime = federate(&local, &[&remote.server.url]);
	tenant_scope(&local_runtime.store, "acme").await;
	tenant_root(&remote_runtime, "other").await;
	let owner = default_node(&local_runtime.store, "acme").await.node;
	let source = default_node(&remote_runtime.store, "other")
		.await
		.to_string();
	let body = peering::registration(
		&source,
		Some((&remote.server.url, &peering::key_id(&remote_runtime))),
	);
	let path = peering::registration_path("acme", owner.as_str());
	let (subject, _) = tenant_member(&local).await;
	// Act
	let by_administrator = local
		.deployment_administrator
		.post(&path, &body, "json")
		.await
		.unwrap();
	let by_subject = subject.post(&path, &body, "json").await.unwrap();
	// Assert: neither bearer is a Tenant Administrator Mapping (#484).
	assert_eq!(by_administrator.status_code(), 403);
	assert_eq!(by_subject.status_code(), 403);
	let mapping = browser_mapping(&local, "acme", "alice").await;
	let (status, listed) = mapping
		.send(
			"GET",
			&format!("/api/tenants/acme/nodes/{}/registrations", owner.as_str()),
			Value::Null,
		)
		.await;
	assert_eq!(status, 200, "{listed}");
	assert_eq!(listed, json!([]));
}
