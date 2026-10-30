//! Immutable capability references, supported restrictions and Run resolution records.
use super::{EntityRef, Entry, Projection};
use crate::{
	Error, Result,
	exposure::{BindingExposure, ExposurePolicy},
	nodes::DeploymentId,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub const BINDING_SCHEMA: u8 = 1;
pub mod defaults;
pub use defaults::{COMPACTION_DECIDER_ID, COMPACTION_DECIDER_VERSION, COMPACTION_DEFAULT};
mod snapshot;
pub mod source_defaults;
pub mod sources;
pub const MAX_BINDINGS: usize = 128;
/// Upper bound for an Agent definition's Run Tool Parallelism ceiling.
pub const MAX_TOOL_PARALLELISM: u8 = 16;
fn sequential_tools() -> u8 {
	1
}
fn is_sequential_tools(value: &u8) -> bool {
	*value == 1
}
pub const REQUIRED_TOOLS: &[&str] = &["workspace_read", "human_request"];
pub const DEFAULT_TOOLS: &[&str] = &[
	"workspace_observe",
	"workspace_wait",
	"skill_list",
	"skill_load",
	"skill_read",
	"file_search",
	"file_read",
	"task_create",
	"task_delegate",
	"agent_discover",
	"artifact_publish",
	"workspace_message",
	"attachment_copy",
	"memory_mutate",
	"memory_recall",
	"memory_reflect",
];
/// Default builtins that act on the Agent's own memory. Without a `memory`
/// Binding every call would be refused, so a Run excludes them instead of
/// offering them (#600).
pub const MEMORY_TOOLS: &[&str] = &["memory_mutate", "memory_recall", "memory_reflect"];
pub const SKILL_TOOLS: &[&str] = &["skill_list", "skill_load", "skill_read"];
pub const COORDINATOR_TOOLS: &[&str] = &["task_create", "task_delegate", "agent_discover"];
/// Required under `deferred@1` only, with canonical aliases.
pub const EXPOSURE_TOOLS: &[&str] = &[
	"capability_search",
	"capability_describe",
	"capability_load",
	"capability_unload",
];
/// Replaces `SKILL_TOOLS` in the `deferred@1` default set.
pub const SKILL_ASSET_READ: &str = "skill_asset_read";

/// Typed exact edges used by admission and durable snapshot validation. Foreign
/// qualifiers are retained, including delegation targets; no local ID guessing.
pub fn definition_references(
	identity: &QualifiedRef,
	entry: &Entry,
) -> Result<Vec<(QualifiedRef, String)>> {
	let mut result = vec![];
	let local = |reference: &EntityRef| QualifiedRef {
		registry_deployment: identity.registry_deployment.clone(),
		id: reference.id.clone(),
		version: reference.version.clone(),
	};
	match entry.kind.as_str() {
		"agent" => {
			let config: AgentBindings = serde_json::from_value(entry.config.clone())?;
			config.validate()?;
			result.push((local(&config.model), "model".into()));
			if let Some(summary) = config.context_summary() {
				result.push((local(&summary.model), "model".into()));
			}
			result.extend(config.cluster.iter().map(|r| (local(r), "cluster".into())));
			for normalized in entry.normalized_bindings(&identity.registry_deployment)? {
				let kind = match normalized.binding.kind {
					BindingKind::Tool => "tool",
					BindingKind::Bundle => "bundle",
					BindingKind::Skill => "skill",
					BindingKind::Memory => "memory",
					BindingKind::Source => "source",
					BindingKind::Decider => "decider",
					BindingKind::ExternalService => "external_service",
				};
				result.push((normalized.binding.target, kind.into()));
			}
		}
		"cluster" => {
			let config: super::ClusterConfig = serde_json::from_value(entry.config.clone())?;
			result.push((local(&config.coordinator), "agent".into()));
		}
		"bundle" => {
			let config: BundleConfig = serde_json::from_value(entry.config.clone())?;
			config.validate()?;
			result.extend(
				config
					.members
					.into_iter()
					.map(|r| (r, "tool_or_bundle".into())),
			);
		}
		"tool" => {
			let descriptor: crate::tool::providers::ToolDescriptor =
				serde_json::from_value(entry.config.clone())?;
			if descriptor.registry_deployment != identity.registry_deployment {
				return Err(Error::Invalid(
					"descriptor origin differs from its qualified identity".into(),
				));
			}
			if let Some(lifecycle) = descriptor.lifecycle {
				result.extend(
					[lifecycle.poll, lifecycle.cancel]
						.into_iter()
						.map(|r| (r, "tool".into())),
				);
			}
			if let Some(crate::tool::ToolConfig::Agent { node_id, agent }) = descriptor.transport {
				result.push((
					QualifiedRef {
						registry_deployment: node_id,
						id: agent.id,
						version: agent.version,
					},
					"agent".into(),
				));
			}
		}
		"memory" | "source" if entry.config.get("schema_version").is_some() => {
			if let Some(native) = sources::validate_definition(entry)?
				&& let Some(declaration) = native.context_source
			{
				result.extend(declaration.references());
			}
		}
		"memory" => {
			let provider: crate::memory::ProviderConfig =
				serde_json::from_value(entry.config.clone())?;
			if let Some(declaration) = provider.context_source {
				declaration.validate()?;
				result.extend(declaration.references());
			}
			result.extend(
				[
					(provider.policy.extraction, "model"),
					(provider.policy.derivation, "model"),
					(provider.policy.reflection, "model"),
					(provider.policy.embedding, "embedding"),
					(provider.policy.reranker, "reranker"),
					(provider.policy.tokenizer, "tokenizer"),
				]
				.into_iter()
				.map(|(r, k)| (local(&r), k.into())),
			);
		}
		"source" => {
			let source: crate::memory::SourceConfig = serde_json::from_value(entry.config.clone())?;
			result.push((local(&source.memory), "memory".into()));
			if let Some(declaration) = source.context_source {
				result.extend(declaration.references());
			}
		}
		"reranker" => {
			if let crate::memory::RerankerConfig::Model { model } =
				serde_json::from_value(entry.config.clone())?
			{
				result.push((local(&model), "model".into()));
			}
		}
		_ => {}
	}
	for (reference, _) in &result {
		reference.validate()?;
	}
	Ok(result)
}
pub fn reference_kind_matches(expected: &str, actual: &str) -> bool {
	expected == actual || expected == "tool_or_bundle" && matches!(actual, "tool" | "bundle")
}

/// Writable Agent schema. Normalization is a separate trusted registration result.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentBindings {
	pub schema_version: u8,
	pub model: EntityRef,
	#[serde(default)]
	pub instructions: String,
	#[serde(default)]
	pub bindings: Vec<Binding>,
	#[serde(default)]
	pub remove_default: Vec<String>,
	#[serde(default)]
	pub cluster: Option<EntityRef>,
	#[serde(default = "super::max_steps")]
	pub max_steps: i32,
	/// Opt-in Context Policy; absent keeps the legacy prune-only behavior.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub context_policy: Option<crate::context::policy::ContextPolicy>,
	/// The Run Tool Parallelism ceiling, fixed with the Run's Binding snapshot.
	/// Omitted at one so definitions written before this field keep their digest.
	#[serde(
		default = "sequential_tools",
		skip_serializing_if = "is_sequential_tools"
	)]
	pub tool_parallelism: u8,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub exposure: Option<ExposurePolicy>,
	/// Projection Version this Agent version's Runs render with (ADR 0015).
	/// Omitted means Legacy and is not serialized, so existing definitions,
	/// their digests and Binding snapshots stay byte-identical.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub projection_version: Option<crate::projection::ProjectionVersion>,
	/// Prompt-caching opt-in pinned by every Run of this Agent (ADR 0019).
	/// Omitted means `off` and is not serialized, like `projection_version`.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub prompt_cache: Option<crate::projection::PromptCache>,
	/// Completion Policy every Run of this Agent version pins (#112). Omitted
	/// means no verification and is not serialized, like `prompt_cache`.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub completion: Option<crate::completion::CompletionPolicy>,
	/// The Agent Plugin copies this Agent version takes in (ADR 0072). Omitted when empty, so
	/// definitions without plugins keep their bytes and digests.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub agent_plugins: Vec<crate::agent_plugin::AgentPluginRef>,
	/// What this Agent version decides about the servers of those plugins. Omitted when empty,
	/// so definitions without settings keep their bytes and digests.
	#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
	pub agent_plugin_settings: crate::agent_plugin::PluginSettings,
}
impl AgentBindings {
	/// Whether some Agent Plugin carries a Skill, which makes it a Skill context like a Skill
	/// Source.
	pub fn has_plugin_skills(&self) -> bool {
		self.agent_plugins
			.iter()
			.any(|plugin| !plugin.skills.is_empty())
	}
	/// The resolved Exposure policy; absent means `legacy@1`.
	pub fn exposure_policy(&self) -> ExposurePolicy {
		self.exposure.unwrap_or_default()
	}
	pub fn validate(&self) -> Result<()> {
		let policy = self.exposure_policy();
		policy.validate()?;
		let defaults = policy.default_tools();
		if self.schema_version != BINDING_SCHEMA
			|| !(1..=1000).contains(&self.max_steps)
			|| !(1..=MAX_TOOL_PARALLELISM).contains(&self.tool_parallelism)
			|| self.bindings.len() > MAX_BINDINGS
			|| self.instructions.trim().is_empty()
				&& !self.has_plugin_skills()
				&& !self
					.bindings
					.iter()
					.any(|b| matches!(b.kind, BindingKind::Skill | BindingKind::Source))
			|| self
				.remove_default
				.iter()
				.any(|name| name != COMPACTION_DEFAULT && !defaults.contains(&name.as_str()))
			|| self.remove_default.iter().collect::<BTreeSet<_>>().len()
				!= self.remove_default.len()
		{
			return Err(Error::Invalid(
				"invalid Binding schema, defaults or Agent instructions".into(),
			));
		}
		if let Some(version) = self.projection_version
			&& !version.is_implemented()
		{
			return Err(Error::Invalid(format!(
				"Projection Version {version} is not implemented"
			)));
		}
		let mut targets = BTreeSet::new();
		for binding in &self.bindings {
			binding.validate()?;
			if binding.exposure.is_some()
				&& (!policy.is_deferred()
					|| !matches!(
						binding.kind,
						BindingKind::Tool | BindingKind::Bundle | BindingKind::Skill
					)) {
				return Err(Error::Invalid(
					"Binding exposure requires the deferred@1 policy and a Tool, bundle or Skill"
						.into(),
				));
			}
			if !targets.insert(&binding.target) {
				return Err(Error::Invalid("duplicate Binding target".into()));
			}
		}
		if !self.has_memory_binding()
			&& self.bindings.iter().any(|b| {
				b.kind == BindingKind::Tool
					&& b.target
						.id
						.strip_prefix("aidash.")
						.is_some_and(|operation| MEMORY_TOOLS.contains(&operation))
			}) {
			return Err(Error::Invalid("memory tools require a bound memory".into()));
		}
		if self.bindings.iter().any(|b| b.kind == BindingKind::Skill)
			&& self
				.remove_default
				.iter()
				.any(|name| policy.skill_support_tools().contains(&name.as_str()))
		{
			return Err(Error::Invalid(
				"bound Skills require Skill support tools".into(),
			));
		}
		if let Some(policy) = &self.context_policy {
			policy.validate()?;
		}
		if let Some(policy) = &self.completion {
			policy.validate()?;
		}
		self.validate_agent_plugins()?;
		Ok(())
	}
	fn validate_agent_plugins(&self) -> Result<()> {
		use crate::{agent_plugin::MAX_AGENT_PLUGINS, capabilities::skills::MAX_RUN_SKILLS};
		if self.agent_plugins.len() > MAX_AGENT_PLUGINS {
			return Err(Error::Invalid(format!(
				"AGENT_PLUGIN_LIMIT: an Agent takes in at most {MAX_AGENT_PLUGINS} plugins"
			)));
		}
		let mut names = BTreeSet::new();
		let mut digests = BTreeSet::new();
		for plugin in &self.agent_plugins {
			plugin.validate()?;
			if !names.insert(&plugin.name) || !digests.insert(&plugin.digest) {
				return Err(Error::Invalid(
					"AGENT_PLUGIN_DUPLICATE: plugin names and digests are unique".into(),
				));
			}
		}
		if self
			.agent_plugins
			.iter()
			.map(|plugin| plugin.skills.len())
			.sum::<usize>()
			> MAX_RUN_SKILLS
		{
			return Err(Error::Invalid(format!(
				"SKILL_BINDING_LIMIT: an Agent pins at most {MAX_RUN_SKILLS} plugin Skills"
			)));
		}
		crate::agent_plugin::validate_settings(
			&self.agent_plugin_settings,
			&self.agent_plugins,
			self.exposure_policy().is_deferred(),
		)
	}
	fn has_memory_binding(&self) -> bool {
		self.bindings.iter().any(|b| b.kind == BindingKind::Memory)
	}
	pub fn context_summary(&self) -> Option<&crate::context::policy::SummaryPolicy> {
		match self.context_policy.as_ref()? {
			crate::context::policy::ContextPolicy::RecoveryV1 { summary, .. } => summary.as_ref(),
		}
	}
	pub fn normalize(&self, node: &str) -> Result<Vec<NormalizedBinding>> {
		self.normalize_with_compaction(node, None)
	}
	pub fn normalize_with_compaction(
		&self,
		node: &str,
		compaction_override: Option<&QualifiedRef>,
	) -> Result<Vec<NormalizedBinding>> {
		self.normalize_versioned(node, compaction_override, false)
	}
	fn normalize_versioned(
		&self,
		node: &str,
		compaction_override: Option<&QualifiedRef>,
		legacy: bool,
	) -> Result<Vec<NormalizedBinding>> {
		self.validate()?;
		if legacy
			&& self
				.remove_default
				.iter()
				.any(|name| name == "attachment_copy")
		{
			return Err(Error::Invalid("unknown historical default".into()));
		}
		crate::nodes::DeploymentId::parse(node)?;
		let mut result = Vec::new();
		let has_skills = self.bindings.iter().any(|b| b.kind == BindingKind::Skill);
		let policy = self.exposure_policy();
		let required_tools = policy.required_tools();
		let support = policy.skill_support_tools();
		for operation in required_tools.iter().chain(&policy.default_tools()) {
			if legacy && *operation == "attachment_copy" {
				continue;
			}
			let required = required_tools.contains(operation);
			if !required && self.remove_default.iter().any(|name| name == operation) {
				continue;
			}
			let mut target = QualifiedRef::builtin(node, operation);
			if legacy {
				target.version = "1.0.0".into();
			}
			let explicit = self
				.bindings
				.iter()
				.find(|binding| binding.target == target);
			if let Some(explicit) = explicit
				&& explicit.kind != BindingKind::Tool
			{
				return Err(Error::Invalid("builtin must be bound as a Tool".into()));
			}
			result.push(NormalizedBinding {
				binding: explicit.cloned().unwrap_or_else(|| Binding::tool(target)),
				origin: if required {
					BindingOrigin::Required
				} else if has_skills && support.contains(operation) {
					BindingOrigin::SkillSupport
				} else if explicit.is_some() {
					BindingOrigin::Explicit
				} else {
					BindingOrigin::Default
				},
			});
		}
		for binding in &self.bindings {
			if !result
				.iter()
				.any(|bound| bound.binding.target == binding.target)
			{
				result.push(NormalizedBinding {
					binding: binding.clone(),
					origin: BindingOrigin::Explicit,
				});
			}
		}
		source_defaults::add_workspace_observation(node, &mut result)?;
		defaults::add_compaction(self, node, compaction_override, &mut result)?;
		if result.len() > MAX_BINDINGS {
			return Err(Error::Invalid(
				"expanded Bindings exceed 128 entries".into(),
			));
		}
		Ok(result)
	}
}

