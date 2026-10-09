//! Delivery validates admitted provenance independently of selected-root journaling.
use super::*;
use aidash_server::database::native;

async fn bank_id(store: &Store, bank: &Bank) -> Uuid {
	native::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("memory_banks"))
			.and_where(Expr::col("participant_id").eq(Expr::value(bank.participant.unwrap())))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&store.pool)
	.await
	.unwrap()
}
pub(super) async fn candidate(store: &Store, bank: &Bank) -> Uuid {
	let id = Uuid::now_v7();
	let now = chrono::Utc::now();
	let mut tx = native::begin(&store.pool).await.unwrap();
	native::query(
		&Query::insert()
			.into_table(Alias::new("memory_candidates"))
			.columns(
				[
					"id",
					"bank_id",
					"revision",
					"run",
					"text",
					"kind",
					"learning",
					"verification",
					"entities",
					"evidence",
					"links",
					"state",
					"created_at",
					"updated_at",
				]
				.map(Alias::new),
			)
			.from_subquery(
				Query::select()
					.expr(Expr::value(id))
					.expr(Expr::value(bank_id(store, bank).await))
					.expr(Expr::value(1_i64))
					.expr(Expr::value(json!(Evidence::Run {
						id: Uuid::now_v7(),
						revision: 1,
						digest: "fixture".into()
					})))
					.expr(Expr::value("Candidate to review"))
					.expr(Expr::value("world"))
					.expr(Expr::value("fact"))
					.expr(Expr::value("unverified"))
					.expr(Expr::value(json!([])))
					.expr(Expr::value(json!([])))
					.expr(Expr::value(json!([])))
					.expr(Expr::value("pending"))
					.expr(Expr::value(now))
					.expr(Expr::value(now))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	tx.commit().await.unwrap();
	id
}

#[rstest]
#[case("memory_candidates")]
#[case("memory_candidate_reviews")]
#[case("memory_engine_jobs")]
#[tokio::test]
async fn replacement_preserves_each_durable_operation_table_capacity(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
	#[case] table: &str,
) {
	let database = database.await;
	let (store, registry, workspace) = setup(&database, bounds).await;
	let bank = memory::create_participant(
		&store,
		&Actor::Operator,
		workspace,
		memory::CreateParticipant {
			agent: reference("a"),
		},
	)
	.await
	.unwrap()
	.bank;
	let bank_id = bank_id(&store, &bank).await;
	for _ in 0..3 {
		if table == "memory_candidates" {
			candidate(&store, &bank).await;
			continue;
		}
		let now = chrono::Utc::now();
		let mut query = Query::insert();
		query.into_table(Alias::new(table));
		let mut values = Query::select();
		if table == "memory_candidate_reviews" {
			query.columns(
				[
					"operation_id",
					"candidate_id",
					"bank_id",
					"observed_revision",
					"digest",
					"actor",
					"outcome",
					"created_at",
				]
				.map(Alias::new),
			);
			values
				.expr(Expr::value(Uuid::now_v7()))
				.expr(Expr::value(Uuid::now_v7()))
				.expr(Expr::value(bank_id))
				.expr(Expr::value(1_i64))
				.expr(Expr::value("fixture"))
				.expr(Expr::value("operator"))
				.expr(Expr::value(json!(null)))
				.expr(Expr::value(now));
		} else {
			query.columns(
				[
					"id",
					"bank_id",
					"provider_id",
					"provider_version",
					"kind",
					"input",
					"authority",
					"state",
					"attempts",
					"next_attempt",
					"created_at",
					"updated_at",
				]
				.map(Alias::new),
			);
			values
				.expr(Expr::value(Uuid::now_v7()))
				.expr(Expr::value(bank_id))
				.expr(Expr::value("p"))
				.expr(Expr::value("1.0.0"))
				.expr(Expr::value("learn"))
				.expr(Expr::value(
					json!({"kind":"learn","run":Uuid::now_v7(),"revision":1}),
				))
				.expr(Expr::value(json!({"actor":"operator"})))
				.expr(Expr::value("complete"))
				.expr(Expr::value(1_i32))
				.expr(Expr::value(now))
				.expr(Expr::value(now))
				.expr(Expr::value(now));
		}
		query.from_subquery(values.to_owned());
		let mut tx = native::begin(&store.pool).await.unwrap();
		native::query(&query.to_string(PostgresQueryBuilder))
			.execute(&mut *tx)
			.await
			.unwrap();
		tx.commit().await.unwrap();
	}
	let mut replacement = registry.get("p", "1.0.0").await.unwrap();
	replacement.id = "smaller-capacity".into();
	replacement.config["policy"]["retention"]["max_model_operations"] = json!(2);
	registry.register(replacement).await.unwrap();
	let mut agent = registry.get("a", "1.0.0").await.unwrap();
	agent.version = "1.1.0".into();
	replace_memory_binding(&mut agent, reference("smaller-capacity"));
	registry.register(agent).await.unwrap();
	let result = memory::upgrade_participant(
		&store,
		&Actor::Operator,
		bank.clone(),
		memory::UpgradeParticipant {
			agent: EntityRef {
				id: "a".into(),
				version: "1.1.0".into(),
			},
			expected_revision: 1,
		},
	)
	.await;
	assert!(
		matches!(result, Err(aidash_server::Error::Conflict(ref message)) if message == "replacement memory policy is below existing bank storage"),
		"{result:?}"
	);
	let memory::Outcome::Settings(Some(settings)) = memory::operate(
		&store,
		&Actor::Operator,
		memory::Operation {
			operation_id: Uuid::now_v7(),
			provider: reference("p"),
			bank,
			action: memory::Action::Settings,
		},
	)
	.await
	.unwrap() else {
		panic!("retained bank settings")
	};
	assert_eq!((settings.provider, settings.revision), (reference("p"), 1));
}

#[rstest]
#[tokio::test]
async fn recall_validates_each_admitted_provenance_without_root_or_duplicate_charges(
	#[future] database: DatabaseFixture,
	mut bounds: Bounds,

	#[future(awt)]
	#[from(recall_validates_each_admitted_provenance_without_root_or_duplicate_charges_provider)]
	fixture: RecallValidatesEachAdmittedProvenanceWithoutRootOrDuplicateChargesProvider,
) {
	let _provider = fixture.server;

	bounds.max_graph_visits = 3;

	let endpoint = format!("{}/v1", _provider.url);
	let database = database.await;
	let (store, _, workspace) = setup_endpoint(&database, bounds, &endpoint).await;
	let bank = memory::create_participant(
		&store,
		&Actor::Operator,
		workspace,
		memory::CreateParticipant {
			agent: reference("a"),
		},
	)
	.await
	.unwrap()
	.bank;
	let mut admitted = Vec::<Unit>::new();
	for _ in 0..3 {
		let mut body = content("Current transit facts");
		body.evidence = admitted.last().map(Unit::evidence).into_iter().collect();
		admitted.extend(
			memory::mutate(
				&store,
				&Actor::Operator,
				mutation(
					&bank,
					Change::Add {
						id: Uuid::now_v7(),
						content: body,
					},
				),
			)
			.await
			.unwrap(),
		);
	}
	aidash_server::semantic::worker::sweep(&store)
		.await
		.unwrap();
	let recall = || memory::Operation {
		operation_id: Uuid::now_v7(),
		provider: reference("p"),
		bank: bank.clone(),
		action: memory::Action::Recall {
			query: RecallQuery {
				text: "transit facts".into(),
				time: None,
				kinds: vec![],
				max_tokens: 8192,
			},
		},
	};
	let memory::Outcome::Recall(Recall::Ready { units }) =
		memory::operate(&store, &Actor::Operator, recall())
			.await
			.unwrap()
	else {
		panic!("bounded admitted provenance must be recallable")
	};
	assert_eq!(units.len(), 3);
	assert_eq!(
		units
			.iter()
			.map(|u| u.id)
			.collect::<std::collections::BTreeSet<_>>(),
		admitted.iter().map(|u| u.id).collect()
	);
	memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Correct {
				id: admitted[0].id,
				expected_revision: 1,
				content: content("Corrected transit facts"),
			},
		),
	)
	.await
	.unwrap();
	aidash_server::semantic::worker::sweep(&store)
		.await
		.unwrap();
	let memory::Outcome::Recall(Recall::Ready { units }) =
		memory::operate(&store, &Actor::Operator, recall())
			.await
			.unwrap()
	else {
		panic!("current root remains recallable")
	};
	assert_eq!(
		units.len(),
		1,
		"obsolete dependent provenance stays withheld"
	);
	assert_eq!((units[0].id, units[0].revision), (admitted[0].id, 2));
}

