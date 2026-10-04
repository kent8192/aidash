//! Tenant-scoped Creator drafts and immutable Registry admission. Workbench
//! permissions are separate from installation-wide Registry administration.
use crate::apps::execution::models::event_records;
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
		Ok(aidash_application::registry::workbench::drafts::archive(
			&crate::bootstrap::draft_repository(&self.runtime, actor),
			id,
			input,
		)
		.await?
		.into())
	}
	pub(crate) async fn validate(
		&self,
		actor: Actor,
		id: Uuid,
		input: RevisionInput,
	) -> Result<Validation> {
		Ok(
			aidash_application::registry::workbench::publication::validate(
				&crate::bootstrap::draft_repository(&self.runtime, actor),
				&crate::bootstrap::registry_validation(),
				id,
				input,
			)
			.await?,
		)
	}
	pub(crate) async fn versions(&self, actor: Actor, id: Uuid) -> Result<Vec<RegisteredVersion>> {
		Ok(
			aidash_application::registry::workbench::publication::versions(
				&crate::bootstrap::draft_repository(&self.runtime, actor),
				id,
			)
			.await?,
		)
	}
	pub(crate) async fn register(
		&self,
		actor: Actor,
		id: Uuid,
		input: RevisionInput,
	) -> Result<Registration> {
		Ok(
			aidash_application::registry::workbench::publication::register(
				&crate::bootstrap::draft_repository(&self.runtime, actor),
				&crate::bootstrap::registry_validation(),
				id,
				input,
			)
			.await?,
		)
	}
}
