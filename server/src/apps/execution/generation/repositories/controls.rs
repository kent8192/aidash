//! Owned control sessions bind the trusted actor to lifecycle effects and audit.
use super::lifecycle::NativeLifecycle;
use crate::{
	Error,
	authorization::{access::Access, identity::Actor},
	federation::Federation,
};
use aidash_application::{
	authorization::Snapshot,
	ports::generation::lifecycle::{
		GenerationControlScope, GenerationControls, GenerationLifecycleScope,
	},
};
use aidash_domain::{
	generation::requests::{Control, Request},
	identity::Principal,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;

use uuid::Uuid;
pub(crate) struct NativeControls {
	pub runtime: Federation,
	pub actor: Actor,
	pub principal: Principal,
}
enum Mode {
	Operator(crate::database::native::Transaction),
	Subject(Box<Access>),
}
struct Scope {
	runtime: Federation,
	mode: Mode,
}
impl Scope {
	fn lifecycle(&mut self) -> NativeLifecycle<'_> {
		NativeLifecycle {
			runtime: &self.runtime,
			transaction: match &mut self.mode {
				Mode::Operator(tx) => tx,
				Mode::Subject(access) => &mut access.tx,
			},
		}
	}
}
#[async_trait]
impl GenerationControls for NativeControls {
	fn principal(&self) -> &Principal {
		&self.principal
	}
	async fn begin(
		&self,
		_tenant: &str,
	) -> aidash_application::Result<Box<dyn GenerationControlScope>> {
		let mode = match &self.actor {
			Actor::Operator => {
				Mode::Operator(crate::database::native::begin(&self.runtime.store.pool).await?)
			}
			Actor::Subject(identity) => Mode::Subject(Box::new(
				Access::begin_exclusive(&self.runtime.store, identity).await?,
			)),
		};
		Ok(Box::new(Scope {
			runtime: self.runtime.clone(),
			mode,
		}))
	}
	fn notify(&self) {
		self.runtime.notify.notify_waiters();
	}
}
#[async_trait]
impl GenerationControlScope for Scope {
	async fn visible(&mut self, job: &Request) -> aidash_application::Result<bool> {
		let Mode::Subject(access) = &mut self.mode else {
			return Err(aidash_application::Error::Forbidden);
		};
		aidash_application::generation::visibility::visible(
			&mut crate::bootstrap::generation_visibility_scope(access),
			job,
		)
		.await
	}
	async fn decide(&mut self, job: &Request, action: &str) -> aidash_application::Result<bool> {
		let Mode::Subject(access) = &mut self.mode else {
			return Err(aidash_application::Error::Forbidden);
		};
		access
			.decide(
				&access.resource("generation", job.id, job.resource_attributes()),
				action,
			)
			.await
			.map_err(Into::into)
	}
	async fn finish(
		self: Box<Self>,
		result: aidash_application::Result<Request>,
	) -> aidash_application::Result<Request> {
		match self.mode {
			Mode::Operator(tx) => match result {
				Ok(job) => {
					tx.commit().await?;
					Ok(job)
				}
				Err(error) => Err(error),
			},
			Mode::Subject(access) => access
				.finish(result.map_err(Error::from))
				.await
				.map_err(Into::into),
		}
	}
}
#[async_trait]
impl GenerationLifecycleScope for Scope {
	fn node_id(&self) -> &str {
		&self.runtime.config.node_id
	}
	fn now(&self) -> DateTime<Utc> {
		Utc::now()
	}
	async fn unused(
		&mut self,
		job: &Request,
	) -> aidash_application::Result<(i64, aidash_domain::generation::policy::Allowances)> {
		self.lifecycle().unused(job).await
	}
	async fn release_policy(
		&mut self,
		job: &Request,
		unused: i64,
		unused_calls: &aidash_domain::generation::policy::Allowances,
	) -> aidash_application::Result<()> {
		self.lifecycle()
			.release_policy(job, unused, unused_calls)
			.await
	}
	async fn mark_quota_released(&mut self, job: &Request) -> aidash_application::Result<()> {
		self.lifecycle().mark_quota_released(job).await
	}
	async fn cancel_runs(&mut self, job: &Request) -> aidash_application::Result<()> {
		self.lifecycle().cancel_runs(job).await
	}
	async fn authority(&mut self, tenant: &str) -> aidash_application::Result<Snapshot> {
		self.lifecycle().authority(tenant).await
	}
	async fn save_authority(
		&mut self,
		job: &Request,
		snapshot: &Snapshot,
		actor: &str,
	) -> aidash_application::Result<()> {
		self.lifecycle().save_authority(job, snapshot, actor).await
	}
	async fn retire_catalog(&mut self, job: &Request) -> aidash_application::Result<Option<i64>> {
		self.lifecycle().retire_catalog(job).await
	}
	async fn record_retirement(
		&mut self,
		job: &Request,
		revision: i64,
		actor: &str,
	) -> aidash_application::Result<()> {
		self.lifecycle()
			.record_retirement(job, revision, actor)
			.await
	}
	async fn update_status(
		&mut self,
		job: &Request,
		status: &str,
	) -> aidash_application::Result<Request> {
		self.lifecycle().update_status(job, status).await
	}
	async fn history(
		&mut self,
		job: &Request,
		status: &str,
		actor: &str,
		reason: &str,
	) -> aidash_application::Result<()> {
		self.lifecycle().history(job, status, actor, reason).await
	}
	async fn event(
		&mut self,
		workspace: Uuid,
		kind: &str,
		data: Value,
	) -> aidash_application::Result<()> {
		self.lifecycle().event(workspace, kind, data).await
	}
	async fn replay(
		&mut self,
		job: &Request,
		status: &str,
		actor: &str,
		input: &Control,
	) -> aidash_application::Result<bool> {
		self.lifecycle().replay(job, status, actor, input).await
	}
	async fn load(&mut self, tenant: &str, id: Uuid) -> aidash_application::Result<Request> {
		self.lifecycle().load(tenant, id).await
	}
}
