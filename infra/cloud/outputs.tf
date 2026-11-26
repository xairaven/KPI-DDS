# What the Kubernetes overlay needs from the clouds, written as files that the
# overlay reads (both are git-ignored: they contain passwords).

resource "local_file" "k8s_config" {
  filename        = "${path.module}/../k8s/overlays/azure/cloud.env"
  file_permission = "0644"
  content         = <<-EOT
    BMS_HOST=${azurerm_public_ip.bms.fqdn}
    CC_URL=http://${local.aws_ip}:8080
  EOT
}

resource "local_sensitive_file" "k8s_secrets" {
  filename        = "${path.module}/../k8s/overlays/azure/secrets.env"
  file_permission = "0600"
  content         = <<-EOT
    POSTGRES_PASSWORD=${random_password.postgres.result}
    STATE_DB_PASSWORD=${random_password.state_db.result}
    DECONFLICTION_DB_PASSWORD=${random_password.deconfliction_db.result}
    RABBITMQ_PASSWORD=${random_password.rabbitmq.result}
    REDIS_URL=redis://:${random_password.redis.result}@${local.aws_ip}:6379
    BMS_API_TOKENS=${random_password.api_token.result}
  EOT
}

output "admin_cidr" {
  description = "Address allowed to use SSH, the Kubernetes API and the lecturer's UIs."
  value       = local.admin_cidr
}

output "aws_public_ip" {
  value = local.aws_ip
}

output "azure_public_ip" {
  value = local.azure_ip
}

output "bms_url" {
  description = "BMS API (Swagger UI under /swagger/)."
  value       = "https://${azurerm_public_ip.bms.fqdn}"
}

output "api_token" {
  description = "Bearer token of the BMS API: tofu output -raw api_token"
  value       = random_password.api_token.result
  sensitive   = true
}

output "lecturer_map_url" {
  value = "http://${local.aws_ip}:8081"
}

output "lecturer_swagger_url" {
  value = "http://${local.aws_ip}:8080/swagger"
}

output "ssh_aws" {
  value = "ssh -i ~/.ssh/bms-cloud/bms-cloud ubuntu@${local.aws_ip}"
}

output "ssh_azure" {
  value = "ssh -i ~/.ssh/bms-cloud/bms-cloud azureuser@${local.azure_ip}"
}
