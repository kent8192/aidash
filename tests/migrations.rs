// This suite only needs the shared database fixture, not the API helpers.
#[allow(dead_code)]
mod common;
use migration::{Migrator, MigratorTrait};

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn seaorm_migrations_round_trip_a_fresh_schema() {
    let (f, url, schema) = common::setup().await;
    let db = sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(f.store.pool.clone());
    assert_eq!(
        Migrator::get_applied_migrations(&db).await.unwrap().len(),
        13
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

#[tokio::test]
#[ignore = "requires disposable PostgreSQL"]
async fn verified_sqlx_history_is_adopted_without_losing_data() {
    use sea_orm::{ConnectionTrait, DbBackend, Statement};
    let (f, url, schema) = common::setup().await;
    let workspace = f
        .store
        .create_workspace("Existing data", "Keep this workspace")
        .await
        .unwrap();
    let db = sea_orm::SqlxPostgresConnector::from_sqlx_postgres_pool(f.store.pool.clone());
    // Reproduce a database carrying the previous SQLx core migration ledger.
    db.execute_unprepared("CREATE TABLE _sqlx_migrations(version bigint PRIMARY KEY,checksum bytea NOT NULL,success boolean NOT NULL)").await.unwrap();
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres,
        "INSERT INTO _sqlx_migrations VALUES(1,decode($1,'hex'),true)",
        ["bc84c62b45ab09a7de69aeae16ebcb82f7b0b3e33f453ea9c8dc85f11b29aa4e3d43f69f2f4970488cce841e423e8f99".into()])).await.unwrap();
    db.execute_unprepared("DELETE FROM seaql_migrations WHERE version LIKE '%m0001_core'")
        .await
        .unwrap();
    Migrator::up(&db, None).await.unwrap();
    assert_eq!(
        f.store.workspace(workspace.id).await.unwrap().goal,
        "Keep this workspace"
    );
    db.execute_unprepared("DELETE FROM seaql_migrations WHERE version LIKE '%m0001_core'; UPDATE _sqlx_migrations SET checksum=decode('00','hex')").await.unwrap();
    assert!(Migrator::up(&db, None).await.is_err());
    assert_eq!(
        f.store.workspace(workspace.id).await.unwrap().goal,
        "Keep this workspace"
    );
    common::cleanup(f, &url, &schema).await;
}
