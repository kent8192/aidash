//! Node Registrations: which Nodes of other Tenants a Tenant's Node accepts
//! (#241), over HTTP and PostgreSQL.
use super::*;
use aidash_capability::InMemorySigner;
use aidash_domain::nodes::DeploymentKeyId;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A loopback server that answers every request with `document` as JSON.
async fn serve(document: Value) -> String {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	tokio::spawn(async move {
		loop {
			let Ok((mut stream, _)) = listener.accept().await else {
				return;
			};
			let body = document.to_string();
			tokio::spawn(async move {
				let mut request = vec![0; 4096];
				let _ = stream.read(&mut request).await;
				let response = format!(
					"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
					body.len()
				);
				let _ = stream.write_all(response.as_bytes()).await;
				let _ = stream.shutdown().await;
			});
		}
	});
	endpoint
}

fn key(seed: u8) -> (InMemorySigner, DeploymentKeyId) {
	let signer = InMemorySigner::new("kid".into(), [seed; 32]);
	let id = DeploymentKeyId::of_public_key(&signer.public_key().to_bytes());
	(signer, id)
}

fn identity(signer: &InMemorySigner, id: &DeploymentKeyId) -> Value {
	json!({
		"id": "aidash://calm-heron",
		"endpoint": "http://127.0.0.1:1",
		"protocol_version": aidash_domain::federation::protocol::CURRENT,
		"key": {"id": id.as_str(), "public_key_pem": signer.public_key_pem()},
	})
}

impl Deployment {
	async fn default_node(&self, person: &Person, context: &str, tenant: &str) -> String {
		self.nodes_of(person, context, tenant).await[0]["name"]
			.as_str()
			.unwrap()
			.to_owned()
	}

	async fn address_of(&self, person: &Person, context: &str, tenant: &str) -> String {
		self.nodes_of(person, context, tenant).await[0]["address"]
			.as_str()
			.unwrap()
			.to_owned()
	}

	async fn register(
		&self,
		person: &Person,
		context: &str,
		tenant: &str,
		node: &str,
		body: Value,
	) -> (u16, Value) {
		self.as_person(
			person,
			"POST",
			&format!("/api/tenants/{tenant}/nodes/{node}/registrations"),
			Some(context),
			body,
		)
		.await
	}

	async fn registrations(
		&self,
		person: &Person,
		context: &str,
		tenant: &str,
		node: &str,
		route: &str,
	) -> (u16, Value) {
		self.as_person(
			person,
			"GET",
			&format!("/api/tenants/{tenant}/nodes/{node}/{route}"),
			Some(context),
			Value::Null,
		)
		.await
	}
}

