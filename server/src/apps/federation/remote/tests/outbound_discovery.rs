//! Outbound discovery uses both nodes' current authority over native HTTP.
#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
#[path = "../../../execution/tests/support/endpoint.rs"]
mod endpoint;
#[path = "../../../execution/tests/support/execution.rs"]
mod execution_fixtures;
#[path = "../../../execution/tests/support/native_database.rs"]
mod native_database;
#[path = "../../../execution/tests/support/federation.rs"]
mod peering;
// The successful provider fixture also serves other adapter test variants.
mod discovery_fixtures;
#[allow(dead_code)]
#[path = "../../../execution/tests/provider_fixtures.rs"]
mod provider_fixtures;

use aidash_server::apps::identity::models::AuthorizationCredential;
use discovery_fixtures::{DiscoveryPair, MockDiscovery, discovery_pair, mock_discovery};
use endpoint::{EndpointFixture, assert_json};
use reinhardt::db::orm::Model;
use reinhardt::test::APIClient;
use rstest::rstest;
use serde_json::{Value, json};
use std::{sync::atomic::Ordering, time::Duration};
use tokio::time::timeout;

async fn discover(client: &APIClient, search: Value) -> Value {
	assert_json(
		client.post("/api/discover", &search, "json").await.unwrap(),
		200,
	)
}

#[rstest]
#[case::label("/label", json!("Default"))]
#[case::default("/default", json!(true))]
#[tokio::test]
async fn discovery_uses_conditional_node_attributes(
	#[future]
	#[from(execution_fixtures::execution)]
	execution: execution_fixtures::ExecutionFixture,
	#[case] attribute: &str,
	#[case] expected: Value,
) {
	let fixture = Box::pin(execution).await;
	let node = endpoint::default_node(&fixture.app.runtime.store, "acme").await;
	let mut policy = fixture.policy.clone();
	policy["policies"] = json!([
		{"id":"catalog-read","effect":"allow","subjects":{"any":true},"actions":["registry.read"],"resources":{"kinds":["*"]}},
		{"id":"conditional-node","effect":"allow","subjects":{"any":true},"actions":["node.use"],"resources":{"kinds":["node"],"ids":[node.node]},
		 "condition":{"op":"eq","left":{"source":"resource","path":attribute},"right":{"source":"literal","value":expected}}},
	]);
	deployment_administrator(
		&fixture.app,
		"/api/authorization/acme",
		json!({"expected_revision":1,"bundle":policy}),
	)
	.await;
	let discovered = discover(&fixture.subject, json!({})).await;
	assert_eq!(discovered["errors"], json!([]), "{discovered}");
	assert!(
		!discovered["agents"].as_array().unwrap().is_empty(),
		"{discovered}"
	);
}

#[rstest]
#[case::peers(false)]
#[case::expand(true)]
#[tokio::test]
async fn outbound_graph_requires_use_of_its_sending_node(
	#[future] discovery_pair: DiscoveryPair,
	#[case] expand: bool,
) {
	let pair = Box::pin(discovery_pair).await;
	let source = endpoint::default_node(&pair.source_runtime.store, "acme").await;
	let target = endpoint::default_node(&pair.destination_runtime.store, "acme").await;
	let issued = deployment_administrator(
		&pair.destination.app,
		"/api/authorization/acme/credentials",
		json!({"subject":"alice"}),
	)
	.await;
	deployment_administrator(
		&pair.destination.app,
		"/api/authorization/acme/peer-mappings",
		json!({
			"node":target.node,"source_node":source,"source_subject":"alice",
			"credential_id":issued["credential"]["id"],"expected_revision":0,"enabled":true,
		}),
	)
	.await;
	let path = format!("/api/federation/graph/peers?node={}", source.node);
	let input = json!({"node_id":target,"sending_node":source.node,"scope_workspace":null,"mode":"mesh","kinds":["workspace"],"relations":[],"hours":0,"limit":80,"cursor":null,"target_tenant":null});
	for denied in [false, true] {
		if denied {
			let mut policy = pair.source.policy.clone();
			policy["policies"].as_array_mut().unwrap().push(json!({
				"id":"no-sender-use","effect":"deny","subjects":{"any":true},
				"actions":["node.use"],"resources":{"kinds":["node"],"ids":[source.node]},
			}));
			deployment_administrator(
				&pair.source.app,
				"/api/authorization/acme",
				json!({"expected_revision":1,"bundle":policy}),
			)
			.await;
		}
		let response = if expand {
			pair.source
				.subject
				.post("/api/federation/graph", &input, "json")
				.await
				.unwrap()
		} else {
			pair.source.subject.get(&path).await.unwrap()
		};
		assert_json(response, if denied { 403 } else { 200 });
	}
}

