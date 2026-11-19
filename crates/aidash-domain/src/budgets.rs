//! Budgets: the money brake that replaces the count limits (#1012, ADR 0143).
//!
//! A Budget is optional. It covers the Tenant, one Node, one Duty or one
//! person (the root person of every chain, so everything an Agent sets up for
//! that person counts against it). Its Soft Limit sends mail; its Hard Limit
//! stops new model calls, either until a person resumes them or until the
//! Budget has room again. Spend is the provider-reported cost of model calls in
//! nanocredits (one credit is one US dollar on OpenRouter); a call whose cost
//! the provider did not report counts as zero.
use crate::{Error, Result};
use chrono::{DateTime, Datelike, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Recipients of one Budget's Soft Limit mail.
pub const MAX_RECIPIENTS: usize = 20;
/// The longest mail address RFC 5321 allows.
const MAX_ADDRESS_BYTES: usize = 254;

/// What a Budget covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(rename = "BudgetScope")]
pub enum Scope {
	Tenant,
	Node {
		node: String,
	},
	Duty {
		duty_id: Uuid,
	},
	/// The root person of a chain: their own Runs and everything their Agents
	/// set up for them.
	Person {
		subject: String,
	},
}
impl Scope {
	pub fn kind(&self) -> &'static str {
		match self {
			Self::Tenant => "tenant",
			Self::Node { .. } => "node",
			Self::Duty { .. } => "duty",
			Self::Person { .. } => "person",
		}
	}
	/// The scoped key stored beside the kind: empty for the Tenant.
	pub fn key(&self) -> String {
		match self {
			Self::Tenant => String::new(),
			Self::Node { node } => node.clone(),
			Self::Duty { duty_id } => duty_id.to_string(),
			Self::Person { subject } => subject.clone(),
		}
	}
	pub fn from_parts(kind: &str, key: &str) -> Result<Self> {
		Ok(match kind {
			"tenant" => Self::Tenant,
			"node" => Self::Node { node: key.into() },
			"duty" => Self::Duty {
				duty_id: key
					.parse()
					.map_err(|_| Error::Invalid("invalid Duty id".into()))?,
			},
			"person" => Self::Person {
				subject: key.into(),
			},
			_ => return Err(Error::Invalid("unknown Budget scope".into())),
		})
	}
}

/// When spend starts counting again.
#[derive(
	Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, Default,
)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "BudgetPeriod")]
pub enum Period {
	/// Each calendar month in UTC.
	#[default]
	Monthly,
	/// Since the Budget was set, never reset.
	Total,
}
impl Period {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Monthly => "monthly",
			Self::Total => "total",
		}
	}
	pub fn parse(value: &str) -> Result<Self> {
		match value {
			"monthly" => Ok(Self::Monthly),
			"total" => Ok(Self::Total),
			_ => Err(Error::Invalid("unknown Budget period".into())),
		}
	}
	/// The start of the window `now` falls in; a Total Budget counts from
	/// `since`, when it was set.
	pub fn window_start(self, since: DateTime<Utc>, now: DateTime<Utc>) -> DateTime<Utc> {
		match self {
			Self::Monthly => month_start(now.year(), now.month()),
			Self::Total => since,
		}
	}
	/// When the window resets, or None when it never does.
	pub fn next_reset(self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
		match self {
			Self::Monthly => Some(if now.month() == 12 {
				month_start(now.year() + 1, 1)
			} else {
				month_start(now.year(), now.month() + 1)
			}),
			Self::Total => None,
		}
	}
}

fn month_start(year: i32, month: u32) -> DateTime<Utc> {
	Utc.with_ymd_and_hms(year, month, 1, 0, 0, 0)
		.single()
		.expect("the first of a month at midnight UTC exists")
}

/// What reaching the Hard Limit does to a Run that wants another model call.
#[derive(
	Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, Default,
)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "BudgetOnHardLimit")]
pub enum OnHardLimit {
	/// Pause the Run until a person resumes it (the default: money is a
	/// person's decision).
	#[default]
	Stop,
	/// Let the Run sleep until the Budget has room again: the next window, or
	/// a raised limit.
	Sleep,
}
impl OnHardLimit {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Stop => "stop",
			Self::Sleep => "sleep",
		}
	}
	pub fn parse(value: &str) -> Result<Self> {
		match value {
			"stop" => Ok(Self::Stop),
			"sleep" => Ok(Self::Sleep),
			_ => Err(Error::Invalid("unknown Hard Limit action".into())),
		}
	}
}

