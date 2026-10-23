//! A person's Approval Mode and Approval Rules over HTTP (#411, ADR 0076).
use crate::{Error, Result};
use aidash_domain::approval_rules::{
	ANSWER_DEADLINE_SECONDS, ApprovalKind, ApprovalMode, RuleEffect, RuleTarget,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// What stands in the way of an Approval Mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "ApprovalModeReason")]
pub enum ModeReason {
	/// The Tenant's policy does not let this person use the mode.
	Policy,
	/// The mode is the person's to choose, yet takes effect only once they
	/// accept its risk (Bypass).
	NotAccepted,
	/// The Tenant's approval Decider has no Catalog Approval on this Node (Auto).
	DeciderUnapproved,
	/// The Tenant's approval Decider is not registered as one (Auto).
	NoDecider,
}

/// One Approval Mode and whether the Tenant's policy and this Node let this
/// person use it.
#[derive(Debug, Serialize, schemars::JsonSchema)]
#[schemars(rename = "ApprovalModeChoice")]
pub struct ModeChoice {
	pub mode: ApprovalMode,
	pub allowed: bool,
	/// Why the mode is not available, or not yet in effect; none when it is
	/// simply available.
	pub reason: Option<ModeReason>,
}

/// A Decider by its Registry identity.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "ApprovalDeciderReference")]
pub struct DeciderReference {
	pub id: String,
	pub version: String,
}

/// The approval Decider that judges the Tenant's Auto Approval Mode.
#[derive(Debug, Serialize, schemars::JsonSchema)]
#[schemars(rename = "ApprovalDecider")]
pub struct ApprovalDecider {
	pub decider: DeciderReference,
	/// The Tenant has named no other, so the built-in Decider judges.
	pub built_in: bool,
	/// The Decider is registered in the Tenant as one of hook `approval`.
	pub registered: bool,
	/// The exact version holds a Catalog Approval on this Node, which Auto needs.
	pub approved: bool,
	/// `0` until the Tenant first names a Decider or resets to the built-in.
	pub revision: i64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "ApprovalDeciderUpdate")]
pub struct ApprovalDeciderUpdate {
	pub expected_revision: i64,
	/// A Decider of hook `approval` in the Tenant Registry; `null` resets to the
	/// built-in.
	pub decider: Option<DeciderReference>,
}

impl ApprovalDeciderUpdate {
	pub fn validate(&self) -> Result<()> {
		if self.expected_revision < 0 {
			return Err(Error::Invalid("expected_revision is negative".into()));
		}
		if let Some(decider) = &self.decider {
			aidash_domain::policy::identifier(&decider.id)?;
			aidash_domain::policy::identifier(&decider.version)?;
		}
		Ok(())
	}
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
	/// One minute to 30 days; `null` waits for an answer without limit.
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
			.is_some_and(|seconds| !ANSWER_DEADLINE_SECONDS.contains(&seconds))
		{
			return Err(Error::Invalid(format!(
				"answer_deadline_seconds must be from {} seconds to {} seconds, or null",
				ANSWER_DEADLINE_SECONDS.start(),
				ANSWER_DEADLINE_SECONDS.end()
			)));
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
			ApprovalKind::Sampling => {
				aidash_domain::agent_plugin::interaction::valid_sampling_rule(target, self.effect)
			}
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

	#[rstest]
	#[case::none(None, true)]
	#[case::a_minute(Some(60), true)]
	#[case::thirty_days(Some(30 * 24 * 60 * 60), true)]
	#[case::too_short(Some(59), false)]
	#[case::zero(Some(0), false)]
	#[case::negative(Some(-60), false)]
	#[case::too_long(Some(30 * 24 * 60 * 60 + 1), false)]
	fn a_preference_update_keeps_the_answer_deadline_between_a_minute_and_thirty_days(
		#[case] seconds: Option<i64>,
		#[case] accepted: bool,
	) {
		let update = PreferencesUpdate {
			expected_revision: 0,
			default_mode: ApprovalMode::Manual,
			answer_deadline_seconds: seconds,
			accept_bypass: false,
		};
		assert_eq!(update.validate().is_ok(), accepted);
	}
}
