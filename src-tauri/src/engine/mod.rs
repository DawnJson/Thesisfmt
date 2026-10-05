//! 论文格式引擎：解析 docx → 角色识别 → 按规则包检查 → 批注 / 修复。

mod checks;
mod edit;
#[doc(hidden)]
pub mod extract;
mod fix;
mod layout;
mod model;
mod pack;
mod roles;
#[doc(hidden)]
pub mod survey;
mod xml;

#[cfg(test)]
mod tests;

use checks::{Finding, Fix, Severity};
use model::Model;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Read, Write};
use xml::Doc;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

pub use layout::{Layout, PreciseInfo};
pub use pack::{list_packs, set_user_dir, PackMeta};

/// 校验规则包 YAML（开发工具用）。
#[doc(hidden)]
pub fn validate_pack(yaml: &str) -> Result<PackMeta, String> {
    pack::Pack::parse(yaml).map(|p| p.meta)
}

/// 用未内置的规则包 YAML 检查文档，返回 (规则 id, 说明)（开发工具用）。
#[doc(hidden)]
pub fn check_yaml(docx: &[u8], yaml: &str) -> Result<Vec<(String, String)>, String> {
    let a = analyze_with(docx, pack::Pack::parse(yaml)?, None)?;
    Ok(a.items.into_iter().map(|(f, _)| (f.rule_id, f.message)).collect())
}

/// 从用户选的模板提取规则包，存进导入目录；返回其 meta（未核对规范）。
pub fn import_template(docx: &[u8], name: &str) -> Result<PackMeta, String> {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e| e.to_string())?.as_millis();
    let id = format!("{}{stamp}", pack::USER_PREFIX);
    let path = pack::user_pack_path(&id).ok_or("导入目录未设置")?;
    let meta = PackMeta {
        id: id.clone(),
        title: name.to_string(),
        version: "导入".into(),
        source: format!("用户导入的模板《{name}》"),
        valid_from: None,
        org: None,
        kind: None,
        category: None,
        verified: false,
        user: false,
    };
    let yaml = extract::extract_pack(docx, &meta, name)?;
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| format!("创建导入目录失败: {e}"))?;
    std::fs::write(&path, yaml).map_err(|e| format!("保存规则失败 {}: {e}", path.display()))?;
    Ok(pack::load_pack(&id)?.meta)
}

/// 删除导入的规则包；内置的不能删。
pub fn delete_user_pack(id: &str) -> Result<(), String> {
    let path = pack::user_pack_path(id).ok_or("只能删除导入的模板")?;
    std::fs::remove_file(&path).map_err(|e| format!("删除失败 {}: {e}", path.display()))
}

const AUTHOR: &str = "格式体检";
const INITIALS: &str = "格检";
const COMMENTS_TYPE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/comments";
const COMMENTS_CT: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.comments+xml";

#[derive(Serialize)]
pub struct Summary {
    error: usize,
    warning: usize,
    info: usize,
    manual: usize,
    paragraphs: usize,
    fixable: usize,
}

#[derive(Serialize)]
pub struct RoleInfo {
    index: usize,
    role: Option<&'static str>,
    ambiguous: bool,
    text: String,
}

#[derive(Serialize)]
pub struct Report {
    pack: PackMeta,
    summary: Summary,
    findings: Vec<Finding>,
    roles: Vec<RoleInfo>,
    /// 合并了精确检查时的排版依据。
    precise: Option<PreciseInfo>,
}

struct Rel {
    id: String,
    typ: String,
    target: String,
    external: bool,
}

fn parse_rels(bytes: &[u8]) -> Result<Vec<Rel>, String> {
    let doc = Doc::parse(bytes)?;
    Ok(doc
        .child_els(doc.root())
        .filter(|&r| doc.local(r) == "Relationship")
        .map(|r| {
            let get = |k| doc.plain_attr(r, k).unwrap_or("").to_string();
            Rel {
                id: get("Id"),
                typ: get("Type"),
                target: get("Target"),
                external: doc.plain_attr(r, "TargetMode") == Some("External"),
            }
        })
        .collect())
}