/// What a person sets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "BudgetDefinition")]
pub struct Definition {
	pub scope: Scope,
	#[serde(default)]
	pub period: Period,
	/// Mail is sent once per window when spend reaches it.
	#[serde(default)]
	pub soft_limit_nanocredits: Option<i64>,
	#[serde(default)]
	pub hard_limit_nanocredits: Option<i64>,
	#[serde(default)]
	pub on_hard_limit: OnHardLimit,
	/// Who gets the Soft Limit mail, besides nobody: people chosen or typed
	/// by whoever sets the Budget.
	#[serde(default)]
	pub recipients: Vec<String>,
}
impl Definition {
	pub fn validate(&self) -> Result<()> {
		if self.soft_limit_nanocredits.is_none() && self.hard_limit_nanocredits.is_none() {
			return Err(Error::Invalid(
				"a Budget needs a Soft Limit, a Hard Limit or both".into(),
			));
		}
		for limit in [self.soft_limit_nanocredits, self.hard_limit_nanocredits]
			.into_iter()
			.flatten()
		{
			if limit <= 0 {
				return Err(Error::Invalid("a Budget limit must be positive".into()));
			}
		}
		if let (Some(soft), Some(hard)) = (self.soft_limit_nanocredits, self.hard_limit_nanocredits)
			&& soft >= hard
		{
			return Err(Error::Invalid(
				"the Soft Limit must be below the Hard Limit".into(),
			));
		}
		if self.recipients.len() > MAX_RECIPIENTS {
			return Err(Error::Invalid(format!(
				"a Budget mails at most {MAX_RECIPIENTS} recipients"
			)));
		}
		for address in &self.recipients {
			valid_address(address)?;
		}
		if self.soft_limit_nanocredits.is_some() && self.recipients.is_empty() {
			return Err(Error::Invalid(
				"a Soft Limit needs at least one mail recipient".into(),
			));
		}
		match &self.scope {
			Scope::Node { node } if node.is_empty() => {
				Err(Error::Invalid("a Node Budget names its Node".into()))
			}
			Scope::Person { subject } if subject.is_empty() => {
				Err(Error::Invalid("a person's Budget names the person".into()))
			}
			_ => Ok(()),
		}
	}
}

/// A plain shape check; the SMTP server has the last word.
fn valid_address(address: &str) -> Result<()> {
	let Some((local, domain)) = address.split_once('@') else {
		return Err(Error::Invalid("invalid mail address".into()));
	};
	if local.is_empty()
		|| domain.is_empty()
		|| address.len() > MAX_ADDRESS_BYTES
		|| address.chars().any(|c| c.is_whitespace() || c.is_control())
		|| domain.contains('@')
	{
		return Err(Error::Invalid("invalid mail address".into()));
	}
	Ok(())
}

/// A Budget's standing in its current window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "BudgetStanding")]
pub enum Standing {
	Under,
	/// At or above the Soft Limit, below the Hard Limit.
	Soft,
	/// At or above the Hard Limit.
	Hard,
}

impl Definition {
	pub fn standing(&self, spent: i64) -> Standing {
		if self
			.hard_limit_nanocredits
			.is_some_and(|hard| spent >= hard)
		{
			Standing::Hard
		} else if self
			.soft_limit_nanocredits
			.is_some_and(|soft| spent >= soft)
		{
			Standing::Soft
		} else {
			Standing::Under
		}
	}
}

/// What the Hard Limits that cover a Run decide about its next model call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
	Open,
	/// Pause the Run until a person resumes it.
	Stop {
		budget: Uuid,
	},
	/// Sleep until `until`, when the earliest-resetting exhausted Budget has
	/// room again; None means no window resets, so only a raised limit helps.
	Sleep {
		budget: Uuid,
		until: Option<DateTime<Utc>>,
	},
}

/// Decides the gate from every Budget that covers a Run and its spend in
/// each Budget's window. A Stop wins over a Sleep: a person must look.
pub fn gate<'a>(
	covering: impl IntoIterator<Item = (Uuid, &'a Definition, i64)>,
	now: DateTime<Utc>,
) -> Gate {
	let mut sleep: Option<(Uuid, Option<DateTime<Utc>>)> = None;
	for (id, definition, spent) in covering {
		if definition.standing(spent) != Standing::Hard {
			continue;
		}
		match definition.on_hard_limit {
			OnHardLimit::Stop => return Gate::Stop { budget: id },
			OnHardLimit::Sleep => {
				let until = definition.period.next_reset(now);
				// The Run sleeps until every exhausted Budget has room, so the
				// latest reset wins; a Budget that never resets wins outright.
				sleep = match sleep {
					None => Some((id, until)),
					Some((_, None)) => sleep,
					Some((_, Some(current))) => match until {
						None => Some((id, None)),
						Some(next) if next > current => Some((id, Some(next))),
						Some(_) => sleep,
					},
				};
			}
		}
	}
	match sleep {
		Some((budget, until)) => Gate::Sleep { budget, until },
		None => Gate::Open,
	}
}

#[cfg(test)]
mod tests;
