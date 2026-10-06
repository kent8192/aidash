use super::*;

#[rstest::rstest]
#[tokio::test]
async fn independent_database_tiers_converge_after_partial_partition(
	#[future(awt)]
	#[from(common::isolated_test_environment)]
	environment: Arc<TestEnvironment>,
	#[values(3, 16)] count: usize,
	#[values(false, true)] abort: bool,
) {
	let mut nodes = Vec::new();
	let mut workspaces = Vec::new();
	for index in 0..count {
		let node = Node::new(&environment, &format!("tier-{index:02}")).await;
		workspaces.push(
			node.f
				.store
				.create_workspace("Atomic tier", "Independent participant")
				.await
				.unwrap()
				.id,
		);
		nodes.push(node);
	}
	for index in 1..count {
		for (local, remote) in [(&nodes[0], &nodes[index]), (&nodes[index], &nodes[0])] {
			local
				.f
				.register_peer(Peer {
					node_id: remote.f.config.node_id.clone(),
					endpoint: remote.f.config.endpoint.clone(),
					credential_env: format!("AIDASH_SECRET_TRANSACTION_{index:02}"),
					protocol_version: "0.1".into(),
					enabled: true,
				})
				.await
				.unwrap();
			assert_eq!(
				local
					.request(
						reqwest::Method::POST,
						"/api/transactions/trust",
						Some(json!({"node_id":remote.f.config.node_id,"enabled":true}))
					)
					.await
					.0,
				200
			);
		}
	}
	for repetition in 0..3 {
		let manifest:Manifest=serde_json::from_value(json!({"id":Uuid::new_v4(),"coordinator":nodes[0].f.config.node_id,
            "isolation":"serializable","deadline":Utc::now()+Duration::minutes(5),
            "participants":nodes.iter().zip(&workspaces).map(|(node,workspace)|json!({"node_id":node.f.config.node_id,
                "mutations":[{"kind":"workspace_state","workspace_id":workspace,"expected_revision":if abort {0}else{repetition},"state":{"repetition":repetition}}]})).collect::<Vec<_>>() })).unwrap();
		coordinator::submit(&nodes[0].f, &manifest).await.unwrap();
		steps(&nodes[0], manifest.id, count * 2).await;
		if abort {
			coordinator::abort(&nodes[0].f, manifest.id).await.unwrap();
		} else {
			steps(&nodes[0], manifest.id, 1).await;
		}
		nodes[1].stop().await;
		steps(&nodes[0], manifest.id, 2).await;
		for (node, workspace) in nodes.iter().zip(&workspaces).skip(2) {
			assert_eq!(
				node.get(&format!("/api/workspaces/{workspace}")).await.0,
				503
			);
		}
		assert!(
			!coordinator::status(&nodes[0].f, manifest.id)
				.await
				.unwrap()
				.complete
		);
		nodes[1].restart().await;
		let restored = std::time::Instant::now();
		for _ in 0..count * 4 + 5 {
			if coordinator::advance(&nodes[0].f, manifest.id)
				.await
				.unwrap()
				.complete
			{
				break;
			}
		}
		assert!(restored.elapsed() < std::time::Duration::from_secs(180));
		let result = coordinator::status(&nodes[0].f, manifest.id).await.unwrap();
		assert!(result.complete, "{result:?}");
		assert_eq!(
			result.decision.as_deref(),
			Some(if abort { "ABORT" } else { "COMMIT" })
		);
		for (node, workspace) in nodes.iter().zip(&workspaces) {
			assert_eq!(
				node.f.store.workspace(*workspace).await.unwrap().revision,
				if abort { 0 } else { repetition + 1 }
			);
			assert_eq!(
				node.get(&format!("/api/workspaces/{workspace}")).await.0,
				200
			);
		}
	}
	for node in nodes {
		node.cleanup().await;
	}
}
