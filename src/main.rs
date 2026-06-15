mod loader;
mod model;
mod probe;
mod schema;

use anyhow::Result;
use chrono::{Local, TimeZone};
use clap::{Parser, Subcommand};
use loader::WeChatData;
use probe::{ProbeOptions, probe_path};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "wechat-statistics",
    version,
    about = "个人微信消息统计工具 — Phase 0(schema 探测) + 解析骨架(inspect)"
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
