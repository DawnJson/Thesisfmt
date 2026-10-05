//! 从模板 docx 提取规则包草稿：页面设置、页码格式、关键词分隔符、题注编号方式与各角色的段落格式。
//! 每项取模板里的多数值；同一角色的段落格式不一致（多数值不足六成）就不写该项，不猜。

use super::checks::{classify_sections, jc_norm, Kind, CM};
use super::model::{Model, Para, ASCII, EAST_ASIA, HANSI};
use super::pack::{role_label, size_name, Pack, PackMeta, ROLE_LABELS};
use super::roles::{keywords_start, CAP, CAP_FLAT};
use super::xml::Doc;
use regex::Regex;
use serde_yaml_ng::{Mapping, Value};
use std::sync::LazyLock;

static CN_CHAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\u{4e00}-\u{9fff}]").unwrap());
static LATIN_CHAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Za-z0-9]").unwrap());
static KW_SPLIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[;；,，、]").unwrap());

/// 多数值至少占这么多才采用。
const MAJORITY: f64 = 0.6;

/// 占比不低于 `MAJORITY` 的值；没有就 None。
fn majority<T: PartialEq + Clone>(vals: &[T]) -> Option<T> {
    let mut counts: Vec<(&T, usize)> = vec![];
    for v in vals {
        match counts.iter_mut().find(|(x, _)| *x == v) {
            Some(c) => c.1 += 1,
            None => counts.push((v, 1)),
        }
    }
    let (v, n) = counts.into_iter().max_by_key(|&(_, n)| n)?;
    (n as f64 >= vals.len() as f64 * MAJORITY).then(|| v.clone())
}

fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn num(v: f64) -> Value {
    let v = r2(v);
    if v.fract() == 0.0 {
        Value::from(v as i64)
    } else {
        Value::from(v)
    }
}

struct Map(Mapping);

impl Map {
    fn new(source: String) -> Map {
        let mut m = Mapping::new();
        m.insert("source".into(), source.into());
        Map(m)
    }
    fn put(&mut self, k: &str, v: Option<Value>) {
        if let Some(v) = v {
            self.0.insert(k.into(), v);
        }
    }
    /// 除 source 外还有内容。
    fn has_rules(&self) -> bool {
        self.0.len() > 1
    }
}

fn twip_cm(t: i64) -> f64 {
    t as f64 / CM
}

/// 段落的首行缩进（字符；负数为悬挂）。
fn indent_chars(model: &Model, doc: &Doc, p: &Para) -> f64 {
    let nz = |v: Option<i64>| v.filter(|&x| x != 0);
    let size = || {
        p.runs
            .iter()
            .find(|r| !r.text.trim().is_empty())
            .map_or(12.0, |r| model.run_eff(doc, p, r).sz.unwrap_or(20) as f64 / 2.0)
    };
    let ppr = &p.ppr;
    if let Some(c) = nz(ppr.first_line_chars) {
        c as f64 / 100.0
    } else if let Some(c) = nz(ppr.hanging_chars) {
        -(c as f64) / 100.0
    } else if let Some(h) = nz(ppr.hanging) {
        -(h as f64) / 20.0 / size()
    } else if let Some(f) = nz(ppr.first_line) {
        f as f64 / 20.0 / size()
    } else {
        0.0
    }
}

