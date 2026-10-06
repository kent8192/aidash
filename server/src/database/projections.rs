//! Domain projections decoded at the native persistence boundary.
use super::{
	Record,
	native::{self, Decode, Row},
};
use crate::Result;
pub struct RecordRow<T>(T);
impl<T: Record> Decode for RecordRow<T> {
	fn decode(row: &Row, _: &[&str]) -> Result<Self> {
		T::decode(row).map(Self)
	}
}
pub struct Projection<'q, T> {
	query: native::Query<'q, RecordRow<T>>,
}
pub fn query_as<T: Record>(sql: &str) -> Projection<'_, T> {
	Projection {
		query: native::query_as(sql),
	}
}
impl<T: Record> Projection<'_, T> {
	pub fn bind(mut self, value: impl native::Parameter) -> Self {
		self.query = self.query.bind(value);
		self
	}
	pub async fn fetch_all(self, executor: impl native::Executor) -> Result<Vec<T>> {
		Ok(self
			.query
			.fetch_all(executor)
			.await?
			.into_iter()
			.map(|row| row.0)
			.collect())
	}
	pub async fn fetch_one(self, executor: impl native::Executor) -> Result<T> {
		Ok(self.query.fetch_one(executor).await?.0)
	}
	pub async fn fetch_optional(self, executor: impl native::Executor) -> Result<Option<T>> {
		Ok(self.query.fetch_optional(executor).await?.map(|row| row.0))
	}
}
