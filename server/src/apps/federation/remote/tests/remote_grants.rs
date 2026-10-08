#[path = "../../../execution/tests/support/legacy.rs"]
mod common;
use aidash_server::{domain::qualified_agent, federation::Peer};
use common::*;
use serde_json::{Value, json};
use uuid::Uuid;
async fn verify(app: &common::TestApplication, node: &str, grant: Uuid) -> (u16, Value) {
	grant_request(app, node, grant, "verify").await
}
async fn grant_request(
	app: &common::TestApplication,
	node: &str,
	grant: Uuid,
	operation: &str,
) -> (u16, Value) {
	let authorization = format!(
		"Bearer {}",
		std::env::var("AIDASH_SECRET_TEST_PEER").unwrap()
	);
	let response = app
		.client()
		.post_raw_with_headers(
			&format!("/federation/v0.1/scoped/execution/grants/{operation}"),
			json!({"grant_id":grant}).to_string().as_bytes(),
			"application/json",
			&[
				("authorization", authorization.as_str()),
				("x-aidash-node", node),
				("x-aidash-protocol", "0.2"),
			],
		)
		.await
		.unwrap();
	(
		response.status_code(),
		serde_json::from_slice(response.body()).unwrap_or(Value::Null),
	)
}
#[rstest::rstest]
#[tokio::test]
async fn durable_grants_bind_both_nodes_and_revalidate_after_restarts_and_revocation(
	#[future(awt)]
	#[from(common::native_peer)]
	first: common::PeerFixture,
	#[future(awt)]
	#[from(common::native_peer)]
	#[with("aidash://grant-host")]
	second: common::PeerFixture,
) {
	let (a, au, aschema) = first.runtime.parts();
	let (b, bu, bschema) = second.runtime.parts();
	let aa = first.application;
	let ba = second.application;
	let (mut policy, token, task) = bootstrap(&a, &aa, "http://localhost:1").await;
	bootstrap(&b, &ba, "http://localhost:1").await;
	let executor = qualified_agent(&b.config.node_id, "research", "1.0.0");
	policy["subjects"][&executor] = json!({"kind":"agent"});
	assert_eq!(
		request(
			&aa,
			&a.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":1,"bundle":policy})
		)
		.await
		.0,
		200
	);
	{
		let query_bind_1 = &a.config.node_id;
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
						.expr(reinhardt::query::Expr::cust("'AIDASH_SECRET_TEST_PEER'"))
						.expr(reinhardt::query::Expr::cust("'0.2'"))
						.expr(reinhardt::query::Expr::cust("TRUE"))
						.to_owned(),
				)
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(b.store.pool.driver())
		.await
	}
	.unwrap();

	a.register_peer(Peer {
		node_id: b.config.node_id.clone(),
		endpoint: b.config.endpoint.clone(),
		credential_env: "AIDASH_SECRET_TEST_PEER".into(),
		protocol_version: "0.2".into(),
		enabled: true,
	})
	.await
	.unwrap();
	let (_, issued) = request(
		&ba,
		&b.config.api_token,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"alice"}),
	)
	.await;
	assert_eq!(request(&ba,&b.config.api_token,"POST","/api/authorization/acme/peer-mappings",json!({"source_node":a.config.node_id,"source_tenant":"acme","source_subject":"alice","credential_id":issued["credential"]["id"],"expected_revision":0,"enabled":true})).await.0,200);
	let path = format!("/api/tasks/{task}/remote-grants");
	let id = Uuid::new_v4();
	let input = json!({"id":id,"node_id":b.config.node_id,"agent":{"id":"research","version":"1.0.0"},"ttl_seconds":300});
	assert_eq!(
		request(&aa, &a.config.api_token, "POST", &path, input.clone())
			.await
			.0,
		403
	);
	let (status, prepared) = request(&aa, &token, "POST", &path, input.clone()).await;
	assert_eq!(status, 200, "{prepared}");
	assert_eq!(
		request(&aa, &token, "POST", &path, input.clone()).await,
		(200, prepared.clone()),
		"retry must preserve id and expiry"
	);
	assert_eq!(verify(&aa, &b.config.node_id, id).await, (200, json!(true)));
	// A newly constructed store/router must use the persisted grant, not memory.
	let mut fresh = a.clone();
	fresh.store = aidash_server::store::Store::from_pool(
		a.store
			.pool
			.options()
			.clone()
			.connect_with(a.store.pool.connect_options().as_ref().clone())
			.await
			.unwrap(),
		a.config.node_id.clone(),
	)
	.await
	.unwrap();
	// Act: rebuild the application after replacing the durable store connection.
	let fresh_app = common::application(fresh.clone()).await;
	assert_eq!(verify(&fresh_app, &b.config.node_id, id).await.0, 200);
	let mut different = input.clone();
	different["agent"]["id"] = json!("nonexistent");
	assert_ne!(request(&aa, &token, "POST", &path, different).await.0, 200);
	// Workspace data is filtered before delivery and durable dependencies survive
	// a fresh source connection. Revocation of an already observed sibling must
	// deny the whole grant, even though its execution task is still readable.
	let workspace: Uuid = {
		let query_bind_1 = task;
		sqlx::query_scalar(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::SimpleExpr::from(
					reinhardt::query::Expr::col(reinhardt::query::Alias::new("workspace_id")),
				))
				.from(reinhardt::query::Alias::new("tasks"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_one(a.store.pool.driver())
		.await
	}
	.unwrap();
	let (status, sibling) = request(
		&aa,
		&token,
		"POST",
		&format!("/api/workspaces/{workspace}/tasks"),
		json!({"title":"Sibling secret","description":"Scoped snapshot fixture"}),
	)
	.await;
	assert_eq!(status, 200);
	let sibling_id = sibling["id"].as_str().unwrap();
	let mut hidden = policy.clone();
	hidden["policies"].as_array_mut().unwrap().push(json!({"id":"hide-sibling","effect":"deny","subjects":{"ids":[executor]},"actions":["task.read"],"resources":{"kinds":["task"],"ids":[sibling_id]}}));
	assert_eq!(
		request(
			&aa,
			&a.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":2,"bundle":hidden})
		)
		.await
		.0,
		200
	);
	let (status, snapshot) = grant_request(&fresh_app, &b.config.node_id, id, "snapshot").await;
	assert_eq!(status, 200, "{snapshot}");
	assert!(!snapshot.to_string().contains("Sibling secret"));
	assert!(!snapshot.to_string().contains(sibling_id));
	let tracked: i64 = {
		let query_bind_1 = id;
		let query_bind_2 = Uuid::parse_str(sibling_id).unwrap();
		sqlx::query_scalar(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::Expr::cust("COUNT(*)"))
				.from(reinhardt::query::Alias::new(
					"authorization_remote_grant_reads",
				))
				.and_where(SimpleExpr::CustomWithExpr(
					"(grant_id = ? AND resource_id = ?)".to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
					],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_one(a.store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(
		tracked, 0,
		"filtered data must not become a read dependency"
	);
	assert_eq!(
		request(
			&aa,
			&a.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":3,"bundle":policy})
		)
		.await
		.0,
		200
	);
	let (status, snapshot) = grant_request(&fresh_app, &b.config.node_id, id, "snapshot").await;
	assert_eq!(status, 200, "{snapshot}");
	assert!(snapshot.to_string().contains("Sibling secret"));
	let tracked: i64 = {
		let query_bind_1 = id;
		let query_bind_2 = Uuid::parse_str(sibling_id).unwrap();
		sqlx::query_scalar(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::Expr::cust("COUNT(*)"))
				.from(reinhardt::query::Alias::new(
					"authorization_remote_grant_reads",
				))
				.and_where(SimpleExpr::CustomWithExpr(
					"(grant_id = ? AND resource_kind = 'task' AND resource_id = ?)".to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
					],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_one(a.store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(tracked, 1, "record reads before releasing the response");
	assert_eq!(
		request(
			&aa,
			&a.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":4,"bundle":hidden})
		)
		.await
		.0,
		200
	);
	assert_eq!(verify(&aa, &b.config.node_id, id).await.0, 403);
	assert_eq!(
		grant_request(&fresh_app, &b.config.node_id, id, "snapshot")
			.await
			.0,
		403
	);
	assert_eq!(
		grant_request(&fresh_app, &b.config.node_id, id, "describe")
			.await
			.0,
		403
	);
	assert_eq!(
		request(
			&aa,
			&a.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":5,"bundle":policy})
		)
		.await
		.0,
		200
	);
	assert_eq!(verify(&fresh_app, &b.config.node_id, id).await.0, 200);
	// Receiver model metadata changes invalidate the exact execution snapshot,
	// even when the Agent's public definition remains unchanged.
	let metadata: Value = sqlx::query_scalar(
		&reinhardt::query::Query::select()
			.expr(reinhardt::query::SimpleExpr::from(
				reinhardt::query::Expr::col(reinhardt::query::Alias::new("metadata")),
			))
			.from(reinhardt::query::Alias::new("registry"))
			.and_where(reinhardt::query::Expr::cust("id = 'model'"))
			.to_string(reinhardt::query::PostgresQueryBuilder),
	)
	.fetch_one(b.store.pool.driver())
	.await
	.unwrap();
	let mut changed = metadata.clone();
	changed["description"]["en"] = json!("changed fixture");
	{
		let query_bind_1 = changed;
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("registry"))
				.value_expr(
					reinhardt::query::Alias::new("metadata"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					),
				)
				.and_where(reinhardt::query::Expr::cust("id = 'model'"))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(b.store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(verify(&fresh_app, &b.config.node_id, id).await.0, 403);
	{
		let query_bind_1 = metadata;
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("registry"))
				.value_expr(
					reinhardt::query::Alias::new("metadata"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_1.to_owned()).into()],
					),
				)
				.and_where(reinhardt::query::Expr::cust("id = 'model'"))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(b.store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(verify(&fresh_app, &b.config.node_id, id).await.0, 200);
	let mut denied = policy.clone();
	denied["policies"].as_array_mut().unwrap().push(json!({"id":"deny-remote-model","effect":"deny","subjects":{"ids":[executor]},"actions":["model.infer"],"resources":{"kinds":["model"]},"condition":{"op":"eq","left":{"source":"resource","path":"/config/provider"},"right":{"source":"literal","value":"openrouter"}}}));
	assert_eq!(
		request(
			&aa,
			&a.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":6,"bundle":denied})
		)
		.await
		.0,
		200
	);
	assert_eq!(verify(&fresh_app, &b.config.node_id, id).await.0, 403);
	assert_eq!(request(&aa,&token,"POST",&path,json!({"id":Uuid::new_v4(),"node_id":b.config.node_id,"agent":{"id":"research","version":"1.0.0"},"ttl_seconds":300})).await.0,403);
	assert_eq!(
		request(
			&aa,
			&a.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":7,"bundle":policy})
		)
		.await
		.0,
		200
	);
	assert_eq!(verify(&fresh_app, &b.config.node_id, id).await.0, 200);
	let mut denied_read = policy.clone();
	denied_read["policies"].as_array_mut().unwrap().push(json!({"id":"hide-task","effect":"deny","subjects":{"ids":["alice"]},"actions":["task.read"],"resources":{"kinds":["task"],"ids":[task.to_string()]}}));
	assert_eq!(
		request(
			&aa,
			&a.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":8,"bundle":denied_read})
		)
		.await
		.0,
		200
	);
	let (status, state) = request(&aa, &token, "GET", "/api/state", json!({})).await;
	assert_eq!(status, 200);
	assert!(
		!state.to_string().contains(&id.to_string()),
		"grant audit events must respect task visibility"
	);
	assert_eq!(
		request(
			&aa,
			&a.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":9,"bundle":policy})
		)
		.await
		.0,
		200
	);
	// The fixture has one peer secret. Disable its original binding while
	// testing an authenticated different node, then restore it.
	{
		let query_bind_1 = &b.config.node_id;
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("peers"))
				.value_expr(
					reinhardt::query::Alias::new("enabled"),
					reinhardt::query::Expr::cust("FALSE"),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(node_id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(a.store.pool.driver())
		.await
	}
	.unwrap();
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
					.expr(reinhardt::query::Expr::cust("'aidash://unrelated'"))
					.expr(reinhardt::query::Expr::cust("'http://localhost:1'"))
					.expr(reinhardt::query::Expr::cust("'AIDASH_SECRET_TEST_PEER'"))
					.expr(reinhardt::query::Expr::cust("'0.2'"))
					.expr(reinhardt::query::Expr::cust("TRUE"))
					.to_owned(),
			)
			.to_string(reinhardt::query::PostgresQueryBuilder),
	)
	.execute(a.store.pool.driver())
	.await
	.unwrap();
	assert_eq!(
		verify(&aa, "aidash://unrelated", id).await.0,
		403,
		"an authenticated unrelated peer must not use the grant"
	);
	assert_eq!(
		grant_request(&aa, "aidash://unrelated", id, "snapshot")
			.await
			.0,
		403
	);
	sqlx::query(
		&reinhardt::query::Query::delete()
			.from_table(reinhardt::query::Alias::new("peers"))
			.and_where(reinhardt::query::Expr::cust(
				"node_id = 'aidash://unrelated'",
			))
			.to_string(reinhardt::query::PostgresQueryBuilder),
	)
	.execute(a.store.pool.driver())
	.await
	.unwrap();
	{
		let query_bind_1 = &b.config.node_id;
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("peers"))
				.value_expr(
					reinhardt::query::Alias::new("enabled"),
					reinhardt::query::Expr::cust("TRUE"),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(node_id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(a.store.pool.driver())
		.await
	}
	.unwrap();
	let revoke = format!("{path}/{id}/revoke");
	let (status, revoked) = request(&aa, &token, "POST", &revoke, json!({})).await;
	assert_eq!(status, 200);
	assert_eq!(revoked["revoked"], true);
	assert_eq!(
		request(&aa, &token, "POST", &revoke, json!({})).await,
		(200, revoked)
	);
	assert_eq!(verify(&fresh_app, &b.config.node_id, id).await.0, 403);
	assert_eq!(
		request(&aa, &token, "POST", &path, input.clone()).await.0,
		409
	);
	let expiry = Uuid::new_v4();
	let mut next = input.clone();
	next["id"] = json!(expiry);
	assert_eq!(request(&aa, &token, "POST", &path, next).await.0, 200);
	{
		let query_bind_1 = expiry;
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("authorization_remote_grants"))
				.value_expr(
					reinhardt::query::Alias::new("expires_at"),
					reinhardt::query::Expr::cust("CLOCK_TIMESTAMP() - INTERVAL '1 SECOND'"),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(a.store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(verify(&fresh_app, &b.config.node_id, expiry).await.0, 403);
	assert_eq!(
		grant_request(&fresh_app, &b.config.node_id, expiry, "snapshot")
			.await
			.0,
		403
	);
	let current = Uuid::new_v4();
	let mut next = input.clone();
	next["id"] = json!(current);
	assert_eq!(request(&aa, &token, "POST", &path, next).await.0, 200);
	{
		let query_bind_1 = task;
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("tasks"))
				.value_expr(
					reinhardt::query::Alias::new("revision"),
					reinhardt::query::Expr::cust("revision + 1"),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(a.store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(verify(&fresh_app, &b.config.node_id, current).await.0, 403);
	{
		let query_bind_1 = task;
		sqlx::query(
			&reinhardt::query::Query::update()
				.table(reinhardt::query::Alias::new("tasks"))
				.value_expr(
					reinhardt::query::Alias::new("revision"),
					reinhardt::query::Expr::cust("revision - 1"),
				)
				.and_where(SimpleExpr::CustomWithExpr(
					"(id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(a.store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(verify(&fresh_app, &b.config.node_id, current).await.0, 200);
	let (_, replacement) = request(
		&aa,
		&a.config.api_token,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"alice"}),
	)
	.await;
	let replacement_token = replacement["token"].as_str().unwrap();
	let mut same = input.clone();
	same["id"] = json!(current);
	assert_eq!(
		request(&aa, replacement_token, "POST", &path, same).await.0,
		409,
		"a replacement credential cannot take over an existing grant"
	);
	let source_credential: Uuid = {
		let query_bind_1 = current;
		sqlx::query_scalar(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::SimpleExpr::from(
					reinhardt::query::Expr::col(reinhardt::query::Alias::new("credential_id")),
				))
				.from(reinhardt::query::Alias::new("authorization_remote_grants"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(id = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_one(a.store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(
		request(
			&aa,
			&a.config.api_token,
			"POST",
			&format!("/api/authorization/acme/credentials/{source_credential}/revoke"),
			json!({})
		)
		.await
		.0,
		200
	);
	assert_eq!(verify(&fresh_app, &b.config.node_id, current).await.0, 403);
	let replacement_id = Uuid::new_v4();
	let mut next = input.clone();
	next["id"] = json!(replacement_id);
	assert_eq!(
		request(&aa, replacement_token, "POST", &path, next).await.0,
		200
	);
	assert_eq!(
		verify(&fresh_app, &b.config.node_id, replacement_id)
			.await
			.0,
		200
	);
	let (_, receiver_replacement) = request(
		&ba,
		&b.config.api_token,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"alice"}),
	)
	.await;
	for (revision, credential, expected) in [
		(1, &receiver_replacement["credential"]["id"], 403),
		(2, &issued["credential"]["id"], 200),
	] {
		assert_eq!(request(&ba,&b.config.api_token,"POST","/api/authorization/acme/peer-mappings",json!({"source_node":a.config.node_id,"source_tenant":"acme","source_subject":"alice","credential_id":credential,"expected_revision":revision,"enabled":true})).await.0,200);
		assert_eq!(
			verify(&fresh_app, &b.config.node_id, replacement_id)
				.await
				.0,
			expected,
			"receiver credential rebinding must invalidate the prepared authority"
		);
	}
	assert_eq!(
		request(
			&ba,
			&b.config.api_token,
			"POST",
			&format!(
				"/api/authorization/acme/credentials/{}/revoke",
				issued["credential"]["id"].as_str().unwrap()
			),
			json!({})
		)
		.await
		.0,
		200
	);
	assert_ne!(
		verify(&fresh_app, &b.config.node_id, replacement_id)
			.await
			.0,
		200
	);
	let count: i64 = sqlx::query_scalar(
		&reinhardt::query::Query::select()
			.expr(reinhardt::query::Expr::cust("COUNT(*)"))
			.from(reinhardt::query::Alias::new("authorization_remote_grants"))
			.to_string(reinhardt::query::PostgresQueryBuilder),
	)
	.fetch_one(a.store.pool.driver())
	.await
	.unwrap();
	assert_eq!(count, 4);
	for f in [&a, &b] {
		let count: i64 = sqlx::query_scalar(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::Expr::cust("COUNT(*)"))
				.from(reinhardt::query::Alias::new("runs"))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_one(f.store.pool.driver())
		.await
		.unwrap();
		assert_eq!(count, 0, "prepared grants cannot bypass worker admission");
	}
	drop(fresh_app);
	fresh.store.pool.close().await;
	fresh.store.control_pool.close().await;
	drop(ba);
	cleanup(a, &au, &aschema).await;
	cleanup(b, &bu, &bschema).await;
}

use reinhardt::query::QueryStatementBuilder as _;

use reinhardt::query::SimpleExpr;

use reinhardt::query::Expr;
