//! Inspection uses one native audit allocation for usage and contained resource checks.
use crate::apps::{
	execution::models::Run,
	registry::workbench::models::{AgentDraft, AgentDraftRegistration, AgentTestSession},
	workspaces::models::Workspace,
};
use crate::{
	authorization::{access::NativeAccess, identity::Actor},
	federation::Federation,
};
use aidash_application::{
	Result,
	ports::registry::workbench::inspection::{InspectionRepository, InspectionScope},
};
use aidash_domain::{
	RunMetadata,
	identity::Principal,
	policy::Resource,
	registry::{
		EntityRef, Entry,
		workbench::{
			Draft,
			audit::Registration,
			inspection::{Inspection, TestObservation},
		},
	},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reinhardt::db::backends::{TransactionExecutor, dialect::postgres::PgTransactionExecutor};
use uuid::Uuid;
pub(crate) struct Repository<'a> {
	pub(crate) runtime: &'a Federation,
	pub(crate) actor: Actor,
}
// Opening another audited transaction while this lease is live would wait on its own advisory lock.
enum Lease {
	Operator(Box<dyn TransactionExecutor>),
	Subject(Box<NativeAccess>),
}
struct Scope {
	lease: Lease,
	actor: Actor,
}
impl Scope {
	fn tx(&mut self) -> &mut dyn TransactionExecutor {
		match &mut self.lease {
			Lease::Operator(tx) => tx.as_mut(),
			Lease::Subject(access) => access.tx.as_mut(),
		}
	}
}
#[async_trait]
impl InspectionRepository for Repository<'_> {
	fn node_id(&self) -> &str {
		&self.runtime.config.node_id
	}
	async fn begin(&self) -> Result<Box<dyn InspectionScope + '_>> {
		let lease = match &self.actor {
			Actor::Operator => Lease::Operator(Box::new(PgTransactionExecutor::new(
				self.runtime
					.store
					.pool
					.begin()
					.await
					.map_err(crate::Error::from)?,
			))),
			Actor::Subject(identity) => Lease::Subject(Box::new(
				NativeAccess::begin(&self.runtime.store, identity).await?,
			)),
		};
		Ok(Box::new(Scope {
			lease,
			actor: self.actor.clone(),
		}))
	}
}
#[async_trait]
impl InspectionScope for Scope {
	fn principal(&self) -> Principal {
		super::authority::principal(&self.actor)
	}
	async fn require_inspection(&mut self, reference: &EntityRef) -> Result<()> {
		let actor = self.actor.clone();
		aidash_application::registry::workbench::inspection::require(
			&mut crate::bootstrap::draft_authority_scope(self.tx(), &actor),
			reference,
		)
		.await
	}
	async fn effective(&mut self, reference: &EntityRef) -> Result<Entry> {
		crate::apps::registry::services::admission::effective(
			self.tx(),
			&reference.id,
			&reference.version,
		)
		.await
		.map_err(Into::into)
	}
	async fn agent_runs(
		&mut self,
		reference: &EntityRef,
		cursor: Option<(DateTime<Utc>, Uuid)>,
	) -> Result<Vec<RunMetadata>> {
		Run::agent_page(self.tx(), &reference.id, &reference.version, cursor)
			.await
			.map_err(Into::into)
	}
	async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
		match &mut self.lease {
			Lease::Subject(access) => access.workspace(id).await.map_err(Into::into),
			Lease::Operator(_) => Err(aidash_application::Error::Invalid(
				"operator inspection does not require a subject workspace resource".into(),
			)),
		}
	}
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		match &mut self.lease {
			Lease::Subject(access) => access.decide(resource, action).await.map_err(Into::into),
			Lease::Operator(_) => Ok(true),
		}
	}
	async fn run_visible(&mut self, run: &RunMetadata) -> Result<bool> {
		match &mut self.lease {
			Lease::Subject(access) => access.run_visible(run).await.map_err(Into::into),
			Lease::Operator(_) => Ok(true),
		}
	}
	async fn workspace_title(&mut self, id: Uuid) -> Result<Option<String>> {
		Workspace::title_in(self.tx(), id).await.map_err(Into::into)
	}
	async fn registrations(&mut self, reference: &EntityRef) -> Result<Vec<Registration>> {
		Ok(
			AgentDraftRegistration::for_agent(self.tx(), &reference.id, &reference.version, 1)
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
		Ok(AgentDraft::read(self.tx(), id, false).await?.into())
	}
	async fn authorize_draft(&mut self, draft: &Draft) -> Result<()> {
		let actor = self.actor.clone();
		aidash_application::registry::workbench::authorize(
			&mut crate::bootstrap::draft_authority_scope(self.tx(), &actor),
			draft,
			"agent_draft.read",
			true,
		)
		.await
	}
	async fn test_observations(
		&mut self,
		draft: Uuid,
		revision: i64,
	) -> Result<Vec<TestObservation>> {
		Ok(
			AgentTestSession::evidence_page(self.tx(), draft, revision, 101)
				.await?
				.into_iter()
				.map(|r| TestObservation {
					id: r.id,
					status: r.status,
					scenario: r.scenario,
					usage: r.usage,
					created_at: r.created_at,
					expires_at: r.expires_at,
					expired_at: r.expired_at,
				})
				.collect(),
		)
	}
	async fn finish(self: Box<Self>, inspection: Inspection) -> Result<Inspection> {
		match self.lease {
			Lease::Operator(tx) => {
				tx.commit().await.map_err(crate::Error::from)?;
				Ok(inspection)
			}
			Lease::Subject(access) => (*access).finish(Ok(inspection)).await.map_err(Into::into),
		}
	}
}
