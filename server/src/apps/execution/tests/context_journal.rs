//! The Context Journal is authoritative; `runs.context` is its lossy projection.
#[path = "support/legacy.rs"]
mod common;

use aidash_application::ports::execution::ExecutionStore;
use aidash_domain::context::{
	ContextEvent, HistoryEntry, JournalCursor,
	recovery::{Attempt, Failure, Outcome, Settlement, Stage},
};
use aidash_server::{domain::Run, federation::Federation, store::Store};
use common::{
	TestApplication, TestEnvironment, bootstrap, cleanup, request, setup, test_environment,
};
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, Order, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
	SimpleExpr,
};
use serde_json::{Value, json};
use std::sync::Arc;
use uuid::Uuid;

fn event(n: u64) -> ContextEvent {
	ContextEvent::ModelMediaObservation {
		text: format!("observation {n}"),
		through_seq: None,
		truncated: false,
	}
}

/// A leased Run whose projection holds `1..=head`, with `1..=imported` recovered
/// from a pre-journal projection.
async fn leased(f: &Federation, app: &TestApplication, token: &str) -> (Run, Uuid) {
	let (status, created) = request(app, token, "POST", "/api/conversations", json!({
		"title":"Journal", "goal":"Reply", "target":{"id":"research","version":"1.0.0"},"target_kind":"agent"
	}))
	.await;
	assert_eq!(status, 200, "{created}");
	let worker = Uuid::new_v4();
	let run = f
		.store
		.lease_run(worker, 30)
		.await
		.unwrap()
		.expect("leased Run");
	(run, worker)
}

fn project(run: &mut Run, head: u64, imported: u64) {
	run.context.history = (1..=head)
		.map(|seq| HistoryEntry {
			seq,
			event: event(seq),
		})
		.collect();
	run.context.journal = JournalCursor {
		head,
		imported_through: imported,
		inferred_through: 0,
	};
}

