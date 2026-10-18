//! The existing policy, credential and sharing reads stay on one borrowed native transaction.
use crate::{
	apps::{
		identity::models::{AuthorizationBundle, AuthorizationDecision},
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
use uuid::Uuid;
pub(crate) fn principal(actor: &Actor) -> Principal {
	match actor {
		Actor::DeploymentAdministrator => Principal::DeploymentAdministrator,
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
	async fn binding_installation(
		&mut self,
		p: &aidash_domain::registry::Projection,
	) -> Result<()> {
		NativeScope(&mut *self.tx).binding_installation(p).await
	}

	async fn definition(&mut self, id: &str, version: &str) -> Result<Entry> {
		NativeScope(self.tx).definition(id, version).await
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

/// Requires a Tenant action of a Subject of `tenant` on resource `id` in `tx`,
/// where its effect is written. A Deployment Administrator passes. A denial is
/// audited in a transaction of its own, because `tx` rolls back.
pub(crate) async fn require_tenant_action(
	tx: &mut dyn TransactionExecutor,
	pool: &crate::database::native::Pool,
	actor: &Actor,
	tenant: &str,
	action: &str,
	id: &str,
) -> crate::Result<()> {
	let Actor::Subject(identity) = actor else {
		return Ok(());
	};
	if identity.tenant != tenant {
		return Err(crate::Error::Forbidden);
	}
	let snapshot = identity
		.lock_native(&mut *tx, false, pool.dashboard_policy().as_ref())
		.await?;
	let evaluation = Evaluation {
		subject: identity.subject.clone(),
		action: action.into(),
		resource: aidash_domain::policy::Resource {
			tenant: tenant.into(),
			kind: aidash_domain::identity::tenant_administration::resource_kind(action).into(),
			id: id.into(),
			attributes: serde_json::json!({}),
		},
		environment: serde_json::json!({"transport": "api"}),
	};
	let mut decision = snapshot.bundle.evaluate(&evaluation);
	decision.revision = snapshot.revision;
	if decision.allowed {
		AuthorizationDecision::append(&mut *tx, tenant, &evaluation, &decision).await?;
		return Ok(());
	}
	let mut audit = pool
		.begin(&aidash_application::tenancy::TenancyScope::tenant(tenant)?)
		.await?;
	AuthorizationDecision::append(&mut audit, tenant, &evaluation, &decision).await?;
	audit.commit().await?;
	Err(crate::Error::Forbidden)
}
