//! The existing policy, credential and sharing reads stay on one borrowed native transaction.
use crate::{
	apps::{
		identity::models::AuthorizationBundle,
		registry::{repositories::NativeScope, workbench::models::AgentDraftShare},
	},
	authorization::{Authorization, identity::Actor},
};
use aidash_application::{
	Result,
	ports::registry::{DefinitionLookup, workbench::DraftAuthority},
};
use aidash_domain::{
	identity::Principal,
	policy::{Decision, Evaluation, PolicyBundle},
	registry::Entry,
};
use async_trait::async_trait;
use reinhardt::db::backends::TransactionExecutor;
use serde_json::Value;
use uuid::Uuid;
pub(crate) fn principal(actor: &Actor) -> Principal {
	match actor {
		Actor::Operator => Principal::Operator,
		Actor::Subject(identity) => Principal::Subject {
			tenant: identity.tenant.clone(),
			subject: identity.subject.clone(),
		},
	}
}
pub(crate) struct Scope<'a> {
	pub tx: &'a mut dyn TransactionExecutor,
	pub actor: &'a Actor,
	pub policy: Option<aidash_application::ports::authorization::dashboard::AccountPolicy>,
}
#[async_trait]
impl DefinitionLookup for Scope<'_> {
	async fn foreign_agent(
		&mut self,
		node: &str,
		reference: &aidash_domain::registry::bindings::QualifiedRef,
	) -> Result<aidash_domain::registry::bindings::ForeignAgentSnapshot> {
		if !matches!(self.actor, Actor::Operator) {
			return Err(aidash_application::Error::Forbidden);
		}
		NativeScope(&mut *self.tx)
			.foreign_agent(node, reference)
			.await
	}
	async fn binding_installation(
		&mut self,
		p: &aidash_domain::registry::Projection,
	) -> Result<()> {
		NativeScope(&mut *self.tx).binding_installation(p).await
	}

	async fn definition(&mut self, id: &str, version: &str) -> Result<Entry> {
		NativeScope(self.tx).definition(id, version).await
	}
	async fn overrides(&mut self, id: &str, version: &str) -> Result<Option<Value>> {
		NativeScope(self.tx).overrides(id, version).await
	}
	async fn executor_kind(&mut self, id: &str, version: &str) -> Result<Option<String>> {
		NativeScope(self.tx).executor_kind(id, version).await
	}
}
#[async_trait]
impl DraftAuthority for Scope<'_> {
	fn principal(&self) -> Principal {
		principal(self.actor)
	}
	async fn lock_identity(&mut self) -> Result<()> {
		if let Actor::Subject(identity) = self.actor {
			identity
				.lock_native(self.tx, false, self.policy.as_ref())
				.await?;
		}
		Ok(())
	}
	async fn share(&mut self, draft: Uuid, subject: &str) -> Result<Option<(bool, String)>> {
		Ok(AgentDraftShare::current(self.tx, draft, subject)
			.await?
			.map(|row| (row.can_edit, row.documents_digest)))
	}
	async fn evaluate(&mut self, tenant: &str, evaluation: &Evaluation) -> Result<Decision> {
		Authorization::evaluate_native(self.tx, tenant, evaluation)
			.await
			.map_err(Into::into)
	}
	async fn bundle(&mut self, tenant: &str) -> Result<PolicyBundle> {
		Ok(AuthorizationBundle::lock_snapshot(self.tx, tenant, false)
			.await?
			.bundle)
	}
}
