//! 开发工具：用一批高校、期刊模板检验角色识别。`examples/survey.rs` 调用。
//!
//! 对每份 docx 给出各角色段数，以及“看上去应是某角色却没认出来”的段落（按文字特征判断，只作提示）。

use super::analyze;
use super::model::Para;
use super::roles::norm;
use regex::Regex;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::LazyLock;

macro_rules! regex {
    ($name:ident, $p:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($p).unwrap());
    };
}

// 说明文字（“关键词是……”）和规范文件里单独成行的小标题“关键词”都不算；要有冒号或空格隔开的内容
regex!(KW_ZH, r"^关\s*键\s*[词字]\s*(?:[:：]|\s{2,}|\t)\s*\S");
regex!(KW_EN, r"(?i)^key\s*-?\s*words?\s*(?:[:：]|\s{2,}|\t)\s*\S");
regex!(CHAP_CN, r"^第\s*[一二三四五六七八九十百\d]+\s*章");
regex!(CHAP_NUM, r"^\d{1,2}[\s\u{3000}]+[\u{4e00}-\u{9fff}A-Za-z]");
regex!(CHAP_HAN, r"^[一二三四五六七八九十]{1,3}\s*[、.．]");
regex!(SEC_NUM, r"^\d{1,2}(?:[.．]\d{1,2}){1,3}[\s\u{3000}]*[\u{4e00}-\u{9fff}A-Za-z]");
regex!(CAPTION, r"^(?:续)?(?:图|表)\s*(?:\d+|[A-Z])(?:\s*[.\-－–—．]\s*\d+)?(?:[\s\u{3000}]|$)");
regex!(CAPTION_EN, r"^(?:Fig\.?|Figure|Table)\s*\d");
regex!(SENTENCE_END, r"[。；！？]");
// 点线引到页码：目录或图表清单条目
regex!(LEADER, r"[.·…]{4,}\s*[\dIVXivx]*\s*$");

/// 按文字特征估计的应有角色（粗分类，`roles` 前缀匹配即算认出）。
fn expect(p: &Para) -> Option<&'static str> {
    let t = p.text.trim();
    let n = norm(t);
    let low = n.to_lowercase();
    let short = n.chars().count() <= 30 && !SENTENCE_END.is_match(t);
    Some(match () {
        _ if short && matches!(n.as_str(), "摘要" | "中文摘要" | "内容摘要" | "摘要：" | "摘要:") => "abstract_title_zh",
        _ if short && (low == "abstract" || low == "英文摘要" || low == "abstract:") => "abstract_title_en",
        _ if KW_ZH.is_match(t) => "keywords_zh",
        _ if KW_EN.is_match(t) => "keywords_en",
        _ if short && matches!(n.as_str(), "目录" | "目次") => "toc_title",
        _ if short && matches!(n.as_str(), "参考文献" | "主要参考文献" | "参考文献：" | "参考书目") => "references_title",
        _ if short && matches!(n.as_str(), "致谢" | "谢辞" | "致谢辞" | "鸣谢" | "后记") => "ack_title",
        // “附录：”“附录包括以下内容：”是引出下文的说明，不是标题
        _ if short && n.starts_with("附录") && !n.ends_with([':', '：']) && !LEADER.is_match(t) => "appendix_title",
        _ if p.has_figure || p.has_tab || p.style_name.eq_ignore_ascii_case("table of figures") || LEADER.is_match(t) => return None,
        _ if CAPTION.is_match(t) && t.chars().count() <= 80 => "caption",
        // 英文题注应留空角色（不检查、不当正文）
        _ if CAPTION_EN.is_match(t) && t.chars().count() <= 200 => "caption_en",
        _ if short && CHAP_CN.is_match(t) => "chapter",
        _ if short && SEC_NUM.is_match(t) => "section",
        _ if short && (CHAP_NUM.is_match(t) || CHAP_HAN.is_match(t)) => "heading",
        _ if p.outline.is_some_and(|l| l <= 3) && short => "heading",
        _ => return None,
    })
}

fn matches(want: &str, got: Option<&str>) -> bool {
    let Some(g) = got else { return false };
    match want {
        "caption" => g.ends_with("_caption"),
        "caption_en" => false,
        "section" => g.starts_with("section_") || g == "toc_item",
        "heading" => g == "chapter" || g.starts_with("section_") || g.starts_with("toc_") || g.ends_with("_title"),
        // 目录里的“第 1 章 绪论……3”同样算认出
        "chapter" => g == "chapter" || g.starts_with("toc_"),
        w => g == w || g.starts_with("toc_"),
    }
}

/// 一份 docx 的识别统计。
pub fn survey(docx: &[u8]) -> Result<Value, String> {
    let a = analyze(docx, "thu-master", None)?;
    let paras = &a.model.paras;
    let mut roles: BTreeMap<&str, usize> = BTreeMap::new();
    let mut misses: BTreeMap<&str, usize> = BTreeMap::new();
    let mut suspects = vec![];
    let mut nonempty = 0;
    for p in paras {
        if p.text.trim().is_empty() {
            continue;
        }
        nonempty += 1;
        let key = match (p.role, p.ambiguous) {
            (Some(r), false) => r,
            (_, true) => "?",
            (None, false) => "-",
        };
        *roles.entry(key).or_default() += 1;
        // 整段是模板格式说明（“（章标题，三号，黑体，居中）”）的不算应有角色
        if let Some(want) = expect(p).filter(|_| !(p.note && p.role.is_none())) {
            let got = if p.ambiguous { None } else { p.role };
            let ok = if want == "caption_en" { key == "-" } else { matches(want, got) };
            if !ok {
                *misses.entry(want).or_default() += 1;
                if suspects.len() < 30 {
                    suspects.push(json!({
                        "i": p.idx, "want": want, "got": key, "style": p.style_name,
                        "outline": p.outline, "text": p.text.trim().chars().take(40).collect::<String>(),
                    }));
                }
            }
        }
    }
    Ok(json!({
        "paragraphs": paras.len(), "nonempty": nonempty, "tables": a.model.tables.len(),
        "sections": a.model.sections.len(), "roles": roles, "misses": misses, "suspects": suspects,
    }))
}
