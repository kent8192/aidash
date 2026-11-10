//! Run-owned state for the isolated Journal Query kernel.
use crate::{authorization::access::Access, store::Store};
use aidash_application::{
	Error, Result,
	ports::capabilities::{
		journal_query::{JournalSessions, Session},
		runner::RunnerTransport,
	},
};
use async_trait::async_trait;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use super::core_records::{self, Record};

fn record_id(run: Uuid) -> Uuid {
	let digest = Sha256::digest([b"aidash.journal.session/1:".as_slice(), run.as_bytes()].concat());
	let mut bytes = [0; 16];
	bytes.copy_from_slice(&digest[..16]);
	Uuid::from_bytes(bytes)
}

async fn acknowledge_runner(
	transport: &dyn RunnerTransport,
	operation: &str,
	proof: Value,
) -> Result<()> {
	let reply = transport
		.request(
			"POST",
			&format!("/v1/operations/{operation}/ack"),
			Some(&proof),
		)
		.await?;
	if reply["acknowledged"] != true {
		return Err(Error::Conflict("JOURNAL_RECEIPT_UNCONFIRMED".into()));
	}
	Ok(())
}

async fn lock_active(access: &mut Access, run: Uuid) -> Result<()> {
	use reinhardt::query::{
		Alias, Expr, ExprTrait as _, LockType, PostgresQueryBuilder, Query,
		QueryStatementBuilder as _,
	};
	let phase: Option<String> = crate::database::native::query_scalar(
		&Query::select()
			.column(Alias::new("phase"))
			.from(Alias::new("runs"))
			.and_where(Expr::col("id").eq(Expr::value(run)))
			.and_where(Expr::col("control").eq("ACTIVE"))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(&mut **access.tx)
	.await?;
	if phase.is_none_or(|phase| matches!(phase.as_str(), "COMPLETED" | "FAILED" | "CANCELLED")) {
		return Err(Error::Conflict("RUN_NOT_ACTIVE".into()));
	}
	Ok(())
}

pub(crate) struct Scope<'a> {
	pub access: &'a mut Access,
	pub transport: &'a dyn RunnerTransport,
	pub run: Uuid,
	pub record: Record,
}

impl<'a> Scope<'a> {
	pub async fn open(
		access: &'a mut Access,
		transport: &'a dyn RunnerTransport,
		run: Uuid,
	) -> Result<Self> {
		let id = record_id(run);
		let record = match core_records::get(access, id, "journal_session").await {
			Ok(record) => record,
			Err(crate::Error::NotFound(_)) => {
				core_records::insert(
					access,
					id,
					None,
					"journal_session",
					"initial",
					json!({"run":run,"epoch":0}),
					None,
				)
				.await?
			}
			Err(error) => return Err(error.into()),
		};
		if record.data["run"] != json!(run) {
			return Err(Error::Forbidden);
		}
		Ok(Self {
			access,
			transport,
			run,
			record,
		})
	}

	pub async fn update(&mut self) -> Result<()> {
		core_records::update(self.access, &mut self.record)
			.await
			.map_err(Into::into)
	}

