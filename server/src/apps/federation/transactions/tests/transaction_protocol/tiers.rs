use super::*;

#[rstest::rstest]
#[tokio::test]
async fn independent_database_tiers_converge_after_partial_partition(
	#[values(3, 16)] count: usize,
	#[values(false, true)] abort: bool,
	#[future(awt)]
	#[from(tier_nodes)]
	#[with(count)]
	tier_nodes: Vec<Node>,
) {
	let mut nodes = tier_nodes;
	let mut workspaces = Vec::new();
	for node in &nodes {
		workspaces.push(
			node.f
				.store
				.create_workspace("Atomic tier", "Independent participant")
				.await
				.unwrap()
				.id,
		);
	}
	for index in 1..count {
		for (local, remote) in [(&nodes[0], &nodes[index]), (&nodes[index], &nodes[0])] {
			local
				.f
				.register_peer(Peer {
					node_id: remote.f.config.node_id.clone(),
					endpoint: remote.f.config.endpoint.clone(),
					credential_env: format!("AIDASH_SECRET_TRANSACTION_{index:02}"),
					protocol_version: "0.2".into(),
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

#[fixture]
fn tier_group(
	#[default(0)] _offset: usize,
	#[from(common::isolated_test_environment)] _environment: EnvironmentFuture,
	#[from(node)]
	#[with(&format!("tier-{_offset:02}"),_environment.clone())]
	a: BoxFuture<'static, Node>,
	#[from(node)]
	#[with(&format!("tier-{:02}",_offset+1),_environment.clone())]
	b: BoxFuture<'static, Node>,
	#[from(node)]
	#[with(&format!("tier-{:02}",_offset+2),_environment.clone())]
	c: BoxFuture<'static, Node>,
	#[from(node)]
	#[with(&format!("tier-{:02}",_offset+3),_environment.clone())]
	d: BoxFuture<'static, Node>,
) -> Vec<BoxFuture<'static, Node>> {
	vec![a, b, c, d]
}
#[fixture]
async fn tier_nodes(
	#[default(3)] count: usize,
	#[from(common::isolated_test_environment)] _environment: EnvironmentFuture,
	#[from(tier_group)]
	#[with(0,_environment.clone())]
	a: Vec<BoxFuture<'static, Node>>,
	#[from(tier_group)]
	#[with(4,_environment.clone())]
	b: Vec<BoxFuture<'static, Node>>,
	#[from(tier_group)]
	#[with(8,_environment.clone())]
	c: Vec<BoxFuture<'static, Node>>,
	#[from(tier_group)]
	#[with(12,_environment.clone())]
	d: Vec<BoxFuture<'static, Node>>,
) -> Vec<Node> {
	let mut nodes = Vec::new();
	for node in a.into_iter().chain(b).chain(c).chain(d).take(count) {
		nodes.push(node.await);
	}
	nodes
}
