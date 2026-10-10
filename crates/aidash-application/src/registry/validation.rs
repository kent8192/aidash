//! Definition admission resolves local credentials only through the injected port.
use crate::{
	Error, Result,
	ports::{Credentials, bindings::ProviderCatalog, registry::CoreToolCatalog},
};
use aidash_domain::exposure::{
	self, CapabilityIdentity, CapabilityKind, DeferredBudgets, ExposureState,
};
use aidash_domain::registry::rules::skill_instructions;
use aidash_domain::{
	configuration::{validate_endpoint, validate_node_id, validate_secret_reference},
	model::ModelConfig,
	registry::{AgentConfig, ClusterConfig, CompactorConfig, Entry},
	tool::ToolConfig,
};
use serde_json::Value;
use std::sync::Arc;

#[derive(Clone)]
pub struct DefinitionValidation {
	credentials: Arc<dyn Credentials>,
	provider_credentials_enabled: bool,
	core_tools: Arc<dyn CoreToolCatalog>,
}
impl DefinitionValidation {
	pub fn new(credentials: Arc<dyn Credentials>, core_tools: Arc<dyn CoreToolCatalog>) -> Self {
		Self {
			provider_credentials_enabled: false,
			credentials,
			core_tools,
		}
	}
	pub fn with_provider_credentials(mut self, configured: bool) -> Self {
		self.provider_credentials_enabled = configured;
		self
	}
	fn provider_source(
		&self,
		endpoint: &str,
		provider: &str,
		env: Option<&str>,
		catalog: Option<&str>,
	) -> Result<()> {
		aidash_domain::provider_credentials::validate_source(endpoint, provider, env, catalog)?;
		if catalog.is_some() && !self.provider_credentials_enabled {
			return Err(Error::Invalid(
				"Provider Credential Store is not configured".into(),
			));
		}
		Ok(())
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
				outbound: false,
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
			"decider" => {
				let config: aidash_domain::decision::DeciderConfig =
					serde_json::from_value(e.config.clone())?;
				config.validate()?;
				if local {
					let credential = self
						.credentials
						.resolve(&config.credential_env)
						.map_err(|_| Error::Invalid("Decider credential unavailable".into()))?;
					if credential.trim().is_empty() {
						return Err(Error::Invalid(
							"Decider credential must not be empty".into(),
						));
					}
				}
			}
			"memory" | "source" if e.config.get("schema_version").is_some() => {
				let descriptor: aidash_domain::registry::bindings::sources::NativeContext =
					serde_json::from_value(e.config.clone())?;
				descriptor.validate(&e.kind)?;
			}

			"memory" => {
				serde_json::from_value::<aidash_domain::memory::ProviderConfig>(e.config.clone())?
					.policy
					.validate()?
			}
			"source" => {
				let config: aidash_domain::memory::SourceConfig =
					serde_json::from_value(e.config.clone())?;
				if config.max_tokens == 0
					|| config.max_tokens > i32::MAX as usize
					|| config.memory.id.is_empty()
					|| semver::Version::parse(&config.memory.version).is_err()
				{
					return Err(Error::Invalid(
						"memory source requires a versioned provider and positive context cap"
							.into(),
					));
				}
			}
			"reranker" => {
				let config: aidash_domain::memory::RerankerConfig =
					serde_json::from_value(e.config.clone())?;
				if let aidash_domain::memory::RerankerConfig::Model { model } = config
					&& (model.id.is_empty() || semver::Version::parse(&model.version).is_err())
				{
					return Err(Error::Invalid(
						"reranker requires an exact model version".into(),
					));
				}
			}
			"tokenizer" => {
				serde_json::from_value::<aidash_domain::memory::TokenizerConfig>(e.config.clone())?;
			}
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
				if m.provider_credential.is_some() {
					aidash_domain::provider_credentials::validate_model_id(&m.model_id)?;
				}
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
				self.provider_source(
					&m.endpoint,
					&m.provider,
					m.credential_env.as_deref(),
					m.provider_credential.as_deref(),
				)?;
				if let Some(name) = m.credential_env {
					validate_secret_reference(&name)?;
					if local {
						self.credentials.resolve(&name)?;
					}
				}
			}
			"agent" => {
				let input: aidash_domain::registry::bindings::AgentBindings =
					serde_json::from_value(e.config.clone())?;
				input.validate()?;
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
				return Err(Error::Invalid(
					"Tool registration requires a versioned Provider descriptor".into(),
				));
			}
			"tool" => {
				let descriptor: aidash_domain::tool::providers::ToolDescriptor =
					serde_json::from_value(e.config.clone()).map_err(|error| {
						Error::Invalid(format!("invalid Tool descriptor: {error}"))
					})?;
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
		if config.provider_credential.is_some() {
			aidash_domain::provider_credentials::validate_model_id(&config.model)?;
		}
		self.provider_source(
			&config.endpoint,
			&config.provider,
			config.credential_env.as_deref(),
			config.provider_credential.as_deref(),
		)?;
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

	pub fn bound_prompt_headroom(
		&self,
		snapshot: &aidash_domain::registry::bindings::BindingSnapshot,
		private_context: &Value,
	) -> Result<usize> {
		snapshot.validate()?;
		let config = AgentConfig::from_snapshot(snapshot)?;
		let model = snapshot
			.definitions
			.iter()
			.find(|d| {
				d.identity.registry_node == snapshot.agent.registry_node
					&& d.identity.local() == config.model
			})
			.ok_or_else(|| Error::NotFound(config.model.id.clone()))?;
		let model: ModelConfig = serde_json::from_value(model.definition.config.clone())?;
		if let Some(budgets) = config.exposure_policy().budgets() {
			return self.deferred_headroom(snapshot, &config, &model, budgets, private_context);
		}
		let mut instructions = aidash_domain::context::agent_instructions("");
		let mut specifications = vec![];
		for binding in &snapshot.bindings {
			if binding.excluded_reason.is_some() {
				continue;
			}
			if binding.definition.kind == "skill" {
				instructions.push_str(&format!(
					"\nSkill {}@{}:\n{}",
					binding.identity.id,
					binding.identity.version,
					skill_instructions(&binding.definition)?
				));
			} else if binding.definition.kind == "tool" {
				let alias = binding
					.alias
					.as_deref()
					.ok_or_else(|| Error::Invalid("Tool has no alias".into()))?;
				let spec = crate::tools::plugin_specification(&binding.definition, alias);
				specifications.push(spec);
			}
		}
		instructions.push_str("\nAdditional user instructions:\n");
		instructions.push_str(&config.instructions);
		aidash_domain::context::request_context_budget(
			model.context_window,
			model.output_token_limit(),
			&instructions,
			&specifications,
			private_context,
		)
		.map_err(Into::into)
	}

	/// `deferred@1`: the request carries Mandatory exposure, eager tools and
	/// eager Skill blocks, and may grow to the full exposure budgets. Every
	/// single Discoverable capability must also fit its own budget.
	fn deferred_headroom(
		&self,
		snapshot: &aidash_domain::registry::bindings::BindingSnapshot,
		config: &AgentConfig,
		model: &ModelConfig,
		budgets: &DeferredBudgets,
		private_context: &Value,
	) -> Result<usize> {
		let node = self.node_specifications();
		let specifications = crate::registry::bindings::execution::bound_specifications(
			snapshot,
			|binding, descriptor| {
				Ok(
					match (&descriptor.transport, node.get(&descriptor.operation)) {
						(None, Some(specification)) => specification.clone(),
						_ => crate::tools::plugin_specification(
							&binding.definition,
							binding.alias.as_deref().unwrap_or_default(),
						),
					},
				)
			},
		)?;
		let attachments = config
			.skill_attachments
			.iter()
			.map(crate::capabilities::skills::attachment_skill)
			.collect::<Result<Vec<_>>>()?;
		let catalog = exposure::catalog(snapshot, &specifications, &attachments)?;
		for capability in &catalog {
			let (name, budget) = match capability.kind {
				CapabilityKind::Tool => ("schema_bytes", budgets.schema_bytes),
				CapabilityKind::Skill => ("skill_bytes", budgets.skill_bytes),
			};
			if capability.definition_bytes > budget {
				return Err(Error::Invalid(format!(
					"{} {} needs {} bytes, over the deferred@1 {name} budget of {budget} bytes",
					capability.kind.as_str(),
					capability.alias,
					capability.definition_bytes,
				)));
			}
		}
		let selection = exposure::select(budgets, &catalog, &ExposureState::default())?;
		let mut instructions = aidash_domain::context::agent_instructions("");
		for alias in &selection.skills {
			let capability = catalog
				.iter()
				.find(|capability| &capability.alias == alias)
				.ok_or_else(|| Error::Invalid("selected Skill is not Discoverable".into()))?;
			// Only Registry Skills can be eager.
			let CapabilityIdentity::Registry(reference) = &capability.identity else {
				continue;
			};
			let binding = snapshot
				.bindings
				.iter()
				.find(|binding| &binding.identity == reference)
				.ok_or_else(|| Error::Invalid("selected Skill is not bound".into()))?;
			instructions.push_str(&exposure::resident_block(
				capability,
				&skill_instructions(&binding.definition)?,
			));
		}
		instructions.push_str("\nAdditional user instructions:\n");
		instructions.push_str(&config.instructions);
		let exposed = specifications
			.into_iter()
			.filter(|(alias, _)| selection.tools.contains(alias))
			.map(|(_, specification)| specification)
			.collect::<Vec<_>>();
		let reserve = budgets.metadata_bytes
			+ budgets
				.schema_bytes
				.saturating_sub(selection.usage.schema_bytes)
			+ budgets
				.skill_bytes
				.saturating_sub(selection.usage.skill_bytes);
		aidash_domain::context::request_context_budget(
			model.context_window,
			model.output_token_limit(),
			&instructions,
			&exposed,
			private_context,
		)?
		.checked_sub(reserve)
		.filter(|remaining| *remaining >= aidash_domain::context::MIN_CONTEXT_RESERVE)
		.ok_or_else(|| {
			Error::Invalid(
				"deferred@1 exposure budgets cannot fit the model window with output and context reserves"
					.into(),
			)
		})
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

#[cfg(test)]
mod tests;
