//! HTTP input adaptation for private agent definitions.
pub use crate::apps::registry::serializers::knowledge::{PersonalAgent, ReferenceDocument};
use crate::{Error, Result, federation::Federation, registry::Entry};
use http::HeaderMap;
use reinhardt::injectable;
use serde_json::Value;
use uuid::Uuid;
pub async fn load(
	registry: &crate::registry::Registry,
	tenant: &aidash_domain::tenancy::TenantName,
	entry: &Entry,
) -> Result<Value> {
	registry.private_documents(tenant, entry).await
}
#[derive(Clone)]
pub struct PersonalAgents {
	pub(crate) runtime: Federation,
}
#[injectable(scope = "request")]
pub async fn provide_personal_agents(#[inject] runtime: Federation) -> PersonalAgents {
	PersonalAgents { runtime }
}
impl PersonalAgents {
	/// An agent version with its Reference Documents, under `registry.create`.
	pub(crate) async fn create(
		&self,
		actor: crate::authorization::identity::Actor,
		headers: HeaderMap,
		input: PersonalAgent,
	) -> Result<Entry> {
		let crate::authorization::identity::Actor::Subject(identity) = actor else {
			return Err(Error::Forbidden);
		};
		let draft =
			aidash_application::registry::personal::validate_input(input.entry, input.documents)?;
		let key = headers
			.get("idempotency-key")
			.and_then(|value| value.to_str().ok())
			.and_then(|value| Uuid::parse_str(value).ok())
			.ok_or_else(|| Error::Invalid("Idempotency-Key must be a UUID".into()))?;
		let f = &self.runtime;
		let mut access = crate::authorization::access::Access::begin(&f.store, &identity).await?;
		let result = register(f, &mut access, draft, key).await;
		access.finish(result).await
	}
}

async fn register(
	f: &Federation,
	access: &mut crate::authorization::access::Access,
	draft: aidash_application::registry::personal::PersonalDraft,
	key: Uuid,
) -> Result<Entry> {
	let validation = f.registry.validation();
	let mut scope = crate::apps::registry::repositories::requests::TenantRegistration {
		access,
		store: &f.store,
	};
	let registration = aidash_application::registry::personal::prepare(
		&mut scope,
		&validation,
		draft,
		&f.config.node_id,
	)
	.await?;
	Ok(aidash_application::registry::personal::register(
		&mut scope,
		&validation,
		registration,
		key,
		&f.config.node_id,
	)
	.await?)
}
