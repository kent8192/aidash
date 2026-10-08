#[path = "support/upstream.rs"]
mod upstream_fixtures;
use reinhardt::ServerRouter as Router;
use reinhardt::test::fixtures::server::TestServerGuard;
use upstream_fixtures::handler;
#[path = "support/legacy.rs"]
mod common;
#[path = "worker_activation/review.rs"]
mod review;
#[path = "worker_activation/unblock.rs"]
mod unblock;
use aidash_server::{
	activation::{Broker, Settings},
	federation::Federation,
};

use common::*;
use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
use serde_json::{Value, json};
use std::{
	path::PathBuf,
	sync::{
		Arc,
		atomic::{AtomicUsize, Ordering},
	},
	time::{Duration, Instant},
};
use uuid::Uuid;

static PROCESS_TESTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn a(s: &str) -> Alias {
	Alias::new(s)
}
struct Process {
	_fixture_owner: Option<Arc<PreparedActivation>>,
	child: std::process::Child,
	log: PathBuf,
	consumes: bool,
	#[cfg(target_os = "macos")]
	_binary_directory: Arc<ActivationBinary>,
}
impl Process {
	fn start(
		f: &Federation,
		url: &str,
		schema: &str,
		mode: &str,
		directory: &std::path::Path,
		mode_settings: (usize, bool),
		binary: Arc<ActivationBinary>,
	) -> Self {
		let (ordinal, delayed) = mode_settings;
		let mut database = reqwest::Url::parse(url).unwrap();
		database
			.query_pairs_mut()
			.append_pair("options", &format!("-c application_name={schema}"));
		let log = directory.join(format!("{mode}-{ordinal}.log"));
		let file = std::fs::File::create(&log).unwrap();
		let executable = &binary.executable;
		let mut cmd = std::process::Command::new(executable);
		cmd.args(native_process_args(f, mode))
			.envs(native_process_environment(
				f,
				database.as_str(),
				directory,
				if mode == "server" { 0 } else { 2 },
			))
			.env("DATABASE_URL", database.as_str())
			.env("AIDASH_NODE_ID", &f.config.node_id)
			.env("AIDASH_ENDPOINT", &f.config.endpoint)
			.env(
				"AIDASH_LISTEN",
				reqwest::Url::parse(&f.config.endpoint)
					.unwrap()
					.socket_addrs(|| None)
					.unwrap()[0]
					.to_string(),
			)
			.env("AIDASH_API_TOKEN", &f.config.api_token)
			.env("NATS_URL", &f.config.nats_url)
			.env("AIDASH_ACTIVATION_NAMESPACE", schema)
			.env("AIDASH_ACTIVATION_BOOTSTRAP", "false")
			.env("AIDASH_WORKER_SLOTS", "2")
			.env("AIDASH_ENV", "test")
			.env(
				"AIDASH_ACTIVATION_TEST_AFTER_ACK_PAUSE_FILE",
				directory.join("pause-after-ack"),
			)
			.env(
				"AIDASH_ACTIVATION_TEST_PAUSE_FILE",
				directory.join("pause-consumers"),
			)
			.env("RUST_LOG", "aidash=info")
			.stdout(file.try_clone().unwrap())
			.stderr(file);
		if delayed {
			cmd.env("AIDASH_ACTIVATION_TEST_RECOVERY_MS", "60000");
		} else {
			cmd.env_remove("AIDASH_ACTIVATION_TEST_RECOVERY_MS");
		}
		Self {
			_fixture_owner: None,
			child: cmd.spawn().unwrap(),
			log,
			consumes: mode != "server",
			#[cfg(target_os = "macos")]
			_binary_directory: binary,
		}
	}
	async fn ready(&mut self) {
		let start = Instant::now();
		loop {
			let log = std::fs::read_to_string(&self.log).unwrap();
			if log.contains("activation transport ready")
				&& (!self.consumes
					|| log.matches("activation startup recovery complete").count() >= 2)
			{
				break;
			}
			assert!(
				self.child.try_wait().unwrap().is_none(),
				"process exited: {log}"
			);
			assert!(
				start.elapsed() < Duration::from_secs(20),
				"startup timeout: {log}"
			);
			tokio::time::sleep(Duration::from_millis(25)).await;
		}
		// All slots perform their startup recovery before opening measured windows.
		tokio::time::sleep(Duration::from_millis(100)).await;
	}
	fn stop(&mut self) {
		let _ = std::process::Command::new("kill")
			.args(["-TERM", &self.child.id().to_string()])
			.status();
		let start = Instant::now();
		while self.child.try_wait().unwrap().is_none() && start.elapsed() < Duration::from_secs(22)
		{
			std::thread::sleep(Duration::from_millis(20));
		}
	}
}
impl Drop for Process {
	fn drop(&mut self) {
		let _ = self.child.kill();
		let _ = self.child.wait();
	}
}
async fn count(f: &Federation, predicate: &str) -> i64 {
	sqlx::query_scalar(
		&Query::select()
			.expr(Expr::cust("COUNT(*)"))
			.from(a("run_activations"))
			.and_where(Expr::cust(predicate))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(f.store.pool.driver())
	.await
	.unwrap()
}
async fn admit(f: &Federation, token: &str, task: Uuid) -> Uuid {
	let response = f
		.client
		.clone()
		.post(format!("{}/api/tasks/{task}/claim", f.config.endpoint))
		.bearer_auth(token)
		.json(&json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}}))
		.send()
		.await
		.unwrap();
	let status = response.status();
	let value: Value = response.json().await.unwrap();
	assert!(status.is_success(), "admission: {status} {value}");
	f.store
		.runs()
		.await
		.unwrap()
		.into_iter()
		.find(|r| r.task_id == task)
		.unwrap()
		.id
}
async fn complete(f: &Federation, id: Uuid) {
	let start = Instant::now();

	loop {
		let run = f.store.run(id).await.unwrap();
		if run.phase().as_str() == "COMPLETED" {
			break;
		}
		if start.elapsed() >= Duration::from_secs(20) {
			let activations: Value = sqlx::query_scalar(
				&Query::select()
					.expr(Expr::cust(
						"COALESCE(JSONB_AGG(TO_JSONB(run_activations) ORDER BY generation), '[]'::jsonb)",
					))
					.from(a("run_activations"))
					.and_where(Expr::col(a("run_id")).eq(Expr::value(id)))
					.to_string(PostgresQueryBuilder),
			)
			.fetch_one(f.store.pool.driver())
			.await
			.unwrap();
			panic!("did not complete: {run:?}; activations: {activations}");
		}
		tokio::time::sleep(Duration::from_millis(20)).await;
	}
}

