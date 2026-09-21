use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;
fn col(name: &str) -> ColumnDef {
    ColumnDef::new(Alias::new(name))
}
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for (name, history) in [
            ("authorization_peer_mappings", false),
            ("authorization_peer_mapping_history", true),
        ] {
            let mut table = Table::create();
            table.table(Alias::new(name));
            if history {
                table.col(col("sequence").big_integer().auto_increment().primary_key());
            }
            for field in ["source_node", "source_tenant", "source_subject", "tenant"] {
                table.col(col(field).text().not_null());
            }
            table
                .col(col("credential_id").uuid().not_null())
                .col(col("enabled").boolean().not_null())
                .col(
                    col("revision")
                        .big_integer()
                        .not_null()
                        .check(Expr::cust("revision > 0")),
                )
                .col(col("actor").text().not_null())
                .col(
                    col("updated_at")
                        .timestamp_with_time_zone()
                        .not_null()
                        .default(Expr::cust("clock_timestamp()")),
                )
                .foreign_key(
                    ForeignKey::create()
                        .from(Alias::new(name), Alias::new("credential_id"))
                        .to(Alias::new("authorization_credentials"), Alias::new("id")),
                )
                .foreign_key(
                    ForeignKey::create()
                        .from(Alias::new(name), Alias::new("tenant"))
                        .to(Alias::new("authorization_bundles"), Alias::new("tenant")),
                );
            if !history {
                table.primary_key(
                    Index::create()
                        .col(Alias::new("source_node"))
                        .col(Alias::new("source_tenant"))
                        .col(Alias::new("source_subject")),
                );
            }
            manager.create_table(table.to_owned()).await?;
            manager.get_connection().execute_unprepared(&format!("CREATE TRIGGER atomic_write_guard BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON {name} FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard()")).await?;
        }
        manager
            .create_index(
                Index::create()
                    .name("authorization_peer_mapping_history_tenant_sequence")
                    .table(Alias::new("authorization_peer_mapping_history"))
                    .col(Alias::new("tenant"))
                    .col(Alias::new("sequence"))
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for name in [
            "authorization_peer_mapping_history",
            "authorization_peer_mappings",
        ] {
            manager
                .drop_table(Table::drop().table(Alias::new(name)).to_owned())
                .await?;
        }
        Ok(())
    }
}
