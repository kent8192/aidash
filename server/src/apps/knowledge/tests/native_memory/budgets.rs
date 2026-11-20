//! Memory-role model calls and embeddings pass the Budgets like every other
//! model call, and their cost lands in the ledger (#1012, ADR 0143).
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The scripted reflection answers with the first Unit it was given, at a
/// reported cost of 0.0002 credits.
fn router(chat: Arc<AtomicUsize>) -> Arc<Router> {
	Arc::new(
		reinhardt::test::stub::StubRouter::new()
			.route(
				"/v1/chat/completions",
				http::Method::POST,
				reply(move |request: reinhardt::Request| {
					let input = request.json::<serde_json::Value>().unwrap();
					let chat = chat.clone();
					async move {
						chat.fetch_add(1, Ordering::SeqCst);
						let context: serde_json::Value = serde_json::from_str(
							input["messages"][1]["content"].as_str().unwrap(),
						)
						.unwrap();
						let unit = &context["context"][0];
						let step = json!({"step":"answer","reflection":{"text":"Tokyo's subway is useful.","evidence":[{"kind":"unit","bank":unit["bank"],"id":unit["id"],"revision":unit["revision"]}]}});
						reinhardt::Response::ok()
							.with_json(&json!({"choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":step.to_string()}}],"usage":{"prompt_tokens":10,"completion_tokens":5,"cost":0.0002}}))
							.unwrap()
					}
				}),
			)
			.route(
				"/v1/embeddings",
				http::Method::POST,
				reply(|request: reinhardt::Request| {
					let input = request.json::<serde_json::Value>().unwrap();
					async move {
						reinhardt::Response::ok()
							.with_json(&json!({"model":input["model"],"data":[{"index":0,"embedding":[1.0,0.1,0.0]}],"usage":{"prompt_tokens":1,"total_tokens":1}}))
							.unwrap()
					}
				}),
			)
			.into_server_router(),
	)
}

#[fixture]
fn chat_calls() -> Arc<AtomicUsize> {
	Arc::new(AtomicUsize::new(0))
}

#[fixture]
fn budget_router(#[from(chat_calls)] chat: Arc<AtomicUsize>) -> Arc<Router> {
	router(chat)
}

async fn ledger(store: &Store) -> (i64, i64) {
	sqlx::query_as(
		"SELECT COALESCE(SUM(spent_nanocredits), 0)::bigint, COALESCE(SUM(unpriced_calls), 0)::bigint FROM budget_spend WHERE tenant = 'acme'",
	)
	.fetch_one(store.pool.fixture_driver())
	.await
	.unwrap()
}

fn reflect(bank: &Bank) -> memory::Operation {
	memory::Operation {
		operation_id: Uuid::now_v7(),
		provider: reference("p"),
		bank: bank.clone(),
		action: memory::Action::Reflect {
			query: RecallQuery {
				text: "東京 地下鉄 subway".into(),
				time: None,
				kinds: vec![],
				max_tokens: 8192,
			},
		},
	}
}

