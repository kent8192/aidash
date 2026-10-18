//! How Tenants come to exist and who administers them: Tenant names, Tenant
//! labels, the deployment-wide Tenancy Rules, the initial policy of a Tenant
//! created through Aidash, and the conservative last-administrator check.
use crate::identity::tenant_administration::{self as administration, COLLECTION};
use crate::policy::{
	Effect, Evaluation, Group, Policy, PolicyBundle, Resource, ResourceSelector, Subject,
	SubjectKind, SubjectSelector, identifier,
};
use crate::{Error, Result};
use chrono::{DateTime, Duration, Utc};
use serde_json::json;
use std::collections::BTreeSet;

mod words;

/// The subject a Tenant Creator is mapped to.
pub const OWNER: &str = "owner";
/// The Assignable Group whose members may do everything within the Tenant.
pub const ADMINISTRATORS: &str = "administrators";
/// The Assignable Group whose members use the Tenant without administering it.
pub const MEMBERS: &str = "members";
/// Changing the Tenant label.
pub const TENANT_UPDATE: &str = "tenant.update";

/// What every ordinary member may do: use the Tenant and author definitions,
/// never administer people, change Tenant settings, approve use, hold Provider
/// Credentials, disclose research or administer Federation. An explicit list,
/// because deny rules would make the last-administrator check unreliable.
pub const MEMBER_ACTIONS: &[&str] = &[
	"agent.configure",
	"agent.execute",
	"agent_dependency.read",
	"agent_draft.archive",
	"agent_draft.create",
	"agent_draft.read",
	"agent_draft.register",
	"agent_draft.share",
	"agent_draft.test",
	"agent_draft.transfer",
	"agent_draft.write",
	"agent_version.inspect",
	"artifact.create",
	"artifact.read",
	"capability.request",
	"chatgpt.tokens.use.direct",
	"cluster.execute",
	"conversation.create",
	"conversation.read",
	"decision.evidence.read",
	"decision.invoke",
	"decision.state.read",
	"embedding.invoke",
	"file.delete",
	"file.read",
	"file.receive",
	"file.restore",
	"file.share",
	"file.transfer",
	"file.write",
	"generation.read",
	"generation.request",
	"generation.stop",
	"human.answer",
	"human.read",
	"human.request",
	"installation.create",
	"installation.read",
	"marketplace.browse",
	"marketplace.install",
	"marketplace.read",
	"memory.candidate.propose",
	"memory.candidate.read",
	"memory.derive",
	"memory.history.read",
	"memory.invoke",
	"memory.participant.create",
	"memory.participant.use",
	"memory.pin",
	"memory.read",
	"memory.reflect",
	"memory.use",
	"memory.write",
	"message.create",
	"message.read",
	"model.infer",
	"registry.create",
	"registry.read",
	"run.control",
	"run.message",
	"run.read",
	"semantic.read",
	"semantic.search",
	"semantic.use",
	"semantic.write",
	"skill.import",
	"skill.use",
	"task.abandon",
	"task.claim",
	"task.complete",
	"task.create",
	"task.delegate",
	"task.execute",
	"task.read",
	"thread.delete",
	"tool.invoke",
	"workspace.create",
	"workspace.events",
	"workspace.read",
	"workspace.share",
	"workspace.update",
];

const RANDOM_LENGTH: usize = 8;
const ALPHABET: &[u8; 36] = b"abcdefghijklmnopqrstuvwxyz0123456789";
const LABEL_MAX: usize = 64;

/// The identifier Aidash assigns to a Tenant: `<adjective>-<noun>-<8 random
/// characters>`. Nobody chooses it, it never changes and it is never reused.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TenantName(String);

