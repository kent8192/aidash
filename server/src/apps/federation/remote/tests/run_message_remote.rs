#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
#[path = "../../../execution/tests/support/legacy.rs"]
mod common;

use aidash_application::tenancy::TEST_FIXTURE;
use aidash_server::{domain::NewTask, harness::Harness};
use common::{bootstrap, cleanup, request};
use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
use serde_json::json;
use uuid::Uuid;

#[rstest::rstest]
#[tokio::test]
async fn committed_fence_survives_delayed_release_and_terminal_transition_is_atomic(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (status, created) = request(
		&app,
		&token,
		"POST",
		"/api/conversations",
		json!({"title":"Atomic terminal transition","goal":"Reply","target":{"id":"research","version":"1.0.0"},"target_kind":"agent"}),
	)
	.await;
	assert_eq!(status, 200, "{created}");
	let run = f.store.runs(&TEST_FIXTURE).await.unwrap().remove(0);
	assert!(
		(Harness {
			federation: f.clone()
		})
		.worker_once()
		.await
		.unwrap()
	);
	let task = f.store.task(&TEST_FIXTURE, run.task_id).await.unwrap();
	let owner = task.owner.clone().unwrap();
	let key = format!("human:{}:{}", run.id, Uuid::new_v4());
	f.store
		.reserve_remote_run_message(
			&TEST_FIXTURE,
			task.id,
			run.id,
			"aidash://executor",
			&key,
			"correction",
		)
		.await
		.unwrap();
	f.store
		.commit_remote_run_message(&TEST_FIXTURE, task.id, run.id, &key, "correction")
		.await
		.unwrap();
	// Model the executor binding the durable input sequence after local admission.
	{
		let query_bind_1 = task.id;
		let query_bind_2 = run.id;
		let query_bind_3 = &key;
		sqlx::query(
			&Query::update()
				.table(Alias::new("remote_run_message_fences"))
				.value_expr(Alias::new("input_seq"), Expr::cust("1"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(task_id = ? AND run_id = ? AND idempotency_key = ?)".to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
						Expr::value(query_bind_3.to_owned()).into(),
					],
				))
				.to_string(PostgresQueryBuilder),
		)
		.execute(f.store.pool.driver())
		.await
	}
	.unwrap();
	// A rejection response from an earlier overlapping attempt can arrive after
	// the successful commit. It must not delete the non-expiring fence.
	f.store
		.release_remote_run_message(
			&TEST_FIXTURE,
			task.id,
			run.id,
			"aidash://executor",
			std::slice::from_ref(&key),
		)
		.await
		.unwrap();
	let retained: bool = sqlx::query_scalar(
		"SELECT EXISTS(SELECT 1 FROM remote_run_message_fences WHERE task_id = $1 AND run_id = $2 AND idempotency_key = $3 AND expires_at IS NULL AND NOT consumed)",
	)
	.bind(task.id)
	.bind(run.id)
	.bind(&key)
	.fetch_one(f.store.pool.driver())
	.await
	.unwrap();
	assert!(retained);
	assert!(
		f.store
			.transition(
				&TEST_FIXTURE,
				task.id,
				task.revision,
				&owner,
				aidash_server::domain::TaskStatus::Cancelled
			)
			.await
			.is_err(),
		"the committed correction must fence an ordinary terminal transition"
	);
	let cancelled = f
		.store
		.transition_remote_run_message_terminal(
			&TEST_FIXTURE,
			task.id,
			task.revision,
			&owner,
			aidash_server::domain::TaskStatus::Cancelled,
			run.id,
			std::slice::from_ref(&key),
		)
		.await
		.unwrap();
	assert_eq!(cancelled.status.as_str(), "CANCELLED");
	let consumed: bool = sqlx::query_scalar(
		"SELECT consumed FROM remote_run_message_fences WHERE task_id = $1 AND idempotency_key = $2",
	)
	.bind(task.id)
	.bind(&key)
	.fetch_one(f.store.pool.driver())
	.await
	.unwrap();
	assert!(consumed);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn peer_prefixed_legacy_output_checks_remote_fence_without_a_local_run(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	// Retain the disposable environment through the remote fence assertions.
	let runtime_owner = runtime.await;
	let (f, url, schema) = runtime_owner.parts();
	let scope = common::tenant_scope(&f.store, "acme").await;
	let workspace = f
		.store
		.create_workspace(
			&scope,
			"alice",
			"Remote output fence",
			"No executor run exists at home",
		)
		.await
		.unwrap();
	let task = f
		.store
		.create_task(
			&TEST_FIXTURE,
			workspace.id,
			&NewTask {
				title: "Remote task".into(),
				description: "Fence stale output".into(),
				requirements: json!({}),
				dependencies: vec![],
				parent_id: None,
			},
			"human",
			None,
		)
		.await
		.unwrap();
	let remote_run = Uuid::new_v4();
	let input_key = format!("human:{remote_run}:{}", Uuid::new_v4());
	f.store
		.reserve_remote_run_message(
			&TEST_FIXTURE,
			task.id,
			remote_run,
			"aidash://executor",
			&input_key,
			"remote correction",
		)
		.await
		.unwrap();
	f.store
		.commit_remote_run_message(
			&TEST_FIXTURE,
			task.id,
			remote_run,
			&input_key,
			"remote correction",
		)
		.await
		.unwrap();
	let peer_output_key = format!("aidash://executor:{}:{remote_run}:0:output", task.id);
	assert!(
		f.store
			.message(
				&TEST_FIXTURE,
				workspace.id,
				"agent@aidash://executor",
				"stale peer output",
				Some(&peer_output_key),
			)
			.await
			.is_err(),
		"the database fence must not depend on a home-side executor run row"
	);
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn remote_history_references_can_span_multiple_inference_pages(
	#[future(awt)]
	#[from(common::native_application)]
	fixture: common::ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application;
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (status, created) = request(
		&app,
		&token,
		"POST",
		"/api/conversations",
		json!({"title":"Paged remote history","goal":"Reply","target":{"id":"research","version":"1.0.0"},"target_kind":"agent"}),
	)
	.await;
	assert_eq!(status, 200, "{created}");
	let run = f.store.runs(&TEST_FIXTURE).await.unwrap().remove(0);
	let mut history = Vec::new();
	for index in 0..20 {
		let key = format!("human:{}:{}", run.id, Uuid::new_v4());
		let message = f
			.store
			.message_record(
				&TEST_FIXTURE,
				run.workspace_id,
				"human@remote",
				&format!("history {index}: {}", "x".repeat(4096)),
				Some(&format!("remote-history-{index}")),
			)
			.await
			.unwrap();
		history.push((key, message));
	}
	f.store
		.import_remote_run_messages(&TEST_FIXTURE, run.id, &history, 512)
		.await
		.expect("pageable references are not subject to one aggregate page cap");
	let inputs = f.store.run_inputs(&TEST_FIXTURE, run.id).await.unwrap();
	assert_eq!(inputs.len(), history.len());
	assert!(inputs.iter().all(|input| input.reference_only));
	let new_key = format!("human:{}:{}", run.id, Uuid::new_v4());
	f.store
		.accept_run_message(
			&TEST_FIXTURE,
			run.id,
			"human",
			"latest correction",
			&new_key,
			512,
		)
		.await
		.expect("historical reference pages do not consume new-message capacity");
	cleanup(f, &url, &schema).await;
}

use reinhardt::query::QueryStatementBuilder as _;

use reinhardt::query::SimpleExpr;
