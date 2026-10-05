//! 检查项。`run_checks` 返回 [(Finding, Fix)]，Fix 供 fix 使用。

use super::edit::{self, Confirm, Edit, TextEdit};
use super::fix::Parts;
use super::layout::{self, Layout};
use super::model::{Edge, Model, PProps, Para, Table, ASCII, EAST_ASIA, HANSI};
use super::pack::{role_label, size_name, Align, AppendixStyle, ContinuedStyle, LineRule, Pack, PageFmt, Role};
use super::roles::{keywords_start, norm, CAP, CAP_FLAT, CHAP_ARABIC, CHAP_HAN, CONT_CAP, CONT_FLAT, CONT_SUFFIX, FMT_NOTE};
use super::xml::{Doc, Id, Ns};
use regex::Regex;
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

pub const A4: (i64, i64) = (11906, 16838);
const PAGE_TOL: i64 = 40; // twip
const MARGIN_TOL_CM: f64 = 0.02;
/// 附了文字修改的问题在说明末尾补上的提示。
const CONFIRM: &str = "（可预览改动，逐条确认后修改正文文字）";
pub const CM: f64 = 1440.0 / 2.54;

macro_rules! regex {
    ($name:ident, $p:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($p).unwrap());
    };
}

regex!(CN_CHAR, r"[\u{3000}-\u{303f}\u{3400}-\u{4dbf}\u{4e00}-\u{9fff}\u{f900}-\u{faff}\u{ff00}-\u{ffef}]");
regex!(LATIN_CHAR, r"[A-Za-z0-9]");
regex!(
    HALF_PUNCT,
    r"[\u{4e00}-\u{9fff}][,;:?!(]|[,;:?!)][\u{4e00}-\u{9fff}]|[\u{4e00}-\u{9fff}]\.(?:\s|$)|\)[\u{4e00}-\u{9fff}]"
);
regex!(FULL_ALNUM, r"[\u{ff21}-\u{ff3a}\u{ff41}-\u{ff5a}\u{ff10}-\u{ff19}]+");
regex!(KW_SPLIT, r"[;；,，、]");
regex!(PAGE_INSTR, r"\bPAGE\b");
regex!(CHAP_NO, r"^第\s*([一二三四五六七八九十\d]+)\s*章");
regex!(CHAP_PREFIX, r"^第\s*[一二三四五六七八九十百\d]+\s*章\s*");
// 附录序号：罗马数字（含全角 Ⅰ Ⅱ）、大写字母、数字、中文数字或小写字母
regex!(APPX_NO, r"^附录\s*([IVX]{1,4}|[ⅠⅡⅢⅣⅤⅥⅦⅧⅨⅩ]|[A-Z]|\d{1,2}|[一二三四五六七八九十]{1,3}|[a-z])(?:[^A-Za-z0-9]|$)");
regex!(STYLEREF, r#"(?i)^\s*STYLEREF\s+(?:"([^"]+)"|(\S+))"#);
pub(super) static SUB_CAP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[（(][a-zA-Z一二三四五六七八九十\d]+[)）]").unwrap());
regex!(FIG_REF, r"图\s*(?:[A-Z]|\d+)\s*[.\-－–—]\s*\d+(?:\s*(?:[、，,和与及~～至]|到)\s*(?:图\s*)?(?:[A-Z]|\d+)\s*[.\-－–—]\s*\d+)*");
regex!(NUM_PAIR, r"([A-Z]|\d+)\s*[.\-－–—]\s*(\d+)");
regex!(CITE, r"\[(\d+(?:\s*[,，\-–—~～]\s*\d+)*)\]");
regex!(REF_NO, r"^\[(\d+)\]");
regex!(FULL_PUNCT, r"[，。；：！？（）“”‘’、《》【】]");
regex!(EQ_TAIL, r"[（(][^（()）]*[）)]$");
regex!(EQ_NO, r"^[（(](\d+|[A-Z])[-.](\d+)[）)]$");

/// Python 的 `format(v, "g")` / `".{prec}g"`。
pub fn fmt_g(v: f64, prec: usize) -> String {
    if v == 0.0 || !v.is_finite() {
        return if v == 0.0 { "0".into() } else { v.to_string() };
    }
    let exp = v.abs().log10().floor() as i32;
    if exp < -4 || exp >= prec as i32 {
        return v.to_string();
    }
    let s = format!("{:.*}", (prec as i32 - 1 - exp).max(0) as usize, v);
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

fn g6(v: f64) -> String {
    fmt_g(v, 6)
}

fn round2(v: f64) -> f64 {
    format!("{v:.2}").parse().unwrap()
}

/// 字体名比较键：中英文名、GB2312 / GBK 旧名视为同一字体（「楷体_GB2312」即「楷体」）。
fn canon_font(name: &str) -> String {
    let n = name.trim();
    match n.to_lowercase().as_str() {
        "simsun" => "宋体".into(),
        "nsimsun" => "新宋体".into(),
        "simhei" => "黑体".into(),
        "kaiti" | "kaiti_gb2312" | "楷体_gb2312" => "楷体".into(),
        "fangsong" | "fangsong_gb2312" | "仿宋_gb2312" => "仿宋".into(),
        "microsoft yahei" => "微软雅黑".into(),
        "方正小标宋_gbk" | "方正小标宋简体" | "fzxiaobiaosong-b05s" => "方正小标宋".into(),
        l if n.is_ascii() => l.into(),
        _ => n.into(),
    }
}

fn same_font(a: Option<&str>, b: Option<&str>) -> bool {
    a.map(canon_font) == b.map(canon_font)
}

pub(super) fn jc_norm(v: Option<&str>) -> &'static str {
    match v.unwrap_or("left") {
        "both" => "justify",
        "right" | "end" => "right",
        "center" => "center",
        "distribute" => "distribute",
        _ => "left",
    }
}

fn jc_cn(v: &str) -> &str {
    match v {
        "left" => "居左",
        "center" => "居中",
        "right" => "居右",
        "justify" => "两端对齐",
        "distribute" => "分散对齐",
        "outside" => "在外侧（图文框）",
        "inside" => "在内侧（图文框）",
        o => o,
    }
}

fn fmt_cn(v: &str) -> &str {
    match v {
        "upperRoman" => "大写罗马数字",
        "lowerRoman" => "小写罗马数字",
        "decimal" => "阿拉伯数字",
        o => o,
    }
}

fn cm(twip: i64) -> f64 {
    twip as f64 / CM
}

fn fmt_cm(v: f64) -> String {
    let s = format!("{v:.2}");
    format!("{} cm", s.trim_end_matches('0').trim_end_matches('.'))
}

fn pt(v: f64) -> String {
    format!("{} 磅", g6(v))
}

#[derive(Serialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
    Manual,
}

#[derive(Serialize)]
pub struct Location {
    pub chapter: Option<String>,
    pub snippet: String,
}

#[derive(Serialize)]
pub struct Finding {
    pub id: String,
    pub rule_id: String,
    pub severity: Severity,
    pub category: &'static str,
    pub role: Option<&'static str>,
    pub paragraph_index: Option<usize>,
    pub location: Location,
    pub message: String,
    pub expected: Value,
    pub actual: Value,
    pub fixable: bool,
    pub source: String,
    /// 要改正文文字或结构的修复（改文字、拆续表）：给用户看的改动说明。只能逐条确认后执行，
    /// 不计入 `fixable`、不进「全部修复」。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirm: Option<Confirm>,
    #[serde(skip)]
    pub edit: Option<Edit>,
}

#[derive(Clone, Copy)]
pub enum SectFix {
    Paper,
    Margins,
    Distance,
    PgNum(PageFmt),
    /// 本节页码从该值重新开始。
    Start(u32),
    /// 去掉本节的重新编号，接续上一节。
    Continue,
}

/// 在某段之前插入分节符（下一页）时，前后两部分各自的页码设置。
#[derive(Clone, Copy)]
pub enum Split {
    /// 前面是封面 / 声明：不要页眉页脚、不编页码；后面（前置部分）重新编号。
    Cover,
    /// 前面是前置部分、后面是正文：分别用前置与正文页码格式，按需重新编号。
    Body { restart_front: bool, restart_body: bool },
}

#[derive(Clone, Copy)]
pub enum Group {
    Font,
    Align,
    Spacing,
    Indent,
}

#[derive(Clone)]
pub enum Fix {
    Sect { sec: usize, kind: SectFix },
    Para { para: usize, group: Group, role: &'static str },
    /// 在第 `para` 段之前分节。
    Split { para: usize, kind: Split },
    /// 把封面各节的页脚引用移到第 `first` 节（第一个编页码的节），封面不再显示页脚。
    CoverFooter { first: usize },
    /// 给第 `from..=to` 段打开段落开关（keepNext / keepLines / pageBreakBefore）。
    Flag { from: usize, to: usize, flag: &'static str },
    /// 第 `table` 个表的第一行设为「在各页顶端以标题行形式重复出现」。
    RepeatHeader { table: usize },
    /// 关闭 settings.xml 的「奇偶页不同」。
    OddEven,
    /// 页眉部件 `rid` 中有文字的段落按规则包设置对齐、字体、字号。
    HeaderFormat { rid: String },
    /// 页脚部件 `rid` 中含页码域的段落设为对齐 `align`，字体、字号按规则包。
    FooterFormat { rid: String, align: Option<Align> },
    /// 第 `table` 个表改成三线表：顶线、底线、表头线按规则包线宽，去掉全部竖线。
    TableLines { table: usize },
    /// 第 `table` 个表的单元格文字按 table_cell 设置字体、字号、行距、段前后，非居中 / 两端对齐的改居中，单元格上下居中。
    TableCells { table: usize },
    /// 全部脚注段落按规则包设置字体、字号、对齐、行距、段前后与悬挂缩进。
    FootnoteFormat,
    /// 脚注编号改为规则包要求的格式（带圈数字）与每页重新编号。
    FootnoteNumbering,
}

pub struct Ctx<'a> {
    pub model: &'a Model,
    pub doc: &'a Doc,
    pub pack: &'a Pack,
    /// rId -> 页脚部件
    pub footers: &'a HashMap<String, Doc>,
    /// rId -> 页眉部件
    pub headers: &'a HashMap<String, Doc>,
    /// word/settings.xml（可能缺失）
    pub settings: Option<&'a Doc>,
    /// word/footnotes.xml（可能缺失）
    pub footnotes: Option<&'a Doc>,
    /// 精确检查采集到的排版结果；None 表示没跑。
    pub layout: Option<&'a Layout>,
    pub out: Vec<(Finding, Option<Fix>)>,
}

impl Ctx<'_> {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn add(
        &mut self,
        rule_id: &str,
        severity: Severity,
        category: &'static str,
        para: Option<usize>,
        message: String,
        expected: Value,
        actual: Value,
        fixable: bool,
        source: &str,
        fix: Option<Fix>,
        role: Option<&'static str>,
    ) {
        let p = para.map(|i| &self.model.paras[i]);
        let finding = Finding {
            id: String::new(),
            rule_id: rule_id.into(),
            severity,
            category,
            role,
            paragraph_index: para,
            location: Location {
                chapter: p.and_then(|p| p.chapter.clone()),
                snippet: p.map(|p| p.text.trim().chars().take(40).collect()).unwrap_or_default(),
            },
            message,
            expected,
            actual,
            fixable,
            source: source.into(),
            confirm: None,
            edit: None,
        };
        self.out.push((finding, fix));
    }

    /// 给刚加入的那条问题附上待确认的文字修改，并在说明后补上处理方式：能改时提示可确认修改，
    /// 改不了（落在域、修订、制表符里，或改后不变）时接上 `hand`。
    pub(super) fn offer_text(&mut self, para: usize, edits: Vec<(usize, usize, String)>, hand: &str) {
        let p = &self.model.paras[para];
        let len = p.text.chars().count();
        let ordered = edits.iter().try_fold(0, |at, (s, e, _)| (at <= *s && s <= e && *e <= len).then_some(*e)).is_some();
        debug_assert!(ordered, "文字修改区间重叠或越界：{edits:?}");
        let change = edit::change(&p.text, if ordered { &edits } else { &[] });
        let ok = ordered && !edits.is_empty() && change.after != change.before && edit::editable(self.doc, p.el, &edits);
        let f = &mut self.out.last_mut().expect("offer_text 须紧跟 add").0;
        if ok {
            f.message.push_str(CONFIRM);
            let n = change.segs.iter().filter(|s| !s.del.is_empty() || !s.ins.is_empty()).count();
            let token = change.after.clone();
            f.confirm = Some(Confirm { kind: "text", summary: format!("将修改这一段的文字（共 {n} 处）"), text: Some(change), token });
            f.edit = Some(Edit::Text(TextEdit { para, edits }));
        } else {
            f.message.push_str(hand);
        }
    }
}

fn int_attr(doc: &Doc, el: Option<Id>, a: &str) -> Option<i64> {
    el.and_then(|e| doc.wattr(e, a)).and_then(|v| v.parse().ok())
}

// ---------------------------------------------------------------- 页面
fn check_page(ctx: &mut Ctx) {
    let pack = ctx.pack;
    let Some(page) = &pack.page else { return };
    let (doc, model) = (ctx.doc, ctx.model);
    let src = page.source.as_str();
    let kinds = classify_sections(model);
    // 节的页边距与一组要求不符之处：(键, 中文名, 应为, 实际)；`at_least` 时只有小于要求才算不符
    let diffs = |pgmar: Option<Id>, want: [Option<f64>; 4], at_least: bool| -> Vec<(&'static str, &'static str, f64, f64)> {
        [("top", "上"), ("bottom", "下"), ("left", "左"), ("right", "右")]
            .into_iter()
            .zip(want)
            .filter_map(|((side, cn), w)| Some((side, cn, w?, cm(int_attr(doc, pgmar, side)?))))
            .filter(|&(_, _, w, a)| if at_least { a < w - MARGIN_TOL_CM } else { (a - w).abs() > MARGIN_TOL_CM })
            .collect()
    };
    for sec in &model.sections {
        let para = model.first_text_para(sec);
        let pgsz = sec.el.and_then(|e| doc.wchild(e, "pgSz"));
        let pgmar = sec.el.and_then(|e| doc.wchild(e, "pgMar"));
        let n = sec.index + 1;
        let kind = |k| Some(Fix::Sect { sec: sec.index, kind: k });
        if page.paper.is_some() {
            if let (Some(mut w), Some(mut h)) = (int_attr(doc, pgsz, "w"), int_attr(doc, pgsz, "h")) {
                if pgsz.and_then(|e| doc.wattr(e, "orient")) == Some("landscape") {
                    std::mem::swap(&mut w, &mut h);
                }
                if (w - A4.0).abs() > PAGE_TOL || (h - A4.1).abs() > PAGE_TOL {
                    ctx.add(
                        "page.paper", Severity::Error, "page", para,
                        format!("第{n}节纸张应为 A4（21.0×29.7 cm），实际 {}×{}", fmt_cm(cm(w)), fmt_cm(cm(h))),
                        json!({"paper": "A4"}),
                        json!({"width_cm": round2(cm(w)), "height_cm": round2(cm(h))}),
                        true, src, kind(SectFix::Paper), None,
                    );
                }
            }
        }
        // 摘要之前的节（封面、名单、授权说明）：符合正文或任一封面的页边距即可，页眉页脚距不检查
        let cover = kinds[sec.index].is_none();
        if let Some(m) = &page.margins_cm {
            let bad = diffs(pgmar, [m.top, m.bottom, m.left, m.right], m.at_least);
            let cover_ok = || page.cover_margins_cm.iter().any(|c| diffs(pgmar, [Some(c.top), Some(c.bottom), Some(c.left), Some(c.right)], false).is_empty());
            let should = if m.at_least { "应不小于" } else { "应为" };
            if !bad.is_empty() && !(cover && cover_ok()) {
                let (mut be, mut ba) = (Map::new(), Map::new());
                let parts: Vec<String> = bad
                    .iter()
                    .map(|&(side, cn, w, a)| {
                        be.insert(side.into(), json!(w));
                        ba.insert(side.into(), json!(round2(a)));
                        format!("{cn}边距{should} {}，实际 {}", fmt_cm(w), fmt_cm(a))
                    })
                    .collect();
                if cover {
                    let alts: Vec<String> = page
                        .cover_margins_cm
                        .iter()
                        .map(|c| format!("{}（{}）为上 {}、下 {}、左 {}、右 {}", c.name, c.source, fmt_cm(c.top), fmt_cm(c.bottom), fmt_cm(c.left), fmt_cm(c.right)))
                        .collect();
                    let alt = if alts.is_empty() { String::new() } else { format!("；{}", alts.join("；")) };
                    ctx.add(
                        "page.margins", Severity::Error, "page", para,
                        format!("第{n}节在摘要之前，页边距既不符合正文要求（{}），也不符合封面要求{alt}。请按该页实际是哪一页手动设置", parts.join("；")),
                        Value::Object(be), Value::Object(ba), false, src, None, None,
                    );
                } else {
                    ctx.add(
                        "page.margins", Severity::Error, "page", para,
                        format!("第{n}节{}", parts.join("；")),
                        Value::Object(be), Value::Object(ba), true, src, kind(SectFix::Margins), None,
                    );
                }
            }
        }
        if cover {
            continue;
        }
        let (mut be, mut ba, mut parts) = (Map::new(), Map::new(), vec![]);
        let dists = [
            ("header_distance_cm", "header", "页眉距", page.header_distance_cm),
            ("footer_distance_cm", "footer", "页脚距", page.footer_distance_cm),
        ];
        for (key, attr, cn, want) in dists {
            let (Some(want), Some(act)) = (want, int_attr(doc, pgmar, attr)) else { continue };
            if (cm(act) - want).abs() > MARGIN_TOL_CM {
                be.insert(key.into(), json!(want));
                ba.insert(key.into(), json!(round2(cm(act))));
                parts.push(format!("{cn}应为 {}，实际 {}", fmt_cm(want), fmt_cm(cm(act))));
            }
        }
        if !parts.is_empty() {
            ctx.add(
                "page.distance", Severity::Error, "page", para,
                format!("第{n}节{}", parts.join("；")),
                Value::Object(be), Value::Object(ba), true, src, kind(SectFix::Distance), None,
            );
        }
    }
}

