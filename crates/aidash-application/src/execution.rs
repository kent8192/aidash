//! Execution planning shared by HTTP-started and background-worker runs.
use crate::{Error, Result};
use aidash_domain::{
	context::{self, Context, ContextEvent},
	entities::*,
	model::ModelConfig,
	tool::{Continuation, ResultFitting, ToolContract, ToolUseMode, builtin_contract},
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use uuid::Uuid;
pub const POST_TOOL_CONTEXT_RESERVE: usize = 4096;

pub const TOOL_EVENT_RESERVE: usize = 512;

pub const RUN_MESSAGE_SUMMARY_OUTPUT_LIMIT: u32 = 2048;

#[derive(Clone, Copy)]
pub struct WorkspaceReadFitBudget {
	pub requested: usize,
	pub offset: usize,
	pub request_tokens: usize,
	pub request_window: usize,
	pub remaining_calls: usize,
}

#[derive(Clone, Copy)]
pub struct WorkspaceReadRange {
	pub offset: usize,
	pub requested: usize,
}

pub struct RunMessagePage {
	pub entries: Vec<(usize, bool)>,
	pub has_more: bool,
}

pub fn run_message_page(
	inputs: &[aidash_domain::run_input::RunInput],
	after_seq: i64,
	limit: usize,
	references_only: bool,
) -> Result<RunMessagePage> {
	let mut entries = Vec::new();
	let mut encoded_size = 2_usize; // JSON array brackets.
	let mut content_size = 0_usize;
	let mut has_more = false;
	for (index, input) in inputs
		.iter()
		.enumerate()
		.filter(|(_, input)| input.seq > after_seq)
	{
		let id = input
			.message_id
			.ok_or_else(|| Error::External("run message home delivery is pending".into()))?;
		let full = json!({"seq":input.seq,"sender":input.sender,"content":input.content});
		let full_size = full.to_string().len();
		let reference = json!({"seq":input.seq,"sender":input.sender,"record":{"kind":"message","id":id},"requires_workspace_read":true});
		let mut reference_only = references_only || input.reference_only;
		let mut entry = if reference_only {
			reference.clone()
		} else {
			full
		};
		let comma_size = usize::from(!entries.is_empty());
		let mut next_size = encoded_size
			.saturating_add(comma_size)
			.saturating_add(entry.to_string().len());
		let next_content_size = content_size.saturating_add(full_size);
		if next_size > limit && !reference_only {
			reference_only = true;
			entry = reference;
			next_size = encoded_size
				.saturating_add(comma_size)
				.saturating_add(entry.to_string().len());
		}
		if next_size > limit || (!entries.is_empty() && next_content_size > limit) {
			if entries.is_empty() {
				if next_size > limit {
					return Err(Error::Invalid(
						"run message reference exceeds the model context limit".into(),
					));
				}
			} else {
				has_more = true;
				break;
			}
		}
		encoded_size = next_size;
		content_size = next_content_size;
		entries.push((index, reference_only));
	}
	Ok(RunMessagePage { entries, has_more })
}

pub fn request_context_window(window: usize, minimum_request: usize) -> usize {
	let available = window.saturating_sub(minimum_request);
	let reserve =
		POST_TOOL_CONTEXT_RESERVE.min(available.saturating_sub(context::MIN_CONTEXT_RESERVE) / 4);
	window.saturating_sub(reserve.saturating_mul(2))
}

pub fn message_read_range(event: &ContextEvent) -> Option<(Uuid, usize, usize, usize)> {
	let ContextEvent::Tool { call, .. } = event else {
		return None;
	};
	let contract = builtin_contract(&call.name)?;
	message_read_range_for(&contract, event)
}

pub fn message_read_range_for(
	contract: &ToolContract,
	event: &ContextEvent,
) -> Option<(Uuid, usize, usize, usize)> {
	let ContextEvent::Tool {
		call,
		result: output,
	} = event
	else {
		return None;
	};
	if contract.behavior.fitting != Some(ResultFitting::WorkspaceRecord)
		|| call.arguments["kind"] != "message"
		|| output["kind"] != "message"
		|| output["encoding"] != "json"
		|| call.arguments["id"] != output["id"]
	{
		return None;
	}
	let id = output["id"].as_str()?.parse().ok()?;
	let start = output["offset"].as_u64()? as usize;
	let total = output["total_chars"].as_u64()? as usize;
	let content = output["content"].as_str()?;
	let end = start.checked_add(content.chars().count())?;
	let next = output["next_offset"].as_u64().map(|value| value as usize);
	(end <= total && end > start && next.unwrap_or(total) == end).then_some((id, start, end, total))
}

pub fn record_message_read_in(
	coverage_by_id: &mut BTreeMap<Uuid, context::MessageReadCoverage>,
	event: &ContextEvent,
) {
	let Some((id, start, end, total)) = message_read_range(event) else {
		return;
	};
	merge_message_read(coverage_by_id, id, start, end, total);
}

pub fn record_message_read_for(
	coverage: &mut BTreeMap<Uuid, context::MessageReadCoverage>,
	contract: &ToolContract,
	event: &ContextEvent,
) {
	if let Some((id, start, end, total)) = message_read_range_for(contract, event) {
		merge_message_read(coverage, id, start, end, total);
	}
}

fn merge_message_read(
	coverage_by_id: &mut BTreeMap<Uuid, context::MessageReadCoverage>,
	id: Uuid,
	start: usize,
	end: usize,
	total: usize,
) {
	let coverage = coverage_by_id.entry(id).or_default();
	if coverage.total_chars != total {
		coverage.total_chars = total;
		coverage.ranges.clear();
	}
	coverage.ranges.push([start, end]);
	coverage.ranges.sort_unstable_by_key(|range| range[0]);
	let mut merged: Vec<[usize; 2]> = Vec::with_capacity(coverage.ranges.len());
	for range in coverage.ranges.drain(..) {
		if let Some(last) = merged.last_mut()
			&& range[0] <= last[1]
		{
			last[1] = last[1].max(range[1]);
		} else {
			merged.push(range);
		}
	}
	coverage.ranges = merged;
}

pub fn record_message_read(context: &mut Context, event: &ContextEvent) {
	record_message_read_in(&mut context.message_read_coverage, event);
}

pub fn capture_message_read_coverage(context: &mut Context) {
	for entry in &context.history {
		record_message_read_in(&mut context.message_read_coverage, &entry.event);
	}
}

pub fn capture_message_inference_coverage(context: &mut Context) {
	for entry in &context.history {
		record_message_read_in(&mut context.message_inference_coverage, &entry.event);
	}
}

pub fn coverage_complete(
	coverage_by_id: &BTreeMap<Uuid, context::MessageReadCoverage>,
	id: Uuid,
) -> bool {
	coverage_by_id.get(&id).is_some_and(|coverage| {
		coverage.total_chars > 0
			&& coverage.ranges.len() == 1
			&& coverage.ranges[0] == [0, coverage.total_chars]
	})
}

pub fn referenced_message_read(context: &Context, id: Uuid) -> bool {
	coverage_complete(&context.message_read_coverage, id)
}

pub fn referenced_message_inferred(context: &Context, id: Uuid) -> bool {
	coverage_complete(&context.message_inference_coverage, id)
}

pub fn is_required_message_read(
	call: &aidash_domain::provider::ToolCall,
	required_reads: &[Uuid],
	context: &Context,
) -> bool {
	let Some(contract) = builtin_contract(&call.name) else {
		return false;
	};
	is_required_message_read_for(&contract, call, required_reads, context)
}
pub fn is_required_message_read_for(
	contract: &ToolContract,
	call: &aidash_domain::provider::ToolCall,
	required_reads: &[Uuid],
	context: &Context,
) -> bool {
	if contract.behavior.fitting != Some(ResultFitting::WorkspaceRecord)
		|| call.arguments["kind"] != "message"
	{
		return false;
	}
	let Some(id) = call.arguments["id"]
		.as_str()
		.and_then(|id| id.parse::<Uuid>().ok())
	else {
		return false;
	};
	if !required_reads.contains(&id) || referenced_message_read(context, id) {
		return false;
	}
	let next_offset = context
		.message_read_coverage
		.get(&id)
		.and_then(|coverage| {
			coverage
				.ranges
				.iter()
				.find(|range| range[0] == 0)
				.map(|range| range[1])
		})
		.unwrap_or(0);
	call.arguments["offset"].as_u64().unwrap_or(0) as usize == next_offset
}

pub fn workspace_read_range(
	call: &aidash_domain::provider::ToolCall,
) -> Result<WorkspaceReadRange> {
	let offset = match call.arguments.get("offset") {
		None => 0,
		Some(value) => usize::try_from(value.as_u64().ok_or_else(|| {
			Error::Invalid("workspace read offset must be a nonnegative integer".into())
		})?)
		.map_err(|_| Error::Invalid("workspace read offset is too large".into()))?,
	};
	let requested = match call.arguments.get("max_chars") {
		None => 8000,
		Some(value) => {
			let requested = usize::try_from(value.as_u64().ok_or_else(|| {
				Error::Invalid("workspace read max_chars must be a nonnegative integer".into())
			})?)
			.map_err(|_| Error::Invalid("workspace read max_chars is too large".into()))?;
			if requested > 16_000 {
				return Err(Error::Invalid(
					"workspace read max_chars must not exceed 16000".into(),
				));
			}
			requested
		}
	};
	Ok(WorkspaceReadRange { offset, requested })
}

pub fn workspace_read_plan_result(
	call: &aidash_domain::provider::ToolCall,
	step: i32,
	cursor: usize,
	pending: &ToolCallState,
) -> Result<(WorkspaceReadRange, Option<Value>)> {
	tool_result_plan(pending, ResultFitting::WorkspaceRecord, call, step, cursor)
}

pub fn fit_workspace_read_chars(
	context: &Context,
	call: &aidash_domain::provider::ToolCall,
	output: &Value,
	budget: WorkspaceReadFitBudget,
) -> Result<Option<usize>> {
	Ok(fit_tool_result(
		context,
		call,
		"max_chars",
		budget,
		0,
		|chars| workspace_read_result(output, budget.requested, budget.offset, chars),
	))
}

/// Measure the complete call/result envelope, reserving space for remaining calls.
pub fn tool_result_fits(
	context: &Context,
	call: &aidash_domain::provider::ToolCall,
	parameter: &str,
	size: usize,
	output: Value,
	budget: WorkspaceReadFitBudget,
) -> bool {
	let mut bounded = call.clone();
	bounded.arguments[parameter] = json!(size);
	let event = ContextEvent::tool(bounded, output);
	let maximum = budget
		.request_window
		.saturating_sub(budget.remaining_calls.saturating_mul(TOOL_EVENT_RESERVE));
	budget
		.request_tokens
		.saturating_add(context::tool_event_growth(context, &event))
		<= maximum
}

/// Shared quota search preserves each adapter's minimum viable result size.
pub fn fit_tool_result(
	context: &Context,
	call: &aidash_domain::provider::ToolCall,
	parameter: &str,
	budget: WorkspaceReadFitBudget,
	minimum: usize,
	result: impl Fn(usize) -> Value,
) -> Option<usize> {
	let fits = |size| tool_result_fits(context, call, parameter, size, result(size), budget);
	let minimum = minimum.min(budget.requested);
	if !fits(minimum) {
		return (minimum > 0 && fits(0)).then_some(0);
	}
	let mut low = minimum;
	let mut high = budget.requested;
	while low < high {
		let middle = low + (high - low).div_ceil(2);
		if fits(middle) {
			low = middle;
		} else {
			high = middle - 1;
		}
	}
	Some(low)
}

pub fn workspace_read_result(
	output: &Value,
	requested: usize,
	offset: usize,
	chars: usize,
) -> Value {
	let mut result = output.clone();
	let content: String = output["content"]
		.as_str()
		.unwrap_or_default()
		.chars()
		.take(chars)
		.collect();
	let end = offset.saturating_add(content.chars().count());
	let total = output["total_chars"].as_u64().unwrap_or(0) as usize;
	let limited = chars < requested;
	result["content"] = json!(content);
	result["next_offset"] = (end < total).then_some(json!(end)).unwrap_or(Value::Null);
	result["budget_limited"] = json!(chars == 0 || limited);
	if chars == 0 && limited && offset < total {
		result["deferred"] = json!(true);
		result["message"] = json!(
			"No request budget remains for content. Continue on a later turn; do not repeat this read now."
		);
	} else {
		if let Some(object) = result.as_object_mut() {
			object.remove("deferred");
			object.remove("message");
		}
	}
	result
}

pub fn skill_read_range(call: &aidash_domain::provider::ToolCall) -> Result<WorkspaceReadRange> {
	let offset = match call.arguments.get("offset") {
		None => 0,
		Some(value) => usize::try_from(value.as_u64().ok_or_else(|| {
			Error::Invalid("Skill read offset must be a nonnegative integer".into())
		})?)
		.map_err(|_| Error::Invalid("Skill read offset is too large".into()))?,
	};
	let requested = match call.arguments.get("max_chars") {
		None => 8000,
		Some(value) => usize::try_from(value.as_u64().ok_or_else(|| {
			Error::Invalid("Skill read max_chars must be a nonnegative integer".into())
		})?)
		.map_err(|_| Error::Invalid("Skill read max_chars is too large".into()))?,
	};
	if requested > 16_000 {
		return Err(Error::Invalid(
			"Skill read max_chars must not exceed 16000".into(),
		));
	}
	Ok(WorkspaceReadRange { offset, requested })
}

pub fn skill_read_result(output: &Value, chars: usize) -> Value {
	let mut result = output.clone();
	let original = output["text"].as_str().unwrap_or_default();
	let text: String = original.chars().take(chars).collect();
	let end = text.chars().count();
	let offset = output["offset"].as_u64().unwrap_or(0) as usize;
	let total = output["total_chars"].as_u64().unwrap_or(0) as usize;
	let next = offset.saturating_add(end);
	result["text"] = json!(text);
	result["next_offset"] = (next < total).then_some(json!(next)).unwrap_or(Value::Null);
	result["budget_limited"] = json!(end < original.chars().count());
	if end == 0 && !original.is_empty() {
		result["deferred"] = json!(true);
		result["message"] = json!(
			"No request budget remains for this Skill file. Continue on a later turn; do not repeat this read now."
		);
	}
	result
}

pub fn fit_skill_read_chars(
	context: &Context,
	call: &aidash_domain::provider::ToolCall,
	output: &Value,
	budget: WorkspaceReadFitBudget,
) -> Option<usize> {
	fit_tool_result(context, call, "max_chars", budget, 1, |chars| {
		skill_read_result(output, chars)
	})
}

pub fn force_workspace_read_compaction_window(window: usize, minimum_request: usize) -> usize {
	window
		.saturating_sub(POST_TOOL_CONTEXT_RESERVE)
		.max(minimum_request.min(window))
}

pub fn deferred_workspace_read(call: &aidash_domain::provider::ToolCall) -> Box<DeferredRead> {
	Box::new(DeferredRead {
		message: "Retry this workspace_read after reducing the retained context; its result envelope did not fit.".into(),
		call: call.clone(),
	})
}

pub fn deferred_skill_read(call: &aidash_domain::provider::ToolCall) -> Box<DeferredRead> {
	Box::new(DeferredRead {
		message: "Retry this skill_read after reducing the retained context; its result envelope did not fit.".into(),
		call: call.clone(),
	})
}

pub fn deferred_workspace_observation(
	call: &aidash_domain::provider::ToolCall,
) -> Box<DeferredRead> {
	Box::new(DeferredRead {
		message: "Retry this workspace_observe after reducing the retained context; its page did not fit.".into(),
		call: call.clone(),
	})
}

/// Named durable slots are views of one prepared-result protocol.
pub fn tool_result_plan(
	pending: &ToolCallState,
	fitting: ResultFitting,
	call: &aidash_domain::provider::ToolCall,
	step: i32,
	cursor: usize,
) -> Result<(WorkspaceReadRange, Option<Value>)> {
	let range = match fitting {
		ResultFitting::WorkspaceRecord => workspace_read_range(call)?,
		ResultFitting::SkillText => skill_read_range(call)?,
		ResultFitting::Observation => WorkspaceReadRange {
			offset: call.arguments["offset"].as_u64().unwrap_or(0) as usize,
			requested: call.arguments["limit"]
				.as_u64()
				.unwrap_or(context::observation::DEFAULT_LIMIT as u64) as usize,
		},
	};
	let saved = result_plan(pending, fitting)
		.as_ref()
		.filter(|plan| plan.step == step && plan.cursor == cursor && &plan.call == call)
		.map(|plan| plan.result.clone());
	Ok((range, saved))
}

pub fn result_plan(pending: &ToolCallState, fitting: ResultFitting) -> &Option<ReadPlan> {
	match fitting {
		ResultFitting::WorkspaceRecord => &pending.workspace_read_plan,
		ResultFitting::SkillText => &pending.skill_read_plan,
		ResultFitting::Observation => &pending.workspace_observation_plan,
	}
}
pub fn result_plan_mut(
	pending: &mut ToolCallState,
	fitting: ResultFitting,
) -> &mut Option<ReadPlan> {
	match fitting {
		ResultFitting::WorkspaceRecord => &mut pending.workspace_read_plan,
		ResultFitting::SkillText => &mut pending.skill_read_plan,
		ResultFitting::Observation => &mut pending.workspace_observation_plan,
	}
}
pub fn defer_result(
	fitting: ResultFitting,
	call: &aidash_domain::provider::ToolCall,
	selected_media: Vec<aidash_domain::media::Selection>,
) -> (ThinkingState, &'static str) {
	let mut state = ThinkingState {
		force_workspace_read_compaction: true,
		selected_media,
		..Default::default()
	};
	let event = match fitting {
		ResultFitting::WorkspaceRecord => {
			state.deferred_workspace_read = Some(deferred_workspace_read(call));
			"run.read_deferred"
		}
		ResultFitting::SkillText => {
			state.deferred_skill_read = Some(deferred_skill_read(call));
			"run.skill_read_deferred"
		}
		ResultFitting::Observation => {
			state.deferred_workspace_observation = Some(deferred_workspace_observation(call));
			"run.observation_deferred"
		}
	};
	(state, event)
}

pub fn workspace_observation_event_fits(
	context: &Context,
	call: &aidash_domain::provider::ToolCall,
	limit: usize,
	output: &Value,
	request_tokens: usize,
	request_window: usize,
	remaining_calls: usize,
) -> bool {
	tool_result_fits(
		context,
		call,
		"limit",
		limit,
		output.clone(),
		WorkspaceReadFitBudget {
			requested: limit,
			offset: 0,
			request_tokens,
			request_window,
			remaining_calls,
		},
	)
}

pub fn result_artifact_name(title: &str) -> String {
	let limit = 64_000 - " result".len();
	let mut end = title.len().min(limit);
	while !title.is_char_boundary(end) {
		end -= 1;
	}
	format!("{} result", &title[..end])
}

pub fn choose_inference_media(
	selected_parts: Vec<aidash_domain::provider::ContentPart>,
	human_parts: Vec<aidash_domain::provider::ContentPart>,
	headroom: usize,
	human_batch_present: bool,
	model: &ModelConfig,
) -> (Vec<aidash_domain::provider::ContentPart>, bool) {
	let mut combined = selected_parts;
	combined.extend(human_parts.iter().cloned());
	let defer_selected = human_batch_present
		&& (!aidash_domain::provider::ModelRequest::media_within_limits(&combined)
			|| media_request_headroom(headroom, &combined).is_err()
			|| !model.has_current_media_route_for_parts(&combined));
	(
		if defer_selected {
			human_parts
		} else {
			combined
		},
		defer_selected,
	)
}

pub fn media_request_headroom(
	headroom: usize,
	parts: &[aidash_domain::provider::ContentPart],
) -> Result<()> {
	let request = aidash_domain::provider::ModelRequest {
		instructions: String::new(),
		context: json!({}).into(),
		tools: Vec::new(),
		max_output_tokens: 0,
		response_format: None,
		content_parts: Vec::new(),
		cache_scope: None,
		cache_breakpoints: false,
	};
	request
		.ensure_fits_with_parts(headroom, parts)
		.map_err(Into::into)
}

pub fn encoded_run_message_reservation(messages: &[Value]) -> usize {
	if messages.is_empty() {
		return 0;
	}
	let estimate = |current: Value| {
		aidash_domain::provider::ModelRequest {
			instructions: String::new(),
			context: json!({
				"current":current,
				"summary":"",
				"run_message_summary":"",
				"history":[]
			})
			.into(),
			tools: Vec::new(),
			max_output_tokens: 0,
			response_format: None,
			content_parts: Vec::new(),
			cache_scope: None,
			cache_breakpoints: false,
		}
		.estimated_total_tokens()
	};
	estimate(json!({"run_messages":messages})).saturating_sub(estimate(json!({})))
}

pub fn read_only_after_model_media_selection(name: &str) -> bool {
	builtin_contract(name)
		.is_some_and(|contract| contract.behavior.permits(ToolUseMode::MediaPending))
}

pub fn pending_selected_media(pending: &ToolCallState) -> Vec<aidash_domain::media::Selection> {
	pending.selected_media()
}

pub fn check_model_media_headroom(
	headroom: usize,
	parts: Vec<aidash_domain::provider::ContentPart>,
	model: &ModelConfig,
) -> Result<()> {
	let request = aidash_domain::provider::ModelRequest {
		instructions: String::new(),
		context: json!({}).into(),
		tools: Vec::new(),
		max_output_tokens: 0,
		response_format: None,
		content_parts: parts,
		cache_scope: None,
		cache_breakpoints: false,
	};
	request.validate()?;
	if !model.has_current_media_route_for_parts(&request.content_parts) {
		return Err(Error::Invalid(format!(
			"recipient model {} has no current media route for every selected format",
			model.model_id
		)));
	}
	request.ensure_fits(headroom).map_err(Into::into)
}

pub fn media_observation_budget(pending: &ToolCallState) -> usize {
	(pending.request_window / 16).clamp(256, 4096)
}

pub fn record_media_observation(
	context: &mut Context,
	text: &str,
	through_seq: Option<i64>,
	budget: usize,
) {
	let source = text.trim();
	let mut end = source.len().min(1024);
	while !source.is_char_boundary(end) {
		end -= 1;
	}
	loop {
		let event = ContextEvent::ModelMediaObservation {
			text: source[..end].into(),
			through_seq,
			truncated: end < source.len(),
		};
		if event.encoded_len() <= budget {
			context.push(event);
			break;
		}
		end -= 1;
		while !source.is_char_boundary(end) {
			end -= 1;
		}
	}
	let mut used = 0_usize;
	let mut remove = Vec::new();
	for index in (0..context.history.len()).rev() {
		let event = &context.history[index].event;
		if matches!(event, ContextEvent::ModelMediaObservation { .. }) {
			let bytes = event.to_string().len();
			if used.saturating_add(bytes) > budget {
				remove.push(index);
			} else {
				used += bytes;
			}
		}
	}
	for index in remove {
		context.history.remove(index);
	}
}

pub fn stale_media_pending(
	context: &mut Context,
	pending: &ToolCallState,
	observed_input_seq: &mut i64,
) -> ThinkingState {
	pending.stale(context, observed_input_seq)
}

pub fn response_epoch(revision: i64, step: i32) -> i64 {
	revision.saturating_add(i64::from(step)).saturating_add(1)
}

pub enum FrameworkResult {
	Ordinary,
	Approval(Uuid),
	Human(Uuid),
	Wait(i64),
}

impl FrameworkResult {
	pub fn decode(contract: &ToolContract, output: &Value) -> Result<Self> {
		#[derive(serde::Deserialize)]
		struct ApprovalView {
			approval_id: Uuid,
		}
		#[derive(serde::Deserialize)]
		struct HumanView {
			human_request_id: Option<Uuid>,
		}
		#[derive(serde::Deserialize)]
		struct WaitView {
			wait_seconds: Option<i64>,
		}
		#[derive(serde::Deserialize)]
		#[serde(rename_all = "snake_case")]
		enum FrameworkStatus {
			ApprovalRequired,
			#[serde(other)]
			Other,
		}
		let status = output
			.get("status")
			.cloned()
			.and_then(|value| serde_json::from_value::<FrameworkStatus>(value).ok());
		if matches!(status, Some(FrameworkStatus::ApprovalRequired)) {
			let view: ApprovalView = serde_json::from_value(output.clone())
				.map_err(|_| Error::Invalid("approval result is missing a valid ID".into()))?;
			return Ok(Self::Approval(view.approval_id));
		}
		match contract.behavior.continuation {
			Continuation::Human => {
				let view: HumanView = serde_json::from_value(output.clone())?;
				Ok(view.human_request_id.map_or(Self::Ordinary, Self::Human))
			}
			Continuation::Wait => {
				let view: WaitView = serde_json::from_value(output.clone())?;
				Ok(view.wait_seconds.map_or(Self::Ordinary, Self::Wait))
			}
			_ => Ok(Self::Ordinary),
		}
	}
}
/// Return a complete UTF-8 prefix, including one character when the byte quota is tiny.
pub fn bounded_utf8_end(text: &str, start: usize, max_bytes: usize) -> usize {
	let mut end = start.saturating_add(max_bytes).min(text.len());
	while !text.is_char_boundary(end) {
		end -= 1;
	}
	if max_bytes > 0 && end == start && start < text.len() {
		return start + text[start..].chars().next().unwrap().len_utf8();
	}
	end
}

#[cfg(test)]
mod tests;

pub mod media;

pub mod semantic_context;

pub mod cancellation;

pub mod admission;

pub mod summary;

pub mod headroom;
