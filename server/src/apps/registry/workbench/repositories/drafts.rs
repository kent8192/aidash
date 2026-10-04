//! Owned native draft transaction implementing portable editing ports.
use super::authority;
use crate::{
	apps::registry::{
		repositories::NativeScope,
		workbench::{
			models::{AgentDraft, AgentDraftShare},
			serializers::contracts::Draft as NativeDraft,
		},
	},
	authorization::identity::Actor,
	federation::Federation,
};
use aidash_application::{
	Result,
	ports::registry::{
		DefinitionLookup,
		workbench::{DraftAuthority, DraftRepository, DraftScope},
	},
};
use aidash_domain::{
	identity::Principal,
	policy::{Decision, Evaluation, PolicyBundle},
	registry::{
		Entry,
		workbench::{Draft, ShareRecord},
	},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reinhardt::db::backends::{TransactionExecutor, dialect::postgres::PgTransactionExecutor};
use serde_json::Value;
use uuid::Uuid;
pub(crate) struct Repository {
	pub runtime: Federation,
	pub actor: Actor,
}
pub(crate) struct Scope {
	pub tx: PgTransactionExecutor,
	pub actor: Actor,
	node: String,
}
impl Scope {
	fn authority(&mut self) -> authority::Scope<'_> {
		authority::Scope {
			tx: &mut self.tx,
			actor: &self.actor,
		}
	}
}
#[async_trait]
impl DraftRepository for Repository {
	type Scope = Scope;
	fn principal(&self) -> Principal {
		authority::principal(&self.actor)
	}
	async fn original_entry(&self, id: &str, version: &str) -> Result<Entry> {
		self.runtime
			.registry
			.get(id, version)
			.await
			.map_err(Into::into)
	}
	async fn original_documents(&self, entry: &Entry) -> Result<Value> {
		crate::knowledge::load(&self.runtime.registry.db, entry)
			.await
			.map_err(Into::into)
	}
	async fn begin(&self) -> Result<Scope> {
		let result: crate::Result<Scope> = async {
			Ok(Scope {
				tx: PgTransactionExecutor::new(self.runtime.store.pool.begin().await?),
				actor: self.actor.clone(),
				node: self.runtime.config.node_id.clone(),
			})
		}
		.await;
		result.map_err(Into::into)
	}
}
#[async_trait]
impl DefinitionLookup for Scope {
	async fn definition(&mut self, id: &str, version: &str) -> Result<Entry> {
		NativeScope(&mut self.tx).definition(id, version).await
	}
	async fn overrides(&mut self, id: &str, version: &str) -> Result<Option<Value>> {
		NativeScope(&mut self.tx).overrides(id, version).await
	}
	async fn executor_kind(&mut self, id: &str, version: &str) -> Result<Option<String>> {
		NativeScope(&mut self.tx).executor_kind(id, version).await
	}
}
#[async_trait]
impl DraftAuthority for Scope {
	fn principal(&self) -> Principal {
		authority::principal(&self.actor)
	}
	async fn lock_identity(&mut self) -> Result<()> {
		self.authority().lock_identity().await
	}
	async fn share(&mut self, draft: Uuid, subject: &str) -> Result<Option<(bool, String)>> {
		self.authority().share(draft, subject).await
	}
	async fn evaluate(&mut self, tenant: &str, evaluation: &Evaluation) -> Result<Decision> {
		self.authority().evaluate(tenant, evaluation).await
	}
	async fn bundle(&mut self, tenant: &str) -> Result<PolicyBundle> {
		self.authority().bundle(tenant).await
	}
}
#[async_trait]
impl DraftScope for Scope {
	async fn read(&mut self, id: Uuid, lock: bool) -> Result<Draft> {
		Ok(AgentDraft::read(&mut self.tx, id, lock).await?.into())
	}
	async fn insert(&mut self, draft: &Draft, managed_id: &str) -> Result<Draft> {
		Ok(
			AgentDraft::insert(&mut self.tx, &NativeDraft::from(draft.clone()), managed_id)
				.await?
				.into(),
		)
	}
	async fn save_content(
		&mut self,
		id: Uuid,
		entry: Value,
		documents: Value,
		notes: &str,
	) -> Result<Draft> {
		Ok(
			AgentDraft::save_content(&mut self.tx, id, entry, documents, notes)
				.await?
				.into(),
		)
	}
	async fn page(
		&mut self,
		tenant: Option<&str>,
		cursor: Option<(DateTime<Utc>, Uuid)>,
	) -> Result<Vec<Draft>> {
		Ok(AgentDraft::page(&mut self.tx, tenant, cursor)
			.await?
			.into_iter()
			.map(Into::into)
			.collect())
	}
	async fn managed(&mut self, agent: &str) -> Result<bool> {
		AgentDraft::managed(&mut self.tx, agent)
			.await
			.map_err(Into::into)
	}
	async fn append_event(&mut self, kind: &str, payload: Value) -> Result<()> {
		crate::apps::execution::models::event_records::append(
			&mut self.tx,
			&self.node,
			None,
			kind,
			payload,
		)
		.await
		.map(|_| ())
		.map_err(Into::into)
	}
	async fn shares(&mut self, draft: Uuid) -> Result<Vec<ShareRecord>> {
		Ok(AgentDraftShare::page(&mut self.tx, draft)
			.await?
			.into_iter()
			.map(|row| ShareRecord {
				subject: row.subject,
				can_edit: row.can_edit,
				documents_digest: row.documents_digest,
			})
			.collect())
	}
	async fn save_share(
		&mut self,
		draft: Uuid,
		subject: &str,
		can_edit: bool,
		digest: &str,
	) -> Result<()> {
		AgentDraftShare::save(&mut self.tx, draft, subject, can_edit, digest)
			.await
			.map_err(Into::into)
	}
	async fn remove_share(&mut self, draft: Uuid, subject: &str) -> Result<()> {
		AgentDraftShare::remove(&mut self.tx, draft, subject)
			.await
			.map_err(Into::into)
	}
	async fn transfer(&mut self, id: Uuid, owner: &str) -> Result<()> {
		AgentDraft::transfer(&mut self.tx, id, owner)
			.await
			.map_err(Into::into)
	}
	async fn archive(&mut self, id: Uuid, archived: bool) -> Result<()> {
		AgentDraft::archive(&mut self.tx, id, archived)
			.await
			.map_err(Into::into)
	}
	async fn commit(self) -> Result<()> {
		Box::new(self.tx)
			.commit()
			.await
			.map_err(crate::Error::from)
			.map_err(Into::into)
	}
}
