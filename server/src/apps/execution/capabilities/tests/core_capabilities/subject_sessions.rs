use super::*;
use aidash_application::tenancy::TEST_FIXTURE;
use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};

struct SharedThread {
	c: CoreFixture,
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
		let thread =
			c.f.store
				.task(&TEST_FIXTURE, c.task)
				.await
				.unwrap()
				.thread_id;
		let (_, credential) = async {
			(
				200_u16,
				common::issued_credential(&c.f.store, "acme", "bob").await,
			)
		}
		.await;
		let bob = credential["token"].as_str().unwrap().to_owned();
		let mut areas = vec![];
		let mut runs = vec![];
		for (owner, token) in [("alice", &c.token), ("bob", &bob)] {
			let (status, area) = common::post(&c.app, token, &format!("/api/threads/{thread}/agents/research/executions"), json!({"idempotency_key":Uuid::new_v4(),"agent_version":"1.1.0","title":"Private work","description":"One owner's files"})).await;
			assert_eq!(status, 200, "{owner}: {area}");
			let (_, session) = common::get(
				&c.app,
				token,
				&format!(
					"/api/execution-scopes/{}/session",
					area["id"].as_str().unwrap()
				),
			)
			.await;
			let run: Uuid = serde_json::from_value(session["active_run_id"].clone()).unwrap();
			areas.push(area);
			runs.push(run);
		}
		SharedThread {
			c,
			thread,
			bob,
			areas,
			runs,
		}
	}
}

#[rstest::rstest]
#[tokio::test]
async fn same_thread_agent_keeps_subject_run_history_private(
	#[future] shared_thread: SharedThread,
) {
	let f = Box::pin(shared_thread).await;
	assert_ne!(f.areas[0]["id"], f.areas[1]["id"]);
	assert_ne!(f.runs[0], f.runs[1]);
	let run_path = format!("/api/executions/{}", f.runs[0]);
	assert_eq!(common::get(&f.c.app, &f.c.token, &run_path).await.0, 200);
	let (status, denied) = common::get(&f.c.app, &f.bob, &run_path).await;
	assert!(
		matches!(status, 403 | 404),
		"run history must not disclose the other subject's Run: {denied}"
	);
	assert!(denied.get("run").is_none());
	let hidden = common::get(
		&f.c.app,
		&f.bob,
		&format!("/api/executions/{}/execution-scope", f.runs[0]),
	)
	.await;
	let absent = common::get(
		&f.c.app,
		&f.bob,
		&format!("/api/executions/{}/execution-scope", Uuid::new_v4()),
	)
	.await;
	assert_eq!(hidden.0, 403);
	assert_eq!(
		hidden, absent,
		"private run existence must not be disclosed"
	);
	let (_, visible) = common::get(
		&f.c.app,
		&f.c.token,
		&format!("/api/execution-scopes?thread_id={}", f.thread),
	)
	.await;
	assert_eq!(visible["items"].as_array().unwrap().len(), 1);
	f.c.close().await;
}

#[rstest::fixture]
async fn delegated_thread_child(#[future] shared_thread: SharedThread) -> (SharedThread, Uuid) {
	let mut f = Box::pin(shared_thread).await;
	let mut entry =
		f.c.f
			.registry
			.get(
				&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
				"research",
				"1.1.0",
			)
			.await
			.unwrap();
	entry.id = "reviewer".into();
	entry.binding_normalization = None;
	assert_eq!(
		common::post(&f.c.app, &f.c.token, "/api/registry", json!(entry))
			.await
			.0,
		200
	);
	f.c.policy["subjects"][aidash_server::domain::qualified_agent(
		&aidash_domain::nodes::NodeAddress::derived(f.c.f.config.deployment.as_str(), "acme"),
		"reviewer",
		"1.1.0",
	)] = json!({"kind":"agent", "node": aidash_domain::nodes::NodeName::derived("acme")});
	assert_eq!(
		common::post(
			&f.c.app,
			&f.c.f.config.api_token,
			"/api/authorization/acme",
			json!({"expected_revision":2,"bundle":f.c.policy})
		)
		.await
		.0,
		200
	);
	assert_eq!(
		common::post(&f.c.app, &f.c.token, "/api/authorization/acme/catalog", json!({"node":aidash_domain::nodes::NodeName::derived("acme"),"entry":{"id":"reviewer","version":"1.1.0"},"expected_revision":0,"enabled":true}))
		.await
		.0,
		200
	);
	let parent = f.c.f.store.run(&TEST_FIXTURE, f.runs[1]).await.unwrap();
	let (status, child) = common::post(
		&f.c.app,
		&f.bob,
		&format!("/api/threads/{}/tasks", f.thread),
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
	let (f, child) = Box::pin(delegated_thread_child).await;
	// Hold the Thread row while the delete request queues first; the child's
	// admission must wait behind it, then observe the committed deletion.
	let (blocker, blocker_pid) =
		super::lifecycle_tests::hold_thread_lock(f.c.f.store.pool.fixture_driver(), f.thread).await;
	let app = f.c.app.clone();
	let token = f.c.token.clone();
	let path = format!("/api/threads/{}/delete", f.thread);
	let input = json!({"idempotency_key":Uuid::new_v4(),"files":[{"area_id":f.areas[0]["id"],"expected_revision":f.areas[0]["revision"],"choice":"keep"}]});
	let deleting = tokio::spawn(async move { common::post(&app, &token, &path, input).await });
	let delete_pid = super::lifecycle_tests::wait_for_blocked_connection(
		f.c.f.store.pool.fixture_driver(),
		blocker_pid,
	)
	.await;
	let app = f.c.app.clone();
	let token = f.bob.clone();
	let path = format!("/api/tasks/{child}/delegate");
	let input =
		json!({"node_id":f.c.f.config.deployment,"agent":{"id":"reviewer","version":"1.1.0"}});
	let child_path = path.clone();
	let child_input = input.clone();
	let admitting = tokio::spawn(async move { common::post(&app, &token, &path, input).await });
	super::lifecycle_tests::wait_for_blocked_connection(
		f.c.f.store.pool.fixture_driver(),
		delete_pid,
	)
	.await;
	blocker.commit().await.unwrap();
	let (status, deleted) = deleting.await.unwrap();
	assert_eq!(status, 200, "{deleted}");
	let (status, denied) = admitting.await.unwrap();
	assert_eq!(status, 404, "{denied}");
	// Once the deletion has committed the Thread is gone, so the refusal may
	// come from its hidden Thread before any tombstone check.
	let (status, denied) = common::post(&f.c.app, &f.bob, &child_path, child_input).await;
	assert!(
		matches!(status, 403 | 404),
		"a child must also be denied after deletion has already committed: {denied}"
	);
	let areas: i64 = sqlx::query_scalar(
		&Query::select()
			.expr(reinhardt::query::Func::count(
				Expr::col(Alias::new("id")).into(),
			))
			.from(Alias::new("core_execution_scopes"))
			.and_where(Expr::col(Alias::new("agent_id")).eq("reviewer"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(f.c.f.store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(areas, 0);
	f.c.close().await;
}

use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};
