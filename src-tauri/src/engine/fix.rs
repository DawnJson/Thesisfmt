//! 按检查结果写入直接格式。只处理非 ambiguous 的角色段落、页面设置与页眉页脚。

use super::checks::{jc_norm, note_paras, page_field_para, Fix, Finding, Group, SectFix, Severity, Split, A4, CM, CIRCLED};
use super::model::{get_or_add, Model, PPR_ORDER, RPR_ORDER, SECT_ORDER};
use super::pack::{Align, LineRule, Pack, Role};
use super::xml::{Doc, Id};
use std::collections::{HashMap, HashSet};

fn tw(cm: f64) -> String {
    (cm * CM).round_ties_even().to_string()
}

fn round(v: f64) -> String {
    format!("{}", v.round_ties_even() as i64)
}

/// rPr / pPr 必须是 run / p 的第一个子元素。
pub(super) fn first_child(doc: &mut Doc, parent: Id, tag: &str) -> Id {
    if let Some(el) = doc.wchild(parent, tag) {
        return el;
    }
    let el = doc.create(parent, tag);
    doc.insert_at(parent, 0, el);
    el
}

fn fix_section(doc: &mut Doc, sp: Id, kind: SectFix, pack: &Pack) {
    match kind {
        SectFix::Paper => {
            let pgsz = get_or_add(doc, sp, "pgSz", SECT_ORDER);
            let (mut w, mut h) = A4;
            if doc.wattr(pgsz, "orient") == Some("landscape") {
                std::mem::swap(&mut w, &mut h);
            }
            doc.set_wattr(pgsz, "w", &w.to_string());
            doc.set_wattr(pgsz, "h", &h.to_string());
        }
        SectFix::Margins => {
            let pgmar = get_or_add(doc, sp, "pgMar", SECT_ORDER);
            let m = pack.page.as_ref().and_then(|p| p.margins_cm.as_ref()).expect("规则包缺少页边距");
            for (side, v) in [("top", m.top), ("bottom", m.bottom), ("left", m.left), ("right", m.right)] {
                if let Some(v) = v {
                    doc.set_wattr(pgmar, side, &tw(v));
                }
            }
        }
        SectFix::Distance => {
            let pgmar = get_or_add(doc, sp, "pgMar", SECT_ORDER);
            let page = pack.page.as_ref().expect("规则包缺少页面设置");
            if let Some(v) = page.header_distance_cm {
                doc.set_wattr(pgmar, "header", &tw(v));
            }
            if let Some(v) = page.footer_distance_cm {
                doc.set_wattr(pgmar, "footer", &tw(v));
            }
        }
        SectFix::PgNum(fmt) => {
            let el = get_or_add(doc, sp, "pgNumType", SECT_ORDER);
            doc.set_wattr(el, "fmt", fmt.as_str());
        }
        SectFix::Start(n) => set_start(doc, sp, Some(n)),
        SectFix::Continue => set_start(doc, sp, None),
    }
}

/// `Some(n)`：本节页码从 n 重新开始；`None`：去掉重新编号，接续上一节。
fn set_start(doc: &mut Doc, sp: Id, start: Option<u32>) {
    match start {
        Some(n) => {
            let el = get_or_add(doc, sp, "pgNumType", SECT_ORDER);
            doc.set_wattr(el, "start", &n.to_string());
        }
        None => {
            if let Some(el) = doc.wchild(sp, "pgNumType") {
                doc.remove_wattr(el, "start");
            }
        }
    }
}

fn set_fmt(doc: &mut Doc, sp: Id, fmt: Option<super::pack::PageFmt>) {
    if let Some(f) = fmt {
        let el = get_or_add(doc, sp, "pgNumType", SECT_ORDER);
        doc.set_wattr(el, "fmt", f.as_str());
    }
}

/// 段落 `p` 前面紧挨着的正文块（段落或表格）；前面没有块、是 sdt 或已经是节尾时返回 None。
fn prev_block(doc: &Doc, p: Id) -> Option<Id> {
    let parent = doc.parent(p)?;
    let sibs = doc.children(parent);
    let at = sibs.iter().position(|&c| c == p)?;
    let prev = sibs[..at].iter().rev().copied().find(|&c| doc.is_w(c, "p") || doc.is_w(c, "tbl") || doc.is_w(c, "sdt"))?;
    let ends_section = doc.wchild(prev, "pPr").and_then(|x| doc.wchild(x, "sectPr")).is_some();
    (!doc.is_w(prev, "sdt") && !ends_section).then_some(prev)
}

