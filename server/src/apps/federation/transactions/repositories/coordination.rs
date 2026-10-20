//! Native coordinator storage preserves row locks, audits and recovery ownership.
use super::transport;
use crate::apps::federation::transactions::{
	models::{coordinator_records, states::AtomicVotePhase},
	serializers::contracts as native,
	services::{authority, fault, participant},
};
use crate::{Error, Result, federation::Federation};
use aidash_application::tenancy::TenancyScope;
use aidash_application::{
	Result as ApplicationResult,
	ports::transactions::coordination::{
		CoordinatorRepository, ParticipantTransport, RecoveryBatch, RecoveryScope,
	},
};
use aidash_domain::transactions::{
	CoordinatorTransition, Manifest,
	authority::Status,
	coordination::{LocalStatus, ParticipantOperation, Vote},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reinhardt::db::backends::TransactionExecutor;
use serde_json::json;
use uuid::Uuid;

pub(crate) struct Repository {
	pub runtime: Federation,
}

/// Cross-node transaction records are Deployment-wide.
pub(crate) async fn plumbing<T>(
	runtime: &Federation,
	work: impl for<'t> AsyncFnOnce(&'t mut reinhardt::db::orm::AtomicTransaction) -> Result<T>,
) -> Result<T> {
	runtime
		.store
		.control_pool
		.atomic(&TenancyScope::NoTenant, work)
		.await
}

struct Recovery {
	id: Uuid,
	runtime: Federation,
	lease: Box<dyn TransactionExecutor>,
}

#[async_trait]
impl RecoveryScope for Recovery {
	async fn touch(&mut self) -> ApplicationResult<()> {
		coordinator_records::touch(&self.runtime.store.control_pool, self.id)
			.await
			.map_err(Into::into)
	}

	async fn acknowledge(&mut self, node: &str, phase: &str) -> ApplicationResult<()> {
		let phase: AtomicVotePhase = serde_json::from_value(json!(phase))?;
		coordinator_records::acknowledge(&self.runtime.store.control_pool, self.id, node, phase)
			.await
			.map_err(Into::into)
	}

	async fn record_error(&mut self, error: &str) -> ApplicationResult<()> {
		coordinator_records::record_error(&self.runtime.store.control_pool, self.id, error)
			.await
			.map_err(Into::into)
	}

	async fn release(self: Box<Self>) -> ApplicationResult<()> {
		let Self { lease, .. } = *self;
		lease
			.commit()
			.await
			.map_err(Error::from)
			.map_err(Into::into)
	}
}

struct Batch {
	ids: Vec<Uuid>,
}

impl RecoveryBatch for Batch {
	fn ids(&self) -> &[Uuid] {
		&self.ids
	}
}

#[async_trait]
impl CoordinatorRepository for Repository {
	fn node_id(&self) -> &str {
		self.runtime.config.deployment.as_str()
	}

	fn now(&self) -> DateTime<Utc> {
		Utc::now()
	}

	async fn status(&self, id: Uuid) -> ApplicationResult<Status> {
		plumbing(&self.runtime, async |tx| {
			coordinator_records::status(tx, id).await
		})
		.await
		.map(Into::into)
		.map_err(Into::into)
	}

	async fn votes(&self, id: Uuid) -> ApplicationResult<Vec<Vote>> {
		plumbing(&self.runtime, async |tx| {
			coordinator_records::votes(tx, id).await
		})
		.await
		.map(|votes| votes.into_iter().map(Into::into).collect())
		.map_err(Into::into)
	}

	async fn transition(
		&self,
		id: Uuid,
		change: CoordinatorTransition,
		detail: &str,
	) -> ApplicationResult<()> {
		let point = format!("coordinator.{}", change.phase().to_ascii_lowercase());
		fault::cut(id, &format!("{point}.before")).await?;
		coordinator_records::transition(
			&self.runtime.store.control_pool,
			id,
			change.into(),
			detail,
		)
		.await?;
		fault::cut(id, &format!("{point}.after")).await?;
		Ok(())
	}

	async fn acquire_recovery(&self, id: Uuid) -> ApplicationResult<Box<dyn RecoveryScope>> {
		let lease =
			coordinator_records::recovery_lease(&self.runtime.store.control_pool, id).await?;
		Ok(Box::new(Recovery {
			id,
			runtime: self.runtime.clone(),
			lease,
		}))
	}

	async fn recovery_batch(&self, aborted: bool) -> ApplicationResult<Box<dyn RecoveryBatch>> {
		let ids = plumbing(&self.runtime, async |tx| {
			coordinator_records::recovery_candidates(tx, aborted).await
		})
		.await?;
		Ok(Box::new(Batch { ids }))
	}

	async fn issue_authority(&self, manifest: &Manifest, node: &str) -> ApplicationResult<()> {
		authority::issue(&self.runtime, manifest, node)
			.await
			.map_err(Into::into)
	}

	async fn settle_authority(&self, id: Uuid, node: &str, phase: &str) -> ApplicationResult<()> {
		authority::settle(&self.runtime, id, node, phase)
			.await
			.map_err(Into::into)
	}

	async fn scoped(&self, id: Uuid) -> ApplicationResult<bool> {
		authority::scoped(&self.runtime, id)
			.await
			.map_err(Into::into)
	}

	async fn fault(&self, id: Uuid, point: &str) -> ApplicationResult<()> {
		fault::cut(id, point).await.map_err(Into::into)
	}
}

pub(crate) struct Transport {
	pub runtime: Federation,
}

#[async_trait]
impl ParticipantTransport for Transport {
	async fn decision(&self, manifest: &Manifest) -> ApplicationResult<Status> {
		transport::remote::<native::Status>(
			&self.runtime,
			&manifest.coordinator,
			"GET",
			&format!("/transactions/{}/decision", manifest.id),
			None::<&()>,
		)
		.await
		.map(Into::into)
	}

	async fn send(
		&self,
		manifest: &Manifest,
		node: &str,
		operation: ParticipantOperation,
	) -> ApplicationResult<LocalStatus> {
		let response: ApplicationResult<native::LocalStatus> =
			if node == self.runtime.config.deployment {
				match operation {
					ParticipantOperation::Reserve => {
						participant::reserve(&self.runtime, &manifest.coordinator, manifest).await
					}
					ParticipantOperation::Prepare => {
						participant::prepare(&self.runtime, &manifest.coordinator, manifest).await
					}
					ParticipantOperation::Finish => {
						participant::finish(&self.runtime, &manifest.coordinator, manifest).await
					}
				}
				.map_err(Into::into)
			} else {
				transport::remote(
					&self.runtime,
					node,
					"POST",
					&format!("/transactions/{}", operation.as_str()),
					Some(manifest),
				)
				.await
			};
		response.map(Into::into)
	}
}
