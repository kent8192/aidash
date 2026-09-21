// SeaQuery 0.32 cannot express PostgreSQL trigger functions, triggers, or ALTER CHECK constraints.
// Those DDL operations intentionally use SeaORM execution; ordinary queries use builders.
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // SeaQuery 0.32 has no ALTER TABLE CHECK constraint builder.
        manager
            .get_connection()
            .execute_unprepared("ALTER TABLE registry DROP CONSTRAINT registry_kind_check")
            .await?;
        // SeaQuery 0.32 has no ALTER TABLE CHECK constraint builder.
        manager.get_connection().execute_unprepared("ALTER TABLE registry ADD CONSTRAINT registry_kind_check CHECK (kind IN ('agent','model','tool','skill','cluster','node','compactor'))").await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("generation_policies"))
                    .add_column(
                        ColumnDef::new(Alias::new("allocated_compaction_calls"))
                            .big_integer()
                            .not_null()
                            .default(Expr::cust("0"))
                            .check(Expr::cust("allocated_compaction_calls >= 0")),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("generation_budgets"))
                    .add_column(
                        ColumnDef::new(Alias::new("compaction_call_limit"))
                            .big_integer()
                            .not_null()
                            .default(Expr::cust("0"))
                            .check(Expr::cust("compaction_call_limit >= 0")),
                    )
                    .to_owned(),
            )
            .await?;
        manager.alter_table(Table::alter().table(Alias::new("generation_budgets")).add_column(ColumnDef::new(Alias::new("compaction_calls")).big_integer().not_null().default(Expr::cust("0")).check(Expr::cust("compaction_calls >= 0 AND compaction_calls <= compaction_call_limit"))).to_owned()).await?;
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("generation_compaction_usage"))
                    .col(ColumnDef::new(Alias::new("request_id")).uuid().not_null())
                    .col(ColumnDef::new(Alias::new("attempt_id")).uuid().not_null())
                    .col(ColumnDef::new(Alias::new("run_id")).uuid().not_null())
                    .col(ColumnDef::new(Alias::new("provider_id")).text().not_null())
                    .col(
                        ColumnDef::new(Alias::new("provider_version"))
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Alias::new("request_bytes"))
                            .big_integer()
                            .not_null()
                            .check(Expr::cust("request_bytes > 0")),
                    )
                    .col(
                        ColumnDef::new(Alias::new("questions"))
                            .integer()
                            .not_null()
                            .check(Expr::cust("questions > 0")),
                    )
                    .col(
                        ColumnDef::new(Alias::new("created_at"))
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::cust("clock_timestamp()")),
                    )
                    .primary_key(
                        Index::create()
                            .col(Alias::new("request_id"))
                            .col(Alias::new("attempt_id")),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from_tbl(Alias::new("generation_compaction_usage"))
                            .from_col(Alias::new("request_id"))
                            .to_tbl(Alias::new("generation_requests"))
                            .to_col(Alias::new("id")),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from_tbl(Alias::new("generation_compaction_usage"))
                            .from_col(Alias::new("run_id"))
                            .to_tbl(Alias::new("runs"))
                            .to_col(Alias::new("id")),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from_tbl(Alias::new("generation_compaction_usage"))
                            .from_col(Alias::new("provider_id"))
                            .from_col(Alias::new("provider_version"))
                            .to_tbl(Alias::new("registry"))
                            .to_col(Alias::new("id"))
                            .to_col(Alias::new("version")),
                    )
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("generation_compaction_usage"))
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("generation_budgets"))
                    .drop_column(Alias::new("compaction_calls"))
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("generation_budgets"))
                    .drop_column(Alias::new("compaction_call_limit"))
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("generation_policies"))
                    .drop_column(Alias::new("allocated_compaction_calls"))
                    .to_owned(),
            )
            .await?;
        manager.get_connection().execute_unprepared("ALTER TABLE registry DROP CONSTRAINT registry_kind_check; ALTER TABLE registry ADD CONSTRAINT registry_kind_check CHECK (kind IN ('agent','model','tool','skill','cluster','node'))").await?;
        Ok(())
    }
}
