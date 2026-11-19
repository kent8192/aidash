//! Duty Triggers other than the clock and the events they bring (#1011, ADR
//! 0142). A webhook event and another Duty's Task ending are input, never a
//! requester: each starts or wakes a Task that acts for the Duty's requester,
//! and its content reaches the Agent as data.
use super::access::Lease;
use super::duties::{Adapter, start_with};
use crate::apps::workspaces::repositories::{duties as duty_repository, duty_events as repository};
use crate::{
	Error, Result,
	authorization::{
		access::Access,
		identity::{Actor, SubjectIdentity},
	},
	federation::Federation,
};
use aidash_application::duties as app;
use aidash_domain::duties::{
	Duty, Tick, TickStatus,
	events::{self, DutyEvent, DutyTrigger, EventOutcome, TriggerKind},
};
use chrono::Utc;
use hmac::{Hmac, Mac};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};
use sha2::Sha256;
use uuid::Uuid;

/// The path a webhook trigger receives events at.
pub(crate) const WEBHOOK_PREFIX: &str = "/hooks/duties/";

/// Events a Duty's page shows.
const EVENT_HISTORY: u32 = 50;

/// The signing secret of a webhook trigger, derived from the Deployment's API
/// token, so no secret is stored; replacing the token replaces every webhook
/// secret.
pub(crate) fn webhook_secret(f: &Federation, trigger: Uuid) -> String {
	let mut mac = Hmac::<Sha256>::new_from_slice(f.config.api_token.as_bytes())
		.expect("HMAC accepts every key length");
	mac.update(b"aidash-duty-webhook/v1\0");
	mac.update(trigger.as_bytes());
	hex(&mac.finalize().into_bytes())
}

fn hex(bytes: &[u8]) -> String {
	bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
	if !text.len().is_multiple_of(2) {
		return None;
	}
	(0..text.len())
		.step_by(2)
		.map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
		.collect()
}

/// Whether `signature` (`sha256=<hex>`, as GitHub's `X-Hub-Signature-256`
/// sends it) is the HMAC-SHA256 of `body` under `secret`, compared in
/// constant time.
pub fn signature_valid(secret: &str, body: &[u8], signature: Option<&str>) -> bool {
	let Some(tag) = signature
		.and_then(|value| value.trim().strip_prefix("sha256="))
		.and_then(unhex)
	else {
		return false;
	};
	let mut mac =
		Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts every key length");
	mac.update(body);
	mac.verify_slice(&tag).is_ok()
}

/// A Duty Trigger as its Duty's page shows it.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct TriggerView {
	#[serde(flatten)]
	pub trigger: DutyTrigger,
	/// Where a webhook trigger receives events.
	pub url: Option<String>,
	/// The webhook's signing secret, shown once when it is created.
	pub secret: Option<String>,
	/// Where another Node sends a `node_event` trigger's events: the Duty's
	/// Node, which a sender names with the trigger's id (#1013).
	pub node_id: Option<String>,
}

/// What a person asks of a Duty's triggers.
pub(crate) enum Operation {
	Create(Uuid, TriggerKind),
	List(Uuid),
	Delete(Uuid, Uuid),
	Events(Uuid),
}

fn view(f: &Federation, trigger: DutyTrigger, secret: bool, node: &str) -> TriggerView {
	let webhook = matches!(trigger.kind, TriggerKind::Webhook);
	let node_event = matches!(trigger.kind, TriggerKind::NodeEvent { .. });
	TriggerView {
		url: webhook.then(|| format!("{}{WEBHOOK_PREFIX}{}", f.config.endpoint, trigger.id)),
		secret: (webhook && secret).then(|| webhook_secret(f, trigger.id)),
		node_id: node_event.then(|| node.to_owned()),
		trigger,
	}
}

/// The address of the Node a Duty's Workspace belongs to.
async fn duty_node(
	f: &Federation,
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	tenant: &str,
	duty: &Duty,
) -> Result<String> {
	let node = duty_repository::workspace_node(tx, tenant, duty.workspace_id).await?;
	Ok(f.config
		.deployment
		.node(aidash_domain::nodes::NodeName::parse(&node)?)
		.to_string())
}

