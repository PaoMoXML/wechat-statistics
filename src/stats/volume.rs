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
    /// (会话 username, 条数)，按条数降序，最多 `top_n` 条。
    pub top: Vec<(String, i64)>,
}

/// 在线累加器：遍历每张消息表时喂给它，最后 `finalize` 成统计结果。
pub struct VolumeAccum {
    by_base_type: HashMap<i64, i64>,
    per_conv: Vec<(String, i64)>,
    total: i64,
    top_n: usize,
}

impl VolumeAccum {
    pub fn new(top_n: usize) -> Self {
        Self { by_base_type: HashMap::new(), per_conv: Vec::new(), total: 0, top_n }
    }

    pub fn observe_table(&mut self, conn: &Connection, conv: &Conversation) -> Result<()> {
        let m = MessageCols::V4;
        let tbl = quote_ident(&conv.table_name);
        // 基础类型 = local_type & 0xFFFFFFFF。一次 GROUP BY 同时拿到类型分布与会话总数。
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
        self.total += conv_total;
        self.per_conv.push((conv.username.clone(), conv_total));
        Ok(())
    }

    pub fn finalize(self, conversations: usize) -> VolumeStats {
        let mut type_dist: Vec<_> = self.by_base_type.into_iter().collect();
        // 条数降序；条数相同按类型码升序，输出稳定。
        type_dist.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let mut top = self.per_conv;
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
            db_stem: "message_0".into(),
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
        let mut a = VolumeAccum::new(10);
        a.observe_table(&conn, &conv()).unwrap();
        let s = a.finalize(1);
        assert_eq!(s.total_messages, 5);
        let map: HashMap<i64, i64> = s.type_dist.into_iter().collect();
        assert_eq!(map[&1], 2, "纯 1 + 打包 base 1 应合并为 2");
        assert_eq!(map[&3], 1);
        assert_eq!(map[&49], 2, "纯 49 + 打包 base 49 应合并为 2");
        assert_eq!(s.top[0].1, 5);
    }
}
