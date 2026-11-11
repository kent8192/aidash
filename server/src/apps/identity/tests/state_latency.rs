//! Measurement only (#849): `GET /api/state` cost for a Tenant with
//! 665 Workspaces and 670 Runs. Not a regression test; run it on a release build:
//!
//! `cargo test --release --config 'profile.release.package.aidash-server.debug-assertions=true' \
//!   -p aidash-server --test state_latency -- --ignored --nocapture`
//!
//! The fixtures' direct database pool exists only with debug assertions, so the
//! server crate (alone) needs them; its dependencies stay plain release.
//! Set `AIDASH_STATE_MEASURE_FIELDS_ONLY` to stop after the first measured read.
//! Set `AIDASH_STATE_MEASURE_SQL` to print the recorded statements in order.
//! Statement counts come from the thread-local SQLx tracing recorder; field sizes
//! are compact JSON values after parsing and reserializing the response.
#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
use serde_json::{Value, json};
use std::{
	collections::BTreeMap,
	time::{Duration, Instant},
};
use uuid::Uuid;
#[path = "../../execution/tests/support/legacy.rs"]
mod common;
#[path = "../../execution/tests/support/statements.rs"]
mod statements;

const WORKSPACES: i64 = 665;
const RUNS: i64 = 670;
const WARM_UP: usize = 5;
const TIMED: usize = 30;

// Group object keys across Runs (and, recursively, their array elements). The
// byte counts are value-only compact JSON, so children exclude keys and framing.
fn breakdown(path: &str, values: &[&Value], depth: usize) {
	if depth > 10 {
		return;
	}
	let mut groups: BTreeMap<&str, Vec<&Value>> = BTreeMap::new();
	let mut objects = Vec::new();
	for value in values {
		match value {
			Value::Object(_) => objects.push(*value),
			Value::Array(items) => {
				println!(
					"RUN_ARRAY {path} items={} bytes={}",
					items.len(),
					serde_json::to_vec(value).unwrap().len()
				);
				objects.extend(items.iter());
			}
			_ => {}
		}
	}
	for object in objects {
		if let Value::Object(fields) = object {
			for (key, value) in fields {
				groups.entry(key).or_default().push(value);
			}
		}
	}
	let mut totals: Vec<_> = groups
		.into_iter()
		.map(|(key, items)| {
			let bytes: usize = items
				.iter()
				.map(|value| serde_json::to_vec(value).unwrap().len())
				.sum();
			(key, bytes, items)
		})
		.collect();
	totals.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
	for (key, bytes, items) in &totals {
		println!(
			"RUN_FIELD {path}.{key} bytes={bytes} occurrences={}",
			items.len()
		);
	}
	for (key, bytes, items) in totals.into_iter().take(3) {
		if bytes >= 1_000 {
			breakdown(&format!("{path}.{key}"), &items, depth + 1);
		}
	}
}

fn report_runs(state: &Value) {
	let runs = state["runs"].as_array().expect("runs array");
	assert_eq!(runs.len(), 50);
	let mut sizes: Vec<_> = runs
		.iter()
		.map(|run| (serde_json::to_vec(run).unwrap().len(), run))
		.collect();
	sizes.sort_by_key(|(bytes, _)| *bytes);
	let median = (sizes[24].0 + sizes[25].0) as f64 / 2.0;
	println!(
		"RUN_SIZES n={} min={} median={median} max={}",
		sizes.len(),
		sizes[0].0,
		sizes[49].0
	);
	breakdown("runs", &runs.iter().collect::<Vec<_>>(), 0);
	breakdown("largest_run", &[sizes[49].1], 0);
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
	// Nearest-rank percentile.
	let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
	sorted[rank.clamp(1, sorted.len()) - 1]
}

fn summary(label: &str, mut samples: Vec<Duration>) {
	samples.sort();
	let ms = |d: Duration| d.as_secs_f64() * 1e3;
	println!(
		"LATENCY {label} n={} p50_ms={:.1} p95_ms={:.1} max_ms={:.1} min_ms={:.1} mean_ms={:.1}",
		samples.len(),
		ms(percentile(&samples, 50.0)),
		ms(percentile(&samples, 95.0)),
		ms(*samples.last().unwrap()),
		ms(samples[0]),
		samples.iter().map(|d| ms(*d)).sum::<f64>() / samples.len() as f64,
	);
}