/// Runs one person's trigger operation. Only the Requester adds or removes a
/// Duty's triggers; whoever sees the Duty sees them and its events.
pub(crate) async fn execute(f: &Federation, actor: Actor, op: Operation) -> Result<Value> {
	let Actor::Subject(identity) = actor else {
		return Err(Error::Forbidden);
	};
	let mut access = Access::begin(&f.store, &identity).await?;
	let result: Result<Value> = async {
		let now = Utc::now();
		let tenant = access.identity.tenant.clone();
		let subject = access.identity.subject.clone();
		let mut scope = Adapter {
			access: &mut access,
			store: &f.store,
			now,
		};
		match op {
			Operation::Create(id, kind) => {
				if matches!(kind, TriggerKind::Watcher) {
					return Err(Error::Invalid(
						"a Watcher is set on the Duty itself, not added as a trigger".into(),
					));
				}
				let duty = app::editable(&mut scope, id).await?;
				kind.validate()?;
				if let TriggerKind::DutyCompleted { source_duty_id } = &kind {
					if *source_duty_id == duty.id {
						return Err(Error::Invalid("a Duty cannot wake on its own Tasks".into()));
					}
					// The Requester must see the Duty whose Tasks wake this one.
					app::detail(&mut scope, *source_duty_id).await?;
				}
				let trigger = DutyTrigger {
					id: Uuid::now_v7(),
					duty_id: duty.id,
					kind,
					created_at: now,
					created_by: subject,
				};
				repository::insert_trigger(
					&mut *scope.access.tx,
					&tenant,
					duty.workspace_id,
					&trigger,
				)
				.await?;
				let node = duty_node(f, &mut *scope.access.tx, &tenant, &duty).await?;
				Ok(serde_json::to_value(view(f, trigger, true, &node))?)
			}
			Operation::List(id) => {
				let detail = app::detail(&mut scope, id).await?;
				let triggers =
					repository::triggers(&mut *scope.access.tx, &tenant, detail.duty.id).await?;
				let node = duty_node(f, &mut *scope.access.tx, &tenant, &detail.duty).await?;
				Ok(serde_json::to_value(
					triggers
						.into_iter()
						.map(|trigger| view(f, trigger, false, &node))
						.collect::<Vec<_>>(),
				)?)
			}
			Operation::Delete(id, trigger) => {
				let duty = app::editable(&mut scope, id).await?;
				if !repository::delete_trigger(&mut *scope.access.tx, &tenant, duty.id, trigger)
					.await?
				{
					return Err(Error::NotFound("trigger not found".into()));
				}
				Ok(json!({"deleted": trigger}))
			}
			Operation::Events(id) => {
				let detail = app::detail(&mut scope, id).await?;
				Ok(serde_json::to_value(
					repository::events(
						&mut *scope.access.tx,
						&tenant,
						detail.duty.id,
						EVENT_HISTORY,
					)
					.await?,
				)?)
			}
		}
	}
	.await;
	access.finish(result).await
}

/// A request to a webhook trigger. An unknown trigger and a wrong signature
/// are refused alike, so the endpoint tells nobody which triggers exist.
pub(crate) async fn receive(
	f: &Federation,
	trigger: Uuid,
	signature: Option<&str>,
	body: &[u8],
) -> Result<Value> {
	let Some((tenant, trigger)) = repository::locate(&f.store.pool, trigger).await? else {
		return Err(Error::NotFound("webhook not found".into()));
	};
	if !matches!(trigger.kind, TriggerKind::Webhook)
		|| !signature_valid(&webhook_secret(f, trigger.id), body, signature)
	{
		return Err(Error::NotFound("webhook not found".into()));
	}
	let payload = events::payload_of(body)?;
	let event = deliver(f, &tenant, &trigger, payload).await?;
	f.notify.notify_waiters();
	Ok(serde_json::to_value(
		crate::apps::workspaces::serializers::duties::DutyEventReceipt {
			event_id: event.id,
			outcome: event.outcome,
		},
	)?)
}

