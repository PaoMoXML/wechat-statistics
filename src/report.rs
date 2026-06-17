//! 终端报告展示层：把统计 / 深挖 / 情侣结果渲染成终端文本。
//!
//! 从 main.rs 拆出，使 main 只剩 CLI 定义与派发。
//! 数字 / 时长 / 类型名 / 时间格式化统一走 `fmt`、`schema` 模块，不再各持一份拷贝。

use std::collections::HashMap;

use anyhow::{Result, bail};

use crate::couple::CoupleReport;
use crate::curios::Curios;
use crate::dig::{ConversationDetail, LatencyStats};
use crate::loader::WeChatData;
use crate::model::{Contact, Conversation};
use crate::stats::{TemporalStats, VolumeStats};

/// 会话类型：群（含 @chatroom）或好友。
pub fn conv_kind(username: &str) -> &'static str {
    if username.contains("@chatroom") {
        "群"
    } else {
        "好友"
    }
}

/// 对 username 做轻量脱敏（避免把完整 wxid/群 ID 留在日志里）。
pub fn mask_user(username: &str) -> String {
    let chars: Vec<char> = username.chars().collect();
    match chars.len() {
        0..=4 => "*".to_string(),
        n => {
            let head: String = chars.iter().take(3).collect();
            let tail: String = chars.iter().skip(n.saturating_sub(3)).collect();
            format!("{head}…{tail}")
        }
    }
}

/// 按字符宽度截断显示名，超长加省略号（保留首尾各占位的原语义）。
pub fn truncate_display(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    crate::fmt::truncate(s, max.saturating_sub(1))
}

pub struct ContactIndex {
    by_username: HashMap<String, Contact>,
}

impl ContactIndex {
    pub fn build(data: &WeChatData) -> Result<Self> {
        let contacts = data.load_contacts()?;
        let by_username = contacts.into_iter().map(|c| (c.username.clone(), c)).collect();
        Ok(Self { by_username })
    }

    /// 显示名（命中用 remark/nick_name，否则脱敏 username）。
    pub fn name_by_username(&self, username: &str) -> String {
        match self.by_username.get(username) {
            Some(c) => c.display_name().to_string(),
            None => mask_user(username),
        }
    }
}

/// 解析定位会话的三种方式：--with（子串模糊）/ --rank（按消息量名次）/ --user（精确）。
pub fn locate_conversation(
    data: &WeChatData,
    convs: &[Conversation],
    index: &ContactIndex,
    with: Option<&str>,
    rank: Option<usize>,
    user: Option<&str>,
) -> Result<Conversation> {
    if let Some(name) = user {
        return convs
            .iter()
            .find(|c| c.username == name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("--user 未命中：{name}"));
    }
    if let Some(query) = with {
        let q = query.to_lowercase();
        let hits: Vec<&Conversation> = convs
            .iter()
            .filter(|c| {
                let name = index.name_by_username(&c.username).to_lowercase();
                c.username.to_lowercase().contains(&q) || name.contains(&q)
            })
            .collect();
        return match hits.len() {
            0 => bail!("「{query}」未匹配到任何会话"),
            1 => Ok(hits[0].clone()),
            _ => {
                eprintln!("「{query}」匹配到 {} 个会话，请用更精确的片段或 --user：", hits.len());
                for h in hits.iter().take(20) {
                    eprintln!("  · {} [{}]", index.name_by_username(&h.username), h.username);
                }
                bail!("匹配不唯一，请缩小范围");
            }
        };
    }
    if let Some(n) = rank {
        if n == 0 {
            bail!("--rank 从 1 开始");
        }
        let counts = data.count_messages_batch(convs)?;
        let mut paired: Vec<(&Conversation, i64)> =
            convs.iter().zip(counts.iter().copied()).collect();
        paired.sort_by(|a, b| b.1.cmp(&a.1));
        return paired
            .get(n - 1)
            .map(|(c, _)| (*c).clone())
            .ok_or_else(|| anyhow::anyhow!("--rank {n} 超出范围（共 {} 个会话）", convs.len()));
    }
    bail!("请用 --with <昵称片段> / --rank N / --user <wxid> 之一指定目标会话");
}