#[rstest]
#[tokio::test]
async fn discovery_requires_node_use_even_without_any_routes(
	#[future]
	#[from(execution_fixtures::execution)]
	execution: execution_fixtures::ExecutionFixture,
) {
	// Arrange
	let fixture = Box::pin(execution).await;
	let mut policy = fixture.policy.clone();
	policy["policies"].as_array_mut().unwrap().push(json!({
		"id":"no-node-use","effect":"deny","subjects":{"any":true},
		"actions":["node.use"],"resources":{"kinds":["node"]},
	}));
	deployment_administrator(
		&fixture.app,
		"/api/authorization/acme",
		json!({"expected_revision":1,"bundle":policy}),
	)
	.await;
	// Act
	let response = fixture
		.subject
		.post("/api/discover", &json!({}), "json")
		.await
		.unwrap();
	// Assert
	assert_json(response, 403);
}

#[rstest]
#[tokio::test]
async fn sibling_discovery_uses_the_receiving_nodes_catalog_and_policy_resource(
	#[future]
	#[from(execution_fixtures::execution)]
	execution: execution_fixtures::ExecutionFixture,
) {
	// Arrange
	let fixture = Box::pin(execution).await;
	let app = &fixture.app;
	let (status, second) = endpoint::browser_mapping(app, "acme", "alice")
		.await
		.send("POST", "/api/tenants/acme/nodes", json!({"label":"Second"}))
		.await;
	assert_eq!(status, 201, "{second}");
	let name = aidash_domain::nodes::NodeName::parse(second["name"].as_str().unwrap()).unwrap();
	let authorization = aidash_server::authorization::Authorization {
		pool: app.runtime.store.pool.clone(),
	};
	for id in ["model", "http", "research"] {
		authorization
			.set_catalog(
				"acme",
				&name,
				&aidash_server::registry::EntityRef {
					id: id.into(),
					version: "1.0.0".into(),
				},
				0,
				true,
				"deployment_administrator",
			)
			.await
			.unwrap();
	}
	authorization
		.set_catalog(
			"acme",
			&aidash_domain::nodes::NodeName::derived("acme"),
			&aidash_server::registry::EntityRef {
				id: "research".into(),
				version: "1.0.0".into(),
			},
			1,
			false,
			"deployment_administrator",
		)
		.await
		.unwrap();
	// Act: only the sibling has an approved Agent.
	let discovered = discover(&fixture.subject, json!({})).await;
	// Assert
	assert_eq!(discovered["errors"], json!([]), "{discovered}");
	assert_eq!(
		discovered["agents"].as_array().unwrap().len(),
		1,
		"{discovered}"
	);
	assert_eq!(discovered["agents"][0]["node_id"], second["address"]);

	// A deployment-wide ID must not replace the receiving Node in a decision.
	let mut policy = fixture.policy.clone();
	policy["policies"].as_array_mut().unwrap().push(json!({
		"id":"deployment-is-not-a-node","effect":"deny","subjects":{"any":true},
		"actions":["federation.discover"],"resources":{"kinds":["node"],"ids":[app.runtime.config.deployment.as_str()]},
	}));
	policy["policies"].as_array_mut().unwrap().push(json!({
		"id":"receiving-node-label","effect":"deny","subjects":{"any":true},
		"actions":["federation.discover"],"resources":{"kinds":["node"],"ids":[name.as_str()]},
		"condition":{"op":"not_eq","left":{"source":"resource","path":"/label"},"right":{"source":"literal","value":"Second"}},
	}));
	deployment_administrator(
		app,
		"/api/authorization/acme",
		json!({"expected_revision":1,"bundle":policy}),
	)
	.await;
	let allowed = discover(&fixture.subject, json!({})).await;
	assert_eq!(allowed["agents"], discovered["agents"], "{allowed}");
	assert_eq!(allowed["errors"], json!([]), "{allowed}");
}

