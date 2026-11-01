//! A committed send marker prevents a lost peer response from replaying an external effect.
use super::*;
use crate::apps::identity::models::RemoteConnectionCall;
use aidash_application::tenancy::TenancyScope;
use reinhardt::db::orm::{Model, execution::convert_values};
use reinhardt::query::{
	Alias, Expr, ExprTrait, IntoValue, OnConflict, PostgresQueryBuilder, Query,
	QueryStatementBuilder,
};

pub(super) enum Start {
	Send(Uuid),
	Replay(Value),
}
fn scope(owner: &Owner) -> Result<TenancyScope> {
	TenancyScope::tenant(&owner.tenant)
}
fn identity(call: &ConnectionCall) -> Result<Value> {
	Ok(
		json!({"tool_key":call.tool_key,"tool_name":call.tool_name,"kind":call.kind,"secret_name":call.secret_name}),
	)
}
pub(super) async fn start(
	store: &crate::store::Store,
	owner: &Owner,
	grant: Uuid,
	admission: Uuid,
	call: &ConnectionCall,
) -> Result<Start> {
	let mut tx = store
		.control_pool
		.begin(&scope(owner)?)
		.await?
		.into_executor()
		.await?;
	let id = Uuid::now_v7();
	let tool_key = identity(call)?;
	let hash = aidash_domain::registry::rules::digest(&call.arguments);
	let now = chrono::Utc::now();
	let (sql, values) = Query::insert()
		.into_table(Alias::new("remote_connection_calls"))
		.columns(
			[
				"id",
				"tenant",
				"grant_id",
				"admission_id",
				"idempotency_key",
				"tool_key",
				"resource",
				"arguments_hash",
				"status",
				"created_at",
				"updated_at",
			]
			.map(Alias::new),
		)
		.values_panic([
			id.into_value(),
			owner.tenant.clone().into_value(),
			grant.into_value(),
			admission.into_value(),
			call.idempotency_key.clone().into_value(),
			tool_key.clone().into_value(),
			call.resource.clone().into_value(),
			hash.clone().into_value(),
			"started".into_value(),
			now.into_value(),
			now.into_value(),
		])
		.on_conflict(
			OnConflict::columns(["grant_id", "idempotency_key"])
				.do_nothing()
				.to_owned(),
		)
		.returning_all()
		.build(PostgresQueryBuilder);
	let inserted = tx
		.fetch_optional(&sql, convert_values(values))
		.await
		.map_err(crate::Error::from)?;
	if inserted.is_some() {
		tx.commit().await.map_err(crate::Error::from)?;
		return Ok(Start::Send(id));
	}
	let row = RemoteConnectionCall::objects()
		.filter(RemoteConnectionCall::field_grant_id().eq(grant))
		.filter(RemoteConnectionCall::field_idempotency_key().eq(call.idempotency_key.clone()))
		.all_with_executor(tx.as_mut())
		.await
		.map_err(reinhardt::core::exception::Error::from)
		.map_err(crate::Error::from)?
		.pop()
		.ok_or(Error::Forbidden)?;
	if row.admission_id != admission
		|| row.tool_key.0 != tool_key
		|| row.resource != call.resource
		|| row.arguments_hash != hash
	{
		return Err(Error::Conflict(
			"remote call key binds different input".into(),
		));
	}
	let value = if row.status == "started" {
		json!({"error":"mcp_call_outcome_unknown"})
	} else {
		// The ORM's optional JSON projection cannot distinguish a JSON null
		// result from SQL NULL. The status/CHECK constraint preserves that distinction.
		row.result.map_or(Value::Null, |result| result.0)
	};
	tx.commit().await.map_err(crate::Error::from)?;
	Ok(Start::Replay(value))
}
pub(super) async fn authentication(
	store: &crate::store::Store,
	owner: &Owner,
	id: Uuid,
	auth: &CallSecret,
) -> Result<()> {
	let mut query = Query::update();
	query
		.table(Alias::new("remote_connection_calls"))
		.and_where(Expr::col("id").eq(Expr::value(id)));
	match auth {
		// Cloud and Dedicated resolve the same OAuth Connection through the broker.
		CallSecret::Connection { record, .. } | CallSecret::BrokeredConnection { record, .. } => {
			query.value(Alias::new("connection_id"), record.id);
		}
		_ => {
			if let Some(record) = auth.record() {
				query
					.value(Alias::new("secret_id"), record.id)
					.value(Alias::new("secret_version"), record.version.clone())
					.value(Alias::new("secret_host"), record.host.clone());
			}
		}
	}
	let (sql, values) = query.build(PostgresQueryBuilder);
	let mut tx = store
		.control_pool
		.begin(&scope(owner)?)
		.await?
		.into_executor()
		.await?;
	tx.execute(&sql, convert_values(values))
		.await
		.map_err(crate::Error::from)?;
	tx.commit().await.map_err(crate::Error::from)?;
	Ok(())
}
pub(super) async fn finish(
	store: &crate::store::Store,
	owner: &Owner,
	id: Uuid,
	value: &Value,
	failed: bool,
) -> Result<()> {
	let mut tx = store
		.control_pool
		.begin(&scope(owner)?)
		.await?
		.into_executor()
		.await?;
	let (sql, values) = Query::update()
		.table(Alias::new("remote_connection_calls"))
		.value(
			Alias::new("status"),
			if failed { "failed" } else { "completed" },
		)
		.value(Alias::new("result"), value.clone())
		.value(Alias::new("updated_at"), chrono::Utc::now())
		.and_where(Expr::col("id").eq(Expr::value(id)))
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values))
		.await
		.map_err(crate::Error::from)?;
	tx.commit().await.map_err(crate::Error::from)?;
	Ok(())
}

pub(super) async fn abandon(store: &crate::store::Store, owner: &Owner, id: Uuid) -> Result<()> {
	let mut tx = store
		.control_pool
		.begin(&scope(owner)?)
		.await?
		.into_executor()
		.await?;
	let (sql, values) = Query::delete()
		.from_table(Alias::new("remote_connection_calls"))
		.and_where(Expr::col("id").eq(Expr::value(id)))
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values))
		.await
		.map_err(crate::Error::from)?;
	tx.commit().await.map_err(crate::Error::from)?;
	Ok(())
}
