variable "aws_region" {
  description = "AWS region"
  type        = string
  default     = "ap-southeast-1"
}

variable "environment" {
  description = "Deployment environment"
  type        = string
  default     = "production"
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
