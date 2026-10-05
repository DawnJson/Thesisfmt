//! 段落角色识别：纯规则。拿不准的标 ambiguous（不修复）。

use super::model::{Model, Para};
use super::xml::Doc;
use regex::Regex;
use std::cell::Cell;
use std::collections::HashSet;
use std::sync::LazyLock;

macro_rules! regex {
    ($name:ident, $p:expr) => {
        pub static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($p).unwrap());
    };
}

regex!(CHAP, r"^第\s*[一二三四五六七八九十百\d]+\s*章");
regex!(SEC, r"^(\d+(?:[.．]\d+){1,3})[\s\u{3000}]*[\u{4e00}-\u{9fff}A-Za-z]");
regex!(LISTITEM, r"^\d+[.、)）]\s*\S");
// 冒号，或两个以上空格 / 制表符隔开的关键词（“Key words  a; b”）
regex!(KW_ZH, r"^关\s*键\s*[词字]\s*(?:[:：]|\s{2,}|\t)");
regex!(KW_EN, r"(?i)^Key\s*words?\s*(?:[:：]|\s{2,}|\t)");
// 英文图题 / 表题（中英双语题注的英文行）
regex!(CAP_EN, r"^(?:Fig(?:ure)?\.?|Tab(?:le)?\.?)\s*(?:\d+|[A-Z])(?:\s*[.\-‐‑–—]\s*\d+)?(?:[.:：]\s*|\s|$)");
// 不带「第 章」的章编号（GB/T 7713.1：1 引言；或“一、绪论”）
regex!(CHAP_ARABIC, r"^\d{1,2}(?:[\s\u{3000}]+[\u{4e00}-\u{9fff}A-Za-z]|[\u{4e00}-\u{9fff}])");
regex!(CHAP_HAN, r"^([一二三四五六七八九十]{1,3})\s*[、.．]");
// 编号的“章”位可以是附录字母（图 A.1，§2.3.11）
regex!(CAP, r"^(图|表)\s*(\d+|[A-Z])\s*[.\-－–—]\s*(\d+)");
// 全文连续编号的题注（图 1 ×××）：要求按章编号的规则包据此报编号错误
regex!(CAP_FLAT, r"^(图|表)\s*(\d{1,3})(?:[\s\u{3000}]|$)");
regex!(CONT_CAP, r"^续\s*(图|表)\s*(\d+|[A-Z])\s*[.\-－–—]\s*(\d+)");
regex!(CONT_FLAT, r"^续\s*(图|表)\s*(\d{1,3})(?:[\s\u{3000}]|$)");
// 题注后加「（续）」的续表 / 续图（表 2-1 ×××（续））
regex!(CONT_SUFFIX, r"[（(]\s*续\s*[表图]?\s*[）)]$");
// 模板里括号中的格式说明：「（三号黑体）」「(宋体小四，1.5倍行距)」
regex!(
    FMT_NOTE,
    r"[（(【\[][^（()）【】\[\]]{0,80}?(?:(?:宋体|黑体|楷体|仿宋|隶书|Times New Roman)[，,、\s]*(?:初号|小初|[一二三四五六七八]号|小[一二三四五六])|(?:初号|小初|[一二三四五六七八]号字?|小[一二三四五六]号?字?)[，,、\s]*(?:宋体|黑体|楷体|仿宋|隶书)|倍行距|首行缩进|段前|段后|字号)[^（()）【】\[\]]{0,80}[）)】\]]"
);
regex!(REF, r"^\[\d+\]");
// 只有「附录」和序号（附录、附录 A、附录一、附录 II）：带冒号也是标题，不是「附录的内容包括：」这类引导句
regex!(APPX_LABEL, r"^附录\s*(?:[A-Z]|\d{1,2}|[IVX]{1,4}|[Ⅰ-Ⅻ]|[一二三四五六七八九十]{1,3})?\s*[:：]$");
// 图表下方的资料来源、数据来源与注
regex!(TABLE_SOURCE, r"^(?:(?:资料|数据)?来源|注)\s*[:：]");
regex!(EQ_ONLY_NO, r"^[（(][^（()）]*[）)]$");
regex!(TOC_TAIL, r"(?:[.·…\s]{2,}|\t)\s*[\dIVXivx]+\s*$");
// 点线引到页码（“附录…………67”）
regex!(TOC_LEADER, r"[.·…]{4,}\s*[\dIVXivx]+\s*$");
regex!(TOC_SEC, r"^(?:\d+|[A-Z])(?:[.．]\d+)+");
regex!(SENTENCE_END, r"[。；！？]");

