mock_provider "google" {
  mock_data "google_project" {
    defaults = { number = "123456789012" }
  }
}
variables {
  project_id             = "aidash-fixture"
  byok_project_id        = "aidash-byok-fixture"
  release_bucket         = "aidash-fixture-releases"
  deploy_service_account = "deploy@aidash-fixture.iam.gserviceaccount.com"
}
run "retained_disks_and_spot_policy" {
  command = plan
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
      kind          = "test"
      incarnation   = "aaaaaaaaaaaa"
      generation    = 1
      running       = true
      published     = false
      spot          = true
      vm_present    = true
      machine_type  = "e2-standard-4"
      boot_disk_gib = 10
      data_disk_gib = 20
      bundle_object = "bundles/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.tar.gz"
      bundle_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
      release_sha   = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    }
  }
  assert {
    condition     = google_compute_instance.host[0].boot_disk[0].auto_delete == false && google_compute_disk.data.size == 20
    error_message = "VM replacement/preemption must not delete retained data."
  }
  assert {
    condition     = google_compute_instance.host[0].scheduling[0].provisioning_model == "SPOT" && google_compute_instance.host[0].scheduling[0].automatic_restart == false && google_compute_instance.host[0].scheduling[0].instance_termination_action == "STOP"
    error_message = "Spot must stop and require explicit resumption."
  }
  assert {
    condition     = google_compute_firewall.iap.source_ranges == toset(["35.235.240.0/20"])
    error_message = "SSH must be available through IAP only."
  }
  assert {
    condition     = jsondecode(google_compute_instance.host[0].metadata["aidash-provider-credentials"]).broker == var.broker && jsondecode(google_compute_instance.host[0].metadata["aidash-provider-credentials"]).store.environment_id == var.environment_id && output.provider_credentials.fingerprint_key.env == "AIDASH_PROVIDER_FINGERPRINT_KEY"
    error_message = "The managed host must receive Store/broker settings through non-secret metadata."
  }
  assert {
    condition = (
      google_project_iam_member.provider_credential_create[0].project == var.byok_project_id &&
      google_project_iam_member.provider_credential_create[0].role == "projects/aidash-byok-fixture/roles/aidashByokCreate" &&
      length(google_project_iam_member.provider_credential_create[0].condition) == 0 &&
      google_project_iam_member.provider_credential_manage[0].project == var.byok_project_id &&
      google_project_iam_member.provider_credential_manage[0].role == "projects/aidash-byok-fixture/roles/aidashByokManage" &&
      length(google_project_iam_member.provider_credential_manage[0].condition) == 1 &&
      google_project_iam_member.provider_credential_manage[0].condition[0].expression == "resource.name.startsWith('projects/123456789012/secrets/aidash-test-cred-')"
    )
    error_message = "Runtime must bind only bootstrap's fixed Create/Manage roles, with management limited to its environment prefix."
  }
  assert {
    condition = jsondecode(google_compute_instance.host[0].metadata["aidash-provider-credentials"]) == {
      fingerprint_key = { env = "AIDASH_PROVIDER_FINGERPRINT_KEY" }
      store = {
        kind            = "secret_manager"
        byok_project_id = "aidash-byok-fixture"
        environment_id  = "test"
      }
      broker = var.broker
    }
    error_message = "BYOK must deliver the Store descriptor and a fingerprint reference, never the secret value, to server startup."
  }
}

run "preview_uses_shared_tls" {
  command = plan
  variables {
    environment_id   = "pr-2"
    hostname         = "preview.aidash.run"
    preview_tls_disk = "projects/aidash-fixture/zones/us-central1-a/disks/aidash-preview-tls"
    environment = {
      kind          = "pr"
      incarnation   = "bbbbbbbbbbbb"
      generation    = 1
      running       = true
      published     = false
      spot          = true
      vm_present    = true
      machine_type  = "e2-standard-4"
      boot_disk_gib = 10
      data_disk_gib = 20
      bundle_object = "bundles/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.tar.gz"
      bundle_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
      release_sha   = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    }
  }
  assert {
    condition     = length([for disk in google_compute_instance.host[0].attached_disk : disk if disk.device_name == "aidash-preview-tls" && disk.source == var.preview_tls_disk]) == 1
    error_message = "A preview must mount the hostname's shared certificate store."
  }
  assert {
    condition     = google_project_iam_member.provider_credential_manage[0].role == "projects/aidash-byok-fixture/roles/aidashByokManage" && google_project_iam_member.provider_credential_manage[0].condition[0].expression == "resource.name.startsWith('projects/123456789012/secrets/aidash-pr-2-cred-')"
    error_message = "Environments share the fixed role but receive distinct secret-prefix conditions."
  }
  assert {
    condition     = jsondecode(google_compute_instance.host[0].metadata["aidash-provider-credentials"]).broker == null && jsondecode(google_compute_instance.host[0].metadata["aidash-provider-credentials"]).store.environment_id == "pr-2"
    error_message = "Store-only environments must receive their own descriptor without enabling a broker."
  }
}

run "stopped_preview_releases_shared_tls" {
  command = plan
  variables {
    environment_id   = "pr-2"
    hostname         = "preview.aidash.run"
    preview_tls_disk = "projects/aidash-fixture/zones/us-central1-a/disks/aidash-preview-tls"
    environment = {
      kind          = "pr"
      incarnation   = "bbbbbbbbbbbb"
      generation    = 1
      running       = false
      published     = false
      spot          = true
      vm_present    = true
      machine_type  = "e2-standard-4"
      boot_disk_gib = 10
      data_disk_gib = 20
      bundle_object = "bundles/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.tar.gz"
      bundle_sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
      release_sha   = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    }
  }
  assert {
    condition     = length(google_compute_instance.host[0].attached_disk) == 1 && google_compute_instance.host[0].attached_disk[0].device_name == "aidash-data"
    error_message = "A stopped preview must release TLS storage while retaining its private application disk."
  }
}
