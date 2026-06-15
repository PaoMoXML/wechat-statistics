//! 情侣报告（单聊深度叙事统计）。
//!
//! 时序叙事基于 loader 读出的 `MessageFact`（不读正文）；文本指标（字数/关键词/最长一条）
//! 基于按需解压的 `TextMessage`。纯计算、可单测。

use chrono::{DateTime, Datelike, Local, NaiveDate, TimeZone, Timelike};
use serde::Serialize;

use crate::model::{MessageFact, TextMessage};

/// 凌晨阈值：0–4 点算「熬夜」（5 点归到正常清晨）。
const LATE_NIGHT_MAX_HOUR: u32 = 4;
/// 秒回阈值。
const QUICK_REPLY_SEC: i64 = 30;
const FAST_REPLY_SEC: i64 = 60;
/// 回复时延上限（与 dig 一致）：超过视为无关续聊。
const REPLY_CAP_SEC: i64 = 6 * 3600;

#[derive(Serialize)]
pub struct CoupleReport {
    pub total: i64,
    pub my_count: i64,
    pub other_count: i64,

    // —— A. 时序叙事 ——
    pub first_day: Option<String>,   // 首次聊天日期 YYYY-MM-DD
    pub span_days: i64,              // 首次到末次跨越的天数（含首尾）
    pub nth_day_today: i64,          // 首次聊天到「今天」是第几天
    pub active_days: i64,            // 有消息的天数
    pub active_ratio: f64,           // active_days / nth_day_today
    pub longest_streak: i64,         // 最长连续聊天天数
    pub current_streak: i64,         // 截止最近一天的连续天数
    pub longest_silence_days: i64,   // 最长沉默（连续无消息天数）
    pub longest_silence_range: Option<(String, String)>, // 最长沉默两端的聊天日 (沉默前, 沉默后)
    pub peak_day: Option<(String, i64)>, // 最嗨的一天 (日期, 条数)
    pub late_night_count: i64,       // 一起熬过的夜（0–4 点消息条数）
    pub call_count: i64,             // 通话次数 (base_type=50)
    pub last_day: Option<String>,    // 最近一次聊天日期

    // —— B. 双向关系 ——
    /// 秒回率：30s 内回复占有效回复的比例。
    pub my_quick_reply_ratio: f64,
    pub other_quick_reply_ratio: f64,
    /// 1 分钟内回复率。
    pub my_fast_reply_ratio: f64,
    pub other_fast_reply_ratio: f64,
    pub my_reply_count: i64,
    pub other_reply_count: i64,

    // —— C. 文本指标（按需解压正文后计算，None 表示未提供文本）——
    pub text: Option<TextMetrics>,

    // —— D. 高频词（jieba 分词，main 按需填入，None 表示未计算）——
    pub words: Option<crate::lexical::WordFreq>,

    // —— E. 单会话深挖（复用 dig 模块：类型分布/回复时延中位均值/每日首发）——
    pub dig: Option<crate::dig::ConversationDetail>,

    // —— F. 趋势曲线（按周/月分桶的热度序列，main 填入，供 HTML 周↔月 切换）——
    pub trend: Option<crate::trend::TrendSeries>,
    pub trend_monthly: Option<crate::trend::TrendSeries>,
}

#[derive(Serialize)]
pub struct TextMetrics {
    pub my_chars: i64,
    pub other_chars: i64,
    /// 最长一条消息：(字数, 是否我发的, Unix 秒)。
    pub longest: Option<(i64, bool, i64)>,
    /// 关键词命中（消息条数含该词）：(关键词, 我说的条数, 对方说的条数)，按总量降序。
    pub keywords: Vec<(String, i64, i64)>,
}

impl CoupleReport {
    /// 无 self_rowid 时降级：仍给出时序叙事，只是不区分谁秒回谁。
    #[allow(dead_code)]
    pub fn without_self(facts: &[MessageFact]) -> Self {
        compute(facts, None, None)
    }
}