/// 能否在段落 `p` 之前插入分节符。
pub fn can_split_before(doc: &Doc, p: Id) -> bool {
    prev_block(doc, p).is_some()
}

/// 去掉段末的手动分页符：分节符（下一页）本身就换页，两者叠加会多出一张空白页。
fn drop_trailing_page_breaks(doc: &mut Doc, p: Id) {
    let runs: Vec<Id> = doc.child_els(p).filter(|&r| doc.is_w(r, "r")).collect();
    for r in runs.into_iter().rev() {
        let kids: Vec<Id> = doc.child_els(r).collect();
        let is_page_br = |doc: &Doc, k: Id| doc.is_w(k, "br") && doc.wattr(k, "type") == Some("page");
        let content = kids.iter().any(|&k| !doc.is_w(k, "rPr") && !doc.is_w(k, "lastRenderedPageBreak") && !is_page_br(doc, k));
        let breaks: Vec<Id> = kids.into_iter().filter(|&k| is_page_br(doc, k)).collect();
        breaks.into_iter().for_each(|k| doc.detach(k));
        if content {
            break;
        }
    }
}

/// 在第 `para` 段之前插入分节符（下一页）。原节的 sectPr 留给后一部分，前一部分用它的副本。
fn split_before(doc: &mut Doc, model: &Model, pack: &Pack, para: usize, kind: Split) -> bool {
    let p = model.paras[para].el;
    let Some(orig) = model.sections.iter().find(|s| s.paras.contains(&para)).and_then(|s| s.el) else { return false };
    let Some(prev) = prev_block(doc, p) else { return false };
    let host = if doc.is_w(prev, "p") {
        prev
    } else {
        let np = doc.create(p, "p");
        doc.insert_before(p, np);
        np
    };
    drop_trailing_page_breaks(doc, host);
    let earlier = doc.deep_clone(orig);
    let pn = pack.page_numbers.as_ref();
    let restart = pn.and_then(|pn| pn.restart_at);
    match kind {
        Split::Cover => {
            let drop: Vec<Id> = doc
                .child_els(earlier)
                .filter(|&c| ["headerReference", "footerReference", "pgNumType", "titlePg"].iter().any(|t| doc.is_w(c, t)))
                .collect();
            drop.into_iter().for_each(|c| doc.detach(c));
            if restart.is_some() {
                set_start(doc, orig, restart);
            }
        }
        Split::Body { restart_front, restart_body } => {
            set_fmt(doc, earlier, pn.and_then(|pn| pn.front_format));
            set_start(doc, earlier, restart.filter(|_| restart_front));
            set_fmt(doc, orig, pn.and_then(|pn| pn.body_format));
            set_start(doc, orig, restart.filter(|_| restart_body));
        }
    }
    // 副本保留原节的起始方式；后一部分从新的一页开始（缺省 type 即 nextPage）
    if let Some(t) = doc.wchild(orig, "type") {
        doc.detach(t);
    }
    let ppr = first_child(doc, host, "pPr");
    let slot = get_or_add(doc, ppr, "sectPr", PPR_ORDER);
    doc.insert_before(slot, earlier);
    doc.detach(slot);
    true
}

/// 封面各节（第 `first` 节之前）的页脚引用移给第 `first` 节：后者原来继承的页脚保持不变，封面不再有页脚。
fn move_cover_footers(doc: &mut Doc, model: &Model, first: usize) -> bool {
    let Some(target) = model.sections[first].el else { return false };
    let refs = |doc: &Doc, sp: Id| -> Vec<Id> { doc.child_els(sp).filter(|&c| doc.is_w(c, "footerReference")).collect() };
    let typed = |doc: &Doc, r: Id, t: &str| doc.wattr(r, "type") == Some(t);
    for t in ["default", "first", "even"] {
        if refs(doc, target).iter().any(|&r| typed(doc, r, t)) {
            continue;
        }
        let inherited = model.sections[..first]
            .iter()
            .rev()
            .filter_map(|s| s.el)
            .find_map(|sp| refs(doc, sp).into_iter().find(|&r| typed(doc, r, t)));
        if let Some(r) = inherited {
            let copy = doc.deep_clone(r);
            doc.insert_at(target, 0, copy);
        }
    }
    for sp in model.sections[..first].iter().filter_map(|s| s.el) {
        refs(doc, sp).into_iter().for_each(|r| doc.detach(r));
    }
    true
}

