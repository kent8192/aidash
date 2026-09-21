// SeaQuery 0.32 cannot express PostgreSQL trigger functions, triggers, or ALTER CHECK constraints.
// Those DDL operations intentionally use SeaORM execution; ordinary queries use builders.
use sea_orm_migration::prelude::*;
#[derive(DeriveMigrationName)]
pub struct Migration;
fn col(name: &str) -> ColumnDef {
	ColumnDef::new(Alias::new(name))
}
#[async_trait::async_trait]
impl MigrationTrait for Migration {
	async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.create_table(
				Table::create()
					.table(Alias::new("authorization_run_remote_reads"))
					.col(col("run_id").uuid().not_null())
					.col(col("node_id").text().not_null())
					.col(col("entry_id").text().not_null())
					.col(col("entry_version").text().not_null())
					.col(col("digest").text().not_null())
					.col(col("metadata").json_binary().not_null())
					.primary_key(
						Index::create()
							.col(Alias::new("run_id"))
							.col(Alias::new("node_id"))
							.col(Alias::new("entry_id"))
							.col(Alias::new("entry_version"))
							.col(Alias::new("digest")),
					)
					.foreign_key(
						ForeignKey::create()
							.from(
								Alias::new("authorization_run_remote_reads"),
								Alias::new("run_id"),
							)
							.to(Alias::new("runs"), Alias::new("id"))
							.on_delete(ForeignKeyAction::Cascade),
					)
					.to_owned(),
			)
			.await?;
		manager.get_connection().execute_unprepared("CREATE TRIGGER atomic_write_guard BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON authorization_run_remote_reads FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard()").await?;
		manager
			.create_table(
				Table::create()
					.table(Alias::new("authorization_run_outputs"))
					.col(col("run_id").uuid().not_null())
					.col(col("workspace_id").uuid().not_null())
					.col(col("resource_kind").text().not_null())
					.col(col("resource_id").uuid().not_null())
					.primary_key(
						Index::create()
							.col(Alias::new("run_id"))
							.col(Alias::new("resource_kind"))
							.col(Alias::new("resource_id")),
					)
					.foreign_key(
						ForeignKey::create()
							.from(
								Alias::new("authorization_run_outputs"),
								Alias::new("run_id"),
							)
							.to(Alias::new("runs"), Alias::new("id"))
							.on_delete(ForeignKeyAction::Cascade),
					)
					.to_owned(),
			)
			.await?;
		manager
			.create_index(
				Index::create()
					.name("authorization_output_resource")
					.table(Alias::new("authorization_run_outputs"))
					.col(Alias::new("workspace_id"))
					.col(Alias::new("resource_kind"))
					.col(Alias::new("resource_id"))
					.col(Alias::new("run_id"))
					.to_owned(),
			)
			.await?;
		// Older releases recorded outputs among reads. Recover authored output
		// membership, retaining every possible producer when attribution overlaps.
		manager.exec_stmt(sea_orm::sea_query::Query::insert().into_table(sea_orm::sea_query::Alias::new("authorization_run_outputs")).columns(["run_id", "workspace_id", "resource_kind", "resource_id"].map(sea_orm::sea_query::Alias::new)).select_from(sea_orm::sea_query::Query::select().expr(sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col((sea_orm::sea_query::Alias::new("rr"), sea_orm::sea_query::Alias::new("run_id"))))).expr(sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col((sea_orm::sea_query::Alias::new("rr"), sea_orm::sea_query::Alias::new("workspace_id"))))).expr(sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col((sea_orm::sea_query::Alias::new("rr"), sea_orm::sea_query::Alias::new("resource_kind"))))).expr(sea_orm::sea_query::SimpleExpr::from(sea_orm::sea_query::Expr::col((sea_orm::sea_query::Alias::new("rr"), sea_orm::sea_query::Alias::new("resource_id"))))).from_as(sea_orm::sea_query::Alias::new("authorization_run_reads"), sea_orm::sea_query::Alias::new("rr")).join_as(sea_orm::sea_query::JoinType::InnerJoin, sea_orm::sea_query::Alias::new("runs"), sea_orm::sea_query::Alias::new("r"), sea_orm::sea_query::Expr::cust("r.id = rr.run_id")).and_where(sea_orm::sea_query::Expr::cust("r.workspace_id = rr.workspace_id AND ((rr.resource_kind = 'artifact' AND EXISTS(SELECT 1 FROM artifacts AS a WHERE a.id = rr.resource_id AND a.workspace_id = rr.workspace_id AND a.created_by = r.home_node || '/agents/' || r.agent_id || '@' || r.agent_version)) OR (rr.resource_kind = 'message' AND EXISTS(SELECT 1 FROM messages AS m WHERE m.id = rr.resource_id AND m.workspace_id = rr.workspace_id AND m.sender = r.home_node || '/agents/' || r.agent_id || '@' || r.agent_version)))")).to_owned()).expect("valid insert projection").on_conflict(sea_orm::sea_query::OnConflict::new().do_nothing().to_owned()).to_owned()).await?;
		manager.get_connection().execute_unprepared("CREATE TRIGGER atomic_write_guard BEFORE INSERT OR UPDATE OR DELETE OR TRUNCATE ON authorization_run_outputs FOR EACH STATEMENT EXECUTE FUNCTION atomic_write_guard()").await?;
		Ok(())
	}
	async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
		manager
			.drop_table(
				Table::drop()
					.table(Alias::new("authorization_run_outputs"))
					.to_owned(),
			)
			.await?;
		manager
			.drop_table(
				Table::drop()
					.table(Alias::new("authorization_run_remote_reads"))
					.to_owned(),
			)
			.await
	}
}
