// This suite only needs the shared database fixture, not the API helpers.
#[allow(dead_code)]
mod common;
use common::{TestEnvironment, test_environment};
use migration::{Migrator, MigratorTrait};

#[rstest::rstest]
#[tokio::test]
async fn seaorm_migrations_round_trip_a_fresh_schema(
	#[future(awt)]
	#[from(test_environment)]
	_test_environment: std::sync::Arc<TestEnvironment>,
) {
	let (f, url, schema) = common::setup().await;
	let db = sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(f.store.pool.clone());
	assert_eq!(
		Migrator::get_applied_migrations(&db).await.unwrap().len(),
		Migrator::migrations().len()
	);
	Migrator::down(&db, None).await.unwrap();
	assert!(
		Migrator::get_applied_migrations(&db)
			.await
			.unwrap()
			.is_empty()
	);
	Migrator::up(&db, None).await.unwrap();
	f.store
		.create_workspace("After migration", "Schema was rebuilt")
		.await
		.unwrap();
	common::cleanup(f, &url, &schema).await;
}