/// 页眉页脚、settings 与脚注：检查时只读；修复时改动过的部件记入 `dirty`（页眉页脚记 rId，另两者记 "settings" / "footnotes"），由调用方写回。
pub struct Parts {
    pub footers: HashMap<String, Doc>,
    pub headers: HashMap<String, Doc>,
    pub settings: Option<Doc>,
    pub footnotes: Option<Doc>,
    pub dirty: HashSet<String>,
}

/// 脚注段落：字体字号、对齐、行距段前后、悬挂缩进。
fn fix_footnote_format(parts: &mut Parts, pack: &Pack) -> bool {
    let Some(spec) = pack.checks.footnotes.as_ref().and_then(|f| f.format.as_ref()) else { return false };
    let Some(fd) = parts.footnotes.as_mut() else { return false };
    for (_, p) in note_paras(fd) {
        for r in super::model::scan_runs(fd, p).into_iter().filter(|r| r.safe && !r.text.trim().is_empty()) {
            format_run(fd, r.el, spec.cn_font.as_deref(), spec.latin_font.as_deref(), spec.size, None);
        }
        set_spacing(fd, p, spec, false);
        if let Some(a) = spec.align {
            let ppr = first_child(fd, p, "pPr");
            let el = get_or_add(fd, ppr, "jc", PPR_ORDER);
            fd.set_wattr(el, "val", jc(a));
        }
        apply_indent(fd, p, spec, spec.size.unwrap_or(9.0));
    }
    parts.dirty.insert("footnotes".into());
    true
}

const FNPR_ORDER: &[&str] = &["pos", "numFmt", "numStart", "numRestart", "footnote"];

fn set_footnote_pr(d: &mut Doc, pr: Id, fmt: Option<&str>, restart: Option<&str>) {
    for (tag, v) in [("numFmt", fmt), ("numRestart", restart)] {
        if let Some(v) = v {
            let el = get_or_add(d, pr, tag, FNPR_ORDER);
            d.set_wattr(el, "val", v);
        }
    }
}

/// 脚注编号：Word 按各节 sectPr 里的 footnotePr 编号，缺省即阿拉伯数字连续编号，settings 里的不生效（Word 16 实测），
/// 所以每一节都写；settings 已有的 footnotePr 一并改成相同值（Word 自己保存时两处都写）。
fn fix_footnote_numbering(doc: &mut Doc, model: &Model, parts: &mut Parts, pack: &Pack) -> bool {
    let Some(fnr) = pack.checks.footnotes.as_ref() else { return false };
    let fmt = fnr.circled.then_some(CIRCLED[0]);
    let restart = fnr.restart_each_page.then_some("eachPage");
    for sp in model.sections.iter().filter_map(|s| s.el) {
        let pr = get_or_add(doc, sp, "footnotePr", SECT_ORDER);
        set_footnote_pr(doc, pr, fmt, restart);
    }
    if let Some(s) = parts.settings.as_mut() {
        if let Some(pr) = s.wchild(s.root(), "footnotePr") {
            set_footnote_pr(s, pr, fmt, restart);
            parts.dirty.insert("settings".into());
        }
    }
    true
}

/// 按规则给一个 run 写直接格式：字体（去掉主题字体）、字号、加粗。
fn format_run(doc: &mut Doc, r: Id, cn: Option<&str>, latin: Option<&str>, size: Option<f64>, bold: Option<bool>) {
    let rpr = first_child(doc, r, "rPr");
    if cn.is_some() || latin.is_some() {
        let rf = get_or_add(doc, rpr, "rFonts", RPR_ORDER);
        if let Some(f) = cn {
            doc.set_wattr(rf, "eastAsia", f);
            doc.remove_wattr(rf, "eastAsiaTheme");
        }
        if let Some(f) = latin {
            for a in ["ascii", "hAnsi"] {
                doc.set_wattr(rf, a, f);
                doc.remove_wattr(rf, &format!("{a}Theme"));
            }
        }
    }
    if let Some(size) = size {
        let v = round(size * 2.0);
        for t in ["sz", "szCs"] {
            let el = get_or_add(doc, rpr, t, RPR_ORDER);
            doc.set_wattr(el, "val", &v);
        }
    }
    if let Some(b) = bold {
        for t in ["b", "bCs"] {
            let el = get_or_add(doc, rpr, t, RPR_ORDER);
            doc.set_wattr(el, "val", if b { "1" } else { "0" });
        }
    }
}

