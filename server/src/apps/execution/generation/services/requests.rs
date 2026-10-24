use crate::apps::execution::generation::{
	Assignment, Request,
	policy::{Policy, Spec},
};
use crate::{Error, Result, authorization::identity::Actor, federation::Federation};
use reinhardt::injectable;

use uuid::Uuid;

use crate::apps::execution::generation::serializers::requests::{AssignInput, PolicyUpdate, Usage};

#[derive(Clone)]
pub struct GenerationRequests {
	runtime: Federation,
}

#[injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> GenerationRequests {
	GenerationRequests { runtime }
}

impl GenerationRequests {
	pub async fn set_policy(
		&self,
		actor: Actor,
		(tenant, id): (String, String),
		input: PolicyUpdate,
	) -> Result<Policy> {
		aidash_application::generation::policy::set(
			&crate::bootstrap::generation_policy_repository(&self.runtime.store, actor),
			&tenant,
			&id,
			input.expected_revision,
			&input.spec,
			&crate::bootstrap::registry_validation_for(&self.runtime.store),
		)
		.await
		.map_err(Into::into)
	}
	pub async fn policies(&self, actor: Actor, tenant: String) -> Result<Vec<Policy>> {
		aidash_application::generation::policy::list(
			&crate::bootstrap::generation_policy_repository(&self.runtime.store, actor),
			&tenant,
		)
		.await
		.map_err(Into::into)
	}
	pub async fn assign(
		&self,
		actor: Actor,
		(tenant, id): (String, Uuid),
		input: AssignInput,
	) -> Result<Assignment> {
		let f = self.runtime.clone();
		let Actor::Subject(identity) = actor else {
			return Err(Error::Forbidden);
		};
		if tenant != identity.tenant {
			return Err(Error::Forbidden);
		}
		crate::apps::execution::generation::assign(
			&f,
			&identity,
			id,
			&input.policy_id,
			&input.reason,
		)
		.await
	}
	pub async fn requests(&self, actor: Actor, tenant: String) -> Result<Vec<Request>> {
		aidash_application::generation::reads::list(
			&crate::bootstrap::generation_read_repository(&self.runtime.store, actor),
			&tenant,
		)
		.await
		.map_err(Into::into)
	}
	pub async fn control(
		&self,
		actor: Actor,
		(tenant, id): (String, Uuid),
		input: crate::generation::lifecycle::Control,
	) -> Result<Request> {
		aidash_application::generation::lifecycle::control(
			&crate::bootstrap::generation_control_repository(&self.runtime, actor),
			&tenant,
			id,
			&input,
		)
		.await
		.map_err(Into::into)
	}
	pub async fn history(
		&self,
		actor: Actor,
		(tenant, id): (String, Uuid),
	) -> Result<Vec<crate::generation::lifecycle::History>> {
		aidash_application::generation::reads::history(
			&crate::bootstrap::generation_read_repository(&self.runtime.store, actor),
			&tenant,
			id,
		)
		.await
		.map_err(Into::into)
	}
	pub async fn usage(&self, actor: Actor, (tenant, id): (String, Uuid)) -> Result<Usage> {
		aidash_application::generation::reads::usage(
			&crate::bootstrap::generation_read_repository(&self.runtime.store, actor),
			&tenant,
			id,
		)
		.await
		.map_err(Into::into)
	}
	pub async fn spec(&self, actor: Actor, (tenant, id): (String, Uuid)) -> Result<Spec> {
		aidash_application::generation::reads::specification(
			&crate::bootstrap::generation_read_repository(&self.runtime.store, actor),
			&tenant,
			id,
		)
		.await
		.map_err(Into::into)
	}
}
