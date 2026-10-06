//! Native persistence for immutable registry versions and local installations.
use super::states::DefinitionKind;
use super::{AgentKnowledge, Definition, Installation, Package, RegistryRequest};
use crate::apps::registry::serializers::contracts::{Entry, PackageRecord};
use crate::{Error, Result};
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::orm::{AtomicTransaction, DatabaseConnection, Model, OrmExecutor};
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

pub(crate) async fn definitions(
	mut db: DatabaseConnection,
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
	Ok(query.all_with_db(&mut db).await?)
}

pub(crate) async fn installation<E: OrmExecutor>(
	db: &mut E,
	id: &str,
	version: &str,
) -> Result<Option<Installation>> {
	Ok(Installation::objects()
		.filter(Installation::field_id().eq(id.to_owned()))
		.filter(Installation::field_version().eq(version.to_owned()))
		.first_with_db(db)
		.await?)
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

pub(crate) async fn package<E: OrmExecutor>(
	db: &mut E,
	id: &str,
	version: &str,
) -> Result<Package> {
	Package::objects()
		.filter(Package::field_id().eq(id.to_owned()))
		.filter(Package::field_version().eq(version.to_owned()))
		.first_with_db(db)
		.await?
		.ok_or_else(|| Error::NotFound("package".into()))
}

pub(crate) async fn packages(mut db: DatabaseConnection) -> Result<Vec<PackageRecord>> {
	Ok(Package::objects()
		.all()
		.order_by(&["id", "version"])
		.all_with_db(&mut db)
		.await?
		.into_iter()
		.map(package_record)
		.collect())
}

pub(crate) fn package_record(stored: Package) -> PackageRecord {
	PackageRecord {
		id: stored.id,
		version: stored.version,
		manifest: stored.manifest.0,
		digest: stored.digest,
	}
}

pub(crate) async fn publish(
	db: &mut AtomicTransaction,
	id: &str,
	version: &str,
	manifest: Value,
	digest: &str,
	manifest_source: &str,
) -> Result<(PackageRecord, bool)> {
	let (sql, values) = Query::insert()
		.into_table(Alias::new("packages"))
		.columns(["id", "version", "manifest", "digest", "manifest_source"].map(Alias::new))
		.from_subquery(
			reinhardt::query::Query::select()
				.expr(Expr::value(id))
				.expr(Expr::value(version))
				.expr(Expr::value(manifest))
				.expr(Expr::value(digest))
				.expr(Expr::value(manifest_source))
				.to_owned(),
		)
		.on_conflict(
			OnConflict::columns(["id", "version"])
				.do_nothing()
				.to_owned(),
		)
		.build(PostgresQueryBuilder);
	let inserted = OrmExecutor::execute(db, &sql, convert_values(values))
		.await?
		.rows_affected
		== 1;
	let stored = package(db, id, version).await?;
	if stored.digest != digest || stored.manifest_source != manifest_source {
		return Err(Error::Conflict("package version is immutable".into()));
	}
	Ok((package_record(stored), inserted))
}

/// All overlays serialize on the immutable definition, including first install.
pub(crate) async fn install(
	db: &mut AtomicTransaction,
	entry: &Entry,
	digest: &str,
	config: Value,
) -> Result<bool> {
	insert_definition(db, entry).await?;
	let _definition = Definition::objects()
		.filter(Definition::field_id().eq(entry.id.clone()))
		.filter(Definition::field_version().eq(entry.version.clone()))
		.select_for_update()
		.all_with_executor(db)
		.await
		.map_err(FrameworkError::from)?
		.pop()
		.ok_or_else(|| Error::NotFound("package definition".into()))?;
	if let Some(mut stored) = installation(db, &entry.id, &entry.version).await? {
		if stored.config.0 == config {
			return Ok(false);
		}
		stored.config = config.into();
		// Reinhardt #6458: the logical version is part of the primary key.
		// Return to Manager::update_with_conn after composite updates are verified.
		let updated = Installation::objects()
			.filter(Installation::field_id().eq(&stored.id))
			.filter(Installation::field_version().eq(&stored.version))
			.update_fields_with_conn(db, [(Installation::field_config(), stored.config)])
			.await?;
		if updated != 1 {
			return Err(Error::Conflict("installation changed".into()));
		}
	} else {
		let draft = Installation::build()
			.id(&entry.id)
			.version(&entry.version)
			.digest(digest)
			.config(config.into())
			.finish();
		Installation::objects().create_with_conn(db, &draft).await?;
	}
	Ok(true)
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
