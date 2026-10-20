//! A person's Approval Mode and Approval Rules over HTTP (#411, ADR 0076).
use crate::{Error, Result};
use aidash_domain::approval_rules::{ApprovalKind, ApprovalMode, RuleEffect, RuleTarget};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One Approval Mode and whether the Tenant's policy lets this person use it.
#[derive(Debug, Serialize, schemars::JsonSchema)]
#[schemars(rename = "ApprovalModeChoice")]
pub struct ModeChoice {
	pub mode: ApprovalMode,
	pub allowed: bool,
}

/// One person's default Approval Mode, Answer Deadline and acceptance of Bypass.
#[derive(Debug, Serialize, schemars::JsonSchema)]
#[schemars(rename = "ApprovalPreferences")]
pub struct Preferences {
	pub default_mode: ApprovalMode,
	/// How long the person's Runs wait for an answer to their model's
	/// question; none means unlimited.
	pub answer_deadline_seconds: Option<i64>,
	/// When the person accepted the risk of Bypass; none before then.
	pub bypass_accepted_at: Option<DateTime<Utc>>,
	/// `0` until the person first saves their preferences.
	pub revision: i64,
	/// Every mode, with whether the Tenant's policy lets this person use it now.
	pub modes: Vec<ModeChoice>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "ApprovalPreferencesUpdate")]
pub struct PreferencesUpdate {
	pub expected_revision: i64,
	pub default_mode: ApprovalMode,
	#[serde(default)]
	pub answer_deadline_seconds: Option<i64>,
	/// Records the person's acceptance of the risk of Bypass. Needed once,
	/// before Bypass can be the default.
	#[serde(default)]
	pub accept_bypass: bool,
}

impl PreferencesUpdate {
	pub fn validate(&self) -> Result<()> {
		if self.expected_revision < 0 {
			return Err(Error::Invalid("expected_revision is negative".into()));
		}
		if self
			.answer_deadline_seconds
			.is_some_and(|seconds| seconds <= 0)
		{
			return Err(Error::Invalid(
				"answer_deadline_seconds must be positive".into(),
			));
		}
		Ok(())
	}
}

/// A stored Approval Rule, and whether the Tenant's policy still lets its
/// owner approve what it names in advance.
#[derive(Debug, Serialize, schemars::JsonSchema)]
#[schemars(rename = "ApprovalRuleView")]
pub struct Rule {
	pub id: Uuid,
	pub kind: ApprovalKind,
	pub target: RuleTarget,
	pub effect: RuleEffect,
	/// `false` for an allow rule when the ceiling no longer lets the owner
	/// approve this kind and target in advance: it is kept but has no effect.
	/// A deny or ask rule always holds, as it relaxes nothing.
	pub effective: bool,
	pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
#[schemars(rename = "ApprovalRules")]
pub struct Rules {
	pub rules: Vec<Rule>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "ApprovalRuleInput")]
pub struct RuleInput {
	pub kind: ApprovalKind,
	pub target: RuleTarget,
	pub effect: RuleEffect,
}

impl RuleInput {
	/// A target must have the shape its kind compares with (see
	/// [`ApprovalKind`]), so a rule that could never match is refused.
	pub fn validate(&self) -> Result<()> {
		let RuleTarget::Exact(target) = &self.target else {
			return Ok(());
		};
		let valid = match self.kind {
			ApprovalKind::ToolCall => tool(target),
			ApprovalKind::Outbound => origin(target),
			ApprovalKind::WebSearch => provider(target),
			ApprovalKind::PageReading => host(target),
			ApprovalKind::Generation => {
				aidash_domain::policy::identifier(target).is_ok() && !target.contains('#')
			}
		};
		if valid {
			Ok(())
		} else {
			Err(Error::Invalid(format!(
				"the target is not a valid {:?} target",
				self.kind
			)))
		}
	}
}

fn plain(text: &str, allowed: impl Fn(char) -> bool) -> bool {
	!text.is_empty() && text.len() <= 256 && text.chars().all(allowed)
}

/// `tool:<registry id>#<operation>`.
fn tool(target: &str) -> bool {
	let Some((id, operation)) = target.strip_prefix("tool:").and_then(|t| t.split_once('#')) else {
		return false;
	};
	// A rule names a tool and never one of its versions, so no `@`.
	let name = |c: char| !c.is_whitespace() && !c.is_control() && !matches!(c, '#' | '*' | '@');
	plain(id, name) && plain(operation, name)
}