#[rstest]
#[tokio::test]
async fn catalog_reads_distinguish_an_empty_node_from_another_tenants_node(
	#[future]
	#[from(execution_fixtures::execution)]
	execution: execution_fixtures::ExecutionFixture,
) {
	// Arrange
	let fixture = Box::pin(execution).await;
	endpoint::tenant_root(&fixture.app.runtime, "globex").await;
	let foreign = endpoint::default_node(&fixture.app.runtime.store, "globex").await;
	let own = endpoint::default_node(&fixture.app.runtime.store, "acme").await;
	// Act / Assert
	for (node, status) in [(own.node.to_string(), 200), (foreign.node.to_string(), 404)] {
		assert_json(
			fixture
				.subject
				.get(&format!("/api/authorization/acme/catalog?node={node}"))
				.await
				.unwrap(),
			status,
		);
	}
}

#[rstest]
#[tokio::test]
async fn delegating_to_a_sibling_executes_on_that_receiving_node(
	#[future]
	#[from(execution_fixtures::execution)]
	execution: execution_fixtures::ExecutionFixture,
) {
	// Arrange: a portable Agent has approvals on both Nodes, and no Mapping
	// is needed between Nodes belonging to this Tenant.
	let fixture = Box::pin(execution).await;
	let app = &fixture.app;
	let (status, second) = endpoint::browser_mapping(app, "acme", "alice")
		.await
		.send(
			"POST",
			"/api/tenants/acme/nodes",
			json!({"label":"Executor"}),
		)
		.await;
	assert_eq!(status, 201, "{second}");
	let name = aidash_domain::nodes::NodeName::parse(second["name"].as_str().unwrap()).unwrap();
	let address = second["address"].as_str().unwrap();
	let authorization = aidash_server::authorization::Authorization {
		pool: app.runtime.store.pool.clone(),
	};
	for id in ["model", "http", "research"] {
		authorization
			.set_catalog(
				"acme",
				&name,
				&aidash_server::registry::EntityRef {
					id: id.into(),
					version: "1.0.0".into(),
				},
				0,
				true,
				"fixture",
			)
			.await
			.unwrap();
	}
	let mut policy = fixture.policy.clone();
	policy["subjects"][aidash_domain::agent_subject(address, "research", "1.0.0")] =
		json!({"kind":"agent"});
	deployment_administrator(
		app,
		"/api/authorization/acme",
		json!({"expected_revision":1,"bundle":policy}),
	)
	.await;
	// Act
	let delegated = assert_json(
		fixture
			.subject
			.post(
				&format!("/api/tasks/{}/delegate", fixture.task),
				&json!({"node_id":address,"agent":{"id":"research","version":"1.0.0"}}),
				"json",
			)
			.await
			.unwrap(),
		200,
	);
	// Assert
	assert_eq!(delegated["node_id"], address);
	let runs = app
		.runtime
		.store
		.runs(&aidash_application::tenancy::TEST_FIXTURE)
		.await
		.unwrap();
	assert_eq!(runs.len(), 1);
	assert_eq!(runs[0].node, name);
	assert_eq!(
		runs[0].home_node,
		endpoint::default_node(&app.runtime.store, "acme")
			.await
			.to_string()
	);
}
/// Posts `body` as the right actor: the whole-policy replace stays with the
/// Deployment Administrator, while Peer Mappings, Node registrations and
/// revocations go through a Tenant Administrator Mapping (#484). The first
/// credentials are issued from the host.
async fn deployment_administrator(app: &EndpointFixture, path: &str, body: Value) -> Value {
	let tenant = path
		.strip_prefix("/api/authorization/")
		.or_else(|| path.strip_prefix("/api/tenants/"))
		.map(|rest| rest.split('/').next().unwrap());
	let tenant_setting = path.matches('/').count() > 3;
	let (Some(tenant), true) = (tenant, tenant_setting) else {
		return assert_json(
			app.deployment_administrator
				.post(path, &body, "json")
				.await
				.unwrap(),
			200,
		);
	};
	if path.ends_with("/credentials") {
		let subject = body["subject"].as_str().unwrap();
		return endpoint::issued_credential(&app.runtime.store, tenant, subject).await;
	}
	if path.ends_with("/peer-mappings") {
		let (status, response) = host_peer_mapping(app, tenant, body).await;
		assert_eq!(status, 200, "{response}");
		return response;
	}
	let (status, response) = endpoint::browser_mapping(app, tenant, "alice")
		.await
		.send("POST", path, body)
		.await;
	assert_eq!(status, 200, "{response}");
	response
}

