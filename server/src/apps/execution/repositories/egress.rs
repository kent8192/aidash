//! The gateway never trusts a Pod to name its Run, Tenant or credential.
use super::super::capabilities::models::{
	CoreLivePreviews, EgressSession,
	egress::{Budget, RequestRecord},
};
use crate::{
	Error, Result,
	authorization::{access::Access, identity::SubjectIdentity},
	store::Store,
};
use aidash_application::egress::{Admission, Authority, Direction, Meter};
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use aidash_application::tenant_secrets::{
	CachedRoute, Resolution, Resolved, RouteCache, SecretCall, SecretIdentity, SecretRoute,
};
use aidash_capability::tenant_secret::{Attach, Destination, SecretSubject};
use aidash_domain::egress::Limits;
use aidash_domain::preview::live::State;
use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use reinhardt::db::orm::Model;
use secrecy::ExposeSecret;
use sha2::Sha256;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone)]
pub(crate) struct Gateway {
	pub store: Store,
	/// Broker routes live for less than their token so a long operation's next
	/// request is routed afresh (ADR 0095).
	routes: Arc<RouteCache>,
}
impl Gateway {
	pub fn new(store: Store) -> Self {
		Self {
			store,
			routes: Arc::default(),
		}
	}

	/// The broker route for one operation and External service. A route is
	/// reused for 50 seconds so a request in a long operation still starts
	/// with a token that is valid; a stale one is minted again.
	async fn brokered_route(
		&self,
		tenant_secrets: &aidash_application::tenant_secrets::Service,
		tenant: &str,
		name: &str,
		service: &str,
		definition: &aidash_domain::egress::ExternalService,
		operation: Uuid,
	) -> Result<(SecretRoute, SecretIdentity)> {
		if let Some(cached) = self.routes.get(operation, service) {
			return Ok((
				SecretRoute {
					base: cached.base,
					token: cached.token,
				},
				cached.secret,
			));
		}
		let base = reqwest::Url::parse(&definition.base_url).map_err(|_| Error::Forbidden)?;
		let destination = Destination {
			origin: base.origin().ascii_serialization(),
			path_prefix: destination_prefix(base.path()),
		};
		let attach = match &definition.authentication {
			aidash_domain::egress::Authentication::Bearer => Attach::Bearer,
			aidash_domain::egress::Authentication::Header { name } => {
				Attach::Header { name: name.clone() }
			}
		};
		let call = SecretCall {
			destination,
			methods: definition.methods.clone(),
			attach,
			subject: SecretSubject::Operation {
				operation,
				tenant: tenant.to_owned(),
			},
			max_response_bytes: None,
		};
		let resolution: Resolution = tenant_secrets
			.resolve_with_secret(tenant, name, &definition.base_url, Some(call))
			.await
			.map_err(|error| match error {
				// The repairable code stays; any other resolution or signing
				// fault is a Deployment Administrator fault (ADR 0095).
				aidash_application::Error::Invalid(message)
					if message == aidash_domain::tenant_secrets::UNAVAILABLE =>
				{
					aidash_application::Error::Invalid(message)
				}
				aidash_application::Error::Invalid(_) | aidash_application::Error::External(_) => {
					aidash_application::Error::Invalid("broker_unavailable".into())
				}
				other => other,
			})?;
		let Resolved::Brokered(route) = resolution.resolved else {
			return Err(Error::Invalid(
				"this call path cannot use the Tenant Secret broker".into(),
			));
		};
		let secret = resolution
			.secret
			.ok_or_else(|| Error::Invalid("secret_unavailable".into()))?;
		self.routes.insert(
			operation,
			service,
			CachedRoute {
				base: route.base.clone(),
				token: route.token.clone(),
				secret: secret.clone(),
			},
		);
		Ok((route, secret))
	}

	/// The value, broker route and Tenant Secret identity a request for
	/// `definition` carries. With a broker configured the value never reaches
	/// this node; a self-hosted node reads the pinned version (ADR 0095).
	/// `owner` is the operation, or a Live preview's session-lifetime owner.
	async fn secret_material(
		&self,
		tenant: &str,
		name: Option<&str>,
		service: &str,
		definition: &aidash_domain::egress::ExternalService,
		owner: Uuid,
	) -> Result<(
		Option<secrecy::SecretString>,
		Option<SecretRoute>,
		Option<SecretIdentity>,
	)> {
		let Some(name) = name else {
			return Ok((None, None, None));
		};
		let tenant_secrets = self
			.store
			.tenant_secrets
			.as_ref()
			.ok_or_else(|| Error::Invalid("secret_unavailable".into()))?;
		if tenant_secrets.calls.is_some() {
			let (route, secret) = self
				.brokered_route(tenant_secrets, tenant, name, service, definition, owner)
				.await?;
			return Ok((None, Some(route), Some(secret)));
		}
		let resolution = tenant_secrets
			.resolve_with_secret(tenant, name, &definition.base_url, None)
			.await?;
		match resolution.resolved {
			Resolved::Value(value) => Ok((Some(value), None, resolution.secret)),
			Resolved::Brokered(_) => Err(Error::Invalid(
				"this call path cannot use the Tenant Secret broker".into(),
			)),
		}
	}
}

