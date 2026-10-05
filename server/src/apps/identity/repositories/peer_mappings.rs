//! Mapping statements retain policy-before-credential locks, revision fences and control-pool revocation.
use crate::apps::identity::serializers::peer::PeerMapping;
use crate::{
	Result as NativeResult,
	authorization::{Authorization, access::Access, identity::SubjectIdentity},
	federation::Federation,
};
use aidash_application::{
	Result,
	ports::authorization::peer::mappings::{MappingAccess, MappingRepository, MappingWrite},
};
use aidash_domain::identity::peer_mapping::{Mapping, PeerMappingInput};
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Expr, LockType, OnConflict, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
	SimpleExpr,
};
use serde_json::Value;
use uuid::Uuid;
pub(crate) struct Repository<'a> {
	pub runtime: &'a Federation,
}
pub(crate) struct WriteScope {
	tx: crate::database::native::Transaction,
	_visibility: Option<crate::transactions::gate::ReadLease>,
}
pub(crate) struct AccessScope {
	pub access: Box<Access>,
}
#[async_trait]
impl MappingRepository for Repository<'_> {
	type Write = WriteScope;
	type Access = AccessScope;
	fn node_id(&self) -> &str {
		&self.runtime.config.node_id
	}
	async fn enabled_peer(&self, node: &str) -> Result<()> {
		self.runtime
			.peer(node)
			.await
			.map(|_| ())
			.map_err(Into::into)
	}
	async fn begin_write(&self, enabled: bool) -> Result<WriteScope> {
		let result: NativeResult<WriteScope> = async {
			let visibility = if enabled {
				Some(crate::transactions::gate::ReadLease::begin(&self.runtime.store).await?)
			} else {
				None
			};
			let mut tx = if enabled {
				crate::database::native::begin(&self.runtime.store.pool).await?
			} else {
				crate::database::native::begin(&self.runtime.store.control_pool).await?
			};
			if !enabled {
				crate::transactions::authority::control(&mut tx).await?;
			}
			Ok(WriteScope {
				tx,
				_visibility: visibility,
			})
		}
		.await;
		result.map_err(Into::into)
	}
	async fn resolved(&self, node: &str, tenant: &str, subject: &str) -> Result<Option<Mapping>> {
		let f = self.runtime;
		let result: NativeResult<Option<PeerMapping>> = async {
			Ok({
				let query_bind_1 = node;
				let query_bind_2 = tenant;
				let query_bind_3 = subject;
				crate::database::native::query_as(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::SimpleExpr::from(
					reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk),
				))
				.from(reinhardt::query::Alias::new("authorization_peer_mappings"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(source_node = ? AND source_tenant = ? AND source_subject = ? AND enabled)"
						.to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
						Expr::value(query_bind_3.to_owned()).into(),
					],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_optional(&f.store.pool)
		.await?
			})
		}
		.await;
		result.map(|row| row.map(Into::into)).map_err(Into::into)
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
				.scalar_optional(&f.store.pool)
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
			Ok({
				let query_bind_1 = &input.source_node;
				let query_bind_2 = &input.source_tenant;
				let query_bind_3 = &input.source_subject;
				let query_bind_4 = tenant;
				let query_bind_5 = input.credential_id;
				let query_bind_6 = input.enabled;
				crate::database::native::query_as(
					&Query::insert()
						.into_table(Alias::new("authorization_peer_mappings"))
						.columns([
							Alias::new("source_node"),
							Alias::new("source_tenant"),
							Alias::new("source_subject"),
							Alias::new("tenant"),
							Alias::new("credential_id"),
							Alias::new("enabled"),
							Alias::new("revision"),
							Alias::new("actor"),
						])
						.from_subquery(
							Query::select()
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_2.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_3.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_4.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_5.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_6.to_owned()).into()],
								))
								.expr(Expr::cust("1"))
								.expr(Expr::cust("'operator'"))
								.to_owned(),
						)
						.on_conflict(
							OnConflict::columns(["source_node", "source_tenant", "source_subject"])
								.do_nothing()
								.to_owned(),
						)
						.returning_all()
						.to_string(PostgresQueryBuilder),
				)
				.columns(&[
					"source_node",
					"source_tenant",
					"source_subject",
					"tenant",
					"credential_id",
					"enabled",
					"revision",
					"actor",
				])
				.fetch_optional(&mut *self.tx)
				.await?
			})
		}
		.await;
		result.map(|row| row.map(Into::into)).map_err(Into::into)
	}
	async fn update(&mut self, tenant: &str, input: &PeerMappingInput) -> Result<Option<Mapping>> {
		let result:NativeResult<Option<PeerMapping>>=async {Ok({ let query_bind_1 = &input.source_node; let query_bind_2 = &input.source_tenant; let query_bind_3 = &input.source_subject; let query_bind_4 = tenant; let query_bind_5 = input.credential_id; let query_bind_6 = input.enabled; let query_bind_7 = input.expected_revision; crate::database::native::query_as(&Query::update().table(Alias::new("authorization_peer_mappings")).value_expr(Alias::new("credential_id"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_5.to_owned()).into()])).value_expr(Alias::new("enabled"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_6.to_owned()).into()])).value_expr(Alias::new("revision"), Expr::cust("revision + 1")).value_expr(Alias::new("actor"), Expr::cust("'operator'")).value_expr(Alias::new("updated_at"), Expr::cust("CLOCK_TIMESTAMP()")).and_where(SimpleExpr::CustomWithExpr("(source_node = ? AND source_tenant = ? AND source_subject = ? AND tenant = ? AND revision = ?)".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), Expr::value(query_bind_3.to_owned()).into(), Expr::value(query_bind_4.to_owned()).into(), Expr::value(query_bind_7.to_owned()).into()])).returning_all().to_string(PostgresQueryBuilder)).fetch_optional(&mut *self.tx).await? })}.await;
		result.map(|row| row.map(Into::into)).map_err(Into::into)
	}
	async fn history(&mut self, mapping: &Mapping) -> Result<()> {
		let result: NativeResult<()> = async {
			{
				let query_bind_1 = &mapping.source_node;
				let query_bind_2 = &mapping.source_tenant;
				let query_bind_3 = &mapping.source_subject;
				let query_bind_4 = &mapping.tenant;
				let query_bind_5 = mapping.credential_id;
				let query_bind_6 = mapping.enabled;
				let query_bind_7 = mapping.revision;
				crate::database::native::query(
					&Query::insert()
						.into_table(Alias::new("authorization_peer_mapping_history"))
						.columns([
							Alias::new("source_node"),
							Alias::new("source_tenant"),
							Alias::new("source_subject"),
							Alias::new("tenant"),
							Alias::new("credential_id"),
							Alias::new("enabled"),
							Alias::new("revision"),
							Alias::new("actor"),
						])
						.from_subquery(
							Query::select()
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_1.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_2.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_3.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_4.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_5.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_6.to_owned()).into()],
								))
								.expr(SimpleExpr::CustomWithExpr(
									"(?)".to_owned(),
									vec![Expr::value(query_bind_7.to_owned()).into()],
								))
								.expr(Expr::cust("'operator'"))
								.to_owned(),
						)
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut *self.tx)
				.await?
			};
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
		node: &str,
		tenant: &str,
		subject: &str,
	) -> Result<Option<Mapping>> {
		let result: NativeResult<Option<PeerMapping>> = async {
			Ok({
				let query_bind_1 = node;
				let query_bind_2 = tenant;
				let query_bind_3 = subject;
				crate::database::native::query_as(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::SimpleExpr::from(
					reinhardt::query::Expr::col(reinhardt::query::ColumnRef::Asterisk),
				))
				.from(reinhardt::query::Alias::new("authorization_peer_mappings"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(source_node = ? AND source_tenant = ? AND source_subject = ? AND enabled)"
						.to_owned(),
					vec![
						Expr::value(query_bind_1.to_owned()).into(),
						Expr::value(query_bind_2.to_owned()).into(),
						Expr::value(query_bind_3.to_owned()).into(),
					],
				))
				.lock(reinhardt::query::LockType::Share)
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_optional(&mut **self.access.tx)
		.await?
			})
		}
		.await;
		result.map(|row| row.map(Into::into)).map_err(Into::into)
	}
	async fn peer_enabled(&mut self, node: &str) -> Result<bool> {
		let result: NativeResult<Option<String>> = async {
			Ok({
				let query_bind_1 = node;
				crate::database::native::query_scalar(
					&reinhardt::query::Query::select()
						.expr(reinhardt::query::SimpleExpr::from(
							reinhardt::query::Expr::col(reinhardt::query::Alias::new("node_id")),
						))
						.from(reinhardt::query::Alias::new("peers"))
						.and_where(SimpleExpr::CustomWithExpr(
							"(node_id = ? AND enabled)".to_owned(),
							vec![Expr::value(query_bind_1.to_owned()).into()],
						))
						.lock(reinhardt::query::LockType::Share)
						.to_string(reinhardt::query::PostgresQueryBuilder),
				)
				.scalar_optional(&mut **self.access.tx)
				.await?
			})
		}
		.await;
		result.map(|row| row.is_some()).map_err(Into::into)
	}
	fn environment(&mut self) -> &mut Value {
		self.access.environment_mut()
	}
	fn context(&mut self) -> &mut Value {
		&mut self.access.context
	}
}
