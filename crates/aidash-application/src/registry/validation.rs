//! Definition admission resolves local credentials only through the injected port.
use crate::{
	Error, Result,
	ports::{Credentials, bindings::ProviderCatalog, registry::CoreToolCatalog},
};
use aidash_domain::registry::rules::skill_instructions;
use aidash_domain::{
	configuration::{validate_endpoint, validate_node_id, validate_secret_reference},
	model::ModelConfig,
	registry::{AgentConfig, ClusterConfig, CompactorConfig, EntityRef, Entry},
	tool::ToolConfig,
};
use serde_json::Value;
use std::sync::Arc;

#[derive(Clone)]
pub struct DefinitionValidation {
	credentials: Arc<dyn Credentials>,
	core_tools: Arc<dyn CoreToolCatalog>,
}
impl DefinitionValidation {
	pub fn new(credentials: Arc<dyn Credentials>, core_tools: Arc<dyn CoreToolCatalog>) -> Self {
		Self {
			credentials,
			core_tools,
		}
	}
	pub fn node_specifications(
		&self,
	) -> std::collections::BTreeMap<String, aidash_domain::provider::ToolSpec> {
		let mut specifications = crate::tools::builtins()
			.into_iter()
			.map(|(name, tool)| (name, tool.specification()))
			.collect::<std::collections::BTreeMap<_, _>>();
		specifications.extend(self.core_tools.specifications(
			&aidash_domain::capabilities::CoreCapabilities {
				files: true,
				shell: true,
				python: true,
				patch: true,
				skills: true,
				sharing: true,
			},
		));
		specifications
	}
	pub fn validate_in(&self, e: &Entry, local: bool) -> Result<()> {
		aidash_domain::registry::rules::validate_metadata(e, local)?;
		match e.kind.as_str() {
			"embedding" => self.validate_embedding(
				&serde_json::from_value(e.config.clone())
					.map_err(|e| Error::Invalid(e.to_string()))?,
				local,
			)?,
			"compactor" => self.validate_compactor(
				&serde_json::from_value(e.config.clone())
					.map_err(|e| Error::Invalid(e.to_string()))?,
				local,
			)?,
			"model" => {
				let m: ModelConfig = serde_json::from_value(e.config.clone())
					.map_err(|e| Error::Invalid(e.to_string()))?;
				if m.provider != "openrouter"
					|| m.model_id.trim().is_empty()
					|| m.context_window < 2048
					|| (local && m.max_output_tokens.is_none())
					|| m.max_output_tokens
						.is_some_and(|tokens| tokens == 0 || tokens as usize > m.context_window)
					|| !m.modalities.iter().any(|m| m == "text")
				{
					return Err(Error::Invalid(
					"model requires openrouter, model_id, text modality, context_window >= 2048 and a valid max_output_tokens value"
						.into(),
				));
				}
				m.request_timeout()?;
				for route in &m.media_routes {
					if route.tag.is_empty()
						|| !route
							.tag
							.bytes()
							.all(|byte| byte.is_ascii_alphanumeric() || b"._-/".contains(&byte))
						|| route.formats.is_empty()
						|| route.source.trim().is_empty()
						|| route.verified_at >= route.expires_at
						|| route.formats.iter().any(|format| {
							!matches!(
								format.as_str(),
								"image/png"
									| "image/jpeg" | "image/gif"
									| "image/webp" | "wav" | "mp3"
									| "m4a" | "aac" | "ogg" | "webm"
									| "flac"
							)
						}) {
						return Err(Error::Invalid("invalid media route evidence".into()));
					}
				}
				validate_endpoint(&m.endpoint)?;
				if let Some(name) = m.credential_env {
					validate_secret_reference(&name)?;
					if local {
						self.credentials.resolve(&name)?;
					}
				}
			}
			"agent" => {
				let a: AgentConfig = serde_json::from_value(e.config.clone())
					.map_err(|e| Error::Invalid(e.to_string()))?;
				aidash_domain::capabilities::skills::validate_config(&a)?;
				aidash_domain::capabilities::references::validate_config(&a)?;
				if (a.instructions.trim().is_empty()
					&& a.skills.is_empty()
					&& a.skill_attachments.is_empty()
					&& a.skill_roots.is_empty())
					|| !(1..=1000).contains(&a.max_steps)
				{
					return Err(Error::Invalid(
						"agent requires skills or additional instructions and max_steps in 1..1000"
							.into(),
					));
				}
			}
			"cluster" => {
				let cluster: ClusterConfig =
					serde_json::from_value(e.config.clone()).map_err(|error| {
						Error::Invalid(format!("cluster requires a coordinator reference: {error}"))
					})?;
				if cluster.coordinator.id.trim().is_empty()
					|| semver::Version::parse(&cluster.coordinator.version).is_err()
				{
					return Err(Error::Invalid(
						"cluster coordinator requires an id and semantic version".into(),
					));
				}
			}
			"tool" if aidash_domain::tool::legacy_config(&e.config)?.is_some() => {
				self.validate_tool(&e.config, local)?;
			}
			"tool" => {
				let descriptor: aidash_domain::tool::providers::ToolDescriptor =
					serde_json::from_value(e.config.clone())?;
				self.contract(
					&descriptor,
					&aidash_domain::registry::bindings::QualifiedRef {
						registry_node: descriptor.registry_node.clone(),
						id: e.id.clone(),
						version: e.version.clone(),
					},
				)?;
				if let Some(transport) = &descriptor.transport {
					self.validate_tool(&serde_json::to_value(transport)?, local)?;
				}
				if local {
					self.core_tools.provider_available(&descriptor)?;
				}
			}
			"bundle" => {
				let bundle: aidash_domain::registry::bindings::BundleConfig =
					serde_json::from_value(e.config.clone())?;
				bundle.validate()?;
			}
			"skill" => aidash_domain::registry::rules::validate_skill(e)?,
			"memory" | "source" => {
				let descriptor: aidash_domain::registry::bindings::sources::NativeContext =
					serde_json::from_value(e.config.clone())?;
				descriptor.validate(&e.kind)?;
			}
			_ => {}
		}
		Ok(())
	}

