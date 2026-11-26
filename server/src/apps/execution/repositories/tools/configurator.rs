//! Dispatch the built-in agent's calls through the Deployment's own routes.
use crate::{
	Error, Result, apps::identity::services::configurator::Caller, federation::Federation,
};
use aidash_application::tools::configurator::route;
use aidash_domain::{Run, tool::configurator::permits};
use futures_util::StreamExt;
use reinhardt::{InjectionContext, Request, http::Handler};
use serde_json::{Value, json};
use std::sync::{Arc, OnceLock};

struct Api<'a> {
	runtime: &'a Federation,
	caller: Caller,
}

impl Api<'_> {
	async fn call(&self, method: &str, path: &str, body: Value) -> Result<Value> {
		if !permits(self.caller.tenant(), method, path)
			|| body
				.get("tenant")
				.and_then(Value::as_str)
				.is_some_and(|t| t != self.caller.tenant())
			|| path
				.split('?')
				.next()
				.unwrap_or("")
				.split('/')
				.any(|p| p == "secrets")
				&& body.get("value").is_some()
		{
			return Err(Error::Forbidden);
		}
		let bytes = if body.is_null() {
			Vec::new()
		} else {
			serde_json::to_vec(&body)?
		};
		self.dispatch(method, path, "application/json", bytes).await
	}

	async fn dispatch(
		&self,
		method: &str,
		path: &str,
		content_type: &str,
		bytes: Vec<u8>,
	) -> Result<Value> {
		if !permits(self.caller.tenant(), method, path) {
			return Err(Error::Forbidden);
		}
		let method = http::Method::from_bytes(method.as_bytes())
			.map_err(|_| Error::Invalid("invalid HTTP method".into()))?;
		let mut request = Request::builder()
			.method(method)
			.uri(path)
			.header("content-type", content_type)
			.body(bytes.into())
			.build()
			.map_err(Error::Invalid)?;
		request.extensions.insert(self.caller.clone());
		let services = self
			.runtime
			.store
			.http_services
			.upgrade()
			.ok_or_else(|| Error::External("HTTP services unavailable".into()))?;
		let context = Arc::new(InjectionContext::builder(services).build());
		request.set_di_context(context.clone());
		let router = crate::routes().into_server().with_di_context(context);
		self.caller
			.during(async {
				let mut response = router.handle(request).await?;
				let status = response.status;
				if status == http::StatusCode::NO_CONTENT {
					return Ok(Value::Null);
				}
				// Gateway wraps even buffered JSON as a stream. Do not subscribe to
				// event streams or read a file download into a model tool result.
				if response
					.headers
					.get(http::header::CONTENT_TYPE)
					.and_then(|value| value.to_str().ok())
					.and_then(|value| value.split(';').next())
					.is_some_and(|mime| mime != "application/json" && !mime.ends_with("+json"))
				{
					return Err(Error::Invalid(format!("{status} {path}: expected JSON")));
				}
				const MAX_JSON_BYTES: usize = 8 * 1024 * 1024;
				if response.body.len() > MAX_JSON_BYTES {
					return Err(Error::Invalid("API response exceeds 8 MiB".into()));
				}
				let mut bytes = response.body.to_vec();
				if let Some(mut stream) = response.take_stream_body() {
					while let Some(chunk) = stream.next().await {
						let chunk = chunk.map_err(|error| Error::External(error.to_string()))?;
						if bytes.len().saturating_add(chunk.len()) > MAX_JSON_BYTES {
							return Err(Error::Invalid("API response exceeds 8 MiB".into()));
						}
						bytes.extend_from_slice(&chunk);
					}
				}
				let value = if bytes.is_empty() {
					Value::Null
				} else {
					serde_json::from_slice(&bytes)
						.map_err(|_| Error::Invalid(format!("{status} {path}: expected JSON")))?
				};
				if !status.is_success() {
					return Err(Error::Invalid(format!("{status} {path}: {value}")));
				}
				Ok(value)
			})
			.await
	}

	async fn overview(&self, run: &Run) -> Result<Value> {
		let tenant = self.caller.tenant();
		let mut out = json!({});
		for (name, path) in [
			("nodes", format!("/api/tenants/{tenant}/nodes")),
			("members", format!("/api/tenants/{tenant}/mappings")),
			(
				"catalog",
				format!("/api/authorization/{tenant}/catalog?node={}", run.node),
			),
			("connections", format!("/api/tenants/{tenant}/connections")),
			(
				"providers",
				format!("/api/tenants/{tenant}/provider-credential-bindings"),
			),
			("budgets", format!("/api/tenants/{tenant}/budgets")),
			(
				"approval",
				format!("/api/tenants/{tenant}/approval-preferences/mine"),
			),
		] {
			out[name] = match self.call("GET", &path, Value::Null).await {
				Ok(v) => v,
				Err(e) => json!({"error":e.to_string()}),
			};
		}
		Ok(out)
	}

	async fn publish(&self, name: &str, input: &Value) -> Result<Value> {
		let mut draft = input["draft"].clone();
		if draft["entry"]["kind"] != "agent"
			|| !draft["entry"]["config"].is_object()
			|| !input["approval"].is_object()
		{
			return Err(Error::Invalid(
				"provide an agent draft and its Catalog Approval".into(),
			));
		}
		let config = &mut draft["entry"]["config"];
		let management = crate::apps::registry::services::management::RegistryManagement {
			runtime: self.runtime.clone(),
		};
		let actor = self.caller.actor();
		if name == "skill_add" {
			let mut skill = input["skill"].clone();
			if skill["kind"] != "skill"
				|| !skill["config"].is_object()
				|| !input["skill_approval"].is_object()
			{
				return Err(Error::Invalid(
					"provide a Skill definition and its Catalog Approval".into(),
				));
			}
			if let Some(source) = input.get("source") {
				let imported = self
					.caller
					.during(management.skill_import(actor, serde_json::from_value(source.clone())?))
					.await?;
				let Some(selected) = imported.selected else {
					return Ok(json!({"choose_skill_path":imported.skills}));
				};
				skill["config"]["instructions"] = json!(selected.instructions);
				skill["config"]["files"] = json!(selected.files);
			}
			let entry = self.call("POST", "/api/registry", skill).await?;
			self.approve(&entry, input["skill_approval"].clone())
				.await?;
			append(
				config,
				"bindings",
				json!({"kind":"skill","target":{"registry_deployment":self.runtime.config.deployment.as_str(),"id":entry["id"],"version":entry["version"]}}),
			)?;
		} else if name == "agent_plugin_add" {
			let plugin = if let Some(source) = input.get("source") {
				let imported = self
					.caller
					.during(
						management
							.agent_plugin_import(actor, serde_json::from_value(source.clone())?),
					)
					.await?;
				let Some(selected) = imported.selected else {
					return Ok(json!({"choose_plugin_path":imported.plugins}));
				};
				json!(selected.reference)
			} else {
				serde_json::to_value(serde_json::from_value::<
					aidash_domain::agent_plugin::AgentPluginRef,
				>(input["plugin"].clone())?)?
			};
			append(config, "agent_plugins", plugin)?;
		}
		let saved = self
			.call("POST", "/api/workbench/drafts", draft.clone())
			.await?;
		let id = saved["id"]
			.as_str()
			.ok_or_else(|| Error::Invalid("draft has no id".into()))?;
		let registered = self
			.call(
				"POST",
				&format!("/api/workbench/drafts/{id}/register"),
				json!({"expected_revision":saved["revision"]}),
			)
			.await?;
		let approved = self
			.approve(&registered["entry"], input["approval"].clone())
			.await?;
		Ok(json!({"draft":id,"registered":registered,"approval":approved}))
	}

	async fn approve(&self, entry: &Value, mut approval: Value) -> Result<Value> {
		approval["entry"] = json!({"id":entry["id"],"version":entry["version"]});
		approval["enabled"] = json!(true);
		self.call(
			"POST",
			&format!("/api/authorization/{}/catalog", self.caller.tenant()),
			approval,
		)
		.await
	}

	async fn attachment(&self, input: &Value) -> Result<Value> {
		use base64::Engine;
		let thread: uuid::Uuid = serde_json::from_value(input["thread"].clone())?;
		let filename = input["filename"]
			.as_str()
			.ok_or_else(|| Error::Invalid("missing attachment filename".into()))?;
		let media = input["media_type"]
			.as_str()
			.ok_or_else(|| Error::Invalid("missing attachment media type".into()))?;
		let key: uuid::Uuid = serde_json::from_value(input["idempotency_key"].clone())?;
		let data = input["data"]
			.as_str()
			.ok_or_else(|| Error::Invalid("missing attachment data".into()))?;
		if data.len() > 8 * 1024 * 1024 * 4 / 3 + 4 {
			return Err(Error::Invalid("attachment exceeds 8 MiB".into()));
		}
		let bytes = base64::engine::general_purpose::STANDARD
			.decode(data)
			.map_err(|_| Error::Invalid("invalid base64 attachment".into()))?;
		if bytes.len() > 8 * 1024 * 1024 {
			return Err(Error::Invalid("attachment exceeds 8 MiB".into()));
		}
		use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
		let query = format!(
			"filename={}&media_type={}&idempotency_key={key}",
			utf8_percent_encode(filename, NON_ALPHANUMERIC),
			utf8_percent_encode(media, NON_ALPHANUMERIC)
		);
		self.dispatch(
			"POST",
			&format!("/api/threads/{thread}/attachments?{query}"),
			"application/octet-stream",
			bytes,
		)
		.await
	}

	async fn typed(&self, name: &str, mut input: Value) -> Result<Value> {
		if name == "reference_upload" && input.get("thread").is_some() {
			return self.attachment(&input).await;
		}
		if matches!(name, "agent_create" | "skill_add" | "agent_plugin_add") {
			return self.publish(name, &input).await;
		}
		let route =
			route(name).ok_or_else(|| Error::Invalid("unknown configuration tool".into()))?;
		let mut method = route.method;
		let mut path = route.path.replace("{tenant}", self.caller.tenant());
		let args = input
			.as_object_mut()
			.ok_or_else(|| Error::Invalid("expected a JSON object".into()))?;
		for segment in route.path.split('/') {
			if let Some(key) = segment.strip_prefix('{').and_then(|v| v.strip_suffix('}'))
				&& key != "tenant"
			{
				let value = args
					.remove(key)
					.ok_or_else(|| Error::Invalid(format!("missing {key}")))?;
				let param = value
					.as_str()
					.filter(|v| {
						!v.is_empty()
							&& v.bytes()
								.all(|b| b.is_ascii_alphanumeric() || b"-._@".contains(&b))
					})
					.ok_or_else(|| Error::Invalid(format!("invalid {key}")))?;
				path = path.replace(segment, param);
			}
		}
		let mut body = input;
		if !body.is_object() {
			return Err(Error::Invalid("expected an object body".into()));
		}
		match name {
			"mcp_server_add"
				if body["config"]["transport"] != "mcp"
					&& body["config"]["transport"]["transport"] != "mcp" =>
			{
				return Err(Error::Invalid("expected an MCP tool definition".into()));
			}
			"catalog_approve" => body["enabled"] = json!(true),
			"node_update" => {
				let action = body.as_object_mut().and_then(|o| o.remove("action"));
				match action.as_ref().and_then(Value::as_str) {
					Some("label") => {}
					Some("default") => {
						method = "POST";
						path.push_str("/default");
						body = Value::Null;
					}
					Some("instructions") => {
						method = "PUT";
						path.push_str("/instructions");
					}
					_ => return Err(Error::Invalid("unknown Node update".into())),
				}
			}
			"budget_set" => {
				if let Some(id) = body.as_object_mut().and_then(|o| o.remove("budget")) {
					let budget = uuid::Uuid::parse_str(id.as_str().unwrap_or(""))
						.map_err(|_| Error::Invalid("invalid Budget id".into()))?;
					method = "PUT";
					path.push_str(&format!("/{budget}"));
					let revision = body["expected_revision"].take();
					body = body["definition"].take();
					if !body.is_object() {
						return Err(Error::Invalid("expected a Budget definition".into()));
					}
					body["expected_revision"] = revision;
				} else {
					body = body["definition"].take();
				}
			}
			"node_registration" => {
				let mut out = json!({});
				if let Some(reg) = body.get("registration") {
					out["registration"] = self.call(method, &path, reg.clone()).await?;
				}
				if let Some(mapping) = body.get("peer_mapping") {
					out["peer_mapping"] = self
						.call(
							"POST",
							&format!("/api/authorization/{}/peer-mappings", self.caller.tenant()),
							mapping.clone(),
						)
						.await?;
				}
				if out == json!({}) {
					return Err(Error::Invalid(
						"provide registration or peer_mapping".into(),
					));
				}
				return Ok(out);
			}
			"tool_add" | "mcp_server_add" if body["kind"] != "tool" => {
				return Err(Error::Invalid("expected a tool definition".into()));
			}
			_ => {}
		}
		self.call(method, &path, body).await
	}
}

