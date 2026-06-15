//! 词频 / 词云数据：基于 jieba-rs 中文分词 + 内置停用词表。
//!
//! 输入已解压的 `TextMessage` 序列，输出总体 / 我 / 对方 三组高频词，供报告与词云使用。
//! jieba 初始化（加载内置词典）较重，用 `OnceLock` 全局只做一次。

use std::collections::HashMap;
use std::sync::OnceLock;

use jieba_rs::Jieba;
use serde::Serialize;

use crate::model::TextMessage;

#[derive(Serialize)]
pub struct WordFreq {
    /// 总体高频词（降序）。
    pub overall: Vec<(String, i64)>,
    /// 我最常说（降序）。
    pub mine: Vec<(String, i64)>,
    /// 对方最常说（降序）。
    pub theirs: Vec<(String, i64)>,
}

/// 计算高频词。`top_n` 控制每组返回多少个；`self_rowid` 给定时才拆分我/对方。
pub fn word_freq(texts: &[TextMessage], self_rowid: Option<i64>, top_n: usize) -> WordFreq {
    let is_self = |s: i64| self_rowid.map_or(false, |me| s == me);
    let jb = jieba();
    let stop = stopwords();

    let mut overall: HashMap<String, i64> = HashMap::new();
    let mut mine: HashMap<String, i64> = HashMap::new();
    let mut theirs: HashMap<String, i64> = HashMap::new();

    for tm in texts {
        let Some(ref text) = tm.text else { continue };
        let mine_side = is_self(tm.sender_id);
        let bucket = if mine_side { &mut mine } else { &mut theirs };
        for w in jb.cut(text, true) {
            let key = w.trim().to_lowercase();
            if !keep(&key, stop) {
                continue;
            }
            *overall.entry(key.clone()).or_insert(0) += 1;
            *bucket.entry(key).or_insert(0) += 1;
        }
    }

    WordFreq {
        overall: top(&overall, top_n),
        mine: top(&mine, top_n),
        theirs: top(&theirs, top_n),
    }
}

fn top(map: &HashMap<String, i64>, n: usize) -> Vec<(String, i64)> {
    let mut v: Vec<(String, i64)> = map.iter().map(|(k, c)| (k.clone(), *c)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    v.truncate(n);
    v
}

/// 各自「专属词」：一方常用（>=3 次）且另一方几乎不用（< 该侧的 20%）。
/// 返回 (我的专属词, ta 的专属词)，各按次数降序、截断到 `top_n`。
pub fn signature_words(
    texts: &[TextMessage],
    self_rowid: Option<i64>,
    top_n: usize,
) -> (Vec<(String, i64)>, Vec<(String, i64)>) {
    let is_self = |s: i64| self_rowid.map_or(false, |me| s == me);
    let jb = jieba();
    let stop = stopwords();
    let mut mine: HashMap<String, i64> = HashMap::new();
    let mut theirs: HashMap<String, i64> = HashMap::new();
    for tm in texts {
        let Some(ref text) = tm.text else { continue };
        let bucket = if is_self(tm.sender_id) { &mut mine } else { &mut theirs };
        for w in jb.cut(text, true) {
            let k = w.trim().to_lowercase();
            if !keep(&k, stop) {
                continue;
            }
            *bucket.entry(k).or_insert(0) += 1;
        }
    }
    let sig = |a: &HashMap<String, i64>, b: &HashMap<String, i64>| -> Vec<(String, i64)> {
        let mut v: Vec<(String, i64)> = Vec::new();
        for (w, c) in a.iter() {
            // w: &String, c: &i64 —— 显式遍历，避免闭包模式匹配的借用歧义。
            if *c >= 3 && (*b.get(w).unwrap_or(&0) as f64) < (*c as f64 * 0.2) {
                v.push((w.clone(), *c));
            }
        }
        v.sort_by(|x, y| y.1.cmp(&x.1));
        v.truncate(top_n);
        v
    };
    (sig(&mine, &theirs), sig(&theirs, &mine))
}

/// 是否计入词频：去停用词、纯标点/空白、纯数字、单 ASCII 字符噪音。
fn keep(token: &str, stop: &std::collections::HashSet<&'static str>) -> bool {
    if token.is_empty() || stop.contains(token) {
        return false;
    }
    if !token.chars().any(|c| c.is_alphanumeric()) {
        return false;
    }
    if token.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    if token.chars().count() == 1 && token.chars().next().unwrap().is_ascii() {
        return false;
    }
    true
}

fn jieba() -> &'static Jieba {
    static JIEBA: OnceLock<Jieba> = OnceLock::new();
    JIEBA.get_or_init(Jieba::new)
}

