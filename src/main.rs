mod loader;
mod model;
mod probe;
mod schema;
mod stats;

use anyhow::Result;
use chrono::{Local, TimeZone};
use clap::{Parser, Subcommand};
use loader::WeChatData;
use probe::{ProbeOptions, probe_path};
use serde::Serialize;
use stats::{TemporalAccum, VolumeAccum};
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
                    "  {:>2}. [{}] {:<22} {:>8} 条  ({}.{})",
                    i + 1,
                    conv_kind(&c.username),
                    mask_user(&c.username),
                    n,
                    c.db_stem,
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
            println!("✓ 共 {} 个会话，开始跨会话聚合…（纯 SQL，不读正文）", convs.len());

            // 单次遍历，同时喂入两个累加器：消息量 + 时段分布。
            let mut vol = VolumeAccum::new(top);
            let mut tmp = TemporalAccum::new();
            data.for_each_conversation(&convs, |conn, c| {
                vol.observe_table(conn, c)?;
                tmp.observe_table(conn, c)?;
                Ok(())
            })?;

            let vol_stats = vol.finalize(convs.len());
            let tmp_stats = tmp.finalize();

            print_volume_report(&vol_stats);
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

fn print_volume_report(v: &stats::VolumeStats) {
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

    println!("│ 消息数 Top {} 会话：", v.top.len());
    let max_n = v.top.iter().map(|(_, n)| *n).max().unwrap_or(0);
    for (i, (u, n)) in v.top.iter().enumerate() {
        println!(
            "│ {:>2}. [{}] {:<16} {:>12}  {}",
            i + 1,
            conv_kind(u),
            mask_user(u),
            fmt_num(*n),
            bar(*n, max_n, 20)
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
