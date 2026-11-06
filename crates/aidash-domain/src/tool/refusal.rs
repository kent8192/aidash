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
	/// A Tenant Deletion is in progress or has completed for the Run's Tenant.
	TenantDeletion,
	/// The Run's own authority was gone when the effect had already happened.
	RunAuthority,
	/// An authority check refused a step and left no note saying which one.
	Unnamed,
}

/// A step refused by the Run's authority. It names the tool (or stage) and the check,
/// never the resource it addressed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorityRefusal {
	/// The tool, or the stage (such as `worker start`) when no tool was running.
	pub tool: String,
	pub check: RefusalCheck,
	/// The policy or Catalog action that was refused, when the check has one.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub action: Option<String>,
	/// Whose authority refused, set only when it is not the Run's own subject chain: the
	/// Agent a task is delegated to.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub subject: Option<String>,
}

impl fmt::Display for AuthorityRefusal {
	/// The short sentence a Run's `error` shows.
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let tool = &self.tool;
		let action = self.action.as_deref();
		match (self.check, action) {
			(RefusalCheck::Policy, Some(action)) if self.subject.is_some() => {
				let subject = self.subject.as_deref().unwrap_or_default();
				write!(f, "{tool}: {action} was denied for {subject}")
			}
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
			(RefusalCheck::TenantDeletion, _) => {
				write!(f, "{tool}: the Tenant is being deleted")
			}
			(RefusalCheck::RunAuthority, _) => {
				write!(
					f,
					"{tool}: the Run's authority was gone when the call ended"
				)
			}
			(RefusalCheck::Unnamed, _) => {
				write!(f, "{tool}: an authority check refused this step")
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
			subject: None,
		};
		assert_eq!(refusal.to_string(), expected);
	}

	#[rstest]
	#[case(
		RefusalCheck::TenantDeletion,
		"task_delegate: the Tenant is being deleted"
	)]
	#[case(
		RefusalCheck::RunAuthority,
		"task_delegate: the Run's authority was gone when the call ended"
	)]
	#[case(
		RefusalCheck::Unnamed,
		"task_delegate: an authority check refused this step"
	)]
	fn fixed_checks_have_their_own_sentence(#[case] check: RefusalCheck, #[case] expected: &str) {
		let refusal = AuthorityRefusal {
			tool: "task_delegate".into(),
			check,
			action: None,
			subject: None,
		};
		assert_eq!(refusal.to_string(), expected);
	}

	#[rstest]
	fn delegated_subject_is_named() {
		let refusal = AuthorityRefusal {
			tool: "task_delegate".into(),
			check: RefusalCheck::Policy,
			action: Some("reference.read".into()),
			subject: Some("agent:architect".into()),
		};
		assert_eq!(
			refusal.to_string(),
			"task_delegate: reference.read was denied for agent:architect"
		);
	}

	#[rstest]
	fn check_serializes_as_its_name() {
		assert_eq!(
			serde_json::to_value(RefusalCheck::CallUnapprovedDeny).unwrap(),
			serde_json::json!("call_unapproved_deny")
		);
	}
}