/// An event another Node sends to a `node_event` trigger (#1013, ADR 0142).
///
/// Who may send was decided when the request was accepted: a Node of this
/// Tenant only for a subject the Tenant's policy lets use this Node, a Node of
/// another Tenant only through this Tenant's Node Registration (and its Peer
/// Mapping, when the sender names a subject). The sender is the Node the
/// Federation token proved, never the body. The event is input, never a
/// requester: the Duty starts or wakes its Task as its own Requester, and the
/// sender's identity never reaches that Task's authority. An unknown trigger,
/// one of another kind or Node, and one that names another sender are refused
/// alike, so a sender learns nothing of the triggers it may not use.
pub(crate) async fn receive_from_node(
	f: &Federation,
	admitted: aidash_application::federation::inbound::Admitted,
	trigger: Uuid,
	data: Value,
) -> Result<Value> {
	use aidash_application::federation::inbound::Acting;
	let tenant = admitted.tenant.as_str().to_owned();
	if matches!(admitted.acting, Acting::Node)
		&& admitted.sender.deployment == f.config.deployment
		&& f.node_tenant_of(&admitted.sender.node).await? == admitted.tenant
	{
		// A Node of this Tenant names the subject it sends for, so the
		// Tenant's policy decides.
		return Err(Error::Forbidden);
	}
	let payload = events::node_event_payload(&admitted.sender, data)?;
	let not_found = || Error::NotFound("trigger not found".into());
	let Some((found, trigger)) = repository::locate(&f.store.pool, trigger).await? else {
		return Err(not_found());
	};
	if found != tenant || !trigger.kind.admits(&admitted.sender) {
		return Err(not_found());
	}
	// The trigger must belong to a Duty of the receiving Node.
	let scope = aidash_application::tenancy::TenancyScope::tenant(&tenant)?;
	let node = f
		.store
		.pool
		.scoped(&scope, async |tx| {
			let Some(duty) =
				duty_repository::load(&mut **tx, &tenant, trigger.duty_id, false).await?
			else {
				return Ok(None);
			};
			duty_repository::workspace_node(&mut **tx, &tenant, duty.workspace_id)
				.await
				.map(Some)
		})
		.await?;
	if node.as_deref() != Some(admitted.receiver.node.as_str()) {
		return Err(not_found());
	}
	let event = deliver(f, &tenant, &trigger, payload).await?;
	f.notify.notify_waiters();
	Ok(serde_json::to_value(
		crate::apps::workspaces::serializers::duties::DutyEventReceipt {
			event_id: event.id,
			outcome: event.outcome,
		},
	)?)
}

/// `duty_notify`: an Agent sends an event to a Duty on another Node (#1013).
/// It sends from the Run's Node through a route its Tenant set up: to a Node
/// of its own Tenant as the person at the root of the Run's chain, whom that
/// Tenant's policy judges there; to a Node of another Tenant as the Node
/// itself, which that Tenant registered. A Run that arrived through
/// Federation, or whose chain starts at no person, sends none. Its approval
/// was already decided by that person's Approval Mode and Approval Rules.
pub async fn notify(f: &Federation, run: &aidash_domain::Run, input: Value) -> Result<Value> {
	use aidash_application::ports::authorization::ExecutionGrantRepository;
	let input: aidash_application::duties::tool::Notify =
		serde_json::from_value(input).map_err(|error| Error::Invalid(error.to_string()))?;
	if !run.is_local(f.config.deployment.as_str()) {
		return Err(Error::Invalid(
			"a Run that arrived through Federation cannot send Duty events".into(),
		));
	}
	let target = aidash_domain::nodes::NodeAddress::parse(&input.node_id)?;
	// Refuse an oversized event before anything leaves this Node.
	events::node_event_payload(&target, input.payload.clone())?;
	let grant = crate::apps::identity::repositories::execution::Grants { store: &f.store }
		.grant(run.id)
		.await?
		.ok_or(Error::Forbidden)?;
	let person =
		crate::apps::identity::repositories::oauth_connections::owner_of_run(&f.store, run)
			.await?
			.map(|owner| owner.subject)
			.ok_or_else(|| {
				Error::Invalid("this Run's delegation chain does not start at a person".into())
			})?;
	let tenant = aidash_domain::tenancy::TenantName::new(&grant.tenant)?;
	let same_tenant = target.deployment == f.config.deployment
		&& f.node_tenant_of(&target.node).await.ok().as_ref() == Some(&tenant);
	let sender = aidash_domain::federation::route::Sender {
		tenant,
		node: Some(run.node.clone()),
		subject: same_tenant.then_some(person),
	};
	f.request::<Value>(
		&sender,
		&target.to_string(),
		reqwest::Method::POST,
		NODE_EVENTS,
		Some(&json!({"trigger_id": input.trigger_id, "payload": input.payload})),
	)
	.await
}

/// Where a Node receives events for its Duties, under `/federation/v0.1`.
pub(crate) const NODE_EVENTS: &str = "/scoped/duty-events";

