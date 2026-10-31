//! HTTP authorization and composition for registered clients and scope ceilings.
use super::super::{
	repositories::oauth_clients::NativeRepository,
	serializers::oauth_clients::{CeilingWrite, Write},
};
use crate::{
	Error, Result,
	authorization::{access::NativeAccess, identity::Actor},
	federation::Federation,
};
use aidash_application::{
	oauth_clients::{Metadata, Put, ScopeCeiling, Service},
	tenant_secrets::Ownership,
};
use aidash_domain::identity::tenant_administration::{
	COLLECTION, CONNECTION_SCOPES_UPDATE, SECRET_MANAGE, SECRET_USE,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reinhardt::injectable;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;
#[derive(Clone)]
pub struct Management {
	pub runtime: Federation,
}
#[injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> Management {
	Management { runtime }
}
/// Constructed from the existing Store configuration; no second pool or Store.
pub(crate) fn service(store: &crate::store::Store) -> Service {
	Service {
		repository: Arc::new(NativeRepository {
			pool: store.control_pool.clone(),
		}),
		store: store
			.provider_credentials
			.as_ref()
			.map(|credentials| credentials.store.clone()),
	}
}
fn subject<'a>(
	actor: &'a Actor,
	tenant: &str,
) -> Result<&'a crate::authorization::identity::SubjectIdentity> {
	aidash_domain::policy::identifier(tenant)?;
	match actor {
		Actor::Subject(identity) if identity.tenant == tenant => Ok(identity),
		_ => Err(Error::Forbidden),
	}
}
/// Path resources are canonical resource URIs encoded using unpadded base64url.
pub fn decode_resource(encoded: &str) -> Result<String> {
	let invalid = || {
		Error::Invalid("scope ceiling resource must be base64url of a canonical resource".into())
	};
	let resource = String::from_utf8(URL_SAFE_NO_PAD.decode(encoded).map_err(|_| invalid())?)
		.map_err(|_| invalid())?;
	let canonical =
		aidash_domain::oauth_connections::canonical_resource(&resource).map_err(|_| invalid())?;
	if canonical != resource || URL_SAFE_NO_PAD.encode(resource.as_bytes()) != encoded {
		return Err(invalid());
	}
	Ok(resource)
}
impl Management {
	async fn authorize(
		&self,
		actor: &Actor,
		tenant: &str,
		kind: &str,
		key: &str,
		actions: &[&str],
	) -> Result<NativeAccess> {
		let identity = subject(actor, tenant)?;
		let mut access = NativeAccess::begin(&self.runtime.store, identity).await?;
		let resource = access.resource(kind, key, json!({"name":key}));
		for action in actions {
			if access.decide(&resource, action).await? {
				return Ok(access);
			}
		}
		access.finish(Err(Error::Forbidden)).await
	}
	pub async fn list(&self, actor: Actor, tenant: String) -> Result<Vec<Metadata>> {
		let mut access = self
			.authorize(
				&actor,
				&tenant,
				"secret",
				COLLECTION,
				&[SECRET_USE, SECRET_MANAGE],
			)
			.await?;
		let result: Result<Vec<Metadata>> = async {
			let rows = service(&self.runtime.store)
				.list(&tenant, &subject(&actor, &tenant)?.subject)
				.await?;
			let mut visible = Vec::new();
			for row in rows {
				let action = if row.owner == Ownership::Me {
					SECRET_USE
				} else {
					SECRET_MANAGE
				};
				let resource = access.resource("secret", &row.issuer, json!({"name":row.issuer}));
				if access.decide(&resource, action).await? {
					visible.push(row);
				}
			}
			Ok(visible)
		}
		.await;
		access.finish(result).await
	}