#[rstest::rstest]
#[tokio::test]
#[ignore = "measurement only: run on a release build with --ignored --nocapture"]
async fn state_latency_with_665_workspaces_and_670_runs(
	#[future(awt)]
	#[from(common::native_application)]
	// Seeding sends ~2,000 requests from one subject; lift its request budget.
	#[with(aidash_server::http::Settings { actor_burst: 100_000, ..Default::default() })]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	let db = f.store.pool.fixture_driver();
	let (_, token, first_task) = common::bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let seeding = Instant::now();
	let claim = |task: String| {
		let (app, token) = (&app, &token);
		async move {
			let (status, run) = common::request(
				app,
				token,
				"POST",
				&format!("/api/tasks/{task}/claim"),
				json!({"revision":0,"agent":{"id":"research","version":"1.0.0"}}),
			)
			.await;
			assert_eq!(status, 200, "{run}");
		}
	};
	claim(first_task.to_string()).await;
	// A few Conversations first; they may add Workspaces or Runs of their own.
	for index in 0..3 {
		let (status, body) = common::request(
			&app,
			&token,
			"POST",
			"/api/conversations",
			json!({"title":format!("Conversation {index}"),"goal":"Measure","target":{"id":"research","version":"1.0.0"},"target_kind":"agent"}),
		)
		.await;
		assert_eq!(status, 200, "{body}");
	}
	let count = |table: &'static str| async move {
		sqlx::query_scalar::<_, i64>(&format!(
			"SELECT COUNT(*) FROM {table} WHERE tenant = 'acme'"
		))
		.fetch_one(db)
		.await
		.unwrap()
	};
	let mut last_workspace = String::new();
	while count("workspaces").await < WORKSPACES {
		let (status, workspace) = common::request(
			&app,
			&token,
			"POST",
			"/api/workspaces",
			json!({"title":"Measured", "goal":"Bounded state"}),
		)
		.await;
		assert_eq!(status, 200, "{workspace}");
		last_workspace = workspace["id"].as_str().unwrap().to_owned();
		let (status, task) = common::request(
			&app,
			&token,
			"POST",
			&format!("/api/workspaces/{last_workspace}/tasks"),
			json!({"title":"Measured task", "description":"Create a run"}),
		)
		.await;
		assert_eq!(status, 200, "{task}");
		claim(task["id"].as_str().unwrap().to_owned()).await;
	}
	while count("runs").await < RUNS {
		let (status, task) = common::request(
			&app,
			&token,
			"POST",
			&format!("/api/workspaces/{last_workspace}/tasks"),
			json!({"title":"Extra task", "description":"Create a run"}),
		)
		.await;
		assert_eq!(status, 200, "{task}");
		claim(task["id"].as_str().unwrap().to_owned()).await;
	}
	// Five pending Human Requests on the oldest Runs.
	let waiting: Vec<(Uuid, Uuid)> = sqlx::query_as(
		"SELECT id, workspace_id FROM runs WHERE tenant = 'acme' ORDER BY created_at LIMIT 5",
	)
	.fetch_all(db)
	.await
	.unwrap();
	for (index, (run, workspace)) in waiting.iter().enumerate() {
		let request_id = Uuid::new_v4();
		sqlx::query("INSERT INTO human_requests (id, tenant, workspace_id, run_id, kind, prompt, request_key, created_at) VALUES ($1, 'acme', $2, $3, 'QUESTION', 'Still waiting?', $4, now())")
			.bind(request_id).bind(workspace).bind(run).bind(format!("measure-{index}")).execute(db).await.unwrap();
		let state =
			aidash_domain::RunState::Waiting(Box::new(aidash_domain::WaitingState::Human {
				request_id,
				answer_by: None,
				resume: aidash_domain::ResumeState::Ready(aidash_domain::ReadyState {}),
			}));
		let pending = aidash_domain::run_state::encode(&state, &Default::default()).unwrap();
		sqlx::query("UPDATE runs SET phase = 'WAITING', pending = $1 WHERE id = $2")
			.bind(pending)
			.bind(run)
			.execute(db)
			.await
			.unwrap();
	}
	// Artifacts on the newest Tasks.
	let tasks: Vec<(Uuid, Uuid)> = sqlx::query_as(
		"SELECT id, workspace_id FROM tasks WHERE tenant = 'acme' ORDER BY created_at DESC LIMIT 20",
	)
	.fetch_all(db)
	.await
	.unwrap();
	for (task, workspace) in &tasks {
		sqlx::query("INSERT INTO artifacts (id,tenant,workspace_id,task_id,kind,name,content,created_by,idempotency_key) VALUES ($1,'acme',$2,$3,'json','Measured','{}','alice',$1::text)")
			.bind(Uuid::new_v4()).bind(workspace).bind(task).execute(db).await.unwrap();
	}
	let mut shape = vec![];
	for table in [
		"workspaces",
		"tasks",
		"runs",
		"human_requests",
		"artifacts",
		"conversations",
		"events",
		"authorization_decisions",
	] {
		shape.push(format!("{table}={}", count(table).await));
	}
	println!(
		"SEED seconds={:.1} {}",
		seeding.elapsed().as_secs_f64(),
		shape.join(" ")
	);

	let ((status, state), statements) = statements::recorded(common::request(
		&app,
		&token,
		"GET",
		"/api/state",
		Value::Null,
	))
	.await;
	println!("STATEMENTS {}", statements.len());
	if std::env::var_os("AIDASH_STATE_MEASURE_SQL").is_some() {
		for (index, statement) in statements.iter().enumerate() {
			println!("SQL_TRACE {index} {statement}");
		}
	}
	let mut statement_types = BTreeMap::new();
	for statement in &statements {
		let summary = statement.split("\" \"").next().unwrap_or(statement);
		*statement_types.entry(summary).or_insert(0_usize) += 1;
	}
	for (summary, count) in statement_types {
		println!("STATEMENT_TYPE {count} {summary}");
	}
	assert_eq!(status, 200, "{state}");
	for (field, value) in state.as_object().expect("state object") {
		println!(
			"FIELD_BYTES {field} {}",
			serde_json::to_vec(value).unwrap().len()
		);
	}
	println!("BODY_BYTES {}", serde_json::to_vec(&state).unwrap().len());
	report_runs(&state);
	assert!(
		!statements.is_empty(),
		"SQL statement recorder saw no statements"
	);
	if std::env::var_os("AIDASH_STATE_MEASURE_FIELDS_ONLY").is_some() {
		common::cleanup(f, &url, &schema).await;
		return;
	}
	let node = state["workspaces"][0]["node"].as_str().unwrap().to_owned();
	let sizes = |state: &Value| {
		[
			"workspaces",
			"tasks",
			"runs",
			"artifacts",
			"human_requests",
			"conversations",
			"events",
		]
		.map(|field| format!("{field}={}", state[field].as_array().map_or(0, Vec::len)))
		.join(" ")
	};
	println!("RESPONSE {}", sizes(&state));
	for (label, path) in [
		("tenant", "/api/state".to_owned()),
		("node", format!("/api/state?node={node}")),
	] {
		for _ in 0..WARM_UP {
			eprintln!("PROBE664_PHASE warm {label}");
			let (status, body) = common::request(&app, &token, "GET", &path, Value::Null).await;
			assert_eq!(status, 200, "{body}");
		}
		let active_time = || async {
			// Let backends flush their statistics (at most once a second).
			tokio::time::sleep(Duration::from_millis(1500)).await;
			sqlx::query_scalar::<_, f64>(
				"SELECT active_time FROM pg_stat_database WHERE datname = current_database()",
			)
			.fetch_one(db)
			.await
			.unwrap()
		};
		let active_before = active_time().await;
		let mut samples = vec![];
		let mut written = vec![];
		let mut bytes = 0;
		for _ in 0..TIMED {
			let before: i64 = sqlx::query_scalar(
				"SELECT COALESCE(MAX(sequence), 0) FROM authorization_decisions",
			)
			.fetch_one(db)
			.await
			.unwrap();
			eprintln!("PROBE664_PHASE timed {label}");
			let started = Instant::now();
			let (status, body) = common::request(&app, &token, "GET", &path, Value::Null).await;
			samples.push(started.elapsed());
			assert_eq!(status, 200, "{body}");
			bytes = body.to_string().len();
			written.push(
				sqlx::query_scalar::<_, i64>(
					"SELECT COUNT(*) FROM authorization_decisions WHERE sequence > $1",
				)
				.bind(before)
				.fetch_one(db)
				.await
				.unwrap(),
			);
		}
		let active = active_time().await - active_before;
		println!(
			"DB_ACTIVE {label} active_ms_total={active:.1} active_ms_per_read={:.1}",
			active / TIMED as f64,
		);
		println!(
			"DECISION_ROWS {label} per_read={written:?} response_bytes={bytes} {}",
			sizes(
				&common::request(&app, &token, "GET", &path, Value::Null)
					.await
					.1
			)
		);
		summary(label, samples);
	}
	// Several dashboards of one Tenant at once: they queue on the per-Tenant
	// decision lock. Probe lines are unreliable here (shared counters).
	for concurrency in [4usize] {
		let mut samples = vec![];
		eprintln!("PROBE664_PHASE concurrent {concurrency}");
		for _ in 0..10 {
			let reads = (0..concurrency).map(|_| async {
				let started = Instant::now();
				let (status, body) =
					common::request(&app, &token, "GET", "/api/state", Value::Null).await;
				assert_eq!(status, 200, "{body}");
				started.elapsed()
			});
			samples.extend(futures_util::future::join_all(reads).await);
		}
		summary(&format!("tenant_concurrent_{concurrency}"), samples);
	}
	common::cleanup(f, &url, &schema).await;
}
