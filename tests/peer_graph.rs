mod common;

use aidash::api;
use axum::{Router, body::Body, http::Request};
use common::{TestEnvironment, bootstrap, cleanup, request, setup, test_environment};
use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

const SOURCE: &str = "aidash://source";

async fn graph(
	app: &Router,
	token: &str,
	viewer: Value,
	cursor: Option<&str>,
	limit: u16,
) -> (u16, Value) {
	graph_custom(app, token, viewer, json!({"cursor":cursor,"limit":limit})).await
}

async fn graph_custom(app: &Router, token: &str, viewer: Value, overrides: Value) -> (u16, Value) {
	let mut payload = json!({
		"viewer":viewer,"scope_workspace":null,"mode":"mesh",
		"kinds":["workspace","goal","task","agent","tool","model"],
		"relations":["goal","contains","tool","model"],
		"hours":0,"limit":80,"cursor":null,
		"target_tenant":if viewer["kind"] == "operator" { Some("acme") } else { None },
	});
	payload
		.as_object_mut()
		.unwrap()
		.extend(overrides.as_object().unwrap().clone());
	let response = app
		.clone()
		.oneshot(
			Request::builder()
				.method("POST")
				.uri("/federation/v0.1/scoped/graph")
				.header("authorization", format!("Bearer {token}"))
				.header("x-aidash-node", SOURCE)
				.header("x-aidash-protocol", "0.1")
				.header("content-type", "application/json")
				.body(Body::from(payload.to_string()))
				.unwrap(),
		)
		.await
		.unwrap();
	let status = response.status().as_u16();
	let bytes = axum::body::to_bytes(response.into_body(), 4_194_304)
		.await
		.unwrap();
	(
		status,
		serde_json::from_slice(&bytes).unwrap_or(Value::Null),
	)
}

