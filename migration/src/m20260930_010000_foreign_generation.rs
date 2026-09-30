use sea_orm_migration::prelude::*;
#[derive(DeriveMigrationName)]
pub struct Migration;
fn a(s: &str) -> Alias {
	Alias::new(s)
}
#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, m: &SchemaManager) -> Result<(), DbErr> {
		m.alter_table(
			Table::alter()
				.table(a("generation_requests"))
				.add_column(ColumnDef::new(a("home_node")).text().not_null().default(""))
				.add_column(ColumnDef::new(a("foreign_intent")).json_binary())
				.add_column(
					ColumnDef::new(a("prepared"))
						.boolean()
						.not_null()
						.default(false),
				)
				.add_column(ColumnDef::new(a("grant_id")).uuid())
				.add_column(ColumnDef::new(a("admission_id")).uuid())
				.drop_foreign_key(a("generation_requests_task_id_fkey"))
				.drop_foreign_key(a("generation_requests_task_workspace"))
				.drop_foreign_key(a("generation_requests_workspace_id_fkey"))
				.to_owned(),
		)
		.await?;
		// The conditional keys retain all existing local foreign-key guarantees;
		// an explicitly bound foreign Task does not create a local Task/Workspace.
		m.alter_table(
			Table::alter()
				.table(a("generation_requests"))
				.add_column(ColumnDef::new(a("local_task_id")).uuid().generated(
					Expr::cust("CASE WHEN home_node='' THEN task_id ELSE NULL END"),
					true,
				))
				.add_column(ColumnDef::new(a("local_workspace_id")).uuid().generated(
					Expr::cust("CASE WHEN home_node='' THEN workspace_id ELSE NULL END"),
					true,
				))
				.to_owned(),
		)
		.await?;
		for (column, table, target) in [
			("local_task_id", "tasks", "id"),
			(
				"local_workspace_id",
				"authorization_workspaces",
				"workspace_id",
			),
		] {
			m.create_foreign_key(
				ForeignKey::create()
					.name(format!("generation_requests_{column}_fkey"))
					.from(a("generation_requests"), a(column))
					.to(a(table), a(target))
					.to_owned(),
			)
			.await?;
		}
		m.create_foreign_key(
			ForeignKey::create()
				.name("generation_requests_task_workspace")
				.from_tbl(a("generation_requests"))
				.from_col(a("local_task_id"))
				.from_col(a("local_workspace_id"))
				.to_tbl(a("tasks"))
				.to_col(a("id"))
				.to_col(a("workspace_id"))
				.to_owned(),
		)
		.await?;
		// SeaQuery has no ALTER DROP UNIQUE CONSTRAINT / ADD CHECK operation.
		m.get_connection().execute_unprepared("ALTER TABLE generation_requests DROP CONSTRAINT generation_requests_task_id_key, ADD CONSTRAINT generation_foreign_binding CHECK ((home_node='' AND foreign_intent IS NULL AND grant_id IS NULL AND admission_id IS NULL) OR (home_node<>'' AND foreign_intent IS NOT NULL))").await?;
		// SeaQuery cannot express a partial unique index. Terminal history must
		// remain durable while a fresh intent may bind the same foreign Task.
		m.get_connection().execute_unprepared("CREATE UNIQUE INDEX generation_requests_local_task ON generation_requests (task_id) WHERE home_node=''; CREATE UNIQUE INDEX generation_requests_home_task ON generation_requests (home_node, task_id) WHERE home_node<>'' AND status IN ('PENDING_APPROVAL','QUEUED','ACTIVE')").await?;
		m.create_table(
			Table::create()
				.table(a("generation_remote_intents"))
				.col(ColumnDef::new(a("id")).uuid().not_null().primary_key())
				.col(ColumnDef::new(a("task_id")).uuid().not_null())
				.col(ColumnDef::new(a("tenant")).text().not_null())
				.col(ColumnDef::new(a("credential_id")).uuid().not_null())
				.col(ColumnDef::new(a("root_subject")).text().not_null())
				.col(
					ColumnDef::new(a("subject_chain"))
						.array(ColumnType::Text)
						.not_null(),
				)
				.col(ColumnDef::new(a("binding")).json_binary().not_null())
				.col(
					ColumnDef::new(a("cancelled"))
						.boolean()
						.not_null()
						.default(false),
				)
				.col(
					ColumnDef::new(a("cancel_delivered"))
						.boolean()
						.not_null()
						.default(false),
				)
				.col(ColumnDef::new(a("cancel_retry_at")).timestamp_with_time_zone())
				.col(
					ColumnDef::new(a("created_at"))
						.timestamp_with_time_zone()
						.not_null()
						.default(Expr::current_timestamp()),
				)
				.foreign_key(
					ForeignKey::create()
						.from(a("generation_remote_intents"), a("task_id"))
						.to(a("tasks"), a("id")),
				)
				.foreign_key(
					ForeignKey::create()
						.from(a("generation_remote_intents"), a("credential_id"))
						.to(a("authorization_credentials"), a("id")),
				)
				.to_owned(),
		)
		.await?;
		m.create_index(
			Index::create()
				.name("generation_remote_intents_cancel_retry")
				.table(a("generation_remote_intents"))
				.col(a("cancelled"))
				.col(a("cancel_delivered"))
				.col(a("cancel_retry_at"))
				.to_owned(),
		)
		.await?;
		// PostgreSQL statement triggers are not represented by SeaQuery.
		m.get_connection().execute_unprepared("CREATE TRIGGER atomic_write_guard BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON generation_remote_intents FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard()").await?;
		Ok(())
	}
	async fn down(&self, m: &SchemaManager) -> Result<(), DbErr> {
		let query = Query::select()
			.expr(Expr::cust("COUNT(*) AS count"))
			.from(a("generation_requests"))
			.and_where(Expr::col(a("home_node")).ne(""))
			.to_owned();
		let row = m
			.get_connection()
			.query_one(m.get_database_backend().build(&query))
			.await?
			.ok_or_else(|| DbErr::Custom("missing generation count".into()))?;
		if row.try_get::<i64>("", "count")? > 0 {
			return Err(DbErr::Custom(
				"foreign generation records must be retained; cannot roll back this migration"
					.into(),
			));
		}
		m.drop_table(
			Table::drop()
				.table(a("generation_remote_intents"))
				.to_owned(),
		)
		.await?;
		m.drop_index(
			Index::drop()
				.name("generation_requests_home_task")
				.table(a("generation_requests"))
				.to_owned(),
		)
		.await?;
		m.drop_index(
			Index::drop()
				.name("generation_requests_local_task")
				.table(a("generation_requests"))
				.to_owned(),
		)
		.await?;
		m.alter_table(
			Table::alter()
				.table(a("generation_requests"))
				.drop_foreign_key(a("generation_requests_task_workspace"))
				.drop_foreign_key(a("generation_requests_local_task_id_fkey"))
				.drop_foreign_key(a("generation_requests_local_workspace_id_fkey"))
				.drop_column(a("local_task_id"))
				.drop_column(a("local_workspace_id"))
				.to_owned(),
		)
		.await?;
		// Restoring the original named table constraints is unsupported by SeaQuery.
		m.get_connection().execute_unprepared("ALTER TABLE generation_requests DROP CONSTRAINT generation_foreign_binding, ADD CONSTRAINT generation_requests_task_id_key UNIQUE (task_id)").await?;
		for (column, table, target) in [
			("task_id", "tasks", "id"),
			("workspace_id", "authorization_workspaces", "workspace_id"),
		] {
			m.create_foreign_key(
				ForeignKey::create()
					.name(format!("generation_requests_{column}_fkey"))
					.from(a("generation_requests"), a(column))
					.to(a(table), a(target))
					.to_owned(),
			)
			.await?;
		}
		m.create_foreign_key(
			ForeignKey::create()
				.name("generation_requests_task_workspace")
				.from_tbl(a("generation_requests"))
				.from_col(a("task_id"))
				.from_col(a("workspace_id"))
				.to_tbl(a("tasks"))
				.to_col(a("id"))
				.to_col(a("workspace_id"))
				.to_owned(),
		)
		.await?;
		let mut alter = Table::alter();
		alter.table(a("generation_requests"));
		for column in [
			"home_node",
			"foreign_intent",
			"prepared",
			"grant_id",
			"admission_id",
		] {
			alter.drop_column(a(column));
		}
		m.alter_table(alter.to_owned()).await?;
		Ok(())
	}
}
