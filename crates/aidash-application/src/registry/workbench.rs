//! Creator draft authorization and content admission use the same live authority scope.
use crate::{
	Error, Result,
	ports::registry::workbench::DraftAuthority,
	registry::{self, DefinitionValidation},
};
use aidash_domain::{
	identity::{Principal, authority::enabled},
	policy::{Evaluation, Resource},
	registry::{
		EntityRef, Entry, ReferenceDocument,
		workbench::{Draft, check_content, current_share},
	},
};
use serde_json::json;
pub fn author_identity(
	actor: &Principal,
	tenant: Option<&str>,
	owner: Option<&str>,
) -> Result<(String, String)> {
	match actor {
		Principal::Operator => {
			let tenant = tenant
				.filter(|s| !s.trim().is_empty())
				.ok_or_else(|| Error::Invalid("tenant is required".into()))?;
			let owner = owner
				.filter(|s| !s.trim().is_empty())
				.ok_or_else(|| Error::Invalid("owner is required".into()))?;
			Ok((tenant.into(), owner.into()))
		}
		Principal::Subject {
			tenant: local_tenant,
			subject,
		} => {
			if tenant.is_some_and(|value| value != local_tenant)
				|| owner.is_some_and(|value| value != subject)
			{
				return Err(Error::Forbidden);
			}
			Ok((local_tenant.clone(), subject.clone()))
		}
	}
}
pub fn owner_only(actor: &Principal, draft: &Draft) -> Result<()> {
	match actor {
		Principal::Operator => Ok(()),
		Principal::Subject { tenant, subject }
			if tenant == &draft.tenant && subject == &draft.owner =>
		{
			Ok(())
		}
		_ => Err(Error::Forbidden),
	}
}
pub fn ref_key(reference: &EntityRef) -> String {
	format!("{}@{}", reference.id, reference.version)
}
pub async fn authorize(
	scope: &mut dyn DraftAuthority,
	draft: &Draft,
	action: &str,
	shares: bool,
) -> Result<()> {
	let Principal::Subject { tenant, subject } = scope.principal() else {
		return Ok(());
	};
	if tenant != draft.tenant {
		return Err(Error::Forbidden);
	}
	scope.lock_identity().await?;
	let shared = if shares {
		scope.share(draft.id, &subject).await?
	} else {
		None
	};
	let shared = current_share(&draft.documents, shared);
	if subject != draft.owner
		&& match action {
			"agent_draft.read" => shared.is_none(),
			_ => shared.as_ref().is_none_or(|(can_edit, _)| !*can_edit),
		} {
		return Err(Error::Forbidden);
	}
	let decision=scope.evaluate(&draft.tenant,&Evaluation{subject,action:action.into(),resource:Resource{tenant:draft.tenant.clone(),kind:"agent_draft".into(),id:draft.id.to_string(),attributes:json!({"owner":draft.owner,"agent_id":draft.entry["id"],"archived":draft.archived})},environment:json!({})}).await?;
	if decision.allowed {
		Ok(())
	} else {
		Err(Error::Forbidden)
	}
}
pub async fn target_enabled(
	scope: &mut dyn DraftAuthority,
	tenant: &str,
	subject: &str,
) -> Result<()> {
	if enabled(&scope.bundle(tenant).await?, subject) {
		Ok(())
	} else {
		Err(Error::Invalid(
			"target subject must exist and be enabled in the same tenant".into(),
		))
	}
}
pub async fn validate_content(
	scope: &mut dyn DraftAuthority,
	validation: &DefinitionValidation,
	draft: &Draft,
	node: &str,
) -> Result<Entry> {
	let mut entry: Entry = serde_json::from_value(draft.entry.clone())?;
	let documents: Vec<ReferenceDocument> = serde_json::from_value(draft.documents.clone())?;
	check_content(&entry, &documents, &draft.release_notes)?;
	registry::bindings::private::detach(
		scope,
		&mut entry,
		node,
		draft
			.source_id
			.as_deref()
			.zip(draft.source_version.as_deref()),
	)
	.await?;
	let source = if documents.is_empty() {
		None
	} else {
		Some(registry::bindings::private::attach(
			&mut entry,
			node,
			&draft.documents,
		)?)
	};
	let snapshot = if let Some(source) = &source {
		let mut preview = registry::bindings::private::Preview { scope, source };
		let mut catalog = registry::bindings::catalog::LookupCatalog {
			definitions: &mut preview,
			node,
		};
		registry::bindings::resolve(
			&mut catalog,
			validation,
			aidash_domain::registry::bindings::QualifiedRef {
				registry_node: node.into(),
				id: entry.id.clone(),
				version: entry.version.clone(),
			},
			&entry,
			false,
		)
		.await?
	} else {
		let mut catalog = registry::bindings::catalog::LookupCatalog {
			definitions: scope,
			node,
		};
		registry::bindings::resolve(
			&mut catalog,
			validation,
			aidash_domain::registry::bindings::QualifiedRef {
				registry_node: node.into(),
				id: entry.id.clone(),
				version: entry.version.clone(),
			},
			&entry,
			false,
		)
		.await?
	};
	validation.bound_prompt_headroom(&snapshot, &json!({"reference_documents":draft.documents}))?;
	if let Principal::Subject { subject, .. } = scope.principal() {
		for dependency in &snapshot.definitions {
			if dependency.identity == snapshot.agent
				|| source.as_ref().is_some_and(|s| {
					dependency.identity.registry_node == node
						&& s.id == dependency.identity.id
						&& s.version == dependency.identity.version
				}) {
				continue;
			}
			let reference = dependency.identity.local();
			let decision = scope.evaluate(&draft.tenant, &Evaluation { subject: subject.clone(), action: "agent_dependency.read".into(), resource: Resource { tenant: draft.tenant.clone(), kind: "registry_entry".into(), id: ref_key(&reference), attributes: json!({"id":reference.id,"version":reference.version,"registry_node":dependency.identity.registry_node}) }, environment: json!({}) }).await?;
			if !decision.allowed {
				return Err(Error::Forbidden);
			}
		}
	}
	entry.normalize_agent(node)?;
	Ok(entry)
}
#[cfg(test)]
mod tests;

pub mod drafts;

pub mod publication;

pub mod audit;
pub mod inspection;

pub mod permissions;

pub mod incidents;

pub mod report;

pub mod profile;

pub mod sandbox;
