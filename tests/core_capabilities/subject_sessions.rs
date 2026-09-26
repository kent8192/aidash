use super::*;
use sea_orm::sea_query::{Alias, Expr, PostgresQueryBuilder, Query};

struct SharedThread {
	c: CoreFixture,
	workspace: Uuid,
	thread: Uuid,
	bob: String,
	areas: Vec<Value>,
	runs: Vec<Uuid>,
}

#[rstest::fixture]
fn shared_thread(
	#[future] capability_fixture: CoreFixture,
) -> impl std::future::Future<Output = SharedThread> {
	let capability_fixture = Box::pin(capability_fixture);
	async move {
		let c = capability_fixture.await;
		let workspace = c.f.store.task(c.task).await.unwrap().workspace_id;
		let root = source(&c, workspace, "Shared conversation; private working files").await;
		let (_, thread) = request(
			&c.app,
			&c.token,
			"POST",
			&format!("/api/workspaces/{workspace}/threads"),
			json!({"root_message_id":root}),
		)
		.await;
		let thread: Uuid = serde_json::from_value(thread["id"].clone()).unwrap();
		let (_, credential) = request(
			&c.app,
			&c.f.config.api_token,
			"POST",
			"/api/authorization/acme/credentials",
			json!({"subject":"bob"}),
		)
		.await;
		let bob = credential["token"].as_str().unwrap().to_owned();
		let mut areas = vec![];
		let mut runs = vec![];
		for (owner, token) in [("alice", &c.token), ("bob", &bob)] {
			let (status, area) = request(&c.app, token, "POST", &format!("/api/workspaces/{workspace}/threads/{thread}/agents/research/runs"), json!({"idempotency_key":Uuid::new_v4(),"agent_version":"1.1.0","title":"Private work","description":"One owner's files"})).await;
			assert_eq!(status, 200, "{owner}: {area}");
			let (_, session) = request(
				&c.app,
				token,
				"GET",
				&format!(
					"/api/working-areas/{}/session",
					area["id"].as_str().unwrap()
				),
				Value::Null,
			)
			.await;
			let run: Uuid = serde_json::from_value(session["active_run_id"].clone()).unwrap();
			let (status, patch) = request(&c.app, token, "POST", &path_for_patch(run), json!({"idempotency_key":Uuid::new_v4(),"expected_revision":area["revision"],"preconditions":{"private.txt":null},"patch":format!("*** Begin Patch\n*** Add File: private.txt\n+{owner}-only\n*** End Patch")})).await;
			assert_eq!(status, 200, "{patch}");
			let (_, area) = request(
				&c.app,
				token,
				"GET",
				&format!("/api/runs/{run}/working-area"),
				Value::Null,
			)
			.await;
			areas.push(area);
			runs.push(run);
		}
		SharedThread {
			c,
			workspace,
			thread,
			bob,
			areas,
			runs,
		}
	}
}

