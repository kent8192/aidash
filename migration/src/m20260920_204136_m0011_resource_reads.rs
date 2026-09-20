use sea_orm_migration::prelude::*;
#[derive(DeriveMigrationName)]
pub struct Migration;
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if crate::legacy_applied(manager,10,"ce739651f7e0630527879f555fb6754d22fc0d6fe1fe48cfd93c9797953a24b11b982a0773481d24008e13a2f63719f0").await? { return Ok(()); }
        manager
            .create_table(
                Table::create()
                    .table(Alias::new("authorization_run_reads"))
                    .col(ColumnDef::new(Alias::new("run_id")).uuid().not_null())
                    .col(ColumnDef::new(Alias::new("workspace_id")).uuid().not_null())
                    .col(
                        ColumnDef::new(Alias::new("resource_kind"))
                            .text()
                            .not_null()
                            .check(Expr::col(Alias::new("resource_kind")).is_in([
                                "task",
                                "artifact",
                                "message",
                                "run",
                                "conversation",
                                "generation",
                            ])),
                    )
                    .col(ColumnDef::new(Alias::new("resource_id")).uuid().not_null())
                    .primary_key(
                        Index::create()
                            .col(Alias::new("run_id"))
                            .col(Alias::new("resource_kind"))
                            .col(Alias::new("resource_id")),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(Alias::new("authorization_run_reads"), Alias::new("run_id"))
                            .to(Alias::new("runs"), Alias::new("id")),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(
                                Alias::new("authorization_run_reads"),
                                Alias::new("workspace_id"),
                            )
                            .to(
                                Alias::new("authorization_workspaces"),
                                Alias::new("workspace_id"),
                            ),
                    )
                    .to_owned(),
            )
            .await?;
        // Preserve read dependencies for journals produced before source tracking.
        let mut sources = Query::select();
        for (index, (kind, table)) in [
            ("task", "tasks"),
            ("artifact", "artifacts"),
            ("message", "messages"),
            ("conversation", "conversations"),
            ("generation", "generation_requests"),
            ("run", "runs"),
        ]
        .into_iter()
        .enumerate()
        {
            let mut query = Query::select();
            query
                .expr_as(Expr::val(kind), Alias::new("kind"))
                .column((Alias::new("source"), Alias::new("id")))
                .from_as(Alias::new(table), Alias::new("source"))
                .and_where(
                    Expr::col((Alias::new("source"), Alias::new("workspace_id")))
                        .equals((Alias::new("e"), Alias::new("workspace_id"))),
                );
            if kind == "run" {
                query.and_where(
                    Expr::col((Alias::new("source"), Alias::new("id")))
                        .ne(Expr::col((Alias::new("e"), Alias::new("run_id")))),
                );
            }
            if index == 0 {
                sources = query;
            } else {
                sources.union(UnionType::All, query);
            }
        }
        let select = Query::select()
            .columns([
                (Alias::new("e"), Alias::new("run_id")),
                (Alias::new("e"), Alias::new("workspace_id")),
                (Alias::new("s"), Alias::new("kind")),
                (Alias::new("s"), Alias::new("id")),
            ])
            .from_as(Alias::new("authorization_execution"), Alias::new("e"))
            .join_as(
                JoinType::InnerJoin,
                Alias::new("runs"),
                Alias::new("r"),
                Expr::col((Alias::new("r"), Alias::new("id")))
                    .equals((Alias::new("e"), Alias::new("run_id"))),
            )
            .join_lateral(
                JoinType::InnerJoin,
                sources,
                Alias::new("s"),
                Expr::cust("true"),
            )
            .cond_where(
                Condition::any()
                    .add(Expr::col((Alias::new("r"), Alias::new("phase"))).ne("READY"))
                    .add(
                        Expr::col((Alias::new("r"), Alias::new("context")))
                            .ne(Expr::cust("'{}'::jsonb")),
                    )
                    .add(
                        Expr::col((Alias::new("r"), Alias::new("pending")))
                            .ne(Expr::cust("'{}'::jsonb")),
                    ),
            )
            .to_owned();
        manager
            .exec_stmt(
                Query::insert()
                    .into_table(Alias::new("authorization_run_reads"))
                    .columns([
                        Alias::new("run_id"),
                        Alias::new("workspace_id"),
                        Alias::new("resource_kind"),
                        Alias::new("resource_id"),
                    ])
                    .select_from(select)
                    .map_err(|error| DbErr::Custom(error.to_string()))?
                    .on_conflict(OnConflict::new().do_nothing().to_owned())
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        crate::ensure_not_legacy(manager, 10).await?;
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("authorization_run_reads"))
                    .to_owned(),
            )
            .await
    }
}
