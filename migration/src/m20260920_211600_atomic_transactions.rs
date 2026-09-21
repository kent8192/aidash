// SeaQuery 0.32 cannot express PostgreSQL trigger functions, triggers, or ALTER CHECK constraints.
// Those DDL operations intentionally use SeaORM execution; ordinary queries use builders.
use sea_orm_migration::prelude::*;
#[derive(DeriveMigrationName)]
pub struct Migration;
fn col(name: &str) -> ColumnDef {
	ColumnDef::new(Alias::new(name))
}
fn now(name: &str) -> ColumnDef {
	col(name)
		.timestamp_with_time_zone()
		.not_null()
		.default(Expr::current_timestamp())
		.to_owned()
}
fn phases() -> SimpleExpr {
	Expr::col(Alias::new("phase")).is_in([
		"RESERVED",
		"PREPARED",
		"APPLIED",
		"COMMITTED",
		"ABORTED",
	])
}
#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.create_table(
				Table::create()
					.table(Alias::new("atomic_coordinators"))
					.col(col("id").uuid().not_null().primary_key())
					.col(col("digest").text().not_null())
					.col(col("manifest").json_binary().not_null())
					.col(
						col("decision")
							.text()
							.check(Expr::col(Alias::new("decision")).is_in(["COMMIT", "ABORT"])),
					)
					.col(col("visible").boolean().not_null().default(false))
					.col(col("complete").boolean().not_null().default(false))
					.col(col("last_error").text())
					.col(now("created_at"))
					.col(now("updated_at"))
					.check(Expr::cust(
						"NOT visible OR (decision IS NOT NULL AND decision='COMMIT')",
					))
					.check(Expr::cust("NOT complete OR decision IS NOT NULL"))
					.to_owned(),
			)
			.await?;
		manager
			.create_table(
				Table::create()
					.table(Alias::new("atomic_votes"))
					.col(col("transaction_id").uuid().not_null())
					.col(col("node_id").text().not_null())
					.col(col("phase").text().not_null().default("PENDING").check(
						Expr::col(Alias::new("phase")).is_in([
							"PENDING",
							"RESERVED",
							"PREPARED",
							"APPLIED",
							"COMMITTED",
							"ABORTED",
						]),
					))
					.primary_key(
						Index::create()
							.col(Alias::new("transaction_id"))
							.col(Alias::new("node_id")),
					)
					.foreign_key(
						ForeignKey::create()
							.from(Alias::new("atomic_votes"), Alias::new("transaction_id"))
							.to(Alias::new("atomic_coordinators"), Alias::new("id")),
					)
					.to_owned(),
			)
			.await?;
		manager
			.create_table(
				Table::create()
					.table(Alias::new("atomic_participants"))
					.col(col("id").uuid().not_null().primary_key())
					.col(col("coordinator").text().not_null())
					.col(col("digest").text().not_null())
					.col(col("manifest").json_binary().not_null())
					.col(col("phase").text().not_null().check(phases()))
					.col(now("updated_at"))
					.to_owned(),
			)
			.await?;
		manager
			.create_table(
				Table::create()
					.table(Alias::new("atomic_gate"))
					.col(
						col("singleton")
							.boolean()
							.not_null()
							.primary_key()
							.default(true)
							.check(Expr::col(Alias::new("singleton")).eq(true)),
					)
					.col(col("transaction_id").uuid())
					.foreign_key(
						ForeignKey::create()
							.from(Alias::new("atomic_gate"), Alias::new("transaction_id"))
							.to(Alias::new("atomic_participants"), Alias::new("id")),
					)
					.to_owned(),
			)
			.await?;
		manager
			.exec_stmt(
				Query::insert()
					.into_table(Alias::new("atomic_gate"))
					.columns([Alias::new("singleton")])
					.values_panic([Expr::val(true).into()])
					.to_owned(),
			)
			.await?;
		manager
			.create_table(
				Table::create()
					.table(Alias::new("atomic_peer_trust"))
					.col(col("node_id").text().not_null().primary_key())
					.col(col("enabled").boolean().not_null())
					.col(now("updated_at"))
					.foreign_key(
						ForeignKey::create()
							.from(Alias::new("atomic_peer_trust"), Alias::new("node_id"))
							.to(Alias::new("peers"), Alias::new("node_id")),
					)
					.to_owned(),
			)
			.await?;
		manager
			.create_table(
				Table::create()
					.table(Alias::new("atomic_history"))
					.col(
						col("sequence")
							.big_integer()
							.not_null()
							.auto_increment()
							.primary_key(),
					)
					.col(col("transaction_id").uuid().not_null())
					.col(col("role").text().not_null().check(
						Expr::col(Alias::new("role")).is_in([
							"coordinator",
							"participant",
							"trust",
						]),
					))
					.col(col("phase").text().not_null())
					.col(col("detail").text().not_null())
					.col(now("created_at"))
					.to_owned(),
			)
			.await?;
		// PostgreSQL trigger functions have no portable SeaQuery equivalent.
		manager.get_connection().execute_unprepared(r#"CREATE FUNCTION atomic_immutable_decision() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.id<>OLD.id OR NEW.digest<>OLD.digest OR NEW.manifest<>OLD.manifest
       OR (OLD.decision IS NOT NULL AND NEW.decision IS DISTINCT FROM OLD.decision)
       OR (OLD.visible AND NOT NEW.visible) OR (OLD.complete AND NOT NEW.complete) THEN
        RAISE EXCEPTION 'atomic transaction identity and decisions are immutable';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER atomic_immutable_decision BEFORE UPDATE ON atomic_coordinators
FOR EACH ROW EXECUTE FUNCTION atomic_immutable_decision();

-- Statement guards also cover empty-table writes. Read visibility is guarded
-- at every runtime entry boundary, retaining a shared lease through its work.
CREATE FUNCTION atomic_write_guard() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE pending uuid;
BEGIN
    SELECT transaction_id INTO pending FROM atomic_gate WHERE singleton FOR SHARE NOWAIT;
    IF pending IS NOT NULL AND current_setting('aidash.atomic_transaction',true) IS DISTINCT FROM pending::text THEN
        RAISE EXCEPTION 'atomic transaction visibility pending' USING ERRCODE='55P03';
    END IF;
    RETURN NULL;
END;
$$;
DO $$
DECLARE relation text;
BEGIN
    FOR relation IN SELECT tablename FROM pg_tables WHERE schemaname=current_schema()
        AND tablename NOT LIKE 'atomic_%' AND tablename <> 'seaql_migrations'
    LOOP
        EXECUTE format('CREATE TRIGGER atomic_write_guard BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON %I FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard()',relation);
    END LOOP;
END;
$$;
"#).await?;
		Ok(())
	}
	async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		let coordinators = Query::select()
			.expr(Expr::val(1))
			.from(Alias::new("atomic_coordinators"))
			.and_where(Expr::col(Alias::new("complete")).eq(false))
			.to_owned();
		let participants = Query::select()
			.expr(Expr::val(1))
			.from(Alias::new("atomic_participants"))
			.and_where(Expr::col(Alias::new("phase")).is_not_in(["COMMITTED", "ABORTED"]))
			.to_owned();
		let query = Query::select()
			.expr_as(
				Expr::exists(coordinators).or(Expr::exists(participants)),
				Alias::new("pending"),
			)
			.to_owned();
		let pending = manager
			.get_connection()
			.query_one(manager.get_database_backend().build(&query))
			.await?
			.ok_or_else(|| DbErr::Custom("atomic state unavailable".into()))?;
		if pending.try_get::<bool>("", "pending")? {
			return Err(DbErr::Custom(
				"resolve atomic transactions before downgrading".into(),
			));
		}
		manager
			.get_connection()
			.execute_unprepared("DROP FUNCTION atomic_write_guard() CASCADE")
			.await?;
		for table in [
			"atomic_history",
			"atomic_peer_trust",
			"atomic_gate",
			"atomic_participants",
			"atomic_votes",
			"atomic_coordinators",
		] {
			manager
				.drop_table(Table::drop().table(Alias::new(table)).to_owned())
				.await?;
		}
		manager
			.get_connection()
			.execute_unprepared("DROP FUNCTION atomic_immutable_decision()")
			.await?;
		Ok(())
	}
}
