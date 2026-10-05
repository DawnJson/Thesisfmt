//! 文档模型：直接解析样式继承、有效格式、段落/节结构。

use super::xml::{Doc, Id, Ns};
use regex::Regex;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

const UNSAFE_CHILD: [&str; 8] = [
    "fldChar", "instrText", "drawing", "pict", "object", "footnoteReference", "endnoteReference", "commentReference",
];
const TRACKED: [&str; 4] = ["ins", "del", "moveFrom", "moveTo"];

pub const PPR_ORDER: &[&str] = &[
    "pStyle", "keepNext", "keepLines", "pageBreakBefore", "framePr", "widowControl", "numPr", "suppressLineNumbers",
    "pBdr", "shd", "tabs", "suppressAutoHyphens", "kinsoku", "wordWrap", "overflowPunct", "topLinePunct",
    "autoSpaceDE", "autoSpaceDN", "bidi", "adjustRightInd", "snapToGrid", "spacing", "ind", "contextualSpacing",
    "mirrorIndents", "suppressOverlap", "jc", "textDirection", "textAlignment", "textboxTightWrap", "outlineLvl",
    "divId", "cnfStyle", "rPr", "sectPr", "pPrChange",
];
pub const RPR_ORDER: &[&str] = &[
    "rStyle", "rFonts", "b", "bCs", "i", "iCs", "caps", "smallCaps", "strike", "dstrike", "outline", "shadow",
    "emboss", "imprint", "noProof", "snapToGrid", "vanish", "webHidden", "color", "spacing", "w", "kern", "position",
    "sz", "szCs", "highlight", "u", "effect", "bdr", "shd", "fitText", "vertAlign", "rtl", "cs", "em", "lang",
    "eastAsianLayout", "specVanish", "oMath",
];
pub const SECT_ORDER: &[&str] = &[
    "headerReference", "footerReference", "footnotePr", "endnotePr", "type", "pgSz", "pgMar", "paperSrc", "pgBorders",
    "lnNumType", "pgNumType", "cols", "formProt", "vAlign", "noEndnote", "titlePg", "textDirection", "bidi",
    "rtlGutter", "docGrid", "printerSettings", "sectPrChange",
];

/// 按 schema 顺序取得或插入子元素。
pub fn get_or_add(doc: &mut Doc, parent: Id, tag: &str, order: &[&str]) -> Id {
    if let Some(el) = doc.wchild(parent, tag) {
        return el;
    }
    let el = doc.create(parent, tag);
    let later = &order[order.iter().position(|t| *t == tag).unwrap() + 1..];
    let before = doc.children(parent).iter().copied().find(|&c| later.iter().any(|t| doc.is_w(c, t)));
    match before {
        Some(b) => doc.insert_before(b, el),
        None => doc.append(parent, el),
    }
    el
}

fn int(v: Option<&str>) -> Option<i64> {
    let v = v?;
    v.parse().ok().or_else(|| v.parse::<f64>().ok().filter(|f| f.is_finite()).map(|f| f as i64))
}

pub fn toggle(doc: &Doc, el: Id) -> bool {
    !matches!(doc.wattr(el, "val").unwrap_or("true").to_lowercase().as_str(), "0" | "false" | "off")
}

#[derive(Clone, Default)]
pub struct PProps {
    pub jc: Option<String>,
    pub line: Option<i64>,
    pub before: Option<i64>,
    pub after: Option<i64>,
    pub before_lines: Option<i64>,
    pub after_lines: Option<i64>,
    pub line_rule: Option<String>,
    pub first_line: Option<i64>,
    pub first_line_chars: Option<i64>,
    pub hanging: Option<i64>,
    pub hanging_chars: Option<i64>,
    pub outline_lvl: Option<i64>,
    pub num_pr: Option<bool>,
}

#[derive(Clone)]
pub enum FontRef {
    Theme(String),
    Font(String),
}

/// 槽位顺序：ascii / hAnsi / eastAsia / cs。
pub const ASCII: usize = 0;
pub const HANSI: usize = 1;
pub const EAST_ASIA: usize = 2;

#[derive(Clone, Default)]
pub struct RProps {
    pub fonts: [Option<FontRef>; 4],
    pub sz: Option<i64>,
    pub b: Option<bool>,
}

