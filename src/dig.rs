//! 单会话深挖：回复时延、每日首发、发送比例、类型分布。
//!
//! 这批指标依赖消息**时序**（上一条是谁、日界变化），无法用单一 `GROUP BY` 完成，
//! 所以由 loader 把按 `sort_seq` 排好序的消息事实读出来，本模块做纯计算。
//! 不依赖数据库，方便用构造的有序样本做单测。

use std::collections::BTreeMap;

use chrono::Datelike;
use serde::Serialize;

use crate::model::MessageFact;

/// 回复时延上限：6 小时。超过视为无关续聊而非回复（避免「隔天随便发一条」被算成 10h 回复）。
const REPLY_CAP_SEC: i64 = 6 * 3600;

/// 最近多少天的「每日首发」明细写进报告。
const RECENT_OPENER_DAYS: usize = 14;

/// 单会话深挖结果（可序列化为 JSON）。
#[derive(Serialize)]
pub struct ConversationDetail {
    pub is_group: bool,
    pub total: i64,
    pub time_min: Option<i64>,
    pub time_max: Option<i64>,

    /// 发送比例。
    pub my_count: i64,
    pub other_count: i64,

    /// 基础类型分布，按条数降序。
    pub my_type_dist: Vec<(i64, i64)>,
    pub other_type_dist: Vec<(i64, i64)>,

    /// 回复时延统计（秒）。
    pub my_reply: LatencyStats,
    pub other_reply: LatencyStats,
    /// 超过 6h 上限被丢弃的「跨天续聊」次数（信息项）。
    pub capped_replies: i64,

    /// 每日首发总计：我主动开口 / 对方先开口 的天数。
    pub days_self_open: i64,
    pub days_other_open: i64,
    /// 最近 N 天明细：(本地日期, 是否我开口, 首条 Unix 秒)。按日期升序。
    pub recent_openers: Vec<(String, bool, i64)>,
}

#[derive(Serialize, Default)]
pub struct LatencyStats {
    pub samples: i64,
    pub median_sec: Option<i64>,
    pub mean_sec: Option<i64>,
}

impl ConversationDetail {
    /// 基本信息外的「无 self_id」降级结果：只算总览与每日首发（按「任何人」口径）。
    #[allow(dead_code)]
    pub fn without_self(facts: &[MessageFact]) -> Self {
        let total = facts.len() as i64;
        let (time_min, time_max) = time_span(facts);
        let (days_self, days_other, recent) = daily_openers(facts, None);
        Self {
            is_group: false,
            total,
            time_min,
            time_max,
            my_count: 0,
            other_count: total,
            my_type_dist: Vec::new(),
            other_type_dist: type_dist(facts, |_| false),
            my_reply: LatencyStats::default(),
            other_reply: LatencyStats::default(),
            capped_replies: 0,
            days_self_open: days_self,
            days_other_open: days_other,
            recent_openers: recent,
        }
    }
}

/// 核心计算：给定按时序排好的消息事实 + 自己的 sender_id，产出深挖结果。
pub fn compute(facts: &[MessageFact], self_id: i64, is_group: bool) -> ConversationDetail {
    let total = facts.len() as i64;
    let (time_min, time_max) = time_span(facts);

    // 发送比例 + 类型分布
    let my_count = facts.iter().filter(|f| f.sender_id == self_id).count() as i64;
    let other_count = total - my_count;
    let my_type_dist = type_dist(facts, |s| s == self_id);
    let other_type_dist = type_dist(facts, |s| s != self_id);

    // 回复时延 + 每日首发（一次遍历）
    let (my_replies, other_replies, capped) = reply_latencies(facts, self_id);
    let (days_self_open, days_other_open, recent_openers) = daily_openers(facts, Some(self_id));

    ConversationDetail {
        is_group,
        total,
        time_min,
        time_max,
        my_count,
        other_count,
        my_type_dist,
        other_type_dist,
        my_reply: latency_stats(my_replies),
        other_reply: latency_stats(other_replies),
        capped_replies: capped,
        days_self_open,
        days_other_open,
        recent_openers,
    }
}

fn time_span(facts: &[MessageFact]) -> (Option<i64>, Option<i64>) {
    let valid: Vec<i64> = facts.iter().map(|f| f.create_time).filter(|t| *t > 0).collect();
    if valid.is_empty() {
        (None, None)
    } else {
        (Some(*valid.iter().min().unwrap()), Some(*valid.iter().max().unwrap()))
    }
}

/// 按发送者分桶的基础类型分布，返回排序后的 (base_type, 条数)。
/// `is_bucket` 决定某个 sender 归入哪一侧（self / other）。
fn type_dist<F: Fn(i64) -> bool>(facts: &[MessageFact], is_bucket: F) -> Vec<(i64, i64)> {
    let mut m: BTreeMap<i64, i64> = BTreeMap::new();
    for f in facts {
        if is_bucket(f.sender_id) {
            *m.entry(f.base_type).or_insert(0) += 1;
        }
    }
    let mut v: Vec<_> = m.into_iter().collect();
    crate::fmt::sort_by_value_desc(&mut v);
    v
}

/// 回复时延：遍历有序消息，当发送者切换且间隔在 (0, CAP] 内，记一条「当前发送者」的回复。
/// 返回 (我的时延列表, 对方时延列表, 超上限丢弃数)。
fn reply_latencies(facts: &[MessageFact], self_id: i64) -> (Vec<i64>, Vec<i64>, i64) {
    let mut mine = Vec::new();
    let mut theirs = Vec::new();
    let mut capped = 0i64;
    let mut prev: Option<(i64, i64)> = None; // (sender, time)

    for f in facts {
        if f.create_time <= 0 {
            continue;
        }
        if let Some((prev_sender, prev_time)) = prev {
            if f.sender_id != prev_sender {
                let gap = f.create_time - prev_time;
                if gap > 0 && gap <= REPLY_CAP_SEC {
                    if f.sender_id == self_id {
                        mine.push(gap);
                    } else {
                        theirs.push(gap);
                    }
                } else if gap > REPLY_CAP_SEC {
                    capped += 1;
                }
            }
        }
        prev = Some((f.sender_id, f.create_time));
    }
    (mine, theirs, capped)
}

