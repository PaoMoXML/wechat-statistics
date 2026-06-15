//! 趋势曲线：把消息时序按周/月分桶，给出热度随时间变化的序列。
//!
//! 用于「热恋期 vs 平稳期」这类叙事：峰值周、升温/平稳/降温判定。
//! 不读正文，仅用 `MessageFact.create_time`。纯计算、可单测。

use std::collections::BTreeMap;

use chrono::{Datelike, Local, NaiveDate, TimeZone};
use serde::Serialize;

use crate::model::MessageFact;

#[derive(Serialize, Clone)]
pub struct TrendPoint {
    /// 桶标签：周用「MM-DD」（周一日期），月用「YYYY-MM」。
    pub label: String,
    pub count: i64,
}

#[derive(Serialize)]
pub struct TrendSeries {
    pub kind: &'static str, // "周" / "月"
    pub points: Vec<TrendPoint>,
    pub total: i64,
    pub peak: Option<TrendPoint>,
    /// 整体走势：升温 / 平稳 / 降温 / 未知。
    pub tag: &'static str,
}

/// 按自然周（周一起）分桶，自动填补沉默周的空窗（计 0）。
pub fn weekly(facts: &[MessageFact]) -> TrendSeries {
    series(facts, Bucket::Week)
}

/// GitHub 风格全年热力图：按自然周分列、周一到周日分行，每格一天的消息量。
#[derive(Serialize)]
pub struct Heatmap {
    /// 列数（周数）。
    pub weeks: usize,
    /// 每格 (日期 "YYYY-MM-DD", 条数)，长度 = weeks*7，顺序为周0的周一..周日、周1…。
    pub cells: Vec<(String, i64)>,
    pub max: i64,
    /// (列号, "M月") 月份标签（该列含某月 1 号时打标）。
    pub month_labels: Vec<(usize, String)>,
    pub active_days: i64,
    pub span_days: i64,
}

pub fn heatmap(facts: &[MessageFact]) -> Heatmap {
    let mut by_day: BTreeMap<NaiveDate, i64> = BTreeMap::new();
    for f in facts {
        if f.create_time <= 0 {
            continue;
        }
        if let Some(dt) = Local.timestamp_opt(f.create_time, 0).single() {
            *by_day.entry(dt.date_naive()).or_insert(0) += 1;
        }
    }
    if by_day.is_empty() {
        return Heatmap { weeks: 0, cells: Vec::new(), max: 0, month_labels: Vec::new(), active_days: 0, span_days: 0 };
    }
    let first = *by_day.keys().next().unwrap();
    let last = *by_day.keys().last().unwrap();
    let active_days = by_day.len() as i64;
    let span_days = (last - first).num_days() + 1;

    // 网格起点 = 首条消息所在周的周一。
    let off = first.weekday().num_days_from_monday() as i64;
    let mut cursor = first - chrono::Duration::days(off);
    let mut cells: Vec<(String, i64)> = Vec::new();
    let mut month_labels: Vec<(usize, String)> = Vec::new();
    let mut weeks = 0usize;
    while cursor <= last {
        let mut new_month = None;
        for d in 0..7 {
            let day = cursor + chrono::Duration::days(d);
            let key = day.format("%Y-%m-%d").to_string();
            let cnt = *by_day.get(&day).unwrap_or(&0);
            cells.push((key, cnt));
            if day.day() == 1 {
                new_month = Some(format!("{}月", day.month()));
            }
        }
        if let Some(m) = new_month {
            month_labels.push((weeks, m));
        }
        weeks += 1;
        cursor += chrono::Duration::days(7);
    }
    let max = by_day.values().copied().max().unwrap_or(0);
    Heatmap { weeks, cells, max, month_labels, active_days, span_days }
}

/// 按自然月分桶。
pub fn monthly(facts: &[MessageFact]) -> TrendSeries {
    series(facts, Bucket::Month)
}

enum Bucket {
    Week,
    Month,
}

