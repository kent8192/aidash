mock_provider "google" {}
variables {
  project_id             = "aidash-fixture"
  release_bucket         = "aidash-fixture-releases"
  deploy_service_account = "deploy@aidash-fixture.iam.gserviceaccount.com"
}
run "retained_disks_and_spot_policy" {
  command = plan
  variables {
    environment_id = "test"
    hostname       = "test.aidash.run"
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
