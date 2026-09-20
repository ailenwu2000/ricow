//! Web 会话存储封装 (025 / D9): 只调 [`Database`] 的公开方法, 不直接拼 SQL。
//!
//! 存的是**给浏览器回放的对话流水**(用户输入 / 助手回复 / 宿主带级别的提示行),
//! 与 AI 上下文是两件事: 后者由 [`SessionStore::resume_rounds`] 取最近 N 轮往返喂回会话(FR-020)。

use ricow_core::{CoreError, CoreResult};
use ricow_strategy::{
    Database, WebMessageRecord, WebSessionRecord, WEB_ROLE_ASSISTANT, WEB_ROLE_HOST, WEB_ROLE_USER,
};

use crate::ai::session::{parse_turn_divider, Severity};

/// 会话标题上限(字符数, 非字节): 取首条用户消息压平空白后截断(FR-022)。
const TITLE_MAX_CHARS: usize = 20;

/// 恢复旧会话时装入的往返轮数上限(D10 / FR-020)。
pub const RESUME_ROUNDS: u32 = 20;

/// 会话与消息的读写入口; `Clone` 廉价(内部是连接池), 供 axum 各 handler 共享。
#[derive(Clone)]
pub struct SessionStore {
    db: Database,
}

impl SessionStore {
    pub fn new(db: Database) -> Self {
        Self { db }
    }

    /// 新建一个空会话(标题留空, 由首条用户消息生成), 返回会话 id。
    pub async fn create(&self) -> CoreResult<String> {
        let session_id = uuid::Uuid::new_v4().to_string();
        self.db.create_web_session(&session_id, now_ts()).await.map_err(db_err)?;
        Ok(session_id)
    }

    /// 左侧列表: 按最后活动倒序(FR-007)。
    pub async fn list(&self) -> CoreResult<Vec<WebSessionRecord>> {
        self.db.list_web_sessions().await.map_err(db_err)
    }

    /// 删除会话(消息随外键级联删除); 返回是否命中该会话。
    pub async fn delete(&self, session_id: &str) -> CoreResult<bool> {
        Ok(self.db.delete_web_session(session_id).await.map_err(db_err)? > 0)
    }

    /// 一条用户输入落库; **首条**用户消息顺带生成会话标题(FR-022)。
    pub async fn append_user(&self, session_id: &str, content: &str) -> CoreResult<()> {
        self.db
            .append_web_message(session_id, WEB_ROLE_USER, content, "normal", now_ts())
            .await
            .map_err(db_err)?;
        let title = make_title(content);
        if !title.is_empty() {
            // SQL 自带 `AND title = ''` 守卫: 只有首条用户消息真正写入标题。
            self.db.set_web_title_if_empty(session_id, &title).await.map_err(db_err)?;
        }
        Ok(())
    }

    /// 一条助手回复落库。
    pub async fn append_assistant(&self, session_id: &str, content: &str) -> CoreResult<()> {
        self.db
            .append_web_message(session_id, WEB_ROLE_ASSISTANT, content, "normal", now_ts())
            .await
            .map_err(db_err)
    }

    /// 一条宿主提示行落库(带级别): 重开页面后仍按原级别着色(FR-018 / FR-023)。
    pub async fn append_host(
        &self,
        session_id: &str,
        content: &str,
        sev: Severity,
    ) -> CoreResult<()> {
        self.db
            .append_web_message(session_id, WEB_ROLE_HOST, content, sev.as_str(), now_ts())
            .await
            .map_err(db_err)
    }

    /// 某会话全部消息(写入顺序): 切换/重开会话时整屏回放(FR-021)。
    pub async fn messages(&self, session_id: &str) -> CoreResult<Vec<WebMessageRecord>> {
        self.db.list_web_messages(session_id).await.map_err(db_err)
    }

    /// 最近 [`RESUME_ROUNDS`] 轮往返(不含宿主提示行), 已两两配成 (问, 答), 供恢复 AI 上下文
    /// (D10 / FR-020)。
    pub async fn resume_rounds(&self, session_id: &str) -> CoreResult<Vec<(String, String)>> {
        let rows = self.db.recent_web_rounds(session_id, RESUME_ROUNDS).await.map_err(db_err)?;
        Ok(pair_rounds(&rows))
    }

