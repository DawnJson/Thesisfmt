//! 改正文文字或结构的修复。只在用户逐条确认后执行（`apply_confirmed`），不进入「全部修复」「修复本组」。
//!
//! 文字修改是段落文字（`Para.text`，按 run 拼接、制表符记为 `\t`）上的字符区间替换。
//! 落到 `w:t` 节点时，替换文字写进区间起点所在的节点，其余节点只删字，run 的格式不变；
//! 区间碰到制表符、域、修订内的文字时不提供修改。
//!
//! 表格拆分（续表）：在跨页处把表拆成两个表，中间插入“续表”题注（段前分页），表头行复制到续表顶端。

use super::fix::first_child;
use super::model::{get_or_add, scan_runs, toggle, Model, PPR_ORDER};
use super::xml::{Doc, Id, Ns};
use serde::Serialize;

/// 段落 `para`（模型段号）上的若干处替换：(起, 止, 新文字)，字符偏移，互不重叠、按起点升序。
#[derive(Clone)]
pub struct TextEdit {
    pub para: usize,
    pub edits: Vec<(usize, usize, String)>,
}

/// 需用户确认的修改动作。
#[derive(Clone)]
pub enum Edit {
    Text(TextEdit),
    /// 第 `table` 个表在第 `row` 行（0 起）前拆开，续表前插入题注 `caption`，格式取自原表题段 `cap`。
    SplitTable { table: usize, row: usize, cap: usize, caption: String },
}

/// 给用户看的待确认改动。`token` 原样传回执行接口，重算出的不一致就拒绝（文档已变）。
#[derive(Serialize, Clone, PartialEq, Debug)]
pub struct Confirm {
    /// "text"：改段落文字；"split_table"：拆分表格为续表。
    pub kind: &'static str,
    /// 改动说明。
    pub summary: String,
    /// 改段落文字时的改前 / 改后对照。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<TextChange>,
    pub token: String,
}

/// 给用户确认的改动：整段改前 / 改后，以及逐段对照（保留 → 删去 → 换成）。
#[derive(Serialize, Clone, PartialEq, Debug)]
pub struct TextChange {
    pub before: String,
    pub after: String,
    pub segs: Vec<Seg>,
}

/// 字节偏移 → 字符偏移。
pub fn char_at(text: &str, byte: usize) -> usize {
    text[..byte].chars().count()
}

/// 把 `old` 改成 `new` 的最小一处替换（去掉公共前后缀），偏移从 `base` 起算；两者相同时为 None。
pub fn diff(old: &str, new: &str, base: usize) -> Option<(usize, usize, String)> {
    let (a, b): (Vec<char>, Vec<char>) = (old.chars().collect(), new.chars().collect());
    let pre = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    if pre == a.len() && pre == b.len() {
        return None;
    }
    let max = a.len().min(b.len()) - pre;
    let suf = a.iter().rev().zip(b.iter().rev()).take(max).take_while(|(x, y)| x == y).count();
    Some((base + pre, base + a.len() - suf, b[pre..b.len() - suf].iter().collect()))
}

#[derive(Serialize, Clone, PartialEq, Debug)]
pub struct Seg {
    pub keep: String,
    pub del: String,
    pub ins: String,
}

pub fn change(text: &str, edits: &[(usize, usize, String)]) -> TextChange {
    let chars: Vec<char> = text.chars().collect();
    let (mut segs, mut at) = (vec![], 0);
    for (s, e, rep) in edits {
        segs.push(Seg { keep: chars[at..*s].iter().collect(), del: chars[*s..*e].iter().collect(), ins: rep.clone() });
        at = *e;
    }
    segs.push(Seg { keep: chars[at..].iter().collect(), del: String::new(), ins: String::new() });
    let after = segs.iter().flat_map(|s| [s.keep.as_str(), s.ins.as_str()]).collect();
    TextChange { before: text.to_string(), after, segs }
}

/// `Para.text` 每个字符所在的 (`w:t` 节点, 节点内字符偏移)；制表符、不间断连字符、不安全 run 与文本框里的字符为 None
/// （文本框在 mc:Choice / mc:Fallback 里各有一份，只改一份会让两者不一致）。
fn slots(doc: &Doc, p: Id) -> Vec<Option<(Id, usize)>> {
    let mut out = vec![];
    for r in scan_runs(doc, p) {
        let mut boxed = false;
        let mut cur = doc.parent(r.el);
        while let Some(a) = cur.filter(|&a| a != p) {
            boxed |= doc.is_w(a, "txbxContent") || doc.is(a, &Ns::Mc, "AlternateContent");
            cur = doc.parent(a);
        }
        let ok = r.safe && !boxed;
        for ch in doc.child_els(r.el) {
            if doc.is_w(ch, "t") {
                let n = doc.text(ch).chars().count();
                out.extend((0..n).map(|k| ok.then_some((ch, k))));
            } else if doc.is_w(ch, "tab") || doc.is_w(ch, "noBreakHyphen") {
                out.push(None);
            }
        }
    }
    out
}

