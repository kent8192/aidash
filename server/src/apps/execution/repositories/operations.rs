//! Operation records and fenced updates retain the original database expressions.
use crate::apps::execution::capabilities::services::sessions;
use crate::{Error, Result, authorization::access::Access};
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
	SimpleExpr,
};
use serde_json::Value;
use uuid::Uuid;
#[derive(Clone, sqlx::FromRow)]
pub(crate) struct Operation {
	pub id: Uuid,
	pub area_id: Uuid,
	pub run_id: Uuid,
	pub tenant: String,
	pub principal: String,
	pub credential_id: Uuid,
	pub subjects: Value,
	pub digest: String,
	pub kind: String,
	pub state: String,
	pub epoch: i64,
	pub generation: i64,
	pub revision: i64,
	pub policy_revision: i64,
	pub input: Value,
	pub result: Value,
	pub runner_instance: Option<String>,
}

pub(crate) async fn get(access: &mut Access, id: Uuid) -> Result<Operation> {
	{
		let query_bind_1 = id;
		let query_bind_2 = &access.identity.tenant;
		sqlx::query_as(
			&sessions::select("core_operations")
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						),
					),
				)
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("tenant"))).eq(
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						),
					),
				)
				.lock(LockType::Update)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&mut **access.tx)
		.await?
	}
	.ok_or_else(|| Error::NotFound("operation unavailable".into()))
}

pub(crate) async fn set_area(access: &mut Access, id: Uuid, state: &str, epoch: i64) -> Result<()> {
	{
		let query_bind_1 = id;
		let query_bind_2 = state;
		let query_bind_3 = epoch;
		sqlx::query(
			&Query::update()
				.table(Alias::new("core_areas"))
				.value_expr(
					Alias::new("state"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_2.to_owned()).into()],
					),
				)
				.value_expr(
					Alias::new("epoch"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_3.to_owned()).into()],
					),
				)
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						),
					),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **access.tx)
		.await?
	};
	Ok(())
}

pub(crate) async fn persist(access: &mut Access, operation: &Operation) -> Result<()> {
	{
		let query_bind_1 = operation.id;
		let query_bind_2 = &operation.state;
		let query_bind_3 = &operation.result;
		let query_bind_4 = &operation.runner_instance;
		let query_bind_5 = operation.revision;
		sqlx::query(
			&Query::update()
				.table(Alias::new("core_operations"))
				.value_expr(
					Alias::new("state"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_2.to_owned()).into()],
					),
				)
				.value_expr(
					Alias::new("result"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_3.to_owned()).into()],
					),
				)
				.value_expr(
					Alias::new("runner_instance"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_4.to_owned()).into()],
					),
				)
				.value_expr(
					Alias::new("revision"),
					SimpleExpr::CustomWithExpr(
						"(?)".to_owned(),
						vec![Expr::value(query_bind_5.to_owned()).into()],
					),
				)
				.value_expr(Alias::new("updated_at"), Expr::current_timestamp())
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
						SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						),
					),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **access.tx)
		.await?
	};
	Ok(())
}
pub(crate) mod reconciliation;

mod processing;
