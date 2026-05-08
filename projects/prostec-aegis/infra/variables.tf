variable "aws_region" {
  description = "AWS region (default: ap-southeast-1, Singapore)"
  type        = string
  default     = "ap-southeast-1"

  validation {
    condition     = can(regex("^[a-z]{2}-[a-z]+-[0-9]$", var.aws_region))
    error_message = "aws_region must be a valid AWS region (e.g. ap-southeast-1)"
  }
}

variable "environment" {
  description = "Deployment environment (e.g. staging, production)"
  type        = string

  validation {
    condition     = contains(["staging", "production"], var.environment)
    error_message = "environment must be one of: staging, production"
  }
}

variable "project" {
  description = "Project name"
  type        = string
  default     = "aegis"
}

variable "api_domain" {
  description = "FQDN for the Aegis API"
  type        = string
  default     = "api.aegis.prosteclabs.com"
}

variable "route53_zone_id" {
  description = "Route53 hosted zone ID for prosteclabs.com"
  type        = string
}
