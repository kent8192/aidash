use aidash::authorization::policy::{Evaluation, PolicyBundle};
use serde_json::{Value, json};

#[test]
fn policy_management_routes_publish_authenticated_typed_contracts() {
    let document = serde_json::to_value(aidash::api::openapi()).unwrap();
    for (path, method) in [
        ("/api/authorization/{tenant}", "get"),
        ("/api/authorization/{tenant}", "post"),
        ("/api/authorization/{tenant}/evaluate", "post"),
        ("/api/authorization/{tenant}/simulate", "post"),
        ("/api/authorization/{tenant}/revisions", "get"),
        ("/api/authorization/{tenant}/decisions", "get"),
        ("/api/authorization/{tenant}/credentials", "get"),
        ("/api/authorization/{tenant}/credentials", "post"),
        (
            "/api/authorization/{tenant}/credentials/{id}/revoke",
            "post",
        ),
    ] {
        let operation = &document["paths"][path][method];
        assert!(operation.is_object(), "missing {method} {path}");
        assert_eq!(operation["security"], json!([{"bearer_auth":[]}]));
        assert!(operation["responses"]["200"]["content"]["application/json"]["schema"].is_object());
    }
    assert!(document["components"]["schemas"]["PolicyBundle"].is_object());
    let condition = &document["components"]["schemas"]["Condition"];
    assert!(condition.is_object());
    assert!(
        condition
            .to_string()
            .contains("#/components/schemas/Condition"),
        "recursive conditions must retain their type"
    );
}

fn document() -> Value {
    json!({
        "tenant":"acme",
        "subjects":{
            "alice":{"kind":"user","groups":["writers"],"attributes":{"team":"research"}},
            "worker":{"kind":"agent","roles":["reader"],"attributes":{"team":"research"},"delegated_by":"alice"}
        },
        "groups":{"writers":{"roles":["editor"]}},
        "roles":{"reader":{},"editor":{"inherits":["reader"]}},
        "policies":[{
            "id":"read","effect":"allow","subjects":{"roles":["reader"]},
            "actions":["workspace.read"],"resources":{"kinds":["workspace"]},
            "condition":{"op":"all","conditions":[
                {"op":"eq","left":{"source":"subject","path":"/team"},"right":{"source":"resource","path":"/team"}},
                {"op":"contains","left":{"source":"literal","value":["office","vpn"]},"right":{"source":"environment","path":"/network"}}
            ]}
        }]
    })
}

fn input() -> Evaluation {
    serde_json::from_value(json!({"subject":"alice","action":"workspace.read",
        "resource":{"tenant":"acme","kind":"workspace","id":"w1","attributes":{"team":"research"}},
        "environment":{"network":"vpn"}}))
    .unwrap()
}

fn policy(value: Value) -> PolicyBundle {
    serde_json::from_value(value).unwrap()
}

#[test]
fn inherited_group_roles_require_matching_resource_and_environment_attributes() {
    let rules = policy(document());
    rules.validate().unwrap();
    let mut request = input();
    let decision = rules.evaluate(&request);
    assert!(decision.allowed);
    assert_eq!(decision.matched_policies, vec!["read"]);
    assert!(decision.effective_roles.contains("editor"));
    assert!(decision.effective_roles.contains("reader"));
    request.environment = json!({"network":"public"});
    assert!(!rules.evaluate(&request).allowed);
    request.environment = json!({"network":"vpn"});
    request.resource.attributes = json!({"team":"finance"});
    assert!(!rules.evaluate(&request).allowed);
}

#[test]
fn explicit_deny_overrides_allow_in_both_policy_orders() {
    let mut doc = document();
    doc["policies"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"deny","effect":"deny",
        "subjects":{"ids":["alice"]},"actions":["*"],"resources":{"kinds":["*"]}}));
    for _ in 0..2 {
        let decision = policy(doc.clone()).evaluate(&input());
        assert!(!decision.allowed);
        assert_eq!(decision.reason, "explicit_deny");
        assert!(decision.matched_policies.contains(&"deny".into()));
        doc["policies"].as_array_mut().unwrap().reverse();
    }
}

