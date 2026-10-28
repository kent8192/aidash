//! Account-specific regional discovery shared by sends and policy previews.
use super::*;
use aidash_domain::data_protection::Region;

type RegionalCatalogs =
	BTreeMap<(String, String, Option<uuid::Uuid>, Region, String), (Instant, Arc<Value>)>;

async fn catalog(
	access: &dyn ProviderAccess,
	context: &Context,
	clients: &crate::keyless::Clients,
	endpoint: &str,
	source: &Source,
	model: &str,
	path: &str,
) -> Result<Arc<Value>> {
	let region = context.data_protection.region()?.ok_or_else(|| {
		Error::Invalid("regional discovery requires a Data Region restriction".into())
	})?;
	region.openrouter_endpoint()?;
	static CATALOGS: OnceLock<Mutex<RegionalCatalogs>> = OnceLock::new();
	let catalogs = CATALOGS.get_or_init(Mutex::default);
	let key = (
		endpoint.to_owned(),
		context.tenant.clone(),
		context.provider_credential_id,
		region,
		path.to_owned(),
	);
	if let Some(body) = catalogs
		.lock()
		.expect("regional catalog lock")
		.get(&key)
		.filter(|(at, _)| at.elapsed() < Duration::from_secs(30))
		.map(|(_, body)| body.clone())
	{
		return Ok(body);
	}
	let resolved = access.discovery(context, endpoint, source, model).await?;
	let mut request = clients.select(&resolved)?.get(format!(
		"{}/{path}",
		resolved.endpoint.trim_end_matches('/')
	));
	if !resolved.bearer.expose_secret().is_empty() {
		request = request.bearer_auth(resolved.bearer.expose_secret());
	}
	let response = request.send().await.map_err(crate::keyless::send_error)?;
	if !response.status().is_success() {
		return Err(Error::Invalid(format!(
			"Data Region restriction {region:?}: OpenRouter regional {path} is unavailable (HTTP {}); the Tenant's account must support regional routing",
			response.status().as_u16()
		)));
	}
	let body = crate::response::json::<Value>(response, 8 * 1024 * 1024).await?;
	if !body["data"].is_array() {
		return Err(Error::Invalid(format!(
			"Data Region restriction: invalid regional {path} catalog"
		)));
	}
	let body = Arc::new(body);
	let mut cache = catalogs.lock().expect("regional catalog lock");
	cache.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(30));
	if cache.len() >= 128 {
		cache.clear();
	}
	cache.insert(key, (Instant::now(), body.clone()));
	Ok(body)
}

/// No content is sent by discovery. Failure never falls back to the global host.
pub async fn regional_model(
	access: &dyn ProviderAccess,
	context: &Context,
	clients: &crate::keyless::Clients,
	endpoint: &str,
	source: &Source,
	model: &str,
) -> Result<()> {
	let Some(region) = context.data_protection.region()? else {
		return Ok(());
	};
	if !matches!(source, Source::Tenant { provider } if provider == "openrouter") {
		return Ok(());
	}
	let models = catalog(
		access,
		context,
		clients,
		endpoint,
		source,
		model,
		"models/user",
	)
	.await?;
	if !models["data"]
		.as_array()
		.expect("validated catalog")
		.iter()
		.any(|entry| entry["id"] == model)
	{
		return Err(Error::Invalid(format!(
			"Data Region restriction {region:?}: OpenRouter model {model} is unavailable in region"
		)));
	}
	Ok(())
}

/// Validate the finished body, including Provider tools pinned by Agent bindings.
pub async fn regional_request(
	access: &dyn ProviderAccess,
	context: &Context,
	clients: &crate::keyless::Clients,
	endpoint: &str,
	source: &Source,
	body: &Value,
) -> Result<()> {
	if context.data_protection.region()?.is_none() {
		return Ok(());
	}
	let model = body["model"]
		.as_str()
		.ok_or_else(|| Error::Invalid("regional request has no model".into()))?;
	regional_model(access, context, clients, endpoint, source, model).await?;
	if !matches!(source, Source::Tenant { provider } if provider == "openrouter") {
		return Ok(());
	}
	regional_tools(access, context, clients, endpoint, source, &body["tools"]).await
}

/// A Provider tool entry pins its engine under `parameters`; a top-level
/// `engine` is the legacy spelling and is still honoured so neither can hide a
/// non-regional engine.
fn pinned_engine(tool: &Value) -> Option<&str> {
	tool["parameters"]["engine"]
		.as_str()
		.or_else(|| tool["engine"].as_str())
}

/// Regional tool discovery is also available before a Tenant has chosen a model.
pub async fn regional_tools(
	access: &dyn ProviderAccess,
	context: &Context,
	clients: &crate::keyless::Clients,
	endpoint: &str,
	source: &Source,
	tools: &Value,
) -> Result<()> {
	let Some(region) = context.data_protection.region()? else {
		return Ok(());
	};
	let tools: Vec<_> = tools
		.as_array()
		.into_iter()
		.flatten()
		.filter(|tool| tool["type"] != "function")
		.collect();
	if tools.is_empty() {
		return Ok(());
	}
	for tool in &tools {
		let kind = tool["type"].as_str().unwrap_or("");
		if !region.permits_openrouter_tool(kind, pinned_engine(tool)) {
			return Err(Error::Invalid(format!(
				"Data Region restriction {region:?}: Provider tool {kind} or its pinned engine is unavailable in region"
			)));
		}
	}
	let catalog = catalog(
		access, context, clients, endpoint, source, "catalog", "tools",
	)
	.await?;
	for tool in tools {
		let kind = tool["type"].as_str().unwrap_or("");
		if [&tool["engine"], &tool["parameters"]["engine"]]
			.iter()
			.any(|value| !value.is_null() && !value.is_string())
		{
			return Err(Error::Invalid(
				"Data Region restriction: invalid pinned tool engine".into(),
			));
		}
		let engine = pinned_engine(tool);
		let entry = catalog["data"]
			.as_array()
			.expect("validated catalog")
			.iter()
			.find(|entry| entry["id"] == kind);
		let available = entry.is_some_and(|entry| {
			let selected =
				if kind == "openrouter:web_search" && matches!(engine, None | Some("auto")) {
					Some("exa")
				} else {
					engine.or(entry["default_engine"].as_str())
				};
			entry["engines"]
				.as_array()
				.into_iter()
				.flatten()
				.any(|candidate| {
					selected.is_some()
						&& candidate["id"].as_str() == selected
						&& candidate["data_regions"]
							.as_array()
							.into_iter()
							.flatten()
							.any(|value| value.as_str() == Some(region.openrouter_data_region()))
				})
		});
		if !available || !region.permits_openrouter_tool(kind, engine) {
			return Err(Error::Invalid(format!(
				"Data Region restriction {region:?}: Provider tool {kind} or its pinned engine is unavailable in region"
			)));
		}
	}
	Ok(())
}
