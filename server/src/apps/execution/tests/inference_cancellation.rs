#[path = "support/legacy.rs"]
mod common;

use aidash_server::harness::Harness;
use common::*;
use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::{
	io::{AsyncReadExt, AsyncWriteExt},
	sync::{Notify, oneshot},
	task::JoinSet,
	time::timeout,
};
use uuid::Uuid;

struct StalledProvider {
	endpoint: String,
	entered: oneshot::Receiver<()>,
	disconnected: oneshot::Receiver<()>,
	tasks: JoinSet<()>,
}

#[fixture]
async fn stalled_provider(
	#[default(false)] stall_body: bool,
	#[future(awt)] raw_listener: tokio::net::TcpListener,
	#[from(signal)] entered_signal: Signal,
	#[from(signal)] disconnected_signal: Signal,
) -> StalledProvider {
	let listener = raw_listener;
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let (entered_tx, entered) = entered_signal;
	let (disconnected_tx, disconnected) = disconnected_signal;
	let mut tasks = JoinSet::new();
	tasks.spawn(async move {
			let (mut socket, _) = listener.accept().await.unwrap();
			let mut request = Vec::new();
			let mut buffer = [0_u8; 4096];
			// Consume the actual HTTP request before observing cancellation.
			loop {
				let n = socket.read(&mut buffer).await.unwrap();
				assert!(n > 0, "provider request closed before its body arrived");
				request.extend_from_slice(&buffer[..n]);
				assert!(request.len() <= 1_048_576);
				if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
					let headers = std::str::from_utf8(&request[..end]).unwrap();
					let length: usize = headers
						.lines()
						.filter_map(|line| line.split_once(':'))
						.find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
						.expect("JSON request must have a content length")
						.1
						.trim()
						.parse()
						.unwrap();
					if request.len() >= end + 4 + length {
						let body: Value =
							serde_json::from_slice(&request[end + 4..end + 4 + length]).unwrap();
						assert_eq!(body["model"], "fixture");
						break;
					}
				}
			}
			if stall_body {
				// Headers succeed, but the provider never finishes its JSON body.
				socket
					.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 4096\r\n\r\n{")
					.await
					.unwrap();
			}
			entered_tx.send(()).unwrap();
			let n = socket.read(&mut buffer).await.unwrap();
			assert_eq!(n, 0, "cancellation must close the pending HTTP request");
			disconnected_tx.send(()).unwrap();
		});
	StalledProvider {
		endpoint,
		entered,
		disconnected,
		tasks,
	}
}

struct ReleasableProvider {
	endpoint: String,
	entered: oneshot::Receiver<()>,
	release: Option<oneshot::Sender<()>>,
	tasks: JoinSet<()>,
}

#[fixture]
async fn releasable_provider(
	#[future(awt)] raw_listener: tokio::net::TcpListener,
	#[from(signal)] entered_signal: Signal,
	#[from(signal)] release_signal: Signal,
) -> ReleasableProvider {
	let listener = raw_listener;
	let endpoint = format!("http://{}", listener.local_addr().unwrap());
	let (entered_tx, entered) = entered_signal;
	let (release, release_rx) = release_signal;
	let mut tasks = JoinSet::new();
	tasks.spawn(async move {
			let (mut socket, _) = listener.accept().await.unwrap();
			let mut request = Vec::new();
			let mut buffer = [0_u8; 4096];
			loop {
				let n = socket.read(&mut buffer).await.unwrap();
				assert!(n > 0, "provider request closed before its body arrived");
				request.extend_from_slice(&buffer[..n]);
				if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
					let headers = std::str::from_utf8(&request[..end]).unwrap();
					let length: usize = headers
						.lines()
						.filter_map(|line| line.split_once(':'))
						.find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
						.expect("JSON request must have a content length")
						.1
						.trim()
						.parse()
						.unwrap();
					if request.len() >= end + 4 + length {
						let body: Value =
							serde_json::from_slice(&request[end + 4..end + 4 + length]).unwrap();
						assert_eq!(body["model"], "fixture");
						break;
					}
				}
			}
			entered_tx.send(()).unwrap();
			release_rx.await.unwrap();
			let body = r#"{"choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":"Revoked output"}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#;
			let response = format!(
				"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
				body.len()
			);
			socket.write_all(response.as_bytes()).await.unwrap();
		});
	ReleasableProvider {
		endpoint,
		entered,
		release: Some(release),
		tasks,
	}
}

