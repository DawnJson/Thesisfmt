//! 精确检查：依据 Word / WPS 真正排版得到的分页结果，检查跨页表、目录、另起页、页数限制与页码。
//! 只做纯计算：输入是文档模型与采集到的 `Layout`（采集见 `precise.rs`）。

use super::checks::{
    blank, continued_caps, continued_text, parse_cap, set_snippet, table_target, Ctx, Fix, Severity, TableTarget, BY_HAND, SUB_CAP,
    TITLE_ROLES,
};
use super::edit::{self, Confirm, Edit};
use super::model::Model;
use super::pack::{role_label, ContinuedStyle};
use super::roles::norm;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::sync::LazyLock;

/// 第 `i` 个正文段落开头所在页：`p` 物理页（从文档第一页数起），`e` 段末所在页（缺省同 `p`），
/// `d` 该页显示的页码数字，`f` 该页所在节的页码格式（Word 的 WdPageNumberStyle：0 阿拉伯、1 大写罗马、2 小写罗马）。
#[derive(Deserialize)]
pub struct ParaPos {
    pub i: usize,
    pub p: i64,
    pub e: Option<i64>,
    pub d: i64,
    pub f: i64,
}

/// 第 `i` 个正文层表格的起止页（物理页）；`r` 为跨页时每个新页上第一行的行号（0 起，只在 `e > s` 时采集）。
#[derive(Deserialize)]
pub struct TablePos {
    pub i: usize,
    pub s: i64,
    pub e: i64,
    #[serde(default)]
    pub r: Vec<usize>,
}

/// 目录条目指向的标题：所在页与该页的显示页码、标题现文与自动编号。
#[derive(Deserialize)]
pub struct Target {
    pub d: i64,
    pub f: i64,
    pub text: String,
    pub list: String,
}

#[derive(Deserialize)]
pub struct TocEntry {
    pub text: String,
    /// 目录里缓存的页码文字。
    pub cached: Option<String>,
    /// 条目指向的书签；没有 PAGEREF（不带页码的目录）时为 None。
    pub bm: Option<String>,
    /// 书签不存在时为 None。
    pub target: Option<Target>,
}

#[derive(Deserialize)]
pub struct Toc {
    /// TOC 域代码。
    pub code: String,
    pub entries: Vec<TocEntry>,
}

#[derive(Deserialize)]
pub struct Layout {
    /// 排版引擎名称与版本（如 Microsoft Word 16.0）。
    pub name: String,
    pub version: String,
    pub pages: i64,
    pub paras: Vec<ParaPos>,
    pub tables: Vec<TablePos>,
    pub tocs: Vec<Toc>,
    /// 采集耗时（秒），由调用方填写。
    #[serde(default)]
    pub seconds: f64,
}

/// 报告里标明排版依据。
#[derive(Serialize)]
pub struct PreciseInfo {
    engine: String,
    pages: i64,
    seconds: f64,
}

impl Layout {
    /// 解析采集脚本的输出；脚本失败时输出 `{"error": "..."}`。
    pub fn parse(json: &str) -> Result<Layout, String> {
        let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("排版结果不是有效的 JSON：{e}"))?;
        if let Some(err) = v.get("error") {
            return Err(err.as_str().unwrap_or("未知错误").to_string());
        }
        serde_json::from_value(v).map_err(|e| format!("排版结果格式不符：{e}"))
    }

    pub fn info(&self) -> PreciseInfo {
        let version = self.version.trim_end_matches(".0");
        PreciseInfo { engine: format!("{} {version}", self.name), pages: self.pages, seconds: self.seconds }
    }
}

fn roman(mut n: i64) -> String {
    const T: [(i64, &str); 13] = [
        (1000, "M"), (900, "CM"), (500, "D"), (400, "CD"), (100, "C"), (90, "XC"), (50, "L"), (40, "XL"), (10, "X"),
        (9, "IX"), (5, "V"), (4, "IV"), (1, "I"),
    ];
    let mut s = String::new();
    for (v, r) in T {
        while n >= v {
            s.push_str(r);
            n -= v;
        }
    }
    s
}

/// 页码的显示文字；样式不认识（字母等）时返回 None。
fn shown(d: i64, f: i64) -> Option<String> {
    match f {
        0 => Some(d.to_string()),
        1 => Some(roman(d)),
        2 => Some(roman(d).to_lowercase()),
        _ => None,
    }
}

