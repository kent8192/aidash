//! The production model adapters reload policy and pin a currently ZDR-capable route.
use super::*;
use aidash_application::{
	ports::NoProgress,
	provider_access::{Access, Operation},
};
use aidash_domain::{
	data_protection::{DataProtection, Level},
	model::ModelConfig,
	provider::ModelRequest,
};
use axum::{
	Json, Router,
	routing::{get, post},
};
use serde_json::json;
use std::sync::{
	Arc,
	atomic::{AtomicBool, AtomicUsize, Ordering},
};

struct Current {
	policy: Mutex<DataProtection>,
	endpoint: String,
	signed: Mutex<Vec<aidash_domain::data_protection::Requirement>>,
}
#[async_trait::async_trait]
impl ProviderAccess for Current {
	async fn data_protection(&self, _: &Context) -> Result<DataProtection> {
		Ok(*self.policy.lock().unwrap())
	}
	async fn resolve(&self, context: &Context, _: &str, _: &Source) -> Result<Access> {
		if context
			.inference
			.as_ref()
			.is_some_and(|inference| inference.operations.contains(&Operation::Chat))
		{
			self.signed
				.lock()
				.unwrap()
				.push(context.data_protection.requirement());
		}
		Ok(Access {
			endpoint: self.endpoint.clone(),
			bearer: "fixture".to_owned().into(),
			keyless: None,
		})
	}
	async fn check(&self, _: &Context, _: &str, _: &Source) -> Result<()> {
		Ok(())
	}
}
struct Server(tokio::task::JoinHandle<()>);
impl Drop for Server {
	fn drop(&mut self) {
		self.0.abort();
	}
}

