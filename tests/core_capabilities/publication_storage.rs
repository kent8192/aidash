use super::*;
use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};

#[rstest::fixture]
fn storage_fixture(
	#[future] runtime_fixture: CoreFixture,
) -> impl std::future::Future<Output = (CoreFixture, Uuid, Value)> {
	let runtime_fixture = Box::pin(runtime_fixture);
	async move {
		let c = runtime_fixture.await;
		let run = admit(&c).await;
		let (status,patch)=request(&c.app,&c.token,"POST",&path_for_patch(run.id),json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"preconditions":{"kept.txt":null},"patch":format!("*** Begin Patch\n*** Add File: kept.txt\n+{}\n*** End Patch","x".repeat(4096))})).await;
		assert_eq!(status, 200, "{patch}");
		let (_, area) = request(
			&c.app,
			&c.token,
			"GET",
			&format!("/api/runs/{}/working-area", run.id),
			Value::Null,
		)
		.await;
		(c, run.id, area)
	}
}

#[rstest::rstest]
#[tokio::test]
async fn unchanged_exports_reuse_objects_and_superseded_working_bytes_are_reclaimed(
	#[future] storage_fixture: (CoreFixture, Uuid, Value),
) {
	let (c, run, initial) = Box::pin(storage_fixture).await;
	let old: Uuid = serde_json::from_value(initial["manifest"][0]["file_id"].clone()).unwrap();
	let baseline: i64 = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("used_bytes"))
			.from(Alias::new("core_quotas"))
			.and_where(Expr::col(Alias::new("tenant")).eq("acme"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&c.f.store.pool)
	.await
	.unwrap();
	let (stop, rx) = tokio::sync::watch::channel(false);
	let worker = tokio::spawn(aidash::capabilities::operations::run(c.f.store.clone(), rx));
	let mut revision = initial["revision"].clone();
	for command in [":", ":", "printf replacement > kept.txt"] {
		let (status, operation) = request(
			&c.app,
			&c.token,
			"POST",
			&format!("/api/runs/{run}/shell"),
			json!({"idempotency_key":Uuid::new_v4(),"expected_revision":revision,"command":command}),
		)
		.await;
		assert_eq!(status, 200, "{operation}");
		let done =
			operation_until(&c, run, "shell", &operation["operation_id"], &["completed"]).await;
		revision = done["revision"].clone();
		let (_, area) = request(
			&c.app,
			&c.token,
			"GET",
			&format!("/api/runs/{run}/working-area"),
			Value::Null,
		)
		.await;
		if command == ":" {
			assert_eq!(area["manifest"], initial["manifest"]);
		} else {
			assert_ne!(
				area["manifest"][0]["file_id"],
				initial["manifest"][0]["file_id"]
			);
		}
	}
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
	loop {
		let count: i64 = sqlx::query_scalar(
			&Query::select()
				.expr(Expr::col(Alias::new("id")).count())
				.from(Alias::new("core_objects"))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
				.to_string(PostgresQueryBuilder),
		)
		.bind(old)
		.fetch_one(&c.f.store.pool)
		.await
		.unwrap();
		if count == 0 {
			break;
		}
		assert!(
			tokio::time::Instant::now() < deadline,
			"old working object not reclaimed"
		);
		tokio::time::sleep(std::time::Duration::from_millis(100)).await;
	}
	assert!(!c.root.join(old.simple().to_string()).exists());
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
	assert_eq!(
		used,
		baseline - initial["manifest"][0]["size"].as_i64().unwrap() + b"replacement".len() as i64
	);
	stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	c.close().await;
}

async fn seed_receipt(c: &CoreFixture, template: Uuid, id: Uuid, instance: &str, digest: &str) {
	let columns = [
		"id",
		"area_id",
		"run_id",
		"tenant",
		"principal",
		"credential_id",
		"subjects",
		"request_key",
		"digest",
		"kind",
		"state",
		"epoch",
		"generation",
		"revision",
		"policy_revision",
		"input",
		"result",
		"runner_instance",
	];
	let mut select = Query::select();
	for column in columns {
		select.expr(match column {
			"id" => Expr::cust("$1"),
			"request_key" => Expr::cust("$1::text"),
			"runner_instance" => Expr::cust("$2"),
			"digest" => Expr::cust("$4"),
			"result" => Expr::cust("'{\"runner_acknowledged\":false}'::jsonb"),
			_ => Expr::col(Alias::new(column)).into(),
		});
	}
	select
		.from(Alias::new("core_operations"))
		.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$3")));
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("core_operations"))
			.columns(columns.map(Alias::new))
			.select_from(select)
			.unwrap()
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.bind(instance)
	.bind(template)
	.bind(digest)
	.execute(&c.f.store.pool)
	.await
	.unwrap();
}

