//! The existing policy, credential and sharing reads stay on one borrowed native transaction.
use crate::{
	apps::{
		identity::models::{AuthorizationBundle, AuthorizationDecision},
		registry::{repositories::NativeScope, workbench::models::AgentDraftShare},
	},
	authorization::{Authorization, identity::Actor},
	database::native::Pool,
};
use aidash_application::{
	Result,
	ports::{
		authorization::dashboard::AccountPolicy,
		registry::{DefinitionLookup, workbench::DraftAuthority},
	},
	tenancy::TenancyScope,
};
use aidash_domain::{
	identity::Principal,
	policy::{Decision, Evaluation, PolicyBundle},
	registry::Entry,
};
use async_trait::async_trait;
use reinhardt::db::backends::TransactionExecutor;
use std::sync::{Arc, Mutex};
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
	pub policy: Option<AccountPolicy>,
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
	async fn agent_plugin(
		&mut self,
		digest: &str,
	) -> Result<Option<aidash_application::ports::bindings::StoredAgentPlugin>> {
		NativeScope(&mut *self.tx).agent_plugin(digest).await
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

/// The Tenant actions of one Workbench caller (#361). Like
/// `TenantAdministrator::require`, a denial is kept for `record_denial`: the
/// transaction it was decided in rolls back, and auditing it in a second
/// transaction while the first still holds its connection could wait on the
/// pool forever.
#[derive(Clone)]
pub(crate) struct TenantActions {
	actor: Actor,
	node_id: String,
	denial: Arc<Mutex<Option<(Evaluation, Decision)>>>,
}

impl TenantActions {
	/// Persist a removed GCIP Binding before opening the protected transaction.
	pub(crate) async fn check_binding(&self, pool: &Pool) -> crate::Result<()> {
		if let Actor::Subject(identity) = &self.actor {
			identity.check_binding(pool).await?;
		}
		Ok(())
	}

	pub(crate) fn new(actor: Actor, node_id: String) -> Self {
		Self {
			actor,
			node_id,
			denial: Default::default(),
		}
	}

	/// Requires a Tenant action of a Subject of `tenant` on resource `id` in
	/// `tx`, where its effect is written. A Deployment Administrator passes.
	pub(crate) async fn require(
		&self,
		tx: &mut dyn TransactionExecutor,
		policy: Option<&AccountPolicy>,
		tenant: &str,
		action: &str,
		id: &str,
	) -> crate::Result<()> {
		let Actor::Subject(identity) = &self.actor else {
			return Ok(());
		};
		if identity.tenant != tenant {
			return Err(crate::Error::Forbidden);
		}
		let snapshot = identity.lock_native(&mut *tx, false, policy).await?;
		let evaluation = Evaluation {
			subject: identity.subject.clone(),
			action: action.into(),
			resource: aidash_domain::policy::Resource {
				tenant: tenant.into(),
				kind: aidash_domain::identity::tenant_administration::resource_kind(action).into(),
				id: id.into(),
				attributes: serde_json::json!({}),
			},
			environment: serde_json::json!({"node_id": self.node_id, "transport": "api"}),
		};
		let mut decision = snapshot.bundle.evaluate(&evaluation);
		decision.revision = snapshot.revision;
		if decision.allowed {
			AuthorizationDecision::append(&mut *tx, tenant, &evaluation, &decision).await?;
			return Ok(());
		}
		*self.denial.lock().expect("denial lock") = Some((evaluation, decision));
		Err(crate::Error::Forbidden)
	}

	/// Commits a denial kept by `require` once its transaction has ended.
	pub(crate) async fn record_denial(&self, pool: &Pool) -> crate::Result<()> {
		let Some((evaluation, decision)) = self.denial.lock().expect("denial lock").take() else {
			return Ok(());
		};
		let tenant = &evaluation.resource.tenant;
		let mut audit = pool.begin(&TenancyScope::tenant(tenant)?).await?;
		AuthorizationDecision::append(&mut audit, tenant, &evaluation, &decision).await?;
		audit.commit().await?;
		Ok(())
	}
}
