//! Plan Connection settings and contracts (#159). No request or response
//! carries a Plan Token, the issued client or the PKCE verifier.
use aidash_application::plan_connections::{Metadata, Outcome};
use aidash_domain::plan_connections::PlanModel;
use reinhardt::settings;
use serde::{Deserialize, Serialize};

/// `[plan_connections]` with `enabled = true`. Off by default: whoever runs a
/// Self-hosted Deployment turns Plan Connections on, accepting that the host
/// can read the Plan Tokens it stores (ADR 0007, ADR 0008). A server whose
/// Secret Store is not PostgreSQL refuses to start with it on.
/// Turning it off again keeps existing connections readable and
/// disconnectable, and stops everything else.
#[settings(fragment = true, section = "plan_connections")]
pub struct PlanConnectionSettings {
	#[setting(default = "false")]
	pub enabled: bool,
}

/// Starts one authorization. A desktop client names the loopback port it
/// listens on; without one, the owner pastes the callback URL back.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "PlanAuthorizationStart")]
pub struct Start {
	#[serde(default)]
	pub port: Option<u16>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
#[schemars(rename = "PlanAuthorizationStarted")]
pub struct Started {
	/// OpenAI's consent page; the system or dashboard browser opens it.
	pub authorization_url: String,
	/// Where the browser lands after consent. For a paste, nothing listens
	/// there and the owner copies the address from the browser.
	pub redirect_uri: String,
}

/// The callback URL a desktop client relayed or the owner pasted.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "PlanAuthorizationComplete")]
pub struct Complete {
	pub callback_url: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
#[schemars(rename = "PlanAuthorizationCompleted")]
pub struct Completed {
	pub outcome: Outcome,
	/// The owner's connection after the attempt, if any.
	pub connection: Option<Metadata>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "PlanDisconnect")]
pub struct Disconnect {
	pub expected_revision: i64,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
#[schemars(rename = "PlanConnectionStatus")]
pub struct Status {
	/// Whether this Deployment lets its subjects connect a ChatGPT plan.
	pub enabled: bool,
	pub connection: Option<Metadata>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
#[schemars(rename = "PlanModels")]
pub struct Models {
	pub models: Vec<PlanModel>,
}
