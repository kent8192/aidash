//! External service requests from a Scratch Space (#738).
//!
//! Code in an Execution Pod sends `http://services.aidash.internal/<service>/<path>`
//! through its proxy. The Tenant and Scratch Space come from the process's
//! credential, never from the Pod; the Run it names is attribution, not a
//! boundary (ADR 0136). A service is usable when it is on the Scratch Space and
//! a Catalog Approval for the Space's Node, limited to no subjects or Agents,
//! covers one of its versions; the newest such version is used.
use super::super::capabilities::models::egress::{RequestRecord, RunRevocation};
use crate::apps::identity::models::AuthorizationCatalog;
use crate::apps::identity::repositories::{node_deletions, tenant_deletions};
use crate::apps::workspaces::repositories::scratch;
use crate::database::native::Transaction;
use crate::registry::{EntityRef, Entry};
use crate::{Error, Result, store::Store};
use aidash_application::egress::{
	Admission, Direction, ExternalAdmission, Meter, RequestTimes, ScopeSettings, ServiceRefusal,
};
use aidash_application::tenancy::TenancyScope;
use aidash_application::tenant_secrets::{
	CachedRoute, Resolution, Resolved, RouteCache, SecretCall, SecretIdentity, SecretRoute,
};
use aidash_capability::tenant_secret::{Attach, Destination, SecretSubject};
use aidash_domain::egress::{
	ExternalService, SERVICE_APPROVAL_LIMITED, SERVICE_METHOD_FORBIDDEN, SERVICE_NOT_APPROVED,
	SERVICE_NOT_AVAILABLE, ScopeId,
};
use aidash_domain::scratch::ScratchState;
use async_trait::async_trait;
use chrono::Utc;
use reinhardt::db::orm::Model;
use std::sync::Arc;
use uuid::Uuid;