/// 插入点（`s == e`）挂在前一个字之后，没有则挂在后一个字之前。
fn anchor(slots: &[Option<(Id, usize)>], s: usize) -> Option<(Id, usize)> {
    match s.checked_sub(1).and_then(|i| slots[i]) {
        Some((t, k)) => Some((t, k + 1)),
        None => slots.get(s).copied().flatten(),
    }
}

/// 每处替换都能落到可改的 `w:t` 上。
pub fn editable(doc: &Doc, p: Id, edits: &[(usize, usize, String)]) -> bool {
    let sl = slots(doc, p);
    edits.iter().all(|(s, e, _)| {
        *e <= sl.len() && if s == e { anchor(&sl, *s).is_some() } else { sl[*s..*e].iter().all(Option::is_some) }
    })
}

fn splice(doc: &mut Doc, t: Id, from: usize, del: usize, ins: &str) {
    let mut cs: Vec<char> = doc.text(t).chars().collect();
    cs.splice(from..from + del, ins.chars());
    doc.set_text(t, &cs.into_iter().collect::<String>());
}

/// 按区间改写段落 `p` 的文字；从后往前改，前面的偏移不受影响。
pub fn apply(doc: &mut Doc, p: Id, edits: &[(usize, usize, String)]) -> Result<(), String> {
    if !editable(doc, p, edits) {
        return Err("要修改的文字位于域、修订或制表符中，未修改".into());
    }
    let sl = slots(doc, p);
    for (s, e, rep) in edits.iter().rev() {
        if s == e {
            let (t, k) = anchor(&sl, *s).expect("已校验");
            splice(doc, t, k, 0, rep);
            continue;
        }
        // 区间内各节点被删的字数（节点按出现顺序）
        let mut spans: Vec<(Id, usize, usize)> = vec![];
        for &(t, k) in sl[*s..*e].iter().flatten() {
            match spans.last_mut() {
                Some((lt, _, n)) if *lt == t => *n += 1,
                _ => spans.push((t, k, 1)),
            }
        }
        for (i, &(t, k, n)) in spans.iter().enumerate().rev() {
            splice(doc, t, k, n, if i == 0 { rep } else { "" });
        }
    }
    Ok(())
}

/// 文件里所有段落（含表格、文本框里的）的文字，用来核对只改了确认过的那一段。
pub fn para_texts(doc: &Doc) -> Vec<(Id, String)> {
    doc.descendants(doc.root())
        .into_iter()
        .filter(|&p| doc.is_w(p, "p"))
        .map(|p| (p, scan_runs(doc, p).iter().map(|r| r.text.as_str()).collect()))
        .collect()
}

/// 表格的行（直接子元素 `w:tr`）；行包在 sdt / customXml 里时返回 None，不处理。
fn table_rows(doc: &Doc, tbl: Id) -> Option<Vec<Id>> {
    let mut rows = vec![];
    for c in doc.child_els(tbl) {
        if doc.is_w(c, "tr") {
            rows.push(c);
        } else if doc.is_w(c, "sdt") || doc.is_w(c, "customXml") {
            return None;
        }
    }
    Some(rows)
}

/// 表头行数：开头连续设了「标题行重复」的行；都没设时算第一行。
fn header_rows(doc: &Doc, rows: &[Id]) -> usize {
    let repeated = |r: &Id| doc.wchild(*r, "trPr").and_then(|x| doc.wchild(x, "tblHeader")).is_some_and(|h| toggle(doc, h));
    rows.iter().take_while(|r| repeated(r)).count().max(1)
}

fn has_any(doc: &Doc, el: Id, tags: &[&str]) -> bool {
    doc.descendants(el).into_iter().any(|e| tags.iter().any(|t| doc.is_w(e, t)))
}

/// 第 `row` 行前能否拆成续表：拆后原表至少留一行表体；该行没有接续上方的纵向合并单元格；
/// 表头里没有脚注引用、修订（复制后会重复编号或产生假修订）。
pub fn can_split(doc: &Doc, tbl: Id, row: usize) -> bool {
    let Some(rows) = table_rows(doc, tbl) else { return false };
    let h = header_rows(doc, &rows);
    let continues = |tc: Id| {
        doc.wchild(tc, "tcPr").and_then(|x| doc.wchild(x, "vMerge")).is_some_and(|v| doc.wattr(v, "val") != Some("restart"))
    };
    row > h
        && row < rows.len()
        && !doc.child_els(rows[row]).filter(|&c| doc.is_w(c, "tc")).any(continues)
        && !rows[..h].iter().any(|&r| has_any(doc, r, &["footnoteReference", "endnoteReference", "ins", "del", "moveFrom", "moveTo"]))
}

