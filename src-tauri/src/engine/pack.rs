//! 规则包加载与校验：YAML -> 强类型。未知字段一律报错（serde deny_unknown_fields）。

use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::OnceLock;

const SIZE_NAMES: [(&str, f64); 16] = [
    ("初号", 42.0), ("小初", 36.0), ("一号", 26.0), ("小一", 24.0), ("二号", 22.0), ("小二", 18.0),
    ("三号", 16.0), ("小三", 15.0), ("四号", 14.0), ("小四", 12.0), ("五号", 10.5), ("小五", 9.0),
    ("六号", 7.5), ("小六", 6.5), ("七号", 5.5), ("八号", 5.0),
];

pub const ROLE_LABELS: [(&str, &str); 23] = [
    ("chapter", "章标题"), ("section_1", "一级节标题"), ("section_2", "二级节标题"), ("section_3", "三级节标题"),
    ("body", "正文"), ("abstract_title_zh", "中文摘要标题"), ("abstract_title_en", "英文摘要标题"),
    ("abstract_body", "摘要正文"), ("keywords_zh", "中文关键词"), ("keywords_en", "英文关键词"),
    ("toc_title", "目录标题"), ("toc_chapter", "目录章标题行"), ("toc_item", "目录节标题行"),
    ("front_title", "插图清单 / 符号说明等前置部分标题"), ("references_title", "参考文献标题"),
    ("ref_item", "参考文献条目"), ("ack_title", "致谢标题"), ("appendix_title", "附录标题"),
    ("fig_caption", "图题"), ("table_caption", "表题"), ("table_source", "表注 / 资料来源"), ("equation", "表达式"),
    ("end_title", "声明 / 个人简历等部分标题"),
];

pub fn role_label(role: &str) -> &'static str {
    ROLE_LABELS.iter().find(|(r, _)| *r == role).map_or("", |(_, l)| l)
}

/// 字号（pt）的中文名，没有则 `12pt` 形式。
pub fn size_name(pt: f64) -> String {
    match SIZE_NAMES.iter().find(|(_, v)| *v == pt) {
        Some((n, _)) => n.to_string(),
        None => format!("{}pt", super::checks::fmt_g(pt, 6)),
    }
}

#[derive(Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    Left,
    Center,
    Right,
    Justify,
    /// 分散对齐。
    Distribute,
}

impl Align {
    pub fn as_str(self) -> &'static str {
        match self {
            Align::Left => "left",
            Align::Center => "center",
            Align::Right => "right",
            Align::Justify => "justify",
            Align::Distribute => "distribute",
        }
    }
}

#[derive(Deserialize, Clone, Copy, PartialEq, Debug)]
pub enum LineRule {
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "exact")]
    Exact,
    #[serde(rename = "atLeast")]
    AtLeast,
}

impl LineRule {
    pub fn as_str(self) -> &'static str {
        match self {
            LineRule::Auto => "auto",
            LineRule::Exact => "exact",
            LineRule::AtLeast => "atLeast",
        }
    }
}

#[derive(Deserialize, Clone, Copy, PartialEq, Debug)]
pub enum PageFmt {
    #[serde(rename = "upperRoman")]
    UpperRoman,
    #[serde(rename = "lowerRoman")]
    LowerRoman,
    #[serde(rename = "decimal")]
    Decimal,
}

impl PageFmt {
    pub fn as_str(self) -> &'static str {
        match self {
            PageFmt::UpperRoman => "upperRoman",
            PageFmt::LowerRoman => "lowerRoman",
            PageFmt::Decimal => "decimal",
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawSize {
    Num(f64),
    Name(String),
}

/// 字号：数字（pt）、中文字号名或 `12pt`。
fn size<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    let bad = |v: &dyn std::fmt::Debug| {
        serde::de::Error::custom(format!("字号 {v:?} 无法识别（用中文字号名或 12pt）"))
    };
    match RawSize::deserialize(d)? {
        RawSize::Num(n) => Ok(n),
        RawSize::Name(s) => {
            if let Some((_, v)) = SIZE_NAMES.iter().find(|(n, _)| *n == s) {
                return Ok(*v);
            }
            s.strip_suffix("pt").and_then(|n| n.trim_end().parse::<f64>().ok()).ok_or_else(|| bad(&s))
        }
    }
}

fn opt_size<'de, D: Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
    size(d).map(Some)
}

fn nonempty<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let s = String::deserialize(d)?;
    if s.is_empty() {
        return Err(serde::de::Error::custom("应为非空字符串"));
    }
    Ok(s)
}

fn opt_nonempty<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    nonempty(d).map(Some)
}