/// The host a request record names for a Tenant Secret, never its value. A
/// service without a Tenant Secret leaves it null.
fn secret_host(
	secret: Option<&SecretIdentity>,
	definition: &aidash_domain::egress::ExternalService,
) -> Option<String> {
	secret.and_then(|_| {
		reqwest::Url::parse(&definition.base_url)
			.ok()
			.and_then(|url| url.host_str().map(str::to_owned))
	})
}

/// The External service's base path as the broker joins it: from `/`, with a
/// trailing slash.
fn destination_prefix(path: &str) -> String {
	let mut prefix = path.to_owned();
	if prefix.is_empty() {
		prefix.push('/');
	}
	if !prefix.starts_with('/') {
		prefix.insert(0, '/');
	}
	if !prefix.ends_with('/') {
		prefix.push('/');
	}
	prefix
}

fn token_mac(store: &Store, operation: Uuid, expires: DateTime<Utc>) -> Result<Hmac<Sha256>> {
	Ok(keyed_mac(
		runner_key(store)?.expose_secret().as_bytes(),
		b"aidash-operation-egress-v1\0",
		operation,
		expires,
	))
}

const SESSION_DOMAIN: &[u8] = b"aidash-session-egress-v1\0";
const PAGE_DOMAIN: &[u8] = b"aidash-page-egress-v1\0";

/// Who holds a Live preview's egress credential. The Pod's session token may
/// attach the bound service's Tenant Secret; the page token the Preview gateway
/// mints for the viewer's browser never does (ADR 0069).
#[derive(Clone, Copy, PartialEq, Eq)]
enum SessionKind {
	Pod,
	Page,
}

impl SessionKind {
	fn prefix(self) -> &'static str {
		match self {
			Self::Pod => "session:",
			Self::Page => "page:",
		}
	}

	fn domain(self) -> &'static [u8] {
		match self {
			Self::Pod => SESSION_DOMAIN,
			Self::Page => PAGE_DOMAIN,
		}
	}

	fn via(self) -> &'static str {
		match self {
			Self::Pod => "pod",
			Self::Page => "page",
		}
	}
}

/// The Tenant Secret a credential may attach: the Pod's session yes, the
/// viewer's page never, even when the service declares one.
fn credential_name(
	kind: SessionKind,
	definition: &aidash_domain::egress::ExternalService,
) -> Option<&str> {
	match kind {
		SessionKind::Pod => definition.secret.as_deref(),
		SessionKind::Page => None,
	}
}

/// The page credential is MACed under its own domain, so a Pod's session token
/// can never be replayed as a page token or the other way round.
fn egress_mac(
	store: &Store,
	kind: SessionKind,
	preview: Uuid,
	expires: DateTime<Utc>,
) -> Result<Hmac<Sha256>> {
	Ok(keyed_mac(
		runner_key(store)?.expose_secret().as_bytes(),
		kind.domain(),
		preview,
		expires,
	))
}

/// The MAC a session or page token is: `domain || id || expiry`. Kept separate
/// from the Store so the kinds can be tested against the same key.
fn keyed_mac(key: &[u8], domain: &[u8], preview: Uuid, expires: DateTime<Utc>) -> Hmac<Sha256> {
	let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC key");
	mac.update(domain);
	mac.update(preview.as_bytes());
	mac.update(&expires.timestamp_micros().to_be_bytes());
	mac
}

fn token(
	store: &Store,
	kind: SessionKind,
	preview: Uuid,
	expires: DateTime<Utc>,
) -> Result<String> {
	let signature = URL_SAFE_NO_PAD.encode(
		egress_mac(store, kind, preview, expires)?
			.finalize()
			.into_bytes(),
	);
	Ok(format!("{}{preview}.{signature}", kind.prefix()))
}

fn runner_key(store: &Store) -> Result<secrecy::SecretString> {
	let runner = store
		.capabilities
		.0
		.runner
		.as_ref()
		.ok_or_else(|| Error::Invalid("Egress gateway requires a Runner".into()))?;
	let key = std::env::var(&runner.token_env)
		.map_err(|_| Error::Invalid("Egress credential unavailable".into()))?;
	Ok(secrecy::SecretString::from(key))
}

