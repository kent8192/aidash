//! Built-in Memory searches by meaning only with its Thread index's embedding (#1123).
use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// The fact and the query share no word, so only the embedding can join them:
/// texts about Friday land on one axis, anything else on another.
#[derive(Clone)]
struct BuiltinSemanticState {
	fail_queries: Arc<AtomicBool>,
	embedding_calls: Arc<AtomicUsize>,
}
#[fixture]
fn builtin_semantic_state() -> BuiltinSemanticState {
	BuiltinSemanticState {
		fail_queries: Arc::new(AtomicBool::new(false)),
		embedding_calls: Arc::new(AtomicUsize::new(0)),
	}
}
#[fixture]
fn builtin_semantic_router(
	#[from(builtin_semantic_state)] state: BuiltinSemanticState,
) -> std::sync::Arc<Router> {
	let fail = state.fail_queries;
	let calls = state.embedding_calls;
	std::sync::Arc::new(
		reinhardt::test::stub::StubRouter::new()
			.route(
				"/v1/embeddings",
				http::Method::POST,
				reply(move |request: reinhardt::Request| {
					let input = request.json::<serde_json::Value>().unwrap();
					let fail = fail.clone();
					let calls = calls.clone();
					async move {
						calls.fetch_add(1, Ordering::SeqCst);
						let text = input["input"].to_string();
						if fail.load(Ordering::SeqCst) && text.contains('?') {
							return reinhardt::Response::new(http::StatusCode::SERVICE_UNAVAILABLE);
						}
						let vector = if text.contains("Friday") || text.contains("金曜") {
							[1.0, 0.0, 0.0]
						} else {
							[0.0, 1.0, 0.0]
						};
						reinhardt::Response::ok()
							.with_json(&json!({"model":input["model"],"data":[{"index":0,"embedding":vector}],"usage":{"prompt_tokens":1,"total_tokens":1}}))
							.unwrap()
					}
				}),
			)
			.into_server_router(),
	)
}
struct BuiltinSemanticProvider {
	state: BuiltinSemanticState,
	server: reinhardt::test::fixtures::server::TestServerGuard,
}
#[fixture]
async fn builtin_semantic_provider(
	#[from(builtin_semantic_state)] state: BuiltinSemanticState,
	#[from(builtin_semantic_router)]
	#[with(state.clone())]
	_router: std::sync::Arc<Router>,
	#[future(awt)]
	#[from(upstream::upstream)]
	#[with(_router.clone())]
	server: reinhardt::test::fixtures::server::TestServerGuard,
) -> BuiltinSemanticProvider {
	BuiltinSemanticProvider { state, server }
}

fn index_change(
	endpoint: &str,
	expected_revision: i64,
	enabled: bool,
	acknowledge_memory_embeddings: bool,
) -> aidash_server::semantic::ConfigureIndex {
	aidash_server::semantic::ConfigureIndex {
		expected_revision,
		acknowledge_memory_embeddings,
		spec: aidash_server::semantic::IndexSpec {
			embedding: serde_json::from_value(json!({"provider":"openai","endpoint":endpoint,"model":"fixture","model_version":"1.0.0","dimensions":3})).unwrap(),
			vector: aidash_domain::semantic::VectorConfig {
				provider: "postgres".into(),
				endpoint: "local".into(),
				credential_env: None,
			},
			enabled,
			auto_context: false,
			max_sources: 64,
			max_results: 4,
			max_result_tokens: 8192,
			max_input_bytes: 16384,
			max_image_bytes: 8 * 1024 * 1024,
			max_images: 256,
		},
	}
}

