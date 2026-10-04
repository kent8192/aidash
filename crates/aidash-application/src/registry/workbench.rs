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
		AgentConfig, EntityRef, Entry, ReferenceDocument,
		knowledge::digest,
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
	if !documents.is_empty() {
		entry.config["knowledge_digest"] = json!(digest(&draft.documents))
	} else if let Some(config) = entry.config.as_object_mut() {
		config.remove("knowledge_digest");
	}
	if let Principal::Subject { subject, .. } = scope.principal() {
		let config: AgentConfig = serde_json::from_value(entry.config.clone())?;
		for reference in std::iter::once(&config.model)
			.chain(config.tools.iter())
			.chain(config.skills.iter())
			.chain(config.cluster.iter())
		{
			let decision = scope
				.evaluate(
					&draft.tenant,
					&Evaluation {
						subject: subject.clone(),
						action: "agent_dependency.read".into(),
						resource: Resource {
							tenant: draft.tenant.clone(),
							kind: "registry_entry".into(),
							id: ref_key(reference),
							attributes: json!({"id":reference.id,"version":reference.version}),
						},
						environment: json!({}),
					},
				)
				.await?;
			if !decision.allowed {
				return Err(Error::Forbidden);
			}
		}
	}
	registry::validate_references(scope, validation, &entry, node).await?;
	if !documents.is_empty() {
		let config: AgentConfig = serde_json::from_value(entry.config.clone())?;
		let mut references = Vec::new();
		for reference in std::iter::once(&config.model)
			.chain(config.tools.iter())
			.chain(config.skills.iter())
			.chain(config.cluster.iter())
		{
			references.push(registry::effective(scope, &reference.id, &reference.version).await?);
		}
		validation.agent_prompt_headroom(
			&config,
			&references,
			&json!({"reference_documents":draft.documents}),
		)?;
	}
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