/// The Pod's session-lifetime token. It may attach a bound service's Tenant
/// Secret, exactly like an operation token; it ends with the preview.
pub(crate) fn session_token(
	preview: Uuid,
	expires: DateTime<Utc>,
	store: &Store,
) -> Result<String> {
	token(store, SessionKind::Pod, preview, expires)
}

/// The page token only the Preview gateway mints, for the viewer's browser. It
/// is never admitted with a Tenant Secret, and the Pod cannot forge it.
pub(crate) fn page_token(preview: Uuid, expires: DateTime<Utc>, store: &Store) -> Result<String> {
	token(store, SessionKind::Page, preview, expires)
}
async fn budget(access: &mut Access, operation: Uuid, service: &str) -> Result<Option<Budget>> {
	Ok(Budget::objects()
		.filter(Budget::field_operation_id().eq(operation))
		.filter(Budget::field_service().eq(service))
		.all_with_executor(&mut **access.tx)
		.await
		.map_err(reinhardt::core::exception::Error::from)?
		.pop())
}
fn debit(row: &mut Budget, debit: aidash_domain::egress::Debit) -> Result<()> {
	let usage = aidash_domain::egress::Usage {
		sent_bytes: u64::try_from(row.sent_bytes).map_err(|_| Error::Forbidden)?,
		received_bytes: u64::try_from(row.received_bytes).map_err(|_| Error::Forbidden)?,
		requests: u32::try_from(row.requests).map_err(|_| Error::Forbidden)?,
	};
	let limits = Limits {
		sent_bytes: row.max_sent_bytes as u64,
		received_bytes: row.max_received_bytes as u64,
		requests: row.max_requests as u32,
	};
	let next = usage.debit(limits, debit)?;
	row.sent_bytes = next.sent_bytes as i64;
	row.received_bytes = next.received_bytes as i64;
	row.requests = next.requests as i32;
	Ok(())
}
fn make_budget(
	tenant: &str,
	operation: Uuid,
	service: &str,
	expires: DateTime<Utc>,
	limits: Limits,
) -> Budget {
	Budget::build()
		.id(Uuid::now_v7())
		.tenant(tenant.to_owned())
		.operation_id(operation)
		.service(service.to_owned())
		.expires_at(expires)
		.deadline_at(None)
		.sent_bytes(0)
		.received_bytes(0)
		.requests(0)
		.max_sent_bytes(limits.sent_bytes as i64)
		.max_received_bytes(limits.received_bytes as i64)
		.max_requests(limits.requests as i32)
		.finish()
}

/// Called on the operation's existing authority transaction before dispatch.
/// Retrying the Runner request reuses exactly the same token and deadline.
pub(crate) async fn attach(
	store: &Store,
	access: &mut Access,
	input: &mut serde_json::Value,
) -> Result<()> {
	let Some(settings) = &store.capabilities.0.egress else {
		return Ok(());
	};
	let operation: Uuid = serde_json::from_value(input["operation_id"].clone())?;
	let row = match budget(access, operation, "").await? {
		Some(row) => row,
		None => {
			let seconds = input["seconds"]
				.as_i64()
				.ok_or_else(|| Error::Invalid("invalid egress deadline".into()))?;
			let row = make_budget(
				&access.identity.tenant,
				operation,
				"",
				// Pod startup has its own 120-second ceiling. The first request
				// pins the earlier deadline measured by the trusted Runner.
				DateTime::from_timestamp_micros(Utc::now().timestamp_micros())
					.expect("current timestamp")
					+ chrono::Duration::seconds(seconds + 120),
				settings.limits,
			);
			Budget::objects()
				.insert_with_executor(&mut **access.tx, &row)
				.await
				.map_err(reinhardt::core::exception::Error::from)?;
			row
		}
	};
	let signature = URL_SAFE_NO_PAD.encode(
		token_mac(store, operation, row.expires_at)?
			.finalize()
			.into_bytes(),
	);
	input["egress"] =
		serde_json::json!({"url":settings.endpoint,"token":format!("{operation}.{signature}")});
	Ok(())
}

