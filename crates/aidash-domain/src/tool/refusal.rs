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
	/// The other Node asked for approval (`PolicyAsk`, `NodeUseAsk`) while this Node delegated a
	/// Task to it. The ask can arrive after that Node admitted the Task, so a person cannot
	/// decline the call: it may have partly happened.
	RemoteAsk,
	/// A Tenant Deletion is in progress or has completed for the Run's Tenant.
	TenantDeletion,
	/// The Run's own authority was gone when the effect had already happened.
	RunAuthority,
	/// An authority check refused a step and left no note saying which one.
	Unnamed,
}

/// Appended to every call error that the model must not answer by calling again.
pub const DO_NOT_REPEAT: &str = "Do not repeat this call.";

/// Which authority refused a call a person could have lifted (#763). A closed set the model is
/// told about, so it names no action, subject or person.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RefusalKind {
	/// The Tenant policy of the Node the Run executes on.
	Policy,
	/// Catalog Approval of a Registry entry.
	CatalogApproval,
	/// The Agent's own configuration.
	AgentConfiguration,
	/// The authority of the Node the Run was delegated to.
	OtherNode,
}

impl RefusalKind {
	/// The fixed call error a Declined Call hands the model. Every Run of every Tenant gets the
	/// same words for the same kind, and none names an action or a person.
	pub fn declined_sentence(self) -> String {
		let refused = match self {
			Self::Policy => "this Node's policy refused",
			Self::CatalogApproval => "this Node's Catalog Approval refused",
			Self::AgentConfiguration => "this Agent's configuration does not permit",
			Self::OtherNode => "another Node's authority refused",
		};
		format!("A person declined this call, which {refused}. Find another way. {DO_NOT_REPEAT}")
	}
}

impl RefusalCheck {
	/// The kind of refusal a person may decline, or `None` when no decision of a person could
	/// stand for it: the effect already happened, the Tenant is gone, or nothing named the check.
	pub fn declinable_kind(self) -> Option<RefusalKind> {
		match self {
			Self::Policy | Self::CallUnapprovedDeny => Some(RefusalKind::Policy),
			Self::Catalog => Some(RefusalKind::CatalogApproval),
			Self::AgentFlag
			| Self::CoreCapability
			| Self::RegistryTool
			| Self::PluginTool
			| Self::AgentExecution => Some(RefusalKind::AgentConfiguration),
			Self::RemoteAuthority => Some(RefusalKind::OtherNode),
			Self::RemoteAsk | Self::TenantDeletion | Self::RunAuthority | Self::Unnamed => None,
		}
	}
}

/// The tool and input of a stopped call, retained privately for the invocation journal.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RefusedToolCall {
	pub name: String,
	pub arguments: serde_json::Value,
}

impl fmt::Debug for RefusedToolCall {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("RefusedToolCall")
			.field("name", &self.name)
			.field("arguments", &"[redacted]")
			.finish()
	}
}

/// The tool call a refusal stopped, so a pause can offer to decline it. Never shown to people:
/// the call is named only to someone who may decline it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefusedCall {
	/// The call's invocation key.
	pub key: String,
	/// The call's position in its model response.
	pub index: usize,
	/// Whether the tool may run again, which decides whether a row is left `REFUSED`.
	pub replay_safe: bool,
	/// The actual call, since its cursor may index a filtered response.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub tool_call: Option<RefusedToolCall>,
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
	/// The call that was refused, set only where the refusal is raised for one call (#763).
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub call: Option<RefusedCall>,
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
			(RefusalCheck::RemoteAsk, Some(action)) => {
				write!(f, "{tool}: the other Node asks for approval of {action}")
			}
			(RefusalCheck::RemoteAsk, None) => {
				write!(f, "{tool}: the other Node asks for approval")
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
			call: None,
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
			call: None,
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
			call: None,
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

	#[rstest]
	#[case(RefusalCheck::Policy, Some(RefusalKind::Policy))]
	#[case(RefusalCheck::CallUnapprovedDeny, Some(RefusalKind::Policy))]
	#[case(RefusalCheck::Catalog, Some(RefusalKind::CatalogApproval))]
	#[case(RefusalCheck::AgentFlag, Some(RefusalKind::AgentConfiguration))]
	#[case(RefusalCheck::CoreCapability, Some(RefusalKind::AgentConfiguration))]
	#[case(RefusalCheck::RegistryTool, Some(RefusalKind::AgentConfiguration))]
	#[case(RefusalCheck::PluginTool, Some(RefusalKind::AgentConfiguration))]
	#[case(RefusalCheck::AgentExecution, Some(RefusalKind::AgentConfiguration))]
	#[case(RefusalCheck::RemoteAuthority, Some(RefusalKind::OtherNode))]
	#[case(RefusalCheck::RemoteAsk, None)]
	#[case(RefusalCheck::TenantDeletion, None)]
	#[case(RefusalCheck::RunAuthority, None)]
	#[case(RefusalCheck::Unnamed, None)]
	fn only_a_liftable_refusal_has_a_kind(
		#[case] check: RefusalCheck,
		#[case] expected: Option<RefusalKind>,
	) {
		assert_eq!(check.declinable_kind(), expected);
	}

	#[rstest]
	#[case(
		RefusalKind::Policy,
		"A person declined this call, which this Node's policy refused. Find another way. Do not repeat this call."
	)]
	#[case(
		RefusalKind::CatalogApproval,
		"A person declined this call, which this Node's Catalog Approval refused. Find another way. Do not repeat this call."
	)]
	#[case(
		RefusalKind::AgentConfiguration,
		"A person declined this call, which this Agent's configuration does not permit. Find another way. Do not repeat this call."
	)]
	#[case(
		RefusalKind::OtherNode,
		"A person declined this call, which another Node's authority refused. Find another way. Do not repeat this call."
	)]
	fn each_kind_has_one_fixed_sentence(#[case] kind: RefusalKind, #[case] expected: &str) {
		assert_eq!(kind.declined_sentence(), expected);
	}

	#[rstest]
	fn debugging_a_stopped_call_does_not_expose_its_arguments() {
		let stopped = RefusedToolCall {
			name: "workspace_read".into(),
			arguments: serde_json::json!({"private": "sensitive-value"}),
		};
		let debug = format!("{stopped:?}");
		assert!(debug.contains("workspace_read"));
		assert!(!debug.contains("sensitive-value"));
	}

	#[rstest]
	fn refused_call_stays_off_the_wire_until_set() {
		let mut refusal = AuthorityRefusal {
			tool: "task_delegate".into(),
			check: RefusalCheck::Policy,
			action: None,
			subject: None,
			call: None,
		};
		assert!(
			serde_json::to_value(&refusal)
				.unwrap()
				.get("call")
				.is_none()
		);
		refusal.call = Some(RefusedCall {
			key: "run:1:0".into(),
			index: 0,
			replay_safe: true,
			tool_call: None,
		});
		let wire = serde_json::to_value(&refusal).unwrap();
		assert_eq!(wire["call"]["key"], "run:1:0");
		assert_eq!(
			serde_json::from_value::<AuthorityRefusal>(wire).unwrap(),
			refusal
		);
		// The call does not change the short sentence a Run's `error` shows.
		assert_eq!(refusal.to_string(), "task_delegate: policy denied the call");
	}
}
