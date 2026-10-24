//! Tenant-scoped Creator drafts and immutable Registry admission. Workbench
//! permissions are separate from installation-wide Registry administration.
use crate::{Result, authorization::identity::Actor, federation::Federation};
use reinhardt::injectable;
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
				&crate::bootstrap::registry_validation_for(&self.runtime.store),
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
				&crate::bootstrap::registry_validation_for(&self.runtime.store),
				id,
				input,
			)
			.await?,
		)
	}
}