// ---------------------------------------------------------------- 页码
/// 向前继承取得该节 `typ`（default / first / even）类型的页脚：(rId, 部件)（无则 None）。
fn footer_part<'a>(ctx: &Ctx<'a>, sec_index: usize, typ: &str) -> Option<(&'a str, &'a Doc)> {
    let (doc, footers) = (ctx.doc, ctx.footers);
    for i in (0..=sec_index).rev() {
        let Some(sp) = ctx.model.sections[i].el else { continue };
        for r in doc.children(sp).iter().copied().filter(|&c| doc.is_w(c, "footerReference")) {
            if doc.wattr(r, "type") == Some(typ) {
                return doc.attr(r, &Ns::R, "id").and_then(|id| footers.get_key_value(id)).map(|(k, d)| (k.as_str(), d));
            }
        }
    }
    None
}

fn footer_doc<'a>(ctx: &Ctx<'a>, sec_index: usize, typ: &str) -> Option<&'a Doc> {
    footer_part(ctx, sec_index, typ).map(|(_, d)| d)
}

/// 含 PAGE 域的段落及域内第一个 run。
pub(super) fn page_field_para(footer: &Doc) -> Option<(Id, Option<Id>)> {
    let all = footer.descendants(footer.root());
    for p in all.iter().copied().filter(|&p| footer.is_w(p, "p")) {
        let desc = footer.descendants(p);
        for &fs in desc.iter().filter(|&&e| footer.is_w(e, "fldSimple")) {
            if PAGE_INSTR.is_match(footer.wattr(fs, "instr").unwrap_or("")) {
                return Some((p, footer.wchild(fs, "r")));
            }
        }
        if let Some((_, r)) = complex_fields(footer, &desc).into_iter().find(|(i, _)| PAGE_INSTR.is_match(i)) {
            return Some((p, Some(r)));
        }
    }
    None
}

/// 页码段的水平位置：放在图文框里（Word「页码 → 页面底端 → 外侧」插入的就是 `w:framePr w:xAlign="outside"`）时取图文框的位置，否则取段落对齐。
fn page_number_pos<'a>(footer: &'a Doc, p: Id, jc: &'a str) -> &'a str {
    let frame = footer.wchild(p, "pPr").and_then(|x| footer.wchild(x, "framePr"));
    frame.and_then(|f| footer.wattr(f, "xAlign")).unwrap_or(jc)
}

/// 该节默认页脚的页码在外侧图文框里。
fn framed_outside(ctx: &Ctx, sec_index: usize) -> bool {
    footer_part(ctx, sec_index, "default")
        .and_then(|(_, f)| page_field_para(f).map(|(p, _)| page_number_pos(f, p, "") == "outside"))
        .unwrap_or(false)
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Kind {
    Body,
    Front,
    Mixed,
}

pub(super) fn classify_sections(model: &Model) -> Vec<Option<Kind>> {
    let (mut body_seen, mut front_seen) = (false, false);
    model
        .sections
        .iter()
        .map(|sec| {
            let roles: Vec<_> = sec.paras.iter().map(|&i| &model.paras[i]).filter(|p| !p.ambiguous).collect();
            let has_body = roles.iter().any(|p| p.role == Some("chapter"));
            let has_front = roles
                .iter()
                .any(|p| matches!(p.role, Some("abstract_title_zh" | "abstract_title_en" | "toc_title" | "front_title")));
            let k = match (has_body, has_front) {
                (true, true) => Some(Kind::Mixed),
                (true, false) => Some(Kind::Body),
                (false, true) => Some(Kind::Front),
                _ if body_seen => Some(Kind::Body),
                _ if front_seen => Some(Kind::Front),
                _ => None,
            };
            body_seen |= k == Some(Kind::Body);
            front_seen |= k == Some(Kind::Front);
            k
        })
        .collect()
}

/// 封面所在节（第一个编页码的节之前）的首页是否显示页码：开了「首页不同」看首页页脚，否则看默认页脚。
fn cover_shows_number(ctx: &Ctx, sec_index: usize) -> bool {
    let title_pg = ctx.model.sections[sec_index]
        .el
        .and_then(|sp| ctx.doc.wchild(sp, "titlePg"))
        .is_some_and(|t| super::model::toggle(ctx.doc, t));
    footer_doc(ctx, sec_index, if title_pg { "first" } else { "default" }).is_some_and(|f| page_field_para(f).is_some())
}

/// 第一个编页码的节里，摘要 / 目录标题之前还有封面、声明等内容时返回该标题段。
fn cover_shared_title(model: &Model, sec: &super::model::Section) -> Option<usize> {
    let title = sec.paras.iter().copied().find(|&i| {
        let p = &model.paras[i];
        !p.ambiguous && matches!(p.role, Some("abstract_title_zh" | "abstract_title_en" | "toc_title"))
    })?;
    let first = *sec.paras.first()?;
    let text_before = sec.paras.iter().take_while(|&&i| i != title).any(|&i| !model.paras[i].text.trim().is_empty());
    let table_before = model.tables.iter().any(|t| t.after.is_some_and(|a| a >= first && a <= title));
    (text_before || table_before).then_some(title)
}

fn check_page_numbers(ctx: &mut Ctx) {
    let pack = ctx.pack;
    let Some(pn) = &pack.page_numbers else { return };
    let (doc, model) = (ctx.doc, ctx.model);
    let src = pn.source.as_str();
    let kinds = classify_sections(model);
    let first_numbered = kinds.iter().position(Option::is_some);
    if let Some(first) = first_numbered.filter(|_| pn.cover_no_number) {
        // 封面各节显示页码：把页脚引用移给第一个编页码的节
        for sec in &model.sections[..first] {
            if cover_shows_number(ctx, sec.index) {
                ctx.add(
                    "page_numbers.cover", Severity::Error, "numbering", model.first_text_para(sec),
                    format!("第{}节是摘要之前的封面 / 声明页，不应显示页码（页码从摘要开始编排）", sec.index + 1),
                    json!({"page_number": "不显示"}), json!({"page_number": "显示"}), true, src,
                    Some(Fix::CoverFooter { first }), None,
                );
            }
        }
    }
    // 摘要之前的内容与摘要在同一节：封面跟着编页码，摘要也不会从第一页开始
    let cover_split = first_numbered
        .filter(|_| pn.cover_no_number)
        .and_then(|f| cover_shared_title(model, &model.sections[f]).map(|t| (f, t)));
    if let Some((f, title)) = cover_split {
        let can = super::fix::can_split_before(doc, model.paras[title].el);
        ctx.add(
            "page_numbers.cover_shared", Severity::Error, "numbering", Some(title),
            format!(
                "封面 / 声明与「{}」在同一节（第{}节），会跟着一起编页码，摘要也不会从第 1 页开始；应在「{}」前插入分节符（下一页）",
                model.paras[title].text.trim(), f + 1, model.paras[title].text.trim()
            ),
            json!({}), json!({}), can, src, can.then_some(Fix::Split { para: title, kind: Split::Cover }), Some("abstract_title_zh"),
        );
    }
    // 页码在外侧有两种做法：开「奇偶页不同」分别设奇偶页页脚；或页码放在 xAlign="outside" 的图文框里（不需要奇偶页不同）
    let framed = pn.outside && model.sections.iter().any(|s| framed_outside(ctx, s.index));
    let odd_even = odd_even_on(ctx);
    if pn.outside && !odd_even && !framed {
        ctx.add(
            "page_numbers.outside", Severity::Error, "numbering", None,
            "页码应在外侧（奇数页居右、偶数页居左）：插入页码时选「外侧」，或开启「奇偶页不同」并分别设置奇数页、偶数页页脚；文档两者都没有".into(),
            json!({"odd_even_different": true}), json!({"odd_even_different": false}), false, src, None, None,
        );
    }
    let (mut front_seen, mut body_seen) = (false, false);
    for (sec, kind) in model.sections.iter().zip(kinds) {
        let Some(kind) = kind else { continue };
        let para = model.first_text_para(sec);
        let n = sec.index + 1;
        if kind == Kind::Mixed {
            let chapter = sec.paras.iter().copied().find(|&i| model.paras[i].role == Some("chapter") && !model.paras[i].ambiguous);
            let can = chapter.is_some_and(|c| super::fix::can_split_before(doc, model.paras[c].el));
            let split = Split::Body { restart_front: !front_seen, restart_body: !body_seen };
            ctx.add(
                "page_numbers.mixed_section", Severity::Error, "numbering", chapter.or(para),
                format!("第{n}节同时含摘要/目录与正文章节，无法分别设置罗马与阿拉伯页码，应在正文第一章前插入分节符（下一页）"),
                json!({}), json!({}), can, src, chapter.filter(|_| can).map(|c| Fix::Split { para: c, kind: split }), Some("chapter"),
            );
            (front_seen, body_seen) = (true, true);
            continue;
        }
        let front = kind == Kind::Front;
        if front && pn.front_no_number {
            front_seen = true;
            if cover_shows_number(ctx, sec.index) {
                ctx.add(
                    "page_numbers.front_none", Severity::Error, "numbering", para,
                    format!("第{n}节属于前置部分（摘要至目录），规范要求前置部分不编页码，页码从正文开始；该节页脚显示了页码"),
                    json!({"page_number": "不显示"}), json!({"page_number": "显示"}), false, src, None, None,
                );
            }
            continue;
        }
        let want = if front { pn.front_format } else { pn.body_format };
        let pgn = sec.el.and_then(|e| doc.wchild(e, "pgNumType"));
        let act = pgn.and_then(|e| doc.wattr(e, "fmt")).filter(|v| !v.is_empty()).unwrap_or("decimal");
        let part = if front { "前置部分（摘要至符号表）" } else { "正文部分" };
        let start = pgn.and_then(|e| doc.wattr(e, "start")).and_then(|v| v.trim().parse::<u32>().ok());
        let first_of_part = !(if front { front_seen } else { body_seen });
        (front_seen, body_seen) = (front_seen || front, body_seen || !front);
        let fix_start = |kind| Some(Fix::Sect { sec: sec.index, kind });
        match pn.restart_at {
            // 每部分第一节重新编号；第 1 节从 1 开始是默认行为。封面同节时由分节修复一并设置
            Some(at) if first_of_part && start != Some(at) && !(sec.index == 0 && at == 1 && start.is_none())
                && cover_split.is_none_or(|(f, _)| f != sec.index) =>
            {
                let now = start.map_or("接续上一节".to_string(), |s| format!("从 {s} 开始"));
                ctx.add(
                    "page_numbers.restart", Severity::Error, "numbering", para,
                    format!("第{n}节是{part}的第一节，页码应重新从 {at} 开始，实际{now}"),
                    json!({"start": at}), json!({"start": now}), true, src, fix_start(SectFix::Start(at)),
                    Some(if front { "abstract_title_zh" } else { "chapter" }),
                );
            }
            Some(_) if !first_of_part && start.is_some() => {
                ctx.add(
                    "page_numbers.continue", Severity::Error, "numbering", para,
                    format!("第{n}节页码重新从 {} 开始，{part}内应接续上一节连续编排", start.unwrap_or_default()),
                    json!({"start": "接续上一节"}), json!({"start": start}), true, src, fix_start(SectFix::Continue), None,
                );
            }
            _ => {}
        }
        if let Some(want) = want.filter(|w| w.as_str() != act) {
            ctx.add(
                "page_numbers.format", Severity::Error, "numbering", para,
                format!("第{n}节属于{part}，页码应为 {}，实际 {}", fmt_cn(want.as_str()), fmt_cn(act)),
                json!({"format": want.as_str()}), json!({"format": act}), true, src,
                Some(Fix::Sect { sec: sec.index, kind: SectFix::PgNum(want) }), None,
            );
        }
        // 页码在外侧：奇数页（默认页脚）居右、偶数页页脚居左；外侧图文框两者都满足。
        // 不开「奇偶页不同」时只有默认页脚，须放在外侧图文框里，单独报告（不能一键修复）
        if pn.outside && !odd_even && framed && !framed_outside(ctx, sec.index) {
            ctx.add(
                "page_numbers.outside", Severity::Error, "numbering", para,
                format!("第{n}节页码不在外侧：其他节的页码用了外侧图文框，本节没有；请在本节页脚重新插入页码并选「外侧」"),
                json!({"position": "outside"}), json!({"position": "not_outside"}), false, src, None, None,
            );
        }
        let sides: Vec<(&str, &str, Option<Align>)> = if pn.outside && odd_even {
            vec![("default", "奇数页", Some(Align::Right)), ("even", "偶数页", Some(Align::Left))]
        } else if pn.outside {
            vec![("default", "", None)]
        } else {
            vec![("default", "", pn.footer_align)]
        };
        for &(typ, side, align) in &sides {
            let found = footer_part(ctx, sec.index, typ).and_then(|(id, f)| page_field_para(f).map(|r| (id, f, r)));
            let Some((rid, footer, (fp, fr))) = found else {
                ctx.add(
                    "page_numbers.footer_missing", Severity::Error, "numbering", para,
                    format!("第{n}节{part}{side}页脚未找到页码域（PAGE）"), json!({}), json!({}), false, src, None, None,
                );
                continue;
            };
            let (sid, ppr) = model.para_eff_from_el(footer, fp);
            let (mut parts, mut exp, mut actd) = (vec![], Map::new(), Map::new());
            if let Some(want) = align {
                let a = page_number_pos(footer, fp, jc_norm(ppr.jc.as_deref()));
                if a != want.as_str() && !(pn.outside && a == "outside") {
                    parts.push(format!("页码应{}，实际{}", jc_cn(want.as_str()), jc_cn(a)));
                    exp.insert("align".into(), json!(want.as_str()));
                    actd.insert("align".into(), json!(a));
                }
            }
            if let Some(fr) = fr.filter(|_| pn.footer_latin_font.is_some() || pn.footer_size.is_some()) {
                let rpr = footer.wchild(fr, "rPr");
                let rs = rpr.and_then(|x| footer.wchild(x, "rStyle")).and_then(|s| footer.wattr(s, "val"));
                let mut eff = model.styles.r_base(sid.as_deref(), rs);
                super::model::read_rpr(footer, rpr, &mut eff);
                if let Some(want) = &pn.footer_latin_font {
                    let f = model.styles.font(&eff, ASCII).or_else(|| model.styles.font(&eff, HANSI));
                    if !same_font(f.as_deref(), Some(want)) {
                        parts.push(format!("页码字体应为 {want}，实际 {}", f.as_deref().unwrap_or("未确定")));
                        exp.insert("font".into(), json!(want));
                        actd.insert("font".into(), json!(f));
                    }
                }
                if let Some(want) = pn.footer_size {
                    let sz = eff.sz.unwrap_or(20) as f64 / 2.0;
                    if (sz - want).abs() > 0.01 {
                        parts.push(format!("页码字号应为 {}，实际 {}", size_name(want), size_name(sz)));
                        exp.insert("size".into(), json!(want));
                        actd.insert("size".into(), json!(sz));
                    }
                }
            }
            if !parts.is_empty() {
                ctx.add(
                    "page_numbers.footer_format", Severity::Error, "numbering", para,
                    format!("第{n}节{side}页脚{}", parts.join("；")), Value::Object(exp), Value::Object(actd), true, src,
                    Some(Fix::FooterFormat { rid: rid.to_string(), align }), None,
                );
            }
        }
    }
}

// ---------------------------------------------------------------- 页眉
/// 各部分的标题（与章同级）：页眉对应的“部分标题”，也是精确检查里页数统计的终点。
pub(super) const TITLE_ROLES: [&str; 9] = [
    "chapter", "abstract_title_zh", "abstract_title_en", "toc_title", "front_title", "references_title", "appendix_title",
    "ack_title", "end_title",
];

/// 向前继承取得该节 `typ`（default / even）类型的页眉：(定义它的节下标, rId)。
fn header_ref(ctx: &Ctx, sec_index: usize, typ: &str) -> Option<(usize, String)> {
    for i in (0..=sec_index).rev() {
        let Some(sp) = ctx.model.sections[i].el else { continue };
        for r in ctx.doc.children(sp).iter().copied().filter(|&c| ctx.doc.is_w(c, "headerReference")) {
            if ctx.doc.wattr(r, "type") == Some(typ) {
                return ctx.doc.attr(r, &Ns::R, "id").map(|id| (i, id.to_string()));
            }
        }
    }
    None
}

/// settings.xml 开启了「奇偶页不同」。
fn odd_even_on(ctx: &Ctx) -> bool {
    ctx.settings.is_some_and(|s| s.wchild(s.root(), "evenAndOddHeaders").is_some_and(|e| super::model::toggle(s, e)))
}

/// 页眉应有的内容。
#[derive(Clone, Copy)]
enum HeaderWant<'a> {
    Chapter,
    Text(&'a str),
}

fn header_want<'a>(content: Option<super::pack::HeaderContent>, text: Option<&'a str>) -> Option<HeaderWant<'a>> {
    content.map(|_| HeaderWant::Chapter).or(text.map(HeaderWant::Text))
}