async fn cancel_stalled_inference(
	fixture: common::ApplicationFixture,
	mut server: StalledProvider,
	scoped: bool,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	// The fixture uses separate notification objects for controller and worker.

	let (_, subject_token, scoped_task) = bootstrap(&f, &app, &server.endpoint).await;
	let (token, task_id) = if scoped {
		(subject_token, scoped_task.to_string())
	} else {
		let token = f.config.api_token.clone();
		let (status, workspace) = request(
			&app,
			&token,
			"POST",
			"/api/workspaces",
			json!({"title":"Legacy cancellation","goal":"Cancel inference"}),
		)
		.await;
		assert_eq!(status, 200, "{workspace}");
		let (status, task) = request(
			&app,
			&token,
			"POST",
			&format!(
				"/api/workspaces/{}/tasks",
				workspace["id"].as_str().unwrap()
			),
			json!({"title":"Cancel inference","description":"No output expected"}),
		)
		.await;
		assert_eq!(status, 200, "{task}");
		(token, task["id"].as_str().unwrap().to_owned())
	};
	let (status, claimed) = request(
		&app,
		&token,
		"POST",
		&format!("/api/tasks/{task_id}/claim"),
		json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}}),
	)
	.await;
	assert_eq!(status, 200, "{claimed}");
	let harness = Harness {
		federation: f.clone(),
	};
	assert!(harness.worker_once().await.unwrap());
	let run = f.store.runs().await.unwrap().remove(0);
	assert_eq!(run.phase().as_str(), "THINKING");

	let mut workers = JoinSet::new();
	let worker = harness.clone();
	workers.spawn(async move { worker.worker_once().await });
	timeout(Duration::from_secs(10), &mut server.entered)
		.await
		.expect("worker must reach the provider")
		.unwrap();
	let (status, cancelled) = timeout(
		Duration::from_secs(5),
		request(
			&app,
			&token,
			"POST",
			&format!("/api/runs/{}/control", run.id),
			json!({"action":"cancel"}),
		),
	)
	.await
	.expect("cancellation must not be blocked by inference");
	assert_eq!(status, 200, "{cancelled}");
	assert_eq!(cancelled["control"], "CANCELLED");
	assert!(
		timeout(Duration::from_secs(5), workers.join_next())
			.await
			.expect("cancelled inference must release the worker promptly")
			.unwrap()
			.unwrap()
			.unwrap()
	);
	timeout(Duration::from_secs(5), &mut server.disconnected)
		.await
		.expect("cancelled inference must drop its HTTP connection")
		.unwrap();
	server.tasks.join_next().await.unwrap().unwrap();

	// Finish the existing durable cancellation path without another inference.
	for _ in 0..3 {
		if f.store.run(run.id).await.unwrap().phase().as_str() == "CANCELLED" {
			break;
		}
		assert!(harness.worker_once().await.unwrap());
	}
	let cancelled = f.store.run(run.id).await.unwrap();
	assert_eq!(cancelled.phase().as_str(), "CANCELLED");
	assert_eq!(cancelled.control.as_str(), "CANCELLED");
	assert!(cancelled.recovery.retry.is_none());
	assert!(matches!(
		cancelled.state,
		aidash_server::domain::RunState::Cancelled(_)
	));
	assert_eq!(
		f.store.task(run.task_id).await.unwrap().status.as_str(),
		"CANCELLED"
	);
	let snapshot = f.store.snapshot(run.workspace_id).await.unwrap();
	assert!(snapshot.artifacts.is_empty());
	assert!(!snapshot.events.iter().any(|e| e.kind == "model.completed"));
	assert!(!harness.worker_once().await.unwrap());
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn model_completion_save_cannot_overwrite_a_committed_cancellation(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	let (_, subject_token, task_id) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (status, claimed) = request(
		&app,
		&subject_token,
		"POST",
		&format!("/api/tasks/{task_id}/claim"),
		json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}}),
	)
	.await;
	assert_eq!(status, 200, "{claimed}");
	let harness = Harness {
		federation: f.clone(),
	};
	assert!(harness.worker_once().await.unwrap());
	let run = f.store.runs().await.unwrap().remove(0);
	assert_eq!(run.phase().as_str(), "THINKING");
	let worker = uuid::Uuid::new_v4();
	let mut leased = f
		.store
		.lease_run(worker, f.config.lease_seconds)
		.await
		.unwrap()
		.unwrap();
	f.store
		.control(run.id, aidash_server::domain::RunControlAction::Cancel)
		.await
		.unwrap();

	leased.state = aidash_server::domain::RunState::ToolCall(Box::new(common::tool_call(json!({
		"response":{"text":"must not be published","tool_calls":[]},
		"cursor":0
	}))));
	let saved = f.store.save_run(&leased, worker, "model.completed").await;
	assert!(matches!(saved, Err(aidash_server::Error::Conflict(_))));
	let current = f.store.run(run.id).await.unwrap();
	assert_eq!(current.control.as_str(), "CANCELLED");
	assert_eq!(current.phase().as_str(), "THINKING");
	assert!(json!(current.state)["data"].get("response").is_none());
	let snapshot = f.store.snapshot(run.workspace_id).await.unwrap();
	assert!(
		!snapshot
			.events
			.iter()
			.any(|event| event.kind == "model.completed")
	);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn credential_revocation_can_finish_during_inference_and_blocks_result(
	#[future(awt)]
	#[from(lease_application)]
	fixture: common::ApplicationFixture,
	#[future(awt)] releasable_provider: ReleasableProvider,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let mut server = releasable_provider;
	let app = fixture.application;
	let (_, subject_token, task_id) = bootstrap(&f, &app, &server.endpoint).await;
	let operator_token = f.config.api_token.clone();
	let (status, claimed) = request(
		&app,
		&subject_token,
		"POST",
		&format!("/api/tasks/{task_id}/claim"),
		json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}}),
	)
	.await;
	assert_eq!(status, 200, "{claimed}");
	let harness = Harness {
		federation: f.clone(),
	};
	assert!(harness.worker_once().await.unwrap());
	let run = f.store.runs().await.unwrap().remove(0);
	assert_eq!(run.phase().as_str(), "THINKING");
	let control_application_name: String = sqlx::query_scalar(
		&Query::select()
			.expr(Expr::cust("current_setting('application_name')"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(f.store.control_pool.driver())
	.await
	.unwrap();
	assert_eq!(control_application_name, schema);
	let idle_transactions_before_inference: i64 = {
		let query_bind_1 = &schema;
		sqlx::query_scalar(
			&Query::select()
				.expr(Expr::cust("COUNT(*)"))
				.from(Alias::new("pg_stat_activity"))
				.cond_where(
					reinhardt::query::Condition::all()
						.add(
							Expr::col(Alias::new("application_name"))
								.eq(Expr::value(query_bind_1.to_owned())),
						)
						.add(Expr::col(Alias::new("state")).eq(Expr::val("idle in transaction"))),
				)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(f.store.pool.driver())
		.await
	}
	.unwrap();
	let mut workers = JoinSet::new();
	let worker = harness.clone();
	workers.spawn(async move { worker.worker_once().await });
	timeout(Duration::from_secs(10), &mut server.entered)
		.await
		.expect("worker must reach the provider")
		.unwrap();
	let idle_transactions_during_inference: i64 = {
		let query_bind_1 = &schema;
		sqlx::query_scalar(
			&Query::select()
				.expr(Expr::cust("COUNT(*)"))
				.from(Alias::new("pg_stat_activity"))
				.cond_where(
					reinhardt::query::Condition::all()
						.add(
							Expr::col(Alias::new("application_name"))
								.eq(Expr::value(query_bind_1.to_owned())),
						)
						.add(Expr::col(Alias::new("state")).eq(Expr::val("idle in transaction"))),
				)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(f.store.pool.driver())
		.await
	}
	.unwrap();
	assert_eq!(
		idle_transactions_during_inference, idle_transactions_before_inference,
		"inference must not hold an idle authorization transaction"
	);
	let (status, credentials) = request(
		&app,
		&operator_token,
		"GET",
		"/api/authorization/acme/credentials",
		json!({}),
	)
	.await;
	assert_eq!(status, 200, "{credentials}");
	let credential_id = credentials[0]["id"].as_str().unwrap();
	let revoke = format!("/api/authorization/acme/credentials/{credential_id}/revoke");
	let (status, revoked) = timeout(
		Duration::from_secs(5),
		request(&app, &operator_token, "POST", &revoke, json!({})),
	)
	.await
	.expect("credential revocation must not wait for the provider");
	assert_eq!(status, 200, "{revoked}");
	server.release.take().unwrap().send(()).unwrap();
	assert!(
		timeout(Duration::from_secs(5), workers.join_next())
			.await
			.expect("revoked inference must be discarded")
			.unwrap()
			.unwrap()
			.unwrap()
	);
	server.tasks.join_next().await.unwrap().unwrap();
	let current = f.store.run(run.id).await.unwrap();
	assert_eq!(current.control.as_str(), "PAUSED");
	assert_eq!(current.phase().as_str(), "THINKING");
	assert!(json!(current.state)["data"].get("response").is_none());
	let snapshot = f.store.snapshot(run.workspace_id).await.unwrap();
	assert!(
		!snapshot
			.events
			.iter()
			.any(|event| event.kind == "model.completed")
	);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn model_infer_policy_revocation_during_inference_blocks_result(
	#[future(awt)]
	#[from(lease_application)]
	fixture: common::ApplicationFixture,
	#[future(awt)] releasable_provider: ReleasableProvider,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let mut server = releasable_provider;
	let app = fixture.application;
	let (mut policy, subject_token, task_id) = bootstrap(&f, &app, &server.endpoint).await;
	let operator_token = f.config.api_token.clone();
	let (status, claimed) = request(
		&app,
		&subject_token,
		"POST",
		&format!("/api/tasks/{task_id}/claim"),
		json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}}),
	)
	.await;
	assert_eq!(status, 200, "{claimed}");
	let harness = Harness {
		federation: f.clone(),
	};
	assert!(harness.worker_once().await.unwrap());
	let run = f.store.runs().await.unwrap().remove(0);
	let mut workers = JoinSet::new();
	let worker = harness.clone();
	workers.spawn(async move { worker.worker_once().await });
	timeout(Duration::from_secs(10), &mut server.entered)
		.await
		.expect("worker must reach the provider")
		.unwrap();
	policy["policies"].as_array_mut().unwrap().push(json!({
		"id":"revoke-model-infer","effect":"deny","subjects":{"any":true},
		"actions":["model.infer"],"resources":{"kinds":["model"]}
	}));
	let (status, changed) = request(
		&app,
		&operator_token,
		"POST",
		"/api/authorization/acme",
		json!({"expected_revision":1,"bundle":policy}),
	)
	.await;
	assert_eq!(status, 200, "{changed}");
	server.release.take().unwrap().send(()).unwrap();
	assert!(
		timeout(Duration::from_secs(5), workers.join_next())
			.await
			.expect("worker must finish after provider response")
			.unwrap()
			.unwrap()
			.unwrap()
	);
	server.tasks.join_next().await.unwrap().unwrap();
	let current = f.store.run(run.id).await.unwrap();
	assert_eq!(current.control.as_str(), "PAUSED");
	assert_eq!(current.phase().as_str(), "THINKING");
	assert!(json!(current.state)["data"].get("response").is_none());
	let snapshot = f.store.snapshot(run.workspace_id).await.unwrap();
	assert!(
		!snapshot
			.events
			.iter()
			.any(|event| event.kind == "model.completed")
	);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn inference_completion_waits_for_visibility_gate_reacquisition(
	#[future(awt)]
	#[from(lease_application)]
	fixture: common::ApplicationFixture,
	#[future(awt)] releasable_provider: ReleasableProvider,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let mut server = releasable_provider;
	let app = fixture.application;
	let (_, subject_token, task_id) = bootstrap(&f, &app, &server.endpoint).await;
	let (status, claimed) = request(
		&app,
		&subject_token,
		"POST",
		&format!("/api/tasks/{task_id}/claim"),
		json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}}),
	)
	.await;
	assert_eq!(status, 200, "{claimed}");
	let harness = Harness {
		federation: f.clone(),
	};
	assert!(harness.worker_once().await.unwrap());
	let run = f.store.runs().await.unwrap().remove(0);
	let mut workers = JoinSet::new();
	let worker = harness.clone();
	workers.spawn(async move { worker.worker_once().await });
	timeout(Duration::from_secs(10), &mut server.entered)
		.await
		.expect("worker must reach the provider")
		.unwrap();
	let mut reservation = f.store.control_pool.driver().begin().await.unwrap();
	let update = Query::update()
		.table(Alias::new("atomic_gate"))
		.value_expr(Alias::new("transaction_id"), Expr::cust("$1"))
		.and_where(
			reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("singleton")))
				.eq(Expr::cust("TRUE")),
		)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&update)
		.bind(Option::<Uuid>::None)
		.execute(&mut *reservation)
		.await
		.unwrap();
	server.release.take().unwrap().send(()).unwrap();
	let finished_while_reserved = timeout(Duration::from_millis(500), workers.join_next()).await;
	let exited_while_reserved = finished_while_reserved.is_ok();
	reservation.commit().await.unwrap();
	let joined = if exited_while_reserved {
		finished_while_reserved
			.unwrap()
			.expect("worker task must exist")
	} else {
		timeout(Duration::from_secs(10), workers.join_next())
			.await
			.expect("worker must retain inference output until the gate is available")
			.expect("worker task must still exist")
	};
	server.tasks.join_next().await.unwrap().unwrap();
	let worker_result = joined
		.map_err(|error| error.to_string())
		.and_then(|result| result.map_err(|error| error.to_string()));
	let current = f.store.run(run.id).await.unwrap();
	let snapshot = f.store.snapshot(run.workspace_id).await.unwrap();
	cleanup(f, &url, &schema).await;
	assert!(
		!exited_while_reserved,
		"worker exited while gate reservation was active: {worker_result:?}"
	);
	assert_eq!(worker_result, Ok(true));
	assert_eq!(current.phase().as_str(), "TOOL_CALL");
	assert_eq!(
		json!(current.state)["data"]["response"]["text"],
		"Revoked output"
	);
	assert!(
		snapshot
			.events
			.iter()
			.any(|event| event.kind == "model.completed")
	);
}

