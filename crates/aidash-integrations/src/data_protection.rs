//! Per-send policy evaluation and the short-lived OpenRouter ZDR catalog.
use crate::{Error, Result};
use aidash_application::provider_access::{Context, Inference, Operation, ProviderAccess, Source};
use aidash_domain::data_protection::Level;
use secrecy::ExposeSecret;
use serde_json::Value;
use std::{
	collections::BTreeMap,
	sync::{Arc, Mutex, OnceLock},
	time::{Duration, Instant},
};

pub async fn context(access: &dyn ProviderAccess, context: &Context) -> Result<Context> {
	let mut current = context.clone();
	current.data_protection = access.data_protection(context).await?;
	Ok(current)
}

type Catalogs = BTreeMap<(String, String), (Instant, Arc<Value>)>;
/// A call is checked against the current catalog, cached for at most 30 seconds.
/// No bearer, client, request content or runtime handle enters this cache.
pub async fn zdr_routes(
	access: &dyn ProviderAccess,
	context: &Context,
	clients: &crate::keyless::Clients,
	endpoint: &str,
	source: &Source,
	model: &str,
) -> Result<Vec<String>> {
	if context.data_protection.level != Level::ZeroDataRetention
		|| !matches!(source, Source::Tenant { .. })
	{
		return Ok(vec![]);
	}
	static CATALOGS: OnceLock<Mutex<Catalogs>> = OnceLock::new();
	let catalogs = CATALOGS.get_or_init(Mutex::default);
	let key = (endpoint.to_owned(), context.tenant.clone());
	let cached = catalogs
		.lock()
		.expect("ZDR catalog lock")
		.get(&key)
		.filter(|(at, _)| at.elapsed() < Duration::from_secs(30))
		.map(|(_, body)| body.clone());
	let catalog = if let Some(catalog) = cached {
		catalog
	} else {
		let mut discovery = context.clone();
		discovery.inference = Some(if context.is_decision() {
			let mut attempt = context
				.inference
				.clone()
				.expect("decision scope has inference");
			attempt.operations = vec![Operation::DecisionDiscovery];
			attempt
		} else {
			Inference {
				provider_tools: vec![],
				model: model.into(),
				operations: vec![Operation::Discovery],
				max_output_tokens: 1,
				cache_breakpoints: false,
				call: None,
				hosts: vec![],
			}
		});
		let access = access.resolve(&discovery, endpoint, source).await?;
		let mut request = clients.select(&access)?.get(format!(
			"{}/endpoints/zdr",
			access.endpoint.trim_end_matches('/')
		));
		if !access.bearer.expose_secret().is_empty() {
			request = request.bearer_auth(access.bearer.expose_secret());
		}
		let response = request.send().await.map_err(crate::keyless::send_error)?;
		if !response.status().is_success() {
			return Err(Error::Invalid(
				"Zero Data Retention catalog is unavailable".into(),
			));
		}
		let body = crate::response::json::<Value>(response, 8 * 1024 * 1024).await?;
		if !body["data"].is_array() {
			return Err(Error::Invalid(
				"Zero Data Retention catalog is invalid".into(),
			));
		}
		let mut cache = catalogs.lock().expect("ZDR catalog lock");
		cache.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(30));
		if cache.len() >= 128 {
			cache.clear();
		}
		let body = Arc::new(body);
		cache.insert(key, (Instant::now(), body.clone()));
		body
	};
	let routes: Vec<String> = catalog["data"]
		.as_array()
		.into_iter()
		.flatten()
		.filter(|entry| entry["model_id"] == model)
		.filter_map(|entry| entry["tag"].as_str().map(str::to_owned))
		.collect();
	if routes.is_empty() {
		return Err(Error::Invalid(format!(
			"Zero Data Retention: no current ZDR route for {model}"
		)));
	}
	Ok(routes)
}

#[cfg(test)]
mod tests;
