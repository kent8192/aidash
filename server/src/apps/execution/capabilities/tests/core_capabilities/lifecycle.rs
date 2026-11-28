use super::*;
use aidash_application::tenancy::TEST_FIXTURE;

async fn pending_question(c: &CoreFixture, run: Uuid, thread: Uuid) -> Uuid {
	let id = Uuid::new_v4();
	sqlx::query(
		"INSERT INTO human_requests (id, tenant, thread_id, run_id, kind, prompt, request_key) \
		 VALUES ($1, 'acme', $2, $3, 'QUESTION', 'Continue?', 'pending-question')",
	)
	.bind(id)
	.bind(thread)
	.bind(run)
	.execute(c.f.store.pool.fixture_driver())
	.await
	.unwrap();
	id
}

async fn assert_question_closed(c: &CoreFixture, id: Uuid) {
	let closed: (Option<Value>, Option<String>) =
		sqlx::query_as("SELECT response, answered_by FROM human_requests WHERE id = $1")
			.bind(id)
			.fetch_one(c.f.store.pool.fixture_driver())
			.await
			.unwrap();
	assert_eq!(
		closed,
		(
			Some(json!({"answered":false,"reason":"run_ended"})),
			Some("system".into())
		),
	);
}

fn blob_path(c: &CoreFixture, digest: &str) -> std::path::PathBuf {
	let tenant = aidash_server::capabilities::objects::digest(b"acme");
	c.root
		.join("tenants")
		.join(tenant)
		.join("sha256")
		.join(&digest[..2])
		.join(&digest[2..4])
		.join(digest)
}

/// Holds the Thread row lock that Thread deletion, a Thread run and Task
/// admission all take, and returns the holding connection's pid.
pub(super) async fn hold_thread_lock(
	pool: &sqlx::PgPool,
	thread: Uuid,
) -> (sqlx::Transaction<'static, sqlx::Postgres>, i32) {
	use reinhardt::query::{
		Alias, Expr, IntoIden, LockType, PostgresQueryBuilder, Query, SimpleExpr,
	};
	let mut blocker = pool.begin().await.unwrap();
	let lock = Query::select()
		.column(Alias::new("id"))
		.from(Alias::new("threads"))
		.and_where(Expr::col(Alias::new("id")).eq(Expr::value(thread)))
		.lock(LockType::Update)
		.to_string(PostgresQueryBuilder);
	let locked: Uuid = sqlx::query_scalar(&lock)
		.fetch_one(&mut *blocker)
		.await
		.unwrap();
	assert_eq!(locked, thread);
	let pid: i32 = sqlx::query_scalar(
		&Query::select()
			.expr(SimpleExpr::FunctionCall(
				Alias::new("pg_backend_pid").into_iden(),
				vec![],
			))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&mut *blocker)
	.await
	.unwrap();
	(blocker, pid)
}

