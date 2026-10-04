#[path = "../../execution/tests/support/legacy.rs"]
mod common;

use axum::{body::Body, http::Request};
use common::{TestEnvironment, bootstrap, cleanup, request, setup, test_environment};
use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
use serde_json::{Value, json};

use uuid::Uuid;

const SOURCE: &str = "aidash://source";

async fn graph(
	app: &common::TestApplication,
	token: &str,
	viewer: Value,
	cursor: Option<&str>,
	limit: u16,
) -> (u16, Value) {
	graph_custom(app, token, viewer, json!({"cursor":cursor,"limit":limit})).await
}

async fn graph_custom(
	app: &common::TestApplication,
	token: &str,
	viewer: Value,
	overrides: Value,
) -> (u16, Value) {
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

async fn add_peer(f: &aidash_server::federation::Federation, node: &str) {
	{
		let query_bind_1 = node;
		let query_bind_2 = if node == SOURCE {
			"AIDASH_SECRET_TEST_PEER"
		} else {
			"AIDASH_SECRET_GRAPH_THIRD"
		};
		sqlx::query(
			&reinhardt::query::Query::insert()
				.into_table(reinhardt::query::Alias::new("peers"))
				.columns([
					reinhardt::query::Alias::new("node_id"),
					reinhardt::query::Alias::new("endpoint"),
					reinhardt::query::Alias::new("credential_env"),
					reinhardt::query::Alias::new("protocol_version"),
					reinhardt::query::Alias::new("enabled"),
				])
				.from_subquery(
					reinhardt::query::Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.expr(reinhardt::query::Expr::cust("'http://localhost:1'"))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						))
						.expr(reinhardt::query::Expr::cust("'0.1'"))
						.expr(reinhardt::query::Expr::cust("TRUE"))
						.to_owned(),
				)
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(&f.store.pool)
		.await
	}
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
	let app = common::application(f.clone()).await;
	let (mut policy, subject_token, task_id) = bootstrap(&f, &app, "http://localhost:1").await;
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
	{
		let query_bind_1 = other_workspace;
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("workspaces"))
				.columns([Alias::new("id"), Alias::new("title"), Alias::new("goal")])
				.from_subquery(
					reinhardt::query::Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.expr(Expr::cust("'Other tenant'"))
						.expr(Expr::cust("'Unrelated'"))
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&f.store.pool)
		.await
	}
	.unwrap();
	{
		let query_bind_1 = other_workspace;
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("authorization_workspaces"))
				.columns([
					Alias::new("workspace_id"),
					Alias::new("tenant"),
					Alias::new("owner_subject"),
				])
				.from_subquery(
					reinhardt::query::Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.expr(Expr::cust("'other'"))
						.expr(Expr::cust("'alice'"))
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&f.store.pool)
		.await
	}
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
				IntoValue::into_value(Uuid::new_v4()),
				IntoValue::into_value(&f.config.node_id),
				IntoValue::into_value(other_workspace),
				IntoValue::into_value("workspace.updated"),
				IntoValue::into_value(json!({})),
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
	let workspace_id = f
		.store
		.tasks(None)
		.await
		.unwrap()
		.into_iter()
		.find(|task| task.id == task_id)
		.unwrap()
		.workspace_id;
	let mut tasks = Query::insert();
	tasks
		.into_table(Alias::new("tasks"))
		.columns(["id", "workspace_id", "title", "description", "created_by"].map(Alias::new));
	for index in 1..=81_u128 {
		tasks.values_panic([
			IntoValue::into_value(Uuid::from_u128(index + 1000)),
			IntoValue::into_value(workspace_id),
			IntoValue::into_value(format!("Task {index}")),
			IntoValue::into_value("Fixture"),
			IntoValue::into_value("alice"),
		]);
	}
	sqlx::query(&tasks.to_string(PostgresQueryBuilder))
		.execute(&f.store.pool)
		.await
		.unwrap();
	let mut runs = Query::insert();
	runs.into_table(Alias::new("runs")).columns(
		[
			"id",
			"task_id",
			"workspace_id",
			"home_node",
			"agent_id",
			"agent_version",
			"phase",
			"control",
		]
		.map(Alias::new),
	);
	for index in 1..=81_u128 {
		runs.values_panic([
			IntoValue::into_value(Uuid::from_u128(index)),
			IntoValue::into_value(Uuid::from_u128(index + 1000)),
			IntoValue::into_value(workspace_id),
			IntoValue::into_value(&f.config.node_id),
			IntoValue::into_value("research"),
			IntoValue::into_value("1.0.0"),
			IntoValue::into_value("THINKING"),
			IntoValue::into_value(if index == 1 { "PAUSED" } else { "ACTIVE" }),
		]);
	}
	sqlx::query(&runs.to_string(PostgresQueryBuilder))
		.execute(&f.store.pool)
		.await
		.unwrap();
	let mut options = json!({"kinds":["task","run","agent"],"relations":["executes"],"limit":80});
	let mut run_page = Value::Null;
	for _ in 0..4 {
		let (status, page) = graph_custom(&app, &peer_token, viewer.clone(), options.clone()).await;
		assert_eq!(status, 200, "{page}");
		if page["nodes"]
			.as_array()
			.unwrap()
			.iter()
			.any(|node| node["kind"] == "run")
		{
			run_page = page;
			break;
		}
		options["cursor"] = page["next_cursor"].clone();
		assert!(options["cursor"].is_string());
	}
	assert!(run_page.is_object(), "Run page was not reached");
	let nodes = run_page["nodes"].as_array().unwrap();
	let paused = nodes
		.iter()
		.find(|node| node["kind"] == "run" && node["resource_id"] == Uuid::from_u128(1).to_string())
		.unwrap();
	assert_eq!(paused["status"], "PAUSED");
	let agent = nodes.iter().find(|node| node["kind"] == "agent").unwrap();
	let task = nodes
		.iter()
		.find(|node| {
			node["kind"] == "task" && node["resource_id"] == Uuid::from_u128(1001).to_string()
		})
		.unwrap();
	let edges = run_page["edges"].as_array().unwrap();
	assert!(
		edges
			.iter()
			.any(|edge| edge["source"] == agent["id"] && edge["target"] == paused["id"])
	);
	assert!(
		edges
			.iter()
			.any(|edge| edge["source"] == paused["id"] && edge["target"] == task["id"])
	);
	assert!(run_page["next_cursor"].is_string());
	let (status, _) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/acme/catalog",
		json!({"entry":{"id":"research","version":"1.0.0"},"expected_revision":1,"enabled":false}),
	)
	.await;
	assert_eq!(status, 200);
	let (status, unapproved) = graph_custom(
		&app,
		&peer_token,
		viewer.clone(),
		json!({"kinds":["run","agent"],"relations":["executes"],"limit":80}),
	)
	.await;
	assert_eq!(status, 200, "{unapproved}");
	assert!(
		unapproved["nodes"]
			.as_array()
			.unwrap()
			.iter()
			.all(|node| node["kind"] != "agent")
	);
	assert!(unapproved["edges"].as_array().unwrap().is_empty());
	let (status, _) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/acme/catalog",
		json!({"entry":{"id":"research","version":"1.0.0"},"expected_revision":2,"enabled":true}),
	)
	.await;
	assert_eq!(status, 200);
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
			"entry":{"id":"research","version":"1.0.0"},"expected_revision":3,"enabled":false,
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
			"entry":{"id":"research","version":"1.0.0"},"expected_revision":4,"enabled":true,
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
				IntoValue::into_value(gate),
				IntoValue::into_value(&f.config.node_id),
				IntoValue::into_value("fixture"),
				IntoValue::into_value(json!({})),
				IntoValue::into_value("PREPARED"),
			])
			.to_string(PostgresQueryBuilder),
	)
	.execute(&f.store.control_pool)
	.await
	.unwrap();
	{
		let query_bind_1 = gate;
		sqlx::query(
			&Query::update()
				.table(Alias::new("atomic_gate"))
				.value_expr(
					Alias::new("transaction_id"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&f.store.control_pool)
		.await
	}
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
	{
		let query_bind_1 = Option::<Uuid>::None;
		sqlx::query(
			&Query::update()
				.table(Alias::new("atomic_gate"))
				.value_expr(
					Alias::new("transaction_id"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&f.store.control_pool)
		.await
	}
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
	let app = common::application(f.clone()).await;
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
				IntoValue::into_value(*id),
				IntoValue::into_value("Hidden"),
				IntoValue::into_value("Hidden goal"),
			]);
			authorities.values_panic([
				IntoValue::into_value(*id),
				IntoValue::into_value("sparse"),
				IntoValue::into_value("alice"),
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
	{
		let query_bind_1 = final_id;
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("workspaces"))
				.columns([Alias::new("id"), Alias::new("title"), Alias::new("goal")])
				.from_subquery(
					reinhardt::query::Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.expr(Expr::cust("'Last'"))
						.expr(Expr::cust("'Reachable goal'"))
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&f.store.pool)
		.await
	}
	.unwrap();
	{
		let query_bind_1 = final_id;
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("authorization_workspaces"))
				.columns([
					Alias::new("workspace_id"),
					Alias::new("tenant"),
					Alias::new("owner_subject"),
				])
				.from_subquery(
					reinhardt::query::Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.expr(Expr::cust("'sparse'"))
						.expr(Expr::cust("'alice'"))
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&f.store.pool)
		.await
	}
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
			IntoValue::into_value(Uuid::new_v4()),
			IntoValue::into_value(&f.config.node_id),
			IntoValue::into_value(id),
			IntoValue::into_value(kind),
			IntoValue::into_value(json!({})),
			IntoValue::into_value(occurred),
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

use reinhardt::query::QueryStatementBuilder as _;

use reinhardt::query::IntoValue;

use reinhardt::query::SimpleExpr;
