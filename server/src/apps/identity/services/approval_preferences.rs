//! A person's Approval Mode and Approval Rules over HTTP (#411, ADR 0076).
//! Only the owner writes them, acting through a dashboard or desktop session
//! of their own Mapping; a Subject Credential never can. Tenant Administrators
//! with `approval.read` read a member's. The Tenant's policy bounds what is
//! stored: a mode it does not allow cannot be chosen, and a rule beyond it is
//! kept but reported as not effective.
use crate::apps::identity::models::ApprovalPreference;
use crate::apps::identity::models::{AdministrationActor, AdministrationEvent};
use crate::apps::identity::oidc::BrowserOrigin;
use crate::apps::identity::repositories::approval_decider::{self as decider, Availability};
use crate::apps::identity::repositories::approval_preferences as repository;
use crate::apps::identity::serializers::approval_preferences::{
	ApprovalDecider, ApprovalDeciderUpdate, DeciderReference, ModeChoice, ModeReason, Preferences,
	PreferencesUpdate, Rule, RuleInput, Rules, Settings, ThreadMode, ThreadModeUpdate,
};
use crate::apps::identity::services::tenant_administration::TenantAdministrations;
use crate::authorization::{
	access::{Access, NativeAccess},
	identity::Actor,
};
use crate::federation::Federation;
use crate::{Error, Result};
use aidash_application::capabilities::approvals::{CALL_UNAPPROVED, DISCLOSE, INVOKE};
use aidash_domain::approval_rules::{
	ApprovalKind, ApprovalMode, ApprovalRule, MODE_RESOURCE, RuleEffect, RuleTarget,
};
use aidash_domain::capabilities::approvals::approver_eligible;
use aidash_domain::identity::tenancy::TENANT_UPDATE;
use aidash_domain::identity::tenant_administration::APPROVAL_READ;
use aidash_domain::policy::{Evaluation, PolicyBundle, Resource, identifier};
use aidash_domain::registry::EntityRef;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::injectable;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use uuid::Uuid;

#[derive(Clone)]
pub struct ApprovalPreferences {
	pub(crate) runtime: Federation,
}

#[injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> ApprovalPreferences {
	ApprovalPreferences { runtime }
}

/// The modes, in the order they are offered.
const MODES: [ApprovalMode; 4] = [
	ApprovalMode::Manual,
	ApprovalMode::Auto,
	ApprovalMode::DontAsk,
	ApprovalMode::Bypass,
];

/// Whether `bundle` allows `action` on the resource to `subject`, judged for
/// that subject alone and not recorded: the ceiling only shapes what a
/// response offers.
fn permits(bundle: &PolicyBundle, subject: &str, resource: Resource, action: &str) -> bool {
	bundle
		.evaluate(&Evaluation {
			subject: subject.into(),
			action: action.into(),
			resource,
			environment: json!({"transport": "api"}),
		})
		.allowed
}

fn resource(bundle: &PolicyBundle, kind: &str, id: &str, attributes: Value) -> Resource {
	Resource {
		tenant: bundle.tenant.clone(),
		kind: kind.into(),
		id: id.into(),
		attributes,
	}
}

/// Whether the Tenant's policy lets `subject` use `mode`.
fn mode_allowed(bundle: &PolicyBundle, subject: &str, mode: ApprovalMode) -> bool {
	mode.policy_requirement().is_none_or(|(action, id)| {
		permits(
			bundle,
			subject,
			resource(bundle, MODE_RESOURCE, id, json!({})),
			action,
		)
	})
}

/// What the Tenant's policy and this Node say about one mode for one person.
/// `accepted` is whether they accepted the risk of Bypass, and `auto` whether
/// the Tenant's approval Decider can judge here.
fn mode_choice(
	bundle: &PolicyBundle,
	subject: &str,
	mode: ApprovalMode,
	auto: Availability,
	accepted: bool,
) -> ModeChoice {
	let (allowed, reason) = if !mode_allowed(bundle, subject, mode) {
		(false, Some(ModeReason::Policy))
	} else {
		match (mode, auto) {
			(ApprovalMode::Auto, Availability::Unapproved) => {
				(false, Some(ModeReason::DeciderUnapproved))
			}
			(ApprovalMode::Auto, Availability::Missing) => (false, Some(ModeReason::NoDecider)),
			(ApprovalMode::Bypass, _) if !accepted => (true, Some(ModeReason::NotAccepted)),
			_ => (true, None),
		}
	};
	ModeChoice {
		mode,
		allowed,
		reason,
	}
}