/// Writes a Peer Mapping as the host would: a Tenant's HTTP route binds only
/// credentials of service and agent subjects, and a Deployment Administrator
/// has no route to it (#484).
async fn host_peer_mapping(app: &EndpointFixture, tenant: &str, body: Value) -> (u16, Value) {
	let input = serde_json::from_value(body).expect("the Peer Mapping input is well formed");
	match aidash_server::apps::identity::services::peer::write(&app.runtime, tenant, input).await {
		Ok(mapping) => (200, serde_json::to_value(mapping).unwrap()),
		Err(error) => (
			error.http_response().status.as_u16(),
			json!({"error": "refused"}),
		),
	}
}

#[rstest]
#[tokio::test]
async fn discovery_intersects_both_nodes_without_forwarding_subject_tokens(
	#[future] discovery_pair: DiscoveryPair,
) {
	// Arrange
	let mut pair = Box::pin(discovery_pair).await;
	let a = &pair.source.app;
	let b = &pair.destination.app;
	let subject = &pair.source.subject;
	let mut policy = pair.source.policy;
	// Act
	let result = discover(subject, json!({})).await;
	// Assert
	assert_eq!(result["agents"].as_array().unwrap().len(), 1);
	assert_eq!(
		result["errors"][0]["error"],
		"authorized peer discovery unavailable"
	);
	let source_node = endpoint::default_node(&pair.source_runtime.store, "acme").await;
	let destination_node = endpoint::default_node(&pair.destination_runtime.store, "acme").await;
	let issued = deployment_administrator(
		b,
		"/api/authorization/acme/credentials",
		json!({"subject":"alice"}),
	)
	.await;
	deployment_administrator(
		b,
		"/api/authorization/acme/peer-mappings",
		json!({
			"node":destination_node.node,"source_node":source_node,"source_subject":"alice",
			"credential_id":issued["credential"]["id"],"expected_revision":0,"enabled":true,
		}),
	)
	.await;
	let result = discover(subject, json!({})).await;
	assert_eq!(result["agents"].as_array().unwrap().len(), 2);
	assert_eq!(result["errors"], json!([]));
	// Identical local IDs must still be authorized within the remote namespace.
	policy["policies"].as_array_mut().unwrap().push(json!({
        "id":"hide-remote","effect":"deny","subjects":{"any":true},"actions":["registry.read"],
        "resources":{"kinds":["agent"],"ids":[aidash_server::domain::qualified_agent(&destination_node,"research","1.0.0")]},
    }));
	deployment_administrator(
		a,
		"/api/authorization/acme",
		json!({"expected_revision":1,"bundle":policy}),
	)
	.await;
	let result = discover(subject, json!({})).await;
	assert_eq!(result["agents"].as_array().unwrap().len(), 1);
	// The Deployment's own agents are the sending Node's, named by its address.
	assert_eq!(
		result["agents"][0]["node_id"],
		endpoint::default_node(&a.runtime.store, "acme")
			.await
			.to_string()
	);
	let mut observed = Vec::new();
	while let Ok(request) = pair.observed.try_recv() {
		observed.push(request);
	}
	assert_eq!(observed.len(), 3);
	let sender_keys = std::collections::BTreeMap::from([(
		peering::key_id(&pair.source_runtime),
		pair.source_runtime
			.config
			.deployment_key
			.as_ref()
			.unwrap()
			.public_key_pem()
			.to_owned(),
	)]);
	let sender_keys = aidash_capability::PublicKeys::from_pems(sender_keys).unwrap();
	for request in observed {
		let token = request.authorization.strip_prefix("Bearer ").unwrap();
		assert_ne!(token, pair.source.subject_token);
		// The token is the source Deployment's, bound to this one request.
		let claims = aidash_capability::federation::FederationClaims::verify(
			token,
			&sender_keys,
			chrono::Utc::now().timestamp() as u64,
			&aidash_capability::federation::RequestBinding {
				protocol: "0.4",
				method: "POST",
				path: "/scoped/discover",
				body: request.raw.as_bytes(),
			},
		)
		.expect("a Federation token signed with the source's Deployment Key");
		assert_eq!(claims.iss, a.runtime.config.deployment.as_str());
		assert_eq!(claims.sub, source_node.to_string());
		assert_eq!(claims.aud, destination_node.to_string());
		assert_eq!(claims.subject, "alice");
		assert_eq!(request.protocol, "0.4");
		let mut keys: Vec<_> = request
			.body
			.as_object()
			.unwrap()
			.keys()
			.map(String::as_str)
			.collect();
		keys.sort_unstable();
		// Tenant and subject travel in the signed token, never in the body.
		assert_eq!(keys, ["search"]);
		assert_eq!(request.body["search"]["kind"], "agent");
	}
	policy["policies"].as_array_mut().unwrap().pop();
	policy["policies"].as_array_mut().unwrap().push(json!({
		"id":"no-disclosure","effect":"deny","subjects":{"any":true},
		"actions":["federation.discover"],"resources":{"kinds":["node"]},
	}));
	deployment_administrator(
		a,
		"/api/authorization/acme",
		json!({"expected_revision":2,"bundle":policy}),
	)
	.await;
	let result = discover(subject, json!({})).await;
	assert_eq!(result["errors"], json!([]));
	assert!(
		pair.observed.try_recv().is_err(),
		"denied discovery must not disclose tenant, subject or query"
	);
	policy["policies"].as_array_mut().unwrap().pop();
	deployment_administrator(
		a,
		"/api/authorization/acme",
		json!({"expected_revision":3,"bundle":policy}),
	)
	.await;
	aidash_server::authorization::Authorization {
		pool: b.runtime.store.pool.clone(),
	}
	.set_catalog(
		"acme",
		&aidash_domain::nodes::NodeName::derived("acme"),
		&aidash_server::registry::EntityRef {
			id: "research".into(),
			version: "1.0.0".into(),
		},
		1,
		false,
		"deployment_administrator",
	)
	.await
	.unwrap();
	let result = discover(subject, json!({})).await;
	assert_eq!(result["agents"].as_array().unwrap().len(), 1);
	assert_eq!(result["errors"], json!([]));
	deployment_administrator(
		b,
		&format!(
			"/api/authorization/acme/credentials/{}/revoke",
			issued["credential"]["id"].as_str().unwrap()
		),
		json!({}),
	)
	.await;
	let result = discover(subject, json!({})).await;
	assert_eq!(result["agents"].as_array().unwrap().len(), 1);
	assert_eq!(result["errors"].as_array().unwrap().len(), 1);
}

