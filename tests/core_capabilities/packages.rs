use super::*;
#[rstest::rstest]
#[tokio::test]
async fn approved_wheel_installs_offline_with_hashes_and_explicit_memory_reset(
	#[future] runtime_fixture: CoreFixture,
) {
	use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
	let mut c = Box::pin(runtime_fixture).await;
	let mut profile = (*c.f.store.capabilities.0).clone();
	profile.outbound_origins = vec!["https://files.pythonhosted.org".into()];
	profile.package_origins = profile.outbound_origins.clone();
	c.f.store.capabilities = Runtime::new(profile).unwrap();
	c.app = aidash::api::router(c.f.clone());
	let run = admit(&c).await;
	let (stop, rx) = tokio::sync::watch::channel(false);
	let worker = tokio::spawn(aidash::capabilities::operations::run(c.f.store.clone(), rx));
	let python = format!("/api/runs/{}/python", run.id);
	let (status,first)=request(&c.app,&c.token,"POST",&python,json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"code":"import importlib.util\nassert importlib.util.find_spec('itsdangerous') is None\nold_memory=913\nprint('base verified')"})).await;
	assert_eq!(status, 200, "{first}");
	let first = operation_until(&c, run.id, "python", &first["operation_id"], &["completed"]).await;
	let wheel: Value = serde_json::from_str(include_str!("../fixtures/python-wheel.json")).unwrap();
	let outbound = format!("/api/runs/{}/outbound", run.id);
	let fetch = json!({"idempotency_key":Uuid::new_v4(),"url":wheel["url"]});
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(40);
	let fetched = loop {
		let (status, value) = request(&c.app, &c.token, "POST", &outbound, fetch.clone()).await;
		assert_eq!(status, 200, "{value}");
		if value["status"] == "completed" {
			break value;
		}
		assert!(
			tokio::time::Instant::now() < deadline,
			"broker did not complete: {value}"
		);
		tokio::time::sleep(std::time::Duration::from_millis(200)).await;
	};
	assert_eq!(fetched["http_status"], 200);
	assert_eq!(fetched["output_file"]["digest"], wheel["digests"]["sha256"]);
	let install = json!({"idempotency_key":Uuid::new_v4(),"expected_revision":first["revision"],"wheels":[{"outbound_operation_id":fetched["operation_id"],"filename":wheel["filename"],"sha256":wheel["digests"]["sha256"]}]});
	let path = format!("{python}/install");
	let mut profile = (*c.f.store.capabilities.0).clone();
	profile.package_origins.clear();
	let allowed = c.f.store.capabilities.clone();
	c.f.store.capabilities = Runtime::new(profile).unwrap();
	c.app = aidash::api::router(c.f.clone());
	assert_eq!(
		request(&c.app, &c.token, "POST", &path, install.clone())
			.await
			.0,
		403
	);
	c.f.store.capabilities = allowed;
	c.app = aidash::api::router(c.f.clone());
	let mut corrupt = install.clone();
	corrupt["wheels"][0]["sha256"] = json!("0".repeat(64));
	assert_eq!(
		request(&c.app, &c.token, "POST", &path, corrupt).await.0,
		409
	);
	let used: i64 = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("used_bytes"))
			.from(Alias::new("core_quotas"))
			.and_where(Expr::col(Alias::new("tenant")).eq("acme"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&c.f.store.pool)
	.await
	.unwrap();
	let quota = Query::update()
		.table(Alias::new("core_quotas"))
		.value(Alias::new("used_bytes"), Expr::cust("$1"))
		.and_where(Expr::col(Alias::new("tenant")).eq("acme"))
		.to_string(PostgresQueryBuilder);
	sqlx::query(&quota)
		.bind(c.f.store.capabilities.0.retained_bytes as i64 - 1)
		.execute(&c.f.store.pool)
		.await
		.unwrap();
	let (status, op) = request(&c.app, &c.token, "POST", &path, install.clone()).await;
	assert_eq!(status, 200, "{op}");
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
	loop {
		let (status, blocked) = request(
			&c.app,
			&c.token,
			"POST",
			&format!("{python}/poll"),
			json!({"operation_id":op["operation_id"]}),
		)
		.await;
		assert_eq!(status, 200, "{blocked}");
		if blocked["error"]["code"] == "STORAGE_QUOTA" {
			assert_eq!(blocked["error"]["retryable"], true);
			break;
		}
		assert!(
			tokio::time::Instant::now() < deadline,
			"quota failure must be visible: {blocked}"
		);
		tokio::time::sleep(std::time::Duration::from_millis(200)).await;
	}
	let (_, preserved) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/runs/{}/working-area", run.id),
		Value::Null,
	)
	.await;
	assert_eq!(preserved["revision"], first["revision"]);
	assert_eq!(preserved["manifest"], json!([]));
	assert_eq!(
		request(&c.app, &c.token, "POST", &path, install.clone())
			.await
			.1["operation_id"],
		op["operation_id"]
	);
	sqlx::query(&quota)
		.bind(used)
		.execute(&c.f.store.pool)
		.await
		.unwrap();
	let installed =
		operation_until(&c, run.id, "python", &op["operation_id"], &["completed"]).await;
	assert!(
		installed["output"]
			.as_str()
			.unwrap()
			.contains("c6242fc49e35958c8b15141343aa660db5fc54d4f13a1db01a3f5891b98700ef")
	);
	assert_eq!(installed["termination_confirmed"], true);
	let (status, retried) = request(&c.app, &c.token, "POST", &path, install).await;
	assert_eq!(status, 200, "{retried}");
	assert_eq!(retried["operation_id"], op["operation_id"]);
	let (status,reset)=request(&c.app,&c.token,"POST",&python,json!({"idempotency_key":Uuid::new_v4(),"expected_revision":installed["revision"],"expected_session_id":first["session_id"],"code":"raise RuntimeError('must never run before reset acknowledgement')"})).await;
	assert_eq!(status, 200, "{reset}");
	assert_eq!(reset["session_reset"], true);
	assert_eq!(
		reset["packages"]["dependencies"]["packages"][0]["version"],
		"2.2.0"
	);
	let (status,execute)=request(&c.app,&c.token,"POST",&python,json!({"idempotency_key":Uuid::new_v4(),"expected_revision":installed["revision"],"expected_session_id":reset["session_id"],"code":"import itsdangerous,os\nassert 'old_memory' not in globals()\nassert not any('SECRET' in k or 'TOKEN' in k or 'PASSWORD' in k for k in os.environ)\ns=itsdangerous.URLSafeSerializer('not-a-service-secret')\nassert s.loads(s.dumps({'city':'東京'}))=={'city':'東京'}\nprint('offline package usable')"})).await;
	assert_eq!(status, 200, "{execute}");
	let done = operation_until(
		&c,
		run.id,
		"python",
		&execute["operation_id"],
		&["completed"],
	)
	.await;
	assert!(
		done["output"]
			.as_str()
			.unwrap()
			.contains("offline package usable")
	);
	// A larger, pinned pure-Python wheel makes the one-second deadline exercise
	// real offline installation rather than relying on a tiny wheel being slow.
	let large: Value =
		serde_json::from_str(include_str!("../fixtures/python-wheel-timeout.json")).unwrap();
	let fetch = json!({"idempotency_key":Uuid::new_v4(),"url":large["url"]});
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
	let fetched = loop {
		let (status, value) = request(&c.app, &c.token, "POST", &outbound, fetch.clone()).await;
		assert_eq!(status, 200, "{value}");
		if value["status"] == "completed" {
			break value;
		}
		assert!(tokio::time::Instant::now() < deadline, "{value}");
		tokio::time::sleep(std::time::Duration::from_millis(200)).await;
	};
	assert_eq!(fetched["output_file"]["digest"], large["digests"]["sha256"]);
	let limited = json!({"idempotency_key":Uuid::new_v4(),"expected_revision":done["revision"],"timeout_seconds":1,"wheels":[{"outbound_operation_id":fetched["operation_id"],"filename":large["filename"],"sha256":large["digests"]["sha256"]}]});
	let (status, op) = request(&c.app, &c.token, "POST", &path, limited.clone()).await;
	assert_eq!(status, 200, "{op}");
	let timed_out = operation_until(&c, run.id, "python", &op["operation_id"], &["failed"]).await;
	assert_eq!(timed_out["termination_confirmed"], true);
	assert!(
		matches!(timed_out["exit_code"].as_i64(), Some(137 | 143)),
		"the runtime deadline must terminate installation: {timed_out}"
	);
	assert_eq!(
		request(&c.app, &c.token, "POST", &path, limited).await.1["operation_id"],
		op["operation_id"]
	);
	stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	c.close().await;
}