/// 页码文字归一：去空白，Unicode 罗马数字（Ⅰ…Ⅻ / ⅰ…ⅻ）转西文字母，忽略大小写。
fn canon(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| match c {
            '\u{2160}'..='\u{216B}' | '\u{2170}'..='\u{217B}' => roman(i64::from(c as u32 & 0xF) + 1),
            c => c.to_uppercase().to_string(),
        })
        .collect::<String>()
        .to_uppercase()
}

fn head(s: &str, n: usize) -> String {
    s.trim().chars().take(n).collect()
}

fn pages_text(a: i64, b: i64) -> String {
    if a == b { format!("第 {a} 页") } else { format!("第 {a}–{b} 页") }
}

struct Pos<'a> {
    paras: HashMap<usize, &'a ParaPos>,
    tables: HashMap<usize, &'a TablePos>,
}

impl Pos<'_> {
    fn end(&self, para: usize) -> Option<i64> {
        self.paras.get(&para).map(|p| p.e.unwrap_or(p.p))
    }
}

/// 紧邻 `pi` 之前的内容（非空段落或表格）结束所在页；没有前置内容返回 None。
fn prev_end(ctx: &Ctx, pos: &Pos, pi: usize) -> Option<i64> {
    let model = ctx.model;
    let prev = (0..pi).rev().find(|&j| !blank(&model.paras[j]));
    let mut best = prev.and_then(|j| pos.end(j));
    for (ti, t) in model.tables.iter().enumerate() {
        // 表格位于第 near 段之后
        let between = match (t.near, prev) {
            (Some(k), Some(j)) => k >= j && k < pi,
            (Some(k), None) => k < pi,
            (None, _) => prev.is_none(),
        };
        if let Some(tp) = pos.tables.get(&ti).filter(|_| between) {
            best = best.max(Some(tp.e));
        }
    }
    best
}

fn is_title(p: &super::model::Para) -> bool {
    p.role.is_some_and(|r| TITLE_ROLES.contains(&r)) || p.outline == Some(0)
}

/// §2.3.10：每一条文献的内容要尽量写在同一页内。
fn ref_entries(ctx: &mut Ctx, pos: &Pos) {
    let Some(src) = &ctx.pack.layout.ref_entry_page else { return };
    let model = ctx.model;
    for p in model.paras.iter().filter(|p| p.role == Some("ref_item") && !p.ambiguous) {
        let Some(at) = pos.paras.get(&p.idx).filter(|q| q.e.is_some_and(|e| e > q.p)) else { continue };
        let span = pages_text(at.p, at.e.unwrap_or(at.p));
        ctx.add(
            "layout.ref_split", Severity::Warning, "reference", Some(p.idx),
            format!("参考文献条目跨页（{span}）：每条文献应尽量在同一页（可一键修复：设置「段中不分页」，整条移到下一页）"),
            json!({"same_page": true}), json!({"span": span}), true, &src.source,
            Some(Fix::Flag { from: p.idx, to: p.idx, flag: "keepLines" }), Some("ref_item"),
        );
    }
}

pub fn run(ctx: &mut Ctx, lay: &Layout) {
    let pos = Pos {
        paras: lay.paras.iter().map(|p| (p.i, p)).collect(),
        tables: lay.tables.iter().map(|t| (t.i, t)).collect(),
    };
    table_span(ctx, &pos);
    figure_span(ctx, &pos);
    continued(ctx, &pos);
    toc(ctx, lay);
    new_page(ctx, &pos);
    first_chapter_odd(ctx, &pos);
    page_limits(ctx, &pos);
    page_start(ctx, &pos);
    ref_entries(ctx, &pos);
}

