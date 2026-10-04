use serde::Serialize;
// Serializable contracts for authorization.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;

#[derive(Debug, Serialize, sqlx::FromRow, JsonSchema)]
pub struct Credential {
	pub id: Uuid,
	pub tenant: String,
	pub subject: String,
	pub created_at: DateTime<Utc>,
	pub expires_at: DateTime<Utc>,
	pub revoked_at: Option<DateTime<Utc>>,
	pub issued_by: String,
}

/// The bearer value is returned exactly once; only its SHA-256 digest is stored.
#[derive(Serialize, JsonSchema)]
pub struct IssuedCredential {
	pub credential: Credential,
	pub token: String,
}

use uuid::Uuid;