async fn live(
	store: &Store,
	identity: &SubjectIdentity,
	operation: Uuid,
) -> Result<(Access, aidash_domain::Run)> {
	let mut access = Access::begin(store, identity).await?;
	// Read the operation without locking first, to retain the Run-before-operation
	// lock order used by normal operation reconciliation.
	let op = super::super::capabilities::models::CoreOperations::objects()
		.filter(super::super::capabilities::models::CoreOperations::field_id().eq(operation))
		.all_with_executor(&mut **access.tx)
		.await
		.map_err(reinhardt::core::exception::Error::from)?
		.pop()
		.ok_or(Error::Forbidden)?;
	access.subjects = serde_json::from_value(op.subjects.0)?;
	let run = access.run_for_interaction(op.run_id).await?;
	let task = access.task_read(run.task_id).await?;
	let resource = access.task_resource(&task).await?;
	access.require(&resource, "task.execute").await?;
	crate::authorization::catalog::entry(
		&mut access,
		&aidash_domain::registry::EntityRef {
			id: run.agent_id.clone(),
			version: run.agent_version.clone(),
		},
		"agent.execute",
	)
	.await?;
	// Serialize the cancellation check before the operation lock, in the same
	// order as Run control. A gateway token never keeps a stopped Run alive.
	let current = crate::apps::execution::models::Run::objects()
		.filter(crate::apps::execution::models::Run::field_id().eq(run.id))
		.select_for_update()
		.all_with_executor(&mut **access.tx)
		.await
		.map_err(reinhardt::core::exception::Error::from)?
		.pop()
		.ok_or(Error::Forbidden)?;
	let current: aidash_domain::Run = current.try_into()?;
	if current.phase().is_terminal() || current.control != aidash_domain::RunControl::Active {
		return Err(Error::Forbidden);
	}
	let operation = super::operations::get(&mut access, operation).await?;
	if !matches!(operation.state.as_str(), "submitted" | "running")
		|| operation.result["termination_confirmed"] == true
	{
		return Err(Error::Forbidden);
	}
	Ok((access, current))
}

/// A session token's authority: the Run may have finished, but its Live
/// preview must still be serving and the Run still Active.
async fn live_session(
	store: &Store,
	identity: &SubjectIdentity,
	run_id: Uuid,
) -> Result<(Access, aidash_domain::Run)> {
	let mut access = Access::begin(store, identity).await?;
	let run = access.run_for_interaction(run_id).await?;
	let task = access.task_read(run.task_id).await?;
	let resource = access.task_resource(&task).await?;
	access.require(&resource, "task.execute").await?;
	crate::authorization::catalog::entry(
		&mut access,
		&aidash_domain::registry::EntityRef {
			id: run.agent_id.clone(),
			version: run.agent_version.clone(),
		},
		"agent.execute",
	)
	.await?;
	let current = crate::apps::execution::models::Run::objects()
		.filter(crate::apps::execution::models::Run::field_id().eq(run.id))
		.select_for_update()
		.all_with_executor(&mut **access.tx)
		.await
		.map_err(reinhardt::core::exception::Error::from)?
		.pop()
		.ok_or(Error::Forbidden)?;
	let current: aidash_domain::Run = current.try_into()?;
	if current.control != aidash_domain::RunControl::Active {
		return Err(Error::Forbidden);
	}
	Ok((access, current))
}

struct RequestMeter {
	store: Store,
	identity: SubjectIdentity,
	operation: Uuid,
	request: Uuid,
	service: String,
	/// The session budget to debit instead of an operation's.
	session: Option<Uuid>,
}

