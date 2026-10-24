//! Provider-owned operation facts. Descriptors select and constrain these facts.
use super::{
	AuthorizationRequirement, Concurrency, Continuation, DisclosureBoundary, ToolAuthorization,
	ToolBehavior, ToolConfig, ToolContract, ToolEffect, ToolIdentity, Yield, builtin_contract,
};
use crate::{
	Error, Result,
	registry::bindings::{Narrowing, QualifiedRef, validate_alias},
	research::Operation as ResearchOperation,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolTier {
	Builtin,
	/// Acts outside Aidash from an Execution Pod; each Tenant approves it.
	Sandboxed,
	/// A Research provider the Deployment offers; each Tenant approves it.
	Research,
	Integration,
	Fixture,
}

impl ToolTier {
	/// Whether a built-in of this tier needs a Catalog Approval before use.
	pub fn needs_approval(self) -> bool {
		matches!(self, Self::Sandboxed | Self::Research)
	}
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OperationLifecycle {
	pub poll: QualifiedRef,
	pub cancel: QualifiedRef,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToolDescriptor {
	pub registry_deployment: String,
	pub provider: String,
	pub operation: String,
	pub default_alias: String,
	pub tier: ToolTier,
	#[serde(default)]
	pub narrow: Narrowing,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub transport: Option<ToolConfig>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub lifecycle: Option<OperationLifecycle>,
}

pub fn core_provider(operation: &str) -> Option<&'static str> {
	Some(match operation {
		"workspace_read" | "workspace_observe" | "workspace_wait" | "workspace_message" => {
			"core.workspace@1"
		}
		"human_request" => "core.human@1",
		"task_create" | "task_delegate" | "agent_discover" | "task_assign" => "core.tasks@1",
		"skill_list" | "skill_load" | "skill_read" | "skill_asset_read" => "core.skills@1",
		"capability_search" | "capability_describe" | "capability_load" | "capability_unload" => {
			"core.exposure@1"
		}
		"file_search" | "file_read" | "apply_patch" => "core.files@1",
		"artifact_publish" => "core.artifacts@1",
		"memory_mutate" | "memory_recall" | "memory_reflect" => "core.memory@1",
		"shell" | "shell_poll" | "shell_cancel" | "code_interpreter" | "python_poll"
		| "python_cancel" => "core.sandbox@1",
		"outbound_get" => "core.egress@1",
		"file_share" => "core.sharing@1",
		_ => return None,
	})
}
/// Node providers of the host-owned research contract and the operations each
/// implements. Providers are interchangeable: the host behavior is selected by
/// the research operation, so replacing one search provider with another needs
/// no configuration change.
pub const RESEARCH_PROVIDERS: &[(&str, &[&str], ToolTier)] = &[
	("research.brave@1", &["search"], ToolTier::Research),
	(
		"research.page@1",
		&["fetch", "find", "view"],
		ToolTier::Research,
	),
	(
		"research.fixture@1",
		&["search", "fetch", "find", "view"],
		ToolTier::Fixture,
	),
];
pub fn research_operation(
	provider: &str,
	operation: &str,
) -> Option<(ResearchOperation, ToolTier)> {
	RESEARCH_PROVIDERS
		.iter()
		.find(|(name, operations, _)| *name == provider && operations.contains(&operation))
		.and_then(|(_, _, tier)| Some((ResearchOperation::parse(operation)?, *tier)))
}
/// The stable default Model alias of a research operation.
pub fn research_alias(operation: ResearchOperation) -> &'static str {
	match operation {
		ResearchOperation::Search => "research_search",
		ResearchOperation::Fetch => "research_fetch",
		ResearchOperation::Find => "research_find",
		ResearchOperation::View => "research_view",
	}
}
pub fn research_descriptor(node: &str, provider: &str, operation: &str) -> Result<ToolDescriptor> {
	let (research, tier) = research_operation(provider, operation)
		.ok_or_else(|| Error::Invalid("unknown research provider operation".into()))?;
	Ok(ToolDescriptor {
		registry_deployment: node.into(),
		provider: provider.into(),
		operation: operation.into(),
		default_alias: research_alias(research).into(),
		tier,
		narrow: Narrowing::default(),
		transport: None,
		lifecycle: None,
	})
}
/// Declared behavior of one research operation, identical for every provider.
pub fn research_contract(operation: ResearchOperation) -> ToolContract {
	let (yields, disclosure) = match operation {
		ResearchOperation::Search => (vec![Yield::Search], DisclosureBoundary::External),
		ResearchOperation::Fetch => (
			vec![Yield::Fetch, Yield::Citations],
			DisclosureBoundary::External,
		),
		ResearchOperation::Find | ResearchOperation::View => {
			(vec![Yield::Citations], DisclosureBoundary::Local)
		}
	};
	ToolContract {
		identity: ToolIdentity::Builtin(format!("research:{}", operation.as_str())),
		authorization: ToolAuthorization {
			flag: None,
			core: None,
			requirements: Vec::<AuthorizationRequirement>::new(),
		},
		behavior: ToolBehavior {
			// Reading public sources leaves user resources unchanged. A repeated
			// invocation key reuses the recorded attempt instead of dispatching.
			effect: ToolEffect::ReadOnly,
			fitting: None,
			continuation: Continuation::Ordinary,
			catch_up: false,
			media_pending: false,
			// A viewed page reaches the model as an image (ADR 0053).
			yields_model_media: operation == ResearchOperation::View,
			tool_call_approval: false,
			read_only_operation: false,
			// Web disclosure approval takes the worker's outer authority lease.
			concurrency: Concurrency::Sequential,
			exposure_update: false,
			yields,
			disclosure_approval: operation.discloses(),
		},
		disclosure,
		// Evidence is retained by the executing Node and cited from its store.
		remote_exposure: false,
	}
}
pub fn requires_runner(operation: &str) -> bool {
	matches!(
		operation,
		"shell"
			| "shell_poll"
			| "shell_cancel"
			| "code_interpreter"
			| "python_poll"
			| "python_cancel"
	)
}
pub fn tier(operation: &str) -> Option<ToolTier> {
	core_provider(operation)?;
	Some(
		if matches!(
			operation,
			"shell"
				| "shell_poll"
				| "shell_cancel"
				| "code_interpreter"
				| "python_poll"
				| "python_cancel"
				| "outbound_get"
				| "apply_patch"
				| "file_share"
		) {
			ToolTier::Sandboxed
		} else {
			ToolTier::Builtin
		},
	)
}
pub fn core_descriptor(node: &str, operation: &str) -> Result<ToolDescriptor> {
	let provider = core_provider(operation)
		.ok_or_else(|| Error::Invalid("unknown provider operation".into()))?;
	let companions = match operation {
		"shell" => Some(("shell_poll", "shell_cancel")),
		"code_interpreter" => Some(("python_poll", "python_cancel")),
		_ => None,
	};
	Ok(ToolDescriptor {
		registry_deployment: node.into(),
		provider: provider.into(),
		operation: operation.into(),
		default_alias: operation.into(),
		tier: tier(operation).expect("declared core operation"),
		narrow: Narrowing::default(),
		transport: None,
		lifecycle: companions.map(|(poll, cancel)| OperationLifecycle {
			poll: QualifiedRef::builtin(node, poll),
			cancel: QualifiedRef::builtin(node, cancel),
		}),
	})
}

impl ToolDescriptor {
	pub fn validate(&self) -> Result<()> {
		crate::nodes::DeploymentId::parse(&self.registry_deployment)?;
		validate_alias(&self.default_alias)?;
		if self.provider.is_empty() || self.operation.is_empty() {
			return Err(Error::Invalid(
				"descriptor requires a versioned provider and operation".into(),
			));
		}
		self.narrow.intersect(&Narrowing::default())?;
		if let Some(lifecycle) = &self.lifecycle {
			lifecycle.poll.validate()?;
			lifecycle.cancel.validate()?;
			if lifecycle.poll == lifecycle.cancel
				|| lifecycle.poll.registry_deployment != self.registry_deployment
				|| lifecycle.cancel.registry_deployment != self.registry_deployment
			{
				return Err(Error::Invalid(
					"invalid operation lifecycle identities".into(),
				));
			}
		}
		Ok(())
	}
	/// Runtime provider admission additionally verifies deployment and source ownership.
	pub fn declared_contract(&self, identity: QualifiedRef) -> Result<ToolContract> {
		self.validate()?;
		if identity.registry_deployment != self.registry_deployment {
			return Err(Error::Invalid(
				"descriptor registering Node does not match its qualified identity".into(),
			));
		}
		let mut contract = if self.provider.starts_with("core.") {
			if core_provider(&self.operation) != Some(self.provider.as_str())
				|| tier(&self.operation) != Some(self.tier)
				|| self.transport.is_some()
			{
				return Err(Error::Invalid(
					"unknown or incompatible core provider operation".into(),
				));
			}
			let expected = core_descriptor(&self.registry_deployment, &self.operation)?;
			// Installation assigns new descriptor IDs. The resolver verifies the
			// referenced companions' provider and operation, rather than names.
			if self.lifecycle.is_some() != expected.lifecycle.is_some() {
				return Err(Error::Invalid(
					"lifecycle must come from its provider operation".into(),
				));
			}
			let mut contract =
				builtin_contract(&self.operation).expect("declared provider operation");
			// Availability is resolved by Bindings, independently of target-resource authority.
			contract.authorization.flag = None;
			contract.authorization.core = None;
			if self.operation == "human_request" {
				contract.remote_exposure = true;
			}
			contract
		} else if self.provider.starts_with("research.") {
			let (operation, tier) = research_operation(&self.provider, &self.operation)
				.ok_or_else(|| Error::Invalid("unknown research provider operation".into()))?;
			if tier != self.tier || self.transport.is_some() || self.lifecycle.is_some() {
				return Err(Error::Invalid(
					"incompatible research provider descriptor".into(),
				));
			}
			research_contract(operation)
		} else {
			if self.tier != ToolTier::Integration || self.lifecycle.is_some() {
				return Err(Error::Invalid(
					"integration descriptor cannot declare system tier or lifecycle".into(),
				));
			}
			let transport = self.transport.as_ref().ok_or_else(|| {
				Error::Invalid("integration requires transport configuration".into())
			})?;
			let compatible = match transport {
				ToolConfig::Http { .. } => {
					self.provider == "integration.http@1" && self.operation == "invoke"
				}
				ToolConfig::Mcp { .. } => {
					self.provider == "integration.mcp@1" && self.operation == "invoke"
				}
				ToolConfig::Agent { .. } => {
					self.provider == "integration.agent@1" && self.operation == "invoke"
				}
				ToolConfig::Native { .. } => false,
			};
			if !compatible {
				return Err(Error::Invalid(
					"unknown or incompatible integration provider".into(),
				));
			}
			ToolContract::registry(identity.local(), transport)
		};
		validate_restrictions(&self.operation, &self.narrow)?;
		validate_concurrency(&contract, &self.narrow)?;
		contract.identity = ToolIdentity::Descriptor(identity);
		Ok(contract)
	}
}

/// Registry configuration may lower a provider's Concurrency safety, never raise it.
pub fn validate_concurrency(contract: &ToolContract, narrow: &Narrowing) -> Result<()> {
	if narrow
		.concurrency
		.is_some_and(|claim| claim > contract.behavior.concurrency)
	{
		return Err(Error::Invalid(
			"Tool configuration claims more concurrency safety than its provider".into(),
		));
	}
	Ok(())
}

/// Supported argument restrictions are provider declarations, not arbitrary JSON overlays.
pub fn validate_restrictions(operation: &str, narrow: &Narrowing) -> Result<()> {
	let (hosts, scopes, limits): (bool, &[&str], &[&str]) = match operation {
		"outbound_get" => (true, &[], &[]),
		"workspace_read" => (false, &["kind", "id"], &["max_chars"]),
		"workspace_observe" => (false, &[], &["limit"]),
		"file_search" => (false, &["scope"], &["limit"]),
		"file_read" => (false, &["file_id", "representation"], &["max_bytes"]),
		"file_share" => (
			false,
			&[
				"/recipient/node_id",
				"/recipient/agent_id",
				"/recipient/agent_version",
				"/recipient/thread_id",
			],
			&[],
		),
		"skill_read" => (false, &["skill_id", "path"], &["max_chars"]),
		_ => (false, &[], &[]),
	};
	if narrow.decision.is_some()
		|| !hosts && narrow.allowed_hosts.is_some()
		|| narrow
			.scope
			.keys()
			.any(|key| !scopes.contains(&key.as_str()))
		|| narrow
			.limits
			.keys()
			.any(|key| !limits.contains(&key.as_str()))
		|| narrow.allowed_hosts.as_ref().is_some_and(|hosts| {
			hosts
				.iter()
				.any(|host| host.is_empty() || host.contains('/') || host.contains(':'))
		}) {
		return Err(Error::Invalid(
			"provider does not support the submitted restriction".into(),
		));
	}
	narrow.intersect(&Narrowing::default())?;
	Ok(())
}

/// Never let a default reference disguise an explicit request at receiver admission.
pub fn remote_exclusion(
	origin: crate::registry::bindings::BindingOrigin,
	contract: &ToolContract,
) -> Result<Option<String>> {
	if contract.remote_exposure {
		return Ok(None);
	}
	if origin == crate::registry::bindings::BindingOrigin::Default {
		Ok(Some(
			"provider contract is ineligible for remote execution".into(),
		))
	} else {
		Err(Error::Invalid(
			"explicit, required or support capability is unavailable remotely".into(),
		))
	}
}

pub fn reserved_aliases() -> BTreeSet<&'static str> {
	crate::registry::bindings::REQUIRED_TOOLS
		.iter()
		.chain(crate::registry::bindings::DEFAULT_TOOLS)
		.chain(crate::registry::bindings::EXPOSURE_TOOLS)
		.copied()
		.chain([
			crate::registry::bindings::SKILL_ASSET_READ,
			"shell",
			"shell_poll",
			"shell_cancel",
			"code_interpreter",
			"python_poll",
			"python_cancel",
			"outbound_get",
			"apply_patch",
			"file_share",
			"task_assign",
		])
		.collect()
}

#[cfg(test)]
mod tests;
