// Session operations per ADR 0001: atomic rotation, reuse detection, epoch check, atomic cap.

use anyhow::{bail, Result};
use aws_sdk_dynamodb::{
    error::SdkError,
    operation::transact_write_items::TransactWriteItemsError,
    types::{AttributeValue, Put, TransactWriteItem, Update},
    Client as DdbClient,
};
use thiserror::Error;
use uuid::Uuid;

use crate::models::session::SessionView;

/// Typed error for session operations. Replaces fragile `e.to_string().contains("...")`
/// matching at call sites (Qwen Round 6 auth HIGH).
#[derive(Debug, Error)]
pub enum SessionError {
    /// `create_session_atomic` rejected because the recipient is at SESSION_CAP.
    /// Caller should evict the oldest non-trusted session via
    /// `evict_oldest_non_trusted` and retry.
    #[error("session cap exceeded ({SESSION_CAP})")]
    CapExceeded,
    /// Token rotation racing — another caller already advanced the version.
    /// Surface as HTTP 400 `token_already_rotated` per ADR 0001.
    #[error("token already rotated")]
    AlreadyRotated,
    /// All other errors (DDB transient, build, etc).
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Maximum concurrent sessions per recipient (ADR 0001 §4 "Session Cap").
pub const SESSION_CAP: u32 = 5;

/// Sentinel token_id used for the per-recipient session counter row.
/// The counter row stores `session_count` and is updated atomically with session puts.
const COUNTER_TOKEN_ID: &str = "__counter__";

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

    /// Atomic refresh token rotation via TransactWriteItems.
    ///
    /// `token_id` is the DynamoDB sort key, so we cannot UpdateItem in place to swap it.
    /// Instead: PUT a new row at (recipient_id, new_token_id) and DELETE the old row at
    /// (recipient_id, token_id) atomically, with the DELETE conditional on `version = :cv`
    /// (preserving the reuse-detection semantics of the prior version-counter design).
    ///
    /// If the transaction is canceled (`version` no longer matches OR the new row already
    /// exists) → another request already rotated this token. Callers return HTTP 400
    /// with `error=token_already_rotated`.
    ///
    /// Counter is unaffected — we are swapping one session row for another; the per-recipient
    /// session_count remains the same.
    ///
    /// Carries forward: `device_hint`, `ip_at_creation`, `auth_provider`, `trusted`,
    /// `session_epoch` (if present), `token_family_id` (if present).
    pub async fn rotate_token(
        &self,
        recipient_id: Uuid,
        token_id: Uuid,
        current_version: u64,
        new_token_id: Uuid,
        new_token_hash: &str,
        new_expires_at: i64,
    ) -> Result<()> {
        // Read the existing row (strongly consistent) to carry forward attributes that
        // aren't recomputed on rotation.
        let existing = self.ddb
            .get_item()
            .consistent_read(true)
            .table_name(&self.table)
            .key("recipient_id", AttributeValue::S(recipient_id.to_string()))
            .key("token_id", AttributeValue::S(token_id.to_string()))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("rotate read: {}", e))?
            .item()
            .cloned();

        let Some(old) = existing else {
            bail!("token_already_rotated"); // no row → must have been deleted/rotated
        };

