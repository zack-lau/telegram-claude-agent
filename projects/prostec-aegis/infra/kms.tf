data "aws_caller_identity" "current" {}

# App-layer envelope encryption for OAuth refresh token payloads.
# DynamoDB SSE for the oauth_tokens table uses aws_kms_key.dynamodb.
# The application further encrypts individual token payloads with THIS key
# via kms:GenerateDataKey / kms:Decrypt before writing to DynamoDB (two-layer).
resource "aws_kms_key" "oauth_tokens" {
  description             = "Aegis: app-layer envelope encryption for OAuth refresh token payloads"
  deletion_window_in_days = 30
  enable_key_rotation     = true
  multi_region            = false

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid       = "RootFullAccess"
        Effect    = "Allow"
        Principal = { AWS = "arn:aws:iam::${data.aws_caller_identity.current.account_id}:root" }
        Action    = "kms:*"
        Resource  = "*"
      },
      {
        Sid       = "EcsTaskEncryptDecrypt"
        Effect    = "Allow"
        Principal = { AWS = aws_iam_role.ecs_task.arn }
        Action    = ["kms:GenerateDataKey", "kms:Decrypt"]
        Resource  = "*"
      }
    ]
  })

  tags = { Name = "${local.name_prefix}-oauth-tokens" }
}

resource "aws_kms_alias" "oauth_tokens" {
  name          = "alias/${local.name_prefix}-oauth-tokens"
  target_key_id = aws_kms_key.oauth_tokens.key_id
}

resource "aws_kms_key" "dynamodb" {
  description             = "Aegis: DynamoDB table encryption (SSE)"
  deletion_window_in_days = 30
  enable_key_rotation     = true
  multi_region            = false

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid       = "RootFullAccess"
        Effect    = "Allow"
        Principal = { AWS = "arn:aws:iam::${data.aws_caller_identity.current.account_id}:root" }
        Action    = "kms:*"
        Resource  = "*"
      },
      {
        # Confused-deputy mitigation: constrain to this account's DynamoDB only.
        Sid       = "DynamoDBServiceAccess"
        Effect    = "Allow"
        Principal = { Service = "dynamodb.amazonaws.com" }
        Action    = ["kms:GenerateDataKey", "kms:Decrypt", "kms:DescribeKey"]
        Resource  = "*"
        Condition = {
          StringEquals = {
            "kms:CallerAccount" = data.aws_caller_identity.current.account_id
            "kms:ViaService"    = "dynamodb.${var.aws_region}.amazonaws.com"
          }
        }
      }
    ]
  })

  tags = { Name = "${local.name_prefix}-dynamodb" }
}

resource "aws_kms_alias" "dynamodb" {
  name          = "alias/${local.name_prefix}-dynamodb"
  target_key_id = aws_kms_key.dynamodb.key_id
}

resource "aws_kms_key" "logs" {
  description             = "Aegis: CloudWatch Logs encryption"
  deletion_window_in_days = 30
  enable_key_rotation     = true
  multi_region            = false

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Sid       = "RootFullAccess"
        Effect    = "Allow"
        Principal = { AWS = "arn:aws:iam::${data.aws_caller_identity.current.account_id}:root" }
        Action    = "kms:*"
        Resource  = "*"
      },
      {
        Sid       = "CloudWatchLogsServiceAccess"
        Effect    = "Allow"
        Principal = { Service = "logs.${var.aws_region}.amazonaws.com" }
        Action    = ["kms:GenerateDataKey*", "kms:Decrypt", "kms:DescribeKey"]
        Resource  = "*"
        Condition = {
          ArnLike = {
            "kms:EncryptionContext:aws:logs:arn" = "arn:aws:logs:${var.aws_region}:${data.aws_caller_identity.current.account_id}:log-group:*"
          }
        }
      }
    ]
  })

  tags = { Name = "${local.name_prefix}-logs" }
}

resource "aws_kms_alias" "logs" {
  name          = "alias/${local.name_prefix}-logs"
  target_key_id = aws_kms_key.logs.key_id
}
