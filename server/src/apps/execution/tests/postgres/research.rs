//! Research evidence persists per Run: replayable attempts with cost
//! attribution, Run-scoped snapshots and fragments, and citation lookup. The
//! host research use case runs against PostgreSQL with the fixture provider.
use super::*;
use aidash_application::{
	ports::{
		execution::ExecutionStore,
		research::{
			Budget, DisclosureGate, DisclosureState, EvidenceStore, Reservation, Reserved,
			WebDisclosure,
		},
	},
	research::{Call, Research},
};
use aidash_domain::research::{
	Operation,
	account::CostAttribution,
	citations::EvidenceRef,
	evidence::{AttemptOutcome, AttemptState, Limits},
};
use aidash_integrations::research::fixture::FixtureResearch;

struct Permit;
#[async_trait::async_trait]
impl DisclosureGate for Permit {
	async fn admit(
		&self,
		_: Uuid,
		_: &WebDisclosure,
	) -> aidash_application::Result<DisclosureState> {
		Ok(DisclosureState::Permitted)
	}
	async fn consume(&self, _: Uuid, _: Uuid) -> aidash_application::Result<bool> {
		Ok(true)
	}
}

async fn agent(store: &Store) -> Entry {
	seed(&Registry::new(store.pool.clone(), &store.node_id).unwrap()).await
}

async fn leased_run(store: &Store, agent: &Entry) -> Run {
	let workspace = store
		.create_workspace("Research", "Read public sources")
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
	store.lease_run(Uuid::new_v4(), 30).await.unwrap().unwrap()
}

fn reservation(key: &str, account: Option<(&str, u64)>) -> Reservation {
	Reservation {
		key: key.into(),
		operation: Operation::Search,
		provider: "research.brave@1".into(),
		destination: "api.search.brave.com".into(),
		attribution: account.map(|(account_id, price)| CostAttribution {
			account_id: account_id.into(),
			price_micro_usd: price,
		}),
		limits: Limits {
			search_attempts: 3,
			..Limits::default()
		},
		budget: account.map(|_| Budget {
			since: chrono::Utc::now() - chrono::Duration::days(1),
			limit_micro_usd: 10_000,
		}),
	}
}

