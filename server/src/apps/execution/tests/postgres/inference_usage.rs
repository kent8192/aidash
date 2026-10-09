//! Usage Records are dispatched under the worker lease and completed at most once.
use super::*;
use aidash_domain::provider::usage::{
	LEGACY_PROJECTION_VERSION, ProviderCost, ReportedUsage, RequestEstimate, UsageDispatch,
	UsageOutcome,
};
use reinhardt::query::{Alias, Expr, LockType, PostgresQueryBuilder, Query};

/// outcome, input, output, cache read, cache write, reasoning, cost, upstream cost,
/// estimated tokens, estimator, rejection class, completed.
type UsageRow = (
	String,
	Option<i64>,
	Option<i64>,
	Option<i64>,
	Option<i64>,
	Option<i64>,
	Option<i64>,
	Option<i64>,
	i64,
	String,
	Option<String>,
	bool,
);

async fn usage_row(store: &Store, attempt: Uuid) -> UsageRow {
	let sql = Query::select()
		.columns([
			Alias::new("outcome"),
			Alias::new("input_tokens"),
			Alias::new("output_tokens"),
			Alias::new("cache_read_tokens"),
			Alias::new("cache_write_tokens"),
			Alias::new("reasoning_tokens"),
			Alias::new("cost_nanocredits"),
			Alias::new("upstream_cost_nanocredits"),
			Alias::new("estimated_tokens"),
			Alias::new("estimator"),
			Alias::new("rejection_class"),
		])
		.expr(Expr::cust("completed_at IS NOT NULL"))
		.from(Alias::new("inference_usage"))
		.and_where(Expr::col(Alias::new("attempt_id")).eq(Expr::value(attempt)))
		.to_string(PostgresQueryBuilder);
	sqlx::query_as::<_, UsageRow>(&sql)
		.fetch_one(store.pool.driver())
		.await
		.unwrap()
}

fn dispatch(token: Uuid) -> UsageDispatch {
	UsageDispatch {
		attempt: Uuid::now_v7(),
		lease: token,
		response_epoch: 3,
		model_id: "model".into(),
		model_version: "1.0.0".into(),
		projection_version: LEGACY_PROJECTION_VERSION,
		estimate: RequestEstimate::conservative_bytes(1234),
	}
}

fn unreported(outcome: &str, rejection: Option<&str>, completed: bool) -> UsageRow {
	(
		outcome.into(),
		None,
		None,
		None,
		None,
		None,
		None,
		None,
		1234,
		"bytes".into(),
		rejection.map(Into::into),
		completed,
	)
}

#[rstest::rstest]
#[tokio::test]
async fn usage_records_complete_once_and_stale_dispatches_become_unknown(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: Arc<TestEnvironment>,
) {
	// Arrange
	let (store, url, schema) = setup(&_test_environment).await;
	let registry = Registry::new(store.pool.clone(), &store.node_id).unwrap();
	let agent = seed(&registry).await;
	let workspace = store
		.create_workspace("Usage", "Record inference usage")
		.await
		.unwrap();
	let task = store
		.create_task(workspace.id, &new_task(), "human", None)
		.await
		.unwrap();
	store
		.accept_run(&task, &store.node_id, &agent.id, &agent.version)
		.await
		.unwrap();
	let token = Uuid::new_v4();
	let run = store.lease_run(token, 30).await.unwrap().unwrap();
	let completed = dispatch(token);
	let reported = ReportedUsage {
		input_tokens: Some(120),
		output_tokens: Some(30),
		cache_read_tokens: Some(100),
		cost: Some(ProviderCost {
			nanocredits: 123_400,
			upstream_nanocredits: None,
		}),
		..Default::default()
	};

	// Act / Assert: dispatch then completion writes only the reported counts.
	store
		.record_usage_dispatch(&run, token, &completed)
		.await
		.unwrap();
	assert_eq!(
		usage_row(&store, completed.attempt).await,
		unreported("dispatched", None, false)
	);
	assert!(
		store
			.complete_usage_record(
				&run,
				completed.attempt,
				&UsageOutcome::Completed(reported.clone())
			)
			.await
			.unwrap()
	);
	let expected = (
		"completed".to_owned(),
		Some(120),
		Some(30),
		Some(100),
		None,
		None,
		Some(123_400),
		None,
		1234,
		"bytes".to_owned(),
		None,
		true,
	);
	assert_eq!(usage_row(&store, completed.attempt).await, expected);

	// Completion is at most once.
	assert!(
		!store
			.complete_usage_record(&run, completed.attempt, &UsageOutcome::Unknown)
			.await
			.unwrap()
	);
	assert_eq!(usage_row(&store, completed.attempt).await, expected);

	// An attempt left dispatched by a crash becomes unknown at the next dispatch.
	let interrupted = dispatch(token);
	store
		.record_usage_dispatch(&run, token, &interrupted)
		.await
		.unwrap();
	let rejected = dispatch(token);
	store
		.record_usage_dispatch(&run, token, &rejected)
		.await
		.unwrap();
	assert_eq!(
		usage_row(&store, interrupted.attempt).await,
		unreported("unknown", None, true)
	);
	assert_eq!(usage_row(&store, completed.attempt).await, expected);
	assert!(
		!store
			.complete_usage_record(
				&run,
				interrupted.attempt,
				&UsageOutcome::Completed(reported)
			)
			.await
			.unwrap()
	);
	assert_eq!(
		usage_row(&store, interrupted.attempt).await,
		unreported("unknown", None, true)
	);

	// A provider rejection keeps its class and no counts.
	assert!(
		store
			.complete_usage_record(
				&run,
				rejected.attempt,
				&UsageOutcome::Rejected {
					class: "provider_rejected".into()
				}
			)
			.await
			.unwrap()
	);
	assert_eq!(
		usage_row(&store, rejected.attempt).await,
		unreported("rejected", Some("provider_rejected"), true)
	);

	// Another worker's token cannot dispatch for this Run.
	assert!(
		store
			.record_usage_dispatch(&run, Uuid::new_v4(), &dispatch(token))
			.await
			.is_err()
	);
	cleanup(store, &url, &schema).await;
}