	async fn acknowledge_completed(&mut self) -> Result<()> {
		if self.record.data["receipt_pending"] != true {
			return Ok(());
		}
		use reinhardt::query::{
			Alias, Expr, ExprTrait as _, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
		};
		let key = self.record.data["key"].as_str().ok_or(Error::Forbidden)?;
		let status: Option<String> = crate::database::native::query_scalar(
			&Query::select()
				.column(Alias::new("status"))
				.from(Alias::new("invocations"))
				.and_where(Expr::col("run_id").eq(Expr::value(self.run)))
				.and_where(Expr::col("idempotency_key").eq(Expr::value(key)))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_optional(&mut **self.access.tx)
		.await?;
		if status.as_deref() != Some("COMPLETED") {
			return Err(Error::Conflict("JOURNAL_RECEIPT_PENDING".into()));
		}
		let operation = self.record.data["operation_id"]
			.as_str()
			.ok_or(Error::Forbidden)?;
		let digest = self.record.data["digest"]
			.as_str()
			.ok_or(Error::Forbidden)?;
		acknowledge_runner(self.transport, operation, json!({"digest":digest})).await?;
		self.record.data["receipt_pending"] = json!(false);
		self.update().await
	}
}

#[async_trait]
impl JournalSessions for Scope<'_> {
	async fn load(&mut self, run: Uuid) -> Result<Option<Session>> {
		if run != self.run {
			return Err(Error::Forbidden);
		}
		let Some(id) = self.record.data["session_id"].as_str() else {
			return Ok(None);
		};
		Ok(Some(Session {
			id: Uuid::parse_str(id).map_err(|_| Error::Forbidden)?,
			fingerprint: self.record.data["fingerprint"]
				.as_str()
				.ok_or(Error::Forbidden)?
				.into(),
		}))
	}
	async fn stop(&mut self, session: Uuid) -> Result<bool> {
		let mut cancelled = None;
		if self.record.data["pending"] == true {
			let operation = self.record.data["operation_id"]
				.as_str()
				.ok_or(Error::Forbidden)?;
			let digest = self.record.data["digest"]
				.as_str()
				.ok_or(Error::Forbidden)?;
			let instance = self.record.data["runner_instance"]
				.as_str()
				.ok_or(Error::Forbidden)?;
			let observed = self
				.transport
				.request(
					"POST",
					&format!("/v1/operations/{operation}/cancel"),
					Some(&json!({"digest":digest,"instance":instance})),
				)
				.await?;
			if observed["termination_confirmed"] != true && observed["writer_frozen"] != true {
				return Ok(false);
			}
			cancelled = Some(observed);
		}
		let expected = self.record.data["runner_instance"]
			.as_str()
			.map(|instance| json!({"instance":instance}));
		let stopped = self
			.transport
			.request(
				"POST",
				&format!("/v1/sessions/{session}/stop"),
				expected.as_ref(),
			)
			.await?;
		if stopped["termination_confirmed"] != true {
			return Ok(false);
		}
		if cancelled.is_some() || self.record.data["receipt_pending"] == true {
			let operation = self.record.data["operation_id"]
				.as_str()
				.ok_or(Error::Forbidden)?;
			let digest = self.record.data["digest"]
				.as_str()
				.ok_or(Error::Forbidden)?;
			let mut proof = json!({"digest":digest});
			if let Some(output_digest) = cancelled
				.as_ref()
				.and_then(|observed| observed["output_digest"].as_str())
			{
				proof["output_digest"] = json!(output_digest);
			}
			acknowledge_runner(self.transport, operation, proof).await?;
		}
		Ok(true)
	}
	async fn save(&mut self, run: Uuid, session: &Session) -> Result<()> {
		if run != self.run {
			return Err(Error::Forbidden);
		}
		self.record.data["session_id"] = json!(session.id);
		self.record.data["fingerprint"] = json!(session.fingerprint);
		self.record.data["runner_instance"] = Value::Null;
		self.record.data["operation_id"] = Value::Null;
		self.record.data["file_id"] = Value::Null;
		self.record.data["pending"] = json!(false);
		self.record.data["receipt_pending"] = json!(false);
		self.record.data["fresh"] = json!(true);
		self.record.state = "ready".into();
		self.update().await
	}
}