#[rstest::rstest]
#[tokio::test]
async fn attempts_are_reserved_once_settled_once_and_charged_to_the_operator_account(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let (store, url, schema) = setup(&environment).await;
	let run = leased_run(&store, &agent(&store).await).await;
	let evidence = aidash_server::research::evidence_store(store.clone());
	let since = chrono::Utc::now() - chrono::Duration::hours(1);

	assert_eq!(
		evidence
			.reserve(run.id, &reservation("k1", Some(("operator", 5_000))))
			.await
			.unwrap(),
		Reserved::Reserved
	);
	assert_eq!(
		evidence
			.reserve(run.id, &reservation("k1", None))
			.await
			.unwrap(),
		Reserved::Duplicate
	);
	assert_eq!(
		evidence.attempt(run.id, "k1").await.unwrap(),
		Some(AttemptState::Dispatched)
	);
	// Unknown outcomes are charged while dispatched.
	assert_eq!(
		evidence.account_spend("operator", since).await.unwrap(),
		5_000
	);
	let result = json!({"status":"ok"});
	evidence
		.settle(run.id, "k1", AttemptOutcome::Completed, &result)
		.await
		.unwrap();
	assert!(matches!(
		evidence
			.settle(
				run.id,
				"k1",
				AttemptOutcome::Failed,
				&json!({"status":"error"}),
			)
			.await,
		Err(aidash_application::Error::Conflict(_))
	));
	assert_eq!(
		evidence.attempt(run.id, "k1").await.unwrap(),
		Some(AttemptState::Settled {
			outcome: AttemptOutcome::Completed,
			result
		}),
		"a settled attempt is never rewritten"
	);
	assert_eq!(
		evidence
			.reserve(run.id, &reservation("k2", Some(("operator", 5_000))))
			.await
			.unwrap(),
		Reserved::Reserved
	);
	// A failed attempt is never charged.
	evidence
		.settle(
			run.id,
			"k2",
			AttemptOutcome::Failed,
			&json!({"status":"error"}),
		)
		.await
		.unwrap();
	assert_eq!(
		evidence.account_spend("operator", since).await.unwrap(),
		5_000
	);
	assert_eq!(evidence.usage(run.id).await.unwrap().search_attempts, 2);
	assert_eq!(
		evidence
			.reserve(run.id, &reservation("k3", Some(("operator", 6_000))))
			.await
			.unwrap(),
		Reserved::BudgetExhausted
	);
	assert_eq!(
		evidence
			.reserve(run.id, &reservation("k4", None))
			.await
			.unwrap(),
		Reserved::Reserved
	);
	assert_eq!(
		evidence
			.reserve(run.id, &reservation("k5", None))
			.await
			.unwrap(),
		Reserved::AttemptLimit
	);
	assert_eq!(evidence.attempt(Uuid::new_v4(), "k1").await.unwrap(), None);
	cleanup(store, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn fixture_research_runs_the_host_contract_on_postgres_and_citations_stay_run_scoped(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	let (store, url, schema) = setup(&environment).await;
	let agent = agent(&store).await;
	let run = leased_run(&store, &agent).await;
	let evidence = aidash_server::research::evidence_store(store.clone());
	let research = Research {
		store: &evidence,
		gate: &Permit,
		limits: Limits::default(),
	};
	let fixture = FixtureResearch::default();
	let call = |key| Call {
		run: run.id,
		context_revision: 0,
		key,
		provider: "research.fixture@1",
		disclosure_approval: true,
	};

	let found = research
		.search(&call("s1"), &fixture, &json!({"query":"strasse"}))
		.await
		.unwrap();
	assert_eq!(found["status"], "ok", "{found}");
	let source = found["data"]["sources"][0]["source_id"].clone();
	let replay = research
		.search(&call("s1"), &fixture, &json!({"query":"strasse"}))
		.await
		.unwrap();
	assert_eq!(replay, found, "replay keeps the Source record identity");

	let opened = research
		.fetch(
			&call("f1"),
			&fixture,
			&json!({"source_id":source,"max_bytes":5}),
		)
		.await
		.unwrap();
	assert_eq!(opened["status"], "partial", "{opened}");
	assert_eq!(opened["data"]["text"], "Alpha");
	let document = opened["data"]["document_id"].clone();
	let next = research
		.fetch(
			&call("f2"),
			&fixture,
			&json!({"document_id":document,"cursor":opened["data"]["next_cursor"],"max_bytes":24}),
		)
		.await
		.unwrap();
	assert!(
		next["data"]["text"]
			.as_str()
			.unwrap()
			.starts_with(" line\nDie Straße")
	);

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
	assert_eq!(hit["data"]["matches"][0]["pages"], json!([2]));

	// Citation lookup accepts only this Run's delivered fragments.
	let invented = EvidenceRef(Uuid::new_v4());
	assert_eq!(
		ExecutionStore::unknown_evidence(&store, &run, &[reference, invented])
			.await
			.unwrap(),
		[invented]
	);
	let other = leased_run(&store, &agent).await;
	assert_eq!(
		ExecutionStore::unknown_evidence(&store, &other, &[reference])
			.await
			.unwrap(),
		[reference]
	);
	let foreign = research
		.find(
			&Call {
				run: other.id,
				..call("q2")
			},
			&json!({"document_id":document,"query":"line"}),
		)
		.await
		.unwrap();
	assert_eq!(foreign["error"]["code"], "unknown_document");

	let usage = evidence.usage(run.id).await.unwrap();
	assert_eq!(
		(usage.search_attempts, usage.fetch_attempts, usage.snapshots),
		(1, 1, 1)
	);
	cleanup(store, &url, &schema).await;
}

#[rstest::rstest]
#[tokio::test]
async fn the_search_credential_never_reaches_results_or_evidence(
	#[future(awt)]
	#[from(test_environment)]
	environment: Arc<TestEnvironment>,
) {
	use tokio::io::{AsyncReadExt, AsyncWriteExt};
	const KEY: &str = "sentinel-brave-tool-secret";
	let (store, url, schema) = setup(&environment).await;
	let run = leased_run(&store, &agent(&store).await).await;
	// A Brave-compatible endpoint that requires the key and returns one result.
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!(
		"http://{}/res/v1/web/search",
		listener.local_addr().unwrap()
	);
	let server = tokio::spawn(async move {
		let (mut socket, _) = listener.accept().await.unwrap();
		let mut request = Vec::new();
		let mut buffer = [0u8; 4096];
		while !request.windows(4).any(|window| window == b"\r\n\r\n") {
			let read = socket.read(&mut buffer).await.unwrap();
			request.extend_from_slice(&buffer[..read]);
		}
		assert!(String::from_utf8_lossy(&request).contains(KEY));
		let body = json!({"type":"search","web":{"results":[{"url":"https://example.com/a","title":"Example","description":"hint"}]}}).to_string();
		let response = format!(
			"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
			body.len()
		);
		socket.write_all(response.as_bytes()).await.unwrap();
	});
	let now = chrono::Utc::now();
	let profile = aidash_domain::research::account::AccountProfile {
		account_id: "operator".into(),
		agreement_reference: "contract".into(),
		verified_by: "operator".into(),
		verified_at: now - chrono::Duration::days(1),
		review_after: now + chrono::Duration::days(1),
		storage_permitted: true,
		query_training_prohibited: true,
		evaluation_permitted: true,
		retention_days: 30,
		deletion_after_termination_days: 30,
		credential_env: "AIDASH_SECRET_BRAVE_SEARCH".into(),
		price_effective_at: now - chrono::Duration::days(1),
		price_review_after: now + chrono::Duration::days(1),
		request_price_micro_usd: 5_000,
		monthly_base_fee_micro_usd: 0,
		monthly_limit_micro_usd: 1_000_000,
	};
	let brave = aidash_integrations::research::brave::BraveSearch::with_endpoint(
		profile,
		KEY,
		reqwest::Url::parse(&endpoint).unwrap(),
	)
	.unwrap();
	let evidence = aidash_server::research::evidence_store(store.clone());
	let research = Research {
		store: &evidence,
		gate: &Permit,
		limits: Limits::default(),
	};
	let result = research
		.search(
			&Call {
				run: run.id,
				context_revision: 0,
				key: "brave-1",
				provider: "research.brave@1",
				disclosure_approval: true,
			},
			&brave,
			&json!({"query":"rust"}),
		)
		.await
		.unwrap();
	server.await.unwrap();
	assert_eq!(result["status"], "ok", "{result}");
	assert!(!result.to_string().contains(KEY));
	for table in ["research_attempts", "research_sources"] {
		use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};
		let sql = Query::select()
			.expr(Expr::cust("row_to_json(t)::text"))
			.from_as(Alias::new(table), Alias::new("t"))
			.and_where(Expr::col(Alias::new("run_id")).eq(Expr::value(run.id)))
			.to_string(PostgresQueryBuilder);
		let rows: Vec<String> = sqlx::query_scalar(&sql)
			.fetch_all(store.pool.driver())
			.await
			.unwrap();
		assert!(!rows.is_empty(), "{table}");
		assert!(
			rows.iter().all(|row| !row.contains(KEY)),
			"{table}: {rows:?}"
		);
	}
	cleanup(store, &url, &schema).await;
}
