use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		rebuild_memory(manager, true).await?;
		manager
			.create_table(
				Table::create()
					.table(Alias::new("authorization_run_registry_reads"))
					.col(ColumnDef::new(Alias::new("run_id")).uuid().not_null())
					.col(ColumnDef::new(Alias::new("entry_id")).text().not_null())
					.col(
						ColumnDef::new(Alias::new("entry_version"))
							.text()
							.not_null(),
					)
					.primary_key(
						Index::create()
							.col(Alias::new("run_id"))
							.col(Alias::new("entry_id"))
							.col(Alias::new("entry_version")),
					)
					.foreign_key(
						ForeignKey::create()
							.from(
								Alias::new("authorization_run_registry_reads"),
								Alias::new("run_id"),
							)
							.to(Alias::new("runs"), Alias::new("id")),
					)
					.to_owned(),
			)
			.await?;
		// Recover discovered entry dependencies already copied into historical
		// contexts, pending responses, and durable invocation results.
		let mut documents = Query::select()
			.column(Alias::new("id"))
			.expr_as(Expr::col(Alias::new("context")), Alias::new("document"))
			.from(Alias::new("runs"))
			.to_owned();
		documents.union(
			UnionType::All,
			Query::select()
				.column(Alias::new("id"))
				.column(Alias::new("pending"))
				.from(Alias::new("runs"))
				.to_owned(),
		);
		documents.union(
			UnionType::All,
			Query::select()
				.column(Alias::new("run_id"))
				.column(Alias::new("result"))
				.from(Alias::new("invocations"))
				.to_owned(),
		);
		let entries = Query::select()
			.column(Alias::new("id"))
			.expr_as(
				Expr::cust("jsonb_path_query(document, '$.**.entity')"),
				Alias::new("entry"),
			)
			.from_subquery(documents, Alias::new("documents"))
			.to_owned();
		let sources = Query::select()
			.distinct()
			.column((Alias::new("entries"), Alias::new("id")))
			.expr(Expr::cust("entry->>'id'"))
			.expr(Expr::cust("entry->>'version'"))
			.from_subquery(entries, Alias::new("entries"))
			.join_as(
				JoinType::InnerJoin,
				Alias::new("authorization_execution"),
				Alias::new("e"),
				Expr::col((Alias::new("entries"), Alias::new("id")))
					.equals((Alias::new("e"), Alias::new("run_id"))),
			)
			.cond_where(Expr::cust(
				"jsonb_typeof(entry)='object' AND entry ? 'id' AND entry ? 'version' AND entry ? 'kind'",
			))
			.to_owned();
		manager
			.exec_stmt(
				Query::insert()
					.into_table(Alias::new("authorization_run_registry_reads"))
					.columns([
						Alias::new("run_id"),
						Alias::new("entry_id"),
						Alias::new("entry_version"),
					])
					.select_from(sources)
					.map_err(|error| DbErr::Custom(error.to_string()))?
					.on_conflict(OnConflict::new().do_nothing().to_owned())
					.to_owned(),
			)
			.await?;
		Ok(())
	}
	async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		// Refuse a rollback that would merge distinct remote memory namespaces.
		let remote = manager
			.get_connection()
			.query_one(sea_orm::Statement::from_string(
				sea_orm::DbBackend::Postgres,
				"SELECT 1 FROM memory WHERE home_node<>'' LIMIT 1".to_owned(),
			))
			.await?;
		if remote.is_some() {
			return Err(DbErr::Custom(
				"cannot discard remote memory namespaces".into(),
			));
		}
		manager
			.drop_table(
				Table::drop()
					.table(Alias::new("authorization_run_registry_reads"))
					.to_owned(),
			)
			.await?;
		rebuild_memory(manager, false).await
	}
}

async fn rebuild_memory(manager: &SchemaManager<'_>, namespaced: bool) -> Result<(), DbErr> {
	let mut table = Table::create();
	table
		.table(Alias::new("memory_next"))
		.col(ColumnDef::new(Alias::new("agent_id")).text().not_null())
		.col(
			ColumnDef::new(Alias::new("agent_version"))
				.text()
				.not_null(),
		)
		.col(ColumnDef::new(Alias::new("workspace_id")).uuid().not_null())
		.col(
			ColumnDef::new(Alias::new("data"))
				.json_binary()
				.not_null()
				.default(Expr::cust("'{}'")),
		);
	let mut primary = Index::create();
	primary
		.col(Alias::new("agent_id"))
		.col(Alias::new("agent_version"))
		.col(Alias::new("workspace_id"));
	if namespaced {
		table.col(
			ColumnDef::new(Alias::new("home_node"))
				.text()
				.not_null()
				.default(""),
		);
		primary.col(Alias::new("home_node"));
	}
	table.primary_key(&mut primary);
	manager.create_table(table.to_owned()).await?;
	let columns = ["agent_id", "agent_version", "workspace_id", "data"].map(Alias::new);
	manager
		.exec_stmt(
			Query::insert()
				.into_table(Alias::new("memory_next"))
				.columns(columns.clone())
				.select_from(
					Query::select()
						.columns(columns)
						.from(Alias::new("memory"))
						.to_owned(),
				)
				.map_err(|error| DbErr::Custom(error.to_string()))?
				.to_owned(),
		)
		.await?;
	manager
		.drop_table(Table::drop().table(Alias::new("memory")).to_owned())
		.await?;
	manager
		.rename_table(
			Table::rename()
				.table(Alias::new("memory_next"), Alias::new("memory"))
				.to_owned(),
		)
		.await
}