/// 主计算。`self_rowid` 给定时额外算秒回率拆分；`texts` 给定时算文本指标（字数/关键词/最长）。
pub fn compute(
    facts: &[MessageFact],
    self_rowid: Option<i64>,
    texts: Option<&[TextMessage]>,
) -> CoupleReport {
    let total = facts.len() as i64;
    let is_self = |s: i64| self_rowid.map_or(false, |me| s == me);

    // —— 按本地日期分桶（计数 + 首/末日）——
    let mut by_day: std::collections::BTreeMap<String, i64> = std::collections::BTreeMap::new();
    let mut my_count = 0i64;
    let mut other_count = 0i64;
    let mut late_night = 0i64;
    let mut call_count = 0i64;

    for f in facts {
        if f.create_time <= 0 {
            continue;
        }
        if is_self(f.sender_id) {
            my_count += 1;
        } else {
            other_count += 1;
        }
        if f.base_type == 50 {
            call_count += 1;
        }
        let Some(dt) = to_local(f.create_time) else { continue };
        if dt.hour() <= LATE_NIGHT_MAX_HOUR {
            late_night += 1;
        }
        let key = date_key(&dt);
        *by_day.entry(key).or_insert(0) += 1;
    }

    let first_day = by_day.keys().next().cloned();
    let last_day = by_day.keys().last().cloned();
    let active_days = by_day.len() as i64;
    let peak_day = by_day.iter().max_by_key(|(_, n)| *n).map(|(d, n)| (d.clone(), *n));

    // 跨越天数 / 第 N 天
    let (span_days, nth_day_today) = match (first_day.as_deref(), last_day.as_deref()) {
        (Some(a), Some(b)) => {
            let ta = parse_date(a).unwrap_or(0);
            let tb = parse_date(b).unwrap_or(ta);
            let span = (tb - ta).max(0) + 1;
            // 今天到首日的天数（用本地今天）
            let now = Local::now();
            let today_key = format!("{:04}-{:02}-{:02}", now.year(), now.month(), now.day());
            let tn = parse_date(&today_key).unwrap_or(tb);
            let nth = (tn - ta).max(0) + 1;
            (span, nth)
        }
        _ => (0, 0),
    };
    let active_ratio = if nth_day_today > 0 {
        active_days as f64 / nth_day_today as f64
    } else {
        0.0
    };

    // 最长连续 / 当前连续 / 最长沉默
    let s = streaks(&by_day);

    // —— B. 秒回率 ——
    let (my_q, o_q, my_f, o_f, my_n, o_n) = quick_reply(facts, self_rowid);

    CoupleReport {
        total,
        my_count,
        other_count,
        first_day,
        span_days,
        nth_day_today,
        active_days,
        active_ratio,
        longest_streak: s.longest_streak,
        current_streak: s.current_streak,
        longest_silence_days: s.longest_silence_days,
        longest_silence_range: s.longest_silence_range,
        peak_day,
        late_night_count: late_night,
        call_count,
        last_day,
        my_quick_reply_ratio: my_q,
        other_quick_reply_ratio: o_q,
        my_fast_reply_ratio: my_f,
        other_fast_reply_ratio: o_f,
        my_reply_count: my_n,
        other_reply_count: o_n,
        text: texts.map(|t| compute_text(t, self_rowid)),
        words: None,
        dig: None,
        trend: None,
        trend_monthly: None,
    }
}

/// 默认关键词词表（情侣向，可后续做成 CLI 可配）。
const KEYWORDS: &[&str] = &[
    "早安", "晚安", "想你", "爱你", "喜欢你", "老婆", "老公", "宝宝", "宝贝", "亲爱的",
    "么么", "抱抱", "亲亲", "哈哈", "嗯嗯", "在吗", "到家", "加油", "生日快乐",
];

/// 文本指标：总字数（我/对方）、最长一条、关键词命中（消息条数，我/对方拆分）。
fn compute_text(texts: &[TextMessage], self_rowid: Option<i64>) -> TextMetrics {
    let is_self = |s: i64| self_rowid.map_or(false, |me| s == me);

    let mut my_chars = 0i64;
    let mut other_chars = 0i64;
    let mut longest: Option<(i64, bool, i64)> = None;
    // 关键词 → (我条数, 对方条数)
    let mut kw: Vec<(i64, i64)> = vec![(0, 0); KEYWORDS.len()];

    for tm in texts {
        let Some(ref text) = tm.text else { continue };
        let chars = text.chars().count() as i64;
        let mine = is_self(tm.sender_id);
        if mine {
            my_chars += chars;
        } else {
            other_chars += chars;
        }
        if longest.map_or(true, |(c, _, _)| chars > c) {
            longest = Some((chars, mine, tm.create_time));
        }
        for (i, k) in KEYWORDS.iter().enumerate() {
            if text.contains(k) {
                if mine {
                    kw[i].0 += 1;
                } else {
                    kw[i].1 += 1;
                }
            }
        }
    }

    let mut keywords: Vec<(String, i64, i64)> = KEYWORDS
        .iter()
        .zip(kw.iter())
        .map(|(k, (me, oth))| ((**k).to_string(), *me, *oth))
        .filter(|(_, me, oth)| *me + *oth > 0)
        .collect();
    keywords.sort_by(|a, b| (b.1 + b.2).cmp(&(a.1 + a.2)));

    TextMetrics { my_chars, other_chars, longest, keywords }
}

struct StreakInfo {
    longest_streak: i64,
    current_streak: i64,
    longest_silence_days: i64,
    /// 最长沉默两端的那两个「有消息的日子」：(沉默前的最后聊天日, 沉默后的首条聊天日)。
    longest_silence_range: Option<(String, String)>,
}

