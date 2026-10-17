//! Usage Records are dispatched under the worker lease and completed at most once.
use super::*;
use aidash_domain::projection::ProjectionVersion;
use aidash_domain::provider::estimate::{
	ByteEstimate, CALIBRATION_WINDOW, EstimateSnapshot, FRAMING_RESERVE,
};
use aidash_domain::provider::usage::{ProviderCost, ReportedUsage, UsageDispatch, UsageOutcome};
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

/// 200 bytes of basis, a 10-token output reservation and the framing reserve:
/// a 1,234-token byte estimate.
const BYTES: ByteEstimate = ByteEstimate {
	basis: 200,
	media: 0,
	output: 10,
	framing: FRAMING_RESERVE,
};

fn dispatch(token: Uuid) -> UsageDispatch {
	UsageDispatch {
		attempt: Uuid::now_v7(),
		lease: token,
		response_epoch: 3,
		model_id: "model".into(),
		model_version: "1.0.0".into(),
		projection_version: ProjectionVersion::Legacy.number(),
		upstream_model: "vendor/model".into(),
		estimate: EstimateSnapshot::conservative(0).request_estimate(&BYTES),
		calibration_sample: false,
		cache_identity: None,
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
	#[from(store)]
	_store_fixture: StoreFixture,
) {
	// Arrange
	let (store, url, schema) = _store_fixture.parts();
	let registry = Registry::new(store.pool.clone(), &store.node_id).unwrap();
	let agent = seed(&registry).await;
	let workspace = store
		.create_workspace(&ACME, "alice", "Usage", "Record inference usage")
		.await
		.unwrap();
	let task = store
		.create_task(&ACME, workspace.id, &new_task(), "human", None)
		.await
		.unwrap();
	common::local_run(&store, &ACME, &task, &agent).await;
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
		.record_usage_dispatch(&ACME, &run, token, &completed)
		.await
		.unwrap();
	assert_eq!(
		usage_row(&store, completed.attempt).await,
		unreported("dispatched", None, false)
	);
	assert!(
		store
			.complete_usage_record(
				&ACME,
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
			.complete_usage_record(&ACME, &run, completed.attempt, &UsageOutcome::Unknown)
			.await
			.unwrap()
	);
	assert_eq!(usage_row(&store, completed.attempt).await, expected);

	// An attempt left dispatched by a crash becomes unknown at the next dispatch.
	let interrupted = dispatch(token);
	store
		.record_usage_dispatch(&ACME, &run, token, &interrupted)
		.await
		.unwrap();
	let rejected = dispatch(token);
	store
		.record_usage_dispatch(&ACME, &run, token, &rejected)
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
				&ACME,
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
				&ACME,
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
			.record_usage_dispatch(&ACME, &run, Uuid::new_v4(), &dispatch(token))
			.await
			.is_err()
	);
	cleanup(store, &url, &schema).await;
}

async fn leased_run(store: &Store, token: Uuid) -> Run {
	let registry = Registry::new(store.pool.clone(), &store.node_id).unwrap();
	let agent = seed(&registry).await;
	let workspace = store
		.create_workspace(&ACME, "alice", "Usage", "Record inference usage")
		.await
		.unwrap();
	let task = store
		.create_task(&ACME, workspace.id, &new_task(), "human", None)
		.await
		.unwrap();
	common::local_run(&store, &ACME, &task, &agent).await;
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
	#[from(store)]
	_store_fixture: StoreFixture,
) {
	// Arrange
	let (store, url, schema) = _store_fixture.parts();
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
		store.record_usage_dispatch(&ACME, &run, token, &attempt),
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
	#[from(store)]
	_store_fixture: StoreFixture,
) {
	// Arrange
	let (store, url, schema) = _store_fixture.parts();
	let crashed = Uuid::new_v4();
	let run = leased_run(&store, crashed).await;
	let attempt = dispatch(crashed);
	store
		.record_usage_dispatch(&ACME, &run, crashed, &attempt)
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
			.complete_usage_record(&ACME, &run, attempt.attempt, &UsageOutcome::Unknown)
			.await
			.unwrap()
	);
	cleanup(store, &url, &schema).await;
}

