//! Browser-bound desktop handoffs and revocable, rotating native credentials.
//! Business authorization remains in the dashboard identity/session path.
use super::*;
use axum::{
	http::StatusCode,
	response::{Html, IntoResponse, Response},
};
use tower_http::cors::CorsLayer;

const ACCESS_PREFIX: &str = "aidash_desktop_";
const REFRESH_PREFIX: &str = "aidash_refresh_";
const SESSION_COLUMNS: [&str; 11] = [
	"id",
	"identity_id",
	"csrf_hash",
	"last_activity_at",
	"expires_at",
	"revoked_at",
	"desktop",
	"desktop_idle_seconds",
	"access_expires_at",
	"created_at",
	"provider_sid",
];

struct Policy {
	access: i64,
	idle: i64,
	absolute: i64,
}
impl Policy {
	fn load() -> Result<Self> {
		fn seconds(key: &str, default: i64, max: i64) -> Result<i64> {
			let value = std::env::var(key)
				.ok()
				.map(|v| v.parse::<i64>())
				.transpose()
				.map_err(|_| Error::Invalid(format!("invalid {key}")))?
				.unwrap_or(default);
			if !(60..=max).contains(&value) {
				return Err(Error::Invalid(format!("invalid {key}")));
			}
			Ok(value)
		}
		let policy = Self {
			access: seconds("AIDASH_DESKTOP_ACCESS_SECONDS", 300, 900)?,
			idle: seconds("AIDASH_DESKTOP_IDLE_SECONDS", 2_592_000, 31_536_000)?,
			absolute: seconds("AIDASH_DESKTOP_ABSOLUTE_SECONDS", 7_776_000, 31_536_000)?,
		};
		if policy.idle > policy.absolute {
			return Err(Error::Invalid(
				"desktop idle lifetime exceeds absolute lifetime".into(),
			));
		}
		Ok(policy)
	}
}

pub(crate) fn cors() -> CorsLayer {
	// These origins identify bundled UI, not trusted remote websites. Credentials
	// are explicit Bearer headers; cross-origin browser cookies are never enabled.
	CorsLayer::new()
		.allow_origin(
			[
				"tauri://localhost",
				"http://tauri.localhost",
				"http://127.0.0.1:1420",
			]
			.map(|v| v.parse::<header::HeaderValue>().unwrap()),
		)
		.allow_methods([
			Method::GET,
			Method::HEAD,
			Method::POST,
			Method::PUT,
			Method::PATCH,
			Method::DELETE,
			Method::OPTIONS,
		])
		.allow_headers([
			header::AUTHORIZATION,
			header::CONTENT_TYPE,
			header::HeaderName::from_static("x-aidash-context"),
			header::HeaderName::from_static("last-event-id"),
			header::HeaderName::from_static("idempotency-key"),
		])
		.expose_headers([
			header::HeaderName::from_static("x-aidash-event-cursor"),
			header::HeaderName::from_static("x-aidash-next-offset"),
		])
}

pub(crate) fn access_token(headers: &HeaderMap) -> Option<&str> {
	let token = headers
		.get(header::AUTHORIZATION)?
		.to_str()
		.ok()?
		.strip_prefix("Bearer ")?;
	token.starts_with(ACCESS_PREFIX).then_some(token)
}