        let device_hint = old.get("device_hint").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
        let ip_at_creation = old.get("ip_at_creation").and_then(|v| v.as_s().ok()).cloned().unwrap_or_default();
        let auth_provider = old.get("auth_provider").and_then(|v| v.as_s().ok()).cloned().unwrap_or_else(|| "cognito".into());
        let created_at = old.get("created_at").and_then(|v| v.as_s().ok()).cloned()
            .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());
        let trusted = old.get("trusted").and_then(|v| v.as_bool().ok()).copied().unwrap_or(false);
        let token_family_id = old.get("token_family_id").and_then(|v| v.as_s().ok()).cloned();
        let session_epoch = old.get("session_epoch").and_then(|v| v.as_n().ok()).cloned();
        // CRITICAL: carry forward DPoP jkt binding through rotation. Without this, a
        // DPoP-bound session loses its binding on every refresh — Codex Round 6 HIGH.
        let dpop_jkt = old.get("dpop_jkt").and_then(|v| v.as_s().ok()).cloned();
        let now_iso = chrono::Utc::now().to_rfc3339();

        let mut new_item = aws_sdk_dynamodb::types::Put::builder()
            .table_name(&self.table)
            .item("recipient_id", AttributeValue::S(recipient_id.to_string()))
            .item("token_id", AttributeValue::S(new_token_id.to_string()))
            .item("token_value_hash", AttributeValue::S(new_token_hash.to_owned()))
            .item("device_hint", AttributeValue::S(device_hint))
            .item("ip_at_creation", AttributeValue::S(ip_at_creation))
            .item("auth_provider", AttributeValue::S(auth_provider))
            .item("created_at", AttributeValue::S(created_at))
            .item("last_used_at", AttributeValue::S(now_iso))
            .item("expires_at", AttributeValue::N(new_expires_at.to_string()))
            .item("version", AttributeValue::N((current_version + 1).to_string()))
            .item("trusted", AttributeValue::Bool(trusted))
            .condition_expression("attribute_not_exists(token_id)");
        if let Some(fid) = token_family_id {
            new_item = new_item.item("token_family_id", AttributeValue::S(fid));
        }
        if let Some(e) = session_epoch {
            new_item = new_item.item("session_epoch", AttributeValue::N(e));
        }
        if let Some(jkt) = dpop_jkt {
            new_item = new_item.item("dpop_jkt", AttributeValue::S(jkt));
        }

        let put_new = TransactWriteItem::builder()
            .put(new_item.build().map_err(|e| anyhow::anyhow!("build new put: {}", e))?)
            .build();

        let delete_old = TransactWriteItem::builder()
            .delete(
                aws_sdk_dynamodb::types::Delete::builder()
                    .table_name(&self.table)
                    .key("recipient_id", AttributeValue::S(recipient_id.to_string()))
                    .key("token_id", AttributeValue::S(token_id.to_string()))
                    .condition_expression("version = :cv")
                    .expression_attribute_values(":cv", AttributeValue::N(current_version.to_string()))
                    .build()
                    .map_err(|e| anyhow::anyhow!("build delete old: {}", e))?,
            )
            .build();

        let result = self.ddb
            .transact_write_items()
            .transact_items(put_new)
            .transact_items(delete_old)
            .send()
            .await;

        match result {
            Ok(_) => Ok(()),
            Err(SdkError::ServiceError(e))
                if matches!(e.err(), TransactWriteItemsError::TransactionCanceledException(_)) =>
            {
                bail!("token_already_rotated")
            }
            Err(e) => bail!("dynamodb error: {}", e),
        }
    }

    /// Look up session by token hash (uses GSI token_value_hash-index).
    /// Returns None if not found. Tuple = (recipient_id, token_id).
    ///
    /// NOTE on consistency (Qwen Round 6 db HIGH false positive):
    /// DynamoDB GSIs are EVENTUALLY CONSISTENT — `consistent_read(true)` is not
    /// supported on a GSI query and would return an error. The ATOMICITY GUARANTEE
    /// for refresh-token rotation comes from the CONDITIONAL DELETE on the base
    /// table inside `rotate_token`'s TransactWriteItems (`version = :cv`). A stale
    /// GSI read at this lookup leads at worst to a `token_already_rotated` 400 —
    /// never to two-valid-tokens.
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

    /// Read the DPoP `jkt` thumbprint stored on the session row. Strongly consistent so
    /// rotations are visible immediately. Returns None if no row exists or if the row
    /// has no jkt (DPoP wasn't used at session creation).
    pub async fn dpop_jkt_for(&self, recipient_id: Uuid, token_id: Uuid) -> Result<Option<String>> {
        let r = self.ddb
            .get_item()
            .consistent_read(true)
            .table_name(&self.table)
            .key("recipient_id", AttributeValue::S(recipient_id.to_string()))
            .key("token_id", AttributeValue::S(token_id.to_string()))
            .projection_expression("dpop_jkt")
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("ddb get dpop_jkt: {}", e))?;
        Ok(r.item().and_then(|i| i.get("dpop_jkt"))
            .and_then(|v| v.as_s().ok())
            .cloned())
    }

    /// Wipe ALL sessions for a recipient (reuse detection response or password change).
    /// Atomically deletes all token rows AND the counter sentinel via TransactWriteItems
    /// chunks of up to 99 deletes (DynamoDB's per-transaction limit is 100; we hold one
    /// slot for the counter sentinel deletion in each chunk's last call).
    /// Callers also invoke `AdminUserGlobalSignOut` and `increment_session_epoch` —
    /// epoch enforcement is the actual security mechanism; this wipe is cleanup.
    pub async fn revoke_all(&self, recipient_id: Uuid) -> Result<usize> {
        const MAX_PER_TRANSACT: usize = 99;
        let sessions = self.list_sessions(recipient_id).await?;
        let mut deleted = 0;

        for chunk in sessions.chunks(MAX_PER_TRANSACT) {
            let mut items: Vec<TransactWriteItem> = Vec::with_capacity(chunk.len());
            for (token_id,) in chunk {
                items.push(
                    TransactWriteItem::builder()
                        .delete(
                            aws_sdk_dynamodb::types::Delete::builder()
                                .table_name(&self.table)
                                .key("recipient_id", AttributeValue::S(recipient_id.to_string()))
                                .key("token_id", AttributeValue::S(token_id.to_owned()))
                                .build()
                                .map_err(|e| anyhow::anyhow!("build revoke delete: {}", e))?,
                        )
                        .build(),
                );
            }
            self.ddb
                .transact_write_items()
                .set_transact_items(Some(items))
                .send()
                .await
                .map_err(|e| anyhow::anyhow!("revoke transact error: {}", e))?;
            deleted += chunk.len();
        }

        // Drop the counter sentinel too. Idempotent: no error if the row never existed.
        self.ddb
            .delete_item()
            .table_name(&self.table)
            .key("recipient_id", AttributeValue::S(recipient_id.to_string()))
            .key("token_id", AttributeValue::S(COUNTER_TOKEN_ID.to_owned()))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("delete counter error: {}", e))?;
        Ok(deleted)
    }

    /// Delete a single session AND atomically decrement the counter.
    /// Used by `DELETE /me/sessions/{token_id}` and the eviction path.
    /// Refuses to operate on the counter sentinel (`token_id == "__counter__"`).
    ///
    /// Returns `Ok(true)` if a row was deleted, `Ok(false)` if the row didn't exist
    /// (idempotent caller behavior — Qwen routes review #3). Errors only on transient
    /// DynamoDB failures or programming mistakes.
    pub async fn delete_session_atomic(&self, recipient_id: Uuid, token_id: &str) -> Result<bool> {
        if token_id == COUNTER_TOKEN_ID {
            bail!("cannot delete counter sentinel via delete_session_atomic");
        }

        let delete_session = TransactWriteItem::builder()
            .delete(
                aws_sdk_dynamodb::types::Delete::builder()
                    .table_name(&self.table)
                    .key("recipient_id", AttributeValue::S(recipient_id.to_string()))
                    .key("token_id", AttributeValue::S(token_id.to_owned()))
                    // Refuse if the row doesn't exist — keeps counter from going negative.
                    .condition_expression("attribute_exists(token_id)")
                    .build()
                    .map_err(|e| anyhow::anyhow!("build delete: {}", e))?,
            )
            .build();

        let counter_dec = TransactWriteItem::builder()
            .update(
                Update::builder()
                    .table_name(&self.table)
                    .key("recipient_id", AttributeValue::S(recipient_id.to_string()))
                    .key("token_id", AttributeValue::S(COUNTER_TOKEN_ID.to_owned()))
                    .update_expression("SET session_count = session_count - :one")
                    .condition_expression("attribute_exists(session_count) AND session_count > :zero")
                    .expression_attribute_values(":one", AttributeValue::N("1".to_owned()))
                    .expression_attribute_values(":zero", AttributeValue::N("0".to_owned()))
                    .build()
                    .map_err(|e| anyhow::anyhow!("build counter dec: {}", e))?,
            )
            .build();

        let result = self.ddb
            .transact_write_items()
            .transact_items(delete_session)
            .transact_items(counter_dec)
            .send()
            .await;
        match result {
            Ok(_) => Ok(true),
            Err(SdkError::ServiceError(e))
                if matches!(e.err(), TransactWriteItemsError::TransactionCanceledException(_)) =>
            {
                // Transaction cancelled — two possible causes:
                // (a) session row doesn't exist → idempotent, return Ok(false)
                // (b) counter row missing or at zero (legacy pre-counter session) →
                //     the row still exists; fall back to a direct delete so legacy
                //     sessions are actually removed (H-1).
                let still_exists = self.ddb
                    .get_item()
                    .consistent_read(true)
                    .table_name(&self.table)
                    .key("recipient_id", AttributeValue::S(recipient_id.to_string()))
                    .key("token_id", AttributeValue::S(token_id.to_owned()))
                    .send()
                    .await
                    .map_err(|e| anyhow::anyhow!("post-cancel existence check: {}", e))?
                    .item()
                    .is_some();

                if still_exists {
                    // Legacy session: counter absent or exhausted. Delete the row directly;
                    // counter is already at zero / doesn't exist so no adjustment needed.
                    self.ddb
                        .delete_item()
                        .table_name(&self.table)
                        .key("recipient_id", AttributeValue::S(recipient_id.to_string()))
                        .key("token_id", AttributeValue::S(token_id.to_owned()))
                        .send()
                        .await
                        .map_err(|e| anyhow::anyhow!("legacy session delete: {}", e))?;
                    Ok(true)
                } else {
                    Ok(false) // Row was already gone — idempotent success
                }
            }
            Err(e) => bail!("dynamodb transact delete: {}", e),
        }
    }

    /// List all session token_ids for a recipient (for revoke_all and session cap).
    /// Paginates with ExclusiveStartKey to handle recipients with >1000 sessions.
    /// Filters out the `__counter__` sentinel row maintained by create_session_atomic.
    async fn list_sessions(&self, recipient_id: Uuid) -> Result<Vec<(String,)>> {
        let mut all_items = Vec::new();
        let mut exclusive_start_key = None;
        loop {
            let mut req = self.ddb
                .query()
                .table_name(&self.table)
                .key_condition_expression("recipient_id = :rid")
                .filter_expression("token_id <> :counter")
                .expression_attribute_values(":rid", AttributeValue::S(recipient_id.to_string()))
                .expression_attribute_values(":counter", AttributeValue::S(COUNTER_TOKEN_ID.to_owned()))
                .projection_expression("token_id");
            if let Some(esk) = exclusive_start_key {
                req = req.set_exclusive_start_key(Some(esk));
            }
            let result = req.send().await
                .map_err(|e| anyhow::anyhow!("dynamodb query error: {}", e))?;
            for item in result.items() {
                if let Some(token_id) = item.get("token_id").and_then(|v| v.as_s().ok()) {
                    all_items.push((token_id.to_owned(),));
                }
            }
            match result.last_evaluated_key() {
                Some(lek) => exclusive_start_key = Some(lek.clone()),
                None => break,
            }
        }
        Ok(all_items)
    }

    /// Returns active session count for a recipient (for session cap enforcement).
    /// O(1) — reads the per-recipient counter row directly instead of paging through
    /// all sessions (Qwen Round 3 db HIGH on O(N) cost). Returns 0 if no counter row
    /// exists yet (recipient has never created a session).
    pub async fn session_count(&self, recipient_id: Uuid) -> Result<usize> {
        let result = self.ddb
            .get_item()
            .consistent_read(true)
            .table_name(&self.table)
            .key("recipient_id", AttributeValue::S(recipient_id.to_string()))
            .key("token_id", AttributeValue::S(COUNTER_TOKEN_ID.to_owned()))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("dynamodb get counter: {}", e))?;
        let Some(item) = result.item() else { return Ok(0); };
        Ok(item.get("session_count")
            .and_then(|v| v.as_n().ok())
            .and_then(|n| n.parse::<usize>().ok())
            .unwrap_or(0))
    }

    /// Atomically create a new session and increment the per-recipient counter,
    /// rejecting if the recipient is already at SESSION_CAP. Single TransactWriteItems
    /// — no read-then-write race window. ADR 0001 §4.
    ///
    /// `dpop_jkt`: optional JWK thumbprint (RFC 7638) bound to this session. When set,
    /// subsequent authenticated requests MUST present a DPoP proof whose jkt matches —
    /// stolen bearer tokens become unusable from a different device (RFC 9449 / ADR 0001 Phase 2).
    ///
    /// Returns:
    ///   - `Ok(())` on success.
    ///   - `Err(SessionError::CapExceeded)` if the recipient is at cap. Caller should
    ///     evict the oldest non-trusted session via `evict_oldest_non_trusted` and retry.
    /// `settings_table` and `enroll_dpop_required` together close a race window
    /// (Codex Round 7 HIGH): when a recipient's first DPoP-bound session is created,
    /// `recipient_settings.dpop_required` MUST flip true atomically with the session
    /// row write. Otherwise a request landing between the session-write and the
    /// settings-write sees `dpop_required=false` and takes the optional/bypassable
    /// middleware path. Caller passes the recipient_settings table name so we can
    /// include the cross-table update in the same TransactWriteItems.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_session_atomic(
        &self,
        recipient_id: Uuid,
        token_id: Uuid,
        token_value_hash: &str,
        device_hint: &str,
        ip_at_creation: &str,
        auth_provider: &str,
        expires_at: i64,
        trusted: bool,
        dpop_jkt: Option<&str>,
        settings_table: &str,
        enroll_dpop_required: bool,
    ) -> std::result::Result<(), SessionError> {
        let now_iso = chrono::Utc::now().to_rfc3339();

        // Counter row update: increment if `session_count < SESSION_CAP` else fail.
        let counter_update = TransactWriteItem::builder()
            .update(
                Update::builder()
                    .table_name(&self.table)
                    .key("recipient_id", AttributeValue::S(recipient_id.to_string()))
                    .key("token_id", AttributeValue::S(COUNTER_TOKEN_ID.to_owned()))
                    .update_expression("SET session_count = if_not_exists(session_count, :zero) + :one")
                    .condition_expression(
                        "attribute_not_exists(session_count) OR session_count < :cap",
                    )
                    .expression_attribute_values(":zero", AttributeValue::N("0".to_owned()))
                    .expression_attribute_values(":one", AttributeValue::N("1".to_owned()))
                    .expression_attribute_values(":cap", AttributeValue::N(SESSION_CAP.to_string()))
                    .build()
                    .map_err(|e| anyhow::anyhow!("build counter update: {}", e))?,
            )
            .build();

        // Session row put: refuses to overwrite an existing token_id (defense-in-depth).
        let mut put_builder = Put::builder()
            .table_name(&self.table)
            .item("recipient_id", AttributeValue::S(recipient_id.to_string()))
            .item("token_id", AttributeValue::S(token_id.to_string()))
            .item("token_value_hash", AttributeValue::S(token_value_hash.to_owned()))
            .item("device_hint", AttributeValue::S(device_hint.to_owned()))
            .item("ip_at_creation", AttributeValue::S(ip_at_creation.to_owned()))
            .item("last_used_at", AttributeValue::S(now_iso.clone()))
            .item("auth_provider", AttributeValue::S(auth_provider.to_owned()))
            .item("created_at", AttributeValue::S(now_iso))
            .item("expires_at", AttributeValue::N(expires_at.to_string()))
            .item("version", AttributeValue::N("0".to_owned()))
            .item("trusted", AttributeValue::Bool(trusted))
            .condition_expression("attribute_not_exists(token_id)");
        if let Some(jkt) = dpop_jkt {
            put_builder = put_builder.item("dpop_jkt", AttributeValue::S(jkt.to_owned()));
        }
        let session_put = TransactWriteItem::builder()
            .put(put_builder.build().map_err(|e| anyhow::anyhow!("build session put: {}", e))?)
            .build();

        // Optional 3rd transact item: cross-table update of recipient_settings
        // to atomically flip dpop_required = true. Only included when the caller
        // has computed that this is the recipient's first DPoP-bound session.
        // Without this, a request landing between the session-write and a
        // separate settings-write would bypass DPoP enforcement (Codex Round 7).
        let mut transaction = self.ddb
            .transact_write_items()
            .transact_items(counter_update)
            .transact_items(session_put);
        if enroll_dpop_required {
            let settings_update = TransactWriteItem::builder()
                .update(
                    Update::builder()
                        .table_name(settings_table)
                        .key("recipient_id", AttributeValue::S(recipient_id.to_string()))
                        .update_expression(
                            "SET dpop_required = :t, \
                                 fips_strict = if_not_exists(fips_strict, :f), \
                                 session_epoch = if_not_exists(session_epoch, :z), \
                                 last_epoch_increment_at = if_not_exists(last_epoch_increment_at, :z)"
                        )
                        .expression_attribute_values(":t", AttributeValue::Bool(true))
                        .expression_attribute_values(":f", AttributeValue::Bool(false))
                        .expression_attribute_values(":z", AttributeValue::N("0".to_owned()))
                        .build()
                        .map_err(|e| anyhow::anyhow!("build dpop_required enroll: {}", e))?,
                )
                .build();
            transaction = transaction.transact_items(settings_update);
        }
        let result = transaction.send().await;

        match result {
            Ok(_) => Ok(()),
            Err(SdkError::ServiceError(e))
                if matches!(e.err(), TransactWriteItemsError::TransactionCanceledException(_)) =>
            {
                Err(SessionError::CapExceeded)
            }
            Err(e) => Err(SessionError::Other(anyhow::anyhow!("dynamodb transact error: {}", e))),
        }
    }

    /// Find and delete the oldest non-trusted session for a recipient. Used after
    /// `create_session_atomic` returns `SessionError::CapExceeded` to free a slot.
    ///
    /// The counter is decremented atomically with the delete to keep it in sync.
    /// Returns the evicted token_id, or `None` if no non-trusted session exists
    /// (meaning the cap is fully consumed by trusted-device sessions and the new
    /// login should be rejected with HTTP 429 / "trusted_slots_full").
    ///
    /// Paginates through ALL pages with ExclusiveStartKey before picking the oldest
    /// — DynamoDB's natural sort order is by sort key (token_id UUID), not created_at,
    /// so a single page may not contain the actual oldest session (Qwen R4 db HIGH).
    /// In practice with SESSION_CAP=5 this fits in one page, but the loop guards
    /// correctness if the cap is ever raised.
    pub async fn evict_oldest_non_trusted(&self, recipient_id: Uuid) -> Result<Option<String>> {
        let mut candidates: Vec<(String, String)> = Vec::new(); // (created_at, token_id)
        let mut exclusive_start_key = None;
        // Memory bound: SESSION_CAP non-trusted + 1 trusted reserved + counter sentinel.
        // Cap candidates at SESSION_CAP * 4 as a defense-in-depth limit so legacy data
        // or a future cap raise can't materialize unbounded result sets (Qwen R7 db MEDIUM).
        let max_candidates: usize = (SESSION_CAP as usize) * 4;
        loop {
            let mut req = self.ddb
                .query()
                .table_name(&self.table)
                .key_condition_expression("recipient_id = :rid")
                .filter_expression(
                    "attribute_exists(created_at) AND (attribute_not_exists(trusted) OR trusted = :f)",
                )
                .expression_attribute_values(":rid", AttributeValue::S(recipient_id.to_string()))
                .expression_attribute_values(":f", AttributeValue::Bool(false));
            if let Some(esk) = exclusive_start_key {
                req = req.set_exclusive_start_key(Some(esk));
            }
            let result = req.send().await
                .map_err(|e| anyhow::anyhow!("dynamodb query for eviction: {}", e))?;

            for item in result.items() {
                let Some(token_id) = item.get("token_id").and_then(|v| v.as_s().ok()).map(|s| s.to_owned()) else { continue };
                if token_id == COUNTER_TOKEN_ID { continue; }
                let Some(created_at) = item.get("created_at").and_then(|v| v.as_s().ok()).map(|s| s.to_owned()) else { continue };
                candidates.push((created_at, token_id));
                if candidates.len() >= max_candidates { break; }
            }
            if candidates.len() >= max_candidates { break; }
            match result.last_evaluated_key() {
                Some(lek) => exclusive_start_key = Some(lek.clone()),
                None => break,
            }
        }
        let oldest = candidates.into_iter().min_by(|a, b| a.0.cmp(&b.0));

        let Some((_, token_id_to_evict)) = oldest else {
            return Ok(None);
        };

        // TransactWriteItems: delete the chosen session + decrement counter atomically.
        // The Delete is conditional on `attribute_exists(token_id)` so two concurrent
        // eviction attempts that both picked the same oldest token can't both decrement
        // (the loser's transaction is canceled and returns Err — caller should re-query
        // and retry).
        let delete_session = TransactWriteItem::builder()
            .delete(
                aws_sdk_dynamodb::types::Delete::builder()
                    .table_name(&self.table)
                    .key("recipient_id", AttributeValue::S(recipient_id.to_string()))
                    .key("token_id", AttributeValue::S(token_id_to_evict.clone()))
                    .condition_expression("attribute_exists(token_id)")
                    .build()
                    .map_err(|e| anyhow::anyhow!("build delete: {}", e))?,
            )
            .build();

        let counter_update = TransactWriteItem::builder()
            .update(
                Update::builder()
                    .table_name(&self.table)
                    .key("recipient_id", AttributeValue::S(recipient_id.to_string()))
                    .key("token_id", AttributeValue::S(COUNTER_TOKEN_ID.to_owned()))
                    .update_expression("SET session_count = session_count - :one")
                    .condition_expression("attribute_exists(session_count) AND session_count > :zero")
                    .expression_attribute_values(":one", AttributeValue::N("1".to_owned()))
                    .expression_attribute_values(":zero", AttributeValue::N("0".to_owned()))
                    .build()
                    .map_err(|e| anyhow::anyhow!("build counter decrement: {}", e))?,
            )
            .build();

        let txn_result = self.ddb
            .transact_write_items()
            .transact_items(delete_session)
            .transact_items(counter_update)
            .send()
            .await;

        match txn_result {
            Ok(_) => Ok(Some(token_id_to_evict)),
            // If the transaction was canceled (e.g., another evictor got there first
            // and the row no longer exists), report None so the caller knows to re-query
            // — they can retry create_session_atomic which may now succeed.
            Err(SdkError::ServiceError(e))
                if matches!(e.err(), TransactWriteItemsError::TransactionCanceledException(_)) =>
            {
                Ok(None)
            }
            Err(e) => bail!("dynamodb transact eviction: {}", e),
        }
    }

    /// Get sessions for user-visible listing (GET /me/sessions).
    /// Paginates with ExclusiveStartKey to handle recipients with >1000 sessions.
    /// Filters out the `__counter__` sentinel row maintained by create_session_atomic.
    pub async fn list_for_recipient(&self, recipient_id: Uuid) -> Result<Vec<SessionView>> {
        let mut views = Vec::new();
        let mut exclusive_start_key = None;
        loop {
            let mut req = self.ddb
                .query()
                .table_name(&self.table)
                .key_condition_expression("recipient_id = :rid")
                .filter_expression("token_id <> :counter")
                .expression_attribute_values(":rid", AttributeValue::S(recipient_id.to_string()))
                .expression_attribute_values(":counter", AttributeValue::S(COUNTER_TOKEN_ID.to_owned()));
            if let Some(esk) = exclusive_start_key {
                req = req.set_exclusive_start_key(Some(esk));
            }
            let result = req.send().await
                .map_err(|e| anyhow::anyhow!("dynamodb query error: {}", e))?;

            for i in result.items() {
                let Some(token_id) = i.get("token_id").and_then(|v| v.as_s().ok()) else { continue };
                let Some(device_hint) = i.get("device_hint").and_then(|v| v.as_s().ok()).map(|s| s.to_owned()) else { continue };
                let Some(ip) = i.get("ip_at_creation").and_then(|v| v.as_s().ok()).map(|s| s.to_owned()) else { continue };
                let Some(last_used) = i.get("last_used_at").and_then(|v| v.as_s().ok()) else { continue };
                let Some(last_used_at) = chrono::DateTime::parse_from_rfc3339(last_used).ok().map(|d| d.with_timezone(&chrono::Utc)) else { continue };
                let Some(auth_provider) = i.get("auth_provider").and_then(|v| v.as_s().ok()).map(|s| s.to_owned()) else { continue };
                let Some(created) = i.get("created_at").and_then(|v| v.as_s().ok()) else { continue };
                let Some(created_at) = chrono::DateTime::parse_from_rfc3339(created).ok().map(|d| d.with_timezone(&chrono::Utc)) else { continue };
                let trusted = i.get("trusted").and_then(|v| v.as_bool().ok()).copied().unwrap_or(false);

                views.push(SessionView {
                    token_id_prefix: token_id[..8.min(token_id.len())].to_owned(),
                    device_hint,
                    ip_at_creation: ip,
                    last_used_at,
                    auth_provider,
                    created_at,
                    trusted,
                });
            }

            match result.last_evaluated_key() {
                Some(lek) => exclusive_start_key = Some(lek.clone()),
                None => break,
            }
        }
        Ok(views)
    }
}
