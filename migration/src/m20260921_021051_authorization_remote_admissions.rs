use sea_orm_migration::prelude::*;
#[derive(DeriveMigrationName)]
pub struct Migration;
fn col(name: &str) -> ColumnDef {
    ColumnDef::new(Alias::new(name))
}
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("authorization_remote_admissions"))
                    .col(col("id").uuid().not_null().primary_key())
                    .col(col("source_node").text().not_null())
                    .col(col("grant_id").uuid().not_null())
                    .col(col("task_id").uuid().not_null())
                    .col(col("tenant").text().not_null())
                    .col(col("credential_id").uuid().not_null())
                    .col(col("subject_chain").array(ColumnType::Text).not_null())
                    .col(col("description").json_binary().not_null())
                    .col(
                        col("created_at")
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .index(
                        Index::create()
                            .unique()
                            .col(Alias::new("source_node"))
                            .col(Alias::new("grant_id")),
                    )
                    .index(
                        Index::create()
                            .unique()
                            .col(Alias::new("source_node"))
                            .col(Alias::new("task_id")),
                    )
                    .to_owned(),
            )
            .await?;
        // SeaQuery has no PostgreSQL CREATE TRIGGER builder.
        manager.get_connection().execute_unprepared("CREATE TRIGGER atomic_write_guard BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON authorization_remote_admissions FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard()").await?;
        Ok(())
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("authorization_remote_admissions"))
                    .to_owned(),
            )
            .await
    }
}
