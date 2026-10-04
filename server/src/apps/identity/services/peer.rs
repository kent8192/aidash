//! Explicit inbound identity mappings. Peer authentication alone grants no
//! tenant authority, and local subject bearer tokens never cross this boundary.
use crate::apps::federation::peer::services::mapping_records::PeerMappingRecords;
use reinhardt::query::Alias;
use reinhardt::query::Expr;
use reinhardt::query::LockType;
use reinhardt::query::OnConflict;
use reinhardt::query::PostgresQueryBuilder;
use reinhardt::query::Query;
use reinhardt::query::QueryStatementBuilder as _;
use reinhardt::query::SimpleExpr;
use reinhardt::{Depends, injectable};
#[path = "peer/admission.rs"]
pub(crate) mod admission;
#[path = "peer/discovery.rs"]
pub(crate) mod discovery;
#[path = "peer/execution.rs"]
pub(crate) mod execution;
#[path = "peer/graph.rs"]
pub(crate) mod graph;
#[path = "peer/reads.rs"]
pub(crate) mod reads;

use super::{
	Authorization, access::Access, catalog, identity::SubjectIdentity, policy::identifier,
};
use crate::{Error, Result, federation::Federation, registry::Entry};

use serde_json::json;

pub async fn write(f: &Federation, tenant: &str, input: PeerMappingInput) -> Result<PeerMapping> {
	identifier(tenant)?;
	identifier(&input.source_tenant)?;
	identifier(&input.source_subject)?;
	crate::config::validate_node_id(&input.source_node)?;
	if input.source_node == f.config.node_id || !(0..i64::MAX).contains(&input.expected_revision) {
		return Err(Error::Invalid("invalid peer mapping or revision".into()));
	}
	if input.enabled {
		f.peer(&input.source_node).await?;
	}
	// Only disabling an existing mapping bypasses a transaction barrier.
	let _visibility = if input.enabled {
		Some(crate::transactions::gate::ReadLease::begin(&f.store).await?)
	} else {
		None
	};
	if !input.enabled && input.expected_revision == 0 {
		return Err(Error::Invalid(
			"revocation requires an existing mapping".into(),
		));
	}
	let mut tx = if input.enabled {
		f.store.pool.begin().await?
	} else {
		f.store.control_pool.begin().await?
	};
	if !input.enabled {
		crate::transactions::authority::control(&mut tx).await?;
	}
	Authorization::load(&mut tx, tenant).await?;
	let subject: Option<String> = {
		let query_bind_1 = tenant;
		let query_bind_2 = input.credential_id;
		sqlx::query_scalar(
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
		.fetch_optional(&mut *tx)
		.await?
	};
	let identity = SubjectIdentity {
		http_session: None,
		credential_id: input.credential_id,
		tenant: tenant.into(),
		subject: subject.ok_or(Error::Forbidden)?,
	};
	if input.enabled {
		identity
			.lock_with_mode(&mut tx, false)
			.await
			.map_err(mapping_authority_error)?;
	}
	let mapping: PeerMapping = if input.expected_revision == 0 {
        { let query_bind_1 = &input.source_node; let query_bind_2 = &input.source_tenant; let query_bind_3 = &input.source_subject; let query_bind_4 = tenant; let query_bind_5 = input.credential_id; let query_bind_6 = input.enabled; sqlx::query_as(&Query::insert().into_table(Alias::new("authorization_peer_mappings")).columns([Alias::new("source_node"), Alias::new("source_tenant"), Alias::new("source_subject"), Alias::new("tenant"), Alias::new("credential_id"), Alias::new("enabled"), Alias::new("revision"), Alias::new("actor")]).from_subquery(Query::select().expr(SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into()])).expr(SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_2.to_owned()).into()])).expr(SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_3.to_owned()).into()])).expr(SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_4.to_owned()).into()])).expr(SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_5.to_owned()).into()])).expr(SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_6.to_owned()).into()])).expr(Expr::cust("1")).expr(Expr::cust("'operator'")).to_owned()).on_conflict(OnConflict::columns(["source_node", "source_tenant", "source_subject"]).do_nothing().to_owned()).returning_all().to_string(PostgresQueryBuilder)).fetch_optional(&mut *tx).await? }
    } else {
        { let query_bind_1 = &input.source_node; let query_bind_2 = &input.source_tenant; let query_bind_3 = &input.source_subject; let query_bind_4 = tenant; let query_bind_5 = input.credential_id; let query_bind_6 = input.enabled; let query_bind_7 = input.expected_revision; sqlx::query_as(&Query::update().table(Alias::new("authorization_peer_mappings")).value_expr(Alias::new("credential_id"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_5.to_owned()).into()])).value_expr(Alias::new("enabled"), SimpleExpr::CustomWithExpr("(?)".to_owned(), vec![Expr::value(query_bind_6.to_owned()).into()])).value_expr(Alias::new("revision"), Expr::cust("revision + 1")).value_expr(Alias::new("actor"), Expr::cust("'operator'")).value_expr(Alias::new("updated_at"), Expr::cust("CLOCK_TIMESTAMP()")).and_where(SimpleExpr::CustomWithExpr("(source_node = ? AND source_tenant = ? AND source_subject = ? AND tenant = ? AND revision = ?)".to_owned(), vec![Expr::value(query_bind_1.to_owned()).into(), Expr::value(query_bind_2.to_owned()).into(), Expr::value(query_bind_3.to_owned()).into(), Expr::value(query_bind_4.to_owned()).into(), Expr::value(query_bind_7.to_owned()).into()])).returning_all().to_string(PostgresQueryBuilder)).fetch_optional(&mut *tx).await? }
    }.ok_or_else(|| Error::Conflict("peer mapping revision or tenant changed".into()))?;
	{
		let query_bind_1 = &mapping.source_node;
		let query_bind_2 = &mapping.source_tenant;
		let query_bind_3 = &mapping.source_subject;
		let query_bind_4 = &mapping.tenant;
		let query_bind_5 = mapping.credential_id;
		let query_bind_6 = mapping.enabled;
		let query_bind_7 = mapping.revision;
		sqlx::query(
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
		.execute(&mut *tx)
		.await?
	};
	tx.commit().await?;
	Ok(mapping)
}