pub(super) async fn wait_for_blocked_connection(pool: &sqlx::PgPool, blocker: i32) -> i32 {
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
	let path = format!("/api/executions/{}/patch", run.id);
	let (status, result) = request(&c.app,&c.token,"POST",&path,json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"preconditions":{"keep.txt":null},"patch":"*** Begin Patch\n*** Add File: keep.txt\n+kept 東京\n*** End Patch"})).await;
	assert_eq!(status, 200, "{result}");
	let (_, before) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/executions/{}/working-area", run.id),
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
	.fetch_one(c.f.store.pool.fixture_driver())
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
		.execute(c.f.store.pool.fixture_driver())
		.await
		.unwrap();
	let (status,failed)=request(&c.app,&c.token,"POST",&path,json!({"idempotency_key":Uuid::new_v4(),"expected_revision":2,"preconditions":{"a.txt":null,"b.txt":null},"patch":"*** Begin Patch\n*** Add File: a.txt\n+a\n*** Add File: b.txt\n+b\n*** End Patch"})).await;
	assert_eq!(status, 409, "{failed}");
	assert_eq!(failed["error"]["code"], "STORAGE_QUOTA");
	let (_, after) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/executions/{}/working-area", run.id),
		Value::Null,
	)
	.await;
	assert_eq!(before, after, "no partial patch may become visible");
	sqlx::query(&quota_query)
		.bind(used)
		.execute(c.f.store.pool.fixture_driver())
		.await
		.unwrap();
	let known: Vec<Uuid> = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("core_objects"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(c.f.store.pool.fixture_driver())
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
	// Dropping the failed upload schedules its own release, which may already
	// have removed its spool. Whatever remains must still be collected below;
	// the live object exercises the collector either way.
	// A transaction still allocating another object owns the same advisory lock
	// as production. The collector must leave it untouched until rollback.
	let live = Uuid::new_v4();
	let mut tx = c.f.store.pool.fixture_driver().begin().await.unwrap();
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
	let temporary_intent = c.root.join(".intents").join(format!(".{live}.new"));
	tokio::fs::write(&temporary_intent, b"in progress")
		.await
		.unwrap();
	let WorkerControl { stop, receiver: rx } = control_1;
	// Act: collect abandoned bytes while the test holds the allocating transaction lock.
	let worker = tokio::spawn(aidash_server::capabilities::operations::run(
		c.f.store.clone(),
		rx,
	));
	let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
	while abandoned.iter().any(|path| path.exists()) {
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
	assert!(
		temporary_intent.exists(),
		"an active temporary intent must remain"
	);
	tx.rollback().await.unwrap();
	while live_path.exists() || temporary_intent.exists() {
		assert!(
			tokio::time::Instant::now() < deadline,
			"rolled-back bytes not reclaimed"
		);
		tokio::time::sleep(std::time::Duration::from_millis(50)).await;
	}

	let (status, read) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/executions/{}/files/read", run.id),
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
async fn thread_run_waiting_behind_deletion_cannot_create_an_area(
	#[future] capability_fixture: CoreFixture,
) {
	use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
	let c = Box::pin(capability_fixture).await;
	let thread =
		c.f.store
			.task(&TEST_FIXTURE, c.task)
			.await
			.unwrap()
			.thread_id;

	// Hold the Thread row while the delete request queues first. The later
	// run request must wait behind it, then observe the committed deletion.
	let (blocker, blocker_pid) = hold_thread_lock(c.f.store.pool.fixture_driver(), thread).await;

	let delete_app = c.app.clone();
	let delete_token = c.token.clone();
	let delete_path = format!("/api/threads/{thread}/delete");
	let deleting = tokio::spawn(async move {
		request(
			&delete_app,
			&delete_token,
			"POST",
			&delete_path,
			json!({"idempotency_key":Uuid::new_v4()}),
		)
		.await
	});
	let delete_pid =
		wait_for_blocked_connection(c.f.store.pool.fixture_driver(), blocker_pid).await;

	let run_app = c.app.clone();
	let run_token = c.token.clone();
	let run_path = format!("/api/threads/{thread}/agents/research/executions");
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
	wait_for_blocked_connection(c.f.store.pool.fixture_driver(), delete_pid).await;
	blocker.commit().await.unwrap();

	let (delete_status, deleted) = deleting.await.unwrap();
	assert_eq!(delete_status, 200, "{deleted}");
	assert_eq!(deleted["state"], "deleted");
	let (run_status, failure) = creating.await.unwrap();
	assert_eq!(run_status, 404, "{failure}");

	let count_query = Query::select()
		.expr(Expr::cust("count(*)"))
		.from(Alias::new("core_execution_scopes"))
		.and_where(
			reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("thread_id")))
				.eq(Expr::cust("$1")),
		)
		.to_string(PostgresQueryBuilder);
	let areas: i64 = sqlx::query_scalar(&count_query)
		.bind(thread)
		.fetch_one(c.f.store.pool.fixture_driver())
		.await
		.unwrap();
	assert_eq!(areas, 0, "a deleted thread cannot gain a new area");
	c.close().await;
}

