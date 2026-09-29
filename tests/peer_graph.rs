mod common;

use aidash::api;
use axum::{Router, body::Body, http::Request};
use common::{TestEnvironment, bootstrap, cleanup, request, setup, test_environment};
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
	let response = app.clone().oneshot(
		Request::builder().method("POST").uri("/federation/v0.1/scoped/graph")
			.header("authorization",format!("Bearer {token}"))
			.header("x-aidash-node",SOURCE)
			.header("x-aidash-protocol","0.1")
			.header("content-type","application/json")
			.body(Body::from(json!({
				"viewer":viewer,"scope_workspace":null,"mode":"mesh",
				"kinds":["workspace","goal","task","agent","tool","model"],
				"relations":["goal","contains","tool","model"],
				"hours":0,"limit":limit,"cursor":cursor,"target_tenant":if viewer["kind"] == "operator" { Some("acme") } else { None },
			}).to_string())).unwrap()
	).await.unwrap();
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
