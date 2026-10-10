//! DI use cases and bounded disclosure under authorization leases.
use super::super::serializers::management::*;
use super::{distribution, installations, storage::*, *};
use crate::{
	Error, Result,
	authorization::{access::Access, identity::Actor},
	dashboard_auth::BrowserOrigin,
	federation::Federation,
};
use bytes::Bytes;
use http::header;
use reinhardt::{Response, injectable};
use serde::Serialize;

#[derive(Clone)]
pub struct MarketplaceManagement {
	runtime: Federation,
}
#[injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> MarketplaceManagement {
	tracing::trace!("resolving Marketplace management");
	MarketplaceManagement { runtime }
}

fn subject(actor: &Actor) -> Result<&crate::authorization::identity::SubjectIdentity> {
	match actor {
		Actor::Subject(identity) => Ok(identity),
		Actor::Operator => Err(Error::Forbidden),
	}
}

fn principal(actor: &Actor) -> aidash_domain::identity::Principal {
	match actor {
		Actor::Operator => aidash_domain::identity::Principal::Operator,
		Actor::Subject(identity) => aidash_domain::identity::Principal::Subject {
			tenant: identity.tenant.clone(),
			subject: identity.subject.clone(),
		},
	}
}
fn operator(actor: &Actor) -> Result<()> {
	aidash_application::marketplace::operations::require_operator(&principal(actor))
		.map_err(Into::into)
}

/// Serialize and enqueue at most two MiB while every disclosure lease remains
/// held. Queue insertion is the bounded handoff point, ordered before revokers.
/// The response consumer cannot see that queue unless the transaction commits;
/// no database lease is tied to the client's unbounded network drain.
pub(crate) async fn handoff<T: Serialize>(
	store: &crate::store::Store,
	mut access: Access,
	result: Result<T>,
) -> Result<Response> {
	let result = async {
		let value = result?;
		let response = bounded_json(&value)?;
		if let Some(mut audit) = access.marketplace_audit.clone() {
			audit["outcome"] = serde_json::json!("allowed");
			store
				.event(&mut access.tx, None, "marketplace.audit", audit)
				.await?;
		}
		super::storage::credential_current(&mut access).await?;
		Ok(response)
	}
	.await;
	let denial = result.as_ref().err().and_then(|error| {
		access.marketplace_audit.clone().map(|mut audit| {
			audit["outcome"] = serde_json::json!(match error {
				Error::Forbidden => "denied",
				Error::Unauthorized => "unauthorized",
				Error::Conflict(_) => "conflict",
				_ => "failed",
			});
			audit
		})
	});
	let result = access.finish(result).await;
	if let Some(denial) = denial {
		let mut tx = crate::database::native::begin(&store.pool).await?;
		store
			.event(&mut tx, None, "marketplace.audit", denial)
			.await?;
		tx.commit().await?;
	}
	result
}

fn bounded_json<T: Serialize>(value: &T) -> Result<Response> {
	let bytes = serde_json::to_vec(value)?;
	if bytes.len() > 2_097_152 {
		return Err(Error::Invalid(
			"response exceeds two MiB; narrow the query".into(),
		));
	}
	let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
	sender
		.try_send(Ok::<Bytes, Box<dyn std::error::Error + Send + Sync>>(
			Bytes::from(bytes),
		))
		.map_err(|_| Error::Forbidden)?;
	drop(sender);
	let mut response =
		Response::ok().with_stream(Box::pin(futures_util::stream::once(async move {
			receiver.recv().await.expect("one bounded response")
		})));
	response.headers.insert(
		header::CONTENT_TYPE,
		header::HeaderValue::from_static("application/json"),
	);
	response.headers.insert(
		header::CACHE_CONTROL,
		header::HeaderValue::from_static("no-store"),
	);
	Ok(response)
}

fn next_page(response: &mut Response, offset: Option<usize>) {
	if let Some(offset) = offset {
		response
			.headers
			.insert("x-aidash-next-offset", offset.into());
	}
}

