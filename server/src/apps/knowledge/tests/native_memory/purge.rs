//! Actual cleanup failures retain finite intent without blocking unrelated deletions.
use super::*;
use aidash_server::database::native;

#[rstest]
#[tokio::test]
async fn purge_erases_transitive_quotes_after_history_retention_expires(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
	let database = database.await;
	let (store, registry, workspace) = setup(&database, bounds).await;
	let mut provider = registry.get("p", "1.0.0").await.unwrap();
	provider.id = "short-history".into();
	provider.config["policy"]["retention"]["history_days"] = json!(1);
	provider.config["policy"]["retention"]["purge_after_seconds"] = json!(86_400);
	registry.register(provider).await.unwrap();
	let bank = Bank {
		home: store.node_id.clone(),
		tenant: "acme".into(),
		workspace,
		participant: None,
	};
	memory::operate(
		&store,
		&Actor::Operator,
		memory::Operation {
			operation_id: Uuid::now_v7(),
			provider: reference("short-history"),
			bank: bank.clone(),
			action: memory::Action::ConfigureBank {
				expected_revision: 0,
			},
		},
	)
	.await
	.unwrap();
	let mut chain: Vec<Unit> = Vec::new();
	for _ in 0..3 {
		let mut body = content("Quoted private deletion source / 消去する引用");
		if let Some(parent) = chain.last() {
			body.kind = Kind::Observation;
			body.evidence = vec![parent.evidence()];
		}
		let mut input = mutation(
			&bank,
			Change::Add {
				id: Uuid::now_v7(),
				content: body,
			},
		);
		input.provider = reference("short-history");
		chain.push(
			memory::mutate(&store, &Actor::Operator, input)
				.await
				.unwrap()
				.remove(0),
		);
	}
	let independent = Uuid::now_v7();
	let mut input = mutation(
		&bank,
		Change::Add {
			id: independent,
			content: content("Independent current memory"),
		},
	);
	input.provider = reference("short-history");
	memory::mutate(&store, &Actor::Operator, input)
		.await
		.unwrap();
	let mut deletion = mutation(
		&bank,
		Change::Delete {
			id: chain[0].id,
			expected_revision: 1,
		},
	);
	deletion.provider = reference("short-history");
	memory::mutate(&store, &Actor::Operator, deletion)
		.await
		.unwrap();
	// Advance only disposable history and job clocks. Canonical bodies and the
	// independent recovery ledger still come from the real mutation path.
	let mut tx = native::begin(&store.pool).await.unwrap();
	for query in [
		Query::update()
			.table(Alias::new("memory_history"))
			.value(
				Alias::new("updated_at"),
				chrono::Utc::now() - chrono::Duration::days(2),
			)
			.to_string(PostgresQueryBuilder),
		Query::update()
			.table(Alias::new("memory_purge_jobs"))
			.value(
				Alias::new("purge_after"),
				chrono::Utc::now() - chrono::Duration::seconds(1),
			)
			.to_string(PostgresQueryBuilder),
	] {
		native::query(&query).execute(&mut *tx).await.unwrap();
	}
	tx.commit().await.unwrap();
	aidash_server::semantic::worker::sweep(&store)
		.await
		.unwrap();
	let bodies = native::query(
		&Query::select()
			.columns(["id", "text", "stale"].map(Alias::new))
			.from(Alias::new("memory_units"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&store.pool)
	.await
	.unwrap();
	for unit in &chain {
		let row = bodies
			.iter()
			.find(|row| row.try_get::<Uuid>("id").unwrap() == unit.id)
			.unwrap();
		assert!(
			row.try_get::<String>("text").unwrap().is_empty(),
			"expired history must not strand a quoted body"
		);
		if unit.id != chain[0].id {
			assert!(row.try_get::<bool>("stale").unwrap());
		}
	}
	assert_eq!(
		bodies
			.iter()
			.find(|row| row.try_get::<Uuid>("id").unwrap() == independent)
			.unwrap()
			.try_get::<String>("text")
			.unwrap(),
		"Independent current memory"
	);
	let remaining: i64 = native::query_scalar(
		&Query::select()
			.expr(reinhardt::query::Func::count(Expr::col("unit_id").into()))
			.from(Alias::new("memory_history"))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&store.pool)
	.await
	.unwrap();
	assert_eq!(
		remaining, 0,
		"retention also expires the independent unit's history"
	);
	let state: String = native::query_scalar(
		&Query::select()
			.column(Alias::new("state"))
			.from(Alias::new("memory_purge_jobs"))
			.and_where(Expr::col("unit_id").eq(Expr::value(chain[0].id)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&store.pool)
	.await
	.unwrap();
	assert_eq!(state, "purged");
}

#[rstest]
#[case(1)]
#[case(2)]
#[tokio::test]
async fn physical_purge_failure_is_bounded_fair_and_survives_adapter_recreation(
	#[future] database: DatabaseFixture,
	mut bounds: Bounds,
	#[case] max_attempts: usize,
) {
	bounds.max_retries = max_attempts;
	let database = database.await;
	let (store, _, workspace) = setup(&database, bounds).await;
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
	let protected = Uuid::now_v7();
	let independent = Uuid::now_v7();
	let permanent = Uuid::now_v7();
	for id in [protected, independent, permanent] {
		memory::mutate(
			&store,
			&Actor::Operator,
			mutation(
				&bank,
				Change::Add {
					id,
					content: content("Private cleanup fixture / 削除の検証用記憶"),
				},
			),
		)
		.await
		.unwrap();
	}
	// Fixture-only DDL injects a real PostgreSQL DELETE failure without bypassing
	// visibility, tombstones or production cleanup. Runtime queries stay typed.
	sqlx::query("CREATE TABLE fixture_memory_history_hold (unit_id UUID, revision BIGINT, FOREIGN KEY (unit_id, revision) REFERENCES memory_history(unit_id, revision))")
		.execute(store.pool.driver()).await.unwrap();
	let mut tx = native::begin(&store.pool).await.unwrap();
	for held in [protected, permanent] {
		native::query(
			&Query::insert()
				.into_table(Alias::new("fixture_memory_history_hold"))
				.columns([Alias::new("unit_id"), Alias::new("revision")])
				.from_subquery(
					Query::select()
						.expr(Expr::value(held))
						.expr(Expr::value(1_i64))
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await
		.unwrap();
	}
	tx.commit().await.unwrap();
	for id in [protected, independent, permanent] {
		memory::mutate(
			&store,
			&Actor::Operator,
			mutation(
				&bank,
				Change::Delete {
					id,
					expected_revision: 1,
				},
			),
		)
		.await
		.unwrap();
	}
	async fn due(store: &Store) {
		let mut tx = native::begin(&store.pool).await.unwrap();
		native::query(
			&Query::update()
				.table(Alias::new("memory_purge_jobs"))
				.value(
					Alias::new("purge_after"),
					chrono::Utc::now() - chrono::Duration::seconds(1),
				)
				.value(
					Alias::new("next_attempt"),
					chrono::Utc::now() - chrono::Duration::seconds(1),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await
		.unwrap();
		tx.commit().await.unwrap();
	}
	due(&store).await;
	// A ledger outage before policy loading must not exhaust the retry budget.
	let ledger_path = database.recovery_directory.path().join("ledger.cbor");
	let original_ledger = std::fs::read(&ledger_path).unwrap();
	let mut ledger: aidash_domain::memory::recovery::Ledger =
		ciborium::de::from_reader(&original_ledger[40..]).unwrap();
	ledger.units.remove(&protected);
	write_fixture_archive(&ledger_path, &ledger);
	aidash_server::semantic::worker::sweep(&store)
		.await
		.unwrap();
	std::fs::write(&ledger_path, original_ledger).unwrap();
	due(&store).await;
	aidash_server::semantic::worker::sweep(&store)
		.await
		.unwrap();
	let states = native::query(
		&Query::select()
			.columns(["unit_id", "state", "attempts", "last_error"].map(Alias::new))
			.from(Alias::new("memory_purge_jobs"))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&store.pool)
	.await
	.unwrap();
	let failed = states
		.iter()
		.find(|row| row.try_get::<Uuid>("unit_id").unwrap() == protected)
		.unwrap();
	assert_eq!(
		failed.try_get::<String>("state").unwrap(),
		if max_attempts == 1 {
			"failed"
		} else {
			"pending"
		}
	);
	assert_eq!(failed.try_get::<i32>("attempts").unwrap(), 1);
	assert_eq!(
		failed.try_get::<String>("last_error").unwrap(),
		"purge_storage_unavailable"
	);
	let other = states
		.iter()
		.find(|row| row.try_get::<Uuid>("unit_id").unwrap() == independent)
		.unwrap();
	assert_eq!(
		other.try_get::<String>("state").unwrap(),
		"purged",
		"the failed oldest job must not starve another deletion"
	);
	assert!(
		memory::list(
			&store,
			&Actor::Operator,
			memory::ReadBank {
				provider: reference("p"),
				bank
			}
		)
		.await
		.unwrap()
		.is_empty()
	);
	let mut tx = native::begin(&store.pool).await.unwrap();
	native::query(
		&Query::delete()
			.from_table(Alias::new("fixture_memory_history_hold"))
			.and_where(Expr::col("unit_id").eq(Expr::value(protected)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await
	.unwrap();
	tx.commit().await.unwrap();
	let reopened = Store::from_pool(store.pool.driver().clone(), store.node_id.clone())
		.await
		.unwrap();
	let reopened = aidash_server::semantic::services::memory_recovery::attach(
		reopened,
		database.recovery_directory.path().to_owned(),
	)
	.unwrap();
	due(&reopened).await;
	aidash_server::semantic::worker::sweep(&reopened)
		.await
		.unwrap();
	let row = native::query(
		&Query::select()
			.columns(["state", "attempts"].map(Alias::new))
			.from(Alias::new("memory_purge_jobs"))
			.and_where(Expr::col("unit_id").eq(Expr::value(protected)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&reopened.pool)
	.await
	.unwrap();
	assert_eq!(
		row.try_get::<String>("state").unwrap(),
		if max_attempts == 1 {
			"failed"
		} else {
			"purged"
		}
	);
	assert_eq!(row.try_get::<i32>("attempts").unwrap(), max_attempts as i32);
	// The pinned total attempt limit, including one, stops persistent failures
	// from being scheduled after adapter recreation and repeated sweeps.
	for _ in 0..2 {
		due(&reopened).await;
		aidash_server::semantic::worker::sweep(&reopened)
			.await
			.unwrap();
	}
	let exhausted = native::query(
		&Query::select()
			.columns(["state", "attempts", "last_error"].map(Alias::new))
			.from(Alias::new("memory_purge_jobs"))
			.and_where(Expr::col("unit_id").eq(Expr::value(permanent)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&reopened.pool)
	.await
	.unwrap();
	assert_eq!(exhausted.try_get::<String>("state").unwrap(), "failed");
	assert_eq!(
		exhausted.try_get::<i32>("attempts").unwrap(),
		max_attempts as i32
	);
	assert_eq!(
		exhausted.try_get::<String>("last_error").unwrap(),
		"purge_storage_unavailable"
	);
	sqlx::query("DROP TABLE fixture_memory_history_hold")
		.execute(store.pool.driver())
		.await
		.unwrap();
	let bodies: Vec<String> = native::query_scalar(
		&Query::select()
			.column(Alias::new("text"))
			.from(Alias::new("memory_units"))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_all(&reopened.pool)
	.await
	.unwrap();
	assert!(bodies.iter().all(String::is_empty));
}
