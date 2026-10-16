//! Real PostgreSQL regression: MMR reads the current vector generation and only reorders
//! already authorized candidates.
use super::*;

/// Paraphrases share one direction; the manager fact is orthogonal to it.
fn embedding(text: &str) -> [f32; 3] {
	if text.contains("manager") {
		[0., 1., 0.]
	} else {
		[1., 0.05, 0.]
	}
}

async fn recalled(database: &DatabaseFixture, endpoint: &str, lambda: Option<u32>) -> Vec<String> {
	let mut bounds = bounds();
	bounds.max_results = 2;
	let (store, _, workspace) = setup_endpoint_policy(
		database,
		bounds,
		endpoint,
		(false, false, false),
		None,
		None,
		lambda,
	)
	.await;
	let bank = memory::create_participant(
		&store,
		&Actor::DeploymentAdministrator,
		workspace,
		memory::CreateParticipant {
			agent: reference("a"),
		},
	)
	.await
	.unwrap()
	.bank;
	let texts = [
		"Alice works in Tokyo",
		"Alice is employed in Tokyo",
		"Tokyo is where Alice works",
		"Alice's manager is Bob",
	];
	let mut changes = vec![];
	for text in texts {
		changes.push(Change::Add {
			id: Uuid::now_v7(),
			content: content(text),
		});
	}
	let mut admission = mutation(&bank, changes.remove(0));
	admission.changes.extend(changes);
	let admitted = memory::mutate(&store, &Actor::DeploymentAdministrator, admission)
		.await
		.unwrap();
	tokio::time::timeout(std::time::Duration::from_secs(20), async {
		loop {
			aidash_server::semantic::worker::sweep(&store)
				.await
				.unwrap();
			let mut ready = 0;
			for unit in &admitted {
				let state: String = aidash_server::database::native::query_scalar(
					&Query::select()
						.column(Alias::new("state"))
						.from(Alias::new("semantic_entries"))
						.and_where(Expr::col("id").eq(Expr::value(unit.id)))
						.to_string(PostgresQueryBuilder),
				)
				.scalar_one(&store.pool)
				.await
				.unwrap();
				assert_ne!(state, "ERROR", "native vector indexing failed");
				ready += usize::from(state == "READY");
			}
			if ready == admitted.len() {
				break;
			}
			tokio::time::sleep(std::time::Duration::from_millis(50)).await;
		}
	})
	.await
	.expect("the native vector projection must become ready");
	let memory::Outcome::Recall(Recall::Ready { units }) = memory::operate(
		&store,
		&Actor::DeploymentAdministrator,
		memory::Operation {
			operation_id: Uuid::now_v7(),
			provider: reference("p"),
			bank,
			action: memory::Action::Recall {
				query: RecallQuery {
					text: "Where does Alice work?".into(),
					time: None,
					kinds: vec![],
					max_tokens: 8192,
				},
			},
		},
	)
	.await
	.unwrap() else {
		panic!("recall must deliver units");
	};
	let authorized = admitted.iter().map(|unit| unit.id).collect::<Vec<_>>();
	assert!(units.iter().all(|unit| authorized.contains(&unit.id)));
	units.into_iter().map(|unit| unit.content.text).collect()
}

#[rstest::fixture]
async fn diversity_embedding_provider() -> reinhardt::test::fixtures::TestServerGuard {
	let router = reinhardt::test::stub::StubRouter::new()
		.route("/v1/embeddings", http::Method::POST, |request| async move {
			let input = request.json::<serde_json::Value>().unwrap();
			let text = input["input"].as_str().unwrap_or_default().to_owned();
			Ok(reinhardt::Response::ok()
				.with_json(&json!({"model":input["model"],"data":[{"index":0,"embedding":embedding(&text)}],"usage":{"prompt_tokens":1,"total_tokens":1}}))
				.unwrap())
		})
		.into_server_router();
	reinhardt::test::fixtures::test_server_guard(router).await
}

#[rstest]
#[case::unset_policy_keeps_relevance_order(None, false)]
#[case::pinned_lambda_delivers_the_distinct_unit(Some(700_000), true)]
#[tokio::test]
async fn diversity_uses_current_vectors_to_replace_a_paraphrase_with_the_distinct_unit(
	#[future] diversity_embedding_provider: reinhardt::test::fixtures::TestServerGuard,
	#[future] database: DatabaseFixture,
	#[case] lambda: Option<u32>,
	#[case] distinct_delivered: bool,
) {
	let model = diversity_embedding_provider.await;
	let endpoint = format!("{}/v1", model.url);
	let database = database.await;
	let delivered = recalled(&database, &endpoint, lambda).await;
	assert_eq!(delivered.len(), 2, "{delivered:?}");
	let distinct = delivered
		.iter()
		.filter(|text| text.contains("manager"))
		.count();
	assert_eq!(distinct, usize::from(distinct_delivered), "{delivered:?}");
}