#[test]
fn default_deny_and_tenant_boundary_apply_even_to_wildcard_policies() {
    let mut doc = document();
    doc["policies"] = json!([]);
    assert!(!policy(doc.clone()).evaluate(&input()).allowed);
    doc["policies"] = json!([{"id":"all","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}}]);
    let rules = policy(doc);
    assert!(rules.evaluate(&input()).allowed);
    let mut request = input();
    request.resource.tenant = "other".into();
    assert_eq!(rules.evaluate(&request).reason, "tenant_mismatch");
    request.resource.tenant = "acme".into();
    request.subject = "unknown".into();
    assert!(!rules.evaluate(&request).allowed);
    request.subject = "Alice".into();
    assert!(!rules.evaluate(&request).allowed);
}

#[test]
fn missing_attributes_fail_closed_for_inequality_while_null_is_a_value() {
    let mut doc = document();
    doc["policies"][0]["condition"] = json!({"op":"not_eq","left":{"source":"resource","path":"/classification"},"right":{"source":"literal","value":"secret"}});
    let rules = policy(doc);
    let mut request = input();
    assert!(!rules.evaluate(&request).allowed);
    request.resource.attributes["classification"] = Value::Null;
    assert!(rules.evaluate(&request).allowed);
    request.resource.attributes["classification"] = json!("secret");
    assert!(!rules.evaluate(&request).allowed);
}

#[test]
fn delegation_intersects_ancestors_and_honors_revocation() {
    let mut doc = document();
    let mut request = input();
    request.subject = "worker".into();
    assert!(policy(doc.clone()).evaluate(&request).allowed);
    doc["subjects"]["alice"]["attributes"]["team"] = json!("finance");
    assert_eq!(
        policy(doc.clone()).evaluate(&request).reason,
        "delegation_denied"
    );
    doc["subjects"]["alice"]["attributes"]["team"] = json!("research");
    doc["subjects"]["alice"]["enabled"] = json!(false);
    assert!(!policy(doc).evaluate(&request).allowed);
}

#[test]
fn cycles_and_dangling_authority_references_are_rejected() {
    for changed in [
        ("/roles/reader/inherits", json!(["editor"])),
        ("/roles/reader/inherits", json!(["missing"])),
        ("/groups/writers/roles", json!(["missing"])),
        ("/subjects/alice/groups", json!(["missing"])),
    ] {
        let mut doc = document();
        if changed.0 == "/roles/reader/inherits" {
            doc["roles"]["reader"]["inherits"] = changed.1;
        } else {
            *doc.pointer_mut(changed.0).unwrap() = changed.1;
        }
        let rules = policy(doc);
        assert!(rules.validate().is_err(), "accepted {}", changed.0);
        assert!(
            !rules.evaluate(&input()).allowed,
            "unvalidated invalid policy must fail closed"
        );
    }
    let mut doc = document();
    doc["subjects"]["alice"]["delegated_by"] = json!("worker");
    assert!(policy(doc).validate().is_err());
}

#[test]
fn malformed_expressions_duplicate_policy_ids_and_ambiguous_selectors_are_rejected() {
    let mut doc = document();
    doc["policies"][0]["condition"] = json!({"op":"any","conditions":[]});
    assert!(policy(doc).validate().is_err());
    let mut doc = document();
    let duplicate = doc["policies"][0].clone();
    doc["policies"].as_array_mut().unwrap().push(duplicate);
    assert!(policy(doc).validate().is_err());
    let mut doc = document();
    doc["policies"][0]["subjects"] = json!({});
    assert!(policy(doc).validate().is_err());
    let mut doc = document();
    doc["policies"][0]["conditon"] = json!({"op":"any","conditions":[]});
    assert!(
        serde_json::from_value::<PolicyBundle>(doc).is_err(),
        "unknown fields must not silently remove constraints"
    );
}

#[test]
fn expression_depth_and_role_graph_limits_prevent_unbounded_evaluation() {
    let mut expression = json!({"op":"exists","value":{"source":"environment","path":"/network"}});
    for _ in 0..40 {
        expression = json!({"op":"all","conditions":[expression]});
    }
    let mut doc = document();
    doc["policies"][0]["condition"] = expression;
    assert!(policy(doc).validate().is_err());
    let mut doc = document();
    for i in 0..40 {
        doc["roles"][format!("role{i}")] = if i == 0 {
            json!({})
        } else {
            json!({"inherits":[format!("role{}", i-1)]})
        };
    }
    assert!(policy(doc).validate().is_err());
}