struct HeaderInfo {
    /// 各非空段落的可见文字（含域结果），以空格相接。
    text: String,
    /// 域指令（fldSimple 与 fldChar 两种写法）。
    instrs: Vec<String>,
    /// 承载文字/域的第一个段落及其第一个相关 run。
    carrier: Option<(Id, Option<Id>)>,
}

fn field_instrs(hd: &Doc, desc: &[Id]) -> Vec<String> {
    let mut out: Vec<String> = desc
        .iter()
        .filter(|&&e| hd.is_w(e, "fldSimple"))
        .filter_map(|&e| hd.wattr(e, "instr").map(String::from))
        .collect();
    out.extend(complex_fields(hd, desc).into_iter().map(|(i, _)| i));
    out
}

/// fldChar 写法的域:(拼好的指令, begin 所在 run)。同一 run 里可以依次放 begin / instrText / end,按子元素顺序走状态机。
fn complex_fields(doc: &Doc, desc: &[Id]) -> Vec<(String, Id)> {
    let mut out = vec![];
    let mut cur: Option<(String, Id)> = None;
    for &r in desc.iter().filter(|&&e| doc.is_w(e, "r")) {
        for c in doc.child_els(r) {
            if doc.is_w(c, "fldChar") {
                match doc.wattr(c, "fldCharType") {
                    Some("begin") => cur = Some((String::new(), r)),
                    Some("separate" | "end") => out.extend(cur.take()),
                    _ => {}
                }
            } else if doc.is_w(c, "instrText") {
                if let Some((instr, _)) = &mut cur {
                    instr.push_str(&doc.text(c));
                }
            }
        }
    }
    out
}

fn parse_header(hd: &Doc) -> HeaderInfo {
    let (mut texts, mut instrs, mut carrier) = (vec![], vec![], None);
    for p in hd.descendants(hd.root()).into_iter().filter(|&p| hd.is_w(p, "p")) {
        let desc = hd.descendants(p);
        let text: String = desc.iter().filter(|&&t| hd.is_w(t, "t")).map(|&t| hd.text(t)).collect();
        let pi = field_instrs(hd, &desc);
        if text.trim().is_empty() && pi.is_empty() {
            continue;
        }
        if carrier.is_none() {
            let run = desc.iter().copied().find(|&r| {
                hd.is_w(r, "r")
                    && (hd.wchild(r, "fldChar").is_some()
                        || hd.child_els(r).any(|t| hd.is_w(t, "t") && !hd.text(t).trim().is_empty()))
            });
            carrier = Some((p, run));
        }
        if !text.trim().is_empty() {
            texts.push(text.trim().to_string());
        }
        instrs.extend(pi);
    }
    HeaderInfo { text: texts.join(" "), instrs, carrier }
}

/// 比较用：去掉全部空白（含全角空格）并转小写，“摘  要”与“摘要”等价。
fn cmp_norm(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_lowercase()
}

/// 样式名比较键：内置标题样式的本地化名（「标题 1」）与英文名（heading 1）等价。
fn style_key(s: &str) -> String {
    let n = cmp_norm(s);
    match n.strip_prefix("标题") {
        Some(r) => format!("heading{r}"),
        None => n,
    }
}

fn styleref_arg(instr: &str) -> Option<String> {
    let c = STYLEREF.captures(instr)?;
    Some(c.get(1).or(c.get(2))?.as_str().to_string())
}

fn check_headers(ctx: &mut Ctx) {
    let pack = ctx.pack;
    let Some(h) = &pack.headers else { return };
    let (model, headers) = (ctx.model, ctx.headers);
    let src = h.source.as_str();
    let odd_even = odd_even_on(ctx);
    if h.same_odd_even == Some(true) && odd_even {
        ctx.add(
            "headers.odd_even", Severity::Error, "page", None,
            "文档开启了「奇偶页不同」，奇偶页页眉可能不一致；规范要求奇偶页相同（可一键修复：取消「奇偶页不同」，各页统一使用奇数页的页眉页脚）"
                .into(),
            json!({"odd_even_different": false}), json!({"odd_even_different": true}), true, src, Some(Fix::OddEven), None,
        );
    }
    let even = h.even.as_ref().and_then(|e| header_want(e.content, e.text.as_deref()));
    if even.is_some() && !odd_even {
        ctx.add(
            "headers.odd_even", Severity::Error, "page", None,
            "规范要求奇数页、偶数页页眉不同，文档没有开启「奇偶页不同」（布局 → 页面设置 → 版式），请开启后分别设置奇数页、偶数页页眉".into(),
            json!({"odd_even_different": true}), json!({"odd_even_different": false}), false, src, None, None,
        );
    }
    let chapters: Vec<&Para> = model.paras.iter().filter(|p| p.role == Some("chapter") && !p.ambiguous).collect();
    let mut used: Vec<String> = vec![];
    for p in &chapters {
        let name = p.style_name.trim().to_string();
        if !name.is_empty() && !used.contains(&name) {
            used.push(name);
        }
    }
    let styleref_ok = |arg: &str| -> bool {
        if let Ok(n) = arg.parse::<u32>() {
            return n == 1
                && chapters.iter().any(|p| {
                    p.outline == Some(0) || style_key(&p.style_name) == "heading1"
                });
        }
        chapters.iter().any(|p| p.sid.as_deref() == Some(arg) || style_key(&p.style_name) == style_key(arg))
    };
    // 奇偶页不同时分别检查奇数页（默认页眉）与偶数页页眉
    let main = header_want(h.content, h.text.as_deref());
    let mut sides = vec![("default", if even.is_some() { "奇数页" } else { "" }, main)];
    if let Some(e) = even.filter(|_| odd_even) {
        sides.push(("even", "偶数页", Some(e)));
    }
    for (typ, side, want) in sides {
        let mut last_title: Option<String> = None;
        let mut prev_key: Option<Option<String>> = None;
        let mut reported: HashSet<(Option<String>, String)> = HashSet::new();
        for (sec, kind) in model.sections.iter().zip(classify_sections(model)) {
            let titles: Vec<String> = sec
                .paras
                .iter()
                .map(|&i| &model.paras[i])
                .filter(|p| !p.ambiguous && p.role.is_some_and(|r| TITLE_ROLES.contains(&r)))
                .map(|p| p.text.trim().to_string())
                .collect();
            let prev_title = last_title.clone();
            if let Some(t) = titles.last() {
                last_title = Some(t.clone());
            }
            // 封面、声明等摘要之前的节（None）不要求页眉；混合节由页码检查报告
            let Some(kind) = kind else { continue };
            if kind == Kind::Mixed {
                continue;
            }
            let para = model.first_text_para(sec);
            let n = sec.index + 1;
            let part = if kind == Kind::Front { "前置部分" } else { "正文部分" };
            let hdr_ref = header_ref(ctx, sec.index, typ);
            let key = hdr_ref.as_ref().map(|(_, id)| id.clone());
            let same_as_prev = prev_key.as_ref() == Some(&key);
            prev_key = Some(key.clone());
            let inherited = hdr_ref.as_ref().filter(|(o, _)| *o < sec.index).map(|(o, _)| o + 1);
            let info = hdr_ref.as_ref().and_then(|(_, id)| headers.get(id)).map(|d| (d, parse_header(d)));
            let info = info.filter(|(_, i)| !i.text.trim().is_empty() || !i.instrs.is_empty());
            if kind == Kind::Front && h.front_none {
                if let Some((_, i)) = info.filter(|_| !same_as_prev) {
                    ctx.add(
                        "headers.front_none", Severity::Error, "page", para,
                        format!("第{n}节属于前置部分（摘要至目录），规范要求前置部分不设页眉，页眉从正文开始；该节{side}页眉为「{}」", i.text.trim()),
                        json!({"text": ""}), json!({"text": i.text}), false, src, None, None,
                    );
                }
                continue;
            }
            let Some((hd, info)) = info else {
                if !same_as_prev {
                    let should = match want {
                        Some(HeaderWant::Text(t)) => format!("应为「{t}」"),
                        _ => "应为所在部分的章标题".into(),
                    };
                    ctx.add(
                        "headers.missing", Severity::Error, "page", para,
                        format!("第{n}节{part}缺少{side}页眉（{should}）"), json!({}), json!({}), false, src, None, None,
                    );
                }
                continue;
            };
            let shown = if info.text.trim().is_empty() { "无文字".to_string() } else { format!("「{}」", info.text) };
            match want {
                Some(HeaderWant::Text(t)) => {
                    if cmp_norm(&info.text) != cmp_norm(t) && reported.insert((key.clone(), t.to_string())) {
                        let msg = match inherited {
                            Some(o) => format!("第{n}节{side}页眉继承自第{o}节（链接到前一节），内容为{shown}，应为「{t}」"),
                            None => format!("第{n}节{side}页眉应为「{t}」，实际为{shown}"),
                        };
                        ctx.add(
                            "headers.content", Severity::Error, "page", para, msg,
                            json!({"text": t}), json!({"text": info.text}), false, src, None, None,
                        );
                    }
                }
                Some(HeaderWant::Chapter) => {
                    let refs: Vec<String> = info.instrs.iter().filter_map(|i| styleref_arg(i)).collect();
                    if !refs.is_empty() {
                        if !refs.iter().any(|a| styleref_ok(a)) && !same_as_prev {
                            let usedtxt = if used.is_empty() { "未使用样式".to_string() } else { used.join("、") };
                            ctx.add(
                                "headers.content", Severity::Error, "page", para,
                                format!(
                                    "第{n}节{side}页眉的 STYLEREF 域引用了样式「{}」，但章标题并未使用该样式（章标题样式：{usedtxt}），页眉将显示为空或错误内容",
                                    refs[0]
                                ),
                                json!({"styleref": used}), json!({"styleref": refs[0]}), false, src, None, None,
                            );
                        }
                    } else {
                        let hn = cmp_norm(&info.text);
                        let matches = |title: &str| {
                            !hn.is_empty() && (hn == cmp_norm(title) || hn == cmp_norm(&CHAP_PREFIX.replace(title.trim(), "")))
                        };
                        if titles.len() >= 2 {
                            if titles.iter().filter(|t| matches(t)).count() <= 1
                                && reported.insert((key.clone(), titles.join("|")))
                            {
                                ctx.add(
                                    "headers.content", Severity::Warning, "page", para,
                                    format!(
                                        "第{n}节含多个章（{}），静态{side}页眉只能对应一个；请每章分节并取消「链接到前一节」，或插入 STYLEREF 域",
                                        titles.join("、")
                                    ),
                                    json!({"titles": titles}), json!({"text": info.text}), false, src, None, None,
                                );
                            }
                        } else if let Some(exp) = titles.first().cloned().or(prev_title) {
                            if !matches(&exp) && reported.insert((key.clone(), exp.clone())) {
                                let msg = match inherited {
                                    Some(o) => format!(
                                        "第{n}节{side}页眉继承自第{o}节（链接到前一节），内容为{shown}，与本节章标题「{exp}」不一致；请在页眉编辑状态取消「链接到前一节」后修改"
                                    ),
                                    None => format!("第{n}节{side}页眉内容应为所在章标题「{exp}」，实际为{shown}"),
                                };
                                ctx.add(
                                    "headers.content", Severity::Error, "page", para, msg,
                                    json!({"text": exp}), json!({"text": info.text}), false, src, None, None,
                                );
                            }
                        }
                    }
                }
                None => {}
            }
            // 格式：共享同一页眉部件的连续各节只报一次
            let Some((hp, hr)) = info.carrier.filter(|_| !same_as_prev) else { continue };
            let (sid, ppr) = model.para_eff_from_el(hd, hp);
            let (mut parts, mut exp, mut actd) = (vec![], Map::new(), Map::new());
            if let Some(want) = h.align {
                let a = jc_norm(ppr.jc.as_deref());
                if a != want.as_str() {
                    parts.push(format!("{side}页眉应{}，实际{}", jc_cn(want.as_str()), jc_cn(a)));
                    exp.insert("align".into(), json!(want.as_str()));
                    actd.insert("align".into(), json!(a));
                }
            }
            if let Some(hr) = hr.filter(|_| h.cn_font.is_some() || h.latin_font.is_some() || h.size.is_some()) {
                let rpr = hd.wchild(hr, "rPr");
                let rs = rpr.and_then(|x| hd.wchild(x, "rStyle")).and_then(|s| hd.wattr(s, "val"));
                let mut eff = model.styles.r_base(sid.as_deref(), rs);
                super::model::read_rpr(hd, rpr, &mut eff);
                let blank = info.text.trim().is_empty();
                if let Some(want) = h.cn_font.as_deref().filter(|_| CN_CHAR.is_match(&info.text)) {
                    let f = model.styles.font(&eff, EAST_ASIA);
                    if f.is_none() || !same_font(f.as_deref(), Some(want)) {
                        parts.push(format!("{side}页眉中文字体应为 {want}，实际 {}", f.as_deref().unwrap_or("未确定")));
                        exp.insert("cn_font".into(), json!(want));
                        actd.insert("cn_font".into(), json!(f));
                    }
                }
                if let Some(want) = h.latin_font.as_deref().filter(|_| blank || LATIN_CHAR.is_match(&info.text)) {
                    let f = model.styles.font(&eff, ASCII).or_else(|| model.styles.font(&eff, HANSI));
                    if !same_font(f.as_deref(), Some(want)) {
                        parts.push(format!("{side}页眉西文字体应为 {want}，实际 {}", f.as_deref().unwrap_or("未确定")));
                        exp.insert("latin_font".into(), json!(want));
                        actd.insert("latin_font".into(), json!(f));
                    }
                }
                if let Some(want) = h.size {
                    let sz = eff.sz.unwrap_or(20) as f64 / 2.0;
                    if (sz - want).abs() > 0.01 {
                        parts.push(format!("{side}页眉字号应为 {}，实际 {}", size_name(want), size_name(sz)));
                        exp.insert("size".into(), json!(want));
                        actd.insert("size".into(), json!(sz));
                    }
                }
            }
            if !parts.is_empty() {
                ctx.add(
                    "headers.format", Severity::Error, "page", para,
                    format!("第{n}节{}", parts.join("；")), Value::Object(exp), Value::Object(actd), true, src,
                    hdr_ref.as_ref().map(|(_, id)| Fix::HeaderFormat { rid: id.clone() }), None,
                );
            }
        }
    }
}

