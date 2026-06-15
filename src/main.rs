mod content;
mod couple;
mod curios;
mod dig;
mod lexical;
mod loader;
mod model;
mod probe;
mod render;
mod schema;
mod stats;
mod trend;

use anyhow::{Result, bail};
use chrono::{Local, TimeZone};
use clap::{Parser, Subcommand};
use loader::WeChatData;
use model::Contact;
use probe::{ProbeOptions, probe_path};
use serde::Serialize;
use stats::{TemporalAccum, VolumeAccum};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "wechat-statistics",
    version,
    about = "个人微信消息统计工具 — schema 探测 + 解析 + 全局统计"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 探测已解密 SQLite 库的表结构（Phase 0）
    Schema {
        /// 单个 .db 文件，或包含多个 .db 的目录
        #[arg(short, long)]
        db: PathBuf,

        /// 跳过 COUNT(*)（GB 级大库可显著加速）
        #[arg(long)]
        no_count: bool,

        /// 跳过采样行（只要结构、不要数据，JSON 最小）
        #[arg(long)]
        no_samples: bool,

        /// 每个表的采样行数
        #[arg(long, default_value_t = 3)]
        limit: usize,

        /// JSON 中每组保留的表名数量（结构相同会自动分组，名字是冗余的）
        #[arg(long, default_value_t = 50)]
        max_table_names: usize,

        /// 同时把探测结果写为 JSON（供后续 schema 适配层读取）
        #[arg(long)]
        json: Option<PathBuf>,
    },

    /// 自检：构造一个仿微信 schema 的临时库并探测它
    /// （本机无微信/无 sqlite3 CLI 时也能验证整条探测流程）
    Selftest,

    /// 解析已解密数据目录，校验适配层并打印聚合概览（Phase 1 骨架）
    Inspect {
        /// 已解密的微信 4.x 数据目录（含 contact.db / session.db / message_*.db）
        #[arg(long)]
        data: PathBuf,

        /// 展示消息数 Top N 会话
        #[arg(long, default_value_t = 10)]
        top: usize,
    },

    /// 全局消息统计：消息量 / 类型分布 / 媒体拆分 / 活跃时段（跨所有会话）
    Stats {
        /// 已解密的微信 4.x 数据目录（含 contact.db / session.db / message_*.db）
        #[arg(long)]
        data: PathBuf,

        /// 展示消息数 Top N 会话
        #[arg(long, default_value_t = 10)]
        top: usize,

        /// 同时把统计结果写为 JSON
        #[arg(long)]
        json: Option<PathBuf>,
    },

    /// 单会话深挖：发送比例 / 类型对比 / 回复时延 / 每日谁先开口
    Dig {
        /// 已解密的微信 4.x 数据目录（目录名即自己的 wxid）
        #[arg(long)]
        data: PathBuf,

        /// 按昵称/备注/用户名片段定位会话（唯一命中即用，多个则列出候选并退出）
        #[arg(long)]
        with: Option<String>,

        /// 按全局消息量第 N 名定位（1 = 消息最多的会话）
        #[arg(long)]
        rank: Option<usize>,

        /// 精确用户名（wxid / 群ID）定位
        #[arg(long)]
        user: Option<String>,

        /// 同时把深挖结果写为 JSON
        #[arg(long)]
        json: Option<PathBuf>,
    },

    /// 情侣报告（单聊叙事统计）：在一起第N天/最长连续/最长沉默/最嗨一天/熬夜/通话/秒回率
    Couple {
        /// 已解密的微信 4.x 数据目录（目录名即自己的 wxid）
        #[arg(long)]
        data: PathBuf,

        /// 按昵称/备注/用户名片段定位会话
        #[arg(long)]
        with: Option<String>,

        /// 按全局消息量第 N 名定位
        #[arg(long)]
        rank: Option<usize>,

        /// 精确用户名（wxid）定位
        #[arg(long)]
        user: Option<String>,

        /// 高频词每组返回个数（0 关闭，默认 12）
        #[arg(long, default_value_t = 12)]
        words: usize,

        /// 渲染自包含 HTML 报告并写入该路径
        #[arg(long)]
        html: Option<PathBuf>,

        /// 渲染 PPT 风格翻页 HTML 并写入该路径
        #[arg(long)]
        slides: Option<PathBuf>,

        /// 同时把报告写为 JSON
        #[arg(long)]
        json: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Schema { db, no_count, no_samples, limit, max_table_names, json } => {
            let opts = ProbeOptions {
                no_count,
                no_samples,
                sample_limit: limit,
                max_table_names,
                ..Default::default()
            };
            let reports = probe_path(&db, &opts)?;

            if let Some(json_path) = json {
                let pretty = serde_json::to_string_pretty(&reports)?;
                std::fs::write(&json_path, pretty)?;
                println!("\n✓ 探测结果已写入 {}", json_path.display());
            }
        }
        Command::Selftest => {
            let tmp = run_selftest()?;
            println!("\n[自检通过] 仿微信库已生成: {}", tmp.display());
            println!("可手动复跑: cargo run -- schema --db {}", tmp.display());
        }
        Command::Inspect { data, top } => {
            let data = WeChatData::open(&data)?;

            // 1) 联系人（适配层会校验 contact 表列）
            let contacts = data.load_contacts()?;
            let chatrooms = contacts.iter().filter(|c| c.is_chatroom).count();
            println!("✓ 联系人表解析成功：共 {} 个（其中群 {}）", contacts.len(), chatrooms);

            // 2) 会话映射
            let convs = data.load_conversations()?;
            println!("✓ 会话映射解析成功：共 {} 个会话", convs.len());

            // 3) 批量统计消息数，按多寡排序
            let counts = data.count_messages_batch(&convs)?;
            let mut paired: Vec<(&model::Conversation, i64)> =
                convs.iter().zip(counts.iter().copied()).collect();
            paired.sort_by(|a, b| b.1.cmp(&a.1));
            let total: i64 = paired.iter().map(|(_, n)| *n).sum();

            println!("\n这 {} 个会话合计约 {total} 条消息。Top {top}：", convs.len());
            for (i, (c, n)) in paired.iter().take(top).enumerate() {
                println!(
                    "  {:>2}. [{}] {:<22} {:>8} 条  ({} 分片 · {})",
                    i + 1,
                    conv_kind(&c.username),
                    mask_user(&c.username),
                    n,
                    c.db_stems.len(),
                    c.table_name
                );
            }

            // 4) 对消息最多的会话做聚合（纯 SQL，不读正文）
            if let Some((topc, _)) = paired.first() {
                let st = data.conversation_stats(topc)?;
                println!("\n消息最多会话的聚合统计：");
                println!("  消息总数      : {}", st.count);
                println!("  不同发送者数  : {}", st.distinct_senders);
                if let (Some(a), Some(b)) = (st.time_min, st.time_max) {
                    println!("  时间跨度      : {} ~ {}", fmt_ts(a), fmt_ts(b));
                }
                let mut td = st.type_dist.clone();
                td.sort_by(|a, b| b.1.cmp(&a.1));
                println!("  类型分布(local_type):");
                for (t, n) in td {
                    println!("    type {:>6} : {}", t, n);
                }
            }

            println!("\n✓ 适配层与解析骨架工作正常（read-only，未读取任何正文内容）。");
        }
        Command::Stats { data, top, json } => {
            let data = WeChatData::open(&data)?;
            let convs = data.load_conversations()?;
            let index = ContactIndex::build(&data)?;
            println!("✓ 共 {} 个会话，开始跨会话聚合…（纯 SQL，不读正文）", convs.len());

            // 单次遍历，同时喂入两个累加器：消息量 + 时段分布。
            // self_rowid 由 for_each_conversation 按库解析传入（Name2Id per-DB）。
            let mut vol = VolumeAccum::new(top);
            let mut tmp = TemporalAccum::new();
            data.for_each_conversation(&convs, |conn, c, self_rowid| {
                vol.observe_table(conn, c, self_rowid)?;
                tmp.observe_table(conn, c)?;
                Ok(())
            })?;

            let vol_stats = vol.finalize(convs.len());
            let tmp_stats = tmp.finalize();

            print_volume_report(&vol_stats, &index);
            print_temporal_report(&tmp_stats);

            if let Some(json_path) = json {
                #[derive(Serialize)]
                struct Out<'a> {
                    volume: &'a stats::VolumeStats,
                    temporal: &'a stats::TemporalStats,
                }
                let out = Out { volume: &vol_stats, temporal: &tmp_stats };
                std::fs::write(&json_path, serde_json::to_string_pretty(&out)?)?;
                println!("\n✓ 统计结果已写入 {}", json_path.display());
            }
        }
        Command::Dig { data, with, rank, user, json } => {
            let data = WeChatData::open(&data)?;
            let convs = data.load_conversations()?;
            let index = ContactIndex::build(&data)?;

            // 定位目标会话
            let conv = locate_conversation(&data, &convs, &index, with.as_deref(), rank, user.as_deref())?;
            println!("✓ 目标会话：{} [{}] ({})", index.name_by_username(&conv.username), conv_kind(&conv.username), conv.username);

            // 读取时序事实（跨全部分片，sender_id 已归一化 0=我 / 1=对方）→ 深挖
            let facts = data.message_facts(&conv)?;
            let is_group = conv.username.contains("@chatroom");
            let detail = dig::compute(&facts, 0, is_group);

            print_dig_report(&conv, &detail, &index);

            if let Some(json_path) = json {
                std::fs::write(&json_path, serde_json::to_string_pretty(&detail)?)?;
                println!("\n✓ 深挖结果已写入 {}", json_path.display());
            }
        }
        Command::Couple { data, with, rank, user, words, html, slides, json } => {
            let data = WeChatData::open(&data)?;
            let convs = data.load_conversations()?;
            let index = ContactIndex::build(&data)?;

            let conv = locate_conversation(&data, &convs, &index, with.as_deref(), rank, user.as_deref())?;
            let name = index.name_by_username(&conv.username).to_string();
            println!("✓ 目标会话：{} [{}] ({})", name, conv_kind(&conv.username), conv.username);

            // 跨全部分片读取，sender_id 已归一化（0=我 / 1=对方），统一传 self_id=0
            let facts = data.message_facts(&conv)?;
            let texts = data.text_messages(&conv)?;
            let mut report = couple::compute(&facts, Some(0), Some(&texts));

            if words > 0 {
                report.words = Some(lexical::word_freq(&texts, Some(0), words));
            }

            // 嵌入 dig 的单会话深挖（类型分布 / 回复时延中位均值 / 每日首发）
            let is_group = conv.username.contains("@chatroom");
            report.dig = Some(dig::compute(&facts, 0, is_group));

            // 趋势曲线（按周 + 按月，供 HTML 周↔月 切换）
            report.trend = Some(trend::weekly(&facts));
            report.trend_monthly = Some(trend::monthly(&facts));

            // 趣味统计（叙事彩蛋 / 节奏对比 / 词趣）
            let inputs = curios::ScoreInputs {
                total: report.total,
                my_count: report.my_count,
                other_count: report.other_count,
                longest_streak: report.longest_streak,
                active_ratio: report.active_ratio,
                my_quick: report.my_quick_reply_ratio,
                other_quick: report.other_quick_reply_ratio,
            };
            let curios = curios::compute(&facts, &texts, Some(0), &inputs);

            print_couple_report(&conv, &report, &index);
            print_curios(&curios);

            if let Some(html_path) = html {
                let html = render::couple_html(&conv, &name, &report, &curios);
                std::fs::write(&html_path, html)?;
                println!("\n✓ HTML 报告已写入 {}", html_path.display());
            }
            if let Some(slides_path) = slides {
                let html = render::couple_slides(&conv, &name, &report, &curios);
                std::fs::write(&slides_path, html)?;
                println!("\n✓ PPT 幻灯片已写入 {}", slides_path.display());
            }
            if let Some(json_path) = json {
                let out = serde_json::json!({ "report": report, "curios": curios });
                std::fs::write(&json_path, serde_json::to_string_pretty(&out)?)?;
                println!("\n✓ 情侣报告已写入 {}", json_path.display());
            }
        }
    }
    Ok(())
}

