use serde::{Deserialize, Serialize};
// Serializable configuration contracts.
use crate::apps::execution::capabilities::services::core::{CoreCapabilities, references, skills};

#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema, reinhardt::Validate)]
#[serde(deny_unknown_fields)]
pub struct Configure {
	pub idempotency_key: Uuid,
	#[validate(length(min = 1, max = 128))]
	pub source_version: String,
	#[validate(length(min = 1, max = 128))]
	pub new_version: String,
	pub core_capabilities: CoreCapabilities,
	pub skill_attachments: Vec<skills::SkillAttachment>,
	pub skill_roots: Vec<String>,
	pub reference_attachments: Vec<references::Attachment>,
}

use uuid::Uuid;