// ---------------------------------------------------------------- 角色段落格式
fn first_size(model: &Model, doc: &Doc, para: usize) -> f64 {
    let p = &model.paras[para];
    p.runs
        .iter()
        .find(|r| !r.text.trim().is_empty())
        .map_or(12.0, |r| model.run_eff(doc, p, r).sz.unwrap_or(20) as f64 / 2.0)
}

fn category(role: &str) -> &'static str {
    if role.starts_with("chapter") || role.starts_with("section") {
        "heading"
    } else if matches!(role, "fig_caption" | "table_caption") {
        "caption"
    } else if role.starts_with("abstract") || role.starts_with("keywords") {
        "abstract"
    } else if matches!(role, "ref_item" | "references_title") {
        "reference"
    } else {
        "body"
    }
}

enum Act {
    Font(Option<String>),
    Sz(f64),
    B(bool),
}

fn font_group(ctx: &mut Ctx, pi: usize, role: &'static str, spec: &Role, label: &str) {
    let (model, doc) = (ctx.model, ctx.doc);
    let para = &model.paras[pi];
    let (want_cn, want_lat) = (spec.cn_font.as_deref(), spec.latin_font.as_deref());
    let (want_sz, want_b) = (spec.size, spec.bold);
    if want_cn.is_none() && want_lat.is_none() && want_sz.is_none() && want_b.is_none() {
        return;
    }
    // 下标：0 中文字体 / 1 西文字体 / 2 字号 / 3 加粗
    let mut vals: [Vec<String>; 4] = Default::default();
    let mut bad: [Option<Act>; 4] = Default::default();
    let (mut fixable, mut undetermined_only) = (false, true);
    for r in para.runs.iter().filter(|r| !r.text.trim().is_empty()) {
        let eff = model.run_eff(doc, para, r);
        let mut checks: Vec<(usize, Act, bool)> = vec![];
        if let Some(w) = want_cn.filter(|_| CN_CHAR.is_match(&r.text)) {
            let a = model.styles.font(&eff, EAST_ASIA);
            let ok = a.is_some() && same_font(a.as_deref(), Some(w));
            checks.push((0, Act::Font(a), ok));
        }
        if let Some(w) = want_lat.filter(|_| LATIN_CHAR.is_match(&r.text)) {
            let a = model.styles.font(&eff, ASCII).or_else(|| model.styles.font(&eff, HANSI));
            let ok = a.is_some() && same_font(a.as_deref(), Some(w));
            checks.push((1, Act::Font(a), ok));
        }
        if let Some(w) = want_sz {
            let a = eff.sz.unwrap_or(20) as f64 / 2.0;
            checks.push((2, Act::Sz(a), (a - w).abs() < 0.01));
        }
        if let Some(w) = want_b {
            let a = eff.b.unwrap_or(false);
            checks.push((3, Act::B(a), a == w));
        }
        for (key, act, ok) in checks {
            let id = match &act {
                Act::Font(f) => format!("{f:?}"),
                Act::Sz(v) => format!("{}", v.to_bits()),
                Act::B(b) => b.to_string(),
            };
            if !vals[key].contains(&id) {
                vals[key].push(id);
            }
            if !ok {
                fixable |= r.safe;
                if !matches!(act, Act::Font(None)) {
                    undetermined_only = false;
                }
                bad[key].get_or_insert(act);
            }
        }
    }
    if bad.iter().all(Option::is_none) {
        return;
    }
    let names = ["中文字体", "西文字体", "字号", "加粗"];
    let (mut parts, mut exp, mut actd, mut mixed) = (vec![], Map::new(), Map::new(), false);
    for (key, act) in bad.iter().enumerate() {
        let Some(act) = act else { continue };
        let (sw, sa) = match act {
            Act::Sz(a) => (size_name(want_sz.unwrap()), size_name(*a)),
            Act::B(a) => {
                let f = |b: bool| if b { "加粗" } else { "不加粗" }.to_string();
                (f(want_b.unwrap()), f(*a))
            }
            Act::Font(a) => {
                let w = if key == 0 { want_cn } else { want_lat };
                (w.unwrap().to_string(), a.clone().unwrap_or_else(|| "未确定".into()))
            }
        };
        parts.push(format!("{}应为 {sw}，实际 {sa}", names[key]));
        exp.insert(names[key].into(), json!(sw));
        actd.insert(names[key].into(), json!(sa));
        mixed |= vals[key].len() > 1;
    }
    let msg = format!("{label}{}{}", parts.join("；"), if mixed { "（段内格式不一致，仅报告第一处）" } else { "" });
    if mixed {
        actd.insert("混合".into(), json!(true));
    }
    ctx.add(
        &format!("role.{role}.font"),
        if undetermined_only { Severity::Warning } else { Severity::Error },
        category(role), Some(pi), msg, Value::Object(exp), Value::Object(actd), fixable, &spec.source,
        Some(Fix::Para { para: pi, group: Group::Font, role }), Some(role),
    );
}

/// 返回 (磅, 以行计的字符串)。
fn spacing_val(pt_v: Option<i64>, lines: Option<i64>) -> (Option<f64>, Option<String>) {
    match lines {
        Some(0) => (Some(0.0), None),
        Some(l) => (None, Some(format!("{} 行", g6(l as f64 / 100.0)))),
        None => (Some(pt_v.unwrap_or(0) as f64 / 20.0), None),
    }
}

fn align_group(ctx: &mut Ctx, pi: usize, role: &'static str, spec: &Role, label: &str) {
    let Some(want) = spec.align else { return };
    let a = jc_norm(ctx.model.paras[pi].ppr.jc.as_deref());
    if a != want.as_str() {
        ctx.add(
            &format!("role.{role}.align"), Severity::Error, category(role), Some(pi),
            format!("{label}对齐应为 {}，实际 {}", jc_cn(want.as_str()), jc_cn(a)),
            json!({"align": want.as_str()}), json!({"align": a}), true, &spec.source,
            Some(Fix::Para { para: pi, group: Group::Align, role }), Some(role),
        );
    }
}

fn line_show(rule: &str, v: f64) -> String {
    match rule {
        "exact" => format!("固定值 {} 磅", g6(v)),
        "atLeast" => format!("最小值 {} 磅", g6(v)),
        _ => format!("{} 倍行距", g6(v)),
    }
}

/// 行距、段前、段后与规则不符之处：(描述, 期望, 实际)。
fn spacing_diffs(ppr: &PProps, spec: &Role, has_math: bool) -> (Vec<String>, Map<String, Value>, Map<String, Value>) {
    let (mut parts, mut exp, mut actd) = (vec![], Map::new(), Map::new());
    if let Some(line) = spec.line.filter(|_| !has_math) {
        let arule = ppr.line_rule.as_deref().unwrap_or("auto");
        let aline = ppr.line.unwrap_or(240) as f64;
        let aval = if arule == "auto" { aline / 240.0 } else { aline / 20.0 };
        let tol = if line.rule == LineRule::Auto { 0.01 } else { 0.05 };
        if arule != line.rule.as_str() || (aval - line.value).abs() > tol {
            let (se, sa) = (line_show(line.rule.as_str(), line.value), line_show(arule, round2(aval)));
            parts.push(format!("行距应为 {se}，实际 {sa}"));
            exp.insert("line".into(), json!(se));
            actd.insert("line".into(), json!(sa));
        }
    }
    let sp = [
        ("before", "段前", spec.before, spec.before_lines, ppr.before, ppr.before_lines),
        ("after", "段后", spec.after, spec.after_lines, ppr.after, ppr.after_lines),
    ];
    for (key, cn, want, want_lines, v, lines) in sp {
        let (p, l) = spacing_val(v, lines);
        let shown = || l.clone().unwrap_or_else(|| pt(p.unwrap_or(0.0)));
        if let Some(want) = want {
            if p.is_none_or(|p| (p - want).abs() > 0.05) {
                parts.push(format!("{cn}应为 {}，实际 {}", pt(want), shown()));
                exp.insert(key.into(), json!(pt(want)));
                actd.insert(key.into(), json!(shown()));
            }
        } else if let Some(want) = want_lines {
            // 以行计：Word 用 beforeLines / afterLines（百分之一行）；0 行与 0 磅等价
            let ok = match lines.filter(|&x| x != 0) {
                Some(x) => (x as f64 / 100.0 - want).abs() <= 0.05,
                None => want == 0.0 && p.is_some_and(|p| p.abs() <= 0.05),
            };
            if !ok {
                let sw = format!("{} 行", g6(want));
                parts.push(format!("{cn}应为 {sw}，实际 {}", shown()));
                exp.insert(key.into(), json!(sw));
                actd.insert(key.into(), json!(shown()));
            }
        }
    }
    (parts, exp, actd)
}

fn spacing_group(ctx: &mut Ctx, pi: usize, role: &'static str, spec: &Role, label: &str) {
    let para = &ctx.model.paras[pi];
    // 段内有公式时行距可按需设置；独立成行的表达式本身须单倍行距
    let (parts, exp, actd) = spacing_diffs(&para.ppr, spec, para.has_math && para.role != Some("equation"));
    if !parts.is_empty() {
        let msg = format!("{label}{}", parts.join("；"));
        ctx.add(
            &format!("role.{role}.spacing"), Severity::Error, category(role), Some(pi), msg,
            Value::Object(exp), Value::Object(actd), true, &spec.source,
            Some(Fix::Para { para: pi, group: Group::Spacing, role }), Some(role),
        );
    }
}

fn indent_group(ctx: &mut Ctx, pi: usize, role: &'static str, spec: &Role, label: &str) {
    let para = &ctx.model.paras[pi];
    let ppr = &para.ppr;
    let none = spec.first_line_chars.is_none() && spec.hanging_chars.is_none() && spec.first_line_cm.is_none() && spec.hanging_cm.is_none();
    if none || ppr.num_pr == Some(true) {
        return;
    }
    let size = first_size(ctx.model, ctx.doc, pi);
    let nz = |v: Option<i64>| v.filter(|&x| x != 0);
    let chars = if let Some(fc) = nz(ppr.first_line_chars) {
        fc as f64 / 100.0
    } else if let Some(hc) = nz(ppr.hanging_chars) {
        -(hc as f64) / 100.0
    } else if let Some(h) = nz(ppr.hanging) {
        -(h as f64) / 20.0 / size
    } else if let Some(f) = nz(ppr.first_line) {
        f as f64 / 20.0 / size
    } else {
        0.0
    };
    // 以厘米计的要求：实际值换算成厘米比较（字符数按本段字号折算）
    let to_cm = |c: f64| c * size * 20.0 / CM;
    let act = if chars.abs() < 0.05 {
        "无缩进".to_string()
    } else if spec.first_line_cm.is_some() || spec.hanging_cm.is_some() {
        format!("{}缩进 {} 厘米", if chars > 0.0 { "首行" } else { "悬挂" }, fmt_g(to_cm(chars.abs()), 2))
    } else if chars > 0.0 {
        format!("首行缩进 {} 字符", fmt_g(chars, 2))
    } else {
        "悬挂缩进".to_string()
    };
    let fix = Some(Fix::Para { para: pi, group: Group::Indent, role });
    let first = spec.first_line_chars.map(|w| (chars - w).abs() > 0.05).or(spec.first_line_cm.map(|w| (to_cm(chars) - w).abs() > 0.05));
    if let Some(bad) = first {
        if bad {
            let sw = match (spec.first_line_chars, spec.first_line_cm) {
                (Some(w), _) | (None, Some(w)) if w == 0.0 => "无缩进".into(),
                (Some(w), _) => format!("首行缩进 {} 字符", g6(w)),
                (None, w) => format!("首行缩进 {} 厘米", g6(w.unwrap_or_default())),
            };
            ctx.add(
                &format!("role.{role}.indent"), Severity::Error, category(role), Some(pi),
                format!("{label}缩进应为 {sw}，实际 {act}"), json!({"indent": sw}), json!({"indent": act}), true,
                &spec.source, fix, Some(role),
            );
        }
    } else if let Some(h) = spec.hanging_cm {
        // 厘米计的悬挂缩进要求等于该值
        if chars > -0.05 || (to_cm(-chars) - h).abs() > 0.05 {
            let sw = format!("悬挂缩进 {} 厘米", g6(h));
            ctx.add(
                &format!("role.{role}.indent"), Severity::Error, category(role), Some(pi),
                format!("{label}缩进应为 {sw}，实际 {act}"), json!({"indent": sw}), json!({"indent": act}), true,
                &spec.source, fix, Some(role),
            );
        }
    } else if let Some(h) = spec.hanging_chars.filter(|_| chars >= -0.05) {
        let sw = format!("悬挂缩进 {} 字符", g6(h));
        ctx.add(
            &format!("role.{role}.indent"), Severity::Error, category(role), Some(pi),
            format!("{label}应为悬挂缩进，实际 {act}（一键修复设为悬挂 {} 字符）", g6(h)), json!({"indent": sw}), json!({"indent": act}), true,
            &spec.source, fix, Some(role),
        );
    }
}

fn check_roles(ctx: &mut Ctx) {
    let (pack, model) = (ctx.pack, ctx.model);
    for pi in 0..model.paras.len() {
        let para = &model.paras[pi];
        let Some(role) = para.role.filter(|_| !para.ambiguous) else { continue };
        let Some(spec) = pack.roles.get(role) else { continue };
        let label = role_label(role);
        font_group(ctx, pi, role, spec, label);
        align_group(ctx, pi, role, spec, label);
        spacing_group(ctx, pi, role, spec, label);
        indent_group(ctx, pi, role, spec, label);
    }
}

