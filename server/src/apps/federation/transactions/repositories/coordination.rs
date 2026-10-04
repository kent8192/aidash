//! Native coordinator storage preserves row locks, audits and recovery ownership.
use crate::apps::federation::transactions::{
	models::{coordinator_records, states::AtomicVotePhase},
	serializers::contracts as native,
	services::{authority, coordinator, fault, participant},
};
use crate::{Error, Result, federation::Federation};
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
use reinhardt::db::{
	backends::{
		DatabaseConnection as BackendConnection, TransactionExecutor, dialect::PostgresBackend,
	},
	orm::{DatabaseConnection, DatabaseConnectionLease},
};
use reqwest::Method;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

pub(crate) struct Repository {
	pub runtime: Federation,
}

pub(crate) fn connection(runtime: &Federation) -> Result<DatabaseConnectionLease> {
	Ok(DatabaseConnectionLease::register(BackendConnection::new(
		Arc::new(PostgresBackend::new(runtime.store.control_pool.clone())),
	))?)
}

struct Recovery {
	id: Uuid,
	runtime: Federation,
	connection: Option<DatabaseConnectionLease>,
	lease: Box<dyn TransactionExecutor>,
}

impl Recovery {
	fn connection(&mut self) -> Result<DatabaseConnection> {
		if self.connection.is_none() {
			self.connection = Some(connection(&self.runtime)?);
		}
		Ok(self
			.connection
			.as_ref()
			.expect("connection retained")
			.handle())
	}
}

#[async_trait]
impl RecoveryScope for Recovery {
	async fn touch(&mut self) -> ApplicationResult<()> {
		coordinator_records::touch(self.connection()?, self.id)
			.await
			.map_err(Into::into)
	}

	async fn acknowledge(&mut self, node: &str, phase: &str) -> ApplicationResult<()> {
		let phase: AtomicVotePhase = serde_json::from_value(json!(phase))?;
		coordinator_records::acknowledge(self.connection()?, self.id, node, phase)
			.await
			.map_err(Into::into)
	}

	async fn record_error(&mut self, error: &str) -> ApplicationResult<()> {
		coordinator_records::record_error(self.connection()?, self.id, error)
			.await
			.map_err(Into::into)
	}

	async fn release(self: Box<Self>) -> ApplicationResult<()> {
		let Self {
			connection, lease, ..
		} = *self;
		// Release the registration before committing the advisory transaction,
		// retaining the connection and lease ownership order on success and failure.
		// Both owners also release their resources through RAII on cancellation.
		drop(connection);
		lease
			.commit()
			.await
			.map_err(Error::from)
			.map_err(Into::into)
	}
}

struct Batch {
	ids: Vec<Uuid>,
	_connection: DatabaseConnectionLease,
}

impl RecoveryBatch for Batch {
	fn ids(&self) -> &[Uuid] {
		&self.ids
	}
}

#[async_trait]
impl CoordinatorRepository for Repository {
	fn node_id(&self) -> &str {
		&self.runtime.config.node_id
	}

	fn now(&self) -> DateTime<Utc> {
		Utc::now()
	}

	async fn status(&self, id: Uuid) -> ApplicationResult<Status> {
		let lease = connection(&self.runtime)?;
		coordinator_records::status(&mut lease.handle(), id)
			.await
			.map(Into::into)
			.map_err(Into::into)
	}

	async fn votes(&self, id: Uuid) -> ApplicationResult<Vec<Vote>> {
		let lease = connection(&self.runtime)?;
		coordinator_records::votes(&mut lease.handle(), id)
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
		let lease = connection(&self.runtime)?;
		let point = format!("coordinator.{}", change.phase().to_ascii_lowercase());
		fault::cut(id, &format!("{point}.before")).await?;
		coordinator_records::transition(lease.handle(), id, change.into(), detail).await?;
		fault::cut(id, &format!("{point}.after")).await?;
		Ok(())
	}

	async fn acquire_recovery(&self, id: Uuid) -> ApplicationResult<Box<dyn RecoveryScope>> {
		let backend = BackendConnection::new(Arc::new(PostgresBackend::new(
			self.runtime.store.control_pool.clone(),
		)));
		let lease = coordinator_records::recovery_lease(&backend, id).await?;
		Ok(Box::new(Recovery {
			id,
			runtime: self.runtime.clone(),
			connection: None,
			lease,
		}))
	}

	async fn recovery_batch(&self, aborted: bool) -> ApplicationResult<Box<dyn RecoveryBatch>> {
		let lease = connection(&self.runtime)?;
		let ids = coordinator_records::recovery_candidates(&mut lease.handle(), aborted).await?;
		Ok(Box::new(Batch {
			ids,
			_connection: lease,
		}))
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
		coordinator::remote::<native::Status>(
			&self.runtime,
			&manifest.coordinator,
			Method::GET,
			&format!("/transactions/{}/decision", manifest.id),
			None::<&()>,
		)
		.await
		.map(Into::into)
		.map_err(Into::into)
	}

	async fn send(
		&self,
		manifest: &Manifest,
		node: &str,
		operation: ParticipantOperation,
	) -> ApplicationResult<LocalStatus> {
		let response: Result<native::LocalStatus> = if node == self.runtime.config.node_id {
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
		} else {
			coordinator::remote(
				&self.runtime,
				node,
				Method::POST,
				&format!("/transactions/{}", operation.as_str()),
				Some(manifest),
			)
			.await
		};
		response.map(Into::into).map_err(Into::into)
	}
}
