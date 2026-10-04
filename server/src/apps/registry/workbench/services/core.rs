//! Tenant-scoped Creator drafts and immutable Registry admission. Workbench
//! permissions are separate from installation-wide Registry administration.
use crate::apps::execution::models::event_records;
use crate::apps::identity::models::AuthorizationBundle;
use crate::apps::registry::models::transaction_records;
use crate::apps::registry::services::admission;
use crate::apps::registry::workbench::models::{
	AgentDraft, AgentDraftRegistration, AgentDraftShare,
};
use crate::{
	Error, Result,
	authorization::{
		Authorization,
		identity::Actor,
		policy::{Evaluation, Resource},
	},
	federation::Federation,
	knowledge::{ReferenceDocument, digest, validate as validate_documents},
	registry::{AgentConfig, Entry},
};
use reinhardt::db::backends::{TransactionExecutor, dialect::postgres::PgTransactionExecutor};
use reinhardt::injectable;

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use uuid::Uuid;

#[path = "audit.rs"]
pub mod audit;
#[path = "incident.rs"]
pub mod incident;
#[path = "profile.rs"]
pub mod profile;
#[path = "test.rs"]
pub mod test;
#[path = "trust.rs"]
pub mod trust;
pub use incident::purge_expired as purge_incident_evidence;
pub use test::purge_expired;

fn author_identity(
	actor: &Actor,
	tenant: Option<&str>,
	owner: Option<&str>,
) -> Result<(String, String)> {
	match actor {
		Actor::Operator => {
			let tenant = tenant
				.filter(|s| !s.trim().is_empty())
				.ok_or_else(|| Error::Invalid("tenant is required".into()))?;
			let owner = owner
				.filter(|s| !s.trim().is_empty())
				.ok_or_else(|| Error::Invalid("owner is required".into()))?;
			Ok((tenant.into(), owner.into()))
		}
		Actor::Subject(identity) => {
			if tenant.is_some_and(|value| value != identity.tenant)
				|| owner.is_some_and(|value| value != identity.subject)
			{
				return Err(Error::Forbidden);
			}
			Ok((identity.tenant.clone(), identity.subject.clone()))
		}
	}
}

async fn authorize(
	tx: &mut dyn TransactionExecutor,
	actor: &Actor,
	draft: &Draft,
	action: &str,
	shares: bool,
) -> Result<()> {
	let Actor::Subject(identity) = actor else {
		return Ok(());
	};
	if identity.tenant != draft.tenant {
		return Err(Error::Forbidden);
	}
	identity.lock_native(tx, false).await?;
	let shared = if shares {
		AgentDraftShare::current(tx, draft.id, &identity.subject)
			.await?
			.map(|row| (row.can_edit, row.documents_digest))
	} else {
		None
	};
	let current_share =
		shared.filter(|(_, documents_digest)| documents_digest == &digest(&draft.documents));
	if identity.subject != draft.owner
		&& match action {
			"agent_draft.read" => current_share.is_none(),
			_ => current_share.as_ref().is_none_or(|(can_edit, _)| !can_edit),
		} {
		return Err(Error::Forbidden);
	}
	let decision = Authorization::evaluate_native(
		tx,
		&draft.tenant,
		&Evaluation {
			subject: identity.subject.clone(),
			action: action.into(),
			resource: Resource {
				tenant: draft.tenant.clone(),
				kind: "agent_draft".into(),
				id: draft.id.to_string(),
				attributes: json!({"owner":draft.owner,"agent_id":draft.entry["id"],"archived":draft.archived}),
			},
			environment: json!({}),
		},
	)
	.await?;
	if decision.allowed {
		Ok(())
	} else {
		Err(Error::Forbidden)
	}
}

fn check_content(
	entry: &Entry,
	documents: &[ReferenceDocument],
	release_notes: &str,
) -> Result<()> {
	if entry.kind != "agent" || entry.id.is_empty() || entry.id.len() > 100 {
		return Err(Error::Invalid(
			"draft must contain a managed agent identity".into(),
		));
	}
	if !documents.is_empty() {
		validate_documents(documents)?;
	}
	if release_notes.len() > 8192 {
		return Err(Error::Invalid("release notes exceed 8 KiB".into()));
	}
	let _: AgentConfig =
		serde_json::from_value(entry.config.clone()).map_err(|e| Error::Invalid(e.to_string()))?;
	Ok(())
}

