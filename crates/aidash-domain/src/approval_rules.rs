//! Approval Modes and Approval Rules are a person's own approvals, given in
//! advance and never beyond what the Tenant's policy lets that person approve
//! (ADR 0076). [`resolve`] is the one place that orders them.
use crate::{capabilities::records::Record, decision::BlockReason};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fmt;
use uuid::Uuid;

/// The policy action that lets a person's rules and mode approve in advance.
/// It is judged on the resource being approved, for the person alone.
pub const APPROVAL_ADVANCE: &str = "approval.advance";
/// The policy action that lets a person use the Auto mode, judged on the
/// resource kind [`MODE_RESOURCE`] with id `auto`.
pub const MODE_AUTO: &str = "approval.mode.auto";
/// The policy action that lets a person use the Bypass mode, judged on the
/// resource kind [`MODE_RESOURCE`] with id `bypass`. Only a policy that names
/// it grants it; a wildcard allow does not (see `policy::EXPLICIT_ACTIONS`).
pub const MODE_BYPASS: &str = "approval.mode.bypass";
/// The resource kind a mode is judged on.
pub const MODE_RESOURCE: &str = "approval_mode";

/// What an approval is for. The target strings below are what a rule names and
/// what [`resolve`] compares them with.
#[derive(
	Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalKind {
	/// A call of a Tenant's HTTP or MCP tool. The target is
	/// `tool:{registry_id}#{operation}`. A rule names the tool and never a
	/// version, unlike the target of a Run grant, which includes it.
	ToolCall,
	/// An outbound read. The target is its origin.
	Outbound,
	/// A Web search disclosure. The target is the Research provider the
	/// Run's Binding names, such as `research.brave@1`.
	WebSearch,
	/// A Page reading disclosure. The target is the page host.
	PageReading,
	/// A generated Agent request. The target is the Generation policy id.
	Generation,
}

/// The targets of one kind that a rule covers. Serialized as the string
/// `"every"` or as `{"exact": "<target>"}`, so no target string can be mistaken
/// for the wildcard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RuleTarget {
	Every,
	Exact(String),
}

impl RuleTarget {
	pub fn matches(&self, target: &str) -> bool {
		match self {
			Self::Every => true,
			Self::Exact(exact) => exact == target,
		}
	}
}

#[derive(
	Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum RuleEffect {
	Allow,
	Ask,
	Deny,
}

/// One person's allow, ask or deny decision on a kind and a target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApprovalRule {
	pub id: Uuid,
	pub kind: ApprovalKind,
	pub target: RuleTarget,
	pub effect: RuleEffect,
}

/// Who decides, for one person's Runs, an approval they would otherwise wait for.
#[derive(
	Debug,
	Clone,
	Copy,
	Default,
	PartialEq,
	Eq,
	Hash,
	PartialOrd,
	Ord,
	Serialize,
	Deserialize,
	JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
	/// A Designated approver is asked.
	#[default]
	Manual,
	/// The Tenant's approval Decider decides.
	Auto,
	/// The approval is refused, so the Run never waits.
	DontAsk,
	/// The approval is given, within the ceiling.
	Bypass,
}

impl ApprovalMode {
	/// The policy action and the id of the `approval_mode` resource that lets
	/// a person use this mode, if the Tenant's policy has to allow it.
	pub fn policy_requirement(self) -> Option<(&'static str, &'static str)> {
		match self {
			Self::Manual | Self::DontAsk => None,
			Self::Auto => Some((MODE_AUTO, "auto")),
			Self::Bypass => Some((MODE_BYPASS, "bypass")),
		}
	}
}

/// What the Tenant's policy says about one approval before any person's rule
/// or mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TenantStance {
	/// An explicit deny.
	Forbidden,
	/// Direct permission, or a Read-only operation: no approval is needed.
	Direct,
	NeedsApproval,
}

/// What the Tenant's policy lets the person the chain starts at do in advance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ceiling {
	/// Their rules and mode may approve this resource.
	pub may_approve: bool,
	/// Their mode is one the policy lets them use. A mode that is not
	/// allowed acts as Manual.
	pub mode_allowed: bool,
}

/// The answers of the Tenant's policy for one approval, from which
/// [`Settings::ceiling`] derives the [`Ceiling`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Standing {
	/// The person is an eligible approver of the resource.
	pub approver: bool,
	/// The policy allows [`APPROVAL_ADVANCE`] on the resource for the person.
	pub advance: bool,
	/// The policy allows the person's mode, if it has to
	/// (see [`ApprovalMode::policy_requirement`]).
	pub mode: bool,
}