async fn projection_state(store: &Store, id: Uuid) -> Option<String> {
	aidash_server::database::native::query_scalar(
		&Query::select()
			.column(Alias::new("state"))
			.from(Alias::new("semantic_entries"))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(store.pool.in_scope(&TEST_FIXTURE))
	.await
	.unwrap()
}

async fn recall_with(store: &Store, bank: &Bank, provider: &str, text: &str) -> Recall {
	let operation = memory::Operation {
		operation_id: Uuid::now_v7(),
		provider: reference(provider),
		bank: bank.clone(),
		action: memory::Action::Recall {
			query: RecallQuery {
				text: text.into(),
				time: None,
				kinds: vec![],
				max_tokens: 8192,
			},
		},
	};
	match memory::operate(store, &administrator(store).await, operation)
		.await
		.unwrap()
	{
		memory::Outcome::Recall(recall) => recall,
		other => panic!("unexpected memory outcome: {other:?}"),
	}
}

async fn recall(store: &Store, bank: &Bank, text: &str) -> Recall {
	recall_with(store, bank, "aidash.memory", text).await
}

#[rstest]
#[tokio::test]
async fn builtin_memory_uses_thread_index_embedding(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
	#[future(awt)]
	#[from(builtin_semantic_provider)]
	fixture: BuiltinSemanticProvider,
) {
	let endpoint = format!("{}/v1", fixture.server.url);
	let database = database.await;
	let (store, registry, thread) = setup_endpoint(&database, bounds, &endpoint).await;
	let (proof, recorder) = super::review_lifecycle::journal_proof_with_bank(&store, thread).await;
	let actor = administrator(&store).await;
	let bank = Bank {
		home: aidash_domain::nodes::NodeAddress::derived(store.deployment.as_str(), "acme")
			.to_string(),
		tenant: "acme".into(),
		thread,
		participant: None,
	};
	memory::operate(
		&store,
		&actor,
		memory::Operation {
			operation_id: Uuid::now_v7(),
			provider: reference("aidash.memory"),
			bank: bank.clone(),
			action: memory::Action::ConfigureBank {
				expected_revision: 0,
			},
		},
	)
	.await
	.unwrap();

	let friday = Uuid::now_v7();
	let plants = Uuid::now_v7();
	for (id, text) in [
		(friday, "Releases are frozen on Fridays."),
		(plants, "The office plants need water."),
	] {
		memory::mutate(
			&store,
			&actor,
			Mutation {
				provider: reference("aidash.memory"),
				..mutation(
					&bank,
					Change::Add {
						id,
						content: Content {
							evidence: vec![proof.clone()],
							..content(text)
						},
					},
				)
			},
		)
		.await
		.unwrap();
		assert_eq!(
			projection_state(&store, id).await.as_deref(),
			Some("PENDING")
		);
	}
	// A fact not yet embedded is left to keyword search, which shares no word.
	let question = "金曜日にデプロイしてもいい?";
	assert_eq!(recall(&store, &bank, question).await, Recall::Empty);

	// Facts admitted before the index existed have no projection; configuring
	// the index embeds them.
	aidash_server::database::native::query(
		&Query::delete()
			.from_table(Alias::new("semantic_points"))
			.and_where(Expr::col("entry_id").is_in([Expr::value(friday), Expr::value(plants)]))
			.to_string(PostgresQueryBuilder),
	)
	.execute(store.pool.in_scope(&TEST_FIXTURE))
	.await
	.unwrap();
	aidash_server::database::native::query(
		&Query::delete()
			.from_table(Alias::new("semantic_entries"))
			.and_where(Expr::col("id").is_in([Expr::value(friday), Expr::value(plants)]))
			.to_string(PostgresQueryBuilder),
	)
	.execute(store.pool.in_scope(&TEST_FIXTURE))
	.await
	.unwrap();
	assert_eq!(projection_state(&store, friday).await, None);
	aidash_server::semantic::service::configure(
		&store,
		&actor,
		thread,
		index_change(&endpoint, 1, true, false),
	)
	.await
	.unwrap();
	assert!(projection_state(&store, friday).await.is_some());
	let budget = Uuid::now_v7();
	let db = store.pool.fixture_driver();
	sqlx::query("INSERT INTO budgets (id, tenant, scope_kind, scope_key, period, hard_limit_nanocredits, on_hard_limit, recipients, revision, created_at, created_by, updated_at) VALUES ($1, 'acme', 'tenant', '', 'monthly', 1, 'stop', '{}', 1, now(), 'alice', now())")
		.bind(budget)
		.execute(db)
		.await
		.unwrap();
	sqlx::query("INSERT INTO budget_spend (tenant, day, node, root_subject, duty_id, spent_nanocredits, unpriced_calls) VALUES ('acme', date_trunc('day', now(), 'UTC'), 'elsewhere', '', '00000000-0000-0000-0000-000000000000', 1, 0)")
		.execute(db)
		.await
		.unwrap();
	aidash_server::semantic::worker::sweep(&store)
		.await
		.unwrap();
	assert_eq!(fixture.state.embedding_calls.load(Ordering::SeqCst), 0);
	assert_eq!(
		projection_state(&store, friday).await.as_deref(),
		Some("ERROR")
	);
	sqlx::query("UPDATE budgets SET hard_limit_nanocredits = 1000000000 WHERE id = $1")
		.bind(budget)
		.execute(db)
		.await
		.unwrap();
	sqlx::query("UPDATE semantic_entries SET next_attempt = now() WHERE id IN ($1, $2)")
		.bind(friday)
		.bind(plants)
		.execute(db)
		.await
		.unwrap();
	tokio::time::timeout(std::time::Duration::from_secs(10), async {
		loop {
			aidash_server::semantic::worker::sweep(&store)
				.await
				.unwrap();
			let mut ready = true;
			for id in [friday, plants] {
				let state = projection_state(&store, id).await;
				if state.as_deref() == Some("ERROR") {
					let error: Option<String> = aidash_server::database::native::query_scalar(
						&Query::select()
							.column(Alias::new("last_error"))
							.from(Alias::new("semantic_entries"))
							.and_where(Expr::col("id").eq(Expr::value(id)))
							.to_string(PostgresQueryBuilder),
					)
					.scalar_one(store.pool.in_scope(&TEST_FIXTURE))
					.await
					.unwrap();
					panic!("built-in indexing failed: {error:?}");
				}
				ready &= state.as_deref() == Some("READY");
			}
			if ready {
				break;
			}
			tokio::time::sleep(std::time::Duration::from_millis(50)).await;
		}
	})
	.await
	.expect("built-in facts must be embedded once the index is enabled");
	let unpriced: i64 = sqlx::query_scalar(
		"SELECT unpriced_calls FROM budget_spend WHERE tenant = 'acme' AND node = $1 AND root_subject = '' AND duty_id = '00000000-0000-0000-0000-000000000000'",
	)
	.bind(aidash_domain::nodes::NodeName::derived("acme").to_string())
	.fetch_one(db)
	.await
	.unwrap();
	assert_eq!(unpriced, 2);
	let Recall::Ready { units } = recall(&store, &bank, question).await else {
		panic!("the Friday fact must be found by meaning");
	};
	assert_eq!(units[0].id, friday, "the closest fact ranks first");

	// An embedding outage leaves keyword search instead of failing the recall.
	fixture.state.fail_queries.store(true, Ordering::SeqCst);
	let Recall::Ready { units } = recall(&store, &bank, "Releases?").await else {
		panic!("the Friday fact must be found by keyword during the outage");
	};
	assert_eq!(units[0].id, friday);
	fixture.state.fail_queries.store(false, Ordering::SeqCst);

	// The optional embedding must fall back before any I/O at ledger capacity.
	let config = aidash_domain::registry::bindings::memory_defaults::definitions()
		.unwrap()
		.into_iter()
		.find(aidash_domain::registry::bindings::memory_defaults::is_builtin_memory)
		.unwrap()
		.config;
	let cap = config["policy"]["retention"]["max_model_operations"]
		.as_i64()
		.unwrap();
	sqlx::query(
		"INSERT INTO memory_model_operations (id, bank_id, provider_id, provider_version, digest, calls, tokens, cost_micros, created_at, tenant)
		 SELECT gen_random_uuid(), b.id, 'aidash.memory', '1.0.0', 'capacity fixture', 0, 0, 0, now(), b.tenant
		 FROM memory_banks b CROSS JOIN generate_series(1, $2::bigint)
		 WHERE b.thread_id = $1 AND b.participant_id IS NULL
		 LIMIT $2 - (SELECT count(*) FROM memory_model_operations m JOIN memory_banks b ON m.bank_id = b.id
		 WHERE b.thread_id = $1 AND b.participant_id IS NULL)",
	)
	.bind(thread)
	.bind(cap)
	.execute(db)
	.await
	.unwrap();
	let before = fixture.state.embedding_calls.load(Ordering::SeqCst);
	let Recall::Ready { units } = recall(&store, &bank, "Releases?").await else {
		panic!("the Friday fact must be found by keyword at ledger capacity");
	};
	assert_eq!(units[0].id, friday);
	assert_eq!(fixture.state.embedding_calls.load(Ordering::SeqCst), before);
	sqlx::query("DELETE FROM memory_model_operations WHERE digest = 'capacity fixture'")
		.execute(db)
		.await
		.unwrap();

	// The recorder's explicit provider pins the index; moving its empty bank to
	// an Agent version without a memory Binding leaves only Built-in Memory.
	let mut built_in = entry(
		"agent",
		"a",
		json!({"model":reference("m"),"instructions":"Default memory","tools":[],"skills":[]}),
	);
	built_in.version = "1.1.0".into();
	registry
		.register(
			&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
			built_in,
		)
		.await
		.unwrap();
	memory::upgrade_participant(
		&store,
		&administrator(&store).await,
		recorder,
		memory::UpgradeParticipant {
			agent: EntityRef {
				id: "a".into(),
				version: "1.1.0".into(),
			},
			expected_revision: 1,
		},
	)
	.await
	.unwrap();
	let actor = administrator(&store).await;
	let pending = Uuid::now_v7();
	memory::mutate(
		&store,
		&actor,
		Mutation {
			provider: reference("aidash.memory"),
			..mutation(
				&bank,
				Change::Add {
					id: pending,
					content: Content {
						evidence: vec![proof.clone()],
						..content("The conference room needs a projector.")
					},
				},
			)
		},
	)
	.await
	.unwrap();
	assert_eq!(
		projection_state(&store, pending).await.as_deref(),
		Some("PENDING")
	);
	// The warning counts the two vectors, excluding the pending projection.
	// Disabling the index deletes the facts' embeddings, so it must be confirmed.
	let refused = aidash_server::semantic::service::configure(
		&store,
		&actor,
		thread,
		index_change(&endpoint, 2, false, false),
	)
	.await;
	assert!(
		matches!(&refused, Err(aidash_server::Error::Conflict(message)) if message.contains("deletes the embeddings of 2 memory facts")),
		"unexpected unconfirmed change: {refused:?}"
	);
	aidash_server::semantic::service::configure(
		&store,
		&actor,
		thread,
		index_change(&endpoint, 2, false, true),
	)
	.await
	.unwrap();
	assert_eq!(recall(&store, &bank, question).await, Recall::Empty);

	// A deletion while the index is disabled still reaches its projection, so
	// enabling the index again cannot bring the fact back.
	memory::mutate(
		&store,
		&actor,
		Mutation {
			provider: reference("aidash.memory"),
			..mutation(
				&bank,
				Change::Delete {
					id: friday,
					expected_revision: 1,
				},
			)
		},
	)
	.await
	.unwrap();
	assert_eq!(
		projection_state(&store, friday).await.as_deref(),
		Some("DELETED")
	);
	aidash_server::semantic::service::configure(
		&store,
		&actor,
		thread,
		index_change(&endpoint, 3, true, false),
	)
	.await
	.unwrap();
	assert_eq!(
		projection_state(&store, friday).await.as_deref(),
		Some("DELETED")
	);

	let mut keyword = config;
	keyword["policy"]["thread_index_embedding"] = json!(false);
	keyword["policy"]["reranker"] = json!(reference("r"));
	keyword["policy"]["tokenizer"] = json!(reference("t"));
	// Registered policies pin their own model roles.
	for role in ["extraction", "derivation", "reflection"] {
		keyword["policy"][role] = json!(reference("m"));
	}
	registry
		.register(
			&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
			entry("memory", "keyword-only", keyword),
		)
		.await
		.unwrap();
	memory::operate(
		&store,
		&actor,
		memory::Operation {
			operation_id: Uuid::now_v7(),
			provider: reference("keyword-only"),
			bank: bank.clone(),
			action: memory::Action::ConfigureBank {
				expected_revision: 1,
			},
		},
	)
	.await
	.unwrap();
	assert_eq!(
		projection_state(&store, plants).await.as_deref(),
		Some("DELETED")
	);
	let Recall::Ready { units } = recall_with(&store, &bank, "keyword-only", "plants").await else {
		panic!("the keyword policy must keep the plants fact");
	};
	assert_eq!(units[0].id, plants);
}