impl TenantName {
	/// Draws a name from 16 bytes of caller-supplied entropy, such as a random
	/// UUID, so this crate stays free of a random source.
	pub fn from_entropy(entropy: [u8; 16]) -> Self {
		let adjective = words::ADJECTIVES[usize::from(entropy[0]) % words::ADJECTIVES.len()];
		let noun = words::NOUNS[usize::from(entropy[1]) % words::NOUNS.len()];
		let mut random = u64::from_le_bytes(entropy[2..10].try_into().expect("eight bytes"))
			^ u64::from(u32::from_le_bytes(
				entropy[10..14].try_into().expect("four bytes"),
			));
		let mut suffix = String::with_capacity(RANDOM_LENGTH);
		for _ in 0..RANDOM_LENGTH {
			suffix.push(char::from(ALPHABET[(random % 36) as usize]));
			random /= 36;
		}
		Self(format!("{adjective}-{noun}-{suffix}"))
	}

	pub fn as_str(&self) -> &str {
		&self.0
	}

	pub fn into_string(self) -> String {
		self.0
	}
}

impl std::fmt::Display for TenantName {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str(&self.0)
	}
}

/// A Tenant label as stored: trimmed, 1–64 characters, without control
/// characters. It never identifies or routes to a Tenant.
pub fn label(value: &str) -> Result<String> {
	let value = value.trim();
	if value.is_empty() || value.chars().count() > LABEL_MAX || value.chars().any(char::is_control)
	{
		return Err(Error::Invalid(
			"a Tenant label needs 1 to 64 characters without control characters".into(),
		));
	}
	Ok(value.to_owned())
}

/// The label of a Personal Tenant until its administrators change it. It is
/// never taken from Display Attributes.
pub const PERSONAL_LABEL: &str = "Personal";

/// The deployment-wide rules for how Tenants come to exist and how people join
/// them. The defaults are the Self-hosted Offering's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenancyRules {
	pub personal_tenants: bool,
	pub tenant_creation: bool,
	pub creation_per_hour: u32,
	pub creation_total: u32,
	pub invitations: bool,
}

impl Default for TenancyRules {
	fn default() -> Self {
		Self {
			personal_tenants: false,
			tenant_creation: false,
			creation_per_hour: 5,
			creation_total: 20,
			invitations: true,
		}
	}
}

/// Why an External Identity may not create a Tenant now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreationRefusal {
	/// The Tenancy Rules do not let External Identities create Tenants, or this
	/// identity's Tenant Binding forbids it.
	NotAllowed,
	/// Too many creations in the last hour; a slot frees after this many seconds.
	HourlyLimit { retry_after_seconds: i64 },
	/// Too many Tenants created by this identity that are not purged.
	TotalLimit,
}

/// Checks the creation limits for one External Identity. `recent` holds the
/// creation times of its Tenants within the last hour, `total` the number of
/// its Tenants that are not purged. Deployment Administrators are never checked.
pub fn check_creation(
	rules: &TenancyRules,
	now: DateTime<Utc>,
	recent: &[DateTime<Utc>],
	total: u32,
) -> std::result::Result<(), CreationRefusal> {
	if !rules.tenant_creation {
		return Err(CreationRefusal::NotAllowed);
	}
	if total >= rules.creation_total {
		return Err(CreationRefusal::TotalLimit);
	}
	let window = Duration::hours(1);
	let mut recent: Vec<_> = recent
		.iter()
		.copied()
		.filter(|time| *time > now - window)
		.collect();
	recent.sort();
	let limit = rules.creation_per_hour as usize;
	if limit == 0 {
		return Err(CreationRefusal::NotAllowed);
	}
	if recent.len() >= limit {
		// The creation that has to leave the window before another fits.
		let leaving = recent[recent.len() - limit];
		let seconds = (leaving + window - now).num_seconds().max(1);
		return Err(CreationRefusal::HourlyLimit {
			retry_after_seconds: seconds,
		});
	}
	Ok(())
}

