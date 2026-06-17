//! 趣味统计（叙事彩蛋 / 节奏对比 / 词趣）。
//!
//! 全部基于已读出的 `MessageFact`(时序) + `TextMessage`(解压正文) + self_rowid，
//! 不依赖数据库、不依赖 couple（避免循环引用）。纯计算、可单测。

use chrono::Timelike;
use serde::Serialize;

use crate::model::{MessageFact, TextMessage};

/// 关系温度计所需的标量（由 CoupleReport 提供）。
pub struct ScoreInputs {
    pub total: i64,
    pub my_count: i64,
    pub other_count: i64,
    pub longest_streak: i64,
    pub active_ratio: f64,
    pub my_quick: f64,
    pub other_quick: f64,
}

#[derive(Serialize)]
pub struct Curios {
    pub firsts: Vec<First>,
    pub score: RelationScore,
    pub emo: LateEmo,
    pub biorhythm: Biorhythm,
    pub ending: Ending,
    pub rally: Rally,
    pub signature: SignatureWords,
    pub haha: Haha,
    pub perfunctory: Perfunctory,
    pub heatmap: crate::trend::Heatmap,
}

#[derive(Serialize)]
pub struct First {
    pub label: String,
    pub when: Option<String>,
    pub who: Option<&'static str>, // "我" / "ta" / None
    pub snippet: Option<String>,
}

#[derive(Serialize)]
pub struct RelationScore {
    pub score: i64,
    pub label: &'static str,
    /// (项, 得分, 满分)。
    pub breakdown: Vec<(String, i64, i64)>,
}

#[derive(Serialize)]
pub struct LateEmo {
    pub my_count: i64,
    pub other_count: i64,
    /// 最长的一条深夜长文。
    pub longest: Option<EmoMsg>,
}

#[derive(Serialize)]
pub struct EmoMsg {
    pub chars: i64,
    pub is_self: bool,
    pub when: String,
    pub snippet: String,
}

#[derive(Serialize)]
pub struct Biorhythm {
    pub me: [i64; 24],
    pub them: [i64; 24],
}

#[derive(Serialize)]
pub struct Ending {
    /// 「最后一句」归属（>12h 无后续或会话结尾）：各方说了最后一句的次数。
    pub my_last_word: i64,
    pub other_last_word: i64,
    /// 「被晾 >6h」：我/ta 发完后对方 6h+ 才回（或没回）的次数。
    pub my_left_on_read: i64,
    pub other_left_on_read: i64,
}

#[derive(Serialize)]
pub struct Rally {
    pub max_len: i64,
    pub when: Option<String>,
    pub duration_min: i64,
}

#[derive(Serialize)]
pub struct SignatureWords {
    pub mine: Vec<(String, i64)>,
    pub theirs: Vec<(String, i64)>,
}

#[derive(Serialize)]
pub struct Haha {
    /// 按 哈的叠字长度分桶：[2,3,4,5,6+] 各自计数。
    pub me: [i64; 5],
    pub them: [i64; 5],
    pub me_total: i64,
    pub them_total: i64,
}

#[derive(Serialize)]
pub struct Perfunctory {
    pub my_count: i64,
    pub other_count: i64,
}

const LAST_WORD_GAP: i64 = 12 * 3600; // >12h 视为一段对话结束
const LEFT_ON_READ_GAP: i64 = 6 * 3600; // >6h 未回 = 被晾
const RALLY_GAP: i64 = 10 * 60; // 连续对线：相邻间隔 <=10min
const EMO_MIN_CHARS: usize = 50;
const LOVE_WORDS: &[&str] = &["爱你", "喜欢你", "想你", "想你啦", "我爱你"];

/// 主入口。
pub fn compute(facts: &[MessageFact], texts: &[TextMessage], self_rowid: Option<i64>, sc: &ScoreInputs) -> Curios {
    Curios {
        firsts: firsts(facts, texts, self_rowid),
        score: relation_score(sc),
        emo: late_emo(texts, self_rowid),
        biorhythm: biorhythm(facts, self_rowid),
        ending: ending(facts, self_rowid),
        rally: rally(facts),
        signature: crate::lexical::signature_words(texts, self_rowid, 8).into(),
        haha: haha(texts, self_rowid),
        perfunctory: perfunctory(texts, self_rowid),
        heatmap: crate::trend::heatmap(facts),
    }
}

