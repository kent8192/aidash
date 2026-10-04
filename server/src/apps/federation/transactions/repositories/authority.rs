//! Native authority adapter retains the borrowed Access and its SHARE locks.
use crate::{Error, authorization::access::Access};
use aidash_application::{Result, ports::transactions::TransactionAuthorityScope};
use aidash_domain::{
	RunMetadata, Task, identity::execution::ExecutionPrincipal, policy::Resource,
	transactions::authority::SourceAdmission,
};
use async_trait::async_trait;
use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query, SimpleExpr};
use serde_json::Value;
use uuid::Uuid;
pub(crate) struct Scope<'a> {
	pub(crate) access: &'a mut Access,
}
#[async_trait]
impl TransactionAuthorityScope for Scope<'_> {
	fn identity(&self) -> ExecutionPrincipal {
		ExecutionPrincipal {
			tenant: self.access.identity.tenant.clone(),
			subject: self.access.identity.subject.clone(),
			credential_id: self.access.identity.credential_id,
		}
	}
	fn source_node(&self) -> Option<&str> {
		self.access.context["source_node"].as_str()
	}
	fn subjects(&self) -> &[String] {
		&self.access.subjects
	}
	fn set_subjects(&mut self, subjects: Vec<String>) {
		self.access.subjects = subjects;
	}
	fn resource(&self, kind: &str, id: Uuid, attributes: Value) -> Resource {
		self.access.resource(kind, id, attributes)
	}
	fn qualified_resource(&self, kind: &str, node: &str, id: Uuid) -> Resource {
		self.access.resource(
			kind,
			format!("{node}:{id}"),
			serde_json::json!({"node_id":node}),
		)
	}
	async fn require(&mut self, resource: &Resource, action: &str) -> Result<()> {
		self.access
			.require(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn inherit_task(&mut self, task: Uuid) -> Result<()> {
		crate::authorization::execution::inherit_task_origin(self.access, task)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn inherit_local_run(&mut self, run: &RunMetadata) -> Result<()> {
		aidash_application::authorization::execution::inherit_run(
			&mut crate::bootstrap::execution_grant_scope(self.access),
			run,
		)
		.await
	}
	async fn source_admission(
		&mut self,
		run: &RunMetadata,
		coordinator: &str,
	) -> Result<Option<SourceAdmission>> {
		let record: Option<(String, Uuid, Vec<String>, Value)> = {
			let query_bind_1 = run.id;
			let query_bind_2 = coordinator;
			let query_bind_3 = run.task_id;
			sqlx::query_as(
				&Query::select()
					.columns([
						Alias::new("tenant"),
						Alias::new("credential_id"),
						Alias::new("subject_chain"),
						Alias::new("description"),
					])
					.from(Alias::new("authorization_remote_admissions"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id=? AND source_node=? AND task_id=?)".to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
							Expr::value(query_bind_3.to_owned()).into(),
						],
					))
					.lock(reinhardt::query::LockType::Share)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut **self.access.tx)
			.await
			.map_err(Error::from)?
		};

		let Some((tenant, credential_id, subject_chain, description)) = record else {
			return Ok(None);
		};
		let description: crate::authorization::remote::Description =
			serde_json::from_value(description)?;
		Ok(Some(SourceAdmission {
			tenant,
			credential_id,
			subject_chain,
			source_tenant: description.source_tenant,
			source_subject: description.source_subject,
			workspace_id: description.task.workspace_id,
			agent_id: description.inspection.agent.id,
			agent_version: description.inspection.agent.version,
		}))
	}
	async fn run(&mut self, id: Uuid) -> Result<RunMetadata> {
		persistence::run(self.access, id)
			.await
			.map(|run| run.metadata())
			.map_err(Into::into)
	}
	async fn workspace(&mut self, id: Uuid) -> Result<Resource> {
		self.access.workspace(id).await.map_err(Into::into)
	}
	async fn task(&mut self, id: Uuid) -> Result<Task> {
		self.access.task_read(id).await.map_err(Into::into)
	}
	async fn task_resource(&mut self, task: &Task) -> Result<Resource> {
		self.access.task_resource(task).await.map_err(Into::into)
	}
	async fn artifact_creation_resource(&mut self, task: Uuid, owner: &str) -> Result<Resource> {
		self.access
			.artifact_creation_resource(task, owner)
			.await
			.map_err(Into::into)
	}
}

use reinhardt::query::QueryStatementBuilder as _;

pub(in crate::apps::federation::transactions) mod persistence;
