//! 两步确认: preview → 用户批准 → 一次性 token → 携 token 重放。
//!
//! 安全模型 (requirements 4.5): 写操作 (建策略/下单) 首次调用返回 preview,
//! 用户批准后获得一次性 token, 携 token 重放才执行; AI 单次调用无法自行通过。

use chrono::Utc;
use ricow_core::{CoreError, CoreResult};
use ricow_strategy::{Database, PreviewRecord};

/// preview 有效期 (秒)。
const PREVIEW_TTL_SECS: i64 = 15 * 60;

/// 创建 preview, 返回 preview_id (status=pending, 无 token)。
pub async fn create_preview(db: &Database, kind: &str, payload_json: &str) -> CoreResult<String> {
    let preview_id = uuid::Uuid::new_v4().to_string();
    let now = Utc::now().timestamp();
    let rec = PreviewRecord {
        preview_id: preview_id.clone(),
        kind: kind.to_string(),
        payload_json: payload_json.to_string(),
        status: "pending".to_string(),
        token: None,
        created_at: now,
        expires_at: now + PREVIEW_TTL_SECS,
    };
    db.insert_preview(&rec).await.map_err(|e| CoreError::Exchange(e.to_string()))?;
    Ok(preview_id)
}

/// 批准 preview: 校验 pending + 未过期 → 生成一次性 token → 原子置 approved → 返回 token。
pub async fn approve(db: &Database, preview_id: &str) -> CoreResult<String> {
    let rec = fetch(db, preview_id).await?;
    if rec.status != "pending" {
        return Err(CoreError::InvalidArgument(format!(
            "preview 状态不是 pending: {}",
            rec.status
        )));
    }
    if Utc::now().timestamp() > rec.expires_at {
        return Err(CoreError::InvalidArgument("preview 已过期".into()));
    }
    let token = uuid::Uuid::new_v4().to_string();
    let affected = db
        .update_preview(preview_id, "pending", "approved", Some(&token))
        .await
        .map_err(|e| CoreError::Exchange(e.to_string()))?;
    if affected != 1 {
        return Err(CoreError::InvalidArgument("preview 状态已被并发修改, 请重新创建".into()));
    }
    Ok(token)
}

/// 消费 preview: 校验 approved + token 匹配 + 未过期 → 原子置 consumed → 返回 payload_json。
pub async fn consume(db: &Database, preview_id: &str, token: &str) -> CoreResult<String> {
    let rec = fetch(db, preview_id).await?;
    if rec.status != "approved" {
        return Err(CoreError::InvalidArgument(format!(
            "preview 状态不是 approved: {}",
            rec.status
        )));
    }
    if rec.token.as_deref() != Some(token) {
        return Err(CoreError::Auth("一次性 token 不匹配".into()));
    }
    let now = Utc::now().timestamp();
    if now > rec.expires_at {
        return Err(CoreError::InvalidArgument("preview 已过期".into()));
    }
    // 原子 CAS: 条件更新防并发双花 (两个携同一 token 的并发调用只有一个能命中)。
    let affected = db
        .update_preview(preview_id, "approved", "consumed", None)
        .await
        .map_err(|e| CoreError::Exchange(e.to_string()))?;
    if affected != 1 {
        return Err(CoreError::InvalidArgument("preview 已被并发消费, 请重新创建".into()));
    }
    Ok(rec.payload_json)
}

/// 拒绝 preview。
pub async fn reject(db: &Database, preview_id: &str) -> CoreResult<()> {
    let rec = fetch(db, preview_id).await?;
    if rec.status != "pending" {
        return Err(CoreError::InvalidArgument(format!(
            "preview 状态不是 pending: {}",
            rec.status
        )));
    }
    // 不校验过期: 过期记录也需要能清理 (标记 rejected 为终态)。
    let affected = db
        .update_preview(preview_id, "pending", "rejected", None)
        .await
        .map_err(|e| CoreError::Exchange(e.to_string()))?;
    if affected != 1 {
        return Err(CoreError::InvalidArgument("preview 状态已被并发修改, 请重新创建".into()));
    }
    Ok(())
}

/// 查询 preview (供 CLI approve 显示预览)。
pub async fn get_preview(db: &Database, preview_id: &str) -> CoreResult<PreviewRecord> {
    fetch(db, preview_id).await
}

async fn fetch(db: &Database, preview_id: &str) -> CoreResult<PreviewRecord> {
    db.get_preview(preview_id)
        .await
        .map_err(|e| CoreError::Exchange(e.to_string()))?
        .ok_or_else(|| CoreError::InvalidArgument(format!("preview 不存在: {preview_id}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn in_memory_db() -> Database {
        Database::open_in_memory().await.expect("open in-memory db")
    }

    #[tokio::test]
    async fn test_confirm_flow() {
        let db = in_memory_db().await;
        let id = create_preview(&db, "order", r#"{"pair":"ETH","side":"buy"}"#).await.unwrap();

        // 未批准前 consume 失败。
        assert!(consume(&db, &id, "wrong").await.is_err());

        // 批准得 token。
        let token = approve(&db, &id).await.unwrap();
        assert!(!token.is_empty());

        // 正确 token 消费成功。
        let payload = consume(&db, &id, &token).await.unwrap();
        assert!(payload.contains("ETH"));

        // 一次性: 再次消费失败。
        assert!(consume(&db, &id, &token).await.is_err());
    }

    #[tokio::test]
    async fn test_approve_wrong_status() {
        let db = in_memory_db().await;
        let id = create_preview(&db, "strategy", "{}").await.unwrap();
        let token = approve(&db, &id).await.unwrap();
        // 已 approved, 再次 approve 失败。
        assert!(approve(&db, &id).await.is_err());
        // 拒绝 (非 pending) 失败。
        assert!(reject(&db, &id).await.is_err());
        let _ = token;
    }

    #[tokio::test]
    async fn test_reject_flow() {
        let db = in_memory_db().await;
        let id = create_preview(&db, "strategy", "{}").await.unwrap();
        reject(&db, &id).await.unwrap();
        // 已 rejected, 批准失败。
        assert!(approve(&db, &id).await.is_err());
    }

    #[tokio::test]
    async fn test_concurrent_consume_single_spend() {
        let db = in_memory_db().await;
        let id = create_preview(&db, "order", "{}").await.unwrap();
        let token = approve(&db, &id).await.unwrap();

        // 并发携带同一有效 token 的两次消费: 恰好一个成功 (原子 CAS 防双花)。
        let (r1, r2) = tokio::join!(consume(&db, &id, &token), consume(&db, &id, &token));
        assert_eq!(r1.is_ok() as u8 + r2.is_ok() as u8, 1, "一次性 token 必须只能消费一次");
        assert!(r1.unwrap_or_default().is_empty() || r2.unwrap_or_default().is_empty());
    }

    #[tokio::test]
    async fn test_concurrent_approve_single_token() {
        let db = in_memory_db().await;
        let id = create_preview(&db, "order", "{}").await.unwrap();

        // 并发批准: 恰好一个成功, 另一个因状态已变失败。
        let (r1, r2) = tokio::join!(approve(&db, &id), approve(&db, &id));
        assert_eq!(r1.is_ok() as u8 + r2.is_ok() as u8, 1, "并发批准只应有一个生效");

        // 生效的那个 token 可正常消费。
        let token = r1.or(r2).unwrap();
        assert!(consume(&db, &id, &token).await.is_ok());
    }
}
