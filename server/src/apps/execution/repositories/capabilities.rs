//! Operation admission retains the exact native query expressions and caller transaction.
use crate::{
	Error,
	apps::execution::capabilities::services::{
		contracts::{Area, FileEntry, FileScope, Shell},
		operations::{self, Operation},
		sessions,
	},
	authorization::access::Access,
	domain::Run,
	store::Store,
};
use aidash_application::{
	Result,
	ports::capabilities::{AcceptedOperation, OperationAdmissionScope},
};
use aidash_domain::capabilities::operations::{
	AdmissionLimits, AreaSnapshot, MountedFile, ShellRequest,
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
	SimpleExpr,
};
use serde_json::{Value, json};
use uuid::Uuid;
pub(crate) struct Admission<'a> {
	pub(crate) store: &'a Store,
	pub(crate) access: &'a mut Access,
	pub(crate) run: &'a Run,
	pub(crate) area: &'a mut Area,
}
impl From<Shell> for ShellRequest {
	fn from(input: Shell) -> Self {
		Self {
			idempotency_key: input.idempotency_key,
			command: input.command,
			timeout_seconds: input.timeout_seconds,
			background: input.background,
			notify_on_exit: input.notify_on_exit,
			expected_revision: input.expected_revision,
		}
	}
}
impl From<FileEntry> for MountedFile {
	fn from(file: FileEntry) -> Self {
		Self {
			file_id: file.file_id,
			path: file.path,
			digest: file.digest,
			size: file.size,
			media_type: file.media_type,
			scope: match file.scope {
				FileScope::Working => aidash_domain::capabilities::operations::FileScope::Working,
				FileScope::References => {
					aidash_domain::capabilities::operations::FileScope::References
				}
				FileScope::Received => aidash_domain::capabilities::operations::FileScope::Received,
			},
			provenance: file.provenance,
		}
	}
}
impl From<MountedFile> for FileEntry {
	fn from(file: MountedFile) -> Self {
		Self {
			file_id: file.file_id,
			path: file.path,
			digest: file.digest,
			size: file.size,
			media_type: file.media_type,
			scope: match file.scope {
				aidash_domain::capabilities::operations::FileScope::Working => FileScope::Working,
				aidash_domain::capabilities::operations::FileScope::References => {
					FileScope::References
				}
				aidash_domain::capabilities::operations::FileScope::Received => FileScope::Received,
			},
			provenance: file.provenance,
		}
	}
}
#[async_trait]
impl OperationAdmissionScope<Operation> for Admission<'_> {
	fn limits(&self) -> AdmissionLimits {
		let p = &self.store.capabilities.0;
		AdmissionLimits {
			admission: p.admission,
			working_bytes: p.working_bytes,
			command_bytes: p.limits.command_bytes,
			operation_seconds: p.operation_seconds,
			maximum_seconds: p.maximum_seconds,
		}
	}
	async fn background_maximum_seconds(&mut self) -> Result<u64> {
		use crate::apps::execution::capabilities::models::CorePreviewLimits;
		use reinhardt::db::orm::Model as _;
		let ceiling = self.store.capabilities.0.background_maximum_seconds;
		let lowered = CorePreviewLimits::objects()
			.filter(CorePreviewLimits::field_tenant().eq(&self.access.identity.tenant))
			.all_with_executor(&mut **self.access.tx)
			.await
			.map_err(reinhardt::core::exception::Error::from)
			.map_err(Error::from)?
			.pop()
			.and_then(|row| row.background_maximum_seconds)
			.and_then(|seconds| u64::try_from(seconds).ok());
		Ok(lowered.map_or(ceiling, |seconds| ceiling.min(seconds)))
	}
	fn area(&self) -> AreaSnapshot {
		AreaSnapshot {
			id: self.area.id,
			thread_id: self.area.thread_id,
			state: self.area.state.clone(),
			epoch: self.area.epoch,
			revision: self.area.revision,
		}
	}
	async fn previous(&mut self, key: &str) -> Result<Option<(Operation, String)>> {
		let previous: Option<Operation> = {
			let query_bind_1 = &self.access.identity.tenant;
			let query_bind_2 = &self.access.identity.subject;
			let query_bind_3 = key;
			crate::database::native::query_as(
				&sessions::select("core_operations")
					.and_where(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("tenant"))).eq(
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_1.to_owned()).into()],
							),
						),
					)
					.and_where(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("principal"))).eq(
							SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_2.to_owned()).into()],
							),
						),
					)
					.and_where(
						reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("request_key")))
							.eq(SimpleExpr::CustomWithExpr(
								"(?)".to_owned(),
								vec![Expr::value(query_bind_3.to_owned()).into()],
							)),
					)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut **self.access.tx)
			.await?
		};
		Ok(previous.map(|operation| {
			let digest = operation.digest.clone();
			(operation, digest)
		}))
	}
	async fn result(&mut self, operation: &Operation, offset: usize) -> Result<Value> {
		operations::result(self.store, self.access, operation, offset)
			.await
			.map(|result| json!(result))
			.map_err(Into::into)
	}
	async fn active_run(&mut self) -> Result<Option<Uuid>> {
		sessions::status(self.access, self.area)
			.await
			.map(|status| status.active_run_id)
			.map_err(Into::into)
	}
	async fn require_write(&mut self) -> Result<()> {
		sessions::authorize(self.access, self.area, "file.write")
			.await
			.map_err(Into::into)
	}
	async fn request_files(&mut self, run: Uuid) -> Result<Vec<MountedFile>> {
		operations::request_files(self.access, run)
			.await
			.map(|files| files.into_iter().map(Into::into).collect())
			.map_err(Into::into)
	}
	async fn verified_health(&mut self, python: bool) -> Result<()> {
		operations::verified_health(self.store, python)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn admit_sandboxed_tool(&mut self) -> Result<()> {
		aidash_application::usage::require(
			self.store.admission.as_ref(),
			&self.access.identity.tenant,
			aidash_application::usage::Work::SandboxedTool,
		)
		.await
	}
	fn operation_id(&self) -> Uuid {
		Uuid::new_v4()
	}
	async fn set_running(&mut self, epoch: i64) -> Result<()> {
		self.area.epoch = epoch;
		operations::set_area(self.access, self.area.id, "running", epoch)
			.await
			.map_err(Into::into)
	}
	async fn accept(&mut self, operation: AcceptedOperation) -> Result<Operation> {
		let AcceptedOperation {
			id,
			key,
			digest,
			kind,
			epoch,
			input: value,
		} = operation;

		crate::database::native::query(
			&Query::insert()
				.into_table(Alias::new("core_operations"))
				.columns(
					[
						"id",
						"area_id",
						"run_id",
						"tenant",
						"principal",
						"credential_id",
						"subjects",
						"request_key",
						"digest",
						"kind",
						"state",
						"epoch",
						"generation",
						"revision",
						"policy_revision",
						"input",
						"result",
					]
					.map(Alias::new),
				)
				.from_subquery(
					Query::select()
						.expr(Expr::cust("$1"))
						.expr(Expr::cust("$2"))
						.expr(Expr::cust("$3"))
						.expr(Expr::cust("$4"))
						.expr(Expr::cust("$5"))
						.expr(Expr::cust("$6"))
						.expr(Expr::cust("$7"))
						.expr(Expr::cust("$8"))
						.expr(Expr::cust("$9"))
						.expr(Expr::cust("$10"))
						.expr(Expr::cust("$11"))
						.expr(Expr::cust("$12"))
						.expr(Expr::cust("$13"))
						.expr(Expr::cust("$14"))
						.expr(Expr::cust("$15"))
						.expr(Expr::cust("$16"))
						.expr(Expr::cust("$17"))
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.bind(id)
		.bind(self.area.id)
		.bind(self.run.id)
		.bind(&self.access.identity.tenant)
		.bind(&self.access.identity.subject)
		.bind(self.access.identity.credential_id)
		.bind(json!(self.access.subjects))
		.bind(&key)
		.bind(&digest)
		.bind(kind)
		.bind("prepared")
		// A Shell writer has already moved the Area to this epoch; a Pod
		// operation records one no Area epoch can equal (#745).
		.bind(epoch)
		.bind(self.area.generation)
		.bind(self.area.revision)
		.bind(self.access.snapshot.revision)
		.bind(value)
		.bind(json!({}))
		.execute(&mut **self.access.tx)
		.await?;
		self.store
			.event(
				&mut self.access.tx,
				Some(self.area.thread_id),
				"capability.operation_accepted",
				json!({"area_id":self.area.id,"operation_id":id,"epoch":self.area.epoch}),
			)
			.await?;
		operations::get(self.access, id).await.map_err(Into::into)
	}
}

/// Polling owns the same locked native row without introducing a second transaction.
pub(crate) struct Control<'a> {
	pub(crate) store: &'a Store,
	pub(crate) access: &'a mut Access,
	pub(crate) run: &'a Run,
	pub(crate) area: &'a Area,
	operation: Option<Operation>,
}
impl<'a> Control<'a> {
	pub(crate) fn new(
		store: &'a Store,
		access: &'a mut Access,
		run: &'a Run,
		area: &'a Area,
	) -> Self {
		Self {
			store,
			access,
			run,
			area,
			operation: None,
		}
	}
	fn loaded(&self) -> crate::Result<&Operation> {
		self.operation.as_ref().ok_or(Error::Forbidden)
	}
}
#[async_trait]
impl aidash_application::ports::capabilities::OperationControlScope for Control<'_> {
	fn area_id(&self) -> Uuid {
		self.area.id
	}
	fn run_id(&self) -> Uuid {
		self.run.id
	}
	fn principal(&self) -> &str {
		&self.access.identity.subject
	}
	async fn load(
		&mut self,
		id: Uuid,
	) -> Result<aidash_domain::capabilities::operations::OperationState> {
		let operation = operations::get(self.access, id).await?;
		let state = aidash_domain::capabilities::operations::OperationState {
			area_id: operation.area_id,
			run_id: operation.run_id,
			principal: operation.principal.clone(),
			kind: operation.kind.clone(),
			state: operation.state.clone(),
			pod: aidash_domain::capabilities::operations::runs_in_pod(&operation.input),
		};
		self.operation = Some(operation);
		Ok(state)
	}
	fn apply_cancellation(
		&mut self,
		change: aidash_domain::capabilities::operations::Cancellation,
	) -> Result<()> {
		let operation = self.operation.as_mut().ok_or(Error::Forbidden)?;
		operation.state = change.state.into();
		if let Some(result) = change.result {
			operation.result = result;
		}
		Ok(())
	}
	async fn activate_area(&mut self) -> Result<()> {
		operations::set_area(self.access, self.area.id, "active", self.area.epoch)
			.await
			.map_err(Into::into)
	}
	async fn persist(&mut self) -> Result<()> {
		let operation = self.loaded()?.clone();
		operations::persist(self.access, &operation)
			.await
			.map_err(Into::into)
	}
	async fn project(&mut self, offset: usize) -> Result<Value> {
		let operation = self.loaded()?.clone();
		operations::result(self.store, self.access, &operation, offset)
			.await
			.map(|result| json!(result))
			.map_err(Into::into)
	}
}

impl From<ShellRequest> for Shell {
	fn from(v: ShellRequest) -> Self {
		Self {
			idempotency_key: v.idempotency_key,
			command: v.command,
			timeout_seconds: v.timeout_seconds,
			background: v.background,
			notify_on_exit: v.notify_on_exit,
			wait_seconds: None,
			expected_revision: v.expected_revision,
		}
	}
}