async fn leased_run(store: &Store, token: Uuid) -> Run {
	let registry = Registry::new(store.pool.clone(), &store.node_id).unwrap();
	let agent = seed(&registry).await;
	let workspace = store
		.create_workspace("Usage", "Record inference usage")
		.await
		.unwrap();
	let task = store
		.create_task(workspace.id, &new_task(), "human", None)
		.await
		.unwrap();
	store
		.accept_run(&task, &store.node_id, &agent.id, &agent.version)
		.await
		.unwrap();
	store.lease_run(token, 30).await.unwrap().unwrap()
}

/// The worker's retained authority transaction keeps key-share locks on its Run
/// (rows it inserts reference the Run) while the attempt is dispatched. The
/// lease fence must not wait for that transaction, which only ends after
/// provider I/O starts.
#[rstest::rstest]
#[tokio::test]
async fn dispatch_does_not_wait_for_the_workers_own_run_references(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: Arc<TestEnvironment>,
) {
	// Arrange
	let (store, url, schema) = setup(&_test_environment).await;
	let token = Uuid::new_v4();
	let run = leased_run(&store, token).await;
	let mut authority = store.pool.driver().begin().await.unwrap();
	sqlx::query(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("runs"))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::value(run.id)))
			.lock(LockType::KeyShare)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&mut *authority)
	.await
	.unwrap();
	let attempt = dispatch(token);

	// Act
	let dispatched = tokio::time::timeout(
		std::time::Duration::from_secs(10),
		store.record_usage_dispatch(&run, token, &attempt),
	)
	.await;

	// Assert
	dispatched
		.expect("dispatch must not wait for the authority transaction")
		.unwrap();
	authority.rollback().await.unwrap();
	assert_eq!(
		usage_row(&store, attempt.attempt).await,
		unreported("dispatched", None, false)
	);
	cleanup(store, &url, &schema).await;
}

/// A crashed worker's attempt becomes unknown when its lease is recovered, even
/// if the recovered Run never dispatches another inference.
#[rstest::rstest]
#[tokio::test]
async fn recovering_an_expired_lease_finalizes_its_dispatched_attempts(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: Arc<TestEnvironment>,
) {
	// Arrange
	let (store, url, schema) = setup(&_test_environment).await;
	let crashed = Uuid::new_v4();
	let run = leased_run(&store, crashed).await;
	let attempt = dispatch(crashed);
	store
		.record_usage_dispatch(&run, crashed, &attempt)
		.await
		.unwrap();
	// A live lease is not recoverable; this also completes the recovery scan.
	assert!(store.lease_run(Uuid::new_v4(), 30).await.unwrap().is_none());
	sqlx::query(
		&Query::update()
			.table(Alias::new("runs"))
			.value_expr(
				Alias::new("lease_until"),
				Expr::cust("CURRENT_TIMESTAMP - INTERVAL '1 SECOND'"),
			)
			.and_where(Expr::col(Alias::new("id")).eq(Expr::value(run.id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(store.pool.driver())
	.await
	.unwrap();

	// Act
	let recovered = store.lease_run(Uuid::new_v4(), 30).await.unwrap().unwrap();

	// Assert
	assert!(recovered.recovery.lease_recovered);
	assert_eq!(
		usage_row(&store, attempt.attempt).await,
		unreported("unknown", None, true)
	);
	assert!(
		!store
			.complete_usage_record(&run, attempt.attempt, &UsageOutcome::Unknown)
			.await
			.unwrap()
	);
	cleanup(store, &url, &schema).await;
}
