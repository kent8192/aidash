mock_provider "google" {
  mock_data "google_project" {
    defaults = { number = "123456789012" }
  }
}
variables {
  project_id           = "aidash-fixture"
  byok_project_id      = "aidash-byok-fixture"
  node_service_account = "aidash-gke-nodes@aidash-fixture.iam.gserviceaccount.com"
  cluster = {
    name          = "aidash"
    location      = "us-central1-a"
    workload_pool = "aidash-fixture.svc.id.goog"
  }
}

run "running_pool_and_workload_identities" {
  command = apply
  override_resource {
    target = google_service_account.workload["server"]
    values = {
      name  = "projects/aidash-fixture/serviceAccounts/server@aidash-fixture.iam.gserviceaccount.com"
      email = "server@aidash-fixture.iam.gserviceaccount.com"
    }
  }
  override_resource {
    target = google_service_account.workload["worker"]
    values = {
      name  = "projects/aidash-fixture/serviceAccounts/worker@aidash-fixture.iam.gserviceaccount.com"
      email = "worker@aidash-fixture.iam.gserviceaccount.com"
    }
  }
  variables {
    environment_id = "test"
    hostname       = "test.aidash.run"
    broker = {
      endpoint = "https://broker.run.app/api/v1"
      issuer   = "aidash"
      audience = "test"
      kid      = "kms-version"
    }
    environment = {
      kind         = "test"
      incarnation  = "aaaaaaaaaaaa"
      running      = true
      spot         = true
      nodes        = 2
      machine_type = "n2-standard-4"
    }
  }
  assert {
    condition = (
      google_container_node_pool.environment.name == "aidash-test-aaaaaaaaaaaa" && output.node_pool == "aidash-test-aaaaaaaaaaaa" &&
      google_container_node_pool.environment.cluster == "aidash" && google_container_node_pool.environment.location == "us-central1-a" &&
      google_container_node_pool.environment.node_count == 2
    )
    error_message = "A running environment's pool must run the requested node count in the shared cluster."
  }
  assert {
    condition = (
      google_container_node_pool.environment.node_config[0].image_type == "UBUNTU_CONTAINERD" &&
      google_container_node_pool.environment.node_config[0].machine_type == "n2-standard-4" &&
      google_container_node_pool.environment.node_config[0].spot &&
      google_container_node_pool.environment.node_config[0].service_account == var.node_service_account &&
      google_container_node_pool.environment.node_config[0].workload_metadata_config[0].mode == "GKE_METADATA"
    )
    error_message = "Environment nodes must be Ubuntu/containerd N2 with the GKE metadata server hiding the node identity."
  }
  assert {
    condition = (
      google_container_node_pool.environment.node_config[0].labels == tomap({ "aidash.run/environment" = "test" }) &&
      jsonencode(google_container_node_pool.environment.node_config[0].taint) == jsonencode([{ effect = "NO_SCHEDULE", key = "aidash.run/environment", value = "test" }])
    )
    error_message = "Only this environment's Pods may schedule onto its pool."
  }
  assert {
    condition = (
      output.namespace == "aidash-test" &&
      google_service_account_iam_member.workload_identity["server"].service_account_id == google_service_account.workload["server"].name &&
      google_service_account_iam_member.workload_identity["server"].role == "roles/iam.workloadIdentityUser" &&
      google_service_account_iam_member.workload_identity["server"].member == "serviceAccount:aidash-fixture.svc.id.goog[aidash-test/app-aidash-server]" &&
      google_service_account_iam_member.workload_identity["worker"].service_account_id == google_service_account.workload["worker"].name &&
      google_service_account_iam_member.workload_identity["worker"].role == "roles/iam.workloadIdentityUser" &&
      google_service_account_iam_member.workload_identity["worker"].member == "serviceAccount:aidash-fixture.svc.id.goog[aidash-test/app-aidash-worker]" &&
      length(google_service_account_iam_member.workload_identity) == 2
    )
    error_message = "Only the namespace's server and worker Kubernetes service accounts may impersonate their own GSAs."
  }
  assert {
    condition = (
      output.server_service_account == google_service_account.workload["server"].email &&
      output.worker_service_account == google_service_account.workload["worker"].email &&
      output.server_service_account != output.worker_service_account &&
      alltrue([for account in google_service_account.workload : length(account.account_id) <= 30 && can(regex("^ad[a-f0-9]{20}-(server|worker)$", account.account_id))])
    )
    error_message = "Server and worker must use distinct, valid service accounts."
  }
  assert {
    condition = (
      google_project_iam_member.provider_credential_create[0].project == var.byok_project_id &&
      google_project_iam_member.provider_credential_create[0].role == "projects/aidash-byok-fixture/roles/aidashByokCreate" &&
      google_project_iam_member.provider_credential_create[0].member == "serviceAccount:${output.server_service_account}" &&
      length(google_project_iam_member.provider_credential_create[0].condition) == 0 &&
      google_project_iam_member.provider_credential_manage[0].project == var.byok_project_id &&
      google_project_iam_member.provider_credential_manage[0].role == "projects/aidash-byok-fixture/roles/aidashByokManage" &&
      google_project_iam_member.provider_credential_manage[0].member == "serviceAccount:${output.server_service_account}" &&
      length(google_project_iam_member.provider_credential_manage[0].condition) == 1 &&
      google_project_iam_member.provider_credential_manage[0].condition[0].expression == "resource.name.startsWith('projects/123456789012/secrets/aidash-test-cred-')"
    )
    error_message = "Only the server binds bootstrap's fixed Create/Manage roles, with management limited to its environment prefix."
  }
  assert {
    condition = output.provider_credentials == {
      fingerprint_key = { env = "AIDASH_PROVIDER_FINGERPRINT_KEY" }
      store = {
        kind            = "secret_manager"
        byok_project_id = "aidash-byok-fixture"
        environment_id  = "test"
      }
      broker = var.broker
    }
    error_message = "BYOK must deliver the Store descriptor and a fingerprint reference, never the secret value."
  }
  assert {
    condition     = output.gcip.runtime_service_account == output.server_service_account && output.runtime_secret == "aidash-test-aaaaaaaaaaaa-runtime"
    error_message = "GCIP tenant reads belong to the server identity."
  }
}

