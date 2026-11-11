//! Session admission reuses the native expiry, concurrency count and active-slot insert on one scope.
use super::{Repository, Scope};
use crate::apps::registry::workbench::models::AgentTestSession;
use aidash_application::{
	Result,
	ports::registry::workbench::sandbox::admission::{AdmissionRepository, AdmissionScope},
};
use aidash_domain::registry::workbench::{
	Draft,
	sandbox::{TestLimits, TestSession},
};
use async_trait::async_trait;

use serde_json::Value;
use uuid::Uuid;
#[async_trait]
impl AdmissionRepository for Repository {
	async fn begin_admission(&self) -> Result<Box<dyn AdmissionScope + '_>> {
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
impl AdmissionScope for Scope {
	async fn complete_listing(&mut self, id: Uuid, mut scenario: Value) -> Result<TestSession> {
		scenario["starter_identity"] = starter(&self.actor);
		scenario["starter_subject"] = starter_subject(&self.actor);
		AgentTestSession::complete_listing(&mut self.tx, id, scenario)
			.await
			.map_err(Into::into)
	}
	async fn admit(
		&mut self,
		draft: &Draft,
		limits: &TestLimits,
		mut scenario: Value,
		conversation: Value,
	) -> Result<TestSession> {
		scenario["starter_identity"] = starter(&self.actor);
		scenario["starter_subject"] = starter_subject(&self.actor);
		AgentTestSession::admit(
			&mut self.tx,
			&draft.clone().into(),
			&limits.clone().into(),
			scenario,
			conversation,
		)
		.await
		.map_err(Into::into)
	}
}

fn starter(actor: &crate::authorization::identity::Actor) -> Value {
	match actor {
		crate::authorization::identity::Actor::Subject(identity) => identity
			.http_session
			.as_ref()
			.map(|session| serde_json::json!(session.identity_id))
			.unwrap_or(Value::Null),
		_ => Value::Null,
	}
}

fn starter_subject(actor: &crate::authorization::identity::Actor) -> Value {
	match actor {
		crate::authorization::identity::Actor::Subject(identity)
			if identity.http_session.is_some() =>
		{
			serde_json::json!(identity.subject)
		}
		_ => Value::Null,
	}
}
