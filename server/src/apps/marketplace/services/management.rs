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
		Actor::DeploymentAdministrator => Err(Error::Forbidden),
	}
}

/// A Tenant route acts only for a subject of the Tenant it names.
fn approving_subject(
	actor: &Actor,
	tenant: &str,
) -> Result<crate::authorization::identity::SubjectIdentity> {
	match actor {
		Actor::Subject(identity) if identity.tenant == tenant => Ok(identity.clone()),
		_ => Err(Error::Forbidden),
	}
}

/// `catalog.approve` on the exact version an activation approves, decided only
/// for a revision installed in the subject's own Tenant.
async fn require_approval(
	access: &mut Access,
	revision: &aidash_domain::marketplace::Revision,
) -> Result<()> {
	if revision.tenant != access.identity.tenant {
		return Err(Error::Forbidden);
	}
	let entry = &revision.entry;
	let resource = access.resource(
		"registry",
		format!("{}/{}@{}", entry.kind, entry.id, entry.version),
		aidash_application::authorization::catalog::attributes(entry),
	);
	access.require(&resource, "catalog.approve").await
}

fn principal(actor: &Actor) -> aidash_domain::identity::Principal {
	match actor {
		Actor::DeploymentAdministrator => {
			aidash_domain::identity::Principal::DeploymentAdministrator
		}
		Actor::Subject(identity) => aidash_domain::identity::Principal::Subject {
			tenant: identity.tenant.clone(),
			subject: identity.subject.clone(),
		},
	}
}
fn deployment_administrator(actor: &Actor) -> Result<()> {
	aidash_application::marketplace::operations::require_deployment_administrator(&principal(actor))
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
	let tenancy = access.tenancy()?;
	let result = access.finish(result).await;
	if let Some(denial) = denial {
		let mut tx = store.pool.begin(&tenancy).await?;
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
		self.change_audience(actor, &key, None, input).await
	}

	pub(crate) async fn consent(
		&self,
		actor: Actor,
		(key, tenant): (String, String),
		input: AudienceInput,
	) -> Result<Response> {
		self.change_audience(actor, &key, Some(&tenant), input)
			.await
	}

	/// Save an audience or a consent. The events an audience change owes its
	/// recipients are written after it commits: their Tenants are not the
	/// owner's, so the owner's transaction cannot write them.
	async fn change_audience(
		&self,
		actor: Actor,
		key: &str,
		redistributor: Option<&str>,
		input: AudienceInput,
	) -> Result<Response> {
		let f = self.runtime.clone();
		let mut access = begin(&f.store, subject(&actor)?, true).await?;
		let (result, announcements) =
			match distribution::share(&f.store, &mut access, key, redistributor, input).await {
				Ok(shared) => (Ok(shared.audience), shared.announcements),
				Err(error) => (Err(error), vec![]),
			};
		let response = handoff(&f.store, access, result).await?;
		// The change is committed and each recipient still reads the package
		// through its audience, so a failed delivery loses a notification only.
		if let Err(error) = distribution::deliver(&f.store, announcements).await {
			tracing::error!(%error, "audience events were not delivered");
		}
		Ok(response)
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

	/// Activating an installed revision approves it in the subject's Tenant,
	/// under `catalog.approve` (ADR 0029).
	pub(crate) async fn activate(
		&self,
		actor: Actor,
		id: String,
		input: Activate,
	) -> Result<Installation> {
		let f = self.runtime.clone();
		let identity = approving_subject(&actor, &input.tenant)?;
		let mut access = Access::begin_exclusive(&f.store, &identity).await?;
		let result = async {
			let revision = crate::apps::marketplace::repositories::installations::revision(
				&mut access.tx,
				&id,
				input.revision,
			)
			.await?;
			require_approval(&mut access, &revision).await?;
			let enabled = input.enabled;
			let installation = aidash_application::marketplace::operations::activate(
				&mut crate::bootstrap::marketplace_tenant_activation_scope(
					&f.store,
					&mut access.tx,
					principal(&actor),
					identity.tenant.clone(),
				),
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
			if enabled {
				crate::apps::identity::repositories::catalog_approvals::admit_agents(
					&mut access,
					&f.config.node_id,
					&[revision.entry],
				)
				.await?;
			}
			Ok(installation)
		}
		.await;
		access.finish(result).await
	}

	pub(crate) async fn approve_set(
		&self,
		actor: Actor,
		input: aidash_application::marketplace::operations::approval_set::ApprovalSet,
	) -> Result<Vec<Installation>> {
		let f = self.runtime.clone();
		let identity = approving_subject(&actor, &input.tenant)?;
		let mut access = Access::begin_exclusive(&f.store, &identity).await?;
		let result = async {
			let mut entries = Vec::with_capacity(input.installations.len());
			for selection in &input.installations {
				let revision = crate::apps::marketplace::repositories::installations::revision(
					&mut access.tx,
					&selection.installation,
					selection.revision,
				)
				.await?;
				require_approval(&mut access, &revision).await?;
				entries.push(revision.entry);
			}
			let installations =
				aidash_application::marketplace::operations::approval_set::approve_and_activate(
					&mut crate::bootstrap::marketplace_tenant_activation_scope(
						&f.store,
						&mut access.tx,
						principal(&actor),
						identity.tenant.clone(),
					),
					&crate::bootstrap::registry_validation_for(&f.store),
					&input,
					&f.store.node_id,
				)
				.await?;
			crate::apps::identity::repositories::catalog_approvals::admit_agents(
				&mut access,
				&f.config.node_id,
				&entries,
			)
			.await?;
			Ok(installations)
		}
		.await;
		access.finish(result).await
	}

	pub(crate) async fn administration(
		&self,
		actor: Actor,
		browser: Option<BrowserOrigin>,
		query: AdministrationQuery,
	) -> Result<Response> {
		let f = self.runtime.clone();
		deployment_administrator(&actor)?;
		crate::authorization::policy::identifier(&query.tenant)?;
		let origin = browser.as_ref();
		let mut tx = deployment_administrator_begin(&f.store, origin).await?;
		let page = aidash_application::marketplace::operations::administration(
			&mut crate::bootstrap::marketplace_deployment_administrator_scope(
				&f.store,
				&mut tx,
				principal(&actor),
			),
			&aidash_application::marketplace::operations::AdministrationQuery {
				tenant: query.tenant,
				offset: query.offset,
				limit: query.limit,
			},
		)
		.await?;
		// Serialize into the bounded queue before releasing browser and Deployment Administrator leases.
		let mut response = bounded_json(&page.entries)?;
		deployment_administrator_commit(tx, origin).await?;
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