    /// 页面流水里出现过的**最大轮次号**(扫宿主行的分隔线), 没有则 0。
    ///
    /// 恢复会话时用它接续 `turn`: 与页面回放读的是同一份数据, 新一轮的号因此只增不减。
    /// 不能用 [`Self::resume_rounds`] 的条数代替 —— 那里丢掉了失败轮 / 斜杠命令的 user 行,
    /// 会比页面刚显示的号小, 新一轮就会重号或倒退。
    pub async fn last_turn(&self, session_id: &str) -> CoreResult<u64> {
        let rows = self.db.list_web_messages(session_id).await.map_err(db_err)?;
        Ok(rows
            .iter()
            .filter(|r| r.role == WEB_ROLE_HOST)
            .filter_map(|r| parse_turn_divider(&r.content))
            .max()
            .unwrap_or(0))
    }
}

/// 把流水两两配成 (问, 答)。
///
/// **失败的轮次没有 assistant 行**(见 `ai::session::ChatSession::reply` 的错误分支), 这类落单的
/// user 行要丢掉 —— 会话自己的 `history` 同样不记失败的那一问, 两侧口径保持一致。
fn pair_rounds(rows: &[WebMessageRecord]) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let mut pending: Option<String> = None;
    for row in rows {
        match row.role.as_str() {
            WEB_ROLE_USER => pending = Some(row.content.clone()),
            WEB_ROLE_ASSISTANT => {
                if let Some(q) = pending.take() {
                    pairs.push((q, row.content.clone()));
                }
            }
            _ => {}
        }
    }
    pairs
}

/// 当前 Unix 秒。
fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

/// 首条用户消息 → 标题: 压平空白 + 按**字符**截断(不按字节切, 中文与 emoji 安全)。
fn make_title(first_user_message: &str) -> String {
    let flat = first_user_message.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= TITLE_MAX_CHARS {
        return flat;
    }
    let head: String = flat.chars().take(TITLE_MAX_CHARS).collect();
    format!("{head}…")
}

