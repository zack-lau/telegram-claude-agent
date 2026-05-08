data "aws_caller_identity" "current" {}

resource "aws_kms_key" "oauth_tokens" {
  description             = "Aegis: encrypt OAuth refresh tokens at rest"
  deletion_window_in_days = 30
  enable_key_rotation     = true

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
  description             = "Aegis: DynamoDB table encryption"
  deletion_window_in_days = 30
  enable_key_rotation     = true

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
        Sid       = "DynamoDBServiceAccess"
        Effect    = "Allow"
        Principal = { Service = "dynamodb.amazonaws.com" }
        Action    = ["kms:GenerateDataKey", "kms:Decrypt", "kms:DescribeKey"]
        Resource  = "*"
      }
    ]
  })

  tags = { Name = "${local.name_prefix}-dynamodb" }
}

resource "aws_kms_alias" "dynamodb" {
  name          = "alias/${local.name_prefix}-dynamodb"
  target_key_id = aws_kms_key.dynamodb.key_id
}
