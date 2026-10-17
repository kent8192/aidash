//! Recording, checking and undoing Tenant Deletions (ADR 0060). The caller
//! holds the Tenant's policy bundle exclusively, which every authenticated
//! boundary share-locks, so a deletion and a boundary never interleave.
use crate::apps::identity::models::{TenantDeletion, TenantInvitation, database_time};
use crate::{Error, Result};
use aidash_domain::identity::tenancy::deletion::{self, Actor};
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::execution::convert_values;
use reinhardt::db::orm::{Model, OrmExecutor};
use reinhardt::query::{
	Alias, Expr, ExprTrait, IntoValue, PostgresQueryBuilder, Query, QueryStatementBuilder,
	SimpleExpr,
};
use std::collections::BTreeMap;
use uuid::Uuid;

/// The reason a Tenant Deletion leaves on the Runs it stops. A restore turns
/// these Runs into cancelled ones; nothing resumes them.
pub(crate) const RUN_REASON: &str = "tenant deleted";

/// Who records or undoes a deletion, and through which External Identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DeletionActor {
	pub kind: Actor,
	/// Absent only for the Deployment Administrator bearer token.
	pub identity: Option<Uuid>,
}

impl TenantDeletion {
	pub(crate) fn deleted_by(&self) -> Result<Actor> {
		Ok(Actor::parse(&self.deleted_by_kind)?)
	}

	/// The rule's view of this row.
	pub(crate) fn rule(&self) -> Result<deletion::Deletion> {
		Ok(deletion::Deletion {
			deleted_by: self.deleted_by()?,
			purge_after: self.purge_after,
			restored: self.restored_at.is_some(),
			purged: self.purged_at.is_some(),
		})
	}
}

/// The deletion of `tenant` that no restore has closed, purged or not.
pub(crate) async fn current<E: OrmExecutor>(
	tx: &mut E,
	tenant: &str,
) -> Result<Option<TenantDeletion>> {
	Ok(TenantDeletion::objects()
		.filter(TenantDeletion::field_tenant().eq(tenant.to_owned()))
		.filter(TenantDeletion::field_restored_at().is_null())
		.all_with_db(tx)
		.await?
		.pop())
}

/// The current deletions of `tenants`, by Tenant name.
pub(crate) async fn current_of<E: OrmExecutor>(
	tx: &mut E,
	tenants: Vec<String>,
) -> Result<BTreeMap<String, TenantDeletion>> {
	if tenants.is_empty() {
		return Ok(BTreeMap::new());
	}
	Ok(TenantDeletion::objects()
		.filter(TenantDeletion::field_tenant().is_in(tenants))
		.filter(TenantDeletion::field_restored_at().is_null())
		.all_with_db(tx)
		.await?
		.into_iter()
		.map(|deletion| (deletion.tenant.clone(), deletion))
		.collect())
}

/// Whether `tenant` is deleted, for a boundary that already holds its policy.
pub(crate) async fn is_deleted(tx: &mut dyn TransactionExecutor, tenant: &str) -> Result<bool> {
	let (sql, values) = Query::select()
		.expr(Expr::value(1_i32))
		.from(Alias::new(TenantDeletion::table_name()))
		.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
		.and_where(Expr::col("restored_at").is_null())
		.build(PostgresQueryBuilder);
	Ok(tx
		.fetch_optional(&sql, convert_values(values))
		.await?
		.is_some())
}

/// Records the deletion of `tenant` and stops what a restore must not bring
/// back: its pending Invitations are revoked and its unfinished Runs stop.
pub(crate) async fn record(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	actor: DeletionActor,
	grace_days: u32,
) -> Result<TenantDeletion> {
	if is_deleted(tx, tenant).await? {
		return Err(Error::TenantDeleted);
	}
	let now = database_time(tx).await?;
	let row = TenantDeletion {
		id: Uuid::new_v4(),
		tenant: tenant.to_owned(),
		deleted_at: now,
		deleted_by_kind: actor.kind.as_str().to_owned(),
		deleted_by_identity: actor.identity,
		purge_after: deletion::purge_after(now, grace_days),
		restored_at: None,
		restored_by_kind: None,
		restored_by_identity: None,
		purged_at: None,
	};
	let (sql, values) = Query::insert()
		.into_table(Alias::new(TenantDeletion::table_name()))
		.columns(
			[
				"id",
				"tenant",
				"deleted_at",
				"deleted_by_kind",
				"deleted_by_identity",
				"purge_after",
			]
			.map(Alias::new),
		)
		.values_panic([
			IntoValue::into_value(row.id),
			IntoValue::into_value(row.tenant.clone()),
			IntoValue::into_value(row.deleted_at),
			IntoValue::into_value(row.deleted_by_kind.clone()),
			IntoValue::into_value(row.deleted_by_identity),
			IntoValue::into_value(row.purge_after),
		])
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	let (sql, values) = Query::update()
		.table(Alias::new(TenantInvitation::table_name()))
		.value_expr(Alias::new("revoked_at"), Expr::value(now))
		.and_where(Expr::col("tenant").eq(Expr::value(tenant)))
		.and_where(Expr::col("revoked_at").is_null())
		.and_where(Expr::col("redeemed_at").is_null())
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	stop_runs(tx, tenant).await?;
	Ok(row)
}

