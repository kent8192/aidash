//! Native boundary adapters for sandbox admission, session management and background execution.
use super::*;
use crate::apps::registry::workbench::models::AgentTestSession;
use reinhardt::db::backends::{DatabaseConnection as BackendConnection, PostgresBackend};
use reinhardt::db::orm::connection::DatabaseConnectionLease;
use reinhardt::injectable;
use std::sync::Arc;

use aidash_application::registry::workbench::sandbox::execution::Job as TestJob;

/// Remove ordinary test payloads. The metadata and expiry marker remain.
pub async fn purge_expired(pool: &sqlx::PgPool) -> Result<u64> {
	let lease = DatabaseConnectionLease::register(BackendConnection::new(Arc::new(
		PostgresBackend::new(pool.clone()),
	)))?;
	lease
		.handle()
		.atomic(async |tx| AgentTestSession::purge(tx).await)
		.await
}

async fn complete(f: Federation, session_id: Uuid, actor: Actor, job: TestJob) -> Result<()> {
	aidash_runtime::sandbox::complete(
		crate::bootstrap::workbench_sandbox_execution(&f, actor),
		session_id,
		job,
	)
	.await
	.map_err(Into::into)
}

pub use crate::apps::registry::workbench::serializers::test::{TestInput, TestLimits, TestSession};

#[derive(Clone)]
pub struct BehavioralTests {
	pub(crate) runtime: Federation,
}

#[injectable(scope = "request")]
pub async fn provide_test(#[inject] runtime: Federation) -> BehavioralTests {
	BehavioralTests { runtime }
}

impl BehavioralTests {
	pub(crate) async fn get_limits(&self, actor: Actor, id: Uuid) -> Result<TestLimits> {
		Ok(
			aidash_application::registry::workbench::sandbox::get_limits(
				&crate::bootstrap::workbench_sandbox_repository(&self.runtime, actor),
				id,
			)
			.await?
			.into(),
		)
	}
	pub(crate) async fn set_limits(
		&self,
		actor: Actor,
		tenant: String,
		input: TestLimits,
	) -> Result<TestLimits> {
		Ok(
			aidash_application::registry::workbench::sandbox::set_limits(
				&crate::bootstrap::workbench_sandbox_repository(&self.runtime, actor),
				tenant,
				input.into(),
			)
			.await?
			.into(),
		)
	}
	pub(crate) async fn sessions(&self, actor: Actor, id: Uuid) -> Result<Vec<TestSession>> {
		aidash_application::registry::workbench::sandbox::sessions(
			&crate::bootstrap::workbench_sandbox_repository(&self.runtime, actor),
			id,
		)
		.await
		.map_err(Into::into)
	}
	pub(crate) async fn stop(&self, actor: Actor, id: Uuid) -> Result<TestSession> {
		aidash_application::registry::workbench::sandbox::stop(
			&crate::bootstrap::workbench_sandbox_repository(&self.runtime, actor),
			id,
		)
		.await
		.map_err(Into::into)
	}
	pub(crate) async fn start(
		&self,
		actor: Actor,
		id: Uuid,
		input: TestInput,
	) -> Result<TestSession> {
		let runtime = self.runtime.clone();
		let repository = crate::bootstrap::workbench_sandbox_repository(&runtime, actor.clone());
		let credentials = crate::bootstrap::workbench_sandbox_credentials();
		let models = crate::bootstrap::workbench_sandbox_models(&runtime);
		let admitted = aidash_application::registry::workbench::sandbox::admission::admit(
			&aidash_application::registry::workbench::sandbox::admission::Admission {
				repository: &repository,
				credentials: credentials.as_ref(),
				models: &models,
			},
			id,
			input,
		)
		.await?;
		let session_id = admitted.session.id;
		tokio::spawn(async move {
			if let Err(error) = complete(runtime, session_id, actor, admitted.job).await {
				tracing::error!(%session_id,%error,"sandbox session completion failed");
			}
		});
		Ok(admitted.session)
	}
}
