# What both clouds share: the admin's address, the SSH key and the passwords.

# This machine's public IP, as seen from the internet.
data "http" "my_ip" {
  url = "https://checkip.amazonaws.com"
}

locals {
  admin_cidr     = var.admin_cidr != "" ? var.admin_cidr : "${chomp(data.http.my_ip.response_body)}/32"
  ssh_public_key = trimspace(file(pathexpand(var.ssh_public_key_path)))

  # Static addresses are separate resources, created before the servers, so
  # each cloud's firewall can name the other cloud's address without a cycle.
  aws_ip   = aws_eip.hiveemulator.public_ip
  azure_ip = azurerm_public_ip.bms.ip_address
}

# Generated once and kept in the state. Letters and digits only, so they can be
# put into URLs (redis://:password@host) without escaping.
resource "random_password" "redis" {
  length  = 32
  special = false
}

resource "random_password" "postgres" {
  length  = 32
  special = false
}

resource "random_password" "state_db" {
  length  = 32
  special = false
}

resource "random_password" "deconfliction_db" {
  length  = 32
  special = false
}

resource "random_password" "rabbitmq" {
  length  = 32
  special = false
}
