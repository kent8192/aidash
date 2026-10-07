//! Explicit declarations for current native context adapters, without routing or
//! a general ContextSource framework. Source content is authorized separately.
use crate::{
	Error, Result,
	capabilities::{ReferenceAttachment, SkillAttachment},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeContext {
	pub schema_version: u8,
	pub source: NativeSource,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "adapter", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeSource {
	ConversationMemory {},
	SemanticMemory {},
	WorkspaceRetrieval {},
	PrivateReferences {
		digest: String,
	},
	ReferenceAttachments {
		references: Vec<ReferenceAttachment>,
	},
	SkillAttachments {
		attachments: Vec<SkillAttachment>,
	},
	SkillRoots {
		roots: Vec<String>,
	},
}
impl NativeContext {
	pub fn validate(&self, kind: &str) -> Result<()> {
		if self.schema_version != super::BINDING_SCHEMA
			|| self.memory() != (kind == "memory")
			|| !matches!(kind, "memory" | "source")
		{
			return Err(Error::Invalid(
				"unsupported native context Binding adapter or schema".into(),
			));
		}
		fn digest(value: &str) -> bool {
			value.len() == 64 && value.bytes().all(|c| c.is_ascii_hexdigit())
		}
		match &self.source {
			NativeSource::PrivateReferences { digest: value } if !digest(value) => {
				return Err(Error::Invalid(
					"private references require an immutable content digest".into(),
				));
			}
			NativeSource::ReferenceAttachments { references } => {
				let mut seen = BTreeSet::new();
				if references.is_empty()
					|| references.len() > 8
					|| references
						.iter()
						.any(|r| !seen.insert(r.reference_id) || !digest(&r.digest))
				{
					return Err(Error::Invalid(
						"invalid reference context attachments".into(),
					));
				}
			}
			NativeSource::SkillAttachments { attachments } => {
				let mut seen = BTreeSet::new();
				if attachments.is_empty() || attachments.len() > 16 {
					return Err(Error::Invalid("invalid Skill context attachments".into()));
				}
				for attachment in attachments {
					crate::capabilities::skills::validate(attachment)?;
					if !seen.insert(attachment.skill_id) {
						return Err(Error::Invalid("duplicate Skill context attachment".into()));
					}
				}
			}
			NativeSource::SkillRoots { roots } => {
				if roots.is_empty()
					|| roots.len() > 8
					|| roots.iter().collect::<BTreeSet<_>>().len() != roots.len()
				{
					return Err(Error::Invalid("invalid Skill context roots".into()));
				}
				for root in roots {
					crate::registry::rules::validate_path(root)?;
					if root != ".agents/skills" && !root.ends_with("/.agents/skills") {
						return Err(Error::Invalid(
							"Skill roots must address a mounted .agents/skills directory".into(),
						));
					}
				}
			}
			_ => {}
		}
		Ok(())
	}
	pub fn memory(&self) -> bool {
		matches!(
			self.source,
			NativeSource::ConversationMemory { .. } | NativeSource::SemanticMemory { .. }
		)
	}
	pub fn requires_skill_support(&self) -> bool {
		matches!(
			self.source,
			NativeSource::SkillAttachments { .. } | NativeSource::SkillRoots { .. }
		)
	}
}

/// Native adapters mount one combined set, even when several immutable Source
/// declarations contribute content. Validate aggregate identities before a Run
/// is admitted rather than failing at its first mount.
pub(super) fn validate_mounts(contexts: impl IntoIterator<Item = NativeContext>) -> Result<()> {
	let mut references = BTreeSet::new();
	let mut skills = BTreeSet::new();
	let mut roots = BTreeSet::new();
	for context in contexts {
		match context.source {
			NativeSource::ReferenceAttachments {
				references: attachments,
			} => {
				for attachment in attachments {
					if !references.insert(attachment.reference_id) || references.len() > 8 {
						return Err(Error::Invalid(
							"ambiguous or excessive aggregate reference Sources".into(),
						));
					}
				}
			}
			NativeSource::SkillAttachments { attachments } => {
				for attachment in attachments {
					if !skills.insert(attachment.skill_id) || skills.len() > 16 {
						return Err(Error::Invalid(
							"ambiguous or excessive aggregate Skill Sources".into(),
						));
					}
				}
			}
			NativeSource::SkillRoots { roots: paths } => {
				for root in paths {
					if !roots.insert(root) || roots.len() > 8 {
						return Err(Error::Invalid(
							"ambiguous or excessive aggregate Skill root Sources".into(),
						));
					}
				}
			}
			_ => {}
		}
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;
	#[test]
	fn adapters_are_explicit_and_future_framework_or_policy_fields_are_rejected() {
		for adapter in ["conversation_memory", "semantic_memory"] {
			let descriptor: NativeContext =
				serde_json::from_value(json!({"schema_version":1,"source":{"adapter":adapter}}))
					.unwrap();
			descriptor.validate("memory").unwrap();
			assert!(descriptor.validate("source").is_err());
		}
		for config in [
			json!({"schema_version":1,"source":{"adapter":"future_provider"}}),
			json!({"schema_version":1,"source":{"adapter":"conversation_memory","allow_cross_conversation_memory":true}}),
		] {
			assert!(serde_json::from_value::<NativeContext>(config).is_err());
		}
	}
	#[test]
	fn private_sources_pin_digest_and_validate_roots_without_implying_authority() {
		let private = NativeContext {
			schema_version: 1,
			source: NativeSource::PrivateReferences {
				digest: "0".repeat(64),
			},
		};
		private.validate("source").unwrap();
		assert!(!private.memory());
		let roots = NativeContext {
			schema_version: 1,
			source: NativeSource::SkillRoots {
				roots: vec![".agents/skills".into()],
			},
		};
		roots.validate("source").unwrap();
		assert!(roots.requires_skill_support());
		let invalid = NativeContext {
			schema_version: 1,
			source: NativeSource::SkillRoots {
				roots: vec!["../.agents/skills".into()],
			},
		};
		assert!(invalid.validate("source").is_err());
	}
}
