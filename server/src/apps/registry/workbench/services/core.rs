//! Tenant-scoped Creator drafts and immutable Registry admission. Workbench
//! permissions are separate from installation-wide Registry administration.
use crate::apps::execution::models::event_records;
use crate::apps::registry::models::transaction_records;
use crate::apps::registry::services::admission;
use crate::apps::registry::workbench::models::{AgentDraft, AgentDraftRegistration};
use crate::{
	Error, Result,
	authorization::{
		Authorization,
		identity::Actor,
		policy::{Evaluation, Resource},
	},
	federation::Federation,
	knowledge::{ReferenceDocument, digest},
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
	Ok(aidash_application::registry::workbench::author_identity(
		&crate::bootstrap::draft_principal(actor),
		tenant,
		owner,
	)?)
}

async fn authorize(
	tx: &mut dyn TransactionExecutor,
	actor: &Actor,
	draft: &Draft,
	action: &str,
	shares: bool,
) -> Result<()> {
	Ok(aidash_application::registry::workbench::authorize(
		&mut crate::bootstrap::draft_authority_scope(tx, actor),
		&draft.clone().into(),
		action,
		shares,
	)
	.await?)
}

async fn validate_content(
	f: &Federation,
	draft: &Draft,
	actor: &Actor,
	tx: &mut dyn TransactionExecutor,
) -> Result<Entry> {
	Ok(aidash_application::registry::workbench::validate_content(
		&mut crate::bootstrap::draft_authority_scope(tx, actor),
		&crate::bootstrap::registry_validation(),
		&draft.clone().into(),
		&f.config.node_id,
	)
	.await?)
}

fn ref_key(reference: &crate::registry::EntityRef) -> String {
	aidash_application::registry::workbench::ref_key(reference)
}

async fn target_enabled(
	tx: &mut dyn TransactionExecutor,
	tenant: &str,
	subject: &str,
) -> Result<()> {
	Ok(aidash_application::registry::workbench::target_enabled(
		&mut crate::bootstrap::draft_authority_scope(tx, &Actor::Operator),
		tenant,
		subject,
	)
	.await?)
}

fn owner_only(actor: &Actor, draft: &Draft) -> Result<()> {
	Ok(aidash_application::registry::workbench::owner_only(
		&crate::bootstrap::draft_principal(actor),
		&draft.clone().into(),
	)?)
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
	pub(crate) async fn create(&self, actor: Actor, input: CreateDraft) -> Result<Draft> {
		Ok(aidash_application::registry::workbench::drafts::create(
			&crate::bootstrap::draft_repository(&self.runtime, actor),
			input,
		)
		.await?
		.into())
	}
	pub(crate) async fn list(&self, actor: Actor, page: DraftPage) -> Result<Vec<Draft>> {
		Ok(aidash_application::registry::workbench::drafts::list(
			&crate::bootstrap::draft_repository(&self.runtime, actor),
			page,
		)
		.await?
		.into_iter()
		.map(Into::into)
		.collect())
	}
	pub(crate) async fn get(&self, actor: Actor, id: Uuid) -> Result<Draft> {
		Ok(aidash_application::registry::workbench::drafts::get(
			&crate::bootstrap::draft_repository(&self.runtime, actor),
			id,
		)
		.await?
		.into())
	}
	pub(crate) async fn save(&self, actor: Actor, id: Uuid, input: SaveDraft) -> Result<Draft> {
		Ok(aidash_application::registry::workbench::drafts::save(
			&crate::bootstrap::draft_repository(&self.runtime, actor),
			id,
			input,
		)
		.await?
		.into())
	}
	pub(crate) async fn duplicate(
		&self,
		actor: Actor,
		id: Uuid,
		input: RevisionInput,
	) -> Result<Draft> {
		Ok(aidash_application::registry::workbench::drafts::duplicate(
			&crate::bootstrap::draft_repository(&self.runtime, actor),
			id,
			input,
		)
		.await?
		.into())
	}
	pub(crate) async fn adopt(
		&self,
		actor: Actor,
		reference: (String, String),
		input: AdoptInput,
	) -> Result<Draft> {
		Ok(aidash_application::registry::workbench::drafts::adopt(
			&crate::bootstrap::draft_repository(&self.runtime, actor),
			reference,
			input,
		)
		.await?
		.into())
	}
	pub(crate) async fn share(&self, actor: Actor, id: Uuid, input: ShareInput) -> Result<Draft> {
		Ok(aidash_application::registry::workbench::drafts::share(
			&crate::bootstrap::draft_repository(&self.runtime, actor),
			id,
			input,
		)
		.await?
		.into())
	}
	pub(crate) async fn shares(&self, actor: Actor, id: Uuid) -> Result<Vec<DraftShare>> {
		Ok(aidash_application::registry::workbench::drafts::shares(
			&crate::bootstrap::draft_repository(&self.runtime, actor),
			id,
		)
		.await?)
	}
	pub(crate) async fn transfer(
		&self,
		actor: Actor,
		id: Uuid,
		input: TransferInput,
	) -> Result<Draft> {
		Ok(aidash_application::registry::workbench::drafts::transfer(
			&crate::bootstrap::draft_repository(&self.runtime, actor),
			id,
			input,
		)
		.await?
		.into())
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