fn series(facts: &[MessageFact], b: Bucket) -> TrendSeries {
    let kind = match b {
        Bucket::Week => "周",
        Bucket::Month => "月",
    };
    // ordinal(儒略日序号) → (锚点日期, 计数)。num_days_from_ce 返回 i32，键用 i32。
    let mut m: BTreeMap<i32, (NaiveDate, i64)> = BTreeMap::new();
    for f in facts {
        if f.create_time <= 0 {
            continue;
        }
        let Some(dt) = Local.timestamp_opt(f.create_time, 0).single() else { continue };
        let d = dt.date_naive();
        let (ord, anchor) = match b {
            Bucket::Week => {
                let off = d.weekday().num_days_from_monday() as i64;
                let start = d - chrono::Duration::days(off);
                (start.num_days_from_ce(), start)
            }
            Bucket::Month => {
                let first = NaiveDate::from_ymd_opt(d.year(), d.month(), 1).unwrap_or(d);
                (first.num_days_from_ce(), first)
            }
        };
        let e = m.entry(ord).or_insert((anchor, 0));
        e.1 += 1;
    }
    if m.is_empty() {
        return TrendSeries { kind, points: Vec::new(), total: 0, peak: None, tag: "未知" };
    }

    // 填补空窗：周按 7 天步进精确填补；月按自然月枚举填补。
    match b {
        Bucket::Week => {
            let mut o = *m.keys().next().unwrap();
            let last = *m.keys().last().unwrap();
            while o <= last {
                m.entry(o).or_insert_with(|| {
                    let d = NaiveDate::from_num_days_from_ce_opt(o).unwrap_or_default();
                    (d, 0)
                });
                o += 7;
            }
        }
        Bucket::Month => {
            let start_d = (*m.values().min_by_key(|(d, _)| d.num_days_from_ce()).unwrap()).0;
            let end_d = (*m.values().max_by_key(|(d, _)| d.num_days_from_ce()).unwrap()).0;
            let mut y = start_d.year();
            let mut mon = start_d.month();
            loop {
                if let Some(d) = NaiveDate::from_ymd_opt(y, mon, 1) {
                    if d > end_d {
                        break;
                    }
                    m.entry(d.num_days_from_ce()).or_insert((d, 0));
                }
                mon += 1;
                if mon > 12 {
                    mon = 1;
                    y += 1;
                }
            }
        }
    }

    let mut points: Vec<TrendPoint> = m
        .into_iter()
        .map(|(_, (d, c))| TrendPoint {
            label: match b {
                Bucket::Week => d.format("%m-%d").to_string(),
                Bucket::Month => d.format("%Y-%m").to_string(),
            },
            count: c,
        })
        .collect();
    let total: i64 = points.iter().map(|p| p.count).sum();
    let peak = points.iter().max_by_key(|p| p.count).cloned();
    let tag = trajectory(&points);

    // 极少消息时点太密，裁掉首尾全 0 的周（不影响曲线）。
    points = trim_zero_ends(points);

    TrendSeries { kind, points, total, peak, tag }
}

/// 前/后半段总量对比 → 走势标签。
fn trajectory(points: &[TrendPoint]) -> &'static str {
    if points.len() < 4 {
        return "未知";
    }
    let half = points.len() / 2;
    let first_sum: i64 = points[..half].iter().map(|p| p.count).sum();
    let second_sum: i64 = points[half..].iter().map(|p| p.count).sum();
    if first_sum == 0 {
        return if second_sum > 0 { "升温" } else { "未知" };
    }
    let r = second_sum as f64 / first_sum as f64;
    if r < 0.6 {
        "降温"
    } else if r > 1.4 {
        "升温"
    } else {
        "平稳"
    }
}

/// 去掉首尾连续的 0 桶（沉默期不必拉长横轴）。
fn trim_zero_ends(mut points: Vec<TrendPoint>) -> Vec<TrendPoint> {
    while points.first().map_or(false, |p| p.count == 0) {
        points.remove(0);
    }
    while points.last().map_or(false, |p| p.count == 0) {
        points.pop();
    }
    points
}

/// 终端火花线：把计数序列映射成 Unicode 块字符（0 → '·'）。
pub fn sparkline(counts: &[i64]) -> String {
    const CH: &[char] = &['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let max = *counts.iter().max().unwrap_or(&0);
    if max == 0 {
        return counts.iter().map(|_| '·').collect();
    }
    counts
        .iter()
        .map(|&c| {
            if c == 0 {
                '·'
            } else {
                let idx = (((c as f64 / max as f64) * 7.0).round() as usize).min(7);
                CH[idx]
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::MessageFact;

    fn f(time: i64) -> MessageFact {
        MessageFact { create_time: time, sender_id: 1, base_type: 1 }
    }

    #[test]
    fn weekly_buckets_and_peak() {
        // 用 2024-01-01（周一）起的若干周；每 7 天一批，模拟递增热度。
        // 2024-01-01 00:00 UTC = 多数时区落在周一附近；关键是每批间隔 7 天落在同一自然周内。
        let base = 1704067200; // 2024-01-01 00:00:00 UTC
        let day = 86400;
        let facts = vec![
            f(base),                  // 第1周
            f(base + 7 * day), f(base + 7 * day), // 第2周 2条
            f(base + 14 * day), f(base + 14 * day), f(base + 14 * day), // 第3周 3条（峰值）
        ];
        let s = weekly(&facts);
        assert!(s.points.len() >= 3);
        assert_eq!(s.peak.as_ref().unwrap().count, 3, "峰值应为第3周 3 条");
        assert_eq!(s.total, 6);
    }

    #[test]
    fn trajectory_detects_decline() {
        // 前半多、后半少 → 降温
        let pts: Vec<TrendPoint> = (0..10)
            .map(|i| TrendPoint { label: format!("w{i}"), count: if i < 5 { 10 } else { 1 } })
            .collect();
        assert_eq!(trajectory(&pts), "降温");
    }

    #[test]
    fn sparkline_scales_to_blocks() {
        let s = sparkline(&[0, 1, 5, 10]);
        assert!(s.starts_with('·'), "0 → 点");
        assert!(s.ends_with('█'), "最大值 → 满块");
        assert_eq!(s.chars().count(), 4);
    }
}
