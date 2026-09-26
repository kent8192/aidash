use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};

#[rstest::fixture]
async fn withdrawn_operation_fixture(
	#[future] capability_fixture: CoreFixture,
) -> (CoreFixture, aidash::domain::Run, Value, Uuid) {
	let c = Box::pin(capability_fixture).await;
	let run = admit(&c).await;
	let (status, area) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/runs/{}/working-area", run.id),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{area}");
	let id = Uuid::new_v4();
	// Recover a committed dispatch intent whose originating credential was lost.
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("core_operations"))
			.columns(
				[
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
				]
				.map(Alias::new),
			)
			.values_panic((1..=17).map(|i| Expr::cust(format!("${i}"))))
			.to_string(PostgresQueryBuilder),
	)
	.bind(id)
	.bind(serde_json::from_value::<Uuid>(area["id"].clone()).unwrap())
	.bind(run.id)
	.bind("acme")
	.bind("alice")
	.bind(Uuid::new_v4())
	.bind(json!(["alice"]))
	.bind(id.to_string())
	.bind("immutable-intent")
	.bind("shell")
	.bind("prepared")
	.bind(area["epoch"].as_i64().unwrap())
	.bind(area["generation"].as_i64().unwrap())
	.bind(area["revision"].as_i64().unwrap())
	.bind(2_i64)
	.bind(json!({"command":"must never execute","seconds":1}))
	.bind(json!({}))
	.execute(&c.f.store.pool)
	.await
	.unwrap();
	sqlx::query(
		&Query::update()
			.table(Alias::new("core_areas"))
			.value(Alias::new("state"), "running")
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(serde_json::from_value::<Uuid>(area["id"].clone()).unwrap())
	.execute(&c.f.store.pool)
	.await
	.unwrap();
	(c, run, area, id)
}

#[rstest::rstest]
#[tokio::test]
async fn authority_withdrawal_before_dispatch_keeps_saved_files_usable(
	#[future] withdrawn_operation_fixture: (CoreFixture, aidash::domain::Run, Value, Uuid),
) {
	let (c, run, area, id) = Box::pin(withdrawn_operation_fixture).await;
	let (stop, rx) = tokio::sync::watch::channel(false);
	let worker = tokio::spawn(aidash::capabilities::operations::run(c.f.store.clone(), rx));
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(8);
	loop {
		let state: String = sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("state"))
				.from(Alias::new("core_operations"))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
				.to_string(PostgresQueryBuilder),
		)
		.bind(id)
		.fetch_one(&c.f.store.pool)
		.await
		.unwrap();
		if state == "withdrawn" {
			break;
		}
		assert!(tokio::time::Instant::now() < deadline, "{state}");
		tokio::time::sleep(std::time::Duration::from_millis(50)).await;
	}
	stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	let state: String = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("state"))
			.from(Alias::new("core_areas"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(serde_json::from_value::<Uuid>(area["id"].clone()).unwrap())
	.fetch_one(&c.f.store.pool)
	.await
	.unwrap();
	assert_eq!(
		state, "active",
		"an undispatched operation has no uncertain effects"
	);
	let (status,patched) = request(&c.app,&c.token,"POST",&path_for_patch(run.id),json!({"idempotency_key":Uuid::new_v4(),"expected_revision":area["revision"],"preconditions":{"saved.txt":null},"patch":"*** Begin Patch\n*** Add File: saved.txt\n+still usable\n*** End Patch"})).await;
	assert_eq!(status, 200, "{patched}");
	c.close().await;
}

#[rstest::fixture]
async fn extraction_queue_fixture(
	#[future] capability_fixture: CoreFixture,
) -> (CoreFixture, String) {
	let mut c = Box::pin(capability_fixture).await;
	let bytes = b"private original";

	let mut uploads = vec![];
	for index in 0..9 {
		let (status, credential) = request(
			&c.app,
			&c.f.config.api_token,
			"POST",
			"/api/authorization/acme/credentials",
			json!({"subject":"alice"}),
		)
		.await;
		assert_eq!(status, 200, "{credential}");
		let token = credential["token"].as_str().unwrap();
		let (status, upload) = request(&c.app,token,"POST","/api/references/uploads",json!({"idempotency_key":Uuid::new_v4(),"name":format!("record-{index}.txt"),"media_type":"text/plain","size":bytes.len(),"digest":aidash::capabilities::objects::digest(bytes)})).await;
		assert_eq!(status, 200, "{upload}");
		let id: Uuid = serde_json::from_value(upload["reference_id"].clone()).unwrap();
		let path = format!("/api/references/{id}");
		assert_eq!(
			request(
				&c.app,
				token,
				"POST",
				&format!("{path}/chunks"),
				json!({"offset":0,"data":STANDARD.encode(bytes)})
			)
			.await
			.0,
			200
		);
		assert_eq!(
			request(
				&c.app,
				token,
				"POST",
				&format!("{path}/commit"),
				Value::Null
			)
			.await
			.0,
			200
		);
		uploads.push((
			id,
			path,
			credential["credential"]["id"].as_str().unwrap().to_owned(),
		));
	}
	// Revoke the credentials of the actual first scheduling batch without
	// rewriting/reordering its reference rows. The ninth upload stays valid.
	let first: Vec<Uuid> = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("core_records"))
			.and_where(Expr::col(Alias::new("kind")).eq("reference"))
			.and_where(Expr::col(Alias::new("state")).eq("extracting"))
			.limit(8)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&c.f.store.pool)
	.await
	.unwrap();
	assert_eq!(first.len(), 8);
	let last = uploads
		.iter()
		.find(|(id, _, _)| !first.contains(id))
		.unwrap()
		.1
		.clone();
	for (id, _, credential) in uploads {
		if first.contains(&id) {
			let (status, revoked) = request(
				&c.app,
				&c.f.config.api_token,
				"POST",
				&format!("/api/authorization/acme/credentials/{credential}/revoke"),
				json!({}),
			)
			.await;
			assert_eq!(status, 200, "{revoked}");
		}
	}
	// Valid undispatched work can finish rollback without a runtime. Stale
	// credentials must not monopolize the first eight extraction queue slots.
	let mut profile = (*c.f.store.capabilities.0).clone();
	profile.admission = false;
	c.f.store.capabilities = Runtime::new(profile).unwrap();
	c.app = aidash::api::router(c.f.clone());
	(c, last)
}