impl From<(Vec<(String, i64)>, Vec<(String, i64)>)> for SignatureWords {
    fn from((mine, theirs): (Vec<(String, i64)>, Vec<(String, i64)>)) -> Self {
        SignatureWords { mine, theirs }
    }
}

// —— 第一次纪念日 ——
fn firsts(facts: &[MessageFact], texts: &[TextMessage], self_rowid: Option<i64>) -> Vec<First> {
    let who = |s: i64| self_rowid.map(|me| if s == me { "我" } else { "ta" });
    // 按基础类型记录首次出现。
    let mut first_msg: Option<(i64, i64)> = None; // (time, sender)
    let mut first_img: Option<(i64, i64)> = None;
    let mut first_voice: Option<(i64, i64)> = None;
    let mut first_sticker: Option<(i64, i64)> = None;
    let mut first_call: Option<(i64, i64)> = None;
    for f in facts {
        if f.create_time <= 0 {
            continue;
        }
        if first_msg.is_none() {
            first_msg = Some((f.create_time, f.sender_id));
        }
        match f.base_type {
            3 if first_img.is_none() => first_img = Some((f.create_time, f.sender_id)),
            34 if first_voice.is_none() => first_voice = Some((f.create_time, f.sender_id)),
            47 if first_sticker.is_none() => first_sticker = Some((f.create_time, f.sender_id)),
            50 if first_call.is_none() => first_call = Some((f.create_time, f.sender_id)),
            _ => {}
        }
    }
    // 第一句爱意词。
    let mut first_love: Option<(i64, i64, String)> = None;
    for tm in texts {
        if let Some(ref text) = tm.text {
            if LOVE_WORDS.iter().any(|w| text.contains(w)) {
                let snip = crate::fmt::truncate(text, 24);
                first_love = Some((tm.create_time, tm.sender_id, snip));
                break;
            }
        }
    }

    let mk = |label: &str, hit: Option<(i64, i64)>| First {
        label: label.into(),
        when: hit.map(|(t, _)| crate::fmt::fmt_date(t)),
        who: hit.and_then(|(_, s)| who(s)),
        snippet: None,
    };
    let mut out = vec![
        mk("💬 第一条消息", first_msg),
        mk("🖼️ 第一张图片", first_img),
        mk("🎙️ 第一段语音", first_voice),
        mk("🎴 第一个表情", first_sticker),
        mk("📞 第一通通话", first_call),
    ];
    out.push(First {
        label: "💜 第一句「爱你/想你」".into(),
        when: first_love.as_ref().map(|(t, _, _)| crate::fmt::fmt_date(*t)),
        who: first_love.as_ref().and_then(|(_, s, _)| who(*s)),
        snippet: first_love.map(|(_, _, s)| s),
    });
    out
}

// —— 关系温度计 ——
fn relation_score(sc: &ScoreInputs) -> RelationScore {
    // 活跃度(0-25): 5000 条满分。
    let activity = ((sc.total as f64) / 200.0).min(25.0) as i64;
    // 均衡度(0-20): 越接近 50/50 越高。
    let total = (sc.my_count + sc.other_count).max(1) as f64;
    let my_pct = sc.my_count as f64 / total * 100.0;
    let balance = ((1.0 - (my_pct - 50.0).abs() / 50.0) * 20.0).round() as i64;
    // 秒回(0-25): 双方秒回率均值。
    let quick = ((sc.my_quick + sc.other_quick) / 2.0 * 25.0).round() as i64;
    // 连续(0-15): 最长连续天数，15 天满分。
    let streak = sc.longest_streak.min(15);
    // 覆盖(0-15): 活跃天数占比。
    let cover = (sc.active_ratio * 15.0).round() as i64;

    let score = (activity + balance + quick + streak + cover).clamp(0, 100);
    let label = match score {
        90..=100 => "如胶似漆",
        75..=89 => "甜度爆表",
        60..=74 => "稳中有甜",
        45..=59 => "不温不火",
        30..=44 => "有点冷淡",
        _ => "需要加温",
    };
    RelationScore {
        score,
        label,
        breakdown: vec![
            ("活跃度".into(), activity, 25),
            ("互动均衡".into(), balance, 20),
            ("秒回率".into(), quick, 25),
            ("最长连续".into(), streak, 15),
            ("活跃覆盖".into(), cover, 15),
        ],
    }
}