#[rstest::rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn separate_process_notifications_and_negative_control(
	#[from(activation_runtime)] _runtime: common::RuntimeFuture,

	#[from(upstream_fixtures::hits)] calls: Arc<AtomicUsize>,
	#[from(separate_process_notifications_and_negative_control_router)]
	#[with(calls.clone())]
	_router: Arc<Router>,
	#[from(activation_binary)] binary: Arc<ActivationBinary>,
	#[from(prepared_activation)]
	#[with("main", _runtime.clone(), _router.clone(), None)]
	prepared: PreparedActivationFuture,
	#[future(awt)]
	#[from(initial_process)]
	#[with(prepared.clone(), "server", 0, true, true)]
	server: Process,
	#[future(awt)]
	#[from(initial_process)]
	#[with(prepared.clone(), "worker", 0, true, true)]
	first: Process,
) {
	// rstest 0.26.1 loses `from` on mutable awaited parameters; rebind after injection.
	let mut server = server;
	let mut first = first;

	let prepared = prepared.await;
	let (f, url, schema) = prepared.runtime.parts();
	let directory = prepared.directory.path.clone();
	let token = prepared.token.clone();
	let task = prepared.task;
	let broker = prepared.broker.clone();
	let provisioned = broker.clone();
	let provider_task = prepared.provider.clone();

	let recovery_negative = count(&f, "claim_source = 'recovery'").await;
	// Keep a running worker, a live broker, and the publisher; suppress only pulls.
	std::fs::write(directory.join("pause-consumers"), b"test barrier").unwrap();
	tokio::time::sleep(Duration::from_millis(150)).await;
	let negative = admit(&f, &token, task).await;
	tokio::time::sleep(Duration::from_secs(2)).await;
	assert!(count(&f, "published_at IS NOT NULL").await > 0);
	assert_eq!(count(&f, "claimed_at IS NOT NULL").await, 0);
	std::fs::remove_file(directory.join("pause-consumers")).unwrap();
	complete(&f, negative).await;
	assert_eq!(
		count(&f, "claim_source = 'recovery'").await,
		recovery_negative
	);
	assert!(count(&f, "claim_source = 'notification'").await > 0);
	first.stop();
	drop(first);

	let mut samples = Vec::new();
	let mut pids = Vec::new();
	// Each window has a fresh 60s recovery delay and fewer than 60s of samples.
	for window in 0..5 {
		let mut one = /* Act: launch/relaunch tests worker lifecycle and negative controls. */ Process::start(&f, &url, &schema, "worker", &directory, (window * 2 + 1, true), binary.clone());
		let mut two = /* Act: launch/relaunch tests worker lifecycle and negative controls. */ Process::start(&f, &url, &schema, "worker", &directory, (window * 2 + 2, true), binary.clone());
		one.ready().await;
		two.ready().await;
		pids.extend([one.child.id(), two.child.id()]);
		let recovery_before = count(&f, "claim_source = 'recovery'").await;
		let window_start = Instant::now();
		for index in 0..20 {
			// Keep the activation workload constant: historical workspace
			// observation growth is a different performance dimension.
			let workspace: Value = f
				.client
				.clone()
				.post(format!("{}/api/workspaces", f.config.endpoint))
				.bearer_auth(&token)
				.json(&json!({"title":"Activation sample","goal":"Complete"}))
				.send()
				.await
				.unwrap()
				.error_for_status()
				.unwrap()
				.json()
				.await
				.unwrap();
			let workspace = workspace["id"].as_str().unwrap();
			let response = f
				.client
				.clone()
				.post(format!(
					"{}/api/workspaces/{workspace}/tasks",
					f.config.endpoint
				))
				.bearer_auth(&token)
				.json(&json!({"title":"Activation sample","description":"Complete"}))
				.send()
				.await
				.unwrap();
			let task: Value = response.error_for_status().unwrap().json().await.unwrap();
			let task = Uuid::parse_str(task["id"].as_str().unwrap()).unwrap();
			let start = Instant::now();
			let id = admit(&f, &token, task).await;
			let first_claim: (Uuid, i64, String, i64) = loop {
				let query = Query::select()
					.columns([a("id"), a("generation"), a("claim_source"), a("worker_pid")])
					.from(a("run_activations"))
					.and_where(Expr::col(a("run_id")).eq(reinhardt::query::Expr::value(id)))
					.and_where(Expr::col(a("claimed_at")).is_not_null())
					.order_by(a("claimed_at"), reinhardt::query::Order::Asc)
					.limit(1)
					.to_string(PostgresQueryBuilder);
				if let Some(row) = sqlx::query_as(&query)
					.fetch_optional(f.store.pool.driver())
					.await
					.unwrap()
				{
					break row;
				}
				assert!(
					start.elapsed() < Duration::from_secs(2),
					"first lease exceeded 2s for {id}"
				);
				tokio::time::sleep(Duration::from_millis(5)).await;
			};
			let elapsed = start.elapsed().as_secs_f64() * 1000.;
			assert_eq!(first_claim.2, "notification");
			assert!([one.child.id() as i64, two.child.id() as i64].contains(&first_claim.3));
			samples.push(json!({"window":window,"sample":index,"run_id":id,"activation_id":first_claim.0,"generation":first_claim.1,"worker_pid":first_claim.3,"upper_bound_ms":elapsed}));
			complete(&f, id).await;
		}
		assert!(
			window_start.elapsed() < Duration::from_secs(55),
			"recovery suppression window exceeded"
		);
		assert_eq!(
			count(&f, "claim_source = 'recovery'").await,
			recovery_before
		);
		one.stop();
		two.stop();
	}
	// A suspended replica cannot hoard prefetched work. The other replica must
	// drain a batch larger than its two execution slots through notifications.
	let mut busy = /* Act: launch/relaunch tests worker lifecycle and negative controls. */ Process::start(&f, &url, &schema, "worker", &directory, (11, true), binary.clone());
	let mut available = /* Act: launch/relaunch tests worker lifecycle and negative controls. */ Process::start(&f, &url, &schema, "worker", &directory, (12, true), binary.clone());
	busy.ready().await;
	available.ready().await;
	assert!(
		std::process::Command::new("kill")
			.args(["-STOP", &busy.child.id().to_string()])
			.status()
			.unwrap()
			.success()
	);
	let mut batch = Vec::new();
	for _ in 0..6 {
		batch.push(admit(&f, &token, fresh_task(&f, &token).await).await);
	}
	for id in &batch {
		complete(&f, *id).await;
	}
	for id in &batch {
		let owners: Vec<i64> = sqlx::query_scalar(
			&Query::select()
				.column(a("worker_pid"))
				.from(a("run_activations"))
				.and_where(Expr::col(a("run_id")).eq(Expr::value(*id)))
				.and_where(Expr::col(a("claimed_at")).is_not_null())
				.to_string(PostgresQueryBuilder),
		)
		.fetch_all(f.store.pool.driver())
		.await
		.unwrap();
		assert!(!owners.is_empty());
		assert!(
			owners
				.iter()
				.all(|pid| *pid == i64::from(available.child.id()))
		);
	}
	let busy_pid = busy.child.id();
	let available_pid = available.child.id();
	assert!(
		std::process::Command::new("kill")
			.args(["-CONT", &busy_pid.to_string()])
			.status()
			.unwrap()
			.success()
	);
	busy.stop();
	available.stop();
	let mut timings: Vec<f64> = samples
		.iter()
		.map(|v| v["upper_bound_ms"].as_f64().unwrap())
		.collect();
	timings.sort_by(f64::total_cmp);
	let p95 = timings[94];
	let max = timings[99];
	assert!(p95 <= 1000. && max <= 2000., "p95={p95} max={max}");
	assert_eq!(
		calls.load(Ordering::SeqCst),
		107,
		"one real provider completion per admission"
	);
	assert_eq!(
		count(&f, &format!("worker_pid = {}", server.child.id())).await,
		0
	);
	std::fs::write(directory.join("notification-results.json"),serde_json::to_vec_pretty(&json!({
        "AT01":"passed","AT02":"passed with running worker consumption suppressed","AT04":{"suspended_pid":busy_pid,"executing_pid":available_pid,"batch":6},
        "server_pid":server.child.id(),"worker_pids":pids,"slots_per_worker":2,"sample_count":100,"p95_ms":p95,"max_ms":max,
        "measurement":"before HTTP admission to observed committed first lease; upper bound, monotonic time",
        "recovery_delay_ms":60000,"measured_recovery_claims":0,"provider_calls":calls.load(Ordering::SeqCst),"samples":samples
    })).unwrap()).unwrap();
	println!(
		"activation evidence: {} p95={p95:.1}ms max={max:.1}ms",
		directory.display()
	);
	server.stop();
	drop(server);
	drop(provider_task);
	provisioned
		.context
		.delete_stream(&provisioned.stream_name)
		.await
		.unwrap();
	cleanup(f, &url, &schema).await;
}

async fn post_json(f: &Federation, token: &str, path: &str, body: Value) -> Value {
	let response = f
		.client
		.clone()
		.post(format!("{}{path}", f.config.endpoint))
		.bearer_auth(token)
		.json(&body)
		.send()
		.await
		.unwrap();
	let status = response.status();
	let body: Value = response.json().await.unwrap();
	assert!(status.is_success(), "{path}: {status} {body}");
	body
}
async fn fresh_task(f: &Federation, token: &str) -> Uuid {
	let workspace = post_json(
		f,
		token,
		"/api/workspaces",
		json!({"title":"Isolated activation","goal":"Complete"}),
	)
	.await;
	let task = post_json(
		f,
		token,
		&format!(
			"/api/workspaces/{}/tasks",
			workspace["id"].as_str().unwrap()
		),
		json!({"title":"Fixture","description":"Complete"}),
	)
	.await;
	Uuid::parse_str(task["id"].as_str().unwrap()).unwrap()
}
async fn wait_count(f: &Federation, predicate: &str, minimum: i64) {
	let start = Instant::now();
	while count(f, predicate).await < minimum {
		assert!(
			start.elapsed() < Duration::from_secs(8),
			"missing durable state: {predicate}"
		);
		tokio::time::sleep(Duration::from_millis(20)).await;
	}
}