/// A Live preview credential names a preview, not an operation. Only the
/// Pod's kind may attach a bound Tenant Secret (ADR 0069, ADR 0070).
async fn admit_session(
	gateway: &Gateway,
	token: &str,
	kind: SessionKind,
	service: &str,
	method: &str,
	path: &str,
	query_digest: &str,
) -> Result<Admission> {
	let store = &gateway.store;
	if path.len() > 4096 || query_digest.len() != 64 {
		return Err(Error::Forbidden);
	}
	let (id, signature) = token.split_once('.').ok_or(Error::Forbidden)?;
	let preview_id = Uuid::parse_str(id).map_err(|_| Error::Forbidden)?;
	let signature = URL_SAFE_NO_PAD
		.decode(signature)
		.map_err(|_| Error::Forbidden)?;
	let mut tx = store
		.pool
		.begin(&TenancyScope::Deployment(
			DeploymentScopeReason::CapabilityMaintenance,
		))
		.await?;
	let session = EgressSession::objects()
		.filter(EgressSession::field_id().eq(preview_id))
		.all_with_executor(&mut *tx)
		.await
		.map_err(reinhardt::core::exception::Error::from)?
		.pop()
		.ok_or(Error::Forbidden)?;
	let preview = CoreLivePreviews::objects()
		.filter(CoreLivePreviews::field_id().eq(preview_id))
		.all_with_executor(&mut *tx)
		.await
		.map_err(reinhardt::core::exception::Error::from)?
		.pop()
		.ok_or(Error::Forbidden)?;
	tx.commit().await?;
	if session.live_preview_id != preview_id || session.run_id != preview.run_id {
		return Err(Error::Forbidden);
	}
	egress_mac(store, kind, preview_id, session.expires_at)?
		.verify_slice(&signature)
		.map_err(|_| Error::Forbidden)?;
	if session.expires_at <= Utc::now() {
		return Err(Error::Forbidden);
	}
	let state = State::parse(&preview.state)?;
	if !matches!(state, State::Running | State::Stale) {
		return Err(Error::Forbidden);
	}
	let identity = SubjectIdentity {
		http_session: None,
		credential_id: session.credential_id,
		tenant: session.tenant.clone(),
		subject: session.principal.clone(),
	};
	let run = {
		let (access, run) = live_session(store, &identity, preview.run_id).await?;
		access.finish(Ok(())).await?;
		run
	};
	let snapshot = run
		.context
		.binding_snapshot
		.as_deref()
		.ok_or(Error::Forbidden)?;
	snapshot.validate()?;
	let binding = snapshot
		.bindings
		.iter()
		.find(|b| {
			b.definition.kind == "external_service"
				&& b.identity.id == service
				&& b.excluded_reason.is_none()
		})
		.ok_or(Error::Forbidden)?;
	if binding.identity.registry_deployment != store.deployment.as_str() {
		return Err(Error::Forbidden);
	}
	let mut access = live_session(store, &identity, preview.run_id).await?.0;
	let current = crate::authorization::catalog::entry(
		&mut access,
		&binding.identity.local(),
		"registry.read",
	)
	.await?;
	if aidash_domain::registry::rules::digest(&serde_json::to_value(&current)?) != binding.digest {
		return Err(Error::Forbidden);
	}
	crate::marketplace::check_pinned(&mut access, &current).await?;
	let definition = aidash_domain::egress::definition(&current)?;
	let destination = store.keyless.destination(&definition.base_url)?;
	if destination == aidash_domain::keyless::Destination::Forbidden {
		return Err(Error::Forbidden);
	}
	if !definition.methods.iter().any(|allowed| allowed == method) {
		return Err(Error::Forbidden);
	}
	let settings = store
		.capabilities
		.0
		.egress
		.as_ref()
		.ok_or(Error::Forbidden)?;
	// An External service may only lower the Deployment's session limits.
	definition
		.limits
		.unwrap_or(settings.limits)
		.lower_than(settings.limits)?;
	// The Pod's Web server reaches the service with its Tenant Secret; the
	// viewer's page never does, even when the service declares one (ADR 0069).
	// On Cloud and Dedicated the broker attaches it (ADR 0095).
	let (credential, route, secret) = gateway
		.secret_material(
			&identity.tenant,
			credential_name(kind, &definition),
			service,
			&definition,
			preview_id,
		)
		.await?;
	let mut row = EgressSession::objects()
		.filter(EgressSession::field_id().eq(preview_id))
		.select_for_update()
		.all_with_executor(&mut **access.tx)
		.await
		.map_err(reinhardt::core::exception::Error::from)?
		.pop()
		.ok_or(Error::Forbidden)?;
	if row.expires_at <= Utc::now()
		|| row.requests >= row.max_requests
		|| row.sent_bytes >= row.max_sent_bytes
		|| row.received_bytes >= row.max_received_bytes
	{
		return Err(Error::Conflict("egress_limit_exceeded".into()));
	}
	row.requests += 1;
	EgressSession::objects()
		.save_with_executor(&mut **access.tx, &row)
		.await
		.map_err(reinhardt::core::exception::Error::from)?;
	let id = Uuid::now_v7();
	let record = RequestRecord::build()
		.id(id)
		.tenant(identity.tenant.clone())
		.run_id(run.id)
		.operation_id(None)
		.session_id(Some(preview_id))
		.via(kind.via().to_owned())
		.service(service.to_owned())
		.method(method.to_owned())
		.path(path.to_owned())
		.query_digest(query_digest.to_owned())
		.status(0)
		.sent_bytes(0)
		.received_bytes(0)
		.duration_ms(0)
		.error(Some("stream_interrupted".to_owned()))
		.secret_id(secret.as_ref().map(|secret| secret.id))
		.secret_version(secret.as_ref().map(|secret| secret.version.clone()))
		.secret_host(secret_host(secret.as_ref(), &definition))
		.created_at(Utc::now())
		.finish();
	RequestRecord::objects()
		.insert_with_executor(&mut **access.tx, &record)
		.await
		.map_err(reinhardt::core::exception::Error::from)?;
	access.finish(Ok(())).await?;
	Ok(Admission {
		gateway_origin: settings.endpoint.clone(),
		destination,
		request: id,
		operation: preview_id,
		session: Some(preview_id),
		service: service.to_owned(),
		definition,
		expires_at: session.expires_at,
		credential,
		route,
		meter: Arc::new(RequestMeter {
			store: store.clone(),
			identity,
			operation: preview_id,
			request: id,
			service: service.to_owned(),
			session: Some(preview_id),
		}),
	})
}

