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
