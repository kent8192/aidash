//! Router and Watcher scripts of a Duty (#1011, ADR 0142). A script runs as a
//! Run of the Duty's Agent for its Requester, so Egress, External services,
//! approvals, cancel and purge apply as for any Run, but the harness makes the
//! script's tool calls itself and no model is asked. Here a Duty starts those
//! Tasks, and the scheduler pass applies what they decided or found once they
//! end: a Router's action for its event, a Watcher's events delivered like a
//! webhook's.
use super::access::Lease;
use super::duties::start_with;
use super::duty_events::deliver_once;
use crate::apps::workspaces::repositories::{
	duties as duty_repository, duty_events as event_repository, duty_scripts as repository,
};
use crate::{
	Error, Result,
	authorization::{access::Access, identity::SubjectIdentity},
	federation::Federation,
};
use aidash_domain::{
	TaskStatus,
	duties::{
		Duty, Halt, Tick, TickStatus,
		events::{self, DutyEvent, DutyTrigger, EventOutcome, TriggerKind},
		scripts::{self, Invocation, Role, RouterAction, Script},
	},
};
use chrono::{DateTime, Duration, Utc};
use serde_json::Value;
use uuid::Uuid;

/// Script Tasks settled and Watchers run per pass; the rest wait for the next.
const BATCH: u64 = 32;

/// The trigger a Duty's Watcher events name: the Duty itself, since a Watcher
/// is part of the Duty rather than a trigger row.
pub(crate) fn watcher_trigger(duty: &Duty) -> DutyTrigger {
	DutyTrigger {
		id: duty.id,
		duty_id: duty.id,
		kind: TriggerKind::Watcher,
		created_at: duty.created_at,
		created_by: duty.requester.clone(),
	}
}

/// Starts a script Task in the Duty's Thread as the lease's person, and
/// records what it runs. Returns the Task.
async fn start_script(
	f: &Federation,
	lease: &mut Lease,
	duty: &Duty,
	(role, event): (Role, Option<Uuid>),
	invocation: &Invocation,
	now: DateTime<Utc>,
) -> Result<Uuid> {
	let (title, description) = match role {
		Role::Router => (
			format!("Router of {}", duty.definition.title),
			"Runs the Duty's Router on one event without a model; its result is the Router's decision.",
		),
		Role::Watcher => (
			format!("Watcher of {}", duty.definition.title),
			"Runs the Duty's Watcher without a model; its result is the events it found.",
		),
	};
	// Each start has a key of its own: several may start in one pass.
	let task = start_with(
		f,
		lease,
		duty,
		(Utc::now(), ""),
		(title, description.to_owned()),
	)
	.await?;
	let tenant = lease.access_mut().identity.tenant.clone();
	repository::insert_run(
		lease.tx(),
		&tenant,
		repository::NewRun {
			task,
			duty: duty.id,
			role,
			event,
			invocation,
			now,
		},
	)
	.await?;
	Ok(task)
}

/// Starts the Duty's Router on one event, which the caller records as
/// `routing`. The Router reads the event, its trigger and the Duty's open
/// Tasks on stdin.
pub(crate) async fn route(
	f: &Federation,
	lease: &mut Lease,
	duty: &Duty,
	router: &Script,
	(trigger, event): (&DutyTrigger, &DutyEvent),
	now: DateTime<Utc>,
) -> Result<()> {
	let tenant = lease.access_mut().identity.tenant.clone();
	let open = repository::open_tasks(lease.tx(), &tenant, duty.id).await?;
	let stdin = scripts::router_stdin(&event.payload, (trigger.id, trigger.kind.as_str()), &open)?;
	let invocation = Invocation {
		language: router.language,
		source: router.source.clone(),
		stdin,
	};
	start_script(
		f,
		lease,
		duty,
		(Role::Router, Some(event.id)),
		&invocation,
		now,
	)
	.await?;
	Ok(())
}

/// The person a Duty acts as.
fn requester(tenant: &str, duty: &Duty) -> SubjectIdentity {
	SubjectIdentity {
		http_session: None,
		credential_id: duty.credential_id,
		tenant: tenant.to_owned(),
		subject: duty.requester.clone(),
	}
}

async fn load(f: &Federation, tenant: &str, id: Uuid) -> Result<Option<Duty>> {
	let scope = aidash_application::tenancy::TenancyScope::tenant(tenant)?;
	f.store
		.pool
		.scoped(&scope, async |tx| {
			duty_repository::load(&mut **tx, tenant, id, false).await
		})
		.await
}