/// 会话类型：群（含 @chatroom）或好友。
fn conv_kind(username: &str) -> &'static str {
    if username.contains("@chatroom") {
        "群"
    } else {
        "好友"
    }
}

/// 对 username 做轻量脱敏（避免把完整 wxid/群 ID 留在日志里）。
fn mask_user(username: &str) -> String {
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

/// Unix 秒 → 本地可读时间。
fn fmt_ts(secs: i64) -> String {
    match Local.timestamp_opt(secs, 0).single() {
        Some(dt) => dt.format("%Y-%m-%d %H:%M:%S").to_string(),
        None => format!("{secs}(无法解析)"),
    }
}

/// 基础消息类型码 → 中文名（见 schema.rs 注释）。
fn base_type_name(t: i64) -> &'static str {
    match t {
        1 => "文本",
        3 => "图片",
        34 => "语音",
        42 => "名片",
        43 => "视频",
        47 => "表情",
        48 => "位置",
        49 => "文件/应用",
        50 => "音视频通话",
        10000 => "系统消息",
        _ => "其它",
    }
}

/// 千分位格式化：1234567 → "1,234,567"。
fn fmt_num(n: i64) -> String {
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

/// 按最大值等比缩放的文本柱状图。
fn bar(value: i64, max: i64, width: usize) -> String {
    if max <= 0 {
        return String::new();
    }
    let scaled = (value as f64 / max as f64 * width as f64).round() as usize;
    "▇".repeat(scaled.min(width))
}

/// 按字符宽度截断显示名，超长加省略号。
fn truncate_display(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    let end = s.char_indices().nth(max.saturating_sub(1)).map(|(i, _)| i).unwrap_or(s.len());
    format!("{}…", &s[..end])
}

/// 秒数 → 「N 时 M 分」「M 分 S 秒」「S 秒」可读时长。
fn fmt_duration(secs: i64) -> String {
    if secs < 60 {
        return format!("{secs} 秒");
    }
    if secs < 3600 {
        return format!("{} 分 {}", secs / 60, secs % 60);
    }
    format!("{} 时 {} 分", secs / 3600, (secs % 3600) / 60)
}

fn print_curios(c: &curios::Curios) {
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
        println!("│ 🌙 深夜 emo 长文（≥50 字 @0–4 点）：我 {} · ta {}", fmt_num(c.emo.my_count), fmt_num(c.emo.other_count));
        if let Some(e) = &c.emo.longest {
            let who = if e.is_self { "我" } else { "ta" };
            println!("│    最长：{} 字（{} 发，{}）「{}」", fmt_num(e.chars), who, e.when, e.snippet);
        }
    }

    let me_spark = trend::sparkline(&c.biorhythm.me);
    let them_spark = trend::sparkline(&c.biorhythm.them);
    println!("│ 🕐 聊天生物钟（0–23 点）");
    println!("│    我  {me_spark}");
    println!("│    ta  {them_spark}");

    println!("│ 💤 谁先消失：最后一句 我 {} · ta {}", fmt_num(c.ending.my_last_word), fmt_num(c.ending.other_last_word));
    println!("│    被晾 >6h：我 {} · ta {}", fmt_num(c.ending.my_left_on_read), fmt_num(c.ending.other_left_on_read));

    if c.rally.max_len > 0 {
        let when = c.rally.when.as_deref().unwrap_or("");
        println!("│ 🔁 最长对线：{} 条（{}，约 {} 分钟）", fmt_num(c.rally.max_len), when, fmt_num(c.rally.duration_min));
    }

    if !c.signature.mine.is_empty() || !c.signature.theirs.is_empty() {
        let mk = |v: &[(String, i64)]| v.iter().map(|(w, n)| format!("{w}({n})")).collect::<Vec<_>>().join(" ");
        println!("│ 🗣️ 口癖：我 [{}] · ta [{}]", mk(&c.signature.mine), mk(&c.signature.theirs));
    }

    if c.haha.me_total + c.haha.them_total > 0 {
        println!("│ 😂 哈哈指数：我 {} · ta {}", fmt_num(c.haha.me_total), fmt_num(c.haha.them_total));
    }

    println!("│ 🥱 敷衍回复（嗯/哦/好…）：我 {} · ta {}", fmt_num(c.perfunctory.my_count), fmt_num(c.perfunctory.other_count));

    println!("│ 🗓️ 全年热力图：{}/{} 天有聊天（详细见 HTML 报告）", fmt_num(c.heatmap.active_days), fmt_num(c.heatmap.span_days));

    println!("╰─────────────────────────────────────────────────────────");
}

