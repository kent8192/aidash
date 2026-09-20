use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

mod compaction;
pub mod jev;

#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Context {
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub history: Vec<Value>,
    #[serde(default)]
    #[schema(value_type = Option<ContextUsage>)]
    pub usage: Value,
    #[serde(default)]
    pub compactions: u32,
}

// Conservative upper bound for mixed-language text, not a provider tokenizer.
// The budget includes system instructions, tools, workspace and output reserve.
pub fn estimated_tokens(value: &str) -> usize {
    value
        .chars()
        .map(|c| if c.is_ascii() { 1 } else { 2 })
        .sum()
}

pub async fn compact(
    context: &mut Context,
    asker: &dyn jev::JevAsker,
    budget: usize,
    pinned: &Value,
    instructions: &str,
) -> Result<()> {
    let size = |c: &Context| {
        estimated_tokens(
            &json!({"current":pinned,"summary":c.summary,"history":c.history}).to_string(),
        )
    };
    if size(context) <= budget {
        return Ok(());
    }
    let classification_context = json!({
        "instructions":instructions, "current":pinned, "previous_summary":context.summary
    });
    let compacted = compaction::prune(
        &context.history,
        &classification_context,
        asker,
        &compaction::Options::default(),
    )
    .await?;
    let mut candidate = context.clone();
    candidate.history = compacted.history;
    // No summarization fallback: legacy summaries and all non-tool events stay
    // verbatim. Apply nothing unless the complete inference context fits.
    if size(&candidate) > budget {
        return Err(Error::Invalid(
            "Jev compaction could not fit the pinned context and retained history".into(),
        ));
    }
    candidate.compactions += 1;
    tracing::info!(
        requests = compacted.requests,
        stage = compacted.stage,
        calls_dropped = compacted.calls_dropped,
        results_truncated = compacted.results_truncated,
        "Jev context compaction completed"
    );
    *context = candidate;
    Ok(())
}

#[cfg(test)]
mod tests;

#[derive(utoipa::ToSchema)]
pub struct ContextUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub context_window: usize,
    pub compactions: u32,
}
