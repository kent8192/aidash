use super::*;
use async_trait::async_trait;
use std::sync::Mutex;

#[derive(Default)]
struct Transport(Mutex<Vec<(String, String, Option<Value>)>>);

#[async_trait]
impl RunnerTransport for Transport {
	async fn request(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value> {
		self.0
			.lock()
			.unwrap()
			.push((method.into(), path.into(), body.cloned()));
		Ok(if path == "/v1/operations" {
			json!({"status":"awaiting_files"})
		} else {
			json!({"status":"accepted"})
		})
	}
}

#[tokio::test]
async fn submits_the_exact_export_as_a_dedicated_read_only_journal_input() {
	let transport = Transport::default();
	let cell = Cell {
		operation: Uuid::new_v4(),
		area: Uuid::new_v4(),
		session: Uuid::new_v4(),
		runner_instance: "runner-one",
		file: Uuid::new_v4(),
		epoch: 1,
		key_digest: "intent",
		tenant: "acme",
		code: "journal()",
		seconds: 10,
		export: b"{\"seq\":1}\n",
	};
	assert_eq!(
		submit(&transport, &cell).await.unwrap()["status"],
		"accepted"
	);
	let calls = transport.0.lock().unwrap();
	let request = calls[0].2.as_ref().unwrap();
	assert_eq!(request["kind"], "journal");
	assert_eq!(request["runner_instance"], "runner-one");
	assert_eq!(request["files"][0]["scope"], "journal");
	assert_eq!(
		request["files"][0]["digest"],
		format!("{:x}", Sha256::digest(cell.export))
	);
	assert!(request.get("egress").is_none());
	assert_eq!(
		STANDARD
			.decode(calls[1].2.as_ref().unwrap()["data"].as_str().unwrap())
			.unwrap(),
		cell.export
	);
	assert!(calls[2].1.ends_with("/start"));
}

#[test]
fn output_is_bounded_at_a_utf8_boundary_and_requires_a_frozen_kernel() {
	let observed = json!({"status":"completed","writer_frozen":true,"stdout":STANDARD.encode("ab🌸c"),"truncated":false});
	let result = output(&observed, 5).unwrap();
	assert_eq!(result["stdout"], "ab");
	assert_eq!(result["truncated"], true);
	assert!(output(&json!({"status":"completed","writer_frozen":false}), 100).is_err());
	let failed = json!({"status":"failed","writer_frozen":true,"stdout":STANDARD.encode("partial"),"error":{"message":"bad expression"}});
	let result = output(&failed, 100).unwrap();
	assert_eq!(result["stdout"], "partial");
	assert_eq!(result["error"]["message"], "bad expression");
}