/// 相对 `base_dir`（部件所在目录）解析关系目标，得到包内路径。
fn resolve(base_dir: &str, target: &str) -> String {
    let joined = match target.strip_prefix('/') {
        Some(t) => t.to_string(),
        None if base_dir.is_empty() => target.to_string(),
        None => format!("{base_dir}/{target}"),
    };
    let mut parts: Vec<&str> = vec![];
    for seg in joined.split('/') {
        match seg {
            "." | "" => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

fn dir_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(d, _)| d)
}

fn rels_path(part: &str) -> String {
    let (dir, name) = part.rsplit_once('/').unwrap_or(("", part));
    if dir.is_empty() {
        format!("_rels/{name}.rels")
    } else {
        format!("{dir}/_rels/{name}.rels")
    }
}

fn read_entry(zip: &mut ZipArchive<Cursor<&[u8]>>, name: &str) -> Result<Vec<u8>, String> {
    let mut f = zip.by_name(name).map_err(|e| format!("docx 缺少 {name}: {e}"))?;
    let mut buf = Vec::with_capacity(f.size() as usize);
    f.read_to_end(&mut buf).map_err(|e| format!("读取 {name} 失败: {e}"))?;
    Ok(buf)
}

struct Analysis<'a> {
    pack: pack::Pack,
    zip: ZipArchive<Cursor<&'a [u8]>>,
    main_path: String,
    rels: Vec<Rel>,
    doc: Doc,
    model: Model,
    parts: fix::Parts,
    /// 页眉页脚的 rId 与 "settings" -> zip 内路径
    part_paths: HashMap<String, String>,
    items: Vec<(Finding, Option<Fix>)>,
}

fn analyze<'a>(docx: &'a [u8], pack_id: &str, layout: Option<&Layout>) -> Result<Analysis<'a>, String> {
    analyze_with(docx, pack::load_pack(pack_id)?, layout)
}

fn analyze_with<'a>(docx: &'a [u8], pack: pack::Pack, layout: Option<&Layout>) -> Result<Analysis<'a>, String> {
    let mut zip = ZipArchive::new(Cursor::new(docx)).map_err(|e| format!("不是有效的 docx: {e}"))?;
    let root_rels = parse_rels(&read_entry(&mut zip, "_rels/.rels")?)?;
    let main = root_rels.iter().find(|r| r.typ.ends_with("/officeDocument")).ok_or("docx 缺少主文档部件")?;
    let main_path = resolve("", &main.target);
    let doc = Doc::parse(&read_entry(&mut zip, &main_path)?)?;
    let rels = match read_entry(&mut zip, &rels_path(&main_path)) {
        Ok(b) => parse_rels(&b)?,
        Err(_) => vec![],
    };
    let dir = dir_of(&main_path).to_string();
    let mut part_paths = HashMap::new();
    let mut load = |typ: &str| -> Result<Vec<(String, Doc)>, String> {
        rels.iter()
            .filter(|r| !r.external && r.typ.ends_with(typ))
            .map(|r| {
                let path = resolve(&dir, &r.target);
                let d = Doc::parse(&read_entry(&mut zip, &path)?)?;
                part_paths.insert(if matches!(typ, "/settings" | "/footnotes") { typ[1..].into() } else { r.id.clone() }, path);
                Ok((r.id.clone(), d))
            })
            .collect()
    };
    let styles = load("/styles")?.pop().map(|(_, d)| d);
    let theme = load("/theme")?.pop().map(|(_, d)| d);
    let footers: HashMap<String, Doc> = load("/footer")?.into_iter().collect();
    let headers: HashMap<String, Doc> = load("/header")?.into_iter().collect();
    let settings = load("/settings")?.pop().map(|(_, d)| d);
    let footnotes = load("/footnotes")?.pop().map(|(_, d)| d);
    let mut model = Model::new(&doc, styles, theme.as_ref())?;
    roles::assign_roles(&mut model, &doc);
    let parts = fix::Parts { footers, headers, settings, footnotes, dirty: HashSet::new() };
    let items = checks::run_checks(&model, &doc, &pack, &parts, layout);
    Ok(Analysis { pack, zip, main_path, rels, doc, model, parts, part_paths, items })
}