/// Closes `deletion` as restored. Its stopped Runs become cancelled, so the
/// workers finish them as cancelled once the Tenant's authority works again.
pub(crate) async fn restore(
	tx: &mut dyn TransactionExecutor,
	deletion: &TenantDeletion,
	actor: DeletionActor,
) -> Result<()> {
	let now = database_time(tx).await?;
	deletion::check_restore(&deletion.rule()?, actor.kind, now)?;
	let (sql, values) = Query::update()
		.table(Alias::new(TenantDeletion::table_name()))
		.value_expr(Alias::new("restored_at"), Expr::value(now))
		.value_expr(
			Alias::new("restored_by_kind"),
			Expr::value(actor.kind.as_str()),
		)
		.value_expr(
			Alias::new("restored_by_identity"),
			Expr::value(actor.identity),
		)
		.and_where(Expr::col("id").eq(Expr::value(deletion.id)))
		.and_where(Expr::col("restored_at").is_null())
		.and_where(Expr::col("purged_at").is_null())
		.build(PostgresQueryBuilder);
	if tx
		.execute(&sql, convert_values(values))
		.await?
		.rows_affected
		!= 1
	{
		return Err(Error::Conflict("the Tenant is not deleted".into()));
	}
	cancel_stopped_runs(tx, &deletion.tenant).await
}

/// The unfinished Runs of `tenant`, local or admitted from a peer.
fn tenant_runs() -> &'static str {
	"id IN (SELECT run_id FROM authorization_execution WHERE tenant = ? \
	 UNION SELECT id FROM authorization_remote_admissions WHERE tenant = ?) \
	 AND NOT phase IN ('COMPLETED', 'FAILED', 'CANCELLED')"
}

/// Control-plane updates must stay possible while a worker lease is fenced by
/// `run_inputs`, as for a dashboard Run control.
async fn control_plane(tx: &mut dyn TransactionExecutor) -> Result<()> {
	let (sql, values) = Query::select()
		.expr(Expr::cust(
			"set_config('aidash.input_ledger_worker', 'true', true)",
		))
		.build(PostgresQueryBuilder);
	tx.fetch_one(&sql, convert_values(values)).await?;
	Ok(())
}

/// Pauses every unfinished Run of `tenant` under [`RUN_REASON`], cancelled ones
/// included. A paused Run is never claimed, so no worker keeps reclaiming a
/// Run whose authority the deletion now refuses.
async fn stop_runs(tx: &mut dyn TransactionExecutor, tenant: &str) -> Result<()> {
	control_plane(tx).await?;
	let (sql, values) = Query::update()
		.table(Alias::new("runs"))
		.value_expr(Alias::new("control"), Expr::value("PAUSED"))
		.value_expr(Alias::new("error"), Expr::value(RUN_REASON))
		.value_expr(Alias::new("revision"), Expr::cust("revision + 1"))
		.value_expr(Alias::new("updated_at"), Expr::cust("CURRENT_TIMESTAMP"))
		.and_where(SimpleExpr::CustomWithExpr(
			format!("({})", tenant_runs()),
			vec![Expr::value(tenant).into(), Expr::value(tenant).into()],
		))
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	Ok(())
}

async fn cancel_stopped_runs(tx: &mut dyn TransactionExecutor, tenant: &str) -> Result<()> {
	control_plane(tx).await?;
	let (sql, values) = Query::update()
		.table(Alias::new("runs"))
		.value_expr(Alias::new("control"), Expr::value("CANCELLED"))
		.value_expr(Alias::new("revision"), Expr::cust("revision + 1"))
		.value_expr(Alias::new("updated_at"), Expr::cust("CURRENT_TIMESTAMP"))
		.and_where(SimpleExpr::CustomWithExpr(
			format!("({}) AND control = 'PAUSED' AND error = ?", tenant_runs()),
			vec![
				Expr::value(tenant).into(),
				Expr::value(tenant).into(),
				Expr::value(RUN_REASON).into(),
			],
		))
		.build(PostgresQueryBuilder);
	tx.execute(&sql, convert_values(values)).await?;
	Ok(())
}
