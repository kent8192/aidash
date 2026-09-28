use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		for table in ["atomic_subjects", "atomic_preflights"] {
			manager
				.create_table(
					Table::create()
						.table(Alias::new(table))
						.col(ColumnDef::new(Alias::new("id")).uuid().primary_key())
						.col(
							ColumnDef::new(Alias::new("binding"))
								.json_binary()
								.not_null(),
						)
						.to_owned(),
				)
				.await?;
		}
		manager
			.create_table(
				Table::create()
					.table(Alias::new("atomic_authority_attempts"))
					.col(
						ColumnDef::new(Alias::new("transaction_id"))
							.uuid()
							.not_null(),
					)
					.col(ColumnDef::new(Alias::new("node_id")).text().not_null())
					.col(ColumnDef::new(Alias::new("outcome")).text())
					.primary_key(
						Index::create()
							.col(Alias::new("transaction_id"))
							.col(Alias::new("node_id")),
					)
					.to_owned(),
			)
			.await?;
		// SeaQuery cannot express PL/pgSQL trigger functions. Only authority
		// metadata and its audit may bypass a reservation, never application data.
		manager
			.get_connection()
			.execute_unprepared(CONTROL_GUARD)
			.await?;
		Ok(())
	}
	async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		let pending = Query::select()
			.expr_as(
				Expr::exists(
					Query::select()
						.expr(Expr::val(1))
						.from(Alias::new("atomic_coordinators"))
						.and_where(Expr::col(Alias::new("complete")).eq(false))
						.to_owned(),
				)
				.or(Expr::exists(
					Query::select()
						.expr(Expr::val(1))
						.from(Alias::new("atomic_participants"))
						.and_where(
							Expr::col(Alias::new("phase")).is_not_in(["COMMITTED", "ABORTED"]),
						)
						.to_owned(),
				)),
				Alias::new("pending"),
			)
			.to_owned();
		let row = manager
			.get_connection()
			.query_one(manager.get_database_backend().build(&pending))
			.await?
			.ok_or_else(|| DbErr::Custom("transaction state unavailable".into()))?;
		if row.try_get::<bool>("", "pending")? {
			return Err(DbErr::Custom(
				"cannot remove authority while transactions are pending".into(),
			));
		}
		manager
			.get_connection()
			.execute_unprepared(LEGACY_GUARD)
			.await?;
		for table in [
			"atomic_authority_attempts",
			"atomic_preflights",
			"atomic_subjects",
		] {
			manager
				.drop_table(Table::drop().table(Alias::new(table)).to_owned())
				.await?;
		}
		Ok(())
	}
}

const CONTROL_GUARD: &str = r#"
CREATE OR REPLACE FUNCTION atomic_write_guard() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE pending uuid;
BEGIN
    IF current_setting('aidash.transaction_control',true) = 'authority'
       AND TG_TABLE_NAME IN ('authorization_bundles','authorization_revisions',
           'authorization_credentials','authorization_decisions',
           'authorization_peer_mappings','authorization_peer_mapping_history','peers') THEN
        RETURN NULL;
    END IF;
    SELECT transaction_id INTO pending FROM atomic_gate WHERE singleton FOR SHARE NOWAIT;
    IF pending IS NOT NULL AND current_setting('aidash.atomic_transaction',true) IS DISTINCT FROM pending::text THEN
        RAISE EXCEPTION 'atomic transaction visibility pending' USING ERRCODE='55P03';
    END IF;
    RETURN NULL;
END;
$$;
"#;

const LEGACY_GUARD: &str = r#"
CREATE OR REPLACE FUNCTION atomic_write_guard() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE pending uuid;
BEGIN
    SELECT transaction_id INTO pending FROM atomic_gate WHERE singleton FOR SHARE NOWAIT;
    IF pending IS NOT NULL AND current_setting('aidash.atomic_transaction',true) IS DISTINCT FROM pending::text THEN
        RAISE EXCEPTION 'atomic transaction visibility pending' USING ERRCODE='55P03';
    END IF;
    RETURN NULL;
END;
$$;
"#;
