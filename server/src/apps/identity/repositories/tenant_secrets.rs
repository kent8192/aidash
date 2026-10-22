//! Native Tenant locks, Tenant Secret metadata persistence and metadata-only
//! lifecycle events. A Tenant Secret's value is never read here.
use super::super::models::tenant_secrets::TenantSecret as Record;
use aidash_application::{
	Error, Result,
	tenant_secrets::{RECONCILIATION_BATCH_SIZE, Repository, Scope},
};
use aidash_domain::tenant_secrets::TenantSecret;
use async_trait::async_trait;
use reinhardt::db::{
	backends::TransactionExecutor,
	orm::{Model, execution::convert_values},
};
use reinhardt::query::{
	Expr, IntoIden, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};
use serde_json::json;
use uuid::Uuid;

#[derive(Clone)]
pub struct NativeRepository {
	pub pool: crate::database::native::Pool,
	pub node: String,
}
struct NativeScope {
	tx: Box<dyn TransactionExecutor>,
	tenant: String,
	node: String,
}
fn contract(row: Record) -> Result<TenantSecret> {
	Ok(serde_json::from_value(serde_json::to_value(row)?)?)
}
fn failed(error: impl Into<reinhardt::core::exception::Error>) -> Error {
	crate::Error::from(error.into()).into()
}
#[async_trait]
impl Repository for NativeRepository {
	async fn begin(&self, tenant: &str) -> Result<Box<dyn Scope>> {
		let scope = aidash_application::tenancy::TenancyScope::tenant(tenant)?;
		let mut tx = self.pool.begin(&scope).await?.into_executor()?;
		let (sql, values) = Query::select()
			.expr(SimpleExpr::FunctionCall(
				"pg_advisory_xact_lock".into_iden(),
				vec![SimpleExpr::FunctionCall(
					"hashtextextended".into_iden(),
					vec![Expr::value(tenant).into(), Expr::value(71003237_i64).into()],
				)],
			))
			.build(PostgresQueryBuilder);
		tx.execute(&sql, convert_values(values))
			.await
			.map_err(crate::Error::from)?;
		Ok(Box::new(NativeScope {
			tx,
			tenant: tenant.into(),
			node: self.node.clone(),
		}))
	}
	async fn reconciliation_candidates(
		&self,
		after: Option<Uuid>,
		limit: usize,
	) -> Result<Vec<TenantSecret>> {
		// Cleanup reads every Tenant's rows.
		let scope = aidash_application::tenancy::TenancyScope::Deployment(
			aidash_application::tenancy::DeploymentScopeReason::CredentialReconcile,
		);
		let mut tx = self.pool.begin(&scope).await?.into_executor()?;
		let mut query = Record::objects()
			.filter(
				Record::field_state()
					.ne("deleted")
					.or(Record::field_pinned_version().is_not_null()),
			)
			.order_by(&["id"])
			.limit(limit.clamp(1, RECONCILIATION_BATCH_SIZE));
		if let Some(after) = after {
			query = query.filter(Record::field_id().gt(after));
		}
		let rows = query.all_with_executor(tx.as_mut()).await.map_err(failed)?;
		tx.commit().await.map_err(crate::Error::from)?;
		rows.into_iter().map(contract).collect()
	}
	async fn all(&self, tenant: &str) -> Result<Vec<TenantSecret>> {
		let scope = aidash_application::tenancy::TenancyScope::tenant(tenant)?;
		let mut tx = self.pool.begin(&scope).await?.into_executor()?;
		let rows = Record::objects()
			.filter(Record::field_tenant().eq(tenant))
			.order_by(&["id"])
			.all_with_executor(tx.as_mut())
			.await
			.map_err(failed)?;
		tx.commit().await.map_err(crate::Error::from)?;
		rows.into_iter().map(contract).collect()
	}
}
#[async_trait]
impl Scope for NativeScope {
	async fn named(&mut self, name: &str) -> Result<Option<TenantSecret>> {
		let rows = Record::objects()
			.filter(Record::field_tenant().eq(&self.tenant))
			.filter(Record::field_name().eq(name))
			.filter(Record::field_state().ne("deleted"))
			.all_with_executor(self.tx.as_mut())
			.await
			.map_err(failed)?;
		rows.into_iter().next().map(contract).transpose()
	}
	async fn by_id(&mut self, id: Uuid) -> Result<Option<TenantSecret>> {
		let rows = Record::objects()
			.filter(Record::field_tenant().eq(&self.tenant))
			.filter(Record::field_id().eq(id))
			.all_with_executor(self.tx.as_mut())
			.await
			.map_err(failed)?;
		rows.into_iter().next().map(contract).transpose()
	}
	async fn list(&mut self) -> Result<Vec<TenantSecret>> {
		let rows = Record::objects()
			.filter(Record::field_tenant().eq(&self.tenant))
			.filter(Record::field_state().ne("deleted"))
			.order_by(&["name"])
			.all_with_executor(self.tx.as_mut())
			.await
			.map_err(failed)?;
		rows.into_iter().map(contract).collect()
	}
	async fn count(&mut self) -> Result<usize> {
		Ok(Record::objects()
			.filter(Record::field_tenant().eq(&self.tenant))
			.filter(Record::field_state().ne("deleted"))
			.count_with_executor(self.tx.as_mut())
			.await
			.map_err(failed)? as usize)
	}
	async fn insert(&mut self, value: &TenantSecret) -> Result<()> {
		if value.tenant != self.tenant {
			return Err(Error::Forbidden);
		}
		let row: Record = serde_json::from_value(serde_json::to_value(value)?)?;
		// A concurrent writer of the same name loses to the unique index.
		Record::objects()
			.insert_with_executor(self.tx.as_mut(), &row)
			.await
			.map_err(|_| Error::Conflict("a Tenant Secret of that name exists".into()))?;
		Ok(())
	}
	async fn save(&mut self, value: &TenantSecret, event: &str, actor: &str) -> Result<()> {
		if value.tenant != self.tenant {
			return Err(Error::Forbidden);
		}
		let row: Record = serde_json::from_value(serde_json::to_value(value)?)?;
		Record::objects()
			.save_with_executor(self.tx.as_mut(), &row)
			.await
			.map_err(failed)?;
		// Metadata only: never the value, its Fingerprint or its last characters.
		crate::apps::execution::models::event_records::append(
			self.tx.as_mut(),
			&self.node,
			None,
			event,
			json!({
				"tenant": value.tenant,
				"id": value.id,
				"name": value.name,
				"hosts": value.hosts,
				"state": value.state,
				"revision": value.revision,
				"actor": actor,
			}),
		)
		.await?;
		Ok(())
	}
	async fn remove(&mut self, id: Uuid) -> Result<()> {
		Record::objects()
			.delete_with_executor(self.tx.as_mut(), id)
			.await
			.map_err(failed)?;
		Ok(())
	}
	async fn commit(self: Box<Self>) -> Result<()> {
		Ok(self.tx.commit().await.map_err(crate::Error::from)?)
	}
}