#[cfg(test)]
pub fn check(docx: &[u8], pack_id: &str) -> Result<Report, String> {
    check_with(docx, pack_id, None)
}

/// `layout` 为 Some 时合并精确检查（分页、目录）的结果。
pub fn check_with(docx: &[u8], pack_id: &str, layout: Option<&Layout>) -> Result<Report, String> {
    let a = analyze(docx, pack_id, layout)?;
    let count = |s| a.items.iter().filter(|(f, _)| f.severity == s).count();
    let summary = Summary {
        error: count(Severity::Error),
        warning: count(Severity::Warning),
        info: count(Severity::Info),
        manual: count(Severity::Manual),
        paragraphs: a.model.paras.len(),
        fixable: a.items.iter().filter(|(f, _)| f.fixable).count(),
    };
    let roles = a
        .model
        .paras
        .iter()
        .map(|p| RoleInfo {
            index: p.idx,
            role: p.role,
            ambiguous: p.ambiguous,
            text: p.text.trim().chars().take(40).collect(),
        })
        .collect();
    let findings = a.items.into_iter().map(|(f, _)| f).collect();
    Ok(Report { pack: a.pack.meta, summary, findings, roles, precise: layout.map(Layout::info) })
}

/// 在段落开头（`at_end` 为 false）或末尾插入空书签。
fn add_bookmark(doc: &mut Doc, p: xml::Id, id: i64, name: &str, at_end: bool) {
    let id = id.to_string();
    let start = doc.create(p, "bookmarkStart");
    doc.set_wattr(start, "id", &id);
    doc.set_wattr(start, "name", name);
    let end = doc.create(p, "bookmarkEnd");
    doc.set_wattr(end, "id", &id);
    if at_end {
        doc.append(p, start);
        doc.append(p, end);
    } else if let Some(ppr) = doc.wchild(p, "pPr") {
        doc.insert_after(ppr, start);
        doc.insert_after(start, end);
    } else {
        doc.insert_at(p, 0, start);
        doc.insert_at(p, 1, end);
    }
}