	pub fn validate_compactor(&self, config: &CompactorConfig, local: bool) -> Result<()> {
		validate_endpoint(&config.endpoint)?;
		if config.provider != "typesafe-system-one"
			|| config.model.trim().is_empty()
			|| config.model.len() > 128
			|| !(1024..=1_048_576).contains(&config.max_request_bytes)
			|| !(1..=1024).contains(&config.max_questions)
			|| !(128..=1_048_576).contains(&config.max_response_bytes)
		{
			return Err(Error::Invalid(
				"invalid compactor transport or request/response bounds".into(),
			));
		}
		validate_secret_reference(&config.credential_env)?;
		if local
			&& self
				.credentials
				.resolve(&config.credential_env)?
				.trim()
				.is_empty()
		{
			return Err(Error::Invalid(
				"compactor credential must not be empty".into(),
			));
		}
		Ok(())
	}

	pub fn validate_embedding(
		&self,
		config: &aidash_domain::semantic::EmbeddingConfig,
		local: bool,
	) -> Result<()> {
		validate_endpoint(&config.endpoint)?;
		if let Some(name) = &config.credential_env {
			validate_secret_reference(name)?;
			if local {
				self.credentials.resolve(name)?;
			}
		}
		config.validate_parameters()?;
		Ok(())
	}

