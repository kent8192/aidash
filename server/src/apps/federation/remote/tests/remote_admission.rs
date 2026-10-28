#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
#[path = "../../../execution/tests/support/legacy.rs"]
mod common;
use aidash_application::tenancy::TEST_FIXTURE;
use aidash_server::{federation::Federation, store::Store};
use common::*;
use serde_json::{Value, json};
use uuid::Uuid;

/// A request from `a`'s default Node to `b`'s, as `a`'s subject `alice`,
/// signed with `a`'s Deployment Key.
async fn peer(
	app: &common::TestApplication,
	nodes: (&Federation, &Federation),
	path: &str,
	input: Value,
) -> (u16, Value) {
	let (from, to) = nodes;
	let receiver = default_node(&to.store, "acme").await;
	common::peer_request(app, (from, "acme"), &receiver, "alice", path, input).await
}
#[rstest::rstest]
#[tokio::test]
async fn receiver_admission_is_idempotent_scoped_and_revalidated_after_reconnect(
	#[future(awt)]
	#[from(common::native_peer)]
	first: common::PeerFixture,
	#[future(awt)]
	#[from(common::native_peer)]
	#[with("aidash://admission-host")]
	second: common::PeerFixture,
) {
	let (a, au, aschema) = first.runtime.parts();
	let (b, bu, bschema) = second.runtime.parts();
	let aa = first.application;
	let ba = second.application;
	let (mut ap, at, task) = bootstrap(&a, &aa, "http://localhost:1").await;
	let (bp, bt, _) = bootstrap(&b, &ba, "http://localhost:1").await;
	ap["subjects"][aidash_domain::agent_subject(
		&default_node(&b.store, "acme").await.to_string(),
		"research",
		"1.0.0",
	)] = json!({"kind":"agent"});
	assert_eq!(
		request(
			&aa,
			&a.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":1,"bundle":ap})
		)
		.await
		.0,
		200
	);
	common::register_pair((&a, &aa, "acme"), (&b, &ba, "acme")).await;
	let credential = common::issued_credential(&b.store, "acme", "alice").await;
	let mapping = json!({"node":default_node(&b.store,"acme").await.node,"source_node":default_node(&a.store,"acme").await,"source_subject":"alice","credential_id":credential["credential"]["id"],"enabled":true,"expected_revision":0});
	assert_eq!(
		common::set_peer_mapping(&b, "acme", "alice", mapping.clone())
			.await
			.0,
		200
	);
	let grant = Uuid::new_v4();
	let input = json!({"id":grant,"node_id":default_node(&b.store,"acme").await,"agent":{"id":"research","version":"1.0.0"},"ttl_seconds":300});
	let path = format!("/api/tasks/{task}/remote-grants");
	let (status, prepared) = request(&aa, &at, "POST", &path, input.clone()).await;
	assert_eq!(status, 200, "{prepared}");
	let admit = "/federation/v0.1/scoped/execution/admissions";
	let (first, second) = tokio::join!(
		peer(&ba, (&a, &b), admit, json!({"grant_id":grant})),
		peer(&ba, (&a, &b), admit, json!({"grant_id":grant}))
	);
	assert_eq!(first.0, 200, "{}", first.1);
	assert_eq!(
		first, second,
		"duplicate delivery must return the same local binding"
	);
	let source_task = a.store.task(&TEST_FIXTURE, task).await.unwrap();
	let receipt = first.1;
	assert_eq!(receipt["task_id"], task.to_string());
	let verification = format!("{admit}/{}/verify", receipt["id"].as_str().unwrap());
	assert_eq!(
		peer(&ba, (&a, &b), &verification, json!({})).await,
		(200, json!(true))
	);
	let mut fresh = b.clone();
	fresh.store = Store::from_pool(
		b.store
			.pool
			.options()
			.clone()
			.connect_with(b.store.pool.connect_options().as_ref().clone())
			.await
			.unwrap(),
		b.config.deployment.clone(),
	)
	.await
	.unwrap();
	common::peering::carry_over(&mut fresh, &b);
	// Act: rebuild the application after replacing the durable store connection.
	let fresh_app = common::application(fresh.clone()).await;
	assert_eq!(
		peer(&fresh_app, (&a, &b), admit, json!({"grant_id":grant})).await,
		(200, receipt.clone())
	);
	let replacement = Uuid::new_v4();
	let mut another = input.clone();
	another["id"] = json!(replacement);
	assert_eq!(request(&aa, &at, "POST", &path, another).await.0, 200);
	assert_eq!(
		peer(&fresh_app, (&a, &b), admit, json!({"grant_id":replacement}))
			.await
			.0,
		409,
		"another grant cannot replace the task's existing admission"
	);
	// Receiver policy scopes a foreign workspace by its qualified source ID.
	let mut denied = bp.clone();
	denied["policies"].as_array_mut().unwrap().push(json!({"id":"deny-foreign-workspace","effect":"deny","subjects":{"any":true},"actions":["workspace.read"],"resources":{"kinds":["workspace"]},"condition":{"op":"eq","left":{"source":"resource","path":"/source_node"},"right":{"source":"literal","value":default_node(&a.store,"acme").await}}}));
	assert_eq!(
		request(
			&ba,
			&b.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":1,"bundle":denied})
		)
		.await
		.0,
		200
	);
	assert_eq!(
		peer(&fresh_app, (&a, &b), &verification, json!({})).await.0,
		403
	);
	assert_eq!(
		request(
			&ba,
			&b.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":2,"bundle":bp})
		)
		.await
		.0,
		200
	);
	assert_eq!(
		peer(&fresh_app, (&a, &b), &verification, json!({})).await.0,
		200
	);
	let mut denied_model = bp.clone();
	denied_model["policies"].as_array_mut().unwrap().push(json!({"id":"deny-model-in-workspace","effect":"deny","subjects":{"any":true},"actions":["model.infer"],"resources":{"kinds":["model"]},"condition":{"op":"eq","left":{"source":"resource","path":"/source_workspace_id"},"right":{"source":"literal","value":source_task.workspace_id}}}));
	assert_eq!(
		request(
			&ba,
			&b.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":3,"bundle":denied_model})
		)
		.await
		.0,
		200
	);
	assert_eq!(
		peer(&fresh_app, (&a, &b), &verification, json!({})).await.0,
		403,
		"executor metadata checks must include the admitted workspace context"
	);
	assert_eq!(
		request(
			&ba,
			&b.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":4,"bundle":bp})
		)
		.await
		.0,
		200
	);
	let mut denied = ap.clone();
	denied["policies"].as_array_mut().unwrap().push(json!({"id":"deny-source-task","effect":"deny","subjects":{"any":true},"actions":["task.read"],"resources":{"kinds":["task"]}}));
	assert_eq!(
		request(
			&aa,
			&a.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":2,"bundle":denied})
		)
		.await
		.0,
		200
	);
	let denied = peer(&fresh_app, (&a, &b), &verification, json!({})).await;
	assert_ne!(denied.0, 200);
	assert!(!denied.1.to_string().contains("Research"));
	assert_eq!(
		request(
			&aa,
			&a.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":3,"bundle":ap})
		)
		.await
		.0,
		200
	);
	assert_eq!(
		peer(&fresh_app, (&a, &b), &verification, json!({})).await.0,
		200
	);
	// Source outage cannot be replaced with the previously accepted grant.
	// A stand-in that publishes `a`'s key but answers no federation route.
	let outage = common::peering::Remote::standing_in(
		a.config.deployment.as_str(),
		common::peering::key_of(&a),
	)
	.await;
	common::repoint_node((&b, &ba, "acme"), (&a, "acme"), &outage.endpoint, 1).await;
	assert_ne!(
		peer(&fresh_app, (&a, &b), &verification, json!({})).await.0,
		200
	);
	common::repoint_node((&b, &ba, "acme"), (&a, "acme"), &a.config.endpoint, 2).await;
	assert_eq!(
		peer(&fresh_app, (&a, &b), &verification, json!({})).await.0,
		200
	);
	let new_credential = common::issued_credential(&b.store, "acme", "alice").await;
	let mut rotated = mapping.clone();
	rotated["expected_revision"] = json!(1);
	rotated["credential_id"] = new_credential["credential"]["id"].clone();
	assert_eq!(
		common::set_peer_mapping(&b, "acme", "alice", rotated)
			.await
			.0,
		200
	);
	assert_ne!(
		peer(&fresh_app, (&a, &b), &verification, json!({})).await.0,
		200
	);
	let mut restored = mapping;
	restored["expected_revision"] = json!(2);
	assert_eq!(
		common::set_peer_mapping(&b, "acme", "alice", restored)
			.await
			.0,
		200
	);
	assert_eq!(
		peer(&fresh_app, (&a, &b), &verification, json!({})).await.0,
		200
	);
	let (status, state) = request(&ba, &bt, "GET", "/api/state", json!({})).await;
	assert_eq!(status, 200);
	assert!(
		!state.to_string().contains(&task.to_string()),
		"receiver tenants must not receive unscoped source task events"
	);
	assert_eq!(
		request(
			&aa,
			&at,
			"POST",
			&format!("{path}/{grant}/revoke"),
			json!({})
		)
		.await
		.0,
		200
	);
	assert_ne!(
		peer(&fresh_app, (&a, &b), &verification, json!({})).await.0,
		200
	);
	assert_ne!(
		peer(&fresh_app, (&a, &b), admit, json!({"grant_id":grant}))
			.await
			.0,
		200
	);
	let count: i64 = sqlx::query_scalar(
		&reinhardt::query::Query::select()
			.expr(reinhardt::query::Expr::cust("COUNT(*)"))
			.from(reinhardt::query::Alias::new(
				"authorization_remote_admissions",
			))
			.to_string(reinhardt::query::PostgresQueryBuilder),
	)
	.fetch_one(b.store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(count, 1);
	let (status, candidate) = request(
		&aa,
		&at,
		"POST",
		&format!("/api/workspaces/{}/tasks", source_task.workspace_id),
		json!({"title":"Admission race","description":"Fixture"}),
	)
	.await;
	assert_eq!(status, 200);
	let candidate: aidash_server::domain::Task = serde_json::from_value(candidate).unwrap();
	let race_grant = Uuid::new_v4();
	let mut race_input = input.clone();
	race_input["id"] = json!(race_grant);
	assert_eq!(
		request(
			&aa,
			&at,
			"POST",
			&format!("/api/tasks/{}/remote-grants", candidate.id),
			race_input
		)
		.await
		.0,
		200
	);
	let scoped = peer(&fresh_app, (&a, &b), admit, json!({"grant_id":race_grant})).await;
	assert_eq!(scoped.0, 200, "{scoped:?}");
	let runs: i64 = {
		let query_bind_1 = default_node(&a.store, "acme").await.to_string();
		let query_bind_2 = candidate.id;
		sqlx::query_scalar(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::Expr::cust("COUNT(*)"))
				.from(reinhardt::query::Alias::new("runs"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(home_node = ? AND task_id = ?)".to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
					],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_one(b.store.pool.fixture_driver())
		.await
	}
	.unwrap();
	let admissions: i64 = {
		let query_bind_1 = default_node(&a.store, "acme").await.to_string();
		let query_bind_2 = candidate.id;
		sqlx::query_scalar(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::Expr::cust("COUNT(*)"))
				.from(reinhardt::query::Alias::new(
					"authorization_remote_admissions",
				))
				.and_where(SimpleExpr::CustomWithExpr(
					"(source_node = ? AND task_id = ?)".to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
					],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_one(b.store.pool.fixture_driver())
		.await
	}
	.unwrap();
	assert_eq!((runs, admissions), (0, 1));
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
		assert_eq!(
			count, 0,
			"admission may not bypass scoped worker activation"
		);
	}
	drop(fresh_app);
	fresh.store.pool.close().await;
	fresh.store.control_pool.close().await;
	drop(aa);
	drop(ba);
	cleanup(a, &au, &aschema).await;
	cleanup(b, &bu, &bschema).await;
}

use reinhardt::query::QueryStatementBuilder as _;

use reinhardt::query::SimpleExpr;

use reinhardt::query::Expr;
