// Session operations per ADR 0001: atomic rotation, reuse detection, epoch check.

use anyhow::{bail, Result};
use aws_sdk_dynamodb::{
    error::SdkError,
    operation::update_item::UpdateItemError,
    types::AttributeValue,
    Client as DdbClient,
};
use uuid::Uuid;

use crate::models::session::{SessionRecord, SessionView};

pub struct SessionStore<'a> {
    ddb: &'a DdbClient,
    table: String,
}

impl<'a> SessionStore<'a> {
    pub fn new(ddb: &'a DdbClient, table_prefix: &str) -> Self {
        Self {
            ddb,
            table: format!("{}-sessions", table_prefix),
        }
    }

    /// Atomic refresh token rotation using DynamoDB conditional write (version counter).
    /// Returns the new session record on success.
    ///
    /// If ConditionalCheckFailedException → another request already rotated this token.
    /// Callers should return HTTP 400 with error=token_already_rotated.
    pub async fn rotate_token(
        &self,
        recipient_id: Uuid,
        token_id: Uuid,
        current_version: u64,
        new_token_id: Uuid,
        new_token_hash: &str,
        new_expires_at: i64,
    ) -> Result<()> {
        let result = self.ddb
            .update_item()
            .table_name(&self.table)
            .key("recipient_id", AttributeValue::S(recipient_id.to_string()))
            .key("token_id", AttributeValue::S(token_id.to_string()))
            .condition_expression("version = :cv")
            .update_expression(
                "SET token_id = :new_tid, token_value_hash = :new_hash, \
                 version = version + :one, expires_at = :exp, last_used_at = :now"
            )
            .expression_attribute_values(":cv", AttributeValue::N(current_version.to_string()))
            .expression_attribute_values(":new_tid", AttributeValue::S(new_token_id.to_string()))
            .expression_attribute_values(":new_hash", AttributeValue::S(new_token_hash.to_owned()))
            .expression_attribute_values(":one", AttributeValue::N("1".to_owned()))
            .expression_attribute_values(":exp", AttributeValue::N(new_expires_at.to_string()))
            .expression_attribute_values(":now", AttributeValue::S(chrono::Utc::now().to_rfc3339()))
            .send()
            .await;

        match result {
            Ok(_) => Ok(()),
            Err(SdkError::ServiceError(e)) if matches!(e.err(), UpdateItemError::ConditionalCheckFailedException(_)) => {
                bail!("token_already_rotated")
            }
            Err(e) => bail!("dynamodb error: {}", e),
        }
    }

    /// Look up session by token hash (uses GSI token_value_hash-index).
    /// Returns None if not found.
    pub async fn get_by_token_hash(&self, token_hash: &str) -> Result<Option<(String, String)>> {
        let result = self.ddb
            .query()
            .table_name(&self.table)
            .index_name("token_value_hash-index")
            .key_condition_expression("token_value_hash = :h")
            .expression_attribute_values(":h", AttributeValue::S(token_hash.to_owned()))
            .limit(1)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("dynamodb query error: {}", e))?;

        let item = result.items().first();
        match item {
            Some(i) => {
                let recipient_id = i.get("recipient_id")
                    .and_then(|v| v.as_s().ok())
                    .ok_or_else(|| anyhow::anyhow!("missing recipient_id in session record"))?
                    .to_owned();
                let token_id = i.get("token_id")
                    .and_then(|v| v.as_s().ok())
                    .ok_or_else(|| anyhow::anyhow!("missing token_id in session record"))?
                    .to_owned();
                Ok(Some((recipient_id, token_id)))
            }
            None => Ok(None),
        }
    }

    /// Wipe ALL sessions for a recipient (reuse detection response or password change).
    /// Batch-deletes all token rows; callers also invoke AdminUserGlobalSignOut.
    pub async fn revoke_all(&self, recipient_id: Uuid) -> Result<usize> {
        let sessions = self.list_sessions(recipient_id).await?;
        let mut deleted = 0;
        for (token_id,) in &sessions {
            self.ddb
                .delete_item()
                .table_name(&self.table)
                .key("recipient_id", AttributeValue::S(recipient_id.to_string()))
                .key("token_id", AttributeValue::S(token_id.to_owned()))
                .send()
                .await
                .map_err(|e| anyhow::anyhow!("delete session error: {}", e))?;
            deleted += 1;
        }
        Ok(deleted)
    }

    /// List all session token_ids for a recipient (for revoke_all and session cap).
    async fn list_sessions(&self, recipient_id: Uuid) -> Result<Vec<(String,)>> {
        let result = self.ddb
            .query()
            .table_name(&self.table)
            .key_condition_expression("recipient_id = :rid")
            .expression_attribute_values(":rid", AttributeValue::S(recipient_id.to_string()))
            .projection_expression("token_id")
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("dynamodb query error: {}", e))?;

        Ok(result.items()
            .iter()
            .filter_map(|i| {
                i.get("token_id")
                    .and_then(|v| v.as_s().ok())
                    .map(|s| (s.to_owned(),))
            })
            .collect())
    }

    /// Returns active session count for a recipient (for session cap enforcement).
    pub async fn session_count(&self, recipient_id: Uuid) -> Result<usize> {
        let sessions = self.list_sessions(recipient_id).await?;
        Ok(sessions.len())
    }

    /// Get sessions for user-visible listing (GET /me/sessions).
    pub async fn list_for_recipient(&self, recipient_id: Uuid) -> Result<Vec<SessionView>> {
        let result = self.ddb
            .query()
            .table_name(&self.table)
            .key_condition_expression("recipient_id = :rid")
            .expression_attribute_values(":rid", AttributeValue::S(recipient_id.to_string()))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("dynamodb query error: {}", e))?;

        let views = result.items()
            .iter()
            .filter_map(|i| {
                let token_id = i.get("token_id")?.as_s().ok()?;
                let device_hint = i.get("device_hint")?.as_s().ok()?.to_owned();
                let ip = i.get("ip_at_creation")?.as_s().ok()?.to_owned();
                let last_used = i.get("last_used_at")?.as_s().ok()?;
                let last_used_at = chrono::DateTime::parse_from_rfc3339(last_used).ok()?.with_timezone(&chrono::Utc);
                let auth_provider = i.get("auth_provider")?.as_s().ok()?.to_owned();
                let created = i.get("created_at")?.as_s().ok()?;
                let created_at = chrono::DateTime::parse_from_rfc3339(created).ok()?.with_timezone(&chrono::Utc);
                let trusted = i.get("trusted").and_then(|v| v.as_bool().ok()).copied().unwrap_or(false);

                Some(SessionView {
                    token_id_prefix: token_id[..8.min(token_id.len())].to_owned(),
                    device_hint,
                    ip_at_creation: ip,
                    last_used_at,
                    auth_provider,
                    created_at,
                    trusted,
                })
            })
            .collect();

        Ok(views)
    }
}
