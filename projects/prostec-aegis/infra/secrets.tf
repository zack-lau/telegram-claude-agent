# Bootstrap procedure (one-time, before deploying the real Aegis image):
#   aws secretsmanager put-secret-value \
#     --secret-id <output: app_config_secret_arn> \
#     --secret-string '{"aws_region":"ap-southeast-1","dynamodb_prefix":"...","kms_oauth_key_arn":"..."}'
# Terraform creates the secret with a placeholder; ignore_changes preserves any
# out-of-band updates. The ECS execution role has GetSecretValue on this ARN.
resource "aws_secretsmanager_secret" "app_config" {
  name                    = "${local.name_prefix}/app-config"
  description             = "Aegis API server configuration — populate before deploying real image"
  recovery_window_in_days = 30

  tags = { Name = "${local.name_prefix}-app-config" }
}

resource "aws_secretsmanager_secret_version" "app_config" {
  secret_id = aws_secretsmanager_secret.app_config.id

  secret_string = jsonencode({
    placeholder = "replace_with_real_config_before_deployment"
    # Keys when populated: aws_region, dynamodb_prefix, kms_oauth_key_arn
  })

  lifecycle {
    ignore_changes = [secret_string]
  }
}
