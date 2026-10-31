#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
use common::upstream_fixtures;
use futures_util::FutureExt;
#[path = "../../execution/tests/support/legacy.rs"]
mod common;

use aidash_server::{federation::Federation, registry::Registry};
use common::{bootstrap, request};
use serde_json::{Value, json};

use std::sync::Arc;

struct Node {
	f: Federation,
	app: common::TestApplication,
	url: String,
	schema: String,
	_server: upstream_fixtures::FixedServerGuard,
}
impl Node {
	async fn close(self) {
		common::cleanup(self.f, &self.url, &self.schema).await;
	}
}
#[rstest::fixture]
fn graph_runtime(
	#[default("a")] suffix: &str,
	#[from(upstream_fixtures::fixed_listener)] listener: upstream_fixtures::ListenerFuture,
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) -> common::RuntimeFuture {
	let suffix = suffix.to_owned();
	async move {
		let mut owner = runtime.await;
		let f = &mut owner.federation;
		f.config.deployment =
			aidash_domain::nodes::DeploymentId::parse(&format!("aidash://graph-{suffix}")).unwrap();
		f.store.deployment = f.config.deployment.clone();
		f.config.endpoint = format!("http://{}", listener.await.local_addr().unwrap());
		f.config.api_token = format!("peer-graph-{suffix}");
		f.registry = Registry::new(f.store.pool.clone(), &f.config.deployment)
			.unwrap()
			.with_keyless_origins(f.store.keyless.clone());
		owner
	}
	.boxed()
	.shared()
}
#[rstest::fixture]
fn graph_router(
	#[from(common::native_application)] application: common::ApplicationFuture,
) -> upstream_fixtures::RouterFuture {
	async move { application.await.application.native_router() }
		.boxed()
		.shared()
}
#[rstest::fixture]
async fn graph_node(
	#[default("a")] _suffix: &str,
	#[from(upstream_fixtures::fixed_listener)] _listener: upstream_fixtures::ListenerFuture,
	#[from(graph_runtime)]
	#[with(_suffix,_listener.clone())]
	_runtime: common::RuntimeFuture,
	#[from(common::native_application)]
	#[with(Default::default(),aidash_server::sse::Service::new(Default::default()),Arc::new(|r|r),_runtime.clone())]
	_application: common::ApplicationFuture,
	#[from(graph_router)]
	#[with(_application.clone())]
	_router: upstream_fixtures::RouterFuture,
	#[future(awt)]
	#[from(upstream_fixtures::fixed_upstream)]
	#[with(None,_listener.clone(),_router.clone())]
	server: upstream_fixtures::FixedServerGuard,
) -> Node {
	let application = _application.await;
	let (f, url, schema) = application.runtime.parts();
	Node {
		f,
		url,
		schema,
		app: application.application,
		_server: server,
	}
}

/// `local` accepts and addresses `remote`'s default Node.
async fn connect(local: &Node, remote: &Node) {
	common::register_default_node((&local.f, &local.app, "acme"), (&remote.f, "acme")).await;
}

async fn expand(
	node: &Node,
	target: &str,
	token: &str,
	cursor: Option<&str>,
	kinds: &[&str],
) -> (u16, Value) {
	request(
		&node.app,
		token,
		"POST",
		"/api/federation/graph",
		json!({
			"node_id":target,"scope_workspace":null,"mode":"mesh",
			"kinds":kinds,"relations":["goal","contains","tool","model"],
			"hours":0,"limit":2,"cursor":cursor,
		}),
	)
	.await
}

