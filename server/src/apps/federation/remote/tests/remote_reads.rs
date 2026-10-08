use http::Method;
#[path = "../../../execution/tests/support/legacy.rs"]
mod common;
use aidash_server::{federation::Peer, harness::Harness};
use common::upstream_fixtures;
use common::*;
use futures_util::StreamExt;
use reinhardt::ServerRouter as Router;
use serde_json::{Value, json};
use std::sync::{
	Arc,
	atomic::{AtomicUsize, Ordering},
};
use upstream_fixtures::handler;

#[rstest::rstest]
#[tokio::test]
async fn worker_remote_discovery_dependencies_survive_restart_and_hide_revoked_journals(
	#[future(awt)]
	#[from(common::native_application)]
	source: common::ApplicationFixture,
	#[future(awt)]
	#[from(common::native_peer)]
	#[with("aidash://remote-journal")]
	destination: common::PeerFixture,
	#[from(upstream_fixtures::hits)] calls: Arc<AtomicUsize>,
	#[from(remote_model)]
	#[with(calls.clone())]
	_router: Arc<Router>,
	#[future(awt)]
	#[from(upstream_fixtures::upstream)]
	#[with(_router.clone())]
	model_server: reinhardt::test::fixtures::server::TestServerGuard,
) {
	let (a, a_url, a_schema) = source.runtime.parts();
	let (b, b_url, b_schema) = destination.runtime.parts();
	let a_app = source.application;
	let b_app = destination.application;
	let endpoint = model_server.url.clone();
	let (a_policy, token, task) = bootstrap(&a, &a_app, &endpoint).await;
	let (b_policy, _, _) = bootstrap(&b, &b_app, "http://localhost:1").await;
	let mut entry = b.registry.get("research", "1.0.0").await.unwrap();
	entry.id = "remote-only".into();
	entry
		.description
		.insert("en".into(), "remote-private-metadata".into());
	b.registry.register(entry).await.unwrap();
	assert_eq!(
		request(
			&b_app,
			&b.config.api_token,
			"POST",
			"/api/authorization/acme/catalog",
			json!({"entry":{"id":"remote-only","version":"1.0.0"},"expected_revision":0,"enabled":true})
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
						.expr(reinhardt::query::Expr::cust("'0.1'"))
						.expr(reinhardt::query::Expr::cust("TRUE"))
						.to_owned(),
				)
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(b.store.pool.driver())
		.await
	}
	.unwrap();
	let (_, issued) = request(
		&b_app,
		&b.config.api_token,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"alice"}),
	)
	.await;
	assert_eq!(request(&b_app,&b.config.api_token,"POST","/api/authorization/acme/peer-mappings",json!({"source_node":a.config.node_id,"source_tenant":"acme","source_subject":"alice","credential_id":issued["credential"]["id"],"expected_revision":0,"enabled":true})).await.0,200);

	a.register_peer(Peer {
		node_id: b.config.node_id.clone(),
		endpoint: b.config.endpoint.clone(),
		credential_env: "AIDASH_SECRET_TEST_PEER".into(),
		protocol_version: "0.1".into(),
		enabled: true,
	})
	.await
	.unwrap();
	assert_eq!(
		request(
			&a_app,
			&token,
			"POST",
			&format!("/api/tasks/{task}/claim"),
			json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}})
		)
		.await
		.0,
		200
	);
	let worker = Harness {
		federation: a.clone(),
	};
	tokio::time::timeout(std::time::Duration::from_secs(20), async {
		loop {
			worker.worker_once().await.unwrap();
			let count: i64 = sqlx::query_scalar(
				&reinhardt::query::Query::select()
					.expr(reinhardt::query::Expr::cust("COUNT(*)"))
					.from(reinhardt::query::Alias::new(
						"authorization_run_remote_reads",
					))
					.and_where(reinhardt::query::Expr::cust("entry_id = 'remote-only'"))
					.to_string(reinhardt::query::PostgresQueryBuilder),
			)
			.fetch_one(a.store.pool.driver())
			.await
			.unwrap();
			if count == 1 {
				break;
			}
		}
	})
	.await
	.unwrap();
	assert_eq!(calls.load(Ordering::SeqCst), 1);
	let run = a.store.runs().await.unwrap().remove(0);
	let path = format!("/api/runs/{}", run.id);
	assert_eq!(
		request(&a_app, &token, "GET", &path, Value::Null).await.0,
		200
	);
	// A misconfigured/replaced peer must not substitute different metadata under
	// an already copied id/version. Simulate that otherwise forbidden mutation.
	let original =
		serde_json::to_value(b.registry.get("remote-only", "1.0.0").await.unwrap()).unwrap();
	let mut altered = original.clone();
	altered["description"]["en"] = json!("substituted metadata");
	{
		let query_bind_1 = altered;
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
				.and_where(reinhardt::query::Expr::cust("id = 'remote-only'"))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(b.store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(
		request(&a_app, &token, "GET", &path, Value::Null).await.0,
		403
	);
	{
		let query_bind_1 = original;
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
				.and_where(reinhardt::query::Expr::cust("id = 'remote-only'"))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.execute(b.store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(
		request(&a_app, &token, "GET", &path, Value::Null).await.0,
		200
	);

	// Discovery admitted both read and execution authority. Revoke only execution
	// independently at each end while the catalog and credentials remain valid.
	for (node, app, original, resource) in [
		(
			&a,
			&a_app,
			&a_policy,
			aidash_server::domain::qualified_agent(&b.config.node_id, "remote-only", "1.0.0"),
		),
		(&b, &b_app, &b_policy, "remote-only".to_owned()),
	] {
		let mut denied = original.clone();
		denied["policies"].as_array_mut().unwrap().push(json!({"id":"deny-remote-execution","effect":"deny","subjects":{"ids":["alice"]},"actions":["agent.execute"],"resources":{"kinds":["agent"],"ids":[resource]}}));
		assert_eq!(
			request(
				app,
				&node.config.api_token,
				"POST",
				"/api/authorization/acme",
				json!({"expected_revision":1,"bundle":denied})
			)
			.await
			.0,
			200
		);
		assert_eq!(
			request(&a_app, &token, "GET", &path, Value::Null).await.0,
			403
		);
		assert_eq!(
			request(
				app,
				&node.config.api_token,
				"POST",
				"/api/authorization/acme",
				json!({"expected_revision":2,"bundle":original})
			)
			.await
			.0,
			200
		);
		assert_eq!(
			request(&a_app, &token, "GET", &path, Value::Null).await.0,
			200
		);
	}

	assert_eq!(
		request(
			&b_app,
			&b.config.api_token,
			"POST",
			"/api/authorization/acme/catalog",
			json!({"entry":{"id":"remote-only","version":"1.0.0"},"expected_revision":1,"enabled":false})
		)
		.await
		.0,
		200
	);
	assert_eq!(
		request(&a_app, &token, "GET", &path, Value::Null).await.0,
		403
	);
	// A restarted worker uses fresh pools and rechecks the persisted dependency.
	let restarted = Harness {
		federation: a.for_workers().await.unwrap(),
	};
	restarted.worker_once().await.unwrap();
	assert_eq!(
		a.store.run(run.id).await.unwrap().control.as_str(),
		"PAUSED"
	);
	assert_eq!(
		calls.load(Ordering::SeqCst),
		1,
		"revoked metadata must not reach another model call"
	);
	assert_eq!(
		request(
			&b_app,
			&b.config.api_token,
			"POST",
			"/api/authorization/acme/catalog",
			json!({"entry":{"id":"remote-only","version":"1.0.0"},"expected_revision":2,"enabled":true})
		)
		.await
		.0,
		200
	);
	assert_eq!(
		request(
			&a_app,
			&token,
			"POST",
			&format!("/api/runs/{}/control", run.id),
			json!({"action":"resume"})
		)
		.await
		.0,
		200
	);
	for _ in 0..12 {
		restarted.worker_once().await.unwrap();
		if a.store.run(run.id).await.unwrap().phase().as_str() == "COMPLETED" {
			break;
		}
	}
	assert_eq!(
		a.store.run(run.id).await.unwrap().phase().as_str(),
		"COMPLETED"
	);
	assert_eq!(calls.load(Ordering::SeqCst), 2);
	let (status, journal) = request(&a_app, &token, "GET", &path, Value::Null).await;
	assert_eq!(status, 200);
	assert!(journal.to_string().contains("remote-private-metadata"));
	let stream_response = a
		.client
		.clone()
		.request(
			Method::GET,
			a_app.url(format!(
				"/api/events/stream?workspace_id={}",
				run.workspace_id
			)),
		)
		.header("authorization", format!("Bearer {token}"))
		.send()
		.await
		.unwrap();
	assert_eq!(stream_response.status(), 200);
	assert_eq!(
		stream_response.headers()["content-type"],
		"text/event-stream"
	);
	let mut stream = stream_response.bytes_stream();
	tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
		.await
		.unwrap()
		.unwrap()
		.unwrap();

	assert_eq!(
		request(
			&b_app,
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
	assert_eq!(
		request(&a_app, &token, "GET", &path, Value::Null).await.0,
		403
	);
	assert_eq!(
		request(
			&a_app,
			&token,
			"POST",
			&format!("/api/workspaces/{}/messages", run.workspace_id),
			json!({"content":"public-stream-tail"})
		)
		.await
		.0,
		200
	);
	tokio::time::timeout(std::time::Duration::from_secs(20), async {
		loop {
			let chunk = stream.next().await.unwrap().unwrap();
			let frame = String::from_utf8_lossy(&chunk);
			assert!(
				!frame.contains("remote-private-metadata")
					&& !frame.contains("remote-derived-result"),
				"revoked buffered event escaped"
			);
			if frame.contains("public-stream-tail") {
				break;
			}
		}
	})
	.await
	.unwrap();
	drop(stream);
	let (status, state) = request(&a_app, &token, "GET", "/api/state", Value::Null).await;
	assert_eq!(status, 200);
	assert!(!state.to_string().contains("remote-private-metadata"));
	let leaking: Vec<_> = state
		.as_object()
		.unwrap()
		.iter()
		.filter(|(_, value)| value.to_string().contains("remote-derived-result"))
		.map(|(key, _)| key)
		.collect();
	assert!(
		leaking.is_empty(),
		"derived output remains in {leaking:?}; event kinds {:?}",
		state["events"]
			.as_array()
			.unwrap()
			.iter()
			.filter(|event| event.to_string().contains("remote-derived-result"))
			.map(|event| &event["kind"])
			.collect::<Vec<_>>()
	);
	restarted.federation.store.pool.close().await;
	restarted.federation.store.control_pool.close().await;
	drop(b_app);
	drop(model_server);
	cleanup(a, &a_url, &a_schema).await;
	cleanup(b, &b_url, &b_schema).await;
}

use reinhardt::query::QueryStatementBuilder as _;

use reinhardt::query::SimpleExpr;

use reinhardt::query::Expr;

#[rstest::fixture]
fn remote_model(#[from(upstream_fixtures::hits)] calls: Arc<AtomicUsize>) -> Arc<Router> {
	Arc::new(Router::new().handler("/v1/chat/completions",handler(http::Method::POST, move |request: reinhardt::Request| {let body = request.json::<Value>().unwrap();
        let calls=calls.clone(); async move {
            let message=if calls.fetch_add(1,Ordering::SeqCst)==0 {
                json!({"role":"assistant","content":null,"tool_calls":[{"id":"discover","type":"function","function":{"name":"agent_discover","arguments":"{}"}}]})
            } else {
                assert!(body.to_string().contains("remote-private-metadata"));
                json!({"role":"assistant","content":"remote-derived-result"})
            };
            let reason=if message.get("tool_calls").is_some(){"tool_calls"}else{"stop"};
            reinhardt::Response::ok().with_json(&json!({"choices":[{"index":0,"finish_reason":reason,"message":message}],"usage":{"prompt_tokens":1,"completion_tokens":1}})).unwrap()
        }
    })))
}