async fn cache_identity(store: &Store, attempt: Uuid) -> Option<String> {
	let sql = Query::select()
		.column(Alias::new("cache_identity"))
		.from(Alias::new("inference_usage"))
		.and_where(Expr::col(Alias::new("attempt_id")).eq(Expr::value(attempt)))
		.to_string(PostgresQueryBuilder);
	sqlx::query_scalar::<_, Option<String>>(&sql)
		.fetch_one(store.pool.driver())
		.await
		.unwrap()
}

/// A salted attempt records its Stable Prefix cache identity; an unsalted
/// one leaves it unknown rather than empty (ADR 0018).
#[rstest::rstest]
#[tokio::test]
async fn usage_records_keep_the_cache_identity_of_salted_attempts(
	#[future(awt)]
	#[from(store)]
	_store_fixture: StoreFixture,
) {
	// Arrange
	let (store, url, schema) = _store_fixture.parts();
	let token = Uuid::new_v4();
	let run = leased_run(&store, token).await;
	let legacy = dispatch(token);
	let native = UsageDispatch {
		attempt: Uuid::now_v7(),
		projection_version: ProjectionVersion::Native.number(),
		cache_identity: Some("prefix-digest".into()),
		..dispatch(token)
	};

	// Act
	for attempt in [&legacy, &native] {
		store
			.record_usage_dispatch(&ACME, &run, token, attempt)
			.await
			.unwrap();
	}

	// Assert
	assert_eq!(cache_identity(&store, legacy.attempt).await, None);
	assert_eq!(
		cache_identity(&store, native.attempt).await.as_deref(),
		Some("prefix-digest")
	);
	cleanup(store, &url, &schema).await;
}

/// byte basis, output reservation, ratio, upstream model, calibration sample.
type EstimateParts = (Option<i64>, Option<i64>, Option<i64>, Option<String>, bool);

async fn estimate_parts(store: &Store, attempt: Uuid) -> EstimateParts {
	let sql = Query::select()
		.columns([
			Alias::new("byte_basis"),
			Alias::new("output_reservation"),
			Alias::new("estimate_ratio_ppm"),
			Alias::new("upstream_model"),
			Alias::new("calibration_sample"),
		])
		.from(Alias::new("inference_usage"))
		.and_where(Expr::col(Alias::new("attempt_id")).eq(Expr::value(attempt)))
		.to_string(PostgresQueryBuilder);
	sqlx::query_as::<_, EstimateParts>(&sql)
		.fetch_one(store.pool.driver())
		.await
		.unwrap()
}

async fn sample_columns(store: &Store) -> Vec<String> {
	let sql = Query::select()
		.column(Alias::new("column_name"))
		.from((Alias::new("information_schema"), Alias::new("columns")))
		.and_where(Expr::col(Alias::new("table_schema")).eq(Expr::cust("current_schema()")))
		.and_where(
			Expr::col(Alias::new("table_name")).eq(Expr::value("estimate_calibration_samples")),
		)
		.order_by(Alias::new("column_name"), reinhardt::query::Order::Asc)
		.to_string(PostgresQueryBuilder);
	sqlx::query_scalar::<_, String>(&sql)
		.fetch_all(store.pool.driver())
		.await
		.unwrap()
}

