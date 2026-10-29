//! Original catalog joins and locks implement portable admission/discovery ports.
use crate::{
	Result,
	authorization::access::Access,
	registry::{EntityRef, Entry},
};
use aidash_application::ports::catalog::CatalogScope;
use aidash_domain::identity::catalog::CatalogRestriction;
use aidash_domain::policy::Resource;
use async_trait::async_trait;
use reinhardt::query::{
	Alias, Condition, Expr, ExprTrait, JoinType, LockType, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder, TableRef,
};
use serde_json::Value;
pub(crate) struct NativeCatalog<'a>(pub(crate) &'a mut Access);
fn agent_tail_matches(tail: &str, reference: &EntityRef) -> bool {
	tail == format!("{}@{}", reference.id, reference.version)
}

async fn document(access: &mut Access, reference: &EntityRef) -> Result<Option<Value>> {
	if aidash_application::registry::system::builtin_reference(reference) {
		return system_document(access, reference).await.map(Some);
	}
	let node = access.catalog_node().await?;
	let query = if access.inherited_lease {
		Query::select()
			.column((Alias::new("r"), Alias::new("metadata")))
			.from_as(Alias::new("authorization_catalog"), Alias::new("c"))
			.join(
				JoinType::InnerJoin,
				TableRef::table_alias(Alias::new("registry"), Alias::new("r")),
				Condition::all()
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col((
							Alias::new("r"),
							Alias::new("tenant"),
						)))
						.eq(Expr::col((Alias::new("c"), Alias::new("tenant")))),
					)
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col((
							Alias::new("r"),
							Alias::new("id"),
						)))
						.eq(Expr::col((Alias::new("c"), Alias::new("entry_id")))),
					)
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col((
							Alias::new("r"),
							Alias::new("version"),
						)))
						.eq(Expr::col((Alias::new("c"), Alias::new("entry_version")))),
					),
			)
			.and_where(
				Condition::all()
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col((
							Alias::new("c"),
							Alias::new("tenant"),
						)))
						.eq(Expr::cust("$1")),
					)
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col((
							Alias::new("c"),
							Alias::new("entry_id"),
						)))
						.eq(Expr::cust("$2")),
					)
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col((
							Alias::new("c"),
							Alias::new("entry_version"),
						)))
						.eq(Expr::cust("$3")),
					)
					.add(Expr::col((Alias::new("c"), Alias::new("enabled"))))
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col((
							Alias::new("c"),
							Alias::new("node"),
						)))
						.eq(Expr::cust("$4")),
					),
			)
			.to_string(PostgresQueryBuilder)
	} else {
		Query::select()
			.column((Alias::new("r"), Alias::new("metadata")))
			.from_as(Alias::new("authorization_catalog"), Alias::new("c"))
			.join(
				JoinType::InnerJoin,
				TableRef::table_alias(Alias::new("registry"), Alias::new("r")),
				Condition::all()
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col((
							Alias::new("r"),
							Alias::new("tenant"),
						)))
						.eq(Expr::col((Alias::new("c"), Alias::new("tenant")))),
					)
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col((
							Alias::new("r"),
							Alias::new("id"),
						)))
						.eq(Expr::col((Alias::new("c"), Alias::new("entry_id")))),
					)
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col((
							Alias::new("r"),
							Alias::new("version"),
						)))
						.eq(Expr::col((Alias::new("c"), Alias::new("entry_version")))),
					),
			)
			.and_where(
				Condition::all()
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col((
							Alias::new("c"),
							Alias::new("tenant"),
						)))
						.eq(Expr::cust("$1")),
					)
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col((
							Alias::new("c"),
							Alias::new("entry_id"),
						)))
						.eq(Expr::cust("$2")),
					)
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col((
							Alias::new("c"),
							Alias::new("entry_version"),
						)))
						.eq(Expr::cust("$3")),
					)
					.add(Expr::col((Alias::new("c"), Alias::new("enabled"))))
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col((
							Alias::new("c"),
							Alias::new("node"),
						)))
						.eq(Expr::cust("$4")),
					),
			)
			.lock(LockType::Share)
			.lock_tables([Alias::new("c")])
			.to_string(PostgresQueryBuilder)
	};
	let document: Option<Value> = crate::database::native::query_scalar(&query)
		.bind(&access.identity.tenant)
		.bind(&reference.id)
		.bind(&reference.version)
		.bind(node.as_str())
		.scalar_optional(&mut **access.tx)
		.await?;

	Ok(document)
}
async fn documents(access: &mut Access) -> Result<Vec<Value>> {
	let node = access.catalog_node().await?;
	let query = if access.inherited_lease {
		Query::select()
			.column((Alias::new("r"), Alias::new("metadata")))
			.from_as(Alias::new("authorization_catalog"), Alias::new("c"))
			.join(
				JoinType::InnerJoin,
				TableRef::table_alias(Alias::new("registry"), Alias::new("r")),
				Condition::all()
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col((
							Alias::new("r"),
							Alias::new("tenant"),
						)))
						.eq(Expr::col((Alias::new("c"), Alias::new("tenant")))),
					)
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col((
							Alias::new("r"),
							Alias::new("id"),
						)))
						.eq(Expr::col((Alias::new("c"), Alias::new("entry_id")))),
					)
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col((
							Alias::new("r"),
							Alias::new("version"),
						)))
						.eq(Expr::col((Alias::new("c"), Alias::new("entry_version")))),
					),
			)
			.and_where(
				Condition::all()
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col((
							Alias::new("c"),
							Alias::new("tenant"),
						)))
						.eq(Expr::cust("$1")),
					)
					.add(Expr::col((Alias::new("c"), Alias::new("enabled"))))
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col((
							Alias::new("c"),
							Alias::new("node"),
						)))
						.eq(Expr::cust("$2")),
					),
			)
			.order_by((Alias::new("c"), Alias::new("entry_id")), Order::Asc)
			.order_by((Alias::new("c"), Alias::new("entry_version")), Order::Asc)
			.to_string(PostgresQueryBuilder)
	} else {
		Query::select()
			.column((Alias::new("r"), Alias::new("metadata")))
			.from_as(Alias::new("authorization_catalog"), Alias::new("c"))
			.join(
				JoinType::InnerJoin,
				TableRef::table_alias(Alias::new("registry"), Alias::new("r")),
				Condition::all()
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col((
							Alias::new("r"),
							Alias::new("tenant"),
						)))
						.eq(Expr::col((Alias::new("c"), Alias::new("tenant")))),
					)
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col((
							Alias::new("r"),
							Alias::new("id"),
						)))
						.eq(Expr::col((Alias::new("c"), Alias::new("entry_id")))),
					)
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col((
							Alias::new("r"),
							Alias::new("version"),
						)))
						.eq(Expr::col((Alias::new("c"), Alias::new("entry_version")))),
					),
			)
			.and_where(
				Condition::all()
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col((
							Alias::new("c"),
							Alias::new("tenant"),
						)))
						.eq(Expr::cust("$1")),
					)
					.add(Expr::col((Alias::new("c"), Alias::new("enabled"))))
					.add(
						reinhardt::query::SimpleExpr::from(Expr::col((
							Alias::new("c"),
							Alias::new("node"),
						)))
						.eq(Expr::cust("$2")),
					),
			)
			.order_by((Alias::new("c"), Alias::new("entry_id")), Order::Asc)
			.order_by((Alias::new("c"), Alias::new("entry_version")), Order::Asc)
			.lock(LockType::Share)
			.lock_tables([Alias::new("c")])
			.to_string(PostgresQueryBuilder)
	};
	let mut documents: Vec<Value> = crate::database::native::query_scalar(&query)
		.bind(&access.identity.tenant)
		.bind(node.as_str())
		.scalar_all(&mut **access.tx)
		.await?;

	for entry in aidash_application::registry::system::declarations(
		&crate::bootstrap::registry_validation(),
		&access.node_id,
	)? {
		let reference = EntityRef {
			id: entry.id,
			version: entry.version,
		};
		let document = system_document(access, &reference).await?;
		if !documents.iter().any(|existing| {
			existing["id"] == document["id"] && existing["version"] == document["version"]
		}) {
			documents.push(document);
		}
	}
	Ok(documents)
}