fn table_span(ctx: &mut Ctx, pos: &Pos) {
    let Some(src) = &ctx.pack.layout.table_span else { return };
    let (model, doc) = (ctx.model, ctx.doc);
    for (ti, t) in model.tables.iter().enumerate() {
        let Some(tp) = pos.tables.get(&ti).filter(|tp| tp.e > tp.s) else { continue };
        let Some(TableTarget { above, anchor, label, .. }) = table_target(model, doc, t) else { continue };
        let span = pages_text(tp.s, tp.e);
        let num = above.and_then(|c| parse_cap(&model.paras[c])).map_or("X-X".into(), |c| c.label());
        let style = ctx.pack.checks.continued_caption.as_ref().map(|c| c.style).unwrap_or_default();
        let (how, sample) = match style {
            ContinuedStyle::Prefix => ("每页表序前加“续”字", format!("续表 {num}")),
            ContinuedStyle::Suffix => ("每页题注后加“（续）”", format!("表 {num}（续）")),
        };
        ctx.add(
            "layout.table_split", Severity::Warning, "layout", Some(anchor),
            format!("表格跨页（{span}）：规范要求转页接排的表以“续表”形式另页打印，{how}。"),
            json!({"continued_caption": sample}), json!({"span": span}), false, &src.source, None, None,
        );
        set_snippet(ctx, label.clone());
        // 表上方有题注（正式或续表题注）、跨页处的行能拆：提供确认后拆分；一次只拆第一处，拆完重新精确检查再拆下一处
        let offer = above
            .filter(|&c| parse_cap(&model.paras[c]).is_some() && model.paras[c].ppr.num_pr != Some(true))
            .zip(tp.r.first().copied())
            .filter(|&(_, row)| edit::can_split(doc, t.el, row));
        let f = &mut ctx.out.last_mut().unwrap().0;
        match offer {
            Some((cap, row)) => {
                let text = model.paras[cap].text.trim();
                let caption = match style {
                    _ if parse_cap(&model.paras[cap]).is_some_and(|c| c.cont) => text.to_string(),
                    ContinuedStyle::Prefix => format!("续{text}"),
                    ContinuedStyle::Suffix => format!("{text}（续）"),
                };
                let hdr = edit::header_texts(doc, t.el).join(" | ");
                let summary = format!(
                    "在第 {} 行前把表格拆成两段（第 {} 页与第 {} 页交界处）；次页开头插入题注“{caption}”（段前分页，格式同原表题）；\
                     把表头行（“{}”）复制为续表第一行",
                    row + 1,
                    tp.s,
                    tp.s + 1,
                    head(&hdr, 40)
                );
                f.message.push_str("可预览后确认拆分为续表（一次拆第一处跨页，拆完请重新精确检查）");
                f.confirm = Some(Confirm { kind: "split_table", token: summary.clone(), summary, text: None });
                f.edit = Some(Edit::SplitTable { table: ti, row, cap, caption });
            }
            None => f.message.push_str(&format!(
                "{BY_HAND}：光标放在次页第一行，拆分表格（Word：布局 → 拆分表格，或 Ctrl+Shift+Enter），在次页表前写“{sample}”，\
                 再把表头行复制为续表第一行"
            )),
        }
        if !t.header {
            ctx.add(
                "layout.table_header", Severity::Error, "layout", Some(anchor),
                format!("表格跨页（{span}）但表头行没有设置重复：续表应重复表头（可一键修复：把第一行设为「在各页顶端以标题行形式重复出现」）"),
                json!({"repeat_header": true}), json!({"repeat_header": false}), true, &src.source,
                Some(Fix::RepeatHeader { table: ti }), None,
            );
            set_snippet(ctx, label);
        }
    }
}

/// 图题段 `cap` 上方连成一组的图（中间可夹空段、分图题）中第一张图所在段落。
fn figure_group(model: &Model, cap: usize) -> Option<usize> {
    let mut first = None;
    for j in (0..cap).rev() {
        let p = &model.paras[j];
        if p.has_figure {
            first = Some(j);
        } else if !(blank(p) || SUB_CAP.is_match(p.text.trim())) {
            break;
        }
    }
    first
}