fn fix_font(doc: &mut Doc, model: &Model, pi: usize, spec: &Role) {
    for r in &model.paras[pi].runs {
        if r.safe && !r.text.trim().is_empty() {
            format_run(doc, r.el, spec.cn_font.as_deref(), spec.latin_font.as_deref(), spec.size, spec.bold);
        }
    }
}

fn jc(a: Align) -> &'static str {
    if a == Align::Justify { "both" } else { a.as_str() }
}

/// 页眉 / 页脚段落 `p` 及其中全部 run 的格式。
fn format_part_para(part: &mut Doc, p: Id, align: Option<Align>, cn: Option<&str>, latin: Option<&str>, size: Option<f64>) {
    if let Some(a) = align {
        let ppr = first_child(part, p, "pPr");
        let el = get_or_add(part, ppr, "jc", PPR_ORDER);
        part.set_wattr(el, "val", jc(a));
        // 段落在图文框里时对齐不起作用，要移动图文框本身
        let frame = part.wchild(ppr, "framePr").filter(|&f| part.wattr(f, "xAlign").is_some());
        if let (Some(f), Align::Left | Align::Center | Align::Right) = (frame, a) {
            part.set_wattr(f, "xAlign", a.as_str());
        }
    }
    let runs: Vec<Id> = part.descendants(p).into_iter().filter(|&r| part.is_w(r, "r")).collect();
    for r in runs {
        format_run(part, r, cn, latin, size, None);
    }
}

/// 页眉里有文字或域的段落。
fn header_content_paras(hd: &Doc) -> Vec<Id> {
    hd.descendants(hd.root())
        .into_iter()
        .filter(|&p| hd.is_w(p, "p"))
        .filter(|&p| {
            hd.descendants(p).into_iter().any(|e| {
                hd.is_w(e, "fldChar") || hd.is_w(e, "fldSimple") || (hd.is_w(e, "t") && !hd.text(e).trim().is_empty())
            })
        })
        .collect()
}

fn fix_header(parts: &mut Parts, pack: &Pack, rid: &str) -> bool {
    let (Some(h), Some(hd)) = (pack.headers.as_ref(), parts.headers.get_mut(rid)) else { return false };
    for p in header_content_paras(hd) {
        format_part_para(hd, p, h.align, h.cn_font.as_deref(), h.latin_font.as_deref(), h.size);
    }
    parts.dirty.insert(rid.to_string());
    true
}

/// `align`：页码段应有的对齐（页码在外侧时奇偶页页脚各不相同）。
fn fix_footer(parts: &mut Parts, pack: &Pack, rid: &str, align: Option<Align>) -> bool {
    let (Some(pn), Some(ft)) = (pack.page_numbers.as_ref(), parts.footers.get_mut(rid)) else { return false };
    let Some((p, _)) = page_field_para(ft) else { return false };
    format_part_para(ft, p, align, None, pn.footer_latin_font.as_deref(), pn.footer_size);
    parts.dirty.insert(rid.to_string());
    true
}

fn fix_odd_even(parts: &mut Parts) -> bool {
    let Some(s) = parts.settings.as_mut() else { return false };
    let root = s.root();
    while let Some(e) = s.wchild(root, "evenAndOddHeaders") {
        s.detach(e);
    }
    parts.dirty.insert("settings".into());
    true
}

fn fix_para(doc: &mut Doc, model: &Model, pi: usize, group: Group, spec: &Role) {
    let para = &model.paras[pi];
    match group {
        Group::Font => fix_font(doc, model, pi, spec),
        Group::Align => {
            let ppr = first_child(doc, para.el, "pPr");
            let el = get_or_add(doc, ppr, "jc", PPR_ORDER);
            doc.set_wattr(el, "val", jc(spec.align.unwrap()));
        }
        Group::Spacing => set_spacing(doc, para.el, spec, para.has_math && para.role != Some("equation")),
        Group::Indent => {
            let size = spec.size.unwrap_or_else(|| {
                let r = para.runs.iter().find(|r| !r.text.trim().is_empty()).expect("段落无文字");
                model.run_eff(doc, para, r).sz.unwrap_or(24) as f64 / 2.0
            });
            apply_indent(doc, para.el, spec, size);
        }
    }
}

