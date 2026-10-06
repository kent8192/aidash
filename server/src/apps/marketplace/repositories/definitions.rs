//! Definition ports retain the same credential/policy/distribution lease.
use crate::{
	authorization::{access::Access, catalog},
	registry::{EntityRef, Entry},
};
use aidash_application::{Result, ports::marketplace::DefinitionScope};
use aidash_domain::marketplace::{Installation, Revision, Version};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder, SimpleExpr,
};
use serde_json::Value;

use uuid::Uuid;

pub(crate) struct NativeDefinitions<'a, Events = ()> {
	pub(crate) access: &'a mut Access,
	pub(crate) events: Events,
}
pub(crate) async fn raw(
	tx: &mut crate::database::native::Transaction,
	r: &EntityRef,
) -> crate::Result<Entry> {
	let value: Option<Value> = {
		let query_bind_1 = &r.id;
		let query_bind_2 = &r.version;
		crate::database::native::query_scalar(
			&Query::select()
				.column(Alias::new("metadata"))
				.from(Alias::new("registry"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(id=? AND version=?)".to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
					],
				))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_optional(&mut **tx)
		.await?
	};
	serde_json::from_value(value.ok_or(crate::Error::Forbidden)?).map_err(Into::into)
}
#[async_trait]
impl<Events: Send> DefinitionScope for NativeDefinitions<'_, Events> {
	fn tenant(&self) -> &str {
		&self.access.identity.tenant
	}
	async fn raw(&mut self, reference: &EntityRef) -> Result<Entry> {
		raw(&mut self.access.tx, reference)
			.await
			.map_err(Into::into)
	}
	async fn installation(&mut self, id: &str) -> Result<Option<Installation>> {
		super::storage::get(&mut self.access.tx, "marketplace_installations", id)
			.await
			.map_err(Into::into)
	}
	async fn revision(&mut self, id: &str, revision: i64) -> Result<Revision> {
		crate::marketplace::installations::revision(&mut self.access.tx, id, revision)
			.await
			.map_err(Into::into)
	}
	async fn require_installation_read(
		&mut self,
		installation: &Installation,
		revision: i64,
	) -> Result<()> {
		let resource =
			crate::marketplace::installations::resource(self.access, installation, Some(revision));
		self.access
			.require(&resource, "installation.read")
			.await
			.map_err(Into::into)
	}
	async fn catalog(&mut self, reference: &EntityRef, action: &str) -> Result<Entry> {
		catalog::entry(self.access, reference, action)
			.await
			.map_err(Into::into)
	}
	async fn require_export(&mut self, entry: &Entry) -> Result<()> {
		self.access
			.require(&catalog::resource(self.access, entry), "registry.export")
			.await
			.map_err(Into::into)
	}
	async fn matching_publication(
		&mut self,
		reference: &EntityRef,
		source_content: &str,
	) -> Result<Option<Version>> {
		let version: Option<Value> = crate::database::native::query_scalar(
			&Query::select()
				.column(Alias::new("document"))
				.from(Alias::new("marketplace_versions"))
				.and_where(
					Expr::col(Alias::new("owner")).eq(Expr::value(&self.access.identity.tenant)),
				)
				.and_where(Expr::col(Alias::new("source_id")).eq(Expr::value(&reference.id)))
				.and_where(
					Expr::col(Alias::new("source_version")).eq(Expr::value(&reference.version)),
				)
				.and_where(Expr::col(Alias::new("source_content")).eq(Expr::value(source_content)))
				.limit(1)
				.to_string(PostgresQueryBuilder),
		)
		.scalar_optional(&mut **self.access.tx)
		.await?;
		let version = version.map(serde_json::from_value::<Version>).transpose()?;
		Ok(version)
	}
	async fn require_reference_read(&mut self, id: Uuid) -> Result<()> {
		crate::capabilities::references::get(self.access, id, "reference.read")
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
}

#[async_trait]
impl<Events: Send> aidash_application::ports::marketplace::DistributionScope
	for NativeDefinitions<'_, Events>
{
	async fn version(&mut self, key: &str) -> Result<Option<Version>> {
		super::storage::get(&mut self.access.tx, "marketplace_versions", key)
			.await
			.map_err(Into::into)
	}
	async fn audience(
		&mut self,
		key: &str,
	) -> Result<Option<aidash_domain::marketplace::Audience>> {
		super::storage::get(&mut self.access.tx, "marketplace_audiences", key)
			.await
			.map_err(Into::into)
	}
	async fn consent(&mut self, key: &str) -> Result<Option<aidash_domain::marketplace::Audience>> {
		super::storage::get(&mut self.access.tx, "marketplace_consents", key)
			.await
			.map_err(Into::into)
	}
	fn authority(&mut self, name: String, revision: i64) {
		crate::marketplace::storage::authority(self.access, name, revision)
	}
	fn operation(&mut self, name: &str, resource: Value, request: Option<Uuid>) {
		crate::marketplace::storage::operation(self.access, name, resource, request)
	}
	fn package_resource(&self, version: &Version) -> aidash_domain::policy::Resource {
		self.access.resource("package",&version.key,serde_json::json!({"repository_node":version.repository,"owner_tenant":version.owner_tenant,"package_id":version.package_id,"version":version.version,"kind":version.kind,"source_digest":version.digest,"publisher":version.publisher}))
	}
	fn installation_resource(
		&self,
		installation: &Installation,
		revision: Option<i64>,
	) -> aidash_domain::policy::Resource {
		crate::marketplace::installations::resource(self.access, installation, revision)
	}
	async fn require(
		&mut self,
		resource: &aidash_domain::policy::Resource,
		action: &str,
	) -> Result<()> {
		self.access
			.require(resource, action)
			.await
			.map_err(Into::into)
	}
	async fn decide(
		&mut self,
		resource: &aidash_domain::policy::Resource,
		action: &str,
	) -> Result<bool> {
		self.access
			.decide(resource, action)
			.await
			.map_err(Into::into)
	}
}