/// 规则包所属单位类型。
#[derive(Deserialize, Serialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum PackKind {
    University,
    Journal,
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct PackMeta {
    #[serde(deserialize_with = "nonempty")]
    pub id: String,
    #[serde(deserialize_with = "nonempty")]
    pub title: String,
    #[serde(deserialize_with = "nonempty")]
    pub version: String,
    #[serde(deserialize_with = "nonempty")]
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_from: Option<String>,
    /// 学校 / 期刊名称：模板库按它搜索、分组。
    #[serde(default, deserialize_with = "opt_nonempty", skip_serializing_if = "Option::is_none")]
    pub org: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<PackKind>,
    /// 适用范围，如「研究生」「本科」「投稿」。
    #[serde(default, deserialize_with = "opt_nonempty", skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// 已逐条对照该单位的规范原文核对；false 表示只从模板提取。
    #[serde(default)]
    pub verified: bool,
    /// 用户导入的模板（加载时设置，YAML 里不能写）。
    #[serde(default, skip_deserializing, skip_serializing_if = "std::ops::Not::not")]
    pub user: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Margins {
    pub top: Option<f64>,
    pub bottom: Option<f64>,
    pub left: Option<f64>,
    pub right: Option<f64>,
    /// 规范写的是下限（「不小于」）：实际不小于这些值即可，修复仍改成这些值。
    #[serde(default)]
    pub at_least: bool,
}

/// 封面类页面（摘要之前、不编页码的节）另行规定的页边距。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverMargins {
    #[serde(deserialize_with = "nonempty")]
    pub name: String,
    #[serde(deserialize_with = "nonempty")]
    pub source: String,
    pub top: f64,
    pub bottom: f64,
    pub left: f64,
    pub right: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Page {
    #[serde(deserialize_with = "nonempty")]
    pub source: String,
    pub paper: Option<Paper>,
    pub margins_cm: Option<Margins>,
    /// 摘要之前的节：页边距符合正文要求或其中任一组即可，页眉页脚距不检查。
    #[serde(default)]
    pub cover_margins_cm: Vec<CoverMargins>,
    pub header_distance_cm: Option<f64>,
    pub footer_distance_cm: Option<f64>,
}

#[derive(Deserialize, Clone, Copy, PartialEq)]
pub enum Paper {
    A4,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageNumbers {
    #[serde(deserialize_with = "nonempty")]
    pub source: String,
    pub front_format: Option<PageFmt>,
    pub body_format: Option<PageFmt>,
    pub footer_align: Option<Align>,
    #[serde(default, deserialize_with = "opt_nonempty")]
    pub footer_latin_font: Option<String>,
    #[serde(default, deserialize_with = "opt_size")]
    pub footer_size: Option<f64>,
    /// 前置部分、正文各自从这个页码重新开始（各部分第一节的 `pgNumType@start`）。
    pub restart_at: Option<u32>,
    /// 摘要之前的封面、声明等页不显示页码。
    #[serde(default)]
    pub cover_no_number: bool,
    /// 前置部分（摘要至目录、符号表）也不编页码，页码从正文开始。
    #[serde(default)]
    pub front_no_number: bool,
    /// 页码在外侧：奇数页居右、偶数页居左（需开启「奇偶页不同」）；与 `footer_align` 二选一。
    #[serde(default)]
    pub outside: bool,
}

/// 页眉内容来源。
#[derive(Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum HeaderContent {
    /// 页眉文字应为所在部分的章标题（静态文字，或指向章标题样式的 STYLEREF 域）。
    ChapterTitle,
}

/// 偶数页页眉（开启「奇偶页不同」时）：章标题或固定文字，二选一。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvenHeader {
    pub content: Option<HeaderContent>,
    #[serde(default, deserialize_with = "opt_nonempty")]
    pub text: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Headers {
    #[serde(deserialize_with = "nonempty")]
    pub source: String,
    /// 页眉（奇偶页不同时为奇数页页眉）是章标题；与 `text` 二选一。
    pub content: Option<HeaderContent>,
    /// 页眉（奇偶页不同时为奇数页页眉）是固定文字，如学校名、「××大学硕士学位论文」。
    #[serde(default, deserialize_with = "opt_nonempty")]
    pub text: Option<String>,
    /// 奇偶页页眉不同：偶数页页眉的内容；写了就要求开启「奇偶页不同」。
    pub even: Option<EvenHeader>,
    pub align: Option<Align>,
    #[serde(default, deserialize_with = "opt_nonempty")]
    pub cn_font: Option<String>,
    #[serde(default, deserialize_with = "opt_nonempty")]
    pub latin_font: Option<String>,
    #[serde(default, deserialize_with = "opt_size")]
    pub size: Option<f64>,
    /// 奇偶页页眉相同（即不得开启「奇偶页不同」）。
    pub same_odd_even: Option<bool>,
    /// 前置部分（摘要至目录、符号表）不设页眉，页眉从正文开始。
    #[serde(default)]
    pub front_none: bool,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(deny_unknown_fields)]
pub struct Line {
    pub rule: LineRule,
    pub value: f64,
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Role {
    #[serde(deserialize_with = "nonempty")]
    pub source: String,
    #[serde(default, deserialize_with = "opt_nonempty")]
    pub cn_font: Option<String>,
    #[serde(default, deserialize_with = "opt_nonempty")]
    pub latin_font: Option<String>,
    #[serde(default, deserialize_with = "opt_size")]
    pub size: Option<f64>,
    pub bold: Option<bool>,
    pub align: Option<Align>,
    pub line: Option<Line>,
    pub before: Option<f64>,
    pub after: Option<f64>,
    /// 段前 / 段后以行计（Word「0.5 行」）；与 `before` / `after` 二选一。
    pub before_lines: Option<f64>,
    pub after_lines: Option<f64>,
    pub first_line_chars: Option<f64>,
    /// 悬挂缩进（字符）。角色检查接受任何悬挂缩进（参考文献规范允许 2 字符或 1 厘米），修复写入此值；脚注要求等于此值。
    pub hanging_chars: Option<f64>,
    /// 首行缩进 / 悬挂缩进以厘米计；分别与 `first_line_chars` / `hanging_chars` 二选一。
    pub first_line_cm: Option<f64>,
    pub hanging_cm: Option<f64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Keywords {
    #[serde(deserialize_with = "nonempty")]
    pub source: String,
    /// 关键词个数上下限；规则包未写（规范没有限制）就不检查。
    pub min_count: Option<usize>,
    pub max_count: Option<usize>,
    #[serde(deserialize_with = "nonempty")]
    pub zh_separator: String,
    #[serde(deserialize_with = "nonempty")]
    pub en_separator: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    #[serde(deserialize_with = "nonempty")]
    pub source: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TableLines {
    #[serde(deserialize_with = "nonempty")]
    pub source: String,
    pub top_bottom_pt: f64,
    pub header_pt: f64,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Checks {
    pub keywords: Option<Keywords>,
    pub caption_numbering: Option<Source>,
    pub punctuation: Option<Source>,
    /// 表题在表上方。
    pub table_caption: Option<Source>,
    /// 三线表：线条与线宽。
    pub table_lines: Option<TableLines>,
    /// 表内文字格式（用 Role 的字体、字号、对齐、行距、段前后字段）。
    pub table_cell: Option<Role>,
    /// 图题在图下方。
    pub figure_caption: Option<Source>,
    /// 续表 / 续图题注与原题注一致，续表重复表头。
    pub continued_caption: Option<ContinuedCaption>,
    /// 表达式序号：括号、按章编号、置于行末。
    pub equation_number: Option<Source>,
    /// 独立成行的表达式全文统一：要么居中，要么另起一段空两个汉字符。
    pub equation_layout: Option<Source>,
    /// 中英文摘要的篇幅、插图、标点、关键词对应。
    pub abstract_text: Option<AbstractRules>,
    /// 章序号用阿拉伯数字。
    pub chapter_number: Option<Source>,
    /// 附录依次编号（附录 A、附录 B……，或数字、罗马数字），附录内图表与表达式冠以附录序号。
    pub appendix_number: Option<AppendixNumber>,
    /// 图在正文中被引用，且位于首次引用之后。
    pub figure_citation: Option<Source>,
    /// 顺序编码制：正文引用与参考文献表一一对应、按首次引用先后编号。
    pub citations: Option<Source>,
    /// 脚注内容格式与编号方式。
    pub footnotes: Option<Footnotes>,
}

/// 续表 / 续图题注的写法。
#[derive(Deserialize, Clone, Copy, PartialEq, Default, Debug)]
#[serde(rename_all = "snake_case")]
pub enum ContinuedStyle {
    /// 表序前加「续」：续表 2-1 ×××。
    #[default]
    Prefix,
    /// 题注后加「（续）」：表 2-1 ×××（续）。
    Suffix,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContinuedCaption {
    #[serde(deserialize_with = "nonempty")]
    pub source: String,
    #[serde(default)]
    pub style: ContinuedStyle,
}

/// 附录序号的写法。
#[derive(Deserialize, Clone, Copy, PartialEq, Default, Debug)]
#[serde(rename_all = "snake_case")]
pub enum AppendixStyle {
    /// 附录 A、附录 B……
    #[default]
    Letter,
    /// 附录 1、附录 2……
    Arabic,
    /// 附录 I、附录 II……
    Roman,
    /// 附录一、附录二……
    Han,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppendixNumber {
    #[serde(deserialize_with = "nonempty")]
    pub source: String,
    #[serde(default)]
    pub style: AppendixStyle,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AbstractRules {
    #[serde(deserialize_with = "nonempty")]
    pub source: String,
    /// 中文摘要字数（不含空白）范围。
    pub zh_min_chars: Option<usize>,
    pub zh_max_chars: Option<usize>,
    /// 规范写的是「一般」「约」：字数超出范围只作提示，不算问题。
    #[serde(default)]
    pub chars_approx: bool,
    /// 摘要中不得出现图、表。
    #[serde(default)]
    pub no_figures: bool,
    /// 英文摘要只用英文标点。
    #[serde(default)]
    pub en_punctuation: bool,
    /// 中英文关键词个数一致。
    #[serde(default)]
    pub keywords_match: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Footnotes {
    #[serde(deserialize_with = "nonempty")]
    pub source: String,
    /// 脚注段落的字体、字号、对齐、行距、段前后与悬挂缩进（hanging_chars）。
    pub format: Option<Role>,
    /// 序号用带圈数字 ①②③。
    #[serde(default)]
    pub circled: bool,
    /// 序号每页重新编号。
    #[serde(default)]
    pub restart_each_page: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manual {
    #[serde(deserialize_with = "nonempty")]
    pub text: String,
    #[serde(deserialize_with = "nonempty")]
    pub source: String,
    /// 被哪项精确检查（`layout` 下的键）替代；精确检查跑过后不再列为人工确认。
    #[serde(default, deserialize_with = "opt_nonempty")]
    pub replaced_by: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewPage {
    #[serde(deserialize_with = "nonempty")]
    pub source: String,
    pub roles: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageLimit {
    #[serde(deserialize_with = "nonempty")]
    pub source: String,
    pub role: String,
    pub max_pages: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageStart {
    #[serde(deserialize_with = "nonempty")]
    pub source: String,
    /// 摘要首页应显示的页码。
    pub front_first: Option<i64>,
    /// 正文第一章首页应显示的页码。
    pub body_first: Option<i64>,
}

/// 需要 Word / WPS 排版分页才能判断的检查。
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct LayoutRules {
    /// 跨页表：续表与重复表头。
    pub table_span: Option<Source>,
    /// 跨页图：图与图题同页，分页的图次页用续图。
    pub figure_span: Option<Source>,
    /// 目录页码、条目与标题一致。
    pub toc: Option<Source>,
    /// 指定角色的标题须另起页。
    pub new_page: Option<NewPage>,
    /// 正文第一章须在右页（奇数页）。
    pub first_chapter_odd_page: Option<Source>,
    #[serde(default)]
    pub page_limits: Vec<PageLimit>,
    pub page_start: Option<PageStart>,
    /// 参考文献每条尽量在同一页。
    pub ref_entry_page: Option<Source>,
}

impl LayoutRules {
    pub fn has(&self, key: &str) -> bool {
        match key {
            "table_span" => self.table_span.is_some(),
            "figure_span" => self.figure_span.is_some(),
            "toc" => self.toc.is_some(),
            "new_page" => self.new_page.is_some(),
            "first_chapter_odd_page" => self.first_chapter_odd_page.is_some(),
            "page_limits" => !self.page_limits.is_empty(),
            "page_start" => self.page_start.is_some(),
            "ref_entry_page" => self.ref_entry_page.is_some(),
            _ => false,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pack {
    pub meta: PackMeta,
    pub page: Option<Page>,
    pub page_numbers: Option<PageNumbers>,
    pub headers: Option<Headers>,
    #[serde(default)]
    pub roles: BTreeMap<String, Role>,
    #[serde(default)]
    pub checks: Checks,
    #[serde(default)]
    pub layout: LayoutRules,
    #[serde(default)]
    pub manual: Vec<Manual>,
}

// 由 build.rs 列出 packs/*.yaml：[(id, yaml)]
include!(concat!(env!("OUT_DIR"), "/packs.rs"));

impl Pack {
    pub fn parse(yaml: &str) -> Result<Pack, String> {
        let pack: Pack = serde_yaml_ng::from_str(yaml).map_err(|e| e.to_string())?;
        if let Some(bad) = pack.roles.keys().find(|k| !ROLE_LABELS.iter().any(|(r, _)| r == k)) {
            return Err(format!("roles.{bad}: 未知角色"));
        }
        let lay = &pack.layout;
        let layout_roles = lay.new_page.iter().flat_map(|n| &n.roles).chain(lay.page_limits.iter().map(|l| &l.role));
        if let Some(bad) = layout_roles.into_iter().find(|k| !ROLE_LABELS.iter().any(|(r, _)| r == k)) {
            return Err(format!("layout: 未知角色 {bad}"));
        }
        if let Some(bad) = pack.manual.iter().filter_map(|m| m.replaced_by.as_deref()).find(|k| !lay.has(k)) {
            return Err(format!("manual.replaced_by: layout.{bad} 不存在"));
        }
        let roles = pack.roles.iter().map(|(k, r)| (format!("roles.{k}"), r));
        let extra = [("checks.table_cell", pack.checks.table_cell.as_ref())]
            .into_iter()
            .chain([("checks.footnotes.format", pack.checks.footnotes.as_ref().and_then(|f| f.format.as_ref()))])
            .filter_map(|(k, r)| r.map(|r| (k.to_string(), r)));
        for (key, r) in roles.chain(extra) {
            let pairs = [
                ("before", r.before.is_some(), "before_lines", r.before_lines.is_some()),
                ("after", r.after.is_some(), "after_lines", r.after_lines.is_some()),
                ("first_line_chars", r.first_line_chars.is_some(), "first_line_cm", r.first_line_cm.is_some()),
                ("hanging_chars", r.hanging_chars.is_some(), "hanging_cm", r.hanging_cm.is_some()),
            ];
            if let Some((a, _, b, _)) = pairs.iter().find(|p| p.1 && p.3) {
                return Err(format!("{key}: {a} 与 {b} 只能写一个"));
            }
        }
        if let Some(pn) = &pack.page_numbers {
            if pn.outside && pn.footer_align.is_some() {
                return Err("page_numbers: outside 与 footer_align 只能写一个".into());
            }
        }
        if let Some(h) = &pack.headers {
            if h.content.is_some() && h.text.is_some() {
                return Err("headers: content 与 text 只能写一个".into());
            }
            if let Some(e) = &h.even {
                if e.content.is_some() == e.text.is_some() {
                    return Err("headers.even: content 与 text 须写且只写一个".into());
                }
                if h.same_odd_even == Some(true) {
                    return Err("headers: 写了 even（奇偶页不同）就不能要求 same_odd_even".into());
                }
            }
        }
        Ok(pack)
    }
}

/// 用户导入的规则包所在目录（应用启动时设置一次）。
static USER_DIR: OnceLock<PathBuf> = OnceLock::new();

pub fn set_user_dir(dir: PathBuf) {
    let _ = USER_DIR.set(dir);
}

/// 导入的规则包 id 都以此开头，与内置的区分。
pub const USER_PREFIX: &str = "user-";

pub fn user_pack_path(id: &str) -> Option<PathBuf> {
    let ok = id.starts_with(USER_PREFIX) && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    ok.then(|| USER_DIR.get().map(|d| d.join(format!("{id}.yaml")))).flatten()
}

/// 内置规则包在前，导入的按 id（导入时间）排在后面；导入目录里读不出的文件跳过。
pub fn list_packs() -> Vec<PackMeta> {
    let mut out: Vec<PackMeta> = PACKS.iter().map(|(id, _)| load_pack(id).expect("内置规则包非法").meta).collect();
    let mut user: Vec<PackMeta> = USER_DIR
        .get()
        .and_then(|d| std::fs::read_dir(d).ok())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.path().file_stem()?.to_str().map(String::from))
        .filter_map(|id| load_pack(&id).ok().map(|p| p.meta))
        .collect();
    user.sort_by(|a, b| a.id.cmp(&b.id));
    out.extend(user);
    out
}

pub fn load_pack(pack_id: &str) -> Result<Pack, String> {
    let (mut pack, user) = match PACKS.iter().find(|(id, _)| *id == pack_id) {
        Some((_, yaml)) => (Pack::parse(yaml)?, false),
        None => {
            let path = user_pack_path(pack_id).ok_or_else(|| format!("找不到规则包: {pack_id}"))?;
            let yaml = std::fs::read_to_string(&path).map_err(|_| format!("找不到规则包: {pack_id}"))?;
            (Pack::parse(&yaml)?, true)
        }
    };
    if pack.meta.id != pack_id {
        return Err(format!("规则包文件名与 meta.id 不一致: {pack_id}"));
    }
    pack.meta.user = user;
    Ok(pack)
}
