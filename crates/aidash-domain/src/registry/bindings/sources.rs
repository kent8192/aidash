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
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub context_source: Option<crate::context::source::SourceDeclaration>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "adapter", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeSource {
	ConversationMemory {},
	SemanticMemory {},
	WorkspaceRetrieval {
		/// What a local Run step does when this retrieval stays unavailable
		/// after the bounded retry (ADR 0059). Required-Home Runs always pause.
		#[serde(default, skip_serializing_if = "OnUnavailable::is_pause")]
		on_unavailable: OnUnavailable,
	},
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
/// Whether a step may go without optional Workspace semantic context when its
/// retrieval stays unavailable. `Pause` is the default: degrading is an
/// explicit opt-in of the Agent's author.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OnUnavailable {
	#[default]
	Pause,
	Degrade,
}
impl OnUnavailable {
	pub fn is_pause(&self) -> bool {
		*self == Self::Pause
	}
}

impl NativeContext {
	pub fn validate(&self, kind: &str) -> Result<()> {
		if let Some(declaration) = &self.context_source {
			declaration.validate()?;
		}
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
	/// Native providers preserve the existing bounded automatic behavior unless opted in.
	pub fn declaration(
		&self,
		identity: &super::QualifiedRef,
	) -> crate::context::source::SourceDeclaration {
		use crate::context::source::*;
		if let Some(declaration) = &self.context_source {
			return declaration.clone();
		}
		let operation = match self.source {
			NativeSource::ConversationMemory { .. } => "conversation_memory",
			NativeSource::SemanticMemory { .. } => "semantic_memory",
			NativeSource::WorkspaceRetrieval { .. } => "workspace_retrieval",
			NativeSource::PrivateReferences { .. } | NativeSource::ReferenceAttachments { .. } => {
				"references"
			}
			NativeSource::SkillAttachments { .. } | NativeSource::SkillRoots { .. } => "skills",
		};
		let read = match self.source {
			NativeSource::ConversationMemory { .. } | NativeSource::SemanticMemory { .. } => {
				vec!["memory_recall"]
			}
			NativeSource::WorkspaceRetrieval { .. } => vec!["workspace_read"],
			NativeSource::PrivateReferences { .. } | NativeSource::ReferenceAttachments { .. } => {
				vec!["file_read"]
			}
			NativeSource::SkillAttachments { .. } | NativeSource::SkillRoots { .. } => {
				vec!["skill_read"]
			}
		};
		SourceDeclaration {
			provider: "aidash.native-context/1".into(),
			operation: operation.into(),
			tools: SourceTools {
				read: read
					.into_iter()
					.map(|op| super::QualifiedRef::builtin(&identity.registry_deployment, op))
					.collect(),
				..Default::default()
			},
			pin: PinPolicy::Always,
			volatility: if operation == "skills" || operation == "references" {
				Volatility::Run
			} else {
				Volatility::Step
			},
			query: QueryPolicy::default(),
			refresh: RefreshPolicy::default(),
			disclosure: SourceDisclosure::default(),
			required_home: false,
		}
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

/// Validate either a native memory role or an explicit mounted context descriptor.
pub fn validate_definition(entry: &crate::registry::Entry) -> Result<Option<NativeContext>> {
	if entry.config.get("schema_version").is_some() {
		let descriptor: NativeContext = serde_json::from_value(entry.config.clone())?;
		descriptor.validate(&entry.kind)?;
		return Ok(Some(descriptor));
	}
	match entry.kind.as_str() {
		"memory" => serde_json::from_value::<crate::memory::ProviderConfig>(entry.config.clone())?
			.policy
			.validate()?,
		"source" => {
			let source: crate::memory::SourceConfig = serde_json::from_value(entry.config.clone())?;
			if source.max_tokens == 0 || source.max_tokens > i32::MAX as usize {
				return Err(Error::Invalid(
					"memory Source requires a finite context cap".into(),
				));
			}
		}
		_ => return Err(Error::Invalid("unsupported context definition".into())),
	}
	Ok(None)
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
			json!({"schema_version":1,"source":{"adapter":"semantic_memory","on_unavailable":"degrade"}}),
			json!({"schema_version":1,"source":{"adapter":"workspace_retrieval","on_unavailable":"skip"}}),
		] {
			assert!(serde_json::from_value::<NativeContext>(config).is_err());
		}
	}
	#[test]
	fn only_workspace_retrieval_opts_into_degrading_and_pause_stays_implicit() {
		let parse = |source| {
			serde_json::from_value::<NativeContext>(json!({"schema_version":1,"source":source}))
				.unwrap()
		};
		let default = parse(json!({"adapter":"workspace_retrieval"}));
		default.validate("source").unwrap();
		assert!(matches!(
			default.source,
			NativeSource::WorkspaceRetrieval {
				on_unavailable: OnUnavailable::Pause
			}
		));
		// The default is never written, so existing descriptors keep their digest.
		assert_eq!(
			serde_json::to_value(&default).unwrap(),
			json!({"schema_version":1,"source":{"adapter":"workspace_retrieval"}})
		);
		let degrade = parse(json!({"adapter":"workspace_retrieval","on_unavailable":"degrade"}));
		degrade.validate("source").unwrap();
		assert_eq!(
			serde_json::to_value(&degrade).unwrap(),
			json!({"schema_version":1,"source":{"adapter":"workspace_retrieval","on_unavailable":"degrade"}})
		);
	}
	#[test]
	fn private_sources_pin_digest_and_validate_roots_without_implying_authority() {
		let private = NativeContext {
			schema_version: 1,
			context_source: None,
			source: NativeSource::PrivateReferences {
				digest: "0".repeat(64),
			},
		};
		private.validate("source").unwrap();
		assert!(!private.memory());
		let roots = NativeContext {
			schema_version: 1,
			context_source: None,
			source: NativeSource::SkillRoots {
				roots: vec![".agents/skills".into()],
			},
		};
		roots.validate("source").unwrap();
		assert!(roots.requires_skill_support());
		let invalid = NativeContext {
			schema_version: 1,
			context_source: None,
			source: NativeSource::SkillRoots {
				roots: vec!["../.agents/skills".into()],
			},
		};
		assert!(invalid.validate("source").is_err());
	}
}
