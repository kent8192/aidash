//! Bounded, source-attributed factual history for an exact agent version.
use super::*;
use crate::apps::identity::models::AuthorizationCatalogHistory;
use crate::apps::registry::workbench::models::AgentTestSession;
use crate::registry::EntityRef;
use reinhardt::injectable;

pub(crate) use crate::apps::registry::workbench::serializers::audit::AuditQuery;
pub use crate::apps::registry::workbench::serializers::audit::{AuditItem, AuditPage};

#[derive(Clone)]
pub struct AuditHistory {
	pub(crate) runtime: Federation,
}

#[injectable(scope = "request")]
pub async fn provide_audit(#[inject] runtime: Federation) -> AuditHistory {
	AuditHistory { runtime }
}

impl AuditHistory {
	pub(crate) async fn audit(
		&self,
		actor: Actor,
		(id, version): (String, String),
		query: AuditQuery,
	) -> Result<AuditPage> {
		let f = self.runtime.clone();
		if query.offset > 10_000 {
			return Err(Error::Invalid("audit offset too large".into()));
		}
		let tenant = match &actor {
			Actor::Operator => query.tenant.clone(),
			Actor::Subject(identity) => {
				if query
					.tenant
					.as_ref()
					.is_some_and(|tenant| tenant != &identity.tenant)
				{
					return Err(Error::Forbidden);
				}
				Some(identity.tenant.clone())
			}
		};
		let mut tx = PgTransactionExecutor::new(f.store.pool.begin().await?);
		trust::require_inspection(
			&mut tx,
			&actor,
			&EntityRef {
				id: id.clone(),
				version: version.clone(),
			},
		)
		.await?;
		let mut items = Vec::new();
		let registrations = AgentDraftRegistration::for_agent(&mut tx, &id, &version, 100).await?;
		for record in registrations {
			let draft_id = record.draft_id();
			let revision = record.revision;
			let registering_actor = record.actor;
			let at = record.registered_at;
			let draft = AgentDraft::read(&mut tx, draft_id, false).await?;
			if tenant
				.as_ref()
				.is_some_and(|tenant| tenant != &draft.tenant)
			{
				continue;
			}
			match authorize(&mut tx, &actor, &draft, "agent_draft.read", true).await {
				Ok(()) => {}
				Err(Error::Forbidden) => continue,
				Err(error) => return Err(error),
			}
			items.push(AuditItem {
				source: "registry".into(),
				kind: "registered".into(),
				at,
				actor: Some(registering_actor),
				details: json!({"draft_id":draft_id,"draft_revision":revision}),
			});
			let sessions =
				AgentTestSession::evidence_page(&mut tx, draft_id, revision, 100).await?;
			for session in sessions {
				let session_id = session.id;
				let status = session.status;
				let at = session.created_at;
				let usage = session.usage;
				items.push(AuditItem { source: "sandbox".into(), kind: "test".into(), at, actor: None, details: json!({"session_id":session_id,"draft_revision":revision,"status":status,"usage":usage}) });
			}
		}
		let may_read_catalog_history = match (&actor, &tenant) {
			(Actor::Operator, _) => true,
			(Actor::Subject(identity), Some(tenant)) => {
				Authorization::evaluate_native(
					&mut tx,
					tenant,
					&Evaluation {
						subject: identity.subject.clone(),
						action: "authorization_catalog.history.read".into(),
						resource: Resource {
							tenant: tenant.clone(),
							kind: "authorization_catalog".into(),
							id: format!("{id}@{version}"),
							attributes: json!({"entry_id":id,"entry_version":version}),
						},
						environment: json!({}),
					},
				)
				.await?
				.allowed
			}
			_ => false,
		};
		if may_read_catalog_history && let Some(tenant) = &tenant {
			let history =
				AuthorizationCatalogHistory::for_entry(&mut tx, tenant, &id, &version).await?;
			for record in history {
				let revision = record.revision;
				let enabled = record.enabled;
				let actor = record.actor;
				let at = record.created_at;
				items.push(AuditItem {
					source: "catalog".into(),
					kind: "binding_changed".into(),
					at,
					actor: Some(actor),
					details: json!({"tenant":tenant,"revision":revision,"enabled":enabled}),
				});
			}
		}
		Box::new(tx).commit().await?;
		let incidents = incident::Incidents { runtime: f.clone() }
			.list(actor.clone(), (id, version))
			.await?;
		for incident in incidents {
			let events = match incident::read_events(&f, &actor, incident.id, 100).await {
				Ok(events) => events,
				Err(Error::Forbidden) => continue,
				Err(error) => return Err(error),
			};
			for event in events {
				items.push(AuditItem {
					source: "incident".into(),
					kind: "incident_changed".into(),
					at: event.created_at,
					actor: Some(event.actor),
					details: json!({"incident_id":incident.id,"change":event.change}),
				});
			}
		}
		items.sort_by_key(|item| std::cmp::Reverse(item.at));
		let next_offset = (items.len() > query.offset + 50).then_some(query.offset + 50);
		let items = items.into_iter().skip(query.offset).take(50).collect();
		Ok(AuditPage { observed_at: Utc::now(), items, next_offset, source_boundary: "Connected-node Registry, authorized sandbox records, tenant Catalog history and visible incident history, limited to the latest 100 records per source. Other nodes and hidden records are not represented.".into() })
	}
}
