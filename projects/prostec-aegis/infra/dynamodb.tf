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

# Fix H5: renamed from oauth_tokens to oauth_cloud_tokens; table name changed to
# oauth-cloud-tokens; key schema changed to composite recipient_id + provider to match
# the code's composite key pattern (one row per recipient×provider, not per issued token).
# token_value_hash GSI retained for O(1) bearer-token lookup.
resource "aws_dynamodb_table" "oauth_cloud_tokens" {
  name         = "${local.name_prefix}-oauth-cloud-tokens"
  billing_mode = "PAY_PER_REQUEST"
  hash_key     = "recipient_id"
  range_key    = "provider"

  attribute {
    name = "recipient_id"
    type = "S"
  }

  attribute {
    name = "provider"
    type = "S"
  }

  attribute {
    name = "token_value_hash"
    type = "S"
  }

  # KEYS_ONLY: lookup by bearer token hash → get recipient_id + provider for session validation.
  global_secondary_index {
    name            = "token_value_hash-index"
    hash_key        = "token_value_hash"
    projection_type = "KEYS_ONLY"
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

  tags = { Name = "${local.name_prefix}-oauth-cloud-tokens" }
}

# Fix C2: sessions table — one row per active session per ADR 0001.
# hash=recipient_id + range=token_id supports multiple concurrent sessions per recipient.
# token_value_hash-index enables O(1) lookup by bearer token hash on every request.
resource "aws_dynamodb_table" "sessions" {
  name         = "${local.name_prefix}-sessions"
  billing_mode = "PAY_PER_REQUEST"
  hash_key     = "recipient_id"
  range_key    = "token_id"

  attribute {
    name = "recipient_id"
    type = "S"
  }

  attribute {
    name = "token_id"
    type = "S"
  }

  attribute {
    name = "token_value_hash"
    type = "S"
  }

  global_secondary_index {
    name            = "token_value_hash-index"
    hash_key        = "token_value_hash"
    projection_type = "KEYS_ONLY"
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

  tags = { Name = "${local.name_prefix}-sessions" }
}

# Fix C2: deliveries table — tracks per-recipient delivery records.
# doc-id-index enables lookup by document ID across all recipients.
# sender-index enables lookup by sender with time-ordered range on delivered_at.
resource "aws_dynamodb_table" "deliveries" {
  name         = "${local.name_prefix}-deliveries"
  billing_mode = "PAY_PER_REQUEST"
  hash_key     = "recipient_id"
  range_key    = "delivery_id"

  attribute {
    name = "recipient_id"
    type = "S"
  }

  attribute {
    name = "delivery_id"
    type = "S"
  }

  attribute {
    name = "doc_id"
    type = "S"
  }

  attribute {
    name = "sender_id"
    type = "S"
  }

  attribute {
    name = "delivered_at"
    type = "S"
  }

  global_secondary_index {
    name            = "doc-id-index"
    hash_key        = "doc_id"
    projection_type = "ALL"
  }

  global_secondary_index {
    name            = "sender-index"
    hash_key        = "sender_id"
    range_key       = "delivered_at"
    projection_type = "ALL"
  }

  # TTL attribute: unix seconds, set to expires_at + grace period on write.
  # DynamoDB auto-deletes expired delivery rows within ~48h of the TTL value.
  ttl {
    attribute_name = "expires_at_ttl"
    enabled        = true
  }

  server_side_encryption {
    enabled     = true
    kms_key_arn = aws_kms_key.dynamodb.arn
  }

  point_in_time_recovery { enabled = true }

  deletion_protection_enabled = true

  tags = { Name = "${local.name_prefix}-deliveries" }
}

# Fix C2: recipient-settings table — simple key-value settings store per recipient.
resource "aws_dynamodb_table" "recipient_settings" {
  name         = "${local.name_prefix}-recipient-settings"
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

  tags = { Name = "${local.name_prefix}-recipient-settings" }
}

# Streaming upload sessions for chunked envelope encryption (architecture §"Streaming Encryption").
# DynamoDB TTL on `expires_at` auto-deletes orphaned sessions (e.g., agent crashed mid-stream).
# Session lifecycle: init → upload chunks direct to cloud → complete (submit signed manifest)
# OR abort (DELETE) OR auto-expire after 24h.
resource "aws_dynamodb_table" "streaming_uploads" {
  name         = "${local.name_prefix}-streaming-uploads"
  billing_mode = "PAY_PER_REQUEST"
  hash_key     = "upload_uuid"

  attribute {
    name = "upload_uuid"
    type = "S"
  }

  ttl {
    enabled        = true
    attribute_name = "expires_at_unix"
  }

  server_side_encryption {
    enabled     = true
    kms_key_arn = aws_kms_key.dynamodb.arn
  }

  point_in_time_recovery { enabled = true }

  deletion_protection_enabled = true

  tags = { Name = "${local.name_prefix}-streaming-uploads" }
}
