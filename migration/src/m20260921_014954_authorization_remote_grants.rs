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
                    .table(Alias::new("authorization_remote_grants"))
                    .col(col("id").uuid().not_null().primary_key())
                    .col(col("task_id").uuid().not_null())
                    .col(col("task_revision").big_integer().not_null())
                    .col(col("workspace_id").uuid().not_null())
                    .col(col("node_id").text().not_null())
                    .col(col("tenant").text().not_null())
                    .col(col("credential_id").uuid().not_null())
                    .col(col("root_subject").text().not_null())
                    .col(col("subject_chain").array(ColumnType::Text).not_null())
                    .col(col("inspection").json_binary().not_null())
                    .col(col("expires_at").timestamp_with_time_zone().not_null())
                    .col(col("revoked").boolean().not_null().default(false))
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                Alias::new("authorization_remote_grants"),
                                Alias::new("task_id"),
                            )
                            .to(Alias::new("tasks"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager.get_connection().execute_unprepared("CREATE TRIGGER atomic_write_guard BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON authorization_remote_grants FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard()").await?;
        Ok(())
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("authorization_remote_grants"))
                    .to_owned(),
            )
            .await
    }
}