impl MarketplaceManagement {
	pub(crate) async fn browse(&self, actor: Actor, input: Browse) -> Result<Response> {
		let f = self.runtime.clone();
		let mut access = begin(&f.store, subject(&actor)?, false).await?;
		let result = aidash_application::marketplace::management::browse(
			&mut crate::bootstrap::marketplace_distribution_scope(&mut access),
			&aidash_application::marketplace::management::BrowseQuery {
				q: input.q,
				offset: input.offset,
				limit: input.limit,
			},
			&f.store.node_id,
		)
		.await
		.map_err(Into::into);
		handoff(&f.store, access, result).await
	}

	pub(crate) async fn detail(&self, actor: Actor, key: String) -> Result<Response> {
		let f = self.runtime.clone();
		let mut access = begin(&f.store, subject(&actor)?, false).await?;
		let result = distribution::detail(&mut access, &key, &f.store.node_id).await;
		handoff(&f.store, access, result).await
	}

	pub(crate) async fn publish(&self, actor: Actor, input: Publish) -> Result<Response> {
		let f = self.runtime.clone();
		let mut access = begin(&f.store, subject(&actor)?, true).await?;
		let result = distribution::publish(&f.store, &mut access, &input).await;
		handoff(&f.store, access, result).await
	}

	pub(crate) async fn sources(&self, actor: Actor, input: SourceQuery) -> Result<Response> {
		let f = self.runtime.clone();
		let mut access = begin(&f.store, subject(&actor)?, false).await?;
		let result = aidash_application::marketplace::management::sources(
			&mut crate::bootstrap::marketplace_distribution_scope(&mut access),
			&aidash_application::marketplace::management::SourceQuery {
				offset: input.offset,
				limit: input.limit,
			},
			&f.store.node_id,
		)
		.await;
		let (result, next) = match result {
			Ok(page) => (Ok(page.entries), page.next_offset),
			Err(error) => (Err(error.into()), None),
		};
		let mut response = handoff(&f.store, access, result).await?;
		next_page(&mut response, next);
		Ok(response)
	}

	pub(crate) async fn install(
		&self,
		actor: Actor,
		key: String,
		input: Install,
	) -> Result<Response> {
		let f = self.runtime.clone();
		let mut access = begin(&f.store, subject(&actor)?, true).await?;
		let result = installations::install(&f.store, &mut access, &key, &input).await;
		handoff(&f.store, access, result).await
	}

	pub(crate) async fn share(
		&self,
		actor: Actor,
		key: String,
		input: AudienceInput,
	) -> Result<Response> {
		let f = self.runtime.clone();
		let mut access = begin(&f.store, subject(&actor)?, true).await?;
		let result = distribution::share(&f.store, &mut access, &key, None, input).await;
		handoff(&f.store, access, result).await
	}

	pub(crate) async fn consent(
		&self,
		actor: Actor,
		(key, tenant): (String, String),
		input: AudienceInput,
	) -> Result<Response> {
		let f = self.runtime.clone();
		let mut access = begin(&f.store, subject(&actor)?, true).await?;
		let result = distribution::share(&f.store, &mut access, &key, Some(&tenant), input).await;
		handoff(&f.store, access, result).await
	}

	pub(crate) async fn read_consent(
		&self,
		actor: Actor,
		(key, tenant): (String, String),
	) -> Result<Response> {
		let f = self.runtime.clone();
		crate::authorization::policy::identifier(&tenant)?;
		let mut access = begin(&f.store, subject(&actor)?, false).await?;
		let result = aidash_application::marketplace::management::read_consent(
			&mut crate::bootstrap::marketplace_distribution_scope(&mut access),
			&key,
			&tenant,
		)
		.await
		.map_err(Into::into);
		handoff(&f.store, access, result).await
	}

	pub(crate) async fn list_installations(&self, actor: Actor) -> Result<Response> {
		let f = self.runtime.clone();
		let mut access = begin(&f.store, subject(&actor)?, false).await?;
		let result = aidash_application::marketplace::management::list_installations(
			&mut crate::bootstrap::marketplace_distribution_scope(&mut access),
			&f.store.node_id,
		)
		.await
		.map_err(Into::into);
		handoff(&f.store, access, result).await
	}

