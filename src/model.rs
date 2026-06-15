//! 统一数据模型：把 WeChat 4.x 的真实表抽象成与版本无关的结构。
//!
//! 列名映射在 `schema.rs` 完成；本模块只描述「逻辑实体」。

use serde::Serialize;

/// 联系人 / 群（contact.db 的 contact 表）。
#[derive(Debug, Clone, Serialize)]
pub struct Contact {
    /// 内部 ID，消息表的 `real_sender_id` 指向它。
    pub id: i64,
    /// wxid（个人）或 `xxx@chatroom`（群）。
    pub username: String,
    pub nickname: Option<String>,
    pub remark: Option<String>,
    pub alias: Option<String>,
    pub is_chatroom: bool,
}

/// 会话（session.db 的 session_last_message 表）→ 定位消息表。
#[derive(Debug, Clone, Serialize)]
pub struct Conversation {
    /// 会话标识：对方 wxid 或群 ID。
    pub username: String,
    /// 消息所在库，如 `message_0`。
    pub db_stem: String,
    /// 对应的 `Msg_<hash>` 表名。
    pub table_name: String,
    /// 消息条数（由 loader 填充）。
    pub msg_count: Option<i64>,
}

/// 单条消息（message_*.db 的 Msg_<hash> 表）。
/// 默认不载入正文内容，仅记录长度——统计骨架足够，且避免把大量隐私文本拉入内存。
#[derive(Debug, Clone, Serialize)]
pub struct Message {
    pub local_id: i64,
    /// 消息类型码（local_type 原始值）。统计时用 `schema::base_type()` 取基础类型：
    /// `local_type = (subtype << 32) | base_type`，如 1=文本、3=图片、49=文件等。
    pub msg_type: i64,
    /// Unix 秒。
    pub create_time: i64,
    /// 发送者内部 ID（real_sender_id）→ 联系人 id。
    pub sender_id: i64,
    /// 正文长度（字符），用于字数统计。
    pub content_len: i64,
}
