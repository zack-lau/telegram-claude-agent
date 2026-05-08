terraform {
  required_version = ">= 1.5"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.0"
    }
  }

  backend "s3" {
    # bucket is passed at init time — never hardcode account IDs in source:
    #   terraform init -backend-config="bucket=aegis-terraform-state-$(aws sts get-caller-identity --query Account --output text)"
    key            = "production/terraform.tfstate"
    region         = "ap-southeast-1"
    dynamodb_table = "aegis-terraform-locks"
    encrypt        = true
  }
}

provider "aws" {
  region = var.aws_region

  default_tags {
    tags = local.common_tags
  }
}