/// 摘要、目录之外的前置部分标题：插图清单、符号说明等（目录式的清单与说明式的符号表）。
const LIST_TITLES: [&str; 9] =
    ["插图清单", "插图目录", "图目录", "表目录", "附表清单", "插图和附表清单", "插图与附表清单", "图表目录", "图表清单"];
const SYMBOL_TITLES: [&str; 15] = [
    "符号说明", "符号表", "主要符号表", "主要符号说明", "符号对照表", "主要符号对照表", "符号和缩略语说明", "符号与缩略语说明",
    "缩略语表", "缩略词表", "缩略词语表", "缩略语对照表", "缩略词对照表", "中英文缩略词对照表", "术语表",
];
regex!(CAP_REJECT, r"[。；]|所示|表明|显示");
// 分图说明 “(a) ……；(b) ……” 里的分号不算句子
regex!(SUBFIG, r"[(（][a-z][)）]");

/// 去掉全部空白（含全角空格）。
pub(super) fn norm(t: &str) -> String {
    t.chars().filter(|c| !c.is_whitespace()).collect()
}

/// 关键词段里关键词列表开始的字节位置（“关键词：”“Key words  ”之后）。
pub(super) fn keywords_start(text: &str) -> Option<usize> {
    let lead = text.len() - text.trim_start().len();
    let t = &text[lead..];
    KW_ZH.find(t).or_else(|| KW_EN.find(t)).map(|m| lead + m.end())
}

fn toc_like(p: &Para) -> bool {
    p.has_tab || TOC_TAIL.is_match(p.text.trim_end())
}

/// 目录行：带节序号（2.1、A.1）的是节标题行，其余（第 1 章、摘要、参考文献、附录 A……）是章标题行。
fn toc_role(t: &str) -> &'static str {
    if TOC_SEC.is_match(t) { "toc_item" } else { "toc_chapter" }
}

/// 致谢之后、规则包里以章标题格式书写的部分标题（§2.1、§2.3.9.2）。
fn is_end_title(n: &str, t: &str) -> bool {
    n.chars().count() <= 30
        && !SENTENCE_END.is_match(t)
        && (n == "声明"
            || n.starts_with("个人简历")
            || n.starts_with("答辩委员会决议")
            || ((n.starts_with("指导教师") || n.starts_with("指导小组")) && n.ends_with("评语")))
}

fn all_bold(model: &Model, doc: &Doc, p: &Para) -> bool {
    let mut seen = false;
    for r in &p.runs {
        if r.text.trim().is_empty() {
            continue;
        }
        if model.run_eff(doc, p, r).b != Some(true) {
            return false;
        }
        seen = true;
    }
    seen
}

#[derive(PartialEq, Clone, Copy)]
enum Region {
    Cover,
    Toc,
    Abs,
    Front,
    Refs,
    Body,
}

struct State {
    region: Cell<Region>,
    chapter_seen: bool,
    chapter_title: Option<String>,
    /// 上一章是“3 ×××”这样的阿拉伯数字章号时，下一章的号（4）
    next_arabic: Option<i64>,
    /// 没有大纲级别、靠编号与格式认出的“1 ×××”章标题段
    seq_chapters: HashSet<usize>,
}

