//! 消息正文解码：微信 4.x 的 `message_content` / `compress_content` 多为 zstd 压缩。
//!
//! 解码只在该会话需要做文本分析（字数 / 关键词 / 词频）时按需进行；
//! 统计骨架（计数/类型/时段）从不读正文。

/// zstd 帧魔数。
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];

/// 把原始字节解成 UTF-8 文本：zstd 帧先解压，否则按明文处理。
pub fn decode(raw: &[u8]) -> Option<String> {
    if raw.len() >= 4 && raw[..4] == ZSTD_MAGIC {
        let decoded = zstd::decode_all(raw).ok()?;
        String::from_utf8(decoded).ok()
    } else {
        String::from_utf8(raw.to_vec()).ok()
    }
}

/// 取消息正文：优先 `message_content`，空或无法解码时回退 `compress_content`；
/// 并去掉群聊的 `wxid_xxx:\n` / `xxx@chatroom:\n` 发送者前缀（1v1 无前缀，不受影响）。
pub fn decode_msg(message_content: &[u8], compress_content: &[u8]) -> Option<String> {
    if !message_content.is_empty() {
        if let Some(s) = decode(message_content) {
            return Some(strip_group_prefix(s));
        }
    }
    decode(compress_content).map(strip_group_prefix)
}

/// 去掉群聊发送者前缀：仅当前缀形如 `wxid_...` 或含 `@`（如群成员 id）时，切掉第一个 `:\n` 之前的部分。
fn strip_group_prefix(s: String) -> String {
    if let Some(idx) = s.find(":\n") {
        let prefix = &s[..idx];
        if prefix.starts_with("wxid_") || prefix.contains('@') {
            return s[idx + 2..].to_string();
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_plain_utf8() {
        assert_eq!(decode("你好世界".as_bytes()), Some("你好世界".to_string()));
    }

    #[test]
    fn decodes_zstd_frame() {
        // 用 zstd 压缩一段中文，再解回来。
        let original = "今晚月色真美，想你了。";
        let compressed = zstd::encode_all(original.as_bytes(), 3).unwrap();
        assert_eq!(&compressed[..4], &ZSTD_MAGIC, "应是 zstd 帧");
        assert_eq!(decode(&compressed), Some(original.to_string()));
    }

    #[test]
    fn strips_chatroom_prefix_only() {
        assert_eq!(strip_group_prefix("wxid_abc:\n正文".into()), "正文");
        assert_eq!(strip_group_prefix("123456@chatroom:\n正文".into()), "正文");
        // 1v1 无前缀，或冒号前不是 id，原样返回。
        assert_eq!(strip_group_prefix("就是普通文本: 别切".into()), "就是普通文本: 别切");
    }
}