/// 一组图连同图题须在同一页；一幅图分在两页时，次页要用“续图”。
fn figure_span(ctx: &mut Ctx, pos: &Pos) {
    let (model, pack) = (ctx.model, ctx.pack);
    let Some(src) = &pack.layout.figure_span else { return };
    for (cap, p) in model.paras.iter().enumerate() {
        if p.role != Some("fig_caption") || p.chapter.is_none() {
            continue;
        }
        let Some(first) = figure_group(model, cap) else { continue };
        let spans = (first..=cap).filter(|&j| !blank(&model.paras[j])).filter_map(|j| pos.paras.get(&j));
        let (lo, hi) = spans.fold((i64::MAX, i64::MIN), |(lo, hi), q| (lo.min(q.p), hi.max(q.e.unwrap_or(q.p))));
        if lo >= hi {
            continue;
        }
        let span = pages_text(lo, hi);
        let num = parse_cap(p).map_or("X-X".into(), |c| c.label());
        let style = pack.checks.continued_caption.as_ref().map(|c| c.style).unwrap_or_default();
        let want = continued_text(style, "图", &num, "图题");
        ctx.add(
            "layout.figure_split", Severity::Warning, "layout", Some(cap),
            format!(
                "图与图题跨页（{span}）：图题应与图在同一页；一幅图需分在两页时，次页应注明“{want}”。\
                 可一键修复：给图片所在段落设置「与下段同页」，让图与图题一起排到同一页（修复后请重新精确检查；\
                 整组图超过一页时仍会跨页，需缩小图片）。图实在放不下、要拆成两部分时，{BY_HAND}：次页那部分图下写“{want}”"
            ),
            json!({"continued_caption": want}), json!({"span": span}), true, &src.source,
            Some(Fix::Flag { from: first, to: cap - 1, flag: "keepNext" }), None,
        );
        set_snippet(ctx, head(&p.text, 40));
    }
}

/// 续表 / 续图应在转页后的次页开头；与上一部分同页说明修改前文后分页移动了。
fn continued(ctx: &mut Ctx, pos: &Pos) {
    let (model, pack) = (ctx.model, ctx.pack);
    for (at, orig) in continued_caps(model) {
        let Some(c) = parse_cap(&model.paras[at]).filter(|_| orig.is_some()) else { continue };
        let (k, num) = (c.kind, c.label());
        let src = if k == "表" { &pack.layout.table_span } else { &pack.layout.figure_span };
        let Some(src) = src else { continue };
        // 续表从表题开始；续图从图题上方那组图的第一张开始
        let Some(start) = (if k == "表" { Some(at) } else { figure_group(model, at) }) else { continue };
        let (Some(s), Some(prev)) = (pos.paras.get(&start).map(|p| p.p), prev_end(ctx, pos, start)) else { continue };
        if prev < s {
            continue;
        }
        ctx.add(
            "layout.continued_page", Severity::Warning, "layout", Some(at),
            format!(
                "续{k} {num} 与上一部分同在第 {s} 页：续{k}应在转页后的次页开头；可能是修改前文后分页移动了。{BY_HAND}：删掉这个续{k}题注，{}；\
                 合并后若仍跨页，再在新的转页处重新拆分",
                if k == "表" { "把两段表格合并并删去续表里重复的表头行" } else { "把两部分图放回同一组" }
            ),
            json!({}), json!({"page": s}), false, &src.source, None, None,
        );
        set_snippet(ctx, head(&model.paras[at].text, 40));
    }
}

/// TOC 域代码里的标题级别范围 (低, 高)，以及是否按段落大纲级别（`\u`）收录。没有按标题收录的开关时返回 None。
fn toc_levels(code: &str) -> Option<(i64, i64, bool)> {
    static O: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"\\o\s+"(\d+)\s*-\s*(\d+)""#).unwrap());
    static U: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\u\b").unwrap());
    let outline = U.is_match(code);
    match O.captures(code) {
        Some(m) => Some((m[1].parse().ok()?, m[2].parse().ok()?, outline)),
        None => outline.then_some((1, 9, true)),
    }
}

fn is_heading_style(p: &super::model::Para) -> bool {
    let n = p.style_name.trim().to_lowercase();
    n.starts_with("heading") || n.starts_with("标题")
}

