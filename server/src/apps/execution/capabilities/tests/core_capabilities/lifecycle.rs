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
			"/api/execution-scopes/{}/session",
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
			"/api/execution-scopes/{}/session",
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

use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};