fn set<T>(slot: &mut Option<T>, v: Option<T>) {
    if v.is_some() {
        *slot = v;
    }
}

pub fn read_ppr(doc: &Doc, ppr: Option<Id>, acc: &mut PProps) {
    let Some(ppr) = ppr else { return };
    for ch in doc.child_els(ppr) {
        if doc.is_w(ch, "jc") {
            set(&mut acc.jc, doc.wattr(ch, "val").map(String::from));
        } else if doc.is_w(ch, "spacing") {
            set(&mut acc.line, int(doc.wattr(ch, "line")));
            set(&mut acc.before, int(doc.wattr(ch, "before")));
            set(&mut acc.after, int(doc.wattr(ch, "after")));
            set(&mut acc.before_lines, int(doc.wattr(ch, "beforeLines")));
            set(&mut acc.after_lines, int(doc.wattr(ch, "afterLines")));
            set(&mut acc.line_rule, doc.wattr(ch, "lineRule").filter(|v| !v.is_empty()).map(String::from));
        } else if doc.is_w(ch, "ind") {
            set(&mut acc.first_line, int(doc.wattr(ch, "firstLine")));
            set(&mut acc.first_line_chars, int(doc.wattr(ch, "firstLineChars")));
            set(&mut acc.hanging, int(doc.wattr(ch, "hanging")));
            set(&mut acc.hanging_chars, int(doc.wattr(ch, "hangingChars")));
        } else if doc.is_w(ch, "outlineLvl") {
            set(&mut acc.outline_lvl, int(doc.wattr(ch, "val")));
        } else if doc.is_w(ch, "numPr") {
            let off = doc.wchild(ch, "numId").is_some_and(|n| doc.wattr(n, "val") == Some("0"));
            acc.num_pr = Some(!off);
        }
    }
}

pub fn read_rpr(doc: &Doc, rpr: Option<Id>, acc: &mut RProps) {
    let Some(rpr) = rpr else { return };
    const SLOTS: [(&str, &str); 4] =
        [("ascii", "asciiTheme"), ("hAnsi", "hAnsiTheme"), ("eastAsia", "eastAsiaTheme"), ("cs", "cstheme")];
    for ch in doc.child_els(rpr) {
        if doc.is_w(ch, "rFonts") {
            for (i, (slot, theme)) in SLOTS.iter().enumerate() {
                if let Some(t) = doc.wattr(ch, theme).filter(|v| !v.is_empty()) {
                    acc.fonts[i] = Some(FontRef::Theme(t.into()));
                } else if let Some(f) = doc.wattr(ch, slot) {
                    acc.fonts[i] = Some(FontRef::Font(f.into()));
                }
            }
        } else if doc.is_w(ch, "sz") {
            set(&mut acc.sz, int(doc.wattr(ch, "val")));
        } else if doc.is_w(ch, "b") {
            acc.b = Some(toggle(doc, ch));
        }
    }
}

pub struct Styles {
    doc: Option<Doc>,
    el: HashMap<String, Id>,
    name: HashMap<String, String>,
    pub default_p: Option<String>,
    dd_p: PProps,
    dd_r: RProps,
    theme: HashMap<String, String>,
    p_cache: RefCell<HashMap<(Option<String>, Option<String>), PProps>>,
    r_cache: RefCell<HashMap<(Option<String>, Option<String>, Option<String>), RProps>>,
}

