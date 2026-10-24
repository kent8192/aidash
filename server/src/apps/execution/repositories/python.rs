//! Heap row and authority guards preserve the original lock and positive-stop protocols.
use super::capability_records::{domain, native};
use crate::apps::execution::capabilities::{
	serializers::{contracts::Area as NativeArea, python::Python as NativePython},
	services::{
		operations, packages,
		records::{self, Record as NativeRecord},
		sessions,
	},
};
use crate::{
	Result as NativeResult,
	authorization::{access::Access, identity::SubjectIdentity},
	domain::Run,
	store::Store,
};
use aidash_application::tenancy::{DeploymentScopeReason, TenancyScope};
use aidash_application::{
	Error, Result,
	ports::capabilities::python::{
		HeapAuthority, HeapMaintenance, HeapOperation, Limits, PythonRepository, PythonScope,
	},
};
use aidash_domain::{
	RunMetadata,
	capabilities::{operations::ShellRequest, python::Python, records::Record, sessions::Area},
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, LockType, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SimpleExpr,
};
use serde_json::{Value, json};
use uuid::Uuid;

/// The capability worker reaches every Tenant's operations and work areas.
const MAINTENANCE: TenancyScope =
	TenancyScope::Deployment(DeploymentScopeReason::CapabilityMaintenance);
