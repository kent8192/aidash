use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.create_index(
				Index::create()
					.name("channel_message_scope_key")
					.table(Alias::new("messages"))
					.col(Alias::new("workspace_id"))
					.col(Alias::new("id"))
					.unique()
					.to_owned(),
			)
			.await?;
		manager
			.create_index(
				Index::create()
					.name("channel_message_history_order")
					.table(Alias::new("messages"))
					.col(Alias::new("workspace_id"))
					.col(Alias::new("created_at"))
					.col(Alias::new("id"))
					.to_owned(),
			)
			.await?;
		manager
			.create_table(
				Table::create()
					.table(Alias::new("channel_threads"))
					.col(ColumnDef::new(Alias::new("id")).uuid().primary_key())
					.col(ColumnDef::new(Alias::new("workspace_id")).uuid().not_null())
					.col(
						ColumnDef::new(Alias::new("root_message_id"))
							.uuid()
							.not_null()
							.unique_key(),
					)
					.col(ColumnDef::new(Alias::new("created_by")).text().not_null())
					.col(
						ColumnDef::new(Alias::new("created_at"))
							.timestamp_with_time_zone()
							.not_null()
							.default(Expr::current_timestamp()),
					)
					.index(
						Index::create()
							.name("channel_thread_scope_key")
							.unique()
							.col(Alias::new("workspace_id"))
							.col(Alias::new("id")),
					)
					.foreign_key(
						ForeignKey::create()
							.from_tbl(Alias::new("channel_threads"))
							.from_col(Alias::new("workspace_id"))
							.from_col(Alias::new("root_message_id"))
							.to_tbl(Alias::new("messages"))
							.to_col(Alias::new("workspace_id"))
							.to_col(Alias::new("id"))
							.on_delete(ForeignKeyAction::Cascade),
					)
					.to_owned(),
			)
			.await?;
		manager
			.create_table(
				Table::create()
					.table(Alias::new("channel_message_context"))
					.col(
						ColumnDef::new(Alias::new("message_id"))
							.uuid()
							.primary_key(),
					)
					.col(ColumnDef::new(Alias::new("workspace_id")).uuid().not_null())
					.col(ColumnDef::new(Alias::new("thread_id")).uuid())
					.foreign_key(
						ForeignKey::create()
							.from_tbl(Alias::new("channel_message_context"))
							.from_col(Alias::new("workspace_id"))
							.from_col(Alias::new("message_id"))
							.to_tbl(Alias::new("messages"))
							.to_col(Alias::new("workspace_id"))
							.to_col(Alias::new("id"))
							.on_delete(ForeignKeyAction::Cascade),
					)
					.foreign_key(
						ForeignKey::create()
							.from_tbl(Alias::new("channel_message_context"))
							.from_col(Alias::new("workspace_id"))
							.from_col(Alias::new("thread_id"))
							.to_tbl(Alias::new("channel_threads"))
							.to_col(Alias::new("workspace_id"))
							.to_col(Alias::new("id")),
					)
					.to_owned(),
			)
			.await?;
		manager
			.create_index(
				Index::create()
					.name("channel_reply_lookup")
					.table(Alias::new("channel_message_context"))
					.col(Alias::new("workspace_id"))
					.col(Alias::new("thread_id"))
					.col(Alias::new("message_id"))
					.to_owned(),
			)
			.await?;
		// PostgreSQL trigger DDL has no SeaQuery builder. Both tables obey the
		// established cross-node atomic-transaction write barrier.
		for table in ["channel_threads", "channel_message_context"] {
			manager.get_connection().execute_unprepared(&format!(
				"CREATE TRIGGER atomic_write_guard BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON {table} FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard()"
			)).await?;
		}
		Ok(())
	}

	async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		for table in ["channel_message_context", "channel_threads"] {
			manager
				.drop_table(Table::drop().table(Alias::new(table)).to_owned())
				.await?;
		}
		for index in ["channel_message_history_order", "channel_message_scope_key"] {
			manager
				.drop_index(
					Index::drop()
						.name(index)
						.table(Alias::new("messages"))
						.to_owned(),
				)
				.await?;
		}
		Ok(())
	}
}