/// Read-only registration metadata is compiled by the Node before publication.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentNormalization {
	pub schema_version: u8,
	pub registry_deployment: String,
	/// Exact explicit compaction replacement, resolved by trusted registration.
	pub compaction_override: Option<QualifiedRef>,
	pub bindings: Vec<NormalizedBinding>,
}
impl Entry {
	pub fn normalize_agent(&mut self, node: &str) -> Result<()> {
		let replacement = self
			.binding_normalization
			.as_ref()
			.and_then(|saved| saved.compaction_override.clone());
		self.normalize_agent_with_compaction(node, replacement.as_ref())
	}
	pub fn normalize_agent_with_compaction(
		&mut self,
		node: &str,
		replacement: Option<&QualifiedRef>,
	) -> Result<()> {
		if self.kind != "agent" {
			if self.binding_normalization.is_some() {
				return Err(Error::Invalid(
					"normalization is only valid for Agents".into(),
				));
			}
			return Ok(());
		}
		let config: AgentBindings = serde_json::from_value(self.config.clone())?;
		self.binding_normalization = Some(AgentNormalization {
			schema_version: BINDING_SCHEMA,
			registry_deployment: node.into(),
			compaction_override: replacement.cloned(),
			bindings: config.normalize_with_compaction(node, replacement)?,
		});
		Ok(())
	}
	pub fn normalized_bindings(&self, node: &str) -> Result<Vec<NormalizedBinding>> {
		let config: AgentBindings = serde_json::from_value(self.config.clone())?;
		let replacement = self
			.binding_normalization
			.as_ref()
			.and_then(|saved| saved.compaction_override.as_ref());
		if let Some(saved) = &self.binding_normalization {
			if saved.schema_version != BINDING_SCHEMA || saved.registry_deployment != node {
				return Err(Error::Invalid(
					"Agent normalization differs from its immutable definition".into(),
				));
			}
			let current = config.normalize_with_compaction(node, replacement);
			let legacy = config.normalize_versioned(node, replacement, true);
			if !current.is_ok_and(|expected| saved.bindings == expected)
				&& !legacy.is_ok_and(|expected| saved.bindings == expected)
			{
				return Err(Error::Invalid(
					"Agent normalization differs from its immutable definition".into(),
				));
			}
			return Ok(saved.bindings.clone());
		}
		config.normalize_with_compaction(node, replacement)
	}

