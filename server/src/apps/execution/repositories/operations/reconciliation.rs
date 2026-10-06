//! Owned Access scopes retain authority locks, atomic publication and object guards.
use super::Operation;
use crate::apps::execution::capabilities::{
	serializers::contracts::{Area as NativeArea, FileEntry},
	services::{objects::PendingObject, operations, packages, python, service, sessions, skills},
};
use crate::{
	Result as NativeResult,
	authorization::{access::Access, identity::SubjectIdentity},
	store::Store,
};
use aidash_application::{
	Result,
	ports::capabilities::reconciliation::{
		Loaded, OperationReconciliationRepository, OperationReconciliationScope,
	},
};
use aidash_domain::{
	Run,
	capabilities::{
		CoreCapabilities,
		operations::{
			MountedFile,
			reconciliation::{Area, Limits, Snapshot},
		},
	},
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, PostgresQueryBuilder, QueryStatementBuilder as _, SimpleExpr,
};
use serde_json::{Value, json};
use uuid::Uuid;
pub(crate) struct Repository<'a> {
	pub(crate) store: &'a Store,
}
struct Scope<'a> {
	store: &'a Store,
	access: Access,
	run: Option<Run>,
	area: Option<NativeArea>,
	pending: Option<PendingObject>,
}
fn domain(operation: &Operation) -> Snapshot {
	Snapshot {
		id: operation.id,
		area_id: operation.area_id,
		run_id: operation.run_id,
		tenant: operation.tenant.clone(),
		principal: operation.principal.clone(),
		credential_id: operation.credential_id,
		subjects: operation.subjects.clone(),
		digest: operation.digest.clone(),
		kind: operation.kind.clone(),
		state: operation.state.clone(),
		epoch: operation.epoch,
		generation: operation.generation,
		revision: operation.revision,
		policy_revision: operation.policy_revision,
		input: operation.input.clone(),
		result: operation.result.clone(),
		runner_instance: operation.runner_instance.clone(),
	}
}
fn native(operation: &Snapshot) -> Operation {
	Operation {
		id: operation.id,
		area_id: operation.area_id,
		run_id: operation.run_id,
		tenant: operation.tenant.clone(),
		principal: operation.principal.clone(),
		credential_id: operation.credential_id,
		subjects: operation.subjects.clone(),
		digest: operation.digest.clone(),
		kind: operation.kind.clone(),
		state: operation.state.clone(),
		epoch: operation.epoch,
		generation: operation.generation,
		revision: operation.revision,
		policy_revision: operation.policy_revision,
		input: operation.input.clone(),
		result: operation.result.clone(),
		runner_instance: operation.runner_instance.clone(),
	}
}
#[async_trait]
impl OperationReconciliationRepository for Repository<'_> {
	async fn snapshot(&self, id: Uuid) -> Result<Snapshot> {
		let result: NativeResult<Snapshot> = async {
			let snapshot: Operation = {
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
				.fetch_one(&self.store.pool)
				.await?
			};
			Ok(domain(&snapshot))
		}
		.await;
		result.map_err(Into::into)
	}
	async fn begin(
		&self,
		snapshot: &Snapshot,
	) -> Result<Box<dyn OperationReconciliationScope + '_>> {
		let identity = SubjectIdentity {
			http_session: None,
			credential_id: snapshot.credential_id,
			tenant: snapshot.tenant.clone(),
			subject: snapshot.principal.clone(),
		};
		let mut access = Access::begin(self.store, &identity).await?;
		access.subjects = serde_json::from_value(snapshot.subjects.clone())?;
		Ok(Box::new(Scope {
			store: self.store,
			access,
			run: None,
			area: None,
			pending: None,
		}))
	}
	async fn withdraw(&self, snapshot: &Snapshot) -> Result<()> {
		aidash_application::capabilities::withdrawal::withdraw(
			&crate::bootstrap::operation_withdrawal_repository(self.store),
			snapshot.area_id,
			snapshot.id,
		)
		.await
	}
}
impl Scope<'_> {
	fn area(&self) -> &NativeArea {
		self.area.as_ref().expect("reconciliation scope loaded")
	}
}
#[async_trait]
impl OperationReconciliationScope for Scope<'_> {
	fn limits(&self) -> Limits {
		let profile = &self.store.capabilities.0;
		Limits {
			admission: profile.admission,
			working_bytes: profile.working_bytes,
			output_bytes: profile.output_bytes,
			read_bytes: profile.limits.read_bytes,
		}
	}
	async fn load(&mut self, id: Uuid, run: Uuid) -> Result<Loaded> {
		let run = self.access.run_for_interaction(run).await?;
		let area = sessions::for_run(&mut self.access, &run).await?;
		let operation = super::get(&mut self.access, id).await?;
		let result = Loaded {
			run: run.metadata(),
			area: Area {
				id: area.id,
				workspace_id: area.workspace_id,
				generation: area.generation,
				revision: area.revision,
				epoch: area.epoch,
				manifest: area.manifest.clone(),
			},
			operation: domain(&operation),
		};
		self.run = Some(run);
		self.area = Some(area);
		Ok(result)
	}
	async fn configuration(&mut self) -> Result<CoreCapabilities> {
		let run = self.run.as_ref().expect("reconciliation scope loaded");
		service::settings(&mut self.access, run)
			.await
			.map(|config| config.core_capabilities)
			.map_err(Into::into)
	}
	async fn require_builtin(&mut self, kind: &str) -> Result<()> {
		let resource = self
			.access
			.resource("tool", format!("builtin:{kind}"), json!({}));
		self.access
			.require(&resource, "tool.invoke")
			.await
			.map_err(Into::into)
	}
	async fn authorize_packages(&mut self, request: &Value) -> Result<()> {
		let request: packages::Install = serde_json::from_value(request.clone())?;
		let run = self.run.as_ref().expect("reconciliation scope loaded");
		let area = self.area.as_ref().expect("reconciliation scope loaded");
		packages::authorize(self.store, &mut self.access, run, area, &request.wheels)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn set_area(&mut self, state: &str) -> Result<()> {
		super::set_area(
			&mut self.access,
			self.area.as_ref().unwrap().id,
			state,
			self.area.as_ref().unwrap().epoch,
		)
		.await
		.map_err(Into::into)
	}
	async fn complete_python(&mut self, operation: &Snapshot, observed: &Value) -> Result<()> {
		let area = self.area.as_ref().expect("reconciliation scope loaded");
		python::completed(&mut self.access, area, &native(operation), observed)
			.await
			.map_err(Into::into)
	}
	async fn persist(&mut self, operation: &Snapshot) -> Result<()> {
		super::persist(&mut self.access, &native(operation))
			.await
			.map_err(Into::into)
	}
	async fn health(&mut self, cancelling: bool, python: bool) -> Result<Value> {
		if cancelling {
			operations::remote(self.store, reqwest::Method::GET, "/v1/health", None).await
		} else {
			operations::verified_health(self.store, python).await
		}
		.map_err(Into::into)
	}
	async fn request(&mut self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
		use aidash_application::ports::capabilities::runner::RunnerTransport as _;
		crate::bootstrap::operation_runner(self.store)?
			.request(method, path, body.as_ref())
			.await
	}
	async fn dispatch_files(&mut self, operation: &Snapshot) -> Result<Vec<MountedFile>> {
		let area = self.area.as_ref().expect("reconciliation scope loaded");
		let run = self.run.as_ref().expect("reconciliation scope loaded").id;
		operations::request_files(
			&mut self.access,
			run,
			area,
			packages::inputs(&native(operation))?,
		)
		.await
		.map(|files| files.into_iter().map(Into::into).collect())
		.map_err(Into::into)
	}
	async fn upload_files(&mut self, operation: &Snapshot) -> Result<Vec<MountedFile>> {
		let result: NativeResult<Vec<FileEntry>> = async {
			Ok(service::files(self.area())?
				.into_iter()
				.chain(skills::mounted(&mut self.access, self.run.as_ref().unwrap().id).await?)
				.chain(packages::inputs(&native(operation))?)
				.collect())
		}
		.await;
		result
			.map(|files| files.into_iter().map(Into::into).collect())
			.map_err(Into::into)
	}
	async fn read_chunk(&mut self, file: &MountedFile, offset: u64) -> Result<Vec<u8>> {
		self.store
			.capabilities
			.read_chunk(&mut self.access, &file.clone().into(), offset)
			.await
			.map_err(Into::into)
	}
	async fn verified(&mut self, file: &MountedFile) -> Result<()> {
		self.store
			.capabilities
			.verified(&mut self.access, &file.clone().into())
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn begin_output(&mut self, size: u64) -> Result<()> {
		if self.pending.is_some() {
			return Err(aidash_application::Error::External(
				"pending operation output already exists".into(),
			));
		}
		self.pending = Some(
			self.store
				.capabilities
				.begin_object(
					&mut self.access,
					Some(self.area.as_ref().unwrap().id),
					"working",
					size,
				)
				.await?,
		);
		Ok(())
	}
	async fn write_output(&mut self, bytes: &[u8]) -> Result<()> {
		self.pending
			.as_mut()
			.expect("operation output begun")
			.write_block(bytes)
			.await
			.map_err(Into::into)
	}
	async fn finish_output(&mut self, expected: &str) -> Result<(Uuid, String)> {
		self.pending
			.take()
			.expect("operation output begun")
			.finish(&mut self.access, Some(expected))
			.await
			.map_err(Into::into)
	}
	async fn put(&mut self, kind: &str, bytes: &[u8]) -> Result<(Uuid, String)> {
		self.store
			.capabilities
			.put(
				&mut self.access,
				Some(self.area.as_ref().unwrap().id),
				kind,
				bytes,
			)
			.await
			.map_err(Into::into)
	}
	async fn publish(&mut self, entries: Vec<MountedFile>) -> Result<i64> {
		let area = self.area.as_mut().expect("reconciliation scope loaded");
		area.manifest = json!(entries.into_iter().map(FileEntry::from).collect::<Vec<_>>());
		service::publish(self.store, &mut self.access, area).await?;
		Ok(area.revision)
	}
	async fn event(&mut self, data: Value) -> Result<()> {
		self.store
			.event(
				&mut self.access.tx,
				Some(self.area.as_ref().unwrap().workspace_id),
				"capability.operation_changed",
				data,
			)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn finish(self: Box<Self>, result: Result<()>) -> Result<()> {
		let Scope { access, .. } = *self;
		access
			.finish(result.map_err(Into::into))
			.await
			.map_err(Into::into)
	}
}