#[rstest]
#[tokio::test]
async fn discovery_from_a_named_node_reports_agents_by_node_addresses(
	#[future] discovery_pair: DiscoveryPair,
) {
	// Arrange
	let pair = Box::pin(discovery_pair).await;
	let a = &pair.source.app;
	let subject = &pair.source.subject;
	let (status, created) = endpoint::browser_mapping(a, "acme", "alice")
		.await
		.send("POST", "/api/tenants/acme/nodes", json!({"label":"Second"}))
		.await;
	assert_eq!(status, 201, "{created}");
	let second = created["name"].as_str().unwrap();
	let default = endpoint::default_node(&a.runtime.store, "acme").await;
	let discover_from = |node: String| async move {
		assert_json(
			subject
				.post(&format!("/api/discover?node={node}"), &json!({}), "json")
				.await
				.unwrap(),
			200,
		)
	};
	let named = |result: &Value| -> Vec<String> {
		result["agents"]
			.as_array()
			.unwrap()
			.iter()
			.map(|agent| agent["node_id"].as_str().unwrap().to_owned())
			.collect()
	};
	// Act
	let from_default = discover_from(default.node.to_string()).await;
	let from_second = discover_from(second.to_owned()).await;
	// Assert
	assert!(
		named(&from_default).contains(&default.to_string()),
		"the sending Node's own agents carry its address: {from_default}"
	);
	// The second Node holds no approvals, and its sibling's agents carry the
	// sibling's address.
	assert_eq!(named(&from_second), [default.to_string()], "{from_second}");
}

