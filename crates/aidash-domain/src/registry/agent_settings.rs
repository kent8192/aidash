//! Thin settings for existing native resource adapters. Execution availability
//! comes from exact Provider operations, not the adapter's convenience fields.
use super::{AgentConfig, bindings::*};
use crate::{Result, tool::providers::ToolDescriptor};
use sources::{NativeContext, NativeSource};

impl AgentConfig {
	pub fn definition(&self) -> AgentBindings {
		AgentBindings {
			schema_version: self.schema_version,
			model: self.model.clone(),
			instructions: self.instructions.clone(),
			bindings: self.bindings.clone(),
			remove_default: self.remove_default.clone(),
			cluster: self.cluster.clone(),
			max_steps: self.max_steps,
			projection_version: self.projection_version,
			prompt_cache: self.prompt_cache,
		}
	}
	pub fn from_definition(input: AgentBindings) -> Self {
		let mut result = Self {
			memory: input
				.bindings
				.iter()
				.find(|b| b.kind == BindingKind::Memory)
				.map(|b| b.target.local()),
			sources: vec![],
			conversation_memory: false,
			semantic_memory: false,
			workspace_context: false,
			schema_version: input.schema_version,
			bindings: input.bindings,
			remove_default: input.remove_default,
			model: input.model,
			instructions: input.instructions,
			cluster: input.cluster,
			max_steps: input.max_steps,
			projection_version: input.projection_version,
			prompt_cache: input.prompt_cache,
			core_capabilities: Default::default(),
			skill_attachments: vec![],
			skill_roots: vec![],
			reference_attachments: vec![],
			knowledge_digest: None,
			tools: vec![],
			skills: vec![],
			allow_task_creation: Some(false),
			allow_task_delegation: Some(false),
			allow_memory_write: Some(false),
			allow_workspace_retrieval: Some(true),
			allow_cross_conversation_memory: Some(false),
		};
		for name in REQUIRED_TOOLS.iter().chain(DEFAULT_TOOLS) {
			if !result.remove_default.iter().any(|removed| removed == name) {
				result.operation(name);
			}
		}
		result.skills = result
			.bindings
			.iter()
			.filter(|b| b.kind == BindingKind::Skill)
			.map(|b| b.target.local())
			.collect();
		result
	}
	/// Callers must use this view when invoking native source/resource adapters.
	/// Every source declaration and operation was admitted with this exact Run.
	pub fn from_snapshot(snapshot: &BindingSnapshot) -> Result<Self> {
		snapshot.validate()?;
		let definition = snapshot
			.definitions
			.iter()
			.find(|d| d.identity == snapshot.agent)
			.expect("validated Agent closure");
		let input: AgentBindings = serde_json::from_value(definition.definition.config.clone())?;
		let mut result = Self::from_definition(input);
		result.core_capabilities = Default::default();
		result.allow_task_creation = Some(false);
		result.allow_task_delegation = Some(false);
		result.allow_memory_write = Some(false);
		result.allow_workspace_retrieval = Some(false);
		result.skills.clear();
		result.memory = None;
		result.sources.clear();
		for binding in &snapshot.bindings {
			if binding.excluded_reason.is_some() {
				continue;
			}
			match binding.definition.kind.as_str() {
				"tool" => {
					let descriptor: ToolDescriptor =
						serde_json::from_value(binding.definition.config.clone())?;
					result.operation(&descriptor.operation);
					result.tools.push(binding.identity.local());
				}
				"skill" => result.skills.push(binding.identity.local()),
				"memory" | "source" => {
					if binding.definition.config.get("schema_version").is_none() {
						if binding.definition.kind == "memory" {
							if result.memory.replace(binding.identity.local()).is_some() {
								return Err(crate::Error::Invalid(
									"Agent requires one primary memory provider".into(),
								));
							}
							result.conversation_memory = true;
							result.semantic_memory = true;
						} else {
							result.sources.push(binding.identity.local());
						}
						continue;
					}
					let context: NativeContext =
						serde_json::from_value(binding.definition.config.clone())?;
					match context.source {
						NativeSource::ConversationMemory {} => result.conversation_memory = true,
						NativeSource::SemanticMemory {} => result.semantic_memory = true,
						NativeSource::WorkspaceRetrieval {} => result.workspace_context = true,
						NativeSource::PrivateReferences { digest } => {
							result.knowledge_digest = Some(digest)
						}
						NativeSource::ReferenceAttachments { references } => {
							result.reference_attachments.extend(references)
						}
						NativeSource::SkillAttachments { attachments } => {
							result.core_capabilities.skills = true;
							result.skill_attachments.extend(attachments)
						}
						NativeSource::SkillRoots { roots } => {
							result.core_capabilities.skills = true;
							result.skill_roots.extend(roots)
						}
					}
				}
				_ => {}
			}
		}
		crate::capabilities::references::validate_config(&result)?;
		crate::capabilities::skills::validate_config(&result)?;
		Ok(result)
	}
	fn operation(&mut self, name: &str) {
		match name {
			"file_search" | "file_read" => self.core_capabilities.files = true,
			"shell" | "shell_poll" | "shell_cancel" => self.core_capabilities.shell = true,
			"code_interpreter" | "python_install" | "python_poll" | "python_cancel" => {
				self.core_capabilities.python = true
			}
			"apply_patch" => self.core_capabilities.patch = true,
			"file_share" => self.core_capabilities.sharing = true,
			"task_create" => self.allow_task_creation = Some(true),
			"task_delegate" | "task_assign" => self.allow_task_delegation = Some(true),
			"memory_mutate" => self.allow_memory_write = Some(true),
			"memory_recall" | "memory_reflect" => self.allow_cross_conversation_memory = Some(true),
			"outbound_get" => self.core_capabilities.outbound = true,
			"workspace_read" => self.allow_workspace_retrieval = Some(true),
			_ => {}
		}
	}
	pub fn needs_context_authority(&self) -> bool {
		// Mounted files need area authority. Memory, workspace and private Source
		// reads retain their own resource authority without requiring a local area.
		self.core_capabilities.enabled()
	}
	pub fn permits_builtin(&self, name: &str) -> bool {
		REQUIRED_TOOLS.contains(&name)
			|| DEFAULT_TOOLS.contains(&name) && !self.remove_default.iter().any(|n| n == name)
	}
}

impl BindingSnapshot {
	pub fn operation(&self, name: &str) -> Result<&ResolvedBinding> {
		self.bindings
			.iter()
			.find(|binding| {
				binding.excluded_reason.is_none()
					&& binding.definition.kind == "tool"
					&& serde_json::from_value::<ToolDescriptor>(binding.definition.config.clone())
						.is_ok_and(|descriptor| descriptor.operation == name)
			})
			.ok_or_else(|| crate::Error::Invalid(format!("Run has no bound operation: {name}")))
	}
}