#[rstest::rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn atomic_deadlines_duplicates_quarantine_and_input_during_active_owner(
	#[from(activation_runtime)] _runtime: common::RuntimeFuture,
	#[from(deadline_state)] _state: DeadlineState,
	#[from(atomic_deadlines_duplicates_quarantine_and_input_during_active_owner_router)]
	#[with(_state.calls.clone(), _state.blocked.clone(), _state.permits.clone())]
	_router: Arc<Router>,
	#[from(prepared_activation)]
	#[with("faults", _runtime.clone(), _router.clone(), None)]
	prepared: PreparedActivationFuture,
	#[future(awt)]
	#[from(initial_process)]
	#[with(prepared.clone(), "serve", 0, true, false)]
	combined: Process,
	#[future(awt)]
	#[from(initial_process)]
	#[with(prepared.clone(), "worker", 0, true, false)]
	worker: Process,
) {
	let DeadlineState {
		calls,
		blocked,
		permits,
	} = _state;
	let mut combined = combined;
	let mut worker = worker;

	let prepared = prepared.await;
	let (f, url, schema) = prepared.runtime.parts();
	let directory = prepared.directory.path.clone();
	let token = prepared.token.clone();
	let task = prepared.task;
	let broker = prepared.broker.clone();
	let provider = prepared.provider.clone();

	combined.ready().await;
	worker.ready().await;
	let pause = directory.join("pause-consumers");
	std::fs::write(&pause, b"pause").unwrap();
	tokio::time::sleep(Duration::from_millis(150)).await;
	let run = admit(&f, &token, task).await;
	let original = count(&f, "TRUE").await;
	let mut tx = f.store.pool.begin().await.unwrap();
	aidash_server::database::native::query(
		&Query::update()
			.table(a("runs"))
			.value(a("control"), "PAUSED")
			.and_where(Expr::col(a("id")).eq(reinhardt::query::Expr::value(run)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	aidash_server::activation::request_in(&mut tx, run)
		.await
		.unwrap();
	let within: i64 = aidash_server::database::native::query_scalar(
		&Query::select()
			.expr(Expr::cust("COUNT(*)"))
			.from(a("run_activations"))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&mut *tx)
	.await
	.unwrap();
	assert!(within >= original + 2);
	tx.rollback().await.unwrap();
	assert_eq!(count(&f, "TRUE").await, original);
	assert_eq!(f.store.run(run).await.unwrap().control.as_str(), "ACTIVE");
	// Rollback preserved both state and scheduling; a committed future wait
	// now has an independent deadline and must not be claimed early.
	sqlx::query(
		&Query::update()
			.table(a("runs"))
			.value(a("phase"), "WAITING")
			.value_expr(a("revision"), Expr::col(a("revision")).add(1))
			.value_expr(
				a("pending"),
				Expr::value(common::pending(aidash_server::domain::RunState::Waiting(
					Box::new(aidash_server::domain::WaitingState::Timer {
						wake_at: chrono::Utc::now() + chrono::Duration::seconds(3),
						resume: aidash_server::domain::ResumeState::Ready(Default::default()),
					}),
				))),
			)
			.and_where(Expr::col(a("id")).eq(reinhardt::query::Expr::value(run)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(f.store.pool.driver())
	.await
	.unwrap();
	std::fs::remove_file(&pause).unwrap();
	tokio::time::sleep(Duration::from_millis(1200)).await;
	assert!(f.store.run(run).await.unwrap().lease_owner.is_none());
	assert!(count(&f, "state = 'deferred' AND due_at > CURRENT_TIMESTAMP").await > 0);
	complete(&f, run).await;
	assert_eq!(count(&f, "claim_source = 'recovery'").await, 0);
	let before = calls.load(Ordering::SeqCst);
	let row: (Uuid, i64) = sqlx::query_as(
		&Query::select()
			.columns([a("id"), a("generation")])
			.from(a("run_activations"))
			.and_where(Expr::col(a("run_id")).eq(reinhardt::query::Expr::value(run)))
			.order_by(a("generation"), reinhardt::query::Order::Asc)
			.limit(1)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(f.store.pool.driver())
	.await
	.unwrap();
	let envelope = aidash_server::activation::Envelope {
		version: 1,
		node_id: f.config.node_id.clone(),
		run_id: run,
		activation_id: row.0,
		generation: row.1,
	};
	// Omit broker dedup headers: every duplicate reaches the DB disposition path.
	for _ in 0..5 {
		broker
			.context
			.publish(
				broker.subject.clone(),
				serde_json::to_vec(&envelope).unwrap().into(),
			)
			.await
			.unwrap()
			.await
			.unwrap();
	}
	let mut wrong = envelope.clone();
	wrong.node_id = "aidash://other".into();
	let mut unsupported = envelope.clone();
	unsupported.version = 999;
	let mut mismatch = envelope.clone();
	mismatch.activation_id = Uuid::new_v4();
	for payload in [
		b"malformed-private-payload".to_vec(),
		serde_json::to_vec(&wrong).unwrap(),
		serde_json::to_vec(&unsupported).unwrap(),
		serde_json::to_vec(&mismatch).unwrap(),
	] {
		broker
			.context
			.publish(broker.subject.clone(), payload.into())
			.await
			.unwrap()
			.await
			.unwrap();
	}
	let start = Instant::now();
	loop {
		let quarantined: i64 = sqlx::query_scalar(
			&Query::select()
				.expr(Expr::cust("COUNT(*)"))
				.from(a("activation_quarantine"))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(f.store.pool.driver())
		.await
		.unwrap();
		if quarantined == 4 {
			break;
		}
		assert!(start.elapsed() < Duration::from_secs(5));
		tokio::time::sleep(Duration::from_millis(20)).await;
	}
	assert_eq!(calls.load(Ordering::SeqCst), before);
	assert_eq!(
		f.store.run(run).await.unwrap().phase().as_str(),
		"COMPLETED"
	);
	// New accepted input during inference is deferred by another free worker,
	// then survives the old response's stale-inference rejection and retry.
	blocked.store(true, Ordering::SeqCst);
	let task = fresh_task(&f, &token).await;
	let active = admit(&f, &token, task).await;
	let start = Instant::now();
	while calls.load(Ordering::SeqCst) == before {
		assert!(start.elapsed() < Duration::from_secs(5));
		tokio::time::sleep(Duration::from_millis(20)).await;
	}
	post_json(
		&f,
		&token,
		&format!("/api/runs/{active}/message"),
		json!({"content":"Accepted correction","idempotency_key":Uuid::new_v4()}),
	)
	.await;
	wait_count(
		&f,
		&format!("run_id = '{active}' AND reason = 'input' AND state = 'deferred'"),
		1,
	)
	.await;
	assert_eq!(f.store.run(active).await.unwrap().observed_input_seq, 0);
	blocked.store(false, Ordering::SeqCst);
	permits.add_permits(1);
	complete(&f, active).await;
	let finished = f.store.run(active).await.unwrap();
	let seq: i64 = sqlx::query_scalar(
		&Query::select()
			.expr(Expr::cust("MAX(seq)"))
			.from(a("run_inputs"))
			.and_where(Expr::col(a("run_id")).eq(reinhardt::query::Expr::value(active)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(f.store.pool.driver())
	.await
	.unwrap();
	assert_eq!(finished.observed_input_seq, seq);
	assert_eq!(
		calls.load(Ordering::SeqCst),
		before + 2,
		"stale first response must be re-inferred"
	);
	assert_eq!(count(&f, "claim_source = 'recovery'").await, 0);
	std::fs::write(directory.join("result.json"),serde_json::to_vec_pretty(&json!({"result":"passed","combined_pid":combined.child.id(),"worker_pid":worker.child.id(),"atomic_rollback":true,"deadline_without_recovery":true,"database_duplicate_disposition":true,"quarantine_cases":4,"input_while_owned":true,"provider_calls":calls.load(Ordering::SeqCst)})).unwrap()).unwrap();
	combined.stop();
	worker.stop();
	drop(combined);
	drop(worker);
	drop(provider);
	broker
		.context
		.delete_stream(&broker.stream_name)
		.await
		.unwrap();
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn broker_absence_reconnect_and_empty_storage_preserve_accepted_work(
	#[from(activation_runtime)] _runtime: common::RuntimeFuture,

	#[from(upstream_fixtures::hits)] calls: Arc<AtomicUsize>,
	#[from(broker_absence_reconnect_and_empty_storage_preserve_accepted_work_router)]
	#[with(calls.clone())]
	_router: Arc<Router>,

	#[from(nats_outage_proxy)]
	#[with(_runtime.clone())]
	_proxy: NatsOutageFuture,
	#[from(activation_binary)] binary: Arc<ActivationBinary>,
	#[from(prepared_activation)]
	#[with("outage", _runtime.clone(), _router.clone(), Some(_proxy.clone()))]
	prepared: PreparedActivationFuture,
	#[future(awt)]
	#[from(initial_process)]
	#[with(prepared.clone(), "serve", 0, false, false)]
	process: Process,
) {
	let mut process = process;

	let prepared = prepared.await;
	let (f, url, schema) = prepared.runtime.parts();
	let environment = prepared.runtime.environment();
	let directory = prepared.directory.path.clone();
	let token = prepared.token.clone();
	let task = prepared.task;
	let settings = prepared.settings.clone();
	let broker = prepared.broker.clone();
	let provider = prepared.provider.clone();
	let online = prepared._proxy.as_ref().unwrap().online.clone();

	let start = Instant::now();
	loop {
		if f.client
			.get(format!("{}/health", f.config.endpoint))
			.send()
			.await
			.is_ok_and(|r| r.status().is_success())
		{
			break;
		}
		assert!(process.child.try_wait().unwrap().is_none());
		assert!(start.elapsed() < Duration::from_secs(10));
		tokio::time::sleep(Duration::from_millis(25)).await;
	}
	let first = admit(&f, &token, task).await;
	complete(&f, first).await;
	assert!(count(&f, "claim_source = 'recovery'").await > 0);
	assert_eq!(count(&f, "claim_source = 'notification'").await, 0);
	online.send_replace(true);
	process.ready().await;
	let second = admit(&f, &token, fresh_task(&f, &token).await).await;
	complete(&f, second).await;
	assert!(count(&f, "claim_source = 'notification'").await > 0);
	// Transport-only storage loss. The accepted obligation remains in PostgreSQL.
	let pause = directory.join("pause-consumers");
	std::fs::write(&pause, b"pause").unwrap();
	tokio::time::sleep(Duration::from_millis(150)).await;
	let third = admit(&f, &token, fresh_task(&f, &token).await).await;
	wait_count(
		&f,
		&format!("run_id = '{third}' AND published_at IS NOT NULL"),
		1,
	)
	.await;
	online.send_replace(false);
	broker
		.context
		.delete_stream(&broker.stream_name)
		.await
		.unwrap();
	let restored = Broker::provision(&environment.nats_url, &f.config.node_id, &settings)
		.await
		.unwrap();
	online.send_replace(true);
	std::fs::remove_file(&pause).unwrap();
	complete(&f, third).await;
	assert_eq!(calls.load(Ordering::SeqCst), 3);
	let restored_at = Instant::now();
	loop {
		let log = std::fs::read_to_string(&process.log).unwrap();
		assert!(log.contains("fallback"));
		if log.matches("activation transport ready").count() >= 2 {
			break;
		}
		assert!(
			restored_at.elapsed() < Duration::from_secs(10),
			"transport did not revalidate after reconnection"
		);
		tokio::time::sleep(Duration::from_millis(25)).await;
	}
	// A fresh process delays all broad recovery, proving that capacity repair
	// and retention expiry converge through durable publication alone.
	process.stop();
	drop(process);
	let mut process = /* Act: launch/relaunch tests worker lifecycle and negative controls. */ Process::start(&f, &url, &schema, "serve", &directory, (1, true), binary.clone());
	process.ready().await;
	std::fs::write(&pause, b"pause").unwrap();
	tokio::time::sleep(Duration::from_millis(150)).await;
	let mut stream = restored
		.context
		.get_stream(&restored.stream_name)
		.await
		.unwrap();
	let baseline = stream.info().await.unwrap().config.clone();
	// Retain one real activation, then reject further publication while workers
	// are paused. Draining must recover without operator capacity repair.
	let queued = admit(&f, &token, fresh_task(&f, &token).await).await;
	wait_count(
		&f,
		&format!("run_id = '{queued}' AND published_at IS NOT NULL"),
		1,
	)
	.await;
	let mut full = baseline.clone();
	full.max_bytes = stream.info().await.unwrap().state.bytes as i64;
	restored.context.update_stream(full).await.unwrap();
	let rejected = restored
		.context
		.publish(restored.subject.clone(), b"capacity".as_slice().into())
		.await
		.unwrap()
		.await;
	assert!(
		rejected.is_err(),
		"DiscardNew must refuse over-capacity publication"
	);
	let fourth = admit(&f, &token, fresh_task(&f, &token).await).await;
	wait_count(
		&f,
		&format!("run_id = '{fourth}' AND publish_until IS NOT NULL"),
		1,
	)
	.await;
	assert!(f.store.run(fourth).await.unwrap().lease_owner.is_none());
	// Let the failed publisher finish before opening the consumer slots.
	tokio::time::sleep(Duration::from_secs(1)).await;
	std::fs::remove_file(&pause).unwrap();
	wait_count(
		&f,
		&format!("run_id = '{queued}' AND claim_source = 'notification'"),
		1,
	)
	.await;
	restored
		.context
		.update_stream(baseline.clone())
		.await
		.unwrap();
	complete(&f, queued).await;
	complete(&f, fourth).await;
	assert_eq!(
		count(
			&f,
			&format!("run_id = '{fourth}' AND claim_source = 'recovery'")
		)
		.await,
		0
	);
	// Shorten only the disposable stream's retention after setup. Restore its
	// validated configuration before reconnect; never change database deadlines.
	std::fs::write(&pause, b"pause").unwrap();
	tokio::time::sleep(Duration::from_millis(150)).await;
	let fifth = admit(&f, &token, fresh_task(&f, &token).await).await;
	wait_count(
		&f,
		&format!("run_id = '{fifth}' AND published_at IS NOT NULL"),
		1,
	)
	.await;
	let mut expiring = baseline.clone();
	expiring.max_age = Duration::from_secs(1);
	expiring.duplicate_window = Duration::from_secs(1);
	restored.context.update_stream(expiring).await.unwrap();
	let expires = Instant::now();
	while stream.info().await.unwrap().state.messages != 0 {
		assert!(
			expires.elapsed() < Duration::from_secs(4),
			"message did not expire"
		);
		tokio::time::sleep(Duration::from_millis(25)).await;
	}
	assert!(f.store.run(fifth).await.unwrap().lease_owner.is_none());
	restored.context.update_stream(baseline).await.unwrap();
	std::fs::remove_file(&pause).unwrap();
	complete(&f, fifth).await;
	assert_eq!(
		count(
			&f,
			&format!("run_id = '{fifth}' AND claim_source = 'recovery'")
		)
		.await,
		0
	);
	assert!(count(&f, &format!("run_id = '{fifth}' AND publication_epoch > 0")).await > 0);
	assert_eq!(calls.load(Ordering::SeqCst), 6);
	std::fs::write(directory.join("result.json"),serde_json::to_vec_pretty(&json!({"result":"passed","combined_pid":process.child.id(),"broker_absent_at_startup":true,"reconnected":true,"stream_deleted_and_recreated":true,"capacity_refusal_kept_consumer_draining":true,"expired_notification_republished_without_recovery":true,"provider_calls":calls.load(Ordering::SeqCst)})).unwrap()).unwrap();
	process.stop();
	drop(process);
	prepared._proxy.as_ref().unwrap().task.abort();
	drop(provider);
	restored
		.context
		.delete_stream(&restored.stream_name)
		.await
		.unwrap();
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn killed_after_ack_recovers_only_after_real_lease_expiry(
	#[from(activation_runtime)] _runtime: common::RuntimeFuture,

	#[from(upstream_fixtures::hits)] calls: Arc<AtomicUsize>,
	#[from(killed_after_ack_recovers_only_after_real_lease_expiry_router)]
	#[with(calls.clone())]
	_router: Arc<Router>,
	#[from(activation_binary)] binary: Arc<ActivationBinary>,
	#[from(prepared_activation)]
	#[with("crash", _runtime.clone(), _router.clone(), None)]
	prepared: PreparedActivationFuture,
	#[future(awt)]
	#[from(initial_process)]
	#[with(prepared.clone(), "server", 0, true, true)]
	server: Process,
	#[future(awt)]
	#[from(initial_process)]
	#[with(prepared.clone(), "worker", 0, true, true)]
	worker: Process,
) {
	let mut server = server;
	let mut worker = worker;

	let prepared = prepared.await;
	let (f, url, schema) = prepared.runtime.parts();
	let directory = prepared.directory.path.clone();
	let token = prepared.token.clone();
	let task = prepared.task;
	let broker = prepared.broker.clone();
	let provider = prepared.provider.clone();

	let barrier = directory.join("pause-after-ack");
	std::fs::write(&barrier, b"crash checkpoint").unwrap();
	let id = admit(&f, &token, task).await;
	wait_count(&f, "state = 'claimed'", 1).await;
	let leased = f.store.run(id).await.unwrap();
	let owner = leased.lease_owner.unwrap();
	let stream = broker
		.context
		.get_stream(&broker.stream_name)
		.await
		.unwrap();
	let mut consumer: async_nats::jetstream::consumer::PullConsumer =
		stream.get_consumer("workers-v1").await.unwrap();
	let start = Instant::now();
	while consumer.info().await.unwrap().num_ack_pending != 0 {
		assert!(start.elapsed() < Duration::from_secs(3));
		tokio::time::sleep(Duration::from_millis(20)).await;
	}
	assert_eq!(calls.load(Ordering::SeqCst), 0);
	worker.child.kill().unwrap();
	worker.child.wait().unwrap();
	drop(worker);
	std::fs::remove_file(&barrier).unwrap();
	let mut replacement = /* Act: launch/relaunch tests worker lifecycle and negative controls. */ Process::start(&f, &url, &schema, "worker", &directory, (1, true), binary.clone());
	replacement.ready().await;
	// Keep the real lease duration. No SQL timestamp shortening hides early takeover.
	loop {
		let current = f.store.run(id).await.unwrap();
		let now: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
			&Query::select()
				.expr(Expr::cust("CURRENT_TIMESTAMP"))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(f.store.pool.driver())
		.await
		.unwrap();
		if now >= leased.lease_until.unwrap() {
			break;
		}
		assert_eq!(current.lease_owner, Some(owner));
		assert_eq!(current.revision, leased.revision);
		assert_eq!(calls.load(Ordering::SeqCst), 0);
		tokio::time::sleep(Duration::from_millis(100)).await;
	}
	complete(&f, id).await;
	assert_eq!(calls.load(Ordering::SeqCst), 1);
	assert_eq!(
		count(&f, "claim_source = 'recovery'").await,
		0,
		"explicit lease-expiry obligation must activate recovery"
	);
	std::fs::write(directory.join("result.json"),serde_json::to_vec_pretty(&json!({"result":"passed","server_pid":server.child.id(),"replacement_pid":replacement.child.id(),"crash_checkpoint":"committed lease and broker ACK before execution","original_lease_until":leased.lease_until,"early_takeover":false,"recovery_claims":0,"provider_calls":calls.load(Ordering::SeqCst)})).unwrap()).unwrap();
	replacement.stop();
	server.stop();
	drop(replacement);
	drop(server);
	drop(provider);
	broker
		.context
		.delete_stream(&broker.stream_name)
		.await
		.unwrap();
	cleanup(f, &url, &schema).await;
}

struct NatsContainer(String);
impl Drop for NatsContainer {
	fn drop(&mut self) {
		let _ = std::process::Command::new("docker")
			.args(["rm", "--force", &self.0])
			.stdout(std::process::Stdio::null())
			.stderr(std::process::Stdio::null())
			.status();
	}
}
#[rstest::rstest]
#[tokio::test]
async fn scoped_credentials_deny_server_pull_and_validate_without_resetting_consumer(
	#[future] scoped_nats: ScopedNats,
) {
	use futures_util::StreamExt;
	let fixture = scoped_nats.await;
	let operator = fixture.operator.clone();
	let operator_url = fixture.operator_url.clone();
	let server_url = fixture.server_url.clone();
	let worker_url = fixture.worker_url.clone();
	let settings = fixture.settings.clone();
	let node = fixture.node;
	let stream = fixture.stream.clone();
	let subject = fixture.subject.clone();
	let next = fixture.next.clone();
	let directory = fixture.directory.clone();

	let created = operator.consumer.as_ref().unwrap().cached_info().created;
	let mut wrong = settings.clone();
	wrong.max_bytes += 1;
	assert!(
		Broker::connect(&operator_url, node, &wrong, true)
			.await
			.is_err()
	);
	assert_eq!(
		Broker::provision(&operator_url, node, &settings)
			.await
			.unwrap()
			.consumer
			.unwrap()
			.cached_info()
			.created,
		created
	);
	let server = Broker::connect(&server_url, node, &settings, false)
		.await
		.unwrap();
	assert!(server.consumer.is_none());
	assert!(
		Broker::connect(&server_url, node, &settings, true)
			.await
			.is_err(),
		"server cannot inspect/consume the worker consumer"
	);
	assert!(
		Broker::connect(&worker_url, "aidash://another-node", &settings, true)
			.await
			.is_err()
	);
	server
		.context
		.publish(subject.clone(), b"reference-fixture".as_slice().into())
		.await
		.unwrap()
		.await
		.unwrap();
	let mut consumer = operator.consumer.clone().unwrap();
	assert_eq!(consumer.info().await.unwrap().num_pending, 1);
	let denied = tokio::time::timeout(
		Duration::from_millis(300),
		server.context.client().request(
			next.clone(),
			b"{\"batch\":1,\"no_wait\":true}".as_slice().into(),
		),
	)
	.await;
	assert!(denied.is_err() || denied.unwrap().is_err());
	assert_eq!(
		consumer.info().await.unwrap().delivered.consumer_sequence,
		0,
		"server request drained a worker activation"
	);
	let worker = Broker::connect(&worker_url, node, &settings, true)
		.await
		.unwrap();
	let mut messages = worker
		.consumer
		.as_ref()
		.unwrap()
		.fetch()
		.max_messages(1)
		.messages()
		.await
		.unwrap();
	let message = messages.next().await.unwrap().unwrap();
	message.double_ack().await.unwrap();
	assert_eq!(consumer.info().await.unwrap().num_ack_pending, 0);
	std::fs::write(directory.join("result.json"),serde_json::to_vec_pretty(&json!({"result":"passed","stream":stream,"server_publish":true,"server_pull_denied":true,"worker_pull_ack":true,"other_node_denied":true,"mismatch_did_not_reset_consumer":true,"consumer_created":created.to_string()})).unwrap()).unwrap();
}

#[rstest::rstest]
#[tokio::test]
async fn startup_reconciliation_drains_all_batches(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let runtime = runtime.await;
	let (f, url, schema) = runtime.parts();
	let mut insert = Query::insert();
	insert.into_table(a("runs")).columns([
		a("id"),
		a("task_id"),
		a("workspace_id"),
		a("home_node"),
		a("agent_id"),
		a("agent_version"),
		a("control"),
	]);
	for _ in 0..257 {
		insert.values_panic::<_, reinhardt::query::Value>([
			Uuid::new_v4().into(),
			Uuid::new_v4().into(),
			Uuid::new_v4().into(),
			f.config.node_id.clone().into(),
			"fixture".into(),
			"1.0.0".into(),
			"PAUSED".into(),
		]);
	}
	sqlx::query(&insert.to_string(PostgresQueryBuilder))
		.execute(f.store.pool.driver())
		.await
		.unwrap();
	// Model an upgrade from writers predating the activation table/triggers.
	sqlx::query(
		&Query::delete()
			.from_table(a("run_activations"))
			.to_string(PostgresQueryBuilder),
	)
	.execute(f.store.pool.driver())
	.await
	.unwrap();
	let settings = Settings {
		namespace: schema.clone(),
		..Default::default()
	};
	let broker = Broker::provision(&f.config.nats_url, &f.config.node_id, &settings)
		.await
		.unwrap();
	let runtime = aidash_server::activation::Runtime::new(f.clone(), settings, false);
	let (stop, stopping) = tokio::sync::watch::channel(false);
	let task = tokio::spawn(runtime.run(stopping));
	let outcome = tokio::time::timeout(Duration::from_secs(5), async {
		while count(&f, "reason = 'reconcile'").await < 257 {
			tokio::time::sleep(Duration::from_millis(20)).await;
		}
	})
	.await;
	stop.send_replace(true);
	task.await.unwrap().unwrap();
	assert!(
		outcome.is_ok(),
		"startup backfill stopped before the final batch"
	);
	assert_eq!(count(&f, "reason = 'reconcile'").await, 257);
	broker
		.context
		.delete_stream(&broker.stream_name)
		.await
		.unwrap();
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[case::explicit_shutdown(true)]
#[case::continuous_worker(false)]
#[tokio::test]
async fn embedded_worker_uses_dedicated_activation_broker(
	#[case] explicit_shutdown: bool,
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let runtime = runtime.await;
	let (f, url, schema) = runtime.parts();
	let settings = Settings {
		namespace: schema.clone(),
		..Default::default()
	};
	let broker = Broker::provision(&f.config.nats_url, &f.config.node_id, &settings)
		.await
		.unwrap();
	broker
		.context
		.publish(broker.subject.clone(), "invalid activation".into())
		.await
		.unwrap()
		.await
		.unwrap();
	let mut database = reqwest::Url::parse(&url).unwrap();
	database
		.query_pairs_mut()
		.append_pair("options", &format!("-c application_name={schema}"));
	let output = tokio::process::Command::new(std::env::current_exe().unwrap())
		.args([
			"--exact",
			"embedded_worker_child",
			"--ignored",
			"--nocapture",
		])
		.env("AIDASH_ACTIVATION_CHILD_DATABASE", database.as_str())
		.env(
			"AIDASH_ACTIVATION_CHILD_UNTIL",
			explicit_shutdown.to_string(),
		)
		.env("AIDASH_ACTIVATION_NATS_URL", &f.config.nats_url)
		.env("AIDASH_ACTIVATION_NAMESPACE", &schema)
		.env("AIDASH_ACTIVATION_BOOTSTRAP", "false")
		.env_remove("AIDASH_ACTIVATION_TEST_RECOVERY_MS")
		.env_remove("AIDASH_ACTIVATION_TEST_PAUSE_FILE")
		.env_remove("AIDASH_ACTIVATION_TEST_AFTER_ACK_PAUSE_FILE")
		.kill_on_drop(true)
		.output()
		.await
		.unwrap();
	assert!(
		output.status.success(),
		"{}\n{}",
		String::from_utf8_lossy(&output.stdout),
		String::from_utf8_lossy(&output.stderr)
	);
	broker
		.context
		.delete_stream(&broker.stream_name)
		.await
		.unwrap();
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
#[ignore = "subprocess helper: dedicated broker environment must be isolated"]
async fn embedded_worker_child(#[future(awt)] child_runtime: Federation) {
	let f = child_runtime;
	let harness = aidash_server::harness::Harness {
		federation: f.clone(),
	};
	let (stop, stopping) = tokio::sync::watch::channel(false);
	let explicit_shutdown = std::env::var("AIDASH_ACTIVATION_CHILD_UNTIL").unwrap() == "true";
	let mut worker = tokio::spawn(async move {
		if explicit_shutdown {
			harness.run_worker_until(stopping).await
		} else {
			harness.run_worker().await
		}
	});
	let outcome = tokio::time::timeout(Duration::from_secs(15), async {
		loop {
			let quarantined: i64 = sqlx::query_scalar(
				&Query::select()
					.expr(Expr::cust("COUNT(*)"))
					.from(a("activation_quarantine"))
					.to_string(PostgresQueryBuilder),
			)
			.fetch_one(f.store.pool.driver())
			.await
			.unwrap();
			if quarantined == 1 {
				break;
			}
			tokio::time::sleep(Duration::from_millis(20)).await;
		}
	})
	.await;
	assert!(
		!worker.is_finished(),
		"worker exited without a shutdown request"
	);
	if explicit_shutdown {
		stop.send_replace(true);
		(&mut worker).await.unwrap().unwrap();
	} else {
		worker.abort();
		assert!(worker.await.unwrap_err().is_cancelled());
	}
	assert!(
		outcome.is_ok(),
		"embedded worker never consumed the dedicated broker notification"
	);
	assert_eq!(f.config.nats_url, "nats://127.0.0.1:1");
}

use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};

#[rstest::fixture]
fn activation_address() -> std::net::SocketAddr {
	// Real child processes bind this reserved address; restart tests require a stable port.
	std::net::TcpListener::bind("127.0.0.1:0")
		.unwrap()
		.local_addr()
		.unwrap()
}
#[rstest::fixture]
fn activation_runtime(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
	activation_address: std::net::SocketAddr,
) -> common::RuntimeFuture {
	use futures_util::FutureExt;
	async move {
		let mut runtime = runtime.await;
		runtime.federation.config.endpoint = format!("http://{activation_address}");
		runtime
	}
	.boxed()
	.shared()
}

#[rstest::fixture]
async fn child_runtime(
	#[from(reinhardt::test::fixtures::http_client)] reinhardt_http_client: reqwest::Client,
) -> Federation {
	let database = std::env::var("AIDASH_ACTIVATION_CHILD_DATABASE").unwrap();
	let node = "aidash://execution-test";
	let pool = sqlx::PgPool::connect(&database).await.unwrap();
	let store = aidash_server::store::Store::from_pool(pool, node.into())
		.await
		.unwrap();
	Federation {
		sandbox: Default::default(),
		registry: aidash_server::registry::Registry::new(store.pool.clone(), node).unwrap(),
		store,
		config: aidash_server::config::Config {
			node_id: node.into(),
			endpoint: "http://127.0.0.1:8080".into(),
			database_url: database,
			nats_url: "nats://127.0.0.1:1".into(),
			api_token: "fixture".into(),
			web_dir: "web/dist".into(),
			lease_seconds: 30,
			default_host_packages: vec![],
			oidc: None,
		},
		client: reinhardt_http_client,
		notify: Arc::new(tokio::sync::Notify::new()),
	}
}

#[rstest::fixture]
fn separate_process_notifications_and_negative_control_router(
	#[from(upstream_fixtures::hits)] calls: Arc<AtomicUsize>,
) -> Arc<Router> {
	Arc::new(Router::new().handler("/v1/chat/completions",handler(http::Method::POST, move |_request: reinhardt::Request| {
        let calls = calls.clone(); async move {
            calls.fetch_add(1,Ordering::SeqCst);
            reinhardt::Response::ok().with_json(&json!({"choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":"Activation fixture completed"}}],"usage":{"prompt_tokens":1,"completion_tokens":1}})).unwrap()
        }
    })))
}
#[rstest::fixture]
fn atomic_deadlines_duplicates_quarantine_and_input_during_active_owner_router(
	#[from(upstream_fixtures::hits)] calls: Arc<AtomicUsize>,
	#[from(upstream_fixtures::available)] blocked: Arc<std::sync::atomic::AtomicBool>,
	permits: Arc<tokio::sync::Semaphore>,
) -> Arc<Router> {
	Arc::new(Router::new().handler("/v1/chat/completions",handler(http::Method::POST, {
        let calls=calls.clone();let blocked=blocked.clone();let permits=permits.clone();
        move |_request: reinhardt::Request| { let calls=calls.clone();let blocked=blocked.clone();let permits=permits.clone(); async move {
            calls.fetch_add(1,Ordering::SeqCst);
            if blocked.load(Ordering::SeqCst) { permits.acquire().await.unwrap().forget(); }
            reinhardt::Response::ok().with_json(&json!({"choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":"Fixture completed"}}],"usage":{"prompt_tokens":1,"completion_tokens":1}})).unwrap()
        }}
    })))
}
#[rstest::fixture]
fn broker_absence_reconnect_and_empty_storage_preserve_accepted_work_router(
	#[from(upstream_fixtures::hits)] calls: Arc<AtomicUsize>,
) -> Arc<Router> {
	Arc::new(Router::new().handler("/v1/chat/completions",handler(http::Method::POST, move |_request: reinhardt::Request| {let calls=calls.clone();async move {
        calls.fetch_add(1,Ordering::SeqCst);
        reinhardt::Response::ok().with_json(&json!({"choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":"Recovered"}}],"usage":{"prompt_tokens":1,"completion_tokens":1}})).unwrap()
    }})))
}
#[rstest::fixture]
fn killed_after_ack_recovers_only_after_real_lease_expiry_router(
	#[from(upstream_fixtures::hits)] calls: Arc<AtomicUsize>,
) -> Arc<Router> {
	Arc::new(Router::new().handler("/v1/chat/completions",handler(http::Method::POST, move |_request: reinhardt::Request| {let calls=calls.clone();async move {
        calls.fetch_add(1,Ordering::SeqCst);
        reinhardt::Response::ok().with_json(&json!({"choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":"Recovered once"}}],"usage":{"prompt_tokens":1,"completion_tokens":1}})).unwrap()
    }})))
}

#[rstest::fixture]
fn permits() -> Arc<tokio::sync::Semaphore> {
	Arc::new(tokio::sync::Semaphore::new(0))
}
#[rstest::fixture]
fn workspace_id() -> Uuid {
	Uuid::new_v4()
}

type NatsOutageFuture =
	futures_util::future::Shared<futures_util::future::BoxFuture<'static, Arc<NatsOutageProxy>>>;
struct NatsOutageProxy {
	_environment: Arc<TestEnvironment>,
	endpoint: String,
	online: tokio::sync::watch::Sender<bool>,
	task: tokio::task::JoinHandle<()>,
}
impl Drop for NatsOutageProxy {
	fn drop(&mut self) {
		self.task.abort();
	}
}
type NatsListenerFuture = futures_util::future::BoxFuture<'static, tokio::net::TcpListener>;
#[rstest::fixture]
fn nats_proxy_listener() -> NatsListenerFuture {
	use futures_util::FutureExt;
	async move {
		// Raw NATS frames and disconnects are the test contract; HTTP guards cannot proxy them.
		tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap()
	}
	.boxed()
}
#[rstest::fixture]
fn broker_online() -> tokio::sync::watch::Sender<bool> {
	tokio::sync::watch::channel(false).0
}
#[rstest::fixture]
fn nats_outage_proxy(
	#[from(activation_runtime)] runtime: common::RuntimeFuture,
	nats_proxy_listener: NatsListenerFuture,
	broker_online: tokio::sync::watch::Sender<bool>,
) -> NatsOutageFuture {
	use futures_util::FutureExt;
	async move {
		let runtime = runtime.await;
		let environment = runtime.environment();
		let nats = reqwest::Url::parse(&environment.nats_url).unwrap();
		let upstream = format!("{}:{}", nats.host_str().unwrap(), nats.port().unwrap());
		let listener = nats_proxy_listener.await;
		let endpoint = format!("nats://{}", listener.local_addr().unwrap());
		let online = broker_online;
		let mut status = online.subscribe();
		let proxy = tokio::spawn(async move {
			// Aborting the fixture task drops this JoinSet and cancels every live bridge.
			let mut connections = tokio::task::JoinSet::new();
			loop {
				let (mut client, _) = listener.accept().await.unwrap();
				if !*status.borrow_and_update() {
					continue;
				}
				let upstream = upstream.clone();
				let mut status = status.clone();
				connections.spawn(async move {
					let mut server = tokio::net::TcpStream::connect(upstream).await.unwrap();
					tokio::select! {
						_=tokio::io::copy_bidirectional(&mut client,&mut server)=>{},
						_=status.changed()=>{},
					}
				});
			}
		});
		Arc::new(NatsOutageProxy {
			_environment: environment,
			endpoint,
			online,
			task: proxy,
		})
	}
	.boxed()
	.shared()
}

struct ActivationBinary {
	executable: PathBuf,
	_directory: tempfile::TempDir,
}
#[rstest::fixture]
fn activation_binary(
	#[from(reinhardt::test::fixtures::temp_dir)] temp_dir: tempfile::TempDir,
) -> Arc<ActivationBinary> {
	#[cfg(target_os = "macos")]
	let executable = {
		let path = temp_dir.path().join("aidash");
		std::fs::copy(env!("CARGO_BIN_EXE_aidash"), &path)
			.expect("snapshot exact child executable on local storage");
		path
	};
	#[cfg(not(target_os = "macos"))]
	let executable = PathBuf::from(env!("CARGO_BIN_EXE_aidash"));
	Arc::new(ActivationBinary {
		executable,
		_directory: temp_dir,
	})
}
type PreparedActivationFuture =
	futures_util::future::Shared<futures_util::future::BoxFuture<'static, Arc<PreparedActivation>>>;
struct PreparedActivation {
	runtime: common::RuntimeFixture,
	_application: common::TestApplication,
	provider: Arc<TestServerGuard>,
	token: String,
	task: Uuid,
	settings: Settings,
	broker: Broker,
	directory: Arc<ActivationDirectory>,
	_serial: tokio::sync::MutexGuard<'static, ()>,
	_proxy: Option<Arc<NatsOutageProxy>>,
}
struct ActivationDirectory {
	path: PathBuf,
	_directory: tempfile::TempDir,
}
type ActivationDirectoryFuture = futures_util::future::Shared<
	futures_util::future::BoxFuture<'static, Arc<ActivationDirectory>>,
>;
#[rstest::fixture]
fn activation_directory(
	#[default("main")] label: &str,
	#[from(activation_runtime)] runtime: common::RuntimeFuture,
	#[from(reinhardt::test::fixtures::temp_dir)] temp_dir: tempfile::TempDir,
) -> ActivationDirectoryFuture {
	use futures_util::FutureExt;
	let label = label.to_owned();
	async move {
		let _runtime = runtime.await;
		let path = std::env::var_os("AIDASH_ACTIVATION_EVIDENCE_DIR")
			.map(|path| {
				if label == "main" {
					PathBuf::from(path)
				} else {
					PathBuf::from(path).join(&label)
				}
			})
			.unwrap_or_else(|| temp_dir.path().join(&label));
		std::fs::create_dir_all(&path).unwrap();
		Arc::new(ActivationDirectory {
			path,
			_directory: temp_dir,
		})
	}
	.boxed()
	.shared()
}
#[rstest::fixture]
fn process_serial() -> futures_util::future::BoxFuture<'static, tokio::sync::MutexGuard<'static, ()>>
{
	use futures_util::FutureExt;
	PROCESS_TESTS.lock().boxed()
}
#[rstest::fixture]
fn prepared_activation(
	#[default("main")] label: &str,
	#[from(activation_runtime)] runtime: common::RuntimeFuture,
	#[default(Arc::new(Router::new()))] router: Arc<Router>,
	#[default(None)] proxy: Option<NatsOutageFuture>,
	#[from(common::native_application)]
	#[with(Default::default(), aidash_server::sse::Service::new(Default::default()), Arc::new(|router| router), runtime.clone())]
	application: common::ApplicationFuture,
	#[from(upstream_fixtures::provider_transport)]
	#[with(router.clone())]
	provider: upstream_fixtures::UpstreamFuture,
	#[from(activation_resources)]
	#[with(label, runtime.clone())]
	_state: ActivationResources,
) -> PreparedActivationFuture {
	let ActivationResources {
		directory,
		process_serial,
	} = _state;
	let _ = (label, router);
	use futures_util::FutureExt;
	async move {
		let serial = process_serial.await;
		let mut runtime = runtime.await;
		let application = application.await.application;
		let provider = provider.await;
		let directory = directory.await;
		let (_, token, task) = bootstrap(&runtime.federation, &application, &provider.url).await;
		let settings = Settings {
			namespace: runtime.schema.clone(),
			..Default::default()
		};
		let broker = Broker::provision(
			&runtime.federation.config.nats_url,
			&runtime.federation.config.node_id,
			&settings,
		)
		.await
		.unwrap();
		let proxy = if let Some(proxy) = proxy {
			let proxy = proxy.await;
			runtime.federation.config.nats_url = proxy.endpoint.clone();
			Some(proxy)
		} else {
			None
		};
		Arc::new(PreparedActivation {
			runtime,
			_application: application,
			provider,
			token,
			task,
			settings,
			broker,
			directory,
			_serial: serial,
			_proxy: proxy,
		})
	}
	.boxed()
	.shared()
}
struct InitialCommand {
	command: std::process::Command,
	log: PathBuf,
	consumes: bool,
	binary: Arc<ActivationBinary>,
}
type InitialCommandFuture = futures_util::future::BoxFuture<'static, InitialCommand>;
#[rstest::fixture]
fn initial_command(
	prepared_activation: PreparedActivationFuture,
	#[default("server")] mode: &str,
	#[default(0)] ordinal: usize,
	#[default(true)] delayed: bool,
	#[from(activation_binary)] binary: Arc<ActivationBinary>,
	#[from(initial_environment)]
	#[with(prepared_activation.clone(), mode)]
	environment: InitialEnvironmentFuture,
) -> InitialCommandFuture {
	use futures_util::FutureExt;
	let mode = mode.to_owned();
	async move {
		let prepared = prepared_activation.await;
		let f = &prepared.runtime.federation;
		let url = prepared.runtime.url.as_str();
		let schema = prepared.runtime.schema.as_str();
		let directory = prepared.directory.path.as_path();
		let mode = mode.as_str();

		let mut database = reqwest::Url::parse(url).unwrap();
		database
			.query_pairs_mut()
			.append_pair("options", &format!("-c application_name={schema}"));
		let log = directory.join(format!("{mode}-{ordinal}.log"));
		let file = std::fs::File::create(&log).unwrap();
		let executable = &binary.executable;
		let mut cmd = std::process::Command::new(executable);
		cmd.args([mode])
			.envs(environment.await)
			.env("DATABASE_URL", database.as_str())
			.env("AIDASH_NODE_ID", &f.config.node_id)
			.env("AIDASH_ENDPOINT", &f.config.endpoint)
			.env(
				"AIDASH_LISTEN",
				reqwest::Url::parse(&f.config.endpoint)
					.unwrap()
					.socket_addrs(|| None)
					.unwrap()[0]
					.to_string(),
			)
			.env("AIDASH_API_TOKEN", &f.config.api_token)
			.env("NATS_URL", &f.config.nats_url)
			.env("AIDASH_ACTIVATION_NAMESPACE", schema)
			.env("AIDASH_ACTIVATION_BOOTSTRAP", "false")
			.env("AIDASH_WORKER_SLOTS", "2")
			.env("AIDASH_ENV", "test")
			.env(
				"AIDASH_ACTIVATION_TEST_AFTER_ACK_PAUSE_FILE",
				directory.join("pause-after-ack"),
			)
			.env(
				"AIDASH_ACTIVATION_TEST_PAUSE_FILE",
				directory.join("pause-consumers"),
			)
			.env("RUST_LOG", "aidash=info")
			.stdout(file.try_clone().unwrap())
			.stderr(file);
		if delayed {
			cmd.env("AIDASH_ACTIVATION_TEST_RECOVERY_MS", "60000");
		} else {
			cmd.env_remove("AIDASH_ACTIVATION_TEST_RECOVERY_MS");
		}
		InitialCommand {
			command: cmd,
			log,
			consumes: mode != "server",
			binary,
		}
	}
	.boxed()
}
#[rstest::fixture]
async fn initial_process(
	prepared_activation: PreparedActivationFuture,
	#[default("server")] mode: &str,
	#[default(0)] ordinal: usize,
	#[default(true)] delayed: bool,
	#[default(true)] wait_for_ready: bool,
	#[from(initial_command)]
	#[with(prepared_activation.clone(), mode, ordinal, delayed)]
	initial_command: InitialCommandFuture,
) -> Process {
	let owner = prepared_activation.await;
	let _ = (mode, ordinal, delayed);
	let InitialCommand {
		mut command,
		log,
		consumes,
		binary,
	} = initial_command.await;
	let mut process = Process {
		_fixture_owner: Some(owner),
		child: command.spawn().unwrap(),
		log,
		consumes,
		#[cfg(target_os = "macos")]
		_binary_directory: binary,
	};
	#[cfg(not(target_os = "macos"))]
	let _ = binary;
	if wait_for_ready {
		process.ready().await;
	}
	process
}

struct ScopedNats {
	operator: Broker,
	operator_url: String,
	server_url: String,
	worker_url: String,
	settings: Settings,
	node: &'static str,
	stream: String,
	subject: String,
	next: String,
	directory: PathBuf,
	_directory: tempfile::TempDir,
	_container: NatsContainer,
	_serial: tokio::sync::MutexGuard<'static, ()>,
}
#[rstest::fixture]
async fn scoped_nats(
	#[from(reinhardt::test::fixtures::temp_dir)] temp_dir: tempfile::TempDir,
	process_serial: futures_util::future::BoxFuture<'static, tokio::sync::MutexGuard<'static, ()>>,
) -> ScopedNats {
	// reinhardt-web#6660: no NATS fixture exposes the exact scoped JetStream permissions.

	let serial = process_serial.await;
	let name = format!("aidash-activation-permissions-{}", Uuid::new_v4().simple());
	let directory = std::env::var_os("AIDASH_ACTIVATION_EVIDENCE_DIR")
		.map(|path| PathBuf::from(path).join("permissions"))
		.unwrap_or_else(|| temp_dir.path().join("permissions"));
	std::fs::create_dir_all(&directory).unwrap();
	let settings = Settings {
		namespace: name.clone(),
		..Default::default()
	};
	let node = "aidash://permissions";
	let (stream, subject) = Broker::names(node, &settings);
	let next = format!("$JS.API.CONSUMER.MSG.NEXT.{stream}.workers-v1");
	// Public local-only fixture credentials. Permissions are scoped to one exact
	// activation stream; no wildcard crosses Node scopes or grants provisioning.
	let config = format!(
		r#"
port: 4222
jetstream {{ store_dir: "/tmp/jetstream" }}
authorization {{ users: [
 {{ user: "operator", password: "fixture-operator" }},
 {{ user: "server", password: "fixture-server", permissions: {{
   publish: {{ allow: ["{subject}","$JS.API.STREAM.INFO.{stream}"] }}, subscribe: {{allow:["_INBOX.>"]}}
 }} }},
 {{ user: "worker", password: "fixture-worker", permissions: {{
   publish: {{ allow: ["{subject}","$JS.API.STREAM.INFO.{stream}","$JS.API.CONSUMER.INFO.{stream}.workers-v1","{next}","$JS.ACK.{stream}.workers-v1.>"] }}, subscribe: {{allow:["_INBOX.>"]}}
 }} }}
] }}
"#
	);
	let path = directory.join("nats.conf");
	std::fs::write(&path, config).unwrap();
	let output = std::process::Command::new("docker")
		.args([
			"run",
			"--detach",
			"--rm",
			"--name",
			&name,
			"--publish",
			"127.0.0.1::4222",
			"--volume",
			&format!("{}:/etc/nats/auth.conf:ro", path.display()),
			"nats:2.12-alpine",
			"--config",
			"/etc/nats/auth.conf",
		])
		.output()
		.unwrap();
	assert!(output.status.success(), "start scoped NATS fixture");
	let _container = NatsContainer(name.clone());
	let port = std::process::Command::new("docker")
		.args(["port", &name, "4222/tcp"])
		.output()
		.unwrap();
	let address = String::from_utf8(port.stdout).unwrap().trim().to_owned();
	let operator_url = format!("nats://operator:fixture-operator@{address}");
	let server_url = format!("nats://server:fixture-server@{address}");
	let worker_url = format!("nats://worker:fixture-worker@{address}");
	let start = Instant::now();
	let operator = loop {
		if let Ok(broker) = Broker::provision(&operator_url, node, &settings).await {
			break broker;
		}
		if start.elapsed() >= Duration::from_secs(10) {
			let logs = std::process::Command::new("docker")
				.args(["logs", &name])
				.output()
				.unwrap();
			panic!(
				"scoped NATS setup failed: {}",
				String::from_utf8_lossy(&logs.stderr)
			);
		}
		tokio::time::sleep(Duration::from_millis(50)).await;
	};
	ScopedNats {
		operator,
		operator_url,
		server_url,
		worker_url,
		settings,
		node,
		stream,
		subject,
		next,
		directory,
		_directory: temp_dir,
		_container,
		_serial: serial,
	}
}

type InitialEnvironmentFuture =
	futures_util::future::BoxFuture<'static, Vec<(&'static str, String)>>;
#[rstest::fixture]
fn initial_environment(
	prepared_activation: PreparedActivationFuture,
	#[default("server")] mode: &str,
) -> InitialEnvironmentFuture {
	use futures_util::FutureExt;
	let worker_count = if mode == "server" { 0 } else { 2 };
	async move {
		let prepared = prepared_activation.await;
		let f = &prepared.runtime.federation;
		let mut database = reqwest::Url::parse(&prepared.runtime.url).unwrap();
		database.query_pairs_mut().append_pair(
			"options",
			&format!("-c application_name={}", prepared.runtime.schema),
		);
		let database_url = database.as_str();
		let directory = prepared.directory.path.as_path();

		let mut settings = settings_for(database_url);
		let database = reqwest::Url::parse(database_url).unwrap();
		settings
			.core
			.databases
			.get_mut("default")
			.unwrap()
			.options
			.extend(
				database
					.query_pairs()
					.map(|(key, value)| (key.to_string(), value.to_string())),
			);
		settings.node.node_id = f.config.node_id.clone();
		settings.node.endpoint = f.config.endpoint.clone();
		settings.node.api_token = f.config.api_token.clone();
		settings.node.nats_url = f.config.nats_url.clone();
		settings.node.web_dir = f.config.web_dir.clone();
		settings.node.background_enabled = true;
		settings.node.worker_count = worker_count;
		settings.dashboard.oidc = f.config.oidc.clone();
		let destination = directory.join(format!("settings-{}", Uuid::new_v4()));
		std::fs::create_dir_all(&destination).unwrap();
		std::fs::write(destination.join("base.toml"), process_settings(&settings)).unwrap();
		let listen = reqwest::Url::parse(&f.config.endpoint)
			.unwrap()
			.socket_addrs(|| None)
			.unwrap();
		assert_eq!(listen.len(), 1, "fixture must advertise one local listener");
		vec![
			// Multi-process fixtures need bounded executors even on high-core hosts.
			("TOKIO_WORKER_THREADS", "2".into()),
			(
				"REINHARDT_SETTINGS_DIR",
				destination.to_string_lossy().into_owned(),
			),
			("REINHARDT_ENV", "container".into()),
			("AIDASH_LISTEN", listen[0].to_string()),
		]
	}
	.boxed()
}

struct ActivationResources {
	directory: ActivationDirectoryFuture,
	process_serial: futures_util::future::BoxFuture<'static, tokio::sync::MutexGuard<'static, ()>>,
}
#[rstest::fixture]
fn activation_resources(
	#[default("main")] label: &str,
	#[from(activation_runtime)] runtime: common::RuntimeFuture,
	#[from(activation_directory)]
	#[with(label, runtime.clone())]
	directory: ActivationDirectoryFuture,
	process_serial: futures_util::future::BoxFuture<'static, tokio::sync::MutexGuard<'static, ()>>,
) -> ActivationResources {
	let _ = (label, runtime);
	ActivationResources {
		directory,
		process_serial,
	}
}

#[derive(Clone)]
struct DeadlineState {
	calls: Arc<AtomicUsize>,
	blocked: Arc<std::sync::atomic::AtomicBool>,
	permits: Arc<tokio::sync::Semaphore>,
}
#[rstest::fixture]
fn deadline_state(
	#[from(upstream_fixtures::hits)] calls: Arc<AtomicUsize>,
	#[from(upstream_fixtures::available)] blocked: Arc<std::sync::atomic::AtomicBool>,
	permits: Arc<tokio::sync::Semaphore>,
) -> DeadlineState {
	DeadlineState {
		calls,
		blocked,
		permits,
	}
}
