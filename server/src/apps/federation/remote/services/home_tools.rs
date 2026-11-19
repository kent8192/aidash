//! Home-forwarded tools of a Run that a Remote Grant started (ADR 0126, 0139).
//!
//! Each call is a scoped command to the Home, which executes it under its own authority for
//! the delegation chain. The command names the Run and its call key and nothing that steers
//! the Home: the memory banks, the assignee's Node and the Workspace all come from the grant.
use super::runtime::Home;
use crate::Result;
use aidash_domain::{
	HumanRequest,
	memory::{Change, RecallQuery, Unit},
};
use serde_json::{Value, json};
use uuid::Uuid;

impl Home {
	/// `task_assign`, served by the Home's Generation policy. The key follows the task, the
	/// policy and the reason, so a resumed Run repeats the same command and a changed request
	/// is a new one that the Home judges as it does a local change.
	pub(crate) async fn forward_assign(
		&self,
		task: Uuid,
		policy: &str,
		reason: &str,
	) -> Result<Value> {
		let digest = crate::registry::digest(&json!([policy, reason]));
		let key = format!("run:{}:assign:{}:{}", self.run.id, task, digest);
		self.command(
			"task_assign",
			json!({"run_id":self.run.id,"key":key,"task_id":task,"policy":policy,"reason":reason}),
		)
		.await
	}
	/// `workspace_search` over the Home's Workspace, by text.
	pub(crate) async fn forward_workspace_search(&self, input: Value) -> Result<Value> {
		self.command(
			"workspace_search",
			json!({"run_id":self.run.id,"input":input}),
		)
		.await
	}
	/// `connection_request`: the Home's person at the root of the chain is asked to connect.
	pub(crate) async fn forward_connection_request(
		&self,
		prompt: &str,
		key: &str,
	) -> Result<HumanRequest> {
		self.command(
			"connection_request",
			json!({"run_id":self.run.id,"key":key,"prompt":prompt}),
		)
		.await
	}
	/// `memory_recall` and `memory_reflect`: the Home's published bank and this Agent's
	/// participant bank.
	pub(crate) async fn forward_memory_recall(
		&self,
		key: &str,
		query: &RecallQuery,
		reflect: bool,
	) -> Result<Value> {
		self.command(
			if reflect {
				"memory_reflect"
			} else {
				"memory_recall"
			},
			json!({"run_id":self.run.id,"key":key,"query":query}),
		)
		.await
	}
	/// `memory_mutate`: the Home writes only this Agent's participant bank.
	pub(crate) async fn forward_memory_mutate(
		&self,
		key: &str,
		changes: &[Change],
	) -> Result<Vec<Unit>> {
		self.command(
			"memory_mutate",
			json!({"run_id":self.run.id,"key":key,"changes":changes}),
		)
		.await
	}
}