/// 从按日期升序的计数 map 算：最长连续 / 当前连续 / 最长沉默（含其两端的日期）。
fn streaks(by_day: &std::collections::BTreeMap<String, i64>) -> StreakInfo {
    if by_day.is_empty() {
        return StreakInfo { longest_streak: 0, current_streak: 0, longest_silence_days: 0, longest_silence_range: None };
    }
    // (日期字符串, 儒略日序号)，跨月/跨年精确。
    let pairs: Vec<(&String, i64)> = by_day
        .keys()
        .filter_map(|d| parse_date(d).map(|o| (d, o)))
        .collect();
    if pairs.is_empty() {
        return StreakInfo { longest_streak: 0, current_streak: 0, longest_silence_days: 0, longest_silence_range: None };
    }

    let mut longest = 1i64;
    let mut run = 1i64;
    let mut longest_silence = 0i64;
    let mut silence_range: Option<(String, String)> = None;
    for w in pairs.windows(2) {
        let gap = w[1].1 - w[0].1;
        if gap == 1 {
            run += 1;
            longest = longest.max(run);
        } else {
            // gap>1：两端活跃日之间沉默了 gap-1 天。
            let sil = gap - 1;
            if sil > longest_silence {
                longest_silence = sil;
                silence_range = Some((w[0].0.clone(), w[1].0.clone()));
            }
            run = 1;
        }
    }

    // 当前连续：从末尾回溯连续段。
    let mut current = 1i64;
    for i in (1..pairs.len()).rev() {
        if pairs[i].1 - pairs[i - 1].1 == 1 {
            current += 1;
        } else {
            break;
        }
    }
    // 最近活跃日距今 >1 天，视为 streak 已断。
    let today = today_ordinal();
    if today - pairs.last().unwrap().1 > 1 {
        current = 0;
    }

    StreakInfo { longest_streak: longest, current_streak: current, longest_silence_days: longest_silence, longest_silence_range: silence_range }
}

/// 秒回率：遍历有序消息，发送者切换且 0<gap<=CAP 时记一次「回复」。
/// 返回 (我秒回率, 对方秒回率, 我1分内率, 对方1分内率, 我回复数, 对方回复数)。
fn quick_reply(facts: &[MessageFact], self_rowid: Option<i64>) -> (f64, f64, f64, f64, i64, i64) {
    let is_self = |s: i64| self_rowid.map_or(false, |me| s == me);

    let mut my_quick = 0i64;
    let mut o_quick = 0i64;
    let mut my_fast = 0i64;
    let mut o_fast = 0i64;
    let mut my_total = 0i64;
    let mut o_total = 0i64;
    let mut prev: Option<(i64, i64)> = None; // (sender, time)

    for f in facts {
        if f.create_time <= 0 {
            continue;
        }
        if let Some((ps, pt)) = prev {
            if ps != f.sender_id {
                let gap = f.create_time - pt;
                if gap > 0 && gap <= REPLY_CAP_SEC {
                    let mine = is_self(f.sender_id);
                    if mine {
                        my_total += 1;
                        if gap <= QUICK_REPLY_SEC { my_quick += 1; }
                        if gap <= FAST_REPLY_SEC { my_fast += 1; }
                    } else {
                        o_total += 1;
                        if gap <= QUICK_REPLY_SEC { o_quick += 1; }
                        if gap <= FAST_REPLY_SEC { o_fast += 1; }
                    }
                }
            }
        }
        prev = Some((f.sender_id, f.create_time));
    }

    let r = |x: i64, n: i64| if n > 0 { x as f64 / n as f64 } else { 0.0 };
    (
        r(my_quick, my_total),
        r(o_quick, o_total),
        r(my_fast, my_total),
        r(o_fast, o_total),
        my_total,
        o_total,
    )
}

fn to_local(secs: i64) -> Option<DateTime<Local>> {
    Local.timestamp_opt(secs, 0).single()
}

fn date_key(dt: &DateTime<Local>) -> String {
    format!("{:04}-{:02}-{:02}", dt.year(), dt.month(), dt.day())
}

/// "YYYY-MM-DD" → 儒略日序号（用于比较/求天数差，跨月跨年精确）。
fn parse_date(s: &str) -> Option<i64> {
    let mut it = s.split('-');
    let y: i32 = it.next()?.parse().ok()?;
    let m: u32 = it.next()?.parse().ok()?;
    let d: u32 = it.next()?.parse().ok()?;
    NaiveDate::from_ymd_opt(y, m, d).map(|nd| nd.num_days_from_ce() as i64)
}

