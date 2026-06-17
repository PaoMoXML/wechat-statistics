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

impl Contact {
    /// 显示名：优先备注 → 昵称 → username（跳过空串）。
    /// 用于把脱敏 wxid 换成可读名称。
    pub fn display_name(&self) -> &str {
        self.remark
            .as_deref()
            .filter(|s| !s.is_empty())
            .or_else(|| self.nickname.as_deref().filter(|s| !s.is_empty()))
            .unwrap_or(&self.username)
    }
}

/// 会话（session.db 的 session_last_message 表）→ 定位消息表。
#[derive(Debug, Clone, Serialize)]
pub struct Conversation {
    /// 会话标识：对方 wxid 或群 ID。
    pub username: String,
    /// 该会话消息所在的所有库（一个会话可能被**按时间分片**到多个 message_*.db / biz_message_*.db，
    /// 表名同为 `Msg_{md5(username)}`）。读取时必须遍历全部分片并合并。
    pub db_stems: Vec<String>,
    /// 对应的 `Msg_<hash>` 表名（= `Msg_{md5(username)}`，跨分片相同）。
    pub table_name: String,
    /// 消息条数（由 loader 填充）。
    pub msg_count: Option<i64>,
}

/// 单条消息（message_*.db 的 Msg_<hash> 表）。
/// 默认不载入正文内容，仅记录长度——统计骨架足够，且避免把大量隐私文本拉入内存。
/// （Phase 2 按需读取单条消息时启用。）
#[derive(Debug, Clone, Serialize)]
#[allow(dead_code)]
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

/// 时序事实：单条消息用于时序分析的最小信息（不含正文）。
/// 由 loader 按 `sort_seq` 顺序读出，供 dig 模块做回复时延 / 每日首发等序列计算。
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct MessageFact {
    /// Unix 秒。
    pub create_time: i64,
    /// 发送者内部 ID（= Name2Id.rowid；自己的消息为该库的 self_rowid）。
    pub sender_id: i64,
    /// 基础类型码（local_type 已取低 32 位）。
    pub base_type: i64,
}

/// 一条文本消息（仅 base_type=1，已解压、已去群前缀），用于字数 / 关键词 / 词频分析。
#[derive(Debug, Clone, Serialize)]
pub struct TextMessage {
    /// Unix 秒。
    pub create_time: i64,
    /// 发送者 Name2Id.rowid（用于区分「我/对方」）。
    pub sender_id: i64,
    /// 解压后的正文（解码失败为 None）。
    pub text: Option<String>,
}

/// 判定某条消息是否为「我」发的（全项目共享的发送者归一化口径）。
///
/// `self_id` 是归一化后的「我的」发送者 id（loader 已统一为 0=我 / 1=对方，
/// 或各分片的 Name2Id.rowid）；`sender` 是消息的发送者 id。
/// `self_id=None` 时一律视为非我（降级：不区分收发）。
pub fn is_self(self_id: Option<i64>, sender: i64) -> bool {
    self_id.is_some_and(|me| me == sender)
}
