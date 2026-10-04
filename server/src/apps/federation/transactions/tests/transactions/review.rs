use super::*;
use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};

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
	let app = common::application(f.clone()).await;
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
		aidash_server::transactions::coordinator::status(&f, Uuid::parse_str(id).unwrap())
			.await
			.unwrap()
			.decision
			.is_none()
	);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn authorization_submission_and_abort_work_with_one_control_connection(
	#[future(awt)]
	#[from(test_environment)]
	environment: std::sync::Arc<TestEnvironment>,
) {
	let (mut f, url, schema) = setup(&environment).await;
	let app = common::application(f.clone()).await;
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
	let app = common::application(f.clone()).await;
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
	let (status, rows) = request(&app, &token, "GET", "/api/transactions", Value::Null).await;
	assert_eq!(
		status, 200,
		"listing must work with one control connection: {rows}"
	);
	assert_eq!(rows.as_array().unwrap().len(), 1);
	let (status, body) = request(
		&app,
		&token,
		"POST",
		&format!(
			"/api/transactions/{}/abort",
			manifest["id"].as_str().unwrap()
		),
		Value::Null,
	)
	.await;
	assert_eq!(
		status, 200,
		"subject abort must reuse its authority connection: {body}"
	);
	assert_eq!(body["decision"], "ABORT");
	cleanup(f, &url, &schema).await;
}

async fn audit_count(f: &aidash_server::federation::Federation) -> i64 {
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
	let app = common::application_with_settings(
		f.clone(),
		aidash_server::http::Settings {
			auth_burst: 1000,
			actor_burst: 1000,
			..Default::default()
		},
	)
	.await;
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

#[rstest::rstest]
#[tokio::test]
async fn peer_mapping_writes_work_with_one_control_connection(
	#[future(awt)]
	#[from(test_environment)]
	environment: std::sync::Arc<TestEnvironment>,
) {
	let (mut f, url, schema) = setup(&environment).await;
	let app = common::application(f.clone()).await;
	bootstrap(&f, &app, "http://localhost:1").await;
	let (status, credential) = request(
		&app,
		&f.config.api_token,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"alice"}),
	)
	.await;
	assert_eq!(status, 200);
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
			.values_panic::<_, reinhardt::query::Value>([
				"aidash://source".into(),
				"http://localhost:1".into(),
				"AIDASH_SECRET_TEST_PEER".into(),
				"0.1".into(),
				true.into(),
			])
			.to_string(PostgresQueryBuilder),
	)
	.execute(&f.store.pool)
	.await
	.unwrap();
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
	let app = common::application(f.clone()).await;
	for (revision, enabled) in [(0, true), (1, true), (2, false)] {
		let (status, body) = request(&app, &f.config.api_token, "POST", "/api/authorization/acme/peer-mappings",
            json!({"source_node":"aidash://source","source_tenant":"remote","source_subject":"bob","credential_id":credential["credential"]["id"],"enabled":enabled,"expected_revision":revision})).await;
		assert_eq!(
			status, 200,
			"mapping change must not acquire a second control slot: {body}"
		);
		assert_eq!(body["revision"], revision + 1);
	}
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn subject_transaction_errors_do_not_reveal_other_owners(
	#[future(awt)]
	#[from(test_environment)]
	environment: std::sync::Arc<TestEnvironment>,
) {
	let (f, url, schema) = setup(&environment).await;
	let app = common::application(f.clone()).await;
	let (_, token, task) = bootstrap(&f, &app, "http://localhost:1").await;
	let workspace = f.store.task(task).await.unwrap().workspace_id;
	let own = manifest(&f.config.node_id, workspace);
	assert_eq!(
		request(&app, &token, "POST", "/api/transactions", own.clone())
			.await
			.0,
		202
	);
	let operator = manifest(&f.config.node_id, workspace);
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/transactions",
			operator.clone()
		)
		.await
		.0,
		202
	);
	// The foreign subject's binding is immutable fixture state. Its transaction
	// exists and is visible to the operator, but must look absent to Alice.
	let foreign = manifest(&f.config.node_id, workspace);
	assert_eq!(
		request(
			&app,
			&f.config.api_token,
			"POST",
			"/api/transactions",
			foreign.clone()
		)
		.await
		.0,
		202
	);
	{
		let query_bind_1 = Uuid::parse_str(foreign["id"].as_str().unwrap()).unwrap();
		let query_bind_2 = json!({"tenant":"acme","subject":"bob","credential_id":Uuid::new_v4()});
		sqlx::query(
			&Query::insert()
				.into_table(Alias::new("atomic_subjects"))
				.columns([Alias::new("id"), Alias::new("binding")])
				.from_subquery(
					reinhardt::query::Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						))
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&f.store.control_pool)
		.await
	}
	.unwrap();
	for (method, suffix) in [("GET", ""), ("POST", "/abort")] {
		let missing = request(
			&app,
			&token,
			method,
			&format!("/api/transactions/{}{suffix}", Uuid::new_v4()),
			Value::Null,
		)
		.await;
		assert_eq!(missing.0, 404);
		for id in [&operator["id"], &foreign["id"]] {
			let path = format!("/api/transactions/{}{suffix}", id.as_str().unwrap());
			assert_eq!(
				request(&app, &token, method, &path, Value::Null).await,
				missing
			);
		}
		assert_eq!(
			request(
				&app,
				&token,
				method,
				&format!("/api/transactions/{}{suffix}", own["id"].as_str().unwrap()),
				Value::Null
			)
			.await
			.0,
			200
		);
	}
	cleanup(f, &url, &schema).await;
}

use reinhardt::query::QueryStatementBuilder as _;

use reinhardt::query::SimpleExpr;