pub(crate) struct Scope<'a> {
	pub(crate) store: Option<&'a Store>,
	pub(crate) access: &'a mut Access,
	pub(crate) run: Option<&'a Run>,
}
pub(crate) struct Repository<'a> {
	pub(crate) store: &'a Store,
}
struct Authority {
	access: Box<Access>,
}
struct Maintenance<'a> {
	store: &'a Store,
	tx: crate::database::native::Transaction,
}
fn missing() -> Error {
	Error::External("Python repository scope invariant".into())
}
impl From<NativePython> for Python {
	fn from(v: NativePython) -> Self {
		Self {
			idempotency_key: v.idempotency_key,
			code: v.code,
			expected_revision: v.expected_revision,
			expected_session_id: v.expected_session_id,
			timeout_seconds: v.timeout_seconds,
		}
	}
}
#[async_trait]
impl PythonScope for Scope<'_> {
	fn subjects(&self) -> &[String] {
		&self.access.subjects
	}
	fn credential(&self) -> Uuid {
		self.access.identity.credential_id
	}
	fn policy_revision(&self) -> i64 {
		self.access.snapshot.revision
	}
	fn limits(&self) -> Result<Limits> {
		let p = &self.store.ok_or_else(missing)?.capabilities.0;
		Ok(Limits {
			admission: p.admission,
			command_bytes: p.limits.command_bytes,
			idle_seconds: p.idle_seconds,
			image: p.runner.as_ref().map(|p| p.image.clone()),
		})
	}
	async fn load(&mut self, area: Uuid) -> Result<Record> {
		records::get(self.access, area, "python_session")
			.await
			.map(domain)
			.map_err(Into::into)
	}
	async fn create(&mut self, area: Uuid, data: Value) -> Result<Record> {
		records::insert(
			self.access,
			area,
			Some(area),
			"python_session",
			"initial",
			data,
			None,
		)
		.await
		.map(domain)
		.map_err(Into::into)
	}
	async fn update(&mut self, record: &mut Record) -> Result<()> {
		let mut row = native(record);
		records::update(self.access, &mut row).await?;
		*record = domain(row);
		Ok(())
	}
	async fn confirm_stopped(&mut self, record: &Record) -> Result<()> {
		confirm_stopped(self.store.ok_or_else(missing)?, &mut self.access.tx, record).await
	}
	async fn cached(&mut self, key: Uuid, digest: &str) -> Result<Option<Value>> {
		sessions::cached(self.access, key, digest)
			.await
			.map_err(Into::into)
	}
	async fn cache(&mut self, key: Uuid, digest: &str, value: &Value) -> Result<()> {
		sessions::cache(self.access, key, digest, value)
			.await
			.map_err(Into::into)
	}
	async fn previous_result(&mut self, id: Uuid) -> Result<Value> {
		let store = self.store.ok_or_else(missing)?;
		let operation = operations::get(self.access, id).await?;
		Ok(json!(
			operations::result(store, self.access, &operation, 0).await?
		))
	}
	async fn current_run(&mut self, area: &Area) -> Result<Option<Uuid>> {
		sessions::status(self.access, &area.clone().into())
			.await
			.map(|s| s.active_run_id)
			.map_err(Into::into)
	}
	async fn require_write(&mut self, area: &Area) -> Result<()> {
		sessions::authorize(self.access, &area.clone().into(), "file.write")
			.await
			.map_err(Into::into)
	}
	async fn health(&mut self) -> Result<Value> {
		operations::verified_health(self.store.ok_or_else(missing)?, true)
			.await
			.map_err(Into::into)
	}
	async fn request(&mut self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
		let method = reqwest::Method::from_bytes(method.as_bytes()).map_err(|_| missing())?;
		operations::remote(self.store.ok_or_else(missing)?, method, path, body)
			.await
			.map_err(Into::into)
	}
	async fn environment(&mut self, area: &Area) -> Result<Value> {
		packages::environment(
			self.store.ok_or_else(missing)?,
			self.access,
			&area.clone().into(),
		)
		.await
		.map_err(Into::into)
	}
	async fn prepare(
		&mut self,
		area: &mut Area,
		input: ShellRequest,
		extra: Value,
	) -> Result<Value> {
		let mut row: NativeArea = area.clone().into();
		let result = operations::prepare_kind(
			self.store.ok_or_else(missing)?,
			self.access,
			self.run.ok_or_else(missing)?,
			&mut row,
			input.into(),
			"code_interpreter",
			extra,
		)
		.await;
		*area = row.into();
		result.map_err(Into::into)
	}
	async fn event(&mut self, workspace: Uuid, kind: &str, data: Value) -> Result<()> {
		self.store
			.ok_or_else(missing)?
			.event(&mut self.access.tx, Some(workspace), kind, data)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
}
#[async_trait]
impl PythonRepository for Repository<'_> {
	fn admission(&self) -> bool {
		self.store.capabilities.0.admission
	}
	fn idle_seconds(&self) -> u64 {
		self.store.capabilities.0.idle_seconds
	}
	async fn frozen(&self, after: Uuid) -> Result<Vec<Record>> {
		let store = self.store;
		let result: NativeResult<Vec<NativeRecord>> = async {
			let rows = {
				let query_bind_1 = after;
				crate::database::native::query_as(
					&sessions::select("core_records")
						.and_where(
							Expr::col(Alias::new("kind"))
								.eq(reinhardt::query::Expr::value("python_session")),
						)
						.and_where(
							Expr::col(Alias::new("state"))
								.eq(reinhardt::query::Expr::value("frozen")),
						)
						.and_where(
							Expr::col(Alias::new("id")).gt(Expr::value(query_bind_1.to_owned())),
						)
						.order_by(Alias::new("id"), Order::Asc)
						.limit(32)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_all(store.pool.in_scope(&MAINTENANCE))
				.await?
			};
			Ok(rows)
		}
		.await;
		result
			.map(|rows| rows.into_iter().map(domain).collect())
			.map_err(Into::into)
	}
	async fn operation(&self, id: Uuid) -> Result<HeapOperation> {
		let store = self.store;
		let result: NativeResult<operations::Operation> = async {
			let row = {
				let query_bind_1 = id;
				crate::database::native::query_as(
					&sessions::select("core_operations")
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_one(store.pool.in_scope(&MAINTENANCE))
				.await?
			};
			Ok(row)
		}
		.await;
		result
			.map(|o| HeapOperation {
				credential_id: o.credential_id,
				tenant: o.tenant,
				principal: o.principal,
				subjects: o.subjects,
				run_id: o.run_id,
				policy_revision: o.policy_revision,
				area_id: o.area_id,
			})
			.map_err(Into::into)
	}
	async fn authority(&self, o: &HeapOperation) -> Result<Box<dyn HeapAuthority + '_>> {
		let identity = SubjectIdentity {
			http_session: None,
			credential_id: o.credential_id,
			tenant: o.tenant.clone(),
			subject: o.principal.clone(),
		};
		let access = Access::begin(self.store, &identity).await?;
		Ok(Box::new(Authority {
			access: Box::new(access),
		}))
	}
	async fn begin(&self) -> Result<Box<dyn HeapMaintenance + '_>> {
		Ok(Box::new(Maintenance {
			store: self.store,
			tx: self.store.pool.begin(&MAINTENANCE).await?,
		}))
	}
}
#[async_trait]
impl HeapAuthority for Authority {
	fn set_subjects(&mut self, subjects: Vec<String>) {
		self.access.subjects = subjects;
	}
	fn policy_revision(&self) -> i64 {
		self.access.snapshot.revision
	}
	async fn interaction_run(&mut self, id: Uuid) -> Result<RunMetadata> {
		self.access
			.run_for_interaction(id)
			.await
			.map(|run| run.metadata())
			.map_err(Into::into)
	}
	async fn context_authority(&mut self, run: &RunMetadata) -> Result<()> {
		sessions::context_authority(&mut self.access, run)
			.await
			.map_err(Into::into)
	}
	async fn finish(self: Box<Self>, result: Result<()>) -> Result<()> {
		self.access
			.finish(result.map_err(Into::into))
			.await
			.map_err(Into::into)
	}
}
#[async_trait]
impl HeapMaintenance for Maintenance<'_> {
	async fn locked(&mut self, snapshot: &Record, operation: &HeapOperation) -> Result<Record> {
		let result: NativeResult<NativeRecord> = async {
			let tx = &mut self.tx;
			let _: Uuid = {
				let query_bind_1 = operation.area_id;
				crate::database::native::query_scalar(
					&Query::select()
						.column(Alias::new("id"))
						.from(Alias::new("core_areas"))
						.and_where(
							reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(
								SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								),
							),
						)
						.lock(LockType::Update)
						.to_string(PostgresQueryBuilder),
				)
				.scalar_one(&mut **tx)
				.await?
			};
			let current = {
				let query_bind_1 = snapshot.id;
				crate::database::native::query_as(
					&sessions::select("core_records")
						.and_where(
							Expr::col(Alias::new("id")).eq(Expr::value(query_bind_1.to_owned())),
						)
						.and_where(
							Expr::col(Alias::new("kind"))
								.eq(reinhardt::query::Expr::value("python_session")),
						)
						.lock(LockType::Update)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_one(&mut **tx)
				.await?
			};
			Ok(current)
		}
		.await;
		result.map(domain).map_err(Into::into)
	}
	async fn request(&mut self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
		let method = reqwest::Method::from_bytes(method.as_bytes()).map_err(|_| missing())?;
		operations::remote(self.store, method, path, body)
			.await
			.map_err(Into::into)
	}
	async fn update(&mut self, record: &mut Record) -> Result<()> {
		let mut row = native(record);
		records::update_committed(&mut self.tx, &mut row).await?;
		*record = domain(row);
		Ok(())
	}
	async fn confirm_stopped(&mut self, record: &Record) -> Result<()> {
		confirm_stopped(self.store, &mut self.tx, record).await
	}
	async fn finish(self: Box<Self>, result: Result<bool>) -> Result<()> {
		match result {
			Ok(true) => self.tx.commit().await.map_err(Into::into),
			Ok(false) => Ok(()),
			Err(error) => Err(error),
		}
	}
}