/// Only a complete calibration-sample attempt becomes a Deployment-wide
/// sample, and the sample carries no Run, Tenant or attempt identifier
/// (ADR 0017).
#[rstest::rstest]
#[tokio::test]
async fn complete_sample_attempts_record_anonymous_calibration_samples(
	#[future(awt)]
	#[from(store)]
	_store_fixture: StoreFixture,
) {
	// Arrange
	let (store, url, schema) = _store_fixture.parts();
	let token = Uuid::new_v4();
	let run = leased_run(&store, token).await;
	let calibrated = UsageDispatch {
		estimate: EstimateSnapshot::calibrated(4, 250_000, 20).request_estimate(&BYTES),
		calibration_sample: true,
		..dispatch(token)
	};
	let excluded = dispatch(token);
	let rejected = UsageDispatch {
		calibration_sample: true,
		..dispatch(token)
	};
	let reported = ReportedUsage {
		input_tokens: Some(60),
		output_tokens: Some(5),
		..Default::default()
	};

	// Act
	for (attempt, outcome) in [
		(&calibrated, UsageOutcome::Completed(reported.clone())),
		(&excluded, UsageOutcome::Completed(reported.clone())),
		(
			&rejected,
			UsageOutcome::Rejected {
				class: "context_overflow".into(),
			},
		),
	] {
		store
			.record_usage_dispatch(&ACME, &run, token, attempt)
			.await
			.unwrap();
		assert!(
			store
				.complete_usage_record(&ACME, &run, attempt.attempt, &outcome)
				.await
				.unwrap()
		);
	}

	// Assert
	assert_eq!(
		estimate_parts(&store, calibrated.attempt).await,
		(
			Some(200),
			Some(10),
			Some(250_000),
			Some("vendor/model".into()),
			true
		)
	);
	assert_eq!(
		estimate_parts(&store, excluded.attempt).await,
		(
			Some(200),
			Some(10),
			None,
			Some("vendor/model".into()),
			false
		)
	);
	let samples = store
		.calibration_samples("vendor/model", ProjectionVersion::Legacy.number())
		.await
		.unwrap();
	assert_eq!(
		samples,
		vec![aidash_domain::provider::estimate::CalibrationSample {
			byte_basis: 200,
			prompt_tokens: 60,
		}]
	);
	assert!(
		store
			.calibration_samples("vendor/model", ProjectionVersion::Ordered.number())
			.await
			.unwrap()
			.is_empty()
	);
	assert_eq!(
		sample_columns(&store).await,
		[
			"byte_basis",
			"completed_at",
			"id",
			"projection_version",
			"prompt_tokens",
			"upstream_model"
		]
	);
	cleanup(store, &url, &schema).await;
}

/// Each key keeps only its newest calibration window, newest first.
#[rstest::rstest]
#[tokio::test]
async fn calibration_samples_keep_only_the_newest_window_per_key(
	#[future(awt)]
	#[from(store)]
	_store_fixture: StoreFixture,
) {
	// Arrange
	let (store, url, schema) = _store_fixture.parts();
	let token = Uuid::new_v4();
	let run = leased_run(&store, token).await;
	let extra = 5;
	let complete = async |upstream_model: &str, prompt_tokens: u64| {
		let attempt = UsageDispatch {
			upstream_model: upstream_model.into(),
			calibration_sample: true,
			..dispatch(token)
		};
		store
			.record_usage_dispatch(&ACME, &run, token, &attempt)
			.await
			.unwrap();
		let reported = ReportedUsage {
			input_tokens: Some(prompt_tokens),
			..Default::default()
		};
		assert!(
			store
				.complete_usage_record(
					&ACME,
					&run,
					attempt.attempt,
					&UsageOutcome::Completed(reported)
				)
				.await
				.unwrap()
		);
	};
	for tokens in 1..=(CALIBRATION_WINDOW + extra) as u64 {
		complete("vendor/model", tokens).await;
	}
	// Another key is pruned on its own.
	complete("other/model", 7).await;

	// Act
	let samples = store
		.calibration_samples("vendor/model", ProjectionVersion::Legacy.number())
		.await
		.unwrap();

	// Assert
	assert_eq!(samples.len(), CALIBRATION_WINDOW);
	assert_eq!(
		samples
			.iter()
			.map(|sample| sample.prompt_tokens)
			.collect::<Vec<_>>(),
		((extra as u64 + 1)..=(CALIBRATION_WINDOW + extra) as u64)
			.rev()
			.collect::<Vec<_>>()
	);
	assert_eq!(
		store
			.calibration_samples("other/model", ProjectionVersion::Legacy.number())
			.await
			.unwrap()
			.len(),
		1
	);
	cleanup(store, &url, &schema).await;
}