pub fn print_curios(c: &Curios) {
    println!("\n╭─ ✨ 趣味统计 ────────────────────────────────────────────");
    println!("│ 🌡️ 关系温度：{}/100 · {}", c.score.score, c.score.label);
    let bd: Vec<String> = c.score.breakdown.iter().map(|(k, p, m)| format!("{k} {p}/{m}")).collect();
    println!("│    {}", bd.join("  "));

    let has_firsts = c.firsts.iter().any(|f| f.when.is_some());
    if has_firsts {
        println!("│ 🏁 第一次纪念日：");
        for f in &c.firsts {
            if let Some(w) = &f.when {
                let who = f.who.unwrap_or("");
                let extra = f.snippet.as_deref().unwrap_or("");
                let tail = if extra.is_empty() { String::new() } else { format!("「{extra}」") };
                println!("│    {} · {} {} {}", f.label, w, who, tail);
            }
        }
    }

    if c.emo.my_count + c.emo.other_count > 0 {
        println!("│ 🌙 深夜 emo 长文（≥50 字 @0–4 点）：我 {} · ta {}", crate::fmt::fmt_num(c.emo.my_count), crate::fmt::fmt_num(c.emo.other_count));
        if let Some(e) = &c.emo.longest {
            let who = if e.is_self { "我" } else { "ta" };
            println!("│    最长：{} 字（{} 发，{}）「{}」", crate::fmt::fmt_num(e.chars), who, e.when, e.snippet);
        }
    }

    let me_spark = crate::trend::sparkline(&c.biorhythm.me);
    let them_spark = crate::trend::sparkline(&c.biorhythm.them);
    println!("│ 🕐 聊天生物钟（0–23 点）");
    println!("│    我  {me_spark}");
    println!("│    ta  {them_spark}");

    println!("│ 💤 谁先消失：最后一句 我 {} · ta {}", crate::fmt::fmt_num(c.ending.my_last_word), crate::fmt::fmt_num(c.ending.other_last_word));
    println!("│    被晾 >6h：我 {} · ta {}", crate::fmt::fmt_num(c.ending.my_left_on_read), crate::fmt::fmt_num(c.ending.other_left_on_read));

    if c.rally.max_len > 0 {
        let when = c.rally.when.as_deref().unwrap_or("");
        println!("│ 🔁 最长对线：{} 条（{}，约 {} 分钟）", crate::fmt::fmt_num(c.rally.max_len), when, crate::fmt::fmt_num(c.rally.duration_min));
    }

    if !c.signature.mine.is_empty() || !c.signature.theirs.is_empty() {
        let mk = |v: &[(String, i64)]| v.iter().map(|(w, n)| format!("{w}({n})")).collect::<Vec<_>>().join(" ");
        println!("│ 🗣️ 口癖：我 [{}] · ta [{}]", mk(&c.signature.mine), mk(&c.signature.theirs));
    }

    if c.haha.me_total + c.haha.them_total > 0 {
        println!("│ 😂 哈哈指数：我 {} · ta {}", crate::fmt::fmt_num(c.haha.me_total), crate::fmt::fmt_num(c.haha.them_total));
    }

    println!("│ 🥱 敷衍回复（嗯/哦/好…）：我 {} · ta {}", crate::fmt::fmt_num(c.perfunctory.my_count), crate::fmt::fmt_num(c.perfunctory.other_count));

    println!("│ 🗓️ 全年热力图：{}/{} 天有聊天（详细见 HTML 报告）", crate::fmt::fmt_num(c.heatmap.active_days), crate::fmt::fmt_num(c.heatmap.span_days));

    println!("╰─────────────────────────────────────────────────────────");
}