fn new_draft_defaults(entry: &mut Entry) -> Result<()> {
	let config = entry
		.config
		.as_object_mut()
		.ok_or_else(|| Error::Invalid("agent config must be an object".into()))?;
	for key in [
		"allow_task_creation",
		"allow_task_delegation",
		"allow_memory_write",
		"allow_workspace_retrieval",
		"allow_cross_conversation_memory",
	] {
		match config.get(key) {
			Some(Value::Bool(_)) => {}
			None => {
				config.insert(key.into(), json!(false));
			}
			Some(_) => return Err(Error::Invalid(format!("{key} must be a boolean"))),
		}
	}
	Ok(())
}

async fn validate_content(
	f: &Federation,
	draft: &Draft,
	actor: &Actor,
	tx: &mut dyn TransactionExecutor,
) -> Result<Entry> {
	let mut entry: Entry = serde_json::from_value(draft.entry.clone())?;
	let documents: Vec<ReferenceDocument> = serde_json::from_value(draft.documents.clone())?;
	check_content(&entry, &documents, &draft.release_notes)?;
	if !documents.is_empty() {
		entry.config["knowledge_digest"] = json!(digest(&draft.documents));
	} else if let Some(config) = entry.config.as_object_mut() {
		config.remove("knowledge_digest");
	}
	if let Actor::Subject(identity) = actor {
		let config: AgentConfig = serde_json::from_value(entry.config.clone())?;
		for reference in std::iter::once(&config.model)
			.chain(config.tools.iter())
			.chain(config.skills.iter())
			.chain(config.cluster.iter())
		{
			let decision = Authorization::evaluate_native(
				tx,
				&draft.tenant,
				&Evaluation {
					subject: identity.subject.clone(),
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
	admission::validate_references(tx, &entry, &f.config.node_id).await?;
	if !documents.is_empty() {
		let config: AgentConfig = serde_json::from_value(entry.config.clone())?;
		let mut references = Vec::new();
		for reference in std::iter::once(&config.model)
			.chain(config.tools.iter())
			.chain(config.skills.iter())
			.chain(config.cluster.iter())
		{
			references.push(admission::effective(tx, &reference.id, &reference.version).await?);
		}
		crate::registry::validate_agent_prompt(
			&config,
			&references,
			&json!({"reference_documents":draft.documents}),
		)?;
	}
	Ok(entry)
}

fn ref_key(reference: &crate::registry::EntityRef) -> String {
	format!("{}@{}", reference.id, reference.version)
}

async fn target_enabled(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	subject: &str,
) -> Result<()> {
	let snapshot = AuthorizationBundle::lock_snapshot(tx, tenant, false).await?;
	if crate::authorization::identity::enabled(&snapshot, subject) {
		Ok(())
	} else {
		Err(Error::Invalid(
			"target subject must exist and be enabled in the same tenant".into(),
		))
	}
}

fn owner_only(actor: &Actor, draft: &Draft) -> Result<()> {
	match actor {
		Actor::Operator => Ok(()),
		Actor::Subject(identity)
			if identity.tenant == draft.tenant && identity.subject == draft.owner =>
		{
			Ok(())
		}
		_ => Err(Error::Forbidden),
	}
}

pub(crate) use crate::apps::registry::workbench::serializers::contracts::DraftPage;
pub use crate::apps::registry::workbench::serializers::contracts::{
	AdoptInput, ArchiveInput, CreateDraft, Draft, DraftShare, RegisteredVersion, Registration,
	RevisionInput, SaveDraft, ShareInput, TransferInput, Validation,
};

#[derive(Clone)]
pub struct Drafts {
	pub(crate) runtime: Federation,
}

#[injectable(scope = "request")]
pub async fn provide_drafts(#[inject] runtime: Federation) -> Drafts {
	Drafts { runtime }
}

impl Drafts {
	pub(crate) async fn create(&self, actor: Actor, mut input: CreateDraft) -> Result<Draft> {
		let f = self.runtime.clone();
		let (tenant, owner) =
			author_identity(&actor, input.tenant.as_deref(), input.owner.as_deref())?;
		let id = Uuid::now_v7();
		if !input.entry.id.is_empty() {
			return Err(Error::Invalid("new draft must leave entry.id empty; use an authorized version flow for existing identities".into()));
		}
		input.entry.id = id.to_string();
		new_draft_defaults(&mut input.entry)?;
		check_content(&input.entry, &input.documents, &input.release_notes)?;
		let entry = serde_json::to_value(&input.entry)?;
		let documents = serde_json::to_value(&input.documents)?;
		let mut tx = PgTransactionExecutor::new(f.store.pool.begin().await?);
		target_enabled(&mut tx, &tenant, &owner).await?;
		let prospective = Draft {
			id,
			tenant,
			owner,
			revision: 1,
			entry: entry.clone(),
			documents: documents.clone(),
			release_notes: input.release_notes.clone(),
			source_id: None,
			source_version: None,
			archived: false,
			updated_at: Utc::now(),
		};
		authorize(&mut tx, &actor, &prospective, "agent_draft.create", false).await?;
		let saved = AgentDraft::insert(&mut tx, &prospective, &input.entry.id).await?;
		Box::new(tx).commit().await?;
		Ok(saved)
	}
	pub(crate) async fn list(&self, actor: Actor, page: DraftPage) -> Result<Vec<Draft>> {
		let f = self.runtime.clone();
		let mut tx = PgTransactionExecutor::new(f.store.pool.begin().await?);
		let mut visible = Vec::new();
		let mut cursor = match (page.before_updated_at, page.before_id) {
			(None, None) => None,
			(Some(updated_at), Some(id)) => Some((updated_at, id)),
			_ => {
				return Err(Error::Invalid(
					"draft cursor requires both timestamp and ID".into(),
				));
			}
		};
		let tenant = match &actor {
			Actor::Operator => None,
			Actor::Subject(identity) => Some(&identity.tenant),
		};
		loop {
			let rows = AgentDraft::page(&mut tx, tenant.map(String::as_str), cursor).await?;
			let more = rows.len() == 100;
			cursor = rows.last().map(|row| (row.updated_at, row.id));
			for row in rows {
				match authorize(&mut tx, &actor, &row, "agent_draft.read", true).await {
					Ok(()) => visible.push(row),
					Err(Error::Forbidden) => {}
					Err(error) => return Err(error),
				}
				if visible.len() == 100 {
					break;
				}
			}
			if visible.len() == 100 || !more {
				break;
			}
		}
		Box::new(tx).commit().await?;
		Ok(visible)
	}
	pub(crate) async fn get(&self, actor: Actor, id: Uuid) -> Result<Draft> {
		let f = self.runtime.clone();
		let mut tx = PgTransactionExecutor::new(f.store.pool.begin().await?);
		let draft = AgentDraft::read(&mut tx, id, false).await?;
		authorize(&mut tx, &actor, &draft, "agent_draft.read", true).await?;
		Box::new(tx).commit().await?;
		Ok(draft)
	}
	pub(crate) async fn save(&self, actor: Actor, id: Uuid, mut input: SaveDraft) -> Result<Draft> {
		let f = self.runtime.clone();
		let mut tx = PgTransactionExecutor::new(f.store.pool.begin().await?);
		let draft = AgentDraft::read(&mut tx, id, true).await?;
		authorize(&mut tx, &actor, &draft, "agent_draft.write", true).await?;
		if draft.revision != input.expected_revision {
			return Err(Error::Conflict(
				"draft revision changed; local edits were not saved".into(),
			));
		}
		if draft.archived {
			return Err(Error::Conflict(
				"restore the archived draft before editing".into(),
			));
		}
		if input.entry.id != draft.entry["id"].as_str().unwrap_or_default() {
			return Err(Error::Invalid(
				"managed agent identity cannot change".into(),
			));
		}
		new_draft_defaults(&mut input.entry)?;
		check_content(&input.entry, &input.documents, &input.release_notes)?;
		let entry = serde_json::to_value(&input.entry)?;
		let documents = serde_json::to_value(&input.documents)?;
		let saved =
			AgentDraft::save_content(&mut tx, id, entry, documents, &input.release_notes).await?;
		Box::new(tx).commit().await?;
		Ok(saved)
	}
	pub(crate) async fn duplicate(
		&self,
		actor: Actor,
		id: Uuid,
		input: RevisionInput,
	) -> Result<Draft> {
		let f = self.runtime.clone();
		let mut tx = PgTransactionExecutor::new(f.store.pool.begin().await?);
		let original = AgentDraft::read(&mut tx, id, true).await?;
		authorize(&mut tx, &actor, &original, "agent_draft.read", true).await?;
		if original.revision != input.expected_revision {
			return Err(Error::Conflict("draft revision changed".into()));
		}
		let id = Uuid::now_v7();
		let mut entry: Entry = serde_json::from_value(original.entry.clone())?;
		let source_id = entry.id.clone();
		let source_version = entry.version.clone();
		entry.id = id.to_string();
		entry.version = "1.0.0".into();
		new_draft_defaults(&mut entry)?;
		let prospective = Draft {
			id,
			tenant: original.tenant.clone(),
			owner: match &actor {
				Actor::Operator => original.owner.clone(),
				Actor::Subject(identity) => identity.subject.clone(),
			},
			revision: 1,
			entry: serde_json::to_value(&entry)?,
			documents: original.documents.clone(),
			release_notes: String::new(),
			source_id: Some(source_id),
			source_version: Some(source_version),
			archived: false,
			updated_at: Utc::now(),
		};
		authorize(&mut tx, &actor, &prospective, "agent_draft.create", false).await?;
		let copied = AgentDraft::insert(&mut tx, &prospective, &entry.id).await?;
		event_records::append(&mut tx, &f.config.node_id, None, "agent_draft.duplicated", json!({"draft_id":id,"source_id":prospective.source_id,"source_version":prospective.source_version,"tenant":prospective.tenant})).await?;
		Box::new(tx).commit().await?;
		Ok(copied)
	}
	pub(crate) async fn adopt(
		&self,
		actor: Actor,
		(id, version): (String, String),
		input: AdoptInput,
	) -> Result<Draft> {
		let f = self.runtime.clone();
		if !matches!(actor, Actor::Operator) {
			return Err(Error::Forbidden);
		}
		let (tenant, owner) = author_identity(&actor, Some(&input.tenant), Some(&input.owner))?;
		let mut entry = f.registry.get(&id, &version).await?;
		if entry.kind != "agent" {
			return Err(Error::Invalid(
				"only agents can be assigned to Creator".into(),
			));
		}
		let documents = crate::knowledge::load(&f.registry.db, &entry).await?;
		new_draft_defaults(&mut entry)?;
		let mut tx = PgTransactionExecutor::new(f.store.pool.begin().await?);
		target_enabled(&mut tx, &tenant, &owner).await?;
		let existing = AgentDraft::managed(&mut tx, &id).await?;
		if existing {
			return Err(Error::Conflict("agent identity is already managed".into()));
		}
		let draft_id = Uuid::now_v7();
		let prospective = Draft {
			id: draft_id,
			tenant: tenant.clone(),
			owner: owner.clone(),
			revision: 1,
			entry: serde_json::to_value(&entry)?,
			documents,
			release_notes: String::new(),
			source_id: Some(id.clone()),
			source_version: Some(version.clone()),
			archived: false,
			updated_at: Utc::now(),
		};
		let draft = AgentDraft::insert(&mut tx, &prospective, &id).await?;
		event_records::append(
			&mut tx,
			&f.config.node_id,
			None,
			"agent_draft.adopted",
			json!({"draft_id":draft_id,"agent_id":id,"version":version,"tenant":tenant,"owner":owner}),
		)
		.await?;
		Box::new(tx).commit().await?;
		Ok(draft)
	}
	pub(crate) async fn share(&self, actor: Actor, id: Uuid, input: ShareInput) -> Result<Draft> {
		let f = self.runtime.clone();
		let mut tx = PgTransactionExecutor::new(f.store.pool.begin().await?);
		let draft = AgentDraft::read(&mut tx, id, true).await?;
		owner_only(&actor, &draft)?;
		authorize(&mut tx, &actor, &draft, "agent_draft.share", false).await?;
		if draft.owner == input.subject {
			return Err(Error::Invalid("owner does not need a share".into()));
		}
		if input.enabled && draft.documents != json!([]) && !input.include_documents {
			return Err(Error::Invalid(
			"sharing this draft also shares its private documents; acknowledge include_documents"
				.into(),
		));
		}
		if input.enabled {
			target_enabled(&mut tx, &draft.tenant, &input.subject).await?;
		}
		if input.enabled {
			AgentDraftShare::save(
				&mut tx,
				id,
				&input.subject,
				input.can_edit,
				&digest(&draft.documents),
			)
			.await?;
		} else {
			AgentDraftShare::remove(&mut tx, id, &input.subject).await?;
		}
		event_records::append(&mut tx, &f.config.node_id, None, "agent_draft.share_changed", json!({"draft_id":id,"tenant":draft.tenant,"subject":input.subject,"enabled":input.enabled,"can_edit":input.can_edit})).await?;
		Box::new(tx).commit().await?;
		Ok(draft)
	}
	pub(crate) async fn shares(&self, actor: Actor, id: Uuid) -> Result<Vec<DraftShare>> {
		let f = self.runtime.clone();
		let mut tx = PgTransactionExecutor::new(f.store.pool.begin().await?);
		let draft = AgentDraft::read(&mut tx, id, false).await?;
		owner_only(&actor, &draft)?;
		authorize(&mut tx, &actor, &draft, "agent_draft.share", false).await?;
		let rows = AgentDraftShare::page(&mut tx, id).await?;
		Box::new(tx).commit().await?;
		let current_digest = digest(&draft.documents);
		Ok(rows
			.into_iter()
			.map(|row| DraftShare {
				subject: row.subject,
				can_edit: row.can_edit,
				documents_current: row.documents_digest == current_digest,
			})
			.collect())
	}
	pub(crate) async fn transfer(
		&self,
		actor: Actor,
		id: Uuid,
		input: TransferInput,
	) -> Result<Draft> {
		let f = self.runtime.clone();
		let mut tx = PgTransactionExecutor::new(f.store.pool.begin().await?);
		let draft = AgentDraft::read(&mut tx, id, true).await?;
		owner_only(&actor, &draft)?;
		authorize(&mut tx, &actor, &draft, "agent_draft.transfer", false).await?;
		if draft.revision != input.expected_revision {
			return Err(Error::Conflict("draft revision changed".into()));
		}
		target_enabled(&mut tx, &draft.tenant, &input.new_owner).await?;
		// Ownership supersedes a share. Retaining it would restore the former
		// owner's access after a later transfer.
		AgentDraft::transfer(&mut tx, id, &input.new_owner).await?;
		event_records::append(
			&mut tx,
			&f.config.node_id,
			None,
			"agent_draft.owner_changed",
			json!({"draft_id":id,"tenant":draft.tenant,"from":draft.owner,"to":input.new_owner}),
		)
		.await?;
		let result = AgentDraft::read(&mut tx, id, false).await?;
		Box::new(tx).commit().await?;
		Ok(result)
	}
	pub(crate) async fn archive(
		&self,
		actor: Actor,
		id: Uuid,
		input: ArchiveInput,
	) -> Result<Draft> {
		let f = self.runtime.clone();
		let mut tx = PgTransactionExecutor::new(f.store.pool.begin().await?);
		let draft = AgentDraft::read(&mut tx, id, true).await?;
		owner_only(&actor, &draft)?;
		authorize(&mut tx, &actor, &draft, "agent_draft.archive", false).await?;
		if draft.revision != input.expected_revision {
			return Err(Error::Conflict("draft revision changed".into()));
		}
		AgentDraft::archive(&mut tx, id, input.archived).await?;
		event_records::append(
			&mut tx,
			&f.config.node_id,
			None,
			"agent_draft.archived_changed",
			json!({"draft_id":id,"tenant":draft.tenant,"archived":input.archived}),
		)
		.await?;
		let result = AgentDraft::read(&mut tx, id, false).await?;
		Box::new(tx).commit().await?;
		Ok(result)
	}
	pub(crate) async fn validate(
		&self,
		actor: Actor,
		id: Uuid,
		input: RevisionInput,
	) -> Result<Validation> {
		let f = self.runtime.clone();
		let mut tx = PgTransactionExecutor::new(f.store.pool.begin().await?);
		let draft = AgentDraft::read(&mut tx, id, false).await?;
		authorize(&mut tx, &actor, &draft, "agent_draft.write", true).await?;
		if input.expected_revision != draft.revision {
			return Err(Error::Conflict("draft revision changed".into()));
		}
		// Validation is advisory; Register repeats all checks on the locked revision.
		let result = validate_content(&f, &draft, &actor, &mut tx).await;
		Box::new(tx).commit().await?;
		Ok(Validation {
			draft_id: id,
			revision: draft.revision,
			valid: result.is_ok(),
			message: result
				.err()
				.map_or_else(|| "Technical validation passed".into(), |e| e.to_string()),
		})
	}
	pub(crate) async fn versions(&self, actor: Actor, id: Uuid) -> Result<Vec<RegisteredVersion>> {
		let f = self.runtime.clone();
		let mut tx = PgTransactionExecutor::new(f.store.pool.begin().await?);
		let draft = AgentDraft::read(&mut tx, id, false).await?;
		authorize(&mut tx, &actor, &draft, "agent_draft.read", true).await?;
		let managed_id = draft.entry["id"]
			.as_str()
			.ok_or_else(|| Error::Invalid("draft has no managed identity".into()))?;
		let mut native = tx;
		let rows = AgentDraftRegistration::page(&mut native, id, managed_id).await?;
		let draft_knowledge_digest = draft
			.documents
			.as_array()
			.filter(|docs| !docs.is_empty())
			.map(|_| digest(&draft.documents));
		let mut versions = Vec::new();
		for row in rows {
			versions.push(RegisteredVersion {
				draft_knowledge_digest: draft_knowledge_digest.clone(),
				entry: admission::effective(&mut native, managed_id, &row.version).await?,
				draft_revision: Some(row.revision),
				registered_by: Some(row.actor),
				registered_at: Some(row.registered_at),
				release_notes: row.release_notes,
				source_id: row.source_id,
				source_version: row.source_version,
				behavioral_tested: Some(row.behavioral_tested),
			});
		}
		if draft.source_id.as_deref() == Some(managed_id)
			&& let Some(source_version) = &draft.source_version
			&& !versions
				.iter()
				.any(|item| &item.entry.version == source_version)
		{
			versions.push(RegisteredVersion {
				draft_knowledge_digest: draft_knowledge_digest.clone(),
				entry: admission::effective(&mut native, managed_id, source_version).await?,
				draft_revision: None,
				registered_by: None,
				registered_at: None,
				release_notes: String::new(),
				source_id: None,
				source_version: None,
				behavioral_tested: None,
			});
		}
		Box::new(native).commit().await?;
		Ok(versions)
	}
	pub(crate) async fn register(
		&self,
		actor: Actor,
		id: Uuid,
		input: RevisionInput,
	) -> Result<Registration> {
		let f = self.runtime.clone();
		let mut tx = PgTransactionExecutor::new(f.store.pool.begin().await?);
		let draft = AgentDraft::read(&mut tx, id, true).await?;
		authorize(&mut tx, &actor, &draft, "agent_draft.register", true).await?;
		if draft.archived || input.expected_revision != draft.revision {
			return Err(Error::Conflict(
				"draft revision changed or is archived".into(),
			));
		}
		let entry = validate_content(&f, &draft, &actor, &mut tx).await?;
		let mut native = tx;
		let behavioral_tested =
			AgentDraftRegistration::behavioral_evidence(&mut native, &draft, &entry).await?;
		let inserted = admission::register(&mut native, &entry, &f.config.node_id).await?;
		let documents: Vec<ReferenceDocument> = serde_json::from_value(draft.documents.clone())?;
		if !documents.is_empty() {
			transaction_records::insert_documents(&mut native, &entry, draft.documents.clone())
				.await?;
		}
		let registered_by = match &actor {
			Actor::Operator => "operator",
			Actor::Subject(identity) => &identity.subject,
		};
		AgentDraftRegistration::record(
			&mut native,
			&draft,
			&entry,
			registered_by,
			behavioral_tested,
		)
		.await?;
		if inserted {
			event_records::append(&mut native, &f.config.node_id, None, "registry.registered", json!({"id":entry.id,"version":entry.version,"kind":"agent","draft_id":id,"draft_revision":draft.revision})).await?;
		}
		Box::new(native).commit().await?;
		Ok(Registration {
			draft_id: id,
			revision: draft.revision,
			entry,
			behavioral_tested,
		})
	}
}