	pub async fn get(&self, actor: Actor, tenant: String, id: Uuid) -> Result<Metadata> {
		let identity = subject(&actor, &tenant)?;
		let service = service(&self.runtime.store);
		let row = service.get(&tenant, &identity.subject, id).await?;
		let action = if row.owner.is_some() {
			SECRET_USE
		} else {
			SECRET_MANAGE
		};
		let access = self
			.authorize(&actor, &tenant, "secret", &row.issuer, &[action])
			.await?;
		access.finish(Ok(row.into())).await
	}
	pub async fn put(
		&self,
		actor: Actor,
		tenant: String,
		id: Option<Uuid>,
		input: Write,
	) -> Result<Metadata> {
		let issuer = aidash_application::oauth_clients::normalize_issuer(&input.issuer)?;
		let action = if input.owner == Ownership::Me {
			SECRET_USE
		} else {
			SECRET_MANAGE
		};
		let service = service(&self.runtime.store);
		let identity = subject(&actor, &tenant)?;
		let stored_issuer = if let Some(id) = id {
			let row = service.get(&tenant, &identity.subject, id).await?;
			if row.owner.as_deref() != input.owner.subject(&identity.subject) {
				return Err(Error::NotFound("OAuth Client".into()));
			}
			row.issuer
		} else {
			issuer.clone()
		};
		let access = self
			.authorize(&actor, &tenant, "secret", &stored_issuer, &[action])
			.await?;
		let result = service
			.put(
				&tenant,
				&subject(&actor, &tenant)?.subject,
				id,
				Put {
					owner: input.owner,
					issuer,
					client_id: input.client_id,
					client_secret: input.client_secret,
					expected_revision: input.expected_revision,
				},
			)
			.await;
		access.finish(result.map_err(Into::into)).await
	}
	pub async fn delete(&self, actor: Actor, tenant: String, id: Uuid) -> Result<()> {
		let identity = subject(&actor, &tenant)?;
		let service = service(&self.runtime.store);
		let row = service.get(&tenant, &identity.subject, id).await?;
		let action = if row.owner.is_some() {
			SECRET_USE
		} else {
			SECRET_MANAGE
		};
		let access = self
			.authorize(&actor, &tenant, "secret", &row.issuer, &[action])
			.await?;
		let result = service.delete(&tenant, row.owner.as_deref(), id).await;
		access.finish(result.map_err(Into::into)).await
	}
	pub async fn ceiling(
		&self,
		actor: Actor,
		tenant: String,
		encoded: String,
	) -> Result<Option<ScopeCeiling>> {
		let resource = decode_resource(&encoded)?;
		let access = self
			.authorize(
				&actor,
				&tenant,
				"connection_scopes",
				&resource,
				&[CONNECTION_SCOPES_UPDATE],
			)
			.await?;
		let result = service(&self.runtime.store)
			.ceiling(&tenant, &resource)
			.await;
		access.finish(result.map_err(Into::into)).await
	}
	pub async fn put_ceiling(
		&self,
		actor: Actor,
		tenant: String,
		encoded: String,
		input: CeilingWrite,
	) -> Result<ScopeCeiling> {
		let resource = decode_resource(&encoded)?;
		let access = self
			.authorize(
				&actor,
				&tenant,
				"connection_scopes",
				&resource,
				&[CONNECTION_SCOPES_UPDATE],
			)
			.await?;
		let result = service(&self.runtime.store)
			.put_ceiling(&tenant, &resource, input.scopes, input.expected_revision)
			.await;
		access.finish(result.map_err(Into::into)).await
	}
	pub async fn delete_ceiling(
		&self,
		actor: Actor,
		tenant: String,
		encoded: String,
	) -> Result<()> {
		let resource = decode_resource(&encoded)?;
		let access = self
			.authorize(
				&actor,
				&tenant,
				"connection_scopes",
				&resource,
				&[CONNECTION_SCOPES_UPDATE],
			)
			.await?;
		let result = service(&self.runtime.store)
			.delete_ceiling(&tenant, &resource)
			.await;
		access.finish(result.map_err(Into::into)).await
	}
}
/// Called only after the person's last mapping has been removed.
pub(crate) async fn retire_subject(f: &Federation, tenant: &str, subject: &str) -> Result<()> {
	// Reuse the mapping/Plan Connection owner lock so a remapping cannot
	// race retirement. Identity disablement alone retains the mapping.
	let scope = aidash_application::tenancy::TenancyScope::tenant(tenant)?;
	let mut tx = f
		.store
		.control_pool
		.begin(&scope)
		.await?
		.into_executor()
		.await?;
	super::super::repositories::plan_connections::lock_owner(tx.as_mut(), tenant, subject).await?;
	if !super::super::repositories::plan_connections::mapped_identities(
		tx.as_mut(),
		tenant,
		subject,
	)
	.await?
	.is_empty()
	{
		tx.commit().await?;
		return Ok(());
	}
	if let Some(secrets) = &f.store.tenant_secrets {
		secrets.retire_subject(tenant, subject).await?;
	}
	service(&f.store).retire_subject(tenant, subject).await?;
	tx.commit().await?;
	Ok(())
}
