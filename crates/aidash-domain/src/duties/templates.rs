//! Duty templates (#1011 D4; decisions 13 and 47 of the agent activation
//! design): a ready-made Duty that a Marketplace package carries, such as
//! "GitHub CI duty" or "Gmail triage". A template bundles a mission, a
//! procedure, optional Router and Watcher scripts, the Duty Triggers it
//! expects with a short instruction for each, and an optional suggested
//! Agent. Installing one creates a Duty whose Requester is the person who
//! installs it; its scripts need no approval of their own (ADR 0142).
//!
//! A template never carries a secret, a trigger secret, a Requester or an id
//! of the Tenant it came from. Its shape has no place for them: every part
//! refuses unknown fields, so a manifest that names one is refused rather
//! than silently stripped. Its text and scripts are also checked for
//! credentials written into them (see [`secret_like`]): a Watcher reaches an
//! outside service through the Scratch Space's External services, whose
//! proxy adds the credential (ADR 0137), so a script never needs one.
use super::{
	DEFAULT_MAX_LATENESS_SECONDS, DEFAULT_PAUSE_AFTER_FAILURES, Definition, Patch, Trigger,
	events::TriggerKind,
	scripts::{Script, ScriptChoice, Watcher, WatcherChoice},
};
use crate::{
	Error, Result,
	registry::{EntityRef, Entry, Package},
};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use uuid::Uuid;

/// The Marketplace package kind of a Duty template.
pub const KIND: &str = "duty_template";
/// The longest instruction for one trigger, in characters.
pub const INSTRUCTION_MAX: usize = 2_000;
/// The longest description a publisher writes for the Marketplace list.
pub const DESCRIPTION_MAX: usize = 1_000;
/// How much of the mission describes a template published without a
/// description, in characters.
const DESCRIPTION_FROM_MISSION: usize = 200;

/// A ready-made Duty, as a Marketplace package carries it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "DutyTemplate")]
pub struct Template {
	/// The title the installed Duty starts with.
	pub title: String,
	/// What the Duty is for; every Task it starts is asked this.
	pub mission: String,
	/// The checklist its Agent follows on every Task.
	#[serde(default)]
	pub procedure: String,
	/// The script that decides what each event no Task booked does.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub router: Option<Script>,
	/// The script that polls an outside service on its own interval.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub watcher: Option<Watcher>,
	/// The Duty Triggers the template expects, each with what to set up.
	#[serde(default)]
	pub triggers: Vec<ExpectedTrigger>,
	/// The Agent the publisher staffed it with, suggested to whoever installs
	/// it when their Node has approved it.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub agent: Option<SuggestedAgent>,
}

/// A Duty Trigger a template expects, and a short instruction for whoever
/// installs it. A time trigger is set up as given; the others are listed for
/// the person to create, since only a person's own trigger may carry a
/// webhook secret or name a Duty or Node of their Tenant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(rename = "DutyTemplateTrigger")]
pub enum ExpectedTrigger {
	/// The Duty's time trigger: a five-field cron expression in an IANA time
	/// zone.
	Cron {
		expression: String,
		time_zone: String,
		instruction: String,
	},
	/// The Duty's time trigger: a fixed gap in seconds.
	Interval {
		every_seconds: i64,
		instruction: String,
	},
	/// A signed webhook, such as a GitHub `workflow_run` hook.
	Webhook { instruction: String },
	/// A Task of another Duty of the installing Tenant ends.
	DutyCompleted { instruction: String },
	/// An event another Node sends.
	NodeEvent { instruction: String },
}

/// Which trigger of a Duty an instruction is about.
#[derive(
	Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "DutyTemplateTriggerSlot")]
pub enum Slot {
	/// The Duty's time trigger, cron or interval.
	Time,
	Webhook,
	DutyCompleted,
	NodeEvent,
}

