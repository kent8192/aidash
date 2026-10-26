// Detection for the paths the deploy pipeline keeps (#151): broker redeploy and
// misuse of Key Material. These live in the human-run bootstrap so the deploy
// identity, which holds no monitoring or logging role, cannot mute them.
locals {
  broker_environment_ids = sort(keys(google_service_account.broker))
  // Mirrors the Cloud Run service name in modules/credential-broker.
  broker_service_names  = [for id in local.broker_environment_ids : "aidash-${id}-credential-broker"]
  broker_account_emails = [for id in local.broker_environment_ids : google_service_account.broker[id].email]
  broker_account_ids    = [for id in local.broker_environment_ids : google_service_account.broker[id].unique_id]
  // Logging query fragments: ("a" OR "b").
  quoted_services = join(" OR ", [for name in local.broker_service_names : "\"${name}\""])
  quoted_emails   = join(" OR ", [for email in local.broker_account_emails : "\"${email}\""])
  quoted_ids      = join(" OR ", [for id in local.broker_account_ids : "\"${id}\""])
  cloud_run_audit = "protoPayload.serviceName=\"run.googleapis.com\""
  // Structured service-account fields of Cloud Run v2/v1 service and job requests; the
  // substring match on the whole request stays as the fallback for shapes not listed.
  cloud_run_runs_as_broker = "(protoPayload.request.service.template.serviceAccount=(${local.quoted_emails}) OR protoPayload.request.template.serviceAccount=(${local.quoted_emails}) OR protoPayload.request.job.template.template.serviceAccount=(${local.quoted_emails}) OR protoPayload.request.service.spec.template.spec.serviceAccountName=(${local.quoted_emails}) OR protoPayload.request.spec.template.spec.serviceAccountName=(${local.quoted_emails}) OR protoPayload.request:(${local.quoted_emails}))"
}

resource "google_project_service" "alerting" {
  for_each           = local.broker_boundary ? toset([var.project_id, var.byok_project_id]) : toset([])
  project            = each.value
  service            = "monitoring.googleapis.com"
  disable_on_destroy = false
}

// Notification channels belong to the project of the alert policy that uses them.
resource "google_monitoring_notification_channel" "application" {
  for_each     = local.broker_boundary ? var.security_alert_emails : toset([])
  project      = var.project_id
  display_name = "Aidash security alerts ${each.value}"
  type         = "email"
  labels       = { email_address = each.value }
  depends_on   = [google_project_service.alerting]
}
resource "google_monitoring_notification_channel" "byok" {
  for_each     = local.broker_boundary ? var.security_alert_emails : toset([])
  project      = var.byok_project_id
  display_name = "Aidash security alerts ${each.value}"
  type         = "email"
  labels       = { email_address = each.value }
  depends_on   = [google_project_service.alerting]
}

// Path 2: every new revision of a broker service, plus any other Cloud Run
// service or job deployed to run as a broker account.
resource "google_monitoring_alert_policy" "broker_revision" {
  count                 = local.broker_boundary ? 1 : 0
  project               = var.project_id
  display_name          = "Credential Broker: new Cloud Run revision"
  combiner              = "OR"
  severity              = "CRITICAL"
  notification_channels = [for channel in google_monitoring_notification_channel.application : channel.id]
  conditions {
    display_name = "Broker service created or updated"
    condition_matched_log {
      filter = "${local.cloud_run_audit} AND protoPayload.methodName=(\"google.cloud.run.v2.Services.CreateService\" OR \"google.cloud.run.v2.Services.UpdateService\" OR \"google.cloud.run.v1.Services.CreateService\" OR \"google.cloud.run.v1.Services.ReplaceService\") AND protoPayload.resourceName:(${local.quoted_services})"
    }
  }
  conditions {
    display_name = "Cloud Run workload deployed as a broker account"
    condition_matched_log {
      filter = "${local.cloud_run_audit} AND protoPayload.methodName:(\"Services.CreateService\" OR \"Services.UpdateService\" OR \"Services.ReplaceService\" OR \"Jobs.CreateJob\" OR \"Jobs.UpdateJob\" OR \"Jobs.ReplaceJob\") AND ${local.cloud_run_runs_as_broker}"
    }
  }
  conditions {
    display_name = "Compute instance created or re-identified as a broker account"
    condition_matched_log {
      filter = "protoPayload.serviceName=\"compute.googleapis.com\" AND protoPayload.methodName:(\"instances.insert\" OR \"instances.bulkInsert\" OR \"instances.setServiceAccount\" OR \"instanceTemplates.insert\") AND protoPayload.request:(${local.quoted_emails})"
    }
  }
  conditions {
    display_name = "GKE cluster or node pool created or updated with a broker account"
    condition_matched_log {
      filter = "protoPayload.serviceName=\"container.googleapis.com\" AND protoPayload.methodName:(\"CreateCluster\" OR \"UpdateCluster\" OR \"CreateNodePool\" OR \"UpdateNodePool\" OR \"SetNodePoolManagement\") AND protoPayload.request:(${local.quoted_emails})"
    }
  }
  alert_strategy {
    notification_rate_limit { period = "300s" }
    auto_close = "604800s"
  }
  documentation {
    mime_type = "text/markdown"
    content   = "A Credential Broker revision changed, or another workload (Cloud Run, Compute, GKE) was created to run as a broker account, outside human review. Compare the image digest with `enterprise/infra/gcp/environments/broker-image.json` and the deploy workflow run that made the change."
  }
  depends_on = [google_project_service.alerting]
}

