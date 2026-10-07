//! Replacement policies and maintenance keep bank and provider bounds consistent.
use super::*;
use aidash_server::{apps::registry::models::Definition, database::native};

async fn config(database: &DatabaseFixture) -> serde_json::Value {
	Definition::objects()
		.filter(Definition::field_id().eq("p".to_string()))
		.filter(Definition::field_version().eq("1.0.0".to_string()))
		.get_with_db(&mut database.lease.handle())
		.await
		.unwrap()
		.metadata
		.0["config"]
		.clone()
}

#[rstest]
#[case(false, false)]
#[case(false, true)]
#[case(true, false)]
#[case(true, true)]
#[tokio::test]
async fn smaller_storage_policy_rolls_back_shared_changes_and_participant_upgrades(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
	#[case] participant: bool,
	#[case] record_cap: bool,
) {
	let database = database.await;
	let (store, registry, workspace) = setup(&database, bounds).await;
	let bank = if participant {
		memory::create_participant(
			&store,
			&Actor::Operator,
			workspace,
			memory::CreateParticipant {
				agent: reference("a"),
			},
		)
		.await
		.unwrap()
		.bank
	} else {
		Bank {
			home: store.node_id.clone(),
			tenant: "acme".into(),
			workspace,
			participant: None,
		}
	};
	let mut ids = Vec::new();
	for _ in 0..3 {
		let id = Uuid::now_v7();
		memory::mutate(
			&store,
			&Actor::Operator,
			mutation(
				&bank,
				Change::Add {
					id,
					content: content("Storage bound fixture"),
				},
			),
		)
		.await
		.unwrap();
		ids.push(id);
	}
	if record_cap {
		for id in &ids[..2] {
			memory::mutate(
				&store,
				&Actor::Operator,
				mutation(
					&bank,
					Change::Delete {
						id: *id,
						expected_revision: 1,
					},
				),
			)
			.await
			.unwrap();
		}
	}
	let mut replacement = config(&database).await;
	let limits = &mut replacement["policy"]["bounds"];
	limits["max_units"] = json!(1);
	limits["max_candidates"] = json!(1);
	limits["max_results"] = json!(1);
	if record_cap {
		replacement["policy"]["retention"]["max_unit_records"] = json!(2);
	}
	registry
		.register(entry("memory", "small", replacement))
		.await
		.unwrap();
	let settings = || memory::Operation {
		operation_id: Uuid::now_v7(),
		provider: reference("p"),
		bank: bank.clone(),
		action: memory::Action::Settings,
	};
	let memory::Outcome::Settings(Some(before)) =
		memory::operate(&store, &Actor::Operator, settings())
			.await
			.unwrap()
	else {
		panic!("existing settings");
	};
	let result = if participant {
		let mut agent = entry(
			"agent",
			"a",
			json!({"model":reference("m"),"instructions":"Use bounded memory",
			"tools":[],"skills":[],"memory":reference("small"),"allow_memory_write":true}),
		);
		agent.version = "1.1.0".into();
		registry.register(agent).await.unwrap();
		memory::upgrade_participant(
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
		.await
		.map(|_| ())
	} else {
		memory::operate(
			&store,
			&Actor::Operator,
			memory::Operation {
				operation_id: Uuid::now_v7(),
				provider: reference("small"),
				bank: bank.clone(),
				action: memory::Action::ConfigureBank {
					expected_revision: before.revision,
				},
			},
		)
		.await
		.map(|_| ())
	};
	assert!(
		matches!(result, Err(aidash_server::Error::Conflict(message))
		if message == "replacement memory policy is below existing bank storage")
	);
	let memory::Outcome::Settings(Some(after)) =
		memory::operate(&store, &Actor::Operator, settings())
			.await
			.unwrap()
	else {
		panic!("retained settings");
	};
	assert_eq!(after.provider, before.provider);
	assert_eq!(after.revision, before.revision);
	if participant {
		// The failed provider replacement must roll back the Agent CAS as well.
		let still_current = memory::upgrade_participant(
			&store,
			&Actor::Operator,
			bank.clone(),
			memory::UpgradeParticipant {
				agent: reference("a"),
				expected_revision: 1,
			},
		)
		.await
		.unwrap();
		assert_eq!(still_current.revision, 2);
		assert_eq!(still_current.agent, reference("a"));
	}
	memory::mutate(
		&store,
		&Actor::Operator,
		mutation(
			&bank,
			Change::Delete {
				id: ids[2],
				expected_revision: 1,
			},
		),
	)
	.await
	.unwrap();
}

#[rstest]
#[tokio::test]
async fn expiry_drains_in_batches_no_larger_than_mutation_capacity(
	#[future] database: DatabaseFixture,
	mut bounds: Bounds,
) {
	bounds.max_candidates = 2;
	bounds.max_results = 2;
	let database = database.await;
	let (store, _, workspace) = setup_endpoint_retention(
		&database,
		bounds,
		"http://127.0.0.1:9/v1",
		(false, false, false),
		Some(1),
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
	let mut units = Vec::new();
	for _ in 0..5 {
		units.extend(
			memory::mutate(
				&store,
				&Actor::Operator,
				mutation(
					&bank,
					Change::Add {
						id: Uuid::now_v7(),
						content: content("Expired batch fixture"),
					},
				),
			)
			.await
			.unwrap(),
		);
	}
	// Inject the fixture clock into both canonical bodies and independent fences.
	let ledger_path = database.recovery_directory.path().join("ledger.cbor");
	let bytes = std::fs::read(&ledger_path).unwrap();
	let mut ledger: aidash_domain::memory::recovery::Ledger =
		ciborium::de::from_reader(&bytes[40..]).unwrap();
	let mut tx = native::begin(&store.pool).await.unwrap();
	for unit in &mut units {
		unit.learned_at = chrono::DateTime::from_timestamp_micros(
			(chrono::Utc::now() - chrono::Duration::days(2)).timestamp_micros(),
		)
		.unwrap();
		ledger.units.get_mut(&unit.id).unwrap().digest =
			aidash_domain::memory::recovery::digest(unit).unwrap();
		native::query(
			&Query::update()
				.table(Alias::new("memory_units"))
				.value(Alias::new("learned_at"), unit.learned_at)
				.and_where(Expr::col("id").eq(Expr::value(unit.id)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await
		.unwrap();
	}
	tx.commit().await.unwrap();
	write_fixture_archive(&ledger_path, &ledger);
	for expected in [2, 4, 5] {
		let mut tx = native::begin(&store.pool).await.unwrap();
		native::query(
			&Query::update()
				.table(Alias::new("memory_bank_settings"))
				.value(
					Alias::new("next_maintenance"),
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
		let count: i64 = native::query_scalar(
			&Query::select()
				.expr(reinhardt::query::Func::count(
					Expr::col(reinhardt::query::ColumnRef::Asterisk).into(),
				))
				.from(Alias::new("memory_units"))
				.and_where(Expr::col("deleted").eq(true))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_one(&store.pool)
		.await
		.unwrap();
		assert_eq!(count, expected);
	}
}

#[rstest]
#[tokio::test]
async fn semantic_sources_allow_the_pinned_provenance_limit_above_1024(
	#[future] database: DatabaseFixture,
	mut bounds: Bounds,
) {
	use axum::{Json, Router, routing::post};
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
	let server = tokio::spawn(async move {
		axum::serve(listener, Router::new().route("/v1/embeddings", post(|Json(input): Json<serde_json::Value>| async move {
			Json(json!({"model":input["model"],"data":[{"index":0,"embedding":[1.,0.1,0.]}],"usage":{"prompt_tokens":1}}))
		}))).await.unwrap();
	});
	bounds.max_units = 32;
	bounds.max_graph_visits = 4096;
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
	let mut support = Vec::new();
	let mut last = Uuid::nil();
	// A small DAG requires 2046 exact support visits at its last layer.
	for layer in 0..11 {
		let mut next = Vec::new();
		for _ in 0..2 {
			let mut body = content("Pinned semantic provenance budget");
			if layer > 0 {
				body.kind = Kind::Observation;
				body.evidence = support.clone();
			}
			last = Uuid::now_v7();
			let unit = memory::mutate(
				&store,
				&Actor::Operator,
				mutation(
					&bank,
					Change::Add {
						id: last,
						content: body,
					},
				),
			)
			.await
			.unwrap()
			.remove(0);
			next.push(unit.evidence());
		}
		support = next;
	}
	// Old semantic source reads fail here even though admission accepts the DAG.
	aidash_server::semantic::worker::sweep(&store)
		.await
		.unwrap();
	let entries = aidash_server::semantic::service::entries(&store, &Actor::Operator, workspace)
		.await
		.unwrap();
	assert_eq!(
		entries.iter().find(|entry| entry.id == last).unwrap().state,
		"READY"
	);
	server.abort();
}
