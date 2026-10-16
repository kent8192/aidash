//! Immutable definitions on a caller-owned native transaction, each scoped to
//! the Tenant that owns the Registry (ADR 0033).
use super::{AgentKnowledge, Definition};
use crate::apps::registry::serializers::contracts::Entry;
use crate::{Error, Result};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{Model, QueryRow};
use reinhardt::query::{
	Alias, ColumnRef, Expr, ExprTrait, LockType, OnConflict, PostgresQueryBuilder, Query,
	QueryStatementBuilder,
};
use serde_json::Value;

impl Definition {
	pub(crate) async fn read_in(
		tx: &mut dyn TransactionExecutor,
		tenant: &str,
		id: &str,
		version: &str,
	) -> Result<Entry> {
		Ok(serde_json::from_value(
			definition(tx, tenant, id, version)
				.await?
				.metadata
				.into_inner(),
		)?)
	}
}

pub(crate) async fn definition(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	id: &str,
	version: &str,
) -> Result<Definition> {
	let (sql, values) = Query::select()
		.column(ColumnRef::Asterisk)
		.from(Alias::new(Definition::table_name()))
		.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
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
	tenant: &str,
	entry: &Entry,
) -> Result<bool> {
	let value = serde_json::to_value(entry)?;
	let (sql, values) = Query::insert()
		.into_table(Alias::new(Definition::table_name()))
		.columns(["tenant", "id", "version", "kind", "metadata"].map(Alias::new))
		.values_panic([
			IntoValue::into_value(tenant),
			IntoValue::into_value(&entry.id),
			IntoValue::into_value(&entry.version),
			IntoValue::into_value(&entry.kind),
			IntoValue::into_value(value.clone()),
		])
		.on_conflict(
			OnConflict::columns([Alias::new("tenant"), Alias::new("id"), Alias::new("version")])
				.do_nothing()
				.to_owned(),
		)
		.build(PostgresQueryBuilder);
	let inserted = tx
		.execute(&sql, convert_values(values))
		.await?
		.rows_affected
		!= 0;
	if definition(tx, tenant, &entry.id, &entry.version)
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

pub(crate) async fn insert_documents(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	entry: &Entry,
	documents: Value,
) -> Result<()> {
	let _definition = definition(tx, tenant, &entry.id, &entry.version).await?;
	let (sql, values) = Query::insert()
		.into_table(Alias::new(AgentKnowledge::table_name()))
		.columns(["tenant", "agent_id", "agent_version", "documents"].map(Alias::new))
		.values_panic([
			IntoValue::into_value(tenant),
			IntoValue::into_value(&entry.id),
			IntoValue::into_value(&entry.version),
			IntoValue::into_value(documents.clone()),
		])
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
	let stored = AgentKnowledge::objects()
		.filter(AgentKnowledge::field_tenant().eq(tenant))
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

use reinhardt::query::IntoValue;