	pub fn validate_tool(&self, value: &Value, local: bool) -> Result<()> {
		let cfg: ToolConfig =
			serde_json::from_value(value.clone()).map_err(|e| Error::Invalid(e.to_string()))?;
		match &cfg {
			ToolConfig::Native {
				operation,
				allowed_hosts,
			} => {
				if !matches!(operation.as_str(), "echo" | "http_get")
					|| (operation == "http_get" && allowed_hosts.is_empty())
				{
					return Err(Error::Invalid(
						"native tools support echo or http_get with allowed_hosts".into(),
					));
				}
			}
			ToolConfig::Http {
				endpoint,
				credential_env,
				replay,
			}
			| ToolConfig::Mcp {
				endpoint,
				credential_env,
				replay,
				..
			} => {
				validate_endpoint(endpoint)?;
				if let Some(name) = credential_env {
					validate_secret_reference(name)?;
					if local {
						self.credentials.resolve(name)?;
					}
				}
				if !matches!(replay.as_str(), "read_only" | "idempotent" | "unsafe") {
					return Err(Error::Invalid(
						"replay must be read_only, idempotent or unsafe".into(),
					));
				}
				if let ToolConfig::Mcp {
					replay,
					idempotency_argument,
					..
				} = &cfg && replay == "idempotent"
					&& idempotency_argument
						.as_ref()
						.is_none_or(|s| s.trim().is_empty())
				{
					return Err(Error::Invalid(
					"idempotent MCP tools require an idempotency_argument supported by the server"
						.into(),
				));
				}
			}
			ToolConfig::Agent { node_id, agent } => {
				validate_node_id(node_id)?;
				if agent.id.is_empty()
					|| agent.id.len() > 100
					|| !agent
						.id
						.as_bytes()
						.first()
						.is_some_and(u8::is_ascii_alphanumeric)
					|| !agent
						.id
						.bytes()
						.all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
					|| semver::Version::parse(&agent.version).is_err()
				{
					return Err(Error::Invalid(
						"agent tool requires a valid executor ID and semantic version".into(),
					));
				}
			}
		}
		Ok(())
	}

	pub fn agent_prompt_headroom(
		&self,
		config: &AgentConfig,
		references: &[Entry],
		private_context: &Value,
	) -> Result<usize> {
		let get = |reference: &EntityRef| {
			references
				.iter()
				.find(|e| e.id == reference.id && e.version == reference.version)
				.ok_or_else(|| Error::NotFound(reference.id.clone()))
		};
		let model: ModelConfig = serde_json::from_value(get(&config.model)?.config.clone())?;
		let mut instructions = aidash_domain::context::agent_instructions("");
		for skill in &config.skills {
			instructions.push('\n');
			instructions.push_str(&format!("Skill {}@{}:\n", skill.id, skill.version));
			instructions.push_str(&skill_instructions(get(skill)?)?);
		}
		instructions.push_str("\nAdditional user instructions:\n");
		instructions.push_str(&config.instructions);
		let mut builtins = crate::tools::builtins()
			.into_iter()
			.map(|(name, tool)| (name, tool.specification()))
			.collect::<std::collections::BTreeMap<_, _>>();
		builtins.extend(self.core_tools.specifications(&config.core_capabilities));
		let mut specifications = builtins
			.into_iter()
			.filter(|(name, _)| config.permits_builtin(name))
			.map(|(_, tool)| tool)
			.collect::<Vec<_>>();
		for (index, tool) in config.tools.iter().enumerate() {
			let entry = get(tool)?;
			if config.allow_task_delegation == Some(false)
				&& matches!(
					serde_json::from_value::<ToolConfig>(entry.config.clone())?,
					ToolConfig::Agent { .. }
				) {
				continue;
			}
			specifications.push(crate::tools::plugin_specification(
				entry,
				&format!("plugin_{index}"),
			));
		}
		aidash_domain::context::request_context_budget(
			model.context_window,
			model.output_token_limit(),
			&instructions,
			&specifications,
			private_context,
		)
		.map_err(Into::into)
	}
}

impl crate::ports::bindings::ProviderCatalog for DefinitionValidation {
	fn contract(
		&self,
		descriptor: &aidash_domain::tool::providers::ToolDescriptor,
		identity: &aidash_domain::registry::bindings::QualifiedRef,
	) -> Result<aidash_domain::tool::ToolContract> {
		Ok(descriptor.declared_contract(identity.clone())?)
	}
	fn implementation(
		&self,
		descriptor: &aidash_domain::tool::providers::ToolDescriptor,
	) -> Result<String> {
		self.core_tools.provider_available(descriptor)?;
		Ok(format!(
			"{}:aidash-{}",
			descriptor.provider,
			env!("CARGO_PKG_VERSION")
		))
	}
}
