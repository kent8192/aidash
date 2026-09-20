pub use sea_orm_migration::prelude::*;
mod m20260920_200623_m0001_core;
mod m20260920_200623_m0002_federation;
mod m20260920_200623_m0003_task_abandonment;
mod m20260920_200623_m0004_authorization;
mod m20260920_200623_m0005_subject_credentials;
mod m20260920_200623_m0006_execution_authority;
mod m20260920_200623_m0007_interaction_authority;
mod m20260920_200623_m0008_generation;
mod m20260920_200623_m0009_generation_compaction;
mod m20260920_200623_m0010_delivery_retries;
mod m20260920_204136_m0011_resource_reads;

pub struct Migrator;
#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260920_200623_m0001_core::Migration),
            Box::new(m20260920_200623_m0002_federation::Migration),
            Box::new(m20260920_200623_m0003_task_abandonment::Migration),
            Box::new(m20260920_200623_m0004_authorization::Migration),
            Box::new(m20260920_200623_m0005_subject_credentials::Migration),
            Box::new(m20260920_200623_m0006_execution_authority::Migration),
            Box::new(m20260920_200623_m0007_interaction_authority::Migration),
            Box::new(m20260920_200623_m0008_generation::Migration),
            Box::new(m20260920_200623_m0009_generation_compaction::Migration),
            Box::new(m20260920_200623_m0010_delivery_retries::Migration),
            Box::new(m20260920_204136_m0011_resource_reads::Migration),
        ]
    }
}

// Adopt verified SQLx-era migrations without recreating tables or losing data.
async fn legacy_row(
    manager: &SchemaManager<'_>,
    version: i64,
) -> Result<Option<sea_orm::QueryResult>, DbErr> {
    if !manager.has_table("_sqlx_migrations").await? {
        return Ok(None);
    }
    manager
        .get_connection()
        .query_one(sea_orm::Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            "SELECT checksum,success FROM _sqlx_migrations WHERE version=$1",
            [version.into()],
        ))
        .await
}
async fn legacy_applied(
    manager: &SchemaManager<'_>,
    version: i64,
    checksum: &str,
) -> Result<bool, DbErr> {
    let Some(row) = legacy_row(manager, version).await? else {
        return Ok(false);
    };
    let actual: Vec<u8> = row.try_get("", "checksum")?;
    let success: bool = row.try_get("", "success")?;
    let actual: String = actual.iter().map(|byte| format!("{byte:02x}")).collect();
    if !success || actual != checksum {
        return Err(DbErr::Custom(format!(
            "legacy migration {version} failed or its checksum changed"
        )));
    }
    Ok(true)
}
async fn ensure_not_legacy(manager: &SchemaManager<'_>, version: i64) -> Result<(), DbErr> {
    if legacy_row(manager, version).await?.is_some() {
        return Err(DbErr::Custom(
            "adopted SQLx migrations cannot be rolled back by SeaORM".into(),
        ));
    }
    Ok(())
}
