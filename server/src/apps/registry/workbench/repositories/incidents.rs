//! Native incident storage preserves update locks, atomic history and retention SKIP LOCKED batches.
use crate::apps::registry::workbench::models::{AgentIncident, AgentIncidentEvent, AgentTestLimit};
use crate::{
	authorization::{Authorization, identity::Actor},
	federation::Federation,
};
use aidash_application::{
	Result,
	ports::registry::workbench::incidents::{
		IncidentRepository, IncidentRetentionRepository, IncidentRetentionScope, IncidentScope,
	},
};
use aidash_domain::{
	identity::Principal,
	policy::{Decision, Evaluation},
	registry::{
		EntityRef, Entry,
		workbench::incident::{Incident, IncidentEvent},
	},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reinhardt::db::backends::TransactionExecutor;
use serde_json::Value;
use uuid::Uuid;
pub(crate) struct Repository<'a> {
	pub(crate) runtime: &'a Federation,
	pub(crate) actor: Actor,
}
pub(crate) struct RetentionRepository<'a> {
	pub(crate) pool: &'a crate::database::native::Pool,
}
struct Scope {
	tx: crate::database::native::Transaction,
	actor: Actor,
}
#[async_trait]
impl IncidentRepository for Repository<'_> {
	fn principal(&self) -> Principal {
		super::authority::principal(&self.actor)
	}
	async fn begin(&self) -> Result<Box<dyn IncidentScope + '_>> {
		Ok(Box::new(Scope {
			tx: crate::database::native::begin(&self.runtime.store.pool).await?,
			actor: self.actor.clone(),
		}))
	}
}
#[async_trait]
impl IncidentScope for Scope {
	fn principal(&self) -> Principal {
		super::authority::principal(&self.actor)
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
	async fn evaluate(&mut self, tenant: &str, input: &Evaluation) -> Result<Decision> {
		Authorization::evaluate_native(&mut self.tx, tenant, input)
			.await
			.map_err(Into::into)
	}
	async fn require_inspection(&mut self, reference: &EntityRef) -> Result<()> {
		let policy = self.tx.pool().dashboard_policy();
		aidash_application::registry::workbench::inspection::require(
			&mut crate::bootstrap::draft_authority_scope(&mut self.tx, &self.actor, policy),
			reference,
		)
		.await
	}
	async fn effective(&mut self, reference: &EntityRef) -> Result<Entry> {
		crate::apps::registry::services::admission::effective(
			&mut self.tx,
			&reference.id,
			&reference.version,
		)
		.await
		.map_err(Into::into)
	}
	async fn target_enabled(&mut self, tenant: &str, subject: &str) -> Result<()> {
		let policy = self.tx.pool().dashboard_policy();
		aidash_application::registry::workbench::target_enabled(
			&mut crate::bootstrap::draft_authority_scope(&mut self.tx, &Actor::Operator, policy),
			tenant,
			subject,
		)
		.await
	}
	async fn read(&mut self, id: Uuid, lock: bool) -> Result<Incident> {
		AgentIncident::read(&mut self.tx, id, lock)
			.await
			.map_err(Into::into)
	}
	async fn page(
		&mut self,
		reference: &EntityRef,
		tenant: Option<&str>,
		cursor: Option<(DateTime<Utc>, Uuid)>,
	) -> Result<Vec<Incident>> {
		AgentIncident::page(
			&mut self.tx,
			&reference.id,
			&reference.version,
			tenant,
			cursor,
		)
		.await
		.map_err(Into::into)
	}
	async fn insert(&mut self, incident: &Incident) -> Result<()> {
		AgentIncident::insert(&mut self.tx, incident)
			.await
			.map_err(Into::into)
	}
	async fn save(&mut self, incident: &Incident) -> Result<()> {
		AgentIncident::save(&mut self.tx, incident)
			.await
			.map_err(Into::into)
	}
	async fn record_event(&mut self, id: Uuid, actor: &str, change: Value) -> Result<()> {
		AgentIncidentEvent::record(&mut self.tx, id, actor, change)
			.await
			.map_err(Into::into)
	}
	async fn events(&mut self, id: Uuid, limit: usize) -> Result<Vec<IncidentEvent>> {
		AgentIncidentEvent::page(&mut self.tx, id, limit)
			.await
			.map_err(Into::into)
	}
	async fn evidence_days(&mut self, tenant: &str) -> Result<i32> {
		Ok(AgentTestLimit::locked(&mut self.tx, tenant)
			.await?
			.incident_evidence_days)
	}
	async fn commit(self: Box<Self>) -> Result<()> {
		Box::new(self.tx)
			.commit()
			.await
			.map_err(crate::Error::from)
			.map_err(Into::into)
	}
}
#[async_trait]
impl IncidentRetentionRepository for RetentionRepository<'_> {
	async fn begin(&self) -> Result<Box<dyn IncidentRetentionScope + '_>> {
		Ok(Box::new(Scope {
			tx: crate::database::native::begin(self.pool).await?,
			actor: Actor::Operator,
		}))
	}
}
#[async_trait]
impl IncidentRetentionScope for Scope {
	async fn expired(&mut self) -> Result<Vec<Incident>> {
		AgentIncident::expired(&mut self.tx)
			.await
			.map_err(Into::into)
	}
	async fn discard(&mut self, id: Uuid, evidence: Value) -> Result<()> {
		AgentIncident::discard_evidence(&mut self.tx, id, evidence)
			.await
			.map_err(Into::into)
	}
	async fn commit(self: Box<Self>) -> Result<()> {
		Box::new(self.tx)
			.commit()
			.await
			.map_err(crate::Error::from)
			.map_err(Into::into)
	}
}
