//! Exact journal replay and removal of every retained projection variant.
use super::*;
use aidash_domain::{
	context::{ContextEvent, summary::entries_digest},
	provider::ToolCall,
};
use reinhardt::query::{Func, Order};

async fn journal_bytes(store: &Store, run: Uuid) -> Vec<(Value, String)> {
	let sql = Query::select()
		.columns([Alias::new("event"), Alias::new("digest")])
		.from(Alias::new("run_context_events"))
		.and_where(Expr::col(Alias::new("run_id")).eq(Expr::value(run)))
		.order_by(Alias::new("seq"), Order::Asc)
		.to_string(PostgresQueryBuilder);
	sqlx::query_as(&sql)
		.fetch_all(store.pool.fixture_driver())
		.await
		.unwrap()
}
async fn payload_count(store: &Store, run: Uuid) -> i64 {
	let sql = Query::select()
		.expr(Func::count(Expr::col(Alias::new("run_id")).into()))
		.from(Alias::new("research_journal_results"))
		.and_where(Expr::col(Alias::new("run_id")).eq(Expr::value(run)))
		.and_where(Expr::col(Alias::new("bytes")).is_not_null())
		.to_string(PostgresQueryBuilder);
	sqlx::query_scalar(&sql)
		.fetch_one(store.pool.fixture_driver())
		.await
		.unwrap()
}