/// Fires the `duty_completed` triggers of the Duty whose Task ended.
pub(crate) async fn completed(
	f: &Federation,
	tenant: &str,
	duty: Uuid,
	task: Uuid,
	status: TickStatus,
) -> Result<()> {
	let scope = aidash_application::tenancy::TenancyScope::tenant(tenant)?;
	let triggers = f
		.store
		.pool
		.scoped(&scope, async |tx| {
			repository::triggers_on_completion(&mut **tx, tenant, duty).await
		})
		.await?;
	for trigger in triggers {
		let payload = json!({"duty_id": duty, "task_id": task, "status": status.as_str()});
		if let Err(error) = deliver(f, tenant, &trigger, payload).await {
			tracing::warn!(%error, trigger = %trigger.id, "a Duty completion event could not be delivered");
		}
	}
	Ok(())
}

/// Delivers one event to its Duty as the Duty's Requester: to the Task that
/// booked the trigger, else to the Duty's Router (#1011 D3), which decides
/// what it does; without one, as a new Task in the Duty's Thread, unless the
/// Duty is stopped or its previous Task is still open.
pub(crate) async fn deliver(
	f: &Federation,
	tenant: &str,
	trigger: &DutyTrigger,
	payload: Value,
) -> Result<DutyEvent> {
	let scope = aidash_application::tenancy::TenancyScope::tenant(tenant)?;
	let Some(duty) = f
		.store
		.pool
		.scoped(&scope, async |tx| {
			duty_repository::load(&mut **tx, tenant, trigger.duty_id, false).await
		})
		.await?
	else {
		return Err(Error::NotFound("duty not found".into()));
	};
	let now = Utc::now();
	let mut event = DutyEvent {
		id: Uuid::now_v7(),
		duty_id: duty.id,
		trigger_id: trigger.id,
		received_at: now,
		payload,
		outcome: EventOutcome::Skipped,
		reason: None,
		task_id: None,
		router: None,
	};
	let identity = SubjectIdentity {
		http_session: None,
		credential_id: duty.credential_id,
		tenant: tenant.to_owned(),
		subject: duty.requester.clone(),
	};
	let access = match Access::begin(&f.store, &identity).await {
		Ok(access) => access,
		Err(_) => {
			// The Requester lost access: record the event, start nothing.
			event.reason = Some("the Requester lost access".into());
			record(f, tenant, &duty, &event, None).await?;
			return Ok(event);
		}
	};
	let mut lease = Lease::from_access(access);
	let result: Result<Option<Uuid>> = async {
		// Events for one Duty are delivered one at a time: the row lock makes a
		// second event wait until the first has started its Task (or not), so
		// two events arriving together cannot both find the Duty idle.
		let Some(duty) = duty_repository::load(lease.tx(), tenant, trigger.duty_id, true).await?
		else {
			return Err(Error::NotFound("duty not found".into()));
		};
		if let Some((task, run)) = repository::take_booking(lease.tx(), tenant, trigger.id).await? {
			event.outcome = EventOutcome::Delivered;
			event.task_id = Some(task);
			repository::insert_event(lease.tx(), tenant, duty.workspace_id, &event, Some(run))
				.await?;
			repository::wake(lease.tx(), run, event.id).await?;
			return Ok(Some(run));
		}
		if duty.halt.is_some() {
			event.reason = Some(format!("the Duty is {}", duty.status()));
		} else if let Some(router) = &duty.definition.router {
			// The Router decides; it runs without a model and its Task
			// settles in the scheduler pass (#1011 D3).
			event.outcome = EventOutcome::Routing;
			super::duty_scripts::route(f, &mut lease, &duty, router, (trigger, &event), now)
				.await?;
		} else if duty_repository::running(lease.tx(), tenant, duty.id).await? {
			event.reason = Some(aidash_domain::duties::skip::OVERLAP.into());
		} else if let Some(reason) =
			super::duties::budget_skip(&mut lease, tenant, &duty, now).await?
		{
			event.reason = Some(reason);
		} else {
			let description =
				events::task_description(&duty.definition.mission, &trigger.kind, &event.payload);
			let title = duty.definition.title.clone();
			match start_with(f, &mut lease, &duty, (now, ""), (title, description)).await {
				Ok((task, thread)) => {
					event.outcome = EventOutcome::Started;
					event.task_id = Some(task);
					duty_repository::insert_tick(
						lease.tx(),
						tenant,
						duty.workspace_id,
						&Tick {
							id: Uuid::new_v4(),
							duty_id: duty.id,
							scheduled_at: now,
							recorded_at: now,
							status: TickStatus::Started,
							reason: Some(format!("event: {}", trigger.kind.as_str())),
							task_id: Some(task),
							thread_id: Some(thread),
							started_by: None,
						},
					)
					.await?;
				}
				Err(error) => return Err(error),
			}
		}
		repository::insert_event(lease.tx(), tenant, duty.workspace_id, &event, None).await?;
		Ok(None)
	}
	.await;
	match lease.finish(result).await {
		Ok(_) => Ok(event),
		Err(error) => {
			// A refused start is a failed event with its reason, never retried.
			event.outcome = EventOutcome::Failed;
			event.reason = Some(error.to_string());
			event.task_id = None;
			record(f, tenant, &duty, &event, None).await?;
			Ok(event)
		}
	}
}