pub(crate) async fn execute(
	store: &Store,
	run: &aidash_domain::Run,
	tenancy: &aidash_application::tenancy::TenancyScope,
	code: &str,
	target_run: Option<Uuid>,
	range: Option<(u64, u64)>,
	key: &str,
) -> Result<Value> {
	use aidash_application::{
		capabilities::{
			journal_query::{
				self, OpenSession,
				runner::{self, Cell},
			},
			runner::verified_health,
		},
		ports::{capabilities::journal_query::Authority, journal_export::JournalSource},
	};
	let profile = &store.capabilities.0;
	if !profile.admission || code.is_empty() || code.len() > profile.limits.command_bytes {
		return Err(Error::Invalid("JOURNAL_QUERY_UNAVAILABLE".into()));
	}
	let runner_profile = profile.runner.as_ref().ok_or(Error::Forbidden)?;
	let transport = crate::bootstrap::operation_runner(store)?;
	let health = verified_health(
		&transport,
		&crate::bootstrap::runner_health_profile(store)?,
		true,
	)
	.await?;
	let snapshot =
		JournalSource::snapshot(store, tenancy, run.id, target_run.unwrap_or(run.id), range)
			.await?;
	let export = aidash_domain::context::journal_export::to_jsonl(&snapshot.lines).into_bytes();
	if export.len() as u64 >= profile.working_bytes {
		return Err(Error::Invalid("JOURNAL_EXPORT_LIMIT".into()));
	}
	let output_limit = (profile.output_bytes as usize).min(16 * 1024);
	let dependency_bytes = serde_json::to_vec(&snapshot.dependencies)?.len();
	if dependency_bytes.saturating_add(2048) >= output_limit {
		return Err(Error::Invalid("JOURNAL_DEPENDENCIES_LIMIT".into()));
	}
	// A JSON control character can require six bytes in the Data envelope.
	let text_limit = (output_limit - dependency_bytes - 2048) / 6;
	let mut access =
		crate::apps::identity::services::execution::access_for_run(store, &run.metadata(), true)
			.await?
			.ok_or(Error::Forbidden)?;
	lock_active(&mut access, run.id).await?;
	let authority = Authority {
		run: run.id,
		credential: access.identity.credential_id,
		subjects: access.subjects.clone(),
		policy_revision: access.snapshot.revision,
		image: runner_profile.image.clone(),
		runner_instance: health["instance"].as_str().ok_or(Error::Forbidden)?.into(),
		configuration: aidash_domain::registry::rules::digest(&json!([
			target_run.unwrap_or(run.id),
			health["probe"]["resources"],
			profile.operation_seconds,
			profile.maximum_seconds,
			profile.idle_seconds,
			profile.output_bytes
		])),
		generation: snapshot.generation.clone(),
		range,
	};
	let tenant = access.identity.tenant.clone();
	let mut scope = Scope::open(&mut access, &transport, run.id).await?;
	if scope.record.data["key"] != key
		&& scope.record.data["pending"] != true
		&& scope.record.data["fresh"] != true
		&& let Some(session) = scope.record.data["session_id"].as_str()
		&& !scope.record.data["operation_id"].is_null()
	{
		let live = transport
			.request("GET", &format!("/v1/sessions/{session}"), None)
			.await?;
		if live["live"] != true {
			scope.record.data["fingerprint"] = json!("interpreter_lost_or_idle");
		}
	}
	let session = match journal_query::open_session(&mut scope, &authority).await? {
		OpenSession::Ready { session_id } => session_id,
		OpenSession::Reset { session_id } => {
			drop(scope);
			access.finish(Ok(())).await?;
			return Ok(
				json!({"session_reset":true,"session_id":session_id,"error":{"code":"SESSION_RESET"}}),
			);
		}
	};
	let digest = aidash_domain::registry::rules::digest(&json!([
		"journal_query/1",
		run.id,
		key,
		code,
		range,
		snapshot.generation,
		format!("{:x}", Sha256::digest(&export))
	]));
	let same = scope.record.data["key"] == key;
	if same && scope.record.data["digest"] != digest {
		return Err(Error::Conflict("JOURNAL_INPUT_CHANGED".into()));
	}
	if !same && scope.record.data["pending"] == true {
		return Err(Error::Conflict("JOURNAL_SESSION_BUSY".into()));
	}
	if !same {
		scope.acknowledge_completed().await?;
	}
	if !same {
		let epoch = scope.record.data["epoch"]
			.as_i64()
			.unwrap_or(0)
			.checked_add(1)
			.ok_or(Error::Forbidden)?;
		scope.record.data["key"] = json!(key);
		scope.record.data["digest"] = json!(digest);
		scope.record.data["operation_id"] = json!(Uuid::new_v4());
		scope.record.data["runner_instance"] = health["instance"].clone();
		scope.record.data["file_id"] = json!(Uuid::new_v4());
		scope.record.data["epoch"] = json!(epoch);
		scope.record.data["pending"] = json!(true);
		scope.update().await?;
	}
	let operation: Uuid = serde_json::from_value(scope.record.data["operation_id"].clone())?;
	let file: Uuid = serde_json::from_value(scope.record.data["file_id"].clone())?;
	let epoch = scope.record.data["epoch"]
		.as_i64()
		.ok_or(Error::Forbidden)?;
	drop(scope);
	access.finish(Ok(())).await?;
	let cell = Cell {
		operation,
		area: run.id,
		session,
		runner_instance: health["instance"].as_str().ok_or(Error::Forbidden)?,
		file,
		epoch,
		key_digest: &digest,
		tenant: &tenant,
		code,
		seconds: profile.operation_seconds,
		export: &export,
	};
	let mut observed = runner::submit(&transport, &cell).await?;
	let deadline = tokio::time::Instant::now()
		+ std::time::Duration::from_secs(profile.maximum_seconds.saturating_add(90));
	while matches!(
		observed["status"].as_str(),
		Some("accepted" | "starting" | "running" | "finishing" | "cancelling")
	) {
		if tokio::time::Instant::now() >= deadline {
			return Err(Error::Conflict("JOURNAL_OPERATION_PENDING".into()));
		}
		tokio::time::sleep(std::time::Duration::from_millis(250)).await;
		observed = transport
			.request("GET", &format!("/v1/operations/{operation}"), None)
			.await?;
	}
	let mut output = match runner::output(&observed, text_limit) {
		Ok(output) => output,
		Err(error) => {
			if matches!(
				observed["status"].as_str(),
				Some("failed" | "cancelled" | "uncertain")
			) {
				reset_failed(store, run, &transport).await?;
			}
			return Err(error);
		}
	};
	if !snapshot.dependencies.is_empty() {
		output["research_derived"] = json!(true);
	}
	output["dependencies"] = json!(snapshot.dependencies);
	if JournalSource::snapshot(store, tenancy, run.id, run.id, None)
		.await?
		.generation
		!= snapshot.generation
	{
		let stopped = transport
			.request(
				"POST",
				&format!("/v1/sessions/{session}/stop"),
				Some(&json!({"instance":health["instance"]})),
			)
			.await?;
		if stopped["termination_confirmed"] != true {
			return Err(Error::Conflict("JOURNAL_TERMINATION_UNCONFIRMED".into()));
		}
		// This result is deliberately discarded, so it may be acknowledged
		// without waiting for an invocation result to be saved.
		acknowledge_runner(&transport, &operation.to_string(), json!({"digest":digest})).await?;
		return Err(Error::Conflict("JOURNAL_GENERATION_CHANGED".into()));
	}
	let mut access =
		crate::apps::identity::services::execution::access_for_run(store, &run.metadata(), true)
			.await?
			.ok_or(Error::Forbidden)?;
	lock_active(&mut access, run.id).await?;
	let mut scope = Scope::open(&mut access, &transport, run.id).await?;
	if scope.record.data["operation_id"] != json!(operation) {
		return Err(Error::Conflict("JOURNAL_OPERATION_CHANGED".into()));
	}
	scope.record.data["pending"] = json!(false);
	scope.record.data["fresh"] = json!(false);
	scope.record.data["receipt_pending"] = json!(true);
	scope.update().await?;
	drop(scope);
	access.finish(Ok(())).await?;
	Ok(output)
}

