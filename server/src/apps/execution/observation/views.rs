//! Non-content HTTP projections; aggregates require Tenant administrative permission.
use super::repository;
use crate::{
	Error, Result,
	apps::identity::{oidc::BrowserOrigin, services::tenant_administration::TenantAdministrations},
	authorization::{access::Access, identity::Actor},
	federation::Federation,
	http::json::Json,
};
use aidash_domain::{
	decision::{
		observation::{report::*, *},
		*,
	},
	registry::{EntityRef, bindings::QualifiedRef},
};
use chrono::{DateTime, Utc};
use reinhardt::{
	Path, Query, Response, UnifiedRouter, get,
	http::ViewResult,
	put,
	query::{
		Alias, Expr, ExprTrait, PostgresQueryBuilder, Query as SqlQuery, QueryStatementBuilder,
	},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "ApprovalObserverNodeQuery")]
pub struct NodeQuery {
	pub node: Option<aidash_domain::nodes::NodeName>,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "ApprovalObserverUpdate")]
pub struct SettingUpdate {
	pub expected_revision: i64,
	pub decider: Option<EntityRef>,
}
#[derive(Serialize, schemars::JsonSchema)]
#[schemars(rename = "ApprovalObserverSetting")]
pub struct SettingView {
	pub revision: i64,
	pub decider: Option<EntityRef>,
	pub node: String,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "ApprovalObservationRangeQuery")]
