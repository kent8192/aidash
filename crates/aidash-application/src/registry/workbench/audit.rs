//! Factual history hides unauthorized sources and keeps their original transaction boundaries.
use crate::{Error, Result, ports::registry::workbench::audit::AuditRepository};
use aidash_domain::{
	identity::Principal,
	policy::{Evaluation, Resource},
	registry::{
		EntityRef,
		workbench::audit::{AuditItem, AuditPage, AuditQuery},
	},
};
use chrono::Utc;
use serde_json::json;
pub async fn read(
	repository: &dyn AuditRepository,
	reference: EntityRef,
	query: AuditQuery,
) -> Result<AuditPage> {
	if query.offset > 10_000 {
		return Err(Error::Invalid("audit offset too large".into()));
	}
	let actor = repository.principal();
	let tenant = match &actor {
		Principal::Operator => query.tenant.clone(),
		Principal::Subject { tenant, .. } => {
			if query.tenant.as_ref().is_some_and(|t| t != tenant) {
				return Err(Error::Forbidden);
			}
			Some(tenant.clone())
		}
	};
	let mut scope = repository.begin().await?;
	scope.require_inspection(&reference).await?;
	let mut items = Vec::new();
	for record in scope.registrations(&reference).await? {
		let draft_id = record.draft_id;
		let revision = record.revision;
		let draft = scope.draft(draft_id).await?;
		if tenant.as_ref().is_some_and(|t| t != &draft.tenant) {
			continue;
		}
		match scope.authorize_draft(&draft).await {
			Ok(()) => {}
			Err(Error::Forbidden) => continue,
			Err(e) => return Err(e),
		}
		items.push(AuditItem {
			source: "registry".into(),
			kind: "registered".into(),
			at: record.registered_at,
			actor: Some(record.actor),
			details: json!({"draft_id":draft_id,"draft_revision":revision}),
		});
		for session in scope.test_records(draft_id, revision).await? {
			items.push(AuditItem {
				source: "sandbox".into(),
				kind: "test".into(),
				at: session.created_at,
				actor: None,
				details: json!({
					"session_id": session.id,
					"draft_revision": revision,
					"status": session.status,
					"usage": session.usage,
				}),
			});
		}
	}
	let may_read_catalog_history = match (&actor, &tenant) {
		(Principal::Operator, _) => true,
		(Principal::Subject { subject, .. }, Some(tenant)) => {
			scope
				.evaluate(
					tenant,
					&Evaluation {
						subject: subject.clone(),
						action: "authorization_catalog.history.read".into(),
						resource: Resource {
							tenant: tenant.clone(),
							kind: "authorization_catalog".into(),
							id: super::ref_key(&reference),
							attributes: json!({"entry_id":reference.id,"entry_version":reference.version}),
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
		for record in scope.catalog_history(tenant, &reference).await? {
			items.push(AuditItem {
				source: "catalog".into(),
				kind: "binding_changed".into(),
				at: record.created_at,
				actor: Some(record.actor),
				details: json!({"tenant":tenant,"revision":record.revision,"enabled":record.enabled}),
			});
		}
	}
	scope.commit().await?;
	for id in repository.incidents(&reference).await? {
		let events = match repository.incident_events(id).await {
			Ok(e) => e,
			Err(Error::Forbidden) => continue,
			Err(e) => return Err(e),
		};
		for event in events {
			items.push(AuditItem {
				source: "incident".into(),
				kind: "incident_changed".into(),
				at: event.created_at,
				actor: Some(event.actor),
				details: json!({"incident_id":id,"change":event.change}),
			});
		}
	}
	items.sort_by_key(|item| std::cmp::Reverse(item.at));
	let next_offset = (items.len() > query.offset + 50).then_some(query.offset + 50);
	let items = items.into_iter().skip(query.offset).take(50).collect();
	Ok(AuditPage {
		observed_at: Utc::now(),
		items,
		next_offset,
		source_boundary: "Connected-node Registry, authorized sandbox records, tenant Catalog history and visible incident history, limited to the latest 100 records per source. Other nodes and hidden records are not represented.".into(),
	})
}
#[cfg(test)]
mod tests;
