mod common;

use aidash::{
	api,
	domain::{ArtifactInput, qualified_agent},
	federation::Home,
	harness::Harness,
};
use axum::{Json, Router, routing::post};
use common::{bootstrap, cleanup, request, setup};
use serde_json::{Value, json};
use std::sync::{
	Arc,
	atomic::{AtomicUsize, Ordering},
};
use tokio::sync::{Mutex, Notify};
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn old_worker_cannot_lease_after_input_ledger_admission() {
	let (f, url, schema) = setup().await;
	let app = api::router(f.clone());
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (status, created) = request(
		&app,
		&token,
		"POST",
		"/api/conversations",
		json!({"title":"Worker rollout fence","goal":"Reply","target":{"id":"research","version":"1.0.0"},"target_kind":"agent"}),
	)
	.await;
	assert_eq!(status, 200, "{created}");
	let run = f.store.runs().await.unwrap().remove(0);
	let old_worker = Uuid::new_v4();
	let old_lease = sea_orm::sea_query::Query::update()
		.table(sea_orm::sea_query::Alias::new("runs"))
		.value(
			sea_orm::sea_query::Alias::new("lease_owner"),
			sea_orm::sea_query::Expr::cust("$2"),
		)
		.value(
			sea_orm::sea_query::Alias::new("lease_until"),
			sea_orm::sea_query::Expr::cust("CURRENT_TIMESTAMP + INTERVAL '30 seconds'"),
		)
		.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
		.to_string(sea_orm::sea_query::PostgresQueryBuilder);
	sqlx::query(&old_lease)
		.bind(run.id)
		.bind(old_worker)
		.execute(&f.store.pool)
		.await
		.unwrap();
	let key = format!("human:{}:{}", run.id, Uuid::new_v4());
	let limit = f.run_message_limit(&run).await.unwrap();
	assert!(matches!(
		f.store
			.accept_run_message(run.id, "human", "correction", &key, limit)
			.await,
		Err(aidash::error::Error::Conflict(_))
	));
	assert!(f.store.run_inputs(run.id).await.unwrap().is_empty());
	f.store.release_lease(run.id, old_worker).await.unwrap();
	f.store
		.accept_run_message(run.id, "human", "correction", &key, limit)
		.await
		.unwrap();
	assert!(f.store.run(run.id).await.unwrap().ledger_worker_ready);
	assert_eq!(f.store.run_inputs(run.id).await.unwrap().len(), 1);
	assert!(
		sqlx::query(&old_lease)
			.bind(run.id)
			.bind(Uuid::new_v4())
			.execute(&f.store.pool)
			.await
			.is_err()
	);
	let upgraded_worker = Uuid::new_v4();
	let leased = f
		.store
		.lease_run(upgraded_worker, 30)
		.await
		.unwrap()
		.unwrap();
	assert_eq!(leased.id, run.id);
	assert!(leased.ledger_worker_ready);
	let output_key = format!("{}:{}:output", run.id, run.step);
	assert!(
		f.store
			.message_record(
				run.workspace_id,
				"agent",
				"stale legacy response",
				Some(&output_key),
			)
			.await
			.is_err()
	);
	let peer_output_key = format!(
		"{}:{}:{}:{}:output",
		f.store.node_id, run.task_id, run.id, run.step
	);
	assert!(
		f.store
			.message_record(
				run.workspace_id,
				"agent",
				"stale peer-prefixed response",
				Some(&peer_output_key),
			)
			.await
			.is_err(),
		"peer-prefixed legacy output must be fenced too"
	);
	Home::new(f.clone(), leased.clone())
		.response_message(
			upgraded_worker,
			f.store.run_inputs(run.id).await.unwrap()[0].seq,
			&output_key,
			"response after observing the correction",
		)
		.await
		.unwrap();
	assert!(
		f.store
			.renew_lease(run.id, upgraded_worker, 30)
			.await
			.unwrap()
	);
	// An old worker cannot extend an upgraded worker's lease either.
	assert!(
		sqlx::query(
			&sea_orm::sea_query::Query::update()
				.table(sea_orm::sea_query::Alias::new("runs"))
				.value(
					sea_orm::sea_query::Alias::new("lease_until"),
					sea_orm::sea_query::Expr::cust("CURRENT_TIMESTAMP + INTERVAL '60 seconds'"),
				)
				.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
				.to_string(sea_orm::sea_query::PostgresQueryBuilder),
		)
		.bind(run.id)
		.execute(&f.store.pool)
		.await
		.is_err()
	);
	f.store
		.release_lease(run.id, upgraded_worker)
		.await
		.unwrap();
	// Model an older lease that was already active when the fence migration
	// landed, with a correction admitted by the preceding schema.
	let stale_worker = Uuid::new_v4();
	sqlx::query(
		&sea_orm::sea_query::Query::update()
			.table(sea_orm::sea_query::Alias::new("runs"))
			.value(
				sea_orm::sea_query::Alias::new("ledger_worker_ready"),
				sea_orm::sea_query::Expr::value(false),
			)
			.value(
				sea_orm::sea_query::Alias::new("lease_owner"),
				sea_orm::sea_query::Expr::cust("$2"),
			)
			.value(
				sea_orm::sea_query::Alias::new("lease_until"),
				sea_orm::sea_query::Expr::cust("CURRENT_TIMESTAMP + INTERVAL '30 seconds'"),
			)
			.and_where(sea_orm::sea_query::Expr::cust(
				"id = $1 AND set_config('aidash.input_ledger_worker', 'true', true) = 'true'",
			))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(run.id)
	.bind(stale_worker)
	.execute(&f.store.pool)
	.await
	.unwrap();
	let mut stale = f.store.run(run.id).await.unwrap();
	stale.phase = "THINKING".into();
	let error = f
		.store
		.save_run(&stale, stale_worker, "run.message_received")
		.await
		.unwrap_err();
	assert!(error.to_string().contains("requires an upgraded worker"));
	let task = f.store.task(run.task_id).await.unwrap();
	let task = if task.status == "CLAIMED" {
		f.store
			.transition(
				task.id,
				task.revision,
				task.owner.as_deref().unwrap(),
				"RUNNING",
			)
			.await
			.unwrap()
	} else {
		task
	};
	let artifact = ArtifactInput {
		kind: "text".into(),
		name: "stale response".into(),
		content: json!("stale response"),
	};
	let error = f
		.store
		.complete(
			task.id,
			task.owner.as_deref().unwrap(),
			"legacy-completion",
			&artifact,
		)
		.await
		.unwrap_err();
	assert!(
		error.to_string().contains("run messages await inference"),
		"legacy completion must be rejected by the unobserved input fence, got: {error}"
	);
	assert_eq!(f.store.task(task.id).await.unwrap().status, "RUNNING");
	f.store.release_lease(run.id, stale_worker).await.unwrap();
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn upgraded_control_updates_remain_available_while_a_legacy_worker_is_fenced() {
	let (f, url, schema) = setup().await;
	let app = api::router(f.clone());
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (status, created) = request(
		&app,
		&token,
		"POST",
		"/api/conversations",
		json!({"title":"Control rollout fence","goal":"Reply","target":{"id":"research","version":"1.0.0"},"target_kind":"agent"}),
	)
	.await;
	assert_eq!(status, 200, "{created}");
	let run = f.store.runs().await.unwrap().remove(0);
	let old_worker = Uuid::new_v4();
	sqlx::query(
		&sea_orm::sea_query::Query::update()
			.table(sea_orm::sea_query::Alias::new("runs"))
			.value(
				sea_orm::sea_query::Alias::new("lease_owner"),
				sea_orm::sea_query::Expr::cust("$2"),
			)
			.value(
				sea_orm::sea_query::Alias::new("lease_until"),
				sea_orm::sea_query::Expr::cust("CURRENT_TIMESTAMP + INTERVAL '30 seconds'"),
			)
			.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(run.id)
	.bind(old_worker)
	.execute(&f.store.pool)
	.await
	.unwrap();
	let mut tx = f.store.pool.begin().await.unwrap();
	sqlx::query_scalar::<_, String>(
		"SELECT set_config('aidash.input_ledger_worker', 'true', true)",
	)
	.fetch_one(&mut *tx)
	.await
	.unwrap();
	sqlx::query(
		"INSERT INTO run_inputs (run_id, sender, content, idempotency_key) VALUES ($1, 'human', 'pause before continuing', $2)",
	)
	.bind(run.id)
	.bind(format!("human:{}:{}", run.id, Uuid::new_v4()))
	.execute(&mut *tx)
	.await
	.unwrap();
	tx.commit().await.unwrap();
	let paused = f.store.control(run.id, "pause").await.unwrap();
	assert_eq!(paused.control, "PAUSED");
	assert_eq!(paused.lease_owner, Some(old_worker));
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn old_worker_cannot_start_tool_invocation_after_input_backfill() {
	let (f, url, schema) = setup().await;
	let app = api::router(f.clone());
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (status, created) = request(
		&app,
		&token,
		"POST",
		"/api/conversations",
		json!({"title":"Invocation rollout fence","goal":"Reply","target":{"id":"research","version":"1.0.0"},"target_kind":"agent"}),
	)
	.await;
	assert_eq!(status, 200, "{created}");
	let run = f.store.runs().await.unwrap().remove(0);
	let pending = json!({
		"included_input_seq":1,
		"response":{"text":"","tool_calls":[{"id":"stale-call","name":"unsafe","arguments":{"action":"write"}}],"input_tokens":0,"output_tokens":0},
		"cursor":0
	});
	sqlx::query(
		&sea_orm::sea_query::Query::update()
			.table(sea_orm::sea_query::Alias::new("runs"))
			.value(
				sea_orm::sea_query::Alias::new("phase"),
				sea_orm::sea_query::Expr::cust("'TOOL_CALL'"),
			)
			.value(
				sea_orm::sea_query::Alias::new("pending"),
				sea_orm::sea_query::Expr::cust("$2"),
			)
			.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(run.id)
	.bind(&pending)
	.execute(&f.store.pool)
	.await
	.unwrap();
	let old_worker = Uuid::new_v4();
	sqlx::query(
		&sea_orm::sea_query::Query::update()
			.table(sea_orm::sea_query::Alias::new("runs"))
			.value(
				sea_orm::sea_query::Alias::new("lease_owner"),
				sea_orm::sea_query::Expr::cust("$2"),
			)
			.value(
				sea_orm::sea_query::Alias::new("lease_until"),
				sea_orm::sea_query::Expr::cust("CURRENT_TIMESTAMP + INTERVAL '30 seconds'"),
			)
			.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(run.id)
	.bind(old_worker)
	.execute(&f.store.pool)
	.await
	.unwrap();
	let input_key = format!("human:{}:{}", run.id, Uuid::new_v4());
	let mut backfill = f.store.pool.begin().await.unwrap();
	sqlx::query_scalar::<_, String>(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::Expr::cust(
				"set_config('aidash.input_ledger_worker', 'true', true)",
			))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.fetch_one(&mut *backfill)
	.await
	.unwrap();
	sqlx::query(
		&sea_orm::sea_query::Query::insert()
			.into_table(sea_orm::sea_query::Alias::new("run_inputs"))
			.columns([
				sea_orm::sea_query::Alias::new("run_id"),
				sea_orm::sea_query::Alias::new("sender"),
				sea_orm::sea_query::Alias::new("content"),
				sea_orm::sea_query::Alias::new("idempotency_key"),
			])
			.values_panic([
				sea_orm::sea_query::Expr::cust("$1"),
				sea_orm::sea_query::Expr::cust("$2"),
				sea_orm::sea_query::Expr::cust("$3"),
				sea_orm::sea_query::Expr::cust("$4"),
			])
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(run.id)
	.bind("human")
	.bind("recovered correction")
	.bind(input_key)
	.execute(&mut *backfill)
	.await
	.unwrap();
	backfill.commit().await.unwrap();
	let stale_run = f.store.run(run.id).await.unwrap();
	let error = f
		.store
		.invocation_start(
			&stale_run,
			old_worker,
			"stale-tool-invocation",
			"unsafe",
			&json!({"action":"write"}),
			false,
		)
		.await
		.unwrap_err();
	assert!(error.to_string().contains("requires an upgraded worker"));
	let invocation_count: i64 = sqlx::query_scalar(
		&sea_orm::sea_query::Query::select()
			.expr(sea_orm::sea_query::Expr::cust("COUNT(*)"))
			.from(sea_orm::sea_query::Alias::new("invocations"))
			.and_where(sea_orm::sea_query::Expr::cust("run_id = $1"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(run.id)
	.fetch_one(&f.store.pool)
	.await
	.unwrap();
	assert_eq!(invocation_count, 0, "the stale tool effect must not start");
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn upgraded_worker_reclaims_an_expired_legacy_lease_after_input_backfill() {
	let (f, url, schema) = setup().await;
	let app = api::router(f.clone());
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (status, created) = request(
		&app,
		&token,
		"POST",
		"/api/conversations",
		json!({"title":"Reclaim a legacy lease","goal":"Reply","target":{"id":"research","version":"1.0.0"},"target_kind":"agent"}),
	)
	.await;
	assert_eq!(status, 200, "{created}");
	let run = f.store.runs().await.unwrap().remove(0);
	assert!(!run.ledger_worker_ready);
	let old_worker = Uuid::new_v4();
	sqlx::query(
		"UPDATE runs SET lease_owner = $2, lease_until = CURRENT_TIMESTAMP + INTERVAL '30 seconds' WHERE id = $1",
	)
	.bind(run.id)
	.bind(old_worker)
	.execute(&f.store.pool)
	.await
	.unwrap();
	let mut tx = f.store.pool.begin().await.unwrap();
	sqlx::query_scalar::<_, String>(
		"SELECT set_config('aidash.input_ledger_worker', 'true', true)",
	)
	.fetch_one(&mut *tx)
	.await
	.unwrap();
	sqlx::query(
		"INSERT INTO run_inputs (run_id, sender, content, idempotency_key) VALUES ($1, 'human', 'backfilled correction', $2)",
	)
	.bind(run.id)
	.bind(format!("human:{}:{}", run.id, Uuid::new_v4()))
	.execute(&mut *tx)
	.await
	.unwrap();
	tx.commit().await.unwrap();
	sqlx::query(
		"UPDATE runs SET lease_until = CURRENT_TIMESTAMP - INTERVAL '1 second' WHERE id = $1",
	)
	.bind(run.id)
	.execute(&f.store.pool)
	.await
	.unwrap();
	let upgraded_worker = Uuid::new_v4();
	let reclaimed = f
		.store
		.lease_run(upgraded_worker, 30)
		.await
		.expect("an upgraded worker can reclaim the expired legacy lease")
		.expect("the run remains available");
	assert_eq!(reclaimed.id, run.id);
	assert!(reclaimed.ledger_worker_ready);
	assert_eq!(reclaimed.pending["lease_recovered"], true);
	f.store
		.release_lease(run.id, upgraded_worker)
		.await
		.unwrap();
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn messages_accepted_during_and_after_inference_are_seen_before_completion() {
	let entered = Arc::new(Notify::new());
	let release = Arc::new(Notify::new());
	let calls = Arc::new(AtomicUsize::new(0));
	let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
	let provider = Router::new().route(
		"/v1/chat/completions",
		post({
			let entered = entered.clone();
			let release = release.clone();
			let calls = calls.clone();
			let requests = requests.clone();
			move |Json(body): Json<Value>| {
				let entered = entered.clone();
				let release = release.clone();
				let calls = calls.clone();
				let requests = requests.clone();
				async move {
					let call = calls.fetch_add(1, Ordering::SeqCst);
					requests.lock().await.push(body);
					if call == 0 {
						entered.notify_one();
						release.notified().await;
					}
					let text = match call {
						0 => "stale first answer",
						1 => "stale second answer",
						_ => "answer with both corrections",
					};
					Json(json!({"choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":text}}],"usage":{"prompt_tokens":10,"completion_tokens":10}}))
				}
			}
		}),
	);
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let server = tokio::spawn(async move { axum::serve(listener, provider).await.unwrap() });
	let (f, url, schema) = setup().await;
	let app = api::router(f.clone());
	let (_, token, _) = bootstrap(&f, &app, &endpoint).await;
	let (status, created) = request(
		&app,
		&token,
		"POST",
		"/api/conversations",
		json!({"title":"Finalization race","goal":"Give a short answer","target":{"id":"research","version":"1.0.0"},"target_kind":"agent"}),
	)
	.await;
	assert_eq!(status, 200, "{created}");
	let run = f.store.runs().await.unwrap().remove(0);
	let worker = Harness {
		federation: f.clone(),
	};
	assert!(worker.worker_once().await.unwrap());
	let first = tokio::spawn({
		let worker = worker.clone();
		async move { worker.worker_once().await }
	});
	tokio::time::timeout(std::time::Duration::from_secs(10), entered.notified())
		.await
		.expect("provider did not receive first inference");
	let first_key = Uuid::new_v4();
	let path = format!("/api/runs/{}/message", run.id);
	let (status, body) = request(
		&app,
		&token,
		"POST",
		&path,
		json!({"content":"first correction","idempotency_key":first_key}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	release.notify_one();
	assert!(first.await.unwrap().unwrap());
	assert_eq!(f.store.run(run.id).await.unwrap().phase, "THINKING");
	tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
	assert!(worker.worker_once().await.unwrap());
	assert_eq!(f.store.run(run.id).await.unwrap().phase, "TOOL_CALL");
	let second_key = Uuid::new_v4();
	let (status, body) = request(
		&app,
		&token,
		"POST",
		&path,
		json!({"content":"second correction","idempotency_key":second_key}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	assert!(worker.worker_once().await.unwrap());
	assert_eq!(f.store.run(run.id).await.unwrap().phase, "THINKING");
	assert!(worker.worker_once().await.unwrap());
	let lease_token = Uuid::new_v4();
	let leased = f.store.lease_run(lease_token, 30).await.unwrap().unwrap();
	assert_eq!(leased.id, run.id);
	assert!(
		f.store
			.begin_final_completion(&leased, lease_token)
			.await
			.unwrap()
	);
	let (status, body) = request(
		&app,
		&token,
		"POST",
		&path,
		json!({"content":"during final commit","idempotency_key":Uuid::new_v4()}),
	)
	.await;
	assert_eq!(status, 409, "{body}");
	f.store.release_lease(run.id, lease_token).await.unwrap();
	assert!(worker.worker_once().await.unwrap());
	assert_eq!(f.store.run(run.id).await.unwrap().phase, "COMPLETED");
	assert_eq!(f.store.task(run.task_id).await.unwrap().status, "COMPLETED");
	let seen = requests.lock().await;
	assert_eq!(seen.len(), 3);
	assert!(!seen[0].to_string().contains("first correction"));
	assert!(seen[1].to_string().contains("first correction"));
	assert!(seen[2].to_string().contains("first correction"));
	assert!(seen[2].to_string().contains("second correction"));
	drop(seen);
	let snapshot = f.store.snapshot(run.workspace_id).await.unwrap();
	let correction = snapshot
		.messages
		.iter()
		.find(|message| message.content == "first correction")
		.unwrap();
	let tracked: bool = sqlx::query_scalar(&sea_orm::sea_query::Query::select()
		.expr(sea_orm::sea_query::Expr::cust("EXISTS(SELECT 1 FROM authorization_run_reads WHERE run_id = $1 AND resource_kind = 'message' AND resource_id = $2)"))
		.to_string(sea_orm::sea_query::PostgresQueryBuilder))
		.bind(run.id).bind(correction.id).fetch_one(&f.store.pool).await.unwrap();
	assert!(
		tracked,
		"run-directed message must be tracked as an authorized source"
	);
	assert_eq!(snapshot.artifacts.len(), 1);
	assert_eq!(
		snapshot.artifacts[0].content,
		"answer with both corrections"
	);
	assert!(
		snapshot
			.messages
			.iter()
			.any(|message| message.content == "answer with both corrections")
	);
	assert!(
		!snapshot
			.messages
			.iter()
			.any(|message| message.content.starts_with("stale"))
	);
	assert!(
		!snapshot
			.messages
			.iter()
			.any(|message| message.content == "during final commit")
	);
	let (status, body) = request(
		&app,
		&token,
		"POST",
		&path,
		json!({"content":"too late","idempotency_key":Uuid::new_v4()}),
	)
	.await;
	assert_eq!(status, 409, "{body}");
	assert!(body["error"].as_str().unwrap().contains("not accepted"));
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&path,
			json!({"content":"first correction","idempotency_key":first_key}),
		)
		.await
		.0,
		200
	);
	server.abort();
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn included_reference_can_reach_tool_calls_without_becoming_finalizable() {
	let (f, url, schema) = setup().await;
	let app = api::router(f.clone());
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (status, created) = request(
		&app,
		&token,
		"POST",
		"/api/conversations",
		json!({"title":"Read reference","goal":"Reply","target":{"id":"research","version":"1.0.0"},"target_kind":"agent"}),
	)
	.await;
	assert_eq!(status, 200, "{created}");
	let run = f.store.runs().await.unwrap().remove(0);
	assert!(
		Harness {
			federation: f.clone()
		}
		.worker_once()
		.await
		.unwrap()
	);
	let key = format!("human:{}:{}", run.id, Uuid::new_v4());
	f.store
		.accept_run_message(
			run.id,
			"human",
			"reference to read",
			&key,
			f.run_message_limit(&run).await.unwrap(),
		)
		.await
		.unwrap();
	let input_seq = f.store.run_inputs(run.id).await.unwrap()[0].seq;
	let worker = Uuid::new_v4();
	let mut leased = f.store.lease_run(worker, 30).await.unwrap().unwrap();
	assert_eq!(leased.observed_input_seq, 0);
	leased.phase = "TOOL_CALL".into();
	leased.pending = json!({
		"included_input_seq":input_seq,
		"response":{"text":"","tool_calls":[],"input_tokens":1,"output_tokens":1,"usage_complete":true},
		"cursor":0
	});
	f.store
		.save_run(&leased, worker, "model.completed")
		.await
		.expect("the provider request included the reference");
	let leased = f.store.lease_run(worker, 30).await.unwrap().unwrap();
	assert!(
		!f.store
			.begin_final_completion(&leased, worker)
			.await
			.unwrap(),
		"the reference has not been read yet"
	);
	f.store.release_lease(run.id, worker).await.unwrap();
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn a_new_input_discards_pending_tool_calls_and_rotates_a_published_output_key() {
	let (f, url, schema) = setup().await;
	let app = api::router(f.clone());
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (status, created) = request(&app, &token, "POST", "/api/conversations", json!({
		"title":"Stale tool response","goal":"Reply","target":{"id":"research","version":"1.0.0"},"target_kind":"agent"
	})).await;
	assert_eq!(status, 200, "{created}");
	let run = f.store.runs().await.unwrap().remove(0);
	let harness = Harness {
		federation: f.clone(),
	};
	assert!(harness.worker_once().await.unwrap());
	let worker = Uuid::new_v4();
	let mut leased = f.store.lease_run(worker, 30).await.unwrap().unwrap();
	leased.phase = "TOOL_CALL".into();
	leased.pending = json!({
		"included_input_seq":0,
		"response":{"text":"stale tool output","tool_calls":[{"id":"stale-call","name":"workspace_observe","arguments":{}}],"input_tokens":1,"output_tokens":1,"usage_complete":true},
		"cursor":0
	});
	f.store
		.save_run(&leased, worker, "model.completed")
		.await
		.unwrap();
	let publish_worker = Uuid::new_v4();
	let published = f
		.store
		.lease_run(publish_worker, 30)
		.await
		.unwrap()
		.unwrap();
	Home::new(f.clone(), published.clone())
		.response_message(
			publish_worker,
			0,
			&format!("{}:{}:output", run.id, published.step),
			"stale tool output",
		)
		.await
		.unwrap();
	let (status, body) = request(
		&app,
		&token,
		"POST",
		&format!("/api/runs/{}/message", run.id),
		json!({"content":"new correction"}),
	)
	.await;
	assert_eq!(status, 200, "{body}");
	f.store.release_lease(run.id, publish_worker).await.unwrap();
	assert!(harness.worker_once().await.unwrap());
	let current = f.store.run(run.id).await.unwrap();
	assert_eq!(current.phase, "THINKING");
	assert_eq!(current.step, leased.step + 1);
	assert!(current.pending.get("response").is_none());
	assert!(
		f.store
			.snapshot(run.workspace_id)
			.await
			.unwrap()
			.messages
			.iter()
			.any(|message| message.content == "stale tool output")
	);
	let corrected_worker = Uuid::new_v4();
	let corrected = f
		.store
		.lease_run(corrected_worker, 30)
		.await
		.unwrap()
		.unwrap();
	let corrected_seq = f
		.store
		.run_inputs(run.id)
		.await
		.unwrap()
		.last()
		.unwrap()
		.seq;
	Home::new(f.clone(), corrected.clone())
		.response_message(
			corrected_worker,
			corrected_seq,
			&format!("{}:{}:output", run.id, corrected.step),
			"corrected tool output",
		)
		.await
		.expect("the corrected inference uses a fresh deterministic output key");
	assert!(
		f.store
			.snapshot(run.workspace_id)
			.await
			.unwrap()
			.messages
			.iter()
			.any(|message| message.content == "corrected tool output")
	);
	f.store
		.release_lease(run.id, corrected_worker)
		.await
		.unwrap();
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn oversized_catchup_summary_retries_without_advancing_the_input_sequence() {
	let (f, url, schema) = setup().await;
	let app = api::router(f.clone());
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (status, created) = request(
		&app,
		&token,
		"POST",
		"/api/conversations",
		json!({"title":"Summary retry","goal":"Reply","target":{"id":"research","version":"1.0.0"},"target_kind":"agent"}),
	)
	.await;
	assert_eq!(status, 200, "{created}");
	let run = f.store.runs().await.unwrap().remove(0);
	assert!(
		(Harness {
			federation: f.clone()
		})
		.worker_once()
		.await
		.unwrap()
	);
	let worker = Uuid::new_v4();
	let mut leased = f.store.lease_run(worker, 30).await.unwrap().unwrap();
	leased.phase = "TOOL_CALL".into();
	leased.pending = json!({
		"included_input_seq":0,
		"response":{"text":"summary that is too long","tool_calls":[],"input_tokens":1,"output_tokens":1,"usage_complete":true},
		"cursor":0,
		"required_run_message_reads":[],
		"references_read_at_inference":true,
		"run_message_catchup":true,
		"run_message_summary_end_seq":7,
		"run_message_summary_limit":8
	});
	f.store
		.save_run(&leased, worker, "model.completed")
		.await
		.unwrap();
	assert!(
		(Harness {
			federation: f.clone()
		})
		.worker_once()
		.await
		.unwrap()
	);
	let current = f.store.run(run.id).await.unwrap();
	assert_eq!(current.phase, "THINKING");
	assert_eq!(current.step, leased.step + 1);
	assert_eq!(current.context["run_message_summary_seq"], 0);
	assert_eq!(current.context["run_message_summary"], "");
	assert!(
		current.context["history"]
			.as_array()
			.unwrap()
			.iter()
			.any(|event| event["kind"] == "run_message_summary_required" && event["max_bytes"] == 8)
	);
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn reference_only_inputs_suppress_uninformed_tool_calls() {
	let (f, url, schema) = setup().await;
	let app = api::router(f.clone());
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (status, created) = request(
		&app,
		&token,
		"POST",
		"/api/conversations",
		json!({"title":"Referenced correction","goal":"Respond","target":{"id":"research","version":"1.0.0"},"target_kind":"agent"}),
	)
	.await;
	assert_eq!(status, 200, "{created}");
	let run = f.store.runs().await.unwrap().remove(0);
	let key = format!("human:{}:{}", run.id, Uuid::new_v4());
	f.store
		.accept_run_message(
			run.id,
			"human",
			"large correction to inspect",
			&key,
			f.run_message_limit(&run).await.unwrap(),
		)
		.await
		.unwrap();
	let input = f.store.run_inputs(run.id).await.unwrap().remove(0);
	let mut tx = f.store.pool.begin().await.unwrap();
	sqlx::query_scalar::<_, String>(
		"SELECT set_config('aidash.input_ledger_worker', 'true', true)",
	)
	.fetch_one(&mut *tx)
	.await
	.unwrap();
	sqlx::query("UPDATE run_inputs SET reference_only = TRUE WHERE run_id = $1")
		.bind(run.id)
		.execute(&mut *tx)
		.await
		.unwrap();
	tx.commit().await.unwrap();
	let worker = Uuid::new_v4();
	let mut leased = f.store.lease_run(worker, 30).await.unwrap().unwrap();
	leased.phase = "TOOL_CALL".into();
	leased.pending = json!({
		"included_input_seq":input.seq,
		"required_run_message_reads":[input.message_id.unwrap()],
		"references_read_at_inference":false,
		"response":{"text":"uninformed text","tool_calls":[{"id":"mutating-call","name":"workspace_message","arguments":{"content":"uninformed side effect"}}],"input_tokens":1,"output_tokens":1,"usage_complete":true},
		"cursor":0
	});
	f.store
		.save_run(&leased, worker, "model.completed")
		.await
		.unwrap();
	f.store.release_lease(run.id, worker).await.unwrap();
	assert!(
		(Harness {
			federation: f.clone()
		})
		.worker_once()
		.await
		.unwrap()
	);
	let current = f.store.run(run.id).await.unwrap();
	assert_eq!(current.phase, "THINKING");
	assert!(current.pending.get("response").is_none());
	assert_eq!(current.step, leased.step + 1);
	let snapshot = f.store.snapshot(run.workspace_id).await.unwrap();
	assert!(
		!snapshot
			.messages
			.iter()
			.any(|message| message.content == "uninformed side effect")
	);
	let invocations: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM invocations WHERE run_id = $1")
		.bind(run.id)
		.fetch_one(&f.store.pool)
		.await
		.unwrap();
	assert_eq!(invocations, 0);
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn effects_recheck_input_sequence_under_the_run_lock() {
	let (f, url, schema) = setup().await;
	let app = api::router(f.clone());
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (status, created) = request(
		&app,
		&token,
		"POST",
		"/api/conversations",
		json!({"title":"Effect fence","goal":"Respond","target":{"id":"research","version":"1.0.0"},"target_kind":"agent"}),
	)
	.await;
	assert_eq!(status, 200, "{created}");
	let run = f.store.runs().await.unwrap().remove(0);
	let previously_observed = f
		.store
		.run_inputs(run.id)
		.await
		.unwrap()
		.last()
		.map_or(0, |input| input.seq);
	let key = format!("human:{}:{}", run.id, Uuid::new_v4());
	f.store
		.accept_run_message(
			run.id,
			"human",
			"correction admitted after the provider response",
			&key,
			f.run_message_limit(&run).await.unwrap(),
		)
		.await
		.unwrap();
	let worker = Uuid::new_v4();
	let mut leased = f.store.lease_run(worker, 30).await.unwrap().unwrap();
	leased.pending = json!({"included_input_seq":previously_observed});
	assert!(matches!(
		aidash::federation::Home::new(f.clone(), run.clone())
			.response_message(
				Uuid::new_v4(),
				previously_observed,
				&format!("{}:lost-lease-output", run.id),
				"output after lease loss",
			)
			.await,
		Err(aidash::error::Error::Conflict(_))
	));
	assert!(matches!(
		f.store
			.invocation_start(
				&leased,
				worker,
				"stale-invocation",
				"workspace_message",
				&json!({"content":"stale tool effect"}),
				false,
			)
			.await,
		Err(aidash::error::Error::StaleInference)
	));
	assert!(matches!(
		aidash::federation::Home::new(f.clone(), run.clone())
			.response_message(
				worker,
				previously_observed,
				&format!("{}:stale-output", run.id),
				"stale model text",
			)
			.await,
		Err(aidash::error::Error::StaleInference)
	));
	let stale_invocation_count: i64 = sqlx::query_scalar(
		"SELECT COUNT(*) FROM invocations WHERE idempotency_key = 'stale-invocation'",
	)
	.fetch_one(&f.store.pool)
	.await
	.unwrap();
	assert_eq!(stale_invocation_count, 0);
	let messages = f.store.snapshot(run.workspace_id).await.unwrap().messages;
	assert!(
		!messages
			.iter()
			.any(|message| message.content == "stale model text")
	);
	let included_now = f
		.store
		.run_inputs(run.id)
		.await
		.unwrap()
		.last()
		.unwrap()
		.seq;
	aidash::federation::Home::new(f.clone(), run.clone())
		.response_message(
			worker,
			included_now,
			&format!("{}:fresh-output", run.id),
			"fresh response text",
		)
		.await
		.unwrap();
	assert!(
		f.store
			.snapshot(run.workspace_id)
			.await
			.unwrap()
			.messages
			.iter()
			.any(|message| message.content == "fresh response text")
	);
	f.store.release_lease(run.id, worker).await.unwrap();
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn run_message_limit_rejects_oversized_input_before_recording_it() {
	let (f, url, schema) = setup().await;
	let app = api::router(f.clone());
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (status, created) = request(&app, &token, "POST", "/api/conversations", json!({
		"title":"Run message budget","goal":"Respond","target":{"id":"research","version":"1.0.0"},"target_kind":"agent"
	})).await;
	assert_eq!(status, 200, "{created}");
	let run = f.store.runs().await.unwrap().remove(0);
	let path = format!("/api/runs/{}/message", run.id);
	let large = "x".repeat(32_768);
	let (status, body) = request(
		&app,
		&token,
		"POST",
		&path,
		json!({"content":large,"idempotency_key":Uuid::new_v4()}),
	)
	.await;
	assert_eq!(status, 400, "{body}");
	assert!(f.store.run_inputs(run.id).await.unwrap().is_empty());
	assert!(
		!f.store
			.snapshot(run.workspace_id)
			.await
			.unwrap()
			.messages
			.iter()
			.any(|message| message.content == large)
	);
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&path,
			json!({"content":"short correction"})
		)
		.await
		.0,
		200
	);
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn scoped_run_never_infers_from_an_unreadable_message() {
	let (f, url, schema) = setup().await;
	let app = api::router(f.clone());
	let (mut policy, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (status, created) = request(&app, &token, "POST", "/api/conversations", json!({
		"title":"Private correction","goal":"Respond","target":{"id":"research","version":"1.0.0"},"target_kind":"agent"
	})).await;
	assert_eq!(status, 200, "{created}");
	let run = f.store.runs().await.unwrap().remove(0);
	let worker = Harness {
		federation: f.clone(),
	};
	assert!(worker.worker_once().await.unwrap());
	let path = format!("/api/runs/{}/message", run.id);
	assert_eq!(
		request(
			&app,
			&token,
			"POST",
			&path,
			json!({"content":"private correction"})
		)
		.await
		.0,
		200
	);
	let message = f
		.store
		.snapshot(run.workspace_id)
		.await
		.unwrap()
		.messages
		.into_iter()
		.find(|message| message.content == "private correction")
		.unwrap();
	policy["policies"].as_array_mut().unwrap().push(json!({
		"id":"deny-run-correction-read", "effect":"deny",
		"subjects":{"ids":[qualified_agent(&f.config.node_id,"research","1.0.0")]},
		"actions":["message.read"], "resources":{"kinds":["message"],"ids":[message.id]}
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
	assert!(worker.worker_once().await.unwrap());
	let paused = f.store.run(run.id).await.unwrap();
	assert_eq!(paused.control, "PAUSED");
	assert_eq!(paused.phase, "THINKING");
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn queued_terminal_transitions_reject_new_run_messages() {
	let (f, url, schema) = setup().await;
	let app = api::router(f.clone());
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	for (index, terminal) in ["cancel", "failure_pending"].iter().enumerate() {
		let (status, created) = request(&app, &token, "POST", "/api/conversations", json!({
			"title":format!("Terminal {index}"),"goal":"Reply","target":{"id":"research","version":"1.0.0"},"target_kind":"agent"
		})).await;
		assert_eq!(status, 200, "{created}");
		let run = f
			.store
			.runs()
			.await
			.unwrap()
			.into_iter()
			.find(|run| {
				run.workspace_id.to_string() == created["workspace"]["id"].as_str().unwrap()
			})
			.unwrap();
		if *terminal == "cancel" {
			f.store.control(run.id, "cancel").await.unwrap();
		} else {
			sqlx::query(
				&sea_orm::sea_query::Query::update()
					.table(sea_orm::sea_query::Alias::new("runs"))
					.value(
						sea_orm::sea_query::Alias::new("pending"),
						sea_orm::sea_query::Expr::cust(
							"'{\"terminal_transition\":\"FAILED\"}'::jsonb",
						),
					)
					.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
					.to_string(sea_orm::sea_query::PostgresQueryBuilder),
			)
			.bind(run.id)
			.execute(&f.store.pool)
			.await
			.unwrap();
		}
		let (status, body) = request(
			&app,
			&f.config.api_token,
			"POST",
			&format!("/api/runs/{}/message", run.id),
			json!({
				"content":format!("late {index}"),"idempotency_key":Uuid::new_v4()
			}),
		)
		.await;
		assert_eq!(status, 409, "{body}");
		assert!(f.store.run_inputs(run.id).await.unwrap().is_empty());
		assert!(
			!f.store
				.snapshot(run.workspace_id)
				.await
				.unwrap()
				.messages
				.iter()
				.any(|message| message.content == format!("late {index}"))
		);
	}
	cleanup(f, &url, &schema).await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn expired_worker_lease_cannot_begin_final_completion() {
	let (f, url, schema) = setup().await;
	let app = api::router(f.clone());
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (status, created) = request(&app, &token, "POST", "/api/conversations", json!({
		"title":"Expired lease", "goal":"Reply", "target":{"id":"research","version":"1.0.0"},"target_kind":"agent"
	})).await;
	assert_eq!(status, 200, "{created}");
	let run = f.store.runs().await.unwrap().remove(0);
	let worker = Uuid::new_v4();
	sqlx::query(
		&sea_orm::sea_query::Query::update()
			.table(sea_orm::sea_query::Alias::new("runs"))
			.value(
				sea_orm::sea_query::Alias::new("phase"),
				sea_orm::sea_query::Expr::cust("'TOOL_CALL'"),
			)
			.value(
				sea_orm::sea_query::Alias::new("pending"),
				sea_orm::sea_query::Expr::cust("$3"),
			)
			.value(
				sea_orm::sea_query::Alias::new("lease_owner"),
				sea_orm::sea_query::Expr::cust("$2"),
			)
			.value(
				sea_orm::sea_query::Alias::new("lease_until"),
				sea_orm::sea_query::Expr::cust("CURRENT_TIMESTAMP - INTERVAL '1 second'"),
			)
			.and_where(sea_orm::sea_query::Expr::cust("id = $1"))
			.to_string(sea_orm::sea_query::PostgresQueryBuilder),
	)
	.bind(run.id)
	.bind(worker)
	.bind(json!({"response":{"text":"answer","tool_calls":[],"input_tokens":1,"output_tokens":1},"cursor":0}))
	.execute(&f.store.pool)
	.await
	.unwrap();
	let stale = f.store.run(run.id).await.unwrap();
	assert!(matches!(
		f.store.begin_final_completion(&stale, worker).await,
		Err(aidash::Error::Conflict(_))
	));
	assert_ne!(
		f.store.run(run.id).await.unwrap().pending["finalizing"],
		true
	);
	cleanup(f, &url, &schema).await;
}