/// 每日首发：按本地日期分桶，每桶首条的发送者即为「当日开口者」。
/// 返回 (我开口天数, 对方开口天数, 最近 N 天明细)。
/// self_id=None 时（无自己 id）按「任何人」口径：全部计入对方侧。
fn daily_openers(facts: &[MessageFact], self_id: Option<i64>) -> (i64, i64, Vec<(String, bool, i64)>) {
    let mut by_day: BTreeMap<String, (bool, i64)> = BTreeMap::new(); // date -> (is_self_open, first_time)
    for f in facts {
        if f.create_time <= 0 {
            continue;
        }
        let Some(dt) = crate::fmt::local_dt(f.create_time) else { continue };
        let key = format!("{:04}-{:02}-{:02}", dt.year(), dt.month(), dt.day());
        by_day.entry(key).or_insert_with(|| {
            let is_self = crate::model::is_self(self_id, f.sender_id);
            (is_self, f.create_time)
        });
    }
    let mut days_self = 0i64;
    let mut days_other = 0i64;
    for (_, (is_self, _)) in &by_day {
        if *is_self {
            days_self += 1;
        } else {
            days_other += 1;
        }
    }
    let recent: Vec<(String, bool, i64)> = by_day
        .into_iter()
        .rev()
        .take(RECENT_OPENER_DAYS)
        .map(|(d, (is_self, t))| (d, is_self, t))
        .collect();
    (days_self, days_other, recent)
}

fn latency_stats(mut v: Vec<i64>) -> LatencyStats {
    let n = v.len() as i64;
    if v.is_empty() {
        return LatencyStats::default();
    }
    v.sort_unstable();
    let median = v[v.len() / 2];
    let mean = v.iter().sum::<i64>() / n;
    LatencyStats { samples: n, median_sec: Some(median), mean_sec: Some(mean) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::MessageFact;

    fn f(time: i64, sender: i64, bt: i64) -> MessageFact {
        MessageFact { create_time: time, sender_id: sender, base_type: bt }
    }

    #[test]
    fn send_ratio_and_types() {
        // 4 条：我 1 文本 + 1 图片；对方 2 文本。
        let facts = vec![
            f(1000, 1, 1),
            f(2000, 1, 3),
            f(3000, 2, 1),
            f(4000, 2, 1),
        ];
        let d = compute(&facts, 1, false);
        assert_eq!(d.my_count, 2);
        assert_eq!(d.other_count, 2);
        assert_eq!(d.my_type_dist, vec![(1, 1), (3, 1)]);
        assert_eq!(d.other_type_dist, vec![(1, 2)]);
    }

    #[test]
    fn reply_latency_switches_only() {
        // 对方→我(10s,我的回复) 我→对方(20s,对方回复) 我续发(不算) 对方→我(30s,我的回复)
        let facts = vec![
            f(100, 2, 1),
            f(110, 1, 1), // 我回，gap=10
            f(130, 2, 1), // 对方回，gap=20
            f(135, 2, 1), // 对方续发，不算
            f(165, 1, 1), // 我回，gap=30
        ];
        let (mine, theirs, capped) = reply_latencies(&facts, 1);
        assert_eq!(mine, vec![10, 30]);
        assert_eq!(theirs, vec![20]);
        assert_eq!(capped, 0);
    }

    #[test]
    fn reply_cap_drops_long_gaps() {
        // 间隔超过 6h 的切换记为 capped，不进时延（首条时间须 >0 才进入统计）。
        let big = REPLY_CAP_SEC + 1;
        let base = 1_000_000;
        let facts = vec![
            f(base, 2, 1),
            f(base + big, 1, 1), // 我回，gap=big > CAP → capped
            f(base + 2 * big, 2, 1), // 对方回，gap=big > CAP → capped
        ];
        let (mine, theirs, capped) = reply_latencies(&facts, 1);
        assert!(mine.is_empty());
        assert!(theirs.is_empty());
        assert_eq!(capped, 2);
    }

    #[test]
    fn latency_median_and_mean() {
        let s = latency_stats(vec![10, 20, 30, 40]);
        assert_eq!(s.samples, 4);
        assert_eq!(s.median_sec, Some(30)); // v[2]
        assert_eq!(s.mean_sec, Some(25));
    }

    #[test]
    fn daily_openers_count_by_first_sender() {
        // 第一天对方先开口；第二天我先开口；用「日期字符串」直接断言，与 TZ 无关（同日内顺序）。
        let facts = vec![
            f(1700000000, 2, 1), // day A, 对方先
            f(1700001000, 1, 1), //   我后
            f(1700200000, 1, 1), // day B, 我先
            f(1700201000, 2, 1), //   对方后
        ];
        let (self_open, other_open, recent) = daily_openers(&facts, Some(1));
        assert_eq!(self_open, 1);
        assert_eq!(other_open, 1);
        assert_eq!(recent.len(), 2); // 两天
        // recent 倒序（最近在前），且每个 entry 带正确 is_self 标记
        assert!(recent.iter().any(|(d, is_self, _)| *is_self && d.starts_with("20")));
        assert!(recent.iter().any(|(_, is_self, _)| !*is_self));
    }
}