/// 基础类型码 → 中文标签。
fn type_label(t: i64) -> &'static str {
    match t {
        1 => "文本",
        3 => "图片",
        34 => "语音",
        42 => "名片",
        43 => "视频",
        47 => "表情",
        48 => "位置",
        49 => "文件",
        50 => "通话",
        10000 => "系统",
        _ => "其他",
    }
}

fn print_volume_report(v: &stats::VolumeStats, index: &ContactIndex) {
    println!("\n╭─ 微信消息统计 ──────────────────────────────────────────");
    println!("│ 会话总数      : {}", fmt_num(v.conversations as i64));
    println!("│ 消息总数      : {}", fmt_num(v.total_messages));

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
    println!("│   ├ 文本      : {:>14}  ({})", fmt_num(text), pct(text));
    println!("│   ├ 媒体      : {:>14}  ({})", fmt_num(media), pct(media));
    println!("│   ├ 系统      : {:>14}  ({})", fmt_num(system), pct(system));
    println!("│   └ 其它      : {:>14}  ({})", fmt_num(other), pct(other));
    println!(
        "│ 媒体拆分      : 图片 {} · 语音 {} · 视频 {} · 表情 {} · 文件 {} · 通话 {}",
        fmt_num(img), fmt_num(voice), fmt_num(video), fmt_num(sticker), fmt_num(file), fmt_num(call)
    );

    println!("│ 类型分布（基础类型码）：");
    for (t, n) in v.type_dist.iter().take(8) {
        println!("│   {:>6} {:<10} {:>12}", t, base_type_name(*t), fmt_num(*n));
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
            fmt_num(*n),
            fmt_num(*mine),
            fmt_num(*theirs),
            bar(*n, max_n, 16)
        );
    }
    println!("╰─────────────────────────────────────────────────────────");
}

