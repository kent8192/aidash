use super::*;

fn legacy_context() -> Value {
	json!({
		"summary": "",
		"run_message_summary": "",
		"run_message_summary_seq": 0,
		"media_inferred_seq": 0,
		"history": [
			{"kind":"tool","call":{"id":"a","name":"echo","arguments":{}},"result":{"ok":true}},
			{"kind":"run_message_read_required","message_ids":[]}
		],
		"usage": null,
		"compactions": 0,
		"message_read_coverage": {},
		"message_inference_coverage": {}
	})
}

#[test]
fn legacy_history_is_imported_with_journal_sequences() {
	let context: Context = serde_json::from_value(legacy_context()).unwrap();
	assert_eq!(
		context.journal,
		JournalCursor {
			head: 2,
			imported_through: 2,
			inferred_through: 0
		}
	);
	assert_eq!(
		context.history.iter().map(|e| e.seq).collect::<Vec<_>>(),
		[1, 2]
	);
	let stored = serde_json::to_value(&context).unwrap();
	assert!(stored.get("summary").is_none());
	let reloaded: Context = serde_json::from_value(stored).unwrap();
	assert_eq!(reloaded.history, context.history);
	assert_eq!(reloaded.journal, context.journal);
}

#[test]
fn push_assigns_monotonic_sequences_and_rejects_disorder() {
	let mut context: Context = serde_json::from_value(legacy_context()).unwrap();
	context.push(ContextEvent::RunMessageReadRequired {
		message_ids: vec![],
	});
	assert_eq!(context.history.last().unwrap().seq, 3);
	assert_eq!(context.unjournaled(2).count(), 1);
	let mut stored = serde_json::to_value(&context).unwrap();
	stored["history"][0]["seq"] = json!(9);
	assert!(serde_json::from_value::<Context>(stored).is_err());
}

#[test]
fn a_stored_legacy_summary_stays_model_visible_across_saves() {
	let mut stored = legacy_context();
	stored["summary"] = json!("Never deploy on Fridays");
	let context: Context = serde_json::from_value(stored).unwrap();
	let pinned = json!({"task":{"id":"t"}});
	let projected = crate::projection::native::native_context(&context, &pinned);
	assert!(projected.attachments.iter().any(|envelope| {
		envelope.source == crate::projection::native::Source::ExecutionSummary
			&& envelope.content == json!("Never deploy on Fridays")
	}));
	// The next save keeps the text instead of dropping it.
	let saved = serde_json::to_value(&context).unwrap();
	assert_eq!(saved["summary"], json!("Never deploy on Fridays"));
	let reloaded: Context = serde_json::from_value(saved).unwrap();
	assert_eq!(reloaded.legacy_summary, "Never deploy on Fridays");
}