impl Slot {
	/// The slot of a Duty's event trigger. A Watcher's events have none: the
	/// template carries the Watcher itself.
	pub fn of(kind: &TriggerKind) -> Option<Self> {
		match kind {
			TriggerKind::Webhook => Some(Self::Webhook),
			TriggerKind::DutyCompleted { .. } => Some(Self::DutyCompleted),
			TriggerKind::NodeEvent { .. } => Some(Self::NodeEvent),
			TriggerKind::Watcher => None,
		}
	}
}

impl ExpectedTrigger {
	pub fn slot(&self) -> Slot {
		match self {
			Self::Cron { .. } | Self::Interval { .. } => Slot::Time,
			Self::Webhook { .. } => Slot::Webhook,
			Self::DutyCompleted { .. } => Slot::DutyCompleted,
			Self::NodeEvent { .. } => Slot::NodeEvent,
		}
	}

	pub fn instruction(&self) -> &str {
		match self {
			Self::Cron { instruction, .. }
			| Self::Interval { instruction, .. }
			| Self::Webhook { instruction }
			| Self::DutyCompleted { instruction }
			| Self::NodeEvent { instruction } => instruction,
		}
	}

	fn instruction_mut(&mut self) -> &mut String {
		match self {
			Self::Cron { instruction, .. }
			| Self::Interval { instruction, .. }
			| Self::Webhook { instruction }
			| Self::DutyCompleted { instruction }
			| Self::NodeEvent { instruction } => instruction,
		}
	}

	/// The time trigger it sets up, if it is one.
	pub fn time(&self) -> Option<Trigger> {
		match self {
			Self::Cron {
				expression,
				time_zone,
				..
			} => Some(Trigger::Cron {
				expression: expression.clone(),
				time_zone: time_zone.clone(),
			}),
			Self::Interval { every_seconds, .. } => Some(Trigger::Interval {
				every_seconds: *every_seconds,
			}),
			Self::Webhook { .. } | Self::DutyCompleted { .. } | Self::NodeEvent { .. } => None,
		}
	}

	fn of_time(trigger: &Trigger) -> Self {
		match trigger {
			Trigger::Cron {
				expression,
				time_zone,
			} => Self::Cron {
				expression: expression.clone(),
				time_zone: time_zone.clone(),
				instruction: String::new(),
			},
			Trigger::Interval { every_seconds } => Self::Interval {
				every_seconds: *every_seconds,
				instruction: String::new(),
			},
		}
	}

	fn of_slot(slot: Slot) -> Option<Self> {
		let instruction = String::new();
		match slot {
			Slot::Time => None,
			Slot::Webhook => Some(Self::Webhook { instruction }),
			Slot::DutyCompleted => Some(Self::DutyCompleted { instruction }),
			Slot::NodeEvent => Some(Self::NodeEvent { instruction }),
		}
	}
}

/// An Agent a template suggests, by the id and version it had where it was
/// published.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "DutyTemplateAgent")]
pub struct SuggestedAgent {
	pub id: String,
	pub version: String,
}

impl SuggestedAgent {
	pub fn reference(&self) -> EntityRef {
		EntityRef {
			id: self.id.clone(),
			version: self.version.clone(),
		}
	}
}

/// The instruction a publisher writes for one trigger of the Duty they
/// publish.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "DutyTemplateTriggerNote")]
pub struct TriggerNote {
	pub kind: Slot,
	pub instruction: String,
}

/// How a template is listed in the Marketplace.
#[derive(Debug, Clone, Copy)]
pub struct Listing<'a> {
	pub package_id: &'a str,
	/// A semantic version such as `1.0.0`.
	pub version: &'a str,
	pub author: &'a str,
	/// A short description; the start of the mission when empty.
	pub description: &'a str,
}

/// Prefixes of credentials that never belong in a template: Aidash's own
/// Subject Credentials and the services a Duty most often watches.
const CREDENTIAL_PREFIXES: &[&str] = &[
	crate::identity::credentials::SUBJECT_PREFIX,
	crate::identity::desktop::ACCESS_PREFIX,
	"ghp_",
	"gho_",
	"ghu_",
	"ghs_",
	"ghr_",
	"github_pat_",
	"glpat-",
	"xoxb-",
	"xoxp-",
	"xoxa-",
	"xapp-",
	"sk-ant-",
	"sk-or-",
	"sk-proj-",
	"AKIA",
	"ASIA",
	"AIza",
];
/// The fewest token characters after a prefix that make a credential, so
/// prose such as "ghp_ tokens" or "AKIA keys" passes.
const CREDENTIAL_MIN_TAIL: usize = 16;

