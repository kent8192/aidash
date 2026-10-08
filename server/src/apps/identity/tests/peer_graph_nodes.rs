use common::upstream_fixtures;
use futures_util::FutureExt;
#[path = "../../execution/tests/support/legacy.rs"]
mod common;

use aidash_server::{
	federation::{Federation, Peer},
	registry::Registry,
};
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
		f.config.node_id = format!("aidash://graph-{suffix}");
		f.store.node_id = f.config.node_id.clone();
		f.config.endpoint = format!("http://{}", listener.await.local_addr().unwrap());
		f.config.api_token = format!("graph-operator-{suffix}");
		f.registry = Registry::new(f.store.pool.clone(), &f.config.node_id).unwrap();
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

async fn connect(local: &Node, remote: &Node) {
	{
		let query_bind_1 = &remote.f.config.node_id;
		let query_bind_2 = &remote.f.config.endpoint;
		let query_bind_3 =
			if local.f.config.node_id.ends_with("-c") || remote.f.config.node_id.ends_with("-c") {
				"AIDASH_SECRET_GRAPH_THIRD"
			} else {
				"AIDASH_SECRET_TEST_PEER"
			};
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("peers"))
				.columns(
					[
						"node_id",
						"endpoint",
						"credential_env",
						"protocol_version",
						"enabled",
					]
					.map(Alias::new),
				)
				.from_subquery(
					reinhardt::query::Query::select()
						.expr(Expr::value(query_bind_1.to_owned()))
						.expr(Expr::value(query_bind_2.to_owned()))
						.expr(Expr::value(query_bind_3.to_owned()))
						.expr(Expr::val("0.1"))
						.expr(Expr::val(true))
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(local.f.store.pool.driver())
		.await
	}
	.unwrap();
}