pub struct RangeQuery {
	pub start: DateTime<Utc>,
	pub end: DateTime<Utc>,
}
#[derive(Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(rename = "ApprovalObservationRoute")]
pub struct RouteView {
	pub route: u32,
	pub provider_contract: String,
	pub model: String,
	pub hosts: Vec<String>,
	pub effective_threshold: Probability,
	pub answers: std::collections::BTreeMap<String, Probability>,
	pub served_models: Vec<String>,
	pub served_hosts: Vec<String>,
	pub includes_unknown_model: bool,
	pub includes_unknown_host: bool,
}
#[derive(Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(rename = "ApprovalObservation")]
pub struct ObservationView {
	pub kind: ApprovalKind,
	pub settings_revision: i64,
	pub decider: QualifiedRef,
	pub configuration_digest: String,
	pub definition_digest: String,
	pub outcome: ObservationOutcome,
	pub routes: Vec<RouteView>,
	pub human_label: Option<HumanLabel>,
}
fn response<T: Serialize>(value: Result<T>) -> ViewResult<Response> {
	crate::http::json(value).map(|r| r.with_header("Cache-Control", "no-store"))
}
async fn administrative(
	runtime: &Federation,
	actor: Actor,
	origin: Option<BrowserOrigin>,
	tenant: &str,
	write: bool,
) -> Result<(
	TenantAdministrations,
	std::sync::Arc<crate::apps::identity::models::TenantAdministrator>,
	crate::database::native::Transaction,
)> {
	let admins = TenantAdministrations {
		runtime: runtime.clone(),
	};
	let admin = admins.administrator(actor, origin, tenant).await?;
	let mut tx = runtime.store.pool.begin(&admin.scope()?).await?;
	let snapshot = admin.lock(&mut *tx, write).await?;
	let allowed = admin
		.require(
			&mut *tx,
			&snapshot,
			if write {
				aidash_domain::identity::tenancy::TENANT_UPDATE
			} else {
				aidash_domain::identity::tenant_administration::APPROVAL_READ
			},
			if write {
				tenant
			} else {
				aidash_domain::identity::tenant_administration::COLLECTION
			},
			json!({}),
		)
		.await;
	if let Err(error) = allowed {
		return admins.finish(&admin, Err(error)).await;
	}
	Ok((admins, admin, tx))
}
async fn selected_node(
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	tenant: &str,
	query: NodeQuery,
) -> Result<aidash_domain::nodes::NodeName> {
	let node = match query.node {
		Some(node) => {
			crate::apps::identity::repositories::nodes::get(tx, tenant, node.as_str()).await?
		}
		None => crate::apps::identity::repositories::nodes::default_of(tx, tenant).await?,
	};
	Ok(node.name)
}
#[get(
	"/api/tenants/{tenant}/approval-observer",
	name = "approval-observer",
	auth = "protected"
)]
pub async fn settings(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
	Query(query): Query<NodeQuery>,
) -> ViewResult<Response> {
	response(
		async {
			let (admins, admin, mut tx) =
				administrative(&runtime, actor, origin, &tenant, false).await?;
			let result = async {
				let node = selected_node(&mut *tx, &tenant, query).await?;
				let setting = repository::settings(&mut *tx, &tenant).await?;
				tx.commit().await?;
				Ok(SettingView {
					revision: setting.revision,
					decider: setting.decider.map(|p| p.identity.local()),
					node: node.to_string(),
				})
			}
			.await;
			admins.finish(&admin, result).await
		}
		.await,
	)
}
#[put(
	"/api/tenants/{tenant}/approval-observer",
	name = "approval-observer-update",
	auth = "protected"
)]
pub async fn update(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
	Query(query): Query<NodeQuery>,
	Json(input): Json<SettingUpdate>,
) -> ViewResult<Response> {
	response(
		async {
			let (admins, admin, mut tx) =
				administrative(&runtime, actor, origin, &tenant, true).await?;
			let result = async {
				let node = selected_node(&mut *tx, &tenant, query).await?;
				let setting = repository::save_settings(
					&mut *tx,
					&tenant,
					input.expected_revision,
					input.decider.as_ref(),
					&node,
					runtime.config.deployment.as_str(),
				)
				.await?;
				tx.commit().await?;
				Ok(SettingView {
					revision: setting.revision,
					decider: setting.decider.map(|p| p.identity.local()),
					node: node.to_string(),
				})
			}
			.await;
			admins.finish(&admin, result).await
		}
		.await,
	)
}
#[get(
	"/api/tenants/{tenant}/approval-observations/{kind}/{id}",
	name = "approval-observation",
	auth = "protected"
)]
pub async fn individual(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	request: reinhardt::Request,
) -> ViewResult<Response> {
	response(
		async {
			let tenant = request.path_params.get("tenant").ok_or(Error::Forbidden)?;
			let kind = request.path_params.get("kind").ok_or(Error::Forbidden)?;
			let id = Uuid::parse_str(request.path_params.get("id").ok_or(Error::Forbidden)?)
				.map_err(|_| Error::Invalid("invalid approval id".into()))?;
			individual_read(&runtime, actor, tenant, kind, id).await
		}
		.await,
	)
}
async fn individual_read(
	runtime: &Federation,
	actor: Actor,
	tenant: &str,
	kind: &str,
	id: Uuid,
) -> Result<Option<ObservationView>> {
	let Actor::Subject(identity) = actor else {
		return Err(Error::Forbidden);
	};
	if identity.tenant != tenant {
		return Err(Error::Forbidden);
	}
	let mut access = Access::begin(&runtime.store, &identity).await?;
	let result = async {
		match kind {
			"capability" => {
				let mut found = None;
				for record_kind in aidash_application::capabilities::approvals::KINDS {
					match crate::apps::execution::capabilities::services::records::get(
						&mut access,
						id,
						record_kind,
					)
					.await
					{
						Ok(r) => {
							found = Some(r);
							break;
						}
						Err(Error::NotFound(_)) => {}
						Err(e) => return Err(e),
					}
				}
				let record = found.ok_or_else(|| Error::NotFound("approval".into()))?;
				if !crate::apps::execution::capabilities::services::approvals::visible(
					&mut access,
					&record,
				)
				.await?
				{
					return Err(Error::Forbidden);
				}
			}
			"generation" => {
				let job = crate::apps::execution::generation::repositories::lifecycle::load(
					&mut access.tx,
					tenant,
					id,
				)
				.await?;
				if !aidash_application::generation::visibility::visible(
					&mut crate::bootstrap::generation_visibility_scope(&mut access),
					&job,
				)
				.await?
				{
					return Err(Error::Forbidden);
				}
			}
			_ => return Err(Error::NotFound("approval kind".into())),
		}
		let Some(row) = repository::observation(&mut *access.tx, tenant, id).await? else {
			let population = crate::database::native::query(
				&SqlQuery::select()
					.columns(["observer_unavailable", "settlement"].map(Alias::new))
					.from(Alias::new("approval_observation_population"))
					.and_where(Expr::col("approval").eq(Expr::value(id)))
					.and_where(Expr::col("tenant").eq(tenant))
					.to_string(PostgresQueryBuilder),
			)
			.fetch_optional(&mut *access.tx)
			.await?;
			if let Some(population) = population {
				let view: Option<Value> = population.try_get("observer_unavailable")?;
				if let Some(mut view) = view {
					let settlement: String = population.try_get("settlement")?;
					let settlement: SettlementClass = serde_json::from_value(json!(settlement))?;
					view["human_label"] = json!(settlement.label());
					return Ok(Some(serde_json::from_value(view)?));
				}
			}
			return Ok(None);
		};
		if row.approval_type != kind {
			return Err(Error::NotFound("approval".into()));
		}
		let frame: ObservationFrame = serde_json::from_value(row.frame.0)?;
		let evidence: Option<ObservationEvidence> = row
			.evidence
			.map(|e| serde_json::from_value(e.0))
			.transpose()?;
		if let Some(e) = &evidence {
			e.validate(&frame)?;
		}
		let pin = frame.settings.decider.ok_or(Error::Forbidden)?;
		let settlement: SettlementClass = serde_json::from_value(json!(row.settlement))?;
		Ok(Some(ObservationView {
			kind: frame.kind,
			settings_revision: frame.settings.revision,
			decider: pin.identity,
			configuration_digest: pin.configuration_digest,
			definition_digest: pin.definition_digest,
			outcome: evidence
				.as_ref()
				.map_or(ObservationOutcome::Pending, |e| e.outcome.clone()),
			routes: evidence
				.map_or_else(Vec::new, |e| e.routes.into_iter().map(route_view).collect()),
			human_label: settlement.label(),
		}))
	}
	.await;
	access.finish(result).await
}
#[get(
	"/api/tenants/{tenant}/approval-observations/report",
	name = "approval-observation-report",
	auth = "protected"
)]
pub async fn aggregate(
	#[inject] runtime: Federation,
	#[inject] actor: Actor,
	#[inject] origin: Option<BrowserOrigin>,
	Path(tenant): Path<String>,
	Query(query): Query<RangeQuery>,
) -> ViewResult<Response> {
	response(
		async {
			let range = ReportRange {
				start: query.start,
				end: query.end,
			};
			range.validate()?;
			let (admins, admin, mut tx) =
				administrative(&runtime, actor, origin, &tenant, false).await?;
			let result = report_read(&mut tx, &tenant, range).await;
			let result = match result {
				Ok(report) => {
					tx.commit().await?;
					Ok(report)
				}
				Err(e) => Err(e),
			};
			admins.finish(&admin, result).await
		}
		.await,
	)
}
pub(super) async fn report_read(
	tx: &mut crate::database::native::Transaction,
	tenant: &str,
	range: ReportRange,
) -> Result<ObservationReport> {
	range.validate()?;
	let rows = crate::database::native::query(
		&SqlQuery::select()
			.columns(
				[
					"approval",
					"approval_type",
					"kind",
					"observer_unavailable",
					"settlement",
					"created_at",
				]
				.map(Alias::new),
			)
			.from(Alias::new("approval_observation_population"))
			.and_where(Expr::col("tenant").eq(tenant))
			.and_where(Expr::col("created_at").gte(Expr::value(range.start)))
			.and_where(Expr::col("created_at").lt(Expr::value(range.end)))
			.limit(10001)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&mut **tx)
	.await?;
	if rows.len() > 10000 {
		return Err(Error::Invalid(
			"report exceeds 10000 approvals; choose a shorter range".into(),
		));
	}
	let mut samples = Vec::with_capacity(rows.len());
	for row in rows {
		let approval: Uuid = row.try_get("approval")?;
		let created_at: DateTime<Utc> = row.try_get("created_at")?;
		let stored: String = row.try_get("settlement")?;
		let mut settlement: SettlementClass = serde_json::from_value(json!(stored))?;
		let kind: String = row.try_get("approval_type")?;
		let approval_kind: ApprovalKind =
			serde_json::from_value(json!(row.try_get::<String>("kind")?))?;
		let unavailable: Option<Value> = row.try_get("observer_unavailable")?;
		if settlement.label().is_none() {
			let current = if kind == "capability" {
				crate::database::native::query(
					&SqlQuery::select()
						.columns(["state", "data", "expires_at"].map(Alias::new))
						.from(Alias::new("core_records"))
						.and_where(Expr::col("id").eq(Expr::value(approval)))
						.to_string(PostgresQueryBuilder),
				)
				.fetch_optional(&mut **tx)
				.await?
			} else {
				None
			};
			if let Some(current) = current {
				let state: String = current.try_get("state")?;
				let data: Value = current.try_get("data")?;
				let expiry: Option<DateTime<Utc>> = current.try_get("expires_at")?;
				settlement = match state.as_str() {
					"pending" if expiry.is_some_and(|e| e <= Utc::now()) => {
						SettlementClass::Expired
					}
					"revoked" => SettlementClass::Revoked,
					"closed" => SettlementClass::Cancelled,
					_ => repository::initial_settlement(&state, &data),
				};
			}
			if kind == "generation" {
				let state: Option<String> = crate::database::native::query_scalar(
					&SqlQuery::select()
						.column(Alias::new("status"))
						.from(Alias::new("generation_requests"))
						.and_where(Expr::col("id").eq(Expr::value(approval)))
						.to_string(PostgresQueryBuilder),
				)
				.scalar_optional(&mut **tx)
				.await?;
				settlement = match state.as_deref() {
					Some("EXPIRED") => SettlementClass::Expired,
					Some("STOPPED" | "DELETED") | None => SettlementClass::Cancelled,
					Some("PENDING_APPROVAL") => SettlementClass::Pending,
					_ if settlement == SettlementClass::Pending => SettlementClass::Auto,
					_ => settlement,
				};
			}
		}
		if let Some(observed) = repository::observation(&mut **tx, tenant, approval).await? {
			let frame: ObservationFrame = serde_json::from_value(observed.frame.0)?;
			let evidence: Option<ObservationEvidence> = observed
				.evidence
				.map(|e| serde_json::from_value(e.0))
				.transpose()?;
			samples.push(ReportSample::observed(
				&frame,
				evidence.as_ref(),
				settlement,
			)?);
		} else {
			samples.push(ReportSample {
				approval,
				created_at,
				group: None,
				kind: approval_kind,
				outcome: unavailable.map(|_| ObservationOutcome::Unavailable {
					reason: UnavailableReason::MissingState,
				}),
				settlement,
				maximum_category_probability: None,
			});
		}
	}
	Ok(report(range, &samples)?)
}
pub(crate) fn routes() -> UnifiedRouter {
	use crate::apps::identity::services::boundary::AccessBoundary;
	UnifiedRouter::new().server(|server| {
		server
			.endpoint(settings)
			.with_route_middleware(AccessBoundary::authenticated().with_visibility())
			.endpoint(update)
			.with_route_middleware(AccessBoundary::authenticated().with_visibility())
			.endpoint(aggregate)
			.with_route_middleware(AccessBoundary::authenticated().with_visibility())
			.endpoint(individual)
			.with_route_middleware(AccessBoundary::authenticated().with_visibility())
	})
}

