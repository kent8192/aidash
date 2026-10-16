//! Plan Connection endpoints (#159, ADR 0007, ADR 0031). The server runs Sign
//! in with ChatGPT: it keeps `state`, `nonce` and the PKCE verifier, and the
//! callback URL reaches it from a desktop relay or a paste. Only the owner's
//! own dashboard or desktop session may use these endpoints; a Subject
//! Credential never can.
use super::oidc::{self, BrowserOrigin};
use crate::apps::identity::models::plan_connections::{PlanAuthorization, PlanConnectionHost};
use crate::apps::identity::serializers::plan_connections::{
	Complete, Completed, Disconnect, Models, Start, Started, Status,
};
use crate::{Error, Result, authorization::identity::Actor, federation::Federation};
use aidash_application::plan_connections::{
	self as application, Disconnection, Owner, Pending, Service,
};
use aidash_domain::plan_connections::{self as domain, PASTE_PORT};
use aidash_domain::provider_credentials::Provider;
use chrono::{Duration, Utc};
use http::HeaderMap;
use reinhardt::{Response, injectable};
use secrecy::{ExposeSecret, SecretString};
use std::sync::Arc;

#[derive(Clone)]
pub struct PlanConnections {
	pub(crate) runtime: Federation,
}
#[injectable(scope = "request")]
pub async fn provide(#[inject] runtime: Federation) -> PlanConnections {
	PlanConnections { runtime }
}

fn hex_digest(value: &str) -> String {
	oidc::digest(value)
		.iter()
		.map(|b| format!("{b:02x}"))
		.collect()
}

/// The provider in the path: only Provider Catalog entries a Plan Connection
/// serves.
fn plan_provider(provider: &str) -> Result<Provider> {
	Provider::plan(provider).map_err(|_| Error::NotFound("Plan Connection provider".into()))
}

/// The owner is the subject of the caller's own Mapping in the path's Tenant,
/// acting through a dashboard or desktop session.
fn owner(actor: &Actor, origin: Option<&BrowserOrigin>, tenant: &str) -> Result<Owner> {
	let origin = origin.ok_or(Error::Forbidden)?;
	match actor {
		Actor::Subject(identity) if identity.tenant == tenant && origin.mapping_id.is_some() => {
			Ok(Owner {
				tenant: identity.tenant.clone(),
				subject: identity.subject.clone(),
			})
		}
		_ => Err(Error::Forbidden),
	}
}

/// Recorded on Plan Connection events: the External Identity that acted.
fn actor_label(origin: &BrowserOrigin) -> String {
	format!("oidc:{}", origin.identity_id)
}

impl PlanConnections {
	fn service(&self) -> Result<&Arc<Service>> {
		self.runtime.store.plan_connections.as_ref().ok_or_else(|| {
			Error::NotFound("Plan Connections are not enabled on this Deployment".into())
		})
	}
	/// New authorizations and model lists need the capability turned on.
	fn enabled(&self) -> Result<&Arc<Service>> {
		if !self.runtime.store.plan_connections_enabled {
			return Err(Error::NotFound(
				"Plan Connections are not enabled on this Deployment".into(),
			));
		}
		self.service()
	}

	pub(crate) async fn status(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		provider: String,
	) -> Result<Status> {
		plan_provider(&provider)?;
		let owner = owner(&actor, origin.as_ref(), &tenant)?;
		let connection = match &self.runtime.store.plan_connections {
			Some(service) => service.status(&owner).await?,
			None => None,
		};
		Ok(Status {
			enabled: self.runtime.store.plan_connections_enabled,
			connection,
		})
	}

	/// Starts one authorization for the owner's session. A desktop client
	/// names its loopback listener's port; otherwise the owner pastes the
	/// callback URL from a port nothing listens on.
	pub(crate) async fn start(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		headers: HeaderMap,
		tenant: String,
		provider: String,
		input: Start,
	) -> Result<Response> {
		let provider = plan_provider(&provider)?;
		let owner = owner(&actor, origin.as_ref(), &tenant)?;
		let service = self.enabled()?;
		let origin = origin.ok_or(Error::Forbidden)?;
		let context = headers
			.get("x-aidash-context")
			.and_then(|value| value.to_str().ok())
			.ok_or(Error::Forbidden)?
			.to_owned();
		// Refuses a shared subject before OpenAI issues anything.
		let saved_client = service.check(&owner).await?;
		let lease = self.runtime.store.orm_connection()?;
		let host = domain::host_id(PlanConnectionHost::current(lease.handle()).await?)?;
		let request = application::begin(
			&host,
			input.port.unwrap_or(PASTE_PORT),
			saved_client.as_deref(),
		)?;
		PlanAuthorization::build()
			.state_hash(hex_digest(request.state.expose_secret()))
			.session_id(origin.session.id)
			.context(context)
			.tenant(owner.tenant)
			.subject(owner.subject)
			.provider(provider.id().to_owned())
			.saved_client(saved_client)
			.nonce(request.nonce.expose_secret().to_owned())
			.pkce_verifier(request.verifier.expose_secret().to_owned())
			.redirect_uri(request.redirect_uri.clone())
			.expires_at(Utc::now() + Duration::minutes(application::LIFETIME_MINUTES))
			.finish()
			.reserve(lease.handle())
			.await?;
		let mut response = Response::ok().with_json(&Started {
			authorization_url: request.url,
			redirect_uri: request.redirect_uri,
		})?;
		oidc::no_store(&mut response);
		Ok(response)
	}

