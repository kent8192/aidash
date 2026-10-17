//! Immutable definitions on a caller-owned native transaction. These reads and
//! writes reach only the Registry of the Tenant whose scope the transaction was
//! opened in (`aidash.tenant`), the condition row-level security enforces for
//! Tenant-owned tables (#265); outside a Tenant scope they see nothing.
use super::{AgentKnowledge, Definition};
use crate::apps::registry::serializers::contracts::Entry;
use crate::{Error, Result};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{Model, QueryRow};
use reinhardt::query::{
	Alias, ColumnRef, Expr, ExprTrait, LockType, OnConflict, PostgresQueryBuilder, Query,
	QueryStatementBuilder, SimpleExpr,
};
use serde_json::Value;

/// The Tenant of the transaction's scope; empty outside a Tenant scope.
const SCOPE_TENANT: &str = "current_setting('aidash.tenant', true)";
/// The same Tenant for an insert, absent (and refused) outside a Tenant scope.
const INSERT_TENANT: &str = "NULLIF(current_setting('aidash.tenant', true), '')";

/// The Tenant whose scope the transaction was opened in, for ORM filters.
async fn scope_tenant(tx: &mut dyn TransactionExecutor) -> Result<String> {
	let row = tx
		.fetch_one(&format!("SELECT {SCOPE_TENANT} AS tenant"), vec![])
		.await?;
	let value = QueryRow::from_backend_row(row).data;
	Ok(value["tenant"].as_str().unwrap_or_default().to_owned())
}

impl Definition {
	/// A named Tenant's definition; see [`definition_of`].
	pub(crate) async fn read_of(
		tx: &mut dyn TransactionExecutor,
		tenant: &str,
		id: &str,
		version: &str,
	) -> Result<Entry> {
		Ok(serde_json::from_value(
			definition_of(tx, tenant, id, version)
				.await?
				.metadata
				.into_inner(),
		)?)
	}
	pub(crate) async fn read_in(
		tx: &mut dyn TransactionExecutor,
		id: &str,
		version: &str,
	) -> Result<Entry> {
		Ok(serde_json::from_value(
			definition(tx, id, version).await?.metadata.into_inner(),
		)?)
	}
}

pub(crate) async fn definition(
	tx: &mut dyn TransactionExecutor,
	id: &str,
	version: &str,
) -> Result<Definition> {
	definition_where(tx, Expr::cust(SCOPE_TENANT).into(), id, version).await
}

/// A definition of a named Tenant, for work in a Deployment Scope that acts
/// for one Tenant at a time, such as memory maintenance.
pub(crate) async fn definition_of(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	id: &str,
	version: &str,
) -> Result<Definition> {
	definition_where(tx, Expr::value(tenant).into(), id, version).await
}

async fn definition_where(
	tx: &mut dyn TransactionExecutor,
	tenant: SimpleExpr,
	id: &str,
	version: &str,
) -> Result<Definition> {
	let (sql, values) = Query::select()
		.column(ColumnRef::Asterisk)
		.from(Alias::new(Definition::table_name()))
		.and_where(Expr::col("tenant").eq(tenant))
		.and_where(Expr::col("id").eq(Expr::value(id)))
		.and_where(Expr::col("version").eq(Expr::value(version)))
		.lock(LockType::Share)
		.build(PostgresQueryBuilder);
	let row = tx
		.fetch_optional(&sql, convert_values(values))
		.await?
		.ok_or_else(|| Error::NotFound(format!("entity {id}@{version}")))?;
	Ok(serde_json::from_value(
		QueryRow::from_backend_row(row).data,
	)?)
}

pub(crate) async fn insert_definition(
	tx: &mut dyn TransactionExecutor,
	entry: &Entry,
) -> Result<bool> {
	let value = serde_json::to_value(entry)?;
	let (sql, values) = Query::insert()
		.into_table(Alias::new(Definition::table_name()))
		.columns(["tenant", "id", "version", "kind", "metadata"].map(Alias::new))
		.from_subquery(
			Query::select()
				.expr(Expr::cust(INSERT_TENANT))
				.expr(Expr::value(&entry.id))
				.expr(Expr::value(&entry.version))
				.expr(Expr::value(&entry.kind))
				.expr(Expr::value(value.clone()))
				.to_owned(),
		)
		.on_conflict(
			OnConflict::columns([
				Alias::new("tenant"),
				Alias::new("id"),
				Alias::new("version"),
			])
			.do_nothing()
			.to_owned(),
		)
		.build(PostgresQueryBuilder);
	let inserted = tx
		.execute(&sql, convert_values(values))
		.await?
		.rows_affected
		!= 0;
	if definition(tx, &entry.id, &entry.version)
		.await?
		.metadata
		.into_inner()
		!= value
	{
		return Err(Error::Conflict(
			"published versions are immutable; choose a new version".into(),
		));
	}
	Ok(inserted)
}

pub(crate) async fn documents(
	tx: &mut dyn TransactionExecutor,
	entry: &Entry,
) -> Result<Option<Value>> {
	let tenant = scope_tenant(tx).await?;
	Ok(AgentKnowledge::objects()
		.filter(AgentKnowledge::field_tenant().eq(&tenant))
		.filter(AgentKnowledge::field_agent_key().eq(&entry.id))
		.filter(AgentKnowledge::field_agent_version().eq(&entry.version))
		.all_with_executor(tx)
		.await
		.map_err(FrameworkError::from)?
		.into_iter()
		.next()
		.map(|record| record.documents.into_inner()))
}

pub(crate) async fn insert_documents(
	tx: &mut dyn TransactionExecutor,
	entry: &Entry,
	documents: Value,
) -> Result<()> {
	let _definition = definition(tx, &entry.id, &entry.version).await?;
	let (sql, values) = Query::insert()
		.into_table(Alias::new(AgentKnowledge::table_name()))
		.columns(["tenant", "agent_id", "agent_version", "documents"].map(Alias::new))
		.from_subquery(
			Query::select()
				.expr(Expr::cust(INSERT_TENANT))
				.expr(Expr::value(&entry.id))
				.expr(Expr::value(&entry.version))
				.expr(Expr::value(documents.clone()))
				.to_owned(),
		)
		.on_conflict(
			OnConflict::columns([
				Alias::new("tenant"),
				Alias::new("agent_id"),
				Alias::new("agent_version"),
			])
			.do_nothing()
			.to_owned(),
		)
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	let tenant = scope_tenant(tx).await?;
	let stored = AgentKnowledge::objects()
		.filter(AgentKnowledge::field_tenant().eq(&tenant))
		.filter(AgentKnowledge::field_agent_key().eq(&entry.id))
		.filter(AgentKnowledge::field_agent_version().eq(&entry.version))
		.all_with_executor(tx)
		.await
		.map_err(FrameworkError::from)?
		.into_iter()
		.next()
		.ok_or_else(|| Error::NotFound("agent knowledge".into()))?;
	if stored.documents.into_inner() != documents {
		return Err(Error::Conflict(
			"private documents are immutable; choose a new version".into(),
		));
	}
	Ok(())
}
