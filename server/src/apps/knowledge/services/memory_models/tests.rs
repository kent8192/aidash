use super::*;

fn model(context_window: usize, max_output_tokens: u32) -> ModelConfig {
	serde_json::from_value(json!({
		"provider": "openrouter",
		"model_id": "fixture/model",
		"endpoint": "https://openrouter.ai/api/v1",
		"context_window": context_window,
		"max_output_tokens": max_output_tokens,
		"modalities": ["text"],
		"cost": {},
	}))
	.unwrap()
}

fn request(config: &ModelConfig, context: &Value) -> ModelRequest {
	ModelRequest {
		provider_tools: vec![],
		instructions: "Extract memory units as JSON.".into(),
		context: ModelContext::native_data(context),
		tools: vec![],
		max_output_tokens: 0,
		response_format: None,
		content_parts: vec![],
		cache_scope: None,
		cache_breakpoints: false,
		routing_run: None,
		disable_provider_transforms: false,
		input_format: InputFormat::for_model(config),
		estimate: Default::default(),
	}
}

/// #1122: a model whose output limit equals its 32k context window keeps a
/// small extraction inside the window instead of reserving the whole window
/// for output and being refused before provider I/O.
#[test]
fn memory_output_is_clamped_to_the_context_window() {
	// Arrange
	let config = model(32_768, 32_768);
	let context = json!({"journal": "x".repeat(2_000)});
	let mut request = request(&config, &context);
	let cap = config.output_token_limit() as usize;

	// Act
	let output = fit_memory_output(&mut request, config.context_window, cap).unwrap();

	// Assert
	assert!(output < cap);
	assert_eq!(request.max_output_tokens as usize, output);
	assert_eq!(request.estimated_total_tokens(), config.context_window);
	request.ensure_fits(config.context_window).unwrap();
	// The same inputs always give the same reservation (replay key).
	let mut again = self::request(&config, &context);
	assert_eq!(
		fit_memory_output(&mut again, config.context_window, cap).unwrap(),
		output
	);
}

/// A cap that already fits is kept as it is.
#[test]
fn memory_output_keeps_a_cap_that_fits() {
	// Arrange
	let config = model(128_000, 16_384);
	let mut request = request(&config, &json!({"journal": "short"}));

	// Act
	let output = fit_memory_output(&mut request, config.context_window, 16_384).unwrap();

	// Assert
	assert_eq!(output, 16_384);
}

/// Input that leaves no room for a response is refused.
#[test]
fn memory_output_refuses_input_that_fills_the_window() {
	// Arrange
	let config = model(8_192, 8_192);
	let probe = request(&config, &json!({"journal": ""}));
	let filled = config.context_window - probe.estimated_total_tokens();
	let context = json!({"journal": "x".repeat(filled)});
	let mut request = request(&config, &context);

	// Act
	let result = fit_memory_output(&mut request, config.context_window, 8_192);

	// Assert
	assert!(matches!(result, Err(Error::Invalid(_))));
}