#[async_trait]
impl Authority for Gateway {
	async fn admit(
		&self,
		token: &str,
		service: &str,
		method: &str,
		path: &str,
		query_digest: &str,
	) -> aidash_application::Result<Admission> {
		if let Some(session) = token.strip_prefix(SessionKind::Pod.prefix()) {
			return admit_session(
				self,
				session,
				SessionKind::Pod,
				service,
				method,
				path,
				query_digest,
			)
			.await
			.map_err(Into::into);
		}
		if let Some(page) = token.strip_prefix(SessionKind::Page.prefix()) {
			return admit_session(
				self,
				page,
				SessionKind::Page,
				service,
				method,
				path,
				query_digest,
			)
			.await
			.map_err(Into::into);
		}
		let result: Result<Admission> = async {
			if path.len() > 4096 || query_digest.len() != 64 {
				return Err(Error::Forbidden);
			}
			let (id, signature) = token.split_once('.').ok_or(Error::Forbidden)?;
			let operation = Uuid::parse_str(id).map_err(|_| Error::Forbidden)?;
			let signature = URL_SAFE_NO_PAD
				.decode(signature)
				.map_err(|_| Error::Forbidden)?;
			let mut tx = self
				.store
				.pool
				.begin(&TenancyScope::Deployment(
					DeploymentScopeReason::CapabilityMaintenance,
				))
				.await?;
			let op = super::super::capabilities::models::CoreOperations::objects()
				.filter(
					super::super::capabilities::models::CoreOperations::field_id().eq(operation),
				)
				.all_with_executor(&mut *tx)
				.await
				.map_err(reinhardt::core::exception::Error::from)?
				.pop()
				.ok_or(Error::Forbidden)?;
			let lease = Budget::objects()
				.filter(Budget::field_operation_id().eq(operation))
				.filter(Budget::field_service().eq(""))
				.all_with_executor(&mut *tx)
				.await
				.map_err(reinhardt::core::exception::Error::from)?
				.pop()
				.ok_or(Error::Forbidden)?;
			tx.commit().await?;
			token_mac(&self.store, operation, lease.expires_at)?
				.verify_slice(&signature)
				.map_err(|_| Error::Forbidden)?;
			if lease.expires_at <= Utc::now() {
				return Err(Error::Forbidden);
			}
			let seconds = op.input.0["seconds"].as_i64().ok_or(Error::Forbidden)?;
			let identity = SubjectIdentity {
				http_session: None,
				credential_id: op.credential_id,
				tenant: op.tenant,
				subject: op.principal,
			};
			let (mut access, run) = live(&self.store, &identity, operation).await?;
			let snapshot = run
				.context
				.binding_snapshot
				.as_deref()
				.ok_or(Error::Forbidden)?;
			snapshot.validate()?;
			let binding = snapshot
				.bindings
				.iter()
				.find(|b| {
					b.definition.kind == "external_service"
						&& b.identity.id == service
						&& b.excluded_reason.is_none()
				})
				.ok_or(Error::Forbidden)?;
			if binding.identity.registry_deployment != self.store.deployment.as_str() {
				return Err(Error::Forbidden);
			}
			let current = crate::authorization::catalog::entry(
				&mut access,
				&binding.identity.local(),
				"registry.read",
			)
			.await?;
			if aidash_domain::registry::rules::digest(&serde_json::to_value(&current)?)
				!= binding.digest
			{
				return Err(Error::Forbidden);
			}
			crate::marketplace::check_pinned(&mut access, &current).await?;
			let definition = aidash_domain::egress::definition(&current)?;
			let destination = self.store.keyless.destination(&definition.base_url)?;
			if destination == aidash_domain::keyless::Destination::Forbidden {
				return Err(Error::Forbidden);
			}
			if !definition.methods.iter().any(|allowed| allowed == method) {
				return Err(Error::Forbidden);
			}
			let settings = self
				.store
				.capabilities
				.0
				.egress
				.as_ref()
				.ok_or(Error::Forbidden)?;
			let limits = definition
				.limits
				.unwrap_or(settings.limits)
				.lower_than(settings.limits)?;
			let (credential, route, secret) = self
				.secret_material(
					&identity.tenant,
					definition.secret.as_deref(),
					service,
					&definition,
					operation,
				)
				.await?;
			let mut total = budget(&mut access, operation, "")
				.await?
				.ok_or(Error::Forbidden)?;
			// The Runner's timestamps come from its trusted collector. Startup
			// consumes no command time, and an ended command cannot reuse its token.
			use aidash_application::ports::capabilities::runner::RunnerTransport as _;
			let observed = crate::bootstrap::operation_runner(&self.store)?
				.request("GET", &format!("/v1/operations/{operation}"), None)
				.await?;
			if !matches!(observed["status"].as_str(), Some("running" | "finishing")) {
				return Err(Error::Forbidden);
			}
			let started = observed["started_at"].as_f64().ok_or(Error::Forbidden)?;
			if !started.is_finite() || seconds <= 0 {
				return Err(Error::Forbidden);
			}
			let deadline = DateTime::from_timestamp_micros((started * 1_000_000.0) as i64)
				.ok_or(Error::Forbidden)?
				.checked_add_signed(chrono::Duration::seconds(seconds))
				.ok_or(Error::Forbidden)?
				.min(total.expires_at);
			let deadline = total.deadline_at.unwrap_or(deadline).min(deadline);
			if deadline <= Utc::now() {
				return Err(Error::Forbidden);
			}
			total.deadline_at = Some(deadline);
			let mut per_service = match budget(&mut access, operation, service).await? {
				Some(row) => row,
				None => {
					let row = make_budget(&identity.tenant, operation, service, deadline, limits);
					Budget::objects()
						.insert_with_executor(&mut **access.tx, &row)
						.await
						.map_err(reinhardt::core::exception::Error::from)?;
					row
				}
			};
			for row in [&mut total, &mut per_service] {
				if row.requests >= row.max_requests || row.expires_at <= Utc::now() {
					return Err(Error::Conflict("egress_limit_exceeded".into()));
				}
				debit(row, aidash_domain::egress::Debit::Request)?;
				Budget::objects()
					.save_with_executor(&mut **access.tx, row)
					.await
					.map_err(reinhardt::core::exception::Error::from)?;
			}
			let id = Uuid::now_v7();
			// The record names where a value went, never what it was. A service
			// without a Tenant Secret leaves all three columns null.
			let secret_host = secret_host(secret.as_ref(), &definition);
			let record = RequestRecord::build()
				.id(id)
				.tenant(identity.tenant.clone())
				.run_id(run.id)
				.operation_id(Some(operation))
				.session_id(None)
				.service(service.to_owned())
				.method(method.to_owned())
				.path(path.to_owned())
				.query_digest(query_digest.to_owned())
				.status(0)
				.sent_bytes(0)
				.received_bytes(0)
				.duration_ms(0)
				.via("operation".to_owned())
				.error(Some("stream_interrupted".to_owned()))
				.secret_id(secret.as_ref().map(|secret| secret.id))
				.secret_version(secret.as_ref().map(|secret| secret.version.clone()))
				.secret_host(secret_host)
				.created_at(Utc::now())
				.finish();
			RequestRecord::objects()
				.insert_with_executor(&mut **access.tx, &record)
				.await
				.map_err(reinhardt::core::exception::Error::from)?;
			access.finish(Ok(())).await?;
			Ok(Admission {
				gateway_origin: settings.endpoint.clone(),
				destination,
				request: id,
				operation,
				service: service.to_owned(),
				definition,
				expires_at: deadline,
				credential,
				route,
				session: None,
				meter: Arc::new(RequestMeter {
					store: self.store.clone(),
					identity,
					operation,
					request: id,
					service: service.to_owned(),
					session: None,
				}),
			})
		}
		.await;
		result.map_err(Into::into)
	}
}