/// The hosts of the active Tenant Secret `name` of `tenant`, read inside the
/// caller's transaction so definition admission sees what the Tenant sees.
pub(crate) async fn active_hosts_in<E: reinhardt::db::orm::OrmExecutor>(
	db: &mut E,
	tenant: &str,
	name: &str,
) -> Result<Option<Vec<String>>> {
	let rows = Record::objects()
		.filter(Record::field_tenant().eq(tenant))
		.filter(Record::field_name().eq(name))
		.filter(Record::field_state().eq("active"))
		.all_with_db(db)
		.await
		.map_err(failed)?;
	Ok(rows.into_iter().next().map(|row| row.hosts))
}

/// The same, for the Tenant of the transaction's own scope.
pub(crate) async fn active_hosts(
	tx: &mut dyn TransactionExecutor,
	name: &str,
) -> Result<Option<Vec<String>>> {
	let (sql, values) = Query::select()
		.expr_as(
			SimpleExpr::FunctionCall(
				"current_setting".into_iden(),
				vec![
					Expr::value("aidash.tenant").into(),
					Expr::value(true).into(),
				],
			),
			"tenant".into_iden(),
		)
		.build(PostgresQueryBuilder);
	let row = tx
		.fetch_one(&sql, convert_values(values))
		.await
		.map_err(crate::Error::from)?;
	let data = reinhardt::db::orm::QueryRow::from_backend_row(row).data;
	let tenant = data["tenant"].as_str().unwrap_or_default().to_owned();
	if tenant.is_empty() {
		return Ok(None);
	}
	let rows = Record::objects()
		.filter(Record::field_tenant().eq(&tenant))
		.filter(Record::field_name().eq(name))
		.filter(Record::field_state().eq("active"))
		.all_with_executor(tx)
		.await
		.map_err(failed)?;
	Ok(rows.into_iter().next().map(|row| row.hosts))
}
