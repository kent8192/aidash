//! Actual cleanup failures retain finite intent without blocking unrelated deletions.
use super::*;
use aidash_server::database::native;

#[rstest]
#[tokio::test]
async fn physical_purge_failure_is_bounded_fair_and_survives_adapter_recreation(
	#[future] database: DatabaseFixture,
	bounds: Bounds,
) {
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
	assert_eq!(failed.try_get::<String>("state").unwrap(), "pending");
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
	assert_eq!(row.try_get::<String>("state").unwrap(), "purged");
	assert_eq!(row.try_get::<i32>("attempts").unwrap(), 2);
	// Initial attempt plus two pinned retries: persistent storage failure must
	// stop scheduling, including after adapter recreation and repeated sweeps.
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
	assert_eq!(exhausted.try_get::<i32>("attempts").unwrap(), 3);
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
