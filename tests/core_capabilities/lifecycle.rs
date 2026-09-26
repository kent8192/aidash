use super::*;

#[rstest::rstest]
#[tokio::test]
async fn interrupted_patch_keeps_old_manifest_and_reclaims_only_abandoned_objects(
	#[future] capability_fixture: CoreFixture,
) {
	use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
	let c = Box::pin(capability_fixture).await;
	let run = admit(&c).await;
	let path = format!("/api/runs/{}/patch", run.id);
	let (status, result) = request(&c.app,&c.token,"POST",&path,json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"preconditions":{"keep.txt":null},"patch":"*** Begin Patch\n*** Add File: keep.txt\n+kept 東京\n*** End Patch"})).await;
	assert_eq!(status, 200, "{result}");
	let (_, before) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/runs/{}/working-area", run.id),
		Value::Null,
	)
	.await;
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
	// The first object is fsynced; reserving the second object fails. The real
	// request must roll back publication and its quota reservation together.
	let quota = c.f.store.capabilities.0.retained_bytes as i64 - 3;
	let quota_query = Query::update()
		.table(Alias::new("core_quotas"))
		.value(Alias::new("used_bytes"), Expr::cust("$1"))
		.and_where(Expr::col(Alias::new("tenant")).eq("acme"))
		.to_string(PostgresQueryBuilder);
	sqlx::query(&quota_query)
		.bind(quota)
		.execute(&c.f.store.pool)
		.await
		.unwrap();
	let (status,failed)=request(&c.app,&c.token,"POST",&path,json!({"idempotency_key":Uuid::new_v4(),"expected_revision":2,"preconditions":{"a.txt":null,"b.txt":null},"patch":"*** Begin Patch\n*** Add File: a.txt\n+a\n*** Add File: b.txt\n+b\n*** End Patch"})).await;
	assert_eq!(status, 409, "{failed}");
	assert_eq!(failed["error"]["code"], "STORAGE_QUOTA");
	let (_, after) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/runs/{}/working-area", run.id),
		Value::Null,
	)
	.await;
	assert_eq!(before, after, "no partial patch may become visible");
	sqlx::query(&quota_query)
		.bind(used)
		.execute(&c.f.store.pool)
		.await
		.unwrap();
	let known: Vec<Uuid> = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("core_objects"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&c.f.store.pool)
	.await
	.unwrap();
	let mut abandoned = vec![];
	for entry in std::fs::read_dir(&c.root).unwrap() {
		let entry = entry.unwrap();
		if let Some(id) = entry
			.file_name()
			.to_str()
			.and_then(|s| Uuid::parse_str(s).ok())
			&& !known.contains(&id)
		{
			abandoned.push(entry.path());
		}
	}
	assert_eq!(
		abandoned.len(),
		1,
		"fault must leave actual unpublished bytes"
	);
	// A transaction still allocating another object owns the same advisory lock
	// as production. The collector must leave it untouched until rollback.
	let live = Uuid::new_v4();
	let mut tx = c.f.store.pool.begin().await.unwrap();
	sqlx::query(
		&Query::select()
			.expr(Expr::cust("pg_advisory_xact_lock(hashtextextended($1, 0))"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(format!("core-object:{live}"))
	.execute(&mut *tx)
	.await
	.unwrap();
	let live_path = c.root.join(live.simple().to_string());
	tokio::fs::write(&live_path, b"in progress").await.unwrap();
	tokio::fs::write(
		c.root.join(".intents").join(live.to_string()),
		serde_json::to_vec(
			&json!({"id":live,"tenant":"acme","area_id":before["id"],"kind":"working","size":11}),
		)
		.unwrap(),
	)
	.await
	.unwrap();
	let (stop, rx) = tokio::sync::watch::channel(false);
	let worker = tokio::spawn(aidash::capabilities::operations::run(c.f.store.clone(), rx));
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
	while abandoned[0].exists() {
		assert!(
			tokio::time::Instant::now() < deadline,
			"abandoned patch bytes not reclaimed"
		);
		tokio::time::sleep(std::time::Duration::from_millis(50)).await;
	}
	assert!(
		live_path.exists(),
		"collector must not race an active transaction"
	);
	tx.rollback().await.unwrap();
	while live_path.exists() {
		assert!(
			tokio::time::Instant::now() < deadline,
			"rolled-back bytes not reclaimed"
		);
		tokio::time::sleep(std::time::Duration::from_millis(50)).await;
	}
	for id in known {
		assert!(
			c.root.join(id.simple().to_string()).exists(),
			"committed bytes remain owned"
		);
	}
	let (status, read) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/runs/{}/files/read", run.id),
		json!({"file_id":before["manifest"][0]["file_id"],"representation":"text"}),
	)
	.await;
	assert_eq!(status, 200, "{read}");
	assert_eq!(read["content"], "kept 東京\n");
	stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn thread_deletion_pages_more_than_one_hundred_owned_areas(
	#[future] capability_fixture: CoreFixture,
) {
	use sea_orm::sea_query::{Alias, Asterisk, Expr, PostgresQueryBuilder, Query};
	let c = Box::pin(capability_fixture).await;
	let workspace = c.f.store.task(c.task).await.unwrap().workspace_id;
	let root = source(&c, workspace, "Delete a thread with many areas").await;
	let (status, thread) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/workspaces/{workspace}/threads"),
		json!({"root_message_id":root}),
	)
	.await;
	assert_eq!(status, 200, "{thread}");
	let thread_id: Uuid = serde_json::from_value(thread["id"].clone()).unwrap();
	let (status, area) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/workspaces/{workspace}/threads/{thread_id}/agents/research/runs"),
		json!({"idempotency_key":Uuid::new_v4(),"agent_version":"1.1.0","title":"Thread-bound work","description":"Exercise bounded area pagination"}),
	)
	.await;
	assert_eq!(status, 200, "{area}");
	let base_id: Uuid = serde_json::from_value(area["id"].clone()).unwrap();
	let base: aidash::capabilities::contracts::Area = sqlx::query_as(
		&Query::select()
			.column(Asterisk)
			.from(Alias::new("core_areas"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(base_id)
	.fetch_one(&c.f.store.pool)
	.await
	.unwrap();
	for index in 0..100 {
		let mut extra = base.clone();
		extra.id = Uuid::new_v4();
		extra.agent_id = format!("thread-delete-fixture-{index}");
		let query = Query::insert()
			.into_table(Alias::new("core_areas"))
			.columns(
				[
					"id",
					"tenant",
					"home_node",
					"workspace_id",
					"thread_id",
					"agent_id",
					"owner",
					"generation",
					"revision",
					"epoch",
					"state",
					"manifest",
					"constraints",
					"next_sequence",
				]
				.map(Alias::new),
			)
			.values_panic((1..=14).map(|index| Expr::cust(format!("${index}"))))
			.to_string(PostgresQueryBuilder);
		sqlx::query(&query)
			.bind(extra.id)
			.bind(extra.tenant)
			.bind(extra.home_node)
			.bind(extra.workspace_id)
			.bind(extra.thread_id)
			.bind(extra.agent_id)
			.bind(extra.owner)
			.bind(extra.generation)
			.bind(extra.revision)
			.bind(extra.epoch)
			.bind(extra.state)
			.bind(extra.manifest)
			.bind(extra.constraints)
			.bind(extra.next_sequence)
			.execute(&c.f.store.pool)
			.await
			.unwrap();
	}
	let areas: Vec<(Uuid, i64)> = sqlx::query_as(
		&Query::select()
			.columns([Alias::new("id"), Alias::new("revision")])
			.from(Alias::new("core_areas"))
			.and_where(Expr::col(Alias::new("workspace_id")).eq(Expr::cust("$1")))
			.and_where(Expr::col(Alias::new("thread_id")).eq(Expr::cust("$2")))
			.and_where(Expr::col(Alias::new("owner")).eq(Expr::cust("$3")))
			.order_by(Alias::new("id"), sea_orm::sea_query::Order::Asc)
			.to_string(PostgresQueryBuilder),
	)
	.bind(workspace)
	.bind(thread_id)
	.bind("alice")
	.fetch_all(&c.f.store.pool)
	.await
	.unwrap();
	assert_eq!(areas.len(), 101);
	let files = areas
		.iter()
		.map(|(id, revision)| json!({"area_id":id,"expected_revision":revision,"choice":"keep"}))
		.collect::<Vec<_>>();
	let (status, deleted) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/workspaces/{workspace}/threads/{thread_id}/delete"),
		json!({"idempotency_key":Uuid::new_v4(),"files":files}),
	)
	.await;
	assert_eq!(status, 200, "{deleted}");
	assert_eq!(deleted["state"], "deleted");
	assert_eq!(deleted["file_operations"].as_array().unwrap().len(), 101);
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn deleted_thread_retains_files_and_restores_only_into_a_new_authorized_thread(
	#[future] capability_fixture: CoreFixture,
) {
	let c = Box::pin(capability_fixture).await;
	let workspace = c.f.store.task(c.task).await.unwrap().workspace_id;
	let root = source(&c, workspace, "This thread will be deleted").await;
	let (status, thread) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/workspaces/{workspace}/threads"),
		json!({"root_message_id":root}),
	)
	.await;
	assert_eq!(status, 200, "{thread}");
	let thread_id = thread["id"].as_str().unwrap();
	let (status,area)=request(&c.app,&c.token,"POST",&format!("/api/workspaces/{workspace}/threads/{thread_id}/agents/research/runs"),json!({"idempotency_key":Uuid::new_v4(),"agent_version":"1.1.0","title":"Thread-bound work","description":"Preserve files"})).await;
	assert_eq!(status, 200, "{area}");
	let (status, session) = request(
		&c.app,
		&c.token,
		"GET",
		&format!(
			"/api/working-areas/{}/session",
			area["id"].as_str().unwrap()
		),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{session}");
	let run = session["active_run_id"].as_str().unwrap();
	let (status,result)=request(&c.app,&c.token,"POST",&format!("/api/runs/{run}/patch"),json!({"idempotency_key":Uuid::new_v4(),"expected_revision":area["revision"],"preconditions":{"kept.txt":null},"patch":"*** Begin Patch\n*** Add File: kept.txt\n+Keep 東京 exactly\n*** End Patch"})).await;
	assert_eq!(status, 200, "{result}");
	let (_, before) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/runs/{run}/working-area"),
		Value::Null,
	)
	.await;
	let path = format!("/api/workspaces/{workspace}/threads/{thread_id}/delete");
	assert_eq!(
		request(
			&c.app,
			&c.token,
			"POST",
			&path,
			json!({"idempotency_key":Uuid::new_v4(),"files":[]})
		)
		.await
		.0,
		409,
		"file choices must precede disappearing thread controls"
	);
	let input = json!({"idempotency_key":Uuid::new_v4(),"files":[{"area_id":area["id"],"expected_revision":before["revision"],"choice":"keep"}]});
	let (status, deleted) = request(&c.app, &c.token, "POST", &path, input.clone()).await;
	assert_eq!(status, 200, "{deleted}");
	assert_eq!(
		request(&c.app, &c.token, "POST", &path, input).await,
		(200, deleted)
	);
	assert_eq!(
		request(
			&c.app,
			&c.token,
			"GET",
			&format!("/api/workspaces/{workspace}/message-history?thread_id={thread_id}"),
			Value::Null
		)
		.await
		.0,
		404
	);
	let (_, history) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/workspaces/{workspace}/message-history"),
		Value::Null,
	)
	.await;
	assert!(!history.to_string().contains("This thread will be deleted"));
	let (status, inventory) =
		request(&c.app, &c.token, "GET", "/api/working-files", Value::Null).await;
	assert_eq!(status, 200, "{inventory}");
	let retained = &inventory["items"][0];
	assert_eq!(retained["state"], "retained");
	assert_eq!(retained["files"], 1);
	assert!(retained["snapshot_id"].is_string());
	let restore = format!(
		"/api/working-areas/{}/restore",
		area["id"].as_str().unwrap()
	);
	let invalid = json!({"idempotency_key":Uuid::new_v4(),"expected_revision":retained["revision"],"snapshot_id":retained["snapshot_id"],"thread_id":thread_id});
	assert_eq!(
		request(&c.app, &c.token, "POST", &restore, invalid).await.0,
		404
	);
	let new_root = source(&c, workspace, "New authorized conversation").await;
	let (_, new_thread) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/workspaces/{workspace}/threads"),
		json!({"root_message_id":new_root}),
	)
	.await;
	let input = json!({"idempotency_key":Uuid::new_v4(),"expected_revision":retained["revision"],"snapshot_id":retained["snapshot_id"],"thread_id":new_thread["id"]});
	let (_, bob) = request(
		&c.app,
		&c.f.config.api_token,
		"POST",
		"/api/authorization/acme/credentials",
		json!({"subject":"bob"}),
	)
	.await;
	assert_eq!(
		request(
			&c.app,
			bob["token"].as_str().unwrap(),
			"POST",
			&restore,
			input.clone()
		)
		.await
		.0,
		404
	);
	let (status, restored) = request(&c.app, &c.token, "POST", &restore, input).await;
	assert_eq!(status, 200, "{restored}");
	assert_eq!(restored["manifest"], before["manifest"]);
	assert_eq!(restored["thread_id"], new_thread["id"]);
	assert_ne!(restored["generation"], before["generation"]);
	assert_ne!(request(&c.app,&c.token,"POST",&format!("/api/runs/{run}/patch"),json!({"idempotency_key":Uuid::new_v4(),"expected_revision":restored["revision"],"preconditions":{"late.txt":null},"patch":"*** Begin Patch\n*** Add File: late.txt\n+late\n*** End Patch"})).await.0,200);
	c.close().await;
}

