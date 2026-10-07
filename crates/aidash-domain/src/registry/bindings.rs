//! Immutable capability references, supported restrictions and Run resolution records.
use super::{EntityRef, Entry, Projection};
use crate::{Error, Result, configuration::validate_node_id};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub const BINDING_SCHEMA: u8 = 1;
mod snapshot;
pub mod sources;
pub const MAX_BINDINGS: usize = 128;
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
	"memory_mutate",
	"memory_recall",
	"memory_reflect",
];
pub const SKILL_TOOLS: &[&str] = &["skill_list", "skill_load", "skill_read"];
pub const COORDINATOR_TOOLS: &[&str] = &["task_create", "task_delegate", "agent_discover"];

/// Typed exact edges used by admission and durable snapshot validation. Foreign
/// qualifiers are retained, including delegation targets; no local ID guessing.
pub fn definition_references(
	identity: &QualifiedRef,
	entry: &Entry,
) -> Result<Vec<(QualifiedRef, String)>> {
	let mut result = vec![];
	let local = |reference: &EntityRef| QualifiedRef {
		registry_node: identity.registry_node.clone(),
		id: reference.id.clone(),
		version: reference.version.clone(),
	};
	match entry.kind.as_str() {
		"agent" => {
			let config: AgentBindings = serde_json::from_value(entry.config.clone())?;
			config.validate()?;
			result.push((local(&config.model), "model".into()));
			result.extend(config.cluster.iter().map(|r| (local(r), "cluster".into())));
			for normalized in config.normalize(&identity.registry_node)? {
				let kind = match normalized.binding.kind {
					BindingKind::Tool => "tool",
					BindingKind::Bundle => "bundle",
					BindingKind::Skill => "skill",
					BindingKind::Memory => "memory",
					BindingKind::Source => "source",
					BindingKind::Decider => "decider",
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
			if descriptor.registry_node != identity.registry_node {
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
						registry_node: node_id,
						id: agent.id,
						version: agent.version,
					},
					"agent".into(),
				));
			}
		}
		"memory" | "source" => {
			let descriptor: sources::NativeContext = serde_json::from_value(entry.config.clone())?;
			descriptor.validate(&entry.kind)?;
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
}
impl AgentBindings {
	pub fn validate(&self) -> Result<()> {
		if self.schema_version != BINDING_SCHEMA
			|| !(1..=1000).contains(&self.max_steps)
			|| self.bindings.len() > MAX_BINDINGS
			|| self.instructions.trim().is_empty()
				&& !self
					.bindings
					.iter()
					.any(|b| matches!(b.kind, BindingKind::Skill | BindingKind::Source))
			|| self
				.remove_default
				.iter()
				.any(|name| !DEFAULT_TOOLS.contains(&name.as_str()))
			|| self.remove_default.iter().collect::<BTreeSet<_>>().len()
				!= self.remove_default.len()
		{
			return Err(Error::Invalid(
				"invalid Binding schema, defaults or Agent instructions".into(),
			));
		}
		let mut targets = BTreeSet::new();
		for binding in &self.bindings {
			binding.validate()?;
			if !targets.insert(&binding.target) {
				return Err(Error::Invalid("duplicate Binding target".into()));
			}
		}
		if self.bindings.iter().any(|b| b.kind == BindingKind::Skill)
			&& self
				.remove_default
				.iter()
				.any(|name| SKILL_TOOLS.contains(&name.as_str()))
		{
			return Err(Error::Invalid(
				"bound Skills require Skill support tools".into(),
			));
		}
		Ok(())
	}
	pub fn normalize(&self, node: &str) -> Result<Vec<NormalizedBinding>> {
		self.validate()?;
		crate::configuration::validate_node_id(node)?;
		let mut result = Vec::new();
		let has_skills = self.bindings.iter().any(|b| b.kind == BindingKind::Skill);
		for operation in REQUIRED_TOOLS.iter().chain(DEFAULT_TOOLS) {
			let required = REQUIRED_TOOLS.contains(operation);
			if !required && self.remove_default.iter().any(|name| name == operation) {
				continue;
			}
			let target = QualifiedRef::builtin(node, operation);
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
				} else if has_skills && SKILL_TOOLS.contains(operation) {
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
		if result.len() > MAX_BINDINGS {
			return Err(Error::Invalid(
				"expanded Bindings exceed 128 entries".into(),
			));
		}
		Ok(result)
	}
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QualifiedRef {
	pub registry_node: String,
	pub id: String,
	pub version: String,
}
impl QualifiedRef {
	pub fn validate(&self) -> Result<()> {
		validate_node_id(&self.registry_node)?;
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
			registry_node: node.into(),
			id: format!("aidash.{operation}"),
			version: "1.0.0".into(),
		}
	}
	pub fn resource_id(&self) -> String {
		format!("{}/tools/{}@{}", self.registry_node, self.id, self.version)
	}
}

/// No effect, replay, approval or disclosure field is accepted here.
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
					keep_threshold: if a.keep_threshold.value() <= b.keep_threshold.value() {
						a.keep_threshold
					} else {
						b.keep_threshold
					},
					preserve_recent: a.preserve_recent.max(b.preserve_recent),
					forbid_apply: a.forbid_apply || b.forbid_apply,
				}),
				(Some(a), None) | (None, Some(a)) => Some(a.clone()),
				(None, None) => None,
			},
			allowed_hosts,
			scope,
			limits,
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
}
impl Binding {
	pub fn tool(target: QualifiedRef) -> Self {
		Self {
			kind: BindingKind::Tool,
			target,
			alias: None,
			narrow: Narrowing::default(),
			members: vec![],
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
					.is_some_and(|r| r.keep_threshold.value() > 0.5 || r.preserve_recent < 6)
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
	/// Reconstruct an explicit exact Decider from the retained admission closure.
	pub fn decider(&self, hook: crate::decision::Hook) -> Result<crate::decision::BoundDecider> {
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
			};
			pin.check(&binding.definition)?;
			let bound = crate::decision::BoundDecider {
				pin,
				config,
				restrictions: binding.narrow.decision.clone().unwrap_or_default(),
			};
			if found.replace(bound).is_some() {
				return Err(Error::Invalid("duplicate retained Decider hook".into()));
			}
		}
		found.ok_or_else(|| Error::Invalid("Run has no explicit Decider for this hook".into()))
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
		if config.instructions.trim().is_empty()
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
		let model = QualifiedRef {
			registry_node: self.agent.registry_node.clone(),
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
		for operation in REQUIRED_TOOLS {
			let identity = QualifiedRef::builtin(&self.agent.registry_node, operation);
			if !self.bindings.iter().any(|binding| {
				binding.identity == identity
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
