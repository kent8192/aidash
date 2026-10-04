//! Admission uses the existing transaction and immutable coordinator records.
use super::authority::persistence;
use crate::apps::federation::transactions::{
	models::{AtomicPeerTrust, coordinator_records},
	serializers::authority as dto,
	services::fault,
};
use crate::{Error, federation::Federation};
use aidash_application::{
	Result,
	ports::transactions::admission::{AdmissionRepository, AdmissionScope, OwnedAdmissionScope},
};
use aidash_domain::transactions::{
	Manifest,
	authority::{Origin, Status},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reinhardt::db::backends::{DatabaseConnection, TransactionExecutor, dialect::PostgresBackend};
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

pub(crate) struct Scope<'a> {
	pub runtime: &'a Federation,
	pub tx: &'a mut dyn TransactionExecutor,
}

#[async_trait]
impl AdmissionScope for Scope<'_> {
	fn node_id(&self) -> &str {
		&self.runtime.config.node_id
	}
	fn now(&self) -> DateTime<Utc> {
		Utc::now()
	}
	async fn status(&mut self, id: Uuid) -> Result<Status> {
		coordinator_records::status_in(self.tx, id)
			.await
			.map(Into::into)
			.map_err(Into::into)
	}
	async fn match_origin(&mut self, id: Uuid, origin: Option<&Origin>) -> Result<()> {
		let origin = origin.cloned().map(dto::Origin::from);
		persistence::match_origin_native(self.tx, id, origin.as_ref())
			.await
			.map_err(Into::into)
	}
	async fn resolve_peer(&mut self, node: &str) -> Result<()> {
		self.runtime
			.peer(node)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn trusted(&mut self, node: &str) -> Result<bool> {
		AtomicPeerTrust::permits(self.tx, node)
			.await
			.map_err(Into::into)
	}
	async fn admit(&mut self, manifest: &Manifest) -> Result<Status> {
		coordinator_records::admit_in(self.tx, manifest)
			.await
			.map(Into::into)
			.map_err(Into::into)
	}
	async fn bind_origin(&mut self, id: Uuid, origin: &Origin) -> Result<()> {
		persistence::bind_native(
			self.tx,
			"atomic_subjects",
			id,
			&json!(dto::Origin::from(origin.clone())),
		)
		.await
		.map_err(Into::into)
	}
}

struct Owned {
	runtime: Federation,
	tx: Box<dyn TransactionExecutor>,
}

#[async_trait]
impl OwnedAdmissionScope for Owned {
	fn admission(&mut self) -> Box<dyn AdmissionScope + '_> {
		Box::new(Scope {
			runtime: &self.runtime,
			tx: self.tx.as_mut(),
		})
	}
	async fn commit(self: Box<Self>) -> Result<()> {
		self.tx
			.commit()
			.await
			.map_err(Error::from)
			.map_err(Into::into)
	}
}

pub(crate) struct Repository {
	pub runtime: Federation,
}

#[async_trait]
impl AdmissionRepository for Repository {
	async fn begin(&self) -> Result<Box<dyn OwnedAdmissionScope>> {
		let db = DatabaseConnection::new(Arc::new(PostgresBackend::new(
			self.runtime.store.control_pool.clone(),
		)));
		let tx = db.begin().await.map_err(Error::from)?;
		Ok(Box::new(Owned {
			runtime: self.runtime.clone(),
			tx,
		}))
	}
	async fn fault(&self, id: Uuid, point: &str) -> Result<()> {
		fault::cut(id, point).await.map_err(Into::into)
	}
	fn wake(&self) {
		self.runtime.notify.notify_waiters();
	}
}