/// 存储错误统一走会话既有错误类型(与 `commands/mod.rs` 里 `Database::open` 的映射同口径)。
fn db_err(e: impl std::fmt::Display) -> CoreError {
    CoreError::Exchange(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_make_title_flattens_and_truncates_by_chars() {
        assert_eq!(make_title("  今天   行情  如何 "), "今天 行情 如何");
        assert_eq!(make_title(""), "");
        // 上限以内原样保留; 超出按字符截断并标注(不产生半个汉字)。
        let short = "帮我看看策略";
        assert_eq!(make_title(short), short);
        let long = "帮".repeat(30);
        let t = make_title(&long);
        assert_eq!(t.chars().count(), TITLE_MAX_CHARS + 1);
        assert!(t.ends_with('…'));
    }

    /// SC-007: 30 轮旧会话只恢复最近 [`RESUME_ROUNDS`] 轮, 且按写入顺序配对。
    #[tokio::test]
    async fn test_resume_rounds_keeps_the_last_twenty_turns() {
        let store = SessionStore::new(Database::open_in_memory().await.unwrap());
        let sid = store.create().await.unwrap();
        for i in 1..=30 {
            store.append_user(&sid, &format!("问 {i}")).await.unwrap();
            store.append_assistant(&sid, &format!("答 {i}")).await.unwrap();
        }
        let rounds = store.resume_rounds(&sid).await.unwrap();
        assert_eq!(rounds.len(), RESUME_ROUNDS as usize);
        assert_eq!(rounds[0].0, "问 11");
        assert_eq!(rounds[RESUME_ROUNDS as usize - 1].1, "答 30");
    }

    /// 首条用户消息生成标题(FR-022), 且只认首条(后续不覆盖)。
    #[tokio::test]
    async fn test_append_user_sets_title_once() {
        let store = SessionStore::new(Database::open_in_memory().await.unwrap());
        let sid = store.create().await.unwrap();
        store.append_user(&sid, "  今天   行情 ").await.unwrap();
        store.append_user(&sid, "第二个问题").await.unwrap();
        let rows = store.list().await.unwrap();
        assert_eq!(rows[0].title, "今天 行情");
    }

    /// SC-006 / SC-008: 流水落库后按写入顺序整屏回放, 宿主行的级别**原样保留**
    /// (重开页面仍按原色着色, FR-018 / FR-023); 删除会话时消息随外键级联清空。
    #[tokio::test]
    async fn test_messages_replay_keeps_severity_and_delete_cascades() {
        let store = SessionStore::new(Database::open_in_memory().await.unwrap());
        let sid = store.create().await.unwrap();
        store.append_user(&sid, "回测一下").await.unwrap();
        store.append_host(&sid, "── 第 1 轮 ──", Severity::Normal).await.unwrap();
        store.append_assistant(&sid, "跑完了").await.unwrap();
        store.append_host(&sid, "错误: 网络不可达", Severity::Error).await.unwrap();

        let rows = store.messages(&sid).await.unwrap();
        let roles: Vec<&str> = rows.iter().map(|r| r.role.as_str()).collect();
        assert_eq!(roles, vec![WEB_ROLE_USER, WEB_ROLE_HOST, WEB_ROLE_ASSISTANT, WEB_ROLE_HOST]);
        assert_eq!(rows[3].severity, "error", "错误行回放后仍应是错误级(与普通行不同)");
        assert_eq!(rows[1].severity, "normal");
        assert_eq!(rows[0].severity, "normal", "用户行不带级别");

        assert!(store.delete(&sid).await.unwrap(), "首次删除应命中该会话");
        assert!(store.messages(&sid).await.unwrap().is_empty(), "消息应随会话级联删除");
        assert!(!store.delete(&sid).await.unwrap(), "重复删除不再命中");
    }

    /// 恢复会话时接续的轮次号 = 流水里出现过的**最大**号(与页面回放同源), 不随配对丢行而变小。
    #[tokio::test]
    async fn test_last_turn_follows_the_replay_flow() {
        let store = SessionStore::new(Database::open_in_memory().await.unwrap());
        let sid = store.create().await.unwrap();
        assert_eq!(store.last_turn(&sid).await.unwrap(), 0, "空会话从 0 起");

        store.append_user(&sid, "问 1").await.unwrap();
        store.append_host(&sid, "── 第 1 轮 ──", Severity::Normal).await.unwrap();
        store.append_assistant(&sid, "答 1").await.unwrap();
        // 第 2 轮失败: 页面照样显示了分隔线, 但配对时这轮被丢掉
        store.append_user(&sid, "问 2").await.unwrap();
        store.append_host(&sid, "── 第 2 轮 ──", Severity::Normal).await.unwrap();
        store.append_host(&sid, "错误: 网络不可达", Severity::Error).await.unwrap();
        store.append_user(&sid, "问 3").await.unwrap();
        store.append_host(&sid, "── 第 3 轮 ──", Severity::Normal).await.unwrap();
        store.append_assistant(&sid, "答 3").await.unwrap();

        // 配对只剩 2 轮 —— 正是旧口径(`rounds.len()`)会倒退成 2 的场景
        assert_eq!(store.resume_rounds(&sid).await.unwrap().len(), 2);
        assert_eq!(store.last_turn(&sid).await.unwrap(), 3, "接续号取页面最大号, 不随丢行变小");
    }

    /// 失败轮次只有 user 行 → 配不出往返, 直接丢弃(与会话 `history` 口径一致)。
    #[test]
    fn test_pair_rounds_pairs_and_drops_orphan_user_row() {
        let row = |id: i64, role: &str, content: &str| WebMessageRecord {
            message_id: id,
            session_id: "s".to_string(),
            role: role.to_string(),
            content: content.to_string(),
            severity: "normal".to_string(),
            created_at: 0,
        };
        let rows = vec![
            row(1, WEB_ROLE_USER, "问 1"),
            row(2, WEB_ROLE_ASSISTANT, "答 1"),
            // 这一轮失败了(只有错误级 host 行), 落单的 user 行必须丢掉
            row(3, WEB_ROLE_HOST, "错误: 网络不可达"),
            row(4, WEB_ROLE_USER, "问 2"),
            row(5, WEB_ROLE_USER, "问 3"),
            row(6, WEB_ROLE_ASSISTANT, "答 3"),
        ];
        assert_eq!(
            pair_rounds(&rows),
            vec![
                ("问 1".to_string(), "答 1".to_string()),
                ("问 3".to_string(), "答 3".to_string()),
            ]
        );
    }
}
