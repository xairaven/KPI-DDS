variable "azure_subscription_id" {
  description = "Azure subscription (az account show --query id -o tsv)."
  type        = string
}

variable "ssh_public_key_path" {
  description = "Public SSH key installed on both servers."
  type        = string
  default     = "~/.ssh/bms-cloud/bms-cloud.pub"
}

variable "admin_cidr" {
  description = <<-EOT
    Who may use SSH, the Kubernetes API and the lecturer's UIs, e.g. "203.0.113.7/32".
    Empty: the current public IP of this machine.
  EOT
  type        = string
  default     = ""
}

variable "aws_region" {
  description = "The only region this AWS account may use."
  type        = string
  default     = "eu-north-1"
}

variable "aws_instance_type" {
  description = "2 vCPU / 2 GB, free plan eligible."
  type        = string
  default     = "t3.small"
}

variable "azure_location" {
  description = "Allowed by the Azure for Students policy and offers B-series VMs."
  type        = string
  default     = "denmarkeast"
}

variable "azure_vm_size" {
  description = "2 vCPU / 4 GB."
  type        = string
  default     = "Standard_B2als_v2"
}

variable "image_owner" {
  description = "Owner of the images on ghcr.io (bms-*, hivemind)."
  type        = string
  default     = "xairaven"
}