#[rstest::rstest]
#[tokio::test]
async fn three_databases_enforce_two_subjects_and_no_transitive_graph(
	#[future(awt)]
	#[from(graph_node)]
	#[with("a")]
	a: Node,
	#[future(awt)]
	#[from(graph_node)]
	#[with("b")]
	b: Node,
	#[future(awt)]
	#[from(graph_node)]
	#[with("c")]
	c: Node,
) {
	let (mut a_policy, alice_a, _) = bootstrap(&a.f, &a.app, "http://localhost:1").await;
	let (mut b_policy, _, _) = bootstrap(&b.f, &b.app, "http://localhost:1").await;
	let _ = bootstrap(&c.f, &c.app, "http://localhost:1").await;
	connect(&a, &b).await;
	connect(&b, &a).await;
	connect(&b, &c).await;
	connect(&c, &b).await;
	let a_node = common::default_node(&a.f.store, "acme").await;
	let b_node = common::default_node(&b.f.store, "acme").await;
	let target = b_node.to_string();
	a_policy["subjects"]["bob"] = json!({"kind":"user"});
	a_policy["policies"].as_array_mut().unwrap().push(json!({
		"id":"source-registry-deny","effect":"deny","subjects":{"ids":["alice"]},
		"actions":["registry.read"],"resources":{"kinds":["*"]},
	}));
	assert_eq!(
		request(
			&a.app,
			&a.f.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":1,"bundle":a_policy})
		)
		.await
		.0,
		200
	);
	let bob_a = common::issued_credential(&a.f.store, "acme", "bob").await;
	let bob_a = bob_a["token"].as_str().unwrap().to_owned();

	b_policy["subjects"]["bob"] = json!({"kind":"user"});
	b_policy["policies"] = json!([
		{"id":"alice-full","effect":"allow","subjects":{"ids":["alice"]},"actions":["*"],"resources":{"kinds":["*"]}},
		{"id":"bob-graph","effect":"allow","subjects":{"ids":["bob"]},"actions":["federation.graph.read","workspace.read","task.read"],"resources":{"kinds":["*"]}},
	]);
	assert_eq!(
		request(
			&b.app,
			&b.f.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":1,"bundle":b_policy})
		)
		.await
		.0,
		200
	);
	let alice_b = common::issued_credential(&b.f.store, "acme", "alice").await;
	let bob_b = common::issued_credential(&b.f.store, "acme", "bob").await;
	for (subject, credential) in [("alice", &alice_b), ("bob", &bob_b)] {
		let (status, mapping) = common::set_peer_mapping(
			&b.f,
			"acme",
			"alice",
			json!({
				"node":b_node.node,"source_node":a_node,"source_subject":subject,
				"credential_id":credential["credential"]["id"],"enabled":true,"expected_revision":0,
			}),
		)
		.await;
		assert_eq!(status, 200, "{mapping}");
	}
	let hidden = json!({"id":"hidden","version":"1.0.0","kind":"agent","name":{"en":"Hidden B Agent"},
		"description":{"en":"private"},"capabilities":[],"languages":["en"],"schema":{"type":"object"},
		"config":{"model":{"id":"model","version":"1.0.0"},"instructions":"private","schema_version":1,"bindings":[],"remove_default":[]}});
	b.f.registry
		.register(
			&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
			serde_json::from_value(hidden).unwrap(),
		)
		.await
		.unwrap();

	let peers = request(
		&a.app,
		&alice_a,
		"GET",
		"/api/federation/graph/peers",
		Value::Null,
	)
	.await;
	assert_eq!(peers.0, 200);
	assert_eq!(peers.1, json!([{"node_id":target}]));
	let (status, alice_page) = expand(
		&a,
		&target,
		&alice_a,
		None,
		&["workspace", "goal", "task", "agent", "tool", "model"],
	)
	.await;
	assert_eq!(status, 200, "{alice_page}");
	assert!(!alice_page.to_string().contains("aidash://graph-c"));
	assert!(!alice_page.to_string().contains("Hidden B Agent"));
	let mut alice_nodes = alice_page["nodes"].as_array().unwrap().clone();
	let mut cursor = alice_page["next_cursor"].as_str().map(str::to_owned);
	let first_cursor = cursor.clone();
	while let Some(token) = cursor {
		let (status, page) = expand(
			&a,
			&target,
			&alice_a,
			Some(&token),
			&["workspace", "goal", "task", "agent", "tool", "model"],
		)
		.await;
		assert_eq!(status, 200, "{page}");
		alice_nodes.extend(page["nodes"].as_array().unwrap().clone());
		cursor = page["next_cursor"].as_str().map(str::to_owned);
	}
	assert!(
		alice_nodes
			.iter()
			.any(|node| node["kind"] == "agent" && node["resource_id"] == "research")
	);
	assert!(
		alice_nodes
			.iter()
			.any(|node| node["kind"] == "goal" && node["goal_body"] == "Use exactly one tool")
	);
	assert!(
		!alice_nodes
			.iter()
			.any(|node| node["resource_id"] == "hidden")
	);
	assert!(first_cursor.is_some());
	let stolen = first_cursor.as_deref().unwrap();
	assert_eq!(
		expand(
			&a,
			&target,
			&bob_a,
			Some(stolen),
			&["workspace", "goal", "task", "agent", "tool", "model"]
		)
		.await
		.0,
		403
	);
	assert_eq!(
		expand(&a, &target, &alice_a, Some(stolen), &["agent"])
			.await
			.0,
		403
	);
	assert_eq!(
		expand(
			&a,
			&target,
			&alice_a,
			Some(stolen),
			&["workspace", "goal", "task", "agent", "tool", "model"]
		)
		.await
		.0,
		200
	);
	let mut bob_nodes = Vec::new();
	let mut cursor = None;
	loop {
		let (status, page) = expand(
			&a,
			&target,
			&bob_a,
			cursor.as_deref(),
			&["workspace", "goal", "task", "agent", "tool", "model"],
		)
		.await;
		assert_eq!(status, 200, "{page}");
		bob_nodes.extend(page["nodes"].as_array().unwrap().clone());
		cursor = page["next_cursor"].as_str().map(str::to_owned);
		if cursor.is_none() {
			break;
		}
	}
	assert!(bob_nodes.iter().any(|node| node["kind"] == "workspace"));
	assert!(!bob_nodes.iter().any(|node| node["kind"] == "agent"));
	aidash_server::authorization::Authorization {
		pool: b.f.store.pool.clone(),
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
	assert_eq!(
		expand(
			&a,
			&target,
			&alice_a,
			Some(stolen),
			&["workspace", "goal", "task", "agent", "tool", "model"]
		)
		.await
		.0,
		409
	);
	let (status, denied) = common::set_peer_mapping(
		&b.f,
		"acme",
		"alice",
		json!({
			"node":b_node.node,"source_node":a_node,"source_subject":"alice",
			"credential_id":alice_b["credential"]["id"],"enabled":false,"expected_revision":1,
		}),
	)
	.await;
	assert_eq!(status, 200, "{denied}");
	assert_eq!(expand(&a, &target, &alice_a, None, &["agent"]).await.0, 403);
	let (status, _) = common::set_peer_mapping(
		&b.f,
		"acme",
		"alice",
		json!({
			"node":b_node.node,"source_node":a_node,"source_subject":"alice",
			"credential_id":alice_b["credential"]["id"],"enabled":true,"expected_revision":2,
		}),
	)
	.await;
	assert_eq!(status, 200);
	common::set_node_enabled((&a.f, &a.app, "acme"), (&b.f, "acme"), false, 1).await;
	assert_eq!(expand(&a, &target, &alice_a, None, &["agent"]).await.0, 403);
	common::set_node_enabled((&a.f, &a.app, "acme"), (&b.f, "acme"), true, 2).await;
	common::set_node_enabled((&b.f, &b.app, "acme"), (&a.f, "acme"), false, 1).await;
	assert_eq!(expand(&a, &target, &alice_a, None, &["agent"]).await.0, 403);
	common::set_node_enabled((&b.f, &b.app, "acme"), (&a.f, "acme"), true, 2).await;
	// Revoking a Tenant's credential is a member's act through a Mapping, or the
	// host's through the service; never an HTTP Deployment Administrator's (#484).
	aidash_server::authorization::Authorization {
		pool: b.f.store.pool.clone(),
	}
	.revoke_credential(
		"acme",
		alice_b["credential"]["id"]
			.as_str()
			.unwrap()
			.parse()
			.unwrap(),
	)
	.await
	.unwrap();
	assert_eq!(expand(&a, &target, &alice_a, None, &["agent"]).await.0, 403);

	// With Federation off, A refuses before any network call: B would answer
	// 403 for the revoked credential, but A answers 503 itself (#638).
	let mut disabled = a.f.clone();
	disabled.store.egress.enabled = false;
	a.app.context.set_singleton(disabled);
	let (status, body) = expand(&a, &target, &alice_a, None, &["agent"]).await;
	assert_eq!(status, 503, "{body}");
	assert_eq!(body["error"], "federation_disabled");

	a.close().await;
	b.close().await;
	c.close().await;
}