/// Records an event in a transaction of its own.
async fn record(
	f: &Federation,
	tenant: &str,
	duty: &Duty,
	event: &DutyEvent,
	run: Option<Uuid>,
) -> Result<()> {
	let scope = aidash_application::tenancy::TenancyScope::tenant(tenant)?;
	f.store
		.pool
		.scoped(&scope, async |tx| {
			repository::insert_event(&mut **tx, tenant, duty.workspace_id, event, run).await
		})
		.await
}

/// Books the next event of a trigger for the Run's Task (#1011, decision B):
/// that event wakes this Run instead of starting a new Task. Only a trigger of
/// a Duty of the person at the root of the Run's chain may be booked, in that
/// person's Tenant; a federated Run books none.
pub async fn book(
	store: &crate::store::Store,
	run: &aidash_domain::Run,
	trigger: Uuid,
) -> Result<Value> {
	use aidash_application::ports::authorization::ExecutionGrantRepository;
	if !run.is_local(store.deployment.as_str()) {
		return Err(Error::Invalid(
			"a Run that arrived through Federation cannot book Duty events".into(),
		));
	}
	let grant = crate::apps::identity::repositories::execution::Grants { store }
		.grant(run.id)
		.await?
		.ok_or(Error::Forbidden)?;
	let person = crate::apps::identity::repositories::oauth_connections::owner_of_run(store, run)
		.await?
		.map(|owner| owner.subject)
		.ok_or_else(|| {
			Error::Invalid("this Run's delegation chain does not start at a person".into())
		})?;
	let Some((tenant, found)) = repository::locate(&store.pool, trigger).await? else {
		return Err(Error::NotFound("trigger not found".into()));
	};
	if tenant != grant.tenant {
		return Err(Error::NotFound("trigger not found".into()));
	}
	let scope = aidash_application::tenancy::TenancyScope::tenant(&tenant)?;
	store
		.pool
		.scoped(&scope, async |tx| {
			let duty = duty_repository::load(&mut **tx, &tenant, found.duty_id, false)
				.await?
				.filter(|duty| duty.requester == person)
				.ok_or_else(|| Error::NotFound("trigger not found".into()))?;
			repository::book(
				&mut **tx,
				&tenant,
				duty.workspace_id,
				trigger,
				run.task_id,
				run.id,
				Utc::now(),
			)
			.await
		})
		.await?;
	Ok(json!({"booked": trigger}))
}

#[cfg(test)]
mod tests {
	use super::*;
	use rstest::rstest;

	fn sign(secret: &str, body: &[u8]) -> String {
		let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
		mac.update(body);
		format!("sha256={}", hex(&mac.finalize().into_bytes()))
	}

	#[rstest]
	fn only_the_bodys_hmac_under_the_secret_is_accepted() {
		let body = br#"{"conclusion":"success"}"#;
		let signed = sign("secret", body);
		assert!(signature_valid("secret", body, Some(&signed)));
		assert!(!signature_valid("other", body, Some(&signed)));
		assert!(!signature_valid("secret", b"tampered", Some(&signed)));
		assert!(!signature_valid("secret", body, Some("sha256=zz")));
		assert!(!signature_valid(
			"secret",
			body,
			Some(signed.trim_start_matches("sha256="))
		));
		assert!(!signature_valid("secret", body, None));
	}
}
