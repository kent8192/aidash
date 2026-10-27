//! A Live preview is started by its Run, pinned to one Working Area revision,
//! and stops at its idle or absolute limit, when the Run is cancelled, its
//! authority is revoked or its Tenant is deleted (ADR 0070).
//!
//! This module decides order and refusals; the server persists the facts,
//! dispatches the Runner's `serve` operation and the Preview gateway proxies to
//! the Pod. A start retains every File version of its revision (ADR 0095).
use crate::{Error, Result};
use aidash_domain::{
	capabilities::{retention::Holder, sessions::Area},
	preview::live::{Limits, LivePreview, START_SECONDS, State, StopReason, startup_tail},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The Sandboxed tools' request: a command whose Web server declares a port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Start {
	pub command: String,
	pub port: u16,
}

/// What the dashboard and the model read about a Run's Live preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
	pub id: Uuid,
	pub state: State,
	pub stop_reason: Option<StopReason>,
	pub area_id: Uuid,
	pub revision: i64,
	pub port: u16,
	pub limits: Limits,
	pub started_at: DateTime<Utc>,
	pub last_request_at: DateTime<Utc>,
	pub expires_at: DateTime<Utc>,
}

impl Status {
	pub fn of(preview: &LivePreview, area_revision: i64) -> Self {
		Self {
			id: preview.id,
			state: preview.view(area_revision),
			stop_reason: preview.stop_reason,
			area_id: preview.area_id,
			revision: preview.revision,
			port: preview.port,
			limits: preview.limits.clone(),
			started_at: preview.started_at,
			last_request_at: preview.last_request_at,
			expires_at: preview.expires_at,
		}
	}
}

/// What a start returns: the state and, on failure, the bounded Pod log tail.
/// It never carries an address for viewing. Viewing takes a ticket (ADR 0068).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Started {
	pub status: Status,
	pub startup_log: String,
}

/// The Run the start belongs to, as the caller's authority sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunFacts {
	pub id: Uuid,
	pub tenant: String,
	/// The Run's Working Area.
	pub area_id: Uuid,
}

/// The authority, persistence and file holds a start needs.
#[async_trait]
pub trait Scope: Send {
	fn domain(&self) -> Option<&str>;
	/// The Deployment's ceilings from configuration.
	fn ceilings(&self) -> Limits;
	/// The Tenant's lowered values, when it set any.
	async fn tenant_limits(&mut self, tenant: &str) -> Result<Option<Limits>>;
	async fn run(&mut self) -> Result<RunFacts>;
	async fn area(&mut self) -> Result<Area>;
	/// The Run's latest Live preview, whatever its state.
	async fn current(&mut self, run: Uuid) -> Result<Option<LivePreview>>;
	async fn by_id(&mut self, id: Uuid) -> Result<Option<LivePreview>>;
	async fn insert(&mut self, preview: &LivePreview) -> Result<()>;
	async fn save(&mut self, preview: &LivePreview) -> Result<()>;
	/// Retains every File version of `revision` for `holder`.
	async fn retain_revision(
		&mut self,
		area_id: Uuid,
		revision: i64,
		holder: &Holder,
	) -> Result<()>;
	async fn release(&mut self, holder: &Holder) -> Result<()>;
	/// Every preview that is still running or awaiting termination.
	async fn unsettled(&mut self) -> Result<Vec<LivePreview>>;
	/// The area revision behind each preview, so staleness is visible.
	async fn area_revision(&mut self, area_id: Uuid) -> Result<Option<i64>>;
	/// Stores the Egress session token's expiry, which the gateway recomputes.
	async fn egress_expiry(&mut self, preview: Uuid, expires_at: DateTime<Utc>) -> Result<()>;
}

/// The limits effective for `tenant`: the Deployment's ceilings, lowered by the
/// Tenant. A Tenant may not raise either value.
pub async fn effective(scope: &mut dyn Scope, tenant: &str) -> Result<Limits> {
	let ceilings = scope.ceilings();
	match scope.tenant_limits(tenant).await? {
		Some(tenant) => Ok(tenant.lower_than(&ceilings)?),
		None => Ok(ceilings),
	}
}

/// Prepare a start: refuse without a preview domain, take the holds over the
/// revision's File versions (refusing over quota), and record a row that does
/// not yet claim to serve. The caller then dispatches the Runner's `serve`
/// operation and calls [`serving`] or [`failed`].
///
/// The caller stops the Run's previous Live preview first: at most one serves.
pub async fn start(scope: &mut dyn Scope, request: &Start) -> Result<LivePreview> {
	if scope.domain().is_none() {
		return Err(Error::Conflict("PREVIEW_UNAVAILABLE".into()));
	}
	let run = scope.run().await?;
	let area = scope.area().await?;
	if area.id != run.area_id || area.tenant != run.tenant {
		return Err(Error::Forbidden);
	}
	let limits = effective(scope, &run.tenant).await?;
	let id = Uuid::new_v4();
	let now = Utc::now();
	let preview = LivePreview::starting(
		id,
		run.tenant.clone(),
		run.id,
		area.id,
		area.revision,
		request.port,
		limits,
		now,
	)?;
	if request.command.trim().is_empty() {
		return Err(Error::Invalid("live preview command is empty".into()));
	}
	let holder = Holder::LivePreview {
		live_preview_id: id,
	};
	scope
		.retain_revision(area.id, area.revision, &holder)
		.await?;
	if let Err(error) = scope.insert(&preview).await {
		// The hold must not outlive a start that never recorded its row.
		let _ = scope.release(&holder).await;
		return Err(error);
	}
	Ok(preview)
}

