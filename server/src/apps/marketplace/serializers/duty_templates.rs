//! Duty template request and response contracts (#1011 D4).
use crate::apps::workspaces::serializers::duties::DutyView;
use crate::registry::EntityRef;
use aidash_domain::duties::templates::{ExpectedTrigger, TriggerNote};
use reinhardt::Validate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

fn suggest() -> bool {
	true
}

/// How a person publishes one of their Duties as a Marketplace template.
/// The template comes from the Duty; this names the package and adds an
/// instruction for each of the Duty's triggers. It has no place for a
/// secret: a field it does not know is refused.
#[derive(Clone, Serialize, Deserialize, JsonSchema, Validate)]
#[schemars(rename = "DutyTemplatePublish")]
#[serde(deny_unknown_fields)]
pub struct PublishDutyTemplate {
	#[validate(length(min = 1, max = 256))]
	pub package_id: String,
	/// A semantic version such as `1.0.0`.
	#[validate(length(min = 1, max = 64))]
	pub version: String,
	#[validate(length(min = 1, max = 256))]
	pub author: String,
	/// A short description for the Marketplace list; the start of the
	/// mission when empty.
	#[serde(default)]
	#[validate(length(max = 4000))]
	pub description: String,
	/// What whoever installs it sets up for each of the Duty's triggers.
	#[serde(default)]
	pub triggers: Vec<TriggerNote>,
	/// Suggest the Duty's Agent to whoever installs it.
	#[serde(default = "suggest")]
	pub suggest_agent: bool,
	pub idempotency_key: Uuid,
}

/// How a person sets up a Duty from a Marketplace template.
#[derive(Clone, Serialize, Deserialize, JsonSchema, Validate)]
#[schemars(rename = "DutyTemplateInstall")]
#[serde(deny_unknown_fields)]
pub struct InstallDutyTemplate {
	/// The digest of the version the person reviewed.
	#[validate(length(min = 1, max = 128))]
	pub digest: String,
	/// The Workspace the Duty belongs to.
	pub workspace_id: Uuid,
	/// The Agent that carries the Duty out; the one the template suggests
	/// when omitted.
	#[serde(default)]
	pub agent: Option<EntityRef>,
	pub idempotency_key: Uuid,
}

/// The later template version a Duty's Requester moves it to.
#[derive(Clone, Serialize, Deserialize, JsonSchema, Validate)]
#[schemars(rename = "DutyTemplateUpdate")]
#[serde(deny_unknown_fields)]
pub struct UpdateDutyTemplate {
	#[validate(length(min = 1, max = 256))]
	pub key: String,
	#[validate(length(min = 1, max = 128))]
	pub digest: String,
}

/// The Duty a template set up, and the triggers the person creates on it.
#[derive(Clone, Serialize, JsonSchema)]
#[schemars(rename = "DutyTemplateInstalled")]
pub(crate) struct InstalledDuty {
	pub duty: DutyView,
	/// Webhook, `duty_completed` and `node_event` triggers the template
	/// expects, each with its instruction. A webhook's secret is shown only
	/// to the person who creates it.
	pub triggers_to_create: Vec<ExpectedTrigger>,
}