#[rstest::rstest]
#[tokio::test]
async fn inference_result_is_retried_after_atomic_commit_during_provider_wait(
	#[future(awt)]
	#[from(lease_application)]
	fixture: common::ApplicationFixture,
	#[future(awt)] releasable_provider: ReleasableProvider,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let mut server = releasable_provider;
	let app = fixture.application;
	let (_, subject_token, task_id) = bootstrap(&f, &app, &server.endpoint).await;
	let (status, claimed) = request(
		&app,
		&subject_token,
		"POST",
		&format!("/api/tasks/{task_id}/claim"),
		json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}}),
	)
	.await;
	assert_eq!(status, 200, "{claimed}");
	let harness = Harness {
		federation: f.clone(),
	};
	assert!(harness.worker_once().await.unwrap());
	let run = f.store.runs().await.unwrap().remove(0);
	let mut workers = JoinSet::new();
	let worker = harness.clone();
	workers.spawn(async move { worker.worker_once().await });
	timeout(Duration::from_secs(10), &mut server.entered)
		.await
		.expect("worker must reach the provider")
		.unwrap();
	let mut transaction = f.store.control_pool.driver().begin().await.unwrap();
	let update = Query::update()
		.table(Alias::new("atomic_gate"))
		.value_expr(Alias::new("commit_epoch"), Expr::cust("commit_epoch + 1"))
		.and_where(Expr::col(Alias::new("singleton")).eq(reinhardt::query::Expr::value(true)))
		.to_string(PostgresQueryBuilder);
	sqlx::query(&update)
		.execute(&mut *transaction)
		.await
		.unwrap();
	transaction.commit().await.unwrap();
	server.release.take().unwrap().send(()).unwrap();
	let worker_result = timeout(Duration::from_secs(10), workers.join_next())
		.await
		.expect("worker must discard the stale response and schedule a retry")
		.expect("worker task must still exist")
		.map_err(|error| error.to_string())
		.and_then(|result| result.map_err(|error| error.to_string()));
	server.tasks.join_next().await.unwrap().unwrap();
	let current = f.store.run(run.id).await.unwrap();
	let snapshot = f.store.snapshot(run.workspace_id).await.unwrap();
	cleanup(f, &url, &schema).await;
	assert_eq!(worker_result, Ok(true));
	assert_eq!(current.phase().as_str(), "THINKING");
	assert!(current.recovery.retry.is_some());
	assert!(json!(current.state)["data"].get("response").is_none());
	assert!(
		!snapshot
			.events
			.iter()
			.any(|event| event.kind == "model.completed")
	);
}

