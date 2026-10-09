use super::*;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Mutex;

struct FakeJev {
	answer: fn(&str) -> Value,
	seen: Mutex<Vec<(Value, jev::Questions)>>,
}
impl FakeJev {
	fn new(answer: fn(&str) -> Value) -> Self {
		Self {
			answer,
			seen: Mutex::new(Vec::new()),
		}
	}
}
#[async_trait]
impl jev::JevAsker for FakeJev {
	async fn ask(&self, state: &Value, questions: &jev::Questions) -> Result<Value> {
		self.seen
			.lock()
			.unwrap()
			.push((state.clone(), questions.clone()));
		let answers: serde_json::Map<_, _> = questions
			.keys()
			.map(|key| (key.clone(), (self.answer)(key)))
			.collect();
		Ok(json!({"answers":answers}))
	}
}
fn drop_all(_: &str) -> Value {
	json!({"noul":0.0})
}

#[rstest::rstest]
#[tokio::test]
async fn legacy_observation_projection_preserves_human_records_and_failed_contexts() {
	let snapshot = json!({
		"workspace":{"id":uuid::Uuid::new_v4(),"title":"Airline","goal":"Plan","state":{},"revision":0,"created_at":chrono::Utc::now()},
		"tasks":[],"artifacts":[],"messages":[],
		"events":[{"sequence":1,"id":uuid::Uuid::new_v4(),"node_id":"aidash://test","workspace_id":null,"kind":"tool.completed","data":{"result":"nested".repeat(5000)},"created_at":chrono::Utc::now()}]
	});
	let mut context = Context {
		history: vec![
			event(
				json!({"kind":"tool","call":{"id":"observe","name":"workspace_observe","arguments":{}},"result":snapshot}),
			),
			ContextEvent::Human {
				request: "Reject action".into(),
				request_kind: "APPROVAL_REQUIRED".into(),
				response: json!({"approved":false,"data":snapshot}),
			},
		],
		..Default::default()
	};
	let before = json!(context);
	let asker = FakeJev::new(drop_all);
	assert!(
		compact(&mut context, &asker, 1, &json!({}), "")
			.await
			.is_err()
	);
	assert_eq!(json!(context), before);
	compact(&mut context, &asker, 100000, &json!({}), "")
		.await
		.unwrap();
	assert_eq!(
		json!(context.history[0])["result"]["view"],
		"workspace_observation_v1"
	);
	assert!(
		json!(context.history[0])["result"]["events"][0]
			.get("data")
			.is_none()
	);
	assert_eq!(
		json!(context.history[0])["call"],
		before["history"][0]["call"]
	);
	assert_eq!(json!(context.history[1]), before["history"][1]);
	assert_eq!(context.compactions, 0);
}
async fn compact(
	context: &mut Context,
	asker: &dyn jev::JevAsker,
	window: usize,
	pinned: &Value,
	instructions: &str,
) -> Result<()> {
	super::compact(
		context,
		asker,
		&RequestBudget {
			window,
			instructions,
			tools: &[],
			max_output_tokens: 256,
			projection: Default::default(),
		},
		pinned,
	)
	.await
}
fn event(value: Value) -> ContextEvent {
	serde_json::from_value(value).unwrap()
}