fn modes(
	bundle: &PolicyBundle,
	subject: &str,
	auto: Availability,
	accepted: bool,
) -> Vec<ModeChoice> {
	MODES
		.into_iter()
		.map(|mode| mode_choice(bundle, subject, mode, auto, accepted))
		.collect()
}

/// Whether the Tenant's approval Decider can judge on this Node now.
async fn auto_availability(
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	tenant: &str,
) -> Result<Availability> {
	let chosen = decider::chosen(tx, tenant).await?;
	decider::availability(tx, tenant, &chosen).await
}

/// The id of the representative resource a rule that names no one target is
/// judged on: a policy that allows a kind without naming ids allows it.
const REPRESENTATIVE: &str = "any";

/// The resource a rule's approval would be judged on, and the policy action
/// that approving it needs, as far as the rule alone tells. A rule for every
/// target of a kind, and a search rule (whose destination only a Run knows),
/// are judged on `id`: a real target the policy names, or [`REPRESENTATIVE`]
/// when the policy allows the whole kind.
fn judged(
	bundle: &PolicyBundle,
	owner: &str,
	rule: &ApprovalRule,
	id: &str,
) -> (Resource, &'static str) {
	let exact = match &rule.target {
		RuleTarget::Exact(target) => Some(target.as_str()),
		RuleTarget::Every => None,
	};
	match rule.kind {
		ApprovalKind::ToolCall => {
			let (tool, operation) = match exact {
				Some(target) => target
					.strip_prefix("tool:")
					.and_then(|target| target.split_once('#'))
					.unwrap_or((REPRESENTATIVE, INVOKE)),
				None => (id, INVOKE),
			};
			(
				resource(
					bundle,
					"tool",
					tool,
					json!({"operation": operation, "action": CALL_UNAPPROVED}),
				),
				"capability.approve",
			)
		}
		ApprovalKind::Sampling => {
			let raw = exact
				.and_then(|target| target.strip_prefix("sampling:"))
				.and_then(|raw| serde_json::from_str::<Vec<Value>>(raw).ok());
			let logical = raw
				.filter(|parts| parts.len() >= 3)
				.map(|parts| format!("sampling:{}", json!(&parts[..3])))
				.unwrap_or_else(|| id.into());
			(
				resource(
					bundle,
					"sampling",
					&logical,
					json!({"action":"sampling.request_unapproved"}),
				),
				"capability.approve",
			)
		}
		ApprovalKind::Outbound => {
			let origin = exact.unwrap_or(id);
			(
				resource(
					bundle,
					"outbound",
					origin,
					json!({"origin": origin, "action": "network.get"}),
				),
				"capability.approve",
			)
		}
		ApprovalKind::WebSearch => {
			// A search rule names its provider, while its ceiling is judged on
			// the destination a Run chooses.
			let destination = id;
			(
				resource(
					bundle,
					"research",
					destination,
					json!({"destination": destination, "action": DISCLOSE}),
				),
				"capability.approve",
			)
		}
		ApprovalKind::PageReading => {
			let host = exact.unwrap_or(id);
			(
				resource(
					bundle,
					"research",
					host,
					json!({"destination": host, "action": DISCLOSE}),
				),
				"capability.approve",
			)
		}
		ApprovalKind::Generation => {
			let policy = exact.unwrap_or(id);
			// Settings are read before a Task exists: the owner is the root
			// subject, and a nil task id keeps existence conditions evaluable.
			let mut attributes = json!({
				"root_subject": owner,
				"task_id": Uuid::nil(),
			});
			if policy != REPRESENTATIVE {
				attributes["policy_id"] = json!(policy);
			}
			(
				resource(bundle, "generation", REPRESENTATIVE, attributes),
				"generation.approve",
			)
		}
	}
}

/// The real target ids the Tenant's policy names for a rule that covers every
/// target of its kind. The placeholder is not one of them.
fn named_targets(bundle: &PolicyBundle, kind: ApprovalKind) -> BTreeSet<String> {
	let resource_kind = match kind {
		ApprovalKind::ToolCall => "tool",
		ApprovalKind::Sampling => "sampling",
		ApprovalKind::Outbound => "outbound",
		ApprovalKind::WebSearch | ApprovalKind::PageReading => "research",
		ApprovalKind::Generation => "generation",
	};
	bundle
		.policies
		.iter()
		.filter(|policy| {
			policy.resources.kinds.contains("*") || policy.resources.kinds.contains(resource_kind)
		})
		.flat_map(|policy| policy.resources.ids.iter())
		.filter(|id| id.as_str() != REPRESENTATIVE)
		.cloned()
		.collect()
}

