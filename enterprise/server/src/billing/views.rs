//! The billing endpoints. Billing Account endpoints are Cloud only; the usage
//! report serves Cloud and Dedicated, to Deployment Administrators.
use super::service::{Consented, Party, csv};
use aidash_domain::identity::tenancy::BILLING_ATTACH;
use aidash_server::authorization::services::boundary::AccessBoundary;
use aidash_server::dashboard_auth::{authorize_mapping, session_identity};
use aidash_server::federation::Federation;
use aidash_server::http::json::Json;
use aidash_server::{Error, Result};
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use reinhardt::http::ViewResult;
use reinhardt::{Path, Query, Request, Response, StatusCode, UnifiedRouter, get, post};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AccountInput {
	pub label: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TierInput {
	pub tier: String,
}

/// The owner's side of an attachment names the Tenant.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OwnerConsent {
	pub tenant: String,
}

/// The Tenant Administrator's side names the Billing Account.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AdministratorConsent {
	pub account: Uuid,
}

#[derive(Serialize, JsonSchema)]
pub struct ConsentResult {
	pub state: Consented,
}

#[derive(Serialize, JsonSchema)]
pub struct CreatedAccount {
	pub id: Uuid,
	pub label: String,
}

#[derive(Deserialize, JsonSchema)]
pub struct PeriodQuery {
	/// First day, default the first of the current month.
	pub from: Option<NaiveDate>,
	/// Last day, default today.
	pub to: Option<NaiveDate>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AccountPeriodQuery {
	pub starts_at: DateTime<Utc>,
	pub ends_at: DateTime<Utc>,
}

#[derive(Serialize, JsonSchema)]
pub struct UsageReport {
	pub from: NaiveDate,
	pub to: NaiveDate,
	pub rows: Vec<super::store::UsageRow>,
}

fn period(query: PeriodQuery) -> Result<(NaiveDate, NaiveDate)> {
	let today = Utc::now().date_naive();
	let to = query.to.unwrap_or(today);
	let from = query
		.from
		.or_else(|| NaiveDate::from_ymd_opt(to.year(), to.month(), 1))
		.ok_or_else(|| Error::Invalid("invalid period".into()))?;
	Ok((from, to))
}

#[get("/api/billing/tiers", name = "billing-tiers", auth = "protected")]
pub async fn tiers(#[inject] runtime: Federation, request: Request) -> ViewResult<Response> {
	aidash_server::http::json(
		async {
			session_identity(&runtime, &request.headers, &request.method).await?;
			Ok(super::billing(&runtime)?.tiers())
		}
		.await,
	)
}

#[post(
	"/api/billing/accounts",
	name = "billing-accounts-create",
	auth = "protected"
)]
pub async fn create_account(
	#[inject] runtime: Federation,
	request: Request,
	Json(input): Json<AccountInput>,
) -> ViewResult<Response> {
	aidash_server::http::json_status(
		async {
			let owner = session_identity(&runtime, &request.headers, &request.method).await?;
			let account = super::billing(&runtime)?
				.create_account(owner, &input.label)
				.await?;
			Ok(CreatedAccount {
				id: account.id,
				label: account.label,
			})
		}
		.await,
		StatusCode::CREATED,
	)
}

#[get("/api/billing/accounts", name = "billing-accounts", auth = "protected")]
pub async fn accounts(#[inject] runtime: Federation, request: Request) -> ViewResult<Response> {
	aidash_server::http::json(
		async {
			let owner = session_identity(&runtime, &request.headers, &request.method).await?;
			super::billing(&runtime)?.accounts(owner).await
		}
		.await,
	)
}

#[get(
	"/api/billing/accounts/{account}/usage",
	name = "billing-accounts-usage",
	auth = "protected"
)]
pub async fn account_usage(
	#[inject] runtime: Federation,
	request: Request,
	Path(account): Path<Uuid>,
	Query(period): Query<AccountPeriodQuery>,
) -> ViewResult<Response> {
	aidash_server::http::json(
		async {
			let owner = session_identity(&runtime, &request.headers, &request.method).await?;
			super::billing(&runtime)?
				.account_usage(owner, account, period.starts_at, period.ends_at)
				.await
		}
		.await,
	)
}