fn print_temporal_report(t: &stats::TemporalStats) {
    let total: i64 = t.hour.iter().sum();
    println!("\n╭─ 活跃时段（本地时间）  共 {} 条 ──────────────────────", fmt_num(total));

    let hmax = *t.hour.iter().max().unwrap_or(&0);
    for h in 0..24 {
        println!("│ {:02}:00 ▏{:<28} {:>12}", h, bar(t.hour[h], hmax, 28), fmt_num(t.hour[h]));
    }

    println!("│");
    println!("│ 星期分布：");
    let days = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"];
    let wmax = *t.weekday.iter().max().unwrap_or(&0);
    for (i, d) in days.iter().enumerate() {
        println!("│ {} ▏{:<28} {:>12}", d, bar(t.weekday[i], wmax, 28), fmt_num(t.weekday[i]));
    }

    if !t.month.is_empty() {
        println!("│");
        println!("│ 月份趋势（最近 24 个月）：");
        let mmax = t.month.iter().map(|(_, n)| *n).max().unwrap_or(0);
        let start = t.month.len().saturating_sub(24);
        for (ym, n) in t.month.iter().skip(start) {
            println!("│   {} ▏{:<24} {:>12}", ym, bar(*n, mmax, 24), fmt_num(*n));
        }
    }
    println!("╰─────────────────────────────────────────────────────────");
}

