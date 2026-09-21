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
                    .table(Alias::new("authorization_run_remote_reads"))
                    .col(col("run_id").uuid().not_null())
                    .col(col("node_id").text().not_null())
                    .col(col("entry_id").text().not_null())
                    .col(col("entry_version").text().not_null())
                    .col(col("digest").text().not_null())
                    .col(col("metadata").json_binary().not_null())
                    .primary_key(
                        Index::create()
                            .col(Alias::new("run_id"))
                            .col(Alias::new("node_id"))
                            .col(Alias::new("entry_id"))
                            .col(Alias::new("entry_version"))
                            .col(Alias::new("digest")),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                Alias::new("authorization_run_remote_reads"),
                                Alias::new("run_id"),
                            )
                            .to(Alias::new("runs"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager.get_connection().execute_unprepared("CREATE TRIGGER atomic_write_guard BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON authorization_run_remote_reads FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard()").await?;
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("authorization_run_outputs"))
                    .col(col("run_id").uuid().not_null())
                    .col(col("workspace_id").uuid().not_null())
                    .col(col("resource_kind").text().not_null())
                    .col(col("resource_id").uuid().not_null())
                    .primary_key(
                        Index::create()
                            .col(Alias::new("run_id"))
                            .col(Alias::new("resource_kind"))
                            .col(Alias::new("resource_id")),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                Alias::new("authorization_run_outputs"),
                                Alias::new("run_id"),
                            )
                            .to(Alias::new("runs"), Alias::new("id"))
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("authorization_output_resource")
                    .table(Alias::new("authorization_run_outputs"))
                    .col(Alias::new("workspace_id"))
                    .col(Alias::new("resource_kind"))
                    .col(Alias::new("resource_id"))
                    .col(Alias::new("run_id"))
                    .to_owned(),
            )
            .await?;
        // Older releases recorded outputs among reads. Recover authored output
        // membership, retaining every possible producer when attribution overlaps.
        manager.get_connection().execute_unprepared("INSERT INTO authorization_run_outputs SELECT rr.run_id,rr.workspace_id,rr.resource_kind,rr.resource_id FROM authorization_run_reads rr JOIN runs r ON r.id=rr.run_id WHERE r.workspace_id=rr.workspace_id AND ((rr.resource_kind='artifact' AND EXISTS(SELECT 1 FROM artifacts a WHERE a.id=rr.resource_id AND a.workspace_id=rr.workspace_id AND a.created_by=r.home_node || '/agents/' || r.agent_id || '@' || r.agent_version)) OR (rr.resource_kind='message' AND EXISTS(SELECT 1 FROM messages m WHERE m.id=rr.resource_id AND m.workspace_id=rr.workspace_id AND m.sender=r.home_node || '/agents/' || r.agent_id || '@' || r.agent_version))) ON CONFLICT DO NOTHING").await?;
        manager.get_connection().execute_unprepared("CREATE TRIGGER atomic_write_guard BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON authorization_run_outputs FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard()").await?;
        Ok(())
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("authorization_run_outputs"))
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("authorization_run_remote_reads"))
                    .to_owned(),
            )
            .await
    }
}