#[rstest::rstest]
#[tokio::test]
async fn scoped_cancellation_aborts_inference_before_response_headers(
	#[future(awt)]
	#[from(cancellation_application)]
	fixture: common::ApplicationFixture,
	#[future(awt)]
	#[from(stalled_provider)]
	#[with(false)]
	server: StalledProvider,
) {
	cancel_stalled_inference(fixture, server, true).await;
}

#[rstest::rstest]
#[tokio::test]
async fn legacy_cancellation_aborts_inference_during_response_body(
	#[future(awt)]
	#[from(cancellation_application)]
	fixture: common::ApplicationFixture,
	#[future(awt)]
	#[from(stalled_provider)]
	#[with(true)]
	server: StalledProvider,
) {
	cancel_stalled_inference(fixture, server, false).await;
}

use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};

use futures_util::FutureExt;
use rstest::fixture;

type Signal = (oneshot::Sender<()>, oneshot::Receiver<()>);
#[fixture]
fn signal() -> Signal {
	oneshot::channel()
}
#[fixture]
async fn raw_listener() -> tokio::net::TcpListener {
	// Incomplete HTTP headers/bodies and cancellation require the actual TCP state;
	// a framework HTTP handler cannot expose the stalled socket's disconnect.
	tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap()
}

#[fixture]
fn long_lease_runtime(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) -> common::RuntimeFuture {
	async move {
		let mut runtime = runtime.await;
		runtime.federation.config.lease_seconds = 300;
		runtime
	}
	.boxed()
	.shared()
}
#[fixture]
fn controller_notify() -> Arc<Notify> {
	Arc::new(Notify::new())
}
#[fixture]
fn controller_runtime(
	#[from(long_lease_runtime)] runtime: common::RuntimeFuture,
	controller_notify: Arc<Notify>,
) -> common::RuntimeFuture {
	async move {
		let mut runtime = runtime.await;
		runtime.federation.notify = controller_notify;
		runtime
	}
	.boxed()
	.shared()
}
#[fixture]
fn lease_application(
	#[from(long_lease_runtime)] _runtime: common::RuntimeFuture,
	#[from(common::native_application)]
	#[with(aidash_server::http::Settings::default(), aidash_server::sse::Service::new(aidash_server::sse::Settings::default()), Arc::new(|router| router), _runtime.clone())]
	application: common::ApplicationFuture,
) -> common::ApplicationFuture {
	application
}
#[fixture]
fn cancellation_application(
	#[from(long_lease_runtime)] runtime: common::RuntimeFuture,
	#[from(controller_runtime)]
	#[with(runtime.clone())]
	_controller: common::RuntimeFuture,
	#[from(common::native_application)]
	#[with(aidash_server::http::Settings::default(), aidash_server::sse::Service::new(aidash_server::sse::Settings::default()), Arc::new(|router| router), _controller.clone())]
	application: common::ApplicationFuture,
) -> common::ApplicationFuture {
	async move {
		let mut application = application.await;
		application.runtime = runtime.await;
		application
	}
	.boxed()
	.shared()
}