fn print_dig_report(conv: &model::Conversation, d: &dig::ConversationDetail, index: &ContactIndex) {
    let name = index.name_by_username(&conv.username);
    let kind = if d.is_group { "群聊" } else { "单聊" };

    println!("\n╭─ 单会话深挖 · {} [{}] ──────────────────────────────", name, kind);
    println!("│ 消息总数      : {}", fmt_num(d.total));
    if let (Some(a), Some(b)) = (d.time_min, d.time_max) {
        println!("│ 时间跨度      : {} ~ {}", fmt_ts(a), fmt_ts(b));
    }

    // —— 发送比例 ——
    let denom = d.my_count + d.other_count;
    let pct = |x: i64| if denom > 0 { format!("{:.1}%", x as f64 / denom as f64 * 100.0) } else { "—".into() };
    let m = d.my_count.max(1);
    let o = d.other_count.max(1);
    let big = d.my_count.max(d.other_count).max(1);
    println!("│ 发送比例      : 我 {} ({})  ·  对方 {} ({})", fmt_num(d.my_count), pct(d.my_count), fmt_num(d.other_count), pct(d.other_count));
    println!("│   我   ▏{:<20} {}", bar(d.my_count, big, 20), fmt_num(d.my_count));
    println!("│   对方▏{:<20} {}", bar(d.other_count, big, 20), fmt_num(d.other_count));
    if m > 0 && o > 0 {
        let ratio = d.my_count as f64 / o as f64;
        println!("│   我:对方 ≈ {:.2} : 1", ratio);
    }

    // —— 类型对比 ——
    println!("│ 对方最常发的类型：");
    for (t, n) in d.other_type_dist.iter().take(5) {
        println!("│   {:<10} {:>10}  {}", base_type_name(*t), fmt_num(*n), bar(*n, d.other_count, 16));
    }
    println!("│ 我最常发的类型：");
    for (t, n) in d.my_type_dist.iter().take(5) {
        println!("│   {:<10} {:>10}  {}", base_type_name(*t), fmt_num(*n), bar(*n, d.my_count.max(1), 16));
    }

    // —— 回复时延 ——
    println!("│ 回复时延（切换发送者算一次回复，上限 6h）：");
    print_latency("│   我回对方", &d.my_reply);
    print_latency("│   对方回我", &d.other_reply);
    if d.capped_replies > 0 {
        println!("│   （另有 {} 次超过 6h 的续聊未计入）", fmt_num(d.capped_replies));
    }

    // —— 每日首发 ——
    let open_total = d.days_self_open + d.days_other_open;
    let open_pct = |x: i64| if open_total > 0 { format!("{:.1}%", x as f64 / open_total as f64 * 100.0) } else { "—".into() };
    println!("│ 每日谁先开口：我主动 {} 天 ({})  ·  对方先 {} 天 ({})",
        fmt_num(d.days_self_open), open_pct(d.days_self_open), fmt_num(d.days_other_open), open_pct(d.days_other_open));
    if !d.recent_openers.is_empty() {
        println!("│   最近 {} 天明细（最近在上）：", d.recent_openers.len());
        for (date, is_self, t) in d.recent_openers.iter().rev().take(14) {
            let who = if *is_self { "我" } else { "对方" };
            let hhmm = Local.timestamp_opt(*t, 0).single()
                .map(|dt| dt.format("%H:%M").to_string())
                .unwrap_or_else(|| "?".into());
            println!("│     {date}  {who} 先开口  {hhmm}");
        }
    }
    println!("╰─────────────────────────────────────────────────────────");
}