/// docGrid 吸附提示：倍数行距的段落所在节，若启用了网格，实际行距会被网格撑大。
fn check_grid(ctx: &mut Ctx) {
    let pack = ctx.pack;
    if !pack.roles.values().any(|s| s.line.is_some()) {
        return;
    }
    let (doc, model) = (ctx.doc, ctx.model);
    for sec in &model.sections {
        let Some(dg) = sec.el.and_then(|e| doc.wchild(e, "docGrid")) else { continue };
        let Some(ty) = doc.wattr(dg, "type").filter(|t| matches!(*t, "lines" | "linesAndChars")) else { continue };
        let hit = sec.paras.iter().copied().find(|&i| {
            let p = &model.paras[i];
            let line = p.role.and_then(|r| pack.roles.get(r)).is_some_and(|s| s.line.is_some());
            line && !p.ambiguous && p.ppr.line_rule.as_deref().unwrap_or("auto") == "auto"
        });
        let Some(hit) = hit else { continue };
        let source = pack.page.as_ref().map_or("", |p| p.source.as_str());
        ctx.add(
            "page.doc_grid", Severity::Info, "page", model.first_text_para(sec).or(Some(hit)),
            format!(
                "第{}节启用了文档网格（{ty}），该节使用倍数行距的段落会吸附网格，\
                 实际行距可能大于设定值；规范要求固定值行距时不受影响",
                sec.index + 1
            ),
            json!({}), json!({"docGrid": ty}), false, source, None, None,
        );
    }
}

// ---------------------------------------------------------------- 关键词 / 题注 / 标点
fn check_keywords(ctx: &mut Ctx) {
    let pack = ctx.pack;
    let Some(kw) = &pack.checks.keywords else { return };
    let model = ctx.model;
    for (pi, para) in model.paras.iter().enumerate() {
        if !matches!(para.role, Some("keywords_zh" | "keywords_en")) || para.ambiguous {
            continue;
        }
        let zh = para.role == Some("keywords_zh");
        let body = keywords_start(&para.text).map_or("", |s| &para.text[s..]);
        let count = KW_SPLIT.split(body).filter(|x| !x.trim().is_empty()).count();
        let sep = if zh { &kw.zh_separator } else { &kw.en_separator };
        let role = para.role.unwrap();
        let label = role_label(role);
        let range = match (kw.min_count, kw.max_count) {
            (Some(l), Some(h)) if count < l || count > h => Some((format!("应为 {l}–{h} 个"), json!({"min": l, "max": h}))),
            (Some(l), None) if count < l => Some((format!("不应少于 {l} 个"), json!({"min": l}))),
            (None, Some(h)) if count > h => Some((format!("不应超过 {h} 个"), json!({"max": h}))),
            _ => None,
        };
        if let Some((want, expected)) = range.filter(|_| count > 0) {
            ctx.add(
                "keywords.count", Severity::Error, "abstract", Some(pi),
                format!("{label}{want}，实际 {count} 个"),
                expected, json!({"count": count}), false, &kw.source, None, Some(role),
            );
        }
        let mut wrong: Vec<&str> = KW_SPLIT.find_iter(body).map(|m| m.as_str()).filter(|c| c != sep).collect();
        wrong.sort_unstable();
        wrong.dedup();
        if !wrong.is_empty() {
            ctx.add(
                "keywords.separator", Severity::Error, "abstract", Some(pi),
                format!("{label}应使用“{sep}”分隔，发现“{}”", wrong.join("”“")),
                json!({"separator": sep}), json!({"separators": wrong}), false, &kw.source, None, Some(role),
            );
            ctx.offer_text(pi, separator_edits(&para.text, sep, zh), "（需手动修改）");
        }
    }
}

/// 关键词里用错的分隔符连同两侧空白换成 `sep`；英文关键词分隔符后留一个空格。
/// 只隔着空白的几个分隔符（空关键词）合并成一个。
fn separator_edits(text: &str, sep: &str, zh: bool) -> Vec<(usize, usize, String)> {
    let Some(start) = keywords_start(text) else { return vec![] };
    let body = &text[start..];
    let ws = |c: char| c.is_whitespace();
    // (起, 止, 是否要改)：起止含分隔符两侧的空白
    let mut groups: Vec<(usize, usize, bool)> = vec![];
    for m in KW_SPLIT.find_iter(body) {
        let s = body[..m.start()].trim_end_matches(ws).len();
        let rest = &body[m.end()..];
        let e = m.end() + rest.len() - rest.trim_start_matches(ws).len();
        match groups.last_mut() {
            Some(g) if s <= g.1 => *g = (g.0, e, true),
            _ => groups.push((s, e, m.as_str() != sep)),
        }
    }
    groups
        .into_iter()
        .filter(|g| g.2)
        .map(|(s, e, _)| {
            let rep = if zh || e == body.len() { sep.to_string() } else { format!("{sep} ") };
            (edit::char_at(text, start + s), edit::char_at(text, start + e), rep)
        })
        .collect()
}

/// 一到九十九的中文数字。
fn cn_num(s: &str) -> Option<i64> {
    let d = |c: char| "一二三四五六七八九".chars().position(|x| x == c).map(|i| i as i64 + 1);
    let cs: Vec<char> = s.chars().collect();
    match cs[..] {
        ['十'] => Some(10),
        [a] => d(a),
        ['十', b] => Some(10 + d(b)?),
        [a, '十'] => Some(d(a)? * 10),
        [a, '十', b] => Some(d(a)? * 10 + d(b)?),
        _ => None,
    }
}

fn chap_no(text: &str) -> Option<i64> {
    let s = CHAP_NO.captures(text)?.get(1)?.as_str().to_string();
    s.parse::<i64>().ok().or_else(|| cn_num(&s))
}

/// 附录标题里的序号（附录 A、附录 1、附录 II……）；全角罗马数字 Ⅱ 换成 II。
fn appendix_letter(text: &str) -> Option<String> {
    let m = APPX_NO.captures(text.trim())?;
    let full = "ⅠⅡⅢⅣⅤⅥⅦⅧⅨⅩ".chars().position(|c| m[1].starts_with(c));
    let ascii = ["I", "II", "III", "IV", "V", "VI", "VII", "VIII", "IX", "X"];
    Some(full.map_or_else(|| m[1].to_string(), |i| ascii[i].to_string()))
}

/// 每段所在部分的编号键：正文章号（“3”）或附录字母（“A”）；第一章之前、无序号的附录为空串。
/// 图表与表达式按它编号（§2.3.19、§2.3.11）。
fn part_keys(model: &Model) -> Vec<String> {
    let mut cur = String::new();
    let mut chap = 0i64;
    model
        .paras
        .iter()
        .map(|p| {
            if !p.ambiguous {
                match p.role {
                    Some("chapter") => {
                        let t = p.text.trim();
                        let digits: String = t.chars().take_while(char::is_ascii_digit).collect();
                        let arabic = || CHAP_ARABIC.is_match(t).then(|| digits.parse().ok()).flatten();
                        let han = || CHAP_HAN.captures(t).and_then(|m| cn_num(&m[1]));
                        chap = chap_no(t).or_else(arabic).or_else(han).filter(|&n| n != 0).unwrap_or(chap + 1);
                        cur = chap.to_string();
                    }
                    // 图表编号里的附录序号只认字母（图 A.1）；数字序号会与章号混淆，不作编号键
                    Some("appendix_title") => {
                        cur = appendix_letter(&p.text).filter(|l| l.len() == 1 && l.as_bytes()[0].is_ascii_uppercase()).unwrap_or_default()
                    }
                    _ => {}
                }
            }
            cur.clone()
        })
        .collect()
}

/// 编号的“章”位与所在部分不符时的说明。
fn part_mismatch(want: &str, got: &str) -> Option<String> {
    if want.is_empty() || want == got {
        return None;
    }
    let appendix = want.parse::<i64>().is_err();
    Some(if appendix { format!("附录序号应为 {want}，实际 {got}") } else { format!("章号应为 {want}，实际 {got}") })
}

fn check_captions(ctx: &mut Ctx) {
    let pack = ctx.pack;
    let Some(cn) = &pack.checks.caption_numbering else { return };
    let model = ctx.model;
    let keys = part_keys(model);
    let mut last: HashMap<(String, String), i64> = HashMap::new();
    for (pi, para) in model.paras.iter().enumerate() {
        if !matches!(para.role, Some("fig_caption" | "table_caption")) {
            continue;
        }
        // 续图表沿用原编号，不参与编号连续性
        let Some(c) = parse_cap(para).filter(|c| !c.cont) else { continue };
        let (kind, (a, b)) = (c.kind.to_string(), c.num);
        let chap = &keys[pi];
        if a.is_empty() {
            // 全文连续编号：§2.3.19 要求按章编号
            let k = (kind.clone(), chap.clone());
            let want = last.get(&k).copied().unwrap_or(0) + 1;
            last.insert(k, want);
            if !chap.is_empty() {
                ctx.add(
                    "caption.number", Severity::Error, "numbering", Some(pi),
                    format!("{kind}题应按章编号（如 {kind}{chap}-{want}），实际 {kind}{b}"),
                    json!({"number": format!("{chap}-{want}")}), json!({"number": b.to_string()}), false,
                    &cn.source, None, para.role,
                );
            }
            continue;
        }
        let prev = last.get(&(kind.clone(), a.clone())).copied().unwrap_or(0);
        let mut problems: Vec<String> = part_mismatch(chap, &a).into_iter().collect();
        if b != prev + 1 {
            problems.push(format!("序号应为 {}，实际 {b}", prev + 1));
        }
        last.insert((kind.clone(), a.clone()), b);
        if !problems.is_empty() {
            ctx.add(
                "caption.number", Severity::Error, "numbering", Some(pi),
                format!("{kind}题编号不连续或章号不符：{}", problems.join("；")),
                json!({"number": format!("{chap}-{}", prev + 1)}), json!({"number": format!("{a}-{b}")}), false,
                &cn.source, None, para.role,
            );
        }
    }
}

/// 图 / 表题注：是否为续图表（及是否写成题注后加「（续）」）、类别、编号（章号或附录字母, 序）、编号之后的题名。
pub(super) struct Cap {
    pub cont: bool,
    pub suffix: bool,
    pub kind: &'static str,
    pub num: (String, i64),
    pub title: String,
}

impl Cap {
    /// 编号文字：章-序（2-1、A-1）；全文连续编号时只有序号。
    pub fn label(&self) -> String {
        match &self.num {
            (a, b) if a.is_empty() => b.to_string(),
            (a, b) => format!("{a}-{b}"),
        }
    }
}

pub(super) fn parse_cap(p: &Para) -> Option<Cap> {
    if !matches!(p.role, Some("fig_caption" | "table_caption")) {
        return None;
    }
    let t = p.text.trim();
    let (m, cont, flat) = if let Some(m) = CONT_CAP.captures(t) {
        (m, true, false)
    } else if let Some(m) = CONT_FLAT.captures(t) {
        (m, true, true)
    } else if let Some(m) = CAP.captures(t) {
        (m, false, false)
    } else {
        (CAP_FLAT.captures(t)?, false, true)
    };
    let kind = if &m[1] == "图" { "图" } else { "表" };
    let num = if flat { (String::new(), m[2].parse().unwrap_or(0)) } else { (m[2].to_string(), m[3].parse().unwrap_or(0)) };
    let mut title = t[m.get(0).unwrap().end()..].trim();
    // “表 2-1 ×××（续）”
    let sfx = CONT_SUFFIX.find(title).filter(|_| !cont);
    if let Some(s) = sfx {
        title = title[..s.start()].trim();
    }
    Some(Cap { cont: cont || sfx.is_some(), suffix: sfx.is_some(), kind, num, title: title.to_string() })
}

/// 每个续图 / 续表题注段，及它接续的原题注段（之前同类最近的非续题注）。
pub(super) fn continued_caps(model: &Model) -> Vec<(usize, Option<usize>)> {
    let (mut last, mut out) = (HashMap::new(), vec![]);
    for (pi, p) in model.paras.iter().enumerate() {
        let Some(c) = parse_cap(p) else { continue };
        if c.cont {
            out.push((pi, last.get(c.kind).copied()));
        } else {
            last.insert(c.kind, pi);
        }
    }
    out
}

/// 表第一行各单元格的文字（去空白）。
fn first_row(doc: &Doc, tbl: Id) -> Vec<String> {
    let Some(tr) = doc.child_els(tbl).find(|&e| doc.is_w(e, "tr")) else { return vec![] };
    doc.child_els(tr)
        .filter(|&e| doc.is_w(e, "tc"))
        .map(|tc| norm(&doc.descendants(tc).into_iter().filter(|&e| doc.is_w(e, "t")).map(|e| doc.text(e)).collect::<String>()))
        .collect()
}

/// 续表 / 续图类问题的处理提示前缀：要改的是正文文字，修复会被文字指纹守卫拦下。
pub(super) const BY_HAND: &str = "需手动修改（涉及正文文字，程序不会自动修复）";

/// 续表 / 续图题注应有的文字：「续表 2-1 ×××」或「表 2-1 ×××（续）」。
pub(super) fn continued_text(style: ContinuedStyle, kind: &str, num: &str, title: &str) -> String {
    match style {
        ContinuedStyle::Prefix => format!("续{kind} {num} {title}").trim_end().to_string(),
        ContinuedStyle::Suffix => format!("{kind} {num} {title}").trim_end().to_string() + "（续）",
    }
}

fn check_continued(ctx: &mut Ctx) {
    let (pack, model, doc) = (ctx.pack, ctx.model, ctx.doc);
    let Some(src) = &pack.checks.continued_caption else { return };
    let suffix = src.style == ContinuedStyle::Suffix;
    // 表题段 → 紧随其下的表
    let tables: HashMap<usize, &Table> =
        model.tables.iter().filter_map(|t| Some((table_target(model, doc, t)?.above?, t))).collect();
    for (at, orig) in continued_caps(model) {
        let (Some(c), text) = (parse_cap(&model.paras[at]), model.paras[at].text.trim()) else { continue };
        let (k, num) = (c.kind, c.label());
        let Some(oi) = orig else {
            ctx.add(
                "caption.continued", Severity::Error, "caption", Some(at),
                format!("续{k} {num} 前面没有对应的{k} {num}。{BY_HAND}：核对编号，或把它改回正式的“{k} {num}”题注"),
                json!({}), json!({"text": text}), false, &src.source, None, model.paras[at].role,
            );
            continue;
        };
        let Some(o) = parse_cap(&model.paras[oi]) else { continue };
        let onum = o.label();
        let mut problems = vec![];
        if c.num != o.num {
            problems.push(format!("编号应与前面的{k} {onum} 相同，实际 {num}"));
        }
        // 续图须注明图题；续表只要求表序前加“续”，写了表题则须一致；「（续）」写法总带表题
        if c.title.is_empty() {
            if (k == "图" || suffix) && !o.title.is_empty() {
                problems.push(format!("次页应注明{k}题（“{}”）", o.title));
            }
        } else if norm(&c.title) != norm(&o.title) {
            problems.push(format!("{k}题应与原{k}相同（“{}”），实际“{}”", o.title, c.title));
        }
        let want = continued_text(src.style, k, &onum, &o.title);
        if c.suffix != suffix {
            problems.push(if suffix { "应在题注后加“（续）”".to_string() } else { format!("应在{k}序前加“续”字") });
        }
        if !problems.is_empty() {
            ctx.add(
                "caption.continued", Severity::Error, "caption", Some(at),
                format!("续{k}与原{k}不一致：{}；题注应为“{want}”", problems.join("；")),
                json!({"text": want}), json!({"text": text}), false, &src.source, None, model.paras[at].role,
            );
            let full = &model.paras[at].text;
            let lead = edit::char_at(full, full.len() - full.trim_start().len());
            ctx.offer_text(at, edit::diff(text, &want, lead).into_iter().collect(), &format!("。{BY_HAND}"));
        }
        if let (Some(t), Some(ot)) = (tables.get(&at), tables.get(&oi)) {
            let (row, head_row) = (first_row(doc, t.el), first_row(doc, ot.el));
            if row != head_row {
                ctx.add(
                    "table.continued_header", Severity::Error, "table", Some(at),
                    format!(
                        "续表应重复表头：续表第一行为“{}”，原表表头为“{}”。{BY_HAND}：把原表的表头行复制为续表第一行",
                        head(&row.join(" | "), 30),
                        head(&head_row.join(" | "), 30)
                    ),
                    json!({"repeat_header": true}), json!({"repeat_header": false}), false, &src.source, None, None,
                );
            }
        }
    }
}