#[fixture]
fn recall_validates_each_admitted_provenance_without_root_or_duplicate_charges_router()
-> std::sync::Arc<Router> {
	std::sync::Arc::new(reinhardt::test::stub::StubRouter::new()
.route("/v1/embeddings", http::Method::POST, reply(|request: reinhardt::Request| {let input = request.json::<serde_json::Value>().unwrap();async move {
		reinhardt::Response::ok().with_json(&json!({"model":input["model"],"data":[{"index":0,"embedding":[1.,0.1,0.]}],"usage":{"prompt_tokens":1}})).unwrap()
	}})).into_server_router())
}
struct RecallValidatesEachAdmittedProvenanceWithoutRootOrDuplicateChargesProvider {
	server: reinhardt::test::fixtures::server::TestServerGuard,
}
#[fixture]
async fn recall_validates_each_admitted_provenance_without_root_or_duplicate_charges_provider(
	#[from(recall_validates_each_admitted_provenance_without_root_or_duplicate_charges_router)]
	_router: std::sync::Arc<Router>,
	#[future(awt)]
	#[from(upstream::upstream)]
	#[with(_router.clone())]
	server: reinhardt::test::fixtures::server::TestServerGuard,
) -> RecallValidatesEachAdmittedProvenanceWithoutRootOrDuplicateChargesProvider {
	RecallValidatesEachAdmittedProvenanceWithoutRootOrDuplicateChargesProvider { server }
}
