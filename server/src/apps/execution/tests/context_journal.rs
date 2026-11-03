//! The Context Journal is authoritative; `runs.context` is its lossy projection.
#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
#[path = "support/legacy.rs"]
mod common;

use aidash_application::ports::execution::ExecutionStore;
use aidash_application::tenancy::TEST_FIXTURE;
use aidash_domain::context::{
	ContextEvent, HistoryEntry, JournalCursor,
	recovery::{Attempt, Failure, Outcome, Settlement, Stage},
};
use aidash_server::{domain::Run, federation::Federation, store::Store};
use common::{
	ApplicationFixture, TestApplication, bootstrap, cleanup, native_application, request,
};
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, Order, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
	SimpleExpr,
};
use serde_json::{Value, json};
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
			research: None,
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
	.fetch_all(store.pool.fixture_driver())
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
	.fetch_all(store.pool.fixture_driver())
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
	#[from(native_application)]
	fixture: ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application.clone();
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (mut run, worker) = leased(&f, &app, &token).await;
	// Arrange: two events recovered from a legacy projection, one new event.
	project(&mut run, 3, 2);
	// Act
	f.store
		.save_run(&TEST_FIXTURE, &run, worker, "run.sources_observed")
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
		.save_run(&TEST_FIXTURE, &run, worker, "run.sources_observed")
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
	assert_eq!(
		f.store
			.run(&TEST_FIXTURE, run.id)
			.await
			.unwrap()
			.context
			.history
			.len(),
		2
	);
	let range = ExecutionStore::context_journal(&f.store.for_run(TEST_FIXTURE), run.id, 2, 3)
		.await
		.unwrap();
	assert_eq!(
		range,
		vec![
			HistoryEntry {
				research: None,
				seq: 2,
				event: event(2)
			},
			HistoryEntry {
				research: None,
				seq: 3,
				event: event(3)
			},
		]
	);
	assert!(
		ExecutionStore::context_journal(&f.store.for_run(TEST_FIXTURE), run.id, 5, 9)
			.await
			.unwrap()
			.is_empty()
	);
	f.store
		.release_lease(&TEST_FIXTURE, run.id, worker)
		.await
		.unwrap();
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn begin_compaction_abandons_open_attempts_and_enforces_the_call_budget(
	#[future(awt)]
	#[from(native_application)]
	fixture: ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application.clone();
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (mut run, worker) = leased(&f, &app, &token).await;
	project(&mut run, 3, 0);
	f.store
		.save_run(&TEST_FIXTURE, &run, worker, "run.sources_observed")
		.await
		.unwrap();
	let first = attempt(&run, 1, 2);
	let second = attempt(&run, 1, 3);
	let exhausted = attempt(&run, 1, 3);
	// Act
	ExecutionStore::begin_compaction(&f.store.for_run(TEST_FIXTURE), &run, worker, &first, 2)
		.await
		.unwrap();
	ExecutionStore::begin_compaction(&f.store.for_run(TEST_FIXTURE), &run, worker, &second, 2)
		.await
		.unwrap();
	let refused = ExecutionStore::begin_compaction(
		&f.store.for_run(TEST_FIXTURE),
		&run,
		worker,
		&exhausted,
		2,
	)
	.await;
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
		ExecutionStore::begin_compaction(
			&f.store.for_run(TEST_FIXTURE),
			&run,
			Uuid::new_v4(),
			&attempt(&run, 1, 3),
			9
		)
		.await,
		Err(aidash_application::Error::Conflict(_))
	));
	f.store
		.release_lease(&TEST_FIXTURE, run.id, worker)
		.await
		.unwrap();
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn adoption_requires_an_open_attempt_and_its_journaled_range(
	#[future(awt)]
	#[from(native_application)]
	fixture: ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application.clone();
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (mut run, worker) = leased(&f, &app, &token).await;
	project(&mut run, 3, 0);
	f.store
		.save_run(&TEST_FIXTURE, &run, worker, "run.sources_observed")
		.await
		.unwrap();
	let mut compacted = run.clone();
	compacted.context.history.retain(|entry| entry.seq == 3);
	compacted.context.compactions += 1;
	// An unknown attempt is not open.
	assert!(matches!(
		ExecutionStore::adopt_compaction(
			&f.store.for_run(TEST_FIXTURE),
			&compacted,
			worker,
			Uuid::new_v4(),
			&adopted(),
			None,
		)
		.await,
		Err(aidash_application::Error::Conflict(_))
	));
	// The journal does not cover the attempt's source range.
	let beyond = attempt(&run, 1, 9);
	ExecutionStore::begin_compaction(&f.store.for_run(TEST_FIXTURE), &run, worker, &beyond, 9)
		.await
		.unwrap();
	assert!(matches!(
		ExecutionStore::adopt_compaction(
			&f.store.for_run(TEST_FIXTURE),
			&compacted,
			worker,
			beyond.id,
			&adopted(),
			None,
		)
		.await,
		Err(aidash_application::Error::Conflict(_))
	));
	assert_eq!(
		f.store
			.run(&TEST_FIXTURE, run.id)
			.await
			.unwrap()
			.context
			.history
			.len(),
		3
	);
	assert_eq!(
		outcomes(&f.store, run.id).await,
		vec![(beyond.id, None, false)]
	);
	// A settled attempt cannot be adopted.
	let insufficient = attempt(&run, 1, 2);
	ExecutionStore::begin_compaction(
		&f.store.for_run(TEST_FIXTURE),
		&run,
		worker,
		&insufficient,
		9,
	)
	.await
	.unwrap();
	ExecutionStore::settle_compaction(
		&f.store.for_run(TEST_FIXTURE),
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
		ExecutionStore::adopt_compaction(
			&f.store.for_run(TEST_FIXTURE),
			&compacted,
			worker,
			insufficient.id,
			&adopted(),
			None,
		)
		.await,
		Err(aidash_application::Error::Conflict(_))
	));
	// Act: an open attempt whose range is journaled.
	let accepted = attempt(&run, 1, 2);
	ExecutionStore::begin_compaction(&f.store.for_run(TEST_FIXTURE), &run, worker, &accepted, 9)
		.await
		.unwrap();
	ExecutionStore::adopt_compaction(
		&f.store.for_run(TEST_FIXTURE),
		&compacted,
		worker,
		accepted.id,
		&adopted(),
		None,
	)
	.await
	.unwrap();
	// Assert: projection, attempt and event changed together.
	let saved = f.store.run(&TEST_FIXTURE, run.id).await.unwrap();
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
	.fetch_one(f.store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(compactions, 1);
	// Adoption is single-use.
	assert!(matches!(
		ExecutionStore::adopt_compaction(
			&f.store.for_run(TEST_FIXTURE),
			&compacted,
			worker,
			accepted.id,
			&adopted(),
			None,
		)
		.await,
		Err(aidash_application::Error::Conflict(_))
	));
	f.store
		.release_lease(&TEST_FIXTURE, run.id, worker)
		.await
		.unwrap();
	cleanup(f, &url, &schema).await;
}