/// Runs every Watcher that is due, each as its Duty's Requester. Returns the
/// Tasks started. A Watcher whose previous run is still open waits for the
/// next interval; one that cannot start records a failed event and waits too.
pub(crate) async fn run_watchers(f: &Federation, now: DateTime<Utc>) -> Vec<Uuid> {
	let due = match repository::due_watchers(&f.store.pool, now, BATCH).await {
		Ok(due) => due,
		Err(error) => {
			tracing::warn!(%error, "due Watchers could not be read");
			return vec![];
		}
	};
	let mut started = vec![];
	for (tenant, id) in due {
		match watch(f, &tenant, id, now).await {
			Ok(Some(task)) => started.push(task),
			Ok(None) => {}
			// A database outage leaves the Watcher due for the next pass.
			Err(error) if error.is_transient_database() => {
				tracing::warn!(%error, duty = %id, "a Duty's Watcher waits for the database");
			}
			Err(error) => {
				tracing::warn!(%error, duty = %id, "a Duty's Watcher could not start");
				if let Err(error) = watcher_failed(f, &tenant, id, now, &error.to_string()).await {
					tracing::warn!(%error, duty = %id, "a Watcher failure could not be recorded");
				}
			}
		}
	}
	started
}

/// Claims one due Watcher and starts its Task, unless its previous run is open.
async fn watch(f: &Federation, tenant: &str, id: Uuid, now: DateTime<Utc>) -> Result<Option<Uuid>> {
	let Some(duty) = load(f, tenant, id).await? else {
		return Ok(None);
	};
	let access = match Access::begin(&f.store, &requester(tenant, &duty)).await {
		Ok(access) => access,
		Err(
			Error::Unauthorized
			| Error::CredentialUnauthorized(_)
			| Error::Forbidden
			| Error::CredentialForbidden(_),
		) => {
			// The Requester left or lost access: the Duty stops, as a tick would.
			stop_for_requester(f, tenant, id, now).await?;
			return Ok(None);
		}
		Err(error) => return Err(error),
	};
	let mut lease = Lease::from_access(access);
	let result: Result<Option<Uuid>> = async {
		let Some(duty) = repository::claim_watcher(lease.tx(), tenant, id, now).await? else {
			return Ok(None);
		};
		let Some(watcher) = duty.definition.watcher.clone() else {
			return Ok(None);
		};
		repository::advance_watcher(
			lease.tx(),
			tenant,
			duty.id,
			Some(now + Duration::seconds(watcher.every_seconds)),
		)
		.await?;
		if repository::watching(lease.tx(), tenant, duty.id).await? {
			return Ok(None);
		}
		let invocation = Invocation {
			language: watcher.script.language,
			source: watcher.script.source,
			stdin: String::new(),
		};
		start_script(
			f,
			&mut lease,
			&duty,
			(Role::Watcher, None),
			&invocation,
			now,
		)
		.await
		.map(Some)
	}
	.await;
	lease.finish(result).await
}

/// A Watcher that could not start waits for its next interval, and its
/// Duty's events say why.
async fn watcher_failed(
	f: &Federation,
	tenant: &str,
	id: Uuid,
	now: DateTime<Utc>,
	reason: &str,
) -> Result<()> {
	let scope = aidash_application::tenancy::TenancyScope::tenant(tenant)?;
	let mut tx = f.store.pool.begin(&scope).await?;
	let result: Result<()> = async {
		let Some(duty) = duty_repository::load(&mut tx, tenant, id, true).await? else {
			return Ok(());
		};
		let Some(watcher) = &duty.definition.watcher else {
			return Ok(());
		};
		repository::advance_watcher(
			&mut tx,
			tenant,
			id,
			Some(now + Duration::seconds(watcher.every_seconds)),
		)
		.await?;
		let event = script_event(
			&duty,
			now,
			EventOutcome::Failed,
			format!("Watcher: {reason}"),
		);
		event_repository::insert_event(&mut tx, tenant, duty.thread_id, &event, None).await
	}
	.await;
	match result {
		Ok(()) => tx.commit().await,
		Err(error) => {
			tx.rollback().await?;
			Err(error)
		}
	}
}

async fn stop_for_requester(
	f: &Federation,
	tenant: &str,
	id: Uuid,
	now: DateTime<Utc>,
) -> Result<()> {
	let scope = aidash_application::tenancy::TenancyScope::tenant(tenant)?;
	let mut tx = f.store.pool.begin(&scope).await?;
	let result: Result<()> = async {
		let Some(mut duty) = duty_repository::load(&mut tx, tenant, id, true).await? else {
			return Ok(());
		};
		if duty.halt.is_some() {
			return Ok(());
		}
		duty.stop_because(
			Halt::RequesterLeft,
			Some("the Requester lost access".into()),
			now,
		);
		duty.updated_at = now;
		duty_repository::save(&mut tx, tenant, &duty).await?;
		Ok(())
	}
	.await;
	match result {
		Ok(()) => tx.commit().await,
		Err(error) => {
			tx.rollback().await?;
			Err(error)
		}
	}
}