fn check_punctuation(ctx: &mut Ctx) {
    let (pack, model) = (ctx.pack, ctx.model);
    let Some(pc) = &pack.checks.punctuation else { return };
    for (pi, para) in model.paras.iter().enumerate() {
        if para.role != Some("body") || para.ambiguous || !CN_CHAR.is_match(&para.text) {
            continue;
        }
        let hp: Vec<_> = HALF_PUNCT.find_iter(&para.text).map(|m| m.as_str()).collect();
        if !hp.is_empty() {
            let ex: Vec<_> = hp.iter().take(3).map(|x| format!("“{}”", x.trim())).collect();
            ctx.add(
                "punct.halfwidth", Severity::Warning, "punctuation", Some(pi),
                format!("中文语境中出现 {} 处半角标点（如 {}），应使用全角标点", hp.len(), ex.join("、")),
                json!({}), json!({"count": hp.len()}), false, &pc.source, None, Some("body"),
            );
            ctx.offer_text(pi, halfwidth_edits(&para.text), "");
        }
        let fa: Vec<_> = FULL_ALNUM.find_iter(&para.text).collect();
        if !fa.is_empty() {
            ctx.add(
                "punct.fullwidth_alnum", Severity::Warning, "punctuation", Some(pi),
                format!("出现全角字母或数字（如 “{}”），应使用半角", fa[0].as_str()),
                json!({}), json!({"count": fa.len()}), false, &pc.source, None, Some("body"),
            );
            let edits = fa
                .iter()
                .map(|m| {
                    let half = m.as_str().chars().filter_map(|c| char::from_u32(c as u32 - 0xFEE0)).collect();
                    (edit::char_at(&para.text, m.start()), edit::char_at(&para.text, m.end()), half)
                })
                .collect();
            ctx.offer_text(pi, edits, "");
        }
    }
}

fn is_han(c: char) -> bool {
    ('\u{4e00}'..='\u{9fff}').contains(&c)
}

/// 中文语境里的半角标点换成全角（与 `HALF_PUNCT` 同一判据），成对的括号一起换；
/// 标点后（左括号前）多余的半角空格一并去掉。
fn halfwidth_edits(text: &str) -> Vec<(usize, usize, String)> {
    let cs: Vec<char> = text.chars().collect();
    let han = |i: Option<usize>| i.and_then(|i| cs.get(i)).is_some_and(|&c| is_han(c));
    let mut hit: Vec<bool> = (0..cs.len())
        .map(|i| {
            let (prev, next) = (han(i.checked_sub(1)), han(Some(i + 1)));
            match cs[i] {
                ',' | ';' | ':' | '?' | '!' => prev || next,
                '(' => prev,
                ')' => next,
                '.' => prev && cs.get(i + 1).is_none_or(|c| c.is_whitespace()),
                _ => false,
            }
        })
        .collect();
    // 括号配对：最近的另一半（中间没有别的括号）
    let paren = |i: usize, j: usize| matches!((cs[i], cs[j]), ('(', ')') | (')', '('));
    for i in 0..cs.len() {
        let other = match (hit[i], cs[i]) {
            (true, '(') => (i + 1..cs.len()).find(|&j| matches!(cs[j], '(' | ')')),
            (true, ')') => (0..i).rev().find(|&j| matches!(cs[j], '(' | ')')),
            _ => None,
        };
        if let Some(j) = other.filter(|&j| paren(i, j)) {
            hit[j] = true;
        }
    }
    let spaces = |r: &mut dyn Iterator<Item = &char>| r.take_while(|&&c| c == ' ').count();
    let mut out: Vec<(usize, usize, String)> = vec![];
    for i in (0..cs.len()).filter(|&i| hit[i]) {
        let full = match cs[i] {
            ',' => '，',
            ';' => '；',
            ':' => '：',
            '?' => '？',
            '!' => '！',
            '(' => '（',
            ')' => '）',
            _ => '。',
        };
        let (s, e) = if cs[i] == '(' {
            let last = out.last().map_or(0, |e| e.1);
            ((i - spaces(&mut cs[..i].iter().rev())).max(last), i + 1)
        } else {
            (i, i + 1 + spaces(&mut cs[i + 1..].iter()))
        };
        out.push((s, e, full.to_string()));
    }
    out
}

/// 英文摘要里的中文标点换成英文标点；句读、右括号后紧跟字母数字时补一个空格，左括号前同理。
fn en_punct_edits(text: &str) -> Vec<(usize, usize, String)> {
    let cs: Vec<char> = text.chars().collect();
    let alnum = |i: Option<usize>| i.and_then(|i| cs.get(i)).is_some_and(|c| c.is_alphanumeric() && !is_han(*c));
    let mut out = vec![];
    for (i, &c) in cs.iter().enumerate() {
        let ascii = match c {
            '，' | '、' => ",",
            '。' => ".",
            '；' => ";",
            '：' => ":",
            '！' => "!",
            '？' => "?",
            '（' => "(",
            '）' => ")",
            '“' | '”' | '《' | '》' => "\"",
            '‘' | '’' => "'",
            '【' => "[",
            '】' => "]",
            _ => continue,
        };
        let rep = match ascii {
            "(" if alnum(i.checked_sub(1)) => format!(" {ascii}"),
            "," | "." | ";" | ":" | "!" | "?" | ")" if alnum(Some(i + 1)) => format!("{ascii} "),
            _ => ascii.to_string(),
        };
        out.push((i, i + 1, rep));
    }
    out
}

// ---------------------------------------------------------------- 表 / 图 / 表达式
pub(super) fn blank(p: &Para) -> bool {
    p.text.trim().is_empty() && !p.has_figure && !p.has_math
}

pub(super) fn set_snippet(ctx: &mut Ctx, s: String) {
    ctx.out.last_mut().unwrap().0.location.snippet = s;
}

fn head(s: &str, n: usize) -> String {
    s.trim().chars().take(n).collect()
}

/// 版式表（含公式或图片，如公式编号用的无框表格）不是数据表，不检查。
fn is_layout(doc: &Doc, tbl: Id) -> bool {
    doc.descendants(tbl)
        .into_iter()
        .any(|e| doc.is(e, &Ns::M, "oMath") || doc.is_w(e, "drawing") || doc.is_w(e, "pict"))
}

/// 单元格 / 脚注里一个段落与规则不符之处；字体无法确定时不判违规。`doc` 是段落所在部件。
fn para_problems(ctx: &Ctx, doc: &Doc, p: Id, tsid: Option<&str>, spec: &Role) -> Vec<String> {
    let model = ctx.model;
    let (ppr, runs) = model.cell_para(doc, p, tsid);
    if runs.is_empty() {
        return vec![];
    }
    let mut out = vec![];
    for (text, eff) in &runs {
        let fonts = [
            ("中文字体", spec.cn_font.as_deref().filter(|_| CN_CHAR.is_match(text)), model.styles.font(eff, EAST_ASIA)),
            (
                "西文字体",
                spec.latin_font.as_deref().filter(|_| LATIN_CHAR.is_match(text)),
                model.styles.font(eff, ASCII).or_else(|| model.styles.font(eff, HANSI)),
            ),
        ];
        for (name, want, actual) in fonts {
            if let (Some(w), Some(a)) = (want, actual) {
                if !same_font(Some(&a), Some(w)) {
                    out.push(format!("{name}应为 {w}，实际 {a}"));
                }
            }
        }
        if let Some(w) = spec.size {
            let a = eff.sz.unwrap_or(20) as f64 / 2.0;
            if (a - w).abs() > 0.01 {
                out.push(format!("字号应为 {}，实际 {}", size_name(w), size_name(a)));
            }
        }
    }
    if let Some(want) = spec.align {
        let a = jc_norm(ppr.jc.as_deref());
        // §2.3.19：一般居中，不宜居中的可两端对齐
        if a != want.as_str() && a != "justify" {
            out.push(format!("对齐应为 {}，实际 {}", jc_cn(want.as_str()), jc_cn(a)));
        }
    }
    out.extend(spacing_diffs(&ppr, spec, false).0);
    out.dedup();
    out
}

/// 被检查的表：表题（上 / 下）、提示定位的段落与标识文字。正文之前的表和版式表返回 None。
pub(super) struct TableTarget {
    pub above: Option<usize>,
    pub below: Option<usize>,
    pub anchor: usize,
    pub label: String,
}

pub(super) fn table_target(model: &Model, doc: &Doc, t: &Table) -> Option<TableTarget> {
    let near = t.near.filter(|&n| model.paras[n].chapter.is_some())?;
    if is_layout(doc, t.el) {
        return None;
    }
    // 表题：紧邻表上方（空段不计）；表下方的表题只用于给出更准确的提示
    let is_cap = |j: usize| model.paras[j].role == Some("table_caption");
    let above = t.before.and_then(|b| (0..=b).rev().find(|&j| !blank(&model.paras[j]))).filter(|&j| is_cap(j));
    let below = t.after.filter(|&a| is_cap(a));
    let anchor = above.or(below).unwrap_or(near);
    let label = match above.or(below) {
        Some(c) => head(&model.paras[c].text, 40),
        None => {
            let first = doc.descendants(t.el).into_iter().filter(|&e| doc.is_w(e, "t")).map(|e| doc.text(e));
            format!("无表题的表（首个文字：{}）", head(&first.into_iter().find(|s| !s.trim().is_empty()).unwrap_or_default(), 20))
        }
    };
    Some(TableTarget { above, below, anchor, label })
}

fn check_tables(ctx: &mut Ctx) {
    let (pack, model, doc) = (ctx.pack, ctx.model, ctx.doc);
    let cfg = &pack.checks;
    for (ti, t) in model.tables.iter().enumerate() {
        let Some(TableTarget { above, below, anchor, label }) = table_target(model, doc, t) else { continue };
        if let (Some(src), None) = (&cfg.table_caption, above) {
            let (msg, act) = if below.is_some() {
                ("表题应置于表的上方，当前在表下方", "表下方")
            } else {
                ("表上方缺少表题（表序与表题应置于表的上方）", "无表题")
            };
            ctx.add(
                "table.caption", Severity::Error, "table", Some(anchor), msg.into(), json!({"position": "表上方"}),
                json!({"position": act}), false, &src.source, None, None,
            );
            set_snippet(ctx, label.clone());
        }
        if let Some(tl) = &cfg.table_lines {
            let lines = model.styles.table_lines(doc, t.el);
            let (h, v) = (&lines.horizontal, &lines.vertical);
            if h.len() >= 2 {
                let nr = h.len() - 1;
                let vis = |es: &[Edge]| es.iter().any(|e| e.visible);
                let mut problems = vec![];
                if !vis(&h[0]) {
                    problems.push("缺少顶线");
                }
                if !vis(&h[nr]) {
                    problems.push("缺少底线");
                }
                if nr >= 2 && !vis(&h[1]) {
                    problems.push("表头行下缺少横线");
                }
                if v.iter().any(|r| r.first().is_some_and(|e| e.visible) || r.last().is_some_and(|e| e.visible)) {
                    problems.push("存在左/右边竖线");
                }
                if v.iter().any(|r| r.len() > 2 && vis(&r[1..r.len() - 1])) {
                    problems.push("存在内部竖线");
                }
                if !problems.is_empty() {
                    ctx.add(
                        "table.three_line", Severity::Error, "table", Some(anchor),
                        format!("表应采用三线表：{}", problems.join("；")), json!({"style": "三线表"}),
                        json!({"problems": problems}), true, &tl.source, Some(Fix::TableLines { table: ti }), None,
                    );
                    set_snippet(ctx, label.clone());
                }
                let mut ruled = vec![("顶线", &h[0], tl.top_bottom_pt), ("底线", &h[nr], tl.top_bottom_pt)];
                if nr >= 2 {
                    ruled.push(("表头线", &h[1], tl.header_pt));
                }
                let mut bad = vec![];
                for (name, es, want) in ruled {
                    let Some(e) = es.iter().filter(|e| e.visible).find(|e| {
                        !e.single || e.sz.is_some_and(|sz| (sz as f64 - want * 8.0).abs() > 1.0)
                    }) else {
                        continue;
                    };
                    bad.push(if !e.single {
                        format!("{name}应为单直线")
                    } else {
                        format!("{name}线宽应为 {} 磅，实际 {} 磅", g6(want), g6(e.sz.unwrap() as f64 / 8.0))
                    });
                }
                if !bad.is_empty() {
                    ctx.add(
                        "table.line_width", Severity::Error, "table", Some(anchor), format!("三线表线型/线宽不符：{}", bad.join("；")),
                        json!({"top_bottom_pt": tl.top_bottom_pt, "header_pt": tl.header_pt}), json!({"problems": bad}), true,
                        &tl.source, Some(Fix::TableLines { table: ti }), None,
                    );
                    set_snippet(ctx, label.clone());
                }
            }
        }
        if let Some(spec) = &cfg.table_cell {
            let (mut bad, mut first) = (0, None);
            // §2.3.19「居中书写（上下居中，左右居中）」：要求居中时单元格也须上下居中
            let style_valign = model.styles.table_valign(t.style.as_deref());
            for (r, tr) in doc.child_els(t.el).filter(|&e| doc.is_w(e, "tr")).enumerate() {
                for (c, tc) in doc.child_els(tr).filter(|&e| doc.is_w(e, "tc")).enumerate() {
                    let mut probs: Vec<String> = vec![];
                    let paras: Vec<Id> = doc.child_els(tc).filter(|&e| doc.is_w(e, "p")).collect();
                    for &p in &paras {
                        for s in para_problems(ctx, doc, p, t.style.as_deref(), spec) {
                            if !probs.contains(&s) {
                                probs.push(s);
                            }
                        }
                    }
                    let has_text = paras.iter().any(|&p| !model.cell_para(doc, p, t.style.as_deref()).1.is_empty());
                    if spec.align == Some(Align::Center) && has_text {
                        let direct = doc.wchild(tc, "tcPr").and_then(|x| doc.wchild(x, "vAlign")).and_then(|v| doc.wattr(v, "val"));
                        let v = direct.map(String::from).or_else(|| style_valign.clone()).unwrap_or_else(|| "top".into());
                        if v != "center" {
                            probs.push(format!("应上下居中，实际{}", match v.as_str() { "bottom" => "底端对齐", _ => "顶端对齐" }));
                        }
                    }
                    if !probs.is_empty() {
                        bad += 1;
                        first.get_or_insert((r + 1, c + 1, probs));
                    }
                }
            }
            if let Some((r, c, probs)) = first {
                ctx.add(
                    "table.cell_format", Severity::Error, "table", Some(anchor),
                    format!("表内文字有 {bad} 个单元格格式不符（如第 {r} 行第 {c} 列：{}）", probs.join("；")),
                    json!({}), json!({"cells": bad, "first": format!("第 {r} 行第 {c} 列")}), true, &spec.source,
                    Some(Fix::TableCells { table: ti }), None,
                );
                set_snippet(ctx, label);
            }
        }
    }
}

