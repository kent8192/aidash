//! HTTP input adaptation for private agent definitions.
pub use crate::apps::registry::serializers::knowledge::{PersonalAgent, ReferenceDocument};
use crate::{Error, Result, federation::Federation, registry::Entry};
use http::HeaderMap;
use reinhardt::{db::orm::DatabaseConnection, injectable};
use serde_json::Value;
use uuid::Uuid;
pub(crate) fn validate(documents: &[ReferenceDocument]) -> Result<()> {
	aidash_domain::registry::knowledge::validate(documents).map_err(Into::into)
}
pub(crate) use aidash_domain::registry::knowledge::digest;
pub async fn load(db: &DatabaseConnection, entry: &Entry) -> Result<Value> {
	crate::apps::registry::repositories::private_documents(db, entry).await
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
	pub(crate) async fn create(&self, headers: HeaderMap, input: PersonalAgent) -> Result<Entry> {
		let draft =
			aidash_application::registry::personal::validate_input(input.entry, input.documents)?;
		let key = headers
			.get("idempotency-key")
			.and_then(|value| value.to_str().ok())
			.and_then(|value| Uuid::parse_str(value).ok())
			.ok_or_else(|| Error::Invalid("Idempotency-Key must be a UUID".into()))?;
		self.runtime.registry.register_personal(draft, key).await
	}
}
