//! Node Registrations: which Nodes of other Tenants a Tenant's Node accepts
//! (#241), over HTTP and PostgreSQL.
use super::*;
use aidash_capability::InMemorySigner;
use aidash_domain::nodes::DeploymentKeyId;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// A loopback server that answers every request with `document` as JSON.
async fn serve(document: Value) -> (String, Arc<AtomicUsize>) {
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let requests = Arc::new(AtomicUsize::new(0));
	let observed = requests.clone();
	tokio::spawn(async move {
		loop {
			let Ok((mut stream, _)) = listener.accept().await else {
				return;
			};
			observed.fetch_add(1, Ordering::SeqCst);
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
	(endpoint, requests)
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
	pub(super) async fn default_node(
		&self,
		person: &Person,
		context: &str,
		tenant: &str,
	) -> String {
		self.nodes_of(person, context, tenant).await[0]["name"]
			.as_str()
			.unwrap()
			.to_owned()
	}

	pub(super) async fn address_of(&self, person: &Person, context: &str, tenant: &str) -> String {
		self.nodes_of(person, context, tenant).await[0]["address"]
			.as_str()
			.unwrap()
			.to_owned()
	}

	pub(super) async fn register(
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
#[case::list_nodes("GET", "")]
#[case::create_node("POST", "")]
#[case::relabel_node("PATCH", "/:node")]
#[case::list_registrations("GET", "/:node/registrations")]
#[case::registration_history("GET", "/:node/registration-history")]
#[case::register_node("POST", "/:node/registrations")]
#[tokio::test]
async fn deleted_tenants_refuse_node_administration_before_external_requests(
	#[case] method: &str,
	#[case] suffix: &str,
	#[values(false, true)] purged: bool,
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::new(runtime, open_rules()).await;
	let alice = d.person("alice").await;
	let carol = d.person("carol").await;
	sqlx::query("INSERT INTO dashboard_deployment_administrator_grants (identity_id) VALUES ($1)")
		.bind(carol.identity)
		.execute(d.federation.store.pool.fixture_driver())
		.await
		.unwrap();
	let (tenant, context) = d.node_tenant(&alice, "Deleted Node administration").await;
	let node = d.default_node(&alice, &context, &tenant).await;
	let (signer, key_id) = key(7);
	let (endpoint, requests) = serve(identity(&signer, &key_id)).await;
	d.federation
		.store
		.keyless
		.permit_origin_of(&endpoint)
		.unwrap();
	let registrations = format!("/api/tenants/{tenant}/nodes/{node}/registrations");
	let (status, body) = d
		.as_person(
			&alice,
			"POST",
			&registrations,
			Some(&context),
			json!({"source_node":"aidash://calm-heron/able-oak-abcd1234","endpoint":endpoint,"key_id":key_id.as_str(),"atomic_transactions":false,"enabled":true,"expected_revision":0}),
		)
		.await;
	assert_eq!(status, 200, "{body}");
	assert!(
		requests.load(Ordering::SeqCst) > 0,
		"the live registration contacts its counterpart"
	);
	let (status, body) = d
		.as_person(
			&alice,
			"POST",
			&format!("/api/tenants/{tenant}/deletion"),
			Some(&context),
			json!({"confirmation":tenant}),
		)
		.await;
	assert_eq!(status, 200, "{body}");
	if purged {
		let (status, body) = d
			.as_deployment_administrator(
				"POST",
				&format!("/api/tenants/{tenant}/purge"),
				Value::Null,
			)
			.await;
		assert_eq!(status, 200, "{body}");
		// The HTTP command starts the purge asynchronously; wait for its receipt.
		for _ in 0..100 {
			aidash_server::apps::identity::services::tenant_purge::sweep(&d.federation.store, 365)
				.await
				.unwrap();
			if d.count(&format!(
				"SELECT count(*) FROM tenant_deletions WHERE tenant = '{tenant}' AND purged_at IS NOT NULL"
			))
			.await == 1
			{
				break;
			}
			tokio::time::sleep(std::time::Duration::from_millis(100)).await;
		}
		assert_eq!(
			d.count(&format!(
				"SELECT count(*) FROM tenant_deletions WHERE tenant = '{tenant}' AND purged_at IS NOT NULL"
			))
			.await,
			1
		);
	}
	let before = requests.load(Ordering::SeqCst);
	let path = format!(
		"/api/tenants/{tenant}/nodes{}",
		suffix.replace(":node", &node)
	);
	let input = if suffix.ends_with("/registrations") && method == "POST" {
		// A different source has no cached pin: accepting it requires a new lookup.
		json!({"source_node":"aidash://calm-heron/bold-pine-1234abcd","endpoint":endpoint,"key_id":key_id.as_str(),"atomic_transactions":false,"enabled":true,"expected_revision":0})
	} else {
		json!({"label":"Deleted Tenant mutation"})
	};
	// A Deployment Administrator holds no Tenant authority over Nodes, so it is
	// refused before the Tenant's deletion is even reported; the Tenant's own
	// administrator learns the Tenant is deleted.
	for browser in [false, true] {
		let (status, body) = if browser {
			d.as_person(
				&carol,
				method,
				&path,
				Some("deployment_administrator"),
				input.clone(),
			)
			.await
		} else {
			d.as_deployment_administrator(method, &path, input.clone())
				.await
		};
		assert_eq!(
			status, 403,
			"browser={browser} purged={purged} {method} {path}: {body}"
		);
	}
	let (status, body) = d
		.as_person(&alice, method, &path, Some(&context), input.clone())
		.await;
	assert_eq!(
		requests.load(Ordering::SeqCst),
		before,
		"deleted Tenant administration must not contact another Deployment"
	);
	if purged {
		// A purge removes the Tenant's Mappings, so its own administrator has
		// no standing left and is refused outright.
		assert!(
			matches!(status, 403 | 410),
			"purged={purged} {method} {path}: {status} {body}"
		);
	} else {
		assert_eq!(status, 410, "purged={purged} {method} {path}: {body}");
		assert_eq!(body["error"], "tenant_deleted");
	}
	d.finish().await;
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

	let (status, refused) = d
		.as_deployment_administrator(
			"POST",
			&format!("/api/tenants/{tenant}/nodes/{node}/registrations"),
			json!({"source_node":source,"atomic_transactions":false,"enabled":true,"expected_revision":1}),
		)
		.await;
	// Node Registrations are Tenant settings: refused to a Deployment Administrator.
	assert_eq!(status, 403, "{refused}");
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
	let (endpoint, _) = serve(identity(&signer, &id)).await;
	d.federation
		.store
		.keyless
		.permit_origin_of(&endpoint)
		.unwrap();
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
	let (lying, _) = serve(json!({
		"id": "aidash://calm-heron",
		"protocol_version": aidash_domain::federation::protocol::CURRENT,
		"key": {"id": other_id.as_str(), "public_key_pem": signer.public_key_pem()},
	}))
	.await;
	d.federation.store.keyless.permit_origin_of(&lying).unwrap();
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
	] {
		let (status, refused) = d.register(&alice, &context, &tenant, &node, missing).await;
		assert_eq!(status, 400, "{refused}");
	}
	// A loopback endpoint nobody permitted is refused with the one generic reason.
	let (unpermitted, _) = serve(identity(&signer, &id)).await;
	let (status, refused) = d
		.register(
			&alice,
			&context,
			&tenant,
			&node,
			json!({"source_node":"aidash://calm-heron/calm-heron-e5f6g7h8","endpoint":unpermitted,"key_id":id.as_str(),"atomic_transactions":false,"enabled":true,"expected_revision":0}),
		)
		.await;
	assert_eq!(status, 422, "{refused}");
	let (_, closed) = d
		.register(
			&alice,
			&context,
			&tenant,
			&node,
			json!({"source_node":"aidash://calm-heron/calm-heron-e5f6g7h8","endpoint":"https://127.0.0.1:1","key_id":id.as_str(),"atomic_transactions":false,"enabled":true,"expected_revision":0}),
		)
		.await;
	assert_eq!(
		refused["error"], closed["error"],
		"a refused host and a closed port answer alike"
	);

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

#[rstest::rstest]
#[tokio::test]
async fn manual_registration_keys_require_record_authority_and_independent_confirmation(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
	#[from(common::native_peer)]
	#[with("aidash://retained-peer")]
	peer: common::PeerFuture,
) {
	let mut d = Deployment::new(runtime, open_rules()).await;
	let alice = d.person("key-owner").await;
	let bob = d.person("key-member").await;
	let eve = d.person("other-owner").await;
	let (tenant, context) = d.node_tenant(&alice, "Keys").await;
	let node = d.default_node(&alice, &context, &tenant).await;
	let member = d.admit(&tenant, &bob, "member", json!([])).await;
	let (_, foreign) = d.node_tenant(&eve, "Other keys").await;
	let (old, old_id) = key(7);
	let (new, new_id) = key(9);
	let (endpoint, requests) = serve(identity(&old, &old_id)).await;
	d.federation
		.store
		.keyless
		.permit_origin_of(&endpoint)
		.unwrap();
	let source = "aidash://calm-heron/able-oak-abcd1234";
	let (status, created) = d.register(&alice, &context, &tenant, &node,
        json!({"source_node":source,"endpoint":endpoint,"key_id":old_id.as_str(),"atomic_transactions":true,"enabled":true,"expected_revision":0})).await;
	assert_eq!(status, 200, "{created}");
	let route = format!("/api/tenants/{tenant}/nodes/{node}/registration-keys");
	let quarantine = json!({"source_node":source,"expected_keys_revision":1,"expected_authorization_revision":1,"action":"quarantine"});
	let (status, state) = d
		.as_person(&alice, "POST", &route, Some(&context), quarantine.clone())
		.await;
	assert_eq!(status, 200, "{state}");
	assert_eq!(state["quarantined"], true);
	assert_eq!(state["keys_revision"], 2);
	assert_eq!(state["authorization_revision"], 1);
	for (person, acting) in [(&bob, &member), (&eve, &foreign)] {
		let (status, body) = d
			.as_person(person, "POST", &route, Some(acting), quarantine.clone())
			.await;
		assert_eq!(status, 403, "{body}");
		assert_eq!(
			d.as_person(
				person,
				"GET",
				&format!("{route}?source_node={source}"),
				Some(acting),
				Value::Null
			)
			.await
			.0,
			403
		);
	}
	let (status, current) = d
		.as_person(
			&alice,
			"GET",
			&format!("{route}?source_node={source}"),
			Some(&context),
			Value::Null,
		)
		.await;
	assert_eq!(status, 200, "{current}");
	assert_eq!(current, state);

	let (status, edited) = d
		.register(
			&alice,
			&context,
			&tenant,
			&node,
			json!({"source_node":source,"atomic_transactions":false,"enabled":false,"expected_revision":1}),
		)
		.await;
	assert_eq!(status, 200, "{edited}");
	let before = requests.load(Ordering::SeqCst);
	let (status, _) = d.register(&alice, &context, &tenant, &node,
        json!({"source_node":source,"endpoint":endpoint,"key_id":new_id.as_str(),"atomic_transactions":true,"enabled":true,"expected_revision":2})).await;
	assert_eq!(
		status, 409,
		"generic replacement cannot bypass independent confirmation"
	);
	let mut confirm = json!({"source_node":source,"expected_keys_revision":2,"expected_authorization_revision":2,"action":"confirm","public_key_pem":new.public_key_pem(),"expected_key_id":old_id.as_str()});
	let (status, _) = d
		.as_person(&alice, "POST", &route, Some(&context), confirm.clone())
		.await;
	assert_eq!(status, 400);
	confirm["expected_key_id"] = json!(new_id.as_str());
	confirm["expected_authorization_revision"] = json!(1);
	let (status, _) = d
		.as_person(&alice, "POST", &route, Some(&context), confirm.clone())
		.await;
	assert_eq!(status, 409);
	confirm["expected_authorization_revision"] = json!(2);
	let (status, state) = d
		.as_person(&alice, "POST", &route, Some(&context), confirm.clone())
		.await;
	assert_eq!(status, 200, "{state}");
	assert_eq!(state["key_id"], new_id.as_str());
	assert_eq!(state["keys_revision"], 3);
	assert_eq!(state["authorization_revision"], 2);
	assert_eq!(state["quarantined"], false);
	assert_eq!(
		state["enabled"], false,
		"confirmation does not re-enable a registration"
	);
	assert_eq!(
		requests.load(Ordering::SeqCst),
		before,
		"manual confirmation performs no discovery fetch"
	);
	let (status, _) = d
		.as_person(&alice, "POST", &route, Some(&context), confirm)
		.await;
	assert_eq!(status, 409, "a stale crypto snapshot cannot write twice");
	let (_, rows) = d
		.registrations(&alice, &context, &tenant, &node, "registrations")
		.await;
	assert_eq!(rows[0]["revision"], 2);
	assert_eq!(rows[0]["atomic_transactions"], false);
	assert_eq!(rows[0]["enabled"], false);
	assert_eq!(rows[0]["key_id"], new_id.as_str());
	assert_eq!(rows[0]["updated_at"], edited["updated_at"]);
	assert_eq!(rows[0]["actor"], edited["actor"]);
	{
		// The same-key/same-endpoint path changes real permission state even if
		// independent confirmation replaces its repeated pin while it waits.
		let racing_source = "aidash://calm-heron/bold-pine-1234abcd";
		let (status, initial) = d.register(&alice, &context, &tenant, &node,
			json!({"source_node":racing_source,"endpoint":endpoint,"key_id":old_id.as_str(),"atomic_transactions":false,"enabled":true,"expected_revision":0})).await;
		assert_eq!(status, 200, "{initial}");
		assert_eq!(initial["atomic_transactions"], false);
		assert_eq!(initial["revision"], 1);
		let mut blocker = d
			.federation
			.store
			.pool
			.fixture_driver()
			.begin()
			.await
			.unwrap();
		let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
			.fetch_one(&mut *blocker)
			.await
			.unwrap();
		sqlx::query(
			"SELECT 1 FROM node_registrations WHERE tenant=$1 AND node=$2 AND source_node=$3 FOR UPDATE",
		)
		.bind(&tenant)
		.bind(&node)
		.bind(racing_source)
		.fetch_one(&mut *blocker)
		.await
		.unwrap();
		let manual = d.as_person(&alice, "POST", &route, Some(&context),
			json!({"source_node":racing_source,"expected_keys_revision":1,"expected_authorization_revision":1,"action":"confirm","public_key_pem":new.public_key_pem(),"expected_key_id":new_id.as_str()}));
		tokio::pin!(manual);
		tokio::select! {
			result = &mut manual => panic!("manual confirmation did not wait for its row: {result:?}"),
			() = common::wait_for_blocked(&d.federation.store.pool, pid, 1) => {},
		}
		let registration_path = format!("/api/tenants/{tenant}/nodes/{node}/registrations");
		// Separate authorized actors avoid a browser-session lock preceding the
		// ordinary request's initial A read and final row-lock wait.
		let permission = d.as_deployment_administrator("POST", &registration_path,
			json!({"source_node":racing_source,"endpoint":endpoint,"key_id":old_id.as_str(),"atomic_transactions":true,"enabled":true,"expected_revision":1}));
		tokio::pin!(permission);
		tokio::select! {
			result = &mut manual => panic!("manual confirmation escaped its row lock: {result:?}"),
			result = &mut permission => panic!("permission edit did not wait behind confirmation: {result:?}"),
			() = common::wait_for_blocked(&d.federation.store.pool, pid, 2) => {},
		}
		blocker.commit().await.unwrap();
		let ((manual_status, confirmed), (permission_status, edited)) =
			tokio::join!(manual, permission);
		assert_eq!(manual_status, 200, "{confirmed}");
		assert_eq!(confirmed["keys_revision"], 2);
		assert_eq!(confirmed["authorization_revision"], 1);
		assert_eq!(permission_status, 200, "{edited}");
		assert_eq!(edited["revision"], 2);
		assert_eq!(
			edited["atomic_transactions"], true,
			"false -> true is an actual permission change"
		);
		assert_eq!(edited["key_id"], new_id.as_str());
		assert_eq!(edited["endpoint"], endpoint);
		let (encoded, keys_revision, key_id, public_pem, stored_endpoint, authorization_revision, atomic_transactions, enabled):
			(String, i64, String, String, String, i64, bool, bool) = sqlx::query_as(
			"SELECT source_keys,keys_revision,source_key_id,source_public_key_pem,source_endpoint,revision,atomic_transactions,enabled FROM node_registrations WHERE tenant=$1 AND node=$2 AND source_node=$3")
			.bind(&tenant).bind(&node).bind(racing_source).fetch_one(d.federation.store.pool.fixture_driver()).await.unwrap();
		assert_eq!(authorization_revision, 2);
		assert!(atomic_transactions);
		assert!(enabled);
		assert_eq!(keys_revision, 2);
		assert_eq!(key_id, new_id.as_str());
		assert_eq!(public_pem, new.public_key_pem());
		assert_eq!(stored_endpoint, endpoint);
		assert_eq!(
			serde_json::from_str::<Value>(&encoded).unwrap(),
			json!({
				"current":{"key_id":new_id.as_str(),"public_key":new.public_key().to_bytes()},
				"retiring":null,"quarantined":false,
			})
		);
	}
	// A genuine second Deployment and completed atomic journal, not the
	// well-known-only stand-in above, require this exact pair after purge.
	let peer = peer.await;
	let remote = &peer.runtime.federation;
	common::bootstrap(remote, &peer.application, "http://localhost:1").await;
	d.federation.config.endpoint = d.app.url("");
	d.app.context.set_singleton(d.federation.clone());
	common::register_pair(
		(&d.federation, &d.app, &tenant),
		(remote, &peer.application, "acme"),
	)
	.await;
	let local_address = d
		.federation
		.config
		.deployment
		.node(aidash_domain::nodes::NodeName::parse(&node).unwrap())
		.to_string();
	let retained_source = common::default_node(&remote.store, "acme")
		.await
		.to_string();
	let (status, workspace) = d
		.as_person(
			&alice,
			"POST",
			"/api/workspaces",
			Some(&context),
			json!({"title":"Retained receiver", "goal":"Real atomic obligation", "node":node}),
		)
		.await;
	assert_eq!(status, 200, "{workspace}");
	let workspace_id = Uuid::parse_str(workspace["id"].as_str().unwrap()).unwrap();
	let remote_workspace = remote
		.store
		.create_workspace(
			&aidash_application::tenancy::TenancyScope::tenant("acme").unwrap(),
			"alice",
			"Retained counterpart",
			"Real atomic obligation",
		)
		.await
		.unwrap();
	let mut participants = vec![
		json!({"node_id":local_address,"mutations":[{"kind":"workspace_state","workspace_id":workspace_id,"expected_revision":0,"state":{"committed":true}}]}),
		json!({"node_id":retained_source,"mutations":[{"kind":"workspace_state","workspace_id":remote_workspace.id,"expected_revision":0,"state":{"committed":true}}]}),
	];
	participants.sort_by(|a, b| a["node_id"].as_str().cmp(&b["node_id"].as_str()));
	let manifest: aidash_domain::transactions::Manifest = serde_json::from_value(json!({
		"id":Uuid::new_v4(),"coordinator":retained_source,"isolation":"serializable",
		"deadline":chrono::Utc::now()+chrono::Duration::minutes(5),"participants":participants,
	}))
	.unwrap();
	use aidash_server::transactions::coordinator;
	coordinator::submit(remote, &manifest).await.unwrap();
	for _ in 0..20 {
		if coordinator::status(remote, manifest.id)
			.await
			.unwrap()
			.complete
		{
			break;
		}
		coordinator::advance(remote, manifest.id).await.unwrap();
	}
	let terminal = coordinator::status(remote, manifest.id).await.unwrap();
	assert!(terminal.complete, "{terminal:?}");
	assert_eq!(terminal.decision.as_deref(), Some("COMMIT"));
	assert_eq!(
		remote
			.store
			.workspace(&TEST_FIXTURE, remote_workspace.id)
			.await
			.unwrap()
			.revision,
		1
	);
	let (status, disabled) = d.register(&alice, &context, &tenant, &node,
		json!({"source_node":retained_source,"atomic_transactions":false,"enabled":false,"expected_revision":1})).await;
	assert_eq!(status, 200, "{disabled}");
	assert_eq!(disabled["revision"], 2);
	let retained_get = format!("{route}?source_node={retained_source}");
	assert_eq!(
		d.as_person(&alice, "GET", &retained_get, Some(&context), Value::Null)
			.await
			.0,
		200
	);

	let (status, second) = d
		.as_person(
			&alice,
			"POST",
			&format!("/api/tenants/{tenant}/nodes"),
			Some(&context),
			json!({"label":"New default"}),
		)
		.await;
	assert_eq!(status, 201, "{second}");
	let second = second["name"].as_str().unwrap();
	assert_eq!(
		d.as_person(
			&alice,
			"POST",
			&format!("/api/tenants/{tenant}/nodes/{second}/default"),
			Some(&context),
			Value::Null
		)
		.await
		.0,
		200
	);
	assert_eq!(
		d.as_person(
			&alice,
			"POST",
			&format!("/api/tenants/{tenant}/nodes/{node}/deletion"),
			Some(&context),
			json!({"confirmation":node})
		)
		.await
		.0,
		200
	);
	let (status, deleted) = d.as_person(&alice, "POST", &route, Some(&context), json!({"source_node":source,"expected_keys_revision":3,"expected_authorization_revision":2,"action":"quarantine"})).await;
	assert_eq!(
		status, 200,
		"catalog-backed deleted Node trust remains manageable: {deleted}"
	);
	assert_eq!(deleted["quarantined"], true);
	assert_eq!(deleted["enabled"], false);
	assert_eq!(deleted["authorization_revision"], 2);
	let carol = d.person("key-deployment-administrator").await;
	sqlx::query("INSERT INTO dashboard_deployment_administrator_grants (identity_id) VALUES ($1)")
		.bind(carol.identity)
		.execute(d.federation.store.pool.driver())
		.await
		.unwrap();
	let confirm = json!({"source_node":source,"expected_keys_revision":4,"expected_authorization_revision":2,"action":"confirm","public_key_pem":new.public_key_pem(),"expected_key_id":new_id.as_str()});
	// Registration records are Tenant administration (#484, ADR 0102): the
	// browser Deployment Administrator is refused and nothing is recorded.
	let (status, refused) = d
		.as_person(
			&carol,
			"POST",
			&route,
			Some("deployment_administrator"),
			confirm.clone(),
		)
		.await;
	assert_eq!(status, 403, "{refused}");
	assert_eq!(d.count(&format!("SELECT count(*) FROM dashboard_administration_history WHERE tenant = '{tenant}' AND node = '{node}' AND actor_kind = 'deployment_administrator' AND action LIKE 'node_registration.key_%'")).await, 0);
	let (status, confirmed) = d
		.as_person(&alice, "POST", &route, Some(&context), confirm)
		.await;
	assert_eq!(
		status, 200,
		"the Tenant Administrator retains record authority: {confirmed}"
	);
	assert_eq!(confirmed["enabled"], false);
	assert_eq!(d.count(&format!("SELECT count(*) FROM dashboard_administration_history WHERE tenant = '{tenant}' AND node = '{node}' AND action IN ('node_registration.key_quarantine','node_registration.key_confirm')")).await, 5);
	// Public purge followed by the real worker and finite catalog/all-cycle audit
	// expiry physically removes the Node while its replayable participant remains.
	use aidash_server::apps::identity::services::node_purge;
	let (status, requested) = d
		.as_deployment_administrator(
			"POST",
			&format!("/api/tenants/{tenant}/nodes/{node}/purge"),
			Value::Null,
		)
		.await;
	assert_eq!(status, 200, "{requested}");
	let deletion = node_purge::claim(&d.federation.store)
		.await
		.unwrap()
		.into_iter()
		.find(|row| row.node == node)
		.unwrap();
	node_purge::purge_node(&d.federation.store, deletion)
		.await
		.unwrap();
	sqlx::query("UPDATE node_deletions SET purged_at=clock_timestamp()-interval '366 days' WHERE tenant=$1 AND node=$2 AND restored_at IS NULL")
		.bind(&tenant).bind(&node).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
	node_purge::erase_audit(&d.federation.store, 365)
		.await
		.unwrap();
	for table in [
		"nodes",
		"node_deletions",
		"dashboard_administration_history",
		"authorization_catalog_history",
	] {
		let column = if table == "nodes" { "name" } else { "node" };
		assert_eq!(
			d.count(&format!(
				"SELECT count(*) FROM {table} WHERE tenant='{tenant}' AND {column}='{node}'"
			))
			.await,
			0,
			"finite {table} must expire"
		);
	}
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM workspaces WHERE id='{workspace_id}'"
		))
		.await,
		0,
		"actual Node payload was purged"
	);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM node_names WHERE tenant='{tenant}' AND name='{node}'"
		))
		.await,
		1
	);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM atomic_participants WHERE id='{}' AND phase='COMMITTED'",
			manifest.id
		))
		.await,
		1
	);
	async fn retained_row(d: &Deployment, tenant: &str, node: &str, source: &str) -> Value {
		sqlx::query_scalar(
			"SELECT to_jsonb(r) FROM node_registrations r WHERE tenant=$1 AND node=$2 AND source_node=$3",
		)
		.bind(tenant)
		.bind(node)
		.bind(source)
		.fetch_one(d.federation.store.pool.fixture_driver())
		.await
		.unwrap()
	}
	let original = retained_row(&d, &tenant, &node, &retained_source).await;
	let (status, retained) = d
		.as_person(&alice, "GET", &retained_get, Some(&context), Value::Null)
		.await;
	assert_eq!(
		status, 200,
		"current TA must read exact retained trust without catalog: {retained}"
	);
	assert_eq!(retained["keys_revision"], 1);
	assert_eq!(retained["authorization_revision"], 2);
	assert_eq!(
		d.as_person(
			&carol,
			"GET",
			&retained_get,
			Some("deployment_administrator"),
			Value::Null
		)
		.await
		.0,
		200
	);
	// Each current administrator exercises both operations. Confirmation changes
	// only crypto and never enables the disabled atomic permission/registration.
	for (person, acting, revision, action) in [
		(&alice, context.as_str(), 1, "quarantine"),
		(&carol, "deployment_administrator", 2, "confirm"),
		(&carol, "deployment_administrator", 3, "quarantine"),
		(&alice, context.as_str(), 4, "confirm"),
	] {
		let mut input = json!({"source_node":retained_source,"expected_keys_revision":revision,"expected_authorization_revision":2,"action":action});
		if action == "confirm" {
			input["expected_key_id"] = json!(new_id.as_str());
			input["public_key_pem"] = json!(new.public_key_pem());
		}
		let (status, state) = d
			.as_person(person, "POST", &route, Some(acting), input)
			.await;
		assert_eq!(status, 200, "{action} after catalog expiry: {state}");
		assert_eq!(state["keys_revision"], revision + 1);
		assert_eq!(state["authorization_revision"], 2);
		assert_eq!(state["enabled"], false);
		assert_eq!(state["quarantined"], action == "quarantine");
	}
	let current = retained_row(&d, &tenant, &node, &retained_source).await;
	let mut authority_before = original.clone();
	let mut authority_after = current.clone();
	for field in [
		"source_keys",
		"keys_revision",
		"source_key_id",
		"source_public_key_pem",
	] {
		authority_before.as_object_mut().unwrap().remove(field);
		authority_after.as_object_mut().unwrap().remove(field);
	}
	assert_eq!(
		authority_after, authority_before,
		"all authorization/endpoint/actor/timestamp metadata remains unchanged"
	);
	assert_eq!(current["keys_revision"], 5);
	assert_eq!(current["source_key_id"], new_id.as_str());
	assert_eq!(current["source_public_key_pem"], new.public_key_pem());
	let quarantine = json!({"source_node":retained_source,"expected_keys_revision":5,"expected_authorization_revision":2,"action":"quarantine"});
	for (person, acting) in [(&bob, member.as_str()), (&eve, foreign.as_str())] {
		assert_eq!(
			d.as_person(person, "GET", &retained_get, Some(acting), Value::Null)
				.await
				.0,
			403
		);
		assert_eq!(
			d.as_person(person, "POST", &route, Some(acting), quarantine.clone())
				.await
				.0,
			403
		);
	}
	let wrong_tenant = foreign.split(':').nth(1).unwrap();
	let unknown_node =
		aidash_domain::nodes::NodeName::derived("unknown-retained-owner").to_string();
	for (candidate_route, candidate_source) in [
		(
			format!("/api/tenants/{wrong_tenant}/nodes/{node}/registration-keys"),
			retained_source.as_str(),
		),
		(
			format!("/api/tenants/{tenant}/nodes/{unknown_node}/registration-keys"),
			retained_source.as_str(),
		),
		(route.clone(), source), // helper has one real source, never this unrequired pair
	] {
		let (status, body) = d
			.as_deployment_administrator(
				"GET",
				&format!("{candidate_route}?source_node={candidate_source}"),
				Value::Null,
			)
			.await;
		assert_eq!(status, 404, "wrong exact tuple: {body}");
		let mut change = quarantine.clone();
		change["source_node"] = json!(candidate_source);
		assert_eq!(
			d.as_deployment_administrator("POST", &candidate_route, change)
				.await
				.0,
			404
		);
	}
	for (key_revision, authorization_revision) in [(4, 2), (5, 1)] {
		let mut stale = quarantine.clone();
		stale["expected_keys_revision"] = json!(key_revision);
		stale["expected_authorization_revision"] = json!(authorization_revision);
		assert_eq!(
			d.as_deployment_administrator("POST", &route, stale).await.0,
			409
		);
	}
	// A malformed actual journal is an integrity denial, not history authority.
	sqlx::query("UPDATE atomic_participants SET digest='corrupt' WHERE id=$1")
		.bind(manifest.id)
		.execute(d.federation.store.pool.fixture_driver())
		.await
		.unwrap();
	assert_eq!(
		d.as_deployment_administrator("GET", &retained_get, Value::Null)
			.await
			.0,
		409
	);
	assert_eq!(
		d.as_deployment_administrator("POST", &route, quarantine.clone())
			.await
			.0,
		409
	);
	sqlx::query("UPDATE atomic_participants SET digest=$2 WHERE id=$1")
		.bind(manifest.id)
		.bind(manifest.digest().unwrap())
		.execute(d.federation.store.pool.fixture_driver())
		.await
		.unwrap();
	assert_eq!(
		retained_row(&d, &tenant, &node, &retained_source).await,
		current,
		"denials cannot mutate current trust"
	);
	// Reopen a fresh Store and production application over durable state.
	let pool = d
		.federation
		.store
		.pool
		.options()
		.clone()
		.connect_with(d.federation.store.pool.connect_options().as_ref().clone())
		.await
		.unwrap();
	let mut store =
		aidash_server::store::Store::from_pool(pool, d.federation.config.deployment.clone())
			.await
			.unwrap();
	store.egress = d.federation.store.egress.clone();
	store.keyless = d.federation.store.keyless.clone();
	let old_pool = d.federation.store.pool.clone();
	let old_control = d.federation.store.control_pool.clone();
	d.federation.store = store;
	d.app = common::application(d.federation.clone()).await;
	old_pool.close().await;
	old_control.close().await;
	assert_eq!(
		retained_row(&d, &tenant, &node, &retained_source).await,
		current
	);
	assert_eq!(
		d.as_person(&alice, "GET", &retained_get, Some(&context), Value::Null)
			.await
			.1,
		json!({"authorization_revision":2,"enabled":false,"key_id":new_id.as_str(),"keys_revision":5,"quarantined":false})
	);
	let mapping = context.rsplit(':').next().unwrap();
	assert_eq!(
		d.as_deployment_administrator(
			"POST",
			&format!("/api/dashboard/mappings/{mapping}/disable"),
			json!({"expected_revision":1})
		)
		.await
		.0,
		204
	);
	assert_eq!(
		d.as_person(&alice, "GET", &retained_get, Some(&context), Value::Null)
			.await
			.0,
		403
	);
	assert_eq!(
		d.as_person(&alice, "POST", &route, Some(&context), quarantine.clone())
			.await
			.0,
		403
	);
	sqlx::query("DELETE FROM dashboard_deployment_administrator_grants WHERE identity_id=$1")
		.bind(carol.identity)
		.execute(d.federation.store.pool.fixture_driver())
		.await
		.unwrap();
	assert_eq!(
		d.as_person(
			&carol,
			"GET",
			&retained_get,
			Some("deployment_administrator"),
			Value::Null
		)
		.await
		.0,
		403
	);
	assert_eq!(
		d.as_person(
			&carol,
			"POST",
			&route,
			Some("deployment_administrator"),
			quarantine.clone()
		)
		.await
		.0,
		403
	);
	assert_eq!(
		retained_row(&d, &tenant, &node, &retained_source).await,
		current
	);
	let (status, deleted_tenant) = d
		.as_deployment_administrator(
			"POST",
			&format!("/api/tenants/{tenant}/deletion"),
			json!({"confirmation":tenant}),
		)
		.await;
	assert_eq!(status, 200, "{deleted_tenant}");
	assert_eq!(
		d.as_deployment_administrator("GET", &retained_get, Value::Null)
			.await
			.0,
		410
	);
	assert_eq!(
		d.as_deployment_administrator("POST", &route, quarantine)
			.await
			.0,
		410
	);
	assert_eq!(
		retained_row(&d, &tenant, &node, &retained_source).await,
		current
	);
	peer.server.abort();
	common::cleanup(
		peer.runtime.federation.clone(),
		&peer.runtime.url,
		&peer.runtime.schema,
	)
	.await;
	d.finish().await;
}