/// The policy every Tenant created through Aidash starts with. Its Tenant
/// Creator, if any, is the subject `owner` in `administrators`, which may do
/// everything; both groups are Assignable, so Tenant Administrators can
/// appoint each other and add members without a Deployment Administrator.
pub fn initial_policy(tenant: &str, creator: bool) -> Result<PolicyBundle> {
	identifier(tenant)?;
	let assignable = || Group {
		roles: BTreeSet::new(),
		assignable: true,
	};
	let group_policy = |id: &str, group: &str, actions: BTreeSet<String>| Policy {
		id: id.into(),
		effect: Effect::Allow,
		subjects: SubjectSelector {
			groups: [group.to_owned()].into(),
			..Default::default()
		},
		actions,
		resources: ResourceSelector {
			kinds: ["*".to_owned()].into(),
			ids: BTreeSet::new(),
		},
		condition: None,
	};
	let bundle = PolicyBundle {
		tenant: tenant.into(),
		subjects: if creator {
			[(
				OWNER.to_owned(),
				Subject {
					kind: SubjectKind::User,
					groups: [ADMINISTRATORS.to_owned()].into(),
					roles: BTreeSet::new(),
					attributes: json!({}),
					enabled: true,
					delegated_by: None,
				},
			)]
			.into()
		} else {
			Default::default()
		},
		groups: [
			(ADMINISTRATORS.to_owned(), assignable()),
			(MEMBERS.to_owned(), assignable()),
		]
		.into(),
		roles: Default::default(),
		policies: vec![
			group_policy(ADMINISTRATORS, ADMINISTRATORS, ["*".to_owned()].into()),
			group_policy(
				MEMBERS,
				MEMBERS,
				MEMBER_ACTIONS
					.iter()
					.map(|action| (*action).to_owned())
					.collect(),
			),
		],
	};
	bundle.validate()?;
	Ok(bundle)
}

/// Whether one of `subjects` keeps every administration action without any
/// condition. Judged conservatively: allows that carry a condition or are
/// limited to particular resources do not count, and every deny counts as
/// matching. An error therefore refuses a change that might have been safe, and
/// never allows one that leaves the Tenant without an administrator.
pub fn has_unconditional_administrator<'a>(
	bundle: &PolicyBundle,
	subjects: impl IntoIterator<Item = &'a str>,
) -> bool {
	let mut conservative = bundle.clone();
	conservative.policies.retain(|policy| match policy.effect {
		Effect::Allow => {
			policy.condition.is_none()
				&& (policy.resources.ids.is_empty() || policy.resources.ids.contains("*"))
		}
		Effect::Deny => true,
	});
	for policy in &mut conservative.policies {
		if policy.effect == Effect::Deny {
			policy.condition = None;
			policy.resources.ids = BTreeSet::new();
		}
	}
	subjects.into_iter().any(|subject| {
		administration::ADMINISTRATION_ACTIONS.iter().all(|action| {
			conservative
				.evaluate(&Evaluation {
					subject: subject.into(),
					action: (*action).into(),
					resource: Resource {
						tenant: bundle.tenant.clone(),
						kind: administration::resource_kind(action).into(),
						id: COLLECTION.into(),
						attributes: json!({}),
					},
					environment: json!({}),
				})
				.allowed
		})
	})
}

/// Invitation lifetimes: 7 days unless the issuer chooses a shorter one.
pub mod invitation {
	use super::*;

	pub const DEFAULT_SECONDS: i64 = 7 * 24 * 60 * 60;
	pub const MIN_SECONDS: i64 = 60;
	/// The most Invitations one Tenant may have active at once.
	pub const MAX_ACTIVE: usize = 100;

	pub fn lifetime(requested_seconds: Option<i64>) -> Result<Duration> {
		let seconds = requested_seconds.unwrap_or(DEFAULT_SECONDS);
		if !(MIN_SECONDS..=DEFAULT_SECONDS).contains(&seconds) {
			return Err(Error::Invalid(format!(
				"an Invitation lasts between {MIN_SECONDS} and {DEFAULT_SECONDS} seconds"
			)));
		}
		Ok(Duration::seconds(seconds))
	}
}

#[cfg(test)]
mod tests;
