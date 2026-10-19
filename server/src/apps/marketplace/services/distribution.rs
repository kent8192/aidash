use super::*;
use crate::{Result, authorization::access::Access, store::Store};
use aidash_application::{
	marketplace::publication::Shared,
	tenancy::{DeploymentScopeReason, TenancyScope},
};
use aidash_domain::marketplace::events::Announcement;
use std::collections::BTreeSet;

pub(super) async fn detail(access: &mut Access, key: &str, node: &str) -> Result<Detail> {
	aidash_application::marketplace::distribution::detail(
		&mut crate::bootstrap::marketplace_distribution_scope(access),
		key,
		node,
	)
	.await
	.map_err(Into::into)
}

pub(super) fn command(
	input: &Publish,
) -> aidash_application::marketplace::publication::PublishCommand {
	aidash_application::marketplace::publication::PublishCommand {
		source: input.source.clone(),
		package_id: input.package_id.clone(),
		author: input.author.clone(),
		permissions: input.permissions.clone(),
		dependencies: input.dependencies.clone(),
		idempotency_key: input.idempotency_key,
	}
}

pub(super) async fn publish(store: &Store, access: &mut Access, input: &Publish) -> Result<Value> {
	aidash_application::marketplace::publication::publish(
		&mut crate::bootstrap::marketplace_publication_scope(store, access),
		&crate::bootstrap::registry_validation_for(store),
		&command(input),
		store.deployment.as_str(),
	)
	.await
	.map_err(Into::into)
}
pub(super) async fn share(
	store: &Store,
	access: &mut Access,
	key: &str,
	redistributor: Option<&str>,
	input: AudienceInput,
) -> Result<Shared> {
	aidash_application::marketplace::publication::share(
		&mut crate::bootstrap::marketplace_publication_scope(store, access),
		key,
		redistributor,
		aidash_application::marketplace::publication::AudienceChange {
			expected_revision: input.expected_revision,
			tenants: input.tenants,
		},
	)
	.await
	.map_err(Into::into)
}

/// Write each announcement's copies once the change that owes them has
/// committed. An event belongs to one Tenant, so each recipient gets its own,
/// all in one transaction. A recipient with no Tenant root or under a Tenant
/// Deletion gets none: the name came from the owner's input, and a purge must
/// not leave rows behind.
pub(super) async fn deliver(store: &Store, announcements: Vec<Announcement>) -> Result<()> {
	let named = announcements
		.iter()
		.flat_map(|announcement| announcement.tenants.iter().cloned())
		.collect::<BTreeSet<_>>();
	if named.is_empty() {
		return Ok(());
	}
	let scope = TenancyScope::Deployment(DeploymentScopeReason::MarketplaceDistribution);
	store
		.pool
		.atomic(&scope, async |tx| {
			let recipients = crate::apps::identity::repositories::tenants::undeleted(
				tx,
				named.into_iter().collect(),
			)
			.await?;
			for announcement in &announcements {
				for tenant in announcement.tenants.intersection(&recipients) {
					crate::apps::execution::models::event_records::append_for_tenant(
						tx,
						&store.node_id,
						tenant,
						announcement.kind,
						announcement.payload.clone(),
					)
					.await?;
				}
			}
			Ok(())
		})
		.await
}

#[cfg(test)]
mod tests {
	use super::*;
	use reinhardt::core::validators::Validate as _;
	use rstest::rstest;
	use uuid::Uuid;

	#[rstest]
	fn publication_conversion_preserves_persisted_idempotency_bytes() {
		// Arrange: use nonempty optional fields so their ordering is observable.
		let input = Publish {
			source: EntityRef {
				id: "source".into(),
				version: "1.0.0".into(),
			},
			package_id: "package".into(),
			author: "Publisher".into(),
			permissions: vec!["workspace.read".into()],
			dependencies: vec![EntityRef {
				id: "dependency".into(),
				version: "2.0.0".into(),
			}],
			idempotency_key: Uuid::nil(),
		};
		input.validate().unwrap();
		// Act
		let converted = command(&input);
		// Assert: existing native request records remain replayable after extraction.
		assert_eq!(
			serde_json::to_vec(&input).unwrap(),
			serde_json::to_vec(&converted).unwrap()
		);
		assert_eq!(
			aidash_domain::marketplace::definitions::key(&input),
			aidash_domain::marketplace::definitions::key(&converted)
		);
	}
}