	pub(crate) async fn installation(
		&self,
		actor: Actor,
		id: String,
		query: RevisionQuery,
	) -> Result<Response> {
		let f = self.runtime.clone();
		let mut access = begin(&f.store, subject(&actor)?, false).await?;
		let result = installations::view(&mut access, &id, query.revision, &f.store.node_id).await;
		handoff(&f.store, access, result).await
	}

	pub(crate) async fn configure(
		&self,
		actor: Actor,
		id: String,
		input: Configure,
	) -> Result<Response> {
		let f = self.runtime.clone();
		let mut access = begin(&f.store, subject(&actor)?, true).await?;
		let result = installations::configure(&f.store, &mut access, &id, &input).await;
		handoff(&f.store, access, result).await
	}

	pub(crate) async fn compatibility(&self, actor: Actor) -> Result<Compatibility> {
		let f = self.runtime.clone();
		operator(&actor)?;
		let mut tx = crate::database::native::begin(&f.store.pool).await?;
		aidash_application::marketplace::operations::compatibility(
			&mut crate::bootstrap::marketplace_operator_scope(&f.store, &mut tx, principal(&actor)),
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn set_compatibility(
		&self,
		actor: Actor,
		browser: Option<BrowserOrigin>,
		input: CompatibilityInput,
	) -> Result<Compatibility> {
		let f = self.runtime.clone();
		operator(&actor)?;
		let input = aidash_application::marketplace::operations::CompatibilityChange {
			enabled: input.enabled,
			expected_revision: input.expected_revision,
			compatible_instances_confirmed: input.compatible_instances_confirmed,
		};
		aidash_application::marketplace::operations::validate_compatibility(&input)?;
		let origin = browser.as_ref();
		let mut tx = operator_begin(&f.store, origin).await?;
		let result = aidash_application::marketplace::operations::set_compatibility(
			&mut crate::bootstrap::marketplace_operator_scope(&f.store, &mut tx, principal(&actor)),
			&input,
		)
		.await?;
		operator_commit(tx, origin).await?;
		Ok(result)
	}

	pub(crate) async fn activate(
		&self,
		actor: Actor,
		browser: Option<BrowserOrigin>,
		id: String,
		input: Activate,
	) -> Result<Installation> {
		let f = self.runtime.clone();
		operator(&actor)?;
		let origin = browser.as_ref();
		let mut tx = operator_begin(&f.store, origin).await?;
		let result = aidash_application::marketplace::operations::activate(
			&mut crate::bootstrap::marketplace_operator_scope(&f.store, &mut tx, principal(&actor)),
			&id,
			&aidash_application::marketplace::operations::ActivationCommand {
				tenant: input.tenant,
				revision: input.revision,
				expected_activation_revision: input.expected_activation_revision,
				expected_catalog_revision: input.expected_catalog_revision,
				enabled: input.enabled,
			},
		)
		.await?;
		operator_commit(tx, origin).await?;
		Ok(result)
	}

	pub(crate) async fn adopt(
		&self,
		actor: Actor,
		browser: Option<BrowserOrigin>,
		input: Adopt,
	) -> Result<Installation> {
		let f = self.runtime.clone();
		operator(&actor)?;
		crate::authorization::policy::identifier(&input.tenant)?;
		let origin = browser.as_ref();
		let mut tx = operator_begin(&f.store, origin).await?;
		let result = aidash_application::marketplace::operations::adopt(
			&mut crate::bootstrap::marketplace_operator_scope(&f.store, &mut tx, principal(&actor)),
			&crate::bootstrap::registry_validation_for(&self.runtime.store),
			&adoption_command(&input),
			&f.store.node_id,
		)
		.await?;
		operator_commit(tx, origin).await?;
		Ok(result)
	}

	pub(crate) async fn approve_set(
		&self,
		actor: Actor,
		browser: Option<BrowserOrigin>,
		input: aidash_application::marketplace::operations::approval_set::ApprovalSet,
	) -> Result<Vec<Installation>> {
		operator(&actor)?;
		let origin = browser.as_ref();
		let mut tx = operator_begin(&self.runtime.store, origin).await?;
		let result =
			aidash_application::marketplace::operations::approval_set::approve_and_activate(
				&mut crate::bootstrap::marketplace_operator_scope(
					&self.runtime.store,
					&mut tx,
					principal(&actor),
				),
				&crate::bootstrap::registry_validation_for(&self.runtime.store),
				&input,
				&self.runtime.store.node_id,
			)
			.await?;
		operator_commit(tx, origin).await?;
		Ok(result)
	}
	pub(crate) async fn host_packages(
		&self,
		actor: Actor,
		browser: Option<BrowserOrigin>,
		input: aidash_application::marketplace::operations::host_packages::HostPackages,
	) -> Result<aidash_application::marketplace::operations::host_packages::PendingHostPackages> {
		operator(&actor)?;
		let origin = browser.as_ref();
		let mut tx = operator_begin(&self.runtime.store, origin).await?;
		let validation = crate::bootstrap::registry_validation_for(&self.runtime.store);
		let result = aidash_application::marketplace::operations::host_packages::provision(
			&mut crate::bootstrap::marketplace_operator_scope(
				&self.runtime.store,
				&mut tx,
				principal(&actor),
			),
			&validation,
			&validation,
			&input,
			&self.runtime.store.node_id,
		)
		.await?;
		operator_commit(tx, origin).await?;
		Ok(result)
	}

	pub(crate) async fn administration(
		&self,
		actor: Actor,
		browser: Option<BrowserOrigin>,
		query: AdministrationQuery,
	) -> Result<Response> {
		let f = self.runtime.clone();
		operator(&actor)?;
		crate::authorization::policy::identifier(&query.tenant)?;
		let origin = browser.as_ref();
		let mut tx = operator_begin(&f.store, origin).await?;
		let page = aidash_application::marketplace::operations::administration(
			&mut crate::bootstrap::marketplace_operator_scope(&f.store, &mut tx, principal(&actor)),
			&aidash_application::marketplace::operations::AdministrationQuery {
				tenant: query.tenant,
				offset: query.offset,
				limit: query.limit,
			},
		)
		.await?;
		// Serialize into the bounded queue before releasing browser and operator leases.
		let mut response = bounded_json(&page.entries)?;
		operator_commit(tx, origin).await?;
		next_page(&mut response, page.next_offset);
		Ok(response)
	}

	pub(crate) async fn publication_access(
		&self,
		actor: Actor,
		input: Publish,
	) -> Result<Response> {
		let f = self.runtime.clone();
		let mut access = begin(&f.store, subject(&actor)?, false).await?;
		let result = aidash_application::marketplace::publication::preview(
			&mut crate::bootstrap::marketplace_publication_scope(&f.store, &mut access),
			&crate::bootstrap::registry_validation_for(&self.runtime.store),
			&distribution::command(&input),
			&f.store.node_id,
		)
		.await
		.map_err(Into::into);
		handoff(&f.store, access, result).await
	}
}

fn adoption_command(input: &Adopt) -> aidash_application::marketplace::operations::AdoptionCommand {
	aidash_application::marketplace::operations::AdoptionCommand {
		tenant: input.tenant.clone(),
		source: input.source.clone(),
		idempotency_key: input.idempotency_key,
	}
}

#[cfg(test)]
mod conversion_tests {
	use super::*;
	use reinhardt::core::validators::Validate as _;
	use rstest::rstest;
	#[rstest]
	fn adoption_conversion_preserves_operator_replay_bytes() {
		let input = Adopt {
			tenant: "tenant".into(),
			source: EntityRef {
				id: "source".into(),
				version: "1.0.0".into(),
			},
			idempotency_key: Uuid::nil(),
		};
		input.validate().unwrap();
		let converted = adoption_command(&input);
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