/// 行首的阿拉伯数字（“3 超细……”里的 3）。
fn leading_number(t: &str) -> Option<i64> {
    t.chars().take_while(char::is_ascii_digit).collect::<String>().parse().ok()
}

fn sec_level(dots: usize) -> &'static str {
    ["section_1", "section_2", "section_3"][dots - 1]
}

fn outline_role(lvl: i64) -> Option<&'static str> {
    ["chapter", "section_1", "section_2", "section_3"].get(usize::try_from(lvl).ok()?).copied()
}

/// 返回 Some((role, ambiguous)) 表示写入；None 表示不动。区域与章标题状态写进 `st`。
fn classify(model: &Model, doc: &Doc, p: &Para, text: &str, st: &mut State) -> Option<(Option<&'static str>, bool)> {
    let t = text;
    let name = p.style_name.trim().to_lowercase();
    let n = norm(t);
    let set = |role, amb, reg: Option<Region>| {
        if let Some(r) = reg {
            st.region.set(r);
        }
        Some((role, amb))
    };
    // 独立成行的表达式：段内除公式外只有序号
    if p.has_math && st.chapter_seen && (n.is_empty() || EQ_ONLY_NO.is_match(&n)) {
        return set(Some("equation"), false, None);
    }
    // 插图 / 附表清单的条目：格式要求因校而异，不归入目录行
    let toc_line = name.starts_with("toc") && !name.contains("heading") || name.starts_with("目录") && name.chars().count() > 2;
    let in_toc = toc_line || st.region.get() == Region::Toc && toc_like(p);
    if name == "table of figures" || in_toc && (CAP.is_match(t) || CAP_FLAT.is_match(t) || CAP_EN.is_match(t)) {
        return set(None, false, None);
    }
    if in_toc {
        return set(Some(toc_role(t)), false, None);
    }
    // 目录区之外的手打目录行：点线引到页码，或以页码收尾的章节编号行
    if TOC_LEADER.is_match(t) || TOC_TAIL.is_match(t) && (CHAP.is_match(t) || SEC.is_match(t)) {
        return set(Some(toc_role(t)), false, None);
    }
    let title = n.trim_end_matches([':', '：']);
    let low = title.to_lowercase();
    if name == "toc heading" || n == "目录" || n == "目次" {
        return set(Some("toc_title"), false, Some(Region::Toc));
    } else if !st.chapter_seen && (LIST_TITLES.contains(&title) || matches!(low.as_str(), "listoffigures" | "listoftables")) {
        return set(Some("front_title"), false, Some(Region::Toc));
    } else if !st.chapter_seen && (SYMBOL_TITLES.contains(&title) || matches!(low.as_str(), "nomenclature" | "listofabbreviations")) {
        return set(Some("front_title"), false, Some(Region::Front));
    } else if matches!(title, "摘要" | "中文摘要" | "内容摘要") {
        return set(Some("abstract_title_zh"), false, Some(Region::Abs));
    } else if title.to_lowercase() == "abstract" || title == "英文摘要" {
        return set(Some("abstract_title_en"), false, Some(Region::Abs));
    } else if KW_ZH.is_match(t) {
        return set(Some("keywords_zh"), false, Some(Region::Front));
    } else if KW_EN.is_match(t) {
        return set(Some("keywords_en"), false, Some(Region::Front));
    } else if matches!(title, "参考文献" | "主要参考文献" | "参考书目") {
        return set(Some("references_title"), false, Some(Region::Refs));
    } else if matches!(n.as_str(), "致谢" | "谢辞" | "致谢辞" | "后记") {
        return set(Some("ack_title"), false, Some(Region::Body));
    } else if n.starts_with("附录")
        && n.chars().count() <= 40
        && !SENTENCE_END.is_match(t)
        && (!n.ends_with([':', '：']) || APPX_LABEL.is_match(&n))
    {
        return set(Some("appendix_title"), false, Some(Region::Body));
    } else if st.chapter_seen && is_end_title(&n, t) {
        return set(Some("end_title"), false, Some(Region::Body));
    }
    if st.chapter_seen && TABLE_SOURCE.is_match(t) {
        return set(Some("table_source"), false, None);
    }
    if let Some(m) = CONT_CAP.captures(t).or_else(|| CONT_FLAT.captures(t)) {
        return set(Some(if &m[1] == "图" { "fig_caption" } else { "table_caption" }), false, None);
    }
    let cap_style = name == "caption" || name.contains("题注");
    if let Some(m) = CAP.captures(t).or_else(|| CAP_FLAT.captures(t)) {
        let rejected = CAP_REJECT.is_match(t) && !(SUBFIG.is_match(t) && !t.contains('。'));
        if cap_style || (t.chars().count() <= 150 && !rejected) {
            return set(Some(if &m[1] == "图" { "fig_caption" } else { "table_caption" }), false, None);
        }
    }
    let first = t.chars().next().unwrap_or(' ');
    if cap_style && (first == '图' || first == '表') {
        return set(Some(if first == '图' { "fig_caption" } else { "table_caption" }), false, None);
    }
    // 英文题注：规则包只规定中文题注格式，不检查，也不当正文
    if t.chars().count() <= 200 && CAP_EN.is_match(t) {
        return set(None, false, None);
    }
    let short = t.chars().count() <= 60 && !SENTENCE_END.is_match(t);
    let lvl = p.outline;
    let (mut role, mut amb) = (None, false);
    let next_chapter = || {
        st.next_arabic.is_some_and(|k| leading_number(t) == Some(k))
            && CHAP_ARABIC.is_match(t)
            && t.chars().count() <= 30
            && !SENTENCE_END.is_match(t)
    };
    if CHAP.is_match(t) && short || next_chapter() || st.seq_chapters.contains(&p.idx) {
        role = Some("chapter");
    } else if let Some(m) = SEC.captures(t).filter(|_| short && !toc_like(p)) {
        role = Some(sec_level(m[1].matches(['.', '．']).count()));
    } else if let Some(r) = lvl.and_then(outline_role) {
        role = Some(r);
        if t.chars().count() > 80 || SENTENCE_END.is_match(t) {
            amb = true;
        } else if r == "chapter" && !st.chapter_seen && p.ppr.num_pr != Some(true) && !CHAP_ARABIC.is_match(t) && !CHAP_HAN.is_match(t) {
            amb = true; // 前置部分的未编号一级标题（符号表等）
        }
    } else if lvl.is_some() {
        amb = true;
    }
    if let Some(role) = role {
        let is_chapter = role == "chapter" && !amb;
        if is_chapter {
            st.chapter_seen = true;
            st.chapter_title = Some(t.to_string());
            st.next_arabic = CHAP_ARABIC.is_match(t).then(|| leading_number(t)).flatten().map(|n| n + 1);
        }
        return set(Some(role), amb, is_chapter.then_some(Region::Body));
    }
    if amb {
        return set(None, true, None);
    }
    // 无标题特征：按区域归类
    match st.region.get() {
        Region::Refs => set(Some("ref_item"), !REF.is_match(t), None),
        Region::Abs => set(Some("abstract_body"), false, None),
        Region::Toc => {
            st.region.set(Region::Front);
            None
        }
        Region::Body => {
            let listish = LISTITEM.is_match(t) && t.chars().count() <= 30 && !SENTENCE_END.is_match(t);
            if (short && t.chars().count() <= 40 && all_bold(model, doc, p)) || listish {
                set(None, true, None)
            } else {
                set(Some("body"), false, None)
            }
        }
        Region::Cover | Region::Front => None,
    }
}

