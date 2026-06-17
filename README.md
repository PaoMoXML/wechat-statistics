# wechat-statistics

个人微信消息统计工具 —— 读取**已解密**的微信 4.x 明文数据库，做全局统计、单会话深挖与可视化报告。Rust 单二进制 CLI，全程只读，绝不动你的原始数据。

> 本工具**不负责解密**。微信 4.x 的库是 SQLCipher 4 加密的，请先用 [`wechat-dump-rs`](https://github.com/0xlane/wechat-dump-rs) 或 [`chatlog`](https://github.com/sjzar/chatlog) 从微信进程内存提取密钥并解密，拿到明文 `.db` 目录后再交给本工具。

## 功能一览

- **Schema 探测**（Phase 0）：离线探测任意已解密 SQLite 库的表结构、采样行，输出 JSON 供适配层校验。
- **全局统计**（`stats`）：跨所有会话的消息量、类型分布、媒体拆分、活跃时段 —— 全程纯 SQL 聚合，不读取正文。
- **单会话深挖**（`dig`）：发送比例、类型对比、回复时延（中位 / 均值）、每日谁先开口。
- **情侣报告**（`couple`）：在一起第 N 天、最长连续、最长沉默、最嗨一天、熬夜时段、通话、秒回率、高频词（jieba 分词）、按周 / 月趋势曲线，以及叙事彩蛋。可输出**自包含 HTML 仪表盘**与 **PPT 风格翻页幻灯片**。
- **隐私优先**：终端打印一律脱敏（wxid 打码），HTML 报告完全离线、不联网、不上传。

## 前置条件

1. **Rust 工具链**：edition 2024，需 Rust ≥ 1.85。
2. **已解密的数据目录**：一个以**你自己的 wxid 命名**的文件夹，内含 `contact.db`、`session.db`、`message_*.db` / `biz_message_*.db` 等（由解密工具产出）。目录名会被当作「我」的 wxid，用于区分收发双方。

## 构建

```bash
cargo build --release
# 产物：target/release/wechat-statistics
```

`rusqlite` 启用了 `bundled` 特性，无需系统预装 SQLite。

## 快速开始

```bash
# 0) 看一眼数据目录结构对不对（只读，不碰正文）
cargo run --release -- inspect --data /path/to/<你的wxid>

# 1) 全局统计：消息量 / 类型分布 / 活跃时段
cargo run --release -- stats --data /path/to/<你的wxid> --top 20

# 2) 单会话深挖（按昵称/备注片段定位）
cargo run --release -- dig --data /path/to/<你的wxid> --with "老王"

# 3) 情侣报告 + HTML 仪表盘
cargo run --release -- couple --data /path/to/<你的wxid> --rank 1 \
    --html report.html --slides slides.html --json report.json
```

## 命令详解

所有读取命令均带 `--data <目录>`，并支持三种会话定位方式（互斥，按需选一）：

| 参数 | 说明 |
|---|---|
| `--with <片段>` | 按昵称 / 备注 / 用户名模糊匹配，唯一命中即用；多个则列出候选并退出 |
| `--rank <N>` | 按全局消息量第 N 名定位（`1` = 消息最多的会话） |
| `--user <wxid>` | 精确用户名（wxid / 群 ID `xxx@chatroom`）定位 |

### `schema` — 表结构探测（Phase 0）

```bash
wechat-statistics schema --db <单个.db 或目录> [--json out.json] \
    [--no-count] [--no-samples] [--limit 3] [--max-table-names 50]
```

- `--no-count`：跳过 `COUNT(*)`，GB 级大库可显著加速。
- `--no-samples`：只要结构、不要数据，JSON 体积最小。
- 结构相同的表会自动分组合并，`--max-table-names` 控制每组保留的表名数量。

### `selftest` — 自检

无需本机微信即可验证整条探测流程：构造一个仿微信 schema 的临时库、插入样本、再探测它。

```bash
wechat-statistics selftest
```

### `inspect` — 解析骨架自检（Phase 1）

解析联系人 / 会话映射，批量统计消息数并打印 Top N，再对消息最多的会话做聚合。**不读取任何正文**。

```bash
wechat-statistics inspect --data <目录> [--top 10]
```

### `stats` — 全局统计

跨所有会话聚合消息量、类型分布、媒体拆分与活跃时段。单次遍历，纯 SQL。

```bash
wechat-statistics stats --data <目录> [--top 10] [--json out.json]
```

### `dig` — 单会话深挖

发送比例、类型对比、回复时延（中位 / 均值）、每日首发。

```bash
wechat-statistics dig --data <目录> --with "老王" [--json out.json]
```

### `couple` — 情侣报告

叙事化统计 + 趣味彩蛋 + 可视化。

```bash
wechat-statistics couple --data <目录> --rank 1 \
    [--words 12] [--html report.html] [--slides slides.html] [--json out.json]
```

- `--words N`：高频词每组返回个数（`0` 关闭，默认 12，用 jieba-rs 中文分词）。
- `--html`：自包含 HTML 仪表盘（周 ↔ 月趋势切换）。
- `--slides`：PPT 风格翻页 HTML。
- `--json`：完整结构化报告。

> 默认 `.gitignore` 已忽略 `*.html`，生成的报告不会被误提交。

## 工作原理

```
contact.db / session.db / message_*.db （已解密明文 SQLite）
        │
        │  schema.rs：列名映射 + 运行时列校验（版本兼容）
        ▼
统一模型  Contact / Conversation / MessageFact / TextMessage
        │
        │  loader.rs：跨分片合并、sender_id 归一化（0=我 / 1=对方）
        ▼
统计引擎  stats（全局）/ dig（深挖）/ couple（叙事）/ trend（趋势）/ lexical（词频）
        │
        ▼
终端报告 · JSON · 自包含 HTML 仪表盘
```

**几个关键设计：**

- **跨分片合并**：一个会话的消息可能按时间分片到多个 `message_*.db`，表名同为 `Msg_{md5(username)}`。读取时遍历全部分片并按 `create_time` 全局排序合并。详见 [loader.rs](src/loader.rs)。
- **发送者归一化**：`real_sender_id` 是 per-DB 的 `Name2Id.rowid`（不是 `contact.id`），跨分片不可直接比较。loader 在读取时按各分片的 `self_rowid` 统一归一化为 `0=我 / 1=对方`。
- **正文按需加载**：聚合统计走纯 SQL，绝不把正文拉进内存；只有 `dig` / `couple` 这类需要文本的场景才解压读取（zstd）。
- **运行时 schema 校验**：所有读取都走 [schema.rs](src/schema.rs) 的列名映射并在运行时校验，微信小版本结构变动时能及早失败、给出清晰错误。

## 项目状态

- ✅ Phase 0：schema 探测 + 自检
- ✅ Phase 1：解析骨架、全局统计、单会话深挖、情侣报告、HTML 仪表盘
- ⏳ Phase 2：把密钥提取与 SQLCipher 解密内置进单一二进制（当前依赖外部工具产出明文库）

## 许可

仅供个人对自己聊天记录的统计分析与备份研究使用。请遵守当地法律与微信用户协议。
