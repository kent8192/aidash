use super::*;
use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};
fn a(name: &str) -> Alias {
	Alias::new(name)
}
async fn damage(store: &Store, id: Uuid, phase: RunPhase, pending: Value, context: Value) {
	sqlx::query(
		&Query::update()
			.table(a("runs"))
			.value(a("phase"), phase.as_str())
			.value(a("pending"), Expr::cust("$2"))
			.value(a("context"), Expr::cust("$3"))
			.and_where(Expr::col(a("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.bind(pending)
	.bind(context)
	.execute(&store.pool)
	.await
	.unwrap();
}
async fn create(store: &Store, agent: &Entry, workspace: Uuid) -> Run {
	let task = running_task(store, agent, workspace, None).await;
	store
		.accept_run(&task, &store.node_id, &agent.id, &agent.version)
		.await
		.unwrap()
}
#[rstest::rstest]
#[tokio::test]
async fn malformed_rows_fail_without_effect_replay_and_healthy_work_continues(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let (store, url, schema) = setup(&environment).await;
	let f = federation_for(&store);
	let calls = Arc::new(AtomicUsize::new(0));
	let observed = calls.clone();
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
	let server = tokio::spawn(async move {
		axum::serve(listener,axum::Router::new().route("/v1/chat/completions",axum::routing::post(move || {let calls=observed.clone(); async move {
			calls.fetch_add(1,Ordering::SeqCst);
			axum::Json(json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"Done"}}],"usage":{"prompt_tokens":100,"completion_tokens":1}}))
		}}))).await.unwrap();
	});
	f.registry.register(entry("model","model",json!({"provider":"openrouter","model_id":"fixture","endpoint":endpoint,"credential_env":null,"context_window":128000,"max_output_tokens":4096,"modalities":["text"],"cost":{}}))).await.unwrap();
	let agent = f
		.registry
		.register(entry(
			"agent",
			"research",
			json!({"model":{"id":"model","version":"1.0.0"},"instructions":"Finish","tools":[],"skills":[]}),
		))
		.await
		.unwrap();
	let workspace = store
		.create_workspace("Isolation", "Healthy progress")
		.await
		.unwrap();
	let mut damaged = vec![];
	for (phase, pending, context) in [
		(RunPhase::Ready, json!({}), common::context(json!({}))),
		(
			RunPhase::Ready,
			json!({"state_version":2,"data":{},"recovery":{"retry":null,"lease_recovered":false}}),
			common::context(json!({})),
		),
		(
			RunPhase::Waiting,
			json!({"state_version":1,"data":{"reason":"timer","wake_at":"not-a-timestamp","resume":{"phase":"READY","data":{}}},"recovery":{"retry":null,"lease_recovered":false}}),
			common::context(json!({})),
		),
		(
			RunPhase::Waiting,
			json!({"state_version":1,"data":{"reason":"unknown"},"recovery":{"retry":null,"lease_recovered":false}}),
			common::context(json!({})),
		),
		(
			RunPhase::Ready,
			common::pending(RunState::default()),
			json!([]),
		),
		(
			RunPhase::Ready,
			common::pending(RunState::default()),
			common::context(
				json!({"history":[{"kind":"tool","result":{"sensitive":"must not appear in diagnostics"}}]}),
			),
		),
	] {
		let run = create(&store, &agent, workspace.id).await;
		damage(&store, run.id, phase, pending, context).await;
		damaged.push(run);
	}
	// An independent unsafe-effect journal is retained even when the Run state
	// cannot say which effect completed. No new effect may be inferred/replayed.
	let journal = Query::insert()
		.into_table(a("invocations"))
		.columns([
			a("run_id"),
			a("idempotency_key"),
			a("tool"),
			a("input"),
			a("status"),
			a("replay_safe"),
		])
		.values_panic([
			damaged[0].id.into(),
			"uncertain-before-upgrade".into(),
			"unsafe".into(),
			Expr::val(json!({"write":true})).into(),
			"UNCERTAIN".into(),
			false.into(),
		])
		.to_string(PostgresQueryBuilder);
	sqlx::query(&journal).execute(&store.pool).await.unwrap();
	let healthy = create(&store, &agent, workspace.id).await;
	let worker = aidash::harness::Harness {
		federation: f.clone(),
	};
	for _ in 0..16 {
		worker.worker_once().await.unwrap();
	}
	let finished = store.run(healthy.id).await.unwrap();
	assert_eq!(
		finished.phase(),
		RunPhase::Completed,
		"{finished:?}; calls={}",
		calls.load(Ordering::SeqCst)
	);
	assert_eq!(calls.load(Ordering::SeqCst), 1);
	for run in damaged {
		let inspected = store.inspect_run(run.id).await.unwrap();
		assert_eq!(inspected.phase, RunPhase::Failed);
		assert_eq!(
			store.task(run.task_id).await.unwrap().status,
			TaskStatus::Failed
		);
		assert!(!inspected.error.as_ref().unwrap().contains("sensitive"));
	}
	let retained: i64 = sqlx::query_scalar(
		&Query::select()
			.expr(Expr::cust("COUNT(*)"))
			.from(a("invocations"))
			.and_where(Expr::col(a("idempotency_key")).eq("uncertain-before-upgrade"))
			.and_where(Expr::col(a("status")).eq("UNCERTAIN"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&store.pool)
	.await
	.unwrap();
	assert_eq!(retained, 1);
	server.abort();
	cleanup(store, &url, &schema).await;
}
#[rstest::rstest]
#[tokio::test]
async fn malformed_state_respects_pause_live_lease_terminal_and_stop_controls(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let (store, url, schema) = setup(&environment).await;
	let f = federation_for(&store);
	let agent = seed(&f.registry).await;
	let workspace = store
		.create_workspace("Controls", "Do not execute")
		.await
		.unwrap();
	let paused = create(&store, &agent, workspace.id).await;
	store
		.control(paused.id, RunControlAction::Pause)
		.await
		.unwrap();
	damage(&store, paused.id, RunPhase::Ready, json!({}), json!([])).await;
	let live = create(&store, &agent, workspace.id).await;
	let token = Uuid::new_v4();
	let leased = store.lease_run(token, 30).await.unwrap().unwrap();
	assert_eq!(leased.id, live.id);
	damage(&store, live.id, RunPhase::Ready, json!({}), json!([])).await;
	let terminal = create(&store, &agent, workspace.id).await;
	damage(
		&store,
		terminal.id,
		RunPhase::Completed,
		json!({}),
		json!([]),
	)
	.await;
	assert!(store.lease_run(Uuid::new_v4(), 30).await.unwrap().is_none());
	assert_eq!(store.inspect_runs().await.unwrap().len(), 3);
	assert_eq!(
		store.inspect_run(paused.id).await.unwrap().control,
		RunControl::Paused
	);
	assert_eq!(
		store.inspect_run(live.id).await.unwrap().lease_owner,
		Some(token)
	);
	assert_eq!(
		store.inspect_run(terminal.id).await.unwrap().phase,
		RunPhase::Completed
	);
	assert!(
		store
			.control(paused.id, RunControlAction::Resume)
			.await
			.is_err()
	);
	store
		.control(paused.id, RunControlAction::Cancel)
		.await
		.unwrap();
	let worker = aidash::harness::Harness {
		federation: f.clone(),
	};
	worker.worker_once().await.unwrap();
	worker.worker_once().await.unwrap();
	assert_eq!(
		store.inspect_run(paused.id).await.unwrap().phase,
		RunPhase::Cancelled
	);
	assert_eq!(
		store.task(paused.task_id).await.unwrap().status,
		TaskStatus::Cancelled
	);
	cleanup(store, &url, &schema).await;
}
#[rstest::rstest]
#[tokio::test]
async fn bounded_recovery_advances_past_a_full_page_of_future_waits(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let (store, url, schema) = setup(&environment).await;
	let f = federation_for(&store);
	let agent = seed(&f.registry).await;
	let workspace = store
		.create_workspace("Paging", "Later work")
		.await
		.unwrap();
	for _ in 0..129 {
		let run = create(&store, &agent, workspace.id).await;
		damage(
			&store,
			run.id,
			RunPhase::Waiting,
			common::pending(RunState::Waiting(Box::new(WaitingState::Timer {
				wake_at: chrono::Utc::now() + chrono::Duration::hours(1),
				resume: ResumeState::Ready(Default::default()),
			}))),
			common::context(json!({})),
		)
		.await;
	}
	let healthy = create(&store, &agent, workspace.id).await;
	assert!(store.lease_run(Uuid::new_v4(), 30).await.unwrap().is_none());
	assert_eq!(
		store
			.lease_run(Uuid::new_v4(), 30)
			.await
			.unwrap()
			.unwrap()
			.id,
		healthy.id
	);
	cleanup(store, &url, &schema).await;
}