/// 带书签的副本：在第 N 个正文段落（与 `Finding.paragraph_index` 同一枚举）开头插入空书签 `_tfmt_N`；
/// `tables` 为 true 时再给第 N 个正文层表格的第一个单元格段首、最后一个单元格段末加 `_tfmt_tN_s` / `_tfmt_tN_e`。
/// 原文件不动。
fn marked(docx: &[u8], tables: bool) -> Result<(Vec<u8>, usize, usize), String> {
    let mut zip = ZipArchive::new(Cursor::new(docx)).map_err(|e| format!("不是有效的 docx: {e}"))?;
    let root_rels = parse_rels(&read_entry(&mut zip, "_rels/.rels")?)?;
    let main = root_rels.iter().find(|r| r.typ.ends_with("/officeDocument")).ok_or("docx 缺少主文档部件")?;
    let main_path = resolve("", &main.target);
    let mut doc = Doc::parse(&read_entry(&mut zip, &main_path)?)?;
    let model = Model::new(&doc, None, None)?;
    let mut next_id = doc
        .descendants(doc.root())
        .into_iter()
        .filter(|&e| doc.is_w(e, "bookmarkStart"))
        .filter_map(|e| doc.wattr(e, "id").and_then(|v| v.parse::<i64>().ok()))
        .max()
        .map_or(0, |m| m + 1);
    for p in &model.paras {
        add_bookmark(&mut doc, p.el, next_id, &format!("_tfmt_{}", p.idx), false);
        next_id += 1;
    }
    for (n, t) in model.tables.iter().enumerate().filter(|_| tables) {
        let cells = |doc: &Doc, rows: Vec<xml::Id>| -> Vec<xml::Id> {
            rows.into_iter().flat_map(|r| doc.child_els(r).filter(|&c| doc.is_w(c, "tc")).collect::<Vec<_>>()).collect()
        };
        let rows: Vec<_> = doc.child_els(t.el).filter(|&r| doc.is_w(r, "tr")).collect();
        let cells = cells(&doc, rows);
        let paras_of = |doc: &Doc, tc: Option<&xml::Id>| -> Vec<xml::Id> {
            tc.map(|&c| doc.child_els(c).filter(|&p| doc.is_w(p, "p")).collect()).unwrap_or_default()
        };
        let first = paras_of(&doc, cells.first()).first().copied();
        let last = paras_of(&doc, cells.last()).last().copied();
        for (para, suffix, at_end) in [(first, "s", false), (last, "e", true)] {
            if let Some(p) = para {
                add_bookmark(&mut doc, p, next_id, &format!("_tfmt_t{n}_{suffix}"), at_end);
                next_id += 1;
            }
        }
    }
    let bytes = write_zip(&mut zip, HashMap::from([(main_path, doc.to_bytes())]), vec![])?;
    Ok((bytes, model.paras.len(), model.tables.len()))
}

/// 预览用副本：只带段落书签。
pub fn preview(docx: &[u8]) -> Result<Vec<u8>, String> {
    marked(docx, false).map(|(bytes, _, _)| bytes)
}

/// 精确检查用副本：段落与表格书签齐全；同时返回正文段落数与表格数。
pub fn layout_copy(docx: &[u8]) -> Result<(Vec<u8>, usize, usize), String> {
    marked(docx, true)
}

/// 复制原包，替换/追加指定部件；未改动的部件原样拷贝（不重新压缩）。
fn write_zip(
    zip: &mut ZipArchive<Cursor<&[u8]>>,
    mut replace: HashMap<String, Vec<u8>>,
    add: Vec<(String, Vec<u8>)>,
) -> Result<Vec<u8>, String> {
    let err = |e: zip::result::ZipError| format!("写 docx 失败: {e}");
    let mut out = ZipWriter::new(Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for i in 0..zip.len() {
        let name = zip.by_index_raw(i).map_err(err)?.name().to_string();
        match replace.remove(&name) {
            Some(bytes) => {
                out.start_file(name, opts).map_err(err)?;
                out.write_all(&bytes).map_err(|e| e.to_string())?;
            }
            None => out.raw_copy_file(zip.by_index_raw(i).map_err(err)?).map_err(err)?,
        }
    }
    for (name, bytes) in add {
        out.start_file(name, opts).map_err(err)?;
        out.write_all(&bytes).map_err(|e| e.to_string())?;
    }
    Ok(out.finish().map_err(err)?.into_inner())
}

fn now_iso() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let (days, sod) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    let z = days + 719468;
    let (era, doe) = (z.div_euclid(146097), z.rem_euclid(146097));
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", sod / 3600, sod % 3600 / 60, sod % 60)
}

fn add_el(doc: &mut Doc, like: xml::Id, parent: xml::Id, local: &str, attrs: &[(&str, &str)]) -> xml::Id {
    let e = doc.create(like, local);
    for (k, v) in attrs {
        doc.set_wattr(e, k, v);
    }
    doc.append(parent, e);
    e
}