// —— 深夜 emo ——
fn late_emo(texts: &[TextMessage], self_rowid: Option<i64>) -> LateEmo {
    let is_self = |s: i64| crate::model::is_self(self_rowid, s);
    let mut my_count = 0i64;
    let mut other_count = 0i64;
    let mut longest: Option<EmoMsg> = None;
    for tm in texts {
        let Some(ref text) = tm.text else { continue };
        let Some(dt) = crate::fmt::local_dt(tm.create_time) else { continue };
        if dt.hour() > 4 {
            continue;
        }
        let chars = text.chars().count() as i64;
        if (chars as usize) < EMO_MIN_CHARS {
            continue;
        }
        let mine = is_self(tm.sender_id);
        if mine { my_count += 1; } else { other_count += 1; }
        if longest.as_ref().map_or(true, |e| chars > e.chars) {
            longest = Some(EmoMsg {
                chars,
                is_self: mine,
                when: crate::fmt::fmt_date(tm.create_time),
                snippet: crate::fmt::truncate(text, 40),
            });
        }
    }
    LateEmo { my_count, other_count, longest }
}

// —— 聊天生物钟 ——
fn biorhythm(facts: &[MessageFact], self_rowid: Option<i64>) -> Biorhythm {
    let is_self = |s: i64| crate::model::is_self(self_rowid, s);
    let mut me = [0i64; 24];
    let mut them = [0i64; 24];
    for f in facts {
        let Some(dt) = crate::fmt::local_dt(f.create_time) else { continue };
        let h = dt.hour() as usize;
        if h < 24 {
            if is_self(f.sender_id) { me[h] += 1; } else { them[h] += 1; }
        }
    }
    Biorhythm { me, them }
}

// —— 谁先消失 / 冷场 ——
fn ending(facts: &[MessageFact], self_rowid: Option<i64>) -> Ending {
    let is_self = |s: i64| crate::model::is_self(self_rowid, s);
    let mut my_last_word = 0i64;
    let mut other_last_word = 0i64;
    let mut my_left = 0i64;
    let mut other_left = 0i64;
    let mut prev: Option<(i64, i64)> = None; // (time, sender)
    for f in facts {
        if f.create_time <= 0 {
            continue;
        }
        if let Some((pt, ps)) = prev {
            let gap = f.create_time - pt;
            if gap > LEFT_ON_READ_GAP {
                // 上一条发完后被晾 >6h：ps 那一侧「被晾」。
                if is_self(ps) { my_left += 1; } else { other_left += 1; }
            }
            if gap > LAST_WORD_GAP {
                // 上一条是某段对话的「最后一句」。
                if is_self(ps) { my_last_word += 1; } else { other_last_word += 1; }
            }
        }
        prev = Some((f.create_time, f.sender_id));
    }
    // 末条也算「最后一句」。
    if let Some((_, ps)) = prev {
        if is_self(ps) { my_last_word += 1; } else { other_last_word += 1; }
    }
    Ending { my_last_word, other_last_word, my_left_on_read: my_left, other_left_on_read: other_left }
}

// —— 最长对线 ——
fn rally(facts: &[MessageFact]) -> Rally {
    let mut best_len = 0i64;
    let mut best_start: Option<i64> = None;
    let mut best_end: Option<i64> = None;
    let mut run_len = 0i64;
    let mut run_start: Option<i64> = None;
    let mut prev_time: Option<i64> = None;
    for f in facts {
        if f.create_time <= 0 {
            continue;
        }
        let start_new = match prev_time {
            Some(pt) => f.create_time - pt > RALLY_GAP,
            None => true,
        };
        if start_new {
            run_len = 1;
            run_start = Some(f.create_time);
        } else {
            run_len += 1;
        }
        if run_len > best_len {
            best_len = run_len;
            best_start = run_start;
            best_end = Some(f.create_time);
        }
        prev_time = Some(f.create_time);
    }
    let duration_min = match (best_start, best_end) {
        (Some(a), Some(b)) => ((b - a) / 60).max(0),
        _ => 0,
    };
    Rally { max_len: best_len, when: best_start.map(crate::fmt::fmt_date), duration_min }
}

