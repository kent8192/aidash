//! Original catalog joins and locks implement portable admission/discovery ports.
use crate::{
	Result,
	authorization::access::Access,
	registry::{EntityRef, Entry},
};
use aidash_application::ports::catalog::CatalogScope;
use aidash_domain::policy::Resource;
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Condition, Expr, ExprTrait, JoinType, LockType, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder, TableRef,
};
use serde_json::Value;
pub(crate) struct NativeCatalog<'a>(pub(crate) &'a mut Access);
async fn document(access: &mut Access, reference: &EntityRef) -> Result<Option<Value>> {
	let query = if access.inherited_lease {
		Query::select()
			.column((Alias::new("r"), Alias::new("metadata")))
			.from_as(Alias::new("authorization_catalog"), Alias::new("c"))
			.join(
				JoinType::InnerJoin,
				TableRef::table_alias(Alias::new("registry"), Alias::new("r")),
				Condition::all()
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col((
							Alias::new("r"),
							Alias::new("id"),
						)))
						.eq(Expr::col((Alias::new("c"), Alias::new("entry_id")))),
					)
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col((
							Alias::new("r"),
							Alias::new("version"),
						)))
						.eq(Expr::col((Alias::new("c"), Alias::new("entry_version")))),
					),
			)
			.and_where(
				Condition::all()
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col((
							Alias::new("c"),
							Alias::new("tenant"),
						)))
						.eq(Expr::cust("$1")),
					)
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col((
							Alias::new("c"),
							Alias::new("entry_id"),
						)))
						.eq(Expr::cust("$2")),
					)
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col((
							Alias::new("c"),
							Alias::new("entry_version"),
						)))
						.eq(Expr::cust("$3")),
					)
					.add(Expr::col((Alias::new("c"), Alias::new("enabled")))),
			)
			.to_string(PostgresQueryBuilder)
	} else {
		Query::select()
			.column((Alias::new("r"), Alias::new("metadata")))
			.from_as(Alias::new("authorization_catalog"), Alias::new("c"))
			.join(
				JoinType::InnerJoin,
				TableRef::table_alias(Alias::new("registry"), Alias::new("r")),
				Condition::all()
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col((
							Alias::new("r"),
							Alias::new("id"),
						)))
						.eq(Expr::col((Alias::new("c"), Alias::new("entry_id")))),
					)
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col((
							Alias::new("r"),
							Alias::new("version"),
						)))
						.eq(Expr::col((Alias::new("c"), Alias::new("entry_version")))),
					),
			)
			.and_where(
				Condition::all()
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col((
							Alias::new("c"),
							Alias::new("tenant"),
						)))
						.eq(Expr::cust("$1")),
					)
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col((
							Alias::new("c"),
							Alias::new("entry_id"),
						)))
						.eq(Expr::cust("$2")),
					)
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col((
							Alias::new("c"),
							Alias::new("entry_version"),
						)))
						.eq(Expr::cust("$3")),
					)
					.add(Expr::col((Alias::new("c"), Alias::new("enabled")))),
			)
			.lock(LockType::Share)
			.lock_tables([Alias::new("c")])
			.to_string(PostgresQueryBuilder)
	};
	let document: Option<Value> = sqlx::query_scalar(&query)
		.bind(&access.identity.tenant)
		.bind(&reference.id)
		.bind(&reference.version)
		.fetch_optional(&mut **access.tx)
		.await?;

	Ok(document)
}
async fn documents(access: &mut Access) -> Result<Vec<Value>> {
	let query = if access.inherited_lease {
		Query::select()
			.column((Alias::new("r"), Alias::new("metadata")))
			.from_as(Alias::new("authorization_catalog"), Alias::new("c"))
			.join(
				JoinType::InnerJoin,
				TableRef::table_alias(Alias::new("registry"), Alias::new("r")),
				Condition::all()
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col((
							Alias::new("r"),
							Alias::new("id"),
						)))
						.eq(Expr::col((Alias::new("c"), Alias::new("entry_id")))),
					)
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col((
							Alias::new("r"),
							Alias::new("version"),
						)))
						.eq(Expr::col((Alias::new("c"), Alias::new("entry_version")))),
					),
			)
			.and_where(
				Condition::all()
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col((
							Alias::new("c"),
							Alias::new("tenant"),
						)))
						.eq(Expr::cust("$1")),
					)
					.add(Expr::col((Alias::new("c"), Alias::new("enabled")))),
			)
			.order_by((Alias::new("c"), Alias::new("entry_id")), Order::Asc)
			.order_by((Alias::new("c"), Alias::new("entry_version")), Order::Asc)
			.to_string(PostgresQueryBuilder)
	} else {
		Query::select()
			.column((Alias::new("r"), Alias::new("metadata")))
			.from_as(Alias::new("authorization_catalog"), Alias::new("c"))
			.join(
				JoinType::InnerJoin,
				TableRef::table_alias(Alias::new("registry"), Alias::new("r")),
				Condition::all()
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col((
							Alias::new("r"),
							Alias::new("id"),
						)))
						.eq(Expr::col((Alias::new("c"), Alias::new("entry_id")))),
					)
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col((
							Alias::new("r"),
							Alias::new("version"),
						)))
						.eq(Expr::col((Alias::new("c"), Alias::new("entry_version")))),
					),
			)
			.and_where(
				Condition::all()
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col((
							Alias::new("c"),
							Alias::new("tenant"),
						)))
						.eq(Expr::cust("$1")),
					)
					.add(Expr::col((Alias::new("c"), Alias::new("enabled")))),
			)
			.order_by((Alias::new("c"), Alias::new("entry_id")), Order::Asc)
			.order_by((Alias::new("c"), Alias::new("entry_version")), Order::Asc)
			.lock(LockType::Share)
			.lock_tables([Alias::new("c")])
			.to_string(PostgresQueryBuilder)
	};
	let documents: Vec<Value> = sqlx::query_scalar(&query)
		.bind(&access.identity.tenant)
		.fetch_all(&mut **access.tx)
		.await?;

	Ok(documents)
}
#[async_trait]
impl CatalogScope for NativeCatalog<'_> {
	fn tenant(&self) -> &str {
		&self.0.identity.tenant
	}
	fn inherited_lease(&self) -> bool {
		self.0.inherited_lease
	}
	fn approved(&self, reference: &EntityRef) -> bool {
		self.0
			.approved_catalog
			.contains(&(reference.id.clone(), reference.version.clone()))
	}
	fn remember(&mut self, reference: &EntityRef) {
		self.0
			.approved_catalog
			.insert((reference.id.clone(), reference.version.clone()));
	}
	async fn distribution_lock(&mut self) -> aidash_application::Result<()> {
		crate::marketplace::lock_catalog(&mut self.0.tx, false)
			.await
			.map_err(Into::into)
	}
	async fn document(
		&mut self,
		reference: &EntityRef,
	) -> aidash_application::Result<Option<Value>> {
		document(self.0, reference).await.map_err(Into::into)
	}
	async fn documents(&mut self) -> aidash_application::Result<Vec<Value>> {
		documents(self.0).await.map_err(Into::into)
	}
	fn resource(&self, entry: &Entry) -> Resource {
		crate::authorization::catalog::resource(self.0, entry)
	}
	async fn require(
		&mut self,
		resource: &Resource,
		action: &str,
	) -> aidash_application::Result<()> {
		self.0.require(resource, action).await.map_err(Into::into)
	}
	async fn decide(
		&mut self,
		resource: &Resource,
		action: &str,
	) -> aidash_application::Result<bool> {
		self.0.decide(resource, action).await.map_err(Into::into)
	}
	async fn active(&mut self, entry: &Entry) -> aidash_application::Result<bool> {
		crate::marketplace::active(self.0, entry)
			.await
			.map_err(Into::into)
	}
	async fn check_pinned(&mut self, entry: &Entry) -> aidash_application::Result<()> {
		crate::marketplace::check_pinned(self.0, entry)
			.await
			.map_err(Into::into)
	}
}
