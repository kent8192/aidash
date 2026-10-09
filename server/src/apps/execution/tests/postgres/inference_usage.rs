//! Usage Records are dispatched under the worker lease and completed at most once.
use super::*;
use aidash_domain::provider::usage::{
	LEGACY_PROJECTION_VERSION, ProviderCost, ReportedUsage, RequestEstimate, UsageDispatch,
	UsageOutcome,
};
use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};

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
