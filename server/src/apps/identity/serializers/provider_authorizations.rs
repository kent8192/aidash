//! Provider Authorization contracts. No request or response carries Key Material.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Starts a connection, or a reconnect (rotation) when both
/// `provider_credential_id` and `expected_revision` are present.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "ProviderAuthorizationStart")]
pub struct Start {
	#[serde(default)]
	pub provider_credential_id: Option<Uuid>,
	#[serde(default)]
	pub expected_revision: Option<i64>,
	/// A same-origin path to return to after the callback.
	#[serde(default)]
	pub return_to: Option<String>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
#[schemars(rename = "ProviderAuthorizationStarted")]
pub struct Started {
	/// The provider-hosted consent page; the browser navigates there.
	pub authorization_url: String,
}

/// The provider appends `code` and echoes `state`; a declined consent has no code.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[schemars(rename = "ProviderAuthorizationCallback")]
pub struct CallbackQuery {
	#[serde(default)]
	pub state: Option<String>,
	#[serde(default)]
	pub code: Option<String>,
}

/// Reported to the dashboard as the `provider_authorization` query parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
	Connected,
	Reconnected,
	Denied,
	Failed,
}
impl Outcome {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Connected => "connected",
			Self::Reconnected => "reconnected",
			Self::Denied => "denied",
			Self::Failed => "failed",
		}
	}
}
