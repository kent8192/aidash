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
	Alias, Expr, ExprTrait as _, Order, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

type Recorded = (Uuid, Uuid);
type Output = (Uuid, String, Uuid);

/// `None` from `get` means nobody asked for this key: the caller reads it itself.
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
	passes: Passes,
	/// Evaluations already allowed under this authority (`Access::repeat_key`).
	pub(super) allowed: BTreeSet<String>,
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
	/// A silent peer invalidates successful walks before the next list row.
	pub(super) fn forget_passes(&mut self) {
		if !self.walking {
			self.passes = Passes::default();
		}
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

/// Which of `wanted` nobody has asked for yet, marking them as asked.
fn unasked<V>(loaded: &mut Loaded<Recorded, V>, wanted: Vec<Recorded>) -> Vec<Uuid> {
	wanted
		.into_iter()
		.filter(|key| loaded.asked.insert(*key))
		.map(|(id, _)| id)
		.collect()
}

impl Access {
	/// Load what these recorded reads will ask for, in one statement per table.
	/// Each row still goes through the same policy checks as a single read.
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
