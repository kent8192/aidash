use super::upstream_fixtures;
use super::*;
use reinhardt::ServerRouter as Router;
use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
use reinhardt::test::fixtures::server::TestServerGuard;
use serde_json::Value;
use std::sync::atomic::Ordering;
use upstream_fixtures::{reply, upstream};
fn a(name: &str) -> Alias {
	Alias::new(name)
}
async fn damage(store: &Store, id: Uuid, phase: RunPhase, pending: Value, context: Value) {
	{
		let query_bind_1 = id;
		let query_bind_2 = pending;
		let query_bind_3 = context;
		sqlx::query(
			&Query::update()
				.table(a("runs"))
				.value(a("phase"), phase.as_str())
				.value_expr(
					a("pending"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_2.to_owned()).into()],
					),
				)
				.value_expr(
					a("context"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_3.to_owned()).into()],
					),
				)
				.and_where(Expr::col(a("id")).eq(SimpleExpr::CustomWithExpr(
					"(?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(store.pool.driver())
		.await
	}
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
	#[from(super::store)] _store_fixture: StoreFuture,
	#[from(super::federation)]
	#[with(_store_fixture.clone())]
	_federation: common::RuntimeFuture,
	#[from(upstream_fixtures::hits)] calls: Arc<std::sync::atomic::AtomicUsize>,
	#[from(malformed_rows_fail_without_effect_replay_and_healthy_work_continues_router)]
	#[with(calls.clone())]
	_router: Arc<Router>,
	#[future(awt)]
	#[from(upstream)]
	#[with(_router.clone())]
	server: TestServerGuard,
) {
	let (store, url, schema) = _store_fixture.clone().await.parts();
	let f = _federation.await.federation;

	let endpoint = format!("{}/v1", server.url);
	f.registry.register(entry("model","model",json!({"provider":"openrouter","model_id":"fixture","endpoint":endpoint,"credential_env":null,"context_window":128000,"max_output_tokens":4096,"modalities":["text"],"cost":{}}))).await.unwrap();
	let agent = f
		.registry
		.register(entry(
			"agent",
			"research",
			json!({"model":{"id":"model","version":"1.0.0"},"instructions":"Finish","schema_version":1,"bindings":[],"remove_default":[]}),
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
		.from_subquery(
			Query::select()
				.expr(Expr::value(damaged[0].id))
				.expr(Expr::value("uncertain-before-upgrade"))
				.expr(Expr::value("unsafe"))
				.expr(Expr::value(json!({"write":true})))
				.expr(Expr::value("UNCERTAIN"))
				.expr(Expr::value(false))
				.to_owned(),
		)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&journal)
		.execute(store.pool.driver())
		.await
		.unwrap();
	let healthy = create(&store, &agent, workspace.id).await;
	let worker = aidash_server::harness::Harness {
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
			.and_where(Expr::col(a("idempotency_key")).eq(Expr::value("uncertain-before-upgrade")))
			.and_where(Expr::col(a("status")).eq(Expr::value("UNCERTAIN")))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(store.pool.driver())
	.await
	.unwrap();
	assert_eq!(retained, 1);
	drop(server);
	cleanup(store, &url, &schema).await;
}
#[rstest::rstest]
#[tokio::test]
async fn malformed_state_respects_pause_live_lease_terminal_and_stop_controls(
	#[from(super::store)] _store_fixture: StoreFuture,
	#[from(super::federation)]
	#[with(_store_fixture.clone())]
	_federation: common::RuntimeFuture,
) {
	let (store, url, schema) = _store_fixture.clone().await.parts();
	let f = _federation.await.federation;
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
	let worker = aidash_server::harness::Harness {
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
	#[from(super::store)] _store_fixture: StoreFuture,
	#[from(super::federation)]
	#[with(_store_fixture.clone())]
	_federation: common::RuntimeFuture,
) {
	let (store, url, schema) = _store_fixture.clone().await.parts();
	let f = _federation.await.federation;
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

#[rstest::rstest]
#[tokio::test]
async fn dependency_release_uses_only_the_authoritative_home(
	#[from(super::store)] _store_fixture: StoreFuture,
	#[from(super::federation)]
	#[with(_store_fixture.clone())]
	_federation: common::RuntimeFuture,
) {
	let (store, url, schema) = _store_fixture.clone().await.parts();
	let f = _federation.await.federation;
	let agent = seed(&f.registry).await;
	let workspace = store
		.create_workspace("Dependencies", "Home authority")
		.await
		.unwrap();
	let dependency = running_task(&store, &agent, workspace.id, None).await;
	sqlx::query(
		&Query::update()
			.table(a("runs"))
			.value(a("control"), "PAUSED")
			.and_where(Expr::col(a("task_id")).eq(Expr::value(dependency.id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(store.pool.driver())
	.await
	.unwrap();
	let mut input = new_task();
	input.dependencies = vec![dependency.id];
	let task = store
		.create_task(workspace.id, &input, "human", None)
		.await
		.unwrap();
	let local = store
		.accept_run(&task, &store.node_id, &agent.id, &agent.version)
		.await
		.unwrap();
	let remote = create(&store, &agent, workspace.id).await;
	sqlx::query(
		&Query::update()
			.table(a("runs"))
			.value(a("home_node"), "aidash://remote-home")
			.and_where(Expr::col(a("id")).eq(Expr::value(remote.id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(store.pool.driver())
	.await
	.unwrap();
	let future = chrono::Utc::now() + chrono::Duration::hours(1);
	for run in [&local, &remote] {
		damage(
			&store,
			run.id,
			RunPhase::Waiting,
			common::pending(RunState::Waiting(Box::new(WaitingState::Dependencies {
				wake_at: future,
				resume: Default::default(),
			}))),
			common::context(json!({})),
		)
		.await;
	}
	assert!(store.lease_run(Uuid::new_v4(), 30).await.unwrap().is_none());
	store
		.complete(
			dependency.id,
			dependency.owner.as_deref().unwrap(),
			"done",
			&ArtifactInput {
				kind: "text".into(),
				name: "done".into(),
				content: json!("done"),
			},
		)
		.await
		.unwrap();
	let claimed = store.lease_run(Uuid::new_v4(), 30).await.unwrap().unwrap();
	assert_eq!(
		claimed.id, local.id,
		"local completion releases before the deadline"
	);
	assert!(store.lease_run(Uuid::new_v4(), 30).await.unwrap().is_none());
	damage(
		&store,
		remote.id,
		RunPhase::Waiting,
		common::pending(RunState::Waiting(Box::new(WaitingState::Dependencies {
			wake_at: chrono::Utc::now() - chrono::Duration::seconds(1),
			resume: Default::default(),
		}))),
		common::context(json!({})),
	)
	.await;
	assert_eq!(
		store
			.lease_run(Uuid::new_v4(), 30)
			.await
			.unwrap()
			.unwrap()
			.id,
		remote.id
	);
	cleanup(store, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn invalid_context_failure_delivery_can_resume_without_effect_execution(
	#[from(super::store)] _store_fixture: StoreFuture,
	#[from(super::federation)]
	#[with(_store_fixture.clone())]
	_federation: common::RuntimeFuture,
) {
	let (store, url, schema) = _store_fixture.clone().await.parts();
	let f = _federation.await.federation;
	let agent = seed(&f.registry).await;
	let workspace = store
		.create_workspace("Delivery", "Resume disposition only")
		.await
		.unwrap();
	let ordinary = create(&store, &agent, workspace.id).await;
	store
		.control(ordinary.id, RunControlAction::Pause)
		.await
		.unwrap();
	damage(
		&store,
		ordinary.id,
		RunPhase::Ready,
		common::pending(RunState::default()),
		json!([]),
	)
	.await;
	assert!(
		store
			.control(ordinary.id, RunControlAction::Resume)
			.await
			.is_err(),
		"ordinary execution must still reject an invalid Context"
	);
	let run = create(&store, &agent, workspace.id).await;
	damage(
		&store,
		run.id,
		RunPhase::Ready,
		common::pending(RunState::default()),
		json!([]),
	)
	.await;
	// Claiming repairs only the disposition; it never constructs a usable Context.
	assert!(store.lease_run(Uuid::new_v4(), 30).await.unwrap().is_none());
	let pending = store.inspect_run(run.id).await.unwrap();
	assert!(pending.state.as_ref().unwrap().failure_delivery());
	assert!(pending.context.is_none());
	assert!(pending.state_error.is_some());
	store
		.control(run.id, RunControlAction::Pause)
		.await
		.unwrap();
	let worker = aidash_server::harness::Harness {
		federation: f.clone(),
	};
	worker.worker_once().await.unwrap();
	assert_eq!(
		store.task(run.task_id).await.unwrap().status,
		TaskStatus::Running
	);
	let resumed = store
		.control(run.id, RunControlAction::Resume)
		.await
		.unwrap();
	assert_eq!(resumed.control, RunControl::Active);
	assert!(resumed.state.as_ref().unwrap().failure_delivery());
	worker.worker_once().await.unwrap();
	assert_eq!(
		store.inspect_run(run.id).await.unwrap().phase,
		RunPhase::Failed
	);
	assert_eq!(
		store.task(run.task_id).await.unwrap().status,
		TaskStatus::Failed
	);
	let context: Value = sqlx::query_scalar(
		&Query::select()
			.column(a("context"))
			.from(a("runs"))
			.and_where(Expr::col(a("id")).eq(Expr::value(run.id)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(store.pool.driver())
	.await
	.unwrap();
	assert_eq!(context, json!([]), "diagnostic Context remains unmodified");
	let effects: i64 = sqlx::query_scalar(
		&Query::select()
			.expr(Expr::cust("COUNT(*)"))
			.from(a("invocations"))
			.and_where(Expr::col(a("run_id")).eq(Expr::value(run.id)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(store.pool.driver())
	.await
	.unwrap();
	assert_eq!(effects, 0);
	assert_eq!(store.inspect_run(run.id).await.unwrap().step, 0);
	// seed's provider endpoint is unavailable: any provider replay would fail
	// worker_once rather than delivering the task's Failed disposition.
	cleanup(store, &url, &schema).await;
}

use reinhardt::query::SimpleExpr;

#[rstest::fixture]
fn malformed_rows_fail_without_effect_replay_and_healthy_work_continues_router(
	#[from(upstream_fixtures::hits)] calls: Arc<std::sync::atomic::AtomicUsize>,
) -> Arc<Router> {
	Arc::new(reinhardt::test::stub::StubRouter::new()
.route("/v1/chat/completions", http::Method::POST, reply(move |_request: reinhardt::Request| {let calls=calls.clone(); async move {
			calls.fetch_add(1,Ordering::SeqCst);
			reinhardt::Response::ok().with_json(&json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"Done"}}],"usage":{"prompt_tokens":100,"completion_tokens":1}})).unwrap()
		}})).into_server_router())
}
