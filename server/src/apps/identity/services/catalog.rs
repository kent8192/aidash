use super::{Authorization, access::Access, identity::SubjectIdentity};
use crate::{
	Result,
	registry::{EntityRef, Entry, Search},
	store::Store,
};

impl Authorization {
	/// Operator maintenance capability; the actor string labels history and grants no authority.
	pub async fn set_catalog(
		&self,
		tenant: &str,
		entry: &EntityRef,
		expected_revision: i64,
		enabled: bool,
		actor: &str,
	) -> Result<Binding> {
		aidash_domain::identity::catalog::validate_revision(expected_revision)?;
		let mut tx = self.pool.begin().await?;
		let binding = aidash_application::authorization::catalog::set_catalog(
			&mut crate::bootstrap::catalog_administrator_scope(
				&mut tx,
				aidash_domain::identity::Principal::Operator,
			),
			tenant,
			entry,
			expected_revision,
			enabled,
			actor,
		)
		.await?;
		tx.commit().await?;
		Ok(binding)
	}

	/// Operator maintenance capability, shared with the protected management routes.
	pub async fn catalog(&self, tenant: &str) -> Result<Vec<Binding>> {
		aidash_application::authorization::catalog::administration(
			&mut crate::bootstrap::catalog_administration_read(
				&self.pool,
				aidash_domain::identity::Principal::Operator,
			),
			tenant,
		)
		.await
		.map_err(Into::into)
	}
}

pub(crate) async fn entry(
	access: &mut Access,
	reference: &EntityRef,
	action: &str,
) -> Result<Entry> {
	aidash_application::authorization::catalog::entry(
		&mut crate::bootstrap::catalog_scope(access),
		reference,
		action,
	)
	.await
	.map_err(Into::into)
}

pub(crate) fn resource(access: &Access, entry: &Entry) -> super::policy::Resource {
	access.resource(
		&entry.kind,
		&entry.id,
		aidash_application::authorization::catalog::attributes(entry),
	)
}

pub(crate) async fn list_in(access: &mut Access, search: &Search) -> Result<Vec<Entry>> {
	aidash_application::authorization::catalog::list(
		&mut crate::bootstrap::catalog_scope(access),
		search,
	)
	.await
	.map_err(Into::into)
}

pub async fn list(
	store: &Store,
	identity: &SubjectIdentity,
	search: &Search,
) -> Result<Vec<Entry>> {
	let mut access = Access::begin(store, identity).await?;
	let result = list_in(&mut access, search).await;
	access.finish(result).await
}

pub async fn get(
	store: &Store,
	identity: &SubjectIdentity,
	reference: &EntityRef,
) -> Result<Entry> {
	let mut access = Access::begin(store, identity).await?;
	let result = aidash_application::authorization::catalog::get(
		&mut crate::bootstrap::catalog_scope(&mut access),
		reference,
	)
	.await
	.map_err(Into::into);
	access.finish(result).await
}

pub use crate::apps::identity::serializers::catalog::Binding;

// The caller retains the tenant, compatibility and resource locks until commit.
// Marketplace callers retain their writer and resource locks through commit.
pub(crate) async fn set_in(
	tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
	tenant: &str,
	entry: &EntityRef,
	expected_revision: i64,
	enabled: bool,
	actor: &str,
) -> Result<Binding> {
	aidash_application::authorization::catalog::set_in(
		&mut crate::bootstrap::catalog_mutation_scope(tx),
		tenant,
		entry,
		expected_revision,
		enabled,
		actor,
	)
	.await
	.map_err(Into::into)
}