pub(crate) fn register(
	contracts: &mut crate::config::openapi::Contracts,
	document: &mut reinhardt::rest::openapi::OpenApiSchema,
) -> Result<()> {
	contracts.response::<_, SettingView>(document, settings, 200, "application/json")?;
	contracts.query::<_, NodeQuery>(document, settings)?;
	contracts.path(document, settings, &["String"])?;
	contracts.response::<_, SettingView>(document, update, 200, "application/json")?;
	contracts.request::<_, SettingUpdate>(document, update)?;
	contracts.query::<_, NodeQuery>(document, update)?;
	contracts.path(document, update, &["String"])?;
	contracts.response::<_, Option<ObservationView>>(
		document,
		individual,
		200,
		"application/json",
	)?;
	contracts.path(document, individual, &["String", "String", "Uuid"])?;
	contracts.response::<_, ObservationReport>(document, aggregate, 200, "application/json")?;
	contracts.query::<_, RangeQuery>(document, aggregate)?;
	contracts.path(document, aggregate, &["String"])?;
	Ok(())
}

fn route_view(route: RouteEvidence) -> RouteView {
	let mut models = std::collections::BTreeSet::new();
	let mut hosts = std::collections::BTreeSet::new();
	let mut unknown_model = route.attempts.is_empty();
	let mut unknown_host = route.attempts.is_empty();
	for attempt in &route.attempts {
		match &attempt.provider_report {
			AttemptProviderReport::Unknown => {
				unknown_model = true;
				unknown_host = true;
			}
			AttemptProviderReport::Reported {
				served_model,
				served_host,
				..
			} => {
				if let Some(model) = served_model {
					models.insert(model.clone());
				} else {
					unknown_model = true;
				}
				if let Some(host) = served_host {
					hosts.insert(host.clone());
				} else {
					unknown_host = true;
				}
			}
		}
	}
	RouteView {
		route: route.route,
		provider_contract: route.provider_contract,
		model: route.model,
		hosts: route.hosts,
		effective_threshold: route.effective_threshold,
		answers: route.answers,
		served_models: models.into_iter().collect(),
		served_hosts: hosts.into_iter().collect(),
		includes_unknown_model: unknown_model,
		includes_unknown_host: unknown_host,
	}
}