fn toc(ctx: &mut Ctx, lay: &Layout) {
    let Some(src) = &ctx.pack.layout.toc else { return };
    let model = ctx.model;
    let anchor = ["toc_title", "toc_chapter", "toc_item"]
        .iter()
        .find_map(|r| model.paras.iter().find(|p| p.role == Some(r)))
        .map(|p| p.idx);
    if lay.tocs.is_empty() {
        if let Some(a) = anchor {
            ctx.add(
                "layout.toc_manual", Severity::Warning, "layout", Some(a),
                "目录不是用 Word 目录工具自动生成的（没有 TOC 域），无法核对页码是否准确；规范要求目录宜自动生成".into(),
                json!({"toc": "自动生成"}), json!({"toc": "手工输入"}), false, &src.source, None, None,
            );
        }
        return;
    }
    const FIX: &str = "请在 Word 中右键目录 → 更新域 → 更新整个目录";
    let (mut pages, mut texts) = (vec![], vec![]);
    for e in lay.tocs.iter().flat_map(|t| &t.entries) {
        if e.bm.is_none() {
            continue;
        }
        let Some(t) = &e.target else {
            texts.push((head(&e.text, 20), "（指向的标题已不存在）".to_string()));
            continue;
        };
        let full = norm(&format!("{}{}", t.list, t.text));
        let got = norm(&e.text);
        if got != full && got != norm(&t.text) {
            texts.push((head(&e.text, 20), head(&format!("{} {}", t.list, t.text), 20)));
        }
        if let (Some(c), Some(s)) = (&e.cached, shown(t.d, t.f)) {
            if canon(c) != canon(&s) {
                pages.push((head(&e.text, 20), c.trim().to_string(), s));
            }
        }
    }
    let at = anchor.or_else(|| model.paras.iter().position(|p| p.role.is_some()));
    if let (false, Some((text, cached, real))) = (pages.is_empty(), pages.first()) {
        ctx.add(
            "layout.toc_pages", Severity::Error, "layout", at,
            format!("目录页码已过期：{} 条与实际页码不符（如「{text}」目录为 {cached}，实际为 {real}）。{FIX}", pages.len()),
            json!({}), json!({"count": pages.len(), "first": format!("{text}：目录 {cached}，实际 {real}")}), false,
            &src.source, None, None,
        );
    }
    if let (false, Some((text, now))) = (texts.is_empty(), texts.first()) {
        ctx.add(
            "layout.toc_text", Severity::Error, "layout", at,
            format!("目录条目与标题现文不一致：{} 条（如「{text}」→「{now}」）。{FIX}", texts.len()),
            json!({}), json!({"count": texts.len(), "first": format!("{text} → {now}")}), false, &src.source, None, None,
        );
    }
    // 正文里有标题却不在目录中：按 TOC 域的级别，把标题依序与条目对照
    let mut entries = vec![];
    let (mut lo, mut hi, mut outline) = (i64::MAX, 0, false);
    for t in &lay.tocs {
        if let Some((l, h, o)) = toc_levels(&t.code) {
            (lo, hi, outline) = (lo.min(l), hi.max(h), outline || o);
            entries.extend(t.entries.iter().map(|e| norm(&e.text)));
        }
    }
    if entries.is_empty() {
        return;
    }
    let (mut next, mut missing) = (0, vec![]);
    for p in &model.paras {
        if !p.outline.map(|l| l + 1).is_some_and(|l| (lo..=hi).contains(&l)) {
            continue;
        }
        if p.text.trim().is_empty() || matches!(p.role, Some("toc_title" | "toc_chapter" | "toc_item")) || !(outline || is_heading_style(p)) {
            continue;
        }
        let n = norm(&p.text);
        match entries[next..].iter().position(|e| e.ends_with(&n)) {
            Some(k) => next += k + 1,
            None => missing.push(p.idx),
        }
    }
    if let Some(&first) = missing.first() {
        let text = head(&model.paras[first].text, 20);
        ctx.add(
            "layout.toc_missing", Severity::Error, "layout", Some(first),
            format!("目录缺少 {} 个标题（如「{text}」）。{FIX}", missing.len()),
            json!({}), json!({"count": missing.len()}), false, &src.source, None, None,
        );
    }
}

fn new_page(ctx: &mut Ctx, pos: &Pos) {
    let Some(np) = &ctx.pack.layout.new_page else { return };
    let model = ctx.model;
    for p in &model.paras {
        let Some(role) = p.role.filter(|r| !p.ambiguous && np.roles.iter().any(|x| x == r)) else { continue };
        let (Some(here), Some(before)) = (pos.paras.get(&p.idx), prev_end(ctx, pos, p.idx)) else { continue };
        if here.p == before {
            ctx.add(
                "layout.new_page", Severity::Error, "layout", Some(p.idx),
                format!("{}「{}」应另起一页，当前与上一部分内容同在第 {before} 页（可一键修复：设置「段前分页」）", role_label(role), head(&p.text, 20)),
                json!({"new_page": true}), json!({"page": before}), true, &np.source,
                Some(Fix::Flag { from: p.idx, to: p.idx, flag: "pageBreakBefore" }), Some(role),
            );
        }
    }
}