/// 一条批注（w:comment）：第一段带 annotationRef，多行文字拆成多段。
fn add_comment(doc: &mut Doc, root: xml::Id, id: i64, text: &str, date: &str) {
    let id_s = id.to_string();
    let c = add_el(
        doc, root, root, "comment",
        &[("id", &id_s), ("author", AUTHOR), ("date", date), ("initials", INITIALS)],
    );
    for (i, line) in text.split('\n').enumerate() {
        let p = add_el(doc, root, c, "p", &[]);
        let ppr = add_el(doc, root, p, "pPr", &[]);
        add_el(doc, root, ppr, "pStyle", &[("val", "CommentText")]);
        if i == 0 {
            let r = add_el(doc, root, p, "r", &[]);
            let rpr = add_el(doc, root, r, "rPr", &[]);
            add_el(doc, root, rpr, "rStyle", &[("val", "CommentReference")]);
            add_el(doc, root, r, "annotationRef", &[]);
        }
        let r = add_el(doc, root, p, "r", &[]);
        let t = add_el(doc, root, r, "t", &[]);
        doc.set_plain_attr(t, "xml:space", "preserve");
        let tn = doc.create_text(line);
        doc.append(t, tn);
    }
}

#[cfg(test)]
pub fn annotate(docx: &[u8], pack_id: &str) -> Result<Vec<u8>, String> {
    annotate_with(docx, pack_id, None)
}

/// 带批注副本：同一段落的多条问题合并为一条批注，原有批注保留。
pub fn annotate_with(docx: &[u8], pack_id: &str, layout: Option<&Layout>) -> Result<Vec<u8>, String> {
    let mut a = analyze(docx, pack_id, layout)?;
    let mut by_para: Vec<(usize, Vec<&str>)> = vec![];
    for (f, _) in &a.items {
        let Some(pi) = f.paragraph_index.filter(|_| f.severity != Severity::Manual) else { continue };
        match by_para.iter_mut().find(|(i, _)| *i == pi) {
            Some((_, msgs)) => msgs.push(&f.message),
            None => by_para.push((pi, vec![&f.message])),
        }
    }
    let dir = dir_of(&a.main_path).to_string();
    let existing = a.rels.iter().find(|r| !r.external && r.typ == COMMENTS_TYPE);
    let comments_path = match existing {
        Some(r) => resolve(&dir, &r.target),
        None => resolve(&dir, "comments.xml"),
    };
    let new_part = existing.is_none();
    let mut cdoc = if new_part {
        Doc::parse(
            b"<w:comments xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"/>",
        )?
    } else {
        Doc::parse(&read_entry(&mut a.zip, &comments_path)?)?
    };
    let croot = cdoc.root();
    let mut next_id = cdoc
        .child_els(croot)
        .filter(|&c| cdoc.local(c) == "comment")
        .filter_map(|c| cdoc.wattr(c, "id").and_then(|v| v.parse::<i64>().ok()))
        .max()
        .map_or(0, |m| m + 1);
    let date = now_iso();
    for (pi, msgs) in &by_para {
        let para = &a.model.paras[*pi];
        let direct: Vec<_> = para.runs.iter().filter(|r| a.doc.parent(r.el) == Some(para.el) && r.safe).collect();
        let runs: Vec<_> = if direct.is_empty() { para.runs.iter().collect() } else { direct };
        let (Some(first), Some(last)) = (runs.first(), runs.last()) else { continue };
        let text = match msgs.as_slice() {
            [m] => m.to_string(),
            ms => ms.iter().enumerate().map(|(i, m)| format!("{}. {m}", i + 1)).collect::<Vec<_>>().join("\n"),
        };
        add_comment(&mut cdoc, croot, next_id, &text, &date);
        let id_s = next_id.to_string();
        let doc = &mut a.doc;
        let start = doc.create(para.el, "commentRangeStart");
        doc.set_wattr(start, "id", &id_s);
        doc.insert_before(first.el, start);
        let end = doc.create(para.el, "commentRangeEnd");
        doc.set_wattr(end, "id", &id_s);
        let r = doc.create(para.el, "r");
        let rpr = add_el(doc, para.el, r, "rPr", &[]);
        add_el(doc, para.el, rpr, "rStyle", &[("val", "CommentReference")]);
        add_el(doc, para.el, r, "commentReference", &[("id", &id_s)]);
        doc.insert_after(last.el, r);
        doc.insert_after(last.el, end);
        next_id += 1;
    }
    let mut replace = HashMap::from([(a.main_path.clone(), a.doc.to_bytes())]);
    let mut add = vec![];
    if new_part {
        let rels_name = rels_path(&a.main_path);
        let mut rels = Doc::parse(&read_entry(&mut a.zip, &rels_name)?)?;
        let rroot = rels.root();
        let max = a
            .rels
            .iter()
            .filter_map(|r| r.id.strip_prefix("rId").and_then(|n| n.parse::<u32>().ok()))
            .max()
            .unwrap_or(0);
        let rel = rels.create(rroot, "Relationship");
        rels.set_plain_attr(rel, "Id", &format!("rId{}", max + 1));
        rels.set_plain_attr(rel, "Type", COMMENTS_TYPE);
        rels.set_plain_attr(rel, "Target", "comments.xml");
        rels.append(rroot, rel);
        replace.insert(rels_name, rels.to_bytes());

        let mut ct = Doc::parse(&read_entry(&mut a.zip, "[Content_Types].xml")?)?;
        let croot_ct = ct.root();
        let ov = ct.create(croot_ct, "Override");
        ct.set_plain_attr(ov, "PartName", &format!("/{comments_path}"));
        ct.set_plain_attr(ov, "ContentType", COMMENTS_CT);
        ct.append(croot_ct, ov);
        replace.insert("[Content_Types].xml".into(), ct.to_bytes());
        add.push((comments_path, cdoc.to_bytes()));
    } else {
        replace.insert(comments_path, cdoc.to_bytes());
    }
    write_zip(&mut a.zip, replace, add)
}