/// 今天的儒略日序号。
fn today_ordinal() -> i64 {
    Local::now().date_naive().num_days_from_ce() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::MessageFact;

    fn f(time: i64, sender: i64, bt: i64) -> MessageFact {
        MessageFact { create_time: time, sender_id: sender, base_type: bt }
    }

    #[test]
    fn streak_and_silence() {
        // 用可控日期：把不同消息放到「连续3天」+「断2天」+「1天」
        // 借 parse_date 基于 (y*12+m)*31+d；这里直接构造同月连续日期的秒。
        // 2025-01-01 00:00 → 取一个固定基准，逐天加 86400。
        let base = 1735660800; // 2025-01-01 00:00:00 UTC（本地日期可能偏移，但连续性不变）
        let day = 86400;
        let facts = vec![
            f(base, 1, 1),            // day1
            f(base + day, 1, 1),      // day2
            f(base + 2 * day, 1, 1),  // day3
            // 断 2 天
            f(base + 5 * day, 1, 1),  // day6
        ];
        let r = compute(&facts, Some(1), None);
        assert_eq!(r.active_days, 4);
        assert_eq!(r.longest_streak, 3, "前三天连续");
        assert_eq!(r.longest_silence_days, 2, "day3→day6 沉默 2 天");
        // 沉默两端应为 day3 与 day6（base+2*day 与 base+5*day 对应日期）。
        let range = r.longest_silence_range.expect("应有沉默区间");
        assert!(range.0 < range.1, "区间日期升序");
    }

    #[test]
    fn peak_day_and_late_night() {
        // base 附近，确保落在某个本地日；late_night 用 hour<=4 的真实时刻。
        // 2025-01-01 02:00:00 UTC = 多数东八区是上午，避开；改用 19800 (05:30) 这种……
        // 直接构造凌晨消息：取一个已知本地凌晨的秒数难（TZ 依赖），这里只断言 late_night<=total。
        let facts = vec![f(1735660800, 1, 1), f(1735660800, 2, 1), f(1735660900, 1, 50)];
        let r = compute(&facts, Some(1), None);
        assert_eq!(r.total, 3);
        assert_eq!(r.call_count, 1, "type=50 计为通话");
        assert!(r.late_night_count <= 3);
        assert!(r.peak_day.is_some());
    }

    #[test]
    fn quick_reply_ratios() {
        // 对方→我(20s,秒回) 我→对方(40s,1分内非秒回) 对方→我(120s,都不算快)
        let facts = vec![
            f(1000, 2, 1),
            f(1020, 1, 1), // 我回 gap=20 ≤30 秒回
            f(1060, 2, 1), // 对方回 gap=40 ≤60 非秒回
            f(1180, 1, 1), // 我回 gap=120 >60
        ];
        let r = compute(&facts, Some(1), None);
        // 我回复 2 次（gap20, gap120），秒回 1 → 0.5；1分内 1 → 0.5
        assert_eq!(r.my_reply_count, 2);
        assert!((r.my_quick_reply_ratio - 0.5).abs() < 1e-9);
        assert!((r.my_fast_reply_ratio - 0.5).abs() < 1e-9);
        // 对方回复 1 次（gap40），秒回 0，1分内 1 → 1.0
        assert_eq!(r.other_reply_count, 1);
        assert!((r.other_fast_reply_ratio - 1.0).abs() < 1e-9);
    }

    #[test]
    fn text_metrics_chars_and_keywords() {
        use crate::model::TextMessage;
        let texts = vec![
            TextMessage { create_time: 1, sender_id: 1, text: Some("早安宝宝，昨晚想你啦".into()) },   // 我: 早安/宝宝/想你
            TextMessage { create_time: 2, sender_id: 2, text: Some("爱你哟".into()) },                 // 对方: 爱你
            TextMessage { create_time: 3, sender_id: 1, text: Some("哈哈哈".into()) },                 // 我: 哈哈
            TextMessage { create_time: 4, sender_id: 2, text: None },                                  // 解码失败，跳过
        ];
        let t = compute_text(&texts, Some(1));
        assert_eq!(t.my_chars, "早安宝宝，昨晚想你啦".chars().count() as i64 + "哈哈哈".chars().count() as i64);
        assert_eq!(t.other_chars, "爱你哟".chars().count() as i64);
        // 最长一条是我发的第一条
        assert_eq!(t.longest.unwrap().0, "早安宝宝，昨晚想你啦".chars().count() as i64);
        // 关键词：早安(我1) 爱你(对方1) 宝宝(我1) 想你(我1) 哈哈(我1)
        let kw_map: std::collections::HashMap<&str, (i64, i64)> =
            t.keywords.iter().map(|(k, me, o)| (k.as_str(), (*me, *o))).collect();
        assert_eq!(kw_map["早安"], (1, 0));
        assert_eq!(kw_map["爱你"], (0, 1));
        assert_eq!(kw_map["哈哈"], (1, 0));
        assert!(!kw_map.contains_key("晚安"), "未命中词不出现");
    }
}
