//! Why a Run's authority refused a tool call (an Authority Pause), without protected content.
use serde::{Deserialize, Serialize};
use std::fmt;

/// The check that refused. A closed set so the Deployment Administrator can tell policy denials,
/// which `authorization_decisions` records, from every other refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefusalCheck {
	/// The Tenant policy denied `action` on the resource.
	Policy,
	/// The agent version's flag for this tool is off.
	AgentFlag,
	/// The agent lacks the core capability the tool needs.
	CoreCapability,
	/// Catalog Approval refused the Registry entry the call needs.
	Catalog,
	/// The remote Node's authority could not be refreshed or does not cover the call.
	RemoteAuthority,
	/// The policy explicitly denies `tool.call_unapproved`.
	CallUnapprovedDeny,
	/// The agent is not configured with the Registry tool.
	RegistryTool,
	/// The Run did not pin the Agent Plugin tool.
	PluginTool,
	/// The agent may not delegate to the Node the execution names.
	AgentExecution,
}

/// A tool call refused by the Run's authority. It names the tool and the check,
/// never the resource it addressed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorityRefusal {
	pub tool: String,
	pub check: RefusalCheck,
	/// The policy or Catalog action that was refused, when the check has one.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub action: Option<String>,
}

impl fmt::Display for AuthorityRefusal {
	/// The short sentence a Run's `error` shows.
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let tool = &self.tool;
		let action = self.action.as_deref();
		match (self.check, action) {
			(RefusalCheck::Policy, Some(action)) => write!(f, "{tool}: {action} was denied"),
			(RefusalCheck::Policy, None) => write!(f, "{tool}: policy denied the call"),
			(RefusalCheck::AgentFlag, _) => {
				write!(
					f,
					"{tool}: the agent's configuration does not permit this tool"
				)
			}
			(RefusalCheck::CoreCapability, _) => {
				write!(
					f,
					"{tool}: the agent lacks the core capability this tool needs"
				)
			}
			(RefusalCheck::Catalog, Some(action)) => {
				write!(f, "{tool}: Catalog Approval refused {action}")
			}
			(RefusalCheck::Catalog, None) => write!(f, "{tool}: Catalog Approval refused the call"),
			(RefusalCheck::RemoteAuthority, _) => {
				write!(
					f,
					"{tool}: the remote Node's authority does not cover the call"
				)
			}
			(RefusalCheck::CallUnapprovedDeny, _) => {
				write!(f, "{tool}: tool.call_unapproved is explicitly denied")
			}
			(RefusalCheck::RegistryTool, _) => {
				write!(f, "{tool}: the agent is not configured with this tool")
			}
			(RefusalCheck::PluginTool, _) => {
				write!(f, "{tool}: the Run did not pin this plugin tool")
			}
			(RefusalCheck::AgentExecution, _) => {
				write!(f, "{tool}: the agent may not delegate to this Node")
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use rstest::rstest;

	#[rstest]
	#[case(
		RefusalCheck::Policy,
		Some("task.delegate"),
		"task_delegate: task.delegate was denied"
	)]
	#[case(
		RefusalCheck::AgentFlag,
		None,
		"task_delegate: the agent's configuration does not permit this tool"
	)]
	fn sentence_names_the_tool_and_the_check(
		#[case] check: RefusalCheck,
		#[case] action: Option<&str>,
		#[case] expected: &str,
	) {
		let refusal = AuthorityRefusal {
			tool: "task_delegate".into(),
			check,
			action: action.map(str::to_owned),
		};
		assert_eq!(refusal.to_string(), expected);
	}

	#[rstest]
	fn check_serializes_as_its_name() {
		assert_eq!(
			serde_json::to_value(RefusalCheck::CallUnapprovedDeny).unwrap(),
			serde_json::json!("call_unapproved_deny")
		);
	}
}
