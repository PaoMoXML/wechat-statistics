//! WeChat 4.x schema 适配层。
//!
//! 列名来自 Phase 0 对真实库的探测（见 方案.md / probe.json），不是凭空猜测。
//! 运行时仍会校验表是否真的拥有这些列——若微信后续版本改了列名，
//! 这里会给出明确报错而非静默读错数据。
//!
//! 探测已确认的关键映射：
//! - 消息：`message_*.db` / `biz_message_*.db` 的 `Msg_<hash>` 表
//! - 联系人：`contact.db` 的 `contact` 表（id ↔ username/nick_name）
//! - 会话定位：`session.db` 的 `session_last_message` 表（username → db_stem + table_name）

use anyhow::{Result, bail};
use rusqlite::Connection;

/// 消息表的逻辑字段 → 实际列名。
pub struct MessageCols {
    pub local_id: &'static str,
    pub local_type: &'static str,
    pub create_time: &'static str,
    /// ⚠ 不是 contact.id！是「Name2Id」表的隐式 rowid 外键（每个 message_*.db 各有自己的 Name2Id，
    /// rowid 跨库不同）。取真实 wxid 要 `LEFT JOIN Name2Id n ON real_sender_id = n.rowid`，
    /// 取 n.user_name。判定「我发的」：该库的 self_rowid（见 loader::self_rowid_for）== real_sender_id。
    pub real_sender_id: &'static str,
    pub message_content: &'static str,
}

impl MessageCols {
    /// WeChat 4.x（已对真实库确认）。
    pub const V4: Self = Self {
        local_id: "local_id",
        local_type: "local_type",
        create_time: "create_time",
        real_sender_id: "real_sender_id",
        message_content: "message_content",
    };

    pub fn required(&self) -> Vec<&'static str> {
        vec![
            self.local_id,
            self.local_type,
            self.create_time,
            self.real_sender_id,
            self.message_content,
        ]
    }
}

/// 联系人表的逻辑字段 → 实际列名。
pub struct ContactCols {
    pub id: &'static str,
    pub username: &'static str,
    pub nick_name: &'static str,
    pub remark: &'static str,
    pub alias: &'static str,
    pub is_in_chat_room: &'static str,
}

impl ContactCols {
    pub const V4: Self = Self {
        id: "id",
        username: "username",
        nick_name: "nick_name",
        remark: "remark",
        alias: "alias",
        is_in_chat_room: "is_in_chat_room",
    };

    pub fn required(&self) -> Vec<&'static str> {
        vec![self.id, self.username, self.nick_name, self.remark, self.alias, self.is_in_chat_room]
    }
}

/// 会话映射表的逻辑字段 → 实际列名。
pub struct SessionCols {
    pub username: &'static str,
    pub db_stem: &'static str,
    pub table_name: &'static str,
}

impl SessionCols {
    pub const V4: Self = Self {
        username: "username",
        db_stem: "db_stem",
        table_name: "table_name",
    };

    pub fn required(&self) -> Vec<&'static str> {
        vec![self.username, self.db_stem, self.table_name]
    }
}

/// 给标识符加双引号并转义，避免奇怪表名/列名导致 SQL 解析失败。
pub fn quote_ident(name: &str) -> String {
    let escaped = name.replace('"', "\"\"");
    format!("\"{escaped}\"")
}

/// 返回某张表的全部列名。
pub fn column_names(conn: &Connection, table: &str) -> Result<Vec<String>> {
    let q = format!("PRAGMA table_info({})", quote_ident(table));
    let mut stmt = conn.prepare(&q)?;
    let names = stmt
        .query_map([], |r| r.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(names)
}

/// 校验表是否拥有全部期望列；缺失则报错并列出实际列（大小写不敏感）。
pub fn verify_columns(conn: &Connection, table: &str, required: &[&str]) -> Result<()> {
    let actual = column_names(conn, table)?;
    let lower: Vec<String> = actual.iter().map(|s| s.to_ascii_lowercase()).collect();
    let missing: Vec<&str> = required
        .iter()
        .filter(|c| !lower.iter().any(|a| a == &c.to_ascii_lowercase()))
        .copied()
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    bail!(
        "表 {table} 缺少期望列 {missing:?}；实际列为 {actual:?}。\n\
         这通常意味着微信版本与本工具的 schema 适配层不兼容，\n\
         请用 `schema` 子命令重新探测并更新 src/schema.rs。"
    );
}

/// 解析 `local_type` 的基础消息类型。
///
/// WeChat 4.x 的 `local_type` 是打包值：`(subtype << 32) | base_type`。
/// 例如 `21474836529 = (5 << 32) | 49` → 基础类型 49（文件/应用消息）。
/// 统计媒体/类型分布时必须取低 32 位，否则同一类消息会被拆成无数个 subtype 变体。
/// 常见基础类型码（待 Phase 1 用真实样本进一步核对）：
///   1=文本, 3=图片, 34=语音, 42=名片, 43=视频, 47=表情贴纸,
///   48=位置, 49=文件/应用消息, 50=音视频通话, 10000=系统消息。
///
/// 注：统计引擎目前在 SQL 侧用 `& 4294967295` 直接掩码（更高效），
/// 本函数作为单条消息解析时的工具保留。
#[allow(dead_code)]
pub fn base_type(local_type: i64) -> i64 {
    local_type & 0xFFFF_FFFF
}

/// 解析 `local_type` 的高位子类型。
#[allow(dead_code)]
pub fn sub_type(local_type: i64) -> i64 {
    (local_type >> 32) & 0xFFFF_FFFF
}

/// 基础消息类型码 → 中文标签（与 `base_type` 同源，是全项目唯一的类型名来源）。
///
/// 用于终端报告与 HTML 渲染统一显示，避免多份拷贝文案渐渐走样。
///   1=文本, 3=图片, 34=语音, 42=名片, 43=视频, 47=表情,
///   48=位置, 49=文件, 50=通话, 10000=系统, 其它=其他。
pub fn type_label(t: i64) -> &'static str {
    match t {
        1 => "文本",
        3 => "图片",
        34 => "语音",
        42 => "名片",
        43 => "视频",
        47 => "表情",
        48 => "位置",
        49 => "文件",
        50 => "通话",
        10000 => "系统",
        _ => "其他",
    }
}
