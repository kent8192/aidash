//! Bank admission and retirement regressions use canonical PostgreSQL boundaries.
use super::*;
use aidash_server::database::native;

#[rstest]
#[case(false)]
#[case(true)]
#[tokio::test]
async fn missing_index_rejects_bank_pinning_without_admitting_content(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
	#[case] participant: bool,
) {
	let database = database.await;
	let (store, _, workspace) = setup(&database, bounds).await;
	let mut tx = native::begin(&store.pool).await.unwrap();
	for table in ["semantic_collections", "semantic_indexes"] {
		native::query(
			&Query::delete()
				.from_table(Alias::new(table))
				.and_where(Expr::col("workspace_id").eq(Expr::value(workspace)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await
		.unwrap();
	}
	tx.commit().await.unwrap();
	let result = if participant {
		memory::create_participant(
			&store,
			&Actor::Operator,
			workspace,
			memory::CreateParticipant {
				agent: reference("a"),
			},
		)
		.await
		.map(|_| ())
	} else {
		memory::operate(
			&store,
			&Actor::Operator,
			memory::Operation {
				operation_id: Uuid::now_v7(),
				provider: reference("p"),
				bank: Bank {
					home: store.node_id.clone(),
					tenant: "acme".into(),
					workspace,
					participant: None,
				},
				action: memory::Action::ConfigureBank {
					expected_revision: 0,
				},
			},
		)
		.await
		.map(|_| ())
	};
	assert!(
		matches!(result, Err(aidash_server::Error::Conflict(message)) if message == "memory banks require an enabled Workspace index")
	);
	for table in [
		"memory_bank_settings",
		"memory_participants",
		"memory_units",
	] {
		let count: i64 = native::query_scalar(
			&Query::select()
				.expr(reinhardt::query::Func::count(
					Expr::col(reinhardt::query::ColumnRef::Asterisk).into(),
				))
				.from(Alias::new(table))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_one(&store.pool)
		.await
		.unwrap();
		assert_eq!(count, 0, "rejected admission must roll back {table}");
	}
}

#[rstest]
#[case(Kind::Observation, None)]
#[case(Kind::MentalModel, None)]
#[case(Kind::MentalModel, Some(false))]
#[case(Kind::MentalModel, Some(true))]
#[tokio::test]
async fn stale_manual_derivations_retire_without_age_expiry(
	#[future] database: DatabaseFixture,
	mut bounds: Bounds,
	#[case] kind: Kind,
	#[case] exhausted_repair: Option<bool>,

	#[future(awt)]
	#[from(stale_manual_derivations_retire_without_age_expiry_provider)]
	fixture: StaleManualDerivationsRetireWithoutAgeExpiryProvider,
) {
	let server = fixture.server;

	bounds.max_units = 2;
	bounds.max_candidates = 2;
	bounds.max_results = 2;

	let endpoint = format!("{}/v1", server.url);
	let database = database.await;
	let attempts = bounds.max_retries as i32;
	let (store, _, workspace) = setup_endpoint_flags(
		&database,
		bounds,
		&endpoint,
		(false, false, exhausted_repair.is_some()),
	)
	.await;
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
	let source = memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Add {
				id: Uuid::now_v7(),
				content: content("Admitted support"),
			},
		),
	)
	.await
	.unwrap()
	.remove(0);
	let mut draft = content("Derived draft");
	draft.kind = kind;
	draft.evidence = vec![source.evidence()];
	if kind == Kind::MentalModel {
		draft.mental_model = Some(MentalModel {
			question: "Recurring question?".into(),
			automatic_refresh: exhausted_repair.is_some(),
		});
	}
	let id = Uuid::now_v7();
	let input = mutation(&bank, Change::Add { id, content: draft });
	let memory::Outcome::Units(derived) = memory::operate(
		&store,
		&Actor::Operator,
		memory::Operation {
			operation_id: input.operation_id,
			provider: reference("p"),
			bank: bank.clone(),
			action: memory::Action::Derive {
				kind,
				sources: vec![source.evidence()],
				mutation: input,
			},
		},
	)
	.await
	.unwrap() else {
		panic!("derived unit");
	};

	memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Correct {
				id: source.id,
				expected_revision: source.revision,
				content: content("Corrected admitted support"),
			},
		),
	)
	.await
	.unwrap();
	if let Some(exhausted) = exhausted_repair {
		let mut tx = native::begin(&store.pool).await.unwrap();
		native::query(
			&Query::update()
				.table(Alias::new("memory_engine_jobs"))
				.value(Alias::new("state"), "running")
				.value(
					Alias::new("attempts"),
					if exhausted { attempts } else { attempts - 1 },
				)
				.value(Alias::new("claim"), Uuid::now_v7())
				.value(
					Alias::new("next_attempt"),
					chrono::Utc::now() - chrono::Duration::seconds(1),
				)
				.and_where(Expr::col("kind").eq("mental_model"))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await
		.unwrap();
		tx.commit().await.unwrap();
	}
	aidash_server::semantic::worker::sweep(&store)
		.await
		.unwrap();
	let row = native::query(
		&Query::select()
			.columns(["deleted", "stale"].map(Alias::new))
			.from(Alias::new("memory_units"))
			.and_where(Expr::col("id").eq(Expr::value(derived[0].id)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&store.pool)
	.await
	.unwrap();
	assert!(
		row.try_get::<bool>("deleted").unwrap() == (exhausted_repair != Some(false)),
		"only expired claims with retry allowance preserve stale derived rows"
	);
	if exhausted_repair != Some(false) {
		memory::mutate(
			&store,
			&Actor::Operator,
			mutation(
				&bank,
				Change::Add {
					id: Uuid::now_v7(),
					content: content("Replacement knowledge fits after stale retirement"),
				},
			),
		)
		.await
		.unwrap();
	}
	drop(server);
}

#[rstest]
#[tokio::test]
async fn purge_pages_reader_and_publication_journals_beyond_graph_capacity(
	#[future] database: DatabaseFixture,
	mut bounds: Bounds,
) {
	use aidash_domain::NewTask;
	let page = bounds.max_graph_visits;
	bounds.max_units = 2 * page + 4;
	let database = database.await;
	let (store, _, workspace) = setup(&database, bounds).await;
	let index = aidash_server::semantic::service::get_index(&store, &Actor::Operator, workspace)
		.await
		.unwrap();
	let mut spec = index.configuration().unwrap();
	// Match this regression's admitted bank size within the public index bound.
	spec.max_sources = 2 * page + 4;
	aidash_server::semantic::service::configure(
		&store,
		workspace,
		aidash_server::semantic::ConfigureIndex {
			expected_revision: index.revision,
			spec,
		},
	)
	.await
	.unwrap();
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
	let mut units = Vec::new();
	for text in [
		"Withdrawn body",
		"Independent corrected publication",
		"Independent corrected Run",
	] {
		units.extend(
			memory::mutate(
				&store,
				&Actor::Operator,
				mutation(
					&bank,
					Change::Add {
						id: Uuid::now_v7(),
						content: content(text),
					},
				),
			)
			.await
			.unwrap(),
		);
	}
	let bank_id: Uuid = native::query_scalar(
		&Query::select()
			.column(Alias::new("bank_id"))
			.from(Alias::new("memory_units"))
			.and_where(Expr::col("id").eq(Expr::value(units[0].id)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&store.pool)
	.await
	.unwrap();
	let mut last_publication = Uuid::nil();
	let mut last_run = Uuid::nil();
	for ordinal in 0..(2 * page + 1) {
		let task = store
			.create_task(
				workspace,
				&NewTask {
					title: format!("Memory reader {ordinal}"),
					description: "Retained journal fixture".into(),
					requirements: json!({}),
					dependencies: vec![],
					parent_id: None,
				},
				"operator",
				None,
			)
			.await
			.unwrap();
		last_publication = Uuid::now_v7();
		last_run = Uuid::now_v7();
		let mut tx = native::begin(&store.pool).await.unwrap();
		for query in [
			Query::insert()
				.into_table(Alias::new("runs"))
				.columns(
					[
						"id",
						"task_id",
						"workspace_id",
						"home_node",
						"agent_id",
						"agent_version",
					]
					.map(Alias::new),
				)
				.from_subquery(
					Query::select()
						.expr(Expr::value(last_run))
						.expr(Expr::value(task.id))
						.expr(Expr::value(workspace))
						.expr(Expr::value(&store.node_id))
						.expr(Expr::value("a"))
						.expr(Expr::value("1.0.0"))
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
			Query::insert()
				.into_table(Alias::new("memory_run_reads"))
				.columns(["run_id", "unit_id", "revision"].map(Alias::new))
				.from_subquery(
					Query::select()
						.expr(Expr::value(last_run))
						.expr(Expr::value(units[0].id))
						.expr(Expr::value(1_i64))
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
			Query::insert()
				.into_table(Alias::new("memory_publications"))
				.columns(
					[
						"id",
						"bank_id",
						"source_id",
						"source_revision",
						"revision",
						"authority",
						"deleted",
						"created_at",
					]
					.map(Alias::new),
				)
				.from_subquery(
					Query::select()
						.expr(Expr::value(last_publication))
						.expr(Expr::value(bank_id))
						.expr(Expr::value(units[0].id))
						.expr(Expr::value(1_i64))
						.expr(Expr::value(1_i64))
						.expr(Expr::value(json!({"kind":"operator"})))
						.expr(Expr::value(false))
						.expr(Expr::value(chrono::Utc::now()))
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		] {
			native::query(&query).execute(&mut *tx).await.unwrap();
		}
		tx.commit().await.unwrap();
		let quoted = memory::mutate(
			&store,
			&Actor::Operator,
			mutation(
				&bank,
				Change::Add {
					id: Uuid::now_v7(),
					content: content(&format!("Independent current body {ordinal}")),
				},
			),
		)
		.await
		.unwrap()
		.remove(0);
		let mut tx = native::begin(&store.pool).await.unwrap();
		native::query(
			&Query::update()
				.table(Alias::new("memory_history"))
				.value(
					Alias::new("evidence"),
					json!([Evidence::Run {
						id: last_run,
						revision: 1,
						digest: "historic".into()
					}]),
				)
				.and_where(Expr::col("unit_id").eq(Expr::value(quoted.id)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await
		.unwrap();
		tx.commit().await.unwrap();
	}
	// Independently corrected current bodies no longer quote the source. Their
	// historical evidence lies past two journal pages and still must be erased.
	let mut tx = native::begin(&store.pool).await.unwrap();
	for (unit, proof) in [
		(
			&units[1],
			Evidence::Publication {
				id: last_publication,
				revision: 1,
			},
		),
		(
			&units[2],
			Evidence::Run {
				id: last_run,
				revision: 1,
				digest: "historic".into(),
			},
		),
	] {
		native::query(
			&Query::update()
				.table(Alias::new("memory_history"))
				.value(Alias::new("evidence"), json!([proof]))
				.and_where(Expr::col("unit_id").eq(Expr::value(unit.id)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await
		.unwrap();
	}
	tx.commit().await.unwrap();
	memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Delete {
				id: units[0].id,
				expected_revision: 1,
			},
		),
	)
	.await
	.unwrap();
	let mut tx = native::begin(&store.pool).await.unwrap();
	native::query(
		&Query::update()
			.table(Alias::new("memory_purge_jobs"))
			.value(
				Alias::new("purge_after"),
				chrono::Utc::now() - chrono::Duration::seconds(1),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	tx.commit().await.unwrap();
	aidash_server::semantic::worker::sweep(&store)
		.await
		.unwrap();
	let state: String = native::query_scalar(
		&Query::select()
			.column(Alias::new("state"))
			.from(Alias::new("memory_purge_jobs"))
			.and_where(Expr::col("unit_id").eq(Expr::value(units[0].id)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&store.pool)
	.await
	.unwrap();
	assert_eq!(
		state, "purged",
		"unbounded usage fanout must not exhaust purge retries"
	);
	let histories: i64 = native::query_scalar(
		&Query::select()
			.expr(reinhardt::query::Func::count(
				Expr::col(reinhardt::query::ColumnRef::Asterisk).into(),
			))
			.from(Alias::new("memory_history"))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&store.pool)
	.await
	.unwrap();
	assert_eq!(
		histories, 0,
		"quotations on later journal pages must be erased"
	);
	let remaining = memory::list(
		&store,
		&Actor::Operator,
		memory::ReadBank {
			bank,
			provider: reference("p"),
		},
	)
	.await
	.unwrap();
	assert_eq!(remaining.len(), 2 * page + 3);
	assert!(remaining.iter().all(|unit| !unit.content.text.is_empty()));
}

#[rstest]
#[tokio::test]
async fn registry_rejects_a_memory_policy_above_the_semantic_compute_allowance(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	let database = database.await;
	let (_, registry, _) = setup(&database, bounds).await;
	let mut embedding = registry.get("e", "1.0.0").await.unwrap();
	embedding.id = "large-embedding".into();
	embedding.config["dimensions"] = json!(3072);
	registry.register(embedding).await.unwrap();
	let mut provider = registry.get("p", "1.0.0").await.unwrap();
	provider.id = "oversized-memory".into();
	provider.config["policy"]["embedding"] = json!(reference("large-embedding"));
	provider.config["policy"]["bounds"]["max_units"] = json!(2048);
	provider.config["policy"]["retention"]["max_unit_records"] = json!(2048);
	assert!(matches!(registry.register(provider).await,
        Err(aidash_server::Error::Invalid(message)) if message.contains("compute allowance")));
	assert!(registry.get("oversized-memory", "1.0.0").await.is_err());
}

#[fixture]
fn stale_manual_derivations_retire_without_age_expiry_router() -> std::sync::Arc<Router> {
	std::sync::Arc::new(reinhardt::test::stub::StubRouter::new()
.route("/v1/chat/completions", http::Method::POST, reply(|request: reinhardt::Request| {let input = request.json::<serde_json::Value>().unwrap();async move {
        let context: serde_json::Value = serde_json::from_str(input["messages"][1]["content"].as_str().unwrap()).unwrap();
        let units: Vec<Unit> = serde_json::from_value(context["units"].clone()).unwrap();
        let mut output = content("Derived from admitted evidence");
        output.kind = serde_json::from_value(context["kind"].clone()).unwrap();
        output.mental_model = serde_json::from_value(context["mental_model"].clone()).unwrap();
        output.evidence = units.iter().map(Unit::evidence).collect();
        reinhardt::Response::ok().with_json(&json!({"choices":[{"finish_reason":"stop","message":{"content":serde_json::to_string(&output).unwrap()}}],"usage":{"prompt_tokens":1,"completion_tokens":1}})).unwrap()
    }})).into_server_router())
}
struct StaleManualDerivationsRetireWithoutAgeExpiryProvider {
	server: reinhardt::test::fixtures::server::TestServerGuard,
}
#[fixture]
async fn stale_manual_derivations_retire_without_age_expiry_provider(
	#[from(stale_manual_derivations_retire_without_age_expiry_router)] _router: std::sync::Arc<
		Router,
	>,
	#[future(awt)]
	#[from(upstream::upstream)]
	#[with(_router.clone())]
	server: reinhardt::test::fixtures::server::TestServerGuard,
) -> StaleManualDerivationsRetireWithoutAgeExpiryProvider {
	StaleManualDerivationsRetireWithoutAgeExpiryProvider { server }
}