fn check_figures(ctx: &mut Ctx) {
    let (pack, model) = (ctx.pack, ctx.model);
    let Some(src) = &pack.checks.figure_caption else { return };
    let paras = &model.paras;
    let skip = |j: usize| blank(&paras[j]) || paras[j].has_figure || SUB_CAP.is_match(paras[j].text.trim());
    let is_cap = |j: Option<usize>| j.is_some_and(|j| paras[j].role == Some("fig_caption"));
    let mut seen = HashSet::new();
    for (pi, p) in paras.iter().enumerate() {
        if !p.has_figure || p.chapter.is_none() {
            continue;
        }
        let next = (pi + 1..paras.len()).find(|&j| !skip(j));
        // 上方的图题若紧跟在另一张图下面，则属于那张图
        let prev = (0..pi).rev().find(|&j| !skip(j));
        let above = prev.filter(|&j| {
            is_cap(Some(j)) && !(0..j).rev().find(|&k| !blank(&paras[k])).is_some_and(|k| paras[k].has_figure)
        });
        if is_cap(next) || !seen.insert(next) {
            continue;
        }
        let (msg, act) = if above.is_some() {
            ("图题应置于图的下方，当前在图上方", "图上方")
        } else {
            ("图下方缺少图题（图序与图题应置于图的下方）", "无图题")
        };
        let before = (0..pi).rev().find(|&j| !paras[j].text.trim().is_empty());
        ctx.add(
            "figure.caption", Severity::Error, "figure", Some(pi), msg.into(), json!({"position": "图下方"}),
            json!({"position": act}), false, &src.source, None, None,
        );
        let at = before.map_or(String::new(), |j| format!("（位于“{}”之后）", head(&paras[j].text, 20)));
        set_snippet(ctx, format!("插图{at}"));
    }
}

fn check_equations(ctx: &mut Ctx) {
    let (pack, model, doc) = (ctx.pack, ctx.model, ctx.doc);
    let Some(src) = &pack.checks.equation_number else { return };
    let keys = part_keys(model);
    let mut last = HashMap::<String, i64>::new();
    for (pi, p) in model.paras.iter().enumerate() {
        // 独立成行的表达式：段内除序号外没有文字（角色识别时已判定）
        if p.role != Some("equation") {
            continue;
        }
        let text: String = p.text.chars().filter(|c| !c.is_whitespace()).collect();
        // 序号在文字里（制表位 + 序号）或在公式内（Word 的 “#(序号)” 写法）
        let math: String = doc
            .descendants(p.el)
            .into_iter()
            .filter(|&e| doc.is(e, &Ns::M, "t"))
            .map(|e| doc.text(e))
            .collect::<String>()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        let no = if !text.is_empty() { Some(text) } else { EQ_TAIL.find(&math).map(|m| m.as_str().to_string()) };
        let problem = match no.as_deref().map(|n| (n, EQ_NO.captures(n))) {
            None => Some("表达式缺少序号（序号应加括号置于表达式右边行末）".to_string()),
            Some((n, None)) => Some(format!("序号应为（章-序）形式，实际 {n}")),
            Some((_, Some(m))) => {
                let (a, b) = (m[1].to_string(), m[2].parse::<i64>().unwrap_or(0));
                let prev = last.get(&a).copied().unwrap_or(0);
                last.insert(a.clone(), b);
                let mut bad: Vec<String> = part_mismatch(&keys[pi], &a).into_iter().collect();
                if b != prev + 1 {
                    bad.push(format!("序号应为 {}，实际 {b}", prev + 1));
                }
                (!bad.is_empty()).then(|| format!("表达式序号不符：{}", bad.join("；")))
            }
        };
        if let Some(msg) = problem {
            ctx.add(
                "equation.number", Severity::Error, "numbering", Some(pi), msg, json!({"number": "（章-序）"}),
                json!({"number": no}), false, &src.source, None, None,
            );
            set_snippet(ctx, "表达式".into());
        }
    }
}

/// 独立成行的表达式：居中书写，或另起一段空两个汉字符；全文只能用一种（§2.3.19）。
fn check_equation_layout(ctx: &mut Ctx) {
    let (pack, model, doc) = (ctx.pack, ctx.model, ctx.doc);
    let Some(src) = &pack.checks.equation_layout else { return };
    let (mut centred, mut left) = (vec![], vec![]);
    for (pi, p) in model.paras.iter().enumerate() {
        if p.role != Some("equation") {
            continue;
        }
        // 公式段（oMathPara）按公式自己的对齐，缺省居中；只有行内公式时看段落对齐
        let math_jc = doc.descendants(p.el).into_iter().find(|&e| doc.is(e, &Ns::M, "oMathPara")).map(|op| {
            doc.descendants(op)
                .into_iter()
                .find(|&e| doc.is(e, &Ns::M, "jc"))
                .and_then(|j| doc.attr(j, &Ns::M, "val"))
                .unwrap_or("centerGroup")
        });
        let centre = match math_jc {
            Some(v) => !matches!(v, "left" | "right"),
            None => jc_norm(p.ppr.jc.as_deref()) == "center",
        };
        if centre { centred.push(pi) } else { left.push(pi) }
    }
    if centred.is_empty() || left.is_empty() {
        return;
    }
    let (few, name, other) = if left.len() <= centred.len() { (&left, "左对齐（缩进）", "居中") } else { (&centred, "居中", "左对齐（缩进）") };
    ctx.add(
        "equation.layout", Severity::Warning, "body", Some(few[0]),
        format!(
            "表达式排法全文应统一：{} 个居中、{} 个左对齐（缩进）。此处等 {} 个为{name}，与多数（{other}）不同",
            centred.len(), left.len(), few.len()
        ),
        json!({"layout": other}), json!({"centred": centred.len(), "indented": left.len()}), false, &src.source, None,
        Some("equation"),
    );
    set_snippet(ctx, "表达式".into());
}

/// 摘要标题之后、关键词或下一个部分标题之前：(标题段, 结束段)。
fn abstract_range(model: &Model, title: &str, keywords: &str) -> Option<(usize, usize)> {
    let start = model.paras.iter().position(|p| p.role == Some(title) && !p.ambiguous)?;
    let end = (start + 1..model.paras.len())
        .find(|&j| model.paras[j].role.is_some_and(|r| r == keywords || TITLE_ROLES.contains(&r)))
        .unwrap_or(model.paras.len());
    Some((start, end))
}

fn keyword_count(p: &Para) -> usize {
    let body = keywords_start(&p.text).map_or("", |s| &p.text[s..]);
    KW_SPLIT.split(body).filter(|x| !x.trim().is_empty()).count()
}

/// §2.3.5：中文摘要篇幅、摘要中无图表、英文摘要用英文标点、中英文关键词对应。
fn check_abstract(ctx: &mut Ctx) {
    let (pack, model) = (ctx.pack, ctx.model);
    let Some(ab) = &pack.checks.abstract_text else { return };
    let src = ab.source.as_str();
    let zh = abstract_range(model, "abstract_title_zh", "keywords_zh");
    let en = abstract_range(model, "abstract_title_en", "keywords_en");
    let body = |s: usize, e: usize| (s + 1..e).filter(|&j| model.paras[j].role == Some("abstract_body"));
    if let Some((s, e)) = zh {
        let n: usize = body(s, e).map(|j| model.paras[j].text.chars().filter(|c| !c.is_whitespace()).count()).sum();
        let range = match (ab.zh_min_chars, ab.zh_max_chars) {
            (Some(l), Some(h)) if n < l || n > h => Some(format!("{l}–{h}")),
            (Some(l), None) if n < l => Some(format!("不少于 {l}")),
            (None, Some(h)) if n > h => Some(format!("不超过 {h}")),
            _ => None,
        };
        if let Some(range) = range.filter(|_| n > 0) {
            // 规范写「一般」「约」时只作提示
            let (sev, tail) = if ab.chars_approx { (Severity::Info, "（规范为大致要求，仅供参考）") } else { (Severity::Warning, "") };
            ctx.add(
                "abstract.length", sev, "abstract", Some(s),
                format!("中文摘要正文约 {n} 字（不计空白），应控制在 {range} 字{tail}"),
                json!({"chars": range}), json!({"chars": n}), false, src, None, Some("abstract_title_zh"),
            );
        }
    }
    if ab.no_figures {
        for (s, e) in [zh, en].into_iter().flatten() {
            let fig = (s + 1..e).find(|&j| model.paras[j].has_figure);
            let tbl = model.tables.iter().find_map(|t| t.near.filter(|&n| n >= s && n < e));
            if let Some(at) = fig.or(tbl) {
                ctx.add(
                    "abstract.figure", Severity::Error, "abstract", Some(at),
                    format!("{}中不应出现图片、图表、表格或其他插图材料", role_label(model.paras[s].role.unwrap_or("abstract_title_zh"))),
                    json!({}), json!({"figure": fig.is_some(), "table": tbl.is_some()}), false, src, None, None,
                );
            }
        }
    }
    if let Some((s, e)) = en.filter(|_| ab.en_punctuation) {
        for j in body(s, e) {
            let mut found: Vec<&str> = FULL_PUNCT.find_iter(&model.paras[j].text).map(|m| m.as_str()).collect();
            found.sort_unstable();
            found.dedup();
            if !found.is_empty() {
                ctx.add(
                    "abstract.en_punct", Severity::Error, "punctuation", Some(j),
                    format!("英文摘要应使用英文标点，发现中文标点“{}”", found.join("”“")),
                    json!({}), json!({"punct": found}), false, src, None, Some("abstract_body"),
                );
                ctx.offer_text(j, en_punct_edits(&model.paras[j].text), "（需手动修改）");
            }
        }
    }
    if ab.keywords_match {
        let kw = |r: &str| model.paras.iter().position(|p| p.role == Some(r) && !p.ambiguous);
        if let (Some(z), Some(e)) = (kw("keywords_zh"), kw("keywords_en")) {
            let (nz, ne) = (keyword_count(&model.paras[z]), keyword_count(&model.paras[e]));
            if nz != ne {
                ctx.add(
                    "keywords.match", Severity::Warning, "abstract", Some(e),
                    format!("英文关键词（{ne} 个）应与中文关键词（{nz} 个）一一对应"),
                    json!({"count": nz}), json!({"count": ne}), false, src, None, Some("keywords_en"),
                );
            }
        }
    }
}

/// §2.3.9.2：章序号采用阿拉伯数字。
fn check_chapter_numbers(ctx: &mut Ctx) {
    let (pack, model) = (ctx.pack, ctx.model);
    let Some(src) = &pack.checks.chapter_number else { return };
    for (pi, p) in model.paras.iter().enumerate() {
        if p.role != Some("chapter") || p.ambiguous {
            continue;
        }
        let text = p.text.trim();
        let Some(m) = CHAP_NO.captures(text) else { continue };
        if m[1].parse::<i64>().is_err() {
            let n = chap_no(text);
            let ns = n.map_or("N".into(), |n| n.to_string());
            ctx.add(
                "chapter.number", Severity::Error, "numbering", Some(pi),
                format!("章序号应采用阿拉伯数字：“{}”中的“{}”应写作“{ns}”", &m[0], &m[1]),
                json!({"number": format!("第 {ns} 章")}), json!({"number": &m[0]}), false, &src.source, None,
                Some("chapter"),
            );
            let lead = edit::char_at(&p.text, p.text.len() - p.text.trim_start().len());
            let g = m.get(1).unwrap();
            let edits = n.map(|n| (lead + edit::char_at(text, g.start()), lead + edit::char_at(text, g.end()), n.to_string()));
            ctx.offer_text(pi, edits.into_iter().collect(), "（需手动修改）");
        }
    }
}

/// 第 `k` 个（0 起）附录的序号：字母 A、B……，数字 1、2……，罗马数字 I、II……，或中文数字一、二……
fn appendix_numeral(style: AppendixStyle, k: usize) -> String {
    let n = k + 1;
    match style {
        AppendixStyle::Letter => char::from(b'A' + (k.min(25) as u8)).to_string(),
        AppendixStyle::Arabic => n.to_string(),
        AppendixStyle::Roman => {
            let table = [(10, "X"), (9, "IX"), (5, "V"), (4, "IV"), (1, "I")];
            let (mut rest, mut out) = (n, String::new());
            for (v, s) in table {
                while rest >= v {
                    out.push_str(s);
                    rest -= v;
                }
            }
            out
        }
        AppendixStyle::Han => {
            let d = |i: usize| "一二三四五六七八九".chars().nth(i - 1).unwrap().to_string();
            match n {
                1..=9 => d(n),
                10 => "十".into(),
                11..=19 => format!("十{}", d(n - 10)),
                _ => format!("{}十{}", d(n / 10), if n.is_multiple_of(10) { String::new() } else { d(n % 10) }),
            }
        }
    }
}

/// §2.3.11：附录依次编序号（默认大写字母 A、B、C……），只有一个附录也要编号。
fn check_appendix_numbers(ctx: &mut Ctx) {
    let (pack, model) = (ctx.pack, ctx.model);
    let Some(src) = &pack.checks.appendix_number else { return };
    let how = match src.style {
        AppendixStyle::Letter => "大写字母",
        AppendixStyle::Arabic => "阿拉伯数字",
        AppendixStyle::Roman => "罗马数字",
        AppendixStyle::Han => "中文数字",
    };
    let first = appendix_numeral(src.style, 0);
    let mut k = 0;
    for (pi, p) in model.paras.iter().enumerate() {
        if p.role != Some("appendix_title") || p.ambiguous {
            continue;
        }
        let w = appendix_numeral(src.style, k);
        let problem = match appendix_letter(&p.text) {
            None => Some(format!("附录应编序号（只有一个附录时也要编为“附录 {first}”），此处应为“附录 {w}”")),
            Some(l) if l != w => Some(format!("附录应依次用{how}编序号，此处应为“附录 {w}”，实际“附录 {l}”")),
            _ => None,
        };
        if let Some(msg) = problem {
            ctx.add(
                "appendix.number", Severity::Error, "numbering", Some(pi), msg,
                json!({"number": format!("附录 {w}")}), json!({"text": p.text.trim()}), false, &src.source, None,
                Some("appendix_title"),
            );
            ctx.offer_text(pi, appendix_edits(p, &w), "（需手动修改）");
        }
        k += 1;
    }
}

/// 附录标题里的序号改成 `w`：有大写字母的换字母；没有的把“附录”后的数字、中文数字或小写字母换成 `w`，
/// 什么都没有则在“附录”后补上。序号由自动编号生成的段落不改。
fn appendix_edits(p: &Para, w: &str) -> Vec<(usize, usize, String)> {
    let text = p.text.trim();
    let lead = edit::char_at(&p.text, p.text.len() - p.text.trim_start().len());
    if let Some(g) = APPX_NO.captures(text).and_then(|m| m.get(1)) {
        return vec![(lead + edit::char_at(text, g.start()), lead + edit::char_at(text, g.end()), w.to_string())];
    }
    let Some(rest) = text.strip_prefix("附录").filter(|_| p.ppr.num_pr != Some(true)) else { return vec![] };
    let body = rest.trim_start();
    let mut tok: usize = body.chars().take_while(|c| c.is_ascii_digit() || "一二三四五六七八九十".contains(*c)).map(char::len_utf8).sum();
    let mut cs = body.chars();
    if tok == 0 && cs.next().is_some_and(|c| c.is_ascii_lowercase()) && !cs.next().is_some_and(|c| c.is_ascii_alphabetic()) {
        tok = 1;
    }
    let tail = &body[tok..];
    let end = text.len() - tail.trim_start().len();
    let rep = if tail.trim().is_empty() { format!(" {w}") } else { format!(" {w} ") };
    vec![(lead + 2, lead + edit::char_at(text, end), rep)]
}