#[rstest::rstest]
#[tokio::test]
async fn stale_extraction_credentials_do_not_starve_later_uploads(
	#[future] extraction_queue_fixture: (CoreFixture, String),
) {
	let (c, path) = Box::pin(extraction_queue_fixture).await;
	let (stop, rx) = tokio::sync::watch::channel(false);
	let worker = tokio::spawn(aidash::capabilities::operations::run(c.f.store.clone(), rx));
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
	let finished = loop {
		let (status, state) = request(&c.app, &c.token, "GET", &path, Value::Null).await;
		assert_eq!(status, 200, "{state}");
		if state["state"] == "ready" || tokio::time::Instant::now() >= deadline {
			break state;
		}
		tokio::time::sleep(std::time::Duration::from_millis(100)).await;
	};
	stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	assert_eq!(
		finished["state"], "ready",
		"stale credentials must not starve later work: {finished}"
	);
	assert_eq!(finished["extraction_state"], "admission_disabled");
	let (status, original) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("{path}/download"),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{original}");
	assert_eq!(
		STANDARD.decode(original["data"].as_str().unwrap()).unwrap(),
		b"private original"
	);
	c.close().await;
}

#[rstest::fixture]
async fn full_mount_fixture(
	#[future] capability_fixture: CoreFixture,
) -> (CoreFixture, aidash::domain::Run) {
	let mut c = Box::pin(capability_fixture).await;
	let run = admit(&c).await;
	let skills: Value = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("data"))
			.from(Alias::new("core_records"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(run.id)
	.fetch_one(&c.f.store.pool)
	.await
	.unwrap();
	let mut profile = (*c.f.store.capabilities.0).clone();
	profile.working_bytes = skills
		.as_array()
		.unwrap()
		.iter()
		.flat_map(|skill| skill["files"].as_array().unwrap())
		.map(|file| file["size"].as_u64().unwrap())
		.sum();
	assert!(profile.working_bytes > 1);
	c.f.store.capabilities = Runtime::new(profile).unwrap();
	c.app = aidash::api::router(c.f.clone());
	(c, run)
}

#[rstest::rstest]
#[case(0, "WORKING_QUOTA_EXCEEDED")]
#[case(1, "RUNTIME_UNAVAILABLE")]
#[tokio::test]
async fn mounted_skills_require_writable_capacity_before_operation_commit(
	#[future] full_mount_fixture: (CoreFixture, aidash::domain::Run),
	#[case] writable_bytes: u64,
	#[case] expected_error: &str,
) {
	let (mut c, run) = Box::pin(full_mount_fixture).await;
	let mut profile = (*c.f.store.capabilities.0).clone();
	profile.working_bytes += writable_bytes;
	c.f.store.capabilities = Runtime::new(profile).unwrap();
	c.app = aidash::api::router(c.f.clone());
	let (status, denied) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/runs/{}/shell", run.id),
		json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"command":"printf must-not-run"}),
	)
	.await;
	assert_eq!(status, 409, "{denied}");
	assert!(
		denied["error"]["code"]
			.as_str()
			.unwrap()
			.starts_with(expected_error),
		"{denied}"
	);
	let (_, area) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/runs/{}/working-area", run.id),
		Value::Null,
	)
	.await;
	assert_eq!(area["state"], "active");
	let count: i64 = sqlx::query_scalar(
		&Query::select()
			.expr(Expr::col(Alias::new("id")).count())
			.from(Alias::new("core_operations"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&c.f.store.pool)
	.await
	.unwrap();
	assert_eq!(
		count, 0,
		"rejected admission must not leave a durable dispatch intent"
	);
	c.close().await;
}
