//! The production model adapters reload policy and pin a currently ZDR-capable route.
use super::*;
use aidash_application::{ports::NoProgress, provider_access::Access};
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
		access,
		Context {
			tenant: tenant.clone(),
			..Default::default()
		},
	)
	.unwrap();
	assert!(
		provider
			.infer(request, &NoProgress)
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
