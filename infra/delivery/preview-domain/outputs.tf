output "psl_record" {
  description = "The TXT record to cite in the publicsuffix/list pull request."
  value       = { name = cloudflare_dns_record.psl.name, content = cloudflare_dns_record.psl.content }
}