/// 一个角色的段落格式；没有可确定的项返回 None。
fn role_rules(model: &Model, doc: &Doc, role: &str, paras: &[&Para], template: &str) -> Option<Value> {
    let (mut cn, mut lat, mut sz, mut b) = (vec![], vec![], vec![], vec![]);
    for p in paras {
        for r in p.runs.iter().filter(|r| !r.text.trim().is_empty()) {
            let eff = model.run_eff(doc, p, r);
            if CN_CHAR.is_match(&r.text) {
                cn.push(model.styles.font(&eff, EAST_ASIA));
            }
            if LATIN_CHAR.is_match(&r.text) {
                lat.push(model.styles.font(&eff, ASCII).or_else(|| model.styles.font(&eff, HANSI)));
            }
            sz.push(eff.sz.unwrap_or(20));
            b.push(eff.b.unwrap_or(false));
        }
    }
    let align: Vec<&str> = paras.iter().map(|p| jc_norm(p.ppr.jc.as_deref())).collect();
    let line: Vec<(String, i64)> = paras
        .iter()
        .map(|p| (p.ppr.line_rule.clone().unwrap_or_else(|| "auto".into()), p.ppr.line.unwrap_or(240)))
        .collect();
    // 段前段后：以行计的记作 (true, 百分之一行)，以磅计的记作 (false, twip)
    let spacing = |pt: Option<i64>, lines: Option<i64>| match lines {
        Some(l) if l != 0 => (true, l),
        _ => (false, pt.unwrap_or(0)),
    };
    let before: Vec<_> = paras.iter().map(|p| spacing(p.ppr.before, p.ppr.before_lines)).collect();
    let after: Vec<_> = paras.iter().map(|p| spacing(p.ppr.after, p.ppr.after_lines)).collect();
    // 带自动编号的段落缩进由编号定义决定，不参与
    let indent: Vec<i64> = paras.iter().filter(|p| p.ppr.num_pr != Some(true)).map(|p| (indent_chars(model, doc, p) * 2.0).round() as i64).collect();

    let mut m = Map::new(format!("模板《{template}》中的{}（{} 段）", role_label(role), paras.len()));
    m.put("cn_font", majority(&cn).flatten().map(Value::from));
    m.put("latin_font", majority(&lat).flatten().map(Value::from));
    m.put("size", majority(&sz).map(|v| Value::from(size_name(v as f64 / 2.0))));
    m.put("bold", majority(&b).map(Value::from));
    m.put("align", majority(&align).map(Value::from));
    m.put(
        "line",
        majority(&line).map(|(rule, v)| {
            let mut l = Mapping::new();
            let val = if rule == "auto" { v as f64 / 240.0 } else { v as f64 / 20.0 };
            l.insert("rule".into(), rule.into());
            l.insert("value".into(), num(val));
            Value::Mapping(l)
        }),
    );
    for (key, vals) in [("before", &before), ("after", &after)] {
        match majority(vals) {
            Some((true, l)) => m.put(&format!("{key}_lines"), Some(num(l as f64 / 100.0))),
            Some((false, v)) => m.put(key, Some(num(v as f64 / 20.0))),
            None => {}
        }
    }
    match majority(&indent) {
        Some(h) if h < 0 => m.put("hanging_chars", Some(num(-h as f64 / 2.0))),
        Some(h) => m.put("first_line_chars", Some(num(h as f64 / 2.0))),
        None => {}
    }
    m.has_rules().then_some(Value::Mapping(m.0))
}

fn page_rules(model: &Model, doc: &Doc, kinds: &[Option<Kind>], template: &str) -> (Option<Value>, Option<Value>) {
    let attr = |el, a| doc.wattr(el, a).and_then(|v| v.parse::<i64>().ok());
    let (mut paper, mut margins, mut dist) = (vec![], vec![], vec![]);
    let (mut front, mut body) = (vec![], vec![]);
    for (sec, kind) in model.sections.iter().zip(kinds) {
        let Some(sp) = sec.el else { continue };
        let fmt = doc
            .wchild(sp, "pgNumType")
            .and_then(|e| doc.wattr(e, "fmt"))
            .filter(|v| !v.is_empty())
            .unwrap_or("decimal")
            .to_string();
        match kind {
            Some(Kind::Front) => front.push(fmt),
            Some(Kind::Body) => body.push(fmt),
            _ => {}
        }
        if *kind != Some(Kind::Body) {
            continue;
        }
        if let Some(sz) = doc.wchild(sp, "pgSz") {
            let (w, h) = (attr(sz, "w").unwrap_or(0), attr(sz, "h").unwrap_or(0));
            let (w, h) = if doc.wattr(sz, "orient") == Some("landscape") { (h, w) } else { (w, h) };
            paper.push((w - super::checks::A4.0).abs() <= 40 && (h - super::checks::A4.1).abs() <= 40);
        }
        if let Some(mar) = doc.wchild(sp, "pgMar") {
            let get = |a| attr(mar, a).map(|v| (twip_cm(v) * 100.0).round() as i64);
            margins.push([get("top"), get("bottom"), get("left"), get("right")]);
            dist.push([get("header"), get("footer")]);
        }
    }
    let mut page = Map::new(format!("模板《{template}》正文各节的页面设置"));
    page.put("paper", (majority(&paper) == Some(true)).then(|| "A4".into()));
    if let Some(m) = majority(&margins) {
        let mut mm = Mapping::new();
        for (k, v) in ["top", "bottom", "left", "right"].into_iter().zip(m) {
            if let Some(v) = v {
                mm.insert(k.into(), num(v as f64 / 100.0));
            }
        }
        if !mm.is_empty() {
            page.put("margins_cm", Some(Value::Mapping(mm)));
        }
    }
    if let Some([h, f]) = majority(&dist) {
        page.put("header_distance_cm", h.map(|v| num(v as f64 / 100.0)));
        page.put("footer_distance_cm", f.map(|v| num(v as f64 / 100.0)));
    }
    let known = |f: String| matches!(f.as_str(), "upperRoman" | "lowerRoman" | "decimal").then(|| Value::from(f));
    let mut pn = Map::new(format!("模板《{template}》各节的页码格式"));
    pn.put("front_format", majority(&front).and_then(known));
    pn.put("body_format", majority(&body).and_then(known));
    (page.has_rules().then_some(Value::Mapping(page.0)), pn.has_rules().then_some(Value::Mapping(pn.0)))
}

