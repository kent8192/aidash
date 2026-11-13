//! What a walk over recorded reads has loaded, so it asks each table once per Run
//! and not once per read (ADR 0116). Loaded rows end with the walk. What passed
//! lasts while later walks ask under the same authority, until a walk fails or the
//! authority is refreshed, as `read_verdicts` does. A scope that reads under
//! another authority than the walk's, as a Remote Grant's producer does, lends
//! its own passes for as long as it does.
use super::Access;
use crate::apps::workspaces::models::{Artifact, Conversation, Message, Task};
use crate::{Result, domain};
use aidash_application::ports::authorization::visibility::provenance::RecordedRead;
use aidash_domain::{nodes::NodeName, registry::EntityRef};
use reinhardt::db::orm::execution::convert_values;
use reinhardt::query::{
	Alias, BinOper, Expr, ExprTrait as _, IntoIden, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SimpleExpr, types::PgBinOper,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

type Recorded = (Uuid, Uuid);
type Output = (Uuid, String, Uuid);
type EntryKey = (String, String);
type NodeEntries<T> = (String, BTreeMap<EntryKey, T>);

/// `None` from `get` means nobody asked for this key: the caller reads it itself.
#[derive(Clone)]
struct Loaded<K, V> {
	asked: BTreeSet<K>,
	found: BTreeMap<K, V>,
}

impl<K, V> Default for Loaded<K, V> {
	fn default() -> Self {
		Self {
			asked: BTreeSet::new(),
			found: BTreeMap::new(),
		}
	}
}

impl<K: Ord + Clone, V: Clone> Loaded<K, V> {
	fn get(&self, key: &K) -> Option<Option<V>> {
		self.asked
			.contains(key)
			.then(|| self.found.get(key).cloned())
	}
}

#[derive(Default)]
pub(crate) struct ReadLookups {
	tasks: Loaded<Recorded, domain::Task>,
	artifacts: Loaded<Recorded, domain::Artifact>,
	messages: Loaded<Recorded, domain::Message>,
	conversations: Loaded<Recorded, domain::Conversation>,
	attachments: Loaded<Recorded, Uuid>,
	producers: Loaded<Output, Vec<Uuid>>,
	grants: Loaded<Output, Vec<Uuid>>,
	walking: bool,
	state: Option<StateRows>,
	passes: Passes,
	/// Evaluations already allowed under this authority (`Access::repeat_key`).
	pub(super) allowed: BTreeSet<String>,
}

/// Raw rows loaded only for the dashboard read. Every row is still checked by
/// the ordinary disclosure code; this is not a cache of authorization verdicts.
struct StateRows {
	authority: String,
	epoch: u64,
	tasks: BTreeMap<Recorded, domain::Task>,
	source_tasks: Loaded<Recorded, domain::Task>,
	source_artifacts: Loaded<Recorded, domain::Artifact>,
	source_messages: Loaded<Recorded, domain::Message>,
	source_conversations: Loaded<Recorded, domain::Conversation>,
	source_attachments: Loaded<Recorded, Uuid>,
	outputs: Loaded<Output, Vec<Uuid>>,
	remote_outputs: Loaded<Output, Vec<Uuid>>,
	runs: BTreeMap<Recorded, domain::RunMetadata>,
	sources: BTreeMap<Uuid, Vec<RecordedRead>>,
	registry_reads: BTreeMap<Uuid, Vec<(String, String)>>,
	definitions: Loaded<(String, String), Value>,
	catalog_restrictions: Option<NodeEntries<Option<Value>>>,
	catalog_documents: Option<NodeEntries<Value>>,
	installations: Loaded<String, Value>,
	revisions: Loaded<String, Value>,
	remote_reads: BTreeMap<
		Uuid,
		Vec<aidash_application::ports::federation::registry_reads::RegistryJournalRow>,
	>,
	snapshots: BTreeMap<Uuid, Option<Value>>,
	received: BTreeMap<Uuid, Option<(String, Uuid, String)>>,
	binding_runs: BTreeSet<Uuid>,
	memory_reads: BTreeSet<Uuid>,
	semantic_reads: BTreeMap<Uuid, Vec<(Uuid, i64)>>,
	core_areas: BTreeMap<
		Uuid,
		Option<Option<crate::apps::execution::capabilities::serializers::contracts::Area>>,
	>,
	humans: BTreeMap<Recorded, Vec<domain::HumanRequest>>,
}

/// A walk reaches the same Run and definition many ways, so it asks about each
/// once. A walk passes only if everything it reached passes, and a failed walk
/// forgets all of this, so a list that hides one item cannot pass it by accident.
/// All of it is true of one authority: a Run visited under one chain has not been
/// visited under another.
#[derive(Default)]
pub(crate) struct Passes {
	authority: Option<String>,
	visited: BTreeSet<(Uuid, bool)>,
	dependencies: BTreeSet<(Uuid, bool)>,
	catalog_reads: BTreeSet<(Option<NodeName>, String, String, bool)>,
	restrictions: BTreeMap<(String, String, String), Option<Value>>,
	areas: BTreeMap<Uuid, (bool, Value)>,
}

impl ReadLookups {
	fn forget_rows(&mut self) {
		*self = Self {
			passes: std::mem::take(&mut self.passes),
			state: self.state.take(),
			allowed: std::mem::take(&mut self.allowed),
			..Self::default()
		};
	}
	pub(super) fn start_walk(&mut self, authority: String) {
		self.forget_rows();
		self.rebind(authority);
		self.walking = true;
	}
	/// Keep the passes only if they were made under `authority`.
	pub(crate) fn rebind(&mut self, authority: String) {
		if self.passes.authority.as_deref() != Some(authority.as_str()) {
			if self
				.state
				.as_ref()
				.is_some_and(|rows| rows.authority != authority)
			{
				self.state = None;
			}
			self.passes = Passes {
				authority: Some(authority),
				..Passes::default()
			};
		}
	}
	/// Hand the passes to a scope that reads under `authority`, which starts with
	/// none. The caller gives them back with [`ReadLookups::restore_passes`].
	pub(crate) fn lend_passes(&mut self, authority: String) -> Passes {
		std::mem::replace(
			&mut self.passes,
			Passes {
				authority: Some(authority),
				..Passes::default()
			},
		)
	}
	/// What the scope that borrowed the walk's authority leaves behind is dropped.
	pub(crate) fn restore_passes(&mut self, passes: Passes) {
		self.passes = passes;
	}
	pub(super) fn end_walk(&mut self, passed: bool) {
		self.forget_rows();
		if !passed {
			self.passes = Passes::default();
		}
	}
	pub(crate) fn walking(&self) -> bool {
		self.walking
	}
	/// `native` keeps the two readers apart, as `read_verdicts` does: only one of
	/// them verifies remote dependencies.
	pub(crate) fn first_visit(&mut self, run: Uuid, native: bool) -> bool {
		!self.walking || self.passes.visited.insert((run, native))
	}
	pub(crate) fn dependency_passed(&self, run: Uuid, native: bool) -> bool {
		self.walking && self.passes.dependencies.contains(&(run, native))
	}
	pub(crate) fn remember_dependency(&mut self, run: Uuid, native: bool) {
		if self.walking {
			self.passes.dependencies.insert((run, native));
		}
	}
	pub(crate) fn catalog_read_passed(
		&self,
		node: &Option<NodeName>,
		reference: &EntityRef,
		native: bool,
	) -> bool {
		self.walking
			&& self.passes.catalog_reads.contains(&(
				node.clone(),
				reference.id.clone(),
				reference.version.clone(),
				native,
			))
	}
	pub(crate) fn remember_catalog_read(
		&mut self,
		node: &Option<NodeName>,
		reference: &EntityRef,
		native: bool,
	) {
		if self.walking {
			self.passes.catalog_reads.insert((
				node.clone(),
				reference.id.clone(),
				reference.version.clone(),
				native,
			));
		}
	}
	pub(crate) fn restriction(&self, node: &str, reference: &EntityRef) -> Option<Option<Value>> {
		if !self.walking {
			return None;
		}
		self.passes
			.restrictions
			.get(&(
				node.to_owned(),
				reference.id.clone(),
				reference.version.clone(),
			))
			.cloned()
	}
	pub(crate) fn remember_restriction(
		&mut self,
		node: &str,
		reference: &EntityRef,
		restriction: &Option<Value>,
	) {
		if self.walking {
			self.passes.restrictions.insert(
				(
					node.to_owned(),
					reference.id.clone(),
					reference.version.clone(),
				),
				restriction.clone(),
			);
		}
	}
	/// Whether a Run's Working Area was readable in this walk, and the context that
	/// checking it left behind.
	pub(crate) fn area(&self, run: Uuid) -> Option<(bool, Value)> {
		if !self.walking {
			return None;
		}
		self.passes.areas.get(&run).cloned()
	}
	pub(crate) fn remember_area(&mut self, run: Uuid, readable: bool, context: &Value) {
		if self.walking {
			self.passes.areas.insert(run, (readable, context.clone()));
		}
	}
	pub(crate) fn task(&self, id: Uuid, workspace: Uuid) -> Option<Option<domain::Task>> {
		self.tasks.get(&(id, workspace))
	}
	pub(crate) fn artifact(&self, id: Uuid, workspace: Uuid) -> Option<Option<domain::Artifact>> {
		self.artifacts.get(&(id, workspace))
	}
	pub(crate) fn message(&self, id: Uuid, workspace: Uuid) -> Option<Option<domain::Message>> {
		self.messages.get(&(id, workspace))
	}
	pub(crate) fn conversation(
		&self,
		id: Uuid,
		workspace: Uuid,
	) -> Option<Option<domain::Conversation>> {
		self.conversations.get(&(id, workspace))
	}
	pub(crate) fn attachment(&self, id: Uuid, workspace: Uuid) -> Option<Option<Uuid>> {
		self.attachments.get(&(id, workspace))
	}
	pub(crate) fn producers(&self, workspace: Uuid, kind: &str, id: Uuid) -> Option<Vec<Uuid>> {
		self.producers
			.get(&(workspace, kind.to_owned(), id))
			.map(Option::unwrap_or_default)
	}
	pub(crate) fn remote_grants(&self, workspace: Uuid, kind: &str, id: Uuid) -> Option<Vec<Uuid>> {
		self.grants
			.get(&(workspace, kind.to_owned(), id))
			.map(Option::unwrap_or_default)
	}
}

#[derive(Deserialize)]
struct StateSource {
	run_id: Uuid,
	workspace_id: Uuid,
	resource_kind: String,
	resource_id: Uuid,
}
#[derive(Deserialize)]
struct StateRegistryRead {
	run_id: Uuid,
	entry_id: String,
	entry_version: String,
}
#[derive(Deserialize)]
struct StateRemoteRead {
	run_id: Uuid,
	node_id: String,
	entry_id: String,
	entry_version: String,
	digest: String,
	metadata: Value,
	same_tenant: bool,
}
#[derive(Deserialize)]
struct StateWorkspace {
	id: Uuid,
	node: String,
}
#[derive(Deserialize)]
struct StateOwner {
	workspace_id: Uuid,
	owner_subject: String,
}
#[derive(Deserialize)]
struct StateDocument {
	key: String,
	document: Value,
}
#[derive(Deserialize)]
struct StateRestriction {
	entry_id: String,
	entry_version: String,
	restriction: Option<Value>,
}
#[derive(Deserialize)]
struct StateDefinition {
	id: String,
	version: String,
	metadata: Value,
}
#[derive(Deserialize)]
struct StateSemanticRead {
	run_id: Uuid,
	entry_id: Uuid,
	revision: i64,
}
#[derive(Deserialize)]
struct StateRunId {
	run_id: Uuid,
}
#[derive(Deserialize)]
struct StateReceived {
	id: Uuid,
	source_node: String,
	grant_id: Uuid,
	node: String,
}
#[derive(Deserialize)]
struct StateCoreRun {
	run_id: Uuid,
	area_id: Uuid,
}
#[derive(Deserialize)]
struct StateSnapshot {
	id: Uuid,
	context: Value,
}

#[derive(Deserialize)]
#[serde(tag = "kind", content = "row", rename_all = "snake_case")]
enum StateBatchRow {
	Task(domain::Task),
	RunOutput(StateRunOutput),
	GrantOutput(StateGrantOutput),
	Definition(StateDefinition),
	Human(domain::HumanRequest),
	Source(StateSource),
	Registry(StateRegistryRead),
	Remote(StateRemoteRead),
	Snapshot(StateSnapshot),
	Received(StateReceived),
	Core(StateCoreRun),
	Memory(StateRunId),
	Binding(StateRunId),
	Semantic(StateSemanticRead),
}

#[derive(Deserialize)]
struct StateRunOutput {
	workspace_id: Uuid,
	resource_kind: String,
	resource_id: Uuid,
	run_id: Uuid,
}

#[derive(Deserialize)]
struct StateGrantOutput {
	workspace_id: Uuid,
	resource_kind: String,
	resource_id: Uuid,
	grant_id: Uuid,
}

#[derive(Deserialize)]
struct AttachmentMessage {
	id: Uuid,
	workspace_id: Uuid,
	message_id: Uuid,
}

#[derive(Deserialize)]
struct ProducedOutput {
	workspace_id: Uuid,
	resource_kind: String,
	resource_id: Uuid,
	owner: Uuid,
}

#[derive(Deserialize)]
struct StateProducedOutput {
	workspace_id: Uuid,
	resource_kind: String,
	resource_id: Uuid,
	owner: Uuid,
	remote: bool,
}

/// Which of `wanted` nobody has asked for yet, marking them as asked.
fn unasked<V>(loaded: &mut Loaded<Recorded, V>, wanted: Vec<Recorded>) -> Vec<Uuid> {
	wanted
		.into_iter()
		.filter(|key| loaded.asked.insert(*key))
		.map(|(id, _)| id)
		.collect()
}

fn snapshot_definitions(snapshot: &Value, deployment: &str, wanted: &mut BTreeSet<EntryKey>) {
	if let Some(definitions) = snapshot.get("definitions").and_then(Value::as_array) {
		for definition in definitions {
			let identity = &definition["identity"];
			if identity["registry_deployment"].as_str() == Some(deployment)
				&& let (Some(id), Some(version)) =
					(identity["id"].as_str(), identity["version"].as_str())
			{
				wanted.insert((id.to_owned(), version.to_owned()));
			}
		}
	}
}

impl Access {
	pub(crate) fn has_state_preload(&mut self) -> bool {
		self.current_state_rows().is_some()
	}

	fn current_state_rows(&mut self) -> Option<&StateRows> {
		let authority = self.authority_context();
		let epoch = self.tx.transaction.as_ref().map_or(0, |tx| tx.lock_epoch());
		if self
			.read_lookups
			.state
			.as_ref()
			.is_some_and(|rows| rows.authority != authority || rows.epoch != epoch)
		{
			self.read_lookups.state = None;
		}
		self.read_lookups.state.as_ref()
	}

	pub(crate) fn state_task(&mut self, id: Uuid, workspace: Uuid) -> Option<domain::Task> {
		self.current_state_rows()?
			.tasks
			.get(&(id, workspace))
			.cloned()
	}

	pub(crate) fn state_source_task(
		&mut self,
		id: Uuid,
		workspace: Uuid,
	) -> Option<Option<domain::Task>> {
		self.current_state_rows()?
			.source_tasks
			.get(&(id, workspace))
	}
	pub(crate) fn state_source_artifact(
		&mut self,
		id: Uuid,
		workspace: Uuid,
	) -> Option<Option<domain::Artifact>> {
		self.current_state_rows()?
			.source_artifacts
			.get(&(id, workspace))
	}
	pub(crate) fn state_source_message(
		&mut self,
		id: Uuid,
		workspace: Uuid,
	) -> Option<Option<domain::Message>> {
		self.current_state_rows()?
			.source_messages
			.get(&(id, workspace))
	}
	pub(crate) fn state_source_conversation(
		&mut self,
		id: Uuid,
		workspace: Uuid,
	) -> Option<Option<domain::Conversation>> {
		self.current_state_rows()?
			.source_conversations
			.get(&(id, workspace))
	}
	pub(crate) fn state_source_attachment(
		&mut self,
		id: Uuid,
		workspace: Uuid,
	) -> Option<Option<Uuid>> {
		self.current_state_rows()?
			.source_attachments
			.get(&(id, workspace))
	}
	pub(crate) fn state_producers(
		&mut self,
		workspace: Uuid,
		kind: &str,
		id: Uuid,
	) -> Option<Vec<Uuid>> {
		self.current_state_rows()?
			.outputs
			.get(&(workspace, kind.to_owned(), id))
			.map(Option::unwrap_or_default)
	}
	pub(crate) fn state_remote_outputs(
		&mut self,
		workspace: Uuid,
		kind: &str,
		id: Uuid,
	) -> Option<Vec<Uuid>> {
		self.current_state_rows()?
			.remote_outputs
			.get(&(workspace, kind.to_owned(), id))
			.map(Option::unwrap_or_default)
	}
	pub(crate) async fn prepare_state_tasks(&mut self, tasks: &[domain::Task]) -> Result<()> {
		self.prepare_state_workspaces(
			&tasks
				.iter()
				.map(|task| task.workspace_id)
				.collect::<Vec<_>>(),
		)
		.await?;
		if self.current_state_rows().is_none() {
			return Ok(());
		}
		let reads: Vec<_> = tasks
			.iter()
			.map(|task| (task.workspace_id, "task".to_owned(), task.id))
			.collect();
		self.prepare_read_sources(&reads).await?;
		let outputs = self.read_lookups.producers.clone();
		let remote_outputs = self.read_lookups.grants.clone();
		if let Some(rows) = self.read_lookups.state.as_mut() {
			for task in tasks {
				rows.tasks
					.insert((task.id, task.workspace_id), task.clone());
			}
			rows.outputs = outputs;
			rows.remote_outputs = remote_outputs;
		}
		Ok(())
	}
	pub(crate) async fn prepare_state_artifacts(
		&mut self,
		artifacts: &[domain::Artifact],
	) -> Result<()> {
		if self.current_state_rows().is_none() || artifacts.is_empty() {
			return Ok(());
		}
		self.prepare_state_workspaces(
			&artifacts
				.iter()
				.map(|row| row.workspace_id)
				.collect::<Vec<_>>(),
		)
		.await?;
		let reads: Vec<_> = artifacts
			.iter()
			.map(|row| (row.workspace_id, "artifact".to_owned(), row.id))
			.collect();
		self.prepare_read_sources(&reads).await?;
		let source_tasks = self.read_lookups.tasks.clone();
		let source_artifacts = self.read_lookups.artifacts.clone();
		let outputs = self.read_lookups.producers.clone();
		let remote_outputs = self.read_lookups.grants.clone();
		if let Some(rows) = self.read_lookups.state.as_mut() {
			rows.source_tasks = source_tasks;
			rows.source_artifacts = source_artifacts;
			rows.outputs = outputs;
			rows.remote_outputs = remote_outputs;
		}
		Ok(())
	}
	pub(crate) fn state_run(&mut self, id: Uuid, workspace: Uuid) -> Option<domain::RunMetadata> {
		self.current_state_rows()?
			.runs
			.get(&(id, workspace))
			.cloned()
	}

	pub(crate) fn state_humans(
		&mut self,
		workspace: Uuid,
		run: Uuid,
	) -> Option<Vec<domain::HumanRequest>> {
		self.current_state_rows()?
			.humans
			.get(&(run, workspace))
			.cloned()
	}

	pub(crate) fn state_sources(&mut self, run: Uuid) -> Option<Vec<RecordedRead>> {
		self.current_state_rows()?.sources.get(&run).cloned()
	}
	pub(crate) fn begin_state_registry_preload(&mut self) {
		self.read_lookups.state = Some(StateRows {
			authority: self.authority_context(),
			epoch: self.tx.transaction.as_ref().map_or(0, |tx| tx.lock_epoch()),
			tasks: BTreeMap::new(),
			source_tasks: Loaded::default(),
			source_artifacts: Loaded::default(),
			source_messages: Loaded::default(),
			source_conversations: Loaded::default(),
			source_attachments: Loaded::default(),
			outputs: Loaded::default(),
			remote_outputs: Loaded::default(),
			runs: BTreeMap::new(),
			sources: BTreeMap::new(),
			registry_reads: BTreeMap::new(),
			definitions: Loaded::default(),
			catalog_restrictions: None,
			catalog_documents: None,
			installations: Loaded::default(),
			revisions: Loaded::default(),
			remote_reads: BTreeMap::new(),
			snapshots: BTreeMap::new(),
			received: BTreeMap::new(),
			binding_runs: BTreeSet::new(),
			memory_reads: BTreeSet::new(),
			semantic_reads: BTreeMap::new(),
			core_areas: BTreeMap::new(),
			humans: BTreeMap::new(),
		});
	}
	pub(crate) async fn prepare_state_builtin_definitions(&mut self) -> Result<()> {
		let wanted: BTreeSet<_> = aidash_application::registry::system::declarations(
			&crate::bootstrap::registry_validation(),
			&self.node_id,
		)?
		.into_iter()
		.map(|entry| (entry.id, entry.version))
		.collect();
		if wanted.is_empty() {
			return Ok(());
		}
		let ids: BTreeSet<_> = wanted.iter().map(|(id, _)| id.as_str()).collect();
		let versions: BTreeSet<_> = wanted.iter().map(|(_, version)| version.as_str()).collect();
		let (sql, values) = Query::select()
			.columns(["id", "version", "metadata"].map(Alias::new))
			.from(Alias::new("registry"))
			.and_where(Expr::col("tenant").eq(Expr::value(&self.identity.tenant)))
			.and_where(Expr::col("id").is_in(ids.into_iter().map(Expr::value)))
			.and_where(Expr::col("version").is_in(versions.into_iter().map(Expr::value)))
			.lock(reinhardt::query::LockType::Share)
			.build(PostgresQueryBuilder);
		let entries: Vec<StateDefinition> = self.fetch_rows(&sql, values).await?;
		if let Some(rows) = self.read_lookups.state.as_mut() {
			rows.definitions.asked.extend(wanted.iter().cloned());
			for entry in entries {
				let key = (entry.id, entry.version);
				if wanted.contains(&key) {
					rows.definitions.found.insert(key, entry.metadata);
				}
			}
		}
		Ok(())
	}
	pub(crate) fn state_installation(
		&mut self,
		id: &str,
	) -> Option<Result<Option<aidash_domain::marketplace::Installation>>> {
		let value = self
			.current_state_rows()?
			.installations
			.get(&id.to_owned())?;
		Some(
			value
				.map(serde_json::from_value)
				.transpose()
				.map_err(Into::into),
		)
	}
	pub(crate) fn state_revision(
		&mut self,
		id: &str,
		revision: i64,
	) -> Option<Result<aidash_domain::marketplace::Revision>> {
		let key = aidash_domain::marketplace::definitions::key(&(id, revision));
		let value = self.current_state_rows()?.revisions.get(&key)?;
		Some(match value {
			Some(value) => serde_json::from_value(value).map_err(Into::into),
			None => Err(crate::Error::Forbidden),
		})
	}
	pub(crate) async fn prepare_state_marketplace(&mut self, entries: &[Value]) -> Result<()> {
		if self.current_state_rows().is_none() {
			return Ok(());
		}
		let projections: Vec<_> = entries
			.iter()
			.filter_map(|value| {
				serde_json::from_value::<crate::registry::Entry>(value.clone()).ok()
			})
			.filter_map(|entry| entry.installation)
			.collect();
		let installations: BTreeSet<_> =
			projections.iter().map(|p| p.installation.clone()).collect();
		let revisions: BTreeSet<_> = projections
			.iter()
			.map(|p| aidash_domain::marketplace::definitions::key(&(&p.installation, p.revision)))
			.collect();
		let mut loaded_installations = Loaded {
			asked: installations.clone(),
			..Loaded::default()
		};
		let mut loaded_revisions = Loaded {
			asked: revisions.clone(),
			..Loaded::default()
		};
		for (table, wanted, loaded, lock) in [
			(
				"marketplace_installations",
				&installations,
				&mut loaded_installations,
				true,
			),
			(
				"marketplace_revisions",
				&revisions,
				&mut loaded_revisions,
				false,
			),
		] {
			if wanted.is_empty() {
				continue;
			}
			let mut query = Query::select();
			query
				.columns(["key", "document"].map(Alias::new))
				.from(Alias::new(table))
				.and_where(
					Expr::col("key").is_in(wanted.iter().map(|key| Expr::value(key.as_str()))),
				);
			if lock {
				query.lock(reinhardt::query::LockType::Share);
			}
			let (sql, values) = query.build(PostgresQueryBuilder);
			let rows: Vec<StateDocument> = self.fetch_rows(&sql, values).await?;
			for row in rows {
				loaded.found.insert(row.key, row.document);
			}
		}
		if let Some(rows) = self.read_lookups.state.as_mut() {
			rows.installations = loaded_installations;
			rows.revisions = loaded_revisions;
		}
		Ok(())
	}
	pub(crate) fn state_catalog_document(
		&mut self,
		node: &str,
		reference: &EntityRef,
	) -> Option<Option<Value>> {
		if self.inherited_lease {
			return None;
		}
		let (loaded_node, documents) = self.current_state_rows()?.catalog_documents.as_ref()?;
		(loaded_node == node).then(|| {
			documents
				.get(&(reference.id.clone(), reference.version.clone()))
				.cloned()
		})
	}
	pub(crate) async fn remember_state_documents(&mut self, documents: &[Value]) -> Result<()> {
		if self.current_state_rows().is_none() {
			return Ok(());
		}
		let node = self.catalog_node().await?;
		let documents = documents
			.iter()
			.filter_map(|document| {
				Some((
					(
						document.get("id")?.as_str()?.to_owned(),
						document.get("version")?.as_str()?.to_owned(),
					),
					document.clone(),
				))
			})
			.collect();
		if let Some(rows) = self.read_lookups.state.as_mut() {
			rows.catalog_documents = Some((node.to_string(), documents));
		}
		Ok(())
	}
	pub(crate) fn state_restriction(
		&mut self,
		node: &str,
		reference: &EntityRef,
	) -> Option<Option<Value>> {
		let (loaded_node, rows) = self.current_state_rows()?.catalog_restrictions.as_ref()?;
		(loaded_node == node).then(|| {
			rows.get(&(reference.id.clone(), reference.version.clone()))
				.cloned()
				.flatten()
		})
	}
	pub(crate) async fn prepare_state_restrictions(&mut self) -> Result<()> {
		if self.current_state_rows().is_none() {
			return Ok(());
		}
		let node = self.catalog_node().await?;
		let (sql, values) = Query::select()
			.columns(["entry_id", "entry_version", "restriction"].map(Alias::new))
			.from(Alias::new("authorization_catalog"))
			.and_where(Expr::col("tenant").eq(Expr::value(&self.identity.tenant)))
			.and_where(Expr::col("node").eq(Expr::value(node.as_str())))
			.and_where(Expr::col("enabled"))
			.build(PostgresQueryBuilder);
		let loaded: Vec<StateRestriction> = self.fetch_rows(&sql, values).await?;
		let rows = loaded
			.into_iter()
			.map(|row| ((row.entry_id, row.entry_version), row.restriction))
			.collect();
		if let Some(state) = self.read_lookups.state.as_mut() {
			state.catalog_restrictions = Some((node.to_string(), rows));
		}
		Ok(())
	}
	pub(crate) fn state_definition(
		&mut self,
		reference: &EntityRef,
	) -> Option<Result<crate::registry::Entry>> {
		let value = self
			.current_state_rows()?
			.definitions
			.get(&(reference.id.clone(), reference.version.clone()))?;
		Some(match value {
			Some(value) => serde_json::from_value(value).map_err(Into::into),
			None => Err(crate::Error::NotFound(format!(
				"entity {}@{}",
				reference.id, reference.version
			))),
		})
	}
	pub(crate) fn state_registry_reads(&mut self, run: Uuid) -> Option<Vec<(String, String)>> {
		self.current_state_rows()?.registry_reads.get(&run).cloned()
	}
	pub(crate) fn state_remote_reads(
		&mut self,
		run: Uuid,
	) -> Option<Vec<aidash_application::ports::federation::registry_reads::RegistryJournalRow>> {
		self.current_state_rows()?.remote_reads.get(&run).cloned()
	}
	pub(crate) fn state_metadata(&mut self, run: Uuid) -> Option<domain::RunMetadata> {
		self.current_state_rows()?
			.runs
			.values()
			.find(|row| row.id == run)
			.cloned()
	}
	pub(crate) fn state_binding_exists(&mut self, run: Uuid) -> Option<bool> {
		let rows = self.current_state_rows()?;
		rows.runs
			.keys()
			.any(|(id, _)| *id == run)
			.then(|| rows.binding_runs.contains(&run))
	}
	pub(crate) fn state_memory_reads(&mut self, run: Uuid) -> Option<bool> {
		let rows = self.current_state_rows()?;
		rows.runs
			.keys()
			.any(|(id, _)| *id == run)
			.then(|| rows.memory_reads.contains(&run))
	}
	pub(crate) fn state_semantic_reads(&mut self, run: Uuid) -> Option<Vec<(Uuid, i64)>> {
		self.current_state_rows()?.semantic_reads.get(&run).cloned()
	}
	pub(crate) fn state_received(&mut self, run: Uuid) -> Option<Option<(String, Uuid, String)>> {
		self.current_state_rows()?.received.get(&run).cloned()
	}
	pub(crate) fn state_core_area(
		&mut self,
		run: Uuid,
	) -> Option<Option<Option<crate::apps::execution::capabilities::serializers::contracts::Area>>>
	{
		self.current_state_rows()?.core_areas.get(&run).cloned()
	}
	pub(crate) fn state_snapshot(&mut self, run: Uuid) -> Option<Option<Value>> {
		self.current_state_rows()?.snapshots.get(&run).cloned()
	}

	/// Load the state window's Workspace rows while retaining the usual live-node and lock checks.
	pub(crate) async fn prepare_state_workspaces(&mut self, ids: &[Uuid]) -> Result<()> {
		self.tx.current_workspaces();
		let ids: Vec<_> = ids
			.iter()
			.copied()
			.filter(|id| {
				!self.tx.workspace_nodes.contains_key(id)
					|| !self.tx.workspace_owners.contains_key(&(*id, true))
			})
			.collect();
		if ids.is_empty() {
			return Ok(());
		}
		let (sql, values) = Query::select()
			.columns(["id", "node"].map(Alias::new))
			.from(Alias::new("workspaces"))
			.and_where(Expr::col("tenant").eq(Expr::value(&self.identity.tenant)))
			.and_where(Expr::col("id").is_in(ids.iter().copied().map(Expr::value)))
			.build(PostgresQueryBuilder);
		let workspaces: Vec<StateWorkspace> = self.fetch_rows(&sql, values).await?;
		let (sql, values) = Query::select()
			.columns(["workspace_id", "owner_subject"].map(Alias::new))
			.from(Alias::new("authorization_workspaces"))
			.and_where(
				crate::apps::identity::repositories::nodes::live_workspace_condition(
					Expr::col((
						Alias::new("authorization_workspaces"),
						Alias::new("workspace_id"),
					))
					.into(),
					Expr::col((Alias::new("authorization_workspaces"), Alias::new("tenant")))
						.into(),
				),
			)
			.and_where(Expr::col("tenant").eq(Expr::value(&self.identity.tenant)))
			.and_where(Expr::col("workspace_id").is_in(ids.iter().copied().map(Expr::value)))
			.lock(reinhardt::query::LockType::Share)
			.build(PostgresQueryBuilder);
		let owners: Vec<StateOwner> = self.fetch_rows(&sql, values).await?;
		for workspace in workspaces {
			self.tx.workspace_nodes.insert(workspace.id, workspace.node);
		}
		for owner in owners {
			self.tx
				.remember_workspace_owner(owner.workspace_id, true, &owner.owner_subject);
		}
		Ok(())
	}

	/// State is the only caller; policies are evaluated by the usual readers.
	pub(crate) async fn prepare_state_runs(&mut self, runs: &[domain::RunMetadata]) -> Result<()> {
		self.prepare_state_runs_inner(runs, None).await
	}

	pub(crate) async fn prepare_state_inspection(
		&mut self,
		run: &domain::RunInspection,
	) -> Result<()> {
		let snapshot = run
			.context
			.as_ref()
			.and_then(|context| context.binding_snapshot.as_deref())
			.map(serde_json::to_value)
			.transpose()?;
		self.prepare_state_runs_inner(std::slice::from_ref(&run.metadata), snapshot.as_ref())
			.await
	}

	async fn prepare_state_runs_inner(
		&mut self,
		runs: &[domain::RunMetadata],
		known_snapshot: Option<&Value>,
	) -> Result<()> {
		if runs.is_empty() {
			self.begin_state_registry_preload();
			self.prepare_state_builtin_definitions().await?;
			return Ok(());
		}
		let task_ids: Vec<_> = runs.iter().map(|run| run.task_id).collect();
		let run_ids: Vec<_> = runs.iter().map(|run| run.id).collect();
		let mut early_definitions = BTreeSet::new();
		for entry in aidash_application::registry::system::declarations(
			&crate::bootstrap::registry_validation(),
			&self.node_id,
		)? {
			early_definitions.insert((entry.id, entry.version));
		}
		if let Some(snapshot) = known_snapshot {
			snapshot_definitions(snapshot, &self.node_id, &mut early_definitions);
		}
		// These are independent row reads under one authority. UNION ALL keeps the
		// ordinary visibility checks in Rust while avoiding a round trip per table.
		let selection = |table: &str, kind: &str, id_column: &str, ids: &[Uuid]| {
			let mut query = Query::select();
			query
				.expr_as(Expr::value(kind).cast_as("text"), Alias::new("kind"))
				.expr_as(
					SimpleExpr::FunctionCall(
						"to_jsonb".into_iden(),
						vec![Expr::col("item").into()],
					),
					Alias::new("row"),
				)
				.from_as(Alias::new(table), Alias::new("item"))
				.and_where(
					Expr::col((Alias::new("item"), Alias::new(id_column)))
						.is_in(ids.iter().copied().map(Expr::value)),
				);
			query
		};
		let mut query = selection("human_requests", "human", "run_id", &run_ids);
		query.union_all(selection("tasks", "task", "id", &task_ids));
		for (table, kind) in [
			("authorization_run_outputs", "run_output"),
			("authorization_remote_outputs", "grant_output"),
		] {
			let mut outputs = selection(table, kind, "resource_id", &task_ids);
			outputs.and_where(
				Expr::col((Alias::new("item"), Alias::new("resource_kind")))
					.eq(Expr::value("task")),
			);
			query.union_all(outputs);
		}
		for (table, kind, id_column) in [
			("authorization_run_reads", "source", "run_id"),
			("authorization_run_registry_reads", "registry", "run_id"),
			("authorization_run_remote_reads", "remote", "run_id"),
			("runs", "snapshot", "id"),
			("core_runs", "core", "run_id"),
			("semantic_run_reads", "semantic", "run_id"),
		] {
			query.union_all(selection(table, kind, id_column, &run_ids));
		}
		let mut received_query = selection(
			"authorization_remote_admissions",
			"received",
			"id",
			&run_ids,
		);
		let semantic = SimpleExpr::Binary(
			Box::new(Expr::col((Alias::new("item"), Alias::new("description"))).into()),
			BinOper::PgOperator(PgBinOper::JsonGetByIndex),
			Box::new(Expr::value("semantic").into()),
		);
		let mode = SimpleExpr::Binary(
			Box::new(semantic),
			BinOper::PgOperator(PgBinOper::JsonGetAsText),
			Box::new(Expr::value("mode").into()),
		);
		received_query
			.and_where(
				Expr::col((Alias::new("item"), Alias::new("tenant")))
					.eq(Expr::value(&self.identity.tenant)),
			)
			.and_where(mode.eq(Expr::value("required_home")));
		query.union_all(received_query);
		let mut memory_ids = Query::select();
		memory_ids
			.distinct()
			.column(Alias::new("run_id"))
			.from(Alias::new("memory_run_reads"))
			.and_where(Expr::col("run_id").is_in(run_ids.iter().copied().map(Expr::value)));
		let mut memory_query = Query::select();
		memory_query
			.expr_as(Expr::value("memory").cast_as("text"), Alias::new("kind"))
			.expr_as(
				SimpleExpr::FunctionCall("to_jsonb".into_iden(), vec![Expr::col("item").into()]),
				Alias::new("row"),
			)
			.from_subquery(memory_ids, Alias::new("item"));
		query.union_all(memory_query);
		// A referenced locking CTE holds exactly the same shared row locks as the
		// single-Run binding lookup, through the enclosing transaction.
		let mut bindings_query = Query::select();
		bindings_query
			.column(Alias::new("run_id"))
			.from(Alias::new("memory_run_bindings"))
			.and_where(Expr::col("run_id").is_in(run_ids.iter().copied().map(Expr::value)))
			.lock(reinhardt::query::LockType::Share);
		query.with_cte("state_locked_bindings", bindings_query);
		let mut binding_rows = Query::select();
		binding_rows
			.expr_as(Expr::value("binding").cast_as("text"), Alias::new("kind"))
			.expr_as(
				SimpleExpr::FunctionCall("to_jsonb".into_iden(), vec![Expr::col("item").into()]),
				Alias::new("row"),
			)
			.from_as(Alias::new("state_locked_bindings"), Alias::new("item"));
		query.union_all(binding_rows);
		if !early_definitions.is_empty() {
			let ids: BTreeSet<_> = early_definitions
				.iter()
				.map(|(id, _)| id.as_str())
				.collect();
			let versions: BTreeSet<_> = early_definitions
				.iter()
				.map(|(_, version)| version.as_str())
				.collect();
			let mut locked = Query::select();
			locked
				.columns(["id", "version", "metadata"].map(Alias::new))
				.from(Alias::new("registry"))
				.and_where(Expr::col("tenant").eq(Expr::value(&self.identity.tenant)))
				.and_where(Expr::col("id").is_in(ids.into_iter().map(Expr::value)))
				.and_where(Expr::col("version").is_in(versions.into_iter().map(Expr::value)))
				.lock(reinhardt::query::LockType::Share);
			query.with_cte("state_locked_definitions", locked);
			let mut rows = Query::select();
			rows.expr_as(
				Expr::value("definition").cast_as("text"),
				Alias::new("kind"),
			)
			.expr_as(
				SimpleExpr::FunctionCall("to_jsonb".into_iden(), vec![Expr::col("item").into()]),
				Alias::new("row"),
			)
			.from_as(Alias::new("state_locked_definitions"), Alias::new("item"));
			query.union_all(rows);
		}
		let (sql, values) = query.build(PostgresQueryBuilder);
		let batch: Vec<StateBatchRow> = self.fetch_rows(&sql, values).await?;
		let (
			mut tasks,
			mut run_outputs,
			mut grant_outputs,
			mut early_rows,
			mut humans,
			mut sources,
			mut registry,
			mut remote,
			mut snapshots,
			mut received,
			mut core_runs,
			mut memory,
			mut bindings,
			mut semantic,
		) = (
			Vec::new(),
			Vec::new(),
			Vec::new(),
			Vec::new(),
			Vec::new(),
			Vec::new(),
			Vec::new(),
			Vec::new(),
			Vec::new(),
			Vec::new(),
			Vec::new(),
			Vec::new(),
			Vec::new(),
			Vec::new(),
		);
		for row in batch {
			match row {
				StateBatchRow::Task(row) => tasks.push(row),
				StateBatchRow::RunOutput(row) => run_outputs.push(row),
				StateBatchRow::GrantOutput(row) => grant_outputs.push(row),
				StateBatchRow::Definition(row) => early_rows.push(row),
				StateBatchRow::Human(row) => humans.push(row),
				StateBatchRow::Source(row) => sources.push(row),
				StateBatchRow::Registry(row) => registry.push(row),
				StateBatchRow::Remote(row) => remote.push(row),
				StateBatchRow::Snapshot(row) => snapshots.push(row),
				StateBatchRow::Received(row) => received.push(row),
				StateBatchRow::Core(row) => core_runs.push(row),
				StateBatchRow::Memory(row) => memory.push(row),
				StateBatchRow::Binding(row) => bindings.push(row),
				StateBatchRow::Semantic(row) => semantic.push(row),
			}
		}
		let mut newly_asked = BTreeSet::new();
		for task in &tasks {
			let key = (task.id, task.workspace_id);
			self.read_lookups.tasks.asked.insert(key);
			self.read_lookups.tasks.found.insert(key, task.clone());
			let output = (task.workspace_id, "task".to_owned(), task.id);
			if self.read_lookups.producers.asked.insert(output.clone()) {
				self.read_lookups.grants.asked.insert(output.clone());
				newly_asked.insert(output);
			}
		}
		run_outputs.sort_by_key(|row| row.run_id);
		for row in run_outputs {
			let key = (row.workspace_id, row.resource_kind, row.resource_id);
			if newly_asked.contains(&key) {
				self.read_lookups
					.producers
					.found
					.entry(key)
					.or_default()
					.push(row.run_id);
			}
		}
		grant_outputs.sort_by_key(|row| row.grant_id);
		for row in grant_outputs {
			let key = (row.workspace_id, row.resource_kind, row.resource_id);
			if newly_asked.contains(&key) {
				self.read_lookups
					.grants
					.found
					.entry(key)
					.or_default()
					.push(row.grant_id);
			}
		}
		sources.sort_by(|a, b| {
			(&a.resource_kind, a.resource_id).cmp(&(&b.resource_kind, b.resource_id))
		});
		registry
			.sort_by(|a, b| (&a.entry_id, &a.entry_version).cmp(&(&b.entry_id, &b.entry_version)));
		remote.sort_by(|a, b| {
			(&a.node_id, &a.entry_id, &a.entry_version, &a.digest).cmp(&(
				&b.node_id,
				&b.entry_id,
				&b.entry_version,
				&b.digest,
			))
		});
		semantic.sort_by_key(|row| (row.entry_id, row.revision));
		let area_ids: Vec<_> = core_runs.iter().map(|row| row.area_id).collect();
		let mut areas = BTreeMap::new();
		if !area_ids.is_empty() {
			let query = crate::apps::execution::repositories::sessions::select("core_areas")
				.and_where(Expr::col("id").is_in(area_ids.into_iter().map(Expr::value)))
				.and_where(Expr::col("tenant").eq(Expr::value(&self.identity.tenant)))
				.to_string(PostgresQueryBuilder);
			let rows: Vec<crate::apps::execution::capabilities::serializers::contracts::Area> =
				crate::database::native::query_as(&query)
					.fetch_all(&mut **self.tx)
					.await?;
			for area in rows {
				areas.insert(area.id, area);
			}
		}
		let mut wanted = early_definitions.clone();
		wanted.extend(
			registry
				.iter()
				.map(|row| (row.entry_id.clone(), row.entry_version.clone())),
		);
		for row in &snapshots {
			if let Some(snapshot) = row.context.get("binding_snapshot") {
				snapshot_definitions(snapshot, &self.node_id, &mut wanted);
			}
		}
		let missing: BTreeSet<_> = wanted.difference(&early_definitions).cloned().collect();
		let mut definitions = Loaded {
			asked: wanted,
			..Loaded::default()
		};
		for entry in early_rows {
			let key = (entry.id, entry.version);
			if early_definitions.contains(&key) {
				definitions.found.insert(key, entry.metadata);
			}
		}
		if !missing.is_empty() {
			let ids: BTreeSet<_> = missing.iter().map(|(id, _)| id.as_str()).collect();
			let versions: BTreeSet<_> = missing
				.iter()
				.map(|(_, version)| version.as_str())
				.collect();
			let (sql, values) = Query::select()
				.columns(["id", "version", "metadata"].map(Alias::new))
				.from(Alias::new("registry"))
				.and_where(Expr::col("tenant").eq(Expr::value(&self.identity.tenant)))
				.and_where(Expr::col("id").is_in(ids.into_iter().map(Expr::value)))
				.and_where(Expr::col("version").is_in(versions.into_iter().map(Expr::value)))
				.lock(reinhardt::query::LockType::Share)
				.build(PostgresQueryBuilder);
			let entries: Vec<StateDefinition> = self.fetch_rows(&sql, values).await?;
			for entry in entries {
				let key = (entry.id, entry.version);
				if missing.contains(&key) {
					definitions.found.insert(key, entry.metadata);
				}
			}
		}
		let mut rows = StateRows {
			authority: self.authority_context(),
			epoch: self.tx.transaction.as_ref().map_or(0, |tx| tx.lock_epoch()),
			tasks: BTreeMap::new(),
			source_tasks: Loaded::default(),
			source_artifacts: Loaded::default(),
			source_messages: Loaded::default(),
			source_conversations: Loaded::default(),
			source_attachments: Loaded::default(),
			outputs: Loaded::default(),
			remote_outputs: Loaded::default(),
			runs: runs
				.iter()
				.map(|run| ((run.id, run.workspace_id), run.clone()))
				.collect(),
			sources: runs.iter().map(|run| (run.id, Vec::new())).collect(),
			registry_reads: runs.iter().map(|run| (run.id, Vec::new())).collect(),
			definitions,
			catalog_restrictions: None,
			catalog_documents: None,
			installations: Loaded::default(),
			revisions: Loaded::default(),
			remote_reads: runs.iter().map(|run| (run.id, Vec::new())).collect(),
			binding_runs: bindings.into_iter().map(|row| row.run_id).collect(),
			memory_reads: memory.into_iter().map(|row| row.run_id).collect(),
			semantic_reads: runs.iter().map(|run| (run.id, Vec::new())).collect(),
			received: runs.iter().map(|run| (run.id, None)).collect(),
			core_areas: runs.iter().map(|run| (run.id, None)).collect(),
			snapshots: runs.iter().map(|run| (run.id, None)).collect(),
			humans: runs
				.iter()
				.map(|run| ((run.id, run.workspace_id), Vec::new()))
				.collect(),
		};
		for task in tasks {
			rows.tasks.insert((task.id, task.workspace_id), task);
		}
		for human in humans {
			if let Some(items) = rows.humans.get_mut(&(human.run_id, human.workspace_id)) {
				items.push(human);
			}
		}
		for source in sources {
			if let Some(items) = rows.sources.get_mut(&source.run_id) {
				items.push((
					source.workspace_id,
					source.resource_kind,
					source.resource_id,
				));
			}
		}
		for read in registry {
			if let Some(items) = rows.registry_reads.get_mut(&read.run_id) {
				items.push((read.entry_id, read.entry_version));
			}
		}
		for read in remote {
			if let Some(items) = rows.remote_reads.get_mut(&read.run_id) {
				items.push((
					read.node_id,
					read.entry_id,
					read.entry_version,
					read.digest,
					read.metadata,
					read.same_tenant,
				));
			}
		}
		for row in semantic {
			if let Some(items) = rows.semantic_reads.get_mut(&row.run_id) {
				items.push((row.entry_id, row.revision));
			}
		}
		for row in received {
			if let Some(value) = rows.received.get_mut(&row.id) {
				*value = Some((row.source_node, row.grant_id, row.node));
			}
		}
		for core in core_runs {
			if let Some(value) = rows.core_areas.get_mut(&core.run_id) {
				*value = Some(areas.get(&core.area_id).cloned());
			}
		}
		for snapshot in snapshots {
			if let Some(value) = rows.snapshots.get_mut(&snapshot.id) {
				*value = snapshot.context.get("binding_snapshot").cloned();
			}
		}
		let tasks: Vec<_> = rows.tasks.values().cloned().collect();
		let sources: Vec<_> = rows.sources.values().flatten().cloned().collect();
		self.read_lookups.state = Some(rows);
		self.prepare_state_tasks(&tasks).await?;
		self.prepare_read_sources(&sources).await?;
		let source_tasks = self.read_lookups.tasks.clone();
		let source_artifacts = self.read_lookups.artifacts.clone();
		let source_messages = self.read_lookups.messages.clone();
		let source_conversations = self.read_lookups.conversations.clone();
		let source_attachments = self.read_lookups.attachments.clone();
		let outputs = self.read_lookups.producers.clone();
		let remote_outputs = self.read_lookups.grants.clone();
		if let Some(rows) = self.read_lookups.state.as_mut() {
			rows.source_tasks = source_tasks;
			rows.source_artifacts = source_artifacts;
			rows.source_messages = source_messages;
			rows.source_conversations = source_conversations;
			rows.source_attachments = source_attachments;
			rows.outputs = outputs;
			rows.remote_outputs = remote_outputs;
		}
		Ok(())
	}

	/// Load what these recorded reads will ask for, in one statement per table.
	/// Each row still goes through the same policy checks as a single read.
	pub(crate) async fn prepare_walk_sources(&mut self, sources: &[RecordedRead]) -> Result<()> {
		// The state preload already includes each source and its outputs. A walk
		// must still authorize them, but it need not load the same rows again.
		if self.current_state_rows().is_some_and(|rows| {
			sources.iter().all(|(workspace, kind, id)| {
				let key = (*id, *workspace);
				match kind.as_str() {
					"task" => rows.source_tasks.get(&key).is_some(),
					"artifact" => rows.source_artifacts.get(&key).is_some(),
					"message" => rows.source_messages.get(&key).is_some(),
					"conversation" => rows.source_conversations.get(&key).is_some(),
					"attachment" => rows.source_attachments.get(&key).is_some(),
					_ => true,
				}
			})
		}) {
			return Ok(());
		}
		self.prepare_read_sources(sources).await
	}

	pub(crate) async fn prepare_read_sources(&mut self, sources: &[RecordedRead]) -> Result<()> {
		let of_kind = |kind: &str| -> Vec<Recorded> {
			sources
				.iter()
				.filter(|(_, source, _)| source == kind)
				.map(|(workspace, _, id)| (*id, *workspace))
				.collect()
		};
		let mut messages = of_kind("message");
		let attachments = of_kind("attachment");
		let wanted = unasked(&mut self.read_lookups.attachments, attachments);
		if !wanted.is_empty() {
			let (sql, values) = Query::select()
				.columns(["id", "workspace_id", "message_id"].map(Alias::new))
				.from(Alias::new("channel_attachments"))
				.and_where(Expr::col("id").is_in(wanted.into_iter().map(Expr::value)))
				.and_where(Expr::col("message_id").is_not_null())
				.build(PostgresQueryBuilder);
			let rows: Vec<AttachmentMessage> = self.fetch_rows(&sql, values).await?;
			for row in rows {
				messages.push((row.message_id, row.workspace_id));
				self.read_lookups
					.attachments
					.found
					.insert((row.id, row.workspace_id), row.message_id);
			}
		}
		let mut tasks = of_kind("task");
		let artifacts = of_kind("artifact");
		let mut outputs: Vec<Output> = artifacts
			.iter()
			.map(|(id, workspace)| (*workspace, "artifact".to_owned(), *id))
			.collect();
		let wanted = unasked(&mut self.read_lookups.artifacts, artifacts);
		if !wanted.is_empty() {
			for artifact in Artifact::read_many_in(&mut **self.tx, &wanted).await? {
				tasks.push((artifact.task_id, artifact.workspace_id));
				self.read_lookups
					.artifacts
					.found
					.insert((artifact.id, artifact.workspace_id), artifact);
			}
		}
		outputs.extend(
			tasks
				.iter()
				.map(|(id, workspace)| (*workspace, "task".to_owned(), *id)),
		);
		let wanted = unasked(&mut self.read_lookups.tasks, tasks);
		if !wanted.is_empty() {
			for task in Task::read_many_in(&mut **self.tx, &wanted).await? {
				self.read_lookups
					.tasks
					.found
					.insert((task.id, task.workspace_id), task);
			}
		}
		outputs.extend(
			messages
				.iter()
				.map(|(id, workspace)| (*workspace, "message".to_owned(), *id)),
		);
		let wanted = unasked(&mut self.read_lookups.messages, messages);
		if !wanted.is_empty() {
			for message in Message::read_many_in(&mut **self.tx, &wanted).await? {
				self.read_lookups
					.messages
					.found
					.insert((message.id, message.workspace_id), message);
			}
		}
		let wanted = unasked(
			&mut self.read_lookups.conversations,
			of_kind("conversation"),
		);
		if !wanted.is_empty() {
			for conversation in Conversation::read_many_in(&mut **self.tx, &wanted).await? {
				self.read_lookups
					.conversations
					.found
					.insert((conversation.id, conversation.workspace_id), conversation);
			}
		}
		self.prepare_outputs(outputs).await
	}

	/// Load which Runs and Remote Grants produced these outputs. Ascending order
	/// within each output matches the single-output readers.
	async fn prepare_outputs(&mut self, outputs: Vec<Output>) -> Result<()> {
		let wanted: Vec<Output> = outputs
			.into_iter()
			.filter(|key| self.read_lookups.producers.asked.insert(key.clone()))
			.collect();
		if wanted.is_empty() {
			return Ok(());
		}
		let workspaces: BTreeSet<Uuid> =
			wanted.iter().map(|(workspace, _, _)| *workspace).collect();
		let kinds: BTreeSet<&str> = wanted.iter().map(|(_, kind, _)| kind.as_str()).collect();
		let ids: BTreeSet<Uuid> = wanted.iter().map(|(_, _, id)| *id).collect();
		for key in &wanted {
			self.read_lookups.grants.asked.insert(key.clone());
		}
		if self.has_state_preload() {
			let selection = |table: &str, owner: &str, remote: bool| {
				let mut query = Query::select();
				query
					.columns(["workspace_id", "resource_kind", "resource_id"].map(Alias::new))
					.expr_as(Expr::col(Alias::new(owner)), Alias::new("owner"))
					.expr_as(Expr::value(remote).cast_as("bool"), Alias::new("remote"))
					.from(Alias::new(table))
					.and_where(
						Expr::col("workspace_id")
							.is_in(workspaces.iter().copied().map(Expr::value)),
					)
					.and_where(
						Expr::col("resource_kind").is_in(kinds.iter().copied().map(Expr::value)),
					)
					.and_where(
						Expr::col("resource_id").is_in(ids.iter().copied().map(Expr::value)),
					);
				query
			};
			let mut query = selection("authorization_run_outputs", "run_id", false);
			query.union_all(selection("authorization_remote_outputs", "grant_id", true));
			let (sql, values) = query.build(PostgresQueryBuilder);
			let mut rows: Vec<StateProducedOutput> = self.fetch_rows(&sql, values).await?;
			rows.sort_by_key(|row| row.owner);
			for row in rows {
				let loaded = if row.remote {
					&mut self.read_lookups.grants
				} else {
					&mut self.read_lookups.producers
				};
				loaded
					.found
					.entry((row.workspace_id, row.resource_kind, row.resource_id))
					.or_default()
					.push(row.owner);
			}
			return Ok(());
		}
		for (table, owner, into_grants) in [
			("authorization_run_outputs", "run_id", false),
			("authorization_remote_outputs", "grant_id", true),
		] {
			let (sql, values) = Query::select()
				.columns(["workspace_id", "resource_kind", "resource_id"].map(Alias::new))
				.expr_as(Expr::col(Alias::new(owner)), Alias::new("owner"))
				.from(Alias::new(table))
				.and_where(
					Expr::col("workspace_id").is_in(workspaces.iter().copied().map(Expr::value)),
				)
				.and_where(Expr::col("resource_kind").is_in(kinds.iter().copied().map(Expr::value)))
				.and_where(Expr::col("resource_id").is_in(ids.iter().copied().map(Expr::value)))
				.order_by(Alias::new(owner), Order::Asc)
				.build(PostgresQueryBuilder);
			let rows: Vec<ProducedOutput> = self.fetch_rows(&sql, values).await?;
			for row in rows {
				let lookups = &mut self.read_lookups;
				let loaded = if into_grants {
					&mut lookups.grants
				} else {
					&mut lookups.producers
				};
				loaded
					.found
					.entry((row.workspace_id, row.resource_kind, row.resource_id))
					.or_default()
					.push(row.owner);
			}
		}
		Ok(())
	}

	async fn fetch_rows<T: serde::de::DeserializeOwned>(
		&mut self,
		sql: &str,
		values: reinhardt::query::Values,
	) -> Result<Vec<T>> {
		self.tx
			.native_mut()
			.fetch_all(sql, convert_values(values))
			.await?
			.into_iter()
			.map(|row| {
				serde_json::from_value(reinhardt::db::orm::QueryRow::from_backend_row(row).data)
					.map_err(Into::into)
			})
			.collect()
	}
}