fn token_char(c: char) -> bool {
	c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// Whether `text` carries something that looks like a credential: a private
/// key block, or a known credential prefix at the start of a token followed
/// by at least 16 token characters. A best-effort guard against a publisher
/// shipping a token they wrote into a script, not a proof of absence.
pub fn secret_like(text: &str) -> bool {
	if text.contains("-----BEGIN") && text.contains("PRIVATE KEY") {
		return true;
	}
	CREDENTIAL_PREFIXES.iter().any(|prefix| {
		text.match_indices(prefix).any(|(index, _)| {
			let starts_token = text[..index]
				.chars()
				.next_back()
				.is_none_or(|c| !token_char(c));
			let tail = text[index + prefix.len()..]
				.chars()
				.take_while(|c| token_char(*c))
				.count();
			starts_token && tail >= CREDENTIAL_MIN_TAIL
		})
	})
}

fn refuse_secret(part: &str, text: &str) -> Result<()> {
	if secret_like(text) {
		return Err(Error::Invalid(format!(
			"a Duty template carries no secrets, and its {part} looks like it holds a credential; reach outside services through the Scratch Space's External services instead"
		)));
	}
	Ok(())
}

fn instruction_valid(text: &str) -> bool {
	!text.trim().is_empty()
		&& text.chars().count() <= INSTRUCTION_MAX
		&& !text
			.chars()
			.any(|c| c.is_control() && c != '\n' && c != '\t')
}

impl Template {
	/// The template of a Duty as it stands, for its Requester to publish:
	/// its title, mission, procedure, scripts and time trigger, the kinds of
	/// its other triggers with empty instructions, and its Agent when
	/// `suggest_agent`. Its Requester, credential, ids, trigger ids and
	/// secrets stay behind: none has a place in a template.
	pub fn draft(definition: &Definition, kinds: &[TriggerKind], suggest_agent: bool) -> Self {
		let mut triggers: Vec<ExpectedTrigger> = definition
			.trigger
			.iter()
			.map(ExpectedTrigger::of_time)
			.collect();
		let slots: BTreeSet<Slot> = kinds.iter().filter_map(Slot::of).collect();
		triggers.extend(slots.into_iter().filter_map(ExpectedTrigger::of_slot));
		Self {
			title: definition.title.trim().to_owned(),
			mission: definition.mission.clone(),
			procedure: definition.procedure.clone(),
			router: definition.router.clone(),
			watcher: definition.watcher.clone(),
			triggers,
			agent: suggest_agent.then(|| SuggestedAgent {
				id: definition.agent.id.clone(),
				version: definition.agent.version.clone(),
			}),
		}
	}

	/// [`Self::draft`] with the publisher's instruction for every trigger.
	/// A note for a trigger the Duty does not have, two notes for one
	/// trigger and a trigger without a note are refused.
	pub fn from_duty(
		definition: &Definition,
		kinds: &[TriggerKind],
		notes: &[TriggerNote],
		suggest_agent: bool,
	) -> Result<Self> {
		let mut template = Self::draft(definition, kinds, suggest_agent);
		let mut noted = BTreeSet::new();
		for note in notes {
			if !noted.insert(note.kind) {
				return Err(Error::Invalid(format!(
					"two instructions for the {} trigger",
					slot_name(note.kind)
				)));
			}
			let trigger = template
				.triggers
				.iter_mut()
				.find(|trigger| trigger.slot() == note.kind)
				.ok_or_else(|| {
					Error::Invalid(format!(
						"the Duty has no {} trigger to write an instruction for",
						slot_name(note.kind)
					))
				})?;
			*trigger.instruction_mut() = note.instruction.trim().to_owned();
		}
		Ok(template)
	}

	/// Checks a template, whether a person publishes it or another Tenant's
	/// package delivers it: the bounds of a Duty's definition, one trigger
	/// of each kind with an instruction, a suggested Agent with an id and a
	/// version, and no credential written into any of its text.
	pub fn validate(&self, now: DateTime<Utc>) -> Result<()> {
		// The same bounds as the Duty it becomes, with a placeholder Agent:
		// the installing person picks the real one.
		self.definition(EntityRef {
			id: "template".into(),
			version: "0.0.0".into(),
		})
		.validate(now)?;
		let mut slots = BTreeSet::new();
		for trigger in &self.triggers {
			if !slots.insert(trigger.slot()) {
				return Err(Error::Invalid(format!(
					"a Duty template lists at most one {} trigger",
					slot_name(trigger.slot())
				)));
			}
			if !instruction_valid(trigger.instruction()) {
				return Err(Error::Invalid(format!(
					"the {} trigger needs an instruction of 1 to {INSTRUCTION_MAX} characters without control characters",
					slot_name(trigger.slot())
				)));
			}
			refuse_secret("trigger instruction", trigger.instruction())?;
		}
		if let Some(agent) = &self.agent {
			crate::policy::identifier(&agent.id)?;
			crate::policy::identifier(&agent.version)?;
		}
		refuse_secret("title", &self.title)?;
		refuse_secret("mission", &self.mission)?;
		refuse_secret("procedure", &self.procedure)?;
		if let Some(router) = &self.router {
			refuse_secret("Router", &router.source)?;
		}
		if let Some(watcher) = &self.watcher {
			refuse_secret("Watcher", &watcher.script.source)?;
		}
		Ok(())
	}

	/// The time trigger the installed Duty starts with.
	pub fn time_trigger(&self) -> Option<Trigger> {
		self.triggers.iter().find_map(ExpectedTrigger::time)
	}

	/// The triggers the installing person creates on the Duty themselves.
	pub fn to_create(&self) -> Vec<ExpectedTrigger> {
		self.triggers
			.iter()
			.filter(|trigger| trigger.slot() != Slot::Time)
			.cloned()
			.collect()
	}

	/// The definition of the Duty it becomes, staffed with `agent`: its
	/// time trigger as given, the Requester's own default Approval Mode and
	/// the default lateness and failure pause.
	pub fn definition(&self, agent: EntityRef) -> Definition {
		Definition {
			task_model: None,
			title: self.title.trim().to_owned(),
			mission: self.mission.clone(),
			procedure: self.procedure.clone(),
			agent,
			trigger: self.time_trigger(),
			approval_mode: None,
			max_lateness_seconds: DEFAULT_MAX_LATENESS_SECONDS,
			pause_after_failures: DEFAULT_PAUSE_AFTER_FAILURES,
			router: self.router.clone(),
			watcher: self.watcher.clone(),
		}
	}

	/// What a newer version of the template changes on a Duty installed from
	/// an older one: its mission, procedure and scripts. The Duty's title,
	/// Agent and triggers stay as its Requester set them.
	pub fn update_patch(&self) -> Patch {
		Patch {
			mission: Some(self.mission.clone()),
			procedure: Some(self.procedure.clone()),
			router: Some(match &self.router {
				Some(router) => ScriptChoice::Set(router.clone()),
				None => ScriptChoice::Remove(super::NoTrigger::None),
			}),
			watcher: Some(match &self.watcher {
				Some(watcher) => WatcherChoice::Set(watcher.clone()),
				None => WatcherChoice::Remove(super::NoTrigger::None),
			}),
			..Patch::default()
		}
	}

	/// The Marketplace package that carries the template. It names no
	/// permissions, dependencies or Tenant Secrets.
	pub fn package(&self, listing: Listing<'_>) -> Result<Package> {
		crate::marketplace::publication::validate_metadata(
			listing.package_id,
			listing.author,
			0,
			0,
		)?;
		semver::Version::parse(listing.version).map_err(|_| {
			Error::Invalid("a template's version is a semantic version such as 1.0.0".into())
		})?;
		let description = listing.description.trim();
		if description.chars().count() > DESCRIPTION_MAX
			|| description
				.chars()
				.any(|c| c.is_control() && c != '\n' && c != '\t')
		{
			return Err(Error::Invalid(format!(
				"a template's description may have at most {DESCRIPTION_MAX} characters without control characters"
			)));
		}
		refuse_secret("description", description)?;
		let description = if description.is_empty() {
			let first = self.mission.trim().lines().next().unwrap_or_default();
			first.chars().take(DESCRIPTION_FROM_MISSION).collect()
		} else {
			description.to_owned()
		};
		Ok(Package {
			entity: Entry {
				binding_normalization: None,
				installation: None,
				id: listing.package_id.to_owned(),
				version: listing.version.to_owned(),
				kind: KIND.to_owned(),
				name: [("en".to_owned(), self.title.trim().to_owned())].into(),
				description: [("en".to_owned(), description)].into(),
				capabilities: vec![],
				tags: vec![],
				languages: vec![],
				skills: vec![],
				schema: serde_json::json!({}),
				config: serde_json::to_value(self)?,
			},
			author: listing.author.to_owned(),
			permissions: vec![],
			dependencies: vec![],
			secrets: vec![],
		})
	}

	/// The template a package carries. A package of another kind, one that
	/// names permissions, dependencies or Tenant Secrets, and a template with
	/// a field it does not know (such as a secret) are refused.
	pub fn from_package(package: &Package) -> Result<Self> {
		if package.entity.kind != KIND {
			return Err(Error::Invalid("the package is not a Duty template".into()));
		}
		if !package.secrets.is_empty()
			|| !package.dependencies.is_empty()
			|| !package.permissions.is_empty()
		{
			return Err(Error::Invalid(
				"a Duty template names no Tenant Secrets, dependencies or permissions".into(),
			));
		}
		serde_json::from_value(package.entity.config.clone())
			.map_err(|error| Error::Invalid(format!("not a valid Duty template: {error}")))
	}
}

fn slot_name(slot: Slot) -> &'static str {
	match slot {
		Slot::Time => "time",
		Slot::Webhook => "webhook",
		Slot::DutyCompleted => "duty_completed",
		Slot::NodeEvent => "node_event",
	}
}

