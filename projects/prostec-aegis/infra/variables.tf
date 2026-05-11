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

variable "alarm_sns_arn" {
  description = "SNS topic ARN for CloudWatch alarm notifications (leave empty to disable)"
  type        = string
  default     = ""
}

variable "task_cpu" {
  description = "ECS task CPU units (256=0.25vCPU, 512=0.5vCPU, 1024=1vCPU)"
  type        = number
  default     = 512

  validation {
    condition     = contains([256, 512, 1024, 2048, 4096], var.task_cpu)
    error_message = "task_cpu must be a valid Fargate CPU value"
  }
}

variable "task_memory" {
  description = "ECS task memory in MiB (min 512; must be compatible with task_cpu)"
  type        = number
  default     = 1024

  validation {
    condition     = var.task_memory >= 512
    error_message = "task_memory must be at least 512 MiB"
  }
}