#[rstest::fixture]
async fn short_staging(#[future] capability_fixture: CoreFixture) -> CoreFixture {
	let mut c = Box::pin(capability_fixture).await;
	let mut profile = (*c.f.store.capabilities.0).clone();
	profile.staging_seconds = 1;
	c.f.store.capabilities = Runtime::new(profile).unwrap();
	c.app = aidash::api::router(c.f.clone());
	c
}
#[rstest::rstest]
#[tokio::test]
async fn expired_upload_releases_only_its_owned_staging_bytes(
	#[future] short_staging: CoreFixture,
) {
	use base64::Engine;
	use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
	let c = Box::pin(short_staging).await;
	let bytes = b"expired original upload";
	let (status,upload)=request(&c.app,&c.token,"POST","/api/references/uploads",json!({"idempotency_key":Uuid::new_v4(),"name":"incomplete.txt","media_type":"text/plain","size":bytes.len(),"digest":aidash::capabilities::objects::digest(bytes)})).await;
	assert_eq!(status, 200, "{upload}");
	let (status, result) = request(
		&c.app,
		&c.token,
		"POST",
		&format!(
			"/api/references/{}/chunks",
			upload["reference_id"].as_str().unwrap()
		),
		json!({"offset":0,"data":base64::engine::general_purpose::STANDARD.encode(bytes)}),
	)
	.await;
	assert_eq!(status, 200, "{result}");
	let object: Uuid = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("core_objects"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&c.f.store.pool)
	.await
	.unwrap();
	let object_path = c.root.join(object.simple().to_string());
	assert!(object_path.exists());
	let (stop, rx) = tokio::sync::watch::channel(false);
	let worker = tokio::spawn(aidash::capabilities::operations::run(c.f.store.clone(), rx));
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
	loop {
		let used: Option<i64> = sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("used_bytes"))
				.from(Alias::new("core_quotas"))
				.and_where(Expr::col(Alias::new("tenant")).eq("acme"))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&c.f.store.pool)
		.await
		.unwrap();
		if used == Some(0) {
			break;
		}
		assert!(
			tokio::time::Instant::now() < deadline,
			"expired staging quota was not released"
		);
		tokio::time::sleep(std::time::Duration::from_millis(100)).await;
	}
	assert!(!object_path.exists());
	assert_eq!(
		request(
			&c.app,
			&c.token,
			"GET",
			&format!(
				"/api/references/{}",
				upload["reference_id"].as_str().unwrap()
			),
			Value::Null
		)
		.await
		.0,
		404
	);
	stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	c.close().await;
}

