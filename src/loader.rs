//! 解析层：把已解密的微信 4.x 数据目录读成统一模型。
//!
//! 所有读取都走 `schema.rs` 的列名映射并在运行时校验，确保版本兼容。
//! 只读访问，绝不写入用户数据库。

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OpenFlags};

use crate::model::{Contact, Conversation};
use crate::schema::{ContactCols, MessageCols, SessionCols, quote_ident, verify_columns};

/// 一个已解密的微信 4.x 数据目录（含 contact.db / session.db / message_*.db …）。
pub struct WeChatData {
    pub root: PathBuf,
}

/// 单个会话的聚合统计（纯 SQL 聚合，不读取正文内容）。
pub struct ConvStats {
    pub count: i64,
    pub distinct_senders: i64,
    pub time_min: Option<i64>,
    pub time_max: Option<i64>,
    /// (local_type, 条数)。
    pub type_dist: Vec<(i64, i64)>,
}

impl WeChatData {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        if !root.join("contact.db").exists() {
            bail!("未找到 contact.db，路径不像解密后的微信 4.x 数据目录: {}", root.display());
        }
        Ok(Self { root })
    }

    /// 以只读方式打开 `<root>/<stem>.db`。
    fn db(&self, stem: &str) -> Result<Connection> {
        let path = self.root.join(format!("{stem}.db"));
        Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .with_context(|| format!("打开 {} 失败", path.display()))
    }

    /// 读取全部联系人 / 群。
    pub fn load_contacts(&self) -> Result<Vec<Contact>> {
        let c = ContactCols::V4;
        let conn = self.db("contact")?;
        verify_columns(&conn, "contact", &c.required())?;
        let sql = format!(
            "SELECT {id}, {u}, {n}, {r}, {a}, {cr} FROM contact",
            id = c.id,
            u = c.username,
            n = c.nick_name,
            r = c.remark,
            a = c.alias,
            cr = c.is_in_chat_room
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], |r| {
            let room: i64 = r.get(5)?;
            Ok(Contact {
                id: r.get(0)?,
                username: r.get(1)?,
                nickname: r.get(2)?,
                remark: r.get(3)?,
                alias: r.get(4)?,
                is_chatroom: room != 0,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| anyhow::anyhow!("读取联系人失败: {e}"))
    }

    /// 读取全部会话映射（username → db_stem + table_name）。
    pub fn load_conversations(&self) -> Result<Vec<Conversation>> {
        let s = SessionCols::V4;
        let conn = self.db("session")?;
        verify_columns(&conn, "session_last_message", &s.required())?;
        let sql = format!(
            "SELECT {u}, {d}, {t} FROM session_last_message",
            u = s.username,
            d = s.db_stem,
            t = s.table_name
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], |r| {
            Ok(Conversation {
                username: r.get(0)?,
                db_stem: r.get(1)?,
                table_name: r.get(2)?,
                msg_count: None,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| anyhow::anyhow!("读取会话失败: {e}"))
    }

    /// 批量统计每个会话的消息条数。
    /// 按 db_stem 分组，每个 message_*.db 只打开一次，避免反复开关连接。
    pub fn count_messages_batch(&self, convs: &[Conversation]) -> Result<Vec<i64>> {
        let mut counts = vec![0i64; convs.len()];
        let mut by_db: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, c) in convs.iter().enumerate() {
            by_db.entry(c.db_stem.as_str()).or_default().push(i);
        }
        for (stem, idxs) in by_db {
            let conn = match self.db(stem) {
                Ok(c) => c,
                Err(_) => continue, // 某个分片缺失不致命，跳过
            };
            for i in idxs {
                let q = format!("SELECT count(*) FROM {}", quote_ident(&convs[i].table_name));
                let n: i64 = conn.query_row(&q, [], |r| r.get(0)).unwrap_or(0);
                counts[i] = n;
            }
        }
        Ok(counts)
    }

    /// 按 db_stem 分组遍历所有会话的消息表，对每个 (连接, 会话) 调用一次 `f`。
    /// 每个 message_*.db 只打开一次连接；BTreeMap 保证遍历顺序稳定（便于复现/日志）。
    /// 单个表出错只打印警告并跳过，不中断整体统计——适合跨数千张表的聚合。
    pub fn for_each_conversation<F>(&self, convs: &[Conversation], mut f: F) -> Result<()>
    where
        F: FnMut(&Connection, &Conversation) -> Result<()>,
    {
        // 过滤掉 db_stem / table_name 为空的退化行（真实 session 表里偶有出现），
        // 它们无法定位消息表，干净跳过并汇总提示，而不是去打开 `.db` 制造噪音。
        let mut by_db: BTreeMap<&str, Vec<&Conversation>> = BTreeMap::new();
        let mut skipped = 0usize;
        for c in convs {
            if c.db_stem.is_empty() || c.table_name.is_empty() {
                skipped += 1;
                continue;
            }
            by_db.entry(c.db_stem.as_str()).or_default().push(c);
        }
        if skipped > 0 {
            eprintln!("ℹ 跳过 {skipped} 个无法定位消息表的会话（db_stem/table_name 为空）");
        }
        for (stem, list) in by_db {
            let conn = match self.db(stem) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("⚠ 跳过 {stem}.db（打开失败）: {e}");
                    continue;
                }
            };
            for c in list {
                if let Err(e) = f(&conn, c) {
                    eprintln!("⚠ 跳过表 {stem}.{}: {e}", c.table_name);
                }
            }
        }
        Ok(())
    }

    /// 对单个会话做聚合统计（纯 SQL，不读取正文内容）。
    pub fn conversation_stats(&self, conv: &Conversation) -> Result<ConvStats> {
        let conn = self.db(&conv.db_stem)?;
        let m = MessageCols::V4;
        verify_columns(&conn, &conv.table_name, &m.required())?;
        let tbl = quote_ident(&conv.table_name);

        let count: i64 = conn.query_row(&format!("SELECT count(*) FROM {tbl}"), [], |r| r.get(0))?;
        let senders: i64 = conn.query_row(
            &format!("SELECT count(DISTINCT {rs}) FROM {tbl}", rs = m.real_sender_id),
            [],
            |r| r.get(0),
        )?;
        let (time_min, time_max): (Option<i64>, Option<i64>) = conn.query_row(
            &format!("SELECT min({ct}), max({ct}) FROM {tbl}", ct = m.create_time),
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let mut stmt = conn.prepare(&format!(
            "SELECT {lt}, count(*) FROM {tbl} GROUP BY {lt}",
            lt = m.local_type
        ))?;
        let type_dist: Vec<(i64, i64)> = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?
            .filter_map(|x| x.ok())
            .collect();

        Ok(ConvStats { count, distinct_senders: senders, time_min, time_max, type_dist })
    }
}
