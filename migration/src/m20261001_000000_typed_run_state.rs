use sea_orm_migration::prelude::*;
#[derive(DeriveMigrationName)]
pub struct Migration;

async fn replace_human_request_reference(
	manager: &SchemaManager<'_>,
	expression: &str,
) -> Result<(), DbErr> {
	manager
		.drop_foreign_key(
			ForeignKey::drop()
				.table(Alias::new("runs"))
				.name("runs_human_request_ref")
				.to_owned(),
		)
		.await?;
	manager
		.alter_table(
			Table::alter()
				.table(Alias::new("runs"))
				.drop_column(Alias::new("pending_human_request_id"))
				.to_owned(),
		)
		.await?;
	manager
		.alter_table(
			Table::alter()
				.table(Alias::new("runs"))
				.add_column(
					ColumnDef::new(Alias::new("pending_human_request_id"))
						.uuid()
						.generated(Expr::cust(expression), true),
				)
				.to_owned(),
		)
		.await?;
	manager
		.create_foreign_key(
			ForeignKey::create()
				.name("runs_human_request_ref")
				.from_tbl(Alias::new("runs"))
				.from_col(Alias::new("pending_human_request_id"))
				.from_col(Alias::new("id"))
				.to_tbl(Alias::new("human_requests"))
				.to_col(Alias::new("id"))
				.to_col(Alias::new("run_id"))
				.on_delete(ForeignKeyAction::Restrict)
				.on_update(ForeignKeyAction::Restrict)
				.to_owned(),
		)
		.await
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.alter_table(
				Table::alter()
					.table(Alias::new("runs"))
					.modify_column(ColumnDef::new(Alias::new("pending")).default(Expr::cust(
						r#"'{"state_version":1,"data":{},"recovery":{"retry":null,"lease_recovered":false}}'::jsonb"#,
					)))
					.modify_column(ColumnDef::new(Alias::new("context")).default(Expr::cust(
						r#"'{"summary":"","run_message_summary":"","run_message_summary_seq":0,"media_inferred_seq":0,"history":[],"usage":null,"compactions":0,"message_read_coverage":{},"message_inference_coverage":{}}'::jsonb"#,
					)))
					.to_owned(),
			)
			.await?;
		replace_human_request_reference(
			manager,
			r#"CASE WHEN pending->'data'->>'reason' IN ('human','external_approval','reconciliation')
                AND jsonb_typeof(pending->'data'->'request_id') = 'string'
                AND pending->'data'->>'request_id' ~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
                THEN (pending->'data'->>'request_id')::uuid END"#,
		)
		.await?;
		// PostgreSQL CHECK alteration and procedural function/trigger DDL
		// cannot be expressed by SeaQuery.
		manager
			.get_connection()
			.execute_unprepared(include_str!("m20261001_000000_typed_run_state.sql"))
			.await?;
		Ok(())
	}
	async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.alter_table(
				Table::alter()
					.table(Alias::new("runs"))
					.modify_column(
						ColumnDef::new(Alias::new("pending")).default(Expr::cust("'{}'::jsonb")),
					)
					.modify_column(
						ColumnDef::new(Alias::new("context")).default(Expr::cust("'{}'::jsonb")),
					)
					.to_owned(),
			)
			.await?;
		replace_human_request_reference(
			manager,
			r#"CASE WHEN jsonb_typeof(pending->'human_request_id') = 'string'
                AND pending->>'human_request_id' ~* '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
                THEN (pending->>'human_request_id')::uuid END"#,
		)
		.await?;
		// Rollback restores the previous guards, without converting executable data.
		// PostgreSQL CHECK alteration cannot be expressed by SeaQuery.
		let (_, _, check) = crate::m20260921_071045_record_constraints::workbench_checks()
			.into_iter()
			.find(|(_, name, _)| *name == "runs_counters")
			.ok_or_else(|| DbErr::Custom("missing Run constraint".into()))?;
		manager
			.get_connection()
			.execute_unprepared(&format!(
				"ALTER TABLE runs DROP CONSTRAINT runs_counters; ALTER TABLE runs ADD CONSTRAINT runs_counters CHECK ({check});",
			))
			.await?;
		manager
			.get_connection()
			.execute_unprepared(include_str!("m20261001_000000_typed_run_state_down.sql"))
			.await?;
		// Restore previous activation function.
		manager
			.get_connection()
			.execute_unprepared(include_str!(
				"m20260929_010000_activation_trigger_lookups.sql"
			))
			.await?;
		Ok(())
	}
}
