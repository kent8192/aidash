//! Native boundary adapters for sandbox admission, session management and background execution.
use super::*;
use crate::apps::registry::workbench::models::AgentTestSession;
use reinhardt::db::orm::connection::DatabaseConnectionLease;
use reinhardt::injectable;

/// Remove ordinary test payloads. The metadata and expiry marker remain.
pub async fn purge_expired(pool: &crate::database::native::Pool) -> Result<u64> {
	let lease = DatabaseConnectionLease::register(pool.connection())?;
	lease
		.handle()
		.atomic(async |tx| AgentTestSession::purge(tx).await)
		.await
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
		let runtime = &self.runtime;
		let permit = runtime.sandbox.permit()?;
		let repository = crate::bootstrap::workbench_sandbox_repository(runtime, actor.clone());
		let credentials = crate::bootstrap::workbench_sandbox_credentials();
		let models = crate::bootstrap::workbench_sandbox_models(runtime);
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
		let execution = crate::bootstrap::workbench_sandbox_execution(runtime, actor);
		let repository = execution.repository.clone();
		let message = admitted.job.input.message.clone();
		if let Err(error) = permit.spawn(
			session_id,
			aidash_runtime::sandbox::complete(execution, session_id, admitted.job),
		) {
			// No job was launched, so persist a known failure and release the admitted active slot.
			aidash_application::registry::workbench::sandbox::execution::settle(
				repository.as_ref(),
				session_id,
				&message,
				Err(
					aidash_application::registry::workbench::sandbox::execution::Failure::Execution(
						aidash_application::Error::Conflict(error.to_string()),
					),
				),
			)
			.await?;
			return Err(error.into());
		}
		Ok(admitted.session)
	}
}
