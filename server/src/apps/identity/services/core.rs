use reinhardt::query::{Alias, Expr, LockType, PostgresQueryBuilder, Query};
use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};
#[path = "access.rs"]
pub mod access;
#[path = "catalog.rs"]
pub mod catalog;
#[path = "execution.rs"]
pub mod execution;
#[path = "identity.rs"]
pub mod identity;
#[path = "interaction.rs"]
pub mod interaction;
#[path = "peer.rs"]
pub mod peer;
pub use super::policy;
#[path = "remote.rs"]
pub mod remote;
#[path = "resources.rs"]
pub mod resources;
#[path = "workspace.rs"]
pub mod workspace;

use crate::apps::identity::models::{
	AuthorizationBundle, AuthorizationDecision, AuthorizationRevision,
};
use crate::database::native::Pool;
use crate::{Error, Result};
use aidash_application::tenancy::TenancyScope;
use policy::{Decision, Evaluation, PolicyBundle, identifier};
use reinhardt::db::backends::TransactionExecutor;
use serde_json::Value;

#[derive(Clone)]
pub struct Authorization {
	pub pool: Pool,
}

impl Authorization {
	pub(crate) async fn evaluate_native(
		tx: &mut dyn TransactionExecutor,
		tenant: &str,
		input: &Evaluation,
	) -> Result<Decision> {
		let mut scope = crate::apps::identity::repositories::NativePolicyScope(tx);
		Ok(
			aidash_application::authorization::Authorization::evaluate_in(
				&mut scope, tenant, input,
			)
			.await?,
		)
	}

	pub async fn replace(
		&self,
		tenant: &str,
		expected_revision: i64,
		bundle: PolicyBundle,
		actor: &str,
	) -> Result<Snapshot> {
		Ok(crate::bootstrap::authorization(self.pool.clone())
			.replace(tenant, expected_revision, bundle, actor)
			.await?)
	}

	pub(crate) async fn load(
		tx: &mut crate::database::native::Transaction,
		tenant: &str,
	) -> Result<Snapshot> {
		Self::load_with_mode(tx, tenant, false).await
	}

	pub(crate) async fn load_with_mode(
		tx: &mut crate::database::native::Transaction,
		tenant: &str,
		exclusive: bool,
	) -> Result<Snapshot> {
		identifier(tenant)?;
		let query = if exclusive {
			Query::select()
				.column(Alias::new("revision"))
				.column(Alias::new("document"))
				.from(Alias::new("authorization_bundles"))
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("tenant")))
						.eq(Expr::cust("$1")),
				)
				.lock(LockType::Update)
				.to_string(PostgresQueryBuilder)
		} else {
			Query::select()
				.column(Alias::new("revision"))
				.column(Alias::new("document"))
				.from(Alias::new("authorization_bundles"))
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("tenant")))
						.eq(Expr::cust("$1")),
				)
				.lock(LockType::Share)
				.to_string(PostgresQueryBuilder)
		};
		let row: Option<(i64, Value)> = crate::database::native::query_as(&query)
			.columns(&["revision", "document"])
			.bind(tenant)
			.fetch_optional(&mut **tx)
			.await?;
		let (revision, document) =
			row.ok_or_else(|| Error::NotFound("authorization policy".into()))?;
		// Checked once the policy is held, as `AuthorizationBundle::lock_snapshot` does.
		let deleted: Option<i32> = crate::database::native::query_scalar(
			&Query::select()
				.expr(Expr::value(1_i32))
				.from(Alias::new("tenant_deletions"))
				.and_where(
					reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("tenant")))
						.eq(Expr::cust("$1")),
				)
				.and_where(Expr::col(Alias::new("restored_at")).is_null())
				.to_string(PostgresQueryBuilder),
		)
		.bind(tenant)
		.scalar_optional(&mut **tx)
		.await?;
		if deleted.is_some() {
			return Err(Error::TenantDeleted);
		}
		Ok(Snapshot {
			revision,
			bundle: serde_json::from_value(document)?,
		})
	}

	pub async fn snapshot(&self, tenant: &str) -> Result<Snapshot> {
		self.pool
			.atomic(&TenancyScope::tenant(tenant)?, async |tx| {
				AuthorizationBundle::lock_snapshot(tx, tenant, false).await
			})
			.await
	}

	pub async fn evaluate_in_transaction(
		tx: &mut crate::database::native::Transaction,
		tenant: &str,
		input: &Evaluation,
	) -> Result<Decision> {
		let mut scope = crate::apps::identity::repositories::SqlPolicyScope(tx);
		Ok(
			aidash_application::authorization::Authorization::evaluate_in(
				&mut scope, tenant, input,
			)
			.await?,
		)
	}

	pub(crate) async fn record(
		tx: &mut crate::database::native::Transaction,
		tenant: &str,
		input: &Evaluation,
		decision: &Decision,
	) -> Result<()> {
		Self::record_many(tx, tenant, &[(input.clone(), decision.clone())]).await
	}

	/// Retain every decision and its allocation/commit order, while acquiring
	/// the transaction-wide audit controls only once for a compound check.
	pub(super) async fn record_many(
		tx: &mut crate::database::native::Transaction,
		tenant: &str,
		records: &[(Evaluation, Decision)],
	) -> Result<()> {
		AuthorizationDecision::append_many(&mut **tx, tenant, records).await
	}

	pub async fn evaluate(&self, tenant: &str, input: &Evaluation) -> Result<Decision> {
		Ok(crate::bootstrap::authorization(self.pool.clone())
			.evaluate(tenant, input)
			.await?)
	}

	pub async fn simulate(&self, tenant: &str, input: &Evaluation) -> Result<Decision> {
		Ok(crate::bootstrap::authorization(self.pool.clone())
			.simulate(tenant, input)
			.await?)
	}

	pub async fn revisions(&self, tenant: &str, after: i64, limit: i64) -> Result<Vec<Value>> {
		self.pool
			.atomic(&TenancyScope::tenant(tenant)?, async |tx| {
				AuthorizationRevision::page(tx, tenant, after, limit).await
			})
			.await
	}

	pub async fn decisions(&self, tenant: &str, after: i64, limit: i64) -> Result<Vec<Value>> {
		self.pool
			.atomic(&TenancyScope::tenant(tenant)?, async |tx| {
				AuthorizationDecision::page(tx, tenant, after, limit).await
			})
			.await
	}
}

#[path = "oidc.rs"]
pub mod oidc;

pub use crate::apps::identity::serializers::contracts::Snapshot;