/// An origin as the outbound policy writes it: `https://host`, nothing more.
fn origin(target: &str) -> bool {
	aidash_domain::capabilities::outbound::permitted_origin(target, &[target.to_owned()]).is_ok()
}

/// `<provider id>@<version>`, such as `research.brave@1`.
fn provider(target: &str) -> bool {
	let name = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_');
	target
		.split_once('@')
		.is_some_and(|(id, version)| plain(id, name) && plain(version, name))
}

/// A lowercase host name, without scheme, port or path.
fn host(target: &str) -> bool {
	plain(target, |c| {
		c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-')
	})
}

/// A member's settings as a Tenant Administrator with `approval.read` sees them.
#[derive(Debug, Serialize, schemars::JsonSchema)]
#[schemars(rename = "ApprovalSettings")]
pub struct Settings {
	pub subject: String,
	pub default_mode: ApprovalMode,
	pub answer_deadline_seconds: Option<i64>,
	pub bypass_accepted_at: Option<DateTime<Utc>>,
	pub revision: i64,
	pub rules: Vec<Rule>,
}

/// The caller's Approval Mode for one Thread.
#[derive(Debug, Serialize, schemars::JsonSchema)]
#[schemars(rename = "ThreadApprovalModeView")]
pub struct ThreadMode {
	/// The mode the caller set for this Thread; none when their default applies.
	pub mode: Option<ApprovalMode>,
	pub default_mode: ApprovalMode,
	pub modes: Vec<ModeChoice>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "ThreadApprovalModeUpdate")]
pub struct ThreadModeUpdate {
	/// `null` clears the Thread's mode, so the caller's default applies.
	pub mode: Option<ApprovalMode>,
}

#[cfg(test)]
mod tests {
	use super::*;
	use rstest::rstest;

	fn input(kind: ApprovalKind, target: &str) -> RuleInput {
		RuleInput {
			kind,
			target: RuleTarget::Exact(target.into()),
			effect: RuleEffect::Allow,
		}
	}

	#[rstest]
	#[case(ApprovalKind::ToolCall, "tool:crm#invoke")]
	#[case(ApprovalKind::Outbound, "https://example.com")]
	#[case(ApprovalKind::WebSearch, "research.brave@1")]
	#[case(ApprovalKind::PageReading, "example.com")]
	#[case(ApprovalKind::Generation, "reviewers")]
	fn a_target_of_its_kinds_shape_is_valid(#[case] kind: ApprovalKind, #[case] target: &str) {
		assert!(input(kind, target).validate().is_ok());
	}

	#[rstest]
	#[case(ApprovalKind::ToolCall, "crm")]
	#[case(ApprovalKind::ToolCall, "tool:crm")]
	#[case(ApprovalKind::ToolCall, "tool:crm@1#invoke")]
	#[case(ApprovalKind::ToolCall, "tool:#invoke")]
	#[case(ApprovalKind::Outbound, "example.com")]
	#[case(ApprovalKind::Outbound, "https://example.com/path")]
	#[case(ApprovalKind::Outbound, "http://example.com")]
	#[case(ApprovalKind::WebSearch, "research.brave")]
	#[case(ApprovalKind::PageReading, "https://example.com")]
	#[case(ApprovalKind::PageReading, "Example.com")]
	#[case(ApprovalKind::Generation, "")]
	#[case(ApprovalKind::Generation, "two words")]
	fn a_target_of_another_shape_is_refused(#[case] kind: ApprovalKind, #[case] target: &str) {
		assert!(input(kind, target).validate().is_err());
	}

	#[test]
	fn every_target_of_a_kind_needs_no_shape() {
		let every = RuleInput {
			kind: ApprovalKind::Outbound,
			target: RuleTarget::Every,
			effect: RuleEffect::Deny,
		};
		assert!(every.validate().is_ok());
	}

	#[test]
	fn a_preference_update_refuses_a_non_positive_deadline() {
		let update = |seconds| PreferencesUpdate {
			expected_revision: 0,
			default_mode: ApprovalMode::Manual,
			answer_deadline_seconds: seconds,
			accept_bypass: false,
		};
		assert!(update(None).validate().is_ok());
		assert!(update(Some(60)).validate().is_ok());
		assert!(update(Some(0)).validate().is_err());
	}
}