#[rstest::rstest]
#[tokio::test]
async fn same_thread_agent_keeps_subject_sessions_private_and_deletion_choices_scoped(
	#[future] shared_thread: SharedThread,
) {
	let f = Box::pin(shared_thread).await;
	assert_ne!(f.areas[0]["id"], f.areas[1]["id"]);
	assert_ne!(f.runs[0], f.runs[1]);
	assert_eq!(
		request(
			&f.c.app,
			&f.bob,
			"GET",
			&format!("/api/runs/{}/working-area", f.runs[0]),
			Value::Null
		)
		.await
		.0,
		404
	);
	let (_, visible) = request(
		&f.c.app,
		&f.c.token,
		"GET",
		&format!("/api/working-areas?thread_id={}", f.thread),
		Value::Null,
	)
	.await;
	assert_eq!(visible["items"].as_array().unwrap().len(), 1);
	let (status, deleted) = request(&f.c.app, &f.c.token, "POST", &format!("/api/workspaces/{}/threads/{}/delete", f.workspace, f.thread), json!({"idempotency_key":Uuid::new_v4(),"files":[{"area_id":f.areas[0]["id"],"expected_revision":f.areas[0]["revision"],"choice":"keep"}]})).await;
	assert_eq!(status, 200, "{deleted}");
	assert_eq!(deleted["file_operations"].as_array().unwrap().len(), 1);
	assert!(
		!deleted
			.to_string()
			.contains(f.areas[1]["id"].as_str().unwrap())
	);
	let (_, bob_files) = request(&f.c.app, &f.bob, "GET", "/api/working-files", Value::Null).await;
	assert_eq!(bob_files["items"][0]["area_id"], f.areas[1]["id"]);
	assert_eq!(bob_files["items"][0]["revision"], f.areas[1]["revision"]);
	let (status, read) = request(
		&f.c.app,
		&f.bob,
		"GET",
		&format!(
			"/api/working-areas/{}/files/{}/download",
			f.areas[1]["id"].as_str().unwrap(),
			f.areas[1]["manifest"][0]["file_id"].as_str().unwrap()
		),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{read}");
	use base64::Engine;
	assert_eq!(
		base64::engine::general_purpose::STANDARD
			.decode(read["data"].as_str().unwrap())
			.unwrap(),
		b"bob-only\n"
	);
	f.c.close().await;
}

#[rstest::fixture]
fn successor_fixture(
	#[future] shared_thread: SharedThread,
) -> impl std::future::Future<Output = (SharedThread, Uuid, Value)> {
	let shared_thread = Box::pin(shared_thread);
	async move {
		let f = shared_thread.await;
		let (_, queued) = request(&f.c.app, &f.c.token, "POST", &format!("/api/workspaces/{}/threads/{}/agents/research/runs", f.workspace, f.thread), json!({"idempotency_key":Uuid::new_v4(),"agent_version":"1.1.0","title":"Successor","description":"Own the next turn"})).await;
		assert_eq!(queued["id"], f.areas[0]["id"]);
		// Model the predecessor's committed terminal transition, before the
		// next Worker executes. The endpoint must honor the live queue owner.
		sqlx::query(
			&Query::update()
				.table(Alias::new("runs"))
				.value(Alias::new("phase"), "COMPLETED")
				.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
				.to_string(PostgresQueryBuilder),
		)
		.bind(f.runs[0])
		.execute(&f.c.f.store.pool)
		.await
		.unwrap();
		let (_, session) = request(
			&f.c.app,
			&f.c.token,
			"GET",
			&format!(
				"/api/working-areas/{}/session",
				f.areas[0]["id"].as_str().unwrap()
			),
			Value::Null,
		)
		.await;
		let successor: Uuid = serde_json::from_value(session["active_run_id"].clone()).unwrap();
		let file = &f.areas[0]["manifest"][0];
		let input = json!({"idempotency_key":Uuid::new_v4(),"expected_revision":f.areas[0]["revision"],"path":"copy.txt","source":{"kind":"file","file_id":file["file_id"],"expected_digest":file["digest"]}});
		(f, successor, input)
	}
}

#[rstest::rstest]
#[tokio::test]
async fn completed_run_cannot_materialize_into_its_successor_but_current_run_can(
	#[future] successor_fixture: (SharedThread, Uuid, Value),
) {
	let (f, successor, input) = Box::pin(successor_fixture).await;
	let (status, rejected) = request(
		&f.c.app,
		&f.c.token,
		"POST",
		&format!("/api/runs/{}/files/materialize", f.runs[0]),
		input.clone(),
	)
	.await;
	assert_eq!(status, 409, "{rejected}");
	assert_eq!(rejected["error"]["code"], "RUN_NOT_ACTIVE");
	let (_, before) = request(
		&f.c.app,
		&f.c.token,
		"GET",
		&format!("/api/runs/{successor}/working-area"),
		Value::Null,
	)
	.await;
	assert_eq!(before["manifest"], f.areas[0]["manifest"]);
	assert_eq!(before["revision"], f.areas[0]["revision"]);
	let (status, copied) = request(
		&f.c.app,
		&f.c.token,
		"POST",
		&format!("/api/runs/{successor}/files/materialize"),
		input,
	)
	.await;
	assert_eq!(status, 200, "{copied}");
	assert_ne!(
		copied["file"]["file_id"],
		f.areas[0]["manifest"][0]["file_id"]
	);
	f.c.close().await;
}

#[rstest::fixture]
async fn delegated_thread_child(#[future] shared_thread: SharedThread) -> (SharedThread, Uuid) {
	let mut f = Box::pin(shared_thread).await;
	let mut entry = f.c.f.registry.get("research", "1.1.0").await.unwrap();
	entry.id = "reviewer".into();
	assert_eq!(
		request(
			&f.c.app,
			&f.c.f.config.api_token,
			"POST",
			"/api/registry",
			json!(entry)
		)
		.await
		.0,
		200
	);
	f.c.policy["subjects"]
		[aidash::domain::qualified_agent(&f.c.f.config.node_id, "reviewer", "1.1.0")] =
		json!({"kind":"agent"});
	assert_eq!(
		request(
			&f.c.app,
			&f.c.f.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":2,"bundle":f.c.policy})
		)
		.await
		.0,
		200
	);
	assert_eq!(
		request(
			&f.c.app,
			&f.c.f.config.api_token,
			"POST",
			"/api/authorization/acme/catalog",
			json!({"entry":{"id":"reviewer","version":"1.1.0"},"expected_revision":0,"enabled":true})
		)
		.await
		.0,
		200
	);
	let parent = f.c.f.store.run(f.runs[1]).await.unwrap();
	let (status, child) = request(
		&f.c.app,
		&f.bob,
		"POST",
		&format!("/api/workspaces/{}/tasks", f.workspace),
		json!({"title":"Child","description":"Inherit Bob's live thread","parent_id":parent.task_id}),
	)
	.await;
	assert_eq!(status, 200, "{child}");
	let id = serde_json::from_value(child["id"].clone()).unwrap();
	(f, id)
}

#[rstest::rstest]
#[tokio::test]
async fn inherited_child_admission_waits_for_deletion_and_rechecks_tombstone(
	#[future] delegated_thread_child: (SharedThread, Uuid),
) {
	use sea_orm::sea_query::LockType;
	let (f, child) = Box::pin(delegated_thread_child).await;
	let mut blocker = f.c.f.store.pool.begin().await.unwrap();
	let _: Uuid = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("channel_threads"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::cust("$1")))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.bind(f.thread)
	.fetch_one(&mut *blocker)
	.await
	.unwrap();
	let app = f.c.app.clone();
	let token = f.c.token.clone();
	let path = format!(
		"/api/workspaces/{}/threads/{}/delete",
		f.workspace, f.thread
	);
	let input = json!({"idempotency_key":Uuid::new_v4(),"files":[{"area_id":f.areas[0]["id"],"expected_revision":f.areas[0]["revision"],"choice":"keep"}]});
	let deleting = tokio::spawn(async move { request(&app, &token, "POST", &path, input).await });
	super::lifecycle_tests::wait_for_channel_thread_lock_waiters(&f.c.f.store.pool, 1).await;
	let app = f.c.app.clone();
	let token = f.bob.clone();
	let path = format!("/api/tasks/{child}/delegate");
	let input = json!({"node_id":f.c.f.config.node_id,"agent":{"id":"reviewer","version":"1.1.0"}});
	let child_path = path.clone();
	let child_input = input.clone();
	let admitting = tokio::spawn(async move { request(&app, &token, "POST", &path, input).await });
	super::lifecycle_tests::wait_for_channel_thread_lock_waiters(&f.c.f.store.pool, 2).await;
	blocker.commit().await.unwrap();
	let (status, deleted) = deleting.await.unwrap();
	assert_eq!(status, 200, "{deleted}");
	let (status, denied) = admitting.await.unwrap();
	assert_eq!(status, 404, "{denied}");
	assert_eq!(
		request(&f.c.app, &f.bob, "POST", &child_path, child_input)
			.await
			.0,
		404,
		"a child must also be denied after deletion has already committed"
	);
	let areas: i64 = sqlx::query_scalar(
		&Query::select()
			.expr(Expr::col(Alias::new("id")).count())
			.from(Alias::new("core_areas"))
			.and_where(Expr::col(Alias::new("agent_id")).eq("reviewer"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&f.c.f.store.pool)
	.await
	.unwrap();
	assert_eq!(areas, 0);
	f.c.close().await;
}