#[rstest::rstest]
#[case::shortened(false)]
#[case::elided(true)]
#[tokio::test]
async fn journal_replay_is_exact_until_expiry_and_late_workers_cannot_recreate_it(
	#[future(awt)]
	#[from(store)]
	fixture: StoreFixture,
	#[case] elided: bool,
) {
	let (store, url, schema) = fixture.parts();
	let mut run = leased_run(&store, &agent(&store).await).await;
	let worker = run.lease_owner.unwrap();
	store.renew_lease(&ACME, run.id, worker, 300).await.unwrap();
	let evidence = aidash_server::research::evidence_store(store.clone(), ACME.clone());
	let research = Research {
		store: &evidence,
		gate: &Permit,
		limits: Limits::default(),
	};
	let provider = Termed {
		fixture: FixtureResearch::default(),
		terms: terms("journal-account"),
	};
	let output = research
		.fetch(
			&Call {
				run: run.id,
				context_revision: 0,
				key: "f1",
				provider: "research.fixture@1",
				disclosure_approval: true,
			},
			&provider,
			&json!({"url":"https://research-fixture.example/strasse"}),
		)
		.await
		.unwrap();
	assert_eq!(output["status"], "ok", "{output}");
	run.context.push_invocation(
		ContextEvent::tool(
			ToolCall {
				id: "call-1".into(),
				name: "research_fetch".into(),
				arguments: json!({"url":"https://research-fixture.example/strasse"}),
			},
			output.clone(),
		),
		"f1",
	);
	store
		.save_run(&ACME, &run, worker, "run.sources_observed")
		.await
		.unwrap();
	let original_journal = journal_bytes(&store, run.id).await;
	assert_eq!(original_journal.len(), 1);
	assert_eq!(
		original_journal[0].0["result"]["contract"],
		aidash_domain::context::research::CONTRACT
	);
	assert!(original_journal[0].0["result"].get("data").is_none());
	let executor = store.for_run(ACME.clone());
	// Selection checkpoints reload the durable row, whose Research results
	// are references. The continuing step must still see materialized content.
	executor.observe_sources(&mut run, worker).await.unwrap();
	assert!(matches!(
		&run.context.history[0].event,
		ContextEvent::Tool { result, .. } if result == &output
	));
	let replay = executor.context_journal(run.id, 1, 1).await.unwrap();
	let ContextEvent::Tool { result, .. } = &replay[0].event else {
		panic!()
	};
	assert_eq!(
		serde_json::to_vec(result).unwrap(),
		serde_json::to_vec(&output).unwrap()
	);
	let commitment = entries_digest(&replay);
	let mut variant = store.run(&ACME, run.id).await.unwrap();
	let ContextEvent::Tool { result, .. } = &mut variant.context.history[0].event else {
		panic!()
	};
	let variant_value = if elided {
		ContextEvent::elided_result("call-1")
	} else {
		json!(format!("{}… shortened", output))
	};
	*result = variant_value.clone();
	store
		.save_run(&ACME, &variant, worker, "run.sources_observed")
		.await
		.unwrap();
	let recovered = store.run(&ACME, run.id).await.unwrap();
	let ContextEvent::Tool { result, .. } = &recovered.context.history[0].event else {
		panic!()
	};
	assert_eq!(
		result, &variant_value,
		"recovery preserves the fitted projection"
	);
	assert_eq!(payload_count(&store, run.id).await, 2);
	// Neither the latest projection nor its journal currently needs the
	// shortened variant. Its orphan copy must still be purged.
	variant.context.history.clear();
	if elided {
		use aidash_domain::context::summary::{ExecutionSummary, SummaryContent, SummaryProvider};
		variant.context.execution_summary = Some(Box::new(
			ExecutionSummary::merge(
				SummaryContent {
					goal: "goal".into(),
					constraints: vec![],
					decisions: vec!["Provider-derived summary text to purge".into()],
					unresolved: vec![],
					resolved: vec![],
					artifacts: vec![],
					verification: vec![],
				},
				None,
				&replay,
				Default::default(),
				"context-recovery/1",
				SummaryProvider {
					model: aidash_domain::registry::EntityRef {
						id: "summarizer".into(),
						version: "1".into(),
					},
					definition_digest: "digest".into(),
				},
			)
			.unwrap(),
		));
	}
	store
		.save_run(&ACME, &variant, worker, "run.sources_observed")
		.await
		.unwrap();
	age(&store, run.id, 31).await;
	assert_eq!(
		aidash_server::research::sweep_account_terms(&store, &[provider.terms])
			.await
			.unwrap(),
		1
	);
	assert_eq!(payload_count(&store, run.id).await, 0);
	let purged_projection = store.run(&ACME, run.id).await.unwrap();
	assert!(purged_projection.context.execution_summary.is_none());
	assert!(
		!serde_json::to_string(&purged_projection.context)
			.unwrap()
			.contains("Provider-derived summary text")
	);
	assert_eq!(journal_bytes(&store, run.id).await, original_journal);
	let expired = executor.context_journal(run.id, 1, 1).await.unwrap();
	assert_eq!(entries_digest(&expired), commitment);
	assert!(
		matches!(&expired[0].event, ContextEvent::Tool { result, .. } if ContextEvent::is_expired_result(result))
	);
	// A worker still holding the original plaintext cannot repopulate any
	// payload or projection after the sweep's durable tombstone.
	store
		.save_run(&ACME, &run, worker, "run.sources_observed")
		.await
		.unwrap();
	assert_eq!(payload_count(&store, run.id).await, 0);
	let recovered = store.run(&ACME, run.id).await.unwrap();
	assert!(
		matches!(&recovered.context.history[0].event, ContextEvent::Tool { result, .. } if ContextEvent::is_expired_result(result))
	);
	assert_eq!(journal_bytes(&store, run.id).await, original_journal);
	if elided {
		// This candidate was computed before the purge. Its adoption must
		// fail atomically rather than retaining a compressed copy afterward.
		let error = store
			.save_run(&ACME, &variant, worker, "context.compacted")
			.await
			.unwrap_err();
		assert!(
			error
				.to_string()
				.contains("Research changed during summary adoption"),
			"{error}"
		);
		assert!(
			store
				.run(&ACME, run.id)
				.await
				.unwrap()
				.context
				.execution_summary
				.is_none()
		);
	}
	cleanup(store, &url, &schema).await;
}

