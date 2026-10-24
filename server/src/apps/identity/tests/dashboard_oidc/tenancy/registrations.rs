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
		.as_deployment_administrator(
			"POST",
			&registrations,
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
	// Browser installation administration permits these reads; its existing
	// request guard refuses Node mutations before they reach the service.
	for browser in [false, true]
		.into_iter()
		.filter(|browser| !browser || method == "GET")
	{
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
			requests.load(Ordering::SeqCst),
			before,
			"deleted Tenant administration must not contact another Deployment"
		);
		assert_eq!(
			status, 410,
			"browser={browser} purged={purged} {method} {path}: {body}"
		);
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
) {
	let d = Deployment::new(runtime, open_rules()).await;
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
	let (status, confirmed) = d.as_person(&carol, "POST", &route, Some("deployment_administrator"), json!({"source_node":source,"expected_keys_revision":4,"expected_authorization_revision":2,"action":"confirm","public_key_pem":new.public_key_pem(),"expected_key_id":new_id.as_str()})).await;
	assert_eq!(
		status, 200,
		"browser administrator retains record authority: {confirmed}"
	);
	assert_eq!(confirmed["enabled"], false);
	assert_eq!(d.count(&format!("SELECT count(*) FROM dashboard_administration_history WHERE tenant = '{tenant}' AND node = '{node}' AND action IN ('node_registration.key_quarantine','node_registration.key_confirm')")).await, 5);
	assert_eq!(d.count(&format!("SELECT count(*) FROM dashboard_administration_history WHERE tenant = '{tenant}' AND node = '{node}' AND action = 'node_registration.key_confirm' AND actor_identity_id = '{}' AND actor_kind = 'deployment_administrator'", carol.identity)).await, 1);
	d.finish().await;
}
