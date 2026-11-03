// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

//! Marketplace-owned ORM persistence. No Cloud model or table is reachable here.
use super::super::models::Document;
use crate::{Error, Result};
use reinhardt::db::{
	backends::DatabaseConnection as Backend,
	orm::{
		AtomicTransaction, DatabaseConnection, DatabaseConnectionLease, Model, OrmExecutor,
		execution::convert_values,
	},
};
use reinhardt::query::{
	Alias, Expr, IntoIden, OnConflict, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use serde::{Serialize, de::DeserializeOwned};

pub struct Store {
	_lease: DatabaseConnectionLease,
	pub connection: DatabaseConnection,
}
impl Store {
	// This independent service owns its schema and role; Cloud's Tenant-scoped
	// Pool cannot compose it. Bootstrap registers only this explicit owned lease,
	// without installing a global/default connection.
	#[allow(clippy::disallowed_methods)]
	pub async fn connect(url: &str) -> Result<Self> {
		let backend = Backend::connect_postgres(url)
			.await
			.map_err(|_| Error::Unavailable)?;
		let lease = DatabaseConnectionLease::register(backend)?;
		let connection = lease.handle();
		let (sql, values) = Query::select()
			.expr_as(
				reinhardt::query::SimpleExpr::FunctionCall("current_schema".into_iden(), vec![]),
				Alias::new("schema"),
			)
			.build(PostgresQueryBuilder);
		let row = connection.query_one(&sql, convert_values(values)).await?;
		if row.get::<String>("schema").as_deref() != Some("marketplace") {
			return Err(Error::Invalid(
				"database search_path must start with marketplace",
			));
		}
		Ok(Self {
			_lease: lease,
			connection,
		})
	}

	pub async fn require_runtime_role(&self) -> Result<()> {
		let (sql, values) = Query::select()
			.expr_as(
				reinhardt::query::SimpleExpr::FunctionCall(
					"has_schema_privilege".into_iden(),
					vec![
						Expr::value("marketplace").into(),
						Expr::value("CREATE").into(),
					],
				),
				Alias::new("can_create"),
			)
			.expr_as(
				reinhardt::query::SimpleExpr::FunctionCall(
					"has_table_privilege".into_iden(),
					vec![
						Expr::value("marketplace.reinhardt_migrations").into(),
						Expr::value("INSERT,UPDATE,DELETE,TRUNCATE,REFERENCES,TRIGGER").into(),
					],
				),
				Alias::new("ledger_write"),
			)
			.build(PostgresQueryBuilder);
		let row = self
			.connection
			.query_one(&sql, convert_values(values))
			.await?;
		if row.get::<bool>("can_create") != Some(false)
			|| row.get::<bool>("ledger_write") != Some(false)
		{
			return Err(Error::Invalid(
				"Marketplace runtime credential must not create schema objects or write migration history",
			));
		}
		Ok(())
	}
	pub async fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
		let mut conn = self.connection;
		get(&mut conn, key).await
	}
	pub async fn all<T: DeserializeOwned>(&self, kind: &str) -> Result<Vec<T>> {
		let mut conn = self.connection;
		all(&mut conn, kind).await
	}
}
pub async fn get<T: DeserializeOwned>(tx: &mut impl OrmExecutor, key: &str) -> Result<Option<T>> {
	let row = Document::objects()
		.filter(Document::field_key().eq(key.to_owned()))
		.first_with_db(tx)
		.await?;
	row.map(|row| serde_json::from_value(row.document.0).map_err(Error::from))
		.transpose()
}
pub async fn all<T: DeserializeOwned>(tx: &mut impl OrmExecutor, kind: &str) -> Result<Vec<T>> {
	Document::objects()
		.filter(Document::field_kind().eq(kind.to_owned()))
		.all_with_db(tx)
		.await?
		.into_iter()
		.map(|row| serde_json::from_value(row.document.0).map_err(Error::from))
		.collect()
}
pub async fn put(
	tx: &mut impl OrmExecutor,
	key: &str,
	kind: &str,
	value: &impl Serialize,
) -> Result<()> {
	let (sql, values) = Query::insert()
		.into_table(Alias::new("marketplace_documents"))
		.columns([
			Alias::new("key"),
			Alias::new("kind"),
			Alias::new("document"),
		])
		.values_panic([
			reinhardt::query::Value::from(key),
			reinhardt::query::Value::from(kind),
			reinhardt::query::Value::from(serde_json::to_value(value)?),
		])
		.on_conflict(
			OnConflict::column(Alias::new("key"))
				.update_columns([Alias::new("kind"), Alias::new("document")])
				.to_owned(),
		)
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	Ok(())
}
pub async fn remove(tx: &mut impl OrmExecutor, key: &str) -> Result<()> {
	Document::objects()
		.filter(Document::field_key().eq(key.to_owned()))
		.delete_with_conn(tx)
		.await?;
	Ok(())
}
/// Serialize Marketplace mutations on a PostgreSQL transaction-scoped advisory
/// lock. It protects uniqueness and authorization against concurrent revocation;
/// it never holds a lock across remote authentication or an HTTP response.
pub async fn lock(tx: &mut AtomicTransaction) -> Result<()> {
	let (sql, values) = Query::select()
		.expr(reinhardt::query::SimpleExpr::FunctionCall(
			"pg_advisory_xact_lock".into_iden(),
			vec![Expr::value(71003310_i64).into()],
		))
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	Ok(())
}

pub async fn lock_key(tx: &mut AtomicTransaction, key: &str) -> Result<()> {
	let lock_id = u64::from_str_radix(&crate::auth::hash(key)[..16], 16)
		.map_err(|_| Error::Unavailable)? as i64;
	let (sql, values) = Query::select()
		.expr(reinhardt::query::SimpleExpr::FunctionCall(
			"pg_advisory_xact_lock".into_iden(),
			vec![Expr::value(lock_id).into()],
		))
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	Ok(())
}