fn first_chapter(ctx: &Ctx) -> Option<usize> {
    ctx.model.paras.iter().find(|p| p.role == Some("chapter") && !p.ambiguous).map(|p| p.idx)
}

fn first_chapter_odd(ctx: &mut Ctx, pos: &Pos) {
    let Some(src) = &ctx.pack.layout.first_chapter_odd_page else { return };
    let Some(ch) = first_chapter(ctx) else { return };
    let Some(page) = pos.paras.get(&ch).map(|p| p.p).filter(|p| p % 2 == 0) else { return };
    ctx.add(
        "layout.chapter_odd_page", Severity::Warning, "layout", Some(ch),
        format!(
            "正文第一章应从右页（奇数页）开始，当前在第 {page} 页（偶数页）；双面打印时需要在前面补空白页。\
             需手动修改：在前置部分最后一页之后插入一个空白页（Word：插入 → 空白页）。\
             正文页码从 1 重新编排时，Word 的「奇数页分节符」不会自动补空白页，改分节类型无效"
        ),
        json!({"page": "奇数页"}), json!({"page": page}), false, &src.source, None, Some("chapter"),
    );
}

fn page_limits(ctx: &mut Ctx, pos: &Pos) {
    let model = ctx.model;
    for lim in &ctx.pack.layout.page_limits {
        for start in model.paras.iter().filter(|p| p.role == Some(lim.role.as_str()) && !p.ambiguous) {
            let Some(from) = pos.paras.get(&start.idx).map(|p| p.p) else { continue };
            let stop = (start.idx + 1..model.paras.len()).find(|&j| is_title(&model.paras[j])).unwrap_or(model.paras.len());
            let last = (start.idx..stop).rev().find(|&j| !blank(&model.paras[j])).unwrap_or(start.idx);
            let mut to = pos.end(last).unwrap_or(from);
            for (ti, t) in model.tables.iter().enumerate() {
                if t.near.is_some_and(|k| k >= start.idx && k < stop) {
                    to = to.max(pos.tables.get(&ti).map_or(to, |tp| tp.e));
                }
            }
            let count = to - from + 1;
            if count > lim.max_pages {
                let label = role_label(&lim.role);
                ctx.add(
                    "layout.page_limit", Severity::Error, "layout", Some(start.idx),
                    format!("{label}占 {count} 页（{}），超过 {} 页的限制", pages_text(from, to), lim.max_pages),
                    json!({"max_pages": lim.max_pages}), json!({"pages": count}), false, &lim.source, None,
                    start.role,
                );
            }
        }
    }
}

fn page_start(ctx: &mut Ctx, pos: &Pos) {
    let Some(ps) = &ctx.pack.layout.page_start else { return };
    let model = ctx.model;
    let abs = model.paras.iter().find(|p| p.role == Some("abstract_title_zh") && !p.ambiguous).map(|p| p.idx);
    for (what, para, want, role) in [
        ("中文摘要首页", abs, ps.front_first, "abstract_title_zh"),
        ("正文第一章首页", first_chapter(ctx), ps.body_first, "chapter"),
    ] {
        // 分节 / 重新编号的静态检查已指出原因并能修复时，不再重复报告结果
        let explained = ctx.out.iter().any(|(f, _)| {
            f.role == Some(role)
                && matches!(f.rule_id.as_str(), "page_numbers.restart" | "page_numbers.cover_shared" | "page_numbers.mixed_section")
        });
        let (Some(para), Some(want)) = (para.filter(|_| !explained), want) else { continue };
        let Some(at) = pos.paras.get(&para).filter(|p| p.d != want) else { continue };
        let fmt = |n| shown(n, at.f).unwrap_or_else(|| n.to_string());
        ctx.add(
            "layout.page_start", Severity::Error, "layout", Some(para),
            format!("{what}的实际页码为 {}，应从 {} 开始", fmt(at.d), fmt(want)),
            json!({"number": fmt(want)}), json!({"number": fmt(at.d)}), false, &ps.source, None, Some(role),
        );
    }
}
