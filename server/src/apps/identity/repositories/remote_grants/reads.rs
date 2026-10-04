//! Reader statements and producer restoration retain the same native authority transaction.
use crate::{
	Error as NativeError, Result as NativeResult,
	authorization::{access::Access, identity::SubjectIdentity},
};
use aidash_application::{
	Result,
	authorization::{Snapshot, visits::ReadVisit},
	ports::authorization::source::{
		SourceAuthorityScope, SourcePeerScope,
		reads::{ProducerScope, SourceReadScope},
	},
};
use aidash_domain::{
	Task,
	federation::{
		Peer,
		dependencies::Reference,
		execution::home::{Grant, HomeBinding},
	},
	policy::{PolicyBundle, Resource},
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, ColumnRef::Asterisk, Expr, LockType, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, SimpleExpr,
};
use serde_json::Value;
use std::collections::BTreeMap;
use uuid::Uuid;
pub(crate) struct Scope<'a> {
	pub(crate) access: &'a mut Access,
	pub(crate) owned_frontier: bool,
}
impl Drop for Scope<'_> {
	fn drop(&mut self) {
		if self.owned_frontier {
			self.access.dependency_frontier = None;
		}
	}
}
struct Viewer {
	snapshot: Snapshot,
	identity: SubjectIdentity,
	subjects: Vec<String>,
	context: Value,
	cached_runs: BTreeMap<(Uuid, Uuid), bool>,
	cached_humans: BTreeMap<Uuid, bool>,
}
struct Producer<'a> {
	access: &'a mut Access,
	viewer: Option<Viewer>,
}
impl Drop for Producer<'_> {
	fn drop(&mut self) {
		if let Some(viewer) = self.viewer.take() {
			self.access.snapshot = viewer.snapshot;
			self.access.identity = viewer.identity;
			self.access.subjects = viewer.subjects;
			self.access.context = viewer.context;
			self.access.cached_runs = viewer.cached_runs;
			self.access.cached_humans = viewer.cached_humans;
		}
	}
}
#[async_trait]
impl SourceAuthorityScope for Producer<'_> {
	fn source_subjects(&self) -> &[String] {
		&self.access.subjects
	}
	fn source_bundle(&self) -> &PolicyBundle {
		&self.access.snapshot.bundle
	}
	fn source_context(&mut self, attributes: Value) {
		self.access.context = attributes;
	}
	fn source_resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		self.access.resource(kind, id, attributes)
	}
	async fn generation_home(
		&mut self,
		task: &Task,
		node: &str,
		generation: Option<&Value>,
	) -> Result<()> {
		crate::generation::foreign::check_home(self.access, task, node, generation)
			.await
			.map_err(Into::into)
	}
	async fn source_workspace(&mut self, id: Uuid) -> Result<Resource> {
		self.access.workspace(id).await.map_err(Into::into)
	}
	async fn source_task_resource(&mut self, task: &Task) -> Result<Resource> {
		self.access.task_resource(task).await.map_err(Into::into)
	}
	async fn source_require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.access
			.require(resource, action)
			.await
			.map_err(Into::into)
	}
}
#[async_trait]
impl ProducerScope for Producer<'_> {
	async fn producer_semantic_sources(&mut self, id: Uuid) -> Result<()> {
		self.access
			.remote_semantic_sources(id)
			.await
			.map_err(Into::into)
	}
}
#[async_trait]
impl SourcePeerScope for Scope<'_> {
	async fn peer(&mut self, node: &str) -> Result<Option<Peer>> {
		super::persistence::peer(self.access, node)
			.await
			.map_err(Into::into)
	}
}
#[async_trait]
impl SourceReadScope for Scope<'_> {
	fn output_visit(&self, id: Uuid) -> Option<ReadVisit> {
		self.access.authority_read_visit("grant", id)
	}
	fn protocol_version(&self) -> &str {
		crate::config::PROTOCOL_VERSION
	}
	async fn read_binding(&mut self, id: Uuid) -> Result<Option<HomeBinding>> {
		crate::apps::identity::repositories::home_execution::binding(self.access, id)
			.await
			.map(|r| r.map(Into::into))
			.map_err(Into::into)
	}
	async fn read_task(&mut self, id: Uuid) -> Result<Task> {
		self.access.task_read(id).await.map_err(Into::into)
	}
	async fn grant_reads(&mut self, id: Uuid) -> Result<bool> {
		self.access
			.grant_reads_visible(id)
			.await
			.map_err(Into::into)
	}
	async fn producer<'a>(&'a mut self, grant: &Grant) -> Result<Box<dyn ProducerScope + 'a>> {
		let original = SubjectIdentity {
			http_session: None,
			credential_id: grant.credential_id,
			tenant: grant.tenant.clone(),
			subject: grant.root_subject.clone(),
		};
		let snapshot = original.lock_with_mode(&mut self.access.tx, false).await?;
		let viewer = Viewer {
			snapshot: std::mem::replace(&mut self.access.snapshot, snapshot),
			identity: std::mem::replace(&mut self.access.identity, original),
			subjects: std::mem::replace(&mut self.access.subjects, grant.subject_chain.clone()),
			context: self.access.context.clone(),
			cached_runs: std::mem::take(&mut self.access.cached_runs),
			cached_humans: std::mem::take(&mut self.access.cached_humans),
		};
		Ok(Box::new(Producer {
			access: &mut *self.access,
			viewer: Some(viewer),
		}))
	}
	fn record_admission(&mut self, node: &str, grant: Uuid, admission: Uuid) -> Result<()> {
		self.access
			.dependency_frontier
			.as_mut()
			.ok_or(NativeError::Forbidden)?
			.push(Reference::Admission {
				node_id: node.into(),
				home_node: self.access.node_id.clone(),
				grant_id: grant,
				admission_id: admission,
			});
		Ok(())
	}
	fn collecting_dependencies(&self) -> bool {
		self.access.dependency_frontier.is_some()
	}
	fn start_dependencies(&mut self) {
		self.access.dependency_frontier = Some(vec![]);
		self.owned_frontier = true;
	}
	fn take_dependencies(&mut self) -> Vec<Reference> {
		self.owned_frontier = false;
		self.access.dependency_frontier.take().unwrap_or_default()
	}
	async fn verify_dependencies(&mut self, pending: Vec<Reference>) -> Result<bool> {
		self.access
			.verify_dependencies(pending)
			.await
			.map_err(Into::into)
	}
	async fn reader_grant(&mut self, execution_node: &str, id: Uuid) -> Result<Option<Grant>> {
		let result: NativeResult<Option<super::Grant>> = async {
			let access = &mut *self.access;
			let query_bind_1 = id;
			let query_bind_2 = execution_node;
			let query_bind_3 = &access.identity.tenant;
			Ok(sqlx::query_as(
				&Query::select()
					.column(Asterisk)
					.from(Alias::new("authorization_remote_grants"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id=? AND node_id=? AND tenant=? AND NOT revoked AND expires_at>CLOCK_TIMESTAMP())"
							.to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
							Expr::value(query_bind_3.to_owned()).into(),
						],
					))
					.lock(LockType::Share)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut **access.tx)
			.await?)
		}
		.await;
		result.map(|r| r.map(Into::into)).map_err(Into::into)
	}
	async fn output_record(&mut self, id: Uuid) -> Result<Option<(String, Value)>> {
		let result: NativeResult<Option<(String, Value)>> = async {
			Ok({
				let query_bind_1 = id;
				sqlx::query_as(
					&Query::select()
						.columns(["node_id", "semantic"].map(Alias::new))
						.from(Alias::new("authorization_remote_grants"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(id=?)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **self.access.tx)
				.await?
			})
		}
		.await;
		result.map_err(Into::into)
	}
}