pub fn print_volume_report(v: &VolumeStats, index: &ContactIndex) {
    println!("\n╭─ 微信消息统计 ──────────────────────────────────────────");
    println!("│ 会话总数      : {}", crate::fmt::fmt_num(v.conversations as i64));
    println!("│ 消息总数      : {}", crate::fmt::fmt_num(v.total_messages));

    // 把基础类型归入 文本 / 媒体细分 / 系统 / 其它。
    let mut text = 0i64;
    let mut system = 0i64;
    let mut other = 0i64;
    let mut img = 0i64;
    let mut voice = 0i64;
    let mut video = 0i64;
    let mut sticker = 0i64;
    let mut file = 0i64;
    let mut call = 0i64;
    for (t, n) in &v.type_dist {
        match *t {
            1 => text += n,
            3 => img += n,
            34 => voice += n,
            43 => video += n,
            47 => sticker += n,
            49 => file += n,
            50 => call += n,
            10000 => system += n,
            _ => other += n,
        }
    }
    let media = img + voice + video + sticker + file + call;
    let total = v.total_messages.max(1) as f64;
    let pct = |x: i64| format!("{:5.1}%", x as f64 / total * 100.0);
    println!("│   ├ 文本      : {:>14}  ({})", crate::fmt::fmt_num(text), pct(text));
    println!("│   ├ 媒体      : {:>14}  ({})", crate::fmt::fmt_num(media), pct(media));
    println!("│   ├ 系统      : {:>14}  ({})", crate::fmt::fmt_num(system), pct(system));
    println!("│   └ 其它      : {:>14}  ({})", crate::fmt::fmt_num(other), pct(other));
    println!(
        "│ 媒体拆分      : 图片 {} · 语音 {} · 视频 {} · 表情 {} · 文件 {} · 通话 {}",
        crate::fmt::fmt_num(img), crate::fmt::fmt_num(voice), crate::fmt::fmt_num(video), crate::fmt::fmt_num(sticker), crate::fmt::fmt_num(file), crate::fmt::fmt_num(call)
    );

    println!("│ 类型分布（基础类型码）：");
    for (t, n) in v.type_dist.iter().take(8) {
        println!("│   {:>6} {:<10} {:>12}", t, crate::schema::type_label(*t), crate::fmt::fmt_num(*n));
    }

    println!("│ 消息数 Top {} 会话（名称为我 / 对方）：", v.top.len());
    let max_n = v.top.iter().map(|(_, n, _, _)| *n).max().unwrap_or(0);
    for (i, (u, n, mine, theirs)) in v.top.iter().enumerate() {
        let name = index.name_by_username(u);
        println!(
            "│ {:>2}. [{}] {:<22} {:>10}  我 {:>6} · 对方 {:>6}  {}",
            i + 1,
            conv_kind(u),
            truncate_display(&name, 20),
            crate::fmt::fmt_num(*n),
            crate::fmt::fmt_num(*mine),
            crate::fmt::fmt_num(*theirs),
            crate::fmt::bar(*n, max_n, 16)
        );
    }
    println!("╰─────────────────────────────────────────────────────────");
}

pub fn print_temporal_report(t: &TemporalStats) {
    let total: i64 = t.hour.iter().sum();
    println!("\n╭─ 活跃时段（本地时间）  共 {} 条 ──────────────────────", crate::fmt::fmt_num(total));

    let hmax = *t.hour.iter().max().unwrap_or(&0);
    for h in 0..24 {
        println!("│ {:02}:00 ▏{:<28} {:>12}", h, crate::fmt::bar(t.hour[h], hmax, 28), crate::fmt::fmt_num(t.hour[h]));
    }

    println!("│");
    println!("│ 星期分布：");
    let days = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"];
    let wmax = *t.weekday.iter().max().unwrap_or(&0);
    for (i, d) in days.iter().enumerate() {
        println!("│ {} ▏{:<28} {:>12}", d, crate::fmt::bar(t.weekday[i], wmax, 28), crate::fmt::fmt_num(t.weekday[i]));
    }

    if !t.month.is_empty() {
        println!("│");
        println!("│ 月份趋势（最近 24 个月）：");
        let mmax = t.month.iter().map(|(_, n)| *n).max().unwrap_or(0);
        let start = t.month.len().saturating_sub(24);
        for (ym, n) in t.month.iter().skip(start) {
            println!("│   {} ▏{:<24} {:>12}", ym, crate::fmt::bar(*n, mmax, 24), crate::fmt::fmt_num(*n));
        }
    }
    println!("╰─────────────────────────────────────────────────────────");
}