/// Whether `rule` still has its effect. A deny or ask rule relaxes nothing, so
/// the ceiling never reaches it. An allow rule is an approval given in advance:
/// `owner` could approve what it names themselves and the policy lets them
/// advance approvals (ADR 0076).
///
/// A rule for every target of a kind is judged on each real target the policy
/// names as well as on [`REPRESENTATIVE`]: a policy that permits approval only
/// for selected ids still lets the rule approve those, so it is not inactive
/// merely because the invented id is not one of them.
fn effective(
	bundle: &PolicyBundle,
	owner: &str,
	rule: &ApprovalRule,
	templates: &[(String, aidash_domain::registry::Entry)],
) -> bool {
	if rule.effect != RuleEffect::Allow {
		return true;
	}
	let mut candidates = BTreeSet::new();
	match &rule.target {
		RuleTarget::Exact(_) => {
			candidates.insert(REPRESENTATIVE.to_owned());
			// A search rule names its provider, while the ceiling it needs is
			// judged on a destination the policy may name by id.
			if rule.kind == ApprovalKind::WebSearch {
				candidates.extend(named_targets(bundle, rule.kind));
			}
		}
		RuleTarget::Every => {
			candidates.insert(REPRESENTATIVE.to_owned());
			candidates.extend(named_targets(bundle, rule.kind));
			if rule.kind == ApprovalKind::Generation {
				candidates.extend(templates.iter().map(|(id, _)| id.clone()));
			}
		}
	}
	candidates
		.into_iter()
		.any(|target| effective_for(bundle, owner, rule, &target, templates))
}

fn effective_for(
	bundle: &PolicyBundle,
	owner: &str,
	rule: &ApprovalRule,
	target: &str,
	templates: &[(String, aidash_domain::registry::Entry)],
) -> bool {
	let (protected, action) = judged(bundle, owner, rule, target);
	let eligible = if action == "capability.approve" {
		approver_eligible(
			bundle,
			&Evaluation {
				subject: owner.into(),
				action: action.into(),
				resource: protected.clone(),
				environment: json!({"transport": "api"}),
			},
			owner,
		)
	} else {
		permits(bundle, owner, protected.clone(), action)
	};
	if !eligible
		|| !permits(
			bundle,
			owner,
			protected,
			aidash_domain::approval_rules::APPROVAL_ADVANCE,
		) {
		return false;
	}
	if rule.kind != ApprovalKind::Generation {
		return true;
	}
	// Generation admission approves the request and the definition the policy
	// would generate: `catalog.approve` on that exact template is required too.
	let policy = match &rule.target {
		RuleTarget::Exact(policy) => policy.as_str(),
		RuleTarget::Every => target,
	};
	let Some((_, template)) = templates.iter().find(|(id, _)| id == policy) else {
		return false;
	};
	permits(
		bundle,
		owner,
		resource(
			bundle,
			"registry",
			&format!("{}/{}@{}", template.kind, template.id, template.version),
			aidash_application::authorization::catalog::attributes(template),
		),
		"catalog.approve",
	)
}

async fn view_rules(
	tx: &mut dyn TransactionExecutor,
	bundle: &PolicyBundle,
	owner: &str,
	rules: Vec<(ApprovalRule, chrono::DateTime<chrono::Utc>)>,
) -> Result<Vec<Rule>> {
	let templates = if rules
		.iter()
		.any(|(rule, _)| rule.kind == ApprovalKind::Generation && rule.effect == RuleEffect::Allow)
	{
		repository::generation_templates(tx, &bundle.tenant).await?
	} else {
		Vec::new()
	};
	Ok(rules
		.into_iter()
		.map(|(rule, created_at)| Rule {
			effective: effective(bundle, owner, &rule, &templates),
			id: rule.id,
			kind: rule.kind,
			target: rule.target,
			effect: rule.effect,
			created_at,
		})
		.collect())
}

