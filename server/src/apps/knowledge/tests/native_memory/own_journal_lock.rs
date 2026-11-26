//! #1121: a Run that recalls a fact learned from its own Journal must not lock
//! its own Run row. Its Journal writes and lease renewal take that row for
//! update on other connections while the recall waits on them, so the lock
//! hung the Run. A library test, because the evidence check is crate-private.
use super::fixture::*;
use crate::apps::execution::test_database::{DatabaseFixture, database};
use crate::apps::knowledge::repositories::{access::Lease, units};
use crate::database::native;
use aidash_application::tenancy::TEST_FIXTURE;
use aidash_domain::memory::*;
use reinhardt::query::{
	Alias, Expr, ExprTrait, LockBehavior, LockType, PostgresQueryBuilder, Query,
	QueryStatementBuilder,
};
use rstest::rstest;
use serde_json::json;
use uuid::Uuid;

/// A completed Run in `thread` whose Journal holds one event at seq 1.
async fn journal_run(store: &crate::store::Store, thread: Uuid) -> Uuid {
	let task = store
		.create_task(
			&TEST_FIXTURE,
			thread,
			&aidash_domain::NewTask {
				title: "Own Journal".into(),
				description: "Recall a fact learned here".into(),
				requirements: json!({}),
				dependencies: vec![],
				parent_id: None,
			},
			"deployment_administrator",
			None,
		)
		.await
		.unwrap();
	let run = Uuid::now_v7();
	let pending = aidash_domain::run_state::encode(
		&aidash_domain::run_state::RunState::Completed(aidash_domain::run_state::TerminalState {}),
		&aidash_domain::run_state::RecoveryState::default(),
	)
	.unwrap();
	let event = json!({"source": "own journal fixture"});
	let digest = aidash_domain::registry::rules::digest(&event);
	let home =
		aidash_domain::nodes::NodeAddress::derived(store.deployment.as_str(), "acme").to_string();
	let node = aidash_domain::nodes::NodeName::derived("acme").to_string();
	let mut tx = store.pool.begin(&TEST_FIXTURE).await.unwrap();
	native::query(
		&Query::insert()
			.into_table(Alias::new("task_executions"))
			.columns(
				[
					"id",
					"task_id",
					"thread_id",
					"home_node",
					"node",
					"agent_id",
					"agent_version",
					"phase",
					"pending",
					"revision",
					"tenant",
				]
				.map(Alias::new),
			)
			.from_subquery(
				Query::select()
					.expr(Expr::value(run))
					.expr(Expr::value(task.id))
					.expr(Expr::value(thread))
					.expr(Expr::value(home))
					.expr(Expr::value(node))
					.expr(Expr::value("a"))
					.expr(Expr::value("1.0.0"))
					.expr(Expr::value("COMPLETED"))
					.expr(Expr::value(pending))
					.expr(Expr::value(1_i64))
					.expr(Expr::value("acme"))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	native::query(
		&Query::insert()
			.into_table(Alias::new("run_context_events"))
			.columns(["run_id", "seq", "origin", "event", "digest", "tenant"].map(Alias::new))
			.from_subquery(
				Query::select()
					.expr(Expr::value(run))
					.expr(Expr::value(1_i64))
					.expr(Expr::value("appended"))
					.expr(Expr::value(event))
					.expr(Expr::value(digest))
					.expr(Expr::value("acme"))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	tx.commit().await.unwrap();
	run
}

/// The reading Run's own row stays unlocked. Another Run's row is kept from
/// removal (`FOR UPDATE` waits) but its ordinary updates (`FOR NO KEY UPDATE`,
/// as lease renewal and step saves take) do not wait.
#[rstest]
#[case::own_run_update(true, LockType::Update, false)]
#[case::own_run_no_key_update(true, LockType::NoKeyUpdate, false)]
#[case::another_run_update(false, LockType::Update, true)]
#[case::another_run_no_key_update(false, LockType::NoKeyUpdate, false)]
#[tokio::test]
async fn journal_evidence_check_never_blocks_the_runs_own_updates(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
	#[case] own: bool,
	#[case] mode: LockType,
	#[case] locked: bool,
) {
	// Arrange
	let database = database.await;
	let (store, _, thread) = setup(&database, bounds).await;
	let run = journal_run(&store, thread).await;
	let proof = Evidence::JournalRange {
		run_id: run,
		from_seq: 1,
		through_seq: 1,
	};
	let reader = if own { run } else { Uuid::now_v7() };
	let mut lease = Lease::within(&store, &TEST_FIXTURE).await.unwrap();

	// Act: check the evidence and keep the transaction open, as a recall does.
	units::current_for_run(
		&mut lease,
		thread,
		std::slice::from_ref(&proof),
		64,
		Some(reader),
	)
	.await
	.unwrap();
	let mut other = store.pool.begin(&TEST_FIXTURE).await.unwrap();
	let update = native::query(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("task_executions"))
			.and_where(Expr::col("id").eq(Expr::value(run)))
			.lock(mode)
			.lock_behavior(LockBehavior::Nowait)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut *other)
	.await;

	// Assert
	assert_eq!(update.is_err(), locked, "{:?}", update.err());
	drop(other);
	lease.finish(Ok::<(), crate::Error>(())).await.unwrap();
}
