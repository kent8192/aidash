use crate::capabilities::records::{self, Record};
use crate::{Error, Result, authorization::access::Access, domain::Run};
use chrono::{DateTime, Utc};
use sea_orm::sea_query::{Alias, Expr, LockType, OnConflict, PostgresQueryBuilder, Query};
use serde_json::{Value, json};
use uuid::Uuid;

pub(crate) fn select(table: &str) -> sea_orm::sea_query::SelectStatement {
	let mut query = Query::select();
	query
		.column(sea_orm::sea_query::Asterisk)
		.from(Alias::new(table));
	query
}
pub(crate) async fn message_dependency(
	access: &mut Access,
	run: &Run,
	message: Uuid,
) -> Result<()> {
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("authorization_run_reads"))
			.columns(["run_id", "workspace_id", "resource_kind", "resource_id"].map(Alias::new))
			.values_panic([
				Expr::cust("$1"),
				Expr::cust("$2"),
				Expr::val("message").into(),
				Expr::cust("$3"),
			])
			.on_conflict(OnConflict::new().do_nothing().to_owned())
			.to_string(PostgresQueryBuilder),
	)
	.bind(run.id)
	.bind(run.workspace_id)
	.bind(message)
	.execute(&mut **access.tx)
	.await?;
	Ok(())
}
pub(crate) async fn state(access: &mut Access, run: &Run) -> Result<Value> {
	sqlx::query(&Query::insert().into_table(Alias::new("web_runs"))
		.columns(["run_id", "tenant", "owner", "data"].map(Alias::new))
		.values_panic((1..=4).map(|i| Expr::cust(format!("${i}"))))
		.on_conflict(OnConflict::column(Alias::new("run_id")).do_nothing().to_owned())
		.to_string(PostgresQueryBuilder))
		.bind(run.id).bind(&access.identity.tenant).bind(&access.identity.subject)
		.bind(json!({"search_attempts":0,"page_attempts":0,"estimated_micro_usd":0,
			"cache_bytes":0,"observation_count":0,"observation_bytes":0,"classification":"unclassified","citation_corrections":0,
			"retention_until":Utc::now()+chrono::Duration::days(90)}))
		.execute(&mut **access.tx).await?;
	let data: Value = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("data"))
			.from(Alias::new("web_runs"))
			.and_where(Expr::cust("run_id=$1 AND tenant=$2"))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.bind(run.id)
	.bind(&access.identity.tenant)
	.fetch_one(&mut **access.tx)
	.await?;
	Ok(data)
}
pub(crate) async fn save_state(access: &mut Access, run: &Run, data: &Value) -> Result<()> {
	sqlx::query(
		&Query::update()
			.table(Alias::new("web_runs"))
			.value(Alias::new("data"), Expr::cust("$2"))
			.and_where(Expr::col(Alias::new("run_id")).eq(Expr::cust("$1")))
			.to_string(PostgresQueryBuilder),
	)
	.bind(run.id)
	.bind(data)
	.execute(&mut **access.tx)
	.await?;
	Ok(())
}
pub(crate) async fn existing(access: &mut Access, run: &Run, key: &str) -> Result<Option<Record>> {
	let id: Option<Uuid> = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("operation_id"))
			.from(Alias::new("web_invocations"))
			.and_where(Expr::cust("run_id=$1 AND invocation_key=$2"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(run.id)
	.bind(key)
	.fetch_optional(&mut **access.tx)
	.await?;
	match id {
		Some(id) => Ok(Some(records::get(access, id, "web.operation").await?)),
		None => Ok(None),
	}
}
pub(crate) async fn insert_operation(
	access: &mut Access,
	run: &Run,
	key: &str,
	data: Value,
) -> Result<Record> {
	let record = records::insert(
		access,
		Uuid::new_v4(),
		None,
		"web.operation",
		"prepared",
		data,
		None,
	)
	.await?;
	sqlx::query(
		&Query::insert()
			.into_table(Alias::new("web_invocations"))
			.columns(["run_id", "invocation_key", "operation_id"].map(Alias::new))
			.values_panic((1..=3).map(|i| Expr::cust(format!("${i}"))))
			.to_string(PostgresQueryBuilder),
	)
	.bind(run.id)
	.bind(key)
	.bind(record.id)
	.execute(&mut **access.tx)
	.await?;
	Ok(record)
}
pub(crate) async fn owned(access: &mut Access, run: &Run, id: Uuid, kind: &str) -> Result<Record> {
	let record = records::get(access, id, kind).await?;
	if record.data["run_id"] != json!(run.id) || record.state == "revoked" {
		return Err(Error::NotFound("web evidence unavailable".into()));
	}
	Ok(record)
}
pub(crate) async fn fingerprint(access: &mut Access, run: &Run) -> Result<String> {
	let inputs: i64 = sqlx::query_scalar(
		&Query::select()
			.expr(Expr::cust("COALESCE(MAX(seq),0)"))
			.from(Alias::new("run_inputs"))
			.and_where(Expr::cust("run_id=$1"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(run.id)
	.fetch_one(&mut **access.tx)
	.await?;
	let reads: Value = sqlx::query_scalar(&Query::select().expr(Expr::cust(
		"COALESCE(jsonb_agg(jsonb_build_array(resource_kind, resource_id, workspace_id) ORDER BY resource_kind, resource_id),'[]'::jsonb)"))
		.from(Alias::new("authorization_run_reads")).and_where(Expr::cust("run_id=$1"))
		.to_string(PostgresQueryBuilder)).bind(run.id).fetch_one(&mut **access.tx).await?;
	let observed: Option<Value> = sqlx::query_scalar(
		&Query::select()
			.expr(Expr::cust("data->'observed_context'"))
			.from(Alias::new("web_runs"))
			.and_where(Expr::cust("run_id=$1"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(run.id)
	.fetch_optional(&mut **access.tx)
	.await?
	.flatten();
	let mut context: Value = sqlx::query_scalar(
		&Query::select()
			.column(Alias::new("context"))
			.from(Alias::new("runs"))
			.and_where(Expr::cust("id=$1"))
			.to_string(PostgresQueryBuilder),
	)
	.bind(run.id)
	.fetch_one(&mut **access.tx)
	.await?;
	let normalized: crate::context::Context = serde_json::from_value(context)?;
	context = json!({"summary":normalized.summary,"history":normalized.history,
		"run_message_summary":normalized.run_message_summary,"run_message_summary_seq":normalized.run_message_summary_seq,
		"compactions":normalized.compactions});
	if let Some(history) = context["history"].as_array_mut() {
		history.retain(|entry| {
			!(entry["kind"] == "tool"
				&& super::contracts::is_web(entry["call"]["name"].as_str().unwrap_or("")))
		});
	}
	Ok(crate::registry::digest(&json!([
		"web-context/1",
		run.id,
		run.agent_id,
		run.agent_version,
		inputs,
		reads,
		context,
		observed
	])))
}
pub(crate) fn expired(record: &Record) -> bool {
	record.expires_at.is_some_and(|t| t <= Utc::now())
}
pub(crate) fn date(value: &Value) -> Result<DateTime<Utc>> {
	serde_json::from_value(value.clone())
		.map_err(|_| Error::Conflict("web operation deadline unavailable".into()))
}
