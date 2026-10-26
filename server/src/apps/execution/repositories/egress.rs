//! The gateway never trusts a Pod to name its Run, Tenant or credential.
use super::super::capabilities::models::egress::{Budget, RequestRecord};
use crate::{
	Error, Result,
	authorization::{access::Access, identity::SubjectIdentity},
	store::Store,
};
use aidash_application::egress::{Admission, Authority, Direction, Meter};
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use aidash_domain::egress::Limits;
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
}

fn token_mac(store: &Store, operation: Uuid, expires: DateTime<Utc>) -> Result<Hmac<Sha256>> {
	let runner = store
		.capabilities
		.0
		.runner
		.as_ref()
		.ok_or_else(|| Error::Invalid("Egress gateway requires a Runner".into()))?;
	let key = std::env::var(&runner.token_env)
		.map_err(|_| Error::Invalid("Egress credential unavailable".into()))?;
	let key = secrecy::SecretString::from(key);
	let mut mac = Hmac::<Sha256>::new_from_slice(key.expose_secret().as_bytes()).expect("HMAC key");
	mac.update(b"aidash-operation-egress-v1\0");
	mac.update(operation.as_bytes());
	mac.update(&expires.timestamp_micros().to_be_bytes());
	Ok(mac)
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

struct RequestMeter {
	store: Store,
	identity: SubjectIdentity,
	operation: Uuid,
	request: Uuid,
	service: String,
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
			let credential = match &definition.secret {
				Some(name) => Some(
					self.store
						.tenant_secrets
						.as_ref()
						.ok_or_else(|| Error::Invalid("secret_unavailable".into()))?
						.resolve_value(&identity.tenant, name, &definition.base_url)
						.await?,
				),
				None => None,
			};
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
			let record = RequestRecord::build()
				.id(id)
				.tenant(identity.tenant.clone())
				.run_id(run.id)
				.operation_id(operation)
				.service(service.to_owned())
				.method(method.to_owned())
				.path(path.to_owned())
				.query_digest(query_digest.to_owned())
				.status(0)
				.sent_bytes(0)
				.received_bytes(0)
				.duration_ms(0)
				.error(Some("stream_interrupted".to_owned()))
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
				meter: Arc::new(RequestMeter {
					store: self.store.clone(),
					identity,
					operation,
					request: id,
					service: service.to_owned(),
				}),
			})
		}
		.await;
		result.map_err(Into::into)
	}
}

#[async_trait]
impl Meter for RequestMeter {
	async fn charge(&self, direction: Direction, bytes: usize) -> aidash_application::Result<()> {
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
