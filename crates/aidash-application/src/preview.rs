//! Issue one-time Live preview tickets and recheck authority at the gateway.
use crate::{Error, Result};
use aidash_domain::preview::{
	PreviewKind, PreviewLabel, PreviewSecret, TICKET_PATH, TICKET_SECONDS, origin,
};
use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The largest File version the gateway buffers for one response.
pub const MAX_PAGE_BYTES: u64 = 64 * 1024 * 1024;

/// Whether this Deployment offers Previews at all, and where.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Availability {
	pub available: bool,
	pub preview_domain: Option<String>,
}

impl Availability {
	pub fn of(domain: Option<&str>) -> Self {
		Self {
			available: domain.is_some(),
			preview_domain: domain.map(str::to_owned),
		}
	}
}

/// A persisted Preview: one random subdomain per viewable thing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
	pub id: Uuid,
	pub label: PreviewLabel,
	pub kind: PreviewKind,
}

/// The ticket facts the adapter stores; the secret itself never is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketIssue {
	pub preview_id: Uuid,
	pub subject: String,
	pub credential_id: Uuid,
	pub origin: String,
	pub digest: String,
	pub expires_at: DateTime<Utc>,
	/// A Federation viewer must keep its Peer Mapping and Run admission current.
	pub peer: Option<Peer>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Peer {
	pub sender: String,
	pub receiver: String,
	pub subject: String,
	pub run_id: Uuid,
}

/// What the dashboard opens: the Preview's ticket URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issued {
	pub url: String,
	pub expires_at: DateTime<Utc>,
}

/// The authority and persistence the ticket use case needs.
#[async_trait]
pub trait Scope: Send {
	/// `None` means this Deployment offers no Previews.
	fn preview_domain(&self) -> Option<&str>;
	fn public_port(&self) -> Option<u16>;
	/// The dashboard origin that requested this ticket.
	fn origin(&self) -> &str;
	fn subject(&self) -> &str;
	fn credential_id(&self) -> Uuid;
	fn peer(&self) -> Option<&Peer> {
		None
	}
	async fn ticket(&mut self, ticket: TicketIssue) -> Result<()>;
	async fn live_preview(&mut self, id: Uuid) -> Result<Preview>;
}

/// A Preview response for one URL path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
	pub tenant: String,
	pub session: Uuid,
	pub frame_ancestor: String,
	pub media_type: String,
	pub content_disposition: Option<String>,
	pub bytes: Vec<u8>,
}

/// The ticket the gateway exchanges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exchange {
	pub cookie: PreviewSecret,
	pub max_age: i64,
}

/// A Live preview the gateway proxies to: its Pod, which never reaches the
/// browser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proxy {
	pub preview_id: Uuid,
	pub tenant: String,
	pub session: Uuid,
	pub frame_ancestor: String,
	/// `<pod address>:<declared port>`; only the gateway reaches it.
	pub upstream: String,
}

/// The gateway's authority: exchange a ticket once, then recheck every request.
#[async_trait]
pub trait Authority: Send + Sync {
	async fn exchange(&self, host: &str, ticket: &str) -> Result<Exchange>;
	async fn view(&self, host: &str, cookie: &str, path: &str) -> Result<Page>;
	/// A Live preview request: recheck authority, refresh the idle timer only
	/// when `activity` is true, and return where to proxy it. A silent socket
	/// probes authority without extending the idle deadline.
	/// It stops the preview when authority is gone.
	async fn live(&self, host: &str, cookie: &str, activity: bool) -> Result<Option<Proxy>>;
	/// Adds the bytes actually sent to the viewing session's record.
	async fn served(&self, tenant: &str, session: Uuid, bytes: u64) -> Result<()>;
}

/// Issue a one-time ticket under the current Run's read authority.
pub async fn issue_live(scope: &mut dyn Scope, live_preview_id: Uuid) -> Result<Issued> {
	// Resolve the resource before checking Deployment configuration, so an
	// unknown or invisible Live preview has the same 404 in every profile.
	let preview = scope.live_preview(live_preview_id).await?;
	let domain = scope
		.preview_domain()
		.ok_or_else(|| Error::Conflict("PREVIEW_UNAVAILABLE".into()))?
		.to_owned();
	issue_ticket(scope, preview, &domain).await
}

async fn issue_ticket(scope: &mut dyn Scope, preview: Preview, domain: &str) -> Result<Issued> {
	let origin = origin(scope.origin())?;
	let secret = PreviewSecret::generate()?;
	let expires_at = Utc::now() + TimeDelta::seconds(TICKET_SECONDS);
	scope
		.ticket(TicketIssue {
			preview_id: preview.id,
			subject: scope.subject().to_owned(),
			credential_id: scope.credential_id(),
			peer: scope.peer().cloned(),
			origin,
			digest: secret.digest(),
			expires_at,
		})
		.await?;
	let host = preview.label.host(domain);
	let port = scope
		.public_port()
		.map(|port| format!(":{port}"))
		.unwrap_or_default();
	Ok(Issued {
		url: format!("https://{host}{port}{TICKET_PATH}?t={}", secret.expose()),
		expires_at,
	})
}

pub mod live;

/// The media type without its parameters.
#[cfg(test)]
mod tests;
