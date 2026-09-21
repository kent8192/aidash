use aidash::{
	config::Config, domain::qualified_agent, federation::Federation, registry::Registry,
	store::Store,
};
use axum::{Router, body::Body, http::Request};
use serde_json::{Value, json};
use sqlx::{
	Connection, Executor,
	postgres::{PgConnection, PgPoolOptions},
};
use std::sync::Arc;
use tower::ServiceExt;
use uuid::Uuid;

pub async fn request(
	app: &Router,
	token: &str,
	method: &str,
	path: &str,
	value: Value,
) -> (u16, Value) {
	let response = app
		.clone()
		.oneshot(
			Request::builder()
				.method(method)
				.uri(path)
				.header("authorization", format!("Bearer {token}"))
				.header("content-type", "application/json")
				.body(Body::from(value.to_string()))
				.unwrap(),
		)
		.await
		.unwrap();
	let status = response.status().as_u16();
	let bytes = axum::body::to_bytes(response.into_body(), 1_048_576)
		.await
		.unwrap();
	(
		status,
		serde_json::from_slice(&bytes).unwrap_or(Value::Null),
	)
}

pub async fn setup() -> (Federation, String, String) {
	let url = std::env::var("AIDASH_TEST_DATABASE_URL").expect("disposable PostgreSQL required");
	let schema = format!("execution_{}", Uuid::new_v4().simple());
	// SeaQuery has no CREATE/DROP SCHEMA builder; these DDL statements isolate fixtures.
	let mut admin = PgConnection::connect(&url).await.unwrap();
	admin
		.execute(format!("CREATE SCHEMA {schema}").as_str())
		.await
		.unwrap();
	let search = schema.clone();
	let pool = PgPoolOptions::new()
		.max_connections(12)
		.after_connect(move |connection, _| {
			let schema = search.clone();
			Box::pin(async move {
				sqlx::query(
					&sea_orm::sea_query::Query::select()
						.expr(sea_orm::sea_query::Expr::cust(
							"set_config('search_path', $1, false)",
						))
						.to_string(sea_orm::sea_query::PostgresQueryBuilder),
				)
				.bind(&schema)
				.execute(&mut *connection)
				.await?;
				sqlx::query(
					&sea_orm::sea_query::Query::select()
						.expr(sea_orm::sea_query::Expr::cust(
							"SET_CONFIG('application_name', $1, FALSE)",
						))
						.to_string(sea_orm::sea_query::PostgresQueryBuilder),
				)
				.bind(&schema)
				.execute(connection)
				.await?;
				Ok(())
			})
		})
		.connect(&url)
		.await
		.unwrap();
	aidash::store::Store::migrate(&pool).await.unwrap();
	let store = Store::from_pool(pool.clone(), "aidash://execution-test".into())
		.await
		.unwrap();
	let federation = Federation {
		store,
		registry: Registry::new(pool, "aidash://execution-test"),
		config: Config {
			node_id: "aidash://execution-test".into(),
			endpoint: "http://localhost:8080".into(),
			listen: "127.0.0.1:0".parse().unwrap(),
			database_url: url.clone(),
			nats_url: "nats://127.0.0.1:4222".into(),
			api_token: "operator-execution-fixture".into(),
			web_dir: "web/dist".into(),
			lease_seconds: 30,
		},
		client: reqwest::Client::new(),
		notify: Arc::new(tokio::sync::Notify::new()),
	};
	(federation, url, schema)
}

pub async fn cleanup(f: Federation, url: &str, schema: &str) {
	f.store.control_pool.close().await;
	f.store.pool.close().await;
	PgConnection::connect(url)
		.await
		.unwrap()
		.execute(format!("DROP SCHEMA {schema} CASCADE").as_str())
		.await
		.unwrap();
}

pub fn policy(node: &str) -> Value {
	let agent = qualified_agent(node, "research", "1.0.0");
	json!({"tenant":"acme","subjects":{"alice":{"kind":"user"},agent:{"kind":"agent"}},
        "policies":[{"id":"approved-work","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}}]})
}

pub async fn bootstrap(f: &Federation, app: &Router, endpoint: &str) -> (Value, String, Uuid) {
	let operator = &f.config.api_token;
	let policy = policy(&f.config.node_id);
	assert_eq!(
		request(
			app,
			operator,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":0,"bundle":policy})
		)
		.await
		.0,
		200
	);
	for (kind, id, config) in [
		(
			"model",
			"model",
			json!({"provider":"openai","model_id":"fixture","endpoint":format!("{endpoint}/v1"),"context_window":128000,"modalities":["text"],"cost":{}}),
		),
		(
			"tool",
			"http",
			json!({"transport":"http","endpoint":format!("{endpoint}/effect"),"credential_env":null,"replay":"idempotent"}),
		),
		(
			"agent",
			"research",
			json!({"model":{"id":"model","version":"1.0.0"},"instructions":"Test approved work","tools":[{"id":"http","version":"1.0.0"}],"skills":[]}),
		),
	] {
		let entry = json!({"id":id,"version":"1.0.0","kind":kind,"name":{"en":id},"description":{"en":"fixture"},"capabilities":[],"languages":["en"],"schema":{"type":"object"},"config":config});
		assert_eq!(
			request(app, operator, "POST", "/api/registry", entry)
				.await
				.0,
			200
		);
		let (status, response) = request(
			app,
			operator,
			"POST",
			"/api/authorization/acme/catalog",
			json!({"entry":{"id":id,"version":"1.0.0"},"expected_revision":0,"enabled":true}),
		)
		.await;
		assert_eq!(status, 200, "catalog admission: {response}");
	}
	let (status, credential) = request(
		app,
		operator,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"alice"}),
	)
	.await;
	assert_eq!(status, 200);
	let token = credential["token"].as_str().unwrap().to_owned();
	let (status, workspace) = request(
		app,
		&token,
		"POST",
		"/api/workspaces",
		json!({"title":"Approved task","goal":"Use exactly one tool"}),
	)
	.await;
	assert_eq!(status, 200);
	let (status, task) = request(
		app,
		&token,
		"POST",
		&format!(
			"/api/workspaces/{}/tasks",
			workspace["id"].as_str().unwrap()
		),
		json!({"title":"Research","description":"Use approved tools"}),
	)
	.await;
	assert_eq!(status, 200);
	(
		policy,
		token,
		Uuid::parse_str(task["id"].as_str().unwrap()).unwrap(),
	)
}