/// An event a Watcher run itself records: why it failed, or what it dropped.
fn script_event(
	duty: &Duty,
	now: DateTime<Utc>,
	outcome: EventOutcome,
	reason: String,
) -> DutyEvent {
	DutyEvent {
		id: Uuid::now_v7(),
		duty_id: duty.id,
		trigger_id: duty.id,
		received_at: now,
		payload: Value::Null,
		outcome,
		reason: Some(reason),
		task_id: None,
		router: None,
	}
}

/// Applies the outcome of every script Task that ended. Returns how many were
/// settled.
pub(crate) async fn settle_all(f: &Federation, now: DateTime<Utc>) -> usize {
	let ended = match repository::ended(&f.store.pool, BATCH).await {
		Ok(ended) => ended,
		Err(error) => {
			tracing::warn!(%error, "ended Duty scripts could not be read");
			return 0;
		}
	};
	let mut settled = 0;
	for script in ended {
		match settle(f, &script, now).await {
			Ok(true) => settled += 1,
			Ok(false) => {}
			Err(error) => {
				tracing::warn!(%error, task = %script.task, "a Duty script could not be settled");
			}
		}
	}
	settled
}

/// What a settled Watcher run leaves to deliver after its transaction.
type Found = Vec<Value>;

/// Settles one ended script Task as its Duty's Requester, exactly once.
async fn settle(f: &Federation, script: &repository::Ended, now: DateTime<Utc>) -> Result<bool> {
	let tenant = script.tenant.as_str();
	let Some(duty) = load(f, tenant, script.duty).await? else {
		return Ok(false);
	};
	let access = match Access::begin(&f.store, &requester(tenant, &duty)).await {
		Ok(access) => access,
		Err(
			Error::Unauthorized
			| Error::CredentialUnauthorized(_)
			| Error::Forbidden
			| Error::CredentialForbidden(_),
		) => {
			// Nothing can act for a Requester who lost access: the outcome is
			// recorded as failed and nothing else happens.
			return settle_failed(f, script, &duty, now, "the Requester lost access").await;
		}
		// Anything else, such as a database outage, waits for the next pass.
		Err(error) => return Err(error),
	};
	let mut lease = Lease::from_access(access);
	let result: Result<Option<Found>> = async {
		if !repository::settle(lease.tx(), tenant, script.task, now).await? {
			return Ok(None);
		}
		// Events of one Duty are applied one at a time, as they are delivered.
		let Some(duty) = duty_repository::load(lease.tx(), tenant, duty.id, true).await? else {
			return Ok(None);
		};
		let (stdout, error) = repository::outcome(lease.tx(), tenant, script.task).await?;
		let output = match script.status {
			TaskStatus::Completed => Ok(stdout.unwrap_or_default()),
			TaskStatus::Cancelled | TaskStatus::Abandoned => {
				Err("its Task was cancelled".to_owned())
			}
			_ => Err(error.unwrap_or_else(|| "its Task failed".into())),
		};
		match script.role {
			Role::Router => {
				let event_id = script
					.event
					.ok_or_else(|| Error::Invalid("a Router run names no event".into()))?;
				decide(f, &mut lease, &duty, event_id, output, now).await?;
				Ok(Some(vec![]))
			}
			Role::Watcher => match output.and_then(|stdout| scripts::parse_watcher(&stdout)) {
				Ok(found) => {
					if let Some(dropped) = found.dropped {
						let event = script_event(&duty, now, EventOutcome::Dropped, dropped);
						event_repository::insert_event(
							lease.tx(),
							tenant,
							duty.thread_id,
							&event,
							None,
						)
						.await?;
					}
					Ok(Some(found.events))
				}
				Err(reason) => {
					let event = script_event(
						&duty,
						now,
						EventOutcome::Failed,
						format!("Watcher: {reason}"),
					);
					event_repository::insert_event(
						lease.tx(),
						tenant,
						duty.thread_id,
						&event,
						None,
					)
					.await?;
					Ok(Some(vec![]))
				}
			},
		}
	}
	.await;
	let found = match lease.finish(result).await {
		Ok(Some(found)) => found,
		Ok(None) => return Ok(false),
		// A database outage leaves the script unsettled for the next pass.
		Err(error) if error.is_transient_database() => return Err(error),
		// A refused decision is a failed event, never retried silently.
		Err(error) => return settle_failed(f, script, &duty, now, &error.to_string()).await,
	};
	// A Watcher's events go the way a webhook's go, each in its own
	// transaction: to a booking, the Router or a new Task.
	let trigger = watcher_trigger(&duty);
	for payload in found {
		if let Err(error) = deliver_once(f, tenant, &trigger, payload).await {
			tracing::warn!(%error, duty = %duty.id, "a Watcher event could not be delivered");
		}
	}
	Ok(true)
}

