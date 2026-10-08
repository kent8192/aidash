//! Registry reads and immutable inserts on a caller-owned SQLx transaction.
use crate::Error;
use aidash_application::{
	Result,
	ports::registry::{DefinitionLookup, DefinitionWriter},
};
use aidash_domain::registry::Entry;
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Expr, ExprTrait, OnConflict, PostgresQueryBuilder, Query, QueryStatementBuilder,
	SimpleExpr,
};
use serde_json::Value;
pub(crate) struct SqlScope<'a>(pub(crate) &'a mut crate::database::native::Transaction);
#[async_trait]
impl DefinitionLookup for SqlScope<'_> {
	async fn foreign_agent(
		&mut self,
		node: &str,
		reference: &aidash_domain::registry::bindings::QualifiedRef,
	) -> Result<aidash_domain::registry::bindings::ForeignAgentSnapshot> {
		super::foreign::native(&mut **self.0, node, reference)
			.await
			.map_err(Into::into)
	}
	async fn definition(&mut self, id: &str, version: &str) -> Result<Entry> {
		let query = Query::select()
			.column(Alias::new("metadata"))
			.from(Alias::new("registry"))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.and_where(Expr::col("version").eq(Expr::value(version)))
			.to_string(PostgresQueryBuilder);
		let value: Option<Value> = crate::database::native::query_scalar(&query)
			.scalar_optional(&mut **self.0)
			.await?;
		Ok(serde_json::from_value(
			value.ok_or_else(|| Error::NotFound(id.into()))?,
		)?)
	}
	async fn binding_installation(
		&mut self,
		projection: &aidash_domain::registry::Projection,
	) -> Result<()> {
		super::bindings::installation_native(self.0, projection)
			.await
			.map_err(Into::into)
	}

	async fn overrides(&mut self, id: &str, version: &str) -> Result<Option<Value>> {
		let query = Query::select()
			.column(Alias::new("config"))
			.from(Alias::new("installations"))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.and_where(Expr::col("version").eq(Expr::value(version)))
			.to_string(PostgresQueryBuilder);
		Ok(crate::database::native::query_scalar(&query)
			.scalar_optional(&mut **self.0)
			.await?)
	}
	async fn executor_kind(&mut self, id: &str, version: &str) -> Result<Option<String>> {
		let query = Query::select()
			.column(Alias::new("kind"))
			.from(Alias::new("registry"))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.and_where(Expr::col("version").eq(Expr::value(version)))
			.to_string(PostgresQueryBuilder);
		Ok(crate::database::native::query_scalar(&query)
			.scalar_optional(&mut **self.0)
			.await?)
	}
}
#[async_trait]
impl DefinitionWriter for SqlScope<'_> {
	async fn insert_definition(&mut self, entry: &Entry) -> Result<bool> {
		insert(self.0, entry).await.map_err(Into::into)
	}
}
async fn insert(
	tx: &mut crate::database::native::Transaction,
	entry: &Entry,
) -> crate::Result<bool> {
	let value = serde_json::to_value(entry)?;
	let inserted = {
		let query_bind_1 = &entry.id;
		let query_bind_2 = &entry.version;
		let query_bind_3 = &entry.kind;
		let query_bind_4 = &value;
		crate::database::native::query(
			&Query::insert()
				.into_table(Alias::new("registry"))
				.columns([
					Alias::new("id"),
					Alias::new("version"),
					Alias::new("kind"),
					Alias::new("metadata"),
				])
				.from_subquery(
					Query::select()
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_2.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_3.to_owned()).into()],
						))
						.expr(SimpleExpr::CustomWithExpr(
							"(?)".to_owned(),
							vec![Expr::value(query_bind_4.to_owned()).into()],
						))
						.to_owned(),
				)
				.on_conflict(
					OnConflict::columns(["id", "version"])
						.do_nothing()
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **tx)
		.await?
	}
	.rows_affected()
		!= 0;
	let stored: Value = {
		let query_bind_1 = &entry.id;
		let query_bind_2 = &entry.version;
		crate::database::native::query_scalar(
			&Query::select()
				.expr(reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
					"metadata",
				))))
				.from(Alias::new("registry"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(id = ? AND version = ?)".to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
					],
				))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_one(&mut **tx)
		.await?
	};
	if stored != value {
		return Err(Error::Conflict(
			"published versions are immutable; choose a new version".into(),
		));
	}
	Ok(inserted)
}
