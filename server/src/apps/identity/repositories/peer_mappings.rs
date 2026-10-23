//! Mapping statements retain policy-before-credential locks, revision fences and control-pool revocation.
use crate::apps::identity::models::TenantAdministrator;
use crate::apps::identity::repositories::node_registrations as registrations;
use crate::apps::identity::serializers::peer::PeerMapping;
use crate::{
	Result as NativeResult,
	authorization::{Authorization, access::Access, identity::SubjectIdentity},
	federation::Federation,
};
use aidash_application::tenancy::TenancyScope;
use aidash_application::{
	Result,
	ports::authorization::peer::mappings::{MappingAccess, MappingRepository, MappingWrite},
};
use aidash_domain::identity::peer_mapping::{Mapping, PeerMappingInput};
use aidash_domain::nodes::{DeploymentId, NodeAddress, NodeName};
use aidash_domain::tenancy::TenantName;
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Expr, LockType, OnConflict, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
	SimpleExpr,
};
use serde_json::{Value, json};
use std::sync::Arc;
use uuid::Uuid;
pub(crate) struct Repository<'a> {
	pub runtime: &'a Federation,
	/// The Tenant Administrator writing through its Mapping; none for a
	/// Deployment Administrator.
	pub administrator: Option<Arc<TenantAdministrator>>,
}
pub(crate) struct WriteScope {
	tx: crate::database::native::Transaction,
	_visibility: Option<crate::transactions::gate::ReadLease>,
	administrator: Option<Arc<TenantAdministrator>>,
	actor: String,
}
pub(crate) struct AccessScope {
	pub access: Box<Access>,
}
#[async_trait]
impl MappingRepository for Repository<'_> {
	type Write = WriteScope;
	type Access = AccessScope;
	fn deployment(&self) -> &DeploymentId {
		&self.runtime.config.deployment
	}
	async fn begin_write(&self, scope: &TenancyScope, enabled: bool) -> Result<WriteScope> {
		let result: NativeResult<WriteScope> = async {
			let visibility = if enabled {
				Some(crate::transactions::gate::ReadLease::begin(&self.runtime.store).await?)
			} else {
				None
			};
			let mut tx = if enabled {
				self.runtime.store.pool.begin(scope).await?
			} else {
				self.runtime.store.control_pool.begin(scope).await?
			};
			if !enabled {
				crate::transactions::authority::control(&mut tx).await?;
			}
			Ok(WriteScope {
				tx,
				_visibility: visibility,
				actor: self
					.administrator
					.as_ref()
					.map_or("deployment_administrator".to_owned(), |administrator| {
						format!("oidc:{}", administrator.identity_id)
					}),
				administrator: self.administrator.clone(),
			})
		}
		.await;
		result.map_err(Into::into)
	}
	async fn resolved(
		&self,
		tenant: &TenantName,
		node: &NodeName,
		source: &NodeAddress,
		subject: &str,
	) -> Result<Option<Mapping>> {
		let result: NativeResult<Option<PeerMapping>> = async {
			crate::database::native::query_as(
				&Query::select()
					.expr(SimpleExpr::from(Expr::col(reinhardt::query::ColumnRef::Asterisk)))
					.from(Alias::new("authorization_peer_mappings"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(tenant = ? AND node = ? AND source_node = ? AND source_subject = ? AND enabled)"
							.to_owned(),
						vec![
							Expr::value(tenant.as_str().to_owned()).into(),
							Expr::value(node.as_str().to_owned()).into(),
							Expr::value(source.to_string()).into(),
							Expr::value(subject.to_owned()).into(),
						],
					))
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(
				self.runtime
					.store
					.pool
					.in_scope(&TenancyScope::Tenant(tenant.clone())),
			)
			.await
		}
		.await;
		result
			.and_then(|row| row.map(Mapping::try_from).transpose())
			.map_err(Into::into)
	}
	async fn credential_subject(&self, mapping: &Mapping) -> Result<Option<String>> {
		let f = self.runtime;
		let result: NativeResult<Option<String>> = async {
			Ok({
				let query_bind_1 = mapping.credential_id;
				let query_bind_2 = &mapping.tenant;
				crate::database::native::query_scalar(
					&reinhardt::query::Query::select()
						.expr(reinhardt::query::SimpleExpr::from(
							reinhardt::query::Expr::col(reinhardt::query::Alias::new("subject")),
						))
						.from(reinhardt::query::Alias::new("authorization_credentials"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(id = ? AND tenant = ?)".to_owned(),
							vec![
								Expr::value(query_bind_1.to_owned()).into(),
								Expr::value(query_bind_2.to_owned()).into(),
							],
						))
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.scalar_optional(
					f.store
						.pool
						.in_scope(&TenancyScope::tenant(&mapping.tenant)?),
				)
				.await?
			})
		}
		.await;
		result.map_err(Into::into)
	}
	async fn begin_access(
		&self,
		mapping: &Mapping,
		subject: &str,
		exclusive: bool,
	) -> Result<AccessScope> {
		let identity = SubjectIdentity {
			http_session: None,
			credential_id: mapping.credential_id,
			tenant: mapping.tenant.clone(),
			subject: subject.into(),
		};
		let access = if exclusive {
			Access::begin_exclusive(&self.runtime.store, &identity).await
		} else {
			Access::begin(&self.runtime.store, &identity).await
		};
		access
			.map(|mut access| {
				access.set_node(mapping.node.clone());
				AccessScope {
					access: Box::new(access),
				}
			})
			.map_err(Into::into)
	}
	async fn begin_subject(
		&self,
		tenant: &TenantName,
		subject: &str,
		exclusive: bool,
	) -> Result<AccessScope> {
		Access::begin_subject(&self.runtime.store, tenant.as_str(), subject, exclusive)
			.await
			.map(|access| AccessScope {
				access: Box::new(access),
			})
			.map_err(Into::into)
	}
}
#[async_trait]
impl MappingWrite for WriteScope {
	async fn policy(&mut self, tenant: &str) -> Result<()> {
		Authorization::load(&mut self.tx, tenant)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn credential_subject(&mut self, tenant: &str, id: Uuid) -> Result<Option<String>> {
		let result: NativeResult<Option<String>> = async {
			Ok({
				let query_bind_1 = tenant;
				let query_bind_2 = id;
				crate::database::native::query_scalar(
					&Query::select()
						.expr(SimpleExpr::from(Expr::col(Alias::new("subject"))))
						.from(Alias::new("authorization_credentials"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(tenant = ? AND id = ?)".to_owned(),
							vec![
								Expr::value(query_bind_1.to_owned()).into(),
								Expr::value(query_bind_2.to_owned()).into(),
							],
						))
						.lock(LockType::Share)
						.to_string(PostgresQueryBuilder),
				)
				.scalar_optional(&mut *self.tx)
				.await?
			})
		}
		.await;
		result.map_err(Into::into)
	}
	/// A Tenant Administrator sets a Peer Mapping under `peer_mapping.update`,
	/// and enables one only to a credential it could issue itself now.
	async fn authorize(
		&mut self,
		tenant: &str,
		input: &PeerMappingInput,
		subject: &str,
	) -> Result<()> {
		let Some(administrator) = self.administrator.clone() else {
			return Ok(());
		};
		let result: NativeResult<()> = async {
			let snapshot = administrator.lock(&mut self.tx, false).await?;
			administrator
				.require(
					&mut self.tx,
					&snapshot,
					aidash_domain::identity::tenant_administration::PEER_MAPPING_UPDATE,
					&format!("{}/{}", input.source_node, input.source_subject),
					json!({"node": input.node.as_str(), "source_node": input.source_node.to_string(), "subject": subject, "tenant": tenant}),
				)
				.await?;
			if input.enabled {
				administrator
					.require_issuable(&mut self.tx, &snapshot, subject)
					.await?;
			}
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	/// A mapped peer acts with an exportable credential only: never a
	/// Mapping's own, and never one a member issued for itself, which would
	/// let someone outside the Deployment act as that person.
	async fn enabled_registration(
		&mut self,
		tenant: &str,
		node: &NodeName,
		source: &NodeAddress,
	) -> Result<()> {
		registrations::get_locked(&mut self.tx, tenant, node.as_str(), source, true)
			.await?
			.filter(|registration| registration.enabled)
			.map(|_| ())
			.ok_or(aidash_application::Error::Unauthorized)
	}
	async fn exportable(&mut self, tenant: &str, id: Uuid) -> Result<()> {
		let exportable: Option<i32> = crate::database::native::query_scalar(
			&Query::select()
				.expr(Expr::cust("1"))
				.from(Alias::new("authorization_credentials"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(tenant = ? AND id = ? AND issued_by <> 'dashboard-oidc' AND mapping_id IS NULL)"
						.to_owned(),
					vec![
						Expr::value(tenant.to_owned()).into(),
						Expr::value(id).into(),
					],
				))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_optional(&mut *self.tx)
		.await
		.map_err(aidash_application::Error::from)?;
		if exportable.is_none() {
			return Err(aidash_application::Error::Forbidden);
		}
		Ok(())
	}
	async fn lock_credential(&mut self, tenant: &str, id: Uuid, subject: &str) -> Result<()> {
		let identity = SubjectIdentity {
			http_session: None,
			credential_id: id,
			tenant: tenant.into(),
			subject: subject.into(),
		};
		identity
			.lock_with_mode(&mut self.tx, false)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn insert(&mut self, tenant: &str, input: &PeerMappingInput) -> Result<Option<Mapping>> {
		let result: NativeResult<Option<PeerMapping>> = async {
			crate::database::native::query_as(
				&Query::insert()
					.into_table(Alias::new("authorization_peer_mappings"))
					.columns(
						[
							"tenant",
							"node",
							"source_node",
							"source_subject",
							"credential_id",
							"enabled",
							"revision",
							"actor",
						]
						.map(Alias::new),
					)
					.values_panic([
						Expr::value(tenant.to_owned()),
						Expr::value(input.node.as_str().to_owned()),
						Expr::value(input.source_node.to_string()),
						Expr::value(input.source_subject.clone()),
						Expr::value(input.credential_id),
						Expr::value(input.enabled),
						Expr::value(1_i64),
						Expr::value(self.actor.clone()),
					])
					.on_conflict(
						OnConflict::columns(
							["tenant", "node", "source_node", "source_subject"].map(Alias::new),
						)
						.do_nothing()
						.to_owned(),
					)
					.returning_all()
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut *self.tx)
			.await
		}
		.await;
		result
			.and_then(|row| row.map(Mapping::try_from).transpose())
			.map_err(Into::into)
	}
	async fn update(&mut self, tenant: &str, input: &PeerMappingInput) -> Result<Option<Mapping>> {
		let result: NativeResult<Option<PeerMapping>> = async {
			crate::database::native::query_as(
				&Query::update()
					.table(Alias::new("authorization_peer_mappings"))
					.value_expr(Alias::new("credential_id"), Expr::value(input.credential_id))
					.value_expr(Alias::new("enabled"), Expr::value(input.enabled))
					.value_expr(Alias::new("revision"), Expr::cust("revision + 1"))
					.value_expr(Alias::new("actor"), Expr::value(self.actor.clone()))
					.value_expr(Alias::new("updated_at"), Expr::cust("CLOCK_TIMESTAMP()"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(tenant = ? AND node = ? AND source_node = ? AND source_subject = ? AND revision = ?)"
							.to_owned(),
						vec![
							Expr::value(tenant.to_owned()).into(),
							Expr::value(input.node.as_str().to_owned()).into(),
							Expr::value(input.source_node.to_string()).into(),
							Expr::value(input.source_subject.clone()).into(),
							Expr::value(input.expected_revision).into(),
						],
					))
					.returning_all()
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut *self.tx)
			.await
		}
		.await;
		result
			.and_then(|row| row.map(Mapping::try_from).transpose())
			.map_err(Into::into)
	}
	async fn history(&mut self, mapping: &Mapping) -> Result<()> {
		let result: NativeResult<()> = async {
			crate::database::native::query(
				&Query::insert()
					.into_table(Alias::new("authorization_peer_mapping_history"))
					.columns(
						[
							"tenant",
							"node",
							"source_node",
							"source_subject",
							"credential_id",
							"enabled",
							"revision",
							"actor",
						]
						.map(Alias::new),
					)
					.values_panic([
						Expr::value(mapping.tenant.clone()),
						Expr::value(mapping.node.as_str().to_owned()),
						Expr::value(mapping.source_node.to_string()),
						Expr::value(mapping.source_subject.clone()),
						Expr::value(mapping.credential_id),
						Expr::value(mapping.enabled),
						Expr::value(mapping.revision),
						Expr::value(self.actor.clone()),
					])
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut *self.tx)
			.await?;
			Ok(())
		}
		.await;
		result.map_err(Into::into)
	}
	async fn commit(self) -> Result<()> {
		self.tx.commit().await?;
		Ok(())
	}
}
#[async_trait]
impl MappingAccess for AccessScope {
	async fn current(
		&mut self,
		tenant: &TenantName,
		node: &NodeName,
		source: &NodeAddress,
		subject: &str,
	) -> Result<Option<Mapping>> {
		let result: NativeResult<Option<PeerMapping>> = async {
			crate::database::native::query_as(
				&Query::select()
					.expr(SimpleExpr::from(Expr::col(reinhardt::query::ColumnRef::Asterisk)))
					.from(Alias::new("authorization_peer_mappings"))
					.and_where(SimpleExpr::CustomWithExpr(
						"(tenant = ? AND node = ? AND source_node = ? AND source_subject = ? AND enabled)"
							.to_owned(),
						vec![
							Expr::value(tenant.as_str().to_owned()).into(),
							Expr::value(node.as_str().to_owned()).into(),
							Expr::value(source.to_string()).into(),
							Expr::value(subject.to_owned()).into(),
						],
					))
					.lock(LockType::Share)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut **self.access.tx)
			.await
		}
		.await;
		result
			.and_then(|row| row.map(Mapping::try_from).transpose())
			.map_err(Into::into)
	}
	async fn registered(
		&mut self,
		tenant: &TenantName,
		node: &NodeName,
		source: &NodeAddress,
	) -> Result<bool> {
		registrations::get_locked(
			&mut *self.access.tx,
			tenant.as_str(),
			node.as_str(),
			source,
			true,
		)
		.await
		.map(|registration| registration.is_some_and(|registration| registration.enabled))
		.map_err(Into::into)
	}
	fn environment(&mut self) -> &mut Value {
		self.access.environment_mut()
	}
	fn context(&mut self) -> &mut Value {
		&mut self.access.context
	}
}