fn print_latency(label: &str, s: &dig::LatencyStats) {
    match (s.median_sec, s.mean_sec) {
        (Some(md), Some(mn)) => {
            println!("{}：中位 {} · 均值 {} · 共 {} 次", label, fmt_duration(md), fmt_duration(mn), fmt_num(s.samples));
        }
        _ => println!("{}：无样本", label),
    }
}

fn print_couple_report(conv: &model::Conversation, r: &couple::CoupleReport, index: &ContactIndex) {
    let name = index.name_by_username(&conv.username);
    let pct = |x: f64| format!("{:.1}%", x * 100.0);

    println!("\n╭─ 💑 情侣报告 · {} ─────────────────────────────────", name);

    // —— 时间叙事 ——
    if let Some(first) = &r.first_day {
        println!("│ 📅 故事开始于 {first}，今天是第 {} 天", fmt_num(r.nth_day_today));
    }
    println!(
        "│    共聊了 {} 条消息（我 {} · 对方 {}），分布在 {} / {} 天里（{}）",
        fmt_num(r.total), fmt_num(r.my_count), fmt_num(r.other_count),
        fmt_num(r.active_days), fmt_num(r.nth_day_today), pct(r.active_ratio)
    );

    println!("│");
    println!("│ 🔥 最长连续聊天：{} 天不间断", fmt_num(r.longest_streak));
    if r.current_streak > 0 {
        println!("│    当前已连续 {} 天 ✨", fmt_num(r.current_streak));
    } else {
        println!("│    （当前连续已中断）");
    }
    println!("│ 🤫 最长沉默：{} 天没说话", fmt_num(r.longest_silence_days));
    if let Some((a, b)) = &r.longest_silence_range {
        println!("│    （{a} 之后，直到 {b} 才再说话）");
    }

    if let Some((day, n)) = &r.peak_day {
        println!("│ 🎉 最嗨的一天：{day}，聊了 {} 条", fmt_num(*n));
    }
    println!("│ 🌙 一起熬过的夜：{} 条凌晨消息（0–4 点）", fmt_num(r.late_night_count));
    if r.call_count > 0 {
        println!("│ 📞 语音/视频通话：{} 次", fmt_num(r.call_count));
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
        println!("│ ✍️ 总字数：我 {} · 对方 {}", fmt_num(t.my_chars), fmt_num(t.other_chars));
        if let Some((chars, is_self, ts)) = &t.longest {
            let who = if *is_self { "我" } else { "对方" };
            let when = Local.timestamp_opt(*ts, 0).single()
                .map(|dt| dt.format("%Y-%m-%d").to_string())
                .unwrap_or_else(|| "?".into());
            println!("│ 📝 最长一条：{} 字（{} 发，{when}）", fmt_num(*chars), who);
        }
        if !t.keywords.is_empty() {
            println!("│ 💬 关键词（消息条数，我 / 对方）：");
            for (k, me, oth) in t.keywords.iter().take(10) {
                println!("│    {:<6} 我 {:>4} · 对方 {:>4}", k, fmt_num(*me), fmt_num(*oth));
            }
        }
    }

    // —— 高频词 ——
    if let Some(wf) = &r.words {
        println!("│");
        if !wf.overall.is_empty() {
            let cloud: Vec<String> = wf.overall.iter().map(|(w, c)| format!("{w}({})", fmt_num(*c))).collect();
            println!("│ ☁️ 总体高频词：{}", cloud.join("  "));
        }
        if !wf.mine.is_empty() {
            let cloud: Vec<String> = wf.mine.iter().map(|(w, c)| format!("{w}({})", fmt_num(*c))).collect();
            println!("│    你最常说：{}", cloud.join("  "));
        }
        if !wf.theirs.is_empty() {
            let cloud: Vec<String> = wf.theirs.iter().map(|(w, c)| format!("{w}({})", fmt_num(*c))).collect();
            println!("│    ta 最常说：{}", cloud.join("  "));
        }
    }

    // —— dig 深挖（类型分布 / 回复时延中位均值 / 每日首发）——
    if let Some(d) = &r.dig {
        let lat = |s: &dig::LatencyStats| match (s.median_sec, s.mean_sec) {
            (Some(md), Some(mn)) => {
                format!("中位 {} · 均值 {}（{} 次）", fmt_duration(md), fmt_duration(mn), fmt_num(s.samples))
            }
            _ => "无样本".to_string(),
        };
        let types = |dist: &[(i64, i64)]| {
            if dist.is_empty() {
                "—".to_string()
            } else {
                dist.iter()
                    .take(4)
                    .map(|(t, n)| format!("{} {}", type_label(*t), fmt_num(*n)))
                    .collect::<Vec<_>>()
                    .join("  ")
            }
        };
        println!("│");
        println!("│ 🔎 深挖");
        println!("│    回复时延：我 {} · ta {}", lat(&d.my_reply), lat(&d.other_reply));
        println!("│    每日首发：我开口 {} 天 · ta 开口 {} 天", fmt_num(d.days_self_open), fmt_num(d.days_other_open));
        println!("│    我常发：{}", types(&d.my_type_dist));
        println!("│    ta常发：{}", types(&d.other_type_dist));
    }

    // —— 趋势曲线 ——
    if let Some(t) = &r.trend {
        if t.points.len() >= 2 {
            let counts: Vec<i64> = t.points.iter().map(|p| p.count).collect();
            let spark = trend::sparkline(&counts);
            let first = t.points.first().unwrap().label.as_str();
            let last = t.points.last().unwrap().label.as_str();
            println!("│");
            println!("│ 📈 趋势（按{}，共 {} 个，{tag}）", t.kind, fmt_num(t.points.len() as i64), tag = t.tag);
            println!("│    {spark}");
            println!("│    {first} ─────────────────→ {last}");
            if let Some(p) = &t.peak {
                println!("│    最热烈的一{}：{}（{} 条）", t.kind, p.label, fmt_num(p.count));
            }
        }
    }

    println!("╰─────────────────────────────────────────────────────────");
}
struct ContactIndex {
    by_username: HashMap<String, Contact>,
}