// —— 哈哈指数 ——
fn haha(texts: &[TextMessage], self_rowid: Option<i64>) -> Haha {
    let is_self = |s: i64| crate::model::is_self(self_rowid, s);
    let mut me = [0i64; 5];
    let mut them = [0i64; 5];
    for tm in texts {
        let Some(ref text) = tm.text else { continue };
        let bucket = if is_self(tm.sender_id) { &mut me } else { &mut them };
        // 找「哈」的最大连续段，每段计一次（按长度分桶）。
        let mut run = 0usize;
        for ch in text.chars() {
            if ch == '哈' {
                run += 1;
            } else {
                if run >= 2 {
                    bucket[bucket_idx(run)] += 1;
                }
                run = 0;
            }
        }
        if run >= 2 {
            bucket[bucket_idx(run)] += 1;
        }
    }
    let me_total: i64 = me.iter().sum();
    let them_total: i64 = them.iter().sum();
    Haha { me, them, me_total, them_total }
}

fn bucket_idx(run: usize) -> usize {
    match run {
        2 => 0,
        3 => 1,
        4 => 2,
        5 => 3,
        _ => 4, // 6+
    }
}

// —— 敷衍指数 ——
fn perfunctory(texts: &[TextMessage], self_rowid: Option<i64>) -> Perfunctory {
    let is_self = |s: i64| crate::model::is_self(self_rowid, s);
    const SET: &[&str] = &["嗯", "哦", "噢", "额", "呃", "嗯嗯", "哦哦", "好", "好的", "嗯好", "嗯.", "哦."];
    let mut my = 0i64;
    let mut other = 0i64;
    for tm in texts {
        let Some(ref text) = tm.text else { continue };
        let t = text.trim();
        if SET.contains(&t) {
            if is_self(tm.sender_id) { my += 1; } else { other += 1; }
        }
    }
    Perfunctory { my_count: my, other_count: other }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MessageFact, TextMessage};

    fn mf(time: i64, sender: i64, bt: i64) -> MessageFact {
        MessageFact { create_time: time, sender_id: sender, base_type: bt }
    }
    fn tm(time: i64, sender: i64, text: &str) -> TextMessage {
        TextMessage { create_time: time, sender_id: sender, text: Some(text.into()) }
    }

    fn sc() -> ScoreInputs {
        ScoreInputs { total: 4000, my_count: 2000, other_count: 2000, longest_streak: 10, active_ratio: 0.5, my_quick: 0.7, other_quick: 0.7 }
    }

    #[test]
    fn score_in_range_and_balanced() {
        let s = relation_score(&sc());
        assert!((0..=100).contains(&s.score));
        // 50/50 均衡度应满分
        assert_eq!(s.breakdown.iter().find(|(k, _, _)| k == "互动均衡").unwrap().1, 20);
    }

    #[test]
    fn rally_finds_longest_burst() {
        // 连续 3 条（间隔 1min），断 20min，再连续 2 条 → 最长 3。
        let facts = vec![mf(1000, 1, 1), mf(1060, 2, 1), mf(1120, 1, 1), mf(2500, 2, 1), mf(2560, 1, 1)];
        let r = rally(&facts);
        assert_eq!(r.max_len, 3);
    }

    #[test]
    fn haha_buckets_runs() {
        let texts = vec![tm(1, 1, "哈哈哈"), tm(2, 2, "哈哈哈哈 哈哈")];
        let h = haha(&texts, Some(1));
        // 我：3 个哈 → bucket[1]；对方：4 个 → bucket[2]，2 个 → bucket[0]
        assert_eq!(h.me[1], 1);
        assert_eq!(h.them[2], 1);
        assert_eq!(h.them[0], 1);
    }

    #[test]
    fn perfunctory_counts_single_words() {
        let texts = vec![tm(1, 1, "嗯"), tm(2, 2, "好的"), tm(3, 1, "在吗？")];
        let p = perfunctory(&texts, Some(1));
        assert_eq!(p.my_count, 1); // 嗯
        assert_eq!(p.other_count, 1); // 好的
    }

    #[test]
    fn firsts_records_milestones() {
        let facts = vec![
            mf(1700000000, 2, 1),   // 第一条消息（对方）
            mf(1700000100, 1, 3),   // 第一张图（我）
            mf(1700000200, 2, 50),  // 第一通通话
        ];
        let texts = vec![tm(1700000300, 1, "今晚好想你呀")];
        let f = firsts(&facts, &texts, Some(1));
        assert_eq!(f[0].who, Some("ta")); // 第一条消息对方
        assert!(f.iter().any(|x| x.label.contains("图片") && x.who == Some("我")));
        assert!(f.iter().any(|x| x.label.contains("通话")));
        let love = f.iter().find(|x| x.label.contains("爱你")).unwrap();
        assert!(love.when.is_some());
    }
}