pub fn print_dig_report(conv: &Conversation, d: &ConversationDetail, index: &ContactIndex) {
    let name = index.name_by_username(&conv.username);
    let kind = if d.is_group { "群聊" } else { "单聊" };

    println!("\n╭─ 单会话深挖 · {} [{}] ──────────────────────────────", name, kind);
    println!("│ 消息总数      : {}", crate::fmt::fmt_num(d.total));
    if let (Some(a), Some(b)) = (d.time_min, d.time_max) {
        println!("│ 时间跨度      : {} ~ {}", crate::fmt::fmt_ts(a), crate::fmt::fmt_ts(b));
    }

    // —— 发送比例 ——
    let denom = d.my_count + d.other_count;
    let pct = |x: i64| if denom > 0 { format!("{:.1}%", x as f64 / denom as f64 * 100.0) } else { "—".into() };
    let m = d.my_count.max(1);
    let o = d.other_count.max(1);
    let big = d.my_count.max(d.other_count).max(1);
    println!("│ 发送比例      : 我 {} ({})  ·  对方 {} ({})", crate::fmt::fmt_num(d.my_count), pct(d.my_count), crate::fmt::fmt_num(d.other_count), pct(d.other_count));
    println!("│   我   ▏{:<20} {}", crate::fmt::bar(d.my_count, big, 20), crate::fmt::fmt_num(d.my_count));
    println!("│   对方▏{:<20} {}", crate::fmt::bar(d.other_count, big, 20), crate::fmt::fmt_num(d.other_count));
    if m > 0 && o > 0 {
        let ratio = d.my_count as f64 / o as f64;
        println!("│   我:对方 ≈ {:.2} : 1", ratio);
    }

    // —— 类型对比 ——
    println!("│ 对方最常发的类型：");
    for (t, n) in d.other_type_dist.iter().take(5) {
        println!("│   {:<10} {:>10}  {}", crate::schema::type_label(*t), crate::fmt::fmt_num(*n), crate::fmt::bar(*n, d.other_count, 16));
    }
    println!("│ 我最常发的类型：");
    for (t, n) in d.my_type_dist.iter().take(5) {
        println!("│   {:<10} {:>10}  {}", crate::schema::type_label(*t), crate::fmt::fmt_num(*n), crate::fmt::bar(*n, d.my_count.max(1), 16));
    }

    // —— 回复时延 ——
    println!("│ 回复时延（切换发送者算一次回复，上限 6h）：");
    print_latency("│   我回对方", &d.my_reply);
    print_latency("│   对方回我", &d.other_reply);
    if d.capped_replies > 0 {
        println!("│   （另有 {} 次超过 6h 的续聊未计入）", crate::fmt::fmt_num(d.capped_replies));
    }

    // —— 每日首发 ——
    let open_total = d.days_self_open + d.days_other_open;
    let open_pct = |x: i64| if open_total > 0 { format!("{:.1}%", x as f64 / open_total as f64 * 100.0) } else { "—".into() };
    println!("│ 每日谁先开口：我主动 {} 天 ({})  ·  对方先 {} 天 ({})",
        crate::fmt::fmt_num(d.days_self_open), open_pct(d.days_self_open), crate::fmt::fmt_num(d.days_other_open), open_pct(d.days_other_open));
    if !d.recent_openers.is_empty() {
        println!("│   最近 {} 天明细（最近在上）：", d.recent_openers.len());
        for (date, is_self, t) in d.recent_openers.iter().rev().take(14) {
            let who = if *is_self { "我" } else { "对方" };
            let hhmm = crate::fmt::local_dt(*t)
                .map(|dt| dt.format("%H:%M").to_string())
                .unwrap_or_else(|| "?".into());
            println!("│     {date}  {who} 先开口  {hhmm}");
        }
    }
    println!("╰─────────────────────────────────────────────────────────");
}