fn checks_rules(model: &Model, template: &str) -> Option<Value> {
    let seps = |role: &str| -> Option<String> {
        let found: Vec<String> = model
            .paras
            .iter()
            .filter(|p| p.role == Some(role) && !p.ambiguous)
            .flat_map(|p| {
                let body = keywords_start(&p.text).map_or("", |s| &p.text[s..]);
                KW_SPLIT.find_iter(body).map(|m| m.as_str().to_string()).collect::<Vec<_>>()
            })
            .collect();
        majority(&found)
    };
    let mut checks = Mapping::new();
    if let (Some(zh), Some(en)) = (seps("keywords_zh"), seps("keywords_en")) {
        let mut k = Map::new(format!("模板《{template}》中关键词的分隔符"));
        k.put("zh_separator", Some(zh.into()));
        k.put("en_separator", Some(en.into()));
        checks.insert("keywords".into(), Value::Mapping(k.0));
    }
    // 题注全部按章编号（图 2-1）才要求按章编号
    let caps: Vec<&Para> = model.paras.iter().filter(|p| matches!(p.role, Some("fig_caption" | "table_caption")) && !p.ambiguous).collect();
    let by_chapter = caps.iter().filter(|p| CAP.is_match(p.text.trim())).count();
    if by_chapter > 0 && !caps.iter().any(|p| CAP_FLAT.is_match(p.text.trim())) {
        checks.insert("caption_numbering".into(), Value::Mapping(Map::new(format!("模板《{template}》中的图表题注按章编号")).0));
    }
    (!checks.is_empty()).then_some(Value::Mapping(checks))
}

/// 由模板生成规则包 YAML（已按规则包格式校验）。`template` 写进各项的出处。
pub fn extract_pack(docx: &[u8], meta: &PackMeta, template: &str) -> Result<String, String> {
    let a = super::analyze(docx, "thu-master", None)?;
    let (model, doc) = (&a.model, &a.doc);
    let kinds = classify_sections(model);
    let mut root = Mapping::new();
    root.insert("meta".into(), serde_yaml_ng::to_value(meta).map_err(|e| e.to_string())?);
    let (page, pn) = page_rules(model, doc, &kinds, template);
    if let Some(v) = page {
        root.insert("page".into(), v);
    }
    if let Some(v) = pn {
        root.insert("page_numbers".into(), v);
    }
    let mut roles = Mapping::new();
    for (role, _) in ROLE_LABELS {
        let paras: Vec<&Para> = model.paras.iter().filter(|p| p.role == Some(role) && !p.ambiguous && !p.text.trim().is_empty()).collect();
        if paras.is_empty() {
            continue;
        }
        if let Some(v) = role_rules(model, doc, role, &paras, template) {
            roles.insert(role.into(), v);
        }
    }
    if roles.is_empty() {
        return Err("模板里没有认出章标题、正文、摘要等段落，无法提取规则".into());
    }
    root.insert("roles".into(), Value::Mapping(roles));
    if let Some(v) = checks_rules(model, template) {
        root.insert("checks".into(), v);
    }
    let yaml = serde_yaml_ng::to_string(&Value::Mapping(root)).map_err(|e| e.to_string())?;
    Pack::parse(&yaml).map_err(|e| format!("提取结果不是合法规则包（程序错误）: {e}"))?;
    Ok(yaml)
}
