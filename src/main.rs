mod content;
mod couple;
mod curios;
mod dig;
mod fmt;
mod lexical;
mod loader;
mod model;
mod probe;
mod render;
mod report;
mod schema;
mod stats;
mod trend;

use anyhow::Result;
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
                    report::conv_kind(&c.username),
                    report::mask_user(&c.username),
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
                    println!("  时间跨度      : {} ~ {}", fmt::fmt_ts(a), fmt::fmt_ts(b));
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
            let index = report::ContactIndex::build(&data)?;
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

            report::print_volume_report(&vol_stats, &index);
            report::print_temporal_report(&tmp_stats);

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
            let index = report::ContactIndex::build(&data)?;

            // 定位目标会话
            let conv = report::locate_conversation(&data, &convs, &index, with.as_deref(), rank, user.as_deref())?;
            println!("✓ 目标会话：{} [{}] ({})", index.name_by_username(&conv.username), report::conv_kind(&conv.username), conv.username);

            // 读取时序事实（跨全部分片，sender_id 已归一化 0=我 / 1=对方）→ 深挖
            let facts = data.message_facts(&conv)?;
            let is_group = conv.username.contains("@chatroom");
            let detail = dig::compute(&facts, 0, is_group);

            report::print_dig_report(&conv, &detail, &index);

            if let Some(json_path) = json {
                std::fs::write(&json_path, serde_json::to_string_pretty(&detail)?)?;
                println!("\n✓ 深挖结果已写入 {}", json_path.display());
            }
        }
        Command::Couple { data, with, rank, user, words, html, slides, json } => {
            let data = WeChatData::open(&data)?;
            let convs = data.load_conversations()?;
            let index = report::ContactIndex::build(&data)?;

            let conv = report::locate_conversation(&data, &convs, &index, with.as_deref(), rank, user.as_deref())?;
            let name = index.name_by_username(&conv.username).to_string();
            println!("✓ 目标会话：{} [{}] ({})", name, report::conv_kind(&conv.username), conv.username);

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

            report::print_couple_report(&conv, &report, &index);
            report::print_curios(&curios);

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
