//! Provider Credential HTTP composition, live policy decisions, and immutable admission pins.
use super::super::serializers::provider_credentials::{BindingUpdate, Page, Revision};
use crate::{
	Error, Result,
	authorization::{access::NativeAccess, identity::Actor},
	federation::Federation,
};
use aidash_application::provider_credentials::{Metadata, Service, Validated};
use aidash_domain::{
	policy::{Evaluation, Resource},
	provider_credentials::{Binding, Provider},
};
use reinhardt::injectable;
use secrecy::SecretString;
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
struct Authority(Option<NativeAccess>);
impl Authority {
	async fn finish<T>(self, value: Result<T>) -> Result<T> {
		match self.0 {
			Some(access) => access.finish(value).await,
			None => value,
		}
	}
}
impl Management {
	fn service(&self) -> Result<Arc<Service>> {
		self.runtime
			.store
			.provider_credentials
			.clone()
			.ok_or_else(|| Error::NotFound("Provider Credential Store is not configured".into()))
	}
	fn tenant(&self, actor: &Actor, tenant: &str) -> Result<()> {
		aidash_domain::policy::identifier(tenant)?;
		if let Actor::Subject(identity) = actor
			&& identity.tenant != tenant
		{
			return Err(Error::Forbidden);
		}
		Ok(())
	}
	async fn authorize(
		&self,
		actor: &Actor,
		tenant: &str,
		id: &str,
		provider: Provider,
		kind: &str,
		verb: &str,
	) -> Result<Authority> {
		self.tenant(actor, tenant)?;
		let resource = Resource {
			tenant: tenant.into(),
			kind: kind.into(),
			id: id.into(),
			attributes: json!({"provider":provider.id()}),
		};
		let action = format!("{kind}.{verb}");
		match actor {
			Actor::Subject(identity) => {
				let mut access = NativeAccess::begin(&self.runtime.store, identity).await?;
				let result = access.require(&resource, &action).await;
				if let Err(error) = result {
					return access.finish(Err(error)).await;
				}
				Ok(Authority(Some(access)))
			}
			Actor::Operator => {
				use crate::apps::identity::{
					models::AuthorizationDecision, repositories::NativePolicyScope,
				};
				use aidash_application::ports::AuthorizationScope;
				let mut tx = self.runtime.store.control_pool.connection().begin().await?;
				let snapshot = NativePolicyScope(tx.as_mut()).load(tenant).await?;
				let evaluation = Evaluation {
					subject: "operator".into(),
					action,
					resource,
					environment: json!({"operator":true}),
				};
				let mut decision = snapshot.bundle.evaluate(&evaluation);
				decision.allowed = true;
				decision.reason = "authenticated operator authority".into();
				decision.revision = snapshot.revision;
				AuthorizationDecision::append(tx.as_mut(), tenant, &evaluation, &decision).await?;
				tx.commit().await?;
				Ok(Authority(None))
			}
		}
	}
	fn actor(actor: &Actor) -> &str {
		match actor {
			Actor::Operator => "operator",
			Actor::Subject(identity) => &identity.subject,
		}
	}
	/// Internal Provider Authorization write path; never registered as an HTTP endpoint.
	pub async fn create(
		&self,
		actor: Actor,
		tenant: String,
		provider: Provider,
		key_material: SecretString,
	) -> Result<Validated> {
		let service = self.service()?;
		let id = Uuid::now_v7();
		let authority = self
			.authorize(
				&actor,
				&tenant,
				&id.to_string(),
				provider,
				"provider_credential",
				"create",
			)
			.await?;
		authority
			.finish(
				service
					.create(&tenant, id, provider, key_material, Self::actor(&actor))
					.await
					.map_err(Into::into),
			)
			.await
	}
	pub async fn list(&self, actor: Actor, tenant: String, page: Page) -> Result<Vec<Metadata>> {
		let service = self.service()?;
		self.tenant(&actor, &tenant)?;
		let mut scope = service.repository.begin(&tenant).await?;
		let rows = scope.list(page.offset, page.limit).await?;
		scope.commit().await?;
		let mut result = Vec::new();
		for row in rows {
			match self
				.authorize(
					&actor,
					&tenant,
					&row.id.to_string(),
					row.provider,
					"provider_credential",
					"read",
				)
				.await
			{
				Ok(authority) => result.push(authority.finish(Ok(row.into())).await?),
				Err(Error::Forbidden) => {}
				Err(error) => return Err(error),
			}
		}
		Ok(result)
	}
	pub async fn get(&self, actor: Actor, tenant: String, id: Uuid) -> Result<Metadata> {
		let service = self.service()?;
		self.tenant(&actor, &tenant)?;
		let mut scope = service.repository.begin(&tenant).await?;
		let row = scope.get(id).await?;
		scope.commit().await?;
		self.authorize(
			&actor,
			&tenant,
			&id.to_string(),
			row.provider,
			"provider_credential",
			"read",
		)
		.await?
		.finish(Ok(row.into()))
		.await
	}
	/// Internal Provider Authorization reconnect path; accepts no HTTP serializer.
	pub async fn rotate(
		&self,
		actor: Actor,
		tenant: String,
		id: Uuid,
		expected_revision: i64,
		key_material: SecretString,
	) -> Result<Validated> {
		let service = self.service()?;
		self.tenant(&actor, &tenant)?;
		let mut scope = service.repository.begin(&tenant).await?;
		let row = scope.get(id).await?;
		scope.commit().await?;
		let authority = self
			.authorize(
				&actor,
				&tenant,
				&id.to_string(),
				row.provider,
				"provider_credential",
				"rotate",
			)
			.await?;
		authority
			.finish(
				service
					.rotate(
						&tenant,
						id,
						expected_revision,
						key_material,
						Self::actor(&actor),
					)
					.await
					.map_err(Into::into),
			)
			.await
	}
	pub async fn control(
		&self,
		actor: Actor,
		tenant: String,
		id: Uuid,
		input: Revision,
		delete: bool,
	) -> Result<Metadata> {
		let service = self.service()?;
		self.tenant(&actor, &tenant)?;
		let mut scope = service.repository.begin(&tenant).await?;
		let row = scope.get(id).await?;
		scope.commit().await?;
		let authority = self
			.authorize(
				&actor,
				&tenant,
				&id.to_string(),
				row.provider,
				"provider_credential",
				if delete { "delete" } else { "revoke" },
			)
			.await?;
		let result = if delete {
			service
				.delete(&tenant, id, input.expected_revision, Self::actor(&actor))
				.await
		} else {
			service
				.revoke(&tenant, id, input.expected_revision, Self::actor(&actor))
				.await
		};
		authority.finish(result.map_err(Into::into)).await
	}
	pub async fn bindings(
		&self,
		actor: Actor,
		tenant: String,
		provider: Option<String>,
	) -> Result<Vec<Binding>> {
		let service = self.service()?;
		self.tenant(&actor, &tenant)?;
		let provider = provider.as_deref().map(Provider::parse).transpose()?;
		let mut scope = service.repository.begin(&tenant).await?;
		let rows = scope.bindings().await?;
		scope.commit().await?;
		let mut result = Vec::new();
		for row in rows
			.into_iter()
			.filter(|r| provider.is_none_or(|p| p == r.provider))
		{
			let authority = self
				.authorize(
					&actor,
					&tenant,
					row.provider.id(),
					row.provider,
					"provider_credential_binding",
					"read",
				)
				.await?;
			result.push(authority.finish(Ok(row)).await?);
		}
		Ok(result)
	}
	pub async fn get_binding(
		&self,
		actor: Actor,
		tenant: String,
		provider: String,
	) -> Result<Binding> {
		self.bindings(actor, tenant, Some(provider))
			.await?
			.into_iter()
			.next()
			.ok_or_else(|| Error::NotFound("Provider Credential Binding".into()))
	}
	pub async fn bind(
		&self,
		actor: Actor,
		tenant: String,
		provider: String,
		input: BindingUpdate,
	) -> Result<Binding> {
		let service = self.service()?;
		let provider = Provider::parse(&provider)?;
		let authority = self
			.authorize(
				&actor,
				&tenant,
				provider.id(),
				provider,
				"provider_credential_binding",
				"update",
			)
			.await?;
		authority
			.finish(
				service
					.bind(
						&tenant,
						provider,
						input.provider_credential_id,
						input.expected_revision,
						Self::actor(&actor),
					)
					.await
					.map_err(Into::into),
			)
			.await
	}
}
