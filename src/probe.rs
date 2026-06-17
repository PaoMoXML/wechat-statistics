//! Phase 0：已解密 SQLite 库的表结构探测。
//!
//! 目标是把 WeChat 4.x 的真实表结构摸清楚，为 Phase 1 的 `schema.rs` 适配层提供事实依据。
//! 绝不硬编码列名——这里只做「如实 dump」，列名映射留给下一阶段。
//!
//! 关键优化：微信会为每个会话/联系人单独建一张结构完全相同的消息表，一个账号可达数千张。
//! 本探测按「表结构指纹」分组去重，只对每种结构的代表表做采样/计数，把数千张重复表坍缩成一项，
//! 既大幅缩小 JSON 体积，又免去对每张表跑 `COUNT(*)` 的开销。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{Local, TimeZone, Utc};
use rusqlite::{Connection, OpenFlags, types::ValueRef};
use serde::Serialize;

#[derive(Debug)]
pub struct ProbeOptions {
    /// 跳过 `SELECT COUNT(*)`（大库扫描慢，可关闭）。
    pub no_count: bool,
    /// 跳过采样行（只要结构、不要数据，JSON 最小）。
    pub no_samples: bool,
    /// 每个表(代表表)的采样行数。
    pub sample_limit: usize,
    /// 文本字段截断长度（字符）。
    pub text_truncate: usize,
    /// JSON 中每组最多保留多少个表名（结构相同时名字是冗余的，只取少量看命名规律即可）。
    pub max_table_names: usize,
}

impl Default for ProbeOptions {
    fn default() -> Self {
        Self {
            no_count: false,
            no_samples: false,
            sample_limit: 3,
            text_truncate: 200,
            max_table_names: 50,
        }
    }
}

/// 一份库的探测报告（可序列化为 JSON，供后续 schema 适配层读取）。
#[derive(Serialize)]
pub struct ProbeReport {
    pub path: String,
    pub table_count: usize,
    pub groups: Vec<TableGroup>,
}

/// 一种被若干张表共享的结构。
#[derive(Serialize)]
pub struct TableGroup {
    /// 共享该结构的表总数。
    pub shared_by: usize,
    /// 表名样本（已按 max_table_names 截断，真实数量见 shared_by）。
    pub table_names: Vec<String>,
    pub columns: Vec<ColumnReport>,
    /// 代表表的行数。
    pub row_count: Option<i64>,
    /// 代表表的采样行：每行是 (列名, 值字符串) 的列表。
    pub samples: Vec<Vec<(String, String)>>,
    /// 代表表的时间戳语义猜测。
    pub time_columns: Vec<TimeColumnGuess>,
}

#[derive(Serialize, Clone)]
pub struct ColumnReport {
    pub name: String,
    pub sql_type: String,
    pub not_null: bool,
    pub pk: bool,
}

#[derive(Serialize)]
pub struct TimeColumnGuess {
    pub column: String,
    pub unit: &'static str,
    pub sample_values: Vec<i64>,
    pub interpreted: Vec<String>,
}

/// 探测单个文件或目录下所有 `.db` / `.sqlite` 文件。
/// 过程中向 stdout 打印人类可读结果，并返回结构化报告。
pub fn probe_path(path: &Path, opts: &ProbeOptions) -> Result<Vec<ProbeReport>> {
    let mut reports = Vec::new();
    if path.is_dir() {
        let mut files: Vec<PathBuf> = std::fs::read_dir(path)
            .with_context(|| format!("读取目录失败: {}", path.display()))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.extension().map_or(false, |ext| {
                    ext.eq_ignore_ascii_case("db") || ext.eq_ignore_ascii_case("sqlite")
                })
            })
            .collect();
        files.sort();
        if files.is_empty() {
            bail!("目录中没有 .db/.sqlite 文件: {}", path.display());
        }
        for f in files {
            reports.push(probe_file(&f, opts)?);
        }
    } else {
        reports.push(probe_file(path, opts)?);
    }
    Ok(reports)
}

