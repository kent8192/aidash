//! Tenant model selection for the Goal judge.
use crate::{Error, Result, database::native};
use aidash_domain::registry::{EntityRef, Entry};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use serde_json::Value;

pub(crate) async fn read(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
) -> Result<Option<EntityRef>> {
	let value: Option<Value> = native::query_scalar(
		&Query::select()
			.column(Alias::new("goal_judge_model"))
			.from(Alias::new("tenants"))
			.and_where(Expr::col("name").eq(tenant))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(tx)
	.await?;
	value
		.map(serde_json::from_value)
		.transpose()
		.map_err(Into::into)
}

pub(crate) async fn registered(
	tx: &mut dyn TransactionExecutor,
	model: &EntityRef,
) -> Result<Option<Entry>> {
	match crate::apps::registry::models::Definition::read_in(tx, &model.id, &model.version).await {
		Ok(entry) if entry.kind == "model" => Ok(Some(entry)),
		Ok(_) | Err(Error::NotFound(_)) => Ok(None),
		Err(error) => Err(error),
	}
}

pub(crate) async fn set(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	model: Option<&EntityRef>,
) -> Result<()> {
	if let Some(model) = model
		&& registered(&mut *tx, model).await?.is_none()
	{
		return Err(Error::Invalid(
			"Goal judge must name a registered model".into(),
		));
	}
	let value = model.map(serde_json::to_value).transpose()?;
	native::query(
		&Query::update()
			.table(Alias::new("tenants"))
			.value_expr(Alias::new("goal_judge_model"), Expr::value(value))
			.and_where(Expr::col("name").eq(tenant))
			.to_string(PostgresQueryBuilder),
	)
	.execute(tx)
	.await?;
	Ok(())
}