#[rstest::rstest]
#[tokio::test]
async fn stale_and_rejected_receipts_cannot_starve_new_terminal_payload_acknowledgement(
	#[future] storage_fixture: (CoreFixture, Uuid, Value),
) {
	use super::extraction_lifecycle_tests::runner;
	let (c, run, initial) = Box::pin(storage_fixture).await;
	let (stop, rx) = tokio::sync::watch::channel(false);
	let worker = tokio::spawn(aidash::capabilities::operations::run(c.f.store.clone(), rx));
	let (status, operation) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/runs/{run}/shell"),
		json!({"idempotency_key":Uuid::new_v4(),"expected_revision":initial["revision"],"command":":"}),
	)
	.await;
	assert_eq!(status, 200, "{operation}");
	operation_until(&c, run, "shell", &operation["operation_id"], &["completed"]).await;
	stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	let template: Uuid = serde_json::from_value(operation["operation_id"].clone()).unwrap();
	let health = runner(&c, reqwest::Method::GET, "/v1/health", None).await;
	let instance = health["instance"].as_str().unwrap();
	let mut staged = vec![];
	for index in 0..40 {
		let mut bytes = *Uuid::new_v4().as_bytes();
		bytes[0] = 0;
		let id = Uuid::from_bytes(bytes);
		if index >= 20 {
			let observed=runner(&c,reqwest::Method::POST,"/v1/operations",Some(json!({"operation_id":id,"area_id":Uuid::new_v4(),"epoch":1,"digest":"receipt-fixture","kind":"shell","code":":","seconds":30,"files":[{"file_id":Uuid::new_v4(),"path":"not-uploaded","scope":"working","size":1,"digest":aidash::capabilities::objects::digest(b"x")}]}))).await;
			assert_eq!(observed["status"], "awaiting_files");
			staged.push(id);
		}
		seed_receipt(
			&c,
			template,
			id,
			if index < 20 {
				"lost-runner-instance"
			} else {
				instance
			},
			"receipt-fixture",
		)
		.await;
	}
	let mut bytes = *Uuid::new_v4().as_bytes();
	bytes[0] = 254;
	let fresh = Uuid::from_bytes(bytes);
	runner(&c,reqwest::Method::POST,"/v1/operations",Some(json!({"operation_id":fresh,"area_id":Uuid::new_v4(),"epoch":1,"digest":"fresh-receipt","kind":"shell","code":"printf fresh-payload","seconds":30,"files":[]}))).await;
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
	loop {
		let observed = runner(
			&c,
			reqwest::Method::GET,
			&format!("/v1/operations/{fresh}"),
			None,
		)
		.await;
		if observed["status"] == "completed" {
			assert_ne!(observed["stdout"], "");
			break;
		}
		assert!(tokio::time::Instant::now() < deadline, "{observed}");
		tokio::time::sleep(std::time::Duration::from_millis(100)).await;
	}
	seed_receipt(&c, template, fresh, instance, "fresh-receipt").await;
	stop.send_replace(false);
	let worker = tokio::spawn(aidash::capabilities::operations::run(
		c.f.store.clone(),
		stop.subscribe(),
	));
	loop {
		let observed = runner(
			&c,
			reqwest::Method::GET,
			&format!("/v1/operations/{fresh}"),
			None,
		)
		.await;
		if observed["acknowledged"] == true {
			assert_eq!(observed["stdout"], "");
			break;
		}
		assert!(
			tokio::time::Instant::now() < deadline,
			"new receipt starved: {observed}"
		);
		tokio::time::sleep(std::time::Duration::from_millis(100)).await;
	}
	let retired: i64 = sqlx::query_scalar(
		&Query::select()
			.expr(Expr::col(Alias::new("id")).count())
			.from(Alias::new("core_operations"))
			.and_where(Expr::col(Alias::new("runner_instance")).eq("lost-runner-instance"))
			.and_where(Expr::cust("result->'runner_acknowledged' = 'true'::jsonb"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&c.f.store.pool)
	.await
	.unwrap();
	assert_eq!(retired, 20);
	stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	for id in staged {
		runner(
			&c,
			reqwest::Method::POST,
			&format!("/v1/operations/{id}/cancel"),
			None,
		)
		.await;
		runner(
			&c,
			reqwest::Method::POST,
			&format!("/v1/operations/{id}/ack"),
			Some(json!({"digest":"receipt-fixture"})),
		)
		.await;
	}
	c.close().await;
}
