//! 时段分布统计：小时 / 星期 / 月份。
//!
//! 全部用 SQLite `strftime` + `'unixepoch','localtime'` 在库内聚合，
//! 不把时间戳读出来再算。一次 GROUP BY 拿到 (小时, 星期, 月份) 联合分布，
//! 在 Rust 侧累加成边缘分布。

use std::collections::BTreeMap;

use anyhow::Result;
use rusqlite::Connection;
use serde::Serialize;

use crate::model::Conversation;
use crate::schema::{MessageCols, quote_ident};

#[derive(Serialize)]
pub struct TemporalStats {
    /// 0..23 各小时消息数（本地时间）。
    pub hour: [i64; 24],
    /// 周一..周日 各天消息数（索引 0=周一，6=周日）。
    pub weekday: [i64; 7],
    /// (YYYYMM, 条数)，按月份升序。
    pub month: Vec<(String, i64)>,
}

pub struct TemporalAccum {
    hour: [i64; 24],
    weekday: [i64; 7],
    month: BTreeMap<String, i64>,
}

impl TemporalAccum {
    pub fn new() -> Self {
        Self { hour: [0; 24], weekday: [0; 7], month: BTreeMap::new() }
    }

    pub fn observe_table(&mut self, conn: &Connection, conv: &Conversation) -> Result<()> {
        let m = MessageCols::V4;
        let tbl = quote_ident(&conv.table_name);
        // 一次扫描拿联合分布；WHERE 过滤掉 0 / 异常时间戳。
        let sql = format!(
            "SELECT \
                cast(strftime('%H', {ct}, 'unixepoch', 'localtime') AS integer), \
                cast(strftime('%w', {ct}, 'unixepoch', 'localtime') AS integer), \
                strftime('%Y%m', {ct}, 'unixepoch', 'localtime'), \
                count(*) \
             FROM {tbl} WHERE {ct} > 0 GROUP BY 1, 2, 3",
            ct = m.create_time
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?, // hour
                r.get::<_, i64>(1)?, // weekday (SQLite %w: 0=周日..6=周六)
                r.get::<_, String>(2)?, // YYYYMM
                r.get::<_, i64>(3)?, // count
            ))
        })?;
        for row in rows {
            let (h, w, ym, n) = row?;
            if (0..24).contains(&h) {
                self.hour[h as usize] += n;
            }
            self.weekday[weekday_sql_to_idx(w)] += n;
            *self.month.entry(ym).or_insert(0) += n;
        }
        Ok(())
    }

    pub fn finalize(self) -> TemporalStats {
        TemporalStats {
            hour: self.hour,
            weekday: self.weekday,
            month: self.month.into_iter().collect(),
        }
    }
}

/// SQLite `%w`（0=周日..6=周六）→ 周一为起点的索引（0=周一..6=周日）。
/// 异常值兜底归到周日桶，避免越界。
fn weekday_sql_to_idx(w: i64) -> usize {
    match w {
        1 => 0, // 周一
        2 => 1,
        3 => 2,
        4 => 3,
        5 => 4,
        6 => 5,
        _ => 6, // 周日（0）及其它异常值
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Conversation;
    use rusqlite::Connection;

    #[test]
    fn weekday_mapping_is_monday_first() {
        assert_eq!(weekday_sql_to_idx(1), 0, "周一=0");
        assert_eq!(weekday_sql_to_idx(6), 5, "周六=5");
        assert_eq!(weekday_sql_to_idx(0), 6, "周日=6");
        assert_eq!(weekday_sql_to_idx(9), 6, "异常值兜底归周日");
    }

    #[test]
    fn sums_match_filtered_row_count() {
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
        // 4 行，其中 1 行 create_time=0 应被 WHERE 过滤掉。
        conn.execute(
            "INSERT INTO \"Msg_a\" (local_type, create_time, real_sender_id) VALUES \
             (1, 1700000000, 1), (1, 1700003600, 1), (1, 1700090000, 1), (1, 0, 1)",
            [],
        )
        .unwrap();
        let conv = Conversation {
            username: "x".into(),
            db_stems: vec!["m".into()],
            table_name: "Msg_a".into(),
            msg_count: None,
        };
        let mut a = TemporalAccum::new();
        a.observe_table(&conn, &conv).unwrap();
        let s = a.finalize();
        // 三个边缘分布的总和都应等于被计入的行数 3（与时区无关，求和可交换）。
        assert_eq!(s.hour.iter().sum::<i64>(), 3);
        assert_eq!(s.weekday.iter().sum::<i64>(), 3);
        assert_eq!(s.month.iter().map(|(_, n)| n).sum::<i64>(), 3);
    }
}