impl Styles {
    fn new(styles: Option<Doc>, theme: Option<&Doc>) -> Styles {
        let mut s = Styles {
            doc: None,
            el: HashMap::new(),
            name: HashMap::new(),
            default_p: None,
            dd_p: PProps::default(),
            dd_r: RProps::default(),
            theme: HashMap::new(),
            p_cache: RefCell::default(),
            r_cache: RefCell::default(),
        };
        if let Some(d) = &styles {
            let root = d.root();
            for st in d.child_els(root).filter(|&c| d.is_w(c, "style")) {
                let Some(sid) = d.wattr(st, "styleId") else { continue };
                s.el.insert(sid.into(), st);
                let name = d.wchild(st, "name").and_then(|n| d.wattr(n, "val")).unwrap_or(sid);
                s.name.insert(sid.into(), name.into());
                if d.wattr(st, "type") == Some("paragraph") && matches!(d.wattr(st, "default"), Some("1" | "true")) {
                    s.default_p = Some(sid.into());
                }
            }
            if let Some(dd) = d.wchild(root, "docDefaults") {
                let rpr = d.wchild(dd, "rPrDefault").and_then(|x| d.wchild(x, "rPr"));
                read_rpr(d, rpr, &mut s.dd_r);
                let ppr = d.wchild(dd, "pPrDefault").and_then(|x| d.wchild(x, "pPr"));
                read_ppr(d, ppr, &mut s.dd_p);
            }
        }
        s.doc = styles;
        if let Some(t) = theme {
            for kind in ["major", "minor"] {
                let font = t.descendants(t.root()).into_iter().find(|&e| t.is(e, &Ns::A, &format!("{kind}Font")));
                let Some(f) = font else { continue };
                let tf = |tag: &str| {
                    t.child(f, &Ns::A, tag)
                        .and_then(|e| t.plain_attr(e, "typeface"))
                        .filter(|v| !v.is_empty())
                        .map(String::from)
                };
                // 主题的 <a:ea> 常为空，简体中文字体写在 <a:font script="Hans" typeface="等线"/>
                let hans = || {
                    t.child_els(f)
                        .find(|&e| t.is(e, &Ns::A, "font") && t.plain_attr(e, "script") == Some("Hans"))
                        .and_then(|e| t.plain_attr(e, "typeface"))
                        .filter(|v| !v.is_empty())
                        .map(String::from)
                };
                for (key, v) in [("Ascii", tf("latin")), ("HAnsi", tf("latin")), ("EastAsia", tf("ea").or_else(hans))] {
                    if let Some(v) = v {
                        s.theme.insert(format!("{kind}{key}"), v);
                    }
                }
            }
        }
        s
    }

    fn chain(&self, sid: Option<&str>) -> Vec<Id> {
        let (Some(d), Some(mut cur)) = (&self.doc, sid.map(String::from)) else { return vec![] };
        let (mut out, mut seen) = (vec![], HashSet::new());
        while let Some(&el) = self.el.get(&cur) {
            if !seen.insert(cur.clone()) {
                break;
            }
            out.push(el);
            match d.wchild(el, "basedOn").and_then(|b| d.wattr(b, "val")) {
                Some(b) if !b.is_empty() => cur = b.into(),
                _ => break,
            }
        }
        out.reverse();
        out
    }

    fn p_props(&self, sid: Option<&str>) -> PProps {
        self.p_props_in(None, sid)
    }

    /// 表格样式 `tsid` 在前、段落样式在后叠加。
    fn p_props_in(&self, tsid: Option<&str>, sid: Option<&str>) -> PProps {
        let key = (tsid.map(String::from), sid.map(String::from));
        if let Some(r) = self.p_cache.borrow().get(&key) {
            return r.clone();
        }
        let mut r = self.dd_p.clone();
        if let Some(d) = &self.doc {
            for s in self.chain(tsid).into_iter().chain(self.chain(sid)) {
                read_ppr(d, d.wchild(s, "pPr"), &mut r);
            }
        }
        self.p_cache.borrow_mut().insert(key, r.clone());
        r
    }

    /// 段落样式 + 字符样式 + docDefaults 得到的 run 基础属性。
    pub fn r_base(&self, psid: Option<&str>, rsid: Option<&str>) -> RProps {
        self.r_base_in(None, psid, rsid)
    }

    fn r_base_in(&self, tsid: Option<&str>, psid: Option<&str>, rsid: Option<&str>) -> RProps {
        let key = (tsid.map(String::from), psid.map(String::from), rsid.map(String::from));
        if let Some(r) = self.r_cache.borrow().get(&key) {
            return r.clone();
        }
        let mut r = self.dd_r.clone();
        if let Some(d) = &self.doc {
            for s in self.chain(tsid).into_iter().chain(self.chain(psid)).chain(self.chain(rsid)) {
                read_rpr(d, d.wchild(s, "rPr"), &mut r);
            }
        }
        self.r_cache.borrow_mut().insert(key, r.clone());
        r
    }

    /// 某槽位的字体名；主题字体查主题表，未确定返回 None。
    pub fn font(&self, eff: &RProps, slot: usize) -> Option<String> {
        match eff.fonts[slot].as_ref()? {
            FontRef::Font(f) => Some(f.clone()).filter(|f| !f.is_empty()),
            FontRef::Theme(t) => self.theme.get(t).cloned(),
        }
    }