// Path 1 and residual misuse: any change to a broker's IAM policy.
resource "google_monitoring_alert_policy" "broker_iam_policy" {
  count                 = local.broker_boundary ? 1 : 0
  project               = var.project_id
  display_name          = "Credential Broker: IAM policy changed"
  combiner              = "OR"
  severity              = "CRITICAL"
  notification_channels = [for channel in google_monitoring_notification_channel.application : channel.id]
  conditions {
    display_name = "Broker Cloud Run service IAM policy set"
    condition_matched_log {
      filter = "${local.cloud_run_audit} AND protoPayload.methodName=(\"google.cloud.run.v2.Services.SetIamPolicy\" OR \"google.cloud.run.v1.Services.SetIamPolicy\") AND protoPayload.resourceName:(${local.quoted_services})"
    }
  }
  conditions {
    display_name = "Broker service account IAM policy set"
    condition_matched_log {
      filter = "protoPayload.serviceName=\"iam.googleapis.com\" AND protoPayload.methodName=\"google.iam.admin.v1.SetIAMPolicy\" AND protoPayload.resourceName:(${local.quoted_emails} OR ${local.quoted_ids})"
    }
  }
  alert_strategy {
    notification_rate_limit { period = "300s" }
    auto_close = "604800s"
  }
  documentation {
    mime_type = "text/markdown"
    content   = "The IAM policy of a Credential Broker service or service account changed. The deploy identity is denied this; investigate who else holds the permission."
  }
  depends_on = [google_project_service.alerting]
}

// Key Material payload reads by anyone but a broker. Failed attempts are logged
// too. Metadata reads (GetSecret, GetSecretVersion, ListSecrets) are routine for
// the server and deploy identities and are deliberately not alerted on.
resource "google_monitoring_alert_policy" "byok_key_material_read" {
  count                 = local.broker_boundary ? 1 : 0
  project               = var.byok_project_id
  display_name          = "Provider Credential Key Material read by a non-broker principal"
  combiner              = "OR"
  severity              = "CRITICAL"
  notification_channels = [for channel in google_monitoring_notification_channel.byok : channel.id]
  conditions {
    display_name = "AccessSecretVersion outside the broker accounts"
    condition_matched_log {
      filter = "logName=\"projects/${var.byok_project_id}/logs/cloudaudit.googleapis.com%2Fdata_access\" AND protoPayload.serviceName=\"secretmanager.googleapis.com\" AND protoPayload.methodName=\"google.cloud.secretmanager.v1.SecretManagerService.AccessSecretVersion\" AND NOT protoPayload.authenticationInfo.principalEmail=(${local.quoted_emails})"
    }
  }
  alert_strategy {
    notification_rate_limit { period = "300s" }
    auto_close = "604800s"
  }
  documentation {
    mime_type = "text/markdown"
    content   = "Key Material was read, or a read was attempted, by a principal that is not a Credential Broker account."
  }
  depends_on = [google_project_service.alerting]
}

// The deploy identity may change BYOK project IAM under a modifiedGrantsByRole
// condition. If that attribute ignores audit configs, deploy could drop DATA_READ
// logging and blind the Key Material alert, so (a) any audit-config change and (b)
// any IAM change by someone other than the deploy identity alert from the Admin
// Activity log, which cannot be turned off. Deploy's own binding-only changes (runtime
// role grants for each Environment) stay quiet. Whether modifiedGrantsByRole covers
// audit configs is a #152 check.
resource "google_monitoring_alert_policy" "byok_project_iam" {
  count                 = local.broker_boundary ? 1 : 0
  project               = var.byok_project_id
  display_name          = "BYOK project: audit configuration or IAM changed"
  combiner              = "OR"
  severity              = "CRITICAL"
  notification_channels = [for channel in google_monitoring_notification_channel.byok : channel.id]
  conditions {
    display_name = "Audit configuration changed"
    condition_matched_log {
      filter = "logName=\"projects/${var.byok_project_id}/logs/cloudaudit.googleapis.com%2Factivity\" AND protoPayload.serviceName=\"cloudresourcemanager.googleapis.com\" AND protoPayload.methodName=\"SetIamPolicy\" AND protoPayload.serviceData.policyDelta.auditConfigDeltas:*"
    }
  }
  conditions {
    display_name = "Project IAM set by a principal other than the deploy identity"
    condition_matched_log {
      filter = "logName=\"projects/${var.byok_project_id}/logs/cloudaudit.googleapis.com%2Factivity\" AND protoPayload.serviceName=\"cloudresourcemanager.googleapis.com\" AND protoPayload.methodName=\"SetIamPolicy\" AND NOT protoPayload.authenticationInfo.principalEmail=\"${google_service_account.automation["deploy"].email}\""
    }
  }
  alert_strategy {
    notification_rate_limit { period = "300s" }
    auto_close = "604800s"
  }
  documentation {
    mime_type = "text/markdown"
    content   = "The BYOK project's audit configuration or IAM policy changed outside the deploy identity's routine runtime-role grants. Check that DATA_READ audit logging for Secret Manager is still on."
  }
  depends_on = [google_project_service.alerting]
}