#[post(
	"/api/billing/accounts/{account}/tier",
	name = "billing-accounts-tier",
	auth = "protected"
)]
pub async fn change_tier(
	#[inject] runtime: Federation,
	request: Request,
	Path(account): Path<Uuid>,
	Json(input): Json<TierInput>,
) -> ViewResult<Response> {
	aidash_server::http::json(
		async {
			let owner = session_identity(&runtime, &request.headers, &request.method).await?;
			super::billing(&runtime)?
				.change_tier(owner, account, &input.tier)
				.await?;
			Ok(serde_json::json!({"tier": input.tier}))
		}
		.await,
	)
}

/// The receiving account's owner consents to covering a Tenant.
#[post(
	"/api/billing/accounts/{account}/attachments",
	name = "billing-accounts-attach",
	auth = "protected"
)]
pub async fn owner_consent(
	#[inject] runtime: Federation,
	request: Request,
	Path(account): Path<Uuid>,
	Json(input): Json<OwnerConsent>,
) -> ViewResult<Response> {
	aidash_server::http::json(
		async {
			aidash_domain::policy::identifier(&input.tenant)?;
			let owner = session_identity(&runtime, &request.headers, &request.method).await?;
			let state = super::billing(&runtime)?
				.consent(Party::AccountOwner, owner, &input.tenant, account)
				.await?;
			Ok(ConsentResult { state })
		}
		.await,
	)
}

/// A Tenant Administrator, whom the Tenant's policy allows `billing.attach`,
/// consents to the Tenant's attachment to a Billing Account.
#[post(
	"/api/tenants/{tenant}/billing/attachment",
	name = "billing-tenant-attach",
	auth = "protected"
)]
pub async fn administrator_consent(
	#[inject] runtime: Federation,
	request: Request,
	Path(tenant): Path<String>,
	Json(input): Json<AdministratorConsent>,
) -> ViewResult<Response> {
	aidash_server::http::json(
		async {
			let administrator = authorize_mapping(
				&runtime,
				&request.headers,
				&request.method,
				&tenant,
				BILLING_ATTACH,
			)
			.await?;
			let state = super::billing(&runtime)?
				.consent(
					Party::TenantAdministrator,
					administrator,
					&tenant,
					input.account,
				)
				.await?;
			Ok(ConsentResult { state })
		}
		.await,
	)
}

#[get("/api/usage", name = "billing-usage", auth = "protected")]
pub async fn usage(
	#[inject] runtime: Federation,
	Query(query): Query<PeriodQuery>,
) -> ViewResult<Response> {
	aidash_server::http::json(
		async {
			let (from, to) = period(query)?;
			let rows = super::billing(&runtime)?.usage(from, to).await?;
			Ok(UsageReport { from, to, rows })
		}
		.await,
	)
}

#[get("/api/usage.csv", name = "billing-usage-csv", auth = "protected")]
pub async fn usage_csv(
	#[inject] runtime: Federation,
	Query(query): Query<PeriodQuery>,
) -> ViewResult<Response> {
	aidash_server::http::response(
		async {
			let (from, to) = period(query)?;
			let rows = super::billing(&runtime)?.usage(from, to).await?;
			Ok(Response::ok()
				.with_body(csv(&rows).into_bytes())
				.with_header("Content-Type", "text/csv; charset=utf-8")
				.with_header(
					"Content-Disposition",
					&format!("attachment; filename=\"usage-{from}-{to}.csv\""),
				))
		}
		.await,
	)
}

/// The usage report, for Cloud and Dedicated.
pub fn usage_routes() -> UnifiedRouter {
	UnifiedRouter::new().server(|server| {
		server
			.endpoint(usage)
			.with_route_middleware(AccessBoundary::deployment_administrator())
			.endpoint(usage_csv)
			.with_route_middleware(AccessBoundary::deployment_administrator())
	})
}

/// Billing Accounts, Tiers and attachments, for Cloud. Each authenticates its
/// caller itself, as Tenant creation does: an owner needs no Mapping.
pub fn account_routes() -> UnifiedRouter {
	UnifiedRouter::new().server(|server| {
		server
			.endpoint(tiers)
			.with_route_middleware(AccessBoundary::public())
			.endpoint(create_account)
			.with_route_middleware(AccessBoundary::public())
			.endpoint(accounts)
			.with_route_middleware(AccessBoundary::public())
			.endpoint(account_usage)
			.with_route_middleware(AccessBoundary::public())
			.endpoint(change_tier)
			.with_route_middleware(AccessBoundary::public())
			.endpoint(owner_consent)
			.with_route_middleware(AccessBoundary::public())
			.endpoint(administrator_consent)
			.with_route_middleware(AccessBoundary::public())
	})
}