run "stopped_pool_scales_to_zero" {
  command = plan
  variables {
    environment_id = "develop"
    hostname       = "develop.aidash.run"
    environment = {
      kind         = "develop"
      incarnation  = "cccccccccccc"
      running      = false
      spot         = false
      nodes        = 2
      machine_type = "n2-standard-8"
    }
  }
  assert {
    condition = (
      google_container_node_pool.environment.name == "aidash-develop-cccccccccccc" &&
      google_container_node_pool.environment.node_count == 0 &&
      !google_container_node_pool.environment.node_config[0].spot &&
      google_container_node_pool.environment.node_config[0].machine_type == "n2-standard-8"
    )
    error_message = "Stopping keeps the pool definition but runs no node."
  }
}

run "preview_has_its_own_namespace_and_prefix" {
  command = plan
  variables {
    environment_id = "pr-2"
    hostname       = "preview.aidash.run"
    environment = {
      kind         = "pr"
      incarnation  = "bbbbbbbbbbbb"
      running      = true
      spot         = true
      nodes        = 1
      machine_type = "n2-standard-4"
    }
  }
  assert {
    condition = (
      output.namespace == "aidash-pr-2" &&
      google_container_node_pool.environment.node_count == 1 &&
      jsonencode(google_container_node_pool.environment.node_config[0].taint) == jsonencode([{ effect = "NO_SCHEDULE", key = "aidash.run/environment", value = "pr-2" }]) &&
      google_service_account_iam_member.workload_identity["server"].member == "serviceAccount:aidash-fixture.svc.id.goog[aidash-pr-2/app-aidash-server]"
    )
    error_message = "A preview is isolated by its own namespace, taint and Workload Identity principals."
  }
  assert {
    condition     = google_project_iam_member.provider_credential_manage[0].condition[0].expression == "resource.name.startsWith('projects/123456789012/secrets/aidash-pr-2-cred-')"
    error_message = "Environments share the fixed role but receive distinct secret-prefix conditions."
  }
  assert {
    condition     = output.provider_credentials.broker == null && output.provider_credentials.store.environment_id == "pr-2"
    error_message = "Store-only environments must receive their own descriptor without enabling a broker."
  }
}
