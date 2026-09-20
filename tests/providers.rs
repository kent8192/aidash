use aidash::{
    provider::{ModelRequest, ToolSpec, provider},
    registry::{Entry, ModelConfig, validate},
};
use axum::{Json, Router, routing::post};
use serde_json::{Value, json};
use std::time::Duration;

fn config(provider: &str, endpoint: String) -> ModelConfig {
    ModelConfig {
        provider: provider.into(),
        model_id: "vendor/fixture-model".into(),
        endpoint,
        credential_env: None,
        context_window: 128000,
        modalities: vec!["text".into()],
        cost: json!({}),
    }
}

#[test]
fn registry_accepts_openrouter_and_rejects_unknown_providers() {
    let mut entry: Entry = serde_json::from_value(json!({
        "id":"router-model","version":"1.0.0","kind":"model",
        "name":{"en":"Router model"},"description":{"en":"Test model"},
        "config":config("openrouter","https://openrouter.ai/api/v1".into())
    }))
    .unwrap();
    validate(&entry).unwrap();
    entry.config["provider"] = json!("unsupported");
    assert!(validate(&entry).is_err());
}

#[tokio::test]
async fn openrouter_and_openai_preserve_tools_text_usage_and_token_limits() {
    let (tx, mut received) = tokio::sync::mpsc::unbounded_channel();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/api/v1/", listener.local_addr().unwrap());
    let app = Router::new().route("/api/v1/chat/completions", post(move |Json(body): Json<Value>| {
        let tx = tx.clone();
        async move {
            tx.send(body.clone()).unwrap();
            let choice = if body["tools"].is_array() {
                json!({"finish_reason":"tool_calls","message":{"content":null,"tool_calls":[{
                    "id":"call-1","type":"function","function":{"name":"read","arguments":"{\"path\":\"notes\"}"}
                }]}})
            } else {
                json!({"finish_reason":"stop","message":{"content":"Completed"}})
            };
            Json(json!({"choices":[choice],"usage":{"prompt_tokens":12,"completion_tokens":7,"cost":0.01}}))
        }
    }));
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    for name in ["openrouter", "openai"] {
        let model = provider(client.clone(), config(name, endpoint.clone())).unwrap();
        for with_tools in [true, false] {
            let response = model.infer(ModelRequest {
                instructions:"Follow the task".into(), context:json!({"task":"Read notes"}),
                tools:if with_tools { vec![ToolSpec {
                    name:"read".into(), description:"Read notes".into(),
                    parameters:json!({"type":"object","properties":{"path":{"type":"string"}}}),
                }] } else { vec![] },
                max_output_tokens:512,
            }).await.unwrap();
            let request = received.recv().await.unwrap();
            assert_eq!(request["model"], "vendor/fixture-model");
            assert_eq!(request["messages"][0]["content"], "Follow the task");
            assert_eq!(
                serde_json::from_str::<Value>(request["messages"][1]["content"].as_str().unwrap())
                    .unwrap(),
                json!({"task":"Read notes"})
            );
            let (limit, absent) = if name == "openrouter" {
                ("max_tokens", "max_completion_tokens")
            } else {
                ("max_completion_tokens", "max_tokens")
            };
            assert_eq!(request[limit], 512);
            assert!(request.get(absent).is_none());
            assert_eq!((response.input_tokens, response.output_tokens), (12, 7));
            if with_tools {
                assert_eq!(request["tools"][0]["function"]["name"], "read");
                assert_eq!(response.tool_calls.len(), 1);
                assert_eq!(response.tool_calls[0].arguments, json!({"path":"notes"}));
            } else {
                assert!(request.get("tools").is_none());
                assert_eq!(response.text, "Completed");
                assert!(response.tool_calls.is_empty());
            }
        }
    }
    server.abort();
}

#[test]
fn refunds_require_complete_usage_and_anthropic_counts_cached_input() {
    let openai = serde_json::json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"ok"}}],"usage":{"completion_tokens":1}});
    assert!(
        !aidash::provider::parse_openai(openai)
            .unwrap()
            .usage_complete
    );
    let anthropic = serde_json::json!({"stop_reason":"end_turn","content":[{"type":"text","text":"ok"}],"usage":{"input_tokens":10,"cache_creation_input_tokens":20,"cache_read_input_tokens":30,"output_tokens":5}});
    let response = aidash::provider::parse_anthropic(anthropic.clone()).unwrap();
    assert_eq!(response.input_tokens, 60);
    assert!(response.usage_complete);
    let mut malformed = anthropic;
    malformed["usage"]["cache_read_input_tokens"] = serde_json::json!("unknown");
    assert!(
        !aidash::provider::parse_anthropic(malformed)
            .unwrap()
            .usage_complete
    );
}