/// The Runner reported the declared port accepting: the preview now serves.
pub async fn serving(
	scope: &mut dyn Scope,
	mut preview: LivePreview,
	pod_host: impl Into<String>,
	egress_expires_at: Option<DateTime<Utc>>,
	startup_log: &str,
	now: DateTime<Utc>,
) -> Result<Started> {
	preview.serving(pod_host, now);
	if let Some(expires_at) = egress_expires_at {
		preview.egress_expires_at = Some(expires_at);
		scope.egress_expiry(preview.id, expires_at).await?;
	}
	scope.save(&preview).await?;
	let area_revision = scope
		.area_revision(preview.area_id)
		.await?
		.unwrap_or(preview.revision);
	Ok(Started {
		status: Status::of(&preview, area_revision),
		startup_log: startup_tail(startup_log),
	})
}

/// The start never served: release the revision's hold and record why.
pub async fn failed(
	scope: &mut dyn Scope,
	mut preview: LivePreview,
	reason: StopReason,
	startup_log: &str,
) -> Result<Started> {
	preview.stopped(reason);
	scope.save(&preview).await?;
	scope
		.release(&Holder::LivePreview {
			live_preview_id: preview.id,
		})
		.await?;
	Ok(Started {
		status: Status::of(&preview, preview.revision),
		startup_log: startup_tail(startup_log),
	})
}

/// The Run's Live preview in its current state, with staleness resolved
/// against the Working Area's revision.
pub async fn status(scope: &mut dyn Scope, run: Uuid) -> Result<Option<Status>> {
	let Some(preview) = scope.current(run).await? else {
		return Ok(None);
	};
	let area_revision = scope
		.area_revision(preview.area_id)
		.await?
		.unwrap_or(preview.revision);
	Ok(Some(Status::of(&preview, area_revision)))
}

/// Begin a stop: it stops serving at once, and its hold stays until the Runner
/// proves the Pod is gone. Repeating the call keeps the first reason.
pub async fn stop(scope: &mut dyn Scope, id: Uuid, reason: StopReason) -> Result<LivePreview> {
	let mut preview = scope
		.by_id(id)
		.await?
		.ok_or_else(|| Error::NotFound("live preview unavailable".into()))?;
	if preview.state.is_stopped() {
		return Ok(preview);
	}
	preview.stopping(reason);
	scope.save(&preview).await?;
	Ok(preview)
}

/// The Runner proved the Pod stopped: settle the row and release the holds, so
/// reclamation may erase the File versions it kept.
pub async fn settled(scope: &mut dyn Scope, id: Uuid) -> Result<Option<LivePreview>> {
	let Some(mut preview) = scope.by_id(id).await? else {
		return Ok(None);
	};
	if !preview.state.is_stopped() {
		return Ok(Some(preview));
	}
	if !preview.termination_confirmed {
		preview.termination_confirmed = true;
		scope.save(&preview).await?;
		scope
			.release(&Holder::LivePreview {
				live_preview_id: preview.id,
			})
			.await?;
	}
	Ok(Some(preview))
}

/// What one sweep pass decided: previews whose Pod must still be stopped, and
/// how many running previews a later revision left stale.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Sweep {
	pub terminate: Vec<LivePreview>,
	pub stale: usize,
}

/// Apply the idle and absolute limits and mark stale previews. The caller then
/// terminates each `terminate` entry through the Runner and settles it.
pub async fn sweep(scope: &mut dyn Scope, now: DateTime<Utc>) -> Result<Sweep> {
	let mut sweep = Sweep::default();
	for mut preview in scope.unsettled().await? {
		if preview.state.is_stopped() {
			if preview.needs_termination() {
				sweep.terminate.push(preview);
			}
			continue;
		}
		if let Some(reason) = preview.expired(now) {
			preview.stopping(reason);
			scope.save(&preview).await?;
			sweep.terminate.push(preview);
			continue;
		}
		if let Some(revision) = scope.area_revision(preview.area_id).await?
			&& revision > preview.revision
		{
			// Stale is a visible state, not a stop: the pinned revision keeps
			// serving until a later turn restarts it.
			preview.state = State::Stale;
			scope.save(&preview).await?;
			sweep.stale += 1;
		}
	}
	Ok(sweep)
}

/// How long a start may wait for the Runner's port.
pub fn start_deadline(now: DateTime<Utc>) -> DateTime<Utc> {
	now + chrono::TimeDelta::seconds(START_SECONDS)
}

#[cfg(test)]
mod tests;