fn valid_secret(value: &str) -> bool {
	(43..=128).contains(&value.len())
		&& value
			.bytes()
			.all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
}
fn callback_url(value: &str) -> Result<reqwest::Url> {
	let url = reqwest::Url::parse(value)
		.map_err(|_| Error::Invalid("invalid desktop callback".into()))?;
	if url.scheme() != "http"
		|| url.host_str() != Some("127.0.0.1")
		|| url.port().is_none()
		|| url.username() != ""
		|| url.password().is_some()
		|| url.query().is_some()
		|| url.fragment().is_some()
		|| url.path() != "/callback"
	{
		return Err(Error::Invalid(
			"desktop callback must use an explicit loopback port and /callback".into(),
		));
	}
	Ok(url)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Start {
	redirect_uri: String,
	state: String,
	code_challenge: String,
}
#[derive(Serialize)]
struct Started {
	authorization_url: String,
}
#[derive(FromRow)]
struct Handoff {
	id: Uuid,
	state: String,
	redirect_uri: String,
	origin: String,
	browser_session_id: Option<Uuid>,
	code_hash: Option<Vec<u8>>,
	expires_at: DateTime<Utc>,
}
async fn start(State(f): State<Federation>, Json(input): Json<Start>) -> Result<Json<Started>> {
	let config = required_config(&f)?;
	callback_url(&input.redirect_uri)?;
	if !valid_secret(&input.state)
		|| input.code_challenge.len() != 43
		|| URL_SAFE_NO_PAD
			.decode(&input.code_challenge)
			.map_or(true, |v| v.len() != 32)
	{
		return Err(Error::Invalid(
			"invalid desktop state or S256 challenge".into(),
		));
	}
	let mut tx = f.store.pool.begin().await?;
	// Serialize bounded admission across replicas using a transaction-owned lock.
	let lock = Query::select()
		.expr(Expr::cust("pg_advisory_xact_lock(714234281)"))
		.to_string(PostgresQueryBuilder);
	sqlx::query(&lock).execute(&mut *tx).await?;
	let prune = Query::delete()
		.from_table(table("desktop_handoffs"))
		.and_where(Expr::col(table("expires_at")).lt(Expr::cust("clock_timestamp()")))
		.to_string(PostgresQueryBuilder);
	sqlx::query(&prune).execute(&mut *tx).await?;
	let count = Query::select()
		.expr(Expr::cust("count(*)"))
		.from(table("desktop_handoffs"))
		.to_string(PostgresQueryBuilder);
	let count: i64 = sqlx::query_scalar(&count).fetch_one(&mut *tx).await?;
	if count >= MAX_PENDING_LOGIN_TRANSACTIONS {
		return Err(Error::RateLimited);
	}
	let id = Uuid::new_v4();
	let insert = Query::insert()
		.into_table(table("desktop_handoffs"))
		.columns(
			[
				"id",
				"state",
				"challenge",
				"redirect_uri",
				"origin",
				"expires_at",
			]
			.map(table),
		)
		.values_panic([
			Expr::cust("$1"),
			Expr::cust("$2"),
			Expr::cust("$3"),
			Expr::cust("$4"),
			Expr::cust("$5"),
			Expr::cust("clock_timestamp()+interval '5 minutes'"),
		])
		.to_string(PostgresQueryBuilder);
	sqlx::query(&insert)
		.bind(id)
		.bind(input.state)
		.bind(input.code_challenge)
		.bind(input.redirect_uri)
		.bind(&config.public_origin)
		.execute(&mut *tx)
		.await?;
	tx.commit().await?;
	Ok(Json(Started {
		authorization_url: format!(
			"{}/auth/desktop/authorize?request={id}",
			config.public_origin
		),
	}))
}
#[derive(Deserialize)]
struct AuthorizationRequest {
	request: Uuid,
}
fn handoff_query(id: Uuid) -> String {
	Query::select()
		.columns(
			[
				"id",
				"state",
				"challenge",
				"redirect_uri",
				"origin",
				"browser_session_id",
				"code_hash",
				"expires_at",
			]
			.map(table),
		)
		.from(table("desktop_handoffs"))
		.and_where(Expr::col(table("id")).eq(id))
		.to_string(PostgresQueryBuilder)
}
async fn authorize(
	State(f): State<Federation>,
	jar: CookieJar,
	QueryParams(input): QueryParams<AuthorizationRequest>,
) -> Result<Response> {
	let config = required_config(&f)?;
	let handoff: Handoff = sqlx::query_as(&handoff_query(input.request))
		.fetch_optional(&f.store.pool)
		.await?
		.ok_or(Error::Unauthorized)?;
	if handoff.expires_at <= Utc::now()
		|| handoff.origin != config.public_origin
		|| handoff.code_hash.is_some()
	{
		return Err(Error::Unauthorized);
	}
	let callback_origin = callback_url(&handoff.redirect_uri)?
		.origin()
		.ascii_serialization();
	let session = match session_from_jar(&f, &jar).await {
		Ok(session) => session,
		Err(Error::Unauthorized) => {
			return Ok(Redirect::to(&format!(
				"/auth/login?return_to=%2Fauth%2Fdesktop%2Fauthorize%3Frequest%3D{}",
				input.request
			))
			.into_response());
		}
		Err(error) => return Err(error),
	};
	let csrf = jar.get(CSRF_COOKIE).ok_or(Error::Unauthorized)?.value();
	if digest(csrf) != session.csrf_hash {
		return Err(Error::Unauthorized);
	}
	let bind = Query::update()
		.table(table("desktop_handoffs"))
		.value(table("browser_session_id"), session.id)
		.and_where(Expr::col(table("id")).eq(handoff.id))
		.and_where(
			Expr::col(table("browser_session_id"))
				.is_null()
				.or(Expr::col(table("browser_session_id")).eq(session.id)),
		)
		.to_string(PostgresQueryBuilder);
	if sqlx::query(&bind)
		.execute(&f.store.pool)
		.await?
		.rows_affected()
		!= 1
	{
		return Err(Error::Unauthorized);
	}
	// Preserve Origin on the same-origin form POST; no-referrer makes it null.
	// Only server-generated UUID/CSRF and an HTML-escaped origin are interpolated.
	let html = format!(
		"<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><meta name=\"referrer\" content=\"same-origin\"><title>Aidash Desktop sign-in</title><h1>Sign in to Aidash Desktop</h1><p>Allow the desktop app on this computer to access {} using your current Aidash identity?</p><form method=\"post\" action=\"/auth/desktop/authorize\"><input type=\"hidden\" name=\"request\" value=\"{}\"><input type=\"hidden\" name=\"csrf\" value=\"{}\"><button type=\"submit\">Continue to Aidash Desktop</button></form><p>Close this page to cancel.</p></html>",
		escape(&config.public_origin),
		handoff.id,
		escape(csrf)
	);
	Ok((
		[(
			header::CONTENT_SECURITY_POLICY,
			// Chromium checks the form's redirect destination as well as its POST.
			format!(
				"default-src 'none'; form-action 'self' {callback_origin}; frame-ancestors 'none'; base-uri 'none'"
			),
		)],
		Html(html),
	)
		.into_response())
}
fn escape(value: &str) -> String {
	value
		.replace('&', "&amp;")
		.replace('<', "&lt;")
		.replace('>', "&gt;")
		.replace('"', "&quot;")
		.replace('\'', "&#39;")
}
#[derive(Deserialize)]
struct Consent {
	request: Uuid,
	csrf: String,
}
async fn consent(
	State(f): State<Federation>,
	headers: HeaderMap,
	jar: CookieJar,
	Form(input): Form<Consent>,
) -> Result<Redirect> {
	let config = required_config(&f)?;
	let session = session_from_jar(&f, &jar).await?;
	if headers.get(header::ORIGIN).and_then(|v| v.to_str().ok())
		!= Some(config.public_origin.as_str())
		|| digest(&input.csrf) != session.csrf_hash
	{
		return Err(Error::Forbidden);
	}
	let mut tx = f.store.pool.begin().await?;
	let code = random_secret();
	let update = Query::update()
		.table(table("desktop_handoffs"))
		.value(table("code_hash"), digest(&code))
		.value(
			table("expires_at"),
			Expr::cust("clock_timestamp()+interval '60 seconds'"),
		)
		.and_where(Expr::col(table("id")).eq(input.request))
		.and_where(Expr::col(table("browser_session_id")).eq(session.id))
		.and_where(Expr::col(table("code_hash")).is_null())
		.and_where(Expr::col(table("expires_at")).gt(Expr::cust("clock_timestamp()")))
		.and_where(Expr::col(table("origin")).eq(&config.public_origin))
		.returning_all()
		.to_string(PostgresQueryBuilder);
	let handoff: Handoff = sqlx::query_as(&update)
		.fetch_optional(&mut *tx)
		.await?
		.ok_or(Error::Unauthorized)?;
	let mut callback = callback_url(&handoff.redirect_uri)?;
	callback
		.query_pairs_mut()
		.append_pair("code", &code)
		.append_pair("state", &handoff.state);
	tx.commit().await?;
	Ok(Redirect::to(callback.as_str()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Exchange {
	code: String,
	state: String,
	verifier: String,
	redirect_uri: String,
}
#[derive(Serialize)]
struct Tokens {
	access_token: String,
	refresh_token: String,
	expires_in: i64,
}
async fn exchange(
	State(f): State<Federation>,
	Json(input): Json<Exchange>,
) -> Result<Json<Tokens>> {
	let config = required_config(&f)?;
	if !valid_secret(&input.code) || !valid_secret(&input.verifier) {
		return Err(Error::Unauthorized);
	}
	let policy = Policy::load()?;
	let proof = Condition::all()
		.add(Expr::col(table("code_hash")).eq(digest(&input.code)))
		.add(Expr::col(table("state")).eq(&input.state))
		.add(
			Expr::col(table("challenge"))
				.eq(URL_SAFE_NO_PAD.encode(Sha256::digest(&input.verifier))),
		)
		.add(Expr::col(table("redirect_uri")).eq(&input.redirect_uri))
		.add(Expr::col(table("origin")).eq(&config.public_origin))
		.add(Expr::col(table("expires_at")).gt(Expr::cust("clock_timestamp()")));
	let lookup = Query::select()
		.column(table("browser_session_id"))
		.from(table("desktop_handoffs"))
		.cond_where(proof.clone())
		.to_string(PostgresQueryBuilder);
	let browser_id = sqlx::query_scalar::<_, Option<Uuid>>(&lookup)
		.fetch_optional(&f.store.pool)
		.await?
		.flatten()
		.ok_or(Error::Unauthorized)?;
	let query = Query::select()
		.columns(SESSION_COLUMNS.map(table))
		.from(table("dashboard_sessions"))
		.and_where(Expr::col(table("id")).eq(browser_id))
		.to_string(PostgresQueryBuilder);
	let browser: BrowserSession = sqlx::query_as(&query)
		.fetch_optional(&f.store.pool)
		.await?
		.ok_or(Error::Unauthorized)?;
	if browser.desktop
		|| browser.revoked_at.is_some()
		|| browser.expires_at <= Utc::now()
		|| browser.last_activity_at <= Utc::now() - Duration::seconds(config.session_idle_seconds)
	{
		return Err(Error::Unauthorized);
	}
	let identity_id = browser.identity_id;
	// Provider validation may perform pooled reads and writes. Finish it before
	// reserving a transaction connection, even when the pool has only one slot.
	validate_session_identity(&f, browser).await?;
	let mut tx = f.store.pool.begin().await?;
	lock_identity(&mut tx, identity_id, LockType::Share).await?;
	// Consume the same proof atomically after preflight; concurrent exchanges
	// still issue at most one session and rejection rolls back consumption.
	let take = Query::delete()
		.from_table(table("desktop_handoffs"))
		.cond_where(proof)
		.returning_all()
		.to_string(PostgresQueryBuilder);
	let handoff: Handoff = sqlx::query_as(&take)
		.fetch_optional(&mut *tx)
		.await?
		.ok_or(Error::Unauthorized)?;
	if handoff.browser_session_id != Some(browser_id) {
		return Err(Error::Unauthorized);
	}
	let query = Query::select()
		.columns(SESSION_COLUMNS.map(table))
		.from(table("dashboard_sessions"))
		.and_where(Expr::col(table("id")).eq(browser_id))
		.lock(LockType::Share)
		.to_string(PostgresQueryBuilder);
	let browser: BrowserSession = sqlx::query_as(&query)
		.fetch_optional(&mut *tx)
		.await?
		.ok_or(Error::Unauthorized)?;
	if browser.desktop
		|| browser.identity_id != identity_id
		|| browser.revoked_at.is_some()
		|| browser.expires_at <= Utc::now()
		|| browser.last_activity_at <= Utc::now() - Duration::seconds(config.session_idle_seconds)
	{
		return Err(Error::Unauthorized);
	}
	let session_id = Uuid::new_v4();
	let tokens = Tokens {
		access_token: format!("{ACCESS_PREFIX}{}", random_secret()),
		refresh_token: format!("{REFRESH_PREFIX}{}", random_secret()),
		expires_in: policy.access,
	};
	let insert = Query::insert()
		.into_table(table("dashboard_sessions"))
		.columns(
			[
				"id",
				"identity_id",
				"token_hash",
				"csrf_hash",
				"created_at",
				"last_activity_at",
				"expires_at",
				"desktop",
				"desktop_idle_seconds",
				"access_expires_at",
				"provider_sid",
			]
			.map(table),
		)
		.values_panic([
			Expr::cust("$1"),
			Expr::cust("$2"),
			Expr::cust("$3"),
			Expr::cust("$4"),
			Expr::cust("clock_timestamp()"),
			Expr::cust("clock_timestamp()"),
			Expr::cust("clock_timestamp()+make_interval(secs => $5)"),
			Expr::val(true).into(),
			Expr::cust("$6"),
			Expr::cust("clock_timestamp()+make_interval(secs => $7)"),
			Expr::cust("$8"),
		])
		.to_string(PostgresQueryBuilder);
	sqlx::query(&insert)
		.bind(session_id)
		.bind(identity_id)
		.bind(digest(&tokens.access_token))
		.bind(digest(&random_secret()))
		.bind(policy.absolute as f64)
		.bind(policy.idle)
		.bind(policy.access as f64)
		.bind(browser.provider_sid)
		.execute(&mut *tx)
		.await?;
	insert_refresh(&mut tx, session_id, &tokens.refresh_token).await?;
	tx.commit().await?;
	Ok(Json(tokens))
}
// Identity-before-session is the shared order with revocation. In particular,
// all-device logout must finish after a concurrent handoff or prevent its issue.
pub(super) async fn lock_identity(
	tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
	id: Uuid,
	lock: LockType,
) -> Result<()> {
	let query = Query::select()
		.columns(["issuer", "last_valid_at", "disabled_at"].map(table))
		.from(table("dashboard_identities"))
		.and_where(Expr::col(table("id")).eq(id))
		.lock(lock)
		.to_string(PostgresQueryBuilder);
	let validity = sqlx::query_as(&query).fetch_optional(&mut **tx).await?;
	identity::validate_dashboard_status(validity)
}
async fn insert_refresh(
	tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
	session: Uuid,
	token: &str,
) -> Result<()> {
	let insert = Query::insert()
		.into_table(table("desktop_refresh_credentials"))
		.columns(["token_hash", "session_id"].map(table))
		.values_panic([Expr::cust("$1"), Expr::cust("$2")])
		.to_string(PostgresQueryBuilder);
	sqlx::query(&insert)
		.bind(digest(token))
		.bind(session)
		.execute(&mut **tx)
		.await?;
	Ok(())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Renewal {
	refresh_token: String,
	next_token: String,
}
async fn refresh(State(f): State<Federation>, Json(input): Json<Renewal>) -> Result<Json<Tokens>> {
	required_config(&f)?;
	if !input.refresh_token.starts_with(REFRESH_PREFIX)
		|| !input.next_token.starts_with(REFRESH_PREFIX)
		|| !valid_secret(&input.next_token)
		|| !valid_secret(&input.refresh_token)
		|| input.refresh_token == input.next_token
	{
		return Err(Error::Unauthorized);
	}
	let policy = Policy::load()?;
	let lookup = Query::select()
		.column(table("session_id"))
		.from(table("desktop_refresh_credentials"))
		.and_where(Expr::col(table("token_hash")).eq(digest(&input.refresh_token)))
		.to_string(PostgresQueryBuilder);
	let id: Uuid = sqlx::query_scalar(&lookup)
		.fetch_optional(&f.store.pool)
		.await?
		.ok_or(Error::Unauthorized)?;
	// Preflight and provider validation use the pool without holding a
	// transaction connection; locked revalidation follows in identity order.
	let preflight = Query::select()
		.columns(SESSION_COLUMNS.map(table))
		.from(table("dashboard_sessions"))
		.and_where(Expr::col(table("id")).eq(id))
		.to_string(PostgresQueryBuilder);
	let candidate: BrowserSession = sqlx::query_as(&preflight)
		.fetch_optional(&f.store.pool)
		.await?
		.ok_or(Error::Unauthorized)?;
	let identity_id = candidate.identity_id;
	validate_session_identity(&f, candidate).await?;
	let mut tx = f.store.pool.begin().await?;
	lock_identity(&mut tx, identity_id, LockType::Share).await?;
	let query = Query::select()
		.columns(SESSION_COLUMNS.map(table))
		.from(table("dashboard_sessions"))
		.and_where(Expr::col(table("id")).eq(id))
		.lock(LockType::Update)
		.to_string(PostgresQueryBuilder);
	let session: BrowserSession = sqlx::query_as(&query)
		.fetch_optional(&mut *tx)
		.await?
		.ok_or(Error::Unauthorized)?;
	if session.identity_id != identity_id {
		return Err(Error::Unauthorized);
	}
	valid_session(&session, false)?;
	let lookup = Query::select()
		.column(table("next_hash"))
		.from(table("desktop_refresh_credentials"))
		.and_where(Expr::col(table("token_hash")).eq(digest(&input.refresh_token)))
		.and_where(Expr::col(table("session_id")).eq(id))
		.to_string(PostgresQueryBuilder);
	let next: Option<Vec<u8>> = sqlx::query_scalar(&lookup)
		.fetch_optional(&mut *tx)
		.await?
		.ok_or(Error::Unauthorized)?;
	if let Some(next) = next {
		let current = Query::select()
			.column(table("session_id"))
			.from(table("desktop_refresh_credentials"))
			.and_where(Expr::col(table("token_hash")).eq(digest(&input.next_token)))
			.and_where(Expr::col(table("next_hash")).is_null())
			.and_where(Expr::col(table("session_id")).eq(id))
			.to_string(PostgresQueryBuilder);
		let current: Option<Uuid> = sqlx::query_scalar(&current)
			.fetch_optional(&mut *tx)
			.await?;
		// Recover a lost response only with BOTH secrets from the same prepared
		// rotation. Reuse with any other successor revokes the complete family.
		if next != digest(&input.next_token) || current.is_none() {
			revoke(&mut tx, id).await?;
			tx.commit().await?;
			return Err(Error::Unauthorized);
		}
	} else {
		insert_refresh(&mut tx, id, &input.next_token).await?;
		let update = Query::update()
			.table(table("desktop_refresh_credentials"))
			.value(table("next_hash"), digest(&input.next_token))
			.and_where(Expr::col(table("token_hash")).eq(digest(&input.refresh_token)))
			.to_string(PostgresQueryBuilder);
		sqlx::query(&update).execute(&mut *tx).await?;
	}
	let tokens = Tokens {
		access_token: format!("{ACCESS_PREFIX}{}", random_secret()),
		refresh_token: input.next_token,
		expires_in: policy.access,
	};
	let update = Query::update()
		.table(table("dashboard_sessions"))
		.value(table("token_hash"), digest(&tokens.access_token))
		.value(
			table("access_expires_at"),
			Expr::cust("clock_timestamp()+make_interval(secs => $1)"),
		)
		.and_where(Expr::col(table("id")).eq(id))
		.to_string(PostgresQueryBuilder);
	sqlx::query(&update)
		.bind(policy.access as f64)
		.execute(&mut *tx)
		.await?;
	tx.commit().await?;
	Ok(Json(tokens))
}
async fn revoke(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, id: Uuid) -> Result<()> {
	let query = Query::update()
		.table(table("dashboard_sessions"))
		.value(table("revoked_at"), Expr::cust("clock_timestamp()"))
		.and_where(Expr::col(table("id")).eq(id))
		.to_string(PostgresQueryBuilder);
	sqlx::query(&query).execute(&mut **tx).await?;
	Ok(())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Revocation {
	refresh_token: String,
}
async fn revoke_refresh(
	State(f): State<Federation>,
	Json(input): Json<Revocation>,
) -> Result<StatusCode> {
	let lookup = Query::select()
		.column(table("session_id"))
		.from(table("desktop_refresh_credentials"))
		.and_where(Expr::col(table("token_hash")).eq(digest(&input.refresh_token)))
		.to_string(PostgresQueryBuilder);
	let mut tx = f.store.pool.begin().await?;
	if let Some(id) = sqlx::query_scalar(&lookup).fetch_optional(&mut *tx).await? {
		revoke(&mut tx, id).await?;
	}
	tx.commit().await?;
	Ok(StatusCode::NO_CONTENT)
}
fn valid_session(session: &BrowserSession, access: bool) -> Result<()> {
	if !session.desktop
		|| session.revoked_at.is_some()
		|| session.expires_at <= Utc::now()
		|| session.last_activity_at
			<= Utc::now()
				- Duration::seconds(session.desktop_idle_seconds.ok_or(Error::Unauthorized)?)
		|| (access && session.access_expires_at.is_none_or(|v| v <= Utc::now()))
	{
		return Err(Error::Unauthorized);
	}
	Ok(())
}
pub(crate) async fn session_record(f: &Federation, token: &str) -> Result<BrowserSession> {
	let query = Query::select()
		.columns(SESSION_COLUMNS.map(table))
		.from(table("dashboard_sessions"))
		.and_where(Expr::col(table("token_hash")).eq(digest(token)))
		.to_string(PostgresQueryBuilder);
	let session: BrowserSession = sqlx::query_as(&query)
		.fetch_optional(&f.store.pool)
		.await?
		.ok_or(Error::Unauthorized)?;
	valid_session(&session, true)?;
	Ok(session)
}
pub(super) fn routes() -> Router<Federation> {
	Router::new()
		.route("/start", post(start))
		.route("/authorize", get(authorize).post(consent))
		.route("/exchange", post(exchange))
		.route("/refresh", post(refresh))
		.route("/revoke", post(revoke_refresh))
}

#[cfg(test)]
mod tests {
	use super::*;
	#[rstest::rstest]
	#[case("https://127.0.0.1:3000/callback")]
	#[case("http://localhost:3000/callback")]
	#[case("http://127.0.0.1/callback")]
	#[case("http://127.0.0.1:3000/callback?token=leak")]
	#[case("http://127.0.0.1:3000/elsewhere")]
	#[case("http://user@127.0.0.1:3000/callback")]
	fn rejects_unsafe_callback(#[case] value: &str) {
		assert!(callback_url(value).is_err());
	}
	#[test]
	fn accepts_only_bounded_loopback_callback() {
		assert!(callback_url("http://127.0.0.1:43157/callback").is_ok());
	}

	#[test]
	fn desktop_policy_enforces_lifetime_bounds() {
		const EXPECTED: &str = "AIDASH_DESKTOP_POLICY_TEST_EXPECTED";
		if let Ok(expected) = std::env::var(EXPECTED) {
			let actual = Policy::load().map_or_else(
				|error| error.to_string(),
				|policy| format!("{},{},{}", policy.access, policy.idle, policy.absolute),
			);
			assert_eq!(actual, expected);
			return;
		}
		let cases: &[(&[(&str, &str)], &str)] = &[
			(&[], "300,2592000,7776000"),
			(
				&[
					("AIDASH_DESKTOP_ACCESS_SECONDS", "60"),
					("AIDASH_DESKTOP_IDLE_SECONDS", "60"),
					("AIDASH_DESKTOP_ABSOLUTE_SECONDS", "60"),
				],
				"60,60,60",
			),
			(
				&[
					("AIDASH_DESKTOP_ACCESS_SECONDS", "900"),
					("AIDASH_DESKTOP_IDLE_SECONDS", "31536000"),
					("AIDASH_DESKTOP_ABSOLUTE_SECONDS", "31536000"),
				],
				"900,31536000,31536000",
			),
			(
				&[("AIDASH_DESKTOP_ACCESS_SECONDS", "59")],
				"invalid AIDASH_DESKTOP_ACCESS_SECONDS",
			),
			(
				&[("AIDASH_DESKTOP_ACCESS_SECONDS", "901")],
				"invalid AIDASH_DESKTOP_ACCESS_SECONDS",
			),
			(
				&[("AIDASH_DESKTOP_ACCESS_SECONDS", "not-a-number")],
				"invalid AIDASH_DESKTOP_ACCESS_SECONDS",
			),
			(
				&[("AIDASH_DESKTOP_IDLE_SECONDS", "59")],
				"invalid AIDASH_DESKTOP_IDLE_SECONDS",
			),
			(
				&[("AIDASH_DESKTOP_IDLE_SECONDS", "31536001")],
				"invalid AIDASH_DESKTOP_IDLE_SECONDS",
			),
			(
				&[("AIDASH_DESKTOP_ABSOLUTE_SECONDS", "59")],
				"invalid AIDASH_DESKTOP_ABSOLUTE_SECONDS",
			),
			(
				&[("AIDASH_DESKTOP_ABSOLUTE_SECONDS", "31536001")],
				"invalid AIDASH_DESKTOP_ABSOLUTE_SECONDS",
			),
			(
				&[
					("AIDASH_DESKTOP_IDLE_SECONDS", "120"),
					("AIDASH_DESKTOP_ABSOLUTE_SECONDS", "60"),
				],
				"desktop idle lifetime exceeds absolute lifetime",
			),
		];
		for (settings, expected) in cases {
			// Isolate each configuration without mutating the parent test environment.
			let output = std::process::Command::new(std::env::current_exe().unwrap())
				.args([
					"--exact",
					"dashboard_auth::desktop::tests::desktop_policy_enforces_lifetime_bounds",
					"--nocapture",
				])
				.env_remove("AIDASH_DESKTOP_ACCESS_SECONDS")
				.env_remove("AIDASH_DESKTOP_IDLE_SECONDS")
				.env_remove("AIDASH_DESKTOP_ABSOLUTE_SECONDS")
				.envs(settings.iter().copied())
				.env(EXPECTED, expected)
				.output()
				.unwrap();
			let stdout = String::from_utf8_lossy(&output.stdout);
			let stderr = String::from_utf8_lossy(&output.stderr);
			assert!(output.status.success(), "{settings:?}: {stdout}{stderr}");
			assert!(stdout.contains("1 passed"));
		}
	}
}
