mod common;
use aidash::{api, domain::qualified_agent, registry::digest};
use axum::{Router, body::Body, http::Request};
use common::*;
use common::{TestEnvironment, test_environment};
use serde_json::{Value, json};
use tower::ServiceExt;

async fn inspect(app: &Router, token: &str, input: Value) -> (u16, Value) {
	let response = app
		.clone()
		.oneshot(
			Request::builder()
				.method("POST")
				.uri("/federation/v0.1/scoped/execution/inspect")
				.header("authorization", format!("Bearer {token}"))
				.header("x-aidash-node", "aidash://source")
				.header("x-aidash-protocol", "0.1")
				.header("content-type", "application/json")
				.body(Body::from(input.to_string()))
				.unwrap(),
		)
		.await
		.unwrap();
	let status = response.status().as_u16();
	let body = axum::body::to_bytes(response.into_body(), 1_048_576)
		.await
		.unwrap();
	(status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

#[rstest::rstest]
#[tokio::test]
async fn receiver_preflight_intersects_executor_and_mapping_without_admitting_a_run(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (f, url, schema) = setup().await;
	let app = api::router(f.clone());
	let (policy, user_token, _) = bootstrap(&f, &app, "http://localhost:1").await;
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
				sea_orm::sea_query::Expr::cust("'aidash://source'"),
				sea_orm::sea_query::Expr::cust("'http://localhost:1'"),
				sea_orm::sea_query::Expr::cust("'AIDASH_SECRET_TEST_PEER'"),
				sea_orm::sea_query::Expr::cust("'0.1'"),
				sea_orm::sea_query::Expr::cust("TRUE"),
			])
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.execute(&f.store.pool)
	.await
	.unwrap();
	let token = std::env::var("AIDASH_SECRET_TEST_PEER").unwrap();
	let input = json!({"tenant":"remote","subject":"bob","agent":{"id":"research","version":"1.0.0"},"requirements":{}});
	assert_eq!(inspect(&app, &token, input.clone()).await.0, 403);
	let (_, issued) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"alice"}),
	)
	.await;
	let credential = &issued["credential"]["id"];
	assert_eq!(request(&app,&f.config.api_token,"POST","/api/authorization/acme/peer-mappings",json!({"source_node":"aidash://source","source_tenant":"remote","source_subject":"bob","credential_id":credential,"enabled":true,"expected_revision":0})).await.0,200);
	for rejected in [&user_token, &f.config.api_token] {
		assert_eq!(inspect(&app, rejected, input.clone()).await.0, 401);
	}
	let (status, result) = inspect(&app, &token, input.clone()).await;
	assert_eq!(status, 200, "{result}");
	assert_eq!(result["node_id"], f.config.node_id);
	assert_eq!(result["agent"]["id"], "research");
	let definitions = result["definitions"].as_array().unwrap();
	assert_eq!(definitions.len(), 3);
	for definition in definitions {
		let entry = f
			.registry
			.get(definition["entry"]["id"].as_str().unwrap(), "1.0.0")
			.await
			.unwrap();
		assert_eq!(
			definition["digest"],
			digest(&serde_json::to_value(entry).unwrap())
		);
		assert_eq!(definition["metadata"]["id"], definition["entry"]["id"]);
	}
	let mut mismatch = input.clone();
	mismatch["requirements"] = json!({"capability":"missing-capability"});
	assert_eq!(inspect(&app, &token, mismatch).await.0, 400);
	let executor = qualified_agent(&f.config.node_id, "research", "1.0.0");
	// Every denied action is tested for both the mapped root and the receiver's
	// executor. An allow on one must never override the other's explicit deny.
	let mut revision = 1;
	for subject in ["alice", executor.as_str()] {
		for (action, kind) in [
			("federation.execute", "node"),
			("agent.execute", "agent"),
			("registry.read", "agent"),
			("registry.read", "model"),
			("model.infer", "model"),
			("tool.invoke", "tool"),
		] {
			let mut denied = policy.clone();
			denied["policies"].as_array_mut().unwrap().push(json!({"id":"deny","effect":"deny","subjects":{"ids":[subject]},"actions":[action],"resources":{"kinds":[kind]}}));
			assert_eq!(
				request(
					&app,
					&f.config.api_token,
					"POST",
					"/api/authorization/acme",
					json!({"expected_revision":revision,"bundle":denied})
				)
				.await
				.0,
				200
			);
			revision += 1;
			assert_eq!(
				inspect(&app, &token, input.clone()).await.0,
				403,
				"{subject} {action} {kind}"
			);
		}
	}
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":revision,"bundle":policy})
		)
		.await
		.0,
		200
	);
	for id in ["model", "http", "research"] {
		assert_eq!(
			request(
				&app,
				&f.config.api_token,
				"POST",
				"/api/authorization/acme/catalog",
				json!({"entry":{"id":id,"version":"1.0.0"},"expected_revision":1,"enabled":false})
			)
			.await
			.0,
			200
		);
		assert_eq!(inspect(&app, &token, input.clone()).await.0, 403);
		assert_eq!(
			request(
				&app,
				&f.config.api_token,
				"POST",
				"/api/authorization/acme/catalog",
				json!({"entry":{"id":id,"version":"1.0.0"},"expected_revision":2,"enabled":true})
			)
			.await
			.0,
			200
		);
	}
	assert_eq!(inspect(&app, &token, input.clone()).await.0, 200);
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			&format!(
				"/api/authorization/acme/credentials/{}/revoke",
				credential.as_str().unwrap()
			),
			json!({})
		)
		.await
		.0,
		200
	);
	assert_eq!(inspect(&app, &token, input).await.0, 403);
	let runs: i64 = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::Expr::cust("COUNT(*)"))
			.from(sea_orm::sea_query::Alias::new("runs"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.fetch_one(&f.store.pool)
	.await
	.unwrap();
	assert_eq!(runs, 0, "inspection must not create an executable run");
	cleanup(f, &url, &schema).await;
}
