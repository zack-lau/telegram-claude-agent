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
    # Future keys: aws_region, dynamodb_prefix, kms_oauth_key_arn
  })

  lifecycle {
    ignore_changes = [secret_string]
  }
}