/// Debits a Live preview's session budget and finishes its request record.
async fn charge_session(
	store: &Store,
	identity: &SubjectIdentity,
	request: Uuid,
	session: Uuid,
	direction: Direction,
	bytes: usize,
) -> Result<()> {
	let bytes = i64::try_from(bytes).map_err(|_| Error::Forbidden)?;
	let mut tx = store
		.pool
		.begin(&TenancyScope::tenant(&identity.tenant)?)
		.await?;
	let mut row = EgressSession::objects()
		.filter(EgressSession::field_id().eq(session))
		.select_for_update()
		.all_with_executor(&mut *tx)
		.await
		.map_err(reinhardt::core::exception::Error::from)?
		.pop()
		.ok_or(Error::Forbidden)?;
	if row.expires_at <= Utc::now() {
		return Err(Error::Forbidden);
	}
	match direction {
		Direction::Sent => {
			row.sent_bytes = row.sent_bytes.saturating_add(bytes);
			if row.sent_bytes > row.max_sent_bytes {
				return Err(Error::Conflict("egress_limit_exceeded".into()));
			}
		}
		Direction::Received => {
			row.received_bytes = row.received_bytes.saturating_add(bytes);
			if row.received_bytes > row.max_received_bytes {
				return Err(Error::Conflict("egress_limit_exceeded".into()));
			}
		}
	}
	EgressSession::objects()
		.save_with_executor(&mut *tx, &row)
		.await
		.map_err(reinhardt::core::exception::Error::from)?;
	let mut record = RequestRecord::objects()
		.filter(RequestRecord::field_id().eq(request))
		.select_for_update()
		.all_with_executor(&mut *tx)
		.await
		.map_err(reinhardt::core::exception::Error::from)?
		.pop()
		.ok_or(Error::Forbidden)?;
	match direction {
		Direction::Sent => record.sent_bytes += bytes,
		Direction::Received => record.received_bytes += bytes,
	}
	RequestRecord::objects()
		.save_with_executor(&mut *tx, &record)
		.await
		.map_err(reinhardt::core::exception::Error::from)?;
	tx.commit().await
}