    /// 表格样式（含 basedOn 链）给单元格的垂直对齐；未设置返回 None（Word 默认顶端）。
    pub fn table_valign(&self, tsid: Option<&str>) -> Option<String> {
        let d = self.doc.as_ref()?;
        self.chain(tsid)
            .into_iter()
            .rev()
            .find_map(|st| d.wchild(st, "tcPr").and_then(|x| d.wchild(x, "vAlign")).and_then(|v| d.wattr(v, "val")))
            .map(String::from)
    }

    /// 表格有效边框：表格样式（含首行/末行条件格式）< 表 tblBorders < 单元格 tcBorders，相邻单元格取优先级高者。
    pub fn table_lines(&self, doc: &Doc, tbl: Id) -> Lines {
        let tpr = doc.wchild(tbl, "tblPr");
        let tsid = tpr.and_then(|x| doc.wchild(x, "tblStyle")).and_then(|s| doc.wattr(s, "val"));
        let (mut whole, mut first, mut last) = ([Edge::default(); 6], [Edge::default(); 6], [Edge::default(); 6]);
        if let Some(sd) = &self.doc {
            for st in self.chain(tsid) {
                apply(&mut whole, read_borders(sd, sd.wchild(st, "tblPr").and_then(|x| sd.wchild(x, "tblBorders")), 1));
                for sp in sd.child_els(st).filter(|&c| sd.is_w(c, "tblStylePr")) {
                    let target = match sd.wattr(sp, "type") {
                        Some("firstRow") => &mut first,
                        Some("lastRow") => &mut last,
                        _ => continue,
                    };
                    apply(target, read_borders(sd, sd.wchild(sp, "tcPr").and_then(|x| sd.wchild(x, "tcBorders")), 2));
                }
            }
        }
        apply(&mut whole, read_borders(doc, tpr.and_then(|x| doc.wchild(x, "tblBorders")), 3));
        let look = tpr.and_then(|x| doc.wchild(x, "tblLook"));
        let flag = |name: &str, bit: u32| {
            look.is_some_and(|l| {
                doc.wattr(l, name).is_some_and(|v| matches!(v, "1" | "true" | "on"))
                    || doc.wattr(l, "val").and_then(|v| u32::from_str_radix(v, 16).ok()).is_some_and(|v| v & bit != 0)
            })
        };
        let (use_first, use_last) = (flag("firstRow", 0x20), flag("lastRow", 0x40));
        let rows: Vec<Vec<Id>> = doc
            .child_els(tbl)
            .filter(|&r| doc.is_w(r, "tr"))
            .map(|tr| doc.child_els(tr).filter(|&c| doc.is_w(c, "tc")).collect::<Vec<_>>())
            .filter(|r| !r.is_empty())
            .collect();
        let nr = rows.len();
        // 每格 [上, 左, 下, 右]
        let cells: Vec<Vec<[Edge; 4]>> = rows
            .iter()
            .enumerate()
            .map(|(r, row)| {
                let nc = row.len();
                row.iter()
                    .enumerate()
                    .map(|(c, &tc)| {
                        let mut e = [
                            whole[if r == 0 { 0 } else { 4 }],
                            whole[if c == 0 { 1 } else { 5 }],
                            whole[if r + 1 == nr { 2 } else { 4 }],
                            whole[if c + 1 == nc { 3 } else { 5 }],
                        ];
                        for cond in [(r == 0 && use_first).then_some(&first), (r + 1 == nr && use_last).then_some(&last)]
                            .into_iter()
                            .flatten()
                        {
                            e.iter_mut().zip(cond).for_each(|(x, &n)| x.merge(Some(n)));
                        }
                        let own = read_borders(doc, doc.wchild(tc, "tcPr").and_then(|x| doc.wchild(x, "tcBorders")), 4);
                        e.iter_mut().zip(own).for_each(|(x, n)| x.merge(n));
                        e
                    })
                    .collect()
            })
            .collect();
        let horizontal = (0..=nr)
            .map(|k| {
                if k == 0 {
                    cells[0].iter().map(|c| c[0]).collect()
                } else if k == nr {
                    cells[nr - 1].iter().map(|c| c[2]).collect()
                } else {
                    (0..cells[k - 1].len().max(cells[k].len()))
                        .filter_map(|c| match (cells[k - 1].get(c), cells[k].get(c)) {
                            (Some(a), Some(b)) => Some(Edge::between(a[2], b[0])),
                            (Some(a), None) => Some(a[2]),
                            (None, Some(b)) => Some(b[0]),
                            (None, None) => None,
                        })
                        .collect()
                }
            })
            .collect();
        let vertical = cells
            .iter()
            .map(|row| {
                (0..=row.len())
                    .map(|k| match k {
                        0 => row[0][1],
                        k if k == row.len() => row[k - 1][3],
                        k => Edge::between(row[k - 1][3], row[k][1]),
                    })
                    .collect()
            })
            .collect();
        Lines { horizontal, vertical }
    }
}