fn stopwords() -> &'static std::collections::HashSet<&'static str> {
    static STOP: OnceLock<std::collections::HashSet<&'static str>> = OnceLock::new();
    STOP.get_or_init(|| STOPWORDS.iter().copied().collect())
}

/// 内置中文停用词表（虚词/代词/语气/常见无义单字）。非穷尽，可后续外置成文件。
const STOPWORDS: &[&str] = &[
    "的", "了", "是", "在", "我", "你", "他", "她", "它", "们", "这", "那", "也", "都", "就",
    "还", "又", "才", "只", "而", "与", "及", "或", "但", "可", "能", "会", "要", "把", "被",
    "让", "使", "给", "对", "和", "跟", "向", "从", "到", "为", "于", "以", "之", "其", "此",
    "个", "么", "吗", "呢", "啊", "呀", "哦", "哈", "嘛", "吧", "呗", "嗯", "呃", "哇", "哎",
    "不", "没", "无", "有", "一", "二", "三", "个", "上", "下", "里", "中", "后", "前", "已",
    "再", "很", "太", "真", "好", "多", "少", "大", "小", "去", "来", "看", "说", "想", "做",
    "着", "过", "地", "得", "吧", "的话", "一个", "我们", "你们", "他们", "这个", "那个",
    "什么", "怎么", "为什么", "那么", "这么", "可以", "这样", "那样", "现在", "然后", "因为",
    "所以", "如果", "不过", "不要", "没有", "不是", "不能", "不会", "一下", "时候", "知道",
    "觉得", "可能", "其实", "应该", "还是", "或者", "已经", "一样", "一直", "一下", "一些",
    "这种", "这种", "东西", "地方", "现在", "自己", "怎么", "这么", "那样", "这里", "那里",
    "the", "a", "an", "is", "are", "was", "were", "to", "of", "in", "on", "and", "or", "i",
    "you", "it", "that", "this", "for", "with", "be", "do", "have", "has",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TextMessage;

    fn tm(time: i64, sender: i64, text: &str) -> TextMessage {
        TextMessage { create_time: time, sender_id: sender, text: Some(text.into()) }
    }

    #[test]
    fn segments_and_ranks() {
        let texts = vec![
            tm(1, 2, "今天天气真好，我们去爬山吧，爬山很累但很开心"),
            tm(2, 1, "好啊，一起爬山！天气真不错"),
            tm(3, 2, "嗯嗯，记得带水"),
        ];
        let wf = word_freq(&texts, Some(1), 5);
        // 「爬山」「天气」应进总体前列；停用词「我们/很/但/吧」被过滤。
        let top: Vec<&str> = wf.overall.iter().map(|(w, _)| w.as_str()).collect();
        assert!(top.contains(&"爬山"), "总体应有「爬山」: {top:?}");
        assert!(top.contains(&"天气"), "总体应有「天气」: {top:?}");
        assert!(!top.contains(&"我们"), "停用词应被过滤");
        // 我（sender 1）的最常说里应有「爬山」
        assert!(wf.mine.iter().any(|(w, _)| w == "爬山"));
    }

    #[test]
    fn filters_digits_and_punct() {
        let texts = vec![tm(1, 1, "123456 ，。！ 天气真好")];
        let wf = word_freq(&texts, Some(1), 10);
        let words: Vec<&str> = wf.overall.iter().map(|(w, _)| w.as_str()).collect();
        assert!(words.contains(&"天气"));
        assert!(!words.iter().any(|w| w.chars().all(|c| c.is_ascii_digit())), "纯数字应过滤");
    }
}