impl ContactIndex {
    fn build(data: &WeChatData) -> Result<Self> {
        let contacts = data.load_contacts()?;
        let by_username = contacts.into_iter().map(|c| (c.username.clone(), c)).collect();
        Ok(Self { by_username })
    }

    /// 显示名（命中用 remark/nick_name，否则脱敏 username）。
    fn name_by_username(&self, username: &str) -> String {
        match self.by_username.get(username) {
            Some(c) => c.display_name().to_string(),
            None => mask_user(username),
        }
    }
}

/// 解析定位会话的三种方式：--with（子串模糊）/ --rank（按消息量名次）/ --user（精确）。
fn locate_conversation(
    data: &WeChatData,
    convs: &[model::Conversation],
    index: &ContactIndex,
    with: Option<&str>,
    rank: Option<usize>,
    user: Option<&str>,
) -> Result<model::Conversation> {
    if let Some(name) = user {
        return convs
            .iter()
            .find(|c| c.username == name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("--user 未命中：{name}"));
    }
    if let Some(query) = with {
        let q = query.to_lowercase();
        let hits: Vec<&model::Conversation> = convs
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
        let mut paired: Vec<(&model::Conversation, i64)> =
            convs.iter().zip(counts.iter().copied()).collect();
        paired.sort_by(|a, b| b.1.cmp(&a.1));
        return paired
            .get(n - 1)
            .map(|(c, _)| (*c).clone())
            .ok_or_else(|| anyhow::anyhow!("--rank {n} 超出范围（共 {} 个会话）", convs.len()));
    }
    bail!("请用 --with <昵称片段> / --rank N / --user <wxid> 之一指定目标会话");
}

