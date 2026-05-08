# Stub — full definition comes in Task 13 (secrets.tf)
# Declared here so iam.tf forward-reference resolves during validation.
resource "aws_secretsmanager_secret" "app_config" {
  name = "${local.name_prefix}-app-config"
  tags = { Name = "${local.name_prefix}-app-config" }
}