async fn reset_failed(
	store: &Store,
	run: &aidash_domain::Run,
	transport: &dyn RunnerTransport,
) -> Result<()> {
	let mut access =
		crate::apps::identity::services::execution::access_for_run(store, &run.metadata(), true)
			.await?
			.ok_or(Error::Forbidden)?;
	let mut scope = Scope::open(&mut access, transport, run.id).await?;
	let session = scope.load(run.id).await?.ok_or(Error::Forbidden)?;
	if !scope.stop(session.id).await? {
		return Err(Error::Conflict("JOURNAL_TERMINATION_UNCONFIRMED".into()));
	}
	scope.record.data["pending"] = json!(false);
	scope.record.data["receipt_pending"] = json!(false);
	scope.record.data["fingerprint"] = json!("cell_failed");
	scope.update().await?;
	drop(scope);
	access.finish(Ok(())).await?;
	Ok(())
}

pub(crate) async fn acknowledge(store: &Store, run: &aidash_domain::Run) -> Result<()> {
	let transport = crate::bootstrap::operation_runner(store)?;
	let mut access =
		crate::apps::identity::services::execution::access_for_run(store, &run.metadata(), true)
			.await?
			.ok_or(Error::Forbidden)?;
	let mut scope = Scope::open(&mut access, &transport, run.id).await?;
	scope.acknowledge_completed().await?;
	drop(scope);
	access.finish(Ok(())).await?;
	Ok(())
}