/// 正文与页眉页脚的全部文字（按部件路径排序，保证前后可比）。
fn fingerprint(doc: &Doc, parts: &fix::Parts) -> Vec<String> {
    let texts = |d: &Doc| -> Vec<String> {
        d.descendants(d.root()).into_iter().filter(|&t| d.is_w(t, "t")).map(|t| d.text(t)).collect()
    };
    let mut hf: Vec<(&String, &Doc)> = parts.headers.iter().chain(&parts.footers).collect();
    hf.sort_by_key(|(id, _)| *id);
    let mut out = texts(doc);
    for (_, d) in hf {
        out.push(String::new());
        out.extend(texts(d));
    }
    out
}

/// 修复副本及 (fixed, skipped)；正文文字指纹前后不一致时返回 Err。
#[cfg(test)]
fn fix_with_stats(docx: &[u8], pack_id: &str) -> Result<(Vec<u8>, usize, usize), String> {
    fix_with(docx, pack_id, None, None, fix::apply_fixes)
}

type Apply = fn(&mut Doc, &Model, &pack::Pack, &[(Finding, Option<Fix>)], &mut fix::Parts) -> (usize, usize);

/// `ids` 为 Some 时只执行这些 finding 的修复动作；`layout` 须与生成这些 id 的那次检查相同，编号才对得上。
fn fix_with(
    docx: &[u8],
    pack_id: &str,
    ids: Option<&[String]>,
    layout: Option<&Layout>,
    apply: Apply,
) -> Result<(Vec<u8>, usize, usize), String> {
    let mut a = analyze(docx, pack_id, layout)?;
    if let Some(ids) = ids {
        a.items.retain(|(f, _)| ids.contains(&f.id));
    }
    let before = fingerprint(&a.doc, &a.parts);
    let (fixed, skipped) = apply(&mut a.doc, &a.model, &a.pack, &a.items, &mut a.parts);
    if fingerprint(&a.doc, &a.parts) != before {
        return Err("修复前后正文文字不一致，已中止，未生成文件".into());
    }
    let mut replace = HashMap::from([(a.main_path.clone(), a.doc.to_bytes())]);
    for key in &a.parts.dirty {
        let part = match key.as_str() {
            "settings" => a.parts.settings.as_ref(),
            "footnotes" => a.parts.footnotes.as_ref(),
            id => a.parts.headers.get(id).or(a.parts.footers.get(id)),
        };
        if let (Some(d), Some(path)) = (part, a.part_paths.get(key)) {
            replace.insert(path.clone(), d.to_bytes());
        }
    }
    Ok((write_zip(&mut a.zip, replace, vec![])?, fixed, skipped))
}