/// 按规则写缩进：悬挂优先于首行；字符数与厘米各按其单位写。
fn apply_indent(doc: &mut Doc, p: Id, spec: &Role, size: f64) {
    if let Some(h) = spec.hanging_chars {
        set_indent(doc, p, "hanging", h, size);
    } else if let Some(h) = spec.hanging_cm {
        set_indent_cm(doc, p, "hanging", h);
    } else if let Some(c) = spec.first_line_chars {
        set_indent(doc, p, "firstLine", c, size);
    } else if let Some(c) = spec.first_line_cm {
        set_indent_cm(doc, p, "firstLine", c);
    }
}

/// 以厘米写首行或悬挂缩进：只写 twip，清掉字符数（`*Chars` 优先于绝对值）与另一种缩进。
fn set_indent_cm(doc: &mut Doc, p: Id, kind: &str, cm: f64) {
    let ppr = first_child(doc, p, "pPr");
    let ind = get_or_add(doc, ppr, "ind", PPR_ORDER);
    for a in ["firstLine", "firstLineChars", "hanging", "hangingChars"] {
        doc.remove_wattr(ind, a);
    }
    doc.set_wattr(ind, kind, &tw(cm));
}

/// 写首行或悬挂缩进：字符数与 twip 同时写，并清掉另一种缩进（`*Chars` 优先于绝对值）。
fn set_indent(doc: &mut Doc, p: Id, kind: &str, chars: f64, size: f64) {
    let ppr = first_child(doc, p, "pPr");
    let ind = get_or_add(doc, ppr, "ind", PPR_ORDER);
    for a in ["firstLine", "firstLineChars", "hanging", "hangingChars"] {
        doc.remove_wattr(ind, a);
    }
    doc.set_wattr(ind, &format!("{kind}Chars"), &round(chars * 100.0));
    doc.set_wattr(ind, kind, &round(chars * size * 20.0));
}

/// 行距（含公式的段不改行距）与段前段后。
fn set_spacing(doc: &mut Doc, p: Id, spec: &Role, has_math: bool) {
    let ppr = first_child(doc, p, "pPr");
    let sp = get_or_add(doc, ppr, "spacing", PPR_ORDER);
    if let Some(line) = spec.line.filter(|_| !has_math) {
        let v = if line.rule == LineRule::Auto { line.value * 240.0 } else { line.value * 20.0 };
        doc.set_wattr(sp, "line", &round(v));
        doc.set_wattr(sp, "lineRule", line.rule.as_str());
    }
    for (key, v) in [("before", spec.before), ("after", spec.after)] {
        if let Some(v) = v {
            doc.set_wattr(sp, key, &round(v * 20.0));
            doc.remove_wattr(sp, &format!("{key}Lines"));
            doc.remove_wattr(sp, &format!("{key}Autospacing"));
        }
    }
    // 以行计：Word 同时写行数（百分之一行）与按 12 磅一行折算的磅值，行数优先
    for (key, v) in [("before", spec.before_lines), ("after", spec.after_lines)] {
        if let Some(v) = v {
            doc.set_wattr(sp, &format!("{key}Lines"), &round(v * 100.0));
            doc.set_wattr(sp, key, &round(v * 240.0));
            doc.remove_wattr(sp, &format!("{key}Autospacing"));
        }
    }
}

const TBLPR_ORDER: &[&str] = &[
    "tblStyle", "tblpPr", "tblOverlap", "bidiVisual", "tblStyleRowBandSize", "tblStyleColBandSize", "tblW", "jc",
    "tblCellSpacing", "tblInd", "tblBorders", "shd", "tblLayout", "tblCellMar", "tblLook", "tblCaption", "tblDescription",
    "tblPrChange",
];
const TCPR_ORDER: &[&str] = &[
    "cnfStyle", "tcW", "gridSpan", "hMerge", "vMerge", "tcBorders", "shd", "noWrap", "tcMar", "textDirection", "tcFitText",
    "vAlign", "hideMark", "headers", "cellIns", "cellDel", "cellMerge", "tcPrChange",
];
const BORDER_ORDER: &[&str] = &["top", "left", "bottom", "right", "insideH", "insideV", "tl2br", "tr2bl"];

