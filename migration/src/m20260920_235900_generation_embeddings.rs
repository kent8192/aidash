// SeaQuery 0.32 cannot express PostgreSQL trigger functions, triggers, or ALTER CHECK constraints.
// Those DDL operations intentionally use SeaORM execution; ordinary queries use builders.
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;
fn column(name: &str) -> ColumnDef {
    ColumnDef::new(Alias::new(name))
}
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
        // SeaQuery does not expose ALTER TABLE CHECK replacement or triggers.
        m.get_connection().execute_unprepared("ALTER TABLE registry DROP CONSTRAINT registry_kind_check; ALTER TABLE registry ADD CONSTRAINT registry_kind_check CHECK (kind IN ('agent','model','tool','skill','cluster','node','compactor','embedding'))").await?;
        for (table, name) in [
            ("generation_policies", "allocated_embedding_calls"),
            ("generation_budgets", "embedding_call_limit"),
            ("generation_budgets", "embedding_calls"),
        ] {
            m.alter_table(
                Table::alter()
                    .table(Alias::new(table))
                    .add_column(
                        column(name)
                            .big_integer()
                            .not_null()
                            .default(0)
                            .check(Expr::cust(format!("{name} >= 0"))),
                    )
                    .to_owned(),
            )
            .await?;
        }
        m.get_connection().execute_unprepared("ALTER TABLE generation_budgets ADD CONSTRAINT embedding_calls_bounded CHECK (embedding_calls <= embedding_call_limit)").await?;
        let mut table = Table::create();
        table
            .table(Alias::new("generation_embedding_usage"))
            .col(column("request_id").uuid().not_null())
            .col(column("attempt_id").uuid().not_null())
            .col(column("workspace_id").uuid().not_null())
            .col(column("run_id").uuid())
            // This durable ledger commits while the indexer holds the source
            // FOR UPDATE in its authority transaction. A source FK would wait
            // for that same transaction; retain its immutable ID as provenance.
            .col(column("entry_id").uuid())
            .col(
                column("purpose")
                    .text()
                    .not_null()
                    .check(Expr::col(Alias::new("purpose")).is_in(["query", "index"])),
            )
            .col(column("provider_id").text().not_null())
            .col(column("provider_version").text().not_null())
            .col(
                column("request_bytes")
                    .big_integer()
                    .not_null()
                    .check(Expr::cust("request_bytes > 0")),
            )
            .col(
                column("reserved_tokens")
                    .big_integer()
                    .not_null()
                    .check(Expr::cust("reserved_tokens > 0")),
            )
            .col(
                column("reported_tokens")
                    .big_integer()
                    .check(Expr::cust("reported_tokens >= 0")),
            )
            .col(
                column("created_at")
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
                    .from_tbl(Alias::new("generation_embedding_usage"))
                    .from_col(Alias::new("provider_id"))
                    .from_col(Alias::new("provider_version"))
                    .to_tbl(Alias::new("registry"))
                    .to_col(Alias::new("id"))
                    .to_col(Alias::new("version")),
            );
        for (name, target) in [
            ("request_id", "generation_requests"),
            ("workspace_id", "workspaces"),
            ("run_id", "runs"),
        ] {
            table.foreign_key(
                ForeignKey::create()
                    .from(Alias::new("generation_embedding_usage"), Alias::new(name))
                    .to(Alias::new(target), Alias::new("id")),
            );
        }
        m.create_table(table.to_owned()).await?;
        m.get_connection().execute_unprepared("CREATE TRIGGER atomic_write_guard BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON generation_embedding_usage FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard()").await?;
        Ok(())
    }
    async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
        m.drop_table(
            Table::drop()
                .table(Alias::new("generation_embedding_usage"))
                .to_owned(),
        )
        .await?;
        for (table, name) in [
            ("generation_budgets", "embedding_calls"),
            ("generation_budgets", "embedding_call_limit"),
            ("generation_policies", "allocated_embedding_calls"),
        ] {
            m.alter_table(
                Table::alter()
                    .table(Alias::new(table))
                    .drop_column(Alias::new(name))
                    .to_owned(),
            )
            .await?;
        }
        m.get_connection().execute_unprepared("ALTER TABLE registry DROP CONSTRAINT registry_kind_check; ALTER TABLE registry ADD CONSTRAINT registry_kind_check CHECK (kind IN ('agent','model','tool','skill','cluster','node','compactor'))").await?;
        Ok(())
    }
}
