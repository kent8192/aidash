use super::*;
use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};

fn manifest(node: &str, workspace: Uuid) -> Value {
	json!({"id":Uuid::new_v4(),"coordinator":node,"isolation":"serializable", "deadline":Utc::now()+Duration::minutes(5),
		"participants":[{"node_id":node,"mutations":[{"kind":"workspace_state","workspace_id":workspace,"expected_revision":0,"state":{}}]}]})
}

#[rstest::rstest]
#[case("coordinator")]
#[case("participants")]
#[tokio::test]
async fn abort_honors_manifest_attribute_denies(
	#[future(awt)]
	#[from(test_environment)]
	environment: std::sync::Arc<TestEnvironment>,
	#[case] attribute: &str,
) {
	let (f, url, schema) = setup(&environment).await;
	let app = api::router(f.clone());
	let (mut policy, token, task) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let workspace = f.store.task(task).await.unwrap().workspace_id;
	let manifest = manifest(&f.config.node_id, workspace);
	assert_eq!(
		request(&app, &token, "POST", "/api/transactions", manifest.clone())
			.await
			.0,
		202
	);
	let expected = if attribute == "coordinator" {
		json!(f.config.node_id)
	} else {
		json!([f.config.node_id])
	};
	policy["policies"].as_array_mut().unwrap().push(json!({
		"id":"deny-abort","effect":"deny","subjects":{"any":true},"actions":["transaction.abort"],"resources":{"kinds":["transaction"]},
		"condition":{"op":"eq","left":{"source":"resource","path":format!("/{attribute}")},"right":{"source":"literal","value":expected}}
	}));
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":1,"bundle":policy})
		)
		.await
		.0,
		200
	);
	let id = manifest["id"].as_str().unwrap();
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&format!("/api/transactions/{id}/abort"),
			Value::Null
		)
		.await
		.0,
		403
	);
	assert!(
		aidash::transactions::coordinator::status(&f, Uuid::parse_str(id).unwrap())
			.await
			.unwrap()
			.decision
			.is_none()
	);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn authorization_and_submission_work_with_one_control_connection(
	#[future(awt)]
	#[from(test_environment)]
	environment: std::sync::Arc<TestEnvironment>,
) {
	let (mut f, url, schema) = setup(&environment).await;
	let app = api::router(f.clone());
	let (_, token, task) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let workspace = f.store.task(task).await.unwrap().workspace_id;
	let old_pool = f.store.control_pool.clone();
	f.store.control_pool = old_pool
		.options()
		.clone()
		.max_connections(1)
		.acquire_timeout(std::time::Duration::from_secs(2))
		.connect_with(old_pool.connect_options().as_ref().clone())
		.await
		.unwrap();
	drop(app);
	old_pool.close().await;
	let app = api::router(f.clone());
	// Each ordinary route holds its visibility lease in the only control slot.
	for path in ["catalog", "revisions", "decisions"] {
		let (status, body) = request(
			&app,
			&f.config.api_token,
			"GET",
			&format!("/api/authorization/acme/{path}"),
			Value::Null,
		)
		.await;
		assert_eq!(status, 200, "{path}: {body}");
	}
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/authorization/acme/credentials",
			json!({"subject":"alice"})
		)
		.await
		.0,
		200
	);
	let manifest = manifest(&f.config.node_id, workspace);
	for _ in 0..2 {
		let (status, body) =
			request(&app, &token, "POST", "/api/transactions", manifest.clone()).await;
		assert_eq!(
			status, 202,
			"new and retried submission must not require another control slot: {body}"
		);
	}
	cleanup(f, &url, &schema).await;
}

async fn audit_count(f: &aidash::federation::Federation) -> i64 {
	sqlx::query_scalar(
		&Query::select()
			.expr(Expr::cust("COUNT(*)"))
			.from(Alias::new("authorization_decisions"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&f.store.pool)
	.await
	.unwrap()
}

#[rstest::rstest]
#[tokio::test]
async fn transaction_visibility_scans_past_hidden_rows_without_poll_audits(
	#[future(awt)]
	#[from(test_environment)]
	environment: std::sync::Arc<TestEnvironment>,
) {
	let (f, url, schema) = setup(&environment).await;
	let app = api::router_with_settings(
		f.clone(),
		aidash::http::Settings {
			auth_burst: 1000,
			actor_burst: 1000,
			..Default::default()
		},
	);
	let (mut policy, token, task) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let workspace = f.store.task(task).await.unwrap().workspace_id;
	let oldest = manifest(&f.config.node_id, workspace);
	assert_eq!(
		request(&app, &token, "POST", "/api/transactions", oldest.clone())
			.await
			.0,
		202
	);
	let mut hidden = Vec::new();
	for _ in 0..201 {
		let next = manifest(&f.config.node_id, workspace);
		hidden.push(next["id"].clone());
		let (status, body) = request(&app, &token, "POST", "/api/transactions", next).await;
		assert_eq!(status, 202, "{body}");
	}
	policy["policies"].as_array_mut().unwrap().push(json!({"id":"hide-newer","effect":"deny","subjects":{"any":true},"actions":["transaction.read"],"resources":{"kinds":["transaction"],"ids":hidden}}));
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":1,"bundle":policy})
		)
		.await
		.0,
		200
	);
	let before = audit_count(&f).await;
	for _ in 0..2 {
		let (status, rows) = request(&app, &token, "GET", "/api/transactions", Value::Null).await;
		assert_eq!(status, 200, "{rows}");
		assert_eq!(rows.as_array().unwrap().len(), 1);
		assert_eq!(rows[0]["id"], oldest["id"]);
	}
	assert_eq!(
		audit_count(&f).await,
		before,
		"unchanged list polls must not persist per-row decisions"
	);
	cleanup(f, &url, &schema).await;
}