/// 边框的一条边。`level` 越大优先级越高（0 未指定，1 表样式，2 条件格式，3 表直接格式，4 单元格直接格式）。
#[derive(Clone, Copy, Default)]
pub struct Edge {
    level: u8,
    pub visible: bool,
    pub single: bool,
    /// 八分之一磅。
    pub sz: Option<i64>,
}

impl Edge {
    fn merge(&mut self, new: Option<Edge>) {
        if let Some(n) = new.filter(|n| n.level >= self.level) {
            *self = n;
        }
    }

    fn between(a: Edge, b: Edge) -> Edge {
        if a.level != b.level {
            return if a.level > b.level { a } else { b };
        }
        match (a.visible, b.visible) {
            (true, true) => if a.sz >= b.sz { a } else { b },
            (true, false) => a,
            _ => b,
        }
    }
}

fn apply(acc: &mut [Edge; 6], new: [Option<Edge>; 6]) {
    acc.iter_mut().zip(new).for_each(|(x, n)| x.merge(n));
}

/// 依次为 上 / 左 / 下 / 右 / insideH / insideV。
fn read_borders(doc: &Doc, el: Option<Id>, level: u8) -> [Option<Edge>; 6] {
    const SIDES: [&[&str]; 6] = [&["top"], &["left", "start"], &["bottom"], &["right", "end"], &["insideH"], &["insideV"]];
    let mut out = [None; 6];
    for ch in el.into_iter().flat_map(|e| doc.child_els(e)) {
        if let Some(i) = SIDES.iter().position(|s| s.iter().any(|n| doc.is_w(ch, n))) {
            let val = doc.wattr(ch, "val").unwrap_or("none");
            out[i] = Some(Edge {
                level,
                visible: !matches!(val, "none" | "nil" | ""),
                single: val == "single",
                sz: int(doc.wattr(ch, "sz")),
            });
        }
    }
    out
}

/// `horizontal[k]`：第 k 行上方的横线（k = 行数为表底线）；`vertical[r][k]`：第 r 行第 k 个单元格左侧的竖线（k = 列数为右边线）。
pub struct Lines {
    pub horizontal: Vec<Vec<Edge>>,
    pub vertical: Vec<Vec<Edge>>,
}

pub struct Run {
    pub el: Id,
    pub text: String,
    pub safe: bool,
}

pub struct Para {
    pub idx: usize,
    pub el: Id,
    pub text: String,
    pub sid: Option<String>,
    pub style_name: String,
    pub ppr: PProps,
    pub runs: Vec<Run>,
    pub has_math: bool,
    pub has_figure: bool,
    pub role: Option<&'static str>,
    pub ambiguous: bool,
    pub chapter: Option<String>,
    pub outline: Option<i64>,
    pub has_tab: bool,
    /// 含模板里的格式说明（括号里的「三号黑体」「1.5 倍行距」等），由角色识别设置。
    pub note: bool,
}

/// 正文层表格。`before` / `after` 仅在紧邻的前 / 后一个块是段落时有值；`near` 为文档顺序上最近的前一段。
pub struct Table {
    pub el: Id,
    pub style: Option<String>,
    pub before: Option<usize>,
    pub after: Option<usize>,
    pub near: Option<usize>,
    /// 首行设为「在各页顶端以标题行形式重复出现」（`trPr/tblHeader`）。
    pub header: bool,
}

