//! 全局统计引擎：跨所有会话聚合消息量与时段分布。
//!
//! 设计：每个统计维度是一个「在线累加器」——遍历消息表时 `observe_table` 喂入，
//! 最后 `finalize` 出结果。这样多个维度能共用同一次遍历（见 main 的 `stats` 子命令），
//! 避免对海量消息表反复扫描。全部走 SQL 聚合，正文内容从不进入内存。

pub mod temporal;
pub mod volume;

pub use temporal::{TemporalAccum, TemporalStats};
pub use volume::{VolumeAccum, VolumeStats};