// The peer/operator is already authenticated. An unusable mapped credential
// denies authority; it does not challenge the caller to replace its bearer.
fn mapping_authority_error(error: Error) -> Error {
	match error {
		Error::Unauthorized => Error::Forbidden,
		other => other,
	}
}
pub(crate) async fn access(
	f: &Federation,
	node: &str,
	tenant: &str,
	subject: &str,
) -> Result<Access> {
	access_mode(f, node, tenant, subject, false).await
}

// Authority RPCs distinguish a permanent denial from a transport outage while
// never reflecting a peer's response body or internal error details.
pub(crate) async fn authority_request<T: serde::de::DeserializeOwned>(
	f: &Federation,
	node: &str,
	path: &str,
	body: &serde_json::Value,
) -> Result<T> {
	let response = f
		.peer_response(node, reqwest::Method::POST, path, Some(body))
		.await
		.map_err(|error| {
			tracing::warn!(%node,%path,error=%error,"authority request failed");
			Error::External("remote execution authority unavailable".into())
		})?;
	let status = response.status().as_u16();
	if !response.status().is_success()
		&& let Some(reason) = response
			.headers()
			.get("x-aidash-semantic-reason")
			.and_then(|value| value.to_str().ok())
			.and_then(|value| serde_json::from_value(serde_json::json!(value)).ok())
	{
		return Err(Error::RemoteSemantic(reason));
	}
	match status {
		200..=299 => crate::response::json(response, 4_194_304)
			.await
			.map_err(|_| Error::External("invalid remote authority response".into())),
		401 | 403 | 404 => Err(Error::Forbidden),
		409 => Err(Error::Conflict("remote execution authority changed".into())),
		503 if response
			.headers()
			.get("x-aidash-transaction-pending")
			.is_some_and(|value| value == "1") =>
		{
			Err(Error::TransactionPending)
		}
		_ => {
			tracing::warn!(%node,%path,status,"authority request rejected");
			Err(Error::External(
				"remote execution authority unavailable".into(),
			))
		}
	}
}

pub(crate) use crate::apps::identity::serializers::peer::{
	DiscoveryInput, HistoryPage, MappingPage, MappingRevision,
};
pub use crate::apps::identity::serializers::peer::{PeerMapping, PeerMappingInput};

use http::HeaderMap;

#[derive(Clone)]
pub struct PeerMappings {
	pub(crate) runtime: Federation,
	records: PeerMappingRecords,
}

