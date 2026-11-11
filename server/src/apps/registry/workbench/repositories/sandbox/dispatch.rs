//! Real dispatch reuses native session UPDATE and profile SHARE locks on one current-identity scope.
use super::{Repository, Scope};
use crate::apps::registry::workbench::models::{AgentTestProfile, AgentTestSession};
use crate::authorization::identity::Actor;
use aidash_application::{
	Result,
	ports::registry::workbench::sandbox::dispatch::{RealDispatchRepository, RealDispatchScope},
};
use aidash_domain::registry::{EntityRef, Entry, workbench::profile::TestProfile};
use async_trait::async_trait;
use reinhardt::db::backends::TransactionExecutor;
use serde_json::Value;
use uuid::Uuid;
#[async_trait]
impl RealDispatchRepository for Repository {
	async fn begin_real(&self) -> Result<Box<dyn RealDispatchScope + '_>> {
		Ok(Box::new(Scope {
			tx: self.store.pool.begin(&self.actor.tenancy()?).await?,
			actor: self.actor.clone(),
			tenant_actions: self.tenant_actions.clone(),
			node_id: self.node_id.clone(),
			ceilings: self.store.test_limit_ceilings,
			tenant_secrets: self.store.tenant_secrets.clone(),
		}))
	}
}
#[async_trait]
impl RealDispatchScope for Scope {
	async fn authorize_test_call(
		&mut self,
		id: Uuid,
		approval: &aidash_application::ports::registry::workbench::sandbox::execution::TestApproval,
		consume: bool,
	) -> Result<()> {
		let Actor::Subject(identity) = &self.actor else {
			return Err(aidash_application::Error::Forbidden);
		};
		let policy = self.tx.pool().dashboard_policy();
		let snapshot = identity
			.lock_native(&mut self.tx, false, policy.as_ref())
			.await?;
		let session = AgentTestSession::read(&mut self.tx, id, true).await?;
		let evaluation = aidash_domain::policy::Evaluation {
			subject: identity.subject.clone(),
			action: approval.action.into(),
			resource: approval.resource.clone(),
			environment: serde_json::json!({}),
		};
		let direct = aidash_domain::capabilities::approvals::workbench_policy(&snapshot.bundle)
			.evaluate(&evaluation);
		if direct.allowed || (direct.reason == "no_matching_allow" && !approval.requires_approval) {
			return Ok(());
		}
		let person = identity
			.http_session
			.as_ref()
			.map(|s| s.identity_id)
			.ok_or(aidash_application::Error::Forbidden)?;
		let pending = &session.scenario["pending_approval"];
		if direct.reason != "no_matching_allow"
			|| session.status != "running"
			|| session.scenario["starter_identity"] != serde_json::json!(person)
			|| session.scenario["starter_subject"] != identity.subject
			|| pending["state"] != "allow_once"
			|| pending["resource"] != serde_json::to_value(&approval.resource)?
			|| pending["call"] != serde_json::to_value(&approval.call)?
			|| pending["action"] != approval.action
			|| !aidash_domain::capabilities::approvals::workbench_approver_eligible(
				&snapshot.bundle,
				&identity.subject,
				&approval.resource,
			) {
			return Err(aidash_application::Error::Forbidden);
		}
		if consume {
			let mut scenario = session.scenario;
			scenario["pending_approval"]["state"] = serde_json::json!("consumed");
			AgentTestSession::complete_listing(&mut self.tx, id, scenario).await?;
		}
		Ok(())
	}
	async fn lock_identity(&mut self) -> Result<()> {
		if let Actor::Subject(identity) = &self.actor {
			let policy = self.tx.pool().dashboard_policy();
			identity
				.lock_native(&mut self.tx, false, policy.as_ref())
				.await?;
		}
		Ok(())
	}
	async fn profile(&mut self, tenant: &str, id: &str) -> Result<TestProfile> {
		AgentTestProfile::locked(&mut self.tx, tenant, id)
			.await
			.map_err(Into::into)
	}
	async fn effective(&mut self, reference: &EntityRef) -> Result<Entry> {
		crate::apps::registry::services::admission::definition(
			&mut self.tx,
			&reference.id,
			&reference.version,
		)
		.await
		.map_err(Into::into)
	}
	async fn write_calls(&mut self, id: Uuid, calls: Value, running: bool) -> Result<()> {
		AgentTestSession::calls(&mut self.tx, id, calls, running)
			.await
			.map_err(Into::into)
	}
	async fn tenant_secret(
		&mut self,
		tenant: &str,
		name: &str,
		endpoint: &str,
		call: aidash_application::tenant_secrets::SecretCall,
	) -> Result<aidash_application::tenant_secrets::CallSecret> {
		// A node without a Secret Store holds no Tenant Secret.
		let Some(service) = &self.tenant_secrets else {
			return Err(aidash_application::Error::Invalid(
				aidash_domain::tenant_secrets::UNAVAILABLE.into(),
			));
		};
		service.call_secret(tenant, name, endpoint, call).await
	}
	async fn rollback(self: Box<Self>) -> Result<()> {
		Box::new(self.tx)
			.rollback()
			.await
			.map_err(crate::Error::from)
			.map_err(Into::into)
	}
}