fn view_preferences(
	bundle: &PolicyBundle,
	owner: &str,
	row: Option<&ApprovalPreference>,
	auto: Availability,
) -> Result<Preferences> {
	let bypass_accepted_at = row.and_then(|row| row.bypass_accepted_at);
	Ok(Preferences {
		default_mode: repository::default_mode(row)?,
		answer_deadline_seconds: row.and_then(|row| row.answer_deadline_seconds),
		bypass_accepted_at,
		revision: row.map_or(0, |row| row.revision),
		modes: modes(bundle, owner, auto, bypass_accepted_at.is_some()),
	})
}

/// The Tenant's approval Decider and what this Node says about it.
async fn view_decider(
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	tenant: &str,
) -> Result<ApprovalDecider> {
	let chosen = decider::chosen(tx, tenant).await?;
	Ok(ApprovalDecider {
		registered: decider::registered(tx, &chosen.reference).await?.is_some(),
		approved: decider::approved(tx, tenant, &chosen.reference).await?,
		built_in: chosen.built_in,
		revision: chosen.revision,
		decider: DeciderReference {
			id: chosen.reference.id,
			version: chosen.reference.version,
		},
	})
}

impl ApprovalPreferences {
	fn administrations(&self) -> TenantAdministrations {
		TenantAdministrations {
			runtime: self.runtime.clone(),
		}
	}

	pub(crate) async fn preferences(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
	) -> Result<Preferences> {
		let administrations = self.administrations();
		let administrator = administrations
			.administrator(actor, origin, &tenant)
			.await?;
		let result = self
			.runtime
			.store
			.pool
			.atomic(&administrator.scope()?, async |tx| {
				let snapshot = &administrator.lock(tx, false).await?;
				let subject = &administrator.identity.subject;
				let row = repository::preference(tx, &tenant, subject).await?;
				let auto = auto_availability(tx, &tenant).await?;
				view_preferences(&snapshot.bundle, subject, row.as_ref(), auto)
			})
			.await;
		administrations.finish(&administrator, result).await
	}

	pub(crate) async fn update_preferences(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		input: PreferencesUpdate,
	) -> Result<Preferences> {
		input.validate()?;
		let administrations = self.administrations();
		let administrator = administrations
			.administrator(actor, origin, &tenant)
			.await?;
		let result = self
			.runtime
			.store
			.pool
			.atomic(&administrator.scope()?, async |tx| {
				let snapshot = &administrator.lock(tx, false).await?;
				let subject = &administrator.identity.subject;
				repository::lock_owner(tx, &tenant, subject).await?;
				// A mode the policy does not name is refused, and the refusal recorded.
				if let Some((action, id)) = input.default_mode.policy_requirement() {
					administrator
						.require(tx, snapshot, action, id, json!({}))
						.await?;
				}
				let row = repository::save_preference(
					tx,
					&tenant,
					subject,
					input.expected_revision,
					&repository::PreferenceWrite {
						mode: input.default_mode,
						answer_deadline_seconds: input.answer_deadline_seconds,
						accept_bypass: input.accept_bypass,
					},
				)
				.await?;
				let auto = auto_availability(tx, &tenant).await?;
				view_preferences(&snapshot.bundle, subject, Some(&row), auto)
			})
			.await;
		administrations.finish(&administrator, result).await
	}

	pub(crate) async fn rules(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
	) -> Result<Rules> {
		let administrations = self.administrations();
		let administrator = administrations
			.administrator(actor, origin, &tenant)
			.await?;
		let result = self
			.runtime
			.store
			.pool
			.atomic(&administrator.scope()?, async |tx| {
				let snapshot = &administrator.lock(tx, false).await?;
				let subject = &administrator.identity.subject;
				let rules = repository::rules(tx, &tenant, subject).await?;
				Ok(Rules {
					rules: view_rules(tx, &snapshot.bundle, subject, rules).await?,
				})
			})
			.await;
		administrations.finish(&administrator, result).await
	}