/// Reclaim terminal Runs without relying on their former Subject's authority.
pub(crate) async fn reap(store: &Store, cursor: &mut Uuid) -> Result<()> {
	use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
	use reinhardt::query::{
		Alias, Expr, ExprTrait as _, LockType, Order, PostgresQueryBuilder, Query,
		QueryStatementBuilder as _,
	};
	const MAINTENANCE: TenancyScope =
		TenancyScope::Deployment(DeploymentScopeReason::CapabilityMaintenance);
	let rows: Vec<Record> = crate::database::native::query_as(
		&super::sessions::select("core_records")
			.and_where(Expr::col("kind").eq("journal_session"))
			.and_where(Expr::col("id").gt(Expr::value(*cursor)))
			.order_by(Alias::new("id"), Order::Asc)
			.limit(32)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(store.pool.in_scope(&MAINTENANCE))
	.await?;
	if rows.is_empty() {
		*cursor = Uuid::nil();
		return Ok(());
	}
	let transport = crate::bootstrap::operation_runner(store)?;
	for snapshot in rows {
		*cursor = snapshot.id;
		let result: Result<()> = async {
			let run: Uuid = serde_json::from_value(snapshot.data["run"].clone())?;
			let phase: Option<String> = crate::database::native::query_scalar(
				&Query::select()
					.column(Alias::new("phase"))
					.from(Alias::new("runs"))
					.and_where(Expr::col("id").eq(Expr::value(run)))
					.to_string(PostgresQueryBuilder),
			)
			.scalar_optional(store.pool.in_scope(&MAINTENANCE))
			.await?;
			if phase
				.as_deref()
				.is_some_and(|value| !matches!(value, "COMPLETED" | "FAILED" | "CANCELLED"))
			{
				if snapshot.data["receipt_pending"] == true {
					acknowledge_maintenance(store, &transport, &snapshot).await?;
				}
				return Ok(());
			}
			let mut tx = store.pool.begin(&MAINTENANCE).await?;
			let phase: Option<String> = crate::database::native::query_scalar(
				&Query::select()
					.column(Alias::new("phase"))
					.from(Alias::new("runs"))
					.and_where(Expr::col("id").eq(Expr::value(run)))
					.lock(LockType::Update)
					.to_string(PostgresQueryBuilder),
			)
			.scalar_optional(&mut *tx)
			.await?;
			if phase
				.as_deref()
				.is_some_and(|value| !matches!(value, "COMPLETED" | "FAILED" | "CANCELLED"))
			{
				return Ok(());
			}
			let current: Option<Record> = crate::database::native::query_as(
				&super::sessions::select("core_records")
					.and_where(Expr::col("id").eq(Expr::value(snapshot.id)))
					.and_where(Expr::col("kind").eq("journal_session"))
					.lock(LockType::Update)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut *tx)
			.await?;
			let Some(record) = current else {
				return Ok(());
			};
			let operation = record.data["operation_id"].as_str();
			let digest = record.data["digest"].as_str();
			let mut receipt = None;
			if record.data["pending"] == true {
				let (operation, digest, instance) = (
					operation.ok_or(Error::Forbidden)?,
					digest.ok_or(Error::Forbidden)?,
					record.data["runner_instance"]
						.as_str()
						.ok_or(Error::Forbidden)?,
				);
				let observed = transport
					.request(
						"POST",
						&format!("/v1/operations/{operation}/cancel"),
						Some(&json!({"digest":digest,"instance":instance})),
					)
					.await?;
				if observed["termination_confirmed"] != true && observed["writer_frozen"] != true {
					return Err(Error::Conflict("JOURNAL_TERMINATION_UNCONFIRMED".into()));
				}
				receipt = Some(observed);
			}
			if let Some(session) = record.data["session_id"].as_str() {
				let expected = record.data["runner_instance"]
					.as_str()
					.map(|instance| json!({"instance":instance}));
				let stopped = transport
					.request(
						"POST",
						&format!("/v1/sessions/{session}/stop"),
						expected.as_ref(),
					)
					.await?;
				if stopped["termination_confirmed"] != true {
					return Err(Error::Conflict("JOURNAL_TERMINATION_UNCONFIRMED".into()));
				}
			}
			if let (Some(operation), Some(digest)) = (operation, digest) {
				let mut proof = json!({"digest":digest});
				if let Some(output_digest) = receipt
					.as_ref()
					.and_then(|value| value["output_digest"].as_str())
				{
					proof["output_digest"] = json!(output_digest);
				}
				acknowledge_runner(&transport, operation, proof).await?;
			}
			crate::database::native::query(
				&Query::delete()
					.from_table(Alias::new("core_records"))
					.and_where(Expr::col("id").eq(Expr::value(record.id)))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut *tx)
			.await?;
			tx.commit().await?;
			Ok(())
		}
		.await;
		if let Err(error) = result {
			tracing::warn!(%error, record = %snapshot.id, "Journal kernel cleanup pending");
		}
	}
	Ok(())
}

async fn acknowledge_maintenance(
	store: &Store,
	transport: &dyn RunnerTransport,
	snapshot: &Record,
) -> Result<()> {
	use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
	use reinhardt::query::{
		Alias, Expr, ExprTrait as _, LockType, PostgresQueryBuilder, Query,
		QueryStatementBuilder as _,
	};
	const MAINTENANCE: TenancyScope =
		TenancyScope::Deployment(DeploymentScopeReason::CapabilityMaintenance);
	let mut tx = store.pool.begin(&MAINTENANCE).await?;
	let current: Option<Record> = crate::database::native::query_as(
		&super::sessions::select("core_records")
			.and_where(Expr::col("id").eq(Expr::value(snapshot.id)))
			.and_where(Expr::col("kind").eq("journal_session"))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut *tx)
	.await?;
	let Some(mut record) = current else {
		return Ok(());
	};
	if record.data["receipt_pending"] != true || record.data["pending"] == true {
		return Ok(());
	}
	let run: Uuid = serde_json::from_value(record.data["run"].clone())?;
	let key = record.data["key"].as_str().ok_or(Error::Forbidden)?;
	let status: Option<String> = crate::database::native::query_scalar(
		&Query::select()
			.column(Alias::new("status"))
			.from(Alias::new("invocations"))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.and_where(Expr::col("idempotency_key").eq(Expr::value(key)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(&mut *tx)
	.await?;
	if status.as_deref() != Some("COMPLETED") {
		return Ok(());
	}
	let operation = record.data["operation_id"]
		.as_str()
		.ok_or(Error::Forbidden)?;
	let digest = record.data["digest"].as_str().ok_or(Error::Forbidden)?;
	acknowledge_runner(transport, operation, json!({"digest":digest})).await?;
	record.data["receipt_pending"] = json!(false);
	super::core_records::update_committed(&mut tx, &mut record).await?;
	tx.commit().await?;
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	struct Transport(Value);
	#[async_trait]
	impl RunnerTransport for Transport {
		async fn request(&self, _: &str, _: &str, _: Option<&Value>) -> Result<Value> {
			Ok(self.0.clone())
		}
	}

	#[tokio::test]
	async fn absent_runner_operation_is_not_an_acknowledgement() {
		let proof = json!({"digest":"intent"});
		assert!(
			acknowledge_runner(&Transport(json!({"status":"absent"})), "op", proof.clone())
				.await
				.is_err()
		);
		assert!(
			acknowledge_runner(&Transport(json!({"acknowledged":true})), "op", proof)
				.await
				.is_ok()
		);
	}
}