#[rstest::rstest]
#[case(None)]
#[case(Some(aidash_domain::model::WireApi::Responses))]
#[tokio::test]
async fn existing_provider_reloads_each_send_and_refuses_a_model_absent_from_the_zdr_catalog(
	#[case] wire_api: Option<aidash_domain::model::WireApi>,
) {
	let bodies = Arc::new(Mutex::new(Vec::new()));
	let catalog_calls = Arc::new(AtomicUsize::new(0));
	let present = Arc::new(AtomicBool::new(true));
	let app = Router::new().route("/v1/chat/completions", post({ let bodies = bodies.clone(); move |Json(body): Json<Value>| { let bodies = bodies.clone(); async move { bodies.lock().unwrap().push(body); Json(json!({"choices":[{"finish_reason":"stop","message":{"content":"done"}}]})) } } }))
		.route("/v1/responses", post({ let bodies = bodies.clone(); move |Json(body): Json<Value>| { let bodies = bodies.clone(); async move { bodies.lock().unwrap().push(body); Json(json!({"id":"resp-fixture","status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"done"}]}]})) } } }))
		.route("/v1/models/user", get(|| async { Json(json!({"data":[{"id":"vendor/model"}]})) }))
		.route("/v1/endpoints/zdr", get({ let calls = catalog_calls.clone(); let present = present.clone(); move || { let calls = calls.clone(); let present = present.clone(); async move { calls.fetch_add(1, Ordering::SeqCst); Json(json!({"data": if present.load(Ordering::SeqCst) { vec![json!({"model_id":"vendor/model","tag":"vendor/zdr"})] } else { vec![] }})) } } }));
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
	let _server = Server(tokio::spawn(async move {
		axum::serve(listener, app).await.unwrap();
	}));
	let access = Arc::new(Current {
		policy: Mutex::new(DataProtection::default()),
		endpoint,
		signed: Mutex::new(vec![]),
	});
	let mut config: ModelConfig = serde_json::from_value(json!({"provider":"openrouter","provider_credential":"openrouter","endpoint":"https://openrouter.ai/api/v1","model_id":"vendor/model","context_window":8192,"max_output_tokens":256,"modalities":["text"],"cost":{},"streaming":false})).unwrap();
	config.wire_api = wire_api;
	let tenant = format!("current-policy-{wire_api:?}");
	let provider = crate::inference::provider(
		crate::semantic::clients().unwrap(),
		config.clone(),
		access.clone(),
		Context {
			tenant: tenant.clone(),
			..Default::default()
		},
	)
	.unwrap();
	let request = ModelRequest {
		provider_tools: vec![],
		instructions: "Answer".into(),
		context: json!({"history":[]}).into(),
		tools: vec![],
		max_output_tokens: 32,
		response_format: None,
		content_parts: vec![],
		cache_scope: None,
		cache_breakpoints: false,
		disable_provider_transforms: false,
		input_format: Default::default(),
		estimate: Default::default(),
	};
	for policy in [
		DataProtection::default(),
		DataProtection {
			model_call_zdr: false,
			..Default::default()
		},
		DataProtection {
			level: Level::NoTraining,
			model_call_zdr: false,
			..Default::default()
		},
		DataProtection {
			level: Level::ZeroDataRetention,
			..Default::default()
		},
	] {
		*access.policy.lock().unwrap() = policy;
		provider.infer(request.clone(), &NoProgress).await.unwrap();
	}
	provider.infer(request.clone(), &NoProgress).await.unwrap();
	assert_eq!(
		&access.signed.lock().unwrap()[..],
		&[
			aidash_domain::data_protection::Requirement::Zdr,
			aidash_domain::data_protection::Requirement::None,
			aidash_domain::data_protection::Requirement::NoTraining,
			aidash_domain::data_protection::Requirement::Zdr,
			aidash_domain::data_protection::Requirement::Zdr
		]
	);
	let sent = bodies.lock().unwrap().clone();
	assert_eq!(
		sent[0]["provider"],
		json!({"zdr":true,"require_parameters":true})
	);
	assert_eq!(sent[1]["provider"], json!({"require_parameters":true}));
	assert_eq!(
		sent[2]["provider"],
		json!({"data_collection":"deny","require_parameters":true})
	);
	assert_eq!(
		sent[3]["provider"],
		json!({"zdr":true,"require_parameters":true,"only":["vendor/zdr"],"allow_fallbacks":true})
	);
	assert_eq!(
		catalog_calls.load(Ordering::SeqCst),
		1,
		"a short-lived catalog is reused, but evaluated on each call"
	);
	let mut absent = config;
	absent.model_id = "vendor/not-zdr".into();
	let provider = crate::inference::provider(
		crate::semantic::clients().unwrap(),
		absent,
		access.clone(),
		Context {
			tenant: tenant.clone(),
			..Default::default()
		},
	)
	.unwrap();
	assert!(
		provider
			.infer(request.clone(), &NoProgress)
			.await
			.unwrap_err()
			.to_string()
			.contains("no current ZDR route")
	);
	assert_eq!(
		bodies.lock().unwrap().len(),
		5,
		"refused model sends no content"
	);
	*access.policy.lock().unwrap() = DataProtection {
		data_region_restricted: true,
		..Default::default()
	}
	.in_deployment(Some(aidash_domain::data_protection::Region::Us));
	assert!(
		provider
			.infer(request, &NoProgress)
			.await
			.unwrap_err()
			.to_string()
			.contains("unavailable in region")
	);
	assert_eq!(
		bodies.lock().unwrap().len(),
		5,
		"regional refusal also precedes content on both model transports"
	);
}

#[tokio::test]
async fn regional_catalogs_gate_models_and_pinned_tool_engines_without_sending_content() {
	use aidash_domain::data_protection::Region;
	let model_calls = Arc::new(AtomicUsize::new(0));
	let tool_calls = Arc::new(AtomicUsize::new(0));
	let app = Router::new()
      .route("/v1/models/user", get({ let calls = model_calls.clone(); move || { let calls=calls.clone(); async move { calls.fetch_add(1,Ordering::SeqCst); Json(json!({"data":[{"id":"vendor/model"}]})) } } }))
      .route("/v1/tools", get({ let calls = tool_calls.clone(); move || { let calls=calls.clone(); async move { calls.fetch_add(1,Ordering::SeqCst); Json(json!({"data":[{"id":"openrouter:web_search","default_engine":"native","engines":[{"id":"exa","data_regions":["us"]},{"id":"native","data_regions":["global"]}]}]})) } } }));
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
	let _server = Server(tokio::spawn(async move {
		axum::serve(listener, app).await.unwrap();
	}));
	let access = Current {
		policy: Mutex::new(DataProtection::default()),
		endpoint,
		signed: Mutex::default(),
	};
	let clients = crate::semantic::clients().unwrap();
	let source = Source::Tenant {
		provider: "openrouter".into(),
	};
	let mut context = Context {
		tenant: uuid::Uuid::new_v4().to_string(),
		..Default::default()
	};
	let endpoint = "https://openrouter.ai/api/v1";
	let body = json!({"model":"vendor/model","tools":[{"type":"openrouter:web_search","engine":"exa"}],"messages":[{"role":"user","content":"private content"}]});
	// With restriction off there is no additional discovery, preserving today's requests.
	regional_request(&access, &context, &clients, endpoint, &source, &body)
		.await
		.unwrap();
	assert_eq!(model_calls.load(Ordering::SeqCst), 0);
	context.data_protection = DataProtection {
		data_region_restricted: true,
		..Default::default()
	}
	.in_deployment(Some(Region::Us));
	regional_request(&access, &context, &clients, endpoint, &source, &body)
		.await
		.unwrap();
	regional_request(&access, &context, &clients, endpoint, &source, &body)
		.await
		.unwrap();
	let mut native = body.clone();
	native["tools"][0]["engine"] = json!("native");
	assert!(
		regional_request(&access, &context, &clients, endpoint, &source, &native)
			.await
			.unwrap_err()
			.to_string()
			.contains("pinned engine")
	);
	let mut absent = body;
	absent["model"] = json!("vendor/absent");
	assert!(
		regional_request(&access, &context, &clients, endpoint, &source, &absent)
			.await
			.unwrap_err()
			.to_string()
			.contains("unavailable in region")
	);
	assert_eq!(model_calls.load(Ordering::SeqCst), 1);
	assert_eq!(tool_calls.load(Ordering::SeqCst), 1);
	context.data_protection = DataProtection {
		data_region_restricted: true,
		..Default::default()
	}
	.in_deployment(Some(Region::Eu));
	assert!(
		regional_request(
			&access,
			&context,
			&clients,
			endpoint,
			&source,
			&json!({"model":"vendor/model","tools":[{"type":"openrouter:web_search"}]})
		)
		.await
		.is_err()
	);
	assert_eq!(
		model_calls.load(Ordering::SeqCst),
		2,
		"each region has its own short cache"
	);
	assert!(
		access.signed.lock().unwrap().is_empty(),
		"only discovery authority was used"
	);
}

#[rstest::rstest]
#[case(None)]
#[case(Some(aidash_domain::model::WireApi::Responses))]
#[tokio::test]
async fn provider_tools_reload_current_policy_and_refuse_no_training_and_zero_retention(
	#[case] wire_api: Option<aidash_domain::model::WireApi>,
) {
	let sends = Arc::new(AtomicUsize::new(0));
	let app = Router::new()
		.route("/v1/chat/completions", post({
			let sends = sends.clone();
			move || { let sends = sends.clone(); async move {
				sends.fetch_add(1, Ordering::SeqCst);
				Json(json!({"choices":[{"finish_reason":"stop","message":{"content":"done"}}]}))
			} }
		}))
		.route("/v1/responses", post({
			let sends = sends.clone();
			move || { let sends = sends.clone(); async move {
				sends.fetch_add(1, Ordering::SeqCst);
				Json(json!({"id":"resp-tool","status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"done"}]}]}))
			} }
		}));
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let access = Arc::new(Current {
		policy: Mutex::new(DataProtection::default()),
		endpoint: format!("http://{}/v1", listener.local_addr().unwrap()),
		signed: Mutex::new(vec![]),
	});
	let _server = Server(tokio::spawn(async move {
		axum::serve(listener, app).await.unwrap();
	}));
	let mut config: ModelConfig = serde_json::from_value(json!({"provider":"openrouter","provider_credential":"openrouter","endpoint":"https://openrouter.ai/api/v1","model_id":"vendor/model","context_window":8192,"max_output_tokens":256,"modalities":["text"],"cost":{},"streaming":false})).unwrap();
	config.wire_api = wire_api;
	let provider = crate::inference::provider(
		crate::semantic::clients().unwrap(),
		config,
		access.clone(),
		Context {
			tenant: format!("provider-policy-{wire_api:?}"),
			run: Some(uuid::Uuid::now_v7()),
			..Default::default()
		},
	)
	.unwrap();
	let tool = aidash_domain::tool::provider_tools::declaration("openrouter:datetime").unwrap();
	let request = ModelRequest {
		provider_tools: vec![aidash_domain::tool::provider_tools::PinnedTool {
			tool_type: tool.tool_type.into(),
			binding: tool.defaults(),
		}],
		instructions: "Answer".into(),
		context: json!({"history":[]}).into(),
		tools: vec![],
		max_output_tokens: 32,
		response_format: None,
		content_parts: vec![],
		cache_scope: None,
		cache_breakpoints: false,
		disable_provider_transforms: false,
		input_format: Default::default(),
		estimate: Default::default(),
	};
	provider.infer(request.clone(), &NoProgress).await.unwrap();
	for level in [Level::NoTraining, Level::ZeroDataRetention] {
		*access.policy.lock().unwrap() = DataProtection {
			level,
			..Default::default()
		};
		let error = provider
			.infer(request.clone(), &NoProgress)
			.await
			.unwrap_err();
		assert!(
			error
				.to_string()
				.contains("Provider tools cannot meet this level"),
			"{error}"
		);
	}
	assert_eq!(
		sends.load(Ordering::SeqCst),
		1,
		"tightened policies send no content"
	);
	assert_eq!(
		access.signed.lock().unwrap().len(),
		1,
		"refusal precedes token issuance"
	);
	// Loosening policy also takes effect without rebuilding an admitted provider.
	*access.policy.lock().unwrap() = DataProtection::default();
	provider.infer(request, &NoProgress).await.unwrap();
	assert_eq!(sends.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn regional_gate_covers_warning_tier_tools_and_engines_pinned_in_parameters() {
	use aidash_domain::data_protection::Region;
	let catalog = json!({"data":[
		{"id":"openrouter:web_search","default_engine":"exa","engines":[{"id":"exa","data_regions":["us"]},{"id":"perplexity","data_regions":["global"]}]},
		{"id":"openrouter:advisor","default_engine":"openrouter","engines":[{"id":"openrouter","data_regions":["us"]}]},
		{"id":"openrouter:subagent","default_engine":"openrouter","engines":[{"id":"openrouter","data_regions":["us"]}]},
		{"id":"openrouter:tool_search","default_engine":"openrouter","engines":[{"id":"openrouter","data_regions":["us"]}]},
		{"id":"openrouter:shell","default_engine":"openrouter","engines":[{"id":"openrouter","data_regions":["us","global"]}]}
	]});
	let app = Router::new()
		.route(
			"/v1/models/user",
			get(|| async { Json(json!({"data":[{"id":"vendor/model"}]})) }),
		)
		.route(
			"/v1/tools",
			get(move || {
				let catalog = catalog.clone();
				async move { Json(catalog) }
			}),
		);
	let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
	let access = Current {
		policy: Mutex::new(DataProtection::default()),
		endpoint: format!("http://{}/v1", listener.local_addr().unwrap()),
		signed: Mutex::default(),
	};
	let _server = Server(tokio::spawn(async move {
		axum::serve(listener, app).await.unwrap();
	}));
	let clients = crate::semantic::clients().unwrap();
	let source = Source::Tenant {
		provider: "openrouter".into(),
	};
	let endpoint = "https://openrouter.ai/api/v1";
	let entry = |kind: &str| {
		let declaration = aidash_domain::tool::provider_tools::declaration(kind).unwrap();
		let mut pinned = aidash_domain::tool::provider_tools::PinnedTool {
			tool_type: kind.into(),
			binding: declaration.defaults(),
		};
		if matches!(kind, "openrouter:advisor" | "openrouter:subagent") {
			pinned.binding.parameters["model"] = json!("vendor/child");
		}
		pinned.entry()
	};
	let context = |region| Context {
		tenant: uuid::Uuid::new_v4().to_string(),
		data_protection: DataProtection {
			data_region_restricted: true,
			..Default::default()
		}
		.in_deployment(Some(region)),
		..Default::default()
	};
	let us = context(Region::Us);
	for kind in [
		"openrouter:advisor",
		"openrouter:subagent",
		"openrouter:tool_search",
	] {
		regional_tools(
			&access,
			&us,
			&clients,
			endpoint,
			&source,
			&json!([entry(kind)]),
		)
		.await
		.unwrap_or_else(|error| panic!("{kind}: {error}"));
		let error = regional_tools(
			&access,
			&context(Region::Jp),
			&clients,
			endpoint,
			&source,
			&json!([entry(kind)]),
		)
		.await
		.unwrap_err();
		assert!(
			error.to_string().contains("unavailable in region"),
			"{kind}: {error}"
		);
	}
	// The experimental model search is absent from this catalog: unknown means refused.
	assert!(
		regional_tools(
			&access,
			&us,
			&clients,
			endpoint,
			&source,
			&json!([entry("openrouter:experimental__search_models")]),
		)
		.await
		.is_err()
	);
	// Shell is global-only whatever a catalog claims, so a restricted Tenant never reaches it.
	for region in [Region::Us, Region::Eu, Region::Jp] {
		let error = regional_tools(
			&access,
			&context(region),
			&clients,
			endpoint,
			&source,
			&json!([entry("openrouter:shell")]),
		)
		.await
		.unwrap_err();
		assert!(
			error.to_string().contains("unavailable in region"),
			"{error}"
		);
	}
	// A Provider tool entry pins its engine under `parameters`; that pin is what is checked.
	let search = |engine: &str| json!([{"type":"openrouter:web_search","parameters":{"engine":engine,"max_results":5}}]);
	regional_tools(&access, &us, &clients, endpoint, &source, &search("exa"))
		.await
		.unwrap();
	assert!(
		regional_tools(
			&access,
			&us,
			&clients,
			endpoint,
			&source,
			&search("perplexity")
		)
		.await
		.unwrap_err()
		.to_string()
		.contains("pinned engine")
	);
}
