//! GCIP sign-in of Cloud and Dedicated: GCIP Tenants, Tenant Bindings,
//! Sign-in Domains and MFA Requirements (ADR 0032). Community sees only the
//! issuers it admits and the Tenant each one is restricted to.
pub mod admission;
pub mod settings;
mod views;

pub use settings::{AuthHelper, GcipConfig, ManagedGcipSource};

use aidash_application::ports::authorization::dashboard::{AccountStatus, Issuers};
use aidash_gcp::gcip::{Services, issuer, tenant};
use aidash_server::authorization::sign_in::{self, LoginDiscovery, LoginQuery, SignInProvider};
use aidash_server::config::SessionConfig;
use aidash_server::federation::Federation;
use aidash_server::{Error, Result};
use async_trait::async_trait;
use http::HeaderMap;
use reinhardt::Response;
use serde::Serialize;
use std::{collections::BTreeMap, sync::Arc};

/// GCIP Tenants with a Tenant Binding, whose External Identities may be
/// mapped only into their bound Tenant. Any other GCIP Tenant admits nobody.
pub struct Pools {
	project: String,
	bindings: BTreeMap<String, String>,
}

impl Issuers for Pools {
	fn admitted(&self) -> Vec<String> {
		self.bindings
			.keys()
			.map(|pool| issuer(&self.project, pool))
			.collect()
	}
	fn admits(&self, issuer: &str) -> bool {
		self.restriction(issuer).is_some()
	}
	fn restriction(&self, issuer: &str) -> Option<String> {
		self.bindings.get(tenant(&self.project, issuer)?).cloned()
	}
	fn restricted_issuer(&self, tenant: &str) -> Option<String> {
		self.bindings
			.iter()
			.find_map(|(pool, bound)| (bound == tenant).then(|| issuer(&self.project, pool)))
	}
	fn retires_refused(&self) -> bool {
		true
	}
}

/// Browser sign-in through GCIP.
pub struct Gcip {
	config: GcipConfig,
	services: Services,
}

#[derive(Serialize)]
struct ClientConfig<'a> {
	project_id: &'a str,
	api_key: &'a str,
	auth_domain: String,
	tenant_id: &'a str,
	providers: Vec<String>,
	password_sign_up: bool,
	/// A hint for the sign-in page; GCIP enforces the MFA Requirement.
	mfa_mandatory: bool,
}

impl Gcip {
	pub fn new(config: &GcipConfig, services: Services) -> Result<Self> {
		Ok(Self {
			config: config.normalized()?,
			services,
		})
	}

	/// `[dashboard.gcip]`, with Google's token keys and account status API.
	pub fn from_settings(value: &serde_json::Value) -> Result<Self> {
		let config: GcipConfig = serde_json::from_value(value.clone())
			.map_err(|error| Error::Invalid(format!("invalid dashboard.gcip: {error}")))?;
		let client = reqwest::Client::builder()
			.timeout(std::time::Duration::from_secs(120))
			.connect_timeout(std::time::Duration::from_secs(10))
			.redirect(reqwest::redirect::Policy::none())
			.build()?;
		let services = Services::new(config.project_id.clone(), client);
		Self::new(&config, services)
	}

	fn callback(&self) -> String {
		format!("{}/auth/gcip/exchange", self.config.public_origin)
	}

	/// The sign-in methods a GCIP Tenant offers.
	fn providers(&self, tenant: &str) -> Vec<String> {
		self.config
			.providers
			.get(tenant)
			.cloned()
			.unwrap_or_else(|| vec!["google.com".into(), "password".into()])
	}

	async fn begin(
		&self,
		f: &Federation,
		headers: &HeaderMap,
		destination: &str,
		tenant: &str,
	) -> Result<Response> {
		sign_in::begin(
			f,
			headers,
			destination,
			issuer(&self.config.project_id, tenant),
			self.callback(),
		)
		.await
	}

	/// The GCIP Tenant of the pending sign-in `state` names.
	async fn pending(
		&self,
		f: &Federation,
		headers: &HeaderMap,
		state: &str,
	) -> Result<(sign_in::Pending, String)> {
		let pending = sign_in::pending(f, headers, state, &self.callback()).await?;
		let tenant = tenant(&self.config.project_id, &pending.issuer)
			.ok_or(Error::Unauthorized)?
			.to_owned();
		Ok((pending, tenant))
	}

