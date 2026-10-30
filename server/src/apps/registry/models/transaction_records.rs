//! Immutable definitions on a caller-owned native transaction. These reads and
//! writes reach only the Registry of the Tenant whose scope the transaction was
//! opened in (`aidash.tenant`), the condition row-level security enforces for
//! Tenant-owned tables (#265); outside a Tenant scope they see nothing.
use super::{AgentKnowledge, AgentKnowledgeImage, Definition};
use crate::apps::registry::serializers::contracts::Entry;
use crate::{Error, Result};
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

#[cfg(test)]
mod image_storage_tests {
	use super::*;
	use crate::apps::execution::test_database::{DatabaseFixture, database};
	use base64::{Engine as _, engine::general_purpose::STANDARD};
	use rstest::rstest;

	#[rstest]
	#[tokio::test]
	async fn image_data_is_binary_and_idempotent(#[future] database: DatabaseFixture) {
		let database = database.await;
		let mut tx = database.begin_fixture().await;
		// A temporary row exercises the actual adapter without manufacturing a Registry identity.
		tx.execute("CREATE TEMP TABLE agent_knowledge (tenant text, agent_id text, agent_version text, PRIMARY KEY (tenant, agent_id, agent_version))", vec![]).await.unwrap();
		tx.execute("CREATE TEMP TABLE agent_knowledge_images (tenant text, agent_id text, agent_version text, document_index integer, media_type text, size bigint, sha256 text, bytes bytea, PRIMARY KEY (tenant, agent_id, agent_version, document_index), FOREIGN KEY (tenant, agent_id, agent_version) REFERENCES agent_knowledge (tenant, agent_id, agent_version) ON DELETE CASCADE)", vec![]).await.unwrap();
		let tenant = scope_tenant(&mut *tx).await.unwrap();
		let entry: Entry = serde_json::from_value(serde_json::json!({"id":"source", "version":"1.0.0", "kind":"source", "name":{"en":"Source"}, "description":{"en":"Source"}, "config":{}})).unwrap();
		let (sql, values) = Query::insert()
			.into_table(Alias::new("agent_knowledge"))
			.columns(["tenant", "agent_id", "agent_version"].map(Alias::new))
			.values_panic([
				reinhardt::query::IntoValue::into_value(tenant),
				reinhardt::query::IntoValue::into_value(entry.id.clone()),
				reinhardt::query::IntoValue::into_value(entry.version.clone()),
			])
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await.unwrap();
		let bytes = b"\x89PNG\r\n\x1a\n\x00binary";
		let data = serde_json::json!([{"document_index":2,"media_type":"image/png","size":bytes.len(),"sha256":aidash_domain::semantic::indexing::content_digest_bytes(bytes),"data_base64":STANDARD.encode(bytes)}]);
		insert_image_data(&mut *tx, &entry, data.clone())
			.await
			.unwrap();
		insert_image_data(&mut *tx, &entry, data).await.unwrap();
		let row = tx.fetch_one("SELECT encode(bytes, 'hex') AS bytes, pg_typeof(bytes)::text AS storage FROM agent_knowledge_images WHERE document_index = 2", vec![]).await.unwrap();
		let row = QueryRow::from_backend_row(row).data;
		assert_eq!(row["storage"], "bytea");
		assert_eq!(
			row["bytes"],
			bytes
				.iter()
				.map(|byte| format!("{byte:02x}"))
				.collect::<String>()
		);
		let changed = serde_json::json!([{"document_index":2,"media_type":"image/png","size":9,"sha256":aidash_domain::semantic::indexing::content_digest_bytes(b"different"),"data_base64":STANDARD.encode(b"different")}]);
		assert!(matches!(
			insert_image_data(&mut *tx, &entry, changed).await,
			Err(Error::Conflict(_))
		));
		tx.execute("DELETE FROM agent_knowledge", vec![])
			.await
			.unwrap();
		let row = tx
			.fetch_one(
				"SELECT count(*) AS count FROM agent_knowledge_images",
				vec![],
			)
			.await
			.unwrap();
		assert_eq!(QueryRow::from_backend_row(row).data["count"], 0);
	}
}

pub(crate) async fn scope_tenant(tx: &mut dyn TransactionExecutor) -> Result<String> {
	let row = tx
		.fetch_one(&format!("SELECT {SCOPE_TENANT} AS tenant"), vec![])
		.await?;
	let value = QueryRow::from_backend_row(row).data;
	Ok(value["tenant"].as_str().unwrap_or_default().to_owned())
}

impl Definition {
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
	private_column(tx, entry, "documents").await
}

pub(crate) async fn image_index(
	tx: &mut dyn TransactionExecutor,
	entry: &Entry,
) -> Result<Option<Value>> {
	private_column(tx, entry, "image_index").await
}

async fn private_column(
	tx: &mut dyn TransactionExecutor,
	entry: &Entry,
	column: &str,
) -> Result<Option<Value>> {
	let (sql, values) = Query::select()
		.column(Alias::new(column))
		.from(Alias::new(AgentKnowledge::table_name()))
		.and_where(Expr::col("tenant").eq(Expr::cust(SCOPE_TENANT)))
		.and_where(Expr::col("agent_id").eq(Expr::value(&entry.id)))
		.and_where(Expr::col("agent_version").eq(Expr::value(&entry.version)))
		.build(PostgresQueryBuilder);
	Ok(tx
		.fetch_optional(&sql, convert_values(values))
		.await?
		.and_then(|row| QueryRow::from_backend_row(row).data.get(column).cloned())
		.filter(|value| !value.is_null()))
}