/// Settles a script Task whose outcome cannot be applied, recording why.
async fn settle_failed(
	f: &Federation,
	script: &repository::Ended,
	duty: &Duty,
	now: DateTime<Utc>,
	reason: &str,
) -> Result<bool> {
	let tenant = script.tenant.as_str();
	let scope = aidash_application::tenancy::TenancyScope::tenant(tenant)?;
	let mut tx = f.store.pool.begin(&scope).await?;
	let result: Result<bool> = async {
		if !repository::settle(&mut tx, tenant, script.task, now).await? {
			return Ok(false);
		}
		match (script.role, script.event) {
			(Role::Router, Some(event)) => {
				event_repository::conclude(
					&mut tx,
					tenant,
					event,
					(EventOutcome::Failed, Some(format!("Router: {reason}"))),
					None,
					(None, None),
				)
				.await?;
			}
			_ => {
				let event = script_event(
					duty,
					now,
					EventOutcome::Failed,
					format!("Watcher: {reason}"),
				);
				event_repository::insert_event(&mut tx, tenant, duty.thread_id, &event, None)
					.await?;
			}
		}
		Ok(true)
	}
	.await;
	match result {
		Ok(value) => {
			tx.commit().await?;
			Ok(value)
		}
		Err(error) => {
			tx.rollback().await?;
			Err(error)
		}
	}
}

/// Applies a Router's decision about one event, as the Duty's Requester, in
/// the lease that settles the Router's Task.
async fn decide(
	f: &Federation,
	lease: &mut Lease,
	duty: &Duty,
	event_id: Uuid,
	output: std::result::Result<String, String>,
	now: DateTime<Utc>,
) -> Result<()> {
	let tenant = lease.access_mut().identity.tenant.clone();
	let Some((event, kind)) = event_repository::find(lease.tx(), &tenant, event_id).await? else {
		return Ok(());
	};
	let action = match output.and_then(|stdout| scripts::parse_router(&stdout)) {
		Ok(action) => action,
		Err(reason) => {
			return event_repository::conclude(
				lease.tx(),
				&tenant,
				event.id,
				(EventOutcome::Failed, Some(format!("Router: {reason}"))),
				None,
				(None, None),
			)
			.await;
		}
	};
	let (outcome, reason, task, run) = match &action {
		RouterAction::Drop {} => (EventOutcome::Dropped, None, None, None),
		RouterAction::Act {} => (EventOutcome::Acted, None, None, None),
		RouterAction::Task { .. } if duty.halt.is_some() => (
			EventOutcome::Skipped,
			Some(format!("the Duty is {}", duty.status())),
			None,
			None,
		),
		RouterAction::Task { title, description } => {
			if let Some(reason) = super::duties::budget_skip(lease, &tenant, duty, now).await? {
				(EventOutcome::Skipped, Some(reason), None, None)
			} else {
				let description = events::routed_task_description(
					&duty.definition.mission,
					&kind,
					&event.payload,
					(title, description),
				);
				// The Task keeps the Duty's own title: a Router often copies
				// event fields (a pull request title, a mail subject) into
				// its title, which would reach the model outside the data
				// fence. Its title and description stay inside it (ADR 0142).
				// Several decisions may start Tasks in one pass; each start
				// keys on its own time.
				let task = start_with(
					f,
					lease,
					duty,
					(Utc::now(), ""),
					(duty.definition.title.clone(), description),
				)
				.await?;
				duty_repository::insert_tick(
					lease.tx(),
					&tenant,
					duty.thread_id,
					&Tick {
						id: Uuid::new_v4(),
						duty_id: duty.id,
						scheduled_at: now,
						recorded_at: now,
						status: TickStatus::Started,
						reason: Some(format!("event: {} (router)", kind.as_str())),
						task_id: Some(task),
						started_by: None,
					},
				)
				.await?;
				(EventOutcome::Started, None, Some(task), None)
			}
		}
		RouterAction::Deliver { task_id, .. } => {
			let open = repository::open_tasks(lease.tx(), &tenant, duty.id).await?;
			let run = if open.iter().any(|task| task.task_id == *task_id) {
				repository::active_run(lease.tx(), *task_id).await?
			} else {
				None
			};
			match run {
				Some(run) => (EventOutcome::Delivered, None, Some(*task_id), Some(run)),
				None => (
					EventOutcome::Failed,
					Some(format!(
						"Router: {task_id} is not an open Task of this Duty"
					)),
					None,
					None,
				),
			}
		}
	};
	event_repository::conclude(
		lease.tx(),
		&tenant,
		event.id,
		(outcome, reason),
		Some(&action),
		(task, run),
	)
	.await?;
	// The input reaches the Task's Run as data with the event, as a booked
	// event does: its next Thread wait returns it (ADR 0142).
	if let Some(run) = run {
		event_repository::wake(lease.tx(), run, event.id).await?;
	}
	Ok(())
}
