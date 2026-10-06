//! Deliberately opt-in, fixed-duration acceptance; failures retain raw evidence.
use super::*;
use reinhardt::query::Expr;
use sqlx::Executor;

async fn servers(
	fixture: &Fixture,
	dir: &Path,
	interval: u64,
	binary: &Path,
	candidate: bool,
) -> Vec<Process> {
	let mut processes = Vec::new();
	for n in 0..3 {
		let mut p = Process::start(fixture, dir, n, interval, binary);
		p.ready(fixture, candidate).await;
		processes.push(p);
	}
	processes
}
fn scope(index: usize, fixture: &Fixture) -> Option<Uuid> {
	// Nine filtered clients per Workspace, plus ten all-Workspace clients.
	(index < 90).then(|| fixture.workspaces[index % 10])
}
async fn observers(
	processes: &[Process],
	fixture: &Fixture,
) -> (Vec<RemoteStream>, mpsc::UnboundedReceiver<Received>) {
	let (send, receive) = mpsc::unbounded_channel();
	let mut streams = Vec::new();
	for n in 0..100 {
		streams.push(
			observe(
				&processes[n % 3],
				fixture,
				n,
				scope(n, fixture),
				-1,
				send.clone(),
			)
			.await,
		);
	}
	// Confirm every real HTTP body and independent notification subscriber.
	for (n, process) in processes.iter().enumerate() {
		assert_eq!(
			metric(&process.metrics().await, "aidash_sse_connections"),
			if n == 0 { 34.0 } else { 33.0 }
		);
	}
	(streams, receive)
}
async fn metrics_all(processes: &[Process]) -> Vec<String> {
	let snapshots = futures_util::future::join_all(processes.iter().map(Process::metrics)).await;
	assert_eq!(
		snapshots
			.iter()
			.map(|text| metric(text, "aidash_sse_connections"))
			.sum::<f64>(),
		100.0,
		"all observers must remain connected throughout each window"
	);
	snapshots
}
async fn prime(fixture: &Fixture, received: &mut mpsc::UnboundedReceiver<Received>) {
	for workspace in &fixture.workspaces {
		fixture
			.f
			.store
			.emit(
				Some(*workspace),
				"workspace.updated",
				json!({"id":workspace,"warmup":true}),
			)
			.await
			.unwrap();
	}
	let mut counts = [0_usize; 100];
	for observation in receive(received, 190, 10).await {
		assert_eq!(observation.data["data"]["warmup"], true);
		counts[observation.client] += 1;
	}
	for (index, count) in counts.into_iter().enumerate() {
		assert_eq!(count, if index < 90 { 1 } else { 10 });
	}
}
fn resources(processes: &[Process]) -> Value {
	let ids = processes
		.iter()
		.map(|p| p.child.id().to_string())
		.collect::<Vec<_>>()
		.join(",");
	let output = Command::new("ps")
		.args(["-p", &ids, "-o", "pid=,time=,%cpu=,rss="])
		.output()
		.unwrap();
	json!({"columns":["pid","cumulative_cpu_time","cpu_percent","rss_kib"],"ps":String::from_utf8(output.stdout).unwrap()})
}
async fn sql_counts(fixture: &Fixture) -> Value {
	let timings: Vec<(String, i64, f64, f64)> = sqlx::query_as(
		&Query::select()
			.columns(["query", "calls", "total_exec_time", "max_exec_time"].map(Alias::new))
			.from((Alias::new("public"), Alias::new("pg_stat_statements")))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(fixture.f.store.pool.driver())
	.await
	.unwrap();
	let rows: Vec<_> = timings
		.iter()
		.map(|(query, calls, _, _)| (query.clone(), *calls))
		.collect();
	let mut total = 0;
	let mut events = 0;
	let mut authority = 0;
	let mut visibility = 0;
	let mut outbox = 0;
	for (query, calls) in &rows {
		total += calls;
		if query.starts_with("SELECT") && query.contains("FROM \"events\"") {
			events += calls;
		}
		if query.contains("authorization_") {
			authority += calls;
		}
		if query.contains("FROM \"atomic_gate\"") {
			visibility += calls;
		}
		if query.starts_with("UPDATE \"events\"") {
			outbox += calls;
		}
	}
	json!({"all":total,"event_reads":events,"authority_statements":authority,"visibility_statements":visibility,"outbox_statements":outbox,"rows":rows,"statement_timings":timings})
}
fn delta(before: &Value, after: &Value, name: &str) -> i64 {
	after[name].as_i64().unwrap() - before[name].as_i64().unwrap()
}
fn quantile(values: &[f64], fraction: f64) -> f64 {
	values[((values.len() as f64 * fraction).ceil() as usize).saturating_sub(1)]
}

#[rstest::rstest]
#[tokio::test(flavor = "multi_thread", worker_threads = 12)]
#[ignore = "fixed 3x30s active + 6x60s idle; scripts/test-sse-delivery.sh --benchmark"]
async fn accepted_load_and_idle_reduction(#[future(awt)] test_environment: Arc<TestEnvironment>) {
	let baseline = PathBuf::from(
		std::env::var_os("AIDASH_SSE_BASELINE_BINARY")
			.expect("build the inspected polling baseline first"),
	);
	let candidate = candidate_binary();
	let mut summaries = Vec::new();
	// One dedicated Compose stack; no concurrent cases or other fixture writes.
	for repetition in 0..3 {
		let fixture = Fixture::new(
			&test_environment,
			Duration::from_secs(60),
			Duration::from_secs(30),
			128,
		)
		.await;
		let dir = directory(&format!("active-{repetition}"), &fixture.schema);
		// SeaQuery has no CREATE EXTENSION DDL builder. This is fixture-only instrumentation.
		fixture
			.f
			.store
			.pool
			.driver()
			.execute("CREATE EXTENSION IF NOT EXISTS pg_stat_statements WITH SCHEMA public")
			.await
			.unwrap();
		let mut processes = servers(&fixture, &dir, 60000, &candidate, true).await;
		let (streams, mut received) = observers(&processes, &fixture).await;
		prime(&fixture, &mut received).await;
		tokio::time::sleep(Duration::from_secs(1)).await;
		let before = metrics_all(&processes).await;
		let sql_before = sql_counts(&fixture).await;
		let resources_before = resources(&processes);
		let window = Instant::now();
		let client = reqwest::Client::new();
		let mut starts = Vec::new();
		let mut commit_responses = Vec::new();
		let mut resources_during = Vec::new();
		for sample in 0..300 {
			tokio::time::sleep_until(window + Duration::from_millis(sample * 100)).await;
			let ws = fixture.workspaces[sample as usize % 10];
			let mut barrier = fixture.f.store.pool.driver().begin().await.unwrap();
			sqlx::query(
				&Query::select()
					.expr(Expr::cust("PG_ADVISORY_XACT_LOCK(71003201)"))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut *barrier)
			.await
			.unwrap();
			let mutation = client
				.patch(format!("{}/api/workspaces/{ws}", processes[0].endpoint))
				.bearer_auth(&fixture.token)
				.json(&json!({"revision": sample / 10,"state":{"sample":sample}}));
			let operation = tokio::spawn(async move { mutation.send().await.unwrap() });
			// Wait until writer A is actually at its durable event/commit boundary.
			tokio::time::timeout(Duration::from_secs(3), async {
				loop {
					let waiting: i64 = sqlx::query_scalar(
						&Query::select()
							.expr(Expr::cust("COUNT(*)"))
							.from(Alias::new("pg_locks"))
							.inner_join(
								Alias::new("pg_stat_activity"),
								Expr::col((Alias::new("pg_locks"), Alias::new("pid")))
									.equals((Alias::new("pg_stat_activity"), Alias::new("pid"))),
							)
							.and_where(
								Expr::col(Alias::new("application_name")).eq("sse-process-0"),
							)
							.and_where(Expr::col(Alias::new("locktype")).eq("advisory"))
							.and_where(Expr::col(Alias::new("objid")).eq(71003201))
							.and_where(
								Expr::col(Alias::new("granted"))
									.eq(reinhardt::query::Expr::value(false)),
							)
							.to_string(PostgresQueryBuilder),
					)
					.fetch_one(fixture.f.store.pool.driver())
					.await
					.unwrap();
					if waiting > 0 {
						break;
					}
					tokio::time::sleep(Duration::from_millis(1)).await;
				}
			})
			.await
			.expect("mutation commit barrier");
			starts.push(Instant::now());
			barrier.commit().await.unwrap();
			let response = operation.await.unwrap();
			let status = response.status();
			let value: Value = response.json().await.unwrap();
			assert_eq!(status, 200, "{value}");
			commit_responses.push(Instant::now().duration_since(window).as_secs_f64());
			if sample % 50 == 0 {
				resources_during.push(resources(&processes));
			}
		}
		tokio::time::sleep_until(window + Duration::from_secs(30)).await;
		let observations = receive(&mut received, 300 * 19, 5).await;
		let after = metrics_all(&processes).await;
		let sql_after = sql_counts(&fixture).await;
		let mut raw = Vec::new();
		let mut latest = vec![0_f64; 300];
		let mut last_id = vec![0; 100];
		let mut seen = std::collections::HashSet::new();
		let mut correctness = true;
		for event in observations {
			let sample = event.data["data"]["state"]["sample"].as_u64().unwrap() as usize;
			let latency = event.at.duration_since(starts[sample]).as_secs_f64() * 1000.0;
			let expected_scope = scope(event.client, &fixture);
			correctness &=
				expected_scope.is_none() || expected_scope == Some(fixture.workspaces[sample % 10]);
			correctness &= event.id > last_id[event.client] && seen.insert((event.client, sample));
			last_id[event.client] = event.id;
			latest[sample] = latest[sample].max(latency);
			raw.push(json!({"sample":sample,"client":event.client,"replica":event.client%3,"sequence":event.id,"latency_ms":latency,"commit_release_seconds":starts[sample].duration_since(window).as_secs_f64(),"receipt_seconds":event.at.duration_since(window).as_secs_f64()}));
		}
		for sample in 0..300 {
			for n in 0..100 {
				if scope(n, &fixture).is_none()
					|| scope(n, &fixture) == Some(fixture.workspaces[sample % 10])
				{
					correctness &= seen.contains(&(n, sample));
				}
			}
		}
		correctness &= received.try_recv().is_err();
		correctness &= commit_responses.last().copied().unwrap() <= 30.1;
		latest.sort_by(f64::total_cmp);
		let clean = (0..3).all(|n| {
			cause(&before[n], "fallback") == cause(&after[n], "fallback")
				&& cause(&before[n], "initial") == cause(&after[n], "initial")
				&& cause(&before[n], "reconnect") == cause(&after[n], "reconnect")
				&& cause(&after[n], "notification") > cause(&before[n], "notification")
		});
		let p95 = quantile(&latest, 0.95);
		let p99 = quantile(&latest, 0.99);
		let summary = json!({"kind":"active","repetition":repetition,"p50_ms":quantile(&latest,0.5),"p95_ms":p95,"p99_ms":p99,"max_ms":latest.last(),"event_count":300,"receipt_count":raw.len(),"correctness":correctness,"notification_only":clean,"percentile":"nearest rank of complete fan-out (slowest expected client) per event","resources_before":resources_before,"resources_during":resources_during,"resources_after":resources(&processes),"sql_before":sql_before,"sql_after":sql_after,"metrics_before":before,"metrics_after":after,"commit_response_seconds":commit_responses});
		std::fs::write(
			dir.join("summary.json"),
			serde_json::to_vec_pretty(&summary).unwrap(),
		)
		.unwrap();
		std::fs::write(dir.join("receipts.json"), serde_json::to_vec(&raw).unwrap()).unwrap();
		eprintln!(
			"active repetition {repetition}: p95={p95:.2}ms p99={p99:.2}ms notification_only={clean}"
		);
		summaries.push(summary);
		drop(streams);
		for process in &mut processes {
			process.stop().await;
		}
		drop(processes);
		fixture.finish().await;
		assert!(
			correctness && clean && p95 <= 500.0 && p99 <= 1000.0,
			"active acceptance failed; retain this window, do not discard it"
		);
	}
	for repetition in 0..3 {
		let mut counts = Vec::new();
		for (label, binary, candidate) in [
			("baseline", baseline.as_path(), false),
			("candidate", candidate.as_path(), true),
		] {
			let fixture = Fixture::new(
				&test_environment,
				Duration::from_secs(5),
				Duration::from_secs(30),
				128,
			)
			.await;
			let dir = directory(&format!("idle-{label}-{repetition}"), &fixture.schema);
			let mut processes = servers(&fixture, &dir, 5000, binary, candidate).await;
			let (streams, mut received) = observers(&processes, &fixture).await;
			prime(&fixture, &mut received).await;
			tokio::time::sleep(Duration::from_secs(2)).await;
			let sql_before = sql_counts(&fixture).await;
			let before = metrics_all(&processes).await;
			let resources_before = resources(&processes);
			let began = Instant::now();
			let mut resources_during = Vec::new();
			for second in [10, 20, 30, 40, 50, 60] {
				tokio::time::sleep_until(began + Duration::from_secs(second)).await;
				resources_during.push(resources(&processes));
			}
			let sql_after = sql_counts(&fixture).await;
			let after = metrics_all(&processes).await;
			let reads = delta(&sql_before, &sql_after, "event_reads");
			assert!(received.try_recv().is_err(), "idle window mutated");
			let summary = json!({"kind":"idle","version":label,"repetition":repetition,"seconds":began.elapsed().as_secs_f64(),"event_queries":reads,"queries_per_connection_second": reads as f64/100.0/60.0,"total_sql":delta(&sql_before,&sql_after,"all"),"authority_sql":delta(&sql_before,&sql_after,"authority_statements"),"visibility_sql":delta(&sql_before,&sql_after,"visibility_statements"),"outbox_sql":delta(&sql_before,&sql_after,"outbox_statements"),"sql_before":sql_before,"sql_after":sql_after,"metrics_before":before,"metrics_after":after,"resources_before":resources_before,"resources_during":resources_during});
			std::fs::write(
				dir.join("summary.json"),
				serde_json::to_vec_pretty(&summary).unwrap(),
			)
			.unwrap();
			eprintln!("idle {label} repetition {repetition}: {reads} event queries");
			counts.push(reads);
			summaries.push(summary);
			drop(streams);
			for process in &mut processes {
				process.stop().await;
			}
			drop(processes);
			fixture.finish().await;
		}
		let reduction = 1.0 - counts[1] as f64 / counts[0] as f64;
		eprintln!(
			"idle repetition {repetition}: reduction={:.2}%",
			reduction * 100.0
		);
		assert!(reduction >= 0.90, "idle query reduction acceptance");
	}
	let dir = directory("acceptance", "summary");
	std::fs::write(
		dir.join("results.json"),
		serde_json::to_vec_pretty(&summaries).unwrap(),
	)
	.unwrap();
}

use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};