async fn add_peer(f: &aidash::federation::Federation, node: &str) {
	sqlx::query(
		&sea_orm::sea_query::Query::insert()
			.into_table(sea_orm::sea_query::Alias::new("peers"))
			.columns([
				sea_orm::sea_query::Alias::new("node_id"),
				sea_orm::sea_query::Alias::new("endpoint"),
				sea_orm::sea_query::Alias::new("credential_env"),
				sea_orm::sea_query::Alias::new("protocol_version"),
				sea_orm::sea_query::Alias::new("enabled"),
			])
			.values_panic([
				sea_orm::sea_query::Expr::cust("$1"),
				sea_orm::sea_query::Expr::cust("'http://localhost:1'"),
				sea_orm::sea_query::Expr::cust("$2"),
				sea_orm::sea_query::Expr::cust("'0.1'"),
				sea_orm::sea_query::Expr::cust("TRUE"),
			])
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(node)
	.bind(if node == SOURCE {
		"AIDASH_SECRET_TEST_PEER"
	} else {
		"AIDASH_SECRET_GRAPH_THIRD"
	})
	.execute(&f.store.pool)
	.await
	.unwrap();
}

#[rstest::rstest]
#[tokio::test]
async fn scoped_graph_sends_only_authorized_projection_and_full_goal(
	#[future(awt)]
	#[from(test_environment)]
	environment: std::sync::Arc<TestEnvironment>,
) {
	let (f, url, schema) = setup(&environment).await;
	let app = api::router(f.clone());
	let (mut policy, subject_token, _) = bootstrap(&f, &app, "http://localhost:1").await;
	let long_goal = format!(
		"First line.\n{}\nEnd of the current Goal.",
		"Authorized context. ".repeat(30)
	);
	let (status, created) = request(
		&app,
		&subject_token,
		"POST",
		"/api/workspaces",
		json!({"title":"Long Goal Workspace","goal":long_goal}),
	)
	.await;
	assert_eq!(status, 200, "{created}");
	add_peer(&f, SOURCE).await;
	let peer_token = std::env::var("AIDASH_SECRET_TEST_PEER").unwrap();
	let (status, visible_peers) = request(
		&app,
		&subject_token,
		"GET",
		"/api/federation/graph/peers",
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{visible_peers}");
	assert_eq!(visible_peers, json!([{"node_id":SOURCE}]));
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"GET",
			"/api/federation/graph/peers",
			Value::Null
		)
		.await
		.0,
		403
	);
	add_peer(&f, "aidash://third").await;
	let viewer = json!({"kind":"subject","tenant":"source-tenant","subject":"source-subject"});
	assert_eq!(
		graph(&app, &peer_token, viewer.clone(), None, 80).await.0,
		403
	);
	let (_, issued) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"alice"}),
	)
	.await;
	let (status, mapping) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/acme/peer-mappings",
		json!({
			"source_node":SOURCE,"source_tenant":"source-tenant","source_subject":"source-subject",
			"credential_id":issued["credential"]["id"],"enabled":true,"expected_revision":0,
		}),
	)
	.await;
	assert_eq!(status, 200, "{mapping}");
	let (status, page) = graph(&app, &peer_token, viewer.clone(), None, 80).await;
	assert_eq!(status, 200, "{page}");
	let nodes = page["nodes"].as_array().unwrap();
	assert!(
		nodes
			.iter()
			.any(|node| node["kind"] == "agent" && node["resource_id"] == "research")
	);
	assert!(
		nodes
			.iter()
			.any(|node| node["kind"] == "goal" && node["goal_body"] == "Use exactly one tool")
	);
	assert!(
		nodes
			.iter()
			.any(|node| node["kind"] == "goal" && node["goal_body"] == long_goal)
	);
	assert!(
		nodes
			.iter()
			.any(|node| node["kind"] == "task" && node["name"]["en"] == "Research")
	);
	assert!(!page.to_string().contains("Use approved tools"));
	assert!(!page.to_string().contains("Test approved work"));
	assert!(!page.to_string().contains("credential_env"));
	assert!(!page.to_string().contains("aidash://third"));
	assert!(page["activity"].as_array().is_some());
	let (status, goals) = graph_custom(
		&app,
		&peer_token,
		viewer.clone(),
		json!({"kinds":["goal"],"relations":["goal"]}),
	)
	.await;
	assert_eq!(status, 200, "{goals}");
	assert!(
		goals["nodes"]
			.as_array()
			.unwrap()
			.iter()
			.all(|node| node["kind"] == "goal")
	);
	assert!(
		goals["nodes"]
			.as_array()
			.unwrap()
			.iter()
			.any(|node| node["goal_body"] == long_goal)
	);
	let (status, first) = graph(&app, &peer_token, viewer.clone(), None, 2).await;
	assert_eq!(status, 200, "{first}");
	let cursor = first["next_cursor"]
		.as_str()
		.expect("additional authorized data");
	assert_eq!(
		graph(&app, &peer_token, viewer.clone(), Some(cursor), 2)
			.await
			.0,
		200
	);
	let mut bad = cursor.to_owned();
	bad.push('x');
	assert_eq!(
		graph(&app, &peer_token, viewer.clone(), Some(&bad), 2)
			.await
			.0,
		403
	);
	let mut other_policy = common::policy(&f.config.node_id);
	other_policy["tenant"] = json!("other");
	let (status, written) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/other",
		json!({"expected_revision":0,"bundle":other_policy}),
	)
	.await;
	assert_eq!(status, 200, "{written}");
	let other_workspace = Uuid::new_v4();
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("workspaces"))
			.columns([Alias::new("id"), Alias::new("title"), Alias::new("goal")])
			.values_panic([
				Expr::cust("$1"),
				Expr::cust("'Other tenant'"),
				Expr::cust("'Unrelated'"),
			])
			.to_string(PostgresQueryBuilder),
	)
	.bind(other_workspace)
	.execute(&f.store.pool)
	.await
	.unwrap();
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("authorization_workspaces"))
			.columns([
				Alias::new("workspace_id"),
				Alias::new("tenant"),
				Alias::new("owner_subject"),
			])
			.values_panic([
				Expr::cust("$1"),
				Expr::cust("'other'"),
				Expr::cust("'alice'"),
			])
			.to_string(PostgresQueryBuilder),
	)
	.bind(other_workspace)
	.execute(&f.store.pool)
	.await
	.unwrap();
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("events"))
			.columns([
				Alias::new("id"),
				Alias::new("node_id"),
				Alias::new("workspace_id"),
				Alias::new("kind"),
				Alias::new("data"),
			])
			.values_panic([
				Expr::val(Uuid::new_v4()).into(),
				Expr::val(&f.config.node_id).into(),
				Expr::val(other_workspace).into(),
				Expr::val("workspace.updated").into(),
				Expr::val(json!({})).into(),
			])
			.to_string(PostgresQueryBuilder),
	)
	.execute(&f.store.pool)
	.await
	.unwrap();
	let (status, after_other_tenant) = graph(&app, &peer_token, viewer.clone(), None, 80).await;
	assert_eq!(status, 200, "{after_other_tenant}");
	assert_eq!(after_other_tenant["generation"], page["generation"]);
	assert_eq!(
		graph(&app, &peer_token, viewer.clone(), Some(cursor), 2)
			.await
			.0,
		200
	);
	let (status, mapping) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/acme/peer-mappings",
		json!({
			"source_node":SOURCE,"source_tenant":"source-tenant","source_subject":"source-subject",
			"credential_id":issued["credential"]["id"],"enabled":false,"expected_revision":1,
		}),
	)
	.await;
	assert_eq!(status, 200, "{mapping}");
	assert_eq!(
		graph(&app, &peer_token, viewer.clone(), None, 80).await.0,
		403
	);
	let (status, mapping) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/acme/peer-mappings",
		json!({
			"source_node":SOURCE,"source_tenant":"source-tenant","source_subject":"source-subject",
			"credential_id":issued["credential"]["id"],"enabled":true,"expected_revision":2,
		}),
	)
	.await;
	assert_eq!(status, 200, "{mapping}");
	assert_eq!(
		graph(&app, &peer_token, viewer.clone(), Some(cursor), 2)
			.await
			.0,
		409
	);
	let (status, catalog) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/acme/catalog",
		json!({
			"entry":{"id":"research","version":"1.0.0"},"expected_revision":1,"enabled":false,
		}),
	)
	.await;
	assert_eq!(status, 200, "{catalog}");
	let (status, without_agent) = graph(&app, &peer_token, viewer.clone(), None, 80).await;
	assert_eq!(status, 200, "{without_agent}");
	assert!(
		!without_agent["nodes"]
			.as_array()
			.unwrap()
			.iter()
			.any(|node| node["resource_id"] == "research")
	);
	let (status, catalog) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/acme/catalog",
		json!({
			"entry":{"id":"research","version":"1.0.0"},"expected_revision":2,"enabled":true,
		}),
	)
	.await;
	assert_eq!(status, 200, "{catalog}");
	policy["policies"].as_array_mut().unwrap().push(json!({
		"id":"deny-workspace-graph","effect":"deny","subjects":{"any":true},
		"actions":["workspace.read"],"resources":{"kinds":["workspace"]},
	}));
	let (status, written) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":1,"bundle":policy}),
	)
	.await;
	assert_eq!(status, 200, "{written}");
	let (status, without_goal) = graph(&app, &peer_token, viewer.clone(), None, 80).await;
	assert_eq!(status, 200, "{without_goal}");
	assert!(
		!without_goal["nodes"]
			.as_array()
			.unwrap()
			.iter()
			.any(|node| node["kind"] == "goal")
	);
	let operator = Uuid::new_v4();
	assert_eq!(
		graph(
			&app,
			&peer_token,
			json!({"kind":"operator","id":operator}),
			None,
			80
		)
		.await
		.0,
		403
	);
	let (status, grant) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/acme/graph-operator-grants",
		json!({
			"source_node":SOURCE,"source_operator":operator,"enabled":true,"expected_revision":0,
		}),
	)
	.await;
	assert_eq!(status, 200, "{grant}");
	let (status, operator_page) = graph(
		&app,
		&peer_token,
		json!({"kind":"operator","id":operator}),
		None,
		80,
	)
	.await;
	assert_eq!(status, 200, "{operator_page}");
	assert!(
		operator_page["nodes"]
			.as_array()
			.unwrap()
			.iter()
			.any(|node| node["kind"] == "goal" && node["goal_body"] == long_goal)
	);
	policy["policies"].as_array_mut().unwrap().push(json!({
		"id":"deny-graph","effect":"deny","subjects":{"ids":["alice"]},
		"actions":["federation.graph.read"],"resources":{"kinds":["node"]},
	}));
	let (status, written) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":2,"bundle":policy}),
	)
	.await;
	assert_eq!(status, 200, "{written}");
	assert_eq!(
		graph(&app, &peer_token, viewer.clone(), None, 80).await.0,
		403
	);
	assert_eq!(
		graph(
			&app,
			&peer_token,
			json!({"kind":"operator","id":operator}),
			None,
			80
		)
		.await
		.0,
		200
	);
	policy["policies"]
		.as_array_mut()
		.unwrap()
		.retain(|rule| rule["id"] != "deny-graph");
	policy["subjects"]["alice"]["enabled"] = json!(false);
	let (status, written) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":3,"bundle":policy}),
	)
	.await;
	assert_eq!(status, 200, "{written}");
	assert_eq!(
		graph(&app, &peer_token, viewer.clone(), None, 80).await.0,
		403
	);
	policy["subjects"]["alice"]["enabled"] = json!(true);
	let (status, written) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":4,"bundle":policy}),
	)
	.await;
	assert_eq!(status, 200, "{written}");
	assert_eq!(
		graph(&app, &peer_token, viewer.clone(), None, 80).await.0,
		200
	);
	let (status, short) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/acme/credentials",
		json!({
			"subject":"alice","expires_in_seconds":1,
		}),
	)
	.await;
	assert_eq!(status, 200, "{short}");
	let (status, mapping) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/acme/peer-mappings",
		json!({
			"source_node":SOURCE,"source_tenant":"source-tenant","source_subject":"source-subject",
			"credential_id":short["credential"]["id"],"enabled":true,"expected_revision":3,
		}),
	)
	.await;
	assert_eq!(status, 200, "{mapping}");
	tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
	assert_eq!(
		graph(&app, &peer_token, viewer.clone(), None, 80).await.0,
		403
	);
	let gate = Uuid::new_v4();
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("atomic_participants"))
			.columns(["id", "coordinator", "digest", "manifest", "phase"].map(Alias::new))
			.values_panic([
				Expr::val(gate).into(),
				Expr::val(&f.config.node_id).into(),
				Expr::val("fixture").into(),
				Expr::val(json!({})).into(),
				Expr::val("PREPARED").into(),
			])
			.to_string(PostgresQueryBuilder),
	)
	.execute(&f.store.control_pool)
	.await
	.unwrap();
	sqlx::query(
		&Query::update()
			.table(Alias::new("atomic_gate"))
			.value(Alias::new("transaction_id"), Expr::cust("$1"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(gate)
	.execute(&f.store.control_pool)
	.await
	.unwrap();
	let (status, _) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/acme/graph-operator-grants",
		json!({
			"source_node":SOURCE,"source_operator":operator,"enabled":false,"expected_revision":1,
		}),
	)
	.await;
	sqlx::query(
		&Query::update()
			.table(Alias::new("atomic_gate"))
			.value(Alias::new("transaction_id"), Expr::cust("$1"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(Option::<Uuid>::None)
	.execute(&f.store.control_pool)
	.await
	.unwrap();
	assert_eq!(status, 200);
	assert_eq!(
		graph(
			&app,
			&peer_token,
			json!({"kind":"operator","id":operator}),
			None,
			80
		)
		.await
		.0,
		403
	);
	let _ = subject_token;
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn sparse_goal_pages_advance_and_activity_reaches_older_page_events(
	#[future(awt)]
	#[from(test_environment)]
	environment: std::sync::Arc<TestEnvironment>,
) {
	let (f, url, schema) = setup(&environment).await;
	let app = api::router(f.clone());
	bootstrap(&f, &app, "http://localhost:1").await;
	add_peer(&f, SOURCE).await;
	let peer_token = std::env::var("AIDASH_SECRET_TEST_PEER").unwrap();
	let final_id = Uuid::from_u128(u128::MAX);
	let policy = json!({
		"tenant":"sparse","subjects":{"alice":{"kind":"user"}},
		"policies":[
			{"id":"graph-read","effect":"allow","subjects":{"any":true},
			 "actions":["federation.graph.read"],"resources":{"kinds":["node"]}},
			{"id":"final-workspace","effect":"allow","subjects":{"any":true},
			 "actions":["workspace.read","workspace.events"],
			 "resources":{"kinds":["workspace"],"ids":[final_id.to_string()]}}
		]
	});
	let (status, written) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/sparse",
		json!({"expected_revision":0,"bundle":policy}),
	)
	.await;
	assert_eq!(status, 200, "{written}");
	let (status, issued) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/sparse/credentials",
		json!({"subject":"alice"}),
	)
	.await;
	assert_eq!(status, 200, "{issued}");
	let (status, mapping) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/sparse/peer-mappings",
		json!({"source_node":SOURCE,"source_tenant":"source-tenant","source_subject":"sparse-subject",
			"credential_id":issued["credential"]["id"],"enabled":true,"expected_revision":0}),
	)
	.await;
	assert_eq!(status, 200, "{mapping}");
	let ids: Vec<Uuid> = (1..=4096).map(Uuid::from_u128).collect();
	for chunk in ids.chunks(256) {
		let mut workspaces = Query::insert();
		workspaces.into_table(Alias::new("workspaces")).columns([
			Alias::new("id"),
			Alias::new("title"),
			Alias::new("goal"),
		]);
		let mut authorities = Query::insert();
		authorities
			.into_table(Alias::new("authorization_workspaces"))
			.columns([
				Alias::new("workspace_id"),
				Alias::new("tenant"),
				Alias::new("owner_subject"),
			]);
		for id in chunk {
			workspaces.values_panic([
				Expr::val(*id).into(),
				Expr::val("Hidden").into(),
				Expr::val("Hidden goal").into(),
			]);
			authorities.values_panic([
				Expr::val(*id).into(),
				Expr::val("sparse").into(),
				Expr::val("alice").into(),
			]);
		}
		sqlx::query(&workspaces.to_string(PostgresQueryBuilder))
			.execute(&f.store.pool)
			.await
			.unwrap();
		sqlx::query(&authorities.to_string(PostgresQueryBuilder))
			.execute(&f.store.pool)
			.await
			.unwrap();
	}
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("workspaces"))
			.columns([Alias::new("id"), Alias::new("title"), Alias::new("goal")])
			.values_panic([
				Expr::cust("$1"),
				Expr::cust("'Last'"),
				Expr::cust("'Reachable goal'"),
			])
			.to_string(PostgresQueryBuilder),
	)
	.bind(final_id)
	.execute(&f.store.pool)
	.await
	.unwrap();
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("authorization_workspaces"))
			.columns([
				Alias::new("workspace_id"),
				Alias::new("tenant"),
				Alias::new("owner_subject"),
			])
			.values_panic([
				Expr::cust("$1"),
				Expr::cust("'sparse'"),
				Expr::cust("'alice'"),
			])
			.to_string(PostgresQueryBuilder),
	)
	.bind(final_id)
	.execute(&f.store.pool)
	.await
	.unwrap();
	let occurred = chrono::Utc::now() - chrono::Duration::seconds(30);
	let mut events = Query::insert();
	events.into_table(Alias::new("events")).columns([
		Alias::new("id"),
		Alias::new("node_id"),
		Alias::new("workspace_id"),
		Alias::new("kind"),
		Alias::new("data"),
		Alias::new("created_at"),
	]);
	let samples = std::iter::once((final_id, "workspace.updated"))
		.chain(std::iter::repeat_n((ids[0], "workspace.updated"), 600))
		.chain(std::iter::repeat_n((final_id, "unrelated.event"), 600));
	for (id, kind) in samples {
		events.values_panic([
			Expr::val(Uuid::new_v4()).into(),
			Expr::val(&f.config.node_id).into(),
			Expr::val(id).into(),
			Expr::val(kind).into(),
			Expr::val(json!({})).into(),
			Expr::val(occurred).into(),
		]);
	}
	sqlx::query(&events.to_string(PostgresQueryBuilder))
		.execute(&f.store.pool)
		.await
		.unwrap();
	let viewer = json!({"kind":"subject","tenant":"source-tenant","subject":"sparse-subject"});
	let options = json!({"kinds":["goal"],"relations":[]});
	let (status, first) = graph_custom(&app, &peer_token, viewer.clone(), options.clone()).await;
	assert_eq!(status, 200, "{first}");
	assert!(first["nodes"].as_array().unwrap().is_empty());
	let cursor = first["next_cursor"].as_str().expect("scan continuation");
	let mut next_options = options;
	next_options["cursor"] = json!(cursor);
	let (status, second) = graph_custom(&app, &peer_token, viewer.clone(), next_options).await;
	assert_eq!(status, 200, "{second}");
	assert!(
		second["nodes"]
			.as_array()
			.unwrap()
			.iter()
			.any(|node| node["goal_body"] == "Reachable goal")
	);
	let options = json!({"kinds":["workspace","goal"],"relations":[],"limit":2});
	let (status, first) = graph_custom(&app, &peer_token, viewer.clone(), options.clone()).await;
	assert_eq!(status, 200, "{first}");
	assert!(first["nodes"].as_array().unwrap().is_empty());
	let mut options = options;
	options["cursor"] = first["next_cursor"].clone();
	let (status, activity) = graph_custom(&app, &peer_token, viewer, options).await;
	assert_eq!(status, 200, "{activity}");
	let reference = json!([
		"resource",
		f.config.node_id,
		"workspace",
		final_id.to_string()
	])
	.to_string();
	assert!(activity["activity"].as_array().unwrap().iter().any(|item| {
		item["kind"] == "workspace.updated"
			&& item["reference"].as_str() == Some(reference.as_str())
	}));
	cleanup(f, &url, &schema).await;
}
