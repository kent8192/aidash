//! pgvector executes inside the application's existing PostgreSQL pool.
//! Physical candidates remain subordinate to the portable source/authority checks.
use crate::{
	Error, Result,
	database::native::{self, Pool},
};
use aidash_application::ports::{EmbeddingProvider, VectorIndex};
use aidash_domain::semantic::{Embedding, EmbeddingConfig, Point, VectorConfig, VectorFilter};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Expr, ExprTrait, LockType, OnConflict, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SimpleExpr,
};
use serde_json::Value;
use uuid::Uuid;

#[derive(Clone)]
pub struct Transport {
	pub(crate) pool: Pool,
	pub(crate) embedding: aidash_integrations::semantic::SemanticClient,
}
fn provider(config: &VectorConfig) -> Result<()> {
	if config.provider != "postgres"
		|| config.endpoint != "local"
		|| config.credential_env.is_some()
	{
		return Err(Error::Invalid(
			"vector search requires the local PostgreSQL provider".into(),
		));
	}
	Ok(())
}
fn vector(values: &[f32]) -> Result<SimpleExpr> {
	if values.is_empty()
		|| values.len() > 8192
		|| values.iter().any(|value| !value.is_finite())
		|| values.iter().all(|value| *value == 0.0)
	{
		return Err(Error::Invalid("invalid cosine embedding".into()));
	}
	let value = format!(
		"[{}]",
		values
			.iter()
			.map(ToString::to_string)
			.collect::<Vec<_>>()
			.join(",")
	);
	Ok(SimpleExpr::CustomWithExpr(
		"CAST(? AS vector)".into(),
		vec![Expr::value(value).into()],
	))
}
#[async_trait]
impl EmbeddingProvider for Transport {
	async fn embed(
		&self,
		config: &EmbeddingConfig,
		text: &str,
	) -> aidash_application::Result<Embedding> {
		self.embedding.embed(config, text).await
	}
}
#[async_trait]
impl VectorIndex for Transport {
	async fn ensure_collection(
		&self,
		config: &VectorConfig,
		collection: &str,
		dimensions: usize,
	) -> aidash_application::Result<()> {
		let result: Result<()> = async {
			provider(config)?;
			if collection.is_empty() || collection.len() > 256 || !(1..=8192).contains(&dimensions)
			{
				return Err(Error::Invalid(
					"invalid PostgreSQL vector collection".into(),
				));
			}
			let mut tx = self.pool.begin().await?;
			native::query(
				&Query::insert()
					.into_table(Alias::new("semantic_vector_collections"))
					.columns([Alias::new("collection"), Alias::new("dimensions")])
					.from_subquery(
						Query::select()
							.expr(Expr::value(collection))
							.expr(Expr::value(dimensions as i32))
							.to_owned(),
					)
					.on_conflict(
						OnConflict::column(Alias::new("collection"))
							.do_nothing()
							.to_owned(),
					)
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut *tx)
			.await?;
			let width: i32 = native::query_scalar(
				&Query::select()
					.column(Alias::new("dimensions"))
					.from(Alias::new("semantic_vector_collections"))
					.and_where(Expr::col(Alias::new("collection")).eq(collection))
					.lock(LockType::Share)
					.to_string(PostgresQueryBuilder),
			)
			.scalar_one(&mut *tx)
			.await?;
			if width != dimensions as i32 {
				return Err(Error::Conflict(
					"embedding dimensions changed without a new index generation".into(),
				));
			}
			tx.commit().await
		}
		.await;
		result.map_err(Into::into)
	}
	async fn upsert(
		&self,
		config: &VectorConfig,
		collection: &str,
		point: Uuid,
		values: &[f32],
		payload: Value,
	) -> aidash_application::Result<()> {
		let result: Result<()> = async {
			provider(config)?;
			let embedding = vector(values)?;
			let workspace: Uuid = serde_json::from_value(payload["workspace_id"].clone())?;
			let tenant = payload["tenant"]
				.as_str()
				.filter(|s| !s.is_empty())
				.ok_or_else(|| Error::Invalid("missing vector tenant".into()))?;
			let mut tx = self.pool.begin().await?;
			let width: i32 = native::query_scalar(
				&Query::select()
					.column(Alias::new("dimensions"))
					.from(Alias::new("semantic_vector_collections"))
					.and_where(Expr::col(Alias::new("collection")).eq(collection))
					.lock(LockType::Share)
					.to_string(PostgresQueryBuilder),
			)
			.scalar_one(&mut *tx)
			.await?;
			if width != values.len() as i32 {
				return Err(Error::Invalid("embedding dimension mismatch".into()));
			}
			// Point IDs are immutable source/index generations. A retry cannot move an ID to another scope.
			let existing: Option<(String, Value)> = native::query_as(
				&Query::select()
					.columns(["collection", "payload"].map(Alias::new))
					.from(Alias::new("semantic_vectors"))
					.and_where(Expr::col(Alias::new("id")).eq(Expr::value(point)))
					.lock(LockType::Update)
					.to_string(PostgresQueryBuilder),
			)
			.columns(&["collection", "payload"])
			.fetch_optional(&mut *tx)
			.await?;
			if existing
				.as_ref()
				.is_some_and(|(c, p)| c != collection || p != &payload)
			{
				return Err(Error::Conflict(
					"vector point identity already belongs to another source revision".into(),
				));
			}
			native::query(
				&Query::insert()
					.into_table(Alias::new("semantic_vectors"))
					.columns([
						Alias::new("id"),
						Alias::new("collection"),
						Alias::new("workspace_id"),
						Alias::new("tenant"),
						Alias::new("embedding"),
						Alias::new("payload"),
					])
					.from_subquery(
						Query::select()
							.expr(Expr::value(point))
							.expr(Expr::value(collection))
							.expr(Expr::value(workspace))
							.expr(Expr::value(tenant))
							.expr(embedding)
							.expr(Expr::value(payload.clone()))
							.to_owned(),
					)
					.on_conflict(OnConflict::column(Alias::new("id")).do_nothing().to_owned())
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut *tx)
			.await?;
			// Also check the winner after a concurrent insert; an existing ID must never silently succeed.
			let actual: (String, Value) = native::query_as(
				&Query::select()
					.columns(["collection", "payload"].map(Alias::new))
					.from(Alias::new("semantic_vectors"))
					.and_where(Expr::col(Alias::new("id")).eq(Expr::value(point)))
					.to_string(PostgresQueryBuilder),
			)
			.columns(&["collection", "payload"])
			.fetch_one(&mut *tx)
			.await?;
			if actual.0 != collection || actual.1 != payload {
				return Err(Error::Conflict(
					"concurrent vector identity conflict".into(),
				));
			}
			tx.commit().await
		}
		.await;
		result.map_err(Into::into)
	}
	async fn delete_point(
		&self,
		config: &VectorConfig,
		collection: &str,
		point: Uuid,
	) -> aidash_application::Result<()> {
		provider(config)?;
		native::query(
			&Query::delete()
				.from_table(Alias::new("semantic_vectors"))
				.and_where(Expr::col(Alias::new("collection")).eq(collection))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::value(point)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&self.pool)
		.await?;
		Ok(())
	}
	async fn delete_collection(
		&self,
		config: &VectorConfig,
		collection: &str,
	) -> aidash_application::Result<()> {
		provider(config)?;
		let mut tx = self.pool.begin().await?;
		native::query(
			&Query::delete()
				.from_table(Alias::new("semantic_vectors"))
				.and_where(Expr::col(Alias::new("collection")).eq(collection))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await?;
		native::query(
			&Query::delete()
				.from_table(Alias::new("semantic_vector_collections"))
				.and_where(Expr::col(Alias::new("collection")).eq(collection))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *tx)
		.await?;
		tx.commit().await?;
		Ok(())
	}
	async fn query(
		&self,
		config: &VectorConfig,
		collection: &str,
		values: &[f32],
		filter: VectorFilter<'_>,
		limit: usize,
	) -> aidash_application::Result<Vec<Point>> {
		provider(config)?;
		if filter.allowed.is_empty() || limit == 0 {
			return Ok(vec![]);
		}
		if filter.allowed.len() > 1024 || limit > 1024 {
			return Err(aidash_application::Error::Invalid(
				"vector search exceeds authorized candidate bounds".into(),
			));
		}
		let distance = SimpleExpr::CustomWithExpr(
			"(? <=> ?)".into(),
			vec![Expr::col(Alias::new("embedding")).into(), vector(values)?],
		);
		// Exact search over the bounded authorized set avoids filtered ANN recall loss.
		let sql = Query::select()
			.column(Alias::new("id"))
			.column(Alias::new("payload"))
			.expr_as(
				SimpleExpr::CustomWithExpr("CAST(1 - ? AS real)".into(), vec![distance.clone()]),
				Alias::new("score"),
			)
			.from(Alias::new("semantic_vectors"))
			.and_where(Expr::col(Alias::new("collection")).eq(collection))
			.and_where(Expr::col(Alias::new("workspace_id")).eq(Expr::value(filter.workspace)))
			.and_where(Expr::col(Alias::new("tenant")).eq(filter.tenant))
			.and_where(
				Expr::col(Alias::new("id")).is_in(filter.allowed.iter().copied().map(Expr::value)),
			)
			.order_by_expr(distance, Order::Asc)
			.order_by(Alias::new("id"), Order::Asc)
			.limit(limit as u64)
			.to_string(PostgresQueryBuilder);
		let rows = native::query(&sql).fetch_all(&self.pool).await?;
		rows.iter()
			.map(|row| {
				Ok(Point {
					id: row.try_get("id")?,
					score: row.try_get("score")?,
					payload: row.try_get("payload")?,
				})
			})
			.collect::<Result<Vec<_>>>()
			.map_err(Into::into)
	}
	async fn present(
		&self,
		config: &VectorConfig,
		collection: &str,
		ids: &[Uuid],
	) -> aidash_application::Result<bool> {
		provider(config)?;
		if ids.is_empty() {
			return Ok(true);
		}
		let count: i64 = native::query_scalar(
			&Query::select()
				.expr(Expr::cust("COUNT(DISTINCT id)"))
				.from(Alias::new("semantic_vectors"))
				.and_where(Expr::col(Alias::new("collection")).eq(collection))
				.and_where(Expr::col(Alias::new("id")).is_in(ids.iter().copied().map(Expr::value)))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_one(&self.pool)
		.await?;
		Ok(count as usize == ids.iter().collect::<std::collections::BTreeSet<_>>().len())
	}
}
