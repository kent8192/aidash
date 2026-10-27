//! Tenant Secret HTTP composition and live policy decisions (ADR 0051). Only a
//! Subject of the Tenant manages its Tenant Secrets, under `secret.manage`; a
//! Deployment Administrator does not.
use super::super::serializers::tenant_secrets::{Revision, Write};
use crate::{
	Error, Result,
	authorization::{access::NativeAccess, identity::Actor},
	federation::Federation,
};
use aidash_application::tenant_secrets::{Metadata, Ownership, Put, Service};
use aidash_domain::identity::tenant_administration::{COLLECTION, SECRET_MANAGE, SECRET_USE};
use reinhardt::injectable;
use serde_json::json;
use std::sync::Arc;

#[derive(Clone)]
pub struct Management {
	pub runtime: Federation,
}
#[injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> Management {
	Management { runtime }
}
impl Management {
	fn service(&self) -> Result<Arc<Service>> {
		self.runtime.store.tenant_secrets.clone().ok_or_else(|| {
			Error::NotFound("Tenant Secrets are not available on this Deployment".into())
		})
	}
	fn subject<'a>(
		&self,
		actor: &'a Actor,
		tenant: &str,
	) -> Result<&'a crate::authorization::identity::SubjectIdentity> {
		aidash_domain::policy::identifier(tenant)?;
		match actor {
			Actor::Subject(identity) if identity.tenant == tenant => Ok(identity),
			_ => Err(Error::Forbidden),
		}
	}
	/// The decision on `name` for any of `actions`, recorded with the Tenant's
	/// decisions once the operation finishes.
	async fn authorize(
		&self,
		actor: &Actor,
		tenant: &str,
		name: &str,
		actions: &[&str],
	) -> Result<NativeAccess> {
		let identity = self.subject(actor, tenant)?;
		let mut access = NativeAccess::begin(&self.runtime.store, identity).await?;
		let resource = access.resource("secret", name, json!({"name": name}));
		let mut allowed = false;
		for action in actions {
			if access.decide(&resource, action).await? {
				allowed = true;
				break;
			}
		}
		if !allowed {
			return access.finish(Err(Error::Forbidden)).await;
		}
		Ok(access)
	}
	/// Whoever may manage or use Tenant Secrets sees their names and hosts.
	pub async fn list(&self, actor: Actor, tenant: String) -> Result<Vec<Metadata>> {
		let service = self.service()?;
		let access = self
			.authorize(&actor, &tenant, COLLECTION, &[SECRET_MANAGE, SECRET_USE])
			.await?;
		let result = service
			.list_for(&tenant, &self.subject(&actor, &tenant)?.subject)
			.await;
		access.finish(result.map_err(Into::into)).await
	}
	pub async fn get(
		&self,
		actor: Actor,
		tenant: String,
		name: String,
		owner: Ownership,
	) -> Result<Metadata> {
		let service = self.service()?;
		let access = self
			.authorize(
				&actor,
				&tenant,
				&name,
				if owner == Ownership::Me {
					&[SECRET_USE]
				} else {
					&[SECRET_MANAGE]
				},
			)
			.await?;
		let result = service
			.get_owned(
				&tenant,
				owner.subject(&self.subject(&actor, &tenant)?.subject),
				&name,
			)
			.await;
		access.finish(result.map_err(Into::into)).await
	}
	pub async fn put(
		&self,
		actor: Actor,
		tenant: String,
		name: String,
		input: Write,
	) -> Result<Metadata> {
		let service = self.service()?;
		let access = self
			.authorize(
				&actor,
				&tenant,
				&name,
				if input.owner == Ownership::Me {
					&[SECRET_USE]
				} else {
					&[SECRET_MANAGE]
				},
			)
			.await?;
		let subject = self.subject(&actor, &tenant)?.subject.clone();
		let result = service
			.put_owned(
				&tenant,
				input.owner.subject(&subject),
				&name,
				Put {
					expected_revision: input.expected_revision,
					value: input.value,
					hosts: input.hosts,
				},
				&subject,
			)
			.await;
		access.finish(result.map_err(Into::into)).await
	}
	pub async fn delete(
		&self,
		actor: Actor,
		tenant: String,
		name: String,
		input: Revision,
	) -> Result<Metadata> {
		let service = self.service()?;
		let access = self
			.authorize(
				&actor,
				&tenant,
				&name,
				if input.owner == Ownership::Me {
					&[SECRET_USE]
				} else {
					&[SECRET_MANAGE]
				},
			)
			.await?;
		let subject = self.subject(&actor, &tenant)?.subject.clone();
		let result = service
			.delete_owned(
				&tenant,
				input.owner.subject(&subject),
				&name,
				input.expected_revision,
				&subject,
			)
			.await;
		access.finish(result.map_err(Into::into)).await
	}
}