/// 构造一个仿 WeChat 4.x schema 的临时 SQLite 库，插入少量样本，然后探测它。
fn run_selftest() -> Result<PathBuf> {
    use rusqlite::Connection;

    let path = std::env::temp_dir().join("wechat_statistics_selftest.db");
    let _ = std::fs::remove_file(&path);
    let conn = Connection::open(&path)?;

    conn.execute_batch(
        r#"
        -- 主消息表 + 两张结构完全相同的「按会话分表」（模拟微信真实存储）
        CREATE TABLE Message (
            mesLocalId   INTEGER PRIMARY KEY,
            talkerId     INTEGER,
            mesType      INTEGER,
            mesCreateTime INTEGER,
            mesContent   TEXT
        );
        CREATE TABLE Message_conv_abc (
            mesLocalId   INTEGER PRIMARY KEY,
            talkerId     INTEGER,
            mesType      INTEGER,
            mesCreateTime INTEGER,
            mesContent   TEXT
        );
        CREATE TABLE Message_conv_def (
            mesLocalId   INTEGER PRIMARY KEY,
            talkerId     INTEGER,
            mesType      INTEGER,
            mesCreateTime INTEGER,
            mesContent   TEXT
        );
        CREATE TABLE Name2Id ( usrName TEXT, id INTEGER );
        CREATE TABLE Friend  ( id INTEGER PRIMARY KEY, usrName TEXT, nickName TEXT );
        "#,
    )?;

    // 1700000000 = 2023-11-14 22:13:20 UTC
    conn.execute(
        "INSERT INTO Message (talkerId, mesType, mesCreateTime, mesContent) VALUES (?, ?, ?, ?)",
        rusqlite::params![1i64, 1i64, 1700000000i64, "在吗？周末一起爬山"],
    )?;
    conn.execute(
        "INSERT INTO Message (talkerId, mesType, mesCreateTime, mesContent) VALUES (?, ?, ?, ?)",
        rusqlite::params![1i64, 3i64, 1700003600i64, "[图片]"],
    )?;
    conn.execute(
        "INSERT INTO Message (talkerId, mesType, mesCreateTime, mesContent) VALUES (?, ?, ?, ?)",
        rusqlite::params![2i64, 49i64, 1700100000i64, "<msg><file>report.pdf</file></msg>"],
    )?;
    conn.execute(
        "INSERT INTO Name2Id (usrName, id) VALUES (?, ?), (?, ?)",
        rusqlite::params!["wxid_abc", 1i64, "wxid_def", 2i64],
    )?;
    conn.execute(
        "INSERT INTO Friend (id, usrName, nickName) VALUES (?, ?, ?), (?, ?, ?)",
        rusqlite::params![1i64, "wxid_abc", "张三", 2i64, "wxid_def", "老王"],
    )?;
    drop(conn); // 关闭连接后再以只读方式探测

    let opts = ProbeOptions::default();
    probe_path(&path, &opts)?;
    Ok(path)
}
