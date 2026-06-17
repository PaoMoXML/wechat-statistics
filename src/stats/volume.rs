//! 全局消息量统计（跨所有会话聚合）。
//!
//! 用纯 SQL 聚合，不把正文读入内存。`local_type` 取低 32 位得到基础类型
//! （见 `schema::base_type`），避免子类型把同类消息拆成无数桶。

use std::collections::HashMap;

use anyhow::Result;
use rusqlite::Connection;
use serde::Serialize;

use crate::model::Conversation;
use crate::schema::{MessageCols, quote_ident};

#[derive(Serialize)]
pub struct VolumeStats {
    pub conversations: usize,
    pub total_messages: i64,
    /// (基础类型码, 条数)，按条数降序。
    pub type_dist: Vec<(i64, i64)>,
    /// (会话 username, 总条数, 我发的条数, 对方发的条数)，按总条数降序，最多 `top_n` 条。
    pub top: Vec<(String, i64, i64, i64)>,
}

/// 在线累加器：遍历每张消息表（含一个会话的多个分片）时喂给它，最后 `finalize`。
pub struct VolumeAccum {
    by_base_type: HashMap<i64, i64>,
    /// username → (total, mine, theirs)。同一会话的多个分片按 username **合并**。
    per_conv: HashMap<String, (i64, i64, i64)>,
    total: i64,
    top_n: usize,
}

impl VolumeAccum {
    pub fn new(top_n: usize) -> Self {
        Self { by_base_type: HashMap::new(), per_conv: HashMap::new(), total: 0, top_n }
    }

    /// `self_rowid` 是该分片所在库的「我的 Name2Id.rowid」（per-DB）。
    /// 一个跨多库分片的会话会触发多次本方法，结果按 username 合并。
    pub fn observe_table(
        &mut self,
        conn: &Connection,
        conv: &Conversation,
        self_rowid: Option<i64>,
    ) -> Result<()> {
        let m = MessageCols::V4;
        let tbl = quote_ident(&conv.table_name);
        let sql = format!(
            "SELECT ({lt} & 4294967295), count(*) FROM {tbl} GROUP BY 1",
            lt = m.local_type
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
        let mut conv_total = 0i64;
        for row in rows {
            let (base, n) = row?;
            *self.by_base_type.entry(base).or_insert(0) += n;
            conv_total += n;
        }

        let (mine, theirs) = match self_rowid {
            Some(sid) => {
                let q = format!(
                    "SELECT SUM({rs}={sid}), SUM({rs}<>{sid}) FROM {tbl}",
                    rs = m.real_sender_id
                );
                let (mine, theirs): (Option<i64>, Option<i64>) =
                    conn.query_row(&q, [], |r| Ok((r.get(0)?, r.get(1)?)))?;
                (mine.unwrap_or(0), theirs.unwrap_or(0))
            }
            None => (0, conv_total),
        };

        self.total += conv_total;
        let e = self.per_conv.entry(conv.username.clone()).or_insert((0, 0, 0));
        e.0 += conv_total;
        e.1 += mine;
        e.2 += theirs;
        Ok(())
    }

    pub fn finalize(self, conversations: usize) -> VolumeStats {
        let mut type_dist: Vec<_> = self.by_base_type.into_iter().collect();
        crate::fmt::sort_by_value_desc(&mut type_dist);
        let mut top: Vec<(String, i64, i64, i64)> = self
            .per_conv
            .into_iter()
            .map(|(u, (t, m, th))| (u, t, m, th))
            .collect();
        top.sort_by(|a, b| b.1.cmp(&a.1));
        top.truncate(self.top_n);
        VolumeStats { conversations, total_messages: self.total, type_dist, top }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Conversation;
    use rusqlite::Connection;

    fn make_msg_table() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE \"Msg_a\" (\
                local_id INTEGER PRIMARY KEY,\
                local_type INTEGER,\
                create_time INTEGER,\
                real_sender_id INTEGER,\
                message_content TEXT)",
        )
        .unwrap();
        conn
    }

    fn conv() -> Conversation {
        Conversation {
            username: "wxid_test".into(),
            db_stems: vec!["message_0".into()],
            table_name: "Msg_a".into(),
            msg_count: None,
        }
    }

    #[test]
    fn groups_by_base_type() {
        let conn = make_msg_table();
        // 混入打包类型： (5<<32)|1 与 (5<<32)|49 应回归到基础类型 1 / 49。
        conn.execute(
            "INSERT INTO \"Msg_a\" (local_type, create_time, real_sender_id, message_content) VALUES \
             (1, 1700000000, 1, 'a'),\
             (21474836481, 1700000001, 1, 'b'),\
             (3, 1700000002, 1, 'c'),\
             (21474836529, 1700000003, 1, 'd'),\
             (49, 1700000004, 1, 'e')",
            [],
        )
        .unwrap();
        // real_sender_id 全是 1 → self_rowid=1 时 mine=5 / theirs=0
        let mut a = VolumeAccum::new(10);
        a.observe_table(&conn, &conv(), Some(1)).unwrap();
        let s = a.finalize(1);
        assert_eq!(s.total_messages, 5);
        let map: HashMap<i64, i64> = s.type_dist.into_iter().collect();
        assert_eq!(map[&1], 2, "纯 1 + 打包 base 1 应合并为 2");
        assert_eq!(map[&3], 1);
        assert_eq!(map[&49], 2, "纯 49 + 打包 base 49 应合并为 2");
        assert_eq!(s.top[0].1, 5);
        assert_eq!(s.top[0].2, 5, "self_id=1 时全部算我发的");
        assert_eq!(s.top[0].3, 0);
    }
}
