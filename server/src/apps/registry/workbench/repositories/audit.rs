//! Native ORM records are converted at the audit boundary without importing rows into business crates.
use crate::apps::{
	identity::models::AuthorizationCatalogHistory,
	registry::workbench::{
		models::{AgentDraft, AgentDraftRegistration, AgentTestSession},
		services::core::incident,
	},
};
use crate::{
	authorization::{Authorization, identity::Actor},
	federation::Federation,
};
use aidash_application::{
	Result,
	ports::registry::workbench::audit::{AuditRepository, AuditScope},
};
use aidash_domain::{
	identity::Principal,
	policy::{Decision, Evaluation},
	registry::{
		EntityRef,
		workbench::{
			Draft,
			audit::{CatalogChange, IncidentChange, Registration, TestRecord},
		},
	},
};
use async_trait::async_trait;
use reinhardt::db::backends::TransactionExecutor;
use uuid::Uuid;
pub(crate) struct Repository<'a> {
	pub(crate) runtime: &'a Federation,
	pub(crate) actor: Actor,
}
struct Scope {
	tx: crate::database::native::Transaction,
	actor: Actor,
}
#[async_trait]
impl AuditRepository for Repository<'_> {
	fn principal(&self) -> Principal {
		super::authority::principal(&self.actor)
	}
	async fn begin(&self) -> Result<Box<dyn AuditScope + '_>> {
		Ok(Box::new(Scope {
			tx: crate::database::native::begin(&self.runtime.store.pool).await?,
			actor: self.actor.clone(),
		}))
	}
	async fn incidents(&self, reference: &EntityRef) -> Result<Vec<Uuid>> {
		incident::Incidents {
			runtime: self.runtime.clone(),
		}
		.list(
			self.actor.clone(),
			(reference.id.clone(), reference.version.clone()),
		)
		.await
		.map(|rows| rows.into_iter().map(|r| r.id).collect())
		.map_err(Into::into)
	}
	async fn incident_events(&self, id: Uuid) -> Result<Vec<IncidentChange>> {
		incident::read_events(self.runtime, &self.actor, id, 100)
			.await
			.map(|rows| {
				rows.into_iter()
					.map(|r| IncidentChange {
						actor: r.actor,
						change: r.change,
						created_at: r.created_at,
					})
					.collect()
			})
			.map_err(Into::into)
	}
}
#[async_trait]
impl AuditScope for Scope {
	async fn require_inspection(&mut self, reference: &EntityRef) -> Result<()> {
		let policy = self.tx.pool().dashboard_policy();
		aidash_application::registry::workbench::inspection::require(
			&mut crate::bootstrap::draft_authority_scope(&mut self.tx, &self.actor, policy),
			reference,
		)
		.await
	}
	async fn registrations(&mut self, reference: &EntityRef) -> Result<Vec<Registration>> {
		Ok(
			AgentDraftRegistration::for_agent(&mut self.tx, &reference.id, &reference.version, 100)
				.await?
				.into_iter()
				.map(|r| Registration {
					draft_id: r.draft_id(),
					revision: r.revision,
					actor: r.actor,
					registered_at: r.registered_at,
				})
				.collect(),
		)
	}
	async fn draft(&mut self, id: Uuid) -> Result<Draft> {
		Ok(AgentDraft::read(&mut self.tx, id, false).await?.into())
	}
	async fn authorize_draft(&mut self, draft: &Draft) -> Result<()> {
		let policy = self.tx.pool().dashboard_policy();
		aidash_application::registry::workbench::authorize(
			&mut crate::bootstrap::draft_authority_scope(&mut self.tx, &self.actor, policy),
			draft,
			"agent_draft.read",
			true,
		)
		.await
	}
	async fn test_records(&mut self, draft: Uuid, revision: i64) -> Result<Vec<TestRecord>> {
		Ok(
			AgentTestSession::evidence_page(&mut self.tx, draft, revision, 100)
				.await?
				.into_iter()
				.map(|r| TestRecord {
					id: r.id,
					status: r.status,
					usage: r.usage,
					created_at: r.created_at,
				})
				.collect(),
		)
	}
	async fn evaluate(&mut self, tenant: &str, input: &Evaluation) -> Result<Decision> {
		Authorization::evaluate_native(&mut self.tx, tenant, input)
			.await
			.map_err(Into::into)
	}
	async fn catalog_history(
		&mut self,
		tenant: &str,
		reference: &EntityRef,
	) -> Result<Vec<CatalogChange>> {
		Ok(AuthorizationCatalogHistory::for_entry(
			&mut self.tx,
			tenant,
			&reference.id,
			&reference.version,
		)
		.await?
		.into_iter()
		.map(|r| CatalogChange {
			revision: r.revision,
			enabled: r.enabled,
			actor: r.actor,
			created_at: r.created_at,
		})
		.collect())
	}
	async fn commit(self: Box<Self>) -> Result<()> {
		Box::new(self.tx)
			.commit()
			.await
			.map_err(crate::Error::from)
			.map_err(Into::into)
	}
}