#[injectable(scope = "request")]
pub(crate) async fn provide_peer_mappings(
	#[inject] runtime: Federation,
	#[inject] records: Depends<PeerMappingRecords>,
) -> PeerMappings {
	PeerMappings {
		runtime,
		records: records.into_inner(),
	}
}

impl PeerMappings {
	pub(crate) async fn history(
		&self,
		tenant: String,
		page: HistoryPage,
	) -> Result<Vec<MappingRevision>> {
		identifier(&tenant)?;
		crate::http::validate(&page)?;
		self.records
			.history(&tenant, page.after, page.limit as usize)
			.await
	}
	pub(crate) async fn list(&self, tenant: String, page: MappingPage) -> Result<Vec<PeerMapping>> {
		identifier(&tenant)?;
		crate::http::validate(&page)?;
		self.records
			.list(&tenant, page.offset as usize, page.limit as usize)
			.await
	}
	pub(crate) async fn set(&self, tenant: String, input: PeerMappingInput) -> Result<PeerMapping> {
		let f = self.runtime.clone();
		write(&f, &tenant, input).await
	}
	pub(crate) async fn discover(
		&self,
		headers: HeaderMap,
		input: DiscoveryInput,
	) -> Result<Vec<Entry>> {
		let f = self.runtime.clone();
		let node = crate::apps::identity::services::http_auth::peer_node(&headers)?;
		let mut access = access(&f, node, &input.tenant, &input.subject).await?;
		let result = async {
			let resource = access.resource("node", &f.config.node_id, json!({}));
			access.require(&resource, "federation.discover").await?;
			let mut search = input.search;
			search.kind = Some("agent".into());
			let mut visible = vec![];
			for entry in catalog::list_in(&mut access, &search).await? {
				if access
					.decide(&catalog::resource(&access, &entry), "agent.execute")
					.await?
				{
					visible.push(entry);
				}
			}
			Ok(visible)
		}
		.await;
		access.finish(result).await
	}
}

pub(crate) async fn access_mode(
	f: &Federation,
	node: &str,
	tenant: &str,
	subject: &str,
	exclusive: bool,
) -> Result<Access> {
	identifier(tenant)?;
	identifier(subject)?;
	let mapping: PeerMapping = {
		let query_bind_1 = node;
		let query_bind_2 = tenant;
		let query_bind_3 = subject;
		sqlx::query_as(
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
	}
	.ok_or(Error::Forbidden)?;
	let local_subject: String = {
		let query_bind_1 = mapping.credential_id;
		let query_bind_2 = &mapping.tenant;
		sqlx::query_scalar(
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
		.fetch_optional(&f.store.pool)
		.await?
	}
	.ok_or(Error::Forbidden)?;
	let identity = SubjectIdentity {
		http_session: None,
		credential_id: mapping.credential_id,
		tenant: mapping.tenant.clone(),
		subject: local_subject,
	};
	let mut access = if exclusive {
		Access::begin_exclusive(&f.store, &identity).await
	} else {
		Access::begin(&f.store, &identity).await
	}
	.map_err(mapping_authority_error)?;
	// Lock in the same order as management: policy, credential, then mapping.
	// A binding changed between resolution and this lease cannot select a new
	// credential or tenant under the old identity.
	let current: Option<PeerMapping> = {
		let query_bind_1 = node;
		let query_bind_2 = tenant;
		let query_bind_3 = subject;
		sqlx::query_as(
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
		.fetch_optional(&mut **access.tx)
		.await?
	};
	if current.as_ref() != Some(&mapping) {
		return Err(Error::Forbidden);
	}
	// Retain enabled peer admission through the metadata read, just as the
	// mapped policy, credential and binding remain leased until completion.
	let enabled: Option<String> = {
		let query_bind_1 = node;
		sqlx::query_scalar(
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
		.fetch_optional(&mut **access.tx)
		.await?
	};
	if enabled.is_none() {
		return Err(Error::Forbidden);
	}
	access.environment["transport"] = json!("federation");
	access.environment["source_node"] = json!(node);
	access.context = json!({"source_node":node,"source_tenant":tenant,"source_subject":subject});
	Ok(access)
}
#[path = "peer/dependencies.rs"]
pub(crate) mod dependencies;

#[path = "peer/semantic.rs"]
pub(crate) mod semantic;