/// 表头各行的段落文字（拆分后续表顶端多出的就是这些）。
pub fn header_texts(doc: &Doc, tbl: Id) -> Vec<String> {
    let rows = table_rows(doc, tbl).unwrap_or_default();
    let h = header_rows(doc, &rows).min(rows.len());
    let ids: Vec<Id> = rows[..h].iter().flat_map(|&r| doc.descendants(r)).collect();
    para_texts(doc).into_iter().filter(|(p, _)| ids.contains(p)).map(|(_, t)| t).collect()
}

/// 第 `row` 行的第一个段落。
pub fn row_first_para(doc: &Doc, tbl: Id, row: usize) -> Option<Id> {
    let r = *table_rows(doc, tbl)?.get(row)?;
    doc.descendants(r).into_iter().find(|&p| doc.is_w(p, "p"))
}

/// 续表题注段：段落格式与第一段文字的字符格式取自原表题 `src`，段前分页；文字为 `text`（制表符写成 `w:tab`）。
fn caption_para(doc: &mut Doc, src: Id, text: &str) -> Id {
    let p = doc.create(src, "p");
    if let Some(ppr) = doc.wchild(src, "pPr") {
        let c = doc.deep_clone(ppr);
        doc.append(p, c);
    }
    let ppr = first_child(doc, p, "pPr");
    for tag in ["sectPr", "pPrChange", "numPr"] {
        if let Some(x) = doc.wchild(ppr, tag) {
            doc.detach(x);
        }
    }
    if let Some(mark) = doc.wchild(ppr, "rPr") {
        for x in doc.child_els(mark).filter(|&x| ["ins", "del", "moveFrom", "moveTo"].iter().any(|t| doc.is_w(x, t))).collect::<Vec<_>>() {
            doc.detach(x);
        }
    }
    let brk = get_or_add(doc, ppr, "pageBreakBefore", PPR_ORDER);
    doc.remove_wattr(brk, "val");
    let r = doc.create(src, "r");
    let rpr = scan_runs(doc, src).iter().find(|r| !r.text.trim().is_empty()).and_then(|r| doc.wchild(r.el, "rPr"));
    if let Some(rpr) = rpr {
        let c = doc.deep_clone(rpr);
        if let Some(x) = doc.wchild(c, "rPrChange") {
            doc.detach(x);
        }
        doc.append(r, c);
    }
    for (i, part) in text.split('\t').enumerate() {
        if i > 0 {
            let tab = doc.create(src, "tab");
            doc.append(r, tab);
        }
        if !part.is_empty() {
            let t = doc.create(src, "t");
            doc.set_text(t, part);
            doc.append(r, t);
        }
    }
    doc.append(p, r);
    p
}

/// 在第 `row` 行前把表拆开：原表留前面各行；其后插入续表题注与新表，新表顶端是表头行的副本（去掉书签、批注标记）。
pub fn split_table(doc: &mut Doc, model: &Model, table: usize, row: usize, cap: usize, caption: &str) -> Result<(), String> {
    let tbl = model.tables[table].el;
    if !can_split(doc, tbl, row) {
        return Err("这个表格无法在该行拆分，未修改".into());
    }
    let rows = table_rows(doc, tbl).expect("已校验");
    let h = header_rows(doc, &rows);
    let new_tbl = doc.deep_clone(tbl);
    let copies = table_rows(doc, new_tbl).expect("结构相同");
    for &r in &copies[h..row] {
        doc.detach(r);
    }
    for &r in &rows[row..] {
        doc.detach(r);
    }
    // 原表的后半段换成新表里的真身，克隆出来的那份丢掉，书签、批注等只留一份
    for (&copy, &orig) in copies[row..].iter().zip(&rows[row..]) {
        doc.insert_before(copy, orig);
        doc.detach(copy);
    }
    let marks = ["bookmarkStart", "bookmarkEnd", "commentRangeStart", "commentRangeEnd", "commentReference", "permStart", "permEnd"];
    for &r in &copies[..h] {
        for x in doc.descendants(r).into_iter().filter(|&x| marks.iter().any(|t| doc.is_w(x, t))).collect::<Vec<_>>() {
            doc.detach(x);
        }
    }
    let p = caption_para(doc, model.paras[cap].el, caption);
    doc.insert_after(tbl, p);
    doc.insert_after(p, new_tbl);
    Ok(())
}