	/// The single response for every unknown sign-in input. It never depends on
	/// which Tenants or Sign-in Domains exist, and it creates no transaction.
	fn not_found(destination: &str) -> Result<Response> {
		sign_in::sign_in_page(&[("error", "not_found"), ("return_to", destination)])
	}
}

#[async_trait]
impl SignInProvider for Gcip {
	fn kind(&self) -> &'static str {
		"gcip"
	}

	fn session(&self) -> SessionConfig<'_> {
		SessionConfig {
			public_origin: &self.config.public_origin,
			session_absolute_seconds: self.config.session_absolute_seconds,
			session_idle_seconds: self.config.session_idle_seconds,
		}
	}

	fn issuers(&self) -> Arc<dyn Issuers> {
		Arc::new(Pools {
			project: self.config.project_id.clone(),
			bindings: self.config.tenant_bindings.clone(),
		})
	}

	fn status(&self) -> Arc<dyn AccountStatus> {
		self.services.status.clone()
	}

	/// `GET /auth/login?org=<Tenant name>` selects the GCIP Tenant explicitly.
	async fn login(
		&self,
		f: &Federation,
		headers: HeaderMap,
		query: LoginQuery,
	) -> Result<Response> {
		let destination = sign_in::return_path(query.return_to.as_deref())?;
		let Some(org) = query.org.as_deref() else {
			return sign_in::sign_in_page(&[("return_to", destination)]);
		};
		match self
			.config
			.tenant_bindings
			.iter()
			.find_map(|(pool, tenant)| (tenant == org).then_some(pool))
		{
			Some(pool) => self.begin(f, &headers, destination, pool).await,
			None => Self::not_found(destination),
		}
	}

	/// `POST /auth/login` routes a verified Sign-in Domain to its GCIP Tenant.
	/// Unmapped domains and malformed addresses get the same response as an
	/// unknown Tenant name, and nothing about the address is retained.
	async fn discover(
		&self,
		f: &Federation,
		headers: HeaderMap,
		input: LoginDiscovery,
	) -> Result<Response> {
		sign_in::require_origin(&headers, &self.config.public_origin)?;
		let destination = sign_in::return_path(input.return_to.as_deref())?;
		match settings::routed_gcip_tenant(&self.config.sign_in_domains, &input.email) {
			Some(pool) => self.begin(f, &headers, destination, pool).await,
			None => Self::not_found(destination),
		}
	}

	async fn transaction(
		&self,
		f: &Federation,
		headers: HeaderMap,
		state: String,
	) -> Result<Response> {
		let (_, tenant) = self.pending(f, &headers, &state).await?;
		if !self.config.tenant_bindings.contains_key(&tenant) {
			return Err(Error::Forbidden);
		}
		let client = ClientConfig {
			project_id: &self.config.project_id,
			api_key: &self.config.web_api_key,
			auth_domain: self.config.auth_domain(),
			tenant_id: &tenant,
			providers: self.providers(&tenant),
			password_sign_up: self.config.password_sign_up.contains(&tenant),
			mfa_mandatory: self.config.mfa_mandatory.contains(&tenant),
		};
		let mut response = Response::ok().with_json(&client)?;
		sign_in::no_store(&mut response);
		Ok(response)
	}

	async fn exchange(
		&self,
		f: &Federation,
		headers: HeaderMap,
		state: String,
		id_token: String,
	) -> Result<Response> {
		sign_in::require_origin(&headers, &self.config.public_origin)?;
		let (pending, tenant) = self.pending(f, &headers, &state).await?;
		if self.services.verifier.project != self.config.project_id {
			return Err(Error::Unauthorized);
		}
		let (signed_in, method) = self
			.services
			.verifier
			.verify(&id_token, &tenant, pending.started_at)
			.await?;
		// Verify the token before the shared authority checks the Binding and
		// durably disables an existing identity whose Binding has been removed.
		// The sign-in method is checked only for a bound GCIP Tenant; the
		// authority refuses any other.
		let bound = self.config.tenant_bindings.get(&tenant);
		if bound.is_some() && !self.providers(&tenant).contains(&method) {
			return Err(Error::Forbidden);
		}
		let identity = sign_in::admit(f, &signed_in).await?;
		admission::record(f, identity.id, &tenant, bound.map(String::as_str)).await?;
		sign_in::start_session(f, &headers, &state, &identity, signed_in.auth_time).await
	}
}

/// The routes of the GCIP sign-in page.
pub fn routes() -> reinhardt::UnifiedRouter {
	views::routes()
}