fn append(config: &mut Value, field: &str, value: Value) -> Result<()> {
	let items = config
		.as_object_mut()
		.and_then(|object| {
			object
				.entry(field)
				.or_insert_with(|| json!([]))
				.as_array_mut()
		})
		.ok_or_else(|| Error::Invalid(format!("invalid {field}")))?;
	if !items.contains(&value) {
		items.push(value);
	}
	Ok(())
}

pub(super) async fn execute(f: &Federation, run: &Run, name: &str, input: Value) -> Result<Value> {
	let api = Api {
		runtime: f,
		caller: Caller::from_run(f, run).await?,
	};
	match name {
		"aidash_api" => {
			api.call(
				input["method"].as_str().unwrap_or(""),
				input["path"].as_str().unwrap_or(""),
				input["body"].clone(),
			)
			.await
		}
		"aidash_api_catalog" => catalog(api.caller.tenant(), input["query"].as_str().unwrap_or("")),
		"tenant_overview" => api.overview(run).await,
		_ => api.typed(name, input).await,
	}
}

fn catalog(tenant: &str, query: &str) -> Result<Value> {
	static DOC: OnceLock<Value> = OnceLock::new();
	let doc = match DOC.get() {
		Some(doc) => doc,
		None => {
			let value = serde_json::to_value(crate::config::openapi::openapi()?)?;
			DOC.get_or_init(|| value)
		}
	};
	let terms = query.to_lowercase();
	let mut matches = vec![];
	let mut truncated = false;
	for (path, methods) in doc["paths"].as_object().into_iter().flatten() {
		let sample = path
			.split('/')
			.map(|p| {
				if p == "{tenant}" {
					tenant
				} else if p.starts_with('{') {
					"id"
				} else {
					p
				}
			})
			.collect::<Vec<_>>()
			.join("/");
		for (method, op) in methods.as_object().into_iter().flatten() {
			let verb = method.to_uppercase();
			if !permits(tenant, &verb, &sample) {
				continue;
			}
			let hay =
				format!("{verb} {path} {} {}", op["operationId"], op["summary"]).to_lowercase();
			if !terms.split_whitespace().all(|s| hay.contains(s)) {
				continue;
			}
			if matches.len() == 12 {
				truncated = true;
				break;
			}
			let successful = op["responses"]
				.as_object()
				.and_then(|responses| responses.iter().find(|(status, _)| status.starts_with('2')));
			let response = successful
				.map(|(_, value)| expand(doc, &value["content"]["application/json"]["schema"], 0));
			matches.push(json!({
				"method": verb,
				"path": path,
				"operation": op["operationId"],
				"parameters": op["parameters"],
				"request": expand(doc, &op["requestBody"]["content"]["application/json"]["schema"], 0),
				"status": successful.map(|(status, _)| status),
				"response": response,
			}));
		}
		if truncated {
			break;
		}
	}
	Ok(json!({"operations":matches,"truncated":truncated}))
}

// Expand a component at the operation boundary, preserving nested references
// to keep recursive documents and tool results bounded.
fn expand(doc: &Value, schema: &Value, depth: usize) -> Value {
	if depth < 2
		&& let Some(pointer) = schema["$ref"].as_str().and_then(|r| r.strip_prefix('#'))
		&& let Some(value) = doc.pointer(pointer)
	{
		return expand(doc, value, depth + 1);
	}
	schema.clone()
}
