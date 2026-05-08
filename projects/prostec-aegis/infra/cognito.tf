resource "aws_cognito_user_pool" "main" {
  name = "${local.name_prefix}-users"

  username_attributes      = ["email"]
  auto_verified_attributes = ["email"]

  username_configuration {
    case_sensitive = false
  }

  password_policy {
    minimum_length                   = 12
    require_lowercase                = true
    require_uppercase                = true
    require_numbers                  = true
    require_symbols                  = true
    temporary_password_validity_days = 7
  }

  # OPTIONAL lets users self-enrol TOTP; set REQUIRED before GA for enterprise tier.
  mfa_configuration = "OPTIONAL"

  software_token_mfa_configuration {
    enabled = true
  }

  account_recovery_setting {
    recovery_mechanism {
      name     = "verified_email"
      priority = 1
    }
  }

  # Switch to SES before launch to control from-address and deliverability.
  email_configuration {
    email_sending_account = "COGNITO_DEFAULT"
  }

  verification_message_template {
    default_email_option = "CONFIRM_WITH_LINK"
  }

  schema {
    name                = "email"
    attribute_data_type = "String"
    required            = true
    mutable             = true

    string_attribute_constraints {
      min_length = 5
      max_length = 254
    }
  }

  # AUDIT logs anomalies without blocking. Flip to ENFORCED once baseline is known.
  user_pool_add_ons {
    advanced_security_mode = "AUDIT"
  }

  # Device tracking — lets user see "active sessions" per device.
  device_configuration {
    challenge_required_on_new_device      = false
    device_only_remembered_on_user_prompt = true
  }

  deletion_protection = "ACTIVE"

  tags = { Name = "${local.name_prefix}-users" }
}

# Server-side API client — secret never exposed to frontend.
resource "aws_cognito_user_pool_client" "api" {
  name         = "${local.name_prefix}-api-client"
  user_pool_id = aws_cognito_user_pool.main.id

  generate_secret = true

  explicit_auth_flows = [
    "ALLOW_USER_SRP_AUTH",      # SRP: password never sent plaintext
    "ALLOW_REFRESH_TOKEN_AUTH",
    "ALLOW_CUSTOM_AUTH",        # for magic-link / OTP flows via Lambda trigger
  ]

  # 30-min access token + 30-day refresh token.
  # Password change must wipe all refresh tokens from DDB (epoch invalidation).
  access_token_validity  = 30
  id_token_validity      = 30
  refresh_token_validity = 30

  token_validity_units {
    access_token  = "minutes"
    id_token      = "minutes"
    refresh_token = "days"
  }

  # Don't leak whether an email address is registered.
  prevent_user_existence_errors = "ENABLED"

  # Revocation endpoint enabled — used when wiping sessions on password change.
  enable_token_revocation = true

  supported_identity_providers = ["COGNITO"]

  read_attributes  = ["email", "email_verified"]
  write_attributes = ["email"]
}

# Cognito-managed domain for the hosted UI and token endpoints.
# Custom domain (auth.aegis.prosteclabs.com) requires a us-east-1 ACM cert — add later.
resource "aws_cognito_user_pool_domain" "main" {
  domain       = "${local.name_prefix}-auth"
  user_pool_id = aws_cognito_user_pool.main.id
}