/// The template version a Duty was installed from, so a later version can be
/// offered to its Requester as an update. Updates are never applied by
/// themselves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "DutyTemplateSource")]
pub struct Source {
	/// The Marketplace version's key.
	pub key: String,
	/// The Node that published it.
	pub repository: String,
	pub owner_tenant: String,
	pub package_id: String,
	pub version: String,
	/// The install request that created the Duty, so a retried install
	/// returns the Duty rather than creating another. Never shown.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	#[schemars(skip)]
	pub install_key: Option<Uuid>,
}

impl Source {
	pub fn of(version: &crate::marketplace::Version, install_key: Option<Uuid>) -> Self {
		Self {
			key: version.key.clone(),
			repository: version.repository.clone(),
			owner_tenant: version.owner_tenant.clone(),
			package_id: version.package_id.clone(),
			version: version.version.clone(),
			install_key,
		}
	}

	/// The source as people see it, without its install key.
	pub fn shown(&self) -> Self {
		Self {
			install_key: None,
			..self.clone()
		}
	}

	/// Whether `candidate` is a later version of the same template: the same
	/// package of the same publisher on the same Node, with a greater
	/// semantic version.
	pub fn superseded_by(&self, candidate: &crate::marketplace::Version) -> bool {
		candidate.kind == KIND && self.precedes(&Self::of(candidate, None))
	}

	/// Whether `later` is a later version of the same template.
	pub fn precedes(&self, later: &Self) -> bool {
		if later.repository != self.repository
			|| later.owner_tenant != self.owner_tenant
			|| later.package_id != self.package_id
		{
			return false;
		}
		match (
			semver::Version::parse(&self.version),
			semver::Version::parse(&later.version),
		) {
			(Ok(current), Ok(next)) => next > current,
			_ => false,
		}
	}
}

#[cfg(test)]
mod tests;