#[async_trait]
impl Meter for RequestMeter {
	async fn charge(&self, direction: Direction, bytes: usize) -> aidash_application::Result<()> {
		if let Some(session) = self.session {
			return charge_session(
				&self.store,
				&self.identity,
				self.request,
				session,
				direction,
				bytes,
			)
			.await
			.map_err(Into::into);
		}
		let result: Result<()> = async {
			let bytes = i64::try_from(bytes).map_err(|_| Error::Forbidden)?;
			let (mut access, _) = live(&self.store, &self.identity, self.operation).await?;
			for service in ["", self.service.as_str()] {
				let mut row = budget(&mut access, self.operation, service)
					.await?
					.ok_or(Error::Forbidden)?;
				if row
					.deadline_at
					.unwrap_or(row.expires_at)
					.min(row.expires_at)
					<= Utc::now()
				{
					return Err(Error::Forbidden);
				}
				debit(
					&mut row,
					match direction {
						Direction::Sent => aidash_domain::egress::Debit::Sent(bytes as u64),
						Direction::Received => aidash_domain::egress::Debit::Received(bytes as u64),
					},
				)?;
				Budget::objects()
					.save_with_executor(&mut **access.tx, &row)
					.await
					.map_err(reinhardt::core::exception::Error::from)?;
			}
			let mut record = RequestRecord::objects()
				.filter(RequestRecord::field_id().eq(self.request))
				.select_for_update()
				.all_with_executor(&mut **access.tx)
				.await
				.map_err(reinhardt::core::exception::Error::from)?
				.pop()
				.ok_or(Error::Forbidden)?;
			match direction {
				Direction::Sent => record.sent_bytes += bytes,
				Direction::Received => record.received_bytes += bytes,
			}
			RequestRecord::objects()
				.save_with_executor(&mut **access.tx, &record)
				.await
				.map_err(reinhardt::core::exception::Error::from)?;
			access.finish(Ok(())).await
		}
		.await;
		result.map_err(Into::into)
	}
	async fn finish(
		&self,
		status: u16,
		elapsed_ms: u64,
		error: Option<&str>,
	) -> aidash_application::Result<()> {
		let result: Result<()> = async {
			// Finishing records must remain possible after cancellation or expiry.
			let mut tx = self
				.store
				.pool
				.begin(&TenancyScope::tenant(&self.identity.tenant)?)
				.await?;
			let Some(mut record) = RequestRecord::objects()
				.filter(RequestRecord::field_id().eq(self.request))
				.select_for_update()
				.all_with_executor(&mut *tx)
				.await
				.map_err(reinhardt::core::exception::Error::from)?
				.pop()
			else {
				return Ok(());
			};
			record.status = i32::from(status);
			record.duration_ms = elapsed_ms.min(i64::MAX as u64) as i64;
			record.error = error.map(str::to_owned);
			RequestRecord::objects()
				.save_with_executor(&mut *tx, &record)
				.await
				.map_err(reinhardt::core::exception::Error::from)?;
			tx.commit().await
		}
		.await;
		result.map_err(Into::into)
	}
}

#[cfg(test)]
mod session_tests;
