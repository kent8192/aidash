//! Native persistence for immutable registry versions.
use super::states::DefinitionKind;
use super::{AgentKnowledge, Definition, RegistryRequest};
use crate::apps::registry::serializers::contracts::Entry;
use crate::{Error, Result};
use reinhardt::db::orm::{AtomicTransaction, Model, OrmExecutor};
use serde_json::Value;
use uuid::Uuid;

pub(crate) async fn definition<E: OrmExecutor>(
	db: &mut E,
	id: &str,
	version: &str,
) -> Result<Definition> {
	Definition::objects()
		.filter(Definition::field_id().eq(id.to_owned()))
		.filter(Definition::field_version().eq(version.to_owned()))
		.first_with_db(db)
		.await?
		.ok_or_else(|| Error::NotFound(format!("entity {id}@{version}")))
}

pub(crate) async fn definitions<E: OrmExecutor>(
	db: &mut E,
	kind: Option<&str>,
	offset: usize,
	limit: Option<usize>,
) -> Result<Vec<Definition>> {
	let mut query = Definition::objects()
		.all()
		.order_by(&["id", "version"])
		.offset(offset);
	if let Some(kind) = kind {
		let Ok(kind) = serde_json::from_value::<DefinitionKind>(Value::String(kind.to_owned()))
		else {
			return Ok(Vec::new());
		};
		query = query.filter(Definition::field_kind().eq(kind));
	}
	if let Some(limit) = limit {
		query = query.limit(limit);
	}
	Ok(query.all_with_db(db).await?)
}

/// A conflicting insert waits for the winning transaction before reading it.
pub(crate) async fn insert_definition<E: OrmExecutor>(db: &mut E, entry: &Entry) -> Result<bool> {
	let value = serde_json::to_value(entry)?;
	let (sql, values) = Query::insert()
		.into_table(Alias::new("registry"))
		.columns(["id", "version", "kind", "metadata"].map(Alias::new))
		.from_subquery(
			reinhardt::query::Query::select()
				.expr(Expr::value(&entry.id))
				.expr(Expr::value(&entry.version))
				.expr(Expr::value(&entry.kind))
				.expr(Expr::value(value.clone()))
				.to_owned(),
		)
		.on_conflict(
			OnConflict::columns(["id", "version"])
				.do_nothing()
				.to_owned(),
		)
		.build(PostgresQueryBuilder);
	let inserted = db
		.execute(&sql, convert_values(values))
		.await?
		.rows_affected
		== 1;
	let stored = definition(db, &entry.id, &entry.version).await?;
	if stored.metadata.0 != value {
		return Err(Error::Conflict(
			"published versions are immutable; choose a new version".into(),
		));
	}
	Ok(inserted)
}

pub(crate) async fn assign_id<E: OrmExecutor>(
	db: &mut E,
	entry: &mut Entry,
	key: Option<Uuid>,
) -> Result<()> {
	let request = serde_json::to_value(&*entry)?;
	if entry.id.is_empty() {
		entry.id = Uuid::now_v7().to_string();
	}
	let Some(key) = key else {
		return Ok(());
	};
	let draft = RegistryRequest::build()
		.key(key)
		.request(request.clone().into())
		.entity_id(&entry.id)
		.finish();
	RegistryRequest::objects()
		.bulk_create_with_conn(db, vec![draft], None, true, false)
		.await?;
	let stored = RegistryRequest::objects()
		.filter(RegistryRequest::field_key().eq(key))
		.get_with_db(db)
		.await?;
	if stored.request.0 != request {
		return Err(Error::Conflict(
			"registration idempotency key reused with different input".into(),
		));
	}
	entry.id = stored.entity_id;
	Ok(())
}

pub(crate) async fn documents<E: OrmExecutor>(db: &mut E, entry: &Entry) -> Result<Option<Value>> {
	Ok(AgentKnowledge::objects()
		.filter(AgentKnowledge::field_agent_key().eq(entry.id.clone()))
		.filter(AgentKnowledge::field_agent_version().eq(entry.version.clone()))
		.first_with_db(db)
		.await?
		.map(|record| record.documents.0))
}

pub(crate) async fn insert_documents(
	db: &mut AtomicTransaction,
	entry: &Entry,
	documents: Value,
) -> Result<()> {
	let _definition = definition(db, &entry.id, &entry.version).await?;
	let draft = AgentKnowledge::build()
		.agent_key(&entry.id)
		.agent_version(&entry.version)
		.documents(documents.clone().into())
		.finish();
	AgentKnowledge::objects()
		.bulk_create_with_conn(db, vec![draft], None, true, false)
		.await?;
	let stored = AgentKnowledge::objects()
		.filter(AgentKnowledge::field_agent_key().eq(entry.id.clone()))
		.filter(AgentKnowledge::field_agent_version().eq(entry.version.clone()))
		.get_with_db(db)
		.await?;
	if stored.documents.0 != documents {
		return Err(Error::Conflict(
			"private documents are immutable; choose a new version".into(),
		));
	}
	Ok(())
}

use reinhardt::db::orm::execution::convert_values;
use reinhardt::query::{
	Alias, Expr, OnConflict, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