pub fn print_latency(label: &str, s: &LatencyStats) {
    match (s.median_sec, s.mean_sec) {
        (Some(md), Some(mn)) => {
            println!("{}：中位 {} · 均值 {} · 共 {} 次", label, crate::fmt::fmt_duration(md), crate::fmt::fmt_duration(mn), crate::fmt::fmt_num(s.samples));
        }
        _ => println!("{}：无样本", label),
    }
}

pub fn print_couple_report(conv: &Conversation, r: &CoupleReport, index: &ContactIndex) {
    let name = index.name_by_username(&conv.username);
    let pct = |x: f64| format!("{:.1}%", x * 100.0);

    println!("\n╭─ 💑 情侣报告 · {} ─────────────────────────────────", name);

    // —— 时间叙事 ——
    if let Some(first) = &r.first_day {
        println!("│ 📅 故事开始于 {first}，今天是第 {} 天", crate::fmt::fmt_num(r.nth_day_today));
    }
    println!(
        "│    共聊了 {} 条消息（我 {} · 对方 {}），分布在 {} / {} 天里（{}）",
        crate::fmt::fmt_num(r.total), crate::fmt::fmt_num(r.my_count), crate::fmt::fmt_num(r.other_count),
        crate::fmt::fmt_num(r.active_days), crate::fmt::fmt_num(r.nth_day_today), pct(r.active_ratio)
    );

    println!("│");
    println!("│ 🔥 最长连续聊天：{} 天不间断", crate::fmt::fmt_num(r.longest_streak));
    if r.current_streak > 0 {
        println!("│    当前已连续 {} 天 ✨", crate::fmt::fmt_num(r.current_streak));
    } else {
        println!("│    （当前连续已中断）");
    }
    println!("│ 🤫 最长沉默：{} 天没说话", crate::fmt::fmt_num(r.longest_silence_days));
    if let Some((a, b)) = &r.longest_silence_range {
        println!("│    （{a} 之后，直到 {b} 才再说话）");
    }

    if let Some((day, n)) = &r.peak_day {
        println!("│ 🎉 最嗨的一天：{day}，聊了 {} 条", crate::fmt::fmt_num(*n));
    }
    println!("│ 🌙 一起熬过的夜：{} 条凌晨消息（0–4 点）", crate::fmt::fmt_num(r.late_night_count));
    if r.call_count > 0 {
        println!("│ 📞 语音/视频通话：{} 次", crate::fmt::fmt_num(r.call_count));
    }

    // —— 双向 ——
    println!("│");
    if r.my_reply_count + r.other_reply_count > 0 {
        println!("│ ⚡ 秒回率（30 秒内回复）：我 {} · 对方 {}", pct(r.my_quick_reply_ratio), pct(r.other_quick_reply_ratio));
        println!("│    1 分钟内回复率     ：我 {} · 对方 {}", pct(r.my_fast_reply_ratio), pct(r.other_fast_reply_ratio));
    }

    if let Some(last) = &r.last_day {
        println!("│");
        println!("│ 💌 最近一次聊天：{last}");
    }

    // —— 文本指标 ——
    if let Some(t) = &r.text {
        println!("│");
        println!("│ ✍️ 总字数：我 {} · 对方 {}", crate::fmt::fmt_num(t.my_chars), crate::fmt::fmt_num(t.other_chars));
        if let Some((chars, is_self, ts)) = &t.longest {
            let who = if *is_self { "我" } else { "对方" };
            let when = crate::fmt::local_dt(*ts)
                .map(|dt| dt.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| "?".into());
            println!("│ 📝 最长一条：{} 字（{} 发，{when}）", crate::fmt::fmt_num(*chars), who);
        }
        if !t.keywords.is_empty() {
            println!("│ 💬 关键词（消息条数，我 / 对方）：");
            for (k, me, oth) in t.keywords.iter().take(10) {
                println!("│    {:<6} 我 {:>4} · 对方 {:>4}", k, crate::fmt::fmt_num(*me), crate::fmt::fmt_num(*oth));
            }
        }
    }

    // —— 高频词 ——
    if let Some(wf) = &r.words {
        println!("│");
        if !wf.overall.is_empty() {
            let cloud: Vec<String> = wf.overall.iter().map(|(w, c)| format!("{w}({})", crate::fmt::fmt_num(*c))).collect();
            println!("│ ☁️ 总体高频词：{}", cloud.join("  "));
        }
        if !wf.mine.is_empty() {
            let cloud: Vec<String> = wf.mine.iter().map(|(w, c)| format!("{w}({})", crate::fmt::fmt_num(*c))).collect();
            println!("│    你最常说：{}", cloud.join("  "));
        }
        if !wf.theirs.is_empty() {
            let cloud: Vec<String> = wf.theirs.iter().map(|(w, c)| format!("{w}({})", crate::fmt::fmt_num(*c))).collect();
            println!("│    ta 最常说：{}", cloud.join("  "));
        }
    }

    // —— dig 深挖（类型分布 / 回复时延中位均值 / 每日首发）——
    if let Some(d) = &r.dig {
        let lat = |s: &LatencyStats| match (s.median_sec, s.mean_sec) {
            (Some(md), Some(mn)) => {
                format!("中位 {} · 均值 {}（{} 次）", crate::fmt::fmt_duration(md), crate::fmt::fmt_duration(mn), crate::fmt::fmt_num(s.samples))
            }
            _ => "无样本".to_string(),
        };
        let types = |dist: &[(i64, i64)]| {
            if dist.is_empty() {
                "—".to_string()
            } else {
                dist.iter()
                    .take(4)
                    .map(|(t, n)| format!("{} {}", crate::schema::type_label(*t), crate::fmt::fmt_num(*n)))
                    .collect::<Vec<_>>()
                    .join("  ")
            }
        };
        println!("│");
        println!("│ 🔎 深挖");
        println!("│    回复时延：我 {} · ta {}", lat(&d.my_reply), lat(&d.other_reply));
        println!("│    每日首发：我开口 {} 天 · ta 开口 {} 天", crate::fmt::fmt_num(d.days_self_open), crate::fmt::fmt_num(d.days_other_open));
        println!("│    我常发：{}", types(&d.my_type_dist));
        println!("│    ta常发：{}", types(&d.other_type_dist));
    }

    // —— 趋势曲线 ——
    if let Some(t) = &r.trend {
        if t.points.len() >= 2 {
            let counts: Vec<i64> = t.points.iter().map(|p| p.count).collect();
            let spark = crate::trend::sparkline(&counts);
            let first = t.points.first().unwrap().label.as_str();
            let last = t.points.last().unwrap().label.as_str();
            println!("│");
            println!("│ 📈 趋势（按{}，共 {} 个，{tag}）", t.kind, crate::fmt::fmt_num(t.points.len() as i64), tag = t.tag);
            println!("│    {spark}");
            println!("│    {first} ─────────────────→ {last}");
            if let Some(p) = &t.peak {
                println!("│    最热烈的一{}：{}（{} 条）", t.kind, p.label, crate::fmt::fmt_num(p.count));
            }
        }
    }

    println!("╰─────────────────────────────────────────────────────────");
}