fn probe_file(path: &Path, opts: &ProbeOptions) -> Result<ProbeReport> {
    println!("\n========== {} ==========", path.display());
    let conn = open_readonly(path)?;

    let table_names = list_tables(&conn)?;
    let table_count = table_names.len();
    println!("共 {table_count} 张表，正在按结构指纹归纳…");

    // 按结构指纹分组：sig -> (代表列结构, 全部表名)
    let mut groups: BTreeMap<String, (Vec<ColumnReport>, Vec<String>)> = BTreeMap::new();
    for name in &table_names {
        let cols = list_columns(&conn, name)?;
        let sig = signature(&cols);
        let entry = groups.entry(sig).or_insert_with(|| (cols.clone(), Vec::new()));
        entry.1.push(name.clone());
    }

    // 按成员数降序，被大量共享的结构排在最前（通常就是消息表）。
    let mut groups_vec: Vec<(Vec<ColumnReport>, Vec<String>)> = groups.into_values().collect();
    groups_vec.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

    println!("归纳为 {} 种结构\n", groups_vec.len());

    let total = groups_vec.len();
    let mut out_groups = Vec::with_capacity(total);
    for (i, (cols, names)) in groups_vec.iter().enumerate() {
        out_groups.push(probe_group(&conn, i, total, cols, names, opts)?);
    }

    Ok(ProbeReport { path: path.display().to_string(), table_count, groups: out_groups })
}

fn probe_group(
    conn: &Connection,
    idx: usize,
    total: usize,
    cols: &[ColumnReport],
    names: &[String],
    opts: &ProbeOptions,
) -> Result<TableGroup> {
    let rep = &names[0];
    println!(
        "---- 结构 {}/{} ：被 {} 张表共享，代表表 [{}] ----",
        idx + 1,
        total,
        names.len(),
        rep
    );

    // stdout 只展示前若干个表名 + 省略提示。
    let preview: Vec<&str> = names.iter().take(10).map(|s| s.as_str()).collect();
    if names.len() > 10 {
        println!("  表名(前 10，共 {}): {} …", names.len(), preview.join(", "));
    } else {
        println!("  表名: {}", preview.join(", "));
    }

    for c in cols {
        let mut flags = String::new();
        if c.pk { flags.push_str(" PK"); }
        if c.not_null { flags.push_str(" NOTNULL"); }
        println!("  · {:<24} {:<12}{flags}", c.name, c.sql_type);
    }

    let row_count = if opts.no_count {
        println!("  代表表行数: (已跳过 --no-count)");
        None
    } else {
        let n = count_rows(conn, rep)?;
        println!("  代表表行数: {n}");
        Some(n)
    };

    let samples = if opts.no_samples {
        println!("  采样: (已跳过 --no-samples)");
        Vec::new()
    } else {
        let s = sample_rows(conn, rep, opts)?;
        for (i, row) in s.iter().enumerate() {
            let pairs: Vec<String> = row.iter().map(|(k, v)| format!("{k}={v}")).collect();
            println!("  样本[{i}]: {}", pairs.join("  "));
        }
        s
    };

    let time_columns = analyze_time_columns(conn, rep, cols, opts);
    for g in &time_columns {
        println!("  ⏱  {} 疑似「{}」时间戳; 样例: {}", g.column, g.unit, g.interpreted.join("; "));
    }
    println!();

    let table_names = names.iter().take(opts.max_table_names).cloned().collect();

    Ok(TableGroup { shared_by: names.len(), table_names, columns: cols.to_vec(), row_count, samples, time_columns })
}

/// 以只读方式打开。若库仍处于加密状态，给出明确提示。
fn open_readonly(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("打开数据库失败: {}", path.display()))?;

    // 加密库（SQLCipher）能被 open，但首次查询会报 SQLITE_NOTADB。
    match conn.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(())) {
        Ok(()) => Ok(conn),
        Err(e) => bail!(
            "无法读取 {} —— 这通常是「数据库仍处于加密状态」。\n\
             错误: {e}\n\
             请先用 wechat-dump-rs / chatlog 解密后再探测。",
            path.display()
        ),
    }
}

fn list_tables(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT name FROM sqlite_master \
         WHERE type='table' AND name NOT LIKE 'sqlite_%' \
         ORDER BY name",
    )?;
    let names = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(names)
}

