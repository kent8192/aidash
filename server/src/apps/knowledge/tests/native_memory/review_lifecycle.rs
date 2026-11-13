//! Bank admission and retirement regressions use canonical PostgreSQL boundaries.
use super::*;
use aidash_application::tenancy::TEST_FIXTURE;
use aidash_server::database::native;

async fn journal_proof(store: &Store, workspace: Uuid) -> Evidence {
	let participant = memory::create_participant(
		store,
		&administrator(store).await,
		workspace,
		memory::CreateParticipant {
			agent: reference("a"),
		},
	)
	.await
	.unwrap();
	let (visibility, run) = super::policy_bounds::recorded_run(store, &participant.bank, &[]).await;
	assert!(super::policy_bounds::run_visible(&visibility, store, run).await);
	let event = json!({"source":"journal fixture"});
	let digest = aidash_domain::registry::rules::digest(&event);
	let mut tx = store.pool.begin(&TEST_FIXTURE).await.unwrap();
	native::query(
		&Query::insert()
			.into_table(Alias::new("run_context_events"))
			.columns(["run_id", "seq", "origin", "event", "digest", "tenant"].map(Alias::new))
			.from_subquery(
				Query::select()
					.expr(Expr::value(run))
					.expr(Expr::value(1_i64))
					.expr(Expr::value("appended"))
					.expr(Expr::value(event))
					.expr(Expr::value(digest))
					.expr(Expr::value("acme"))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	tx.commit().await.unwrap();
	Evidence::JournalRange {
		run_id: run,
		from_seq: 1,
		through_seq: 1,
	}
}

#[rstest]
#[tokio::test]
async fn builtin_only_banks_do_not_prevent_initializing_a_recovery_ledger(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	let database = database.await;
	let (store, _, workspace) = setup(&database, bounds).await;
	let bank = Bank {
		home: aidash_domain::nodes::NodeAddress::derived(store.deployment.as_str(), "acme")
			.to_string(),
		tenant: "acme".into(),
		workspace,
		participant: None,
	};
	memory::operate(
		&store,
		&administrator(&store).await,
		memory::Operation {
			operation_id: Uuid::now_v7(),
			provider: reference("aidash.memory"),
			bank,
			action: memory::Action::ConfigureBank {
				expected_revision: 0,
			},
		},
	)
	.await
	.unwrap();
	let without_ledger = Store::from_pool(
		database.connection.clone().into_postgres().unwrap(),
		store.deployment.clone(),
	)
	.await
	.unwrap();
	let directory = tempfile::tempdir().unwrap();
	aidash_server::semantic::services::memory_recovery::initialize(
		&without_ledger,
		directory.path().to_owned(),
	)
	.await
	.unwrap();
}

#[rstest]
#[tokio::test]
async fn journal_evidence_requires_current_run_read_authority(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	let database = database.await;
	let (store, _, workspace) = setup(&database, bounds).await;
	let participant = memory::create_participant(
		&store,
		&administrator(&store).await,
		workspace,
		memory::CreateParticipant {
			agent: reference("a"),
		},
	)
	.await
	.unwrap();
	let bank = participant.bank;
	let (_, run) = super::policy_bounds::recorded_run(&store, &bank, &[]).await;
	let event = json!({"source":"journal fixture"});
	let digest = aidash_domain::registry::rules::digest(&event);
	let mut tx = store.pool.begin(&TEST_FIXTURE).await.unwrap();
	native::query(
		&Query::insert()
			.into_table(Alias::new("run_context_events"))
			.columns(["run_id", "seq", "origin", "event", "digest", "tenant"].map(Alias::new))
			.from_subquery(
				Query::select()
					.expr(Expr::value(run))
					.expr(Expr::value(1_i64))
					.expr(Expr::value("appended"))
					.expr(Expr::value(event))
					.expr(Expr::value(digest))
					.expr(Expr::value("acme"))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	tx.commit().await.unwrap();
	let actor = administrator(&store).await;
	let mut fact = content("Journal-supported fact");
	fact.evidence = vec![Evidence::JournalRange {
		run_id: run,
		from_seq: 1,
		through_seq: 1,
	}];
	let id = Uuid::now_v7();
	memory::mutate(
		&store,
		&actor,
		mutation(&bank, Change::Add { id, content: fact }),
	)
	.await
	.unwrap();
	let read = || memory::ReadBank {
		provider: reference("p"),
		bank: bank.clone(),
	};
	assert!(
		memory::list(&store, &actor, read())
			.await
			.unwrap()
			.iter()
			.any(|unit| unit.id == id)
	);
	aidash_server::authorization::Authorization { pool: store.pool.clone() }.replace("acme", 2,
		serde_json::from_value(json!({"tenant":"acme","subjects":{"alice":{"kind":"user"}},"policies":[
			{"id":"read","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}},
			{"id":"hide-runs","effect":"deny","subjects":{"any":true},"actions":["run.read"],"resources":{"kinds":["*"]}}
		]})).unwrap(), "deployment_administrator").await.unwrap();
	assert!(
		memory::list(&store, &actor, read())
			.await
			.unwrap()
			.is_empty()
	);
}

#[rstest]
#[tokio::test]
async fn builtin_evicts_oldest_units_when_the_live_cap_is_reached(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	let database = database.await;
	let (store, _, workspace) = setup(&database, bounds).await;
	let proof = journal_proof(&store, workspace).await;
	let bank = Bank {
		home: aidash_domain::nodes::NodeAddress::derived(store.deployment.as_str(), "acme")
			.to_string(),
		tenant: "acme".into(),
		workspace,
		participant: None,
	};
	let provider = reference("aidash.memory");
	let actor = administrator(&store).await;
	memory::operate(
		&store,
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
	let ids: Vec<_> = (0..65).map(|_| Uuid::now_v7()).collect();
	for chunk in ids[..64].chunks(32) {
		memory::mutate(
			&store,
			&actor,
			Mutation {
				operation_id: Uuid::now_v7(),
				provider: provider.clone(),
				bank: bank.clone(),
				changes: chunk
					.iter()
					.map(|id| {
						let mut fact = content(&format!("Fact {id}"));
						fact.evidence = vec![proof.clone()];
						Change::Add {
							id: *id,
							content: fact,
						}
					})
					.collect(),
			},
		)
		.await
		.unwrap();
	}
	let oldest: Uuid = native::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("memory_units"))
			.and_where(Expr::col("deleted").eq(false))
			.order_by(Alias::new("updated_at"), reinhardt::query::Order::Asc)
			.order_by(Alias::new("id"), reinhardt::query::Order::Asc)
			.limit(1)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(store.pool.in_scope(&TEST_FIXTURE))
	.await
	.unwrap();
	memory::mutate(
		&store,
		&actor,
		Mutation {
			operation_id: Uuid::now_v7(),
			provider: provider.clone(),
			bank: bank.clone(),
			changes: vec![Change::Add {
				id: ids[64],
				content: {
					let mut fact = content("newest fact");
					fact.evidence = vec![proof.clone()];
					fact
				},
			}],
		},
	)
	.await
	.unwrap();
	let units = memory::list(
		&store,
		&actor,
		memory::ReadBank {
			provider: provider.clone(),
			bank: bank.clone(),
		},
	)
	.await
	.unwrap();
	assert_eq!(units.len(), 64);
	assert!(units.iter().any(|unit| unit.id == ids[64]));
	assert!(!units.iter().any(|unit| unit.id == oldest));
	let recall = memory::operate(
		&store,
		&actor,
		memory::Operation {
			operation_id: Uuid::now_v7(),
			provider,
			bank,
			action: memory::Action::Recall {
				query: RecallQuery {
					text: "newest".into(),
					time: None,
					kinds: vec![],
					max_tokens: 8192,
				},
			},
		},
	)
	.await
	.unwrap();
	let memory::Outcome::Recall(Recall::Ready { units }) = recall else {
		panic!("expected recall at capacity")
	};
	assert!(units.iter().any(|unit| unit.id == ids[64]));
}

#[rstest]
#[tokio::test]
async fn builtin_eviction_keeps_the_oldest_unit_when_the_new_unit_rests_on_it(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	// Arrange: a full built-in bank of 64 live Units.
	let database = database.await;
	let (store, _, workspace) = setup(&database, bounds).await;
	let proof = journal_proof(&store, workspace).await;
	let bank = Bank {
		home: aidash_domain::nodes::NodeAddress::derived(store.deployment.as_str(), "acme")
			.to_string(),
		tenant: "acme".into(),
		workspace,
		participant: None,
	};
	let provider = reference("aidash.memory");
	let actor = administrator(&store).await;
	memory::operate(
		&store,
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
	let ids: Vec<_> = (0..65).map(|_| Uuid::now_v7()).collect();
	for chunk in ids[..64].chunks(32) {
		memory::mutate(
			&store,
			&actor,
			Mutation {
				operation_id: Uuid::now_v7(),
				provider: provider.clone(),
				bank: bank.clone(),
				changes: chunk
					.iter()
					.map(|id| {
						let mut fact = content(&format!("Fact {id}"));
						fact.evidence = vec![proof.clone()];
						Change::Add {
							id: *id,
							content: fact,
						}
					})
					.collect(),
			},
		)
		.await
		.unwrap();
	}
	let eviction_order: Vec<Uuid> = native::query(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("memory_units"))
			.and_where(Expr::col("deleted").eq(false))
			.order_by(Alias::new("updated_at"), reinhardt::query::Order::Asc)
			.order_by(Alias::new("id"), reinhardt::query::Order::Asc)
			.limit(2)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(store.pool.in_scope(&TEST_FIXTURE))
	.await
	.unwrap()
	.iter()
	.map(|row| row.try_get("id").unwrap())
	.collect();
	let (oldest, next_oldest) = (eviction_order[0], eviction_order[1]);

	// Act: add a Unit supported by the oldest Unit, the first eviction candidate.
	let added = memory::mutate(
		&store,
		&actor,
		Mutation {
			operation_id: Uuid::now_v7(),
			provider: provider.clone(),
			bank: bank.clone(),
			changes: vec![Change::Add {
				id: ids[64],
				content: {
					let mut fact = content("fact resting on the oldest unit");
					fact.evidence = vec![
						proof.clone(),
						Evidence::Unit {
							bank: bank.clone(),
							id: oldest,
							revision: 1,
						},
					];
					fact
				},
			}],
		},
	)
	.await;

	// Assert: the addition is admitted and an unrelated Unit is evicted instead.
	assert!(added.is_ok(), "{added:?}");
	let units = memory::list(&store, &actor, memory::ReadBank { provider, bank })
		.await
		.unwrap();
	assert_eq!(units.len(), 64);
	assert!(units.iter().any(|unit| unit.id == ids[64] && !unit.stale));
	assert!(units.iter().any(|unit| unit.id == oldest));
	assert!(!units.iter().any(|unit| unit.id == next_oldest));
}

#[rstest]
#[tokio::test]
async fn builtin_keyword_bank_serves_without_a_recovery_ledger(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	let database = database.await;
	let (store, _, workspace) = setup(&database, bounds).await;
	let proof = journal_proof(&store, workspace).await;
	let mut tx = store.pool.begin(&TEST_FIXTURE).await.unwrap();
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
	let bank = Bank {
		home: aidash_domain::nodes::NodeAddress::derived(store.deployment.as_str(), "acme")
			.to_string(),
		tenant: "acme".into(),
		workspace,
		participant: None,
	};
	let provider = reference("aidash.memory");
	let actor = administrator(&store).await;
	// A fresh Store has no attached recovery ledger, as on the default deployment.
	let without_ledger = Store::from_pool(
		database.connection.clone().into_postgres().unwrap(),
		store.deployment.clone(),
	)
	.await
	.unwrap();
	memory::operate(
		&without_ledger,
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
		&without_ledger,
		&actor,
		Mutation {
			operation_id: Uuid::now_v7(),
			provider: provider.clone(),
			bank: bank.clone(),
			changes: vec![Change::Add {
				id,
				content: {
					let mut fact = content("Mercury launch details");
					fact.evidence = vec![proof.clone()];
					fact
				},
			}],
		},
	)
	.await
	.unwrap();
	let Evidence::JournalRange { run_id, .. } = proof else {
		unreachable!()
	};
	let mut tx = without_ledger.pool.begin(&TEST_FIXTURE).await.unwrap();
	native::query(
		&Query::delete()
			.from_table(Alias::new("run_context_events"))
			.and_where(Expr::col("run_id").eq(Expr::value(run_id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	tx.commit().await.unwrap();
	let outcome = memory::operate(
		&without_ledger,
		&actor,
		memory::Operation {
			operation_id: Uuid::now_v7(),
			provider,
			bank,
			action: memory::Action::Recall {
				query: RecallQuery {
					text: "Mercury".into(),
					time: None,
					kinds: vec![],
					max_tokens: 8192,
				},
			},
		},
	)
	.await
	.unwrap();
	let memory::Outcome::Recall(Recall::Ready { units }) = outcome else {
		panic!("expected keyword recall")
	};
	assert_eq!(
		units.iter().map(|unit| unit.id).collect::<Vec<_>>(),
		vec![id]
	);
}

#[rstest]
#[tokio::test]
async fn keyword_only_bank_can_write_and_recall_without_a_workspace_index(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	let database = database.await;
	let (store, registry, workspace) = setup(&database, bounds).await;
	let tenant = aidash_domain::tenancy::TenantName::new("acme").unwrap();
	let mut provider = registry.get(&tenant, "p", "1.0.0").await.unwrap();
	provider.id = "keyword-only".into();
	provider.config["policy"]["embedding"] = serde_json::Value::Null;
	registry.register(&tenant, provider).await.unwrap();
	let actor = administrator(&store).await;
	let mut tx = store.pool.begin(&TEST_FIXTURE).await.unwrap();
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
	let bank = Bank {
		home: aidash_domain::nodes::NodeAddress::derived(store.deployment.as_str(), "acme")
			.to_string(),
		tenant: "acme".into(),
		workspace,
		participant: None,
	};
	let provider = reference("keyword-only");
	memory::operate(
		&store,
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
		&store,
		&actor,
		Mutation {
			operation_id: Uuid::now_v7(),
			provider: provider.clone(),
			bank: bank.clone(),
			changes: vec![Change::Add {
				id,
				content: content("Mercury mission launch details"),
			}],
		},
	)
	.await
	.unwrap();
	let result = memory::operate(
		&store,
		&actor,
		memory::Operation {
			operation_id: Uuid::now_v7(),
			provider,
			bank,
			action: memory::Action::Recall {
				query: RecallQuery {
					text: "Mercury".into(),
					time: None,
					kinds: vec![],
					max_tokens: 8192,
				},
			},
		},
	)
	.await
	.unwrap();
	let memory::Outcome::Recall(Recall::Ready { units }) = result else {
		panic!("keyword recall must return the admitted Unit")
	};
	assert_eq!(
		units.iter().map(|unit| unit.id).collect::<Vec<_>>(),
		vec![id]
	);
}

#[rstest]
#[tokio::test]
async fn keyword_only_upgrade_refuses_a_nonempty_indexed_bank(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	let database = database.await;
	let (store, registry, workspace) = setup(&database, bounds).await;
	let tenant = aidash_domain::tenancy::TenantName::new("acme").unwrap();
	let mut provider = registry.get(&tenant, "p", "1.0.0").await.unwrap();
	provider.id = "keyword-only".into();
	provider.config["policy"]["embedding"] = serde_json::Value::Null;
	registry.register(&tenant, provider).await.unwrap();
	let actor = administrator(&store).await;
	let bank = Bank {
		home: aidash_domain::nodes::NodeAddress::derived(store.deployment.as_str(), "acme")
			.to_string(),
		tenant: "acme".into(),
		workspace,
		participant: None,
	};
	memory::operate(
		&store,
		&actor,
		memory::Operation {
			operation_id: Uuid::now_v7(),
			provider: reference("p"),
			bank: bank.clone(),
			action: memory::Action::ConfigureBank {
				expected_revision: 0,
			},
		},
	)
	.await
	.unwrap();
	memory::mutate(
		&store,
		&actor,
		mutation(
			&bank,
			Change::Add {
				id: Uuid::now_v7(),
				content: content("Indexed before the policy change"),
			},
		),
	)
	.await
	.unwrap();
	let result = memory::operate(
		&store,
		&actor,
		memory::Operation {
			operation_id: Uuid::now_v7(),
			provider: reference("keyword-only"),
			bank,
			action: memory::Action::ConfigureBank {
				expected_revision: 1,
			},
		},
	)
	.await;
	assert!(
		matches!(result, Err(aidash_server::Error::Conflict(message)) if message.contains("clear the bank"))
	);
}

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
	let mut tx = store.pool.begin(&TEST_FIXTURE).await.unwrap();
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
			&administrator(&store).await,
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
			&administrator(&store).await,
			memory::Operation {
				operation_id: Uuid::now_v7(),
				provider: reference("p"),
				bank: Bank {
					home: aidash_domain::nodes::NodeAddress::derived(
						store.deployment.as_str(),
						"acme",
					)
					.to_string(),
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
		.scalar_one(store.pool.in_scope(&TEST_FIXTURE))
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
		&administrator(&store).await,
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
		&administrator(&store).await,
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
		&administrator(&store).await,
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
		&administrator(&store).await,
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
		let mut tx = store.pool.begin(&TEST_FIXTURE).await.unwrap();
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
	.fetch_one(store.pool.in_scope(&TEST_FIXTURE))
	.await
	.unwrap();
	assert!(
		row.try_get::<bool>("deleted").unwrap() == (exhausted_repair != Some(false)),
		"only expired claims with retry allowance preserve stale derived rows"
	);
	if exhausted_repair != Some(false) {
		memory::mutate(
			&store,
			&administrator(&store).await,
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
	let index = aidash_server::semantic::service::get_index(
		&store,
		&administrator(&store).await,
		workspace,
	)
	.await
	.unwrap();
	let mut spec = index.configuration().unwrap();
	// Match this regression's admitted bank size within the public index bound.
	spec.max_sources = 2 * page + 4;
	aidash_server::semantic::service::configure(
		&store,
		&administrator(&store).await,
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
		&administrator(&store).await,
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
				&administrator(&store).await,
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
	.scalar_one(store.pool.in_scope(&TEST_FIXTURE))
	.await
	.unwrap();
	let mut last_publication = Uuid::nil();
	let mut last_run = Uuid::nil();
	for ordinal in 0..(2 * page + 1) {
		let task = store
			.create_task(
				&TEST_FIXTURE,
				workspace,
				&NewTask {
					title: format!("Memory reader {ordinal}"),
					description: "Retained journal fixture".into(),
					requirements: json!({}),
					dependencies: vec![],
					parent_id: None,
				},
				"deployment_administrator",
				None,
			)
			.await
			.unwrap();
		last_publication = Uuid::now_v7();
		last_run = Uuid::now_v7();
		let mut tx = store.pool.begin(&TEST_FIXTURE).await.unwrap();
		for query in [
			Query::insert()
				.into_table(Alias::new("runs"))
				.columns(
					[
						"id",
						"task_id",
						"workspace_id",
						"home_node",
						"node",
						"agent_id",
						"agent_version",
						"tenant",
					]
					.map(Alias::new),
				)
				.from_subquery(
					Query::select()
						.expr(Expr::value(last_run))
						.expr(Expr::value(task.id))
						.expr(Expr::value(workspace))
						.expr(Expr::value(
							aidash_domain::nodes::NodeAddress::derived(
								store.deployment.as_str(),
								"acme",
							)
							.to_string(),
						))
						.expr(Expr::value(
							aidash_domain::nodes::NodeName::derived("acme").to_string(),
						))
						.expr(Expr::value("a"))
						.expr(Expr::value("1.0.0"))
						.expr(Expr::value("acme"))
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
			Query::insert()
				.into_table(Alias::new("memory_run_reads"))
				.columns(["run_id", "unit_id", "revision", "tenant"].map(Alias::new))
				.from_subquery(
					Query::select()
						.expr(Expr::value(last_run))
						.expr(Expr::value(units[0].id))
						.expr(Expr::value(1_i64))
						.expr(Expr::value("acme"))
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
						"tenant",
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
						.expr(Expr::value(json!({"kind":"deployment_administrator"})))
						.expr(Expr::value(false))
						.expr(Expr::value(chrono::Utc::now()))
						.expr(Expr::value("acme"))
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		] {
			native::query(&query).execute(&mut *tx).await.unwrap();
		}
		tx.commit().await.unwrap();
		let quoted = memory::mutate(
			&store,
			&administrator(&store).await,
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
		let mut tx = store.pool.begin(&TEST_FIXTURE).await.unwrap();
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
	let mut tx = store.pool.begin(&TEST_FIXTURE).await.unwrap();
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
		&administrator(&store).await,
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
	let mut tx = store.pool.begin(&TEST_FIXTURE).await.unwrap();
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
	.scalar_one(store.pool.in_scope(&TEST_FIXTURE))
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
	.scalar_one(store.pool.in_scope(&TEST_FIXTURE))
	.await
	.unwrap();
	assert_eq!(
		histories, 0,
		"quotations on later journal pages must be erased"
	);
	let remaining = memory::list(
		&store,
		&administrator(&store).await,
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
	let mut embedding = registry
		.get(
			&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
			"e",
			"1.0.0",
		)
		.await
		.unwrap();
	embedding.id = "large-embedding".into();
	embedding.config["dimensions"] = json!(3072);
	registry
		.register(
			&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
			embedding,
		)
		.await
		.unwrap();
	let mut provider = registry
		.get(
			&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
			"p",
			"1.0.0",
		)
		.await
		.unwrap();
	provider.id = "oversized-memory".into();
	provider.config["policy"]["embedding"] = json!(reference("large-embedding"));
	provider.config["policy"]["bounds"]["max_units"] = json!(2048);
	provider.config["policy"]["retention"]["max_unit_records"] = json!(2048);
	assert!(
		matches!(registry.register(&aidash_domain::tenancy::TenantName::new("acme").unwrap(), provider).await,
        Err(aidash_server::Error::Invalid(message)) if message.contains("compute allowance"))
	);
	assert!(
		registry
			.get(
				&aidash_domain::tenancy::TenantName::new("acme").unwrap(),
				"oversized-memory",
				"1.0.0"
			)
			.await
			.is_err()
	);
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

#[rstest]
#[case("account_terms")]
#[case("revocation")]
#[tokio::test]
async fn research_fact_is_hidden_after_journal_expiry_and_source_loss(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
	#[case] loss: &str,
) {
	let database = database.await;
	let (store, _, workspace) = setup(&database, bounds).await;
	let proof = journal_proof(&store, workspace).await;
	let Evidence::JournalRange { run_id, .. } = proof.clone() else {
		unreachable!()
	};
	let result = json!({});
	let digest = aidash_domain::registry::rules::digest(&result);
	let source = Uuid::now_v7();
	let research_ref = json!({"contract":"aidash.research.reference/1", "invocation":"memory-research", "digest":digest, "operation":"search", "sources":[source], "documents":[]});
	let event = json!({"kind":"tool", "call":{"id":"call", "name":"research", "arguments":{}}, "result":research_ref});
	let event_digest = aidash_domain::registry::rules::digest(&event);
	let mut tx = store.pool.begin(&TEST_FIXTURE).await.unwrap();
	native::query(
		&Query::update()
			.table(Alias::new("run_context_events"))
			.value(Alias::new("event"), event)
			.value(Alias::new("digest"), event_digest)
			.and_where(Expr::col("run_id").eq(Expr::value(run_id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	native::query(
		&Query::insert()
			.into_table(Alias::new("research_journal_results"))
			.columns(
				[
					"id",
					"tenant",
					"run_id",
					"invocation_key",
					"digest",
					"reference",
					"bytes",
					"created_at",
				]
				.map(Alias::new),
			)
			.from_subquery(
				Query::select()
					.expr(Expr::value(Uuid::now_v7()))
					.expr(Expr::value("acme"))
					.expr(Expr::value(run_id))
					.expr(Expr::value("memory-research"))
					.expr(Expr::value(digest.clone()))
					.expr(Expr::value(research_ref))
					.expr(Expr::value(b"{}".to_vec()))
					.expr(Expr::value(chrono::Utc::now()))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	tx.commit().await.unwrap();
	let bank = Bank {
		home: aidash_domain::nodes::NodeAddress::derived(store.deployment.as_str(), "acme")
			.to_string(),
		tenant: "acme".into(),
		workspace,
		participant: None,
	};
	let provider = reference("aidash.memory");
	let actor = administrator(&store).await;
	memory::operate(
		&store,
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
	let mut fact = content("Research comet fact");
	fact.evidence = vec![proof];
	memory::mutate(
		&store,
		&actor,
		Mutation {
			operation_id: Uuid::now_v7(),
			provider: provider.clone(),
			bank: bank.clone(),
			changes: vec![Change::Add { id, content: fact }],
		},
	)
	.await
	.unwrap();
	let mut tx = store.pool.begin(&TEST_FIXTURE).await.unwrap();
	native::query(
		&Query::delete()
			.from_table(Alias::new("run_context_events"))
			.and_where(Expr::col("run_id").eq(Expr::value(run_id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	tx.commit().await.unwrap();
	let read = || memory::ReadBank {
		provider: provider.clone(),
		bank: bank.clone(),
	};
	assert!(
		memory::list(&store, &actor, read())
			.await
			.unwrap()
			.iter()
			.any(|unit| unit.id == id)
	);
	let mut tx = store.pool.begin(&TEST_FIXTURE).await.unwrap();
	if loss == "account_terms" {
		native::query(
			&Query::update()
				.table(Alias::new("research_journal_results"))
				.value(Alias::new("bytes"), None::<Vec<u8>>)
				.value(Alias::new("purge_reason"), "account_terms")
				.and_where(Expr::col("run_id").eq(Expr::value(run_id)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await
		.unwrap();
	} else {
		let removed = aidash_domain::research::revocation::Removed::default();
		native::query(
			&Query::insert()
				.into_table(Alias::new("research_revocations"))
				.columns(
					[
						"run_id",
						"tenant",
						"revoked_by",
						"reason",
						"revoked_at",
						"removed",
					]
					.map(Alias::new),
				)
				.from_subquery(
					Query::select()
						.expr(Expr::value(run_id))
						.expr(Expr::value("acme"))
						.expr(Expr::value(
							json!({"kind":"subject","tenant":"acme","subject":"alice"}),
						))
						.expr(Expr::value("test"))
						.expr(Expr::value(chrono::Utc::now()))
						.expr(Expr::value(serde_json::to_value(removed).unwrap()))
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await
		.unwrap();
	}
	tx.commit().await.unwrap();
	assert!(
		!memory::list(&store, &actor, read())
			.await
			.unwrap()
			.iter()
			.any(|unit| unit.id == id)
	);
}