#[rstest::rstest]
#[tokio::test]
async fn deleting_a_thread_schedules_the_scratch_wipe_and_closes_its_records(
	#[future] capability_fixture: CoreFixture,
) {
	let c = Box::pin(capability_fixture).await;
	let thread =
		c.f.store
			.task(&TEST_FIXTURE, c.task)
			.await
			.unwrap()
			.thread_id;
	let marker = "This thread will be deleted";
	source(&c, thread, marker).await;
	let (status,area)=request(&c.app,&c.token,"POST",&format!("/api/threads/{thread}/agents/research/executions"),json!({"idempotency_key":Uuid::new_v4(),"agent_version":"1.1.0","title":"Thread-bound work","description":"Preserve files"})).await;
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
	let question = pending_question(&c, run.parse().unwrap(), thread).await;
	let path = format!("/api/threads/{thread}/delete");
	let input = json!({"idempotency_key":Uuid::new_v4()});
	let (status, deleted) = request(&c.app, &c.token, "POST", &path, input.clone()).await;
	assert_eq!(status, 200, "{deleted}");
	assert_question_closed(&c, question).await;
	assert_eq!(
		request(&c.app, &c.token, "POST", &path, input).await,
		(200, deleted)
	);
	// The deleted Thread is gone for everyone, its messages with it.
	let (status, history) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/threads/{thread}/message-history"),
		Value::Null,
	)
	.await;
	assert!(matches!(status, 403 | 404), "{history}");
	assert!(!history.to_string().contains(marker));
	// Every other read and write of it refuses too: its snapshot, a new message, a new Task
	// and the Runs of a Task it held (#1008).
	for (method, path, body) in [
		("GET", format!("/api/threads/{thread}"), Value::Null),
		(
			"POST",
			format!("/api/threads/{thread}/channel-messages"),
			json!({"content":"after deletion","idempotency_key":Uuid::new_v4()}),
		),
		(
			"POST",
			format!("/api/threads/{thread}/tasks"),
			json!({"title":"After deletion","description":"Must be refused"}),
		),
		(
			"GET",
			format!("/api/tasks/{}/executions", c.task),
			Value::Null,
		),
	] {
		let (status, body) = request(&c.app, &c.token, method, &path, body).await;
		assert!(
			matches!(status, 403 | 404),
			"{method} {path}: {status} {body}"
		);
	}
	let pending: bool =
		sqlx::query_scalar("SELECT scratch_wipe_requested FROM threads WHERE id = $1")
			.bind(thread)
			.fetch_one(c.f.store.pool.fixture_driver())
			.await
			.unwrap();
	assert!(pending, "the Thread is queued for removal from Scratch");
	c.close().await;
}

