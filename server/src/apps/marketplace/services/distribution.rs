use super::*;
use crate::{Result, authorization::access::Access, store::Store};

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
		&crate::bootstrap::registry_validation(),
		&command(input),
		&store.node_id,
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
) -> Result<Audience> {
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
