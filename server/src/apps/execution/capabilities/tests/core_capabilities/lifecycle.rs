use super::*;

pub(super) async fn wait_for_channel_thread_lock_waiters(pool: &sqlx::PgPool, minimum: i64) {
	use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
	let query = Query::select()
		.expr(Expr::cust("count(*)"))
		.from(Alias::new("pg_stat_activity"))
		.and_where(Expr::col(Alias::new("wait_event_type")).eq("Lock"))
		.and_where(Expr::col(Alias::new("query")).like("%channel_threads%"))
		.to_string(PostgresQueryBuilder);
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
	loop {
		let waiting: i64 = sqlx::query_scalar(&query).fetch_one(pool).await.unwrap();
		if waiting >= minimum {
			return;
		}
		assert!(
			tokio::time::Instant::now() < deadline,
			"expected {minimum} thread lock waiters; found {waiting}"
		);
		tokio::time::sleep(std::time::Duration::from_millis(10)).await;
	}
}

async fn wait_for_blocked_connection(pool: &sqlx::PgPool, blocker: i32) -> i32 {
	use reinhardt::query::{Alias, Expr, IntoIden, PostgresQueryBuilder, Query, SimpleExpr};
	let query = Query::select()
		.column(Alias::new("pid"))
		.from(Alias::new("pg_stat_activity"))
		.and_where(Expr::val(blocker).eq(SimpleExpr::FunctionCall(
			Alias::new("ANY").into_iden(),
			vec![SimpleExpr::FunctionCall(
				Alias::new("pg_blocking_pids").into_iden(),
				vec![Expr::col(Alias::new("pid")).into()],
			)],
		)))
		.limit(1)
		.to_string(PostgresQueryBuilder);
	tokio::time::timeout(std::time::Duration::from_secs(10), async {
		loop {
			if let Some(pid) = sqlx::query_scalar(&query)
				.fetch_optional(pool)
				.await
				.unwrap()
			{
				return pid;
			}
			tokio::time::sleep(std::time::Duration::from_millis(10)).await;
		}
	})
	.await
	.expect("expected a connection waiting for the controlled transaction")
}

