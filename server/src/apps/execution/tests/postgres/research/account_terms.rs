//! Provider results under the Research account terms (ADR 0058): the sweep
//! replaces every stored copy with a tombstone once its basis ends, keeps
//! what citations need, and leaves pages Aidash's own reader fetched alone.
use super::*;
use aidash_application::ports::research::{Outbound, PageProvider, SearchProvider};
use aidash_domain::research::{
	Failure,
	account::AccountProfile,
	evidence::{PurgeReason, passage_digest},
	search::{ProviderResults, SearchQuery},
};
use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
use serde_json::Value;

/// The fixture corpus served by a service with Research account terms.
struct Termed {
	fixture: FixtureResearch,
	terms: AccountProfile,
}

#[async_trait::async_trait]
impl SearchProvider for Termed {
	fn disclosure(&self, query: &SearchQuery) -> std::result::Result<Outbound, Failure> {
		self.fixture.disclosure(query)
	}
	fn account(&self) -> std::result::Result<Option<AccountProfile>, Failure> {
		Ok(Some(self.terms.clone()))
	}
	async fn search(&self, query: &SearchQuery) -> std::result::Result<ProviderResults, Failure> {
		self.fixture.search(query).await
	}
}

#[async_trait::async_trait]
impl PageProvider for Termed {
	fn admit(&self, url: &reqwest::Url) -> std::result::Result<(), Failure> {
		self.fixture.admit(url)
	}
	fn account(&self) -> std::result::Result<Option<AccountProfile>, Failure> {
		Ok(Some(self.terms.clone()))
	}
	async fn fetch(&self, url: &reqwest::Url) -> std::result::Result<ExtractedPage, Failure> {
		self.fixture.fetch(url).await
	}
}

fn terms(account: &str) -> AccountProfile {
	let now = chrono::Utc::now();
	AccountProfile {
		account_id: account.into(),
		agreement_reference: "contract".into(),
		verified_by: "deployment_administrator".into(),
		verified_at: now - chrono::Duration::days(1),
		review_after: now + chrono::Duration::days(1),
		storage_permitted: true,
		query_training_prohibited: true,
		evaluation_permitted: true,
		retention_days: 30,
		deletion_after_termination_days: 7,
		terminated_at: None,
		credential_env: "AIDASH_SECRET_TERMED".into(),
		price_effective_at: now - chrono::Duration::days(1),
		price_review_after: now + chrono::Duration::days(1),
		request_price_micro_usd: 5_000,
		monthly_base_fee_micro_usd: 0,
		monthly_limit_micro_usd: 1_000_000,
	}
}

/// Date every attempt of `run` back by `days`, as if retrieved then.
async fn age(store: &Store, run: Uuid, days: i64) {
	let sql = Query::update()
		.table(Alias::new("research_attempts"))
		.value(
			Alias::new("created_at"),
			chrono::Utc::now() - chrono::Duration::days(days),
		)
		.and_where(Expr::col(Alias::new("run_id")).eq(Expr::value(run)))
		.to_string(PostgresQueryBuilder);
	sqlx::query(&sql)
		.execute(store.pool.driver())
		.await
		.unwrap();
}

/// Move the recorded end of `account` back to `days` ago.
async fn ended_ago(store: &Store, account: &str, days: i64) {
	let sql = Query::update()
		.table(Alias::new("research_account_terminations"))
		.value(
			Alias::new("terminated_at"),
			chrono::Utc::now() - chrono::Duration::days(days),
		)
		.and_where(Expr::col(Alias::new("account_id")).eq(Expr::value(account)))
		.to_string(PostgresQueryBuilder);
	sqlx::query(&sql)
		.execute(store.pool.driver())
		.await
		.unwrap();
}