pub(crate) async fn insert_image_data(
	tx: &mut dyn TransactionExecutor,
	entry: &Entry,
	data: Value,
) -> Result<()> {
	use base64::{Engine as _, engine::general_purpose::STANDARD};
	let tenant = scope_tenant(tx).await?;
	let images = data
		.as_array()
		.ok_or_else(|| Error::Invalid("invalid private images".into()))?;
	let mut seen = std::collections::HashSet::new();
	for image in images {
		let index = image["document_index"]
			.as_u64()
			.and_then(|index| i32::try_from(index).ok())
			.ok_or_else(|| Error::Invalid("invalid private image index".into()))?;
		if !seen.insert(index) {
			return Err(Error::Invalid("duplicate private image index".into()));
		}
		let encoded = image["data_base64"]
			.as_str()
			.ok_or_else(|| Error::Invalid("invalid private image data".into()))?;
		let bytes = STANDARD
			.decode(encoded)
			.map_err(|_| Error::Invalid("invalid private image data".into()))?;
		let media_type = image["media_type"]
			.as_str()
			.filter(|value| value.starts_with("image/"))
			.ok_or_else(|| Error::Invalid("invalid private image media type".into()))?;
		let size = i64::try_from(bytes.len())
			.map_err(|_| Error::Invalid("invalid private image size".into()))?;
		let digest = aidash_domain::semantic::indexing::content_digest_bytes(&bytes);
		if image["size"].as_i64() != Some(size) || image["sha256"].as_str() != Some(digest.as_str())
		{
			return Err(Error::Invalid(
				"private image metadata does not match bytes".into(),
			));
		}
		let (sql, values) = Query::insert()
			.into_table(Alias::new(AgentKnowledgeImage::table_name()))
			.columns(
				[
					"tenant",
					"agent_id",
					"agent_version",
					"document_index",
					"media_type",
					"size",
					"sha256",
					"bytes",
				]
				.map(Alias::new),
			)
			.values_panic([
				reinhardt::query::IntoValue::into_value(tenant.clone()),
				reinhardt::query::IntoValue::into_value(entry.id.clone()),
				reinhardt::query::IntoValue::into_value(entry.version.clone()),
				reinhardt::query::IntoValue::into_value(index),
				reinhardt::query::IntoValue::into_value(media_type.to_owned()),
				reinhardt::query::IntoValue::into_value(size),
				reinhardt::query::IntoValue::into_value(digest.clone()),
				reinhardt::query::IntoValue::into_value(bytes.clone()),
			])
			.on_conflict(
				OnConflict::columns(
					["tenant", "agent_id", "agent_version", "document_index"].map(Alias::new),
				)
				.do_nothing()
				.to_owned(),
			)
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
		let (sql, values) = Query::select()
			.columns(["media_type", "size", "sha256"].map(Alias::new))
			.expr_as(Expr::cust("encode(bytes, 'hex')"), Alias::new("image_hex"))
			.from(Alias::new(AgentKnowledgeImage::table_name()))
			.and_where(Expr::col("tenant").eq(Expr::value(&tenant)))
			.and_where(Expr::col("agent_id").eq(Expr::value(&entry.id)))
			.and_where(Expr::col("agent_version").eq(Expr::value(&entry.version)))
			.and_where(Expr::col("document_index").eq(index))
			.build(PostgresQueryBuilder);
		let stored = tx
			.fetch_optional(&sql, convert_values(values))
			.await?
			.map(|row| QueryRow::from_backend_row(row).data);
		let expected: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
		if !stored.as_ref().is_some_and(|row| {
			row["image_hex"].as_str() == Some(&expected)
				&& row["media_type"].as_str() == Some(media_type)
				&& row["size"].as_i64() == Some(size)
				&& row["sha256"].as_str() == Some(&digest)
		}) {
			return Err(Error::Conflict(
				"private image data is immutable; choose a new version".into(),
			));
		}
	}

	Ok(())
}

/// Attach vectors exactly once to the private row inside its caller's transaction.
pub(crate) async fn insert_image_index(
	tx: &mut dyn TransactionExecutor,
	entry: &Entry,
	index: Value,
) -> Result<()> {
	let tenant = scope_tenant(tx).await?;
	let (sql, values) = Query::update()
		.table(Alias::new(AgentKnowledge::table_name()))
		.value(Alias::new("image_index"), Expr::value(index.clone()))
		.and_where(Expr::col("tenant").eq(Expr::value(&tenant)))
		.and_where(Expr::col("agent_id").eq(Expr::value(&entry.id)))
		.and_where(Expr::col("agent_version").eq(Expr::value(&entry.version)))
		.and_where(Expr::col("image_index").is_null())
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	if image_index(tx, entry).await? != Some(index) {
		return Err(Error::Conflict(
			"private image index is immutable; choose a new version".into(),
		));
	}
	Ok(())
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
	let stored = self::documents(tx, entry)
		.await?
		.ok_or_else(|| Error::NotFound("agent knowledge".into()))?;
	if stored != documents {
		return Err(Error::Conflict(
			"private documents are immutable; choose a new version".into(),
		));
	}
	Ok(())
}