#[rstest::rstest]
#[case::corrupted_payload(true)]
#[case::missing_payload(false)]
#[tokio::test]
async fn journal_references_never_resolve_to_unverified_content(
	#[future(awt)]
	#[from(store)]
	fixture: StoreFixture,
	#[case] corrupt: bool,
) {
	let (store, url, schema) = fixture.parts();
	let mut run = leased_run(&store, &agent(&store).await).await;
	let worker = run.lease_owner.unwrap();
	run.context.push_invocation(ContextEvent::tool(ToolCall { id:"call-1".into(),name:"research_search".into(),arguments:json!({"query":"q"}) }, json!({"contract":aidash_domain::research::RESULT_CONTRACT,"operation":"search","data":{"sources":[],"answer":"provider bytes"}})), "s1");
	store
		.save_run(&ACME, &run, worker, "run.sources_observed")
		.await
		.unwrap();
	let before = journal_bytes(&store, run.id).await;
	if corrupt {
		let sql = Query::update()
			.table(Alias::new("research_journal_results"))
			.value(Alias::new("bytes"), b"{\"different\":true}".to_vec())
			.and_where(Expr::col(Alias::new("run_id")).eq(Expr::value(run.id)))
			.to_string(PostgresQueryBuilder);
		sqlx::query(&sql)
			.execute(store.pool.fixture_driver())
			.await
			.unwrap();
	} else {
		let sql = Query::delete()
			.from_table(Alias::new("research_journal_results"))
			.and_where(Expr::col(Alias::new("run_id")).eq(Expr::value(run.id)))
			.to_string(PostgresQueryBuilder);
		sqlx::query(&sql)
			.execute(store.pool.fixture_driver())
			.await
			.unwrap();
	}
	let result = store
		.for_run(ACME.clone())
		.context_journal(run.id, 1, 1)
		.await;
	if corrupt {
		assert!(
			result
				.unwrap_err()
				.to_string()
				.contains("payload digest mismatch")
		);
	} else {
		let entries = result.unwrap();
		assert!(
			matches!(&entries[0].event, ContextEvent::Tool { result, .. } if result["contract"] == "aidash.research.unavailable/1")
		);
	}
	assert_eq!(journal_bytes(&store, run.id).await, before);
	cleanup(store, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn legacy_inline_journals_are_refused_without_rewriting_their_commitments(
	#[future(awt)]
	#[from(store)]
	fixture: StoreFixture,
) {
	let (store, url, schema) = fixture.parts();
	let mut run = leased_run(&store, &agent(&store).await).await;
	let worker = run.lease_owner.unwrap();
	let event = ContextEvent::tool(
		ToolCall {
			id: "old-call".into(),
			name: "research_search".into(),
			arguments: json!({"query":"q"}),
		},
		json!({"contract":aidash_domain::research::RESULT_CONTRACT,"operation":"search","data":{"sources":[],"answer":"legacy provider content"}}),
	);
	let original = serde_json::to_value(&event).unwrap();
	let digest = aidash_domain::registry::rules::digest(&original);
	let sql = Query::insert()
		.into_table(Alias::new("run_context_events"))
		.columns([
			Alias::new("run_id"),
			Alias::new("seq"),
			Alias::new("tenant"),
			Alias::new("origin"),
			Alias::new("event"),
			Alias::new("digest"),
		])
		.values_expr(vec![
			Expr::value(run.id).into(),
			Expr::value(1_i64).into(),
			Expr::value("acme").into(),
			Expr::value("appended").into(),
			Expr::value(original).into(),
			Expr::value(digest).into(),
		])
		.unwrap()
		.to_string(PostgresQueryBuilder);
	sqlx::query(&sql)
		.execute(store.pool.fixture_driver())
		.await
		.unwrap();
	let before = journal_bytes(&store, run.id).await;
	let error = store
		.for_run(ACME.clone())
		.context_journal(run.id, 1, 1)
		.await
		.unwrap_err();
	assert!(error.to_string().contains("development database reset"));
	run.context.push_invocation(event, "s1");
	let error = store
		.save_run(&ACME, &run, worker, "run.sources_observed")
		.await
		.unwrap_err();
	assert!(error.to_string().contains("development database reset"));
	assert_eq!(journal_bytes(&store, run.id).await, before);
	assert_eq!(
		payload_count(&store, run.id).await,
		0,
		"a refused save rolls back its payload freeze"
	);
	cleanup(store, &url, &schema).await;
}