/// 只修复 `ids` 指定的问题（id 与 `layout` 取自对同一份字节的最近一次 `check_with`）。
pub fn fix_selected(docx: &[u8], pack_id: &str, ids: &[String], layout: Option<&Layout>) -> Result<Vec<u8>, String> {
    fix_with(docx, pack_id, Some(ids), layout, fix::apply_fixes).map(|(bytes, _, _)| bytes)
}

/// 执行一条已确认的修改（改文字、拆续表）：`id` 取自对同一份字节、同一 `layout` 的最近一次检查，
/// `token` 是用户确认时看到的 `Confirm.token`。重新计算改动并核对：token 须与确认时一致；
/// 改文字时全文只有这一段变化且等于确认的文字，拆续表时全文段落只多出续表题注与表头副本。页眉页脚等其他部件不动。
pub fn apply_confirmed(docx: &[u8], pack_id: &str, id: &str, layout: Option<&Layout>, token: &str) -> Result<Vec<u8>, String> {
    let mut a = analyze(docx, pack_id, layout)?;
    let (f, _) = a.items.iter().find(|(f, _)| f.id == id).ok_or("找不到这条问题，请重新检查")?;
    let (Some(c), Some(e)) = (&f.confirm, f.edit.clone()) else { return Err("这条问题没有需确认的修改".into()) };
    if c.token != token {
        return Err("文档已变化，改动与确认时不一致，请重新检查".into());
    }
    let before = edit::para_texts(&a.doc);
    let texts = |v: &[(xml::Id, String)]| v.iter().map(|(_, t)| t.clone()).collect::<Vec<_>>();
    let ok = match e {
        edit::Edit::Text(e) => {
            let p = a.model.paras[e.para].el;
            edit::apply(&mut a.doc, p, &e.edits)?;
            let after = edit::para_texts(&a.doc);
            before.len() == after.len()
                && before.iter().zip(&after).all(|((i, x), (j, y))| i == j && if *i == p { *y == c.token } else { x == y })
        }
        edit::Edit::SplitTable { table, row, cap, caption } => {
            let tbl = a.model.tables[table].el;
            let first = edit::row_first_para(&a.doc, tbl, row).ok_or("找不到要拆分的行")?;
            let x = before.iter().position(|(p, _)| *p == first).ok_or("找不到要拆分的行")?;
            let mut want = texts(&before);
            want.splice(x..x, std::iter::once(caption.clone()).chain(edit::header_texts(&a.doc, tbl)));
            edit::split_table(&mut a.doc, &a.model, table, row, cap, &caption)?;
            texts(&edit::para_texts(&a.doc)) == want
        }
    };
    if !ok {
        return Err("修改结果与确认的内容不符，已中止，未修改文件".into());
    }
    let replace = HashMap::from([(a.main_path.clone(), a.doc.to_bytes())]);
    write_zip(&mut a.zip, replace, vec![])
}

#[cfg(test)]
pub fn fix(docx: &[u8], pack_id: &str) -> Result<Vec<u8>, String> {
    fix_with_stats(docx, pack_id).map(|(bytes, _, _)| bytes)
}