/// `purge_reason` and the recorded result's error code of one attempt.
async fn attempt(store: &Store, run: Uuid, key: &str) -> (Option<String>, Value) {
	let sql = Query::select()
		.expr(Expr::cust(
			"json_build_object('reason', purge_reason, 'result', result)::text",
		))
		.from(Alias::new("research_attempts"))
		.and_where(Expr::col(Alias::new("run_id")).eq(Expr::value(run)))
		.and_where(Expr::col(Alias::new("invocation_key")).eq(Expr::value(key)))
		.to_string(PostgresQueryBuilder);
	let row: String = sqlx::query_scalar(&sql)
		.fetch_one(store.pool.driver())
		.await
		.unwrap();
	let row: Value = serde_json::from_str(&row).unwrap();
	(
		row["reason"].as_str().map(str::to_owned),
		row["result"]["error"]["code"].clone(),
	)
}

#[rstest::rstest]
#[tokio::test]
async fn expired_provider_results_become_tombstones_that_still_verify_citations(
	#[future(awt)]
	#[from(store)]
	_store_fixture: StoreFixture,
) {
	// Arrange: a search and a read through a service with account terms, a
	// find in that page, and a read of the same Source by Aidash's own reader.
	let (store, url, schema) = _store_fixture.parts();
	let run = leased_run(&store, &agent(&store).await).await;
	let evidence = aidash_server::research::evidence_store(store.clone(), TEST_FIXTURE);
	let research = Research {
		store: &evidence,
		gate: &Permit,
		limits: Limits::default(),
	};
	let termed = Termed {
		fixture: FixtureResearch::default(),
		terms: terms("termed"),
	};
	let native = FixtureResearch::default();
	let call = |key| Call {
		run: run.id,
		context_revision: 0,
		key,
		provider: "research.fixture@1",
		disclosure_approval: true,
	};
	let found = research
		.search(&call("s1"), &termed, &json!({"query":"strasse"}))
		.await
		.unwrap();
	let source: Uuid =
		serde_json::from_value(found["data"]["sources"][0]["source_id"].clone()).unwrap();
	let read = research
		.fetch(&call("f1"), &termed, &json!({"source_id":source}))
		.await
		.unwrap();
	let document: Uuid = serde_json::from_value(read["data"]["document_id"].clone()).unwrap();
	let hit = research
		.find(
			&call("q1"),
			&json!({"document_id":document,"query":"STRASSE"}),
		)
		.await
		.unwrap();
	let reference: EvidenceRef = hit["data"]["matches"][0]["evidence_ref"]
		.as_str()
		.unwrap()
		.parse()
		.unwrap();
	let delivered = evidence
		.fragments(run.id, &[reference])
		.await
		.unwrap()
		.remove(0);
	let own = research
		.fetch(&call("f2"), &native, &json!({"source_id":source}))
		.await
		.unwrap();
	let own_document: Uuid = serde_json::from_value(own["data"]["document_id"].clone()).unwrap();
	let swept = || {
		aidash_server::research::sweep_account_terms(&store, std::slice::from_ref(&termed.terms))
	};

	// Act: within the terms nothing goes; past them the sweep purges.
	age(&store, run.id, 29).await;
	assert_eq!(swept().await.unwrap(), 0);
	age(&store, run.id, 31).await;
	let purged = swept().await.unwrap();

	// Assert: both Provider results are tombstones, marked by their reason.
	assert_eq!(purged, 2);
	for key in ["s1", "f1"] {
		assert_eq!(
			attempt(&store, run.id, key).await,
			(Some("account_terms".into()), json!("result_expired")),
			"{key}"
		);
	}
	// Aidash's own read of the same Source is not a Provider result.
	assert_eq!(attempt(&store, run.id, "f2").await.0, None);
	let own = evidence
		.snapshot(run.id, own_document)
		.await
		.unwrap()
		.unwrap();
	assert!(own.readable().is_ok());
	assert!(!own.lines.is_empty());
	// The Source keeps its identity and address, not the provider's title.
	let kept = evidence.source(run.id, source).await.unwrap().unwrap();
	assert_eq!(kept.title, "");
	assert!(!kept.url.is_empty());
	// The snapshot keeps identity and digests; the model may read it again.
	let snapshot = evidence.snapshot(run.id, document).await.unwrap().unwrap();
	assert!(snapshot.lines.is_empty());
	assert_eq!(
		snapshot.purged.map(|purge| purge.reason),
		Some(PurgeReason::AccountTerms)
	);
	assert_eq!(snapshot.readable(), Err(Failure::SnapshotPurged));
	// The fragment's text is gone; its digest still verifies the citation.
	let fragment = evidence
		.fragments(run.id, &[reference])
		.await
		.unwrap()
		.remove(0);
	assert_eq!(fragment.text, "");
	assert_eq!(fragment.passage_digest, passage_digest(&delivered.text));
	assert_eq!(fragment.text_digest, delivered.text_digest);
	// The Harness learns which results to replace in later requests.
	let expired = ExecutionStore::expired_research(&store.for_run(TEST_FIXTURE), &run)
		.await
		.unwrap();
	assert_eq!(expired.sources, [source].into());
	assert_eq!(expired.documents, [document].into());
	assert!(expired.through.is_some());
	// The stored projection records the purges applied to it.
	let sql = Query::select()
		.expr(Expr::cust("context->>'research_expired_through'"))
		.from(Alias::new("runs"))
		.and_where(Expr::col(Alias::new("id")).eq(Expr::value(run.id)))
		.to_string(PostgresQueryBuilder);
	let watermark: Option<String> = sqlx::query_scalar(&sql)
		.fetch_one(store.pool.driver())
		.await
		.unwrap();
	assert_eq!(
		watermark.map(|at| at.parse::<chrono::DateTime<chrono::Utc>>().unwrap()),
		expired.through
	);
	// A later sweep finds nothing left to purge.
	assert_eq!(swept().await.unwrap(), 0);
	cleanup(store, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_recorded_termination_deletes_results_by_its_deadline_even_once_undeclared(
	#[future(awt)]
	#[from(store)]
	_store_fixture: StoreFixture,
) {
	// Arrange: a recent search under terms that keep results for 30 days.
	let (store, url, schema) = _store_fixture.parts();
	let run = leased_run(&store, &agent(&store).await).await;
	let evidence = aidash_server::research::evidence_store(store.clone(), TEST_FIXTURE);
	let research = Research {
		store: &evidence,
		gate: &Permit,
		limits: Limits::default(),
	};
	let termed = Termed {
		fixture: FixtureResearch::default(),
		terms: terms("ending"),
	};
	let found = research
		.search(
			&Call {
				run: run.id,
				context_revision: 0,
				key: "s1",
				provider: "research.fixture@1",
				disclosure_approval: true,
			},
			&termed,
			&json!({"query":"strasse"}),
		)
		.await
		.unwrap();
	assert_eq!(found["status"], "ok", "{found}");
	age(&store, run.id, 2).await;
	let mut ended = termed.terms.clone();

	// Act: the agreement ended yesterday; its deletion period has not passed.
	ended.terminated_at = Some(chrono::Utc::now() - chrono::Duration::days(1));
	let within = aidash_server::research::sweep_account_terms(&store, &[ended.clone()])
		.await
		.unwrap();
	// The declaration is removed; the recorded end still applies, and a
	// later end never replaces it.
	ended_ago(&store, "ending", 8).await;
	ended.terminated_at = Some(chrono::Utc::now());
	let undeclared = aidash_server::research::sweep_account_terms(&store, &[])
		.await
		.unwrap();
	let again = aidash_server::research::sweep_account_terms(&store, &[ended])
		.await
		.unwrap();

	// Assert
	assert_eq!((within, undeclared, again), (0, 1, 0));
	assert_eq!(
		attempt(&store, run.id, "s1").await,
		(Some("account_terms".into()), json!("result_expired"))
	);
	let sql = Query::select()
		.expr(Expr::cust("count(*)"))
		.from(Alias::new("research_account_terminations"))
		.and_where(Expr::col(Alias::new("account_id")).eq(Expr::value("ending")))
		.and_where(
			Expr::col(Alias::new("terminated_at"))
				.lt(Expr::value(chrono::Utc::now() - chrono::Duration::days(7))),
		)
		.to_string(PostgresQueryBuilder);
	let recorded: i64 = sqlx::query_scalar(&sql)
		.fetch_one(store.pool.driver())
		.await
		.unwrap();
	assert_eq!(recorded, 1);
	cleanup(store, &url, &schema).await;
}
