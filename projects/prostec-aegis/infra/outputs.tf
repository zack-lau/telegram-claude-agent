output "api_url" {
  description = "Aegis API base URL"
  value       = "https://${var.api_domain}"
}

output "alb_dns_name" {
  description = "ALB DNS name (for testing before DNS propagates)"
  value       = aws_lb.main.dns_name
}

output "ecs_cluster_name" {
  description = "ECS cluster name"
  value       = aws_ecs_cluster.main.name
}

output "ecs_service_name" {
  description = "ECS service name"
  value       = aws_ecs_service.api.name
}

output "dynamodb_tables" {
  description = "DynamoDB table names"
  value = {
    key_directory = aws_dynamodb_table.key_directory.name
    api_keys      = aws_dynamodb_table.api_keys.name
    revocations   = aws_dynamodb_table.revocations.name
    audit_logs    = aws_dynamodb_table.audit_logs.name
    oauth_tokens  = aws_dynamodb_table.oauth_tokens.name
  }
}

output "kms_key_arns" {
  description = "KMS key ARNs"
  value = {
    oauth_tokens = aws_kms_key.oauth_tokens.arn
    dynamodb     = aws_kms_key.dynamodb.arn
    logs         = aws_kms_key.logs.arn
  }
}

output "cognito_user_pool_id" {
  description = "Cognito User Pool ID"
  value       = aws_cognito_user_pool.main.id
}

output "cognito_user_pool_arn" {
  description = "Cognito User Pool ARN"
  value       = aws_cognito_user_pool.main.arn
}

output "cognito_client_id" {
  description = "Cognito app client ID (server-side API client)"
  value       = aws_cognito_user_pool_client.api.id
  sensitive   = true
}

output "cognito_jwks_uri" {
  description = "Cognito JWKS URI for stateless JWT verification in the API"
  value       = "https://cognito-idp.${var.aws_region}.amazonaws.com/${aws_cognito_user_pool.main.id}/.well-known/jwks.json"
}

output "cognito_issuer_url" {
  description = "Cognito token issuer URL"
  value       = "https://cognito-idp.${var.aws_region}.amazonaws.com/${aws_cognito_user_pool.main.id}"
}

output "vpc_id" {
  description = "VPC ID"
  value       = aws_vpc.main.id
}

output "private_subnet_ids" {
  description = "Private subnet IDs (ECS tasks)"
  value       = aws_subnet.private[*].id
}

output "ecs_task_role_arn" {
  description = "ECS task role ARN — reference when adding IAM policies for the real API"
  value       = aws_iam_role.ecs_task.arn
}

output "app_config_secret_arn" {
  description = "Secrets Manager ARN for API server config"
  value       = aws_secretsmanager_secret.app_config.arn
}