	/// Completes the pending authorization the callback URL's `state` names.
	/// The URL must come from the session that started it, for the same
	/// owner; anything else is refused before OpenAI is called.
	pub(crate) async fn complete(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		provider: String,
		input: Complete,
	) -> Result<Response> {
		let provider = plan_provider(&provider)?;
		let owner = owner(&actor, origin.as_ref(), &tenant)?;
		let service = self.enabled()?;
		let origin = origin.ok_or(Error::Forbidden)?;
		let state = callback_state(&input.callback_url)?;
		let lease = self.runtime.store.orm_connection()?;
		let pending = PlanAuthorization::consume(lease.handle(), hex_digest(&state)).await?;
		if pending.session_id != origin.session.id
			|| pending.tenant != owner.tenant
			|| pending.subject != owner.subject
			|| pending.provider != provider.id()
		{
			return Err(Error::Unauthorized);
		}
		let callback = domain::parse_callback(&input.callback_url, &pending.redirect_uri)?;
		let verifier = SecretString::from(pending.pkce_verifier);
		let outcome = service
			.complete(
				&owner,
				Pending {
					state: &state,
					verifier: &verifier,
					nonce: &pending.nonce,
					redirect_uri: &pending.redirect_uri,
					saved_client: pending.saved_client.as_deref(),
				},
				&callback,
				&actor_label(&origin),
			)
			.await?;
		let connection = service.status(&owner).await?;
		let mut response = Response::ok().with_json(&Completed {
			outcome,
			connection,
		})?;
		oidc::no_store(&mut response);
		Ok(response)
	}

	/// Revokes the grant at OpenAI and deletes Aidash's copy. Works after the
	/// capability is turned off, so owners can always end their grant here.
	pub(crate) async fn disconnect(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		provider: String,
		input: Disconnect,
	) -> Result<Disconnection> {
		plan_provider(&provider)?;
		let owner = owner(&actor, origin.as_ref(), &tenant)?;
		let origin = origin.ok_or(Error::Forbidden)?;
		Ok(self
			.service()?
			.disconnect(&owner, input.expected_revision, &actor_label(&origin))
			.await?)
	}

	/// The owner's ChatGPT model list, read with their own Plan Connection.
	pub(crate) async fn models(
		&self,
		actor: Actor,
		origin: Option<BrowserOrigin>,
		tenant: String,
		provider: String,
	) -> Result<Models> {
		plan_provider(&provider)?;
		let owner = owner(&actor, origin.as_ref(), &tenant)?;
		let service = self.enabled()?;
		Ok(Models {
			models: service.models(&owner).await?,
		})
	}
}

/// Ends the Plan Connection of a subject that no enabled Mapping of an
/// enabled External Identity reaches any more: its owner's Mapping was
/// disabled, including leaving the Tenant (ADR 0007). A remaining Mapping of
/// a disabled identity does not keep the connection: that identity never
/// connected it, and restoring it later must not hand it the owner's plan.
/// Identity Disablement alone keeps the Mapping and only pauses the
/// connection, so it never reaches here. Runs after the Mapping change
/// commits; a connection left behind cannot serve calls, and the next
/// approval for the subject retries before it may map anyone else.
pub(crate) async fn retire_unmapped(
	f: &Federation,
	tenant: &str,
	subject: &str,
	actor: &str,
) -> Result<()> {
	let Some(service) = &f.store.plan_connections else {
		return Ok(());
	};
	let mut tx = f.store.control_pool.connection().begin().await?;
	let reachable = crate::apps::identity::repositories::plan_connections::mapped_identities(
		tx.as_mut(),
		tenant,
		subject,
	)
	.await?;
	tx.commit().await?;
	if reachable > 0 {
		return Ok(());
	}
	Ok(service
		.retire(
			&Owner {
				tenant: tenant.to_owned(),
				subject: subject.to_owned(),
			},
			actor,
		)
		.await?)
}

/// [`retire_unmapped`] after a Mapping was disabled. The disable already
/// committed, so a failure is logged rather than reported as the
/// administrator's.
pub(crate) async fn retire_disabled_mapping(f: &Federation, mapping: uuid::Uuid, actor: &str) {
	let result = async {
		let lease = f.store.orm_connection()?;
		let Some(mapping) =
			crate::apps::identity::models::DashboardMapping::find(&mut lease.handle(), mapping)
				.await?
		else {
			return Ok(());
		};
		retire_unmapped(f, &mapping.tenant, &mapping.subject, actor).await
	}
	.await;
	if result.is_err() {
		tracing::warn!("the Plan Connection of a disabled Mapping was not retired");
	}
}

/// The `state` of a relayed or pasted callback URL, read only to find its
/// pending authorization. The URL is parsed strictly against that
/// authorization's redirect URI afterwards.
fn callback_state(value: &str) -> Result<String> {
	let refused = || Error::Invalid("the URL is not a Sign in with ChatGPT callback".into());
	if value.len() > 8192 {
		return Err(refused());
	}
	let url = reqwest::Url::parse(value.trim()).map_err(|_| refused())?;
	let mut states = url
		.query_pairs()
		.filter(|(name, _)| name == "state")
		.map(|(_, value)| value.into_owned());
	match (states.next(), states.next()) {
		(Some(state), None) if !state.is_empty() && state.len() <= 512 => Ok(state),
		_ => Err(refused()),
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn callback_states_are_read_once_and_bounded() {
		assert_eq!(
			callback_state("http://127.0.0.1:1457/auth/callback?code=c&state=s1").unwrap(),
			"s1"
		);
		for refused in [
			"http://127.0.0.1:1457/auth/callback?code=c",
			"http://127.0.0.1:1457/auth/callback?state=a&state=b",
			"http://127.0.0.1:1457/auth/callback?state=",
			"not a url",
		] {
			assert!(callback_state(refused).is_err(), "{refused}");
		}
		assert!(
			callback_state(&format!("http://127.0.0.1:1457/?state={}", "s".repeat(513))).is_err()
		);
	}
}
