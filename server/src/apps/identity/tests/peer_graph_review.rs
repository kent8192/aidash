#[path = "../../execution/tests/support/legacy.rs"]
mod common;

use aidash_server::{domain::Task, federation::Federation};
use axum::{body::Body, http::Request};
use common::{TestEnvironment, bootstrap, cleanup, request, setup, test_environment};
use reinhardt::query::{Alias, ColumnRef, Expr, PostgresQueryBuilder, Query};
use serde_json::{Value, json};

use uuid::Uuid;

const SOURCE: &str = "aidash://source";

async fn project(app: &common::TestApplication, viewer: Value, options: Value) -> (u16, Value) {
	let mut input = json!({
		"viewer":viewer,"scope_workspace":null,"mode":"mesh",
		"kinds":["workspace"],"relations":[],"hours":0,"limit":80,"cursor":null,
		"target_tenant":if viewer["kind"] == "operator" { Some("acme") } else { None },
	});
	input
		.as_object_mut()
		.unwrap()
		.extend(options.as_object().unwrap().clone());
	let token = std::env::var("AIDASH_SECRET_TEST_PEER").unwrap();
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
				.body(Body::from(input.to_string()))
				.unwrap(),
		)
		.await
		.unwrap();
	let status = response.status().as_u16();
	let body = axum::body::to_bytes(response.into_body(), 4_194_304)
		.await
		.unwrap();
	(status, serde_json::from_slice(&body).unwrap())
}

async fn credential(f: &Federation, app: &common::TestApplication, tenant: &str) -> Uuid {
	let (status, issued) = request(
		app,
		&f.config.api_token,
		"POST",
		&format!("/api/authorization/{tenant}/credentials"),
		json!({"subject":"alice"}),
	)
	.await;
	assert_eq!(status, 200, "{issued}");
	issued["credential"]["id"]
		.as_str()
		.unwrap()
		.parse()
		.unwrap()
}

async fn viewer(f: &Federation, app: &common::TestApplication) -> (Value, Uuid) {
	{
		let query_bind_1 = SOURCE;
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
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.expr(Expr::cust("'http://localhost:1'"))
						.expr(Expr::cust("'AIDASH_SECRET_TEST_PEER'"))
						.expr(Expr::cust("'0.1'"))
						.expr(Expr::cust("TRUE"))
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&f.store.pool)
		.await
	}
	.unwrap();
	let id = credential(f, app, "acme").await;
	let (status, mapping) = request(
		app,
		&f.config.api_token,
		"POST",
		"/api/authorization/acme/peer-mappings",
		json!({
			"source_node":SOURCE,"source_tenant":"source-tenant","source_subject":"source-subject",
			"credential_id":id,"enabled":true,"expected_revision":0,
		}),
	)
	.await;
	assert_eq!(status, 200, "{mapping}");
	(
		json!({"kind":"subject","tenant":"source-tenant","subject":"source-subject"}),
		id,
	)
}

/// Activation writes a receiver run and an admission, not a local workspace/task.
async fn admitted_run(
	f: &Federation,
	template: &Task,
	credential: Uuid,
	tenant: &str,
	source: &str,
	workspace: Uuid,
) -> Uuid {
	let id = Uuid::new_v4();
	let mut task = template.clone();
	task.id = Uuid::new_v4();
	task.workspace_id = workspace;
	let agent: Value = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("metadata"))
			.from(Alias::new("registry"))
			.and_where(Expr::cust("id='research' AND version='1.0.0'"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&f.store.pool)
	.await
	.unwrap();
	let grant = Uuid::new_v4();
	let description = json!({
		"grant_id":grant,"source_node":source,"target_node":f.config.node_id,
		"source_tenant":"source-tenant","source_subject":"source-subject","task":task,
		"inspection":{"node_id":f.config.node_id,"authority_digest":format!("sha256:{}", "0".repeat(64)),
			"agent":agent,"definitions":[]},
		"expires_at":chrono::Utc::now() + chrono::Duration::hours(1),
	});
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("authorization_remote_admissions"))
			.columns(
				[
					"id",
					"source_node",
					"grant_id",
					"task_id",
					"tenant",
					"credential_id",
					"subject_chain",
					"description",
				]
				.map(Alias::new),
			)
			.from_subquery(((1..=8).map(|index| Expr::cust(format!("${index}")))).fold(
				reinhardt::query::Query::select(),
				|mut select, expr| {
					select.expr(expr);
					select
				},
			))
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.bind(source)
	.bind(grant)
	.bind(task.id)
	.bind(tenant)
	.bind(credential)
	.bind(vec![
		"alice".to_owned(),
		aidash_server::domain::qualified_agent(&f.config.node_id, "research", "1.0.0"),
	])
	.bind(description)
	.execute(&f.store.pool)
	.await
	.unwrap();
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("runs"))
			.columns(
				[
					"id",
					"task_id",
					"workspace_id",
					"home_node",
					"agent_id",
					"agent_version",
				]
				.map(Alias::new),
			)
			.from_subquery(((1..=6).map(|index| Expr::cust(format!("${index}")))).fold(
				reinhardt::query::Query::select(),
				|mut select, expr| {
					select.expr(expr);
					select
				},
			))
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.bind(task.id)
	.bind(workspace)
	.bind(source)
	.bind("research")
	.bind("1.0.0")
	.execute(&f.store.pool)
	.await
	.unwrap();
	id
}

