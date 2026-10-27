output "psl_record" {
  description = "The TXT record to cite in the publicsuffix/list pull request."
  value       = var.psl_pull_request_url == null ? null : { name = cloudflare_dns_record.psl[0].name, content = cloudflare_dns_record.psl[0].content }
}