pub(super) fn runner_key(store: &Store) -> Result<secrecy::SecretString> {
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

/// Whether one External service may be used from a Scratch Space now.
enum Usability {
	Usable(Box<Entry>),
	Refused(&'static str),
}

/// The newest version of `service` that a Catalog Approval on `node` covers
/// for everyone. An approval limited to certain subjects or Agents does not
/// count: code in a shared Execution Pod can claim another Run (#738).
async fn usable_version(
	tx: &mut Transaction,
	tenant: &str,
	node: &str,
	service: &str,
) -> Result<Usability> {
	let approvals = AuthorizationCatalog::objects()
		.filter(AuthorizationCatalog::field_tenant().eq(tenant.to_owned()))
		.filter(AuthorizationCatalog::field_node().eq(node.to_owned()))
		.filter(AuthorizationCatalog::field_entry_key().eq(service.to_owned()))
		.filter(AuthorizationCatalog::field_enabled().eq(true))
		.all_with_executor(&mut **tx)
		.await
		.map_err(reinhardt::core::exception::Error::from)?;
	let mut open = Vec::new();
	let mut limited = false;
	for approval in approvals {
		let restricted = approval
			.restriction
			.as_ref()
			.is_some_and(|value| !value.0.is_null());
		if restricted {
			limited = true;
		} else if let Ok(version) = semver::Version::parse(&approval.entry_version) {
			open.push((version, approval.entry_version));
		}
	}
	open.sort_by(|a, b| b.0.cmp(&a.0));
	for (_, version) in open {
		let reference = EntityRef {
			id: service.to_owned(),
			version,
		};
		let entry = match crate::apps::identity::repositories::catalog_approvals::definition(
			tx, &reference,
		)
		.await
		{
			Ok(entry) => entry,
			Err(aidash_application::Error::NotFound(_)) => continue,
			Err(error) => return Err(error.into()),
		};
		if entry.kind != "external_service" {
			continue;
		}
		// A recalled Marketplace version is not used, as for a new Run.
		if crate::apps::registry::repositories::marketplace_recalls::check(&mut **tx, &entry)
			.await
			.is_err()
		{
			continue;
		}
		return Ok(Usability::Usable(Box::new(entry)));
	}
	Ok(Usability::Refused(if limited {
		SERVICE_APPROVAL_LIMITED
	} else {
		SERVICE_NOT_APPROVED
	}))
}

/// What the Scratch Space behind a scope allows now: refused when it is not
/// Active, or its Tenant or Node is deleted.
async fn space_services(
	tx: &mut Transaction,
	scope: &ScopeId,
) -> Result<std::collections::BTreeSet<String>> {
	let space = scratch::read(&mut **tx, &scope.tenant, scope.scratch_space)
		.await?
		.ok_or(Error::Forbidden)?;
	if space.state != ScratchState::Active
		|| space.node.as_str() != scope.node
		|| tenant_deletions::is_deleted(&mut **tx, &scope.tenant).await?
		|| !node_deletions::current_of(&mut **tx, &scope.tenant, vec![scope.node.clone()])
			.await?
			.is_empty()
	{
		return Err(Error::Forbidden);
	}
	Ok(space.settings.external_services)
}

async fn revoked(tx: &mut Transaction, tenant: &str, run: Uuid) -> Result<bool> {
	Ok(!RunRevocation::objects()
		.filter(RunRevocation::field_tenant().eq(tenant.to_owned()))
		.filter(RunRevocation::field_run_id().eq(run))
		.all_with_executor(&mut **tx)
		.await
		.map_err(reinhardt::core::exception::Error::from)?
		.is_empty())
}

/// The services of the Scratch Space that are usable now, by id.
async fn available(
	tx: &mut Transaction,
	scope: &ScopeId,
	services: &std::collections::BTreeSet<String>,
) -> Result<Vec<String>> {
	let mut usable = Vec::new();
	for service in services {
		if let Usability::Usable(_) =
			usable_version(tx, &scope.tenant, &scope.node, service).await?
		{
			usable.push(service.clone());
		}
	}
	Ok(usable)
}

/// Broker routes and Tenant Secret values for External service requests.
#[derive(Clone)]
pub(crate) struct ServiceSecrets {
	store: Store,
	/// Broker routes live for less than their token, keyed by Scratch Space and
	/// service version, so that a newly approved version never reuses a route
	/// minted for an older one's address (ADR 0095).
	routes: Arc<RouteCache>,
}
impl ServiceSecrets {
	pub fn new(store: Store) -> Self {
		Self {
			store,
			routes: Arc::default(),
		}
	}

	async fn brokered_route(
		&self,
		tenant_secrets: &aidash_application::tenant_secrets::Service,
		scope: &ScopeId,
		name: &str,
		key: &str,
		definition: &ExternalService,
	) -> Result<(SecretRoute, SecretIdentity)> {
		if let Some(cached) = self.routes.get(scope.scratch_space, key) {
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
			path_prefix: aidash_application::tenant_secrets::path_directory(base.path()).to_owned(),
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
			subject: SecretSubject::ScratchSpace {
				tenant: scope.tenant.clone(),
				node: scope.node.clone(),
				space: scope.scratch_space,
			},
			max_response_bytes: None,
		};
		let resolution: Resolution = tenant_secrets
			.resolve_with_secret(&scope.tenant, name, &definition.base_url, Some(call))
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
			scope.scratch_space,
			key,
			CachedRoute {
				base: route.base.clone(),
				token: route.token.clone(),
				secret: secret.clone(),
			},
		);
		Ok((route, secret))
	}

	async fn connection_material(
		&self,
		tx: &mut Transaction,
		scope: &ScopeId,
		run: Uuid,
		resource: &str,
		definition: &ExternalService,
	) -> Result<(Option<secrecy::SecretString>, Option<SecretRoute>)> {
		// Use the admission transaction for Run and grant reads. Concurrent
		// admissions must not wait for another slot in the execution pool.
		use crate::apps::identity::models::{AuthorizationBundle, AuthorizationExecution};
		let run = crate::apps::execution::models::Run::read_in(&mut **tx, run, None)
			.await?
			.filter(|r| r.is_local(self.store.deployment.as_str()) && r.node.as_str() == scope.node)
			.ok_or(Error::Forbidden)?;
		let grant = AuthorizationExecution::objects()
			.filter(AuthorizationExecution::field_run_id().eq(run.id))
			.filter(AuthorizationExecution::field_tenant().eq(scope.tenant.clone()))
			.all_with_executor(&mut **tx)
			.await
			.map_err(reinhardt::core::exception::Error::from)?
			.pop()
			.ok_or(Error::Forbidden)?;
		let policy = AuthorizationBundle::lock_snapshot(&mut **tx, &scope.tenant, false).await?;
		if !crate::apps::identity::repositories::approval_preferences::is_person(
			&policy.bundle,
			&grant.root_subject,
		) {
			return Err(Error::Forbidden);
		}
		let owner = aidash_application::oauth_connections::Owner {
			tenant: scope.tenant.clone(),
			subject: grant.root_subject,
		};
		let oauth = self
			.store
			.oauth_connections
			.as_ref()
			.ok_or(Error::Forbidden)?;
		if oauth.calls.is_some() {
			let call = SecretCall {
				destination: aidash_application::tenant_secrets::endpoint_destination(
					&definition.base_url,
				)?,
				methods: definition.methods.clone(),
				attach: Attach::Bearer,
				subject: SecretSubject::ScratchSpace {
					tenant: scope.tenant.clone(),
					node: scope.node.clone(),
					space: scope.scratch_space,
				},
				max_response_bytes: None,
			};
			return Ok((
				None,
				Some(oauth.brokered_route(&owner, resource, call).await?),
			));
		}
		Ok((
			Some(
				oauth
					.space_bearer(&owner, resource, scope.scratch_space)
					.await?
					.access_token,
			),
			None,
		))
	}

	async fn material(
		&self,
		scope: &ScopeId,
		key: &str,
		definition: &ExternalService,
	) -> Result<(
		Option<secrecy::SecretString>,
		Option<SecretRoute>,
		Option<SecretIdentity>,
	)> {
		let Some(name) = definition.secret.as_deref() else {
			return Ok((None, None, None));
		};
		let tenant_secrets = self
			.store
			.tenant_secrets
			.as_ref()
			.ok_or_else(|| Error::Invalid("secret_unavailable".into()))?;
		if tenant_secrets.calls.is_some() {
			let (route, secret) = self
				.brokered_route(tenant_secrets, scope, name, key, definition)
				.await?;
			return Ok((None, Some(route), Some(secret)));
		}
		let resolution = tenant_secrets
			.resolve_with_secret(&scope.tenant, name, &definition.base_url, None)
			.await?;
		match resolution.resolved {
			Resolved::Value(value) => Ok((Some(value), None, resolution.secret)),
			Resolved::Brokered(_) => Err(Error::Invalid(
				"this call path cannot use the Tenant Secret broker".into(),
			)),
		}
	}

	/// Admits one request from `scope` to `service` and writes its request
	/// record. OAuth reads the Run to resolve its owner without locking it. A revoked
	/// Run is refused, and so is a scope whose daily allowance or the
	/// service's daily share is spent (`egress_budget_exhausted`).
	pub(crate) async fn admit(
		&self,
		scope: &ScopeSettings,
		service: &str,
		method: &str,
		path: &str,
		query_digest: &str,
	) -> Result<ExternalAdmission> {
		if path.len() > 4096 || query_digest.len() != 64 {
			return Err(Error::Forbidden);
		}
		// Preparation commands have no Run and get no External service.
		let run = scope.run_id.ok_or(Error::Forbidden)?;
		let store = &self.store;
		let mut tx = store
			.pool
			.begin(&TenancyScope::tenant(&scope.id.tenant)?)
			.await?;
		if revoked(&mut tx, &scope.id.tenant, run).await? {
			return Err(Error::Forbidden);
		}
		let services = space_services(&mut tx, &scope.id).await?;
		let refusal = |code, available| {
			Ok(ExternalAdmission::Refused(ServiceRefusal {
				code,
				available,
			}))
		};
		if !services.contains(service) {
			let available = available(&mut tx, &scope.id, &services).await?;
			tx.commit().await?;
			return refusal(SERVICE_NOT_AVAILABLE, available);
		}
		let entry = match usable_version(&mut tx, &scope.id.tenant, &scope.id.node, service).await?
		{
			Usability::Usable(entry) => *entry,
			Usability::Refused(code) => {
				let available = available(&mut tx, &scope.id, &services).await?;
				tx.commit().await?;
				return refusal(code, available);
			}
		};
		let definition = aidash_domain::egress::definition(&entry)?;
		if !definition.methods.iter().any(|allowed| allowed == method) {
			tx.commit().await?;
			return refusal(SERVICE_METHOD_FORBIDDEN, Vec::new());
		}
		let destination = store.keyless.destination(&definition.base_url)?;
		if destination == aidash_domain::keyless::Destination::Forbidden {
			return Err(Error::Forbidden);
		}
		check_service_limits(store, &definition)?;
		// The Space's daily allowance and the service's own daily share are
		// read in the transaction that writes the request record, before any
		// secret is attached. A spent budget, or a failed read, rolls back:
		// no request record is left unfinished and no broker route is minted.
		super::scope_egress::check_budget(
			&mut tx,
			scope,
			definition.limits.map(|limits| (service, limits)),
		)
		.await?;
		let key = format!("{service}@{}", entry.version);
		let (credential, route, secret) = if let Some(resource) = &definition.oauth_connection {
			match self
				.connection_material(&mut tx, &scope.id, run, resource, &definition)
				.await
			{
				Ok((token, route)) => (token, route, None),
				Err(_) => {
					return Ok(ExternalAdmission::Reconnect {
						resource: resource.clone(),
					});
				}
			}
		} else {
			self.material(&scope.id, &key, &definition).await?
		};
		let id = Uuid::now_v7();
		let record = RequestRecord::build()
			.id(id)
			.tenant(scope.id.tenant.clone())
			.run_id(run)
			.via("pod".to_owned())
			.service(service.to_owned())
			.method(method.to_owned())
			.path(path.to_owned())
			.query_digest(query_digest.to_owned())
			.status(0)
			.sent_bytes(0)
			.received_bytes(0)
			.duration_ms(0)
			.admission_ms(None)
			.settle_ms(None)
			.error(None)
			.secret_id(secret.as_ref().map(|secret| secret.id))
			.secret_version(secret.as_ref().map(|secret| secret.version.clone()))
			.secret_host(secret_host(secret.as_ref(), &definition))
			.created_at(Utc::now())
			.finish();
		RequestRecord::objects()
			.insert_with_executor(&mut *tx, &record)
			.await
			.map_err(reinhardt::core::exception::Error::from)?;
		tx.commit().await?;
		let expires_at = Utc::now()
			+ chrono::Duration::from_std(scope.tunnel_limits.max_lifetime)
				.unwrap_or_else(|_| chrono::Duration::days(1));
		Ok(ExternalAdmission::Admitted(Box::new(Admission {
			destination,
			request: id,
			service: service.to_owned(),
			definition,
			expires_at,
			credential,
			route,
			meter: Arc::new(ServiceMeter {
				store: store.clone(),
				scope: scope.id.clone(),
				run,
				request: id,
				service: EntityRef {
					id: service.to_owned(),
					version: entry.version,
				},
			}),
		})))
	}
}

/// Registration checks limits against the Deployment maximums (ADR 0054).
/// Check again when using a version because those maximums can be lowered.
pub(crate) fn check_service_limits(
	store: &Store,
	definition: &ExternalService,
) -> aidash_domain::Result<()> {
	let ceilings = store
		.capabilities
		.0
		.egress
		.as_ref()
		.map(|egress| egress.limits)
		.unwrap_or_default();
	if let Some(limits) = definition.limits {
		limits.lower_than(ceilings)?;
	}
	Ok(())
}

/// The host a request record names for a Tenant Secret, never its value. A
/// service without a Tenant Secret leaves it null.
fn secret_host(secret: Option<&SecretIdentity>, definition: &ExternalService) -> Option<String> {
	secret.and_then(|_| {
		reqwest::Url::parse(&definition.base_url)
			.ok()
			.and_then(|url| url.host_str().map(str::to_owned))
	})
}

/// Records one request's bytes and rechecks, while it runs, that it may still
/// go on (#738): its Run is not revoked, its Scratch Space is Active and keeps
/// the service, and the admitted version is still approved for everyone on the
/// Space's Node. Scope budgets are charged by `ScopedMeter`, not here.
pub(crate) struct ServiceMeter {
	store: Store,
	scope: ScopeId,
	run: Uuid,
	request: Uuid,
	/// The admitted version; a newer approval does not move a running request.
	service: EntityRef,
}

impl ServiceMeter {
	async fn still_usable(&self, tx: &mut Transaction) -> Result<()> {
		if revoked(tx, &self.scope.tenant, self.run).await? {
			return Err(Error::Forbidden);
		}
		if !space_services(tx, &self.scope)
			.await?
			.contains(&self.service.id)
		{
			return Err(Error::Forbidden);
		}
		let approvals = AuthorizationCatalog::objects()
			.filter(AuthorizationCatalog::field_tenant().eq(self.scope.tenant.clone()))
			.filter(AuthorizationCatalog::field_node().eq(self.scope.node.clone()))
			.filter(AuthorizationCatalog::field_entry_key().eq(self.service.id.clone()))
			.filter(AuthorizationCatalog::field_entry_version().eq(self.service.version.clone()))
			.filter(AuthorizationCatalog::field_enabled().eq(true))
			.all_with_executor(&mut **tx)
			.await
			.map_err(reinhardt::core::exception::Error::from)?;
		let open = approvals.iter().any(|approval| {
			approval
				.restriction
				.as_ref()
				.is_none_or(|value| value.0.is_null())
		});
		if !open {
			return Err(Error::Forbidden);
		}
		Ok(())
	}

	/// Adds forwarded bytes to the request record, then rechecks, in one
	/// transaction, so that a store failure leaves nothing half committed for
	/// the gateway's retry to count twice.
	async fn record_and_recheck(&self, direction: Direction, bytes: i64) -> Result<Result<()>> {
		let mut tx = self
			.store
			.pool
			.begin(&TenancyScope::tenant(&self.scope.tenant)?)
			.await?;
		let mut record = RequestRecord::objects()
			.filter(RequestRecord::field_id().eq(self.request))
			.select_for_update()
			.all_with_executor(&mut *tx)
			.await
			.map_err(reinhardt::core::exception::Error::from)?
			.pop()
			.ok_or(Error::Forbidden)?;
		match direction {
			Direction::Sent => record.sent_bytes = record.sent_bytes.saturating_add(bytes),
			Direction::Received => {
				record.received_bytes = record.received_bytes.saturating_add(bytes)
			}
		}
		RequestRecord::objects()
			.save_with_executor(&mut *tx, &record)
			.await
			.map_err(reinhardt::core::exception::Error::from)?;
		// A refusal is a decision about bytes already forwarded: they stay.
		let decision = match self.still_usable(&mut tx).await {
			Ok(()) => Ok(()),
			Err(Error::Forbidden) => Err(Error::Forbidden),
			Err(error) => return Err(error),
		};
		tx.commit().await?;
		Ok(decision)
	}
}

/// Clamps a counter to its signed column.
fn column(value: u64) -> i64 {
	value.min(i64::MAX as u64) as i64
}

#[async_trait]
impl Meter for ServiceMeter {
	async fn check_active(&self) -> aidash_application::Result<()> {
		let result: Result<()> = async {
			// One transaction, so that the gateway's one-second budget for a
			// check covers all of it.
			let mut tx = self
				.store
				.pool
				.begin(&TenancyScope::tenant(&self.scope.tenant)?)
				.await?;
			self.still_usable(&mut tx).await?;
			tx.commit().await
		}
		.await;
		result.map_err(Into::into)
	}
	async fn charge(&self, direction: Direction, bytes: usize) -> aidash_application::Result<()> {
		let bytes = i64::try_from(bytes).map_err(|_| aidash_application::Error::Forbidden)?;
		// The bytes already crossed the gateway, so they are recorded whatever
		// the recheck decides.
		self.record_and_recheck(direction, bytes)
			.await
			.map_err(aidash_application::Error::from)?
			.map_err(aidash_application::Error::from)
	}
	async fn finish(
		&self,
		status: u16,
		times: RequestTimes,
		error: Option<&str>,
	) -> aidash_application::Result<()> {
		let result: Result<()> = async {
			// Finishing records must remain possible after a refusal.
			let mut tx = self
				.store
				.pool
				.begin(&TenancyScope::tenant(&self.scope.tenant)?)
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
			record.duration_ms = column(times.duration_ms);
			record.admission_ms = Some(column(times.admission_ms));
			record.settle_ms = Some(column(times.settle_ms));
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

/// The newest approved version for each service, or the approval refusal code.
pub(crate) async fn usable_services(
	tx: &mut Transaction,
	tenant: &str,
	node: &str,
	services: &std::collections::BTreeSet<String>,
) -> Result<Vec<(String, std::result::Result<Entry, &'static str>)>> {
	let mut out = Vec::new();
	for service in services {
		let usable = match usable_version(tx, tenant, node, service).await? {
			Usability::Usable(entry) => Ok(*entry),
			Usability::Refused(reason) => Err(reason),
		};
		out.push((service.clone(), usable));
	}
	Ok(out)
}
