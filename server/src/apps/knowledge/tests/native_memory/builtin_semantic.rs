//! Built-in Memory searches by meaning only with its Thread index's embedding (#1123).
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

/// The fact and the query share no word, so only the embedding can join them:
/// texts about Friday land on one axis, anything else on another.
#[derive(Clone)]
struct BuiltinSemanticState {
	fail_queries: Arc<AtomicBool>,
}
#[fixture]
fn builtin_semantic_state() -> BuiltinSemanticState {
	BuiltinSemanticState {
		fail_queries: Arc::new(AtomicBool::new(false)),
	}
}
#[fixture]
fn builtin_semantic_router(
	#[from(builtin_semantic_state)] state: BuiltinSemanticState,
) -> std::sync::Arc<Router> {
	let fail = state.fail_queries.clone();
	std::sync::Arc::new(
		reinhardt::test::stub::StubRouter::new()
			.route(
				"/v1/embeddings",
				http::Method::POST,
				reply(move |request: reinhardt::Request| {
					let input = request.json::<serde_json::Value>().unwrap();
					let fail = fail.clone();
					async move {
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
	let Recall::Ready { units } = recall(&store, &bank, question).await else {
		panic!("the Friday fact must be found by meaning");
	};
	assert_eq!(units[0].id, friday, "the closest fact ranks first");

	// An embedding outage leaves keyword search instead of failing the recall.
	fixture.state.fail_queries.store(true, Ordering::SeqCst);
	assert_eq!(
		recall(&store, &bank, "金曜はデプロイ禁止?").await,
		Recall::Empty
	);
	fixture.state.fail_queries.store(false, Ordering::SeqCst);

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
}