async fn event(f: &Federation, workspace: Option<Uuid>, kind: &str, data: Value, age: i64) {
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("events"))
			.columns(
				[
					"id",
					"node_id",
					"workspace_id",
					"kind",
					"data",
					"created_at",
				]
				.map(Alias::new),
			)
			.from_subquery(((1..=6).map(|index| Expr::cust(format!("${index}")))).fold(
				reinhardt::query::Query::select(),
				|mut select, expr| {
					select.expr(expr);
					select
				},
			))
			.to_string(PostgresQueryBuilder),
	)
	.bind(Uuid::new_v4())
	.bind(&f.config.node_id)
	.bind(workspace)
	.bind(kind)
	.bind(data)
	.bind(chrono::Utc::now() - chrono::Duration::seconds(age))
	.execute(&f.store.pool)
	.await
	.unwrap();
}

async fn bump_run(f: &Federation, id: Uuid) {
	{
		let query_bind_1 = id;
		sqlx::query(
			&Query::update()
				.table(Alias::new("runs"))
				.value_expr(Alias::new("revision"), Expr::cust("revision + 1"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(id=?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&f.store.pool)
		.await
	}
	.unwrap();
}

fn run_ids(page: &Value) -> Vec<Uuid> {
	page["nodes"]
		.as_array()
		.unwrap()
		.iter()
		.filter(|node| node["kind"] == "run")
		.map(|node| node["resource_id"].as_str().unwrap().parse().unwrap())
		.collect()
}

#[rstest::rstest]
#[tokio::test]
async fn admitted_graph_runs_use_receiver_authority_and_scope_bound_generations(
	#[future(awt)]
	#[from(test_environment)]
	environment: std::sync::Arc<TestEnvironment>,
) {
	let (f, url, schema) = setup(&environment).await;
	let app = common::application(f.clone()).await;
	let (mut policy, token, task_id) = bootstrap(&f, &app, "http://localhost:1").await;
	let (subject, credential) = viewer(&f, &app).await;
	let task: Task = {
		let query_bind_1 = task_id;
		aidash_server::database::query_as(
			&Query::select()
				.column(ColumnRef::Asterisk)
				.from(Alias::new("tasks"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(id=?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(&f.store.pool)
		.await
	}
	.unwrap();
	let workspace = Uuid::new_v4();
	let mut expected = vec![];
	for _ in 0..3 {
		expected.push(admitted_run(&f, &task, credential, "acme", SOURCE, workspace).await);
	}
	let third = admitted_run(&f, &task, credential, "acme", "aidash://third", workspace).await;
	let other_scope = admitted_run(&f, &task, credential, "acme", SOURCE, Uuid::new_v4()).await;
	let mut other_policy = common::policy(&f.config.node_id);
	other_policy["tenant"] = json!("other");
	let (status, value) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/other",
		json!({"expected_revision":0,"bundle":other_policy}),
	)
	.await;
	assert_eq!(status, 200, "{value}");
	let other_credential = self::credential(&f, &app, "other").await;
	let other_tenant = admitted_run(&f, &task, other_credential, "other", SOURCE, workspace).await;
	for index in 0..6 {
		event(
			&f,
			None,
			"run.phase",
			json!({"run_id":expected[0],"workspace_id":workspace}),
			60 - index,
		)
		.await;
	}
	event(
		&f,
		None,
		"run.phase",
		json!({"run_id":third,"workspace_id":workspace}),
		1,
	)
	.await;
	let options =
		json!({"scope_workspace":workspace,"kinds":["run","agent"],"relations":["executes"]});
	let (status, page) = project(&app, subject.clone(), options.clone()).await;
	assert_eq!(status, 200, "{page}");
	let mut actual = run_ids(&page);
	actual.sort();
	expected.sort();
	assert_eq!(actual, expected);
	for denied in [third, other_scope, other_tenant] {
		assert!(!actual.contains(&denied));
	}
	let activity = page["activity"].as_array().unwrap();
	assert_eq!(activity.len(), 6);
	assert!(
		activity
			.windows(2)
			.all(|pair| { pair[0]["at"].as_str().unwrap() >= pair[1]["at"].as_str().unwrap() })
	);
	let local_workspace_count: i64 = {
		let query_bind_1 = workspace;
		sqlx::query_scalar(
			&Query::select()
				.expr(Expr::cust("COUNT(*)"))
				.from(Alias::new("authorization_workspaces"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(workspace_id=?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(&f.store.pool)
		.await
	}
	.unwrap();
	assert_eq!(local_workspace_count, 0);
	let operator = Uuid::new_v4();
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
	let (status, operator_page) = project(
		&app,
		json!({"kind":"operator","id":operator}),
		options.clone(),
	)
	.await;
	assert_eq!(status, 200, "{operator_page}");
	let mut actual = run_ids(&operator_page);
	actual.sort();
	assert_eq!(actual, expected);
	let mut paged = options.clone();
	paged["limit"] = json!(2);
	let (status, first) = project(&app, subject.clone(), paged.clone()).await;
	assert_eq!(status, 200, "{first}");
	assert!(first["next_cursor"].is_string(), "{first}");
	paged["cursor"] = first["next_cursor"].clone();
	let (status, created) = request(
		&app,
		&token,
		"POST",
		"/api/workspaces",
		json!({"title":"Unrelated same-tenant activity","goal":"Do not reset the remote cursor"}),
	)
	.await;
	assert_eq!(status, 200, "{created}");
	bump_run(&f, other_scope).await;
	bump_run(&f, third).await;
	bump_run(&f, other_tenant).await;
	event(
		&f,
		None,
		"run.phase",
		json!({"run_id":third,"workspace_id":workspace}),
		1,
	)
	.await;
	let (status, continued) = project(&app, subject.clone(), paged.clone()).await;
	assert_eq!(status, 200, "{continued}");
	bump_run(&f, expected[0]).await;
	assert_eq!(project(&app, subject.clone(), paged).await.0, 409);
	policy["policies"].as_array_mut().unwrap().push(json!({
		"id":"deny-remote-run","effect":"deny","subjects":{"ids":["alice"]},
		"actions":["run.read"],"resources":{"kinds":["run"]},
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
	let (status, denied) = project(&app, subject, options).await;
	assert_eq!(status, 200, "{denied}");
	assert!(run_ids(&denied).is_empty());
	assert!(denied["activity"].as_array().unwrap().is_empty());
	cleanup(f, &url, &schema).await;
}

async fn event_decisions(f: &Federation) -> i64 {
	sqlx::query_scalar(
		&Query::select()
			.expr(Expr::cust("COUNT(*)"))
			.from(Alias::new("authorization_decisions"))
			.and_where(Expr::cust("tenant='acme' AND action='workspace.events'"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&f.store.pool)
	.await
	.unwrap()
}

#[rstest::rstest]
#[tokio::test]
async fn graph_activity_is_newest_first_and_checks_workspace_events_once(
	#[future(awt)]
	#[from(test_environment)]
	environment: std::sync::Arc<TestEnvironment>,
) {
	let (f, url, schema) = setup(&environment).await;
	let app = common::application(f.clone()).await;
	let (mut policy, _, task_id) = bootstrap(&f, &app, "http://localhost:1").await;
	let (subject, _) = viewer(&f, &app).await;
	let workspace: Uuid = {
		let query_bind_1 = task_id;
		sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("workspace_id"))
				.from(Alias::new("tasks"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(id=?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(&f.store.pool)
		.await
	}
	.unwrap();
	for index in 0..6 {
		event(
			&f,
			Some(workspace),
			"workspace.updated",
			json!({}),
			60 - index,
		)
		.await;
	}
	let before = event_decisions(&f).await;
	let (status, page) = project(&app, subject.clone(), json!({})).await;
	assert_eq!(status, 200, "{page}");
	assert_eq!(event_decisions(&f).await - before, 1);
	let activity = page["activity"].as_array().unwrap();
	assert!(activity.len() >= 6);
	assert_eq!(activity[0]["kind"], "workspace.updated");
	let updates: Vec<_> = activity
		.iter()
		.filter(|item| item["kind"] == "workspace.updated")
		.collect();
	assert!(
		updates
			.windows(2)
			.all(|pair| { pair[0]["at"].as_str().unwrap() >= pair[1]["at"].as_str().unwrap() })
	);
	policy["policies"].as_array_mut().unwrap().push(json!({
		"id":"deny-events","effect":"deny","subjects":{"ids":["alice"]},
		"actions":["workspace.events"],"resources":{"kinds":["workspace"]},
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
	let (status, page) = project(&app, subject, json!({})).await;
	assert_eq!(status, 200, "{page}");
	assert!(!page["nodes"].as_array().unwrap().is_empty());
	assert!(page["activity"].as_array().unwrap().is_empty());
	cleanup(f, &url, &schema).await;
}

use reinhardt::query::QueryStatementBuilder as _;

use reinhardt::query::SimpleExpr;