/// 设一条边框：`Some(磅)` 为单直线，`None` 为无。左右边统一写 left / right，去掉同义的 start / end。
fn set_border(doc: &mut Doc, parent: Id, side: &str, pt: Option<f64>) {
    let alias = match side {
        "left" => Some("start"),
        "right" => Some("end"),
        _ => None,
    };
    if let Some(old) = alias.and_then(|a| doc.wchild(parent, a)) {
        doc.detach(old);
    }
    let el = get_or_add(doc, parent, side, BORDER_ORDER);
    match pt {
        Some(p) => {
            doc.set_wattr(el, "val", "single");
            doc.set_wattr(el, "sz", &round(p * 8.0));
            doc.set_wattr(el, "space", "0");
            doc.set_wattr(el, "color", "auto");
        }
        None => {
            doc.set_wattr(el, "val", "nil");
            for a in ["sz", "space", "color", "themeColor", "themeTint", "themeShade"] {
                doc.remove_wattr(el, a);
            }
        }
    }
}

fn rows(doc: &Doc, tbl: Id) -> Vec<Vec<Id>> {
    doc.child_els(tbl)
        .filter(|&r| doc.is_w(r, "tr"))
        .map(|tr| doc.child_els(tr).filter(|&c| doc.is_w(c, "tc")).collect::<Vec<_>>())
        .filter(|r| !r.is_empty())
        .collect()
}

/// 三线表：顶线、底线 `outer` 磅，表头行下 `header` 磅，去掉全部竖线；表内其余横线（辅助线）不动。
fn three_line(doc: &mut Doc, tbl: Id, outer: f64, header: f64) -> bool {
    let rows = rows(doc, tbl);
    let nr = rows.len();
    if nr == 0 {
        return false;
    }
    let tpr = first_child(doc, tbl, "tblPr");
    let tb = get_or_add(doc, tpr, "tblBorders", TBLPR_ORDER);
    for (side, pt) in [("top", Some(outer)), ("left", None), ("bottom", Some(outer)), ("right", None), ("insideV", None)] {
        set_border(doc, tb, side, pt);
    }
    for (r, row) in rows.iter().enumerate() {
        for &tc in row {
            let tcpr = first_child(doc, tc, "tcPr");
            let b = get_or_add(doc, tcpr, "tcBorders", TCPR_ORDER);
            set_border(doc, b, "left", None);
            set_border(doc, b, "right", None);
            if r == 0 {
                set_border(doc, b, "top", Some(outer));
            }
            if r + 1 == nr {
                set_border(doc, b, "bottom", Some(outer));
            } else if r == 0 {
                set_border(doc, b, "bottom", Some(header));
            }
            if r == 1 {
                set_border(doc, b, "top", Some(header));
            }
        }
    }
    true
}

/// 单元格文字：字体字号、行距段前后；对齐既非居中也非两端对齐的改居中；规则要求居中时单元格上下居中。
fn fix_cells(doc: &mut Doc, model: &Model, tbl: Id, tsid: Option<&str>, spec: &Role) -> bool {
    for tc in rows(doc, tbl).into_iter().flatten() {
        let paras: Vec<Id> = doc.child_els(tc).filter(|&e| doc.is_w(e, "p")).collect();
        for p in paras {
            for r in super::model::scan_runs(doc, p).into_iter().filter(|r| r.safe && !r.text.trim().is_empty()) {
                format_run(doc, r.el, spec.cn_font.as_deref(), spec.latin_font.as_deref(), spec.size, None);
            }
            set_spacing(doc, p, spec, false);
            let jc_now = model.cell_para(doc, p, tsid).0.jc;
            if let Some(a) = spec.align.filter(|_| !matches!(jc_norm(jc_now.as_deref()), "center" | "justify")) {
                let ppr = first_child(doc, p, "pPr");
                let el = get_or_add(doc, ppr, "jc", PPR_ORDER);
                doc.set_wattr(el, "val", jc(a));
            }
        }
        if spec.align == Some(Align::Center) {
            let tcpr = first_child(doc, tc, "tcPr");
            let v = get_or_add(doc, tcpr, "vAlign", TCPR_ORDER);
            doc.set_wattr(v, "val", "center");
        }
    }
    true
}

