//! Built-in Memory searches by meaning only with its Thread index's embedding (#1123).
use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// The fact and the query share no word, so only the embedding can join them:
/// texts about Friday land on one axis, anything else on another.
#[derive(Clone)]
struct BuiltinSemanticState {
	fail_embeddings: Arc<AtomicBool>,
	embedding_calls: Arc<AtomicUsize>,
	reflection_calls: Arc<AtomicUsize>,
	replay_reflection: Arc<AtomicBool>,
}
#[fixture]
fn builtin_semantic_state() -> BuiltinSemanticState {
	BuiltinSemanticState {
		fail_embeddings: Arc::new(AtomicBool::new(false)),
		embedding_calls: Arc::new(AtomicUsize::new(0)),
		reflection_calls: Arc::new(AtomicUsize::new(0)),
		replay_reflection: Arc::new(AtomicBool::new(false)),
	}
}
#[fixture]
fn builtin_semantic_router(
	#[from(builtin_semantic_state)] state: BuiltinSemanticState,
) -> std::sync::Arc<Router> {
	let fail = state.fail_embeddings;
	let calls = state.embedding_calls;
	let reflections = state.reflection_calls;
	let replay_reflection = state.replay_reflection;
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
						if fail.load(Ordering::SeqCst) {
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
			.route(
				"/v1/chat/completions",
				http::Method::POST,
				reply(move |request: reinhardt::Request| {
					let request = request.json::<serde_json::Value>().unwrap();
					let reflections = reflections.clone();
					let replay = replay_reflection.load(Ordering::SeqCst);
					async move {
						let call = reflections.fetch_add(1, Ordering::SeqCst);
						if replay && call == 0 {
							return reinhardt::Response::new(http::StatusCode::SERVICE_UNAVAILABLE);
						}
						let context: serde_json::Value = serde_json::from_str(
							request["messages"][1]["content"].as_str().unwrap(),
						).unwrap();
						let unit = &context["context"][0];
						let step = if replay && call % 2 == 1 {
							json!({"step":"recall","query":{"text":"water","time":null,"kinds":[],"max_tokens":8192}})
						} else {
							json!({"step":"answer","reflection":{"text":"The office plants need water.","evidence":[{"kind":"unit","bank":unit["bank"],"id":unit["id"],"revision":unit["revision"]}]}})
						};
						reinhardt::Response::ok()
							.with_json(&json!({"choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":step.to_string()}}],"usage":{"prompt_tokens":10,"completion_tokens":5}}))
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

async fn recall(store: &Store, bank: &Bank, text: &str) -> Recall {
	let operation = memory::Operation {
		operation_id: Uuid::now_v7(),
		provider: reference("aidash.memory"),
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
	fixture.state.fail_embeddings.store(true, Ordering::SeqCst);
	let before = fixture.state.embedding_calls.load(Ordering::SeqCst);
	let Recall::Ready { units } = recall(&store, &bank, "Releases").await else {
		panic!("the Friday fact must be found by keyword during the outage");
	};
	assert_eq!(units[0].id, friday);
	assert_eq!(
		fixture.state.embedding_calls.load(Ordering::SeqCst),
		before + 1
	);
	fixture.state.fail_embeddings.store(false, Ordering::SeqCst);

	// The optional embedding must fall back before any I/O at ledger capacity.
	let config = builtin_config();
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
	let Recall::Ready { units } = recall(&store, &bank, "Releases").await else {
		panic!("the Friday fact must be found by keyword at ledger capacity");
	};
	assert_eq!(units[0].id, friday);
	assert_eq!(fixture.state.embedding_calls.load(Ordering::SeqCst), before);
	sqlx::query("DELETE FROM memory_model_operations WHERE digest = 'capacity fixture'")
		.execute(db)
		.await
		.unwrap();

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

	let mut keyword = person_config(config);
	keyword["policy"]["thread_index_embedding"] = json!(false);
	registry
		.register(
			&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
			entry("memory", "keyword-only", keyword),
		)
		.await
		.unwrap();
	let actor = administrator(&store).await;
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
	let stored: (bool, String) =
		sqlx::query_as("SELECT deleted, text FROM memory_units WHERE id = $1")
			.bind(plants)
			.fetch_one(db)
			.await
			.unwrap();
	assert_eq!(stored, (false, "The office plants need water.".into()));
}

fn builtin_config() -> serde_json::Value {
	use aidash_domain::registry::bindings::memory_defaults;
	memory_defaults::definitions()
		.unwrap()
		.into_iter()
		.find(memory_defaults::is_builtin_memory)
		.unwrap()
		.config
}

fn person_config(mut config: serde_json::Value) -> serde_json::Value {
	// A human-operated bank uses explicit model roles.
	for role in ["extraction", "derivation", "reflection"] {
		config["policy"][role] = json!(reference("m"));
	}
	config["policy"]["reranker"] = json!(reference("r"));
	config["policy"]["tokenizer"] = json!(reference("t"));
	config["policy"]["maintain_observations"] = json!(false);
	config
}

async fn bank_with_thread_embedding(
	store: &Store,
	registry: &aidash_server::registry::Registry,
	thread: Uuid,
	config: serde_json::Value,
) -> (
	Bank,
	EntityRef,
	Uuid,
	aidash_server::authorization::identity::Actor,
) {
	let bank = Bank {
		home: aidash_domain::nodes::NodeAddress::derived(store.deployment.as_str(), "acme")
			.to_string(),
		tenant: "acme".into(),
		thread,
		participant: None,
	};
	let provider = reference("optional-embedding");
	registry
		.register(
			&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
			entry("memory", &provider.id, config),
		)
		.await
		.unwrap();
	let actor = administrator(store).await;
	memory::operate(
		store,
		&actor,
		memory::Operation {
			operation_id: Uuid::now_v7(),
			provider: provider.clone(),
			bank: bank.clone(),
			action: memory::Action::ConfigureBank {
				expected_revision: 0,
			},
		},
	)
	.await
	.unwrap();
	let id = Uuid::now_v7();
	memory::mutate(
		store,
		&actor,
		Mutation {
			provider: provider.clone(),
			..mutation(
				&bank,
				Change::Add {
					id,
					content: content("The office plants need water."),
				},
			)
		},
	)
	.await
	.unwrap();
	assert_eq!(
		projection_state(store, id).await.as_deref(),
		Some("PENDING")
	);
	(bank, provider, id, actor)
}

#[rstest]
#[case::pending_projection(false)]
#[case::failed_embedding(true)]
#[tokio::test]
async fn builtin_memory_uses_thread_index_embedding_in_reflection_replay(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
	#[case] fail_embedding: bool,
	#[future(awt)]
	#[from(builtin_semantic_provider)]
	fixture: BuiltinSemanticProvider,
) {
	let endpoint = format!("{}/v1", fixture.server.url);
	let database = database.await;
	let (store, registry, thread) = setup_endpoint(&database, bounds, &endpoint).await;
	let mut config = person_config(builtin_config());
	// A failed dispatched embedding still counts toward the durable budget.
	config["policy"]["bounds"]["max_model_calls"] = json!(4 + usize::from(fail_embedding));
	config["policy"]["bounds"]["max_model_tokens"] = json!(65536);
	// A replay waits as Pending until the last attempt's call deadline passes,
	// so backdate the attempts by more than the policy's own deadline.
	let past_deadline = config["policy"]["bounds"]["max_call_seconds"]
		.as_i64()
		.unwrap()
		+ 60;
	let (bank, provider, id, actor) =
		bank_with_thread_embedding(&store, &registry, thread, config).await;
	fixture
		.state
		.replay_reflection
		.store(true, Ordering::SeqCst);
	if fail_embedding {
		aidash_server::semantic::worker::sweep(&store)
			.await
			.unwrap();
		assert_eq!(projection_state(&store, id).await.as_deref(), Some("READY"));
		fixture.state.fail_embeddings.store(true, Ordering::SeqCst);
	}
	let mut operation = memory::Operation {
		operation_id: Uuid::now_v7(),
		provider,
		bank,
		action: memory::Action::Reflect {
			query: RecallQuery {
				text: "plants".into(),
				time: None,
				kinds: vec![],
				max_tokens: 8192,
			},
		},
	};
	assert!(
		memory::operate(&store, &actor, operation.clone())
			.await
			.is_err()
	);
	assert_eq!(fixture.state.reflection_calls.load(Ordering::SeqCst), 1);
	if fail_embedding {
		assert_eq!(fixture.state.embedding_calls.load(Ordering::SeqCst), 2);
		fixture.state.fail_embeddings.store(false, Ordering::SeqCst);
	} else {
		assert_eq!(fixture.state.embedding_calls.load(Ordering::SeqCst), 0);
		aidash_server::semantic::worker::sweep(&store)
			.await
			.unwrap();
		assert_eq!(projection_state(&store, id).await.as_deref(), Some("READY"));
	}
	let calls_before_retry = fixture.state.embedding_calls.load(Ordering::SeqCst);
	sqlx::query("UPDATE memory_model_attempts SET created_at = now() - make_interval(secs => $2) WHERE operation_id = $1")
		.bind(operation.operation_id)
		.bind(past_deadline as f64)
		.execute(store.pool.fixture_driver())
		.await
		.unwrap();

	// The initial model call replays against its original keyword context.
	// Its follow-up recall is new work and can embed under a separate identity.
	let outcome = memory::operate(&store, &actor, operation.clone())
		.await
		.unwrap();
	assert!(matches!(outcome, memory::Outcome::Reflection(_)));
	assert_eq!(fixture.state.reflection_calls.load(Ordering::SeqCst), 3);
	assert_eq!(
		fixture.state.embedding_calls.load(Ordering::SeqCst),
		calls_before_retry + 1,
		"the retry must embed only the follow-up query"
	);

	// A new reflection with an already-ready index embeds both recall queries.
	operation.operation_id = Uuid::now_v7();
	let outcome = memory::operate(&store, &actor, operation).await.unwrap();
	assert!(matches!(outcome, memory::Outcome::Reflection(_)));
	assert_eq!(fixture.state.reflection_calls.load(Ordering::SeqCst), 5);
	assert_eq!(
		fixture.state.embedding_calls.load(Ordering::SeqCst),
		calls_before_retry + 3
	);
}

#[rstest]
#[case::pending_projection(false, 65536, 0)]
#[case::token_allowance(true, 5, 0)]
#[case::cost_allowance(true, 65536, 1_000_000_000_000)]
#[tokio::test]
async fn builtin_memory_skips_optional_embedding_without_spending_a_call(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
	#[case] index_ready: bool,
	#[case] max_model_tokens: usize,
	#[case] embedding_price: u64,
	#[future(awt)]
	#[from(builtin_semantic_provider)]
	fixture: BuiltinSemanticProvider,
) {
	let endpoint = format!("{}/v1", fixture.server.url);
	let database = database.await;
	let (store, registry, thread) = setup_endpoint(&database, bounds, &endpoint).await;
	let mut config = person_config(builtin_config());
	config["policy"]["bounds"]["max_model_calls"] = json!(1);
	config["policy"]["bounds"]["max_model_tokens"] = json!(max_model_tokens);
	config["policy"]["prices"]["embedding"]["input_per_million"] = json!(embedding_price);
	let (bank, provider, id, actor) =
		bank_with_thread_embedding(&store, &registry, thread, config).await;
	if index_ready {
		aidash_server::semantic::worker::sweep(&store)
			.await
			.unwrap();
		assert_eq!(projection_state(&store, id).await.as_deref(), Some("READY"));
	}
	let index_calls = usize::from(index_ready);
	assert_eq!(
		fixture.state.embedding_calls.load(Ordering::SeqCst),
		index_calls
	);

	let query = RecallQuery {
		text: "plants".into(),
		time: None,
		kinds: vec![],
		max_tokens: 8192,
	};
	let outcome = memory::operate(
		&store,
		&actor,
		memory::Operation {
			operation_id: Uuid::now_v7(),
			provider,
			bank,
			action: if index_ready {
				memory::Action::Recall { query }
			} else {
				memory::Action::Reflect { query }
			},
		},
	)
	.await
	.unwrap();
	match outcome {
		memory::Outcome::Recall(Recall::Ready { units }) if index_ready => {
			assert_eq!(units[0].id, id)
		}
		memory::Outcome::Reflection(reflection) if !index_ready => {
			assert!(
				matches!(reflection.evidence.as_slice(), [Evidence::Unit { id: cited, revision: 1, .. }] if *cited == id)
			);
			assert_eq!(fixture.state.reflection_calls.load(Ordering::SeqCst), 1);
		}
		other => panic!("expected recall or reflection on the stored fact, got {other:?}"),
	}
	assert_eq!(
		fixture.state.embedding_calls.load(Ordering::SeqCst),
		index_calls
	);
}