	pub(crate) async fn add_rule(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		input: RuleInput,
	) -> Result<Rule> {
		input.validate()?;
		let administrations = self.administrations();
		let administrator = administrations
			.administrator(actor, origin, &tenant)
			.await?;
		let result = self
			.runtime
			.store
			.pool
			.atomic(&administrator.scope()?, async |tx| {
				let snapshot = &administrator.lock(tx, false).await?;
				let subject = &administrator.identity.subject;
				repository::lock_owner(tx, &tenant, subject).await?;
				let rule = ApprovalRule {
					id: Uuid::new_v4(),
					kind: input.kind,
					target: input.target,
					effect: input.effect,
				};
				let stored_id = repository::add_rule(tx, &tenant, subject, &rule).await?;
				if stored_id != rule.id {
					return Err(Error::Conflict("the Approval Rule already exists".into()));
				}
				let stored = repository::rules(tx, &tenant, subject)
					.await?
					.into_iter()
					.find(|(stored, _)| stored.id == stored_id)
					.ok_or_else(|| Error::Invalid("the Approval Rule is unavailable".into()))?;
				view_rules(tx, &snapshot.bundle, subject, vec![stored])
					.await?
					.pop()
					.ok_or_else(|| Error::Invalid("the Approval Rule is unavailable".into()))
			})
			.await;
		administrations.finish(&administrator, result).await
	}

	pub(crate) async fn delete_rule(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		id: Uuid,
	) -> Result<()> {
		let administrations = self.administrations();
		let administrator = administrations
			.administrator(actor, origin, &tenant)
			.await?;
		let result = self
			.runtime
			.store
			.pool
			.atomic(&administrator.scope()?, async |tx| {
				administrator.lock(tx, false).await?;
				let subject = &administrator.identity.subject;
				repository::lock_owner(tx, &tenant, subject).await?;
				if repository::delete_rule(tx, &tenant, subject, id).await? {
					Ok(())
				} else {
					Err(Error::NotFound("Approval Rule".into()))
				}
			})
			.await;
		administrations.finish(&administrator, result).await
	}

	/// A member's settings, for a Tenant Administrator with `approval.read`.
	/// Administrators change none of it; they change only the ceiling.
	pub(crate) async fn settings_of(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		subject: String,
	) -> Result<Settings> {
		identifier(&subject)?;
		let administrations = self.administrations();
		let administrator = administrations
			.administrator(actor, origin, &tenant)
			.await?;
		let result = self
			.runtime
			.store
			.pool
			.atomic(&administrator.scope()?, async |tx| {
				let snapshot = &administrator.lock(tx, false).await?;
				administrator
					.require(tx, snapshot, APPROVAL_READ, &subject, json!({}))
					.await?;
				if !snapshot.bundle.subjects.contains_key(&subject) {
					return Err(Error::NotFound("subject".into()));
				}
				let row = repository::preference(tx, &tenant, &subject).await?;
				let rules = repository::rules(tx, &tenant, &subject).await?;
				let auto = auto_availability(tx, &tenant).await?;
				let preferences = view_preferences(&snapshot.bundle, &subject, row.as_ref(), auto)?;
				Ok(Settings {
					default_mode: preferences.default_mode,
					answer_deadline_seconds: preferences.answer_deadline_seconds,
					bypass_accepted_at: preferences.bypass_accepted_at,
					revision: preferences.revision,
					rules: view_rules(tx, &snapshot.bundle, &subject, rules).await?,
					subject,
				})
			})
			.await;
		administrations.finish(&administrator, result).await
	}

	/// The Tenant's approval Decider, for any member.
	pub(crate) async fn decider(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
	) -> Result<ApprovalDecider> {
		identifier(&tenant)?;
		let mut access = self.session(actor, origin).await?;
		let result = async {
			if access.identity.tenant != tenant {
				return Err(Error::Forbidden);
			}
			view_decider(&mut *access.tx, &tenant).await
		}
		.await;
		access.finish(result).await
	}

	/// Names the Tenant's approval Decider, or with `null` resets it to the
	/// built-in, under `tenant.update`. Only a Decider of hook `approval` that
	/// the Tenant Registry holds can be named.
	pub(crate) async fn set_decider(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		input: ApprovalDeciderUpdate,
	) -> Result<ApprovalDecider> {
		input.validate()?;
		identifier(&tenant)?;
		let (Actor::Subject(identity), Some(origin)) = (actor, origin) else {
			return Err(Error::Forbidden);
		};
		let mapping = origin.mapping_id.ok_or(Error::Forbidden)?;
		if identity.tenant != tenant {
			return Err(Error::Forbidden);
		}
		identity.check_binding(&self.runtime.store.pool).await?;
		let mut access = NativeAccess::begin(&self.runtime.store, &identity).await?;
		let resource = Resource {
			tenant: tenant.clone(),
			kind: "tenant".into(),
			id: tenant.clone(),
			attributes: json!({}),
		};
		if let Err(error) = access.require(&resource, TENANT_UPDATE).await {
			return access.finish(Err(error)).await;
		}
		let result = async {
			let (tx, _) = access.tenant_transaction();
			decider::lock(tx, &tenant).await?;
			let target = input.decider.map(|decider| EntityRef {
				id: decider.id,
				version: decider.version,
			});
			if let Some(target) = &target
				&& decider::registered(tx, target).await?.is_none()
			{
				return Err(Error::Invalid(
					"the Decider is not a registered Decider of hook approval".into(),
				));
			}
			decider::save(tx, &tenant, input.expected_revision, target.as_ref()).await?;
			AdministrationActor::Identity {
				identity: origin.identity_id,
				mapping: Some(mapping),
			}
			.record(
				tx,
				AdministrationEvent {
					action: TENANT_UPDATE,
					identity: None,
					registration: None,
					mapping: None,
					tenant: Some(&tenant),
					subject: None,
					invitation: None,
				},
			)
			.await?;
			view_decider(tx, &tenant).await
		}
		.await;
		access.finish(result).await
	}

