//! Native guard reads preserve the worker's Access connection and registry locks.
use crate::{
	Result as NativeResult,
	apps::identity::services::{access::Access, catalog},
	federation::Federation,
};
use aidash_application::{
	Result,
	ports::authorization::guard::{RunGuardScope, UseRecheckScope},
};
use aidash_domain::{
	RunMetadata, Task,
	policy::{PolicyBundle, Resource},
	registry::{EntityRef, Entry},
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Condition, Expr, ExprTrait as _, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SimpleExpr,
};
use uuid::Uuid;
pub(crate) struct RunGuard<'a> {
	pub(crate) federation: &'a Federation,
	pub(crate) access: &'a mut Access,
}
/// The Use Recheck over one `Access`, for a caller that holds no `Federation`: a resume, or a
/// Run's action that asks whether the Run may still use what it was admitted with.
pub(crate) struct UseGuard<'a> {
	pub(crate) access: &'a mut Access,
}
#[async_trait]
impl UseRecheckScope for UseGuard<'_> {
	fn node_id(&self) -> &str {
		&self.access.node_id
	}
	async fn catalog(&mut self, reference: &EntityRef, action: &str) -> Result<Entry> {
		catalog::entry(self.access, reference, action)
			.await
			.map_err(Into::into)
	}
	async fn reference_embedding_available(&mut self, reference: &EntityRef) -> Result<bool> {
		crate::apps::registry::services::knowledge::embedding_available(self.access, reference)
			.await
			.map(|entry| entry.is_some())
			.map_err(Into::into)
	}
	async fn closure_catalog(
		&mut self,
		reference: &EntityRef,
		action: &str,
		users: &[EntityRef],
	) -> Result<Entry> {
		catalog::closure_entry(self.access, reference, action, users)
			.await
			.map_err(Into::into)
	}
	async fn metadata(&mut self, reference: &EntityRef) -> Result<Entry> {
		catalog::metadata(self.access, reference)
			.await
			.map_err(Into::into)
	}
	async fn binding_snapshot(
		&mut self,
		run: &RunMetadata,
	) -> Result<aidash_domain::registry::bindings::BindingSnapshot> {
		stored_snapshot(self.access, run).await?.ok_or_else(|| {
			aidash_application::Error::Invalid("Run has no admitted Binding snapshot".into())
		})
	}
}

/// The Binding snapshot a Run executing here was admitted with, if it has one. A damaged
/// context has none: its Run's next boundary is failure delivery, which reads no context, and
/// a resume must not fail on it.
async fn stored_snapshot(
	access: &mut Access,
	run: &RunMetadata,
) -> Result<Option<aidash_domain::registry::bindings::BindingSnapshot>> {
	crate::apps::execution::models::Run::binding_snapshot_in(&mut **access.tx, run.id)
		.await
		.map_err(Into::into)
}

/// The first definition a Run may no longer use, or none (the Use Recheck). A Run without a
/// readable Binding snapshot, such as one left to deliver its failure, is checked for its own
/// Agent only, as its next boundary checks it. A Run of another Home executing here is checked
/// too, as the read walk checked it before; `local_only` leaves it out for a person's resume,
/// which controls only the Runs of this Home.
pub(crate) async fn use_block(
	access: &mut Access,
	run: &RunMetadata,
	local_only: bool,
) -> Result<Option<EntityRef>> {
	use aidash_application::authorization::execution::guard::recheck;
	if local_only && !run.is_local(&access.node_id) {
		return Ok(None);
	}
	let binding = stored_snapshot(access, run).await?.is_some();
	Ok(recheck(&mut UseGuard { access }, run, binding)
		.await?
		.blocked())
}

#[async_trait]
impl RunGuardScope for RunGuard<'_> {
	fn bundle(&self) -> &PolicyBundle {
		&self.access.snapshot.bundle
	}
	async fn run_visible(&mut self, run: &RunMetadata) -> Result<bool> {
		crate::apps::execution::repositories::node_cancellations::require_run_open(
			&mut **self.access.tx,
			run.id,
		)
		.await?;
		self.access.run_visible(run).await.map_err(Into::into)
	}
	async fn cluster_targets(&mut self, workspace: Uuid) -> Result<Vec<String>> {
		let result: NativeResult<Vec<String>> = async {
			Ok({
				let query_bind_1 = workspace;
				crate::database::native::query_scalar(
					&Query::select()
						.column(Alias::new("target"))
						.from(Alias::new("conversations"))
						.cond_where(
							Condition::all()
								.add(
									reinhardt::query::SimpleExpr::from(Expr::col(Alias::new(
										"workspace_id",
									)))
									.eq(SimpleExpr::CustomWithExpr(
										"(?)".to_owned(),
										vec![Expr::value(query_bind_1.to_owned()).into()],
									)),
								)
								.add(Expr::cust("target_kind='cluster'")),
						)
						.to_string(PostgresQueryBuilder),
				)
				.scalar_all(&mut **self.access.tx)
				.await?
			})
		}
		.await;
		result.map_err(Into::into)
	}
	async fn task_read(&mut self, task: Uuid) -> Result<Task> {
		self.access.task_read(task).await.map_err(Into::into)
	}
	async fn task_resource(&mut self, task: &Task) -> Result<Resource> {
		self.access.task_resource(task).await.map_err(Into::into)
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.access
			.require(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn require_live(&mut self, task: Uuid, agent: &EntityRef) -> Result<()> {
		crate::generation::provision::require_live(
			self.access,
			self.federation.config.deployment.as_str(),
			task,
			agent,
		)
		.await
		.map_err(Into::into)
	}
	async fn check_pinned(&mut self, entry: &Entry) -> Result<()> {
		crate::marketplace::check_pinned(self.access, entry)
			.await
			.map_err(Into::into)
	}
	async fn context_authority(&mut self, run: &RunMetadata) -> Result<()> {
		crate::capabilities::sessions::context_authority(self.access, run)
			.await
			.map_err(Into::into)
	}
}
#[async_trait]
impl UseRecheckScope for RunGuard<'_> {
	fn node_id(&self) -> &str {
		self.federation.config.deployment.as_str()
	}
	async fn catalog(&mut self, reference: &EntityRef, action: &str) -> Result<Entry> {
		catalog::entry(self.access, reference, action)
			.await
			.map_err(Into::into)
	}
	async fn reference_embedding_available(&mut self, reference: &EntityRef) -> Result<bool> {
		crate::apps::registry::services::knowledge::embedding_available(self.access, reference)
			.await
			.map(|entry| entry.is_some())
			.map_err(Into::into)
	}
	async fn closure_catalog(
		&mut self,
		reference: &EntityRef,
		action: &str,
		users: &[EntityRef],
	) -> Result<Entry> {
		catalog::closure_entry(self.access, reference, action, users)
			.await
			.map_err(Into::into)
	}
	async fn metadata(&mut self, reference: &EntityRef) -> Result<Entry> {
		catalog::metadata(self.access, reference)
			.await
			.map_err(Into::into)
	}
	async fn binding_snapshot(
		&mut self,
		run: &RunMetadata,
	) -> Result<aidash_domain::registry::bindings::BindingSnapshot> {
		let row = crate::apps::execution::models::Run::read_in(&mut **self.access.tx, run.id, None)
			.await?
			.ok_or(aidash_application::Error::Forbidden)?;
		row.context
			.binding_snapshot
			.map(|snapshot| *snapshot)
			.ok_or_else(|| {
				aidash_application::Error::Invalid("Run has no admitted Binding snapshot".into())
			})
	}
}