/// Deleting a Thread stops everyone's work in it (#1008): a Node
/// administrator who owns nothing in the Thread deletes it, and the owner's
/// execution there is cancelled and its Task, like every open Task of the
/// Thread, ended.
#[rstest::rstest]
#[tokio::test]
async fn a_node_administrator_deleting_a_thread_stops_its_owners_work(
	#[future] capability_fixture: CoreFixture,
) {
	// Arrange: alice's execution in her Thread; bob administers its Node.
	let c = Box::pin(capability_fixture).await;
	let thread =
		c.f.store
			.task(&TEST_FIXTURE, c.task)
			.await
			.unwrap()
			.thread_id;
	let (status, area) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/threads/{thread}/agents/research/executions"),
		json!({"idempotency_key":Uuid::new_v4(),"agent_version":"1.1.0","title":"Live work","description":"Still running"}),
	)
	.await;
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
	let run: Uuid = serde_json::from_value(session["active_run_id"].clone()).unwrap();
	let task: Uuid = sqlx::query_scalar("SELECT task_id FROM task_executions WHERE id = $1")
		.bind(run)
		.fetch_one(c.f.store.pool.fixture_driver())
		.await
		.unwrap();
	let mut policy = c.policy.clone();
	policy["subjects"]["bob"] = json!({"kind": "user"});
	common::replace_policy(&c.f.store, "acme", policy).await;
	let bob = common::issued_credential(&c.f.store, "acme", "bob").await;
	// Act: bob owns no Area in the Thread, so he chooses no files.
	let (status, deleted) = request(
		&c.app,
		bob["token"].as_str().unwrap(),
		"POST",
		&format!("/api/threads/{thread}/delete"),
		json!({"idempotency_key":Uuid::new_v4()}),
	)
	.await;
	// Assert
	assert_eq!(status, 200, "{deleted}");
	let control: String = sqlx::query_scalar("SELECT control FROM task_executions WHERE id = $1")
		.bind(run)
		.fetch_one(c.f.store.pool.fixture_driver())
		.await
		.unwrap();
	assert_eq!(control, "CANCELLED");
	let open: Vec<(Uuid, String)> = sqlx::query_as(
		"SELECT id, status FROM tasks WHERE thread_id = $1 AND status NOT IN ('COMPLETED', 'FAILED', 'CANCELLED', 'ABANDONED')",
	)
	.bind(thread)
	.fetch_all(c.f.store.pool.fixture_driver())
	.await
	.unwrap();
	assert!(open.is_empty(), "{open:?}");
	let ended: String = sqlx::query_scalar("SELECT status FROM tasks WHERE id = $1")
		.bind(task)
		.fetch_one(c.f.store.pool.fixture_driver())
		.await
		.unwrap();
	assert_eq!(ended, "CANCELLED");
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
	let digest: String = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("digest"))
			.from(Alias::new("core_objects"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(c.f.store.pool.fixture_driver())
	.await
	.unwrap();
	let object_path = blob_path(&c, &digest);
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
		.fetch_optional(c.f.store.pool.fixture_driver())
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
	let (status,result)=request(&c.app,&c.token,"POST",&format!("/api/executions/{}/patch",run.id),json!({"idempotency_key":Uuid::new_v4(),"expected_revision":1,"preconditions":{"a.txt":null,"b.txt":null},"patch":"*** Begin Patch\n*** Add File: a.txt\n+first 東京\n*** Add File: b.txt\n+second\n*** End Patch"})).await;
	assert_eq!(status, 200, "{result}");
	let (status, area) = request(
		&c.app,
		&c.token,
		"GET",
		&format!("/api/executions/{}/working-area", run.id),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{area}");
	(c, area)
}

#[rstest::rstest]
#[tokio::test]
async fn cleanup_retries_when_blob_lock_is_held(
	#[future] cleanup_fault_fixture: (CoreFixture, Value),
) {
	use reinhardt::query::{Alias, Expr, LockType, PostgresQueryBuilder, Query, SimpleExpr};
	let (c, area) = Box::pin(cleanup_fault_fixture).await;
	let id = area["id"].as_str().unwrap();
	let digest = area["manifest"][1]["digest"].as_str().unwrap();
	let owned = blob_path(&c, digest);
	let backup = c.root.join("lock-test-backup");
	let (status, confirmation) = request(
		&c.app,
		&c.token,
		"POST",
		&format!("/api/working-areas/{id}/deletion-confirmation"),
		json!({"expected_revision":area["revision"]}),
	)
	.await;
	assert_eq!(status, 200, "{confirmation}");
	let (status, deletion) = request(&c.app, &c.token, "POST", &format!("/api/working-areas/{id}/cleanup"), json!({"idempotency_key":Uuid::new_v4(),"expected_revision":area["revision"],"choice":"irreversible","confirmation_id":confirmation["confirmation_id"]})).await;
	assert_eq!(status, 200, "{deletion}");
	tokio::fs::rename(&owned, &backup).await.unwrap();
	tokio::fs::create_dir(&owned).await.unwrap();
	let path = format!(
		"/api/file-cleanups/{}/reconcile",
		deletion["operation_id"].as_str().unwrap()
	);
	// Stage reference removal and commit it before forcing the lock collision.
	let (status, result) = request(&c.app, &c.token, "POST", &path, json!({})).await;
	assert!(
		status >= 400,
		"directory must prevent reclamation: {result}"
	);
	let mut lock = c.f.store.pool.fixture_driver().begin().await.unwrap();
	let query = Query::select()
		.expr(SimpleExpr::CustomWithExpr(
			"pg_advisory_xact_lock(hashtextextended(?, 0))".into(),
			vec![Expr::value(format!("core-blob:acme:{digest}")).into()],
		))
		.to_string(PostgresQueryBuilder);
	sqlx::query(&query).execute(&mut *lock).await.unwrap();
	let (status, result) = request(&c.app, &c.token, "POST", &path, json!({})).await;
	assert_eq!(status, 200, "{result}");
	assert!(owned.is_dir());
	assert_eq!(
		result["state"], "deleting",
		"locked blob must remain pending while its bytes exist"
	);
	lock.rollback().await.unwrap();
	// A row skipped by FOR UPDATE is also busy, not a missing blob.
	let mut row_lock = c.f.store.pool.fixture_driver().begin().await.unwrap();
	let query = Query::select()
		.column(Alias::new("size"))
		.from(Alias::new("core_blobs"))
		.and_where(Expr::col(Alias::new("tenant")).eq("acme"))
		.and_where(Expr::col(Alias::new("digest")).eq(digest))
		.lock(LockType::Update)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&query).fetch_one(&mut *row_lock).await.unwrap();
	let (status, result) = request(&c.app, &c.token, "POST", &path, json!({})).await;
	assert_eq!(status, 200, "{result}");
	assert_eq!(result["state"], "deleting");
	assert!(owned.is_dir());
	row_lock.rollback().await.unwrap();
	tokio::fs::remove_dir(&owned).await.unwrap();
	tokio::fs::rename(&backup, &owned).await.unwrap();
	let (status, result) = request(&c.app, &c.token, "POST", &path, json!({})).await;
	assert_eq!(status, 200, "{result}");
	assert_eq!(result["state"], "deleted");
	assert!(!owned.exists());
	c.close().await;
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
	let owned = blob_path(&c, area["manifest"][1]["digest"].as_str().unwrap());
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
	// Deterministic cleanup failure after the durable intent fenced every
	// writer: the trigger below makes the reference removal fail inside the
	// worker's erasure, so it is a database failure, not a storage one. The
	// directory at the blob path only makes any byte deletion that does run
	// fail too. No unrelated path is modified.
	tokio::fs::rename(&owned, &backup).await.unwrap();
	tokio::fs::create_dir(&owned).await.unwrap();
	// SeaQuery cannot express PostgreSQL trigger/function DDL. This fixture-only
	// barrier pauses the real worker while reporting the failed erasure, so
	// status polling deterministically overlaps the failure transaction.
	let barrier_key = Uuid::new_v4().as_u128() as i64;
	// Only the table owner can add a trigger; the server role holds DML.
	let owner = common::owner_pool(c.f.store.pool.fixture_driver()).await;
	// Keep reference removal from committing until failure reporting is observed.
	// This leaves no unreferenced blob for the concurrent reclamation loop.
	sqlx::query("CREATE FUNCTION fail_cleanup_erasure() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'controlled cleanup failure'; END; $$")
		.execute(&owner).await.unwrap();
	sqlx::query("CREATE TRIGGER fail_cleanup_erasure BEFORE DELETE ON core_objects FOR EACH ROW EXECUTE FUNCTION fail_cleanup_erasure()")
		.execute(&owner).await.unwrap();
	sqlx::query(&format!("CREATE FUNCTION pause_cleanup_failure() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock({barrier_key}); RETURN NEW; END; $$"))
		.execute(&owner).await.unwrap();
	sqlx::query("CREATE TRIGGER pause_cleanup_failure BEFORE UPDATE OF state ON core_records FOR EACH ROW WHEN (NEW.state = 'cleanup_failed') EXECUTE FUNCTION pause_cleanup_failure()")
		.execute(&owner).await.unwrap();
	let mut barrier = c.f.store.pool.fixture_driver().begin().await.unwrap();
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
	let failure_pid =
		wait_for_blocked_connection(c.f.store.pool.fixture_driver(), barrier_pid).await;
	let app = c.app.clone();
	let token = c.token.clone();
	let polling_path = status_path.clone();
	let polling =
		tokio::spawn(async move { request(&app, &token, "GET", &polling_path, Value::Null).await });
	wait_for_blocked_connection(c.f.store.pool.fixture_driver(), failure_pid).await;
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
	sqlx::query("DROP TRIGGER fail_cleanup_erasure ON core_objects")
		.execute(&owner)
		.await
		.unwrap();
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
		&format!("/api/executions/{}/working-area", run.id),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{area}");
	let area_id: Uuid = serde_json::from_value(area["id"].clone()).unwrap();
	let question = pending_question(&c, run.id, run.thread_id).await;
	let (status, cleanup) = request(&c.app, &c.token, "POST", &format!("/api/working-areas/{area_id}/cleanup"), json!({"idempotency_key":Uuid::new_v4(),"expected_revision":area["revision"],"choice":"recoverable"})).await;
	assert_eq!(status, 200, "{cleanup}");
	assert_question_closed(&c, question).await;
	let operation_id: Uuid = serde_json::from_value(cleanup["operation_id"].clone()).unwrap();
	// Pause a worker transaction after it has locked the area, before its record.
	let mut worker = c.f.store.pool.fixture_driver().begin().await.unwrap();
	let lock = Query::select()
		.column(Alias::new("id"))
		.from(Alias::new("core_execution_scopes"))
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
	wait_for_blocked_connection(c.f.store.pool.fixture_driver(), pid).await;
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
