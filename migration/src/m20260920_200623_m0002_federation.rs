use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("delegations"))
                    .col(ColumnDef::new(Alias::new("task_id")).uuid().primary_key())
                    .col(ColumnDef::new(Alias::new("node_id")).text().not_null())
                    .col(ColumnDef::new(Alias::new("agent_id")).text().not_null())
                    .col(
                        ColumnDef::new(Alias::new("agent_version"))
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("delivered"))
                            .boolean()
                            .not_null()
                            .default(Expr::cust("false")),
                    )
                    .col(
                        ColumnDef::new(Alias::new("created_at"))
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::cust("now()")),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from_tbl(Alias::new("delegations"))
                            .from_col(Alias::new("task_id"))
                            .to_tbl(Alias::new("tasks"))
                            .to_col(Alias::new("id")),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("peer_events"))
                    .col(ColumnDef::new(Alias::new("node_id")).text().not_null())
                    .col(ColumnDef::new(Alias::new("event_id")).uuid().not_null())
                    .primary_key(
                        Index::create()
                            .col(Alias::new("node_id"))
                            .col(Alias::new("event_id")),
                    )
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Alias::new("peer_events")).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Alias::new("delegations")).to_owned())
            .await?;
        Ok(())
    }
}
