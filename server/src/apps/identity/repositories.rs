//! Policy ports preserve the caller's transaction and policy/audit lock lifetime.
use super::models::{AuthorizationBundle, AuthorizationDecision};
use aidash_application::{
	Result,
	authorization::Snapshot,
	ports::{AuthorizationScope, AuthorizationStore, AuthorizationTransaction},
};
use aidash_domain::policy::{Decision, Evaluation, PolicyBundle};
use async_trait::async_trait;
use reinhardt::db::backends::{DatabaseConnection, PostgresBackend, TransactionExecutor};
use sqlx::{PgPool, Postgres, Transaction};
use std::sync::Arc;

pub struct PolicyRepository {
	pub pool: PgPool,
}

pub struct NativePolicyScope<'a>(pub &'a mut dyn TransactionExecutor);

#[async_trait]
impl AuthorizationScope for NativePolicyScope<'_> {
	async fn load(&mut self, tenant: &str) -> Result<Snapshot> {
		Ok(AuthorizationBundle::lock_snapshot(self.0, tenant, false).await?)
	}

	async fn record_decision(
		&mut self,
		tenant: &str,
		input: &Evaluation,
		decision: &Decision,
	) -> Result<()> {
		Ok(AuthorizationDecision::append(self.0, tenant, input, decision).await?)
	}
}

/// Compound SQL transactions keep their existing connection and isolation level.
pub struct SqlPolicyScope<'a, 'connection>(pub &'a mut Transaction<'connection, Postgres>);

#[async_trait]
impl AuthorizationScope for SqlPolicyScope<'_, '_> {
	async fn load(&mut self, tenant: &str) -> Result<Snapshot> {
		Ok(super::services::core::Authorization::load(self.0, tenant).await?)
	}

	async fn record_decision(
		&mut self,
		tenant: &str,
		input: &Evaluation,
		decision: &Decision,
	) -> Result<()> {
		Ok(super::services::core::Authorization::record(self.0, tenant, input, decision).await?)
	}
}

struct PolicyTransaction(Box<dyn TransactionExecutor>);

#[async_trait]
impl AuthorizationScope for PolicyTransaction {
	async fn load(&mut self, tenant: &str) -> Result<Snapshot> {
		NativePolicyScope(self.0.as_mut()).load(tenant).await
	}

	async fn record_decision(
		&mut self,
		tenant: &str,
		input: &Evaluation,
		decision: &Decision,
	) -> Result<()> {
		NativePolicyScope(self.0.as_mut())
			.record_decision(tenant, input, decision)
			.await
	}
}

#[async_trait]
impl AuthorizationTransaction for PolicyTransaction {
	async fn replace(
		&mut self,
		tenant: &str,
		expected_revision: i64,
		bundle: &PolicyBundle,
		actor: &str,
	) -> Result<i64> {
		let document = serde_json::to_value(bundle).map_err(crate::Error::from)?;
		Ok(AuthorizationBundle::replace(
			self.0.as_mut(),
			tenant,
			expected_revision,
			document,
			actor,
		)
		.await?)
	}

	async fn commit(self: Box<Self>) -> Result<()> {
		self.0.commit().await.map_err(crate::Error::from)?;
		Ok(())
	}
}

#[async_trait]
impl AuthorizationStore for PolicyRepository {
	async fn begin(&self) -> Result<Box<dyn AuthorizationTransaction>> {
		let connection = DatabaseConnection::new(Arc::new(PostgresBackend::new(self.pool.clone())));
		let transaction = connection.begin().await.map_err(crate::Error::from)?;
		Ok(Box::new(PolicyTransaction(transaction)))
	}
}

pub(crate) mod catalog;

pub(crate) mod catalog_mutation;

pub(crate) mod dependencies;

pub(crate) mod foreign_reads;

pub(crate) mod execution;

pub(crate) mod graph;

pub(crate) mod admission;

pub(crate) mod visibility;

pub(crate) mod worker_tasks;

pub(crate) mod tools;

pub(crate) mod remote_commands;
