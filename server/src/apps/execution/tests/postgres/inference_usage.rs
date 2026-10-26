//! Usage Records are dispatched under the worker lease and completed at most once.
use super::*;
use aidash_domain::projection::ProjectionVersion;
use aidash_domain::provider::estimate::{
	ByteEstimate, CALIBRATION_WINDOW, EstimateSnapshot, FRAMING_RESERVE, TokenEncoding,
};
use aidash_domain::provider::usage::{
	ProviderCost, ProviderIdentity, ReportedUsage, ShadowEstimate, UsageDispatch, UsageOutcome,
	UsagePurpose,
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
		.fetch_one(store.pool.fixture_driver())
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
	tokenized: None,
};

fn dispatch(token: Uuid) -> UsageDispatch {
	UsageDispatch {
		attempt: Uuid::now_v7(),
		lease: token,
		response_epoch: 3,
		model_id: "model".into(),
		model_version: "1.0.0".into(),
		projection_version: ProjectionVersion::Legacy.number(),
		harness_contract: aidash_domain::harness_contract::HarnessContract::Legacy.number(),
		upstream_model: "vendor/model".into(),
		purpose: UsagePurpose::Main,
		requested_route: None,
		shadow_estimates: vec![],
		estimate: EstimateSnapshot::conservative(0).request_estimate(&BYTES),
		calibration_sample: false,
		cache_identity: None,
	}
}

