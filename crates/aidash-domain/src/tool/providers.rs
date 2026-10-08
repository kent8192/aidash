//! Provider-owned operation facts. Descriptors select and constrain these facts.
use super::{ToolConfig, ToolContract, ToolIdentity, builtin_contract};
use crate::{
	Error, Result,
	registry::bindings::{Narrowing, QualifiedRef, validate_alias},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolTier {
	Builtin,
	Host,
	Integration,
	Fixture,
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
	pub registry_node: String,
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
		"skill_list" | "skill_load" | "skill_read" => "core.skills@1",
		"file_search" | "file_read" | "apply_patch" => "core.files@1",
		"artifact_publish" => "core.artifacts@1",
		"memory_mutate" | "memory_recall" | "memory_reflect" => "core.memory@1",
		"shell" | "shell_poll" | "shell_cancel" | "code_interpreter" | "python_install"
		| "python_poll" | "python_cancel" => "core.sandbox@1",
		"outbound_get" => "core.egress@1",
		"file_share" => "core.sharing@1",
		_ => return None,
	})
}
pub fn requires_runner(operation: &str) -> bool {
	matches!(
		operation,
		"shell"
			| "shell_poll"
			| "shell_cancel"
			| "code_interpreter"
			| "python_install"
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
				| "python_install"
				| "python_poll"
				| "python_cancel"
				| "outbound_get"
				| "apply_patch"
				| "file_share"
				| "task_assign"
		) {
			ToolTier::Host
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
		"code_interpreter" | "python_install" => Some(("python_poll", "python_cancel")),
		_ => None,
	};
	Ok(ToolDescriptor {
		registry_node: node.into(),
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
		crate::configuration::validate_node_id(&self.registry_node)?;
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
				|| lifecycle.poll.registry_node != self.registry_node
				|| lifecycle.cancel.registry_node != self.registry_node
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
		if identity.registry_node != self.registry_node {
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
			let expected = core_descriptor(&self.registry_node, &self.operation)?;
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
		contract.identity = ToolIdentity::Descriptor(identity);
		Ok(contract)
	}
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
		.copied()
		.chain([
			"shell",
			"shell_poll",
			"shell_cancel",
			"code_interpreter",
			"python_install",
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