async fn expand(node: &Node, token: &str, cursor: Option<&str>, kinds: &[&str]) -> (u16, Value) {
	request(
		&node.app,
		token,
		"POST",
		"/api/federation/graph",
		json!({
			"node_id":"aidash://graph-b","scope_workspace":null,"mode":"mesh",
			"kinds":kinds,"relations":["goal","contains","tool","model"],
			"hours":0,"limit":2,"cursor":cursor,"target_tenant":null,
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
	let third_secret = std::env::var("AIDASH_SECRET_GRAPH_THIRD").unwrap();
	b.f.authenticate_peer(&c.f.config.node_id, &third_secret)
		.await
		.unwrap();
	c.f.authenticate_peer(&b.f.config.node_id, &third_secret)
		.await
		.unwrap();

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
	let (status, bob_a) = request(
		&a.app,
		&a.f.config.api_token,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"bob"}),
	)
	.await;
	assert_eq!(status, 200, "{bob_a}");
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
	let (status, alice_b) = request(
		&b.app,
		&b.f.config.api_token,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"alice"}),
	)
	.await;
	assert_eq!(status, 200, "{alice_b}");
	let (status, bob_b) = request(
		&b.app,
		&b.f.config.api_token,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"bob"}),
	)
	.await;
	assert_eq!(status, 200, "{bob_b}");
	for (subject, credential) in [("alice", &alice_b), ("bob", &bob_b)] {
		let (status, mapping) = request(
			&b.app,
			&b.f.config.api_token,
			"POST",
			"/api/authorization/acme/peer-mappings",
			json!({
				"source_node":a.f.config.node_id,"source_tenant":"acme","source_subject":subject,
				"credential_id":credential["credential"]["id"],"enabled":true,"expected_revision":0,
			}),
		)
		.await;
		assert_eq!(status, 200, "{mapping}");
	}
	let hidden = json!({"id":"hidden","version":"1.0.0","kind":"agent","name":{"en":"Hidden B Agent"},
		"description":{"en":"private"},"capabilities":[],"languages":["en"],"schema":{"type":"object"},
		"config":{"model":{"id":"model","version":"1.0.0"},"instructions":"private","tools":[],"skills":[]}});
	assert_eq!(
		request(
			&b.app,
			&b.f.config.api_token,
			"POST",
			"/api/registry",
			hidden
		)
		.await
		.0,
		200
	);

	let peers = request(
		&a.app,
		&alice_a,
		"GET",
		"/api/federation/graph/peers",
		Value::Null,
	)
	.await;
	assert_eq!(peers.0, 200);
	assert_eq!(peers.1, json!([{"node_id":"aidash://graph-b"}]));
	let (status, alice_page) = expand(
		&a,
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
			&bob_a,
			Some(stolen),
			&["workspace", "goal", "task", "agent", "tool", "model"]
		)
		.await
		.0,
		403
	);
	assert_eq!(expand(&a, &alice_a, Some(stolen), &["agent"]).await.0, 403);
	assert_eq!(
		expand(
			&a,
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
	let (status, withdrawn) = request(
		&b.app,
		&b.f.config.api_token,
		"POST",
		"/api/authorization/acme/catalog",
		json!({
			"entry":{"id":"research","version":"1.0.0"},"expected_revision":1,"enabled":false,
		}),
	)
	.await;
	assert_eq!(status, 200, "{withdrawn}");
	assert_eq!(
		expand(
			&a,
			&alice_a,
			Some(stolen),
			&["workspace", "goal", "task", "agent", "tool", "model"]
		)
		.await
		.0,
		409
	);
	let (status, denied) = request(
		&b.app,
		&b.f.config.api_token,
		"POST",
		"/api/authorization/acme/peer-mappings",
		json!({
			"source_node":a.f.config.node_id,"source_tenant":"acme","source_subject":"alice",
			"credential_id":alice_b["credential"]["id"],"enabled":false,"expected_revision":1,
		}),
	)
	.await;
	assert_eq!(status, 200, "{denied}");
	assert_eq!(expand(&a, &alice_a, None, &["agent"]).await.0, 403);
	let (status, _) = request(
		&b.app,
		&b.f.config.api_token,
		"POST",
		"/api/authorization/acme/peer-mappings",
		json!({
			"source_node":a.f.config.node_id,"source_tenant":"acme","source_subject":"alice",
			"credential_id":alice_b["credential"]["id"],"enabled":true,"expected_revision":2,
		}),
	)
	.await;
	assert_eq!(status, 200);
	let source_peer = Peer {
		node_id: b.f.config.node_id.clone(),
		endpoint: b.f.config.endpoint.clone(),
		credential_env: "AIDASH_SECRET_TEST_PEER".into(),
		protocol_version: "0.1".into(),
		enabled: false,
	};
	a.f.register_peer(source_peer.clone()).await.unwrap();
	assert_eq!(expand(&a, &alice_a, None, &["agent"]).await.0, 403);
	a.f.register_peer(Peer {
		enabled: true,
		..source_peer
	})
	.await
	.unwrap();
	let receiver_peer = Peer {
		node_id: a.f.config.node_id.clone(),
		endpoint: a.f.config.endpoint.clone(),
		credential_env: "AIDASH_SECRET_TEST_PEER".into(),
		protocol_version: "0.1".into(),
		enabled: false,
	};
	b.f.register_peer(receiver_peer.clone()).await.unwrap();
	assert_eq!(expand(&a, &alice_a, None, &["agent"]).await.0, 403);
	b.f.register_peer(Peer {
		enabled: true,
		..receiver_peer
	})
	.await
	.unwrap();
	let (status, _) = request(
		&b.app,
		&b.f.config.api_token,
		"POST",
		&format!(
			"/api/authorization/acme/credentials/{}/revoke",
			alice_b["credential"]["id"].as_str().unwrap()
		),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200);
	assert_eq!(expand(&a, &alice_a, None, &["agent"]).await.0, 403);

	a.close().await;
	b.close().await;
	c.close().await;
}

use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query, QueryStatementBuilder as _};