async fn prepare_summary_usage(store: &Store, run: &Run, token: Uuid, dispatch: &UsageDispatch) {
	use aidash_application::ports::execution::ExecutionStore;
	use aidash_domain::context::recovery::{Attempt, Stage};
	let attempt = Attempt {
		id: dispatch.attempt,
		run_id: run.id,
		stage: Stage::Summary,
		policy_version: "context-recovery/1".into(),
		provider: "summarizer@1.0.0#sha256:fixture".into(),
		source_from_seq: 1,
		source_through_seq: 1,
		base_revision: run.revision,
		observed_input_seq: run.observed_input_seq,
		before_tokens: dispatch.estimate.tokens,
	};
	ExecutionStore::begin_compaction(&store.for_run((*ACME).clone()), run, token, &attempt, 10)
		.await
		.unwrap();
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
	let registry = Registry::new(store.pool.clone(), &store.deployment).unwrap();
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
	let registry = Registry::new(store.pool.clone(), &store.deployment).unwrap();
	let agent = seed(&registry).await;
	let workspace = store
		.create_workspace(&ACME, "alice", "Usage", "Record inference usage")
		.await
		.unwrap();
	let task = store
		.create_task(&ACME, workspace.id, &new_task(), "human", None)
		.await
		.unwrap();
	common::local_run(store, &ACME, &task, &agent).await;
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
	let mut authority = store.pool.fixture_driver().begin().await.unwrap();
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
	let auxiliary = UsageDispatch {
		purpose: UsagePurpose::Summary,
		..dispatch(crashed)
	};
	prepare_summary_usage(&store, &run, crashed, &auxiliary).await;
	store
		.record_usage_dispatch(&ACME, &run, crashed, &auxiliary)
		.await
		.unwrap();
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
	.execute(store.pool.fixture_driver())
	.await
	.unwrap();

	// Act
	let recovered = store.lease_run(Uuid::new_v4(), 30).await.unwrap().unwrap();

	// Assert
	assert!(recovered.recovery.lease_recovered);
	assert_eq!(
		usage_row(&store, auxiliary.attempt).await,
		unreported("unknown", None, true)
	);
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
		.fetch_one(store.pool.fixture_driver())
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

/// A tokenized estimate is stored under its own confidence, which the
/// database check admits since execution/0034 (#276).
#[rstest::rstest]
#[tokio::test]
async fn usage_records_admit_tokenized_estimates(
	#[future(awt)]
	#[from(store)]
	_store_fixture: StoreFixture,
) {
	// Arrange
	let (store, url, schema) = _store_fixture.parts();
	let token = Uuid::new_v4();
	let run = leased_run(&store, token).await;
	let tokenized = UsageDispatch {
		estimate: EstimateSnapshot::tokenized(4, TokenEncoding::O200kBase, None).request_estimate(
			&ByteEstimate {
				tokenized: Some(50),
				..BYTES
			},
		),
		..dispatch(token)
	};

	// Act
	store
		.record_usage_dispatch(&ACME, &run, token, &tokenized)
		.await
		.unwrap();

	// Assert
	let sql = Query::select()
		.columns([
			Alias::new("estimator"),
			Alias::new("estimate_confidence"),
			Alias::new("estimated_tokens"),
		])
		.from(Alias::new("inference_usage"))
		.and_where(Expr::col(Alias::new("attempt_id")).eq(Expr::value(tokenized.attempt)))
		.to_string(PostgresQueryBuilder);
	let row = sqlx::query_as::<_, (String, String, i64)>(&sql)
		.fetch_one(store.pool.fixture_driver())
		.await
		.unwrap();
	assert_eq!(
		row,
		(
			"tokenized".into(),
			"tokenized".into(),
			50 + 10 + FRAMING_RESERVE as i64
		)
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
		.fetch_one(store.pool.fixture_driver())
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
		.fetch_all(store.pool.fixture_driver())
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

/// Auxiliary calls retain their independent terminal write, and cannot add
/// calibration samples. Facts survive at-most-once completion and export intact.
#[rstest::rstest]
#[tokio::test]
async fn summary_usage_is_independent_from_main_and_keeps_prompt_free_evidence(
	#[future(awt)]
	#[from(store)]
	_store_fixture: StoreFixture,
) {
	let (store, url, schema) = _store_fixture.parts();
	let token = Uuid::new_v4();
	let run = leased_run(&store, token).await;
	let snapshot = EstimateSnapshot::calibrated(3, 250_000, 20);
	let auxiliary = UsageDispatch {
		purpose: UsagePurpose::Summary,
		requested_route: Some("fixture-route".into()),
		shadow_estimates: vec![ShadowEstimate {
			snapshot,
			estimate: snapshot.request_estimate(&BYTES),
		}],
		calibration_sample: true, // The persistence boundary enforces main-only calibration.
		..dispatch(token)
	};
	let main = dispatch(token);
	prepare_summary_usage(&store, &run, token, &auxiliary).await;
	for attempt in [&auxiliary, &main] {
		store
			.record_usage_dispatch(&ACME, &run, token, attempt)
			.await
			.unwrap();
	}
	assert_eq!(
		usage_row(&store, auxiliary.attempt).await,
		unreported("dispatched", None, false)
	);
	let identity = ProviderIdentity {
		reported_model: Some("upstream/served".into()),
		reported_provider: Some("fixture-provider".into()),
		reported_route: Some("fixture-route".into()),
		provider_request_id: Some("provider-request-1".into()),
	};
	let usage = ReportedUsage {
		input_tokens: Some(60),
		provider_identity: Some(identity.clone()),
		..Default::default()
	};
	assert!(
		store
			.complete_usage_record(
				&ACME,
				&run,
				auxiliary.attempt,
				&UsageOutcome::Completed(usage)
			)
			.await
			.unwrap()
	);
	let sql = Query::select()
		.columns(
			[
				"purpose",
				"requested_route",
				"provider_identity",
				"shadow_estimates",
				"calibration_sample",
				"cost_nanocredits",
			]
			.map(Alias::new),
		)
		.from(Alias::new("inference_usage"))
		.and_where(Expr::col("attempt_id").eq(Expr::value(auxiliary.attempt)))
		.to_string(PostgresQueryBuilder);
	let evidence = sqlx::query_as::<
		_,
		(
			String,
			Option<String>,
			Option<serde_json::Value>,
			Option<serde_json::Value>,
			bool,
			Option<i64>,
		),
	>(&sql)
	.fetch_one(store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(
		evidence,
		(
			"summary".into(),
			Some("fixture-route".into()),
			Some(serde_json::to_value(identity).unwrap()),
			Some(serde_json::to_value(&auxiliary.shadow_estimates).unwrap()),
			false,
			None
		)
	);
	assert!(
		store
			.calibration_samples("vendor/model", ProjectionVersion::Legacy.number())
			.await
			.unwrap()
			.is_empty()
	);
	assert_eq!(
		usage_row(&store, main.attempt).await,
		unreported("dispatched", None, false)
	);
	cleanup(store, &url, &schema).await;
}

// Compile the production query document at its owning boundary without a
// test-only visibility export from the operations app.
#[path = "../../../operations/services/diagnostics/evaluation.rs"]
mod evaluation_queries;

/// Evaluation diagnostics execute against the migrated schema, preserve unknown
/// costs and recovery evidence, and obey the caller's Tenant RLS scope.
#[rstest::rstest]
#[tokio::test]
async fn evaluation_export_keeps_usage_and_recovery_without_private_context(
	#[future(awt)]
	#[from(store)]
	_store_fixture: StoreFixture,
) {
	let (store, url, schema) = _store_fixture.parts();
	let token = Uuid::new_v4();
	let mut run = leased_run(&store, token).await;
	run.context.legacy_summary = "private prompt must never leave diagnostics".into();
	run.context.compactions = 3;
	run.context.recovery.window_permille = 750;
	run.context.recovery.overflow_attempts = 2;
	run.context.recovery.overflow_request_tokens = Some(7000);
	run.context.estimate.demoted = true;
	let context = serde_json::to_value(&run.context).unwrap();
	let update = Query::update()
		.table(Alias::new("runs"))
		.value_expr(Alias::new("context"), Expr::value(context))
		.and_where(Expr::col("id").eq(Expr::value(run.id)))
		.to_string(PostgresQueryBuilder);
	sqlx::query(&update)
		.execute(store.pool.fixture_driver())
		.await
		.unwrap();
	let attempt = UsageDispatch {
		cache_identity: Some("opaque-prefix-digest".into()),
		..dispatch(token)
	};
	store
		.record_usage_dispatch(&ACME, &run, token, &attempt)
		.await
		.unwrap();
	store
		.complete_usage_record(&ACME, &run, attempt.attempt, &UsageOutcome::Unknown)
		.await
		.unwrap();
	let document = evaluation_queries::document();
	let mut exported = serde_json::Map::new();
	let mut tx = store.pool.begin(&ACME).await.unwrap();
	for (table, query) in document.as_object().unwrap() {
		let rows: Vec<serde_json::Value> =
			aidash_server::database::native::query_scalar(query.as_str().unwrap())
				.scalar_all(&mut *tx)
				.await
				.unwrap();
		exported.insert(table.clone(), serde_json::json!(rows));
	}
	tx.rollback().await.unwrap();
	let usage = &exported["inference_usage"][0];
	assert_eq!(usage["attempt_id"], serde_json::json!(attempt.attempt));
	assert_eq!(usage["outcome"], "unknown");
	assert!(usage["cost_nanocredits"].is_null());
	assert_eq!(usage["cache_identity"], "opaque-prefix-digest");
	let recovery = &exported["run_recovery"][0];
	assert_eq!(recovery["run_id"], serde_json::json!(run.id));
	assert_eq!(recovery["compactions"], 3);
	assert_eq!(recovery["overflow_attempts"], 2);
	assert_eq!(recovery["window_permille"], 750);
	assert_eq!(recovery["estimator_demoted"], true);
	let text = serde_json::to_string(&exported).unwrap();
	assert!(!text.contains("private prompt"));
	assert!(!text.contains(&token.to_string()));
	let unrelated = aidash_application::tenancy::TenancyScope::tenant("unrelated").unwrap();
	let mut tx = store.pool.begin(&unrelated).await.unwrap();
	for query in document.as_object().unwrap().values() {
		let rows: Vec<serde_json::Value> =
			aidash_server::database::native::query_scalar(query.as_str().unwrap())
				.scalar_all(&mut *tx)
				.await
				.unwrap();
		assert!(rows.is_empty());
	}
	tx.rollback().await.unwrap();
	cleanup(store, &url, &schema).await;
}

/// The same unavailable outcome can occur before I/O, after I/O, or in a
/// historical row with unknown dispatch state. Only the explicit false flag
/// proves that a missing provider Usage Record means no call occurred.
#[rstest::rstest]
#[tokio::test]
async fn summary_dispatch_proof_is_atomic_and_independent_from_compaction_outcome(
	#[future(awt)]
	#[from(store)]
	_store_fixture: StoreFixture,
) {
	use aidash_application::ports::execution::ExecutionStore;
	use aidash_domain::context::recovery::{Outcome, Settlement};
	let (store, url, schema) = _store_fixture.parts();
	let token = Uuid::new_v4();
	let run = leased_run(&store, token).await;
	let mut expected = std::collections::BTreeMap::new();
	let mut physical = None;
	for (historical, dispatched) in [(true, false), (false, false), (false, true)] {
		let dispatch = UsageDispatch {
			purpose: UsagePurpose::Summary,
			..dispatch(token)
		};
		prepare_summary_usage(&store, &run, token, &dispatch).await;
		if historical {
			let sql = Query::update()
				.table(Alias::new("context_compaction_attempts"))
				.value_expr(
					Alias::new("provider_dispatched"),
					Expr::value(Option::<bool>::None),
				)
				.and_where(Expr::col("id").eq(Expr::value(dispatch.attempt)))
				.to_string(PostgresQueryBuilder);
			sqlx::query(&sql)
				.execute(store.pool.fixture_driver())
				.await
				.unwrap();
		}
		if dispatched {
			store
				.record_usage_dispatch(&ACME, &run, token, &dispatch)
				.await
				.unwrap();
			store
				.complete_usage_record(&ACME, &run, dispatch.attempt, &UsageOutcome::Unknown)
				.await
				.unwrap();
			physical = Some(dispatch.attempt);
		}
		ExecutionStore::settle_compaction(
			&store.for_run((*ACME).clone()),
			&run,
			token,
			dispatch.attempt,
			&Settlement {
				outcome: Outcome::Unavailable,
				reason: None,
				candidate_digest: None,
				after_tokens: None,
			},
		)
		.await
		.unwrap();
		expected.insert(
			dispatch.attempt.to_string(),
			if historical { None } else { Some(dispatched) },
		);
	}
	let document = evaluation_queries::document();
	let mut tx = store.pool.begin(&ACME).await.unwrap();
	let journals: Vec<serde_json::Value> = aidash_server::database::native::query_scalar(
		document["context_compaction_attempts"].as_str().unwrap(),
	)
	.scalar_all(&mut *tx)
	.await
	.unwrap();
	let usage: Vec<serde_json::Value> = aidash_server::database::native::query_scalar(
		document["inference_usage"].as_str().unwrap(),
	)
	.scalar_all(&mut *tx)
	.await
	.unwrap();
	tx.rollback().await.unwrap();
	assert_eq!(journals.len(), 3);
	for row in journals {
		assert_eq!(row["outcome"], "unavailable");
		assert_eq!(
			row["provider_dispatched"],
			serde_json::json!(expected[row["attempt_id"].as_str().unwrap()])
		);
	}
	assert_eq!(usage.len(), 1);
	assert_eq!(usage[0]["attempt_id"], serde_json::json!(physical.unwrap()));
	assert_eq!(usage[0]["outcome"], "unknown");
	// A caller cannot create a physical Summary Usage Record without its
	// matching open journal. The failed transaction retains neither row.
	let orphan = UsageDispatch {
		purpose: UsagePurpose::Summary,
		..dispatch(token)
	};
	assert!(
		store
			.record_usage_dispatch(&ACME, &run, token, &orphan)
			.await
			.is_err()
	);
	let count: i64 = sqlx::query_scalar(
		&Query::select()
			.expr(Expr::cust("COUNT(*)"))
			.from(Alias::new("inference_usage"))
			.and_where(Expr::col("attempt_id").eq(Expr::value(orphan.attempt)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(count, 0);
	cleanup(store, &url, &schema).await;
}
