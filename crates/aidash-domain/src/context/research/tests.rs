use super::*;
use crate::{
	context::{Context, summary::entries_digest},
	provider::ToolCall,
};

fn delivered() -> HistoryEntry {
	let mut context = Context::default();
	context.push_invocation(ContextEvent::tool(ToolCall {
		id: "call-1".into(), name: "page".into(), arguments: json!({"source_id":"source"}),
	}, json!({"contract":crate::research::RESULT_CONTRACT,"operation":"fetch","data":{
		"document_id":"f93043b8-4965-4c40-b04d-685165324f2e","text":"exact provider passage: 日本語\n\"quoted\"","score":0.0000001
	}})), "run:3:0");
	context.history.remove(0)
}

#[test]
fn durable_history_has_only_identity_digest_and_provenance() {
	let entry = delivered();
	let bytes = serde_json::to_vec(&entry).unwrap();
	assert!(!String::from_utf8_lossy(&bytes).contains("exact provider passage"));
	let stored: HistoryEntry = serde_json::from_slice(&bytes).unwrap();
	let reference = stored.research_reference().unwrap();
	assert_eq!(reference.invocation, "run:3:0");
	assert_eq!(reference.documents.len(), 1);
	assert_eq!(serde_json::to_vec(&stored).unwrap(), bytes);
	let ContextEvent::Tool { result, .. } = &entry.event else {
		panic!()
	};
	assert_eq!(reference.digest, crate::registry::rules::digest(result));
	assert!(result.to_string().contains("exact provider passage"));
}

#[test]
fn ordinary_reference_contract_results_survive_context_save_and_replay() {
	for result in [
		json!({"contract": CONTRACT}),
		json!({"contract": CONTRACT, "digest": "tool-defined", "status": "complete"}),
	] {
		let mut context = Context::default();
		let event = ContextEvent::tool(
			ToolCall {
				id: "ordinary-call".into(),
				name: "ordinary-tool".into(),
				arguments: json!({}),
			},
			result,
		);
		context.push(event.clone());
		let before = entries_digest(&context.history);
		let bytes = serde_json::to_vec(&context).unwrap();
		let restored: Context = serde_json::from_slice(&bytes)
			.expect("ordinary tool JSON must remain readable after saving a Context");
		assert_eq!(restored.history[0].event, event);
		assert!(restored.history[0].research_reference().is_none());
		assert_eq!(entries_digest(&restored.history), before);
		assert_eq!(serde_json::to_vec(&restored).unwrap(), bytes);
	}
}

#[test]
fn malformed_internal_references_still_fail_integrity_validation() {
	let stored = serde_json::to_value(delivered()).unwrap();
	for (field, value) in [
		("digest", Some(json!("not-a-digest"))),
		("invocation", Some(json!(""))),
		("operation", Some(json!("unknown"))),
		("sources", Some(json!(["not-a-uuid"]))),
		("documents", None),
	] {
		let mut corrupted = stored.clone();
		let result = corrupted["event"]["result"].as_object_mut().unwrap();
		if let Some(value) = value {
			result.insert(field.into(), value);
		} else {
			result.remove(field);
		}
		let error = serde_json::from_value::<HistoryEntry>(corrupted).unwrap_err();
		assert!(
			error
				.to_string()
				.contains("invalid Research journal reference")
		);
	}
	let mut marked_reference = stored;
	marked_reference["inline_result"] = json!(true);
	let error = serde_json::from_value::<HistoryEntry>(marked_reference).unwrap_err();
	assert!(
		error
			.to_string()
			.contains("invalid inline Research result marker")
	);
}

#[test]
fn purged_values_keep_journal_and_summary_range_commitments() {
	let entry = delivered();
	let digest = entries_digest(std::slice::from_ref(&entry));
	for placeholder in [
		ContextEvent::expired_result("call-1"),
		ContextEvent::revoked_result("call-1"),
		entry.research_reference().unwrap().unavailable(),
	] {
		let mut purged = entry.clone();
		let ContextEvent::Tool { result, .. } = &mut purged.event else {
			panic!()
		};
		*result = placeholder;
		assert_eq!(entries_digest(&[purged]), digest);
	}
}

#[test]
fn shortened_copies_keep_provenance_and_have_their_own_exact_digest() {
	let original = delivered();
	let mut shortened = original.clone();
	let ContextEvent::Tool { result, .. } = &mut shortened.event else {
		panic!()
	};
	*result = json!("exact provider passage… shortened");
	let before = original.research_reference().unwrap();
	let after = shortened.research_reference().unwrap();
	assert_ne!(before.digest, after.digest);
	assert_eq!(before.documents, after.documents);
	assert_eq!(before.invocation, after.invocation);
	assert!(
		!serde_json::to_string(&shortened)
			.unwrap()
			.contains("shortened")
	);
}