/// Source checkpoints adopt their stored revision and retain the lease so
/// the same step can continue after a pinned estimate or workspace recovery.
#[rstest::rstest]
#[case::estimate(false)]
#[case::workspace_recovered(true)]
#[tokio::test]
async fn a_source_checkpoint_is_durable_and_keeps_the_lease(
	#[case] recovered: bool,
	#[future(awt)]
	#[from(native_application)]
	fixture: ApplicationFixture,
) {
	// Arrange
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application.clone();
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (mut run, worker) = leased(&f, &app, &token).await;
	run.recovery.degraded.outage = true;
	run = f
		.store
		.save_run(&TEST_FIXTURE, &run, worker, "run.sources_observed")
		.await
		.unwrap();
	let snapshot =
		aidash_domain::provider::estimate::EstimateSnapshot::calibrated(run.step, 250_000, 20);
	run.context.estimate.snapshot = Some(snapshot);
	let revision = run.revision;
	// Act
	if recovered {
		run.recovery.degraded.recovered();
		ExecutionStore::recover_sources(&f.store.for_run(TEST_FIXTURE), &mut run, worker)
			.await
			.unwrap();
	} else {
		ExecutionStore::pin_estimate(&f.store.for_run(TEST_FIXTURE), &mut run, worker)
			.await
			.unwrap();
	}
	// Assert: the saved revision is adopted, the lease still admits the step's
	// next save, and a reload reads the snapshot.
	assert_eq!(run.revision, revision + 1);
	assert_eq!(run.recovery.degraded.outage, !recovered);
	f.store
		.save_run(&TEST_FIXTURE, &run, worker, "run.sources_observed")
		.await
		.unwrap();
	assert_eq!(
		f.store
			.run(&TEST_FIXTURE, run.id)
			.await
			.unwrap()
			.context
			.estimate
			.snapshot,
		Some(snapshot)
	);
	f.store
		.release_lease(&TEST_FIXTURE, run.id, worker)
		.await
		.unwrap();
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn provider_tools_activity_remains_inspectable_after_projection_compaction(
	#[future(awt)]
	#[from(native_application)]
	fixture: ApplicationFixture,
) {
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application.clone();
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (mut run, worker) = leased(&f, &app, &token).await;
	project(&mut run, 0, 0);
	let activity = json!({"type":"web_search_call","action":{"query":"test"},"result":{"url":"https://example.com"}});
	run.context.push(ContextEvent::Assistant {
		epoch: 1,
		text: "Reported source".into(),
		calls: vec![],
		provider_activity: vec![activity.clone()],
	});
	run.context.push(event(2));
	f.store
		.save_run(&TEST_FIXTURE, &run, worker, "run.sources_observed")
		.await
		.unwrap();
	run.context.history.clear();
	f.store
		.save_run(&TEST_FIXTURE, &run, worker, "run.sources_observed")
		.await
		.unwrap();
	let (status, details) = request(
		&app,
		&token,
		"GET",
		&format!("/api/runs/{}", run.id),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{details}");
	assert!(
		details["run"]["context"]["history"]
			.as_array()
			.unwrap()
			.is_empty()
	);
	assert_eq!(
		details["provider_tool_activity"].as_array().unwrap().len(),
		1
	);
	assert_eq!(
		details["provider_tool_activity"][0]["items"],
		json!([activity])
	);
	let (status, page) = request(
		&app,
		&token,
		"GET",
		&format!("/api/runs/{}?offset=100", run.id),
		Value::Null,
	)
	.await;
	assert_eq!(status, 200, "{page}");
	assert!(page.get("provider_tool_activity").is_none());
	f.store
		.release_lease(&TEST_FIXTURE, run.id, worker)
		.await
		.unwrap();
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn provider_container_pin_survives_recovery_and_terminal_state(
	#[future(awt)]
	#[from(native_application)]
	fixture: ApplicationFixture,
) {
	use aidash_application::{
		provider_access::{Context, ProviderAccess},
		tenancy::TenancyScope,
	};
	use aidash_server::apps::{
		execution::models::{
			Run as Record,
			states::{RunControl, RunPhase},
		},
		identity::repositories::provider_credentials::AdmittedAccess,
	};
	use reinhardt::db::orm::Model;
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application.clone();
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (run, worker) = leased(&f, &app, &token).await;
	let context = Context {
		tenant: "acme".into(),
		run: Some(run.id),
		..Default::default()
	};
	let policy = json!({"type":"disabled"});
	let first = AdmittedAccess {
		store: f.store.clone(),
	}
	.provider_container(&context, &policy)
	.await
	.unwrap();
	assert_eq!(first, format!("aidash_{}", run.id.simple()));
	let recovered = AdmittedAccess {
		store: f.store.clone(),
	};
	assert_eq!(
		recovered
			.provider_container(&context, &policy)
			.await
			.unwrap(),
		first
	);
	assert!(
		recovered
			.provider_container(
				&context,
				&json!({"type":"allowlist","allowed_domains":["example.com"]})
			)
			.await
			.unwrap_err()
			.to_string()
			.contains("NETWORK_CHANGED")
	);
	let scope = TenancyScope::tenant("acme").unwrap();
	for (phase, control) in [
		(RunPhase::Failed, RunControl::Active),
		(RunPhase::Cancelled, RunControl::Cancelled),
	] {
		f.store
			.control_pool
			.atomic(&scope, async |tx| {
				Record::objects()
					.filter(Record::field_id().eq(run.id))
					.update_fields_with_conn(tx, [(Record::field_phase(), phase)])
					.await?;
				Record::objects()
					.filter(Record::field_id().eq(run.id))
					.update_fields_with_conn(tx, [(Record::field_control(), control)])
					.await?;
				Ok(())
			})
			.await
			.unwrap();
		assert!(
			recovered
				.provider_container(&context, &policy)
				.await
				.unwrap_err()
				.to_string()
				.contains("RUN_ENDED")
		);
		let pin = f
			.store
			.control_pool
			.atomic(&scope, async |tx| {
				let row = Record::objects()
					.filter(Record::field_id().eq(run.id))
					.all_with_executor(tx)
					.await
					.map_err(reinhardt::core::exception::Error::from)?
					.pop()
					.unwrap();
				Ok(row.provider_container.unwrap().0)
			})
			.await
			.unwrap();
		assert_eq!(pin, json!({"id":first,"network_policy":policy}));
	}
	f.store
		.release_lease(&TEST_FIXTURE, run.id, worker)
		.await
		.unwrap();
	cleanup(f, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn provider_warning_approval_persists_its_exact_disclosure(
	#[future(awt)]
	#[from(native_application)]
	fixture: ApplicationFixture,
) {
	use aidash_application::tenancy::TenancyScope;
	use aidash_server::apps::identity::models::AuthorizationCatalogHistory;
	use reinhardt::db::orm::Model;
	let (f, url, schema) = fixture.runtime.parts();
	let app = fixture.application.clone();
	let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
	let (run, worker) = leased(&f, &app, &token).await;
	let mut input = json!({"node":run.node,"entry":{"id":"provider.openrouter.shell","version":"1.0.0"},"enabled":true,"expected_revision":0});
	let path = "/api/authorization/acme/catalog".to_owned();
	let (status, body) = request(&app, &token, "POST", &path, input.clone()).await;
	assert_eq!(status, 400, "{body}");
	assert!(
		body.to_string()
			.contains("PROVIDER_TOOL_ACKNOWLEDGMENT_REQUIRED")
	);
	input["provider_warning_acknowledged"] = json!(true);
	let (status, body) = request(&app, &token, "POST", &path, input).await;
	assert_eq!(status, 200, "{body}");
	let records = f
		.store
		.control_pool
		.atomic(&TenancyScope::tenant("acme").unwrap(), async |tx| {
			Ok(AuthorizationCatalogHistory::objects()
				.filter(
					AuthorizationCatalogHistory::field_entry_key().eq("provider.openrouter.shell"),
				)
				.all_with_executor(tx)
				.await
				.map_err(reinhardt::core::exception::Error::from)?)
		})
		.await
		.unwrap();
	assert_eq!(records.len(), 1);
	assert_eq!(records[0].revision, 1);
	assert_eq!(
		records[0].provider_warning_acknowledgment,
		aidash_domain::tool::provider_tools::warning_acknowledgment(
			"provider.openrouter.shell",
			"1.0.0"
		)
	);
	f.store
		.release_lease(&TEST_FIXTURE, run.id, worker)
		.await
		.unwrap();
	cleanup(f, &url, &schema).await;
}
