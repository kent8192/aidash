//! The Tenant's stored Agent Plugin copies (ADR 0072). A copy is addressed by
//! its tree digest and never updated: registration and Run admission compare
//! what an Agent definition pins with what the row holds, so a changed copy
//! would invalidate definitions registered earlier.
use crate::apps::registry::models::AgentPluginCopy;
use crate::{Error, Result};
use aidash_application::ports::bindings::StoredAgentPlugin;
use aidash_domain::agent_plugin::{LoadedPlugin, PluginFile, tree_digest};
use base64::Engine;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{Model, OrmExecutor, QueryRow};
use reinhardt::query::{
	Alias, Expr, ExprTrait, OnConflict, PostgresQueryBuilder, Query, QueryStatementBuilder,
	SimpleExpr,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Encoding {
	Utf8,
	Base64,
}

/// One fetched file as `files` keeps it. Text stays text; anything else is base64.
#[derive(Serialize, Deserialize)]
struct StoredFile {
	path: String,
	encoding: Encoding,
	content: String,
}

impl StoredFile {
	fn of(file: &PluginFile) -> Self {
		match std::str::from_utf8(&file.bytes) {
			Ok(text) if !text.contains('\0') => Self {
				path: file.path.clone(),
				encoding: Encoding::Utf8,
				content: text.to_owned(),
			},
			_ => Self {
				path: file.path.clone(),
				encoding: Encoding::Base64,
				content: base64::engine::general_purpose::STANDARD.encode(&file.bytes),
			},
		}
	}
}

fn invalid(reason: &str) -> Error {
	Error::Invalid(format!("AGENT_PLUGIN_INVALID: {reason}"))
}

/// The files as stored: sorted by path, the order the tree digest uses.
fn stored_files(files: &[PluginFile]) -> Result<Value> {
	let mut stored: Vec<StoredFile> = files.iter().map(StoredFile::of).collect();
	stored.sort_by(|a, b| a.path.cmp(&b.path));
	Ok(serde_json::to_value(stored)?)
}

/// What registration and admission read: not the files, which only a stdio
/// server needs.
#[derive(Deserialize)]
struct StoredCopy {
	source: String,
	plugin: LoadedPlugin,
}

fn select(tenant: SimpleExpr, digest: &str) -> (String, reinhardt::query::Values) {
	Query::select()
		.columns(["source", "plugin"].map(Alias::new))
		.from(Alias::new(AgentPluginCopy::table_name()))
		.and_where(Expr::col("tenant").eq(tenant))
		.and_where(Expr::col("digest").eq(Expr::value(digest)))
		.build(PostgresQueryBuilder)
}

fn stored(row: reinhardt::db::backends::Row) -> Result<StoredAgentPlugin> {
	let copy: StoredCopy = serde_json::from_value(QueryRow::from_backend_row(row).data)?;
	Ok(StoredAgentPlugin {
		source: copy.source,
		plugin: copy.plugin,
	})
}

/// The copy of the transaction's Tenant scope with this digest. Outside a Tenant
/// scope there is no Tenant, so no row matches.
pub async fn get(
	tx: &mut dyn TransactionExecutor,
	digest: &str,
) -> Result<Option<StoredAgentPlugin>> {
	let (sql, values) = select(crate::database::scope::tenant().into(), digest);
	tx.fetch_optional(&sql, convert_values(values))
		.await?
		.map(stored)
		.transpose()
}

/// The copy of a named Tenant, for a scope that already knows it.
pub async fn get_of<E: OrmExecutor>(
	db: &mut E,
	tenant: &str,
	digest: &str,
) -> Result<Option<StoredAgentPlugin>> {
	db.fetch_optional_generated(select(Expr::value(tenant).into(), digest), None)
		.await?
		.map(stored)
		.transpose()
}

/// Keeps the import in the transaction's Tenant scope and returns the stored
/// copy. A copy with the same digest stays as it is, so a later change to how
/// plugins load never alters what earlier Agent definitions pinned.
pub async fn insert(
	tx: &mut dyn TransactionExecutor,
	source: &str,
	plugin: &LoadedPlugin,
	files: &[PluginFile],
	denied: &[String],
) -> Result<StoredAgentPlugin> {
	if plugin.digest != tree_digest(files) {
		return Err(invalid("the files do not match the plugin digest"));
	}
	let (sql, values) = Query::insert()
		.into_table(Alias::new(AgentPluginCopy::table_name()))
		.columns(
			[
				"tenant", "digest", "name", "source", "plugin", "files", "denied",
			]
			.map(Alias::new),
		)
		.from_subquery(
			Query::select()
				.expr(crate::database::scope::tenant())
				.expr(Expr::value(&plugin.digest))
				.expr(Expr::value(plugin.manifest.name.as_str()))
				.expr(Expr::value(source))
				.expr(Expr::value(serde_json::to_value(plugin)?))
				.expr(Expr::value(stored_files(files)?))
				.expr(Expr::value(serde_json::to_value(denied)?))
				.to_owned(),
		)
		.on_conflict(
			OnConflict::columns(["tenant", "digest"].map(Alias::new))
				.do_nothing()
				.to_owned(),
		)
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	let stored = get(tx, &plugin.digest)
		.await?
		.ok_or_else(|| Error::NotFound("Agent Plugin copy".into()))?;
	if stored.source != source
		&& (stored.source.starts_with("marketplace://") || source.starts_with("marketplace://"))
	{
		return Err(Error::Conflict(
			"AGENT_PLUGIN_SOURCE_CONFLICT: digest is already pinned to another source".into(),
		));
	}
	Ok(stored)
}