	/// Whether this Agent version pins the builtin Tool that can select Thread Attachments.
	/// This describes its Tools, not its model modalities, routes, or Run authority.
	pub fn supports_thread_attachment_selection(&self, node: &str) -> Result<bool> {
		if self.kind != "agent" {
			return Err(Error::Invalid("expected Agent definition".into()));
		}
		Ok(self.normalized_bindings(node)?.iter().any(|item| {
			item.binding.kind == BindingKind::Tool
				&& attachment_selection_tool(&item.binding.target, &item.binding.narrow, node)
		}))
	}
}

fn attachment_selection_tool(identity: &QualifiedRef, narrow: &Narrowing, node: &str) -> bool {
	identity.registry_deployment == node
		&& identity.id == "aidash.workspace_read"
		&& identity.version == "1.0.1"
		&& [("kind", "attachment"), ("representation", "model_input")]
			.iter()
			.all(|(key, value)| {
				narrow
					.scope
					.get(*key)
					.is_none_or(|values| values.contains(*value))
			})
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QualifiedRef {
	pub registry_deployment: String,
	pub id: String,
	pub version: String,
}
impl QualifiedRef {
	pub fn validate(&self) -> Result<()> {
		DeploymentId::parse(&self.registry_deployment)?;
		if self.id.is_empty()
			|| self.id.len() > 100
			|| !self
				.id
				.bytes()
				.all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
			|| !self
				.id
				.as_bytes()
				.first()
				.is_some_and(u8::is_ascii_alphanumeric)
			|| semver::Version::parse(&self.version).is_err()
		{
			return Err(Error::Invalid(
				"invalid qualified capability reference".into(),
			));
		}
		Ok(())
	}
	pub fn local(&self) -> EntityRef {
		EntityRef {
			id: self.id.clone(),
			version: self.version.clone(),
		}
	}
	pub fn builtin(node: &str, operation: &str) -> Self {
		Self {
			registry_deployment: node.into(),
			id: format!("aidash.{operation}"),
			version: if matches!(
				operation,
				"workspace_read" | "workspace_observe" | "workspace_message"
			) {
				"1.0.1"
			} else {
				"1.0.0"
			}
			.into(),
		}
	}
	pub fn resource_id(&self) -> String {
		format!(
			"{}/tools/{}@{}",
			self.registry_deployment, self.id, self.version
		)
	}
}

/// No effect, replay, approval or disclosure field is accepted here. Concurrency
/// safety may only be lowered; a claim above the provider's is rejected.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Narrowing {
	#[serde(skip_serializing_if = "Option::is_none")]
	pub decision: Option<crate::decision::Restrictions>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub allowed_hosts: Option<BTreeSet<String>>,
	#[serde(skip_serializing_if = "BTreeMap::is_empty")]
	pub scope: BTreeMap<String, BTreeSet<String>>,
	#[serde(skip_serializing_if = "BTreeMap::is_empty")]
	pub limits: BTreeMap<String, u64>,
	#[serde(skip_serializing_if = "Option::is_none")]
	pub concurrency: Option<crate::tool::Concurrency>,
}
impl Narrowing {
	pub fn intersect(&self, other: &Self) -> Result<Self> {
		let allowed_hosts = match (&self.allowed_hosts, &other.allowed_hosts) {
			(Some(a), Some(b)) => Some(a.intersection(b).cloned().collect()),
			(Some(a), None) | (None, Some(a)) => Some(a.clone()),
			(None, None) => None,
		};
		let mut scope = self.scope.clone();
		for (field, values) in &other.scope {
			scope
				.entry(field.clone())
				.and_modify(|current| *current = current.intersection(values).cloned().collect())
				.or_insert_with(|| values.clone());
		}
		let mut limits = self.limits.clone();
		for (field, ceiling) in &other.limits {
			limits
				.entry(field.clone())
				.and_modify(|value| *value = (*value).min(*ceiling))
				.or_insert(*ceiling);
		}
		let result = Self {
			decision: match (&self.decision, &other.decision) {
				(Some(a), Some(b)) => Some(crate::decision::Restrictions {
					threshold: if a.threshold.value() <= b.threshold.value() {
						a.threshold
					} else {
						b.threshold
					},
					preserve_recent: a.preserve_recent.max(b.preserve_recent),
					forbid_apply: a.forbid_apply || b.forbid_apply,
					max_calls_per_run: match (a.max_calls_per_run, b.max_calls_per_run) {
						(Some(a), Some(b)) => Some(a.min(b)),
						(a, b) => a.or(b),
					},
				}),
				(Some(a), None) | (None, Some(a)) => Some(a.clone()),
				(None, None) => None,
			},
			allowed_hosts,
			scope,
			limits,
			concurrency: match (self.concurrency, other.concurrency) {
				(Some(a), Some(b)) => Some(a.min(b)),
				(a, b) => a.or(b),
			},
		};
		if result
			.allowed_hosts
			.as_ref()
			.is_some_and(BTreeSet::is_empty)
			|| result.scope.values().any(BTreeSet::is_empty)
			|| result.limits.values().any(|limit| *limit == 0)
		{
			return Err(Error::Invalid(
				"capability restrictions have an empty intersection".into(),
			));
		}
		Ok(result)
	}
	pub fn apply(&self, input: &mut Value) -> Result<()> {
		if !input.is_object() {
			return Err(Error::Invalid("tool input must be an object".into()));
		}
		if let Some(hosts) = &self.allowed_hosts {
			let url = input
				.get("url")
				.and_then(Value::as_str)
				.and_then(|url| url::Url::parse(url).ok())
				.ok_or_else(|| Error::Invalid("restricted tool requires a URL".into()))?;
			if !url.host_str().is_some_and(|host| hosts.contains(host)) {
				return Err(Error::Invalid(
					"tool URL exceeds Binding host restrictions".into(),
				));
			}
		}
		for (field, permitted) in &self.scope {
			let value = if field.starts_with('/') {
				input.pointer(field)
			} else {
				input.get(field)
			};
			if !value
				.and_then(Value::as_str)
				.is_some_and(|value| permitted.contains(value))
			{
				return Err(Error::Invalid(format!(
					"tool argument {field} exceeds Binding scope"
				)));
			}
		}
		let object = input.as_object_mut().expect("validated object");
		for (field, ceiling) in &self.limits {
			match object.get(field) {
				Some(value) if value.as_u64().is_some_and(|value| value <= *ceiling) => {}
				None => {
					object.insert(field.clone(), Value::from(*ceiling));
				}
				_ => {
					return Err(Error::Invalid(format!(
						"tool argument {field} exceeds Binding limit"
					)));
				}
			}
		}
		Ok(())
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BindingKind {
	Tool,
	Bundle,
	Skill,
	Memory,
	Source,
	Decider,
	ExternalService,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Binding {
	pub kind: BindingKind,
	pub target: QualifiedRef,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub alias: Option<String>,
	#[serde(default)]
	pub narrow: Narrowing,
	/// An empty member list binds the entire bundle. IDs are unique within a bundle.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub members: Vec<String>,
	/// Only valid for Tools, bundles and Skills under the `deferred@1` policy.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub exposure: Option<BindingExposure>,
}
impl Binding {
	pub fn tool(target: QualifiedRef) -> Self {
		Self {
			kind: BindingKind::Tool,
			target,
			alias: None,
			narrow: Narrowing::default(),
			members: vec![],
			exposure: None,
		}
	}
	pub fn validate(&self) -> Result<()> {
		self.target.validate()?;
		if self.kind == BindingKind::Decider {
			if self.narrow.allowed_hosts.is_some()
				|| !self.narrow.scope.is_empty()
				|| !self.narrow.limits.is_empty()
				|| self
					.narrow
					.decision
					.as_ref()
					.is_some_and(|r| r.threshold.value() > 0.5 || r.preserve_recent < 6)
			{
				return Err(Error::Invalid(
					"invalid Decider Binding restrictions".into(),
				));
			}
		} else if self.narrow.decision.is_some() {
			return Err(Error::Invalid(
				"decision restrictions require a Decider Binding".into(),
			));
		}
		// Only Tool and Bundle Bindings bind Tools whose concurrency can be lowered.
		if self.narrow.concurrency.is_some()
			&& !matches!(self.kind, BindingKind::Tool | BindingKind::Bundle)
		{
			return Err(Error::Invalid(
				"concurrency restrictions require a Tool or Bundle Binding".into(),
			));
		}
		if let Some(alias) = &self.alias {
			validate_alias(alias)?;
		}
		if self.kind != BindingKind::Tool && self.alias.is_some()
			|| self.kind != BindingKind::Bundle && !self.members.is_empty()
			|| self.members.len() > MAX_BINDINGS
			|| self.members.iter().collect::<BTreeSet<_>>().len() != self.members.len()
		{
			return Err(Error::Invalid(
				"invalid Binding alias or member selection".into(),
			));
		}
		self.narrow.intersect(&Narrowing::default())?;
		Ok(())
	}
}
pub fn validate_alias(alias: &str) -> Result<()> {
	if alias.is_empty()
		|| alias.len() > 64
		|| !alias
			.bytes()
			.all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
	{
		return Err(Error::Invalid(
			"tool alias must contain 1..64 ASCII letters, digits, '_' or '-'".into(),
		));
	}
	Ok(())
}

/// Only the registration resolver creates provenance; it is absent from writable Bindings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BindingOrigin {
	Required,
	Default,
	Explicit,
	SkillSupport,
	Companion,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NormalizedBinding {
	pub binding: Binding,
	pub origin: BindingOrigin,
}
impl NormalizedBinding {
	/// Tools whose own Binding sets an `exposure`. Such a Binding is the documented
	/// per-member override of a bundle, so bundle expansion skips that member instead
	/// of producing a second, conflicting Binding for the same exact capability.
	pub fn member_overrides(bindings: &[Self]) -> BTreeSet<QualifiedRef> {
		bindings
			.iter()
			.filter(|n| n.binding.kind == BindingKind::Tool && n.binding.exposure.is_some())
			.map(|n| n.binding.target.clone())
			.collect()
	}
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BundleConfig {
	pub members: Vec<QualifiedRef>,
}
impl BundleConfig {
	pub fn validate(&self) -> Result<()> {
		if self.members.is_empty()
			|| self.members.len() > MAX_BINDINGS
			|| self
				.members
				.iter()
				.map(|member| &member.id)
				.collect::<BTreeSet<_>>()
				.len() != self.members.len()
		{
			return Err(Error::Invalid(
				"bundle requires 1..128 members with unique IDs".into(),
			));
		}
		for member in &self.members {
			member.validate()?;
		}
		Ok(())
	}
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResolvedBinding {
	pub identity: QualifiedRef,
	pub definition: Entry,
	pub digest: String,
	pub origin: BindingOrigin,
	pub alias: Option<String>,
	pub narrow: Narrowing,
	pub installation: Option<Projection>,
	pub provider_contract_digest: Option<String>,
	pub provider_implementation: Option<String>,
	pub excluded_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BindingSnapshot {
	pub schema_version: u8,
	pub agent: QualifiedRef,
	/// Admission placement determines which implicit defaults must remain excluded.
	pub remote: bool,
	pub bindings: Vec<ResolvedBinding>,
	pub definitions: Vec<ResolvedDefinition>,
	/// Public Deployment Administrator Agent transports retain the receiver's complete admitted closure.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub foreign_agents: Vec<ForeignAgentSnapshot>,
	/// The Skills of each Agent Plugin copy the Agent definition takes in, in its order.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub agent_plugins: Vec<crate::agent_plugin::PinnedAgentPlugin>,
}

impl BindingSnapshot {
	/// Whether the admitted, pinned Tools can select a Thread Attachment for model input.
	pub fn supports_thread_attachment_selection(&self) -> bool {
		self.bindings.iter().any(|binding| {
			binding.excluded_reason.is_none()
				&& binding.definition.kind == "tool"
				&& attachment_selection_tool(
					&binding.identity,
					&binding.narrow,
					&self.agent.registry_deployment,
				)
		})
	}
	/// Whether the Agent's pinned model can be given a page image now (ADR
	/// 0053): it declares image input and holds a current media route for the
	/// PNG a viewed page is delivered as.
	pub fn model_reads_page_images(&self) -> bool {
		let Some(model) = self
			.definitions
			.iter()
			.find(|definition| definition.identity == self.agent)
			.map(|agent| &agent.definition.config["model"])
		else {
			return false;
		};
		let (Some(id), Some(version)) = (model["id"].as_str(), model["version"].as_str()) else {
			return false;
		};
		self.definitions
			.iter()
			.find(|definition| {
				definition.definition.kind == "model"
					&& definition.identity.id == id
					&& definition.identity.version == version
			})
			.and_then(|model| {
				serde_json::from_value::<crate::model::ModelConfig>(model.definition.config.clone())
					.ok()
			})
			.is_some_and(|model| {
				model.modalities.iter().any(|modality| modality == "image")
					&& model.has_current_media_route_for_type("image/png")
			})
	}
}

/// One authenticated receiver's public closure. Nested foreign or private
/// dependencies require their own supported disclosure/admission protocol.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ForeignAgentSnapshot {
	pub schema_version: u8,
	pub agent: QualifiedRef,
	pub bindings: Vec<ResolvedBinding>,
	pub definitions: Vec<ResolvedDefinition>,
}

impl ForeignAgentSnapshot {
	pub fn snapshot(&self) -> BindingSnapshot {
		BindingSnapshot {
			schema_version: self.schema_version,
			agent: self.agent.clone(),
			remote: true,
			bindings: self.bindings.clone(),
			definitions: self.definitions.clone(),
			foreign_agents: vec![],
			agent_plugins: vec![],
		}
	}
	pub fn from_snapshot(snapshot: BindingSnapshot) -> Result<Self> {
		if !snapshot.remote || !snapshot.foreign_agents.is_empty() {
			return Err(Error::Invalid(
				"foreign Agent requires a single receiver closure".into(),
			));
		}
		let result = Self {
			schema_version: snapshot.schema_version,
			agent: snapshot.agent,
			bindings: snapshot.bindings,
			definitions: snapshot.definitions,
		};
		result.validate()?;
		Ok(result)
	}
	pub fn validate(&self) -> Result<()> {
		if self.definitions.iter().any(|definition| {
			definition.identity.registry_deployment != self.agent.registry_deployment
				|| definition.definition.installation.is_some()
				|| matches!(
					definition.definition.kind.as_str(),
					"memory" | "source" | "skill"
				) && !source_defaults::workspace_observation()
					.is_ok_and(|source| source == definition.definition)
				|| definition.definition.kind == "agent"
					&& serde_json::from_value::<AgentBindings>(definition.definition.config.clone())
						.is_ok_and(|agent| !agent.agent_plugins.is_empty())
		}) {
			return Err(Error::Invalid(
				"foreign Agent has an unsupported private or cross-Node dependency".into(),
			));
		}
		self.snapshot().validate()
	}
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResolvedDefinition {
	pub identity: QualifiedRef,
	pub definition: Entry,
	pub digest: String,
}
impl ResolvedDefinition {
	pub fn new(identity: QualifiedRef, definition: Entry) -> Result<Self> {
		let digest = super::rules::digest(&serde_json::to_value(&definition)?);
		let result = Self {
			identity,
			definition,
			digest,
		};
		result.validate()?;
		Ok(result)
	}
	pub fn validate(&self) -> Result<()> {
		self.identity.validate()?;
		if self.identity.id != self.definition.id
			|| self.identity.version != self.definition.version
			|| self.digest != super::rules::digest(&serde_json::to_value(&self.definition)?)
		{
			return Err(Error::Invalid(
				"pinned definition identity or digest changed".into(),
			));
		}
		Ok(())
	}
}
impl BindingSnapshot {
	/// Reconstruct an exact Decider from the retained admission closure.
	pub fn decider(&self, hook: crate::decision::Hook) -> Result<crate::decision::BoundDecider> {
		self.bound_decider(hook)?
			.map(|(bound, _)| bound)
			.ok_or_else(|| Error::Invalid("Run has no Decider for this hook".into()))
	}
	/// The Decider bound for `hook` with its pinned definition, or `None` when
	/// the Run has none for it.
	pub fn bound_decider(
		&self,
		hook: crate::decision::Hook,
	) -> Result<Option<(crate::decision::BoundDecider, super::Entry)>> {
		self.validate()?;
		let mut found = None;
		for binding in self
			.bindings
			.iter()
			.filter(|b| b.definition.kind == "decider")
		{
			let config: crate::decision::DeciderConfig =
				serde_json::from_value(binding.definition.config.clone())?;
			if config.hook != hook {
				continue;
			}
			let pin = crate::decision::DeciderPin {
				identity: binding.identity.clone(),
				definition_digest: binding.digest.clone(),
				configuration_digest: config.digest()?,
				provider_implementation: binding.provider_implementation.clone().ok_or_else(
					|| {
						Error::Invalid(
							"Decider snapshot lacks a Node Provider implementation".into(),
						)
					},
				)?,
			};
			pin.check(&binding.definition)?;
			let bound = crate::decision::BoundDecider {
				pin,
				config,
				restrictions: binding.narrow.decision.clone().unwrap_or_default(),
			};
			if found.replace((bound, binding.definition.clone())).is_some() {
				return Err(Error::Invalid("duplicate retained Decider hook".into()));
			}
		}
		Ok(found)
	}
	pub fn validate(&self) -> Result<()> {
		self.agent.validate()?;
		if self.schema_version != BINDING_SCHEMA
			|| self.bindings.len() > MAX_BINDINGS
			|| self.definitions.len() > MAX_BINDINGS
			|| serde_json::to_vec(self)?.len() > 3_000_000
		{
			return Err(Error::Invalid(
				"invalid or oversized Run Binding snapshot".into(),
			));
		}
		let mut definitions = BTreeMap::new();
		for definition in &self.definitions {
			definition.validate()?;
			if definitions
				.insert(&definition.identity, definition)
				.is_some()
			{
				return Err(Error::Invalid(
					"duplicate qualified snapshot definition".into(),
				));
			}
		}
		for definition in &self.definitions {
			for (reference, kind) in
				definition_references(&definition.identity, &definition.definition)?
			{
				if definitions
					.get(&reference)
					.is_none_or(|target| !reference_kind_matches(&kind, &target.definition.kind))
				{
					return Err(Error::Invalid(
						"snapshot lacks a typed exact dependency".into(),
					));
				}
			}
		}
		let agent_definition = definitions
			.get(&self.agent)
			.ok_or_else(|| Error::Invalid("snapshot lacks its Agent definition".into()))?;
		if agent_definition.definition.kind != "agent" {
			return Err(Error::Invalid("snapshot root is not an Agent".into()));
		}
		let config: AgentBindings =
			serde_json::from_value(agent_definition.definition.config.clone())?;
		config.validate()?;
		self.validate_closure(&config, &definitions)?;
		let expected_foreign =
			self.definitions
				.iter()
				.try_fold(BTreeSet::new(), |mut roots, definition| {
					for (reference, kind) in
						definition_references(&definition.identity, &definition.definition)?
					{
						if kind == "agent"
							&& reference.registry_deployment
								!= definition.identity.registry_deployment
						{
							roots.insert(reference);
						}
					}
					Ok::<_, Error>(roots)
				})?;
		let mut foreign = BTreeSet::new();
		for snapshot in &self.foreign_agents {
			snapshot.validate()?;
			if !expected_foreign.contains(&snapshot.agent)
				|| !foreign.insert(snapshot.agent.clone())
			{
				return Err(Error::Invalid(
					"foreign Agent snapshot has an unrelated or duplicate root".into(),
				));
			}
			for definition in &snapshot.definitions {
				if definitions
					.get(&definition.identity)
					.is_none_or(|saved| **saved != *definition)
				{
					return Err(Error::Invalid(
						"foreign Agent closure differs from the pinned definitions".into(),
					));
				}
			}
		}
		if foreign != expected_foreign {
			return Err(Error::Invalid(
				"snapshot lacks an exact foreign Agent closure".into(),
			));
		}
		if config.instructions.trim().is_empty()
			&& !config.has_plugin_skills()
			&& !config.bindings.iter().any(|binding| {
				binding.kind == BindingKind::Skill
					|| binding.kind == BindingKind::Source
						&& definitions.get(&binding.target).is_some_and(|definition| {
							serde_json::from_value::<sources::NativeContext>(
								definition.definition.config.clone(),
							)
							.is_ok_and(|source| source.requires_skill_support())
						})
			}) {
			return Err(Error::Invalid(
				"Agent requires instructions or a bound Skill context".into(),
			));
		}
		let policy = config.exposure_policy();
		let model = QualifiedRef {
			registry_deployment: self.agent.registry_deployment.clone(),
			id: config.model.id,
			version: config.model.version,
		};
		if definitions
			.get(&model)
			.is_none_or(|entry| entry.definition.kind != "model")
		{
			return Err(Error::Invalid(
				"snapshot lacks its exact model definition".into(),
			));
		}
		let mut aliases = BTreeSet::new();
		for binding in &self.bindings {
			binding.identity.validate()?;
			if binding.identity.id != binding.definition.id
				|| binding.identity.version != binding.definition.version
				|| binding.digest
					!= super::rules::digest(&serde_json::to_value(&binding.definition)?)
				|| binding.installation != binding.definition.installation
				|| definitions.get(&binding.identity).is_none_or(|definition| {
					definition.definition != binding.definition
						|| definition.digest != binding.digest
				}) {
				return Err(Error::Invalid(
					"Run Binding definition identity or digest changed".into(),
				));
			}
			if binding.definition.kind == "tool" {
				if binding.alias.is_none()
					|| binding
						.provider_contract_digest
						.as_deref()
						.is_none_or(str::is_empty)
					|| binding.excluded_reason.is_none()
						&& binding
							.provider_implementation
							.as_deref()
							.is_none_or(str::is_empty)
				{
					return Err(Error::Invalid(
						"Tool snapshot lacks alias or Provider evidence".into(),
					));
				}
			} else if binding.definition.kind == "decider" {
				if binding.alias.is_some()
					|| binding.excluded_reason.is_some()
					|| binding.provider_contract_digest.is_none()
					|| binding.provider_implementation.is_none()
				{
					return Err(Error::Invalid(
						"Decider snapshot lacks exact Provider evidence".into(),
					));
				}
			} else if binding.alias.is_some()
				|| binding.provider_contract_digest.is_some()
				|| binding.provider_implementation.is_some()
				|| binding.excluded_reason.is_some()
			{
				return Err(Error::Invalid(
					"context Bindings cannot declare Tool dispatch fields".into(),
				));
			}
			if binding.excluded_reason.is_some() && binding.origin != BindingOrigin::Default {
				return Err(Error::Invalid(
					"only implicit defaults may be excluded".into(),
				));
			}
			if let Some(alias) = &binding.alias {
				validate_alias(alias)?;
				if binding.excluded_reason.is_none() && !aliases.insert(alias) {
					return Err(Error::Invalid(format!(
						"duplicate resolved tool alias: {alias}"
					)));
				}
			}
		}
		for operation in policy.required_tools() {
			let identity = QualifiedRef::builtin(&self.agent.registry_deployment, operation);
			if !self.bindings.iter().any(|binding| {
				(binding.identity == identity
					|| operation == "workspace_read"
						&& binding.identity.registry_deployment == identity.registry_deployment
						&& binding.identity.id == identity.id
						&& binding.identity.version == "1.0.0")
					&& binding.origin == BindingOrigin::Required
					&& binding.alias.as_deref() == Some(operation)
					&& binding.excluded_reason.is_none()
			}) {
				return Err(Error::Invalid(format!(
					"snapshot lacks required operation: {operation}"
				)));
			}
		}
		Ok(())
	}
}

#[cfg(test)]
mod tests;