/// 没有大纲级别、样式也不是标题的“1 绪论”“2 ×××”：同一格式（样式、字号、加粗）的一组短行，
/// 编号恰为 1、2、3……（至少两章），比正文字号大或加粗，且至少一章后面跟着“k.1”这样的节，才认作章标题。
fn numbered_chapters(model: &Model, doc: &Doc) -> HashSet<usize> {
    let first_run = |p: &Para| p.runs.iter().find(|r| !r.text.trim().is_empty()).map(|r| model.run_eff(doc, p, r).sz.unwrap_or(20));
    let mut sizes: Vec<i64> = model.paras.iter().filter(|p| p.text.trim().chars().count() >= 60).filter_map(first_run).collect();
    sizes.sort_unstable();
    let body = sizes.get(sizes.len() / 2).copied().unwrap_or(24);
    // 格式签名（样式名, 字号, 加粗）→ [(段号, 章号)]
    type Sig = (String, i64, bool);
    let mut groups: Vec<(Sig, Vec<(usize, i64)>)> = vec![];
    for (i, p) in model.paras.iter().enumerate() {
        let t = p.text.trim();
        let cand = CHAP_ARABIC.is_match(t)
            && t.chars().count() <= 30
            && !SENTENCE_END.is_match(t)
            && p.outline.is_none()
            && p.ppr.num_pr != Some(true)
            && !toc_like(p);
        let (Some(k), Some(sz)) = (cand.then(|| leading_number(t)).flatten(), first_run(p)) else { continue };
        let bold = all_bold(model, doc, p);
        if sz <= body && !bold {
            continue;
        }
        let sig = (p.style_name.clone(), sz, bold);
        match groups.iter_mut().find(|(s, _)| *s == sig) {
            Some((_, v)) => v.push((i, k)),
            None => groups.push((sig, vec![(i, k)])),
        }
    }
    let mut out = HashSet::new();
    for (_, g) in groups {
        let consecutive = g.len() >= 2 && g.iter().enumerate().all(|(j, &(_, k))| k == j as i64 + 1);
        let has_section = |j: usize| {
            let (start, k) = g[j];
            let end = g.get(j + 1).map_or(model.paras.len(), |x| x.0);
            let prefix = format!("{k}.");
            let prefix_full = format!("{k}．");
            model.paras[start + 1..end]
                .iter()
                .any(|p| SEC.is_match(p.text.trim()) && (p.text.trim().starts_with(&prefix) || p.text.trim().starts_with(&prefix_full)))
        };
        if consecutive && (0..g.len()).any(has_section) {
            out.extend(g.iter().map(|&(i, _)| i));
        }
    }
    out
}