#[rstest::fixture]
async fn cleanup_fault_fixture(#[future] capability_fixture: CoreFixture) -> (CoreFixture, Value) {
	let c = Box::pin(capability_fixture).await;
	let run = admit(&c).await;
	let (status,result)=request(&c.app,&c.token,"POST",&format!("/api/runs/{}/patch",run.id),json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"preconditions":{"a.txt":null,"b.txt":null},"patch":"*** Begin Patch\n*** Add File: a.txt\n+first 東京\n*** Add File: b.txt\n+second\n*** End Patch"})).await;
	assert_eq!(status, 200, "{result}");
	let (status, area) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/runs/{}/working-area", run.id),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{area}");
	(c, area)
}

#[rstest::rstest]
#[tokio::test]
async fn failed_snapshot_preserves_active_files_and_interrupted_delete_is_reconcilable(
	#[future] cleanup_fault_fixture: (CoreFixture, Value),
) {
	let (c, area) = Box::pin(cleanup_fault_fixture).await;
	let id = area["id"].as_str().unwrap();
	let file: Uuid = serde_json::from_value(area["manifest"][1]["file_id"].clone()).unwrap();
	let owned = c.root.join(file.simple().to_string());
	let backup = c.root.join("fault-backup");
	tokio::fs::rename(&owned, &backup).await.unwrap();
	let (status,failed)=request(&c.app,&c.token,"POST",&format!("/api/working-areas/{id}/cleanup"),json!({"idempotency_key":Uuid::new_v4(),"expected_revision":area["revision"],"choice":"recoverable"})).await;
	assert!(
		status >= 400,
		"snapshot must not succeed with unavailable bytes: {failed}"
	);
	tokio::fs::rename(&backup, &owned).await.unwrap();
	let (status, inventory) =
		request(&c.app, &c.token, "GET", "/api/working-files", Value::Null).await;
	assert_eq!(status, 200, "{inventory}");
	assert_eq!(inventory["items"][0]["state"], "active");
	assert_eq!(inventory["items"][0]["revision"], area["revision"]);
	assert_eq!(inventory["items"][0]["files"], 2);
	let (status, confirmation) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/working-areas/{id}/deletion-confirmation"),
		json!({"expected_revision":area["revision"]}),
	)
	.await;
	assert_eq!(status, 200, "{confirmation}");
	let (status,deletion)=request(&c.app,&c.token,"POST",&format!("/api/working-areas/{id}/cleanup"),json!({"idempotency_key":Uuid::new_v4(),"expected_revision":area["revision"],"choice":"irreversible","confirmation_id":confirmation["confirmation_id"]})).await;
	assert_eq!(status, 200, "{deletion}");
	assert_eq!(deletion["state"], "deleting");
	// Deterministic storage failure while deleting an owned object, after the
	// durable intent fenced every writer. No unrelated path is modified.
	tokio::fs::rename(&owned, &backup).await.unwrap();
	tokio::fs::create_dir(&owned).await.unwrap();
	let (stop, rx) = tokio::sync::watch::channel(false);
	let worker = tokio::spawn(aidash::capabilities::operations::run(c.f.store.clone(), rx));
	let status_path = format!(
		"/api/file-cleanups/{}",
		deletion["operation_id"].as_str().unwrap()
	);
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
	loop {
		let (status, state) = request(&c.app, &c.token, "GET", &status_path, Value::Null).await;
		assert_eq!(status, 200, "{state}");
		if state["state"] == "cleanup_failed" {
			break;
		}
		assert!(tokio::time::Instant::now() < deadline, "{state}");
		tokio::time::sleep(std::time::Duration::from_millis(100)).await;
	}
	stop.send(true).unwrap();
	worker.await.unwrap().unwrap();
	let (status, inventory) =
		request(&c.app, &c.token, "GET", "/api/working-files", Value::Null).await;
	assert_eq!(status, 200, "{inventory}");
	assert_eq!(
		inventory["items"][0]["cleanup_operation_id"],
		deletion["operation_id"]
	);
	tokio::fs::remove_dir(&owned).await.unwrap();
	tokio::fs::rename(&backup, &owned).await.unwrap();
	let (status, result) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("{status_path}/reconcile"),
		json!({}),
	)
	.await;
	assert_eq!(status, 200, "{result}");
	assert_eq!(result["state"], "deleted");
	assert!(!owned.exists());
	let (status, retry) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("{status_path}/reconcile"),
		json!({}),
	)
	.await;
	assert_eq!(status, 200, "{retry}");
	assert_eq!(retry, result);
	c.close().await;
}
