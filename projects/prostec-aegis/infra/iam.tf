# ECS Task Execution Role — used by ECS control plane to pull images and push logs
resource "aws_iam_role" "ecs_execution" {
  name = "${local.name_prefix}-ecs-execution"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { Service = "ecs-tasks.amazonaws.com" }
      Action    = "sts:AssumeRole"
    }]
  })
}

resource "aws_iam_role_policy_attachment" "ecs_execution_managed" {
  role       = aws_iam_role.ecs_execution.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy"
}

resource "aws_iam_role_policy" "ecs_execution_secrets" {
  name = "read-app-config-secret"
  role = aws_iam_role.ecs_execution.id

  # Fix M11: also allow reading the OAuth client credential secrets
  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect = "Allow"
      Action = ["secretsmanager:GetSecretValue"]
      Resource = [
        aws_secretsmanager_secret.app_config.arn,
        aws_secretsmanager_secret.google_oauth_client.arn,
        aws_secretsmanager_secret.microsoft_oauth_client.arn,
      ]
    }]
  })
}

# ECS Task Role — assumed by the running container (app permissions)
resource "aws_iam_role" "ecs_task" {
  name = "${local.name_prefix}-ecs-task"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { Service = "ecs-tasks.amazonaws.com" }
      Action    = "sts:AssumeRole"
    }]
  })
}

# Fix C9: DeleteItem removed from the broad allow list.
# Fix C2: sessions, deliveries, recipient-settings, and oauth-cloud-tokens ARNs added.
# Fix N3: sessions table gets DeleteItem explicitly — required for revoke_session/revoke_all.
resource "aws_iam_role_policy" "ecs_task_dynamodb" {
  name = "dynamodb-crud"
  role = aws_iam_role.ecs_task.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Action = [
          "dynamodb:GetItem",
          "dynamodb:PutItem",
          "dynamodb:UpdateItem",
          "dynamodb:Query",
        ]
        Resource = [
          aws_dynamodb_table.key_directory.arn,
          aws_dynamodb_table.api_keys.arn,
          "${aws_dynamodb_table.api_keys.arn}/index/*",
          aws_dynamodb_table.revocations.arn,
          aws_dynamodb_table.oauth_cloud_tokens.arn,
          "${aws_dynamodb_table.oauth_cloud_tokens.arn}/index/*",
          aws_dynamodb_table.sessions.arn,
          "${aws_dynamodb_table.sessions.arn}/index/*",
          aws_dynamodb_table.deliveries.arn,
          "${aws_dynamodb_table.deliveries.arn}/index/*",
          aws_dynamodb_table.recipient_settings.arn,
          aws_dynamodb_table.streaming_uploads.arn,
        ]
      },
      # Sessions need DeleteItem for revoke_session/revoke_all.
      # oauth_cloud_tokens needs DeleteItem for token revocation.
      # deliveries needs DeleteItem for burn-after-read cleanup.
      # streaming_uploads needs DeleteItem for abort + after complete (TTL handles orphans).
      {
        Effect = "Allow"
        Action = ["dynamodb:DeleteItem"]
        Resource = [
          aws_dynamodb_table.sessions.arn,
          aws_dynamodb_table.oauth_cloud_tokens.arn,
          aws_dynamodb_table.deliveries.arn,
          aws_dynamodb_table.streaming_uploads.arn,
        ]
      },
      # Fix C9: explicit Deny for destructive ops on audit_logs (append-only).
      {
        Effect = "Deny"
        Action = [
          "dynamodb:DeleteItem",
          "dynamodb:BatchWriteItem",
        ]
        Resource = [
          aws_dynamodb_table.audit_logs.arn,
          "${aws_dynamodb_table.audit_logs.arn}/index/*",
        ]
      }
    ]
  })
}

# audit_logs is append-only — TTL handles expiry, no Update/Delete from the task role.
# Explicit Deny overrides any future accidental Allow attached to this role.
resource "aws_iam_role_policy" "ecs_task_audit_logs" {
  name = "audit-logs-append-only"
  role = aws_iam_role.ecs_task.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Action = [
          "dynamodb:PutItem",
          "dynamodb:Query",
        ]
        Resource = [
          aws_dynamodb_table.audit_logs.arn,
          "${aws_dynamodb_table.audit_logs.arn}/index/*",
        ]
      },
      {
        Effect = "Deny"
        Action = [
          "dynamodb:UpdateItem",
          "dynamodb:DeleteItem",
          "dynamodb:BatchWriteItem",
        ]
        Resource = [
          aws_dynamodb_table.audit_logs.arn,
          "${aws_dynamodb_table.audit_logs.arn}/index/*",
        ]
      }
    ]
  })
}

# Fix H3: added kms:Encrypt alongside GenerateDataKey and Decrypt.
# Fix M8: scoped Resource to specific KMS key ARN instead of "*".
resource "aws_iam_role_policy" "ecs_task_kms_oauth" {
  name = "kms-oauth-tokens"
  role = aws_iam_role.ecs_task.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect   = "Allow"
      Action   = ["kms:GenerateDataKey", "kms:Decrypt", "kms:Encrypt"]
      Resource = [aws_kms_key.oauth_tokens.arn]
    }]
  })
}

# Cognito admin ops — needed for:
#   AdminGetUser: verify recipient exists before delivering
#   AdminUserGlobalSignOut: wipe all Cognito sessions on account compromise
#   AdminDisableUser: suspend compromised accounts
resource "aws_iam_role_policy" "ecs_task_cognito" {
  name = "cognito-admin"
  role = aws_iam_role.ecs_task.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect = "Allow"
      Action = [
        "cognito-idp:AdminGetUser",
        "cognito-idp:AdminUserGlobalSignOut",
        "cognito-idp:AdminDisableUser",
      ]
      Resource = [aws_cognito_user_pool.main.arn]
    }]
  })
}