#[rstest]
#[tokio::test]
async fn in_flight_discovery_retains_source_authority_and_subsequent_requests_honor_revocation(
	#[future] mock_discovery: MockDiscovery,
) {
	// Arrange
	let fixture = Box::pin(mock_discovery).await;
	let app = &fixture.execution.app;
	fixture.pause.store(true, Ordering::SeqCst);
	// A separate Tenant Administrator of the same Tenant revokes alice's
	// credential: a third party, as a Deployment Administrator once was. Alice
	// revoking her own credential while her discovery holds it is a separate
	// lock-order problem.
	endpoint::replace_policy(
		&app.runtime.store,
		"acme",
		json!({"tenant":"acme","subjects":{"alice":{"kind":"user"},"carol":{"kind":"user"}},
			"policies":[{"id":"fixture","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}},{"id":"call-unapproved","effect":"allow","subjects":{"any":true},"actions":["tool.call_unapproved"],"resources":{"kinds":["tool"]}}]}),
	)
	.await;
	let credential = app
		.database
		.fixture(async |tx| {
			// The fixture Subject's credential is the oldest: the Mappings that
			// register Nodes issue their own afterwards.
			Ok(AuthorizationCredential::objects()
				.filter(AuthorizationCredential::field_subject().eq("alice".to_owned()))
				.all_with_db(tx)
				.await?
				.into_iter()
				.min_by_key(|credential| credential.created_at)
				.expect("the fixture Subject holds a credential"))
		})
		.await;
	let input = json!({});
	let active = fixture
		.execution
		.subject
		.post("/api/discover", &input, "json");
	tokio::pin!(active);
	// Act
	timeout(Duration::from_secs(5), async {
		tokio::select! {
			_ = &mut active => panic!("discovery completed before the peer barrier"),
			() = fixture.started.notified() => {},
		}
	})
	.await
	.expect("outbound discovery reaches the peer");
	let path = format!(
		"/api/authorization/acme/credentials/{}/revoke",
		credential.id
	);
	let member = endpoint::browser_mapping(app, "acme", "carol").await;
	let revoker = member.send("POST", &path, input.clone());
	tokio::pin!(revoker);
	// Assert
	assert!(
		timeout(Duration::from_millis(100), &mut revoker)
			.await
			.is_err(),
		"revocation must wait for the admitted boundary"
	);
	fixture.release.notify_one();
	let result = assert_json(
		timeout(Duration::from_secs(5), active)
			.await
			.unwrap()
			.unwrap(),
		200,
	);
	assert_eq!(result["agents"].as_array().unwrap().len(), 2);
	let (status, revoked) = timeout(Duration::from_secs(5), revoker).await.unwrap();
	assert_eq!(status, 200, "{revoked}");
	assert_json(
		fixture
			.execution
			.subject
			.post("/api/discover", &input, "json")
			.await
			.unwrap(),
		401,
	);
}

#[rstest]
#[tokio::test]
async fn source_filters_search_results_and_rejects_invalid_peer_metadata(
	#[future] mock_discovery: MockDiscovery,
) {
	// Arrange
	let fixture = Box::pin(mock_discovery).await;
	let subject = &fixture.execution.subject;
	// Act
	let result = discover(subject, json!({"capability":"not-present"})).await;
	// Assert
	assert_eq!(result["agents"], json!([]));
	assert_eq!(result["errors"], json!([]));
	for selected in 1..=3 {
		fixture.mode.store(selected, Ordering::SeqCst);
		let result = discover(subject, json!({})).await;
		assert_eq!(result["agents"].as_array().unwrap().len(), 1);
		assert_eq!(
			result["agents"][0]["node_id"],
			endpoint::default_node(&fixture.execution.app.runtime.store, "acme")
				.await
				.to_string()
		);
		assert_eq!(
			result["errors"][0]["error"],
			"invalid peer discovery response"
		);
	}
}

#[rstest]
#[case::name(None)]
#[case::label(Some(("/label", json!("Default"))))]
#[case::default(Some(("/default", json!(true))))]
#[tokio::test]
async fn registry_verification_uses_the_receiving_nodes_policy_resource(
	#[future]
	#[from(execution_fixtures::execution)]
	execution: execution_fixtures::ExecutionFixture,
	#[case] condition: Option<(&str, Value)>,
) {
	let fixture = Box::pin(execution).await;
	let app = &fixture.app;
	let f = peering::enable(&app.runtime, &app.context);
	let receiver = endpoint::default_node(&f.store, "acme").await;
	let remote = peering::Remote::start("aidash://registry-source").await;
	let sender = aidash_domain::nodes::NodeName::derived("source").to_string();
	peering::permit(&f, &remote.endpoint);
	deployment_administrator(
		app,
		&peering::registration_path("acme", receiver.node.as_str()),
		remote.registration(&sender),
	)
	.await;
	let issued = deployment_administrator(
		app,
		"/api/authorization/acme/credentials",
		json!({"subject":"alice"}),
	)
	.await;
	deployment_administrator(
		app,
		"/api/authorization/acme/peer-mappings",
		json!({
			"node":receiver.node,"source_node":remote.node(&sender),"source_subject":"bob",
			"credential_id":issued["credential"]["id"],"enabled":true,"expected_revision":0,
		}),
	)
	.await;
	let entry = f
		.registry
		.get(
			&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
			"research",
			"1.0.0",
		)
		.await
		.unwrap();
	let input = json!({"references":[{"entry":{"id":"research","version":"1.0.0"},"digest":aidash_server::registry::digest(&serde_json::to_value(entry).unwrap())}]});
	let mut policy = fixture.policy.clone();
	policy["policies"] = json!([
		{"id":"definitions","effect":"allow","subjects":{"any":true},"actions":["node.use","registry.read","agent.execute"],"resources":{"kinds":["*"]}},
		{"id":"receiver","effect":"allow","subjects":{"any":true},"actions":["federation.discover"],"resources":{"kinds":["node"],"ids":[receiver.node]}},
	]);
	if let Some((path, value)) = condition {
		policy["policies"][1]["condition"] = json!({"op":"eq","left":{"source":"resource","path":path},"right":{"source":"literal","value":value}});
	}
	for (denied, id) in [
		(false, json!(receiver.node)),
		(
			true,
			json!(aidash_domain::nodes::NodeName::derived("other")),
		),
		(true, json!(receiver.to_string())),
		(true, json!(receiver.deployment)),
	] {
		policy["policies"][1]["resources"]["ids"] = json!([id]);
		endpoint::replace_policy(&f.store, "acme", policy.clone()).await;
		let response = peering::post_as(
			&app.anonymous,
			&receiver.to_string(),
			peering::Bearer::Signed(&remote, sender.clone()),
			"bob",
			"/federation/v0.1/scoped/registry/verify",
			&input,
		)
		.await;
		let status = response.status_code();
		let body = response.json_value().unwrap();
		assert_eq!(status, if denied { 403 } else { 200 }, "{body}");
		if !denied {
			assert_eq!(body, true);
		}
	}
}
