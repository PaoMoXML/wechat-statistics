//! 解析层：把已解密的微信 4.x 数据目录读成统一模型。
//!
//! 所有读取都走 `schema.rs` 的列名映射并在运行时校验，确保版本兼容。
//! 只读访问，绝不写入用户数据库。

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OpenFlags};

use crate::model::{Contact, Conversation, MessageFact, TextMessage};
use crate::schema::{ContactCols, MessageCols, SessionCols, column_names, quote_ident, verify_columns};

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

    /// 我的 wxid（= 数据目录名）。
    pub fn my_wxid(&self) -> Option<String> {
        self.root.file_name()?.to_str().map(str::to_string)
    }

    /// 扫描所有 message_*.db / biz_message_*.db，建立 `table_name → 所在库列表` 索引。
    /// 一个会话的表（`Msg_{md5(username)}`）可能按时间分片到多个库，这里一次性发现全部分片。
    fn shard_index(&self) -> Result<HashMap<String, Vec<String>>> {
        let mut idx: HashMap<String, Vec<String>> = HashMap::new();
        for entry in std::fs::read_dir(&self.root).with_context(|| format!("读取目录失败: {}", self.root.display()))? {
            let Ok(e) = entry else { continue };
            let path = e.path();
            let Some(stem) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
            else {
                continue;
            };
            // 只关心 message_*.db / biz_message_*.db。
            let is_msg = (stem.starts_with("message_") || stem.starts_with("biz_message_"))
                && path.extension().map_or(false, |x| x.eq_ignore_ascii_case("db"));
            if !is_msg {
                continue;
            }
            let conn = match self.db(&stem) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let mut q = conn.prepare(
                "SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'Msg\\_%' ESCAPE '\\' ",
            )?;
            let names = q.query_map([], |r| r.get::<_, String>(0))?;
            for n in names.flatten() {
                idx.entry(n).or_default().push(stem.clone());
            }
        }
        Ok(idx)
    }

    /// 读取全部会话映射（username → 全部分片库 + 表名）。
    /// 表名 = `Msg_{md5(username)}`，跨分片相同；用 shard_index 找出该表所在的所有库。
    pub fn load_conversations(&self) -> Result<Vec<Conversation>> {
        let s = SessionCols::V4;
        let conn = self.db("session")?;
        verify_columns(&conn, "session_last_message", &s.required())?;
        let idx = self.shard_index()?;

        let sql = format!(
            "SELECT {u}, {d}, {t} FROM session_last_message",
            u = s.username,
            d = s.db_stem,
            t = s.table_name
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], |r| {
            let username: String = r.get(0)?;
            let session_stem: String = r.get(1)?;
            let table_name: String = r.get(2)?;
            let db_stems = idx
                .get(&table_name)
                .cloned()
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| vec![session_stem]);
            Ok(Conversation { username, db_stems, table_name, msg_count: None })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| anyhow::anyhow!("读取会话失败: {e}"))
    }

    /// 批量统计每个会话的消息条数（**跨全部分片求和**）。
    /// 按 db_stem 分组，每个 message_*.db 只打开一次。
    pub fn count_messages_batch(&self, convs: &[Conversation]) -> Result<Vec<i64>> {
        let mut counts = vec![0i64; convs.len()];
        // (stem) -> [(conv_index), ...]
        let mut by_db: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, c) in convs.iter().enumerate() {
            for stem in &c.db_stems {
                by_db.entry(stem.as_str()).or_default().push(i);
            }
        }
        for (stem, idxs) in by_db {
            let conn = match self.db(stem) {
                Ok(c) => c,
                Err(_) => continue, // 某个分片缺失不致命，跳过
            };
            for i in idxs {
                let q = format!("SELECT count(*) FROM {}", quote_ident(&convs[i].table_name));
                let n: i64 = conn.query_row(&q, [], |r| r.get(0)).unwrap_or(0);
                counts[i] += n;
            }
        }
        Ok(counts)
    }

    /// 遍历所有会话的**全部分片**消息表：对每个 (连接, 会话, 该库的我的rowid) 调用一次 `f`。
    /// 一个跨多库分片的会话会触发多次 `f`（每个分片一次），调用方需按 username 合并。
    /// 每个 message_*.db 只打开一次连接，并只解析一次该库的 self_rowid（per-DB）。
    pub fn for_each_conversation<F>(&self, convs: &[Conversation], mut f: F) -> Result<()>
    where
        F: FnMut(&Connection, &Conversation, Option<i64>) -> Result<()>,
    {
        // 过滤掉无分片 / 无表名的退化行。
        let mut by_db: BTreeMap<&str, Vec<&Conversation>> = BTreeMap::new();
        let mut skipped = 0usize;
        for c in convs {
            if c.db_stems.is_empty() || c.table_name.is_empty() {
                skipped += 1;
                continue;
            }
            for stem in &c.db_stems {
                by_db.entry(stem.as_str()).or_default().push(c);
            }
        }
        if skipped > 0 {
            eprintln!("ℹ 跳过 {skipped} 个无法定位消息表的会话");
        }
        let my_wxid = self.my_wxid();
        for (stem, list) in by_db {
            let conn = match self.db(stem) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("⚠ 跳过 {stem}.db（打开失败）: {e}");
                    continue;
                }
            };
            let self_rowid = my_wxid.as_deref().and_then(|w| resolve_self_rowid(&conn, w));
            for c in list {
                if let Err(e) = f(&conn, c, self_rowid) {
                    eprintln!("⚠ 跳过表 {stem}.{}: {e}", c.table_name);
                }
            }
        }
        Ok(())
    }

    /// 对单个会话做聚合统计（**跨全部分片合并**，纯 SQL，不读取正文）。
    pub fn conversation_stats(&self, conv: &Conversation) -> Result<ConvStats> {
        let m = MessageCols::V4;
        let mut count = 0i64;
        let mut senders = 0i64; // 各分片 distinct 之和（粗略上界，调试用）
        let mut tmin: Option<i64> = None;
        let mut tmax: Option<i64> = None;
        let mut td: HashMap<i64, i64> = HashMap::new();

        for stem in &conv.db_stems {
            let conn = match self.db(stem) {
                Ok(c) => c,
                Err(_) => continue,
            };
            if verify_columns(&conn, &conv.table_name, &m.required()).is_err() {
                continue;
            }
            let tbl = quote_ident(&conv.table_name);
            let n: i64 = conn.query_row(&format!("SELECT count(*) FROM {tbl}"), [], |r| r.get(0))?;
            count += n;
            let s: i64 = conn.query_row(
                &format!("SELECT count(DISTINCT {rs}) FROM {tbl}", rs = m.real_sender_id),
                [],
                |r| r.get(0),
            )?;
            senders += s;
            let (a, b): (Option<i64>, Option<i64>) = conn.query_row(
                &format!("SELECT min({ct}), max({ct}) FROM {tbl}", ct = m.create_time),
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            if let Some(a) = a {
                tmin = Some(tmin.map_or(a, |m| m.min(a)));
            }
            if let Some(b) = b {
                tmax = Some(tmax.map_or(b, |m| m.max(b)));
            }
            let mut st = conn.prepare(&format!(
                "SELECT ({lt} & 4294967295), count(*) FROM {tbl} GROUP BY 1",
                lt = m.local_type
            ))?;
            let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
            for r in rows.flatten() {
                *td.entry(r.0).or_insert(0) += r.1;
            }
        }
        let mut type_dist: Vec<(i64, i64)> = td.into_iter().collect();
        type_dist.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        Ok(ConvStats { count, distinct_senders: senders, time_min: tmin, time_max: tmax, type_dist })
    }

    /// 按时序读取单个会话的**全部**消息事实（跨全部分片合并，不含正文）。
    /// `sender_id` 在读取时按各分片的 self_rowid **归一化为 0=我 / 1=对方**
    /// （real_sender_id 是 per-DB 的 Name2Id.rowid，跨分片不可直接比较）。
    /// 跨分片按 create_time 全局排序（分片按时间切分，非重叠）。
    pub fn message_facts(&self, conv: &Conversation) -> Result<Vec<MessageFact>> {
        let m = MessageCols::V4;
        let my_wxid = self.my_wxid();
        let mut all: Vec<MessageFact> = Vec::new();
        for stem in &conv.db_stems {
            let conn = match self.db(stem) {
                Ok(c) => c,
                Err(_) => continue,
            };
            if verify_columns(&conn, &conv.table_name, &m.required()).is_err() {
                continue;
            }
            let tbl = quote_ident(&conv.table_name);
            let cols = column_names(&conn, &conv.table_name)?;
            let order: &str = if cols.iter().any(|c| c.eq_ignore_ascii_case("sort_seq")) {
                "sort_seq"
            } else {
                "create_time, local_id"
            };
            let self_rowid = my_wxid.as_deref().and_then(|w| resolve_self_rowid(&conn, w));
            let sql = format!(
                "SELECT {ct}, {rs}, ({lt} & 4294967295) FROM {tbl} ORDER BY {order}",
                ct = m.create_time,
                rs = m.real_sender_id,
                lt = m.local_type,
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map([], |r| {
                let raw: i64 = r.get(1)?;
                let sender: i64 = match self_rowid {
                    Some(sr) if raw == sr => 0,
                    _ => 1,
                };
                Ok(MessageFact { create_time: r.get(0)?, sender_id: sender, base_type: r.get(2)? })
            })?;
            for rf in rows.flatten() {
                all.push(rf);
            }
        }
        all.sort_by_key(|f| f.create_time);
        Ok(all)
    }

    /// 读取单个会话的**全部**文本消息（跨全部分片合并，base_type=1），按时序返回。
    /// 正文已解压并去群前缀；`sender_id` 同样归一化为 0=我 / 1=对方。
    pub fn text_messages(&self, conv: &Conversation) -> Result<Vec<TextMessage>> {
        let m = MessageCols::V4;
        let my_wxid = self.my_wxid();
        let mut all: Vec<TextMessage> = Vec::new();
        for stem in &conv.db_stems {
            let conn = match self.db(stem) {
                Ok(c) => c,
                Err(_) => continue,
            };
            if verify_columns(&conn, &conv.table_name, &m.required()).is_err() {
                continue;
            }
            let tbl = quote_ident(&conv.table_name);
            let cols = column_names(&conn, &conv.table_name)?;
            let order: &str = if cols.iter().any(|c| c.eq_ignore_ascii_case("sort_seq")) {
                "sort_seq"
            } else {
                "create_time, local_id"
            };
            let has_cc = cols.iter().any(|c| c.eq_ignore_ascii_case("compress_content"));
            let cc_col: &str = if has_cc { "compress_content" } else { "''" };
            let self_rowid = my_wxid.as_deref().and_then(|w| resolve_self_rowid(&conn, w));
            let sql = format!(
                "SELECT {ct}, {rs}, {mc}, {cc} FROM {tbl} WHERE ({lt} & 4294967295) = 1 ORDER BY {order}",
                ct = m.create_time,
                rs = m.real_sender_id,
                mc = m.message_content,
                cc = cc_col,
                lt = m.local_type,
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt.query_map([], |r| {
                let time: i64 = r.get(0)?;
                let raw: i64 = r.get(1)?;
                let sender: i64 = match self_rowid {
                    Some(sr) if raw == sr => 0,
                    _ => 1,
                };
                let mc: &[u8] = match r.get_ref(2)? {
                    rusqlite::types::ValueRef::Text(b) | rusqlite::types::ValueRef::Blob(b) => b,
                    _ => &[],
                };
                let cc: &[u8] = match r.get_ref(3)? {
                    rusqlite::types::ValueRef::Text(b) | rusqlite::types::ValueRef::Blob(b) => b,
                    _ => &[],
                };
                Ok(TextMessage { create_time: time, sender_id: sender, text: crate::content::decode_msg(mc, cc) })
            })?;
            for rm in rows.flatten() {
                all.push(rm);
            }
        }
        all.sort_by_key(|t| t.create_time);
        Ok(all)
    }
}

/// Name2Id 表的用户名列（message_*.db 用 `user_name`，部分 biz 库用 `username`）。
/// Name2Id 表不存在时返回 None。
fn name2id_user_col(conn: &Connection) -> Option<&'static str> {
    let cols = column_names(conn, "Name2Id").ok()?;
    if cols.iter().any(|c| c.eq_ignore_ascii_case("user_name")) {
        Some("user_name")
    } else if cols.iter().any(|c| c.eq_ignore_ascii_case("username")) {
        Some("username")
    } else {
        None
    }
}

/// 在某个 message_*.db 里解析「我自己」的 Name2Id.rowid。
/// `real_sender_id` 是 Name2Id.rowid 外键（不是 contact.id！），所以「我发的」就是
/// `real_sender_id == 我的 rowid`。Name2Id 缺失或未命中返回 None（降级：不区分收发）。
fn resolve_self_rowid(conn: &Connection, my_wxid: &str) -> Option<i64> {
    let col = name2id_user_col(conn)?;
    let sql = format!("SELECT rowid FROM \"Name2Id\" WHERE \"{col}\" = ? LIMIT 1");
    conn.query_row(&sql, [my_wxid], |r| r.get::<_, i64>(0)).ok()
}
