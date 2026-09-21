mod common;
use aidash::{api, federation::Peer, harness::Harness};
use axum::{Json, Router, routing::post};
use common::*;
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::sync::{
	Arc,
	atomic::{AtomicUsize, Ordering},
};
use tower::ServiceExt;

#[tokio::test]
#[ignore = "requires disposable PostgreSQL and peer fixture credential"]
async fn worker_remote_discovery_dependencies_survive_restart_and_hide_revoked_journals() {
	let (a, a_url, a_schema) = setup().await;
	let (mut b, b_url, b_schema) = setup().await;
	b.config.node_id = "aidash://remote-journal".into();
	b.store.node_id = b.config.node_id.clone();
	let b_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	b.config.endpoint = format!("http://{}", b_listener.local_addr().unwrap());
	let calls = Arc::new(AtomicUsize::new(0));
	let observed = calls.clone();
	let model = Router::new().route("/v1/chat/completions",post(move |Json(body): Json<Value>| {
        let calls=observed.clone(); async move {
            let message=if calls.fetch_add(1,Ordering::SeqCst)==0 {
                json!({"role":"assistant","content":null,"tool_calls":[{"id":"discover","type":"function","function":{"name":"agent_discover","arguments":"{}"}}]})
            } else {
                assert!(body.to_string().contains("remote-private-metadata"));
                json!({"role":"assistant","content":"remote-derived-result"})
            };
            let reason=if message.get("tool_calls").is_some(){"tool_calls"}else{"stop"};
            Json(json!({"choices":[{"index":0,"finish_reason":reason,"message":message}],"usage":{"prompt_tokens":1,"completion_tokens":1}}))
        }
    }));
	let model_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", model_listener.local_addr().unwrap());
	let model_server = tokio::spawn(async move {
		axum::serve(model_listener, model).await.unwrap();
	});
	let a_app = api::router(a.clone());
	let b_app = api::router(b.clone());
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
				sea_orm::sea_query::Expr::cust("$1"),
				sea_orm::sea_query::Expr::cust("'http://localhost:1'"),
				sea_orm::sea_query::Expr::cust("'AIDASH_SECRET_TEST_PEER'"),
				sea_orm::sea_query::Expr::cust("'0.1'"),
				sea_orm::sea_query::Expr::cust("TRUE"),
			])
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(&a.config.node_id)
	.execute(&b.store.pool)
	.await
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
	let peer_app = b_app.clone();
	let peer_server = tokio::spawn(async move {
		axum::serve(b_listener, peer_app).await.unwrap();
	});
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
				&sea_orm::sea_query::Query::select()
					.expr(sea_orm::sea_query::Expr::cust("COUNT(*)"))
					.from(sea_orm::sea_query::Alias::new(
						"authorization_run_remote_reads",
					))
					.and_where(sea_orm::sea_query::Expr::cust("entry_id = 'remote-only'"))
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.fetch_one(&a.store.pool)
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
	sqlx::query(
		&sea_orm::sea_query::Query::update()
			.table(sea_orm::sea_query::Alias::new("registry"))
			.value(
				sea_orm::sea_query::Alias::new("metadata"),
				sea_orm::sea_query::Expr::cust("$1"),
			)
			.and_where(sea_orm::sea_query::Expr::cust("id = 'remote-only'"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(altered)
	.execute(&b.store.pool)
	.await
	.unwrap();
	assert_eq!(
		request(&a_app, &token, "GET", &path, Value::Null).await.0,
		403
	);
	sqlx::query(
		&sea_orm::sea_query::Query::update()
			.table(sea_orm::sea_query::Alias::new("registry"))
			.value(
				sea_orm::sea_query::Alias::new("metadata"),
				sea_orm::sea_query::Expr::cust("$1"),
			)
			.and_where(sea_orm::sea_query::Expr::cust("id = 'remote-only'"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(original)
	.execute(&b.store.pool)
	.await
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
			aidash::domain::qualified_agent(&b.config.node_id, "remote-only", "1.0.0"),
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
	assert_eq!(a.store.run(run.id).await.unwrap().control, "PAUSED");
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
		if a.store.run(run.id).await.unwrap().phase == "COMPLETED" {
			break;
		}
	}
	assert_eq!(a.store.run(run.id).await.unwrap().phase, "COMPLETED");
	assert_eq!(calls.load(Ordering::SeqCst), 2);
	let (status, journal) = request(&a_app, &token, "GET", &path, Value::Null).await;
	assert_eq!(status, 200);
	assert!(journal.to_string().contains("remote-private-metadata"));
	let stream_response = a_app
		.clone()
		.oneshot(
			axum::http::Request::get(format!(
				"/api/events/stream?workspace_id={}",
				run.workspace_id
			))
			.header("authorization", format!("Bearer {token}"))
			.body(axum::body::Body::empty())
			.unwrap(),
		)
		.await
		.unwrap();
	assert_eq!(stream_response.status(), 200);
	assert_eq!(
		stream_response.headers()["content-type"],
		"text/event-stream"
	);
	let mut stream = stream_response.into_body().into_data_stream();
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
	peer_server.abort();
	model_server.abort();
	let _ = peer_server.await;
	let _ = model_server.await;
	cleanup(a, &a_url, &a_schema).await;
	cleanup(b, &b_url, &b_schema).await;
}