/// 正文里引用到的图编号（章号或附录字母, 序）→ 首次引用所在段。“图 2-1～2-3”这类范围展开。
fn figure_citations(model: &Model) -> HashMap<(String, i64), usize> {
    let mut first = HashMap::new();
    for (j, p) in model.paras.iter().enumerate() {
        if p.chapter.is_none() || matches!(p.role, Some("fig_caption" | "table_caption" | "toc_chapter" | "toc_item")) {
            continue;
        }
        for m in FIG_REF.find_iter(&p.text) {
            let s = m.as_str();
            let mut prev: Option<(String, i64, usize)> = None;
            for c in NUM_PAIR.captures_iter(s) {
                let (a, b, at) = (c[1].to_string(), c[2].parse::<i64>().unwrap_or(0), c.get(0).unwrap().start());
                if let Some((pa, pb, end)) = &prev {
                    let ranged = s[*end..at].contains(['~', '～', '至', '到']);
                    if ranged && *pa == a && b > *pb && b - pb <= 50 {
                        for k in pb + 1..b {
                            first.entry((a.clone(), k)).or_insert(j);
                        }
                    }
                }
                first.entry((a.clone(), b)).or_insert(j);
                prev = Some((a, b, c.get(0).unwrap().end()));
            }
        }
    }
    first
}

/// §2.3.19：图宜紧置于首次引用该图的文字之后。
fn check_figure_citations(ctx: &mut Ctx) {
    let (pack, model) = (ctx.pack, ctx.model);
    let Some(src) = &pack.checks.figure_citation else { return };
    let first = figure_citations(model);
    for (pi, p) in model.paras.iter().enumerate() {
        // 全文连续编号（图 1）已由 caption.number 报告，引用按“章-序”匹配，这里跳过
        let Some(c) = parse_cap(p).filter(|c| c.kind == "图" && !c.cont && !c.num.0.is_empty()) else { continue };
        let num = c.label();
        let msg = match first.get(&c.num) {
            None => format!("图 {num} 未在正文中被引用；图宜紧置于首次引用该图的文字之后"),
            Some(&j) if j > pi => format!(
                "图 {num} 位于首次引用它的文字（「{}」）之前；图宜紧置于首次引用该图的文字之后",
                head(&model.paras[j].text, 20)
            ),
            _ => continue,
        };
        ctx.add(
            "figure.citation", Severity::Warning, "figure", Some(pi), msg, json!({}), json!({}),
            false, &src.source, None, Some("fig_caption"),
        );
    }
}

/// 方括号内的引用序号；含 0、过大的数或过长的范围时不当作文献引用（如区间 [0, 1]）。
fn cite_numbers(inner: &str) -> Option<Vec<i64>> {
    let mut out = vec![];
    let mut range_from: Option<i64> = None;
    let mut rest = inner;
    loop {
        let digits = rest.trim_start();
        let len = digits.find(|c: char| !c.is_ascii_digit()).unwrap_or(digits.len());
        let n: i64 = digits[..len].parse().ok()?;
        if n == 0 || n > 2000 {
            return None;
        }
        match range_from.take() {
            Some(a) if n > a && n - a <= 100 => out.extend(a + 1..=n),
            Some(_) => return None,
            None => out.push(n),
        }
        let after = digits[len..].trim_start();
        let Some(sep) = after.chars().next() else { break };
        if matches!(sep, '-' | '–' | '—' | '~' | '～') {
            range_from = Some(n);
        }
        rest = &after[sep.len_utf8()..];
    }
    Some(out)
}

/// 第 3 章顺序编码制：参考文献表连续编号，正文引用与表一一对应，按首次引用先后编号。
fn check_citations(ctx: &mut Ctx) {
    let (pack, model) = (ctx.pack, ctx.model);
    let Some(src) = &pack.checks.citations else { return };
    let entries: Vec<(usize, i64)> = model
        .paras
        .iter()
        .enumerate()
        .filter(|(_, p)| p.role == Some("ref_item") && !p.ambiguous)
        .filter_map(|(i, p)| Some((i, REF_NO.captures(p.text.trim())?[1].parse().ok()?)))
        .collect();
    // 没有 [n] 编号的文献表按著者-出版年制处理，不检查
    if entries.is_empty() {
        return;
    }
    let src = src.source.as_str();
    if let Some((k, &(pi, n))) = entries.iter().enumerate().find(|(k, (_, n))| *n != *k as i64 + 1) {
        ctx.add(
            "citation.list", Severity::Error, "reference", Some(pi),
            format!("参考文献表应按 [1]、[2]……连续编号：第 {} 条为 [{n}]（需手动修改）", k + 1),
            json!({"number": k + 1}), json!({"number": n}), false, src, None, Some("ref_item"),
        );
    }
    let known: HashSet<i64> = entries.iter().map(|&(_, n)| n).collect();
    let (mut first, mut order, mut missing) = (HashMap::new(), vec![], vec![]);
    for (j, p) in model.paras.iter().enumerate() {
        if p.chapter.is_none() || matches!(p.role, Some("ref_item" | "references_title" | "toc_chapter" | "toc_item")) {
            continue;
        }
        for c in CITE.captures_iter(&p.text) {
            for n in cite_numbers(&c[1]).unwrap_or_default() {
                if !known.contains(&n) {
                    missing.push((j, n));
                } else if let std::collections::hash_map::Entry::Vacant(v) = first.entry(n) {
                    v.insert(j);
                    order.push(n);
                }
            }
        }
    }
    if let Some(&(j, n)) = missing.first() {
        let mut nums: Vec<i64> = missing.iter().map(|&(_, n)| n).collect();
        nums.sort_unstable();
        nums.dedup();
        ctx.add(
            "citation.missing", Severity::Error, "reference", Some(j),
            format!("正文引用了参考文献表中没有的文献 [{n}]（共 {} 个序号：{}）", nums.len(), cite_list(&nums)),
            json!({}), json!({"numbers": nums}), false, src, None, None,
        );
    }
    let mut next = 1;
    for &n in &order {
        if n == next {
            next += 1;
            continue;
        }
        ctx.add(
            "citation.order", Severity::Warning, "reference", Some(first[&n]),
            format!("顺序编码制应按正文首次引用的先后编号：此处首次引用 [{n}]，但 [{next}] 还未被引用过（需手动调整序号）"),
            json!({"number": next}), json!({"number": n}), false, src, None, None,
        );
        break;
    }
    let uncited: Vec<(usize, i64)> = entries.iter().copied().filter(|(_, n)| !first.contains_key(n)).collect();
    if let Some(&(pi, _)) = uncited.first() {
        let nums: Vec<i64> = uncited.iter().map(|&(_, n)| n).collect();
        ctx.add(
            "citation.uncited", Severity::Warning, "reference", Some(pi),
            format!(
                "{} 条参考文献未在正文中引用：{}。参考文献表应与正文引用一一对应；只阅读未引用的文献可列入附录“书目”",
                nums.len(), cite_list(&nums)
            ),
            json!({}), json!({"numbers": nums}), false, src, None, Some("ref_item"),
        );
    }
}

fn cite_list(nums: &[i64]) -> String {
    let shown: Vec<String> = nums.iter().take(10).map(|n| format!("[{n}]")).collect();
    if nums.len() > 10 { format!("{}……", shown.join("、")) } else { shown.join("、") }
}

/// footnotes.xml 里真正的脚注（不含分隔线）的全部段落：(脚注 id, 段落)。
pub(super) fn note_paras(fdoc: &Doc) -> Vec<(String, Id)> {
    fdoc.child_els(fdoc.root())
        .filter(|&f| fdoc.is_w(f, "footnote") && fdoc.wattr(f, "type").is_none_or(|t| t == "normal"))
        .flat_map(|f| {
            let id = fdoc.wattr(f, "id").unwrap_or_default().to_string();
            fdoc.descendants(f).into_iter().filter(|&p| fdoc.is_w(p, "p")).map(move |p| (id.clone(), p))
        })
        .collect()
}

/// 带圈数字的脚注编号格式；写入时用第一个（Word 设置「①, ②, ③…」保存的值）。
pub(super) const CIRCLED: [&str; 2] = ["decimalEnclosedCircle", "decimalEnclosedCircleChinese"];

/// §2.3.9.4：脚注小五、宋体 / Times New Roman、两端对齐、单倍行距、段前后 0、悬挂 1.5 字符；序号 ①②③ 按页编排。
fn check_footnotes(ctx: &mut Ctx) {
    let (pack, model, doc) = (ctx.pack, ctx.model, ctx.doc);
    let (Some(fnr), Some(fdoc)) = (&pack.checks.footnotes, ctx.footnotes) else { return };
    let notes = note_paras(fdoc);
    if notes.is_empty() {
        return;
    }
    // 脚注 id → 正文里引用它的段
    let mut cited_in: HashMap<String, usize> = HashMap::new();
    for (pi, p) in model.paras.iter().enumerate() {
        for r in doc.descendants(p.el).into_iter().filter(|&e| doc.is_w(e, "footnoteReference")) {
            cited_in.entry(doc.wattr(r, "id").unwrap_or_default().to_string()).or_insert(pi);
        }
    }
    let anchor = |id: &str| cited_in.get(id).copied();
    let src = fnr.source.as_str();
    if let Some(spec) = &fnr.format {
        let (mut bad, mut first): (HashSet<&str>, Option<(&str, Vec<String>)>) = (HashSet::new(), None);
        for (id, p) in &notes {
            let mut probs = para_problems(ctx, fdoc, *p, None, spec);
            if let Some(h) = spec.hanging_chars {
                let (_, ppr) = model.para_eff_from_el(fdoc, *p);
                let want_tw = h * spec.size.unwrap_or(9.0) * 20.0;
                let ok = ppr.hanging_chars == Some((h * 100.0).round() as i64)
                    || (ppr.hanging_chars.is_none() && ppr.hanging.is_some_and(|v| (v as f64 - want_tw).abs() <= 20.0));
                if !ok && !model.cell_para(fdoc, *p, None).1.is_empty() {
                    probs.push(format!("悬挂缩进应为 {} 字符", g6(h)));
                }
            }
            if let Some(h) = spec.hanging_cm {
                let (_, ppr) = model.para_eff_from_el(fdoc, *p);
                let ok = ppr.hanging_chars.is_none_or(|c| c == 0) && ppr.hanging.is_some_and(|v| (v as f64 / CM - h).abs() <= 0.05);
                if !ok && !model.cell_para(fdoc, *p, None).1.is_empty() {
                    probs.push(format!("悬挂缩进应为 {} 厘米", g6(h)));
                }
            }
            if !probs.is_empty() {
                bad.insert(id);
                first.get_or_insert((id, probs));
            }
        }
        if let Some((id, probs)) = first {
            ctx.add(
                "footnote.format", Severity::Error, "body", anchor(id),
                format!("{} 条脚注格式不符（如引用于此段的脚注：{}）", bad.len(), probs.join("；")),
                json!({}), json!({"notes": bad.len()}), true, src, Some(Fix::FootnoteFormat), None,
            );
            set_snippet(ctx, "脚注".into());
        }
    }
    if fnr.circled || fnr.restart_each_page {
        // Word 按各节 sectPr 的 footnotePr 编号，缺省为阿拉伯数字连续编号；settings 里的不生效（Word 16 实测）
        let fpr = |sec: Option<Id>, tag: &str| -> Option<String> {
            let pr = doc.wchild(sec?, "footnotePr")?;
            doc.wchild(pr, tag).and_then(|e| doc.wattr(e, "val")).map(String::from)
        };
        let mut probs: Vec<String> = vec![];
        for sec in &model.sections {
            let fmt = fpr(sec.el, "numFmt").unwrap_or_else(|| "decimal".into());
            let restart = fpr(sec.el, "numRestart").unwrap_or_else(|| "continuous".into());
            if fnr.circled && !CIRCLED.contains(&fmt.as_str()) {
                probs.push(format!("序号应为 ①②③ 带圈数字，实际为 {}", if fmt == "decimal" { "1, 2, 3" } else { fmt.as_str() }));
            }
            if fnr.restart_each_page && restart != "eachPage" {
                probs.push(format!("序号应每页重新编号，实际为{}", if restart == "eachSect" { "每节重新编号" } else { "全文连续编号" }));
            }
        }
        probs.sort();
        probs.dedup();
        if !probs.is_empty() {
            ctx.add(
                "footnote.numbering", Severity::Error, "body", anchor(&notes[0].0),
                format!("脚注{}", probs.join("；")), json!({"note_format": "①②③", "restart": "每页"}), json!({"problems": probs}),
                true, src, Some(Fix::FootnoteNumbering), None,
            );
            set_snippet(ctx, "脚注".into());
        }
    }
}

/// 模板里括号中的格式说明（「（三号黑体）」「（宋体小四，1.5 倍行距）」）：不属于任何规范条文，提交前应删掉。
fn check_template_notes(ctx: &mut Ctx) {
    let model = ctx.model;
    for (pi, p) in model.paras.iter().enumerate().filter(|(_, p)| p.note) {
        let notes: Vec<_> = FMT_NOTE.find_iter(&p.text).collect();
        let shown = notes.iter().take(2).map(|m| format!("“{}”", m.as_str())).collect::<Vec<_>>().join("、");
        let whole = super::roles::strip_notes(&p.text).is_some_and(|s| s.is_empty());
        ctx.add(
            "template.note", Severity::Warning, "body", Some(pi),
            format!("疑似模板里的格式说明文字未删除：{shown}"),
            json!({}), json!({"text": shown}), false, "通用检查：提交的论文不应保留模板里的格式说明文字", None, p.role,
        );
        if whole {
            ctx.out.last_mut().unwrap().0.message.push_str("。整段都是说明，请删除这一段");
        } else {
            let edits = notes.iter().map(|m| (edit::char_at(&p.text, m.start()), edit::char_at(&p.text, m.end()), String::new())).collect();
            ctx.offer_text(pi, edits, "（需手动删除）");
        }
    }
}

fn check_manual(ctx: &mut Ctx) {
    let pack = ctx.pack;
    for (i, m) in pack.manual.iter().enumerate() {
        // 精确检查跑过且规则包配置了对应项时，由它替代人工确认
        if ctx.layout.is_some() && m.replaced_by.as_deref().is_some_and(|k| pack.layout.has(k)) {
            continue;
        }
        ctx.add(
            &format!("manual.{}", i + 1), Severity::Manual, "manual", None, m.text.clone(), json!({}), json!({}), false,
            &m.source, None, None,
        );
    }
}

fn check_layout(ctx: &mut Ctx) {
    if let Some(lay) = ctx.layout {
        layout::run(ctx, lay);
    }
}

pub fn run_checks(model: &Model, doc: &Doc, pack: &Pack, parts: &Parts, layout: Option<&Layout>) -> Vec<(Finding, Option<Fix>)> {
    let mut ctx = Ctx {
        model,
        doc,
        pack,
        footers: &parts.footers,
        headers: &parts.headers,
        settings: parts.settings.as_ref(),
        footnotes: parts.footnotes.as_ref(),
        layout,
        out: vec![],
    };
    // 精确检查的结果排在静态检查之后（只多不少），可修复项的编号才不受是否跑过精确检查影响
    let steps: [fn(&mut Ctx); 22] = [
        check_page, check_page_numbers, check_headers, check_roles, check_grid, check_keywords, check_captions,
        check_continued, check_tables, check_figures, check_equations, check_equation_layout, check_punctuation,
        check_abstract, check_chapter_numbers, check_appendix_numbers, check_figure_citations, check_citations,
        check_template_notes, check_footnotes, check_layout, check_manual,
    ];
    steps.iter().for_each(|f| f(&mut ctx));
    for (n, (f, _)) in ctx.out.iter_mut().enumerate() {
        f.id = format!("f{}", n + 1);
    }
    ctx.out
}