/// Both release and revoked/idle maintenance hold Area then session-record locks.
/// A session may contain multiple cell epochs in one physical generation. Keep
/// each command receipt intact and record only execution-writer termination;
/// allocation collectors and their billing journal have an independent lifetime.
async fn confirm_stopped(
	store: &Store,
	tx: &mut crate::database::native::Transaction,
	record: &Record,
) -> Result<()> {
	use reinhardt::query::{BinOper, types::PgBinOper};
	// An initial session may never have admitted a cell; there is no receipt.
	if record.data["operation_id"].is_null() {
		return Ok(());
	}
	let instance = record.data["instance"].as_str().ok_or(Error::Forbidden)?;
	let session = record.data["session_id"].as_str().ok_or(Error::Forbidden)?;
	let health = operations::remote(store, reqwest::Method::GET, "/v1/health", None).await?;
	// A replacement journal can answer "absent" to stop. That is not proof
	// about the saved writer, so retain its only session binding for retry.
	if health["instance"].as_str() != Some(instance) {
		return Err(Error::Conflict("PYTHON_RUNNER_CHANGED".into()));
	}
	let id: Uuid = serde_json::from_value(record.data["operation_id"].clone())?;
	let anchor: operations::Operation = crate::database::native::query_as(
		&sessions::select("core_operations")
			.and_where(Expr::col(Alias::new("id")).eq(Expr::value(id)))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&mut **tx)
	.await?;
	if anchor.tenant != record.tenant
		|| Some(anchor.area_id) != record.area_id
		|| anchor.kind != "code_interpreter"
		|| anchor.input["session_id"].as_str() != Some(session)
		|| anchor
			.runner_instance
			.as_deref()
			.is_some_and(|saved| saved != instance)
		|| (anchor.runner_instance.is_none()
			&& anchor.state != "prepared"
			&& anchor.result["effects_may_have_occurred"] != false)
	{
		return Err(Error::Conflict("PYTHON_OPERATION_CHANGED".into()));
	}
	crate::database::native::query(
		&Query::update()
			.table(Alias::new("core_operations"))
			.value_expr(
				Alias::new("result"),
				Expr::col(Alias::new("result")).binary(
					BinOper::PgOperator(PgBinOper::Concatenate),
					Expr::value(json!({"termination_confirmed":true})),
				),
			)
			.and_where(Expr::col(Alias::new("tenant")).eq(Expr::value(record.tenant.clone())))
			.and_where(Expr::col(Alias::new("area_id")).eq(Expr::value(anchor.area_id)))
			.and_where(Expr::col(Alias::new("kind")).eq("code_interpreter"))
			.and_where(Expr::col(Alias::new("generation")).eq(anchor.generation))
			.and_where(Expr::col(Alias::new("epoch")).lte(anchor.epoch))
			.and_where(Expr::col(Alias::new("runner_instance")).eq(instance))
			.and_where(
				Expr::col(Alias::new("input"))
					.binary(
						BinOper::PgOperator(PgBinOper::JsonGetAsText),
						Expr::value("session_id"),
					)
					.eq(session),
			)
			.and_where(Expr::col(Alias::new("state")).is_in([
				"completed",
				"cancelled",
				"failed",
				"withdrawn",
				"uncertain",
			]))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **tx)
	.await?;
	Ok(())
}