#[rstest::rstest]
#[tokio::test]
async fn interrupted_patch_keeps_old_manifest_and_reclaims_only_abandoned_objects(
	#[future] capability_fixture: CoreFixture,
	#[from(worker_control)] control_1: WorkerControl,
) {
	use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
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
	.fetch_one(c.f.store.pool.driver())
	.await
	.unwrap();
	// The first object is fsynced; reserving the second object fails. The real
	// request must roll back publication and its quota reservation together.
	let quota = c.f.store.capabilities.0.retained_bytes as i64 - 3;
	let quota_query = Query::update()
		.table(Alias::new("core_quotas"))
		.value_expr(Alias::new("used_bytes"), Expr::cust("$1"))
		.and_where(
			reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("tenant")))
				.eq(Expr::cust("'acme'")),
		)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&quota_query)
		.bind(quota)
		.execute(c.f.store.pool.driver())
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
		.execute(c.f.store.pool.driver())
		.await
		.unwrap();
	let known: Vec<Uuid> = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("core_objects"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(c.f.store.pool.driver())
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
	let mut tx = c.f.store.pool.driver().begin().await.unwrap();
	{
		let query_bind_1 = format!("core-object:{live}");
		sqlx::query(
			&Query::select()
				.expr(SimpleExpr::CustomWithExpr(
					"(pg_advisory_xact_lock(hashtextextended(?, 0)))".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await
	}
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
	let WorkerControl { stop, receiver: rx } = control_1;
	// Act: collect abandoned bytes while the test holds the allocating transaction lock.
	let worker = tokio::spawn(aidash_server::capabilities::operations::run(
		c.f.store.clone(),
		rx,
	));
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
	use reinhardt::query::{Alias, ColumnRef, Expr, PostgresQueryBuilder, Query};
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
	let base: aidash_server::capabilities::contracts::Area = {
		let query_bind_1 = base_id;
		aidash_server::database::native::query_as(
			&Query::select()
				.column(ColumnRef::Asterisk)
				.from(Alias::new("core_areas"))
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						),
					),
				)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(c.f.store.pool.driver())
		.await
	}
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
			.from_subquery(
				((1..=14).map(|index| Expr::cust(format!("${index}")))).fold(
					reinhardt::query::Query::select(),
					|mut select, expr| {
						select.expr(expr);
						select
					},
				),
			)
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
			.execute(c.f.store.pool.driver())
			.await
			.unwrap();
	}
	let areas: Vec<(Uuid, i64)> = {
		let query_bind_1 = workspace;
		let query_bind_2 = thread_id;
		let query_bind_3 = "alice";
		sqlx::query_as(
			&Query::select()
				.columns([Alias::new("id"), Alias::new("revision")])
				.from(Alias::new("core_areas"))
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("workspace_id"))).eq(
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						),
					),
				)
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("thread_id"))).eq(
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						),
					),
				)
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("owner"))).eq(
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						),
					),
				)
				.order_by(Alias::new("id"), reinhardt::query::Order::Asc)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_all(c.f.store.pool.driver())
		.await
	}
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
async fn thread_run_waiting_behind_deletion_cannot_create_an_area(
	#[future] capability_fixture: CoreFixture,
) {
	use reinhardt::query::{Alias, Expr, LockType, PostgresQueryBuilder, Query};
	let c = Box::pin(capability_fixture).await;
	let workspace = c.f.store.task(c.task).await.unwrap().workspace_id;
	let root = source(&c, workspace, "Serialize run creation with deletion").await;
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

	// Hold the serialization row while the delete request queues first. The
	// later run request must wait behind it, then observe the committed tombstone.
	let mut blocker = c.f.store.pool.driver().begin().await.unwrap();
	let lock_thread = Query::select()
		.column(Alias::new("id"))
		.from(Alias::new("channel_threads"))
		.and_where(
			reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(Expr::cust("$1")),
		)
		.lock(LockType::Update)
		.to_string(PostgresQueryBuilder);
	let locked: Uuid = sqlx::query_scalar(&lock_thread)
		.bind(thread_id)
		.fetch_one(&mut *blocker)
		.await
		.unwrap();
	assert_eq!(locked, thread_id);

	let delete_app = c.app.clone();
	let delete_token = c.token.clone();
	let delete_path = format!("/api/workspaces/{workspace}/threads/{thread_id}/delete");
	let deleting = tokio::spawn(async move {
		request(
			&delete_app,
			&delete_token,
			"POST",
			&delete_path,
			json!({"idempotency_key":Uuid::new_v4(),"files":[]}),
		)
		.await
	});
	wait_for_channel_thread_lock_waiters(c.f.store.pool.driver(), 1).await;

	let run_app = c.app.clone();
	let run_token = c.token.clone();
	let run_path = format!("/api/workspaces/{workspace}/threads/{thread_id}/agents/research/runs");
	let creating = tokio::spawn(async move {
		request(
			&run_app,
			&run_token,
			"POST",
			&run_path,
			json!({"idempotency_key":Uuid::new_v4(),"agent_version":"1.1.0","title":"Must not run","description":"The thread is being deleted"}),
		)
		.await
	});
	wait_for_channel_thread_lock_waiters(c.f.store.pool.driver(), 2).await;
	blocker.commit().await.unwrap();

	let (delete_status, deleted) = deleting.await.unwrap();
	assert_eq!(delete_status, 200, "{deleted}");
	assert_eq!(deleted["state"], "deleted");
	let (run_status, failure) = creating.await.unwrap();
	assert_eq!(run_status, 404, "{failure}");

	let count_query = Query::select()
		.expr(Expr::cust("count(*)"))
		.from(Alias::new("core_areas"))
		.and_where(
			reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("workspace_id")))
				.eq(Expr::cust("$1")),
		)
		.and_where(
			reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("thread_id")))
				.eq(Expr::cust("$2")),
		)
		.to_string(PostgresQueryBuilder);
	let areas: i64 = sqlx::query_scalar(&count_query)
		.bind(workspace)
		.bind(thread_id)
		.fetch_one(c.f.store.pool.driver())
		.await
		.unwrap();
	assert_eq!(areas, 0, "a tombstoned thread cannot gain a new area");
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn thread_run_rejects_a_hundredth_queued_run(#[future] capability_fixture: CoreFixture) {
	let c = Box::pin(capability_fixture).await;
	let workspace = c.f.store.task(c.task).await.unwrap().workspace_id;
	let root = source(&c, workspace, "Bound thread run queue admission").await;
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
		json!({"idempotency_key":Uuid::new_v4(),"agent_version":"1.1.0","title":"First run","description":"Create the area"}),
	)
	.await;
	assert_eq!(status, 200, "{area}");
	let area_id: Uuid = serde_json::from_value(area["id"].clone()).unwrap();
	for index in 0..99 {
		let (status, queued) = request(
			&c.app,
			&c.token,
			"POST",
			&format!("/api/working-areas/{area_id}/queue"),
			json!({"idempotency_key":Uuid::new_v4(),"agent_version":"1.1.0","title":format!("Queued run {index}"),"description":format!("Run {index}")}),
		)
		.await;
		assert_eq!(status, 200, "run {index}: {queued}");
	}
	let (status, rejected) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/workspaces/{workspace}/threads/{thread_id}/agents/research/runs"),
		json!({"idempotency_key":Uuid::new_v4(),"agent_version":"1.1.0","title":"Overflow run","description":"Must not create run 101"}),
	)
	.await;
	assert_eq!(status, 409, "{rejected}");
	assert_eq!(rejected["error"]["code"], "QUEUE_LIMIT");
	let (status, session) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/working-areas/{area_id}/session"),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{session}");
	assert_eq!(session["queue"].as_array().unwrap().len(), 100);
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
	c.app.context.set_singleton(c.f.clone());
	c
}
#[rstest::rstest]
#[tokio::test]
async fn expired_upload_releases_only_its_owned_staging_bytes(
	#[future] short_staging: CoreFixture,
	#[from(worker_control)] control_1: WorkerControl,
) {
	use base64::Engine;
	use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
	let c = Box::pin(short_staging).await;
	let bytes = b"expired original upload";
	let (status,upload)=request(&c.app,&c.token,"POST","/api/references/uploads",json!({"idempotency_key":Uuid::new_v4(),"name":"incomplete.txt","media_type":"text/plain","size":bytes.len(),"digest":aidash_server::capabilities::objects::digest(bytes)})).await;
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
	.fetch_one(c.f.store.pool.driver())
	.await
	.unwrap();
	let object_path = c.root.join(object.simple().to_string());
	assert!(object_path.exists());
	let WorkerControl { stop, receiver: rx } = control_1;
	// Act: collect only after the incomplete upload has written its staging bytes.
	let worker = tokio::spawn(aidash_server::capabilities::operations::run(
		c.f.store.clone(),
		rx,
	));
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
	loop {
		let used: Option<i64> = sqlx::query_scalar(
			&Query::select()
				.column(Alias::new("used_bytes"))
				.from(Alias::new("core_quotas"))
				.and_where(Expr::col(Alias::new("tenant")).eq("acme"))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(c.f.store.pool.driver())
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
	#[from(worker_control)] control_1: WorkerControl,
) {
	use reinhardt::query::{Alias, Expr, IntoIden, PostgresQueryBuilder, Query, SimpleExpr};
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
	// SeaQuery cannot express PostgreSQL trigger/function DDL. This fixture-only
	// barrier pauses the real worker while reporting its storage failure, so
	// status polling deterministically overlaps the failure transaction.
	let barrier_key = Uuid::new_v4().as_u128() as i64;
	sqlx::query(&format!("CREATE FUNCTION pause_cleanup_failure() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock({barrier_key}); RETURN NEW; END; $$"))
		.execute(c.f.store.pool.driver()).await.unwrap();
	sqlx::query("CREATE TRIGGER pause_cleanup_failure BEFORE UPDATE OF state ON core_records FOR EACH ROW WHEN (NEW.state = 'cleanup_failed') EXECUTE FUNCTION pause_cleanup_failure()")
		.execute(c.f.store.pool.driver()).await.unwrap();
	let mut barrier = c.f.store.pool.driver().begin().await.unwrap();
	sqlx::query(
		&Query::select()
			.expr(SimpleExpr::FunctionCall(
				Alias::new("pg_advisory_xact_lock").into_iden(),
				vec![Expr::value(barrier_key).into()],
			))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *barrier)
	.await
	.unwrap();
	let barrier_pid: i32 = sqlx::query_scalar(
		&Query::select()
			.expr(SimpleExpr::FunctionCall(
				Alias::new("pg_backend_pid").into_iden(),
				vec![],
			))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&mut *barrier)
	.await
	.unwrap();
	let WorkerControl { stop, receiver: rx } = control_1;
	// Act: reconcile cleanup after installing and locking the failure barrier.
	let worker = tokio::spawn(aidash_server::capabilities::operations::run(
		c.f.store.clone(),
		rx,
	));
	let status_path = format!(
		"/api/file-cleanups/{}",
		deletion["operation_id"].as_str().unwrap()
	);
	let failure_pid = wait_for_blocked_connection(c.f.store.pool.driver(), barrier_pid).await;
	let app = c.app.clone();
	let token = c.token.clone();
	let polling_path = status_path.clone();
	let polling =
		tokio::spawn(async move { request(&app, &token, "GET", &polling_path, Value::Null).await });
	wait_for_blocked_connection(c.f.store.pool.driver(), failure_pid).await;
	barrier.rollback().await.unwrap();
	let (status, state) = polling.await.unwrap();
	assert_eq!(
		status, 200,
		"status polling must not deadlock failure reporting: {state}"
	);
	assert_eq!(
		state["state"], "cleanup_failed",
		"failure reporting must commit before the waiting status read"
	);
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

#[rstest::rstest]
#[tokio::test]
async fn cleanup_status_does_not_block_the_worker_while_waiting_for_its_area(
	#[future] capability_fixture: CoreFixture,
) {
	use reinhardt::query::{
		Alias, Expr, IntoIden, LockBehavior, LockType, PostgresQueryBuilder, Query, SimpleExpr,
	};
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
	let area_id: Uuid = serde_json::from_value(area["id"].clone()).unwrap();
	let (status, cleanup) = request(&c.app, &c.token, "POST", &format!("/api/working-areas/{area_id}/cleanup"), json!({"idempotency_key":Uuid::new_v4(),"expected_revision":area["revision"],"choice":"recoverable"})).await;
	assert_eq!(status, 200, "{cleanup}");
	let operation_id: Uuid = serde_json::from_value(cleanup["operation_id"].clone()).unwrap();
	// Pause a worker transaction after it has locked the area, before its record.
	let mut worker = c.f.store.pool.driver().begin().await.unwrap();
	let lock = Query::select()
		.column(Alias::new("id"))
		.from(Alias::new("core_areas"))
		.and_where(
			reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(Expr::cust("$1")),
		)
		.lock(LockType::Update)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&lock)
		.bind(area_id)
		.fetch_one(&mut *worker)
		.await
		.unwrap();
	let pid: i32 = sqlx::query_scalar(
		&Query::select()
			.expr(SimpleExpr::FunctionCall(
				Alias::new("pg_backend_pid").into_iden(),
				vec![],
			))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&mut *worker)
	.await
	.unwrap();
	let app = c.app.clone();
	let token = c.token.clone();
	let polling = tokio::spawn(async move {
		request(
			&app,
			&token,
			"GET",
			&format!("/api/file-cleanups/{operation_id}"),
			Value::Null,
		)
		.await
	});
	wait_for_blocked_connection(c.f.store.pool.driver(), pid).await;
	let lock = Query::select()
		.column(Alias::new("id"))
		.from(Alias::new("core_records"))
		.and_where(
			reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(Expr::cust("$1")),
		)
		.lock(LockType::Update)
		.lock_behavior(LockBehavior::Nowait)
		.to_string(PostgresQueryBuilder);
	let acquired = sqlx::query(&lock)
		.bind(operation_id)
		.fetch_one(&mut *worker)
		.await;
	worker.rollback().await.unwrap();
	let (status, value) = polling.await.unwrap();
	assert!(
		acquired.is_ok(),
		"status must not hold the cleanup record while waiting for the worker: {acquired:?}"
	);
	assert_eq!(status, 200, "{value}");
	assert_eq!(value["operation_id"], cleanup["operation_id"]);
	c.close().await;
}

use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};
