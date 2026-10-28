#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
#[path = "../../../execution/tests/support/legacy.rs"]
mod common;
use aidash_server::federation::Federation;
use common::*;
use serde_json::{Value, json};
use uuid::Uuid;
/// `from`'s default Node calls `to`'s, acting as itself, with a token signed
/// by `from`'s Deployment Key.
async fn verify(
	app: &common::TestApplication,
	nodes: (&Federation, &Federation),
	grant: Uuid,
) -> (u16, Value) {
	grant_request(app, nodes, grant, "verify").await
}
async fn grant_request(
	app: &common::TestApplication,
	nodes: (&Federation, &Federation),
	grant: Uuid,
	operation: &str,
) -> (u16, Value) {
	let (to, from) = nodes;
	let receiver = default_node(&to.store, "acme").await;
	common::peer_request(
		app,
		(from, "acme"),
		&receiver,
		"",
		&format!("/federation/v0.1/scoped/execution/grants/{operation}"),
		json!({"grant_id":grant}),
	)
	.await
}
/// The same request from a Node of a stand-in Deployment that `to` registered.
async fn grant_request_from(
	app: &common::TestApplication,
	to: &Federation,
	remote: &common::peering::Remote,
	node: &str,
	grant: Uuid,
	operation: &str,
) -> (u16, Value) {
	let receiver = default_node(&to.store, "acme").await;
	let path = format!("/scoped/execution/grants/{operation}");
	let body = json!({"grant_id":grant}).to_string();
	let headers = remote
		.headers(
			node,
			&receiver.to_string(),
			"",
			("POST", &path, body.as_bytes()),
		)
		.await;
	let client = app.client();
	let mut request = client
		.request(http::Method::POST, &format!("/federation/v0.1{path}"))
		.body(bytes::Bytes::copy_from_slice(body.as_bytes()))
		.header(http::header::CONTENT_TYPE, "application/json");
	for (name, value) in &headers {
		request = request.header(*name, value.as_str());
	}
	let response = request.send().await.unwrap();
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
	let executor = aidash_domain::agent_subject(
		&default_node(&b.store, "acme").await.to_string(),
		"research",
		"1.0.0",
	);
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
	common::register_pair((&a, &aa, "acme"), (&b, &ba, "acme")).await;
	let issued = common::issued_credential(&b.store, "acme", "alice").await;
	assert_eq!(common::host_peer_mapping(&b, "acme", json!({"node":default_node(&b.store,"acme").await.node,"source_node":default_node(&a.store,"acme").await,"source_subject":"alice","credential_id":issued["credential"]["id"],"expected_revision":0,"enabled":true})).await.0,200);
	let path = format!("/api/tasks/{task}/remote-grants");
	let id = Uuid::new_v4();
	let input = json!({"id":id,"node_id":default_node(&b.store,"acme").await,"agent":{"id":"research","version":"1.0.0"},"ttl_seconds":300});
	assert_eq!(
		// A Deployment Administrator holds no authority over a Tenant's Tasks (#484).
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
	assert_eq!(verify(&aa, (&a, &b), id).await, (200, json!(true)));
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
		a.config.deployment.clone(),
	)
	.await
	.unwrap();
	common::peering::carry_over(&mut fresh, &a);
	// Act: rebuild the application after replacing the durable store connection.
	let fresh_app = common::application(fresh.clone()).await;
	assert_eq!(verify(&fresh_app, (&a, &b), id).await.0, 200);
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
		.fetch_one(a.store.pool.fixture_driver())
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
	let (status, snapshot) = grant_request(&fresh_app, (&a, &b), id, "snapshot").await;
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
		.fetch_one(a.store.pool.fixture_driver())
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
	let (status, snapshot) = grant_request(&fresh_app, (&a, &b), id, "snapshot").await;
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
		.fetch_one(a.store.pool.fixture_driver())
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
	assert_eq!(verify(&aa, (&a, &b), id).await.0, 403);
	assert_eq!(
		grant_request(&fresh_app, (&a, &b), id, "snapshot").await.0,
		403
	);
	assert_eq!(
		grant_request(&fresh_app, (&a, &b), id, "describe").await.0,
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
	assert_eq!(verify(&fresh_app, (&a, &b), id).await.0, 200);
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
	.fetch_one(b.store.pool.fixture_driver())
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
		.execute(b.store.pool.fixture_driver())
		.await
	}
	.unwrap();
	assert_eq!(verify(&fresh_app, (&a, &b), id).await.0, 403);
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
		.execute(b.store.pool.fixture_driver())
		.await
	}
	.unwrap();
	assert_eq!(verify(&fresh_app, (&a, &b), id).await.0, 200);
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
	assert_eq!(verify(&fresh_app, (&a, &b), id).await.0, 403);
	assert_eq!(request(&aa,&token,"POST",&path,json!({"id":Uuid::new_v4(),"node_id":default_node(&b.store,"acme").await,"agent":{"id":"research","version":"1.0.0"},"ttl_seconds":300})).await.0,403);
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
	assert_eq!(verify(&fresh_app, (&a, &b), id).await.0, 200);
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
	// A Node of another Deployment that `a` registers is authenticated, yet no
	// grant names it. The grant's own Node stays registered, but disabled.
	let unrelated = common::peering::Remote::start("aidash://unrelated").await;
	let unrelated_node = aidash_domain::nodes::NodeName::derived("unrelated").to_string();
	common::peering::permit(&a, &unrelated.endpoint);
	let owner = default_node(&a.store, "acme").await.node;
	assert_eq!(
		common::BrowserMapping::new(&a, "acme", "alice")
			.await
			.send(
				"POST",
				&common::peering::registration_path("acme", owner.as_str()),
				unrelated.registration(&unrelated_node)
			)
			.await
			.0,
		200
	);
	common::set_node_enabled((&a, &aa, "acme"), (&b, "acme"), false, 1).await;
	assert_eq!(
		grant_request_from(&aa, &a, &unrelated, &unrelated_node, id, "verify")
			.await
			.0,
		403,
		"an authenticated unrelated peer must not use the grant"
	);
	assert_eq!(
		grant_request_from(&aa, &a, &unrelated, &unrelated_node, id, "snapshot")
			.await
			.0,
		403
	);
	assert_eq!(
		verify(&aa, (&a, &b), id).await.0,
		401,
		"a disabled registration admits nothing"
	);
	common::set_node_enabled((&a, &aa, "acme"), (&b, "acme"), true, 2).await;
	let revoke = format!("{path}/{id}/revoke");
	let (status, revoked) = request(&aa, &token, "POST", &revoke, json!({})).await;
	assert_eq!(status, 200);
	assert_eq!(revoked["revoked"], true);
	assert_eq!(
		request(&aa, &token, "POST", &revoke, json!({})).await,
		(200, revoked)
	);
	assert_eq!(verify(&fresh_app, (&a, &b), id).await.0, 403);
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
		.execute(a.store.pool.fixture_driver())
		.await
	}
	.unwrap();
	assert_eq!(verify(&fresh_app, (&a, &b), expiry).await.0, 403);
	assert_eq!(
		grant_request(&fresh_app, (&a, &b), expiry, "snapshot")
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
		.execute(a.store.pool.fixture_driver())
		.await
	}
	.unwrap();
	assert_eq!(verify(&fresh_app, (&a, &b), current).await.0, 403);
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
		.execute(a.store.pool.fixture_driver())
		.await
	}
	.unwrap();
	assert_eq!(verify(&fresh_app, (&a, &b), current).await.0, 200);
	let replacement = common::issued_credential(&a.store, "acme", "alice").await;
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
		.fetch_one(a.store.pool.fixture_driver())
		.await
	}
	.unwrap();
	assert_eq!(
		common::BrowserMapping::new(&a, "acme", "alice")
			.await
			.send(
				"POST",
				&format!("/api/authorization/acme/credentials/{source_credential}/revoke"),
				json!({})
			)
			.await
			.0,
		200
	);
	assert_eq!(verify(&fresh_app, (&a, &b), current).await.0, 403);
	let replacement_id = Uuid::new_v4();
	let mut next = input.clone();
	next["id"] = json!(replacement_id);
	assert_eq!(
		request(&aa, replacement_token, "POST", &path, next).await.0,
		200
	);
	assert_eq!(verify(&fresh_app, (&a, &b), replacement_id).await.0, 200);
	let receiver_replacement = common::issued_credential(&b.store, "acme", "alice").await;
	for (revision, credential, expected) in [
		(1, &receiver_replacement["credential"]["id"], 403),
		(2, &issued["credential"]["id"], 200),
	] {
		assert_eq!(common::host_peer_mapping(&b, "acme", json!({"node":default_node(&b.store,"acme").await.node,"source_node":default_node(&a.store,"acme").await,"source_subject":"alice","credential_id":credential,"expected_revision":revision,"enabled":true})).await.0,200);
		assert_eq!(
			verify(&fresh_app, (&a, &b), replacement_id).await.0,
			expected,
			"receiver credential rebinding must invalidate the prepared authority"
		);
	}
	assert_eq!(
		common::BrowserMapping::new(&b, "acme", "alice")
			.await
			.send(
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
	assert_ne!(verify(&fresh_app, (&a, &b), replacement_id).await.0, 200);
	let count: i64 = sqlx::query_scalar(
		&reinhardt::query::Query::select()
			.expr(reinhardt::query::Expr::cust("COUNT(*)"))
			.from(reinhardt::query::Alias::new("authorization_remote_grants"))
			.to_string(reinhardt::query::PostgresQueryBuilder),
	)
	.fetch_one(a.store.pool.fixture_driver())
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
		.fetch_one(f.store.pool.fixture_driver())
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