pub struct Section {
    pub index: usize,
    pub el: Option<Id>,
    pub paras: Vec<usize>,
}

pub(super) fn scan_runs(doc: &Doc, p: Id) -> Vec<Run> {
    let (mut runs, mut depth) = (vec![], 0usize);
    for r in doc.descendants(p).into_iter().filter(|&r| doc.is_w(r, "r")) {
        let mut in_field = depth > 0;
        if let Some(fc) = doc.wchild(r, "fldChar") {
            match doc.wattr(fc, "fldCharType") {
                Some("begin") => depth += 1,
                Some("end") => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        let mut tracked = false;
        let mut cur = doc.parent(r);
        while let Some(a) = cur.filter(|&a| a != p) {
            if TRACKED.iter().any(|t| doc.is_w(a, t)) {
                tracked = true;
            } else if doc.is_w(a, "fldSimple") {
                in_field = true;
            }
            cur = doc.parent(a);
        }
        let (mut text, mut unsafe_) = (String::new(), false);
        for ch in doc.child_els(r) {
            if doc.is_w(ch, "t") {
                text.push_str(&doc.text(ch));
            } else if doc.is_w(ch, "tab") {
                text.push('\t');
            } else if doc.is_w(ch, "noBreakHyphen") {
                // 题注“包含章节号”、分隔符选“不间断连字符”时 Word 写的就是它（表3‑1）
                text.push('-');
            } else if UNSAFE_CHILD.iter().any(|t| doc.is_w(ch, t)) || doc.is(ch, &Ns::Mc, "AlternateContent") {
                unsafe_ = true;
            }
        }
        runs.push(Run { el: r, text, safe: !(unsafe_ || in_field || tracked) });
    }
    runs
}

/// 依序收集正文层的段落与表格（穿透 sdt）。
fn blocks(doc: &Doc, parent: Id, out: &mut Vec<Id>) {
    for ch in doc.child_els(parent) {
        if doc.is_w(ch, "p") || doc.is_w(ch, "tbl") {
            out.push(ch);
        } else if doc.is_w(ch, "sdt") {
            if let Some(c) = doc.wchild(ch, "sdtContent") {
                blocks(doc, c, out);
            }
        }
    }
}

static HEADING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(?:heading|标题)\s*(\d)$").unwrap());

pub struct Model {
    pub styles: Styles,
    pub paras: Vec<Para>,
    pub tables: Vec<Table>,
    pub sections: Vec<Section>,
}

impl Model {
    pub fn new(doc: &Doc, styles: Option<Doc>, theme: Option<&Doc>) -> Result<Model, String> {
        let styles = Styles::new(styles, theme);
        let root = doc.root();
        let body = doc.wchild(root, "body").ok_or("document.xml 缺少 w:body")?;
        let (mut paras, mut sections, mut tables) = (vec![], vec![], vec![]);
        let mut cur = Section { index: 0, el: None, paras: vec![] };
        let mut ids = vec![];
        blocks(doc, body, &mut ids);
        let (mut prev_para, mut last_tbl): (Option<usize>, Option<usize>) = (None, None);
        for p in ids {
            if doc.is_w(p, "tbl") {
                let style = doc
                    .wchild(p, "tblPr")
                    .and_then(|x| doc.wchild(x, "tblStyle"))
                    .and_then(|s| doc.wattr(s, "val"))
                    .map(String::from);
                let header = doc
                    .child_els(p)
                    .find(|&r| doc.is_w(r, "tr"))
                    .and_then(|tr| doc.wchild(tr, "trPr"))
                    .and_then(|x| doc.wchild(x, "tblHeader"))
                    .is_some_and(|h| toggle(doc, h));
                tables.push(Table { el: p, style, before: prev_para.take(), after: None, near: paras.len().checked_sub(1), header });
                last_tbl = Some(tables.len() - 1);
                continue;
            }
            let idx = paras.len();
            if let Some(t) = last_tbl.take() {
                tables[t].after = Some(idx);
            }
            prev_para = Some(idx);
            let runs = scan_runs(doc, p);
            let text: String = runs.iter().map(|r| r.text.as_str()).collect();
            let ppr_el = doc.wchild(p, "pPr");
            let sid = ppr_el
                .and_then(|x| doc.wchild(x, "pStyle"))
                .and_then(|s| doc.wattr(s, "val"))
                .map(String::from)
                .or_else(|| styles.default_p.clone());
            let style_name = sid
                .as_ref()
                .map(|s| styles.name.get(s).cloned().unwrap_or_else(|| s.clone()))
                .unwrap_or_default();
            let mut ppr = styles.p_props(sid.as_deref());
            read_ppr(doc, ppr_el, &mut ppr);
            let outline = ppr.outline_lvl.or_else(|| {
                HEADING.captures(style_name.trim()).and_then(|m| m[1].parse::<i64>().ok()).map(|n| n - 1)
            });
            let desc = doc.descendants(p);
            let has_math = desc.iter().any(|&e| doc.is(e, &Ns::M, "oMath"));
            let graphic =
                desc.iter().any(|&e| doc.is_w(e, "drawing") || doc.is_w(e, "pict") || doc.is(e, &Ns::Mc, "AlternateContent"));
            // 浮动图（wp:anchor）规范没有位置依据，不当作图片段处理
            let has_figure = graphic && !desc.iter().any(|&e| doc.local(e) == "anchor");
            paras.push(Para {
                idx,
                el: p,
                has_tab: text.contains('\t'),
                has_math,
                has_figure,
                text,
                sid,
                style_name,
                ppr,
                runs,
                role: None,
                ambiguous: false,
                chapter: None,
                outline,
                note: false,
            });
            cur.paras.push(idx);
            if let Some(sp) = ppr_el.and_then(|x| doc.wchild(x, "sectPr")) {
                cur.el = Some(sp);
                let next = Section { index: sections.len() + 1, el: None, paras: vec![] };
                sections.push(std::mem::replace(&mut cur, next));
            }
        }
        cur.el = doc.wchild(body, "sectPr");
        sections.push(cur);
        Ok(Model { styles, paras, tables, sections })
    }

    pub fn first_text_para(&self, sec: &Section) -> Option<usize> {
        sec.paras.iter().copied().find(|&i| !self.paras[i].text.trim().is_empty())
    }

    /// run 的有效字符属性。
    pub fn run_eff(&self, doc: &Doc, para: &Para, run: &Run) -> RProps {
        let rpr = doc.wchild(run.el, "rPr");
        let rs = rpr.and_then(|x| doc.wchild(x, "rStyle")).and_then(|s| doc.wattr(s, "val"));
        let mut acc = self.styles.r_base(para.sid.as_deref(), rs);
        read_rpr(doc, rpr, &mut acc);
        acc
    }

    /// 任意段落元素（页脚等）的有效段落属性及其样式 id。
    pub fn para_eff_from_el(&self, doc: &Doc, p: Id) -> (Option<String>, PProps) {
        self.para_eff_in(doc, p, None)
    }

    fn para_eff_in(&self, doc: &Doc, p: Id, tsid: Option<&str>) -> (Option<String>, PProps) {
        let ppr_el = doc.wchild(p, "pPr");
        let sid = ppr_el
            .and_then(|x| doc.wchild(x, "pStyle"))
            .and_then(|s| doc.wattr(s, "val"))
            .map(String::from)
            .or_else(|| self.styles.default_p.clone());
        let mut ppr = self.styles.p_props_in(tsid, sid.as_deref());
        read_ppr(doc, ppr_el, &mut ppr);
        (sid, ppr)
    }

    /// 表格单元格内一个段落：叠加表格样式后的有效段落属性，以及各非空 run 的 (文字, 有效字符属性)。
    pub fn cell_para(&self, doc: &Doc, p: Id, tsid: Option<&str>) -> (PProps, Vec<(String, RProps)>) {
        let (sid, ppr) = self.para_eff_in(doc, p, tsid);
        let runs = scan_runs(doc, p)
            .into_iter()
            .filter(|r| !r.text.trim().is_empty())
            .map(|r| {
                let rpr = doc.wchild(r.el, "rPr");
                let rs = rpr.and_then(|x| doc.wchild(x, "rStyle")).and_then(|s| doc.wattr(s, "val"));
                let mut acc = self.styles.r_base_in(tsid, sid.as_deref(), rs);
                read_rpr(doc, rpr, &mut acc);
                (r.text, acc)
            })
            .collect();
        (ppr, runs)
    }
}