/// A memory reflection outside any Run is charged to its Tenant and its
/// Thread's Node: a Tenant Budget at its Hard Limit refuses it before any
/// provider call, and once the limit is raised the reflection's reported cost
/// and its unpriced query embedding land in the ledger.
#[rstest]
#[tokio::test]
async fn a_hard_limit_stops_a_memory_reflection_and_its_cost_reaches_the_ledger(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
	#[from(chat_calls)] chat: Arc<AtomicUsize>,
	#[from(budget_router)]
	#[with(chat.clone())]
	_router: Arc<Router>,
	#[future(awt)]
	#[from(upstream::upstream)]
	#[with(_router.clone())]
	server: reinhardt::test::fixtures::server::TestServerGuard,
) {
	// Arrange: one indexed Unit, then a Tenant Budget at its Hard Limit.
	let retries = usize::try_from(bounds.max_retries).unwrap();
	let endpoint = format!("{}/v1", server.url);
	let database = database.await;
	let (store, _, thread) = setup_endpoint(&database, bounds, &endpoint).await;
	let participant = memory::create_participant(
		&store,
		&administrator(&store).await,
		thread,
		memory::CreateParticipant {
			agent: reference("a"),
		},
	)
	.await
	.unwrap();
	let bank = participant.bank;
	let id = Uuid::now_v7();
	memory::mutate(
		&store,
		&administrator(&store).await,
		mutation(
			&bank,
			Change::Add {
				id,
				content: content("東京の地下鉄は便利です。 Tokyo subway is useful."),
			},
		),
	)
	.await
	.unwrap();
	tokio::time::timeout(std::time::Duration::from_secs(10), async {
		loop {
			aidash_server::semantic::worker::sweep(&store)
				.await
				.unwrap();
			let state: String =
				sqlx::query_scalar("SELECT state FROM semantic_entries WHERE id = $1")
					.bind(id)
					.fetch_one(store.pool.fixture_driver())
					.await
					.unwrap();
			if state == "READY" {
				break;
			}
			tokio::time::sleep(std::time::Duration::from_millis(50)).await;
		}
	})
	.await
	.expect("the Unit must be indexed");
	let (indexed_spend, indexed_unpriced) = ledger(&store).await;
	assert_eq!(indexed_spend, 0);
	assert!(
		indexed_unpriced >= 1,
		"indexing embeddings count as unpriced calls"
	);
	let budget = Uuid::now_v7();
	sqlx::query("INSERT INTO budgets (id, tenant, scope_kind, scope_key, period, hard_limit_nanocredits, on_hard_limit, recipients, revision, created_at, created_by, updated_at) VALUES ($1, 'acme', 'tenant', '', 'monthly', 1, 'stop', '{}', 1, now(), 'alice', now())")
		.bind(budget)
		.execute(store.pool.fixture_driver())
		.await
		.unwrap();
	sqlx::query("INSERT INTO budget_spend (tenant, day, node, root_subject, duty_id, spent_nanocredits, unpriced_calls) VALUES ('acme', date_trunc('day', now(), 'UTC'), 'elsewhere', '', '00000000-0000-0000-0000-000000000000', 1, 0)")
		.execute(store.pool.fixture_driver())
		.await
		.unwrap();
	// Act: the same operation, refused more often than its retries allow.
	let operation = reflect(&bank);
	for _ in 0..=retries {
		let refused =
			memory::operate(&store, &administrator(&store).await, operation.clone()).await;
		// Assert: refused before any reflection call was sent.
		assert!(
			matches!(
				refused,
				Err(aidash_server::Error::BudgetExhausted { stop: true, .. })
			),
			"{refused:?}"
		);
	}
	assert_eq!(chat.load(Ordering::SeqCst), 0);
	assert_eq!(ledger(&store).await, (1, indexed_unpriced));
	// A refused call was never sent, so it spent none of the operation's
	// attempts.
	let attempts: i64 =
		sqlx::query_scalar("SELECT count(*) FROM memory_model_attempts WHERE operation_id = $1")
			.bind(operation.operation_id)
			.fetch_one(store.pool.fixture_driver())
			.await
			.unwrap();
	assert_eq!(attempts, 0);

	// Act: the limit is raised and the reflection runs.
	sqlx::query("UPDATE budgets SET hard_limit_nanocredits = 1000000000 WHERE id = $1")
		.bind(budget)
		.execute(store.pool.fixture_driver())
		.await
		.unwrap();
	let reflected = memory::operate(&store, &administrator(&store).await, operation)
		.await
		.unwrap();
	// Assert: its cost and its query embedding are in the ledger.
	assert!(
		matches!(reflected, memory::Outcome::Reflection(_)),
		"{reflected:?}"
	);
	assert_eq!(chat.load(Ordering::SeqCst), 1);
	let (spent, unpriced) = ledger(&store).await;
	assert_eq!(spent, 1 + 200_000);
	assert_eq!(unpriced, indexed_unpriced + 1);
	drop(server);
}
