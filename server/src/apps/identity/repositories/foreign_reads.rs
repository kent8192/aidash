//! Shared row locks and source-authority restoration stay in the native adapter.
use crate::authorization::{access::Access, catalog, identity::SubjectIdentity};
use aidash_application::{
	Result,
	authorization::Snapshot,
	ports::federation::foreign_reads::{
		AdmissionEvidence, ForeignRunReadScope, ForeignSourceAuthority,
	},
};
use aidash_domain::{
	RunMetadata,
	federation::execution::Description,
	policy::Resource,
	registry::{EntityRef, Entry},
};
use async_trait::async_trait;
use reinhardt::query::{Alias, Expr, JoinType, LockType, PostgresQueryBuilder, Query, SimpleExpr};
use serde_json::Value;
use uuid::Uuid;

pub(crate) struct NativeForeignReads<'a>(pub(crate) &'a mut Access);

#[async_trait]
impl ForeignRunReadScope for NativeForeignReads<'_> {
	fn node(&self) -> &str {
		&self.0.node_id
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		self.0.resource(kind, id, attributes)
	}
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		self.0.decide(resource, action).await.map_err(Into::into)
	}
	async fn admission(&mut self, run: &RunMetadata) -> Result<Option<AdmissionEvidence>> {
		let record: Option<(Uuid, Vec<String>, serde_json::Value)> = {
			let query_bind_1 = run.id;
			let query_bind_2 = &self.0.identity.tenant;
			let query_bind_3 = &run.home_node;
			let query_bind_4 = run.task_id;
			crate::database::native::query_as(
				&Query::select()
					.columns(["credential_id", "subject_chain", "description"].map(Alias::new))
					.from(Alias::new("authorization_remote_admissions"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(id=? AND tenant=? AND source_node=? AND task_id=?)".to_owned(),
						vec![
							Expr::value(query_bind_1.to_owned()).into(),
							Expr::value(query_bind_2.to_owned()).into(),
							Expr::value(query_bind_3.to_owned()).into(),
							Expr::value(query_bind_4.to_owned()).into(),
						],
					))
					.lock(LockType::Share)
					.to_string(PostgresQueryBuilder),
			)
			.columns(&["credential_id", "subject_chain", "description"])
			.fetch_optional(&mut **self.0.tx)
			.await?
		};

		record
			.map(|(credential_id, subjects, value)| {
				Ok(AdmissionEvidence {
					credential_id,
					subjects,
					description: serde_json::from_value(value)?,
				})
			})
			.transpose()
	}
	async fn mapped_credential(
		&mut self,
		run: &RunMetadata,
		d: &Description,
	) -> Result<Option<Uuid>> {
		let mapped: Option<Uuid> = {
			let query_bind_1 = &run.home_node;
			let query_bind_2 = &d.source_tenant;
			let query_bind_3 = &d.source_subject;
			let query_bind_4 = &self.0.identity.tenant;
			crate::database::native::query_scalar(&Query::select()
				.column(Alias::new("credential_id"))
				.from(Alias::new("authorization_peer_mappings"))
				.and_where(SimpleExpr::CustomWithExpr("(source_node=? AND source_tenant=? AND source_subject=? AND tenant=? AND enabled)".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), Expr::value(query_bind_3.to_owned()).into(), Expr::value(query_bind_4.to_owned()).into()]))
				.lock(LockType::Share)
				.to_string(PostgresQueryBuilder))
		.scalar_optional(&mut **self.0.tx)
		.await?
		};

		Ok(mapped)
	}
	async fn source_snapshot(&mut self, credential_id: Uuid, root: &str) -> Result<Snapshot> {
		SubjectIdentity {
			http_session: None,
			credential_id,
			tenant: self.0.identity.tenant.clone(),
			subject: root.into(),
		}
		.lock_with_mode(&mut self.0.tx, false)
		.await
		.map_err(Into::into)
	}
	async fn retired_definition(
		&mut self,
		run: &RunMetadata,
		d: &Description,
		credential_id: Uuid,
		terminal_statuses: &[&str],
	) -> Result<Option<Entry>> {
		let value: Option<serde_json::Value> = {
			let query_bind_1 = &self.0.identity.tenant;
			let query_bind_2 = &run.home_node;
			let query_bind_3 = run.task_id;
			let query_bind_4 = d.grant_id;
			let query_bind_5 = run.id;
			let query_bind_6 = &run.agent_id;
			let query_bind_7 = &run.agent_version;
			let query_bind_8 = &d.inspection.generation;
			let query_bind_9 = credential_id;
			let query_bind_10 = terminal_statuses;
			crate::database::native::query_scalar(&Query::select()
					.column((Alias::new("g"), Alias::new("definition")))
					.from_as(Alias::new("generation_requests"), Alias::new("g")).join(JoinType::InnerJoin, reinhardt::query::TableRef::table_alias(Alias::new("authorization_catalog"), Alias::new("c")), Expr::cust("c.tenant=g.tenant AND c.entry_id=g.agent_id AND c.entry_version=g.agent_version"))
					.and_where(SimpleExpr::CustomWithExpr("(g.tenant=? AND g.home_node=? AND g.task_id=? AND g.grant_id=? AND g.admission_id=? AND g.agent_id=? AND g.agent_version=? AND g.foreign_intent=? AND g.credential_id=? AND g.prepared AND g.status=ANY(?) AND g.quota_released AND NOT c.enabled AND g.retired_catalog_revision=c.revision)".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), Expr::value(query_bind_3.to_owned()).into(), Expr::value(query_bind_4.to_owned()).into(), Expr::value(query_bind_5.to_owned()).into(), Expr::value(query_bind_6.to_owned()).into(), Expr::value(query_bind_7.to_owned()).into(), Expr::value(query_bind_8.to_owned()).into(), Expr::value(query_bind_9.to_owned()).into(), crate::database::text_array(query_bind_10.to_owned())]))
					.lock(LockType::Share).lock_tables([Alias::new("g"), Alias::new("c")])
					.to_string(PostgresQueryBuilder))
			.scalar_optional(&mut **self.0.tx)
			.await?
		};
		Ok(value.map(serde_json::from_value).transpose()?)
	}
	fn source_authority(
		&mut self,
		snapshot: Snapshot,
		subjects: Vec<String>,
	) -> Box<dyn ForeignSourceAuthority + '_> {
		let viewer_snapshot = std::mem::replace(&mut self.0.snapshot, snapshot);
		let viewer_subjects = std::mem::replace(&mut self.0.subjects, subjects);
		Box::new(NativeForeignAuthority {
			access: self.0,
			viewer_snapshot: Some(viewer_snapshot),
			viewer_subjects: Some(viewer_subjects),
		})
	}
}

struct NativeForeignAuthority<'a> {
	access: &'a mut Access,
	viewer_snapshot: Option<Snapshot>,
	viewer_subjects: Option<Vec<String>>,
}
impl Drop for NativeForeignAuthority<'_> {
	fn drop(&mut self) {
		if let Some(snapshot) = self.viewer_snapshot.take() {
			self.access.snapshot = snapshot;
		}
		if let Some(subjects) = self.viewer_subjects.take() {
			self.access.subjects = subjects;
		}
	}
}
#[async_trait]
impl ForeignSourceAuthority for NativeForeignAuthority<'_> {
	fn catalog_resource(&self, entry: &Entry) -> Resource {
		catalog::resource(self.access, entry)
	}
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		self.access
			.decide(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn catalog_entry(&mut self, reference: &EntityRef, action: &str) -> Result<Entry> {
		catalog::entry(self.access, reference, action)
			.await
			.map_err(Into::into)
	}
}

use reinhardt::query::QueryStatementBuilder;
