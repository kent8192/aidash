#[path = "../../execution/tests/support/legacy.rs"]
mod common;

use aidash_server::{
	config::Config,
	federation::{Federation, Peer},
	registry::Registry,
};
use common::{TestEnvironment, bootstrap, request, test_environment};
use serde_json::{Value, json};
use sqlx::{Connection, Executor, postgres::PgConnection};
use std::sync::Arc;
use uuid::Uuid;

struct Node {
	f: Federation,
	database: String,
	admin: String,
	server: tokio::task::JoinHandle<()>,
}

impl Node {
	async fn new(environment: &TestEnvironment, suffix: &str) -> Self {
		let admin = environment.database_url.clone();
		let database = format!("graph_{}_{}", suffix, Uuid::new_v4().simple());
		// SeaQuery has no CREATE/DROP DATABASE builder; separate databases are the isolation under test.
		PgConnection::connect(&admin)
			.await
			.unwrap()
			.execute(format!("CREATE DATABASE {database}").as_str())
			.await
			.unwrap();
		let mut url = reqwest::Url::parse(&admin).unwrap();
		url.set_path(&format!("/{database}"));
		let node_id = format!("aidash://graph-{suffix}");
		let store = common::native_store(url.as_str(), &node_id).await;
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let endpoint = format!("http://{}", listener.local_addr().unwrap());
		let config = Config {
			node_id,
			endpoint,
			database_url: url.to_string(),
			nats_url: environment.nats_url.clone(),
			api_token: format!("graph-operator-{suffix}"),
			web_dir: "web/dist".into(),
			lease_seconds: 30,
			default_host_packages: vec![],
			oidc: None,
			gcip: None,
			prompt_cache: None,
		};
		let f = Federation {
			sandbox: Default::default(),
			gcip: None,
			registry: Registry::new(store.pool.clone(), &store.node_id).unwrap(),
			store,
			config,
			client: reqwest::Client::new(),
			notify: Arc::new(tokio::sync::Notify::new()),
		};
		let app = common::application(f.clone()).await;
		let server = tokio::spawn(async move {
			axum::serve(listener, app.test_transport()).await.unwrap();
		});
		Self {
			f,
			database,
			admin,
			server,
		}
	}
	async fn app(&self) -> common::TestApplication {
		common::application(self.f.clone()).await
	}
	async fn close(self) {
		self.server.abort();
		let _ = self.server.await;
		self.f.store.control_pool.close().await;
		self.f.store.pool.close().await;
		PgConnection::connect(&self.admin)
			.await
			.unwrap()
			.execute(format!("DROP DATABASE {} WITH (FORCE)", self.database).as_str())
			.await
			.unwrap();
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
						.expr(Expr::val("0.2"))
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
		&node.app().await,
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
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let a = Node::new(&environment, "a").await;
	let b = Node::new(&environment, "b").await;
	let c = Node::new(&environment, "c").await;
	let (mut a_policy, alice_a, _) = bootstrap(&a.f, &a.app().await, "http://localhost:1").await;
	let (mut b_policy, _, _) = bootstrap(&b.f, &b.app().await, "http://localhost:1").await;
	let _ = bootstrap(&c.f, &c.app().await, "http://localhost:1").await;
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
			&a.app().await,
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
		&a.app().await,
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
			&b.app().await,
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
		&b.app().await,
		&b.f.config.api_token,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"alice"}),
	)
	.await;
	assert_eq!(status, 200, "{alice_b}");
	let (status, bob_b) = request(
		&b.app().await,
		&b.f.config.api_token,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"bob"}),
	)
	.await;
	assert_eq!(status, 200, "{bob_b}");
	for (subject, credential) in [("alice", &alice_b), ("bob", &bob_b)] {
		let (status, mapping) = request(
			&b.app().await,
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
		"config":{"model":{"id":"model","version":"1.0.0"},"instructions":"private","schema_version":1,"bindings":[],"remove_default":[]}});
	assert_eq!(
		request(
			&b.app().await,
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
		&a.app().await,
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
		&b.app().await,
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
		&b.app().await,
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
		&b.app().await,
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
		protocol_version: "0.2".into(),
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
		protocol_version: "0.2".into(),
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
		&b.app().await,
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
