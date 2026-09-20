use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if crate::legacy_applied(manager, 7, "0184b2b23defa7313327c202d80226e1d4c4676c4de233c64699f8e635bcab40e82f89ef81ad2eb9d486e16fd5a8d54d").await? { return Ok(()); }
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("conversations"))
                    .add_column(
                        ColumnDef::new(Alias::new("created_by"))
                            .text()
                            .not_null()
                            .default(Expr::cust("'human'")),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("human_requests"))
                    .add_column(ColumnDef::new(Alias::new("answered_by")).text())
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        crate::ensure_not_legacy(manager, 7).await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("human_requests"))
                    .drop_column(Alias::new("answered_by"))
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("conversations"))
                    .drop_column(Alias::new("created_by"))
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}
