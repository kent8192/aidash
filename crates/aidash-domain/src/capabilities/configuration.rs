//! Immutable agent revisions preserve attachment identity and catalog approval boundaries.
use super::{CoreCapabilities, ReferenceAttachment, SkillAttachment};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Configure {
	pub idempotency_key: Uuid,
	pub source_version: String,
	pub new_version: String,
	pub core_capabilities: CoreCapabilities,
	pub skill_attachments: Vec<SkillAttachment>,
	pub skill_roots: Vec<String>,
	pub reference_attachments: Vec<ReferenceAttachment>,
}
