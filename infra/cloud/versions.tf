# Two clouds in one configuration:
#   AWS   (eu-north-1, Stockholm):  the lecturer's system (Redis, Communication
#                                    Control, map, HiveMinds) in Docker Compose
#   Azure (denmarkeast, Copenhagen): BMS on k3s (Kubernetes) behind HTTPS
#
# The state file (terraform.tfstate) holds the generated passwords, so it stays
# on this machine and out of git.

terraform {
  required_version = ">= 1.10"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 6.74"
    }
    azurerm = {
      source  = "hashicorp/azurerm"
      version = "~> 5.15"
    }
    random = {
      source  = "hashicorp/random"
      version = "~> 3.9"
    }
    http = {
      source  = "hashicorp/http"
      version = "~> 3.6"
    }
    local = {
      source  = "hashicorp/local"
      version = "~> 2.9"
    }
  }
}

# Credentials come from ~/.aws (aws configure) and the Azure CLI (az login).
provider "aws" {
  region = var.aws_region

  default_tags {
    tags = {
      Project = "bms"
    }
  }
}

provider "azurerm" {
  features {}
  subscription_id = var.azure_subscription_id
}
