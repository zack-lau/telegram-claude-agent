resource "aws_dynamodb_table" "key_directory" {
  name         = "${local.name_prefix}-key-directory"
  billing_mode = "PAY_PER_REQUEST"
  hash_key     = "recipient_id"

  attribute {
    name = "recipient_id"
    type = "S"
  }

  server_side_encryption {
    enabled     = true
    kms_key_arn = aws_kms_key.dynamodb.arn
  }

  point_in_time_recovery { enabled = true }

  deletion_protection_enabled = true

  tags = { Name = "${local.name_prefix}-key-directory" }
}

resource "aws_dynamodb_table" "api_keys" {
  name         = "${local.name_prefix}-api-keys"
  billing_mode = "PAY_PER_REQUEST"
  hash_key     = "key_id"

  attribute {
    name = "key_id"
    type = "S"
  }

  attribute {
    name = "owner_id"
    type = "S"
  }

  global_secondary_index {
    name            = "owner_id-index"
    hash_key        = "owner_id"
    projection_type = "ALL"
  }

  server_side_encryption {
    enabled     = true
    kms_key_arn = aws_kms_key.dynamodb.arn
  }

  point_in_time_recovery { enabled = true }

  deletion_protection_enabled = true

  tags = { Name = "${local.name_prefix}-api-keys" }
}

resource "aws_dynamodb_table" "revocations" {
  name         = "${local.name_prefix}-revocations"
  billing_mode = "PAY_PER_REQUEST"
  hash_key     = "key_fingerprint"

  attribute {
    name = "key_fingerprint"
    type = "S"
  }

  server_side_encryption {
    enabled     = true
    kms_key_arn = aws_kms_key.dynamodb.arn
  }

  point_in_time_recovery { enabled = true }

  deletion_protection_enabled = true

  tags = { Name = "${local.name_prefix}-revocations" }
}

resource "aws_dynamodb_table" "audit_logs" {
  name         = "${local.name_prefix}-audit-logs"
  billing_mode = "PAY_PER_REQUEST"
  hash_key     = "agent_id"
  range_key    = "timestamp_event_id"

  attribute {
    name = "agent_id"
    type = "S"
  }

  attribute {
    name = "timestamp_event_id"
    type = "S"
  }

  attribute {
    name = "recipient_id"
    type = "S"
  }

  global_secondary_index {
    name            = "recipient_id-index"
    hash_key        = "recipient_id"
    range_key       = "timestamp_event_id"
    projection_type = "ALL"
  }

  ttl {
    attribute_name = "expires_at"
    enabled        = true
  }

  server_side_encryption {
    enabled     = true
    kms_key_arn = aws_kms_key.dynamodb.arn
  }

  point_in_time_recovery { enabled = true }

  deletion_protection_enabled = true

  tags = { Name = "${local.name_prefix}-audit-logs" }
}

resource "aws_dynamodb_table" "oauth_tokens" {
  name         = "${local.name_prefix}-oauth-tokens"
  billing_mode = "PAY_PER_REQUEST"
  hash_key     = "recipient_id"

  attribute {
    name = "recipient_id"
    type = "S"
  }

  server_side_encryption {
    enabled     = true
    kms_key_arn = aws_kms_key.dynamodb.arn
  }

  point_in_time_recovery { enabled = true }

  deletion_protection_enabled = true

  tags = { Name = "${local.name_prefix}-oauth-tokens" }
}