/// 括号里的格式说明（「（三号黑体）」）去掉后的文字；没有说明返回 None。
pub(super) fn strip_notes(text: &str) -> Option<String> {
    FMT_NOTE.is_match(text).then(|| FMT_NOTE.replace_all(text, "").trim().to_string())
}

pub fn assign_roles(model: &mut Model, doc: &Doc) {
    let seq_chapters = numbered_chapters(model, doc);
    let mut st = State { region: Cell::new(Region::Cover), chapter_seen: false, chapter_title: None, next_arabic: None, seq_chapters };
    for i in 0..model.paras.len() {
        model.paras[i].chapter = st.chapter_title.clone();
        if model.paras[i].text.trim().is_empty() && !model.paras[i].has_math {
            continue;
        }
        // 模板里的格式说明：去掉说明再识别；整段都是说明的不当任何角色
        let stripped = strip_notes(&model.paras[i].text);
        let out = match stripped.as_deref() {
            Some("") => Some((None, false)),
            Some(s) => classify(model, doc, &model.paras[i], s, &mut st),
            None => classify(model, doc, &model.paras[i], model.paras[i].text.trim(), &mut st),
        };
        let p = &mut model.paras[i];
        p.note = stripped.is_some();
        if let Some((role, amb)) = out {
            p.role = role;
            p.ambiguous = amb;
            if role == Some("chapter") && !amb {
                p.chapter = st.chapter_title.clone();
            }
        }
    }
}
