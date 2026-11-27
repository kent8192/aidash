use crate::Result;
use crate::apps::knowledge::models::cleanup;
use crate::apps::knowledge::services::core::service::Lease;
use crate::apps::knowledge::*;
use crate::{authorization::identity::Actor, federation::Federation};
use reinhardt::injectable;
use uuid::Uuid;

#[derive(Clone)]
pub struct SemanticEntries {
	runtime: Federation,
}

#[injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> SemanticEntries {
	SemanticEntries { runtime }
}

impl SemanticEntries {
	pub async fn configure(
		&self,
		actor: Actor,
		thread: Uuid,
		input: ConfigureIndex,
	) -> Result<Index> {
		let f = self.runtime.clone();
		service::configure(&f.store, &actor, thread, input).await
	}
	pub async fn index(&self, actor: Actor, thread: Uuid) -> Result<Index> {
		let f = self.runtime.clone();
		service::get_index(&f.store, &actor, thread).await
	}
	pub async fn put(&self, actor: Actor, thread: Uuid, input: PutEntry) -> Result<Entry> {
		let f = self.runtime.clone();
		service::put(&f.store, &actor, thread, input).await
	}
	pub async fn entries(
		&self,
		actor: Actor,
		thread: Uuid,
	) -> Result<crate::apps::knowledge::serializers::contracts::EntryList> {
		let f = self.runtime.clone();
		service::entries_counted(&f.store, &actor, thread).await
	}
	pub async fn delete(
		&self,
		actor: Actor,
		(thread, id): (Uuid, Uuid),
		input: Revision,
	) -> Result<Entry> {
		let f = self.runtime.clone();
		service::change(&f.store, &actor, thread, id, input.expected_revision, true).await
	}
	pub async fn reindex(
		&self,
		actor: Actor,
		(thread, id): (Uuid, Uuid),
		input: Revision,
	) -> Result<Entry> {
		let f = self.runtime.clone();
		service::change(&f.store, &actor, thread, id, input.expected_revision, false).await
	}
	pub async fn search(&self, actor: Actor, thread: Uuid, input: Search) -> Result<SearchResult> {
		// Retrieval issues reads serially. The authority lease and any nested
		// vector reads borrow from the same Tenant's Work share.
		service::search(&self.runtime.store, &actor, thread, input).await
	}
	pub async fn history(&self, actor: Actor, thread: Uuid) -> Result<Vec<History>> {
		let f = self.runtime.clone();
		service::history_list(&f.store, &actor, thread).await
	}
	/// The Thread's cleanup state, under `semantic.read` for a Subject.
	pub async fn cleanup(&self, actor: Actor, thread: Uuid) -> Result<CleanupStatus> {
		let store = &self.runtime.store;
		let mut lease = Lease::begin(store, &actor, thread).await?;
		let result = async {
			if lease.access().is_some() {
				lease.thread(thread, "semantic.read").await?;
			}
			cleanup::status(&mut **lease.tx(), thread).await
		}
		.await;
		lease.finish(result).await
	}
}