	/// The caller's own session: a dashboard or desktop session of their Mapping.
	async fn session(&self, actor: Actor, origin: Option<BrowserOrigin>) -> Result<Access> {
		let (Actor::Subject(identity), Some(origin)) = (actor, origin) else {
			return Err(Error::Forbidden);
		};
		origin.mapping_id.ok_or(Error::Forbidden)?;
		identity.check_binding(&self.runtime.store.pool).await?;
		Access::begin(&self.runtime.store, &identity).await
	}

	/// The caller's mode for one Thread, when they can read the Thread.
	pub(crate) async fn thread_mode(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		workspace: Uuid,
		thread: Uuid,
	) -> Result<ThreadMode> {
		let mut access = self.session(actor, origin).await?;
		let result = async {
			crate::apps::execution::capabilities::services::endpoints::readable_thread(
				&mut access,
				workspace,
				thread,
			)
			.await?;
			let (tenant, subject) = (
				access.identity.tenant.clone(),
				access.identity.subject.clone(),
			);
			let row = repository::preference(&mut *access.tx, &tenant, &subject).await?;
			let auto = auto_availability(&mut *access.tx, &tenant).await?;
			Ok(ThreadMode {
				mode: repository::thread_mode(&mut *access.tx, &tenant, &subject, thread).await?,
				default_mode: repository::default_mode(row.as_ref())?,
				modes: modes(
					&access.snapshot.bundle,
					&subject,
					auto,
					row.as_ref()
						.is_some_and(|row| row.bypass_accepted_at.is_some()),
				),
			})
		}
		.await;
		access.finish(result).await
	}

	/// Sets, or with `null` clears, the caller's mode for one Thread. A running
	/// Run of the caller in that Thread uses it at its next approval.
	pub(crate) async fn set_thread_mode(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		workspace: Uuid,
		thread: Uuid,
		input: ThreadModeUpdate,
	) -> Result<ThreadMode> {
		let mut access = self.session(actor, origin).await?;
		let result = async {
			crate::apps::execution::capabilities::services::endpoints::readable_thread(
				&mut access,
				workspace,
				thread,
			)
			.await?;
			let (tenant, subject) = (
				access.identity.tenant.clone(),
				access.identity.subject.clone(),
			);
			repository::lock_owner(&mut *access.tx, &tenant, &subject).await?;
			let row = repository::preference(&mut *access.tx, &tenant, &subject).await?;
			if let Some(mode) = input.mode {
				if let Some((action, id)) = mode.policy_requirement() {
					let target = access.resource(MODE_RESOURCE, id, json!({}));
					access.require(&target, action).await?;
				}
				if mode == ApprovalMode::Bypass
					&& row
						.as_ref()
						.is_none_or(|row| row.bypass_accepted_at.is_none())
				{
					return Err(Error::Invalid(
						"accept the risk of Bypass in your preferences first".into(),
					));
				}
			}
			repository::set_thread_mode(&mut *access.tx, &tenant, &subject, thread, input.mode)
				.await?;
			let auto = auto_availability(&mut *access.tx, &tenant).await?;
			Ok(ThreadMode {
				mode: input.mode,
				default_mode: repository::default_mode(row.as_ref())?,
				modes: modes(
					&access.snapshot.bundle,
					&subject,
					auto,
					row.as_ref()
						.is_some_and(|row| row.bypass_accepted_at.is_some()),
				),
			})
		}
		.await;
		access.finish(result).await
	}
}

#[cfg(test)]
mod tests;