fn list_columns(conn: &Connection, table: &str) -> Result<Vec<ColumnReport>> {
    let q = format!("PRAGMA table_info({})", quote_ident(table));
    let mut stmt = conn.prepare(&q)?;
    let rows = stmt.query_map([], |r| {
        Ok(ColumnReport {
            name: r.get::<_, String>(1)?,
            sql_type: r.get::<_, String>(2)?,
            not_null: r.get::<_, i64>(3)? != 0,
            pk: r.get::<_, i64>(5)? != 0,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| anyhow::anyhow!("读取列信息失败 ({table}): {e}"))
}

fn count_rows(conn: &Connection, table: &str) -> Result<i64> {
    let q = format!("SELECT count(*) FROM {}", quote_ident(table));
    conn.query_row(&q, [], |r| r.get(0))
        .map_err(|e| anyhow::anyhow!("COUNT 失败 ({table}): {e}"))
}

fn sample_rows(conn: &Connection, table: &str, opts: &ProbeOptions) -> Result<Vec<Vec<(String, String)>>> {
    let q = format!("SELECT * FROM {} LIMIT {}", quote_ident(table), opts.sample_limit);
    let mut stmt = conn.prepare(&q)?;
    let col_names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let rows = stmt.query_map([], |row| {
        let mut kv = Vec::with_capacity(col_names.len());
        for (i, col) in col_names.iter().enumerate() {
            let v = row.get_ref(i)?;
            kv.push((col.clone(), render_value(v, opts.text_truncate)));
        }
        Ok(kv)
    })?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| anyhow::anyhow!("采样失败 ({table}): {e}"))
}

/// 对名字含 time/create/stamp/date 的整型列，尝试按 Unix 秒/毫秒解释样本值，
/// 用来确认 WeChat `mesCreateTime` 到底是秒还是毫秒、是否带 2000 年偏移。
fn analyze_time_columns(
    conn: &Connection,
    table: &str,
    columns: &[ColumnReport],
    opts: &ProbeOptions,
) -> Vec<TimeColumnGuess> {
    let mut out = Vec::new();
    for c in columns {
        if !is_time_candidate(&c.name) || !c.sql_type.to_ascii_uppercase().contains("INT") {
            continue;
        }
        let q = format!(
            "SELECT {} FROM {} LIMIT {}",
            quote_ident(&c.name),
            quote_ident(table),
            opts.sample_limit.max(5)
        );
        let Ok(raws) = conn
            .prepare(&q)
            .and_then(|mut s| {
                s.query_map([], |r| r.get::<_, i64>(0))
                    .map(|it| it.filter_map(|x| x.ok()).collect::<Vec<_>>())
            })
        else {
            continue;
        };
        if raws.is_empty() {
            continue;
        }
        let mut interpreted = Vec::new();
        let mut unit = "未知";
        for v in &raws {
            if let Some((u, s)) = interpret_ts(*v) {
                unit = u;
                interpreted.push(format!("{v} → {s}"));
            }
        }
        if interpreted.is_empty() {
            interpreted.push(format!("{} 无法解释为合理时间戳（是否带 2000 年偏移？）", raws[0]));
        }
        out.push(TimeColumnGuess {
            column: c.name.clone(),
            unit,
            sample_values: raws,
            interpreted,
        });
    }
    out
}

fn is_time_candidate(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["time", "create", "stamp", "date"].iter().any(|kw| n.contains(kw))
}

/// 合理 Unix 时间戳范围：2000-01-01 ~ 2100-01-01。
fn interpret_ts(v: i64) -> Option<(&'static str, String)> {
    const MIN_SEC: i64 = 946_684_800; // 2000-01-01
    const MAX_SEC: i64 = 4_102_444_800; // 2100-01-01
    let try_secs = |s: i64, label: &'static str| -> Option<(&'static str, String)> {
        if (MIN_SEC..=MAX_SEC).contains(&s) {
            let utc = Utc.timestamp_opt(s, 0).single()?;
            let local = utc.with_timezone(&Local);
            Some((
                label,
                format!(
                    "{} | 本地 {}",
                    utc.format("%Y-%m-%d %H:%M:%S"),
                    local.format("%Y-%m-%d %H:%M:%S %:z")
                ),
            ))
        } else {
            None
        }
    };
    try_secs(v, "Unix秒")
        .or_else(|| (v > 1_000_000_000_000).then(|| v / 1000).and_then(|s| try_secs(s, "Unix毫秒")))
}

/// 结构指纹：列(名+类型+约束)排序后拼接。结构相同的表（无论列顺序、表名）指纹一致。
fn signature(cols: &[ColumnReport]) -> String {
    let mut entries: Vec<String> = cols
        .iter()
        .map(|c| {
            format!(
                "{}:{}{}{}",
                c.name,
                c.sql_type,
                if c.pk { 'P' } else { '-' },
                if c.not_null { 'N' } else { '-' }
            )
        })
        .collect();
    entries.sort();
    entries.join(",")
}

fn render_value(v: ValueRef, max_chars: usize) -> String {
    match v {
        ValueRef::Null => "NULL".to_string(),
        ValueRef::Integer(i) => i.to_string(),
        ValueRef::Real(f) => format!("{f}"),
        ValueRef::Text(b) => {
            let s = String::from_utf8_lossy(b).to_string();
            crate::fmt::truncate(&s, max_chars)
        }
        ValueRef::Blob(b) => format!("<BLOB {} bytes>", b.len()),
    }
}

/// 给标识符加双引号并转义，避免奇怪表名/列名导致 SQL 解析失败。
fn quote_ident(name: &str) -> String {
    let escaped = name.replace('"', "\"\"");
    format!("\"{escaped}\"")
}