/// The mode and the rules of the person a chain starts at.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
	pub mode: ApprovalMode,
	pub rules: Vec<ApprovalRule>,
	/// The person has accepted the risk of Bypass.
	pub bypass_accepted: bool,
	/// The Tenant's approval Decider can judge on this Node: it holds a Catalog
	/// Approval. Only Auto reads it, so a store may leave it false for another
	/// mode.
	pub auto_available: bool,
}

impl Settings {
	pub fn ceiling(&self, standing: Standing) -> Ceiling {
		Ceiling {
			may_approve: standing.approver && standing.advance,
			mode_allowed: match self.mode {
				ApprovalMode::Manual | ApprovalMode::DontAsk => true,
				ApprovalMode::Auto => standing.mode && self.auto_available,
				ApprovalMode::Bypass => standing.mode && self.bypass_accepted,
			},
		}
	}
}

/// What gave an approval in advance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GivenBy {
	Rule(Uuid),
	Bypass,
	/// The approval Decider allowed it; the id is its decision evidence.
	Auto(Uuid),
}

impl GivenBy {
	/// What an approval record keeps as `given_by`.
	pub fn data(self) -> Value {
		match self {
			Self::Rule(id) => json!({"rule":id}),
			Self::Bypass => json!({"mode":"bypass"}),
			Self::Auto(decision) => json!({"mode":"auto","decision":decision}),
		}
	}
}

impl fmt::Display for GivenBy {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::Rule(id) => write!(f, "Approval Rule {id}"),
			Self::Bypass => f.write_str("the Bypass Approval Mode"),
			Self::Auto(decision) => write!(f, "the Auto Approval Mode (decision {decision})"),
		}
	}
}

/// What refused an approval before anyone was asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusedBy {
	Tenant,
	Rule(Uuid),
	DontAsk,
	/// The approval Decider blocked it, or gave no verdict.
	Auto(BlockReason),
}

impl RefusedBy {
	/// What an approval record keeps as `refused_by`.
	pub fn data(self) -> Value {
		match self {
			Self::Tenant => json!({"policy":"tenant"}),
			Self::Rule(id) => json!({"rule":id}),
			Self::DontAsk => json!({"mode":"dont_ask"}),
			Self::Auto(reason) => json!({"mode":"auto","reason":reason}),
		}
	}

	/// The reason an approval record shows as its `error`.
	pub fn code(self) -> &'static str {
		match self {
			Self::Tenant => "REFUSED_BY_POLICY",
			Self::Rule(_) => "REFUSED_BY_APPROVAL_RULE",
			Self::DontAsk => "REFUSED_BY_DONT_ASK",
			Self::Auto(_) => "REFUSED_BY_AUTO",
		}
	}
}

/// How often the approval Decider has refused a Run's calls since a person last
/// approved one. A person's approval clears both counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AutoBlocks {
	/// Blocks in a row, none of the Run's later calls having been allowed by
	/// the Decider. A refusal for want of a verdict is not a block.
	pub trailing: u32,
	/// Refusals in all, with or without a verdict.
	pub since_person: u32,
}

impl AutoBlocks {
	/// Blocks in a row that return the Run to asking.
	pub const TRAILING_LIMIT: u32 = 3;
	/// Refusals in all that return the Run to asking.
	pub const RUN_LIMIT: u32 = 20;

	/// Whether the Run asks a person again until one approves a call.
	pub const fn suspended(self) -> bool {
		self.trailing >= Self::TRAILING_LIMIT || self.since_person >= Self::RUN_LIMIT
	}

	/// Counts the refusals recorded on a Run's approvals, in the order they were
	/// decided. A person's approval clears both counts and a call the Decider
	/// allowed ends the blocks in a row. A refusal for want of a verdict counts
	/// toward the total only, so one bad call is not a streak, yet a long outage
	/// still returns the Run to asking a person. Records decided at the same
	/// instant keep the order given.
	pub fn of(records: &[Record]) -> Self {
		Self::from_data(records.iter().map(|record| &record.data))
	}

