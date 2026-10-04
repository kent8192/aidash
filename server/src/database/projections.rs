//! A scoped PostgreSQL projection converted to an application/domain value.
use super::records::{Record, RecordRow};
use sqlx::{Encode, Executor, Postgres, Type, postgres::PgArguments, query::QueryAs};

pub struct Projection<'q, T> {
	query: QueryAs<'q, Postgres, RecordRow<T>, PgArguments>,
}

pub fn query_as<T: Record + Send + Unpin>(sql: &str) -> Projection<'_, T> {
	Projection {
		query: sqlx::query_as(sql),
	}
}

impl<'q, T: Record + Send + Unpin> Projection<'q, T> {
	pub fn bind<V: 'q + Encode<'q, Postgres> + Type<Postgres>>(mut self, value: V) -> Self {
		self.query = self.query.bind(value);
		self
	}

	pub async fn fetch_all<'e, E>(self, executor: E) -> Result<Vec<T>, sqlx::Error>
	where
		'q: 'e,
		E: Executor<'e, Database = Postgres>,
	{
		Ok(self
			.query
			.fetch_all(executor)
			.await?
			.into_iter()
			.map(|row| row.0)
			.collect())
	}

	pub async fn fetch_one<'e, E>(self, executor: E) -> Result<T, sqlx::Error>
	where
		'q: 'e,
		E: Executor<'e, Database = Postgres>,
	{
		Ok(self.query.fetch_one(executor).await?.0)
	}

	pub async fn fetch_optional<'e, E>(self, executor: E) -> Result<Option<T>, sqlx::Error>
	where
		'q: 'e,
		E: Executor<'e, Database = Postgres>,
	{
		Ok(self.query.fetch_optional(executor).await?.map(|row| row.0))
	}
}