async fn journal_rows(store: &Store, run: Uuid) -> Vec<(i64, String, Value, String)> {
	let rows = sqlx::query(
		&Query::select()
			.columns(["seq", "origin", "event", "digest"].map(Alias::new))
			.from(Alias::new("run_context_events"))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.order_by(Alias::new("seq"), Order::Asc)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(store.pool.driver())
	.await
	.unwrap();
	use sqlx::Row as _;
	rows.into_iter()
		.map(|row| {
			(
				row.get("seq"),
				row.get("origin"),
				row.get("event"),
				row.get("digest"),
			)
		})
		.collect()
}

async fn outcomes(store: &Store, run: Uuid) -> Vec<(Uuid, Option<String>, bool)> {
	let rows = sqlx::query(
		&Query::select()
			.columns(["id", "outcome", "settled_at"].map(Alias::new))
			.from(Alias::new("context_compaction_attempts"))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.order_by(Alias::new("created_at"), Order::Asc)
			.order_by(Alias::new("id"), Order::Asc)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(store.pool.driver())
	.await
	.unwrap();
	use sqlx::Row as _;
	rows.into_iter()
		.map(|row| {
			(
				row.get("id"),
				row.get("outcome"),
				row.get::<Option<chrono::DateTime<chrono::Utc>>, _>("settled_at")
					.is_some(),
			)
		})
		.collect()
}

fn attempt(run: &Run, from: u64, through: u64) -> Attempt {
	Attempt {
		id: Uuid::new_v4(),
		run_id: run.id,
		stage: Stage::Summary,
		policy_version: "context-recovery/1".into(),
		provider: "summarizer@1.0.0#sha256:fixture".into(),
		source_from_seq: from,
		source_through_seq: through,
		base_revision: run.revision,
		observed_input_seq: run.observed_input_seq,
		before_tokens: 1_000,
	}
}

fn adopted() -> Settlement {
	Settlement {
		outcome: Outcome::Adopted,
		reason: None,
		candidate_digest: Some("sha256:candidate".into()),
		after_tokens: Some(100),
	}
}

#[rstest::rstest]
#[tokio::test]
async fn saves_journal_original_events_with_their_origin(
	#[future(awt)]
	#[from(test_environment)]
	test_environment: Arc<TestEnvironment>,
) {
	let (f, url, schema) = setup(&test_environment).await;
	let app = common::application(f.clone()).await;
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (mut run, worker) = leased(&f, &app, &token).await;
	// Arrange: two events recovered from a legacy projection, one new event.
	project(&mut run, 3, 2);
	// Act
	f.store
		.save_run(&run, worker, "run.sources_observed")
		.await
		.unwrap();
	// Assert
	let rows = journal_rows(&f.store, run.id).await;
	assert_eq!(
		rows.iter()
			.map(|(seq, origin, _, _)| (*seq, origin.as_str()))
			.collect::<Vec<_>>(),
		vec![(1, "imported"), (2, "imported"), (3, "appended")]
	);
	for (seq, _, stored, digest) in &rows {
		assert_eq!(*stored, serde_json::to_value(event(*seq as u64)).unwrap());
		assert_eq!(*digest, aidash_domain::registry::rules::digest(stored));
	}
	// Arrange: the projection drops early events and appends a fourth.
	run.context.history.retain(|entry| entry.seq == 3);
	run.context.push(event(4));
	// Act
	f.store
		.save_run(&run, worker, "run.sources_observed")
		.await
		.unwrap();
	// Assert: the journal keeps the originals; the projection is lossy.
	let rows = journal_rows(&f.store, run.id).await;
	assert_eq!(
		rows.iter()
			.map(|(seq, origin, _, _)| (*seq, origin.as_str()))
			.collect::<Vec<_>>(),
		vec![
			(1, "imported"),
			(2, "imported"),
			(3, "appended"),
			(4, "appended")
		]
	);
	assert_eq!(f.store.run(run.id).await.unwrap().context.history.len(), 2);
	let range = ExecutionStore::context_journal(&f.store, run.id, 2, 3)
		.await
		.unwrap();
	assert_eq!(
		range,
		vec![
			HistoryEntry {
				seq: 2,
				event: event(2)
			},
			HistoryEntry {
				seq: 3,
				event: event(3)
			},
		]
	);
	assert!(
		ExecutionStore::context_journal(&f.store, run.id, 5, 9)
			.await
			.unwrap()
			.is_empty()
	);
	f.store.release_lease(run.id, worker).await.unwrap();
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn begin_compaction_abandons_open_attempts_and_enforces_the_call_budget(
	#[future(awt)]
	#[from(test_environment)]
	test_environment: Arc<TestEnvironment>,
) {
	let (f, url, schema) = setup(&test_environment).await;
	let app = common::application(f.clone()).await;
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (mut run, worker) = leased(&f, &app, &token).await;
	project(&mut run, 3, 0);
	f.store
		.save_run(&run, worker, "run.sources_observed")
		.await
		.unwrap();
	let first = attempt(&run, 1, 2);
	let second = attempt(&run, 1, 3);
	let exhausted = attempt(&run, 1, 3);
	// Act
	ExecutionStore::begin_compaction(&f.store, &run, worker, &first, 2)
		.await
		.unwrap();
	ExecutionStore::begin_compaction(&f.store, &run, worker, &second, 2)
		.await
		.unwrap();
	let refused = ExecutionStore::begin_compaction(&f.store, &run, worker, &exhausted, 2).await;
	// Assert: the spent budget is typed, and every earlier attempt is settled.
	assert!(
		matches!(
			refused,
			Err(aidash_application::Error::Context(
				Failure::SummaryUnavailable
			))
		),
		"{refused:?}"
	);
	assert_eq!(
		outcomes(&f.store, run.id).await,
		vec![
			(first.id, Some("abandoned".into()), true),
			(second.id, Some("abandoned".into()), true),
		]
	);
	// A foreign lease cannot record an attempt.
	assert!(matches!(
		ExecutionStore::begin_compaction(&f.store, &run, Uuid::new_v4(), &attempt(&run, 1, 3), 9)
			.await,
		Err(aidash_application::Error::Conflict(_))
	));
	f.store.release_lease(run.id, worker).await.unwrap();
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn adoption_requires_an_open_attempt_and_its_journaled_range(
	#[future(awt)]
	#[from(test_environment)]
	test_environment: Arc<TestEnvironment>,
) {
	let (f, url, schema) = setup(&test_environment).await;
	let app = common::application(f.clone()).await;
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (mut run, worker) = leased(&f, &app, &token).await;
	project(&mut run, 3, 0);
	f.store
		.save_run(&run, worker, "run.sources_observed")
		.await
		.unwrap();
	let mut compacted = run.clone();
	compacted.context.history.retain(|entry| entry.seq == 3);
	compacted.context.compactions += 1;
	// An unknown attempt is not open.
	assert!(matches!(
		ExecutionStore::adopt_compaction(&f.store, &compacted, worker, Uuid::new_v4(), &adopted())
			.await,
		Err(aidash_application::Error::Conflict(_))
	));
	// The journal does not cover the attempt's source range.
	let beyond = attempt(&run, 1, 9);
	ExecutionStore::begin_compaction(&f.store, &run, worker, &beyond, 9)
		.await
		.unwrap();
	assert!(matches!(
		ExecutionStore::adopt_compaction(&f.store, &compacted, worker, beyond.id, &adopted()).await,
		Err(aidash_application::Error::Conflict(_))
	));
	assert_eq!(f.store.run(run.id).await.unwrap().context.history.len(), 3);
	assert_eq!(
		outcomes(&f.store, run.id).await,
		vec![(beyond.id, None, false)]
	);
	// A settled attempt cannot be adopted.
	let insufficient = attempt(&run, 1, 2);
	ExecutionStore::begin_compaction(&f.store, &run, worker, &insufficient, 9)
		.await
		.unwrap();
	ExecutionStore::settle_compaction(
		&f.store,
		&run,
		worker,
		insufficient.id,
		&Settlement {
			outcome: Outcome::Insufficient,
			reason: None,
			candidate_digest: Some("sha256:candidate".into()),
			after_tokens: Some(900),
		},
	)
	.await
	.unwrap();
	assert!(matches!(
		ExecutionStore::adopt_compaction(&f.store, &compacted, worker, insufficient.id, &adopted())
			.await,
		Err(aidash_application::Error::Conflict(_))
	));
	// Act: an open attempt whose range is journaled.
	let accepted = attempt(&run, 1, 2);
	ExecutionStore::begin_compaction(&f.store, &run, worker, &accepted, 9)
		.await
		.unwrap();
	ExecutionStore::adopt_compaction(&f.store, &compacted, worker, accepted.id, &adopted())
		.await
		.unwrap();
	// Assert: projection, attempt and event changed together.
	let saved = f.store.run(run.id).await.unwrap();
	assert_eq!(saved.context.history.len(), 1);
	assert_eq!(saved.context.compactions, run.context.compactions + 1);
	assert_eq!(
		outcomes(&f.store, run.id).await,
		vec![
			(beyond.id, Some("abandoned".into()), true),
			(insufficient.id, Some("insufficient".into()), true),
			(accepted.id, Some("adopted".into()), true),
		]
	);
	assert_eq!(journal_rows(&f.store, run.id).await.len(), 3);
	let compactions: i64 = sqlx::query_scalar(
		&Query::select()
			.expr(Expr::cust("COUNT(*)"))
			.from(Alias::new("events"))
			.and_where(Expr::col("kind").eq(Expr::value("context.compacted")))
			.and_where(SimpleExpr::CustomWithExpr(
				"(data->>'run_id' = ?)".to_owned(),
				vec![Expr::value(run.id.to_string()).into()],
			))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(f.store.pool.driver())
	.await
	.unwrap();
	assert_eq!(compactions, 1);
	// Adoption is single-use.
	assert!(matches!(
		ExecutionStore::adopt_compaction(&f.store, &compacted, worker, accepted.id, &adopted())
			.await,
		Err(aidash_application::Error::Conflict(_))
	));
	f.store.release_lease(run.id, worker).await.unwrap();
	cleanup(f, &url, &schema).await;
}