	/// [`Self::of`] over the `data` of approval records, for a store that reads
	/// only what the counts need: `decided_at`, `given_by`, `refused_by` and
	/// `approval_mode`.
	pub fn from_data<'a>(data: impl IntoIterator<Item = &'a Value>) -> Self {
		let mut decided: Vec<(DateTime<Utc>, Decision)> = data
			.into_iter()
			.filter_map(|data| {
				let at = serde_json::from_value(data.get("decided_at")?.clone()).ok()?;
				Some((at, Decision::of(data)?))
			})
			.collect();
		decided.sort_by_key(|(at, _)| *at);
		decided
			.into_iter()
			.fold(Self::default(), |counts, (_, decision)| match decision {
				Decision::Person => Self::default(),
				Decision::Allowed => Self {
					trailing: 0,
					..counts
				},
				Decision::Blocked => Self {
					trailing: counts.trailing.saturating_add(1),
					since_person: counts.since_person.saturating_add(1),
				},
				Decision::NoVerdict => Self {
					since_person: counts.since_person.saturating_add(1),
					..counts
				},
			})
	}
}

/// What decided an approval record, as far as [`AutoBlocks`] counts.
enum Decision {
	Person,
	Allowed,
	Blocked,
	/// The Decider refused for want of a verdict.
	NoVerdict,
}

impl Decision {
	fn of(data: &Value) -> Option<Self> {
		if data["given_by"]["mode"] == "auto" {
			return Some(Self::Allowed);
		}
		if data["refused_by"]["mode"] == "auto" {
			return Some(
				if data["refused_by"]["reason"] == BlockReason::Unavailable.id() {
					Self::NoVerdict
				} else {
					Self::Blocked
				},
			);
		}
		// What a person's decision records; a rule or a mode records `advance`.
		matches!(
			data["approval_mode"].as_str(),
			Some("allow_once" | "allow_run")
		)
		.then_some(Self::Person)
	}
}

/// The outcome of the order of evaluation for one approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
	Refuse(RefusedBy),
	/// No approval is needed.
	Proceed,
	/// The person gave the approval in advance.
	Approve(GivenBy),
	/// A Designated approver decides.
	Ask,
	/// The Tenant's approval Decider decides.
	Judge,
}

/// The order of evaluation of one approval (ADR 0076):
///
/// 1. The Tenant's policy refuses, or lets the call proceed without approval.
/// 2. Without `settings` (the Run's chain does not start at a person of this
///    Tenant) a Designated approver is asked.
/// 3. The person's rules that match `kind` and `target`: a deny refuses, else
///    an ask asks (an explicit ask asks in Auto and Bypass too, and is refused
///    in Don't ask), else an allow approves, but only within the ceiling.
///    Among rules of one effect the smallest id decides.
/// 4. The person's mode, which acts as Manual when the policy does not allow it.
///
/// `auto_suspended` returns an Auto person's approvals to asking.
pub fn resolve(
	stance: TenantStance,
	settings: Option<&Settings>,
	ceiling: Ceiling,
	kind: ApprovalKind,
	target: &str,
	auto_suspended: bool,
) -> Resolution {
	match stance {
		TenantStance::Forbidden => return Resolution::Refuse(RefusedBy::Tenant),
		TenantStance::Direct => return Resolution::Proceed,
		TenantStance::NeedsApproval => {}
	}
	let Some(settings) = settings else {
		return Resolution::Ask;
	};
	let mode = if ceiling.mode_allowed {
		settings.mode
	} else {
		ApprovalMode::Manual
	};
	let matching = |effect: RuleEffect| {
		settings
			.rules
			.iter()
			.filter(|rule| {
				rule.effect == effect && rule.kind == kind && rule.target.matches(target)
			})
			.map(|rule| rule.id)
			.min()
	};
	if let Some(id) = matching(RuleEffect::Deny) {
		return Resolution::Refuse(RefusedBy::Rule(id));
	}
	if matching(RuleEffect::Ask).is_some() {
		return match mode {
			ApprovalMode::DontAsk => Resolution::Refuse(RefusedBy::DontAsk),
			_ => Resolution::Ask,
		};
	}
	if ceiling.may_approve
		&& let Some(id) = matching(RuleEffect::Allow)
	{
		return Resolution::Approve(GivenBy::Rule(id));
	}
	match mode {
		ApprovalMode::Manual => Resolution::Ask,
		ApprovalMode::DontAsk => Resolution::Refuse(RefusedBy::DontAsk),
		ApprovalMode::Bypass if ceiling.may_approve => Resolution::Approve(GivenBy::Bypass),
		ApprovalMode::Auto if ceiling.may_approve && !auto_suspended => Resolution::Judge,
		ApprovalMode::Bypass | ApprovalMode::Auto => Resolution::Ask,
	}
}

#[cfg(test)]
mod tests;
