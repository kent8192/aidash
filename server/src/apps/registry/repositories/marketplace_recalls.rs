//! Append-only Deployment-wide knowledge; a disconnected Connection never
//! deletes a previously learned stop, and Run admission never contacts catalog.
use crate::apps::registry::models::MarketplacePluginRecall;
use crate::{Error, Result};
use aidash_domain::{agent_plugin::MarketplaceSource, marketplace::distribution::Recall};
use reinhardt::db::{
	backends::TransactionExecutor,
	orm::{Model, OrmExecutor, execution::convert_values},
};
use reinhardt::query::{
	Alias, Expr, ExprTrait, OnConflict, PostgresQueryBuilder, Query, QueryStatementBuilder,
};

pub async fn retain(tx: &mut dyn TransactionExecutor, recalls: &[Recall]) -> Result<()> {
	for recall in recalls {
		if MarketplaceSource::parse(&format!(
			"marketplace://{}/{}",
			recall.plugin_id, recall.version
		))
		.is_none() || !recall.digest.strip_prefix("sha256:").is_some_and(|hex| {
			hex.len() == 64
				&& hex
					.bytes()
					.all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
		}) || recall.reason.len() > 4096
		{
			return Err(Error::Invalid("invalid Marketplace Recall".into()));
		}
		let (sql, values) = Query::insert()
			.into_table(Alias::new(MarketplacePluginRecall::table_name()))
			.columns(["plugin_id", "version", "digest", "reason", "recalled_at"].map(Alias::new))
			.values_panic::<_, reinhardt::query::Value>([
				recall.plugin_id.into(),
				recall.version.clone().into(),
				recall.digest.clone().into(),
				recall.reason.clone().into(),
				recall.recalled_at.into(),
			])
			.on_conflict(
				OnConflict::columns(["plugin_id", "version"].map(Alias::new))
					.do_nothing()
					.to_owned(),
			)
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values)).await?;
	}
	Ok(())
}

pub async fn receipt(
	tx: &mut dyn TransactionExecutor,
	source: &MarketplaceSource,
	digest: &str,
	expires: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<()> {
	let uri = format!("marketplace://{}/{}", source.plugin_id, source.version);
	let (sql, values) = Query::update()
		.table(Alias::new("agent_plugin_copies"))
		.value(Alias::new("entitlement_expires_at"), expires)
		.and_where(Expr::col("tenant").eq(crate::database::scope::tenant()))
		.and_where(Expr::col("digest").eq(Expr::value(digest)))
		.and_where(Expr::col("source").eq(Expr::value(uri)))
		.build(PostgresQueryBuilder);
	if tx
		.execute(&sql, convert_values(values))
		.await?
		.rows_affected
		!= 1
	{
		return Err(Error::Conflict(
			"Marketplace acquisition copy provenance differs".into(),
		));
	}
	Ok(())
}

fn pins(entry: &crate::registry::Entry) -> Vec<(MarketplaceSource, String)> {
	entry
		.config
		.get("agent_plugins")
		.and_then(serde_json::Value::as_array)
		.into_iter()
		.flatten()
		.filter_map(|plugin| {
			Some((
				MarketplaceSource::parse(plugin.get("source")?.as_str()?)?,
				plugin.get("digest")?.as_str()?.to_owned(),
			))
		})
		.collect()
}
fn acquisition_query(
	tenant: reinhardt::query::SimpleExpr,
	digest: &str,
) -> (String, reinhardt::query::Values) {
	Query::select()
		.column(Alias::new("entitlement_expires_at"))
		.from(Alias::new("agent_plugin_copies"))
		.and_where(Expr::col("tenant").eq(tenant))
		.and_where(Expr::col("digest").eq(Expr::value(digest)))
		.limit(1)
		.build(PostgresQueryBuilder)
}
fn acquired(row: Option<reinhardt::db::backends::Row>) -> Result<()> {
	use reinhardt::db::orm::QueryRow;
	let row = row.ok_or_else(|| Error::Invalid("AGENT_PLUGIN_MISSING: no acquired copy".into()))?;
	let value = QueryRow::from_backend_row(row).data;
	let expires: Option<chrono::DateTime<chrono::Utc>> = serde_json::from_value(
		value
			.get("entitlement_expires_at")
			.cloned()
			.unwrap_or(serde_json::Value::Null),
	)?;
	if expires.is_some_and(|deadline| deadline <= chrono::Utc::now()) {
		return Err(Error::Forbidden);
	}
	Ok(())
}
/// New Agent versions use the receipt, without changing existing local copies.
pub async fn check_acquisition<E: OrmExecutor>(
	db: &mut E,
	tenant: &str,
	entry: &crate::registry::Entry,
) -> Result<()> {
	for (_, digest) in pins(entry) {
		acquired(
			db.fetch_optional_generated(
				acquisition_query(Expr::value(tenant).into(), &digest),
				None,
			)
			.await?,
		)?;
	}
	Ok(())
}
pub async fn check_acquisition_native(
	tx: &mut dyn TransactionExecutor,
	entry: &crate::registry::Entry,
) -> Result<()> {
	for (_, digest) in pins(entry) {
		let (sql, values) = acquisition_query(crate::database::scope::tenant().into(), &digest);
		acquired(tx.fetch_optional(&sql, convert_values(values)).await?)?;
	}
	Ok(())
}
/// New Runs only. Continuation of an admitted Run does not call this method.
pub async fn check(tx: &mut dyn TransactionExecutor, entry: &crate::registry::Entry) -> Result<()> {
	for (source, _) in pins(entry) {
		let (sql, values) = Query::select()
			.column(Alias::new("plugin_id"))
			.from(Alias::new(MarketplacePluginRecall::table_name()))
			.and_where(Expr::col("plugin_id").eq(Expr::value(source.plugin_id)))
			.and_where(Expr::col("version").eq(Expr::value(source.version)))
			.limit(1)
			.build(PostgresQueryBuilder);
		if tx
			.fetch_optional(&sql, convert_values(values))
			.await?
			.is_some()
		{
			return Err(Error::Forbidden);
		}
	}
	check_acquisition_native(tx, entry).await
}
