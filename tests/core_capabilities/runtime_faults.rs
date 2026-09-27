use super::*;
#[rstest::rstest]
#[tokio::test]
async fn sandbox_cpu_and_memory_limits_are_enforced_under_load(
	#[future] runtime_fixture: CoreFixture,
) {
	let c = Box::pin(runtime_fixture).await;
	let run = admit(&c).await;
	let (stop, rx) = tokio::sync::watch::channel(false);
	let worker = tokio::spawn(aidash::capabilities::operations::run(c.f.store.clone(), rx));
	let path = format!("/api/runs/{}/shell", run.id);
	let command = "python - <<'PY'\nimport os,time,resource,json\nstart=time.monotonic()\npids=[]\nfor i in range(4):\n pid=os.fork()\n if pid==0:\n  while time.monotonic()-start<4: pass\n  os._exit(0)\n pids.append(pid)\nfor pid in pids: os.waitpid(pid,0)\nu=resource.getrusage(resource.RUSAGE_CHILDREN)\ncpu=u.ru_utime+u.ru_stime\nwall=time.monotonic()-start\nassert cpu>0.2 and cpu<=2*wall+1,(cpu,wall)\nprint(json.dumps({'cpu_seconds':cpu,'wall_seconds':wall,'ceiling_cpus':2}))\nPY";
	let (status,op)=request(&c.app,&c.token,"POST",&path,json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"timeout_seconds":30,"command":command})).await;
	assert_eq!(status, 200, "{op}");
	let cpu = operation_until(&c, run.id, "shell", &op["operation_id"], &["completed"]).await;
	assert!(cpu["output"].as_str().unwrap().contains("cpu_seconds"));
	eprintln!("measured CPU limit: {}", cpu["output"]);
	let command = "python - <<'PY'\nfrom pathlib import Path\nPath('before-memory-limit.txt').write_text('saved')\nchunks=[]\ntry:\n for i in range(80):\n  block=bytearray(64*1024*1024)\n  block[::4096]=bytes([1])*(len(block)//4096)\n  chunks.append(block)\nexcept MemoryError:\n raise SystemExit(17)\nraise RuntimeError('memory ceiling failed')\nPY";
	let (status,op)=request(&c.app,&c.token,"POST",&path,json!({"idempotency_key":Uuid::new_v4(),"expected_revision":cpu["revision"],"timeout_seconds":30,"command":command})).await;
	assert_eq!(status, 200, "{op}");
	let memory = operation_until(&c, run.id, "shell", &op["operation_id"], &["failed"]).await;
	assert_eq!(memory["termination_confirmed"], true, "{memory}");
	assert_ne!(memory["exit_code"], 0);
	assert!(
		!memory["output"]
			.as_str()
			.unwrap()
			.contains("memory ceiling failed"),
		"memory overcommit must fail before the sentinel"
	);
	let (status,op)=request(&c.app,&c.token,"POST",&path,json!({"idempotency_key":Uuid::new_v4(),"expected_revision":memory["revision"],"command":"test \"$(cat before-memory-limit.txt)\" = saved && printf recovered"})).await;
	assert_eq!(status, 200, "{op}");
	let recovered = operation_until(&c, run.id, "shell", &op["operation_id"], &["completed"]).await;
	assert_eq!(recovered["output"], "recovered");
	stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn idle_heap_is_physically_stopped_without_deleting_saved_files(
	#[future] runtime_fixture: CoreFixture,
) {
	use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
	let c = Box::pin(runtime_fixture).await;
	let run = admit(&c).await;
	let (stop, rx) = tokio::sync::watch::channel(false);
	let worker = tokio::spawn(aidash::capabilities::operations::run(c.f.store.clone(), rx));
	let path = format!("/api/runs/{}/python", run.id);
	let (status,op)=request(&c.app,&c.token,"POST",&path,json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"code":"private_value=91\nfrom pathlib import Path\nPath('kept.txt').write_text('kept')"})).await;
	assert_eq!(status, 200, "{op}");
	let first = operation_until(&c, run.id, "python", &op["operation_id"], &["completed"]).await;
	// Advance the persisted idle clock, without sleeping for the production 30 minutes.
	sqlx::query(
		&Query::update()
			.table(Alias::new("core_records"))
			.value(
				Alias::new("data"),
				Expr::cust("jsonb_set(data, '{last_used}', to_jsonb($1::timestamptz))"),
			)
			.and_where(Expr::col(Alias::new("kind")).eq("python_session"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(chrono::Utc::now() - chrono::Duration::minutes(31))
	.execute(&c.f.store.pool)
	.await
	.unwrap();
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
	loop {
		let state: String = sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("state"))
				.from(Alias::new("core_records"))
				.and_where(Expr::col(Alias::new("kind")).eq("python_session"))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(&c.f.store.pool)
		.await
		.unwrap();
		if state == "reset" {
			break;
		}
		assert!(
			tokio::time::Instant::now() < deadline,
			"idle process was not stopped: {state}"
		);
		tokio::time::sleep(std::time::Duration::from_millis(100)).await;
	}
	let (status,reset)=request(&c.app,&c.token,"POST",&path,json!({"idempotency_key":Uuid::new_v4(),"expected_revision":first["revision"],"expected_session_id":first["session_id"],"code":"raise RuntimeError('stale code executed')"})).await;
	assert_eq!(status, 200, "{reset}");
	assert_eq!(reset["error"]["code"], "SESSION_RESET");
	assert_eq!(reset["reset_reason"], "idle_timeout");
	let (status,next)=request(&c.app,&c.token,"POST",&path,json!({"idempotency_key":Uuid::new_v4(),"expected_revision":first["revision"],"expected_session_id":reset["session_id"],"code":"from pathlib import Path\nassert 'private_value' not in globals()\nprint(Path('kept.txt').read_text())"})).await;
	assert_eq!(status, 200, "{next}");
	let done = operation_until(&c, run.id, "python", &next["operation_id"], &["completed"]).await;
	assert!(done["output"].as_str().unwrap().contains("kept"));
	stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	c.close().await;
}
#[rstest::rstest]
#[tokio::test]
async fn python_timeout_exports_prior_files_and_requires_new_memory_ack(
	#[future] runtime_fixture: CoreFixture,
) {
	let c = Box::pin(runtime_fixture).await;
	let run = admit(&c).await;
	let (stop, rx) = tokio::sync::watch::channel(false);
	let worker = tokio::spawn(aidash::capabilities::operations::run(c.f.store.clone(), rx));
	let input = json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"timeout_seconds":5,"code":"from pathlib import Path\nimport time\nsecret_counter=53\nPath('before-timeout.txt').write_text('saved before timeout 東京')\ntime.sleep(40)"});
	let path = format!("/api/runs/{}/python", run.id);
	let (status, operation) = request(&c.app, &c.token, "POST", &path, input).await;
	assert_eq!(status, 200, "{operation}");
	let result = operation_until(
		&c,
		run.id,
		"python",
		&operation["operation_id"],
		&["failed"],
	)
	.await;
	assert_eq!(result["termination_confirmed"], true, "{result}");
	assert_eq!(result["writer_frozen"], false);
	assert_eq!(result["effects_may_have_occurred"], true);
	let (_, area) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/runs/{}/working-area", run.id),
		Value::Null,
	)
	.await;
	let file = area["manifest"]
		.as_array()
		.unwrap()
		.iter()
		.find(|f| f["path"] == "before-timeout.txt")
		.expect("files written before timeout must remain visible");
	let (status, content) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/runs/{}/files/read", run.id),
		json!({"file_id":file["file_id"],"representation":"text"}),
	)
	.await;
	assert_eq!(status, 200, "{content}");
	assert_eq!(content["content"], "saved before timeout 東京");
	let (status,reset)=request(&c.app,&c.token,"POST",&path,json!({"idempotency_key":Uuid::new_v4(),"expected_revision":area["revision"],"expected_session_id":result["session_id"],"code":"Path('MUST_NOT_RUN').write_text('bad')"})).await;
	assert_eq!(status, 200, "{reset}");
	assert_eq!(reset["session_reset"], true);
	assert_eq!(reset["status"], "blocked");
	assert_ne!(reset["session_id"], result["session_id"]);
	let (status,fresh)=request(&c.app,&c.token,"POST",&path,json!({"idempotency_key":Uuid::new_v4(),"expected_revision":area["revision"],"expected_session_id":reset["session_id"],"code":"from pathlib import Path\nassert not Path('MUST_NOT_RUN').exists()\nassert 'secret_counter' not in globals()\nprint(Path('before-timeout.txt').read_text())"})).await;
	assert_eq!(status, 200, "{fresh}");
	let result =
		operation_until(&c, run.id, "python", &fresh["operation_id"], &["completed"]).await;
	assert!(
		result["output"]
			.as_str()
			.unwrap()
			.contains("saved before timeout 東京")
	);
	stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	c.close().await;
}
#[rstest::rstest]
#[tokio::test]
async fn shell_output_saturation_is_bounded_and_control_stays_usable(
	#[future] runtime_fixture: CoreFixture,
) {
	let c = Box::pin(runtime_fixture).await;
	let run = admit(&c).await;
	let (stop, rx) = tokio::sync::watch::channel(false);
	let worker = tokio::spawn(aidash::capabilities::operations::run(c.f.store.clone(), rx));
	let (status,operation)=request(&c.app,&c.token,"POST",&format!("/api/runs/{}/shell",run.id),json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"command":"python -c 'import sys,time; sys.stdout.write(\"x\"*(10*1024*1024)); sys.stdout.flush(); time.sleep(30)'"})).await;
	assert_eq!(status, 200, "{operation}");
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(45);
	loop {
		let (status, result) = request(
			&c.app,
			&c.token,
			"POST",
			&format!("/api/runs/{}/shell/poll", run.id),
			json!({"operation_id":operation["operation_id"]}),
		)
		.await;
		assert_eq!(status, 200, "{result}");
		assert!(result["output"].as_str().unwrap().len() <= 16384);
		if result["truncated"] == true {
			break;
		}
		assert!(
			tokio::time::Instant::now() < deadline,
			"output never reached the bounded capture: {result}"
		);
		tokio::time::sleep(std::time::Duration::from_millis(200)).await;
	}
	let (status, area) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/runs/{}/working-area", run.id),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{area}");
	let (status,queue)=request(&c.app,&c.token,"POST",&format!("/api/working-areas/{}/queue",area["id"].as_str().unwrap()),json!({"idempotency_key":Uuid::new_v4(),"agent_version":"1.1.0","title":"Next turn","description":"Controls remain available while stdout floods"})).await;
	assert_eq!(status, 200, "{queue}");
	assert_eq!(queue["queue"].as_array().unwrap().len(), 2);
	let (status, result) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/runs/{}/shell/cancel", run.id),
		json!({"operation_id":operation["operation_id"]}),
	)
	.await;
	assert_eq!(status, 200, "{result}");
	let result = operation_until(
		&c,
		run.id,
		"shell",
		&operation["operation_id"],
		&["cancelled"],
	)
	.await;
	assert_eq!(result["termination_confirmed"], true);
	assert_eq!(result["truncated"], true);
	assert!(result["output"].as_str().unwrap().len() <= 16384);
	use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
	let captured: Vec<i64> = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("size"))
			.from(Alias::new("core_objects"))
			.and_where(Expr::col(Alias::new("kind")).eq("output"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&c.f.store.pool)
	.await
	.unwrap();
	assert_eq!(captured, vec![8 << 20]);
	stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn disabled_admission_stops_live_work_and_keeps_files_for_explicit_restart(
	#[future] runtime_fixture: CoreFixture,
) {
	use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
	let mut c = Box::pin(runtime_fixture).await;
	let run = admit(&c).await;
	let (stop, rx) = tokio::sync::watch::channel(false);
	let worker = tokio::spawn(aidash::capabilities::operations::run(c.f.store.clone(), rx));
	let path = format!("/api/runs/{}/python", run.id);
	let (status, op) = request(&c.app, &c.token, "POST", &path, json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"code":"from pathlib import Path\nprivate_heap=43\nPath('saved.txt').write_text('preserved 東京')"})).await;
	assert_eq!(status, 200, "{op}");
	let first = operation_until(&c, run.id, "python", &op["operation_id"], &["completed"]).await;
	stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	// Apply the supported rollback contract: restart services with admission off,
	// preserving the same database, object store and isolated runner identity.
	let enabled = (*c.f.store.capabilities.0).clone();
	let mut disabled = enabled.clone();
	disabled.admission = false;
	c.f.store.capabilities = Runtime::new(disabled).unwrap();
	c.app = aidash::api::router(c.f.clone());
	let (stop, rx) = tokio::sync::watch::channel(false);
	let worker = tokio::spawn(aidash::capabilities::operations::run(c.f.store.clone(), rx));
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
	loop {
		let state: String = sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("state"))
				.from(Alias::new("core_records"))
				.and_where(Expr::col(Alias::new("kind")).eq("python_session"))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(&c.f.store.pool)
		.await
		.unwrap();
		if state == "reset" {
			break;
		}
		assert!(
			tokio::time::Instant::now() < deadline,
			"heap not stopped: {state}"
		);
		tokio::time::sleep(std::time::Duration::from_millis(100)).await;
	}
	let (status, denied) = request(&c.app, &c.token, "POST", &path, json!({"idempotency_key":Uuid::new_v4(),"expected_revision":first["revision"],"code":"raise RuntimeError('disabled work ran')"})).await;
	assert_eq!(status, 409, "{denied}");
	assert_eq!(denied["error"]["code"], "CAPABILITIES_DISABLED");
	let (status, inventory) =
		request(&c.app, &c.token, "GET", "/api/working-files", Value::Null).await;
	assert_eq!(status, 200, "{inventory}");
	assert_eq!(inventory["items"][0]["files"], 1);
	stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	c.f.store.capabilities = Runtime::new(enabled).unwrap();
	c.app = aidash::api::router(c.f.clone());
	let (stop, rx) = tokio::sync::watch::channel(false);
	let worker = tokio::spawn(aidash::capabilities::operations::run(c.f.store.clone(), rx));
	let (status,reset)=request(&c.app,&c.token,"POST",&path,json!({"idempotency_key":Uuid::new_v4(),"expected_revision":first["revision"],"expected_session_id":first["session_id"],"code":"raise RuntimeError('stale code ran')"})).await;
	assert_eq!(status, 200, "{reset}");
	assert_eq!(reset["error"]["code"], "SESSION_RESET");
	assert_eq!(reset["reset_reason"], "admission_disabled");
	let (status,next)=request(&c.app,&c.token,"POST",&path,json!({"idempotency_key":Uuid::new_v4(),"expected_revision":first["revision"],"expected_session_id":reset["session_id"],"code":"from pathlib import Path\nassert 'private_heap' not in globals()\nprint(Path('saved.txt').read_text())"})).await;
	assert_eq!(status, 200, "{next}");
	let done = operation_until(&c, run.id, "python", &next["operation_id"], &["completed"]).await;
	assert!(done["output"].as_str().unwrap().contains("preserved 東京"));
	stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn profile_drift_still_cancels_an_active_operation(#[future] runtime_fixture: CoreFixture) {
	let mut c = Box::pin(runtime_fixture).await;
	let run = admit(&c).await;
	let (stop, rx) = tokio::sync::watch::channel(false);
	let worker = tokio::spawn(aidash::capabilities::operations::run(c.f.store.clone(), rx));
	let (status, operation) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/runs/{}/shell", run.id),
		json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"command":"printf started; sleep 30"}),
	)
	.await;
	assert_eq!(status, 200, "{operation}");
	operation_until(
		&c,
		run.id,
		"shell",
		&operation["operation_id"],
		&["running"],
	)
	.await;
	stop.send(true).unwrap();
	worker.await.unwrap().unwrap();

	let mut disabled = (*c.f.store.capabilities.0).clone();
	disabled.admission = false;
	disabled.working_bytes -= 1;
	c.f.store.capabilities = Runtime::new(disabled).unwrap();
	c.app = aidash::api::router(c.f.clone());
	let (stop, rx) = tokio::sync::watch::channel(false);
	let worker = tokio::spawn(aidash::capabilities::operations::run(c.f.store.clone(), rx));
	let cancelled = operation_until(
		&c,
		run.id,
		"shell",
		&operation["operation_id"],
		&["cancelled"],
	)
	.await;
	assert_eq!(cancelled["termination_confirmed"], true, "{cancelled}");
	stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn cleanup_waits_for_proven_shell_stop_and_fences_delayed_writes(
	#[future] runtime_fixture: CoreFixture,
) {
	let c = Box::pin(runtime_fixture).await;
	let run = admit(&c).await;
	let (stop, rx) = tokio::sync::watch::channel(false);
	let worker = tokio::spawn(aidash::capabilities::operations::run(c.f.store.clone(), rx));
	let (status,op)=request(&c.app,&c.token,"POST",&format!("/api/runs/{}/shell",run.id),json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"command":"printf saved > live.txt; printf ready; sleep 30; printf forbidden > late.txt"})).await;
	assert_eq!(status, 200, "{op}");
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(40);
	loop {
		let running = operation_until(&c, run.id, "shell", &op["operation_id"], &["running"]).await;
		if running["output"].as_str().unwrap().contains("ready") {
			break;
		}
		assert!(
			tokio::time::Instant::now() < deadline,
			"Shell did not create its initial file"
		);
		tokio::time::sleep(std::time::Duration::from_millis(100)).await;
	}
	let cleanup = format!(
		"/api/working-areas/{}/cleanup",
		op["area_id"].as_str().unwrap()
	);
	let (status, blocked) = request(
		&c.app,
		&c.token,
		"POST",
		&cleanup,
		json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"choice":"recoverable"}),
	)
	.await;
	assert_eq!(status, 409, "{blocked}");
	assert_eq!(blocked["error"]["code"], "AREA_BUSY");
	let (status, cancel) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/runs/{}/shell/cancel", run.id),
		json!({"operation_id":op["operation_id"]}),
	)
	.await;
	assert_eq!(status, 200, "{cancel}");
	let cancelled = operation_until(&c, run.id, "shell", &op["operation_id"], &["cancelled"]).await;
	assert_eq!(cancelled["termination_confirmed"], true);
	let (_, area) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/runs/{}/working-area", run.id),
		Value::Null,
	)
	.await;
	assert_eq!(
		area["manifest"].as_array().unwrap().len(),
		1,
		"no delayed write: {area}"
	);
	assert_eq!(area["manifest"][0]["path"], "live.txt");
	let (status,cleaned)=request(&c.app,&c.token,"POST",&cleanup,json!({"idempotency_key":Uuid::new_v4(),"expected_revision":area["revision"],"choice":"recoverable"})).await;
	assert_eq!(status, 200, "{cleaned}");
	let (status,stale)=request(&c.app,&c.token,"POST",&format!("/api/runs/{}/patch",run.id),json!({"idempotency_key":Uuid::new_v4(),"expected_revision":area["revision"],"preconditions":{"late.txt":null},"patch":"*** Begin Patch\n*** Add File: late.txt\n+must not recreate deleted generation\n*** End Patch"})).await;
	assert!(status >= 400, "stale generation must not write: {stale}");
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
	loop {
		let (status, result) = request(
			&c.app,
			&c.token,
			"GET",
			&format!(
				"/api/file-cleanups/{}",
				cleaned["operation_id"].as_str().unwrap()
			),
			Value::Null,
		)
		.await;
		assert_eq!(status, 200, "{result}");
		if result["state"] == "recoverable" {
			break;
		}
		assert!(tokio::time::Instant::now() < deadline, "{result}");
		tokio::time::sleep(std::time::Duration::from_millis(100)).await;
	}
	stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	c.close().await;
}
