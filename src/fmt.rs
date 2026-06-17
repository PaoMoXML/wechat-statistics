//! 共享的展示与通用小工具：数字 / 时长 / 时间格式化、文本柱状图、截断、百分比、通用排序。
//!
//! 把原本散落在 main.rs / render.rs / dig.rs / couple.rs / curios.rs / probe.rs 的重复实现集中于此，
//! 各模块统一调用这里，避免多份拷贝渐渐走样。

use chrono::{DateTime, Local, TimeZone};

/// 千分位格式化：`1234567` → `"1,234,567"`（负数带负号）。
pub fn fmt_num(n: i64) -> String {
    let s = n.unsigned_abs().to_string();
    let bytes = s.as_bytes();
    let mut out = String::new();
    if n < 0 {
        out.push('-');
    }
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

/// 秒数 → 「N 时 M 分」「M 分 S 秒」「S 秒」可读时长。
pub fn fmt_duration(secs: i64) -> String {
    if secs < 60 {
        return format!("{secs} 秒");
    }
    if secs < 3600 {
        return format!("{} 分 {}", secs / 60, secs % 60);
    }
    format!("{} 时 {} 分", secs / 3600, (secs % 3600) / 60)
}

/// Unix 秒 → 本地可读时间 `YYYY-MM-DD HH:MM:SS`；无法解析时回退为原值标注。
pub fn fmt_ts(secs: i64) -> String {
    match local_dt(secs) {
        Some(dt) => dt.format("%Y-%m-%d %H:%M:%S").to_string(),
        None => format!("{secs}(无法解析)"),
    }
}

/// Unix 秒 → 本地日期 `YYYY-MM-DD`；无法解析时返回空串。
pub fn fmt_date(secs: i64) -> String {
    local_dt(secs)
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

/// Unix 秒 → 本地 `DateTime`；非法时间戳返回 `None`（各模块共享的 `to_local`）。
pub fn local_dt(secs: i64) -> Option<DateTime<Local>> {
    Local.timestamp_opt(secs, 0).single()
}

/// 按最大值等比缩放的文本柱状图（`▇` 块字符）。`max <= 0` 时返回空串。
pub fn bar(value: i64, max: i64, width: usize) -> String {
    if max <= 0 {
        return String::new();
    }
    let scaled = (value as f64 / max as f64 * width as f64).round() as usize;
    "▇".repeat(scaled.min(width))
}

/// 按字符宽度截断并加省略号：字符数 ≤ `max_chars` 原样返回，否则取前 `max_chars` 个字符加「…」。
pub fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let end = s
        .char_indices()
        .nth(max_chars)
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    format!("{}…", &s[..end])
}

/// 比率 `part / whole`（`0.0..1.0`）；`whole <= 0` 时返回 `0.0`。
pub fn ratio(part: i64, whole: i64) -> f64 {
    if whole > 0 {
        part as f64 / whole as f64
    } else {
        0.0
    }
}

/// 百分比 `part / whole * 100`（`0.0..100.0`）；`whole <= 0` 时返回 `0.0`。
pub fn pct(part: i64, whole: i64) -> f64 {
    ratio(part, whole) * 100.0
}

/// 把 `(键, 值)` 列表按 **值降序、键升序** 原地排序。
/// 用于「(类型码 / 词, 计数) 按计数降序、相同计数按码升序」这类高频分布整理。
pub fn sort_by_value_desc<K: Ord, V: Ord>(v: &mut [(K, V)]) {
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
}