/// Reserved spelling alone is insufficient: persisted bytes must match Node composition.
async fn system_document(access: &mut Access, reference: &EntityRef) -> Result<Value> {
	let expected = aidash_application::registry::system::declarations(
		&crate::bootstrap::registry_validation(),
		&access.node_id,
	)?
	.into_iter()
	.find(|entry| entry.id == reference.id && entry.version == reference.version)
	.ok_or(crate::Error::Forbidden)?;
	let stored = crate::apps::registry::models::transaction_records::definition(
		&mut **access.tx,
		&reference.id,
		&reference.version,
	)
	.await?;
	let entry: Entry = serde_json::from_value(stored.metadata.0)?;
	if entry != expected {
		return Err(crate::Error::Conflict(
			"system declaration bytes differ from admitted Node composition".into(),
		));
	}
	Ok(serde_json::to_value(entry)?)
}
#[async_trait]
impl CatalogScope for NativeCatalog<'_> {
	fn tenant(&self) -> &str {
		&self.0.identity.tenant
	}
	fn inherited_lease(&self) -> bool {
		self.0.inherited_lease
	}
	fn approved(&self, reference: &EntityRef) -> bool {
		self.0
			.approved_catalog
			.contains(&(reference.id.clone(), reference.version.clone()))
	}
	fn remember(&mut self, reference: &EntityRef) {
		self.0
			.approved_catalog
			.insert((reference.id.clone(), reference.version.clone()));
	}
	fn direct_user(&self, reference: &EntityRef) -> String {
		// A Run's subject chain ends with the agent that is executing. The
		// agent version itself is used by the subject before it (whoever
		// started or delegated the Run); a dependency is used by the agent.
		let identity = self.0.identity.subject.clone();
		let Some(last) = self.0.subjects.last() else {
			return identity;
		};
		let agent = last
			.rsplit_once("/agents/")
			.is_some_and(|(_, tail)| agent_tail_matches(tail, reference));
		if agent {
			self.0
				.subjects
				.iter()
				.rev()
				.nth(1)
				.cloned()
				.unwrap_or(identity)
		} else {
			last.clone()
		}
	}
	async fn permits_restriction(
		&mut self,
		reference: &EntityRef,
		direct_user: &str,
	) -> aidash_application::Result<bool> {
		let node = self.0.catalog_node().await?;
		let restriction: Option<Value> = crate::database::native::query_scalar(
			&Query::select()
				.column(Alias::new("restriction"))
				.from(Alias::new("authorization_catalog"))
				.and_where(Expr::col(Alias::new("tenant")).eq(Expr::cust("$1")))
				.and_where(Expr::col(Alias::new("entry_id")).eq(Expr::cust("$2")))
				.and_where(Expr::col(Alias::new("entry_version")).eq(Expr::cust("$3")))
				.and_where(Expr::col(Alias::new("node")).eq(Expr::cust("$4")))
				.and_where(Expr::col(Alias::new("enabled")))
				.to_string(PostgresQueryBuilder),
		)
		.bind(&self.0.identity.tenant)
		.bind(&reference.id)
		.bind(&reference.version)
		.bind(node.as_str())
		.scalar_optional(&mut **self.0.tx)
		.await?;
		let Some(restriction) = restriction.filter(|value| !value.is_null()) else {
			return Ok(true);
		};
		let restriction: CatalogRestriction =
			serde_json::from_value(restriction).map_err(aidash_application::Error::from)?;
		let groups = self
			.0
			.snapshot
			.bundle
			.subjects
			.get(direct_user)
			.map(|subject| subject.groups.clone())
			.unwrap_or_default();
		Ok(restriction.allows(direct_user, &groups))
	}
	fn admitted(&self, reference: &EntityRef, action: &str) -> Option<Entry> {
		self.0
			.tx
			.admitted(&self.0.catalog_admission(reference, action))
	}
	fn admit(&mut self, reference: &EntityRef, action: &str, entry: &Entry) {
		let admission = self.0.catalog_admission(reference, action);
		self.0.tx.admit(admission, entry.clone());
	}
	async fn distribution_lock(&mut self) -> aidash_application::Result<()> {
		crate::marketplace::lock_catalog(&mut self.0.tx, false)
			.await
			.map_err(Into::into)
	}
	async fn document(
		&mut self,
		reference: &EntityRef,
	) -> aidash_application::Result<Option<Value>> {
		document(self.0, reference).await.map_err(Into::into)
	}
	async fn documents(&mut self) -> aidash_application::Result<Vec<Value>> {
		documents(self.0).await.map_err(Into::into)
	}
	fn resource(&self, entry: &Entry) -> Resource {
		crate::authorization::catalog::resource(self.0, entry)
	}
	async fn require(
		&mut self,
		resource: &Resource,
		action: &str,
	) -> aidash_application::Result<()> {
		self.0.require(resource, action).await.map_err(Into::into)
	}
	async fn decide(
		&mut self,
		resource: &Resource,
		action: &str,
	) -> aidash_application::Result<bool> {
		self.0.decide(resource, action).await.map_err(Into::into)
	}
	async fn active(&mut self, entry: &Entry) -> aidash_application::Result<bool> {
		crate::marketplace::active(self.0, entry)
			.await
			.map_err(Into::into)
	}
	async fn check_pinned(&mut self, entry: &Entry) -> aidash_application::Result<()> {
		crate::marketplace::check_pinned(self.0, entry)
			.await
			.map_err(Into::into)
	}
}
