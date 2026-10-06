//! Publication persistence retains the caller's transaction and distribution lock.
use super::{
	definitions::NativeDefinitions,
	storage::{get, put},
};
use crate::{authorization::access::Access, store::Store};
use aidash_application::{
	Result,
	ports::marketplace::{DistributionWriter, PublicationScope},
};
use aidash_domain::{
	marketplace::definitions::key,
	marketplace::{Audience, ConsentEdge, Replay, Version},
	registry::Entry,
};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use uuid::Uuid;

fn target(package: &str, redistributor: Option<&str>) -> (&'static str, String) {
	match redistributor {
		Some(tenant) => ("marketplace_consents", key(&(package, tenant))),
		None => ("marketplace_audiences", package.to_owned()),
	}
}
#[async_trait]
impl DistributionWriter for NativeDefinitions<'_, &Store> {
	async fn distribution(
		&mut self,
		package: &str,
		redistributor: Option<&str>,
	) -> Result<Option<Audience>> {
		let (table, key) = target(package, redistributor);
		get(&mut self.access.tx, table, &key)
			.await
			.map_err(Into::into)
	}
	async fn save_distribution(
		&mut self,
		package: &str,
		redistributor: Option<&str>,
		audience: &Audience,
	) -> Result<()> {
		let (table, key) = target(package, redistributor);
		put(&mut self.access.tx, table, &key, audience).await?;
		crate::marketplace::storage::authority(
			self.access,
			format!("{table}:{key}"),
			audience.revision,
		);
		Ok(())
	}
	async fn event(&mut self, kind: &str, payload: Value) -> Result<()> {
		self.events
			.event(&mut self.access.tx, None, kind, payload)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
}
#[async_trait]
impl PublicationScope for NativeDefinitions<'_, &Store> {
	fn actor(&self) -> &str {
		&self.access.identity.subject
	}
	fn audit_resource(&mut self, resource: Value) {
		if let Some(audit) = &mut self.access.marketplace_audit {
			audit["resource"] = resource;
		}
	}
	async fn provenance(&mut self, entry: &Entry) -> Result<BTreeSet<ConsentEdge>> {
		crate::marketplace::installations::provenance(
			&mut self.access.tx,
			entry,
			&self.access.identity.tenant,
		)
		.await
		.map_err(Into::into)
	}
	async fn publication_replay(&mut self, request: Uuid) -> Result<Option<Replay>> {
		let id = key(&(
			self.access.identity.tenant.as_str(),
			self.access.identity.subject.as_str(),
			"publish",
			request,
		));
		get(&mut self.access.tx, "marketplace_requests", &id)
			.await
			.map_err(Into::into)
	}
	async fn remember_publication(
		&mut self,
		request: Uuid,
		fingerprint: String,
		result: Value,
	) -> Result<()> {
		let id = key(&(
			self.access.identity.tenant.as_str(),
			self.access.identity.subject.as_str(),
			"publish",
			request,
		));
		put(
			&mut self.access.tx,
			"marketplace_requests",
			&id,
			&Replay {
				fingerprint,
				result,
			},
		)
		.await
		.map_err(Into::into)
	}
	async fn package_kind(&mut self, repository: &str, package: &str) -> Result<Option<String>> {
		publication_kind(self.access, repository, package)
			.await
			.map_err(Into::into)
	}
	async fn insert_version(&mut self, version: &Version, audience: &Audience) -> Result<()> {
		insert_in(&mut self.access.tx, version, audience)
			.await
			.map_err(Into::into)
	}
}
async fn publication_kind(
	access: &mut Access,
	repository: &str,
	package: &str,
) -> crate::Result<Option<String>> {
	crate::database::native::query_scalar(
		&Query::select()
			.column(Alias::new("kind"))
			.from(Alias::new("marketplace_versions"))
			.and_where(Expr::col(Alias::new("repository")).eq(Expr::value(repository)))
			.and_where(Expr::col(Alias::new("owner")).eq(Expr::value(&access.identity.tenant)))
			.and_where(Expr::col(Alias::new("package_id")).eq(Expr::value(package)))
			.limit(1)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(&mut **access.tx)
	.await
}
pub(crate) async fn insert_in(
	tx: &mut crate::database::native::Transaction,
	version: &Version,
	audience: &Audience,
) -> crate::Result<()> {
	let source_content = aidash_domain::marketplace::definitions::content(
		&aidash_domain::marketplace::definitions::manifest(version)?.entity,
	);
	crate::database::native::query(
		&Query::insert()
			.into_table(Alias::new("marketplace_versions"))
			.columns(
				[
					"key",
					"document",
					"repository",
					"owner",
					"package_id",
					"version",
					"kind",
					"source_id",
					"source_version",
					"source_content",
				]
				.map(Alias::new),
			)
			.from_subquery({
				let mut values = Query::select();
				for i in 1..=10 {
					values.expr(Expr::cust(format!("${i}")));
				}
				values.to_owned()
			})
			.to_string(PostgresQueryBuilder),
	)
	.bind(&version.key)
	.bind(json!(version))
	.bind(&version.repository)
	.bind(&version.owner_tenant)
	.bind(&version.package_id)
	.bind(&version.version)
	.bind(&version.kind)
	.bind(&version.source.id)
	.bind(&version.source.version)
	.bind(source_content)
	.execute(&mut **tx)
	.await?;
	put(tx, "marketplace_audiences", &version.key, audience).await
}