/// trPr 子元素顺序（ECMA-376 CT_TrPr）。
const TRPR_ORDER: &[&str] = &[
    "cnfStyle", "divId", "gridBefore", "gridAfter", "wBefore", "wAfter", "cantSplit", "trHeight", "tblHeader",
    "tblCellSpacing", "jc", "hidden", "ins", "del", "trPrChange",
];

/// 表格第一行设为重复标题行；trPr 位于 tblPrEx 之后、单元格之前。
fn repeat_header(doc: &mut Doc, tbl: Id) -> bool {
    let Some(tr) = doc.child_els(tbl).find(|&e| doc.is_w(e, "tr")) else { return false };
    let trpr = match doc.wchild(tr, "trPr") {
        Some(x) => x,
        None => {
            let x = doc.create(tr, "trPr");
            let at = usize::from(doc.child_els(tr).next().is_some_and(|c| doc.is_w(c, "tblPrEx")));
            doc.insert_at(tr, at, x);
            x
        }
    };
    let el = get_or_add(doc, trpr, "tblHeader", TRPR_ORDER);
    doc.remove_wattr(el, "val");
    true
}

/// 返回 (fixed, skipped)。
pub fn apply_fixes(doc: &mut Doc, model: &Model, pack: &Pack, items: &[(Finding, Option<Fix>)], parts: &mut Parts) -> (usize, usize) {
    // 分节会复制原节的 sectPr，所以先做节内的修复；封面分节要在正文分节之前，两者可能落在同一节
    let rank = |fix: &Option<Fix>| match fix {
        Some(Fix::CoverFooter { .. }) => 1,
        Some(Fix::Split { kind: Split::Cover, .. }) => 2,
        Some(Fix::Split { kind: Split::Body { .. }, .. }) => 3,
        _ => 0,
    };
    let mut ordered: Vec<&(Finding, Option<Fix>)> = items.iter().collect();
    ordered.sort_by_key(|(_, fix)| rank(fix));
    let (mut fixed, mut skipped) = (0, 0);
    for (f, fix) in ordered {
        if f.severity == Severity::Manual {
            continue;
        }
        let Some(fix) = fix.clone().filter(|_| f.fixable) else {
            let id = f.rule_id.as_str();
            let counted = ["role.", "page.paper", "page.margins", "page.distance", "page_numbers.format"];
            if counted.iter().any(|p| id.starts_with(p)) {
                skipped += 1;
            }
            continue;
        };
        let done = match fix {
            Fix::Sect { sec, kind } => match model.sections[sec].el {
                Some(sp) => {
                    fix_section(doc, sp, kind, pack);
                    true
                }
                None => false,
            },
            Fix::Para { para, group, role } => {
                fix_para(doc, model, para, group, &pack.roles[role]);
                true
            }
            Fix::Split { para, kind } => split_before(doc, model, pack, para, kind),
            Fix::CoverFooter { first } => move_cover_footers(doc, model, first),
            Fix::Flag { from, to, flag } => {
                for p in &model.paras[from..=to] {
                    let ppr = first_child(doc, p.el, "pPr");
                    let el = get_or_add(doc, ppr, flag, PPR_ORDER);
                    doc.remove_wattr(el, "val");
                }
                true
            }
            Fix::RepeatHeader { table } => repeat_header(doc, model.tables[table].el),
            Fix::OddEven => fix_odd_even(parts),
            Fix::HeaderFormat { rid } => fix_header(parts, pack, &rid),
            Fix::FooterFormat { rid, align } => fix_footer(parts, pack, &rid, align),
            Fix::FootnoteFormat => fix_footnote_format(parts, pack),
            Fix::FootnoteNumbering => fix_footnote_numbering(doc, model, parts, pack),
            Fix::TableLines { table } => match &pack.checks.table_lines {
                Some(tl) => three_line(doc, model.tables[table].el, tl.top_bottom_pt, tl.header_pt),
                None => false,
            },
            Fix::TableCells { table } => match &pack.checks.table_cell {
                Some(spec) => fix_cells(doc, model, model.tables[table].el, model.tables[table].style.as_deref(), spec),
                None => false,
            },
        };
        if done {
            fixed += 1;
        } else {
            skipped += 1;
        }
    }
    (fixed, skipped)
}
