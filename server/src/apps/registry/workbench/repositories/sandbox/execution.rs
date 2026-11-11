//! Current inference authority and running-only progress/finalization keep their original transactions.
use super::{Repository, Scope};
use crate::apps::registry::workbench::models::AgentTestSession;
use aidash_application::{
	Result,
	ports::registry::workbench::sandbox::execution::{
		ExecutionRepository, ExecutionScope, TestApproval,
	},
};
use aidash_domain::registry::{
	Entry,
	workbench::{
		Draft,
		sandbox::{TestOutcome, TestSession},
	},
};
use async_trait::async_trait;

use serde_json::Value;
use uuid::Uuid;
#[async_trait]
impl ExecutionRepository for Repository {
	fn search_destination(&self, provider: &str, arguments: &Value) -> Result<Option<String>> {
		(self.search_destination)(provider, arguments)
	}
	async fn approve_once(&self, id: Uuid, approval: &TestApproval) -> Result<bool> {
		if approval.action == "network.get"
			&& (!self.store.capabilities.0.admission
				|| !self
					.store
					.capabilities
					.0
					.outbound_origins
					.contains(&approval.resource.id))
		{
			return Ok(false);
		}
		let crate::authorization::identity::Actor::Subject(identity) = &self.actor else {
			return Ok(false);
		};
		let person = identity
			.http_session
			.as_ref()
			.map(|session| session.identity_id);
		let request_id = Uuid::now_v7();
		{
			let mut tx = self.store.pool.begin(&self.actor.tenancy()?).await?;
			let policy = tx.pool().dashboard_policy();
			let snapshot = match identity.lock_native(&mut tx, false, policy.as_ref()).await {
				Ok(snapshot) => snapshot,
				Err(crate::Error::Forbidden | crate::Error::Unauthorized) => return Ok(false),
				Err(error) => return Err(error.into()),
			};
			let session = AgentTestSession::read(&mut tx, id, true).await?;
			if session.status != "running" {
				return Ok(false);
			}
			let evaluation = aidash_domain::policy::Evaluation {
				subject: identity.subject.clone(),
				action: approval.action.into(),
				resource: approval.resource.clone(),
				environment: serde_json::json!({}),
			};
			let direct = aidash_domain::capabilities::approvals::workbench_policy(&snapshot.bundle)
				.evaluate(&evaluation);
			if direct.allowed {
				return Ok(true);
			}
			if direct.reason == "no_matching_allow" && !approval.requires_approval {
				return Ok(true);
			}
			let Some(person) = person else {
				return Ok(false);
			};
			if session.scenario["starter_identity"] != serde_json::json!(person)
				|| session.scenario["starter_subject"] != identity.subject
			{
				return Ok(false);
			}
			if direct.reason != "no_matching_allow"
				|| !aidash_domain::capabilities::approvals::workbench_approver_eligible(
					&snapshot.bundle,
					&identity.subject,
					&approval.resource,
				) {
				return Ok(false);
			}
			let mut scenario = session.scenario;
			scenario["pending_approval"] = serde_json::json!({"id":request_id,"call":approval.call,"resource":approval.resource,"action":approval.action,"state":"pending"});
			AgentTestSession::complete_listing(&mut tx, id, scenario).await?;
			tx.commit().await?;
		}
		loop {
			tokio::time::sleep(std::time::Duration::from_millis(500)).await;
			let mut tx = self.store.pool.begin(&self.actor.tenancy()?).await?;
			let policy = tx.pool().dashboard_policy();
			let snapshot = match identity.lock_native(&mut tx, false, policy.as_ref()).await {
				Ok(snapshot) => snapshot,
				Err(crate::Error::Forbidden | crate::Error::Unauthorized) => return Ok(false),
				Err(error) => return Err(error.into()),
			};
			let session = AgentTestSession::read(&mut tx, id, true).await?;
			let pending = &session.scenario["pending_approval"];
			if session.status != "running" || pending["id"] != serde_json::json!(request_id) {
				return Ok(false);
			}
			if pending["state"] == "deny" {
				return Ok(false);
			}
			let evaluation = aidash_domain::policy::Evaluation {
				subject: identity.subject.clone(),
				action: approval.action.into(),
				resource: approval.resource.clone(),
				environment: serde_json::json!({}),
			};
			let decision =
				aidash_domain::capabilities::approvals::workbench_policy(&snapshot.bundle)
					.evaluate(&evaluation);
			if decision.allowed
				|| decision.reason != "no_matching_allow"
				|| !aidash_domain::capabilities::approvals::workbench_approver_eligible(
					&snapshot.bundle,
					&identity.subject,
					&approval.resource,
				) {
				let mut scenario = session.scenario;
				scenario["pending_approval"]["state"] = serde_json::json!(if decision.allowed {
					"superseded"
				} else {
					"refused"
				});
				AgentTestSession::complete_listing(&mut tx, id, scenario).await?;
				tx.commit().await?;
				return Ok(decision.allowed);
			}
			match pending["state"].as_str() {
				Some("pending") => {
					tx.commit().await?;
				}
				Some("allow_once") => return Ok(true),
				_ => return Ok(false),
			}
		}
	}
	async fn begin_execution(&self) -> Result<Box<dyn ExecutionScope + '_>> {
		Ok(Box::new(Scope {
			tx: self.store.pool.begin(&self.actor.tenancy()?).await?,
			actor: self.actor.clone(),
			tenant_actions: self.tenant_actions.clone(),
			node_id: self.node_id.clone(),
			ceilings: self.store.test_limit_ceilings,
			tenant_secrets: self.store.tenant_secrets.clone(),
		}))
	}
	async fn read_session(&self, id: Uuid) -> Result<TestSession> {
		self.store
			.pool
			.atomic(&self.actor.tenancy()?, async |tx| {
				AgentTestSession::read(tx, id, false).await
			})
			.await
			.map_err(Into::into)
	}
	async fn progress(
		&self,
		id: Uuid,
		conversation: Value,
		calls: Value,
		usage: Value,
	) -> Result<()> {
		self.store
			.pool
			.atomic(&self.actor.tenancy()?, async |tx| {
				AgentTestSession::progress(tx, id, conversation, calls, usage).await
			})
			.await
			.map_err(Into::into)
	}
	async fn finish(&self, id: Uuid, outcome: &TestOutcome) -> Result<()> {
		self.store
			.pool
			.atomic(&self.actor.tenancy()?, async |tx| {
				AgentTestSession::finish(
					tx,
					id,
					&outcome.status,
					outcome.conversation.clone(),
					outcome.tool_calls.clone(),
					outcome.usage.clone(),
					outcome.error.clone(),
				)
				.await
			})
			.await
			.map_err(Into::into)
	}
}
#[async_trait]
impl ExecutionScope for Scope {
	async fn bindings(
		&mut self,
		draft: &Draft,
		entry: &Entry,
	) -> Result<aidash_domain::registry::bindings::BindingSnapshot> {
		let mut lookup = super::super::authority::Scope {
			policy: self.tx.pool().dashboard_policy(),
			tx: &mut *self.tx,
			actor: &self.actor,
		};
		let source = if draft
			.documents
			.as_array()
			.is_some_and(|docs| !docs.is_empty())
		{
			let mut raw: Entry = serde_json::from_value(draft.entry.clone())?;
			Some(aidash_application::registry::bindings::private::attach(
				&mut raw,
				&self.node_id,
				&draft.documents,
			)?)
		} else {
			None
		};
		let definitions: &mut dyn aidash_application::ports::registry::DefinitionLookup =
			if let Some(source) = &source {
				&mut aidash_application::registry::bindings::private::Preview {
					scope: &mut lookup,
					source,
				}
			} else {
				&mut lookup
			};
		aidash_application::registry::bindings::resolve(
			&mut aidash_application::registry::bindings::catalog::LookupCatalog {
				definitions,
				node: &self.node_id,
			},
			&crate::bootstrap::registry_validation(),
			aidash_domain::registry::bindings::QualifiedRef {
				registry_deployment: self.node_id.clone(),
				id: entry.id.clone(),
				version: entry.version.clone(),
			},
			entry,
			false,
		)
		.await
	}

	async fn validate_content(&mut self, draft: &Draft) -> Result<Entry> {
		let policy = self.tx.pool().dashboard_policy();
		aidash_application::registry::workbench::validate_content(
			&mut crate::bootstrap::draft_authority_scope(&mut self.tx, &self.actor, policy),
			&crate::bootstrap::registry_validation(),
			draft,
			&self.node_id,
		)
		.await
	}
}