#[rstest::rstest]
#[tokio::test]
async fn a_tenant_administrator_registers_a_node_of_another_tenant_and_revises_it(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::new(runtime, open_rules()).await;
	let alice = d.person("alice").await;
	let bob = d.person("bob").await;
	let (alice_tenant, alice_context) = d.node_tenant(&alice, "Alice").await;
	let (bob_tenant, bob_context) = d.node_tenant(&bob, "Bob").await;
	let receiving = d.default_node(&alice, &alice_context, &alice_tenant).await;
	let sending = d.address_of(&bob, &bob_context, &bob_tenant).await;

	let (status, created) = d
		.register(
			&alice,
			&alice_context,
			&alice_tenant,
			&receiving,
			json!({"source_node":sending,"atomic_transactions":false,"enabled":true,"expected_revision":0}),
		)
		.await;
	assert_eq!(status, 200, "{created}");
	assert_eq!(created["revision"], 1);
	assert_eq!(created["source_node"], sending);
	assert_eq!(created["node"], receiving);
	assert_eq!(created["endpoint"], Value::Null);
	assert_eq!(created["key_id"], Value::Null);
	assert_eq!(created["enabled"], true);
	assert!(created["actor"].as_str().unwrap().starts_with("oidc:"));

	let (status, again) = d
		.register(
			&alice,
			&alice_context,
			&alice_tenant,
			&receiving,
			json!({"source_node":sending,"atomic_transactions":false,"enabled":true,"expected_revision":0}),
		)
		.await;
	assert_eq!(status, 409, "creating twice answers a conflict: {again}");

	let (status, revised) = d
		.register(
			&alice,
			&alice_context,
			&alice_tenant,
			&receiving,
			json!({"source_node":sending,"atomic_transactions":true,"enabled":true,"expected_revision":1}),
		)
		.await;
	assert_eq!(status, 200, "{revised}");
	assert_eq!(revised["revision"], 2);
	assert_eq!(revised["atomic_transactions"], true);
	let (status, stale) = d
		.register(
			&alice,
			&alice_context,
			&alice_tenant,
			&receiving,
			json!({"source_node":sending,"atomic_transactions":true,"enabled":false,"expected_revision":1}),
		)
		.await;
	assert_eq!(status, 409, "a stale revision answers a conflict: {stale}");
	let (status, disabled) = d
		.register(
			&alice,
			&alice_context,
			&alice_tenant,
			&receiving,
			json!({"source_node":sending,"atomic_transactions":true,"enabled":false,"expected_revision":2}),
		)
		.await;
	assert_eq!(status, 200, "{disabled}");
	assert_eq!(disabled["enabled"], false);
	assert_eq!(disabled["revision"], 3);

	let (status, listed) = d
		.registrations(
			&alice,
			&alice_context,
			&alice_tenant,
			&receiving,
			"registrations",
		)
		.await;
	assert_eq!(status, 200, "{listed}");
	assert_eq!(listed.as_array().unwrap().len(), 1);
	let (status, history) = d
		.registrations(
			&alice,
			&alice_context,
			&alice_tenant,
			&receiving,
			"registration-history",
		)
		.await;
	assert_eq!(status, 200, "{history}");
	let revisions: Vec<_> = history
		.as_array()
		.unwrap()
		.iter()
		.map(|row| row["revision"].as_i64().unwrap())
		.collect();
	assert_eq!(revisions, [1, 2, 3]);
	assert!(history[0]["sequence"].as_i64().unwrap() < history[2]["sequence"].as_i64().unwrap());
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_registration_names_a_real_node_of_another_tenant_and_a_tenant_never_registers_its_own(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::new(runtime, open_rules()).await;
	let alice = d.person("alice").await;
	let bob = d.person("bob").await;
	let (alice_tenant, alice_context) = d.node_tenant(&alice, "Alice").await;
	let (bob_tenant, bob_context) = d.node_tenant(&bob, "Bob").await;
	let receiving = d.default_node(&alice, &alice_context, &alice_tenant).await;
	let own = d.address_of(&alice, &alice_context, &alice_tenant).await;
	let (status, second) = d
		.create_node(&alice, &alice_context, &alice_tenant, "Second")
		.await;
	assert_eq!(status, 201, "{second}");
	let deployment = d.federation.config.deployment.to_string();
	let _ = (bob_tenant, bob_context);

	for (what, source, expected) in [
		("itself", own.clone(), 400),
		(
			"a Node of its own Tenant",
			second["address"].as_str().unwrap().to_owned(),
			422,
		),
		(
			"a Node nobody holds",
			format!("{deployment}/unknown-node-00000000"),
			422,
		),
	] {
		let (status, body) = d
			.register(
				&alice,
				&alice_context,
				&alice_tenant,
				&receiving,
				json!({"source_node":source,"atomic_transactions":false,"enabled":true,"expected_revision":0}),
			)
			.await;
		assert_eq!(status, expected, "{what}: {body}");
	}
	let (status, body) = d
		.register(
			&alice,
			&alice_context,
			&alice_tenant,
			&receiving,
			json!({"source_node":own,"atomic_transactions":false,"enabled":false,"expected_revision":0}),
		)
		.await;
	assert_eq!(status, 400, "disabling nothing: {body}");
	assert_eq!(d.count("SELECT count(*) FROM node_registrations").await, 0);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn only_a_tenant_administrator_reads_or_writes_registrations(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::new(runtime, open_rules()).await;
	let alice = d.person("alice").await;
	let bob = d.person("bob").await;
	let carol = d.person("carol").await;
	let (tenant, context) = d.node_tenant(&alice, "Team").await;
	let (other_tenant, other_context) = d.node_tenant(&carol, "Carol").await;
	let member = d.admit(&tenant, &bob, "bob", json!(["members"])).await;
	let node = d.default_node(&alice, &context, &tenant).await;
	let source = d.address_of(&carol, &other_context, &other_tenant).await;
	let body = json!({"source_node":source,"atomic_transactions":false,"enabled":true,"expected_revision":0});

	let (status, refused) = d
		.register(&bob, &member, &tenant, &node, body.clone())
		.await;
	assert_eq!(status, 403, "{refused}");
	let (status, refused) = d
		.registrations(&bob, &member, &tenant, &node, "registrations")
		.await;
	assert_eq!(status, 403, "{refused}");
	let (status, refused) = d
		.registrations(&bob, &member, &tenant, &node, "registration-history")
		.await;
	assert_eq!(status, 403, "{refused}");
	let (status, refused) = d
		.register(&carol, &other_context, &tenant, &node, body.clone())
		.await;
	assert!(matches!(status, 403 | 404), "{status} {refused}");
	assert_eq!(d.count("SELECT count(*) FROM node_registrations").await, 0);

	let (status, administration) = d
		.as_person(
			&alice,
			"GET",
			&format!("/api/tenants/{tenant}/administration"),
			Some(&context),
			Value::Null,
		)
		.await;
	assert_eq!(status, 200, "{administration}");
	let actions = administration["actions"].as_array().unwrap();
	for action in ["node_registration.read", "node_registration.update"] {
		assert!(
			actions.iter().any(|held| held == action),
			"{administration}"
		);
	}
	let (status, created) = d.register(&alice, &context, &tenant, &node, body).await;
	assert_eq!(status, 200, "{created}");

	let (status, created) = d
		.as_deployment_administrator(
			"POST",
			&format!("/api/tenants/{tenant}/nodes/{node}/registrations"),
			json!({"source_node":source,"atomic_transactions":false,"enabled":true,"expected_revision":1}),
		)
		.await;
	assert_eq!(status, 200, "{created}");
	assert_eq!(created["actor"], "deployment_administrator");
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn registering_a_node_of_another_deployment_pins_the_key_it_publishes(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::new(runtime, open_rules()).await;
	let alice = d.person("alice").await;
	let (tenant, context) = d.node_tenant(&alice, "Team").await;
	let node = d.default_node(&alice, &context, &tenant).await;
	let (signer, id) = key(5);
	let (other_signer, other_id) = key(6);
	let endpoint = serve(identity(&signer, &id)).await;
	let source = "aidash://calm-heron/able-oak-abcd1234";

	let (status, created) = d
		.register(
			&alice,
			&context,
			&tenant,
			&node,
			json!({"source_node":source,"endpoint":format!("{endpoint}/"),"key_id":id.as_str(),"atomic_transactions":false,"enabled":true,"expected_revision":0}),
		)
		.await;
	assert_eq!(status, 200, "{created}");
	assert_eq!(created["endpoint"], endpoint, "a trailing slash is dropped");
	assert_eq!(created["key_id"], id.as_str());
	let pinned: String = sqlx::query_scalar("SELECT source_public_key_pem FROM node_registrations")
		.fetch_one(d.federation.store.pool.fixture_driver())
		.await
		.unwrap();
	assert_eq!(pinned, signer.public_key_pem());

	// The typed key ID is the anchor: another key ID is refused, and so is a
	// document whose key does not hash to the one typed.
	for (what, typed) in [("a key ID the document does not publish", other_id.as_str())] {
		let (status, refused) = d
			.register(
				&alice,
				&context,
				&tenant,
				&node,
				json!({"source_node":"aidash://calm-heron/calm-heron-e5f6g7h8","endpoint":endpoint,"key_id":typed,"atomic_transactions":false,"enabled":true,"expected_revision":0}),
			)
			.await;
		assert_eq!(status, 422, "{what}: {refused}");
	}
	let lying = serve(json!({
		"id": "aidash://calm-heron",
		"protocol_version": aidash_domain::federation::protocol::CURRENT,
		"key": {"id": other_id.as_str(), "public_key_pem": signer.public_key_pem()},
	}))
	.await;
	let (status, refused) = d
		.register(
			&alice,
			&context,
			&tenant,
			&node,
			json!({"source_node":"aidash://calm-heron/calm-heron-e5f6g7h8","endpoint":lying,"key_id":other_id.as_str(),"atomic_transactions":false,"enabled":true,"expected_revision":0}),
		)
		.await;
	assert_eq!(status, 422, "a key that does not hash to its ID: {refused}");
	let _ = other_signer;
	for missing in [
		json!({"source_node":source,"atomic_transactions":false,"enabled":true,"expected_revision":0}),
		json!({"source_node":source,"endpoint":endpoint,"atomic_transactions":false,"enabled":true,"expected_revision":0}),
		json!({"source_node":source,"endpoint":"http://peer.example.com","key_id":id.as_str(),"atomic_transactions":false,"enabled":true,"expected_revision":0}),
	] {
		let (status, refused) = d.register(&alice, &context, &tenant, &node, missing).await;
		assert_eq!(status, 400, "{refused}");
	}

	// Disabling keeps the pinned key and asks the other Deployment nothing.
	let (status, disabled) = d
		.register(
			&alice,
			&context,
			&tenant,
			&node,
			json!({"source_node":source,"atomic_transactions":false,"enabled":false,"expected_revision":1}),
		)
		.await;
	assert_eq!(status, 200, "{disabled}");
	assert_eq!(disabled["key_id"], id.as_str());
	assert_eq!(disabled["enabled"], false);
	d.finish().await;
}
