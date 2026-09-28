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
