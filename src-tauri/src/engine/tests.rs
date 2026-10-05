//! 引擎测试：fixture 用 XML 字符串拼出最小 docx，不提交二进制。

use super::model::EAST_ASIA;
use super::pack::Pack;
use super::xml::Doc;
use super::*;
use std::io::{Cursor, Read, Write};
use zip::write::SimpleFileOptions;
use serde_json::json;
use zip::ZipWriter;

const PACK: &str = "thu-master";
const W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

// ------------------------------------------------------------------ fixture
/// 角色 -> 合规格式 (中文字体, 西文字体, 字号pt, 对齐, (行距规则, 值), 段前, 段后, 首行缩进字符 / 悬挂)
struct Kind(&'static str, f64, &'static str, (&'static str, f64), f64, f64, Option<f64>);

fn kind(name: &str) -> Kind {
    match name {
        "chapter" => Kind("黑体", 16.0, "center", ("auto", 1.0), 24.0, 18.0, None),
        "section_1" => Kind("黑体", 14.0, "left", ("exact", 20.0), 24.0, 6.0, None),
        "body" => Kind("宋体", 12.0, "justify", ("exact", 20.0), 0.0, 0.0, Some(2.0)),
        "abs_title" => Kind("黑体", 16.0, "center", ("auto", 1.0), 24.0, 18.0, None),
        "fig" => Kind("宋体", 11.0, "center", ("auto", 1.0), 6.0, 12.0, None),
        "tab" => Kind("宋体", 11.0, "center", ("auto", 1.0), 12.0, 6.0, None),
        "ref" => Kind("宋体", 10.5, "left", ("exact", 16.0), 3.0, 0.0, Some(-1.0)), // -1 = 悬挂
        "plain" => Kind("宋体", 12.0, "left", ("exact", 20.0), 0.0, 0.0, None),
        _ => unreachable!(),
    }
}

#[derive(Default, Clone)]
struct Ov {
    cn: Option<&'static str>,
    size: Option<f64>,
    first: Option<f64>,
    align: Option<&'static str>,
    bold: bool,
}

fn add(k: &str, text: &str, ov: Ov) -> String {
    let Kind(cn, size, align, (rule, lv), before, after, first) = kind(k);
    let (cn, size, align, first) = (ov.cn.unwrap_or(cn), ov.size.unwrap_or(size), ov.align.unwrap_or(align), ov.first.or(first));
    let line = if rule == "auto" { format!("w:line=\"{}\" w:lineRule=\"auto\"", lv * 240.0) } else { format!("w:line=\"{}\" w:lineRule=\"exact\"", lv * 20.0) };
    let ind = match first {
        Some(c) if c < 0.0 => "<w:ind w:hanging=\"420\" w:hangingChars=\"200\"/>".to_string(),
        Some(c) => format!("<w:ind w:firstLine=\"{}\" w:firstLineChars=\"{}\"/>", (c * size * 20.0).round(), (c * 100.0) as i64),
        None => String::new(),
    };
    let jc = if align == "justify" { "both" } else { align };
    // §2.3.9.2：章节标题、摘要/目录标题的英文和数字用 Arial
    let lat = if matches!(k, "chapter" | "section_1" | "abs_title") { "Arial" } else { "Times New Roman" };
    format!(
        "<w:p><w:pPr><w:spacing w:before=\"{}\" w:after=\"{}\" {line}/>{ind}<w:jc w:val=\"{jc}\"/></w:pPr>\
         <w:r><w:rPr><w:rFonts w:ascii=\"{lat}\" w:hAnsi=\"{lat}\" w:eastAsia=\"{cn}\"/>{}<w:sz w:val=\"{}\"/></w:rPr>\
         <w:t xml:space=\"preserve\">{text}</w:t></w:r></w:p>",
        before * 20.0,
        after * 20.0,
        if ov.bold { "<w:b/>" } else { "" },
        size * 2.0
    )
}

fn sect(margin_cm: f64, footer: Option<&str>, pgnum: Option<&str>) -> String {
    let m = (margin_cm * 1440.0 / 2.54).round();
    format!(
        "<w:sectPr>{}<w:pgSz w:w=\"11906\" w:h=\"16838\"/><w:pgMar w:top=\"{m}\" w:right=\"{m}\" w:bottom=\"{m}\" w:left=\"{m}\" w:header=\"1247\" w:footer=\"1247\" w:gutter=\"0\"/>{}<w:cols w:space=\"720\"/><w:docGrid w:linePitch=\"360\"/></w:sectPr>",
        footer.map_or(String::new(), |f| format!("<w:footerReference w:type=\"default\" r:id=\"{f}\"/>")),
        pgnum.map_or(String::new(), |f| format!("<w:pgNumType w:fmt=\"{f}\"/>")),
    )
}

struct T {
    paras: Vec<String>,
    last_sect: String,
    styles_extra: String,
    comments: Option<String>,
    /// 页眉部件（完整 XML）；第 i 个对应 rIdH{i+1}、word/header{i+1}.xml
    headers: Vec<String>,
    /// settings.xml 根元素内部内容
    settings: Option<String>,
}

impl T {
    /// 合规（或含已知违规）的三节论文：封面 / 前置（罗马）/ 正文（阿拉伯）。
    fn build(v: bool) -> T {
        let o = Ov::default;
        // 中文摘要 800–1000 字（§2.3.5）
        let abs = "本文研究论文格式自动检查。".repeat(70);
        let mut p = vec![
            add("plain", "清华大学硕士学位论文", Ov { align: Some("center"), ..o() }),
            format!("<w:p><w:pPr>{}</w:pPr></w:p>", sect(3.0, None, None)),
            add("abs_title", "摘  要", o()),
            add("body", &abs, o()),
            add("plain", if v { "关键词：格式、检查、论文、规则、批注、修复" } else { "关键词：格式；检查；论文" }, o()),
            add("abs_title", "Abstract", o()),
            add("body", "This thesis studies format checking.", o()),
            add("plain", if v { "Key words: format, check" } else { "Key words: format; check; thesis" }, o()),
            add("abs_title", "目  录", o()),
        ];
        let front = sect(if v { 2.5 } else { 3.0 }, Some("rId3"), Some(if v { "decimal" } else { "upperRoman" }))
            .replacen("<w:pgNumType ", "<w:pgNumType w:start=\"1\" ", 1);
        p.push(format!("<w:p><w:pPr>{front}</w:pPr></w:p>"));
        p.push(add("chapter", "第1章 绪论", if v { Ov { cn: Some("宋体"), size: Some(15.0), ..o() } } else { o() }));
        p.push(add("section_1", "1.1 研究背景", o()));
        p.push(add("body", "论文格式检查是一项繁琐的工作[1]，系统框架与处理流程见图1-1、图1-2和图2-1，本文使用Python实现。", o()));
        p.push(add("body", "研究者常常需要手工逐项核对页边距、字体和行距等格式要求。", if v { Ov { first: Some(0.0), size: Some(10.5), ..o() } } else { o() }));
        p.push(add("body", if v { "他说,这样不行(见表1)。样例ＡＢＣ。" } else { "他说，这样不行。" }, o()));
        p.push(add("fig", "图1-1 系统框架", o()));
        p.push(add("fig", if v { "图1-3 处理流程" } else { "图1-2 处理流程" }, o()));
        p.push(add("chapter", "第2章 方法", o()));
        p.push(add("fig", "图2-1 示例", o()));
        p.push(add("chapter", "参考文献", o()));
        p.push(add("ref", "[1] 张三. 论文写作[M]. 北京: 出版社, 2020.", o()));
        p.push(add("chapter", "致  谢", o()));
        p.push(add("body", "感谢导师。", o()));
        let body = sect(3.0, Some("rId4"), Some("decimal")).replacen("<w:pgNumType ", "<w:pgNumType w:start=\"1\" ", 1);
        T { paras: p, last_sect: body, styles_extra: String::new(), comments: None, headers: vec![], settings: None }
    }

    fn bytes(&self) -> Vec<u8> {
        let ct_comments = if self.comments.is_some() {
            format!("<Override PartName=\"/word/comments.xml\" ContentType=\"{COMMENTS_CT}\"/>")
        } else {
            String::new()
        };
        let rel_comments = if self.comments.is_some() {
            format!("<Relationship Id=\"rId5\" Type=\"{COMMENTS_TYPE}\" Target=\"comments.xml\"/>")
        } else {
            String::new()
        };
        let rel_parts: String = (1..=self.headers.len())
            .map(|i| format!("<Relationship Id=\"rIdH{i}\" Type=\"{REL}/header\" Target=\"header{i}.xml\"/>"))
            .chain(self.settings.iter().map(|_| format!("<Relationship Id=\"rIdS\" Type=\"{REL}/settings\" Target=\"settings.xml\"/>")))
            .collect();
        let footer = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:ftr xmlns:w=\"{W}\"><w:p><w:pPr><w:pStyle w:val=\"Footer\"/><w:jc w:val=\"center\"/></w:pPr>{}</w:p></w:ftr>",
            [
                "<w:fldChar w:fldCharType=\"begin\"/>",
                "<w:instrText xml:space=\"preserve\"> PAGE </w:instrText>",
                "<w:fldChar w:fldCharType=\"end\"/>"
            ]
            .map(|x| format!("<w:r><w:rPr><w:rFonts w:ascii=\"Times New Roman\" w:hAnsi=\"Times New Roman\"/><w:sz w:val=\"21\"/></w:rPr>{x}</w:r>"))
            .concat()
        );
        let mut files: Vec<(&str, String)> = vec![
            ("[Content_Types].xml", format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/>{ct_comments}</Types>")),
            ("_rels/.rels", format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"{REL}/officeDocument\" Target=\"word/document.xml\"/></Relationships>")),
            ("word/_rels/document.xml.rels", format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"{REL}/styles\" Target=\"styles.xml\"/><Relationship Id=\"rId2\" Type=\"{REL}/theme\" Target=\"theme/theme1.xml\"/><Relationship Id=\"rId3\" Type=\"{REL}/footer\" Target=\"footer1.xml\"/><Relationship Id=\"rId4\" Type=\"{REL}/footer\" Target=\"footer2.xml\"/>{rel_comments}{rel_parts}</Relationships>")),
            ("word/document.xml", format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:document xmlns:w=\"{W}\" xmlns:r=\"{REL}\"><w:body>{}{}</w:body></w:document>", self.paras.concat(), self.last_sect)),
            ("word/styles.xml", format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:styles xmlns:w=\"{W}\"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:asciiTheme=\"minorHAnsi\" w:eastAsiaTheme=\"minorEastAsia\" w:hAnsiTheme=\"minorHAnsi\" w:cstheme=\"minorBidi\"/><w:sz w:val=\"22\"/><w:szCs w:val=\"22\"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after=\"200\" w:line=\"276\" w:lineRule=\"auto\"/></w:pPr></w:pPrDefault></w:docDefaults><w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/></w:style>{}</w:styles>", self.styles_extra)),
            ("word/theme/theme1.xml", "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><a:theme xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"><a:themeElements><a:fontScheme name=\"Office\"><a:majorFont><a:latin typeface=\"Calibri Light\"/><a:ea typeface=\"\"/></a:majorFont><a:minorFont><a:latin typeface=\"Calibri\"/><a:ea typeface=\"\"/></a:minorFont></a:fontScheme></a:themeElements></a:theme>".into()),
            ("word/footer1.xml", footer.clone()),
            ("word/footer2.xml", footer),
        ];
        let hdr_names: Vec<String> = (1..=self.headers.len()).map(|i| format!("word/header{i}.xml")).collect();
        files.extend(hdr_names.iter().zip(&self.headers).map(|(n, x)| (n.as_str(), x.clone())));
        if let Some(s) = &self.settings {
            files.push(("word/settings.xml", format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:settings xmlns:w=\"{W}\">{s}</w:settings>")));
        }
        if let Some(c) = &self.comments {
            files.push(("word/comments.xml", c.clone()));
        }
        let mut z = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, content) in files {
            z.start_file(name, SimpleFileOptions::default()).unwrap();
            z.write_all(content.as_bytes()).unwrap();
        }
        z.finish().unwrap().into_inner()
    }
}

fn entry(data: &[u8], name: &str) -> String {
    let mut z = ZipArchive::new(Cursor::new(data)).unwrap();
    let mut s = String::new();
    z.by_name(name).unwrap().read_to_string(&mut s).unwrap();
    s
}

fn rules<'a>(rep: &'a Report, id: &str) -> Vec<&'a Finding> {
    rep.findings.iter().filter(|f| f.rule_id == id).collect()
}

fn msgs(f: &[&Finding]) -> Vec<String> {
    f.iter().map(|f| f.message.clone()).collect()
}

fn by_text(rep: &Report, prefix: &str) -> usize {
    rep.roles.iter().find(|r| r.text.starts_with(prefix)).unwrap().index
}

fn chk(t: &T) -> Report {
    check(&t.bytes(), PACK).unwrap()
}

// ------------------------------------------------------------------ 规则包
#[test]
fn list_packs_loads_every_builtin() {
    // 内置规则包任一非法时 list_packs 会 panic
    let packs = list_packs();
    assert!(packs.iter().any(|p| p.id == PACK && p.verified && !p.user));
}

#[test]
fn template_extraction_round_trips() {
    // 从合规文档提取的规则再检查它自己：段落格式、页面设置都不应报错
    test_user_dir();
    let bytes = T::build(false).bytes();
    let meta = import_template(&bytes, "示例模板").unwrap();
    assert!(meta.user && !meta.verified && list_packs().iter().any(|p| p.id == meta.id));
    let rep = check(&bytes, &meta.id).unwrap();
    let bad: Vec<_> = rep.findings.iter().filter(|f| f.rule_id.starts_with("role.") || f.rule_id.starts_with("page.")).map(|f| &f.message).collect();
    assert!(bad.is_empty(), "{bad:?}");
    // 违规版本按提取的规则照样报出（正文首行缩进 0、字号五号）
    let v = check(&T::build(true).bytes(), &meta.id).unwrap();
    assert!(!rules(&v, "role.body.indent").is_empty() && !rules(&v, "role.chapter.font").is_empty());
    delete_user_pack(&meta.id).unwrap();
    assert!(check(&bytes, &meta.id).is_err());
    // 只能删导入的；id 不能跳出导入目录
    assert!(delete_user_pack(PACK).is_err() && delete_user_pack("user-../../x").is_err());
}

/// 测试共用的导入目录（导入目录全进程只设一次）。各测试只删自己写的包，不删目录：测试并行，删目录会删掉别的测试的包。
fn test_user_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("tfmt-test-packs");
    pack::set_user_dir(dir.clone());
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn pack_rejects_unknown_field_and_size() {
    let mut raw: serde_yaml_ng::Value = serde_yaml_ng::from_str(include_str!("../../packs/thu-master.yaml")).unwrap();
    let body = &mut raw["roles"]["body"];
    body["colour"] = "red".into();
    let err = Pack::parse(&serde_yaml_ng::to_string(&raw).unwrap()).err().unwrap();
    assert!(err.contains("colour"), "{err}");
    let body = &mut raw["roles"]["body"];
    body.as_mapping_mut().unwrap().remove("colour");
    body["size"] = "超大号".into();
    let err = Pack::parse(&serde_yaml_ng::to_string(&raw).unwrap()).err().unwrap();
    assert!(err.contains("字号"), "{err}");
}

// ------------------------------------------------------------------ 检查
#[test]
fn compliant_has_no_errors() {
    let t = T::build(false).chapter_styles();
    let t = T { headers: vec![hfield("\"标题 1\"", "摘要"), hfield("\"标题 1\"", "绪论")], ..t }.use_headers(Some(1), Some(2));
    let rep = chk(&t);
    let bad: Vec<_> = rep.findings.iter().filter(|f| matches!(f.severity, Severity::Error | Severity::Warning)).map(|f| &f.message).collect();
    assert!(bad.is_empty(), "{bad:?}");
    assert_eq!(rep.summary.manual, 6);
    let role = |prefix: &str| rep.roles.iter().find(|r| r.text.starts_with(prefix)).unwrap().role;
    assert_eq!(role("第1章"), Some("chapter"));
    assert_eq!(role("1.1"), Some("section_1"));
    assert_eq!(role("参考文"), Some("references_title"));
    assert_eq!(role("[1]"), Some("ref_item"));
    assert_eq!(role("清华大"), None); // 封面不当正文
}

#[test]
fn toc_chapter_rows_use_heading_fonts() {
    let mut t = T::build(false);
    let row = |text: &str, cn: &'static str| add("plain", text, Ov { cn: Some(cn), ..Ov::default() });
    // 目录标题在下标 8；章标题行用了宋体（应黑体），节标题行宋体合规
    t.paras.insert(9, row("第1章 绪论……………………1", "宋体"));
    t.paras.insert(10, row("1.1 研究背景……………………1", "宋体"));
    let rep = chk(&t);
    let role = |prefix: &str| rep.roles.iter().find(|r| r.text.starts_with(prefix) && r.text.contains('…')).unwrap().role;
    assert_eq!(role("第1章"), Some("toc_chapter"));
    assert_eq!(role("1.1"), Some("toc_item"));
    let f = rules(&rep, "role.toc_chapter.font");
    assert!(f.len() == 1 && f[0].message.contains("黑体") && f[0].fixable, "{:?}", f.iter().map(|f| &f.message).collect::<Vec<_>>());
    assert!(rules(&rep, "role.toc_item.font").is_empty());
}

#[test]
fn violations_have_specific_rules() {
    let rep = chk(&T::build(true));
    let ids: Vec<_> = rep.findings.iter().map(|f| f.rule_id.as_str()).collect();
    for want in [
        "page.margins", "page_numbers.format", "role.chapter.font", "role.body.font", "role.body.indent",
        "keywords.count", "keywords.separator", "caption.number", "punct.halfwidth", "punct.fullwidth_alnum",
    ] {
        assert!(ids.contains(&want), "缺少 {want}");
    }
    let ch = rules(&rep, "role.chapter.font")[0];
    assert_eq!(ch.paragraph_index, Some(by_text(&rep, "第1章")));
    assert_eq!(ch.message, "章标题中文字体应为 黑体，实际 宋体；字号应为 三号，实际 小三");
    assert_eq!(rules(&rep, "role.body.indent")[0].paragraph_index, Some(by_text(&rep, "研究者")));
    assert_eq!(rules(&rep, "caption.number")[0].paragraph_index, Some(by_text(&rep, "图1-3")));
    assert_eq!(rules(&rep, "keywords.separator").len(), 2);
    let m = rules(&rep, "page.margins")[0];
    assert!(m.severity == Severity::Error && m.paragraph_index == Some(by_text(&rep, "摘"))); // 挂在该节第一个有字的段
    assert!(rules(&rep, "punct.halfwidth").iter().all(|f| f.severity == Severity::Warning));
}

#[test]
fn cover_section_accepts_cover_margins() {
    let cover_sect = |top: f64, bottom: f64, left: f64, right: f64| {
        let tw = |c: f64| (c * 1440.0 / 2.54).round();
        let s = sect(3.0, None, None);
        let m = tw(3.0);
        let s = s.replacen(
            &format!("w:top=\"{m}\" w:right=\"{m}\" w:bottom=\"{m}\" w:left=\"{m}\""),
            &format!("w:top=\"{}\" w:right=\"{}\" w:bottom=\"{}\" w:left=\"{}\"", tw(top), tw(right), tw(bottom), tw(left)),
            1,
        );
        format!("<w:p><w:pPr>{s}</w:pPr></w:p>")
    };
    let with_cover = |s: String| {
        let mut t = T::build(false);
        t.paras[1] = s;
        chk(&t)
    };
    // 中文封面、英文封面的页边距都不算错，也不会被“修复”成正文页边距
    for (t, b, l, r) in [(6.0, 6.0, 4.0, 4.0), (5.5, 5.0, 3.6, 3.6)] {
        let rep = with_cover(cover_sect(t, b, l, r));
        assert!(rules(&rep, "page.margins").is_empty(), "{t} {b} {l} {r}");
    }
    // 都不符合：报告，但不知道是哪一页，不自动修复
    let rep = with_cover(cover_sect(5.0, 5.0, 5.0, 5.0));
    let f = rules(&rep, "page.margins");
    assert!(f.len() == 1 && !f[0].fixable && f[0].message.contains("中文封面"), "{:?}", f.iter().map(|f| &f.message).collect::<Vec<_>>());
    // 正文节用了封面页边距仍然是错的，可修复
    let mut t = T::build(false);
    let last = t.last_sect.clone();
    t.last_sect = last.replace("w:top=\"1701\" w:right=\"1701\" w:bottom=\"1701\" w:left=\"1701\"", "w:top=\"3402\" w:right=\"2268\" w:bottom=\"3402\" w:left=\"2268\"");
    let f = rules(&chk(&t), "page.margins").into_iter().filter(|f| f.fixable).count();
    assert_eq!(f, 1);
}

#[test]
fn ambiguous_not_checked_or_fixed() {
    let mut t = T::build(false);
    // 加粗短句：无法确定是不是标题
    t.paras.push(add("plain", "实验设置", Ov { bold: true, ..Ov::default() }));
    let rep = chk(&t);
    let r = rep.roles.last().unwrap();
    assert!(r.ambiguous && r.role.is_none());
    assert!(!rep.findings.iter().any(|f| f.paragraph_index == Some(r.index)));
}

#[test]
fn docgrid_info() {
    let mut t = T::build(false);
    t.last_sect = t.last_sect.replace("<w:docGrid ", "<w:docGrid w:type=\"lines\" ");
    let g = &chk(&t);
    let g = rules(g, "page.doc_grid");
    assert!(g.len() == 1 && g[0].severity == Severity::Info);
}

// ------------------------------------------------------------------ 继承解析
fn with_eff(t: &T, f: impl FnOnce(&model::Styles, model::RProps)) {
    let bytes = t.bytes();
    let a = analyze(&bytes, PACK, None).unwrap();
    let p = &a.model.paras[0];
    f(&a.model.styles, a.model.run_eff(&a.doc, p, &p.runs[0]));
}

fn simple(styles: &str, para: &str) -> T {
    T { paras: vec![para.into()], last_sect: sect(3.0, None, None), styles_extra: styles.into(), comments: None, headers: vec![], settings: None }
}

#[test]
fn basedon_chain_and_docdefaults() {
    let styles = "<w:style w:type=\"paragraph\" w:styleId=\"A\"><w:name w:val=\"A\"/><w:rPr><w:rFonts w:ascii=\"Arial\" w:hAnsi=\"Arial\"/><w:sz w:val=\"28\"/></w:rPr></w:style>\
                  <w:style w:type=\"paragraph\" w:styleId=\"B\"><w:name w:val=\"B\"/><w:basedOn w:val=\"A\"/><w:rPr><w:rFonts w:eastAsia=\"黑体\"/></w:rPr></w:style>";
    let t = simple(styles, "<w:p><w:pPr><w:pStyle w:val=\"B\"/></w:pPr><w:r><w:t>你好abc</w:t></w:r></w:p>");
    with_eff(&t, |styles, eff| {
        assert_eq!(eff.sz, Some(28)); // 来自 A
        assert_eq!(styles.font(&eff, EAST_ASIA).as_deref(), Some("黑体")); // 来自 B
        assert_eq!(styles.font(&eff, 0).as_deref(), Some("Arial")); // B 的 rFonts 只写 eastAsia，ascii 继承自 A
    });
}

#[test]
fn theme_font_with_empty_east_asia_is_undetermined() {
    with_eff(&simple("", "<w:p><w:r><w:t>你好</w:t></w:r></w:p>"), |styles, eff| {
        assert!(matches!(eff.fonts[EAST_ASIA], Some(model::FontRef::Theme(_))));
        assert_eq!(styles.font(&eff, EAST_ASIA), None);
        assert!(styles.font(&eff, 0).is_some());
    });
}

#[test]
fn direct_east_asia_only_and_size_override() {
    let p = "<w:p><w:r><w:rPr><w:rFonts w:eastAsia=\"楷体\"/><w:sz w:val=\"21\"/></w:rPr><w:t>中文</w:t></w:r></w:p>";
    with_eff(&simple("", p), |styles, eff| {
        assert_eq!(styles.font(&eff, EAST_ASIA).as_deref(), Some("楷体"));
        assert_eq!(eff.sz, Some(21));
        assert_ne!(styles.font(&eff, 0).as_deref(), Some("楷体"));
    });
}

#[test]
fn first_line_chars_wins_over_first_line() {
    let mut t = T::build(false);
    t.paras[12] = t.paras[12].replace("w:firstLine=\"480\"", "w:firstLine=\"0\""); // firstLineChars=200 仍然生效
    assert!(rules(&chk(&t), "role.body.indent").is_empty());
    t.paras[12] = t.paras[12].replace(" w:firstLineChars=\"200\"", "");
    let rep = chk(&t);
    let idx: Vec<_> = rules(&rep, "role.body.indent").iter().map(|f| f.paragraph_index).collect();
    assert_eq!(idx, [Some(12)]);
}

#[test]
fn mixed_runs_report_first_mismatch() {
    let mut t = T::build(false);
    t.paras[12] = t.paras[12].replace(
        "</w:p>",
        "<w:r><w:rPr><w:rFonts w:eastAsia=\"宋体\"/><w:sz w:val=\"21\"/></w:rPr><w:t>后半段</w:t></w:r></w:p>",
    );
    let rep = chk(&t);
    let f = rules(&rep, "role.body.font")[0];
    assert_eq!(f.actual["字号"], "五号");
    assert_eq!(f.actual["混合"], true);
}

#[test]
fn before_lines_overrides_before() {
    let mut t = T::build(false);
    t.paras[12] = t.paras[12].replace("<w:spacing ", "<w:spacing w:beforeLines=\"100\" ");
    let rep = chk(&t);
    assert_eq!(rules(&rep, "role.body.spacing")[0].actual["before"], "1 行");
}

// ------------------------------------------------------------------ 修复
fn texts(data: &[u8]) -> Vec<String> {
    let mut z = ZipArchive::new(Cursor::new(data)).unwrap();
    let re = regex::Regex::new(r"<w:t[ >][^<]*").unwrap();
    let names: Vec<_> = z.file_names().filter(|n| n.starts_with("word/") && n.ends_with(".xml")).map(String::from).collect();
    names
        .into_iter()
        .flat_map(|n| {
            let mut s = String::new();
            z.by_name(&n).unwrap().read_to_string(&mut s).unwrap();
            re.find_iter(&s).map(|m| m.as_str().replace("<w:t xml:space=\"preserve\">", "<w:t>")).collect::<Vec<_>>()
        })
        .collect()
}

fn para_xml(doc: &str, needle: &str) -> String {
    doc.split("<w:p>").find(|s| s.contains(needle)).unwrap().to_string()
}

#[test]
fn fix_clears_fixable_and_keeps_text() {
    let src = T::build(true).bytes();
    let before = check(&src, PACK).unwrap();
    assert!(before.summary.fixable > 0);
    let (out, fixed, skipped) = fix_with_stats(&src, PACK).unwrap();
    assert_eq!((fixed, skipped), (before.summary.fixable, 0));
    let after = check(&out, PACK).unwrap();
    assert!(after.findings.iter().all(|f| !f.fixable));
    assert_eq!(texts(&src), texts(&out));
    let doc = entry(&out, "word/document.xml");
    // 写全了 eastAsia、firstLineChars 与 firstLine 同步
    assert!(para_xml(&doc, "第1章").contains("w:eastAsia=\"黑体\""));
    let body = para_xml(&doc, "研究者");
    assert!(body.contains("w:firstLineChars=\"200\"") && body.contains("w:firstLine=\"480\""));
}

#[test]
fn fix_idempotent_on_compliant() {
    assert_eq!(fix_with_stats(&T::build(false).bytes(), PACK).unwrap().1, 0);
}

fn special() -> T {
    let mut t = T::build(true);
    let fields = [
        "<w:r><w:fldChar w:fldCharType=\"begin\"/></w:r>",
        "<w:r><w:instrText> REF x </w:instrText></w:r>",
        "<w:r><w:fldChar w:fldCharType=\"separate\"/></w:r>",
        "<w:r><w:t>域结果</w:t></w:r>",
        "<w:r><w:fldChar w:fldCharType=\"end\"/></w:r>",
        "<w:ins w:id=\"91\" w:author=\"a\" w:date=\"2024-01-01T00:00:00Z\"><w:r><w:t>插入</w:t></w:r></w:ins>",
        "<w:del w:id=\"92\" w:author=\"a\" w:date=\"2024-01-01T00:00:00Z\"><w:r><w:delText>删除</w:delText></w:r></w:del>",
        "<m:oMath xmlns:m=\"http://schemas.openxmlformats.org/officeDocument/2006/math\"><m:r><m:t>x</m:t></m:r></m:oMath>",
        "<w:fldSimple w:instr=\" PAGEREF y \"><w:r><w:t>简单域</w:t></w:r></w:fldSimple>",
    ];
    t.paras[13] = t.paras[13].replace("</w:p>", &format!("{}</w:p>", fields.concat()));
    t.paras[12] = t.paras[12].replacen("<w:r>", "<w:commentRangeStart w:id=\"0\"/><w:r>", 1).replace(
        "</w:p>",
        "<w:commentRangeEnd w:id=\"0\"/><w:r><w:rPr><w:rStyle w:val=\"CommentReference\"/></w:rPr><w:commentReference w:id=\"0\"/></w:r></w:p>",
    );
    t.comments = Some(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:comments xmlns:w=\"{W}\"><w:comment w:id=\"0\" w:author=\"我\" w:initials=\"我\"><w:p><w:r><w:t>原有批注</w:t></w:r></w:p></w:comment></w:comments>"
    ));
    t
}

fn counts(data: &[u8]) -> Vec<usize> {
    let x = entry(data, "word/document.xml");
    ["<w:fldChar ", "<w:instrText", "<m:oMath", "<w:ins ", "<w:del ", "<w:fldSimple ", "<w:commentRangeStart"]
        .iter()
        .map(|k| x.matches(k).count())
        .collect()
}

#[test]
fn fix_keeps_fields_math_revisions_comments() {
    let src = special().bytes();
    let out = fix(&src, PACK).unwrap();
    assert_eq!(counts(&src), counts(&out));
    assert_eq!((counts(&src)[0], counts(&src)[6]), (3, 1));
    assert_eq!(texts(&src), texts(&out));
    // 域/修订里的 run 没有被写入直接格式
    assert!(entry(&out, "word/document.xml").contains("<w:r><w:t>插入</w:t></w:r></w:ins>"));
}

#[test]
fn guard_error() {
    fn tamper(doc: &mut Doc, model: &Model, _: &pack::Pack, _: &[(Finding, Option<Fix>)], _: &mut fix::Parts) -> (usize, usize) {
        let r = model.paras[12].runs[0].el;
        let t = doc.create(r, "t");
        let tn = doc.create_text("被改");
        doc.append(t, tn);
        doc.append(r, t);
        (1, 0)
    }
    let err = fix_with(&T::build(true).bytes(), PACK, None, None, tamper).unwrap_err();
    assert!(err.contains("文字不一致"), "{err}");
}

#[test]
fn fix_selected_only_fixes_chosen() {
    let src = T::build(true).bytes();
    let before = check(&src, PACK).unwrap();
    let fixable: Vec<String> = before.findings.iter().filter(|f| f.fixable).map(|f| f.id.clone()).collect();
    assert!(fixable.len() > 1);
    let one = &fixable[..1];
    let out = fix_selected(&src, PACK, one, None).unwrap();
    let after = check(&out, PACK).unwrap();
    assert_eq!(after.summary.fixable, before.summary.fixable - 1);
    assert_eq!(texts(&src), texts(&out));
    // 选中全部可修复项 == 全部修复
    let all = fix_selected(&src, PACK, &fixable, None).unwrap();
    assert_eq!(entry(&all, "word/document.xml"), entry(&fix(&src, PACK).unwrap(), "word/document.xml"));
    assert!(check(&all, PACK).unwrap().findings.iter().all(|f| !f.fixable));
    // 空选择不改任何东西
    assert_eq!(check(&fix_selected(&src, PACK, &[], None).unwrap(), PACK).unwrap().summary.fixable, before.summary.fixable);
}

// ------------------------------------------------------------------ 批注
#[test]
fn annotate_one_comment_per_paragraph() {
    let src = special().bytes();
    let rep = check(&src, PACK).unwrap();
    let paras: std::collections::BTreeSet<_> =
        rep.findings.iter().filter(|f| f.severity != Severity::Manual).filter_map(|f| f.paragraph_index).collect();
    assert!(paras.iter().any(|i| rep.findings.iter().filter(|f| f.paragraph_index == Some(*i)).count() > 1)); // 样例里确有同段多条
    let out = annotate(&src, PACK).unwrap();
    let c = entry(&out, "word/comments.xml");
    assert_eq!(c.matches("<w:comment ").count(), paras.len() + 1); // +1 原有批注保留
    assert_eq!(c.matches("w:author=\"格式体检\"").count(), paras.len());
    assert_eq!(c.matches("w:author=\"我\"").count(), 1);
    assert!(c.contains("1. ") && c.contains("2. "));
    let d = entry(&out, "word/document.xml");
    assert_eq!(d.matches("<w:commentRangeStart").count(), paras.len() + 1);
    assert_eq!(d.matches("<w:commentReference").count(), paras.len() + 1);
    // 批注 id 不与原有批注冲突
    let ids: std::collections::BTreeSet<_> =
        regex::Regex::new(r#"<w:comment w:id="(\d+)""#).unwrap().captures_iter(&c).map(|m| m[1].to_string()).collect();
    assert_eq!(ids.len(), paras.len() + 1);
}

#[test]
fn annotate_creates_comments_part() {
    let out = annotate(&T::build(true).bytes(), PACK).unwrap();
    assert!(entry(&out, "word/comments.xml").contains("w:author=\"格式体检\""));
    assert!(entry(&out, "word/_rels/document.xml.rels").contains("Target=\"comments.xml\""));
    assert!(entry(&out, "[Content_Types].xml").contains("/word/comments.xml"));
    // 输出仍能被引擎重新解析
    assert!(check(&out, PACK).is_ok());
}

// ------------------------------------------------------------------ 表 / 图 / 表达式
const THREE: &str = "<w:tblBorders><w:top w:val=\"single\" w:sz=\"12\"/><w:bottom w:val=\"single\" w:sz=\"12\"/></w:tblBorders>";
const HEADER_LINE: &str = "<w:tcBorders><w:bottom w:val=\"single\" w:sz=\"8\"/></w:tcBorders>";
const TL_STYLE: &str = "<w:style w:type=\"table\" w:styleId=\"TL\"><w:name w:val=\"TL\"/><w:tblPr><w:tblBorders><w:top w:val=\"single\" w:sz=\"12\"/><w:bottom w:val=\"single\" w:sz=\"12\"/></w:tblBorders></w:tblPr><w:tblStylePr w:type=\"firstRow\"><w:tcPr><w:tcBorders><w:bottom w:val=\"single\" w:sz=\"8\"/></w:tcBorders></w:tcPr></w:tblStylePr></w:style>";

/// 3 行 2 列的表（单元格上下居中）；`props` 为 tblPr 内容，`head_tc` 为首行单元格 tcPr 内容，`sz` 为单元格字号（半磅）。
fn tbl(props: &str, head_tc: &str, sz: u32) -> String {
    let cell = |tc: &str, text: &str| {
        format!(
            "<w:tc><w:tcPr>{tc}<w:vAlign w:val=\"center\"/></w:tcPr><w:p><w:pPr><w:spacing w:before=\"60\" w:after=\"60\" w:line=\"240\" w:lineRule=\"auto\"/><w:jc w:val=\"center\"/></w:pPr><w:r><w:rPr><w:rFonts w:ascii=\"Times New Roman\" w:hAnsi=\"Times New Roman\" w:eastAsia=\"宋体\"/><w:sz w:val=\"{sz}\"/></w:rPr><w:t>{text}</w:t></w:r></w:p></w:tc>"
        )
    };
    let row = |tc: &str, a: &str, b: &str| format!("<w:tr>{}{}</w:tr>", cell(tc, a), cell(tc, b));
    format!(
        "<w:tbl><w:tblPr>{props}</w:tblPr><w:tblGrid><w:gridCol w:w=\"2000\"/><w:gridCol w:w=\"2000\"/></w:tblGrid>{}{}{}</w:tbl>",
        row(head_tc, "名称", "数值"),
        row("", "甲", "1"),
        row("", "乙", "2")
    )
}

/// 在第 1 章末（第 2 章标题之前）插入若干块。
fn with_blocks(blocks: Vec<String>) -> T {
    let mut t = T::build(false);
    t.paras.splice(17..17, blocks);
    t
}

fn table_rules(rep: &Report) -> Vec<&str> {
    rep.findings
        .iter()
        .map(|f| f.rule_id.as_str())
        .filter(|r| r.starts_with("table.") || r.starts_with("role.table_caption") || r.starts_with("figure."))
        .collect()
}

#[test]
fn three_line_table_with_caption_above_is_clean() {
    let direct = with_blocks(vec![add("tab", "表1-1 示例数据", Ov::default()), tbl(THREE, HEADER_LINE, 22)]);
    assert_eq!(table_rules(&chk(&direct)), Vec::<&str>::new());
    // 三线来自表格样式 + 首行条件格式
    let mut styled = with_blocks(vec![
        add("tab", "表1-1 示例数据", Ov::default()),
        tbl("<w:tblStyle w:val=\"TL\"/><w:tblLook w:val=\"04A0\" w:firstRow=\"1\"/>", "", 22),
    ]);
    styled.styles_extra = TL_STYLE.into();
    assert_eq!(table_rules(&chk(&styled)), Vec::<&str>::new());
    // 没启用 firstRow 条件格式 → 表头线不存在
    let mut no_look = with_blocks(vec![add("tab", "表1-1 示例数据", Ov::default()), tbl("<w:tblStyle w:val=\"TL\"/>", "", 22)]);
    no_look.styles_extra = TL_STYLE.into();
    let rep = chk(&no_look);
    let f = rules(&rep, "table.three_line");
    assert!(f.len() == 1 && f[0].message.contains("表头行下缺少横线") && f[0].fixable);
    // 一键修复后是合格的三线表，文字不变
    let out = fix_selected(&no_look.bytes(), PACK, &[f[0].id.clone()], None).unwrap();
    assert_eq!(table_rules(&check(&out, PACK).unwrap()), Vec::<&str>::new());
}

#[test]
fn full_grid_table_reports_vertical_lines_and_width() {
    let grid = "<w:tblBorders>".to_string()
        + &["top", "left", "bottom", "right", "insideH", "insideV"].map(|s| format!("<w:{s} w:val=\"single\" w:sz=\"4\"/>")).concat()
        + "</w:tblBorders>";
    let rep = chk(&with_blocks(vec![add("tab", "表1-1 示例数据", Ov::default()), tbl(&grid, "", 22)]));
    let f = rules(&rep, "table.three_line");
    assert!(f.len() == 1 && f[0].message.contains("左/右边竖线") && f[0].message.contains("内部竖线"), "{:?}", f.iter().map(|f| &f.message).collect::<Vec<_>>());
    assert_eq!(f[0].paragraph_index, Some(by_text(&rep, "表1-1")));
    assert_eq!(f[0].category, "table");
    let w = rules(&rep, "table.line_width");
    assert!(w.len() == 1 && w[0].message.contains("顶线线宽应为 1.5 磅，实际 0.5 磅"));
    // 修复：去掉全部竖线、顶底线 1.5 磅、表头线 1 磅；表内横线（辅助线）不动
    let t = with_blocks(vec![add("tab", "表1-1 示例数据", Ov::default()), tbl(&grid, "", 22)]);
    let out = fix_selected(&t.bytes(), PACK, &[f[0].id.clone(), w[0].id.clone()], None).unwrap();
    let after = check(&out, PACK).unwrap();
    assert!(rules(&after, "table.three_line").is_empty() && rules(&after, "table.line_width").is_empty(), "{:?}", table_rules(&after));
    assert_eq!(texts(&t.bytes()), texts(&out));
}

#[test]
fn table_caption_position() {
    let below = chk(&with_blocks(vec![tbl(THREE, HEADER_LINE, 22), add("tab", "表1-1 示例数据", Ov::default())]));
    let f = rules(&below, "table.caption");
    assert!(f.len() == 1 && f[0].actual["position"] == "表下方" && f[0].paragraph_index == Some(by_text(&below, "表1-1")));
    let missing = chk(&with_blocks(vec![tbl(THREE, HEADER_LINE, 22)]));
    let f = rules(&missing, "table.caption");
    assert!(f.len() == 1 && f[0].actual["position"] == "无表题" && f[0].location.snippet.contains("名称"));
    // 表题段落本身仍按 table_caption 角色检查格式
    let wrong = chk(&with_blocks(vec![add("tab", "表1-1 示例数据", Ov { size: Some(12.0), ..Ov::default() }), tbl(THREE, HEADER_LINE, 22)]));
    assert_eq!(rules(&wrong, "role.table_caption.font").len(), 1);
    // 表下的资料来源：五号、单倍行距、段前 6 段后 12
    let src = chk(&with_blocks(vec![add("tab", "表1-1 示例数据", Ov::default()), tbl(THREE, HEADER_LINE, 22), add("plain", "资料来源：国家统计局", Ov::default())]));
    let i = by_text(&src, "资料来源");
    assert_eq!(src.roles.iter().find(|r| r.text.starts_with("资料来源")).unwrap().role, Some("table_source"));
    let f = rules(&src, "role.table_source.font");
    assert!(f.len() == 1 && f[0].paragraph_index == Some(i) && f[0].message.contains("五号") && f[0].fixable);
}

#[test]
fn table_cell_text_is_one_finding_per_table() {
    let rep = chk(&with_blocks(vec![add("tab", "表1-1 示例数据", Ov::default()), tbl(THREE, HEADER_LINE, 24)]));
    let f = rules(&rep, "table.cell_format");
    assert!(f.len() == 1 && f[0].message.contains("6 个单元格") && f[0].message.contains("第 1 行第 1 列") && f[0].message.contains("字号应为 11pt"));
    let t = with_blocks(vec![add("tab", "表1-1 示例数据", Ov::default()), tbl(THREE, HEADER_LINE, 24)]);
    let out = fix_selected(&t.bytes(), PACK, &[f[0].id.clone()], None).unwrap();
    assert!(rules(&check(&out, PACK).unwrap(), "table.cell_format").is_empty());
    // 单元格没有上下居中（无 vAlign 即顶端对齐）也报，修复后写入 vAlign=center
    let top = with_blocks(vec![add("tab", "表1-1 示例数据", Ov::default()), tbl(THREE, HEADER_LINE, 22).replace("<w:vAlign w:val=\"center\"/>", "")]);
    let f = rules(&chk(&top), "table.cell_format").into_iter().map(|f| (f.id.clone(), f.message.clone())).collect::<Vec<_>>();
    assert!(f.len() == 1 && f[0].1.contains("应上下居中，实际顶端对齐"), "{f:?}");
    let out = fix_selected(&top.bytes(), PACK, &[f[0].0.clone()], None).unwrap();
    assert_eq!(entry(&out, "word/document.xml").matches("<w:vAlign w:val=\"center\"/>").count(), 6);
}

#[test]
fn layout_table_with_math_is_not_checked() {
    let math = "<w:p><m:oMathPara xmlns:m=\"http://schemas.openxmlformats.org/officeDocument/2006/math\"><m:oMath><m:r><m:t>x</m:t></m:r></m:oMath></m:oMathPara></w:p>";
    let layout = format!("<w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w=\"2000\"/></w:tblGrid><w:tr><w:tc>{math}</w:tc></w:tr></w:tbl>");
    assert_eq!(table_rules(&chk(&with_blocks(vec![layout]))), Vec::<&str>::new());
}

fn fig_para() -> String {
    "<w:p><w:r><w:drawing><wp:inline xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\"/></w:drawing></w:r></w:p>".into()
}

#[test]
fn figure_caption_position() {
    let ok = chk(&with_blocks(vec![fig_para(), add("fig", "图1-3 示例", Ov::default())]));
    assert!(rules(&ok, "figure.caption").is_empty());
    let above = chk(&with_blocks(vec![add("fig", "图1-3 示例", Ov::default()), fig_para()]));
    let f = rules(&above, "figure.caption");
    assert!(f.len() == 1 && f[0].actual["position"] == "图上方" && f[0].category == "figure");
    let none = chk(&with_blocks(vec![add("body", "前文。", Ov::default()), fig_para(), add("body", "后文。", Ov::default())]));
    assert_eq!(rules(&none, "figure.caption")[0].actual["position"], "无图题");
    let second = chk(&with_blocks(vec![fig_para(), add("fig", "图1-3 示例", Ov::default()), fig_para(), add("body", "后文。", Ov::default())]));
    let f = rules(&second, "figure.caption");
    assert!(f.len() == 1 && f[0].actual["position"] == "无图题");
    // 分图 (a)(b) 夹在图之间不算缺图题；浮动图不检查
    let sub = chk(&with_blocks(vec![
        fig_para(),
        add("plain", "(a) 左", Ov::default()),
        fig_para(),
        add("plain", "(b) 右", Ov::default()),
        add("fig", "图1-3 示例", Ov::default()),
    ]));
    assert!(rules(&sub, "figure.caption").is_empty());
    let float = "<w:p><w:r><w:drawing><wp:anchor xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\"/></w:drawing></w:r></w:p>".to_string();
    assert!(rules(&chk(&with_blocks(vec![float])), "figure.caption").is_empty());
}

#[test]
fn equation_number_format_and_sequence() {
    let eq = |no: &str| {
        format!("<w:p><m:oMathPara xmlns:m=\"http://schemas.openxmlformats.org/officeDocument/2006/math\"><m:oMath><m:r><m:t>E=mc</m:t></m:r></m:oMath></m:oMathPara>{no}</w:p>")
    };
    let num = |n: &str| format!("<w:r><w:tab/><w:t>{n}</w:t></w:r>");
    let rep = chk(&with_blocks(vec![eq(&num("（1-1）")), eq(&num("(1-2)")), eq(""), eq(&num("（2-3）")), eq(&num("（1.3）"))]));
    let msgs: Vec<_> = rules(&rep, "equation.number").iter().map(|f| f.message.as_str()).collect();
    assert_eq!(msgs.len(), 2, "{msgs:?}");
    assert!(msgs[0].contains("缺少序号"));
    assert!(msgs[1].contains("章号应为 1，实际 2"));
}

// ------------------------------------------------------------------ 精确检查（分页、目录）
fn lay(pages: &[i64]) -> Layout {
    Layout {
        name: "Microsoft Word".into(),
        version: "16.0".into(),
        pages: pages.iter().copied().max().unwrap_or(1),
        paras: pages.iter().enumerate().map(|(i, &p)| layout::ParaPos { i, p, e: None, d: p, f: 0 }).collect(),
        tables: vec![],
        tocs: vec![],
        seconds: 0.0,
    }
}

/// `T::build(false)` 每段所在页：封面 1、摘要 2、Abstract 3、目录 4、第 1 章起 5、参考文献 6、致谢 7。
/// 第 2 章（17）与上一段同在第 5 页。
const PAGES: [i64; 23] = [1, 1, 2, 2, 2, 3, 3, 3, 4, 4, 5, 5, 5, 5, 5, 5, 5, 5, 5, 6, 6, 7, 7];

fn lrep(t: &T, l: &Layout) -> Report {
    check_with(&t.bytes(), PACK, Some(l)).unwrap()
}

fn layout_rules(rep: &Report) -> Vec<&str> {
    rep.findings.iter().map(|f| f.rule_id.as_str()).filter(|r| r.starts_with("layout.")).collect()
}

#[test]
fn layout_json_parse() {
    let ok = r#"{"name":"Microsoft Word","version":"16.0","pages":3,"paras":[{"i":0,"p":1,"e":2,"d":1,"f":0}],
        "tables":[{"i":0,"s":1,"e":2}],"tocs":[{"code":" TOC \\o \"1-3\" ","entries":[
        {"text":"第1章 绪论","cached":"3","bm":"_Toc1","target":{"d":3,"f":0,"text":"绪论","list":"第1章"}},
        {"text":"无页码","cached":null,"bm":null,"target":null}]}]}"#;
    let l = Layout::parse(ok).unwrap();
    assert_eq!((l.pages, l.paras[0].e, l.tables[0].e), (3, Some(2), 2));
    assert_eq!(l.tocs[0].entries[0].target.as_ref().unwrap().list, "第1章");
    assert!(l.tocs[0].entries[1].bm.is_none() && l.tocs[0].entries[1].target.is_none());
    assert_eq!(Layout::parse(r#"{"error":"Word 打不开"}"#).err().unwrap(), "Word 打不开");
    assert!(Layout::parse("not json").is_err());
    assert!(Layout::parse(r#"{"name":"x"}"#).err().unwrap().contains("格式不符"));
}

#[test]
fn precise_findings_are_merged() {
    let t = T::build(false);
    assert!(chk(&t).findings.iter().all(|f| f.category != "layout"));
    let rep = lrep(&t, &lay(&PAGES));
    let info = serde_json::to_value(rep.precise.as_ref().unwrap()).unwrap();
    assert_eq!((info["engine"].as_str(), info["pages"].as_i64()), (Some("Microsoft Word 16"), Some(7)));
    let l: Vec<_> = rep.findings.iter().filter(|f| f.category == "layout").collect();
    assert!(!l.is_empty() && l.iter().all(|f| f.severity != Severity::Manual));
    assert_eq!(rep.summary.error, rep.findings.iter().filter(|f| f.severity == Severity::Error).count());
    assert!(chk(&t).precise.is_none());
}

#[test]
fn chapter_not_on_new_page() {
    let rep = lrep(&T::build(false), &lay(&PAGES));
    let f = rules(&rep, "layout.new_page");
    assert_eq!(f.len(), 1, "{:?}", layout_rules(&rep));
    assert_eq!(f[0].paragraph_index, Some(17));
    assert!(f[0].message.contains("第2章") && f[0].message.contains("第 5 页"));
    // 一键修复：带着同一份排版结果修，编号对得上；只给该段加「段前分页」，文字不变
    let l = lay(&PAGES);
    let out = fix_selected(&T::build(false).bytes(), PACK, &[f[0].id.clone()], Some(&l)).unwrap();
    let xml = entry(&out, "word/document.xml");
    assert_eq!(xml.matches("<w:pageBreakBefore/>").count(), 1);
    assert!(xml.find("<w:pageBreakBefore/>").unwrap() < xml.find("第2章").unwrap() && xml.find("第2章").unwrap() - xml.find("<w:pageBreakBefore/>").unwrap() < 600);
    // 第 2 章挪到下一页后不再报
    let mut pages = PAGES;
    pages[17..19].iter_mut().for_each(|p| *p = 6);
    pages[19..].iter_mut().for_each(|p| *p += 1);
    assert!(rules(&lrep(&T::build(false), &lay(&pages)), "layout.new_page").is_empty());
}

#[test]
fn first_chapter_must_start_on_right_page() {
    let t = T::build(false);
    assert!(rules(&lrep(&t, &lay(&PAGES)), "layout.chapter_odd_page").is_empty());
    let mut pages = PAGES;
    pages[10..].iter_mut().for_each(|p| *p += 1);
    let rep = lrep(&t, &lay(&pages));
    let f = rules(&rep, "layout.chapter_odd_page");
    assert!(f.len() == 1 && f[0].paragraph_index == Some(10) && f[0].message.contains("第 6 页"));
}

#[test]
fn ack_over_one_page() {
    let t = T::build(false);
    assert!(rules(&lrep(&t, &lay(&PAGES)), "layout.page_limit").is_empty());
    let mut l = lay(&PAGES);
    l.paras[22].e = Some(9); // 致谢最后一段延续到第 9 页
    let rep = lrep(&t, &l);
    let f = rules(&rep, "layout.page_limit");
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].paragraph_index, Some(21));
    assert!(f[0].message.contains("致谢") && f[0].message.contains("3 页（第 7–9 页）") && f[0].actual["pages"] == 3);
}

#[test]
fn body_first_page_number_must_be_one() {
    let t = T::build(false);
    let mut l = lay(&PAGES);
    // 前置部分大写罗马、从摘要页起为 I；正文从 1 重新编号
    for p in &mut l.paras {
        (p.f, p.d) = if p.i < 10 { (1, p.p - 1) } else { (0, p.p - 4) };
    }
    assert!(rules(&lrep(&t, &l), "layout.page_start").is_empty());
    // 前置部分没从 I 起，正文也没有重新编号
    for p in &mut l.paras {
        p.d += if p.i < 10 { 1 } else { 3 };
    }
    let rep = lrep(&t, &l);
    let f = rules(&rep, "layout.page_start");
    assert_eq!(f.len(), 2);
    assert_eq!(f[0].paragraph_index, Some(2));
    assert!(f[0].message.contains("摘要") && f[0].message.contains("II") && f[0].message.contains("应从 I"));
    assert!(f[1].paragraph_index == Some(10) && f[1].message.contains("实际页码为 4"));
}

fn spanning(header: bool) -> T {
    let mut table = tbl(THREE, HEADER_LINE, 22);
    if header {
        table = table.replacen("<w:tr>", "<w:tr><w:trPr><w:tblHeader/></w:trPr>", 1);
    }
    with_blocks(vec![add("tab", "表1-1 示例数据", Ov::default()), table])
}

#[test]
fn table_spanning_pages_needs_repeated_header() {
    let t = spanning(false);
    let n = chk(&t).summary.paragraphs;
    let mut l = lay(&vec![1; n]);
    l.tables.push(layout::TablePos { i: 0, s: 5, e: 6, r: vec![] });
    let rep = lrep(&t, &l);
    let caption = by_text(&rep, "表1-1");
    let split = rules(&rep, "layout.table_split");
    assert!(split.len() == 1 && split[0].paragraph_index == Some(caption) && split[0].message.contains("第 5–6 页"));
    assert!(split[0].severity == Severity::Warning);
    let hdr = rules(&rep, "layout.table_header");
    assert!(hdr.len() == 1 && hdr[0].severity == Severity::Error && hdr[0].paragraph_index == Some(caption));
    // 一键修复：首行写入 trPr/tblHeader（排在单元格之前）；再查不再报
    let out = fix_selected(&t.bytes(), PACK, &[hdr[0].id.clone()], Some(&l)).unwrap();
    assert!(entry(&out, "word/document.xml").contains("<w:tr><w:trPr><w:tblHeader/></w:trPr><w:tc>"));
    assert!(rules(&check_with(&out, PACK, Some(&l)).unwrap(), "layout.table_header").is_empty());
    // 首行设了重复标题行：只剩续表提示
    let rep = lrep(&spanning(true), &l);
    assert!(rules(&rep, "layout.table_header").is_empty() && rules(&rep, "layout.table_split").len() == 1);
    // 没跨页：都不报
    l.tables[0].e = 5;
    let rep = lrep(&t, &l);
    assert!(rules(&rep, "layout.table_split").is_empty() && rules(&rep, "layout.table_header").is_empty());
}

#[test]
fn spanning_table_is_split_into_continued_table_only_after_confirmation() {
    let t = spanning(false);
    let src = t.bytes();
    let n = chk(&t).summary.paragraphs;
    let mut l = lay(&vec![5; n]);
    // 第 3 行（乙）落到第 6 页
    l.tables.push(layout::TablePos { i: 0, s: 5, e: 6, r: vec![2] });
    let rep = check_with(&src, PACK, Some(&l)).unwrap();
    let f = rules(&rep, "layout.table_split")[0];
    let c = f.confirm.as_ref().expect("应提供续表拆分");
    assert!(!f.fixable && c.kind == "split_table" && c.text.is_none());
    assert!(c.summary.contains("第 3 行前") && c.summary.contains("“续表1-1 示例数据”") && c.summary.contains("名称 | 数值"), "{}", c.summary);
    let tables = |b: &[u8]| entry(b, "word/document.xml").matches("<w:tbl>").count();
    // 全部修复、按 id 选中都不拆
    assert_eq!(tables(&fix_selected(&src, PACK, std::slice::from_ref(&f.id), Some(&l)).unwrap()), 1);
    assert!(apply_confirmed(&src, PACK, &f.id, Some(&l), "别的说明").is_err());
    let out = apply_confirmed(&src, PACK, &f.id, Some(&l), &c.token).unwrap();
    let doc = entry(&out, "word/document.xml");
    assert_eq!(tables(&out), 2);
    // 原表：名称、甲；续表题注段前分页；续表：名称（表头副本）、乙
    let order: Vec<usize> = ["名称", "甲", "续表1-1 示例数据", "名称", "乙"]
        .iter()
        .scan(0, |at, s| {
            *at += doc[*at..].find(&format!(">{s}<")).unwrap() + 1;
            Some(*at)
        })
        .collect();
    assert!(order.windows(2).all(|w| w[0] < w[1]));
    let cap = para_xml(&doc, "续表1-1");
    assert!(cap.contains("<w:pageBreakBefore/>") && cap.contains("w:eastAsia=\"宋体\""), "{cap}");
    let after = check(&out, PACK).unwrap();
    assert!(rules(&after, "caption.continued").is_empty() && rules(&after, "table.continued_header").is_empty(), "{:?}", msgs(&rules(&after, "caption.continued")));
    // 跨页处就是表头之后第一行：拆了原表只剩表头，不提供
    l.tables[0].r = vec![1];
    let f = check_with(&src, PACK, Some(&l)).unwrap().findings.into_iter().find(|f| f.rule_id == "layout.table_split").unwrap();
    assert!(f.confirm.is_none() && f.message.contains("需手动修改"));
    // 跨页行接续上方的纵向合并单元格：不提供
    let mut tm = spanning(false);
    let at = tm.paras[18].rfind("<w:tr><w:tc><w:tcPr>").unwrap() + "<w:tr><w:tc><w:tcPr>".len();
    tm.paras[18].insert_str(at, "<w:vMerge/>");
    let merged = tm.bytes();
    l.tables[0].r = vec![2];
    let f = check_with(&merged, PACK, Some(&l)).unwrap().findings.into_iter().find(|f| f.rule_id == "layout.table_split").unwrap();
    assert!(f.confirm.is_none());
}

fn cap(text: &str) -> String {
    add(if text.contains('图') { "fig" } else { "tab" }, text, Ov::default())
}

fn table() -> String {
    tbl(THREE, HEADER_LINE, 22)
}

#[test]
fn continued_captions_must_match_original() {
    let msgs = |rep: &Report, id: &str| rules(rep, id).iter().map(|f| f.message.clone()).collect::<Vec<_>>();
    // 续表写不写表题都可以；表头重复了
    for cont in ["续表1-1 示例数据", "续表 1-1"] {
        let rep = chk(&with_blocks(vec![cap("表1-1 示例数据"), table(), cap(cont), table()]));
        assert!(msgs(&rep, "caption.continued").is_empty() && msgs(&rep, "table.continued_header").is_empty(), "{cont}");
        assert!(rules(&rep, "caption.number").is_empty() && rules(&rep, "table.caption").is_empty());
    }
    // 编号、表题都不一致，续表没有重复表头
    let no_header = table().replacen("名称", "丙", 1).replacen("数值", "3", 1);
    let rep = chk(&with_blocks(vec![cap("表1-1 示例数据"), table(), cap("续表1-2 其他数据"), no_header]));
    let f = rules(&rep, "caption.continued");
    assert!(f.len() == 1 && f[0].paragraph_index == Some(by_text(&rep, "续表")), "{:?}", msgs(&rep, "caption.continued"));
    assert!(f[0].message.contains("应与前面的表 1-1 相同") && f[0].message.contains("（“示例数据”），实际“其他数据”"), "{}", f[0].message);
    let h = msgs(&rep, "table.continued_header");
    assert!(h.len() == 1 && h[0].contains("丙 | 3") && h[0].contains("名称 | 数值"), "{h:?}");
    // 前面没有原表
    let rep = chk(&with_blocks(vec![cap("续表1-1 示例数据"), table()]));
    assert!(msgs(&rep, "caption.continued").iter().any(|m| m.contains("前面没有对应的表 1-1")));
    // 续图须注明图题
    let rep = chk(&with_blocks(vec![fig_para(), cap("图1-3 示例"), fig_para(), cap("续图1-3")]));
    let f = msgs(&rep, "caption.continued");
    assert!(f.len() == 1 && f[0].contains("次页应注明图题（“示例”）"), "{f:?}");
    let rep = chk(&with_blocks(vec![fig_para(), cap("图1-3 示例"), fig_para(), cap("续图1-3 示例")]));
    assert!(rules(&rep, "caption.continued").is_empty() && rules(&rep, "figure.caption").is_empty());
}

/// 各段页码：默认第 5 页，`at` 中列出的段落改为指定页。
fn pages_with(t: &T, at: &[(usize, i64)]) -> Layout {
    let mut pages = vec![5; chk(t).summary.paragraphs];
    at.iter().for_each(|&(i, p)| pages[i] = p);
    lay(&pages)
}

#[test]
fn figure_and_caption_must_share_a_page() {
    // 基础样例里已有图 1-1、图 1-2（无插图），新插入的图编为 1-3
    let t = with_blocks(vec![fig_para(), cap("图1-3 示例")]);
    let c = by_text(&chk(&t), "图1-3");
    let rep = lrep(&t, &pages_with(&t, &[(c, 6)]));
    let f = rules(&rep, "layout.figure_split");
    assert!(f.len() == 1 && f[0].paragraph_index == Some(c) && f[0].severity == Severity::Warning, "{:?}", layout_rules(&rep));
    assert!(f[0].message.contains("第 5–6 页") && f[0].message.contains("续图 1-3"), "{}", f[0].message);
    // 一键修复：从第一张图到图题前一段都设「与下段同页」，图题本身不设
    let l = pages_with(&t, &[(c, 6)]);
    let out = fix_selected(&t.bytes(), PACK, &[f[0].id.clone()], Some(&l)).unwrap();
    let xml = entry(&out, "word/document.xml");
    let (k, at) = (xml.matches("<w:keepNext/>").count(), xml.find("图1-3").unwrap());
    assert!(k == 1 && xml.find("<w:keepNext/>").unwrap() < at, "{k}");
    // 分图 (a)(b) 落在两页同样算跨页；同页不报
    let sub = |s: &str| add("plain", s, Ov::default());
    let t = with_blocks(vec![fig_para(), sub("(a) 左"), fig_para(), sub("(b) 右"), cap("图1-3 示例")]);
    let c = by_text(&chk(&t), "图1-3");
    assert_eq!(rules(&lrep(&t, &pages_with(&t, &[(c - 2, 6), (c - 1, 6), (c, 6)])), "layout.figure_split").len(), 1);
    assert!(rules(&lrep(&t, &pages_with(&t, &[])), "layout.figure_split").is_empty());
}

#[test]
fn continued_part_must_start_next_page() {
    // 续图：第二组图与原图同页 → 分页已移动
    let t = with_blocks(vec![fig_para(), cap("图1-3 示例"), fig_para(), cap("续图1-3 示例")]);
    let c = by_text(&chk(&t), "续图");
    let same = lrep(&t, &pages_with(&t, &[]));
    let f = rules(&same, "layout.continued_page");
    assert!(f.len() == 1 && f[0].paragraph_index == Some(c) && f[0].message.contains("同在第 5 页"), "{:?}", layout_rules(&same));
    let next = lrep(&t, &pages_with(&t, &[(c - 1, 6), (c, 6)]));
    assert!(rules(&next, "layout.continued_page").is_empty() && rules(&next, "layout.figure_split").is_empty());
    // 续表：上一部分表格结束页与续表题同页才报
    let t = with_blocks(vec![cap("表1-1 示例数据"), table(), cap("续表1-1 示例数据"), table()]);
    let c = by_text(&chk(&t), "续表");
    let mut l = pages_with(&t, &[]);
    l.tables = vec![layout::TablePos { i: 0, s: 5, e: 5, r: vec![] }, layout::TablePos { i: 1, s: 5, e: 5, r: vec![] }];
    assert_eq!(rules(&lrep(&t, &l), "layout.continued_page").len(), 1);
    let mut l = pages_with(&t, &[(c, 6)]);
    l.tables = vec![layout::TablePos { i: 0, s: 5, e: 5, r: vec![] }, layout::TablePos { i: 1, s: 6, e: 6, r: vec![] }];
    let rep = lrep(&t, &l);
    assert!(rules(&rep, "layout.continued_page").is_empty() && rules(&rep, "layout.table_split").is_empty());
}

fn te(text: &str, cached: &str, d: i64, title: &str, list: &str) -> layout::TocEntry {
    layout::TocEntry {
        text: text.into(),
        cached: Some(cached.into()),
        bm: Some("_Toc1".into()),
        target: Some(layout::Target { d, f: 0, text: title.into(), list: list.into() }),
    }
}

fn toc(entries: Vec<layout::TocEntry>) -> layout::Toc {
    layout::Toc { code: " TOC \\o \"1-3\" \\h ".into(), entries }
}

#[test]
fn stale_toc_pages_and_text() {
    let t = T::build(false);
    let mut l = lay(&PAGES);
    l.tocs.push(toc(vec![
        te("第1章 绪论", "5", 5, "绪论", "第1章"),
        te("第2章 方法", "6", 5, "方法", "第2章"), // 页码过期
        te("参考文献", "6", 6, "参考文献", ""),
        te("致谢", "6", 7, "致谢", ""), // 页码过期
        te("1.1 研究背景", "5", 5, "1.1 研究方法", ""), // 标题现文已改
    ]));
    let rep = lrep(&t, &l);
    let pages = rules(&rep, "layout.toc_pages");
    assert_eq!(pages.len(), 1, "{:?}", layout_rules(&rep));
    assert!(pages[0].message.contains("2 条") && pages[0].message.contains("目录为 6，实际为 5") && pages[0].message.contains("更新整个目录"));
    let text = rules(&rep, "layout.toc_text");
    assert!(text.len() == 1 && text[0].message.contains("1 条") && text[0].message.contains("1.1 研究方法"));
    assert_eq!(pages[0].paragraph_index, Some(8)); // 目录标题
    // 页码全部一致、条目文字一致：不报
    l.tocs[0] = toc(vec![te("第1章 绪论", "5", 5, "绪论", "第1章"), te("参考文献", "6", 6, "参考文献", "")]);
    assert!(layout_rules(&lrep(&t, &l)).iter().all(|r| !r.starts_with("layout.toc")));
    // 目标标题已被删除
    l.tocs[0].entries.push(layout::TocEntry { text: "没了".into(), cached: Some("9".into()), bm: Some("_Toc9".into()), target: None });
    assert_eq!(rules(&lrep(&t, &l), "layout.toc_text").len(), 1);
}

#[test]
fn roman_toc_pages_compare_by_style() {
    let t = T::build(false);
    let mut l = lay(&PAGES);
    let mut e = te("摘要", "Ⅱ", 2, "摘要", "");
    e.target.as_mut().unwrap().f = 1; // 大写罗马，Unicode 罗马数字缓存
    l.tocs.push(toc(vec![e]));
    assert!(rules(&lrep(&t, &l), "layout.toc_pages").is_empty());
    l.tocs[0].entries[0].cached = Some("Ⅲ".into());
    assert_eq!(rules(&lrep(&t, &l), "layout.toc_pages").len(), 1);
}

#[test]
fn heading_missing_from_toc() {
    let heading = |text: &str| format!("<w:p><w:pPr><w:pStyle w:val=\"Heading1\"/></w:pPr><w:r><w:t>{text}</w:t></w:r></w:p>");
    let mut t = with_blocks(vec![heading("第3章 新增"), heading("附加说明")]);
    t.styles_extra = "<w:style w:type=\"paragraph\" w:styleId=\"Heading1\"><w:name w:val=\"heading 1\"/><w:pPr><w:outlineLvl w:val=\"0\"/></w:pPr></w:style>".into();
    let n = chk(&t).summary.paragraphs;
    let mut l = lay(&vec![1; n]);
    l.tocs.push(toc(vec![te("附加说明", "1", 1, "附加说明", "")]));
    let rep = lrep(&t, &l);
    let f = rules(&rep, "layout.toc_missing");
    assert!(f.len() == 1 && f[0].message.contains("1 个标题") && f[0].message.contains("第3章 新增"), "{:?}", layout_rules(&rep));
    assert_eq!(f[0].paragraph_index, Some(by_text(&rep, "第3章 新增")));
    // 目录里有对应条目（含自动编号前缀）就不报
    l.tocs[0].entries.insert(0, te("第3章 新增", "1", 1, "新增", "第3章"));
    assert!(rules(&lrep(&t, &l), "layout.toc_missing").is_empty());
}

#[test]
fn hand_typed_toc_reported_once() {
    let rep = lrep(&T::build(false), &lay(&PAGES));
    let f = rules(&rep, "layout.toc_manual");
    assert!(f.len() == 1 && f[0].severity == Severity::Warning && f[0].paragraph_index == Some(8));
    assert!(layout_rules(&rep).iter().all(|r| !matches!(*r, "layout.toc_pages" | "layout.toc_text" | "layout.toc_missing")));
}

#[test]
fn manual_items_replaced_only_after_precise_check() {
    let t = T::build(false);
    let manual = |rep: &Report| rep.findings.iter().filter(|f| f.severity == Severity::Manual).map(|f| f.message.clone()).collect::<Vec<_>>();
    let (plain, precise) = (manual(&chk(&t)), manual(&lrep(&t, &lay(&PAGES))));
    let replaced = ["致谢限一页", "续表", "图页面积太大"];
    assert!(replaced.iter().all(|r| plain.iter().any(|m| m.contains(r))));
    assert!(precise.iter().all(|m| replaced.iter().all(|r| !m.contains(r))));
    assert!(precise.iter().any(|m| m.contains("避免用颜色")));
    assert_eq!(precise.len() + replaced.len(), plain.len());
}

#[test]
fn annotate_includes_precise_findings() {
    let t = T::build(false);
    let out = annotate_with(&t.bytes(), PACK, Some(&lay(&PAGES))).unwrap();
    assert!(entry(&out, "word/comments.xml").contains("应另起一页"));
    assert!(!entry(&annotate(&t.bytes(), PACK).unwrap(), "word/comments.xml").contains("应另起一页"));
}

#[test]
fn pack_layout_section_is_strict() {
    let base = include_str!("../../packs/thu-master.yaml");
    assert!(Pack::parse(&base.replace("    front_first: 1", "    front_first: 1\n    typo: 1")).is_err());
    assert!(Pack::parse(&base.replace("replaced_by: page_limits", "replaced_by: nothing")).err().unwrap().contains("replaced_by"));
    assert!(Pack::parse(&base.replace("[abstract_title_zh, abstract_title_en,", "[abstract_title_zh, bogus,")).err().unwrap().contains("bogus"));
}

// ------------------------------------------------------------------ 页眉
fn hrpr(sz: u32) -> String {
    format!("<w:rPr><w:rFonts w:ascii=\"Times New Roman\" w:hAnsi=\"Times New Roman\" w:eastAsia=\"宋体\"/><w:sz w:val=\"{sz}\"/></w:rPr>")
}

fn hpart(inner: &str) -> String {
    format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:hdr xmlns:w=\"{W}\">{inner}</w:hdr>")
}

/// 静态文字页眉。
fn hstatic(text: &str, jc: &str, sz: u32) -> String {
    hpart(&format!("<w:p><w:pPr><w:jc w:val=\"{jc}\"/></w:pPr><w:r>{}<w:t xml:space=\"preserve\">{text}</w:t></w:r></w:p>", hrpr(sz)))
}

/// STYLEREF 域页眉（fldChar 写法，带缓存结果）。
fn hfield(arg: &str, cached: &str) -> String {
    let run = |x: String| format!("<w:r>{}{x}</w:r>", hrpr(21));
    let runs = [
        run("<w:fldChar w:fldCharType=\"begin\"/>".into()),
        run(format!("<w:instrText xml:space=\"preserve\"> STYLEREF {arg} \\* MERGEFORMAT </w:instrText>")),
        run("<w:fldChar w:fldCharType=\"separate\"/>".into()),
        run(format!("<w:t>{cached}</w:t>")),
        run("<w:fldChar w:fldCharType=\"end\"/>".into()),
    ]
    .concat();
    hpart(&format!("<w:p><w:pPr><w:jc w:val=\"center\"/></w:pPr>{runs}</w:p>"))
}

const H1_STYLE: &str = "<w:style w:type=\"paragraph\" w:styleId=\"Heading1\"><w:name w:val=\"heading 1\"/><w:pPr><w:outlineLvl w:val=\"0\"/></w:pPr></w:style>";

impl T {
    /// 章标题段套用 Heading1 样式。
    fn chapter_styles(mut self) -> T {
        self.styles_extra.push_str(H1_STYLE);
        for p in &mut self.paras {
            if p.contains(">第1章") || p.contains(">第2章") {
                *p = p.replacen("<w:pPr>", "<w:pPr><w:pStyle w:val=\"Heading1\"/>", 1);
            }
        }
        self
    }

    /// 前置节（带 rId3 页脚的节）与最后一节挂默认页眉 rIdH{n}。
    fn use_headers(mut self, front: Option<usize>, body: Option<usize>) -> T {
        let put = |s: &str, n: usize| s.replacen("<w:sectPr>", &format!("<w:sectPr><w:headerReference w:type=\"default\" r:id=\"rIdH{n}\"/>"), 1);
        if let Some(n) = front {
            let p = self.paras.iter_mut().find(|p| p.contains("r:id=\"rId3\"")).unwrap();
            *p = put(p, n);
        }
        if let Some(n) = body {
            self.last_sect = put(&self.last_sect, n);
        }
        self
    }
}

fn hs(hdr: Option<usize>, fmt: &str) -> String {
    let s = sect(3.0, Some("rId3"), Some(fmt));
    match hdr {
        Some(n) => s.replacen("<w:sectPr>", &format!("<w:sectPr><w:headerReference w:type=\"default\" r:id=\"rIdH{n}\"/>"), 1),
        None => s,
    }
}

fn end_sect(s: String) -> String {
    format!("<w:p><w:pPr>{s}</w:pPr></w:p>")
}

/// 摘要 / 第1章 / 第2章 各一节；hdr 为各节的页眉序号（None = 不写 headerReference，向前继承）。
fn three(hdr: [Option<usize>; 3], headers: Vec<String>) -> T {
    let o = Ov::default;
    let paras = vec![
        add("abs_title", "摘  要", o()),
        add("body", "本文研究论文格式自动检查。", o()),
        end_sect(hs(hdr[0], "upperRoman")),
        add("chapter", "第1章 绪论", o()),
        add("body", "正文。", o()),
        end_sect(hs(hdr[1], "decimal")),
        add("chapter", "第2章 方法", o()),
        add("body", "正文。", o()),
    ];
    T { paras, last_sect: hs(hdr[2], "decimal"), styles_extra: String::new(), comments: None, headers, settings: None }.chapter_styles()
}

fn hdr_findings(rep: &Report) -> Vec<&Finding> {
    rep.findings.iter().filter(|f| f.rule_id.starts_with("headers.")).collect()
}

#[test]
fn headers_pack_replaces_manual_item() {
    let rep = chk(&T::build(false));
    assert!(rep.findings.iter().all(|f| !(f.severity == Severity::Manual && f.message.starts_with("页眉"))));
}

#[test]
fn headers_odd_even_toggle() {
    let mut t = T::build(false);
    assert!(rules(&chk(&t), "headers.odd_even").is_empty()); // 没有 settings.xml
    t.settings = Some("<w:evenAndOddHeaders/>".into());
    let rep = chk(&t);
    let f = rules(&rep, "headers.odd_even");
    assert!(f.len() == 1 && f[0].severity == Severity::Error && f[0].category == "page" && f[0].fixable);
    assert!(f[0].message.contains("奇偶页不同"));
    // 修复写回 settings.xml，正文不动
    let out = fix_selected(&t.bytes(), PACK, &[f[0].id.clone()], None).unwrap();
    assert!(!entry(&out, "word/settings.xml").contains("evenAndOddHeaders"));
    assert!(rules(&check(&out, PACK).unwrap(), "headers.odd_even").is_empty());
    t.settings = Some("<w:evenAndOddHeaders w:val=\"1\"/>".into());
    assert_eq!(rules(&chk(&t), "headers.odd_even").len(), 1);
    for off in ["0", "false"] {
        t.settings = Some(format!("<w:evenAndOddHeaders w:val=\"{off}\"/>"));
        assert!(rules(&chk(&t), "headers.odd_even").is_empty(), "val={off}");
    }
}

#[test]
fn headers_cover_skipped_abstract_missing() {
    // 封面无页眉不报；摘要所在节没有页眉要报；正文有 STYLEREF 页眉
    let t = T::build(false).chapter_styles();
    let t = T { headers: vec![hfield("\"标题 1\"", "绪论")], ..t }.use_headers(None, Some(1));
    let rep = chk(&t);
    let f = rules(&rep, "headers.missing");
    assert_eq!(f.len(), 1, "{:?}", hdr_findings(&rep).iter().map(|f| &f.message).collect::<Vec<_>>());
    assert_eq!(f[0].paragraph_index, Some(by_text(&rep, "摘")));
    assert!(f[0].message.contains("第2节") && f[0].severity == Severity::Error && f[0].source == "§2.4.2");
    assert_eq!(hdr_findings(&rep).len(), 1);
}

#[test]
fn headers_empty_part_counts_as_missing() {
    let t = three([Some(1), Some(1), Some(1)], vec![hpart("<w:p/>")]);
    let rep = chk(&t);
    assert_eq!(rules(&rep, "headers.missing").len(), 1); // 三节共用同一空页眉，只报一次
    assert_eq!(hdr_findings(&rep).len(), 1);
}

#[test]
fn headers_compliant_styleref_passes() {
    let t = T::build(false).chapter_styles();
    let t = T { headers: vec![hfield("\"标题 1\"", "摘要"), hfield("\"标题 1\"", "绪论")], ..t }.use_headers(Some(1), Some(2));
    let rep = chk(&t);
    let bad: Vec<_> = rep.findings.iter().filter(|f| matches!(f.severity, Severity::Error | Severity::Warning)).map(|f| &f.message).collect();
    assert!(bad.is_empty(), "{bad:?}");
}

#[test]
fn footer_page_number_format_is_fixed_in_footer_part() {
    // 两个页脚都改成居左、Arial 小四
    let src = rezip(&T::build(false).bytes(), |name, xml| {
        if name.contains("/footer") {
            xml.replace("w:val=\"center\"", "w:val=\"left\"").replace("Times New Roman", "Arial").replace("w:val=\"21\"", "w:val=\"24\"")
        } else {
            xml
        }
    });
    let rep = check(&src, PACK).unwrap();
    let f = rules(&rep, "page_numbers.footer_format");
    assert!(!f.is_empty() && f.iter().all(|f| f.fixable), "{:?}", f.iter().map(|f| &f.message).collect::<Vec<_>>());
    let ids: Vec<String> = f.iter().map(|f| f.id.clone()).collect();
    let out = fix_selected(&src, PACK, &ids, None).unwrap();
    for part in ["word/footer1.xml", "word/footer2.xml"] {
        let x = entry(&out, part);
        assert!(x.contains("w:val=\"center\"") && x.contains("Times New Roman") && x.contains("w:val=\"21\"") && x.contains("PAGE"), "{part}: {x}");
    }
    assert!(rules(&check(&out, PACK).unwrap(), "page_numbers.footer_format").is_empty());
}

/// 逐个部件改写后重新打包。
fn rezip(data: &[u8], f: impl Fn(&str, String) -> String) -> Vec<u8> {
    let mut z = ZipArchive::new(Cursor::new(data)).unwrap();
    let mut w = ZipWriter::new(Cursor::new(Vec::new()));
    for i in 0..z.len() {
        let mut file = z.by_index(i).unwrap();
        let (name, mut s) = (file.name().to_string(), String::new());
        file.read_to_string(&mut s).unwrap();
        w.start_file(name.as_str(), SimpleFileOptions::default()).unwrap();
        w.write_all(f(&name, s).as_bytes()).unwrap();
    }
    w.finish().unwrap().into_inner()
}

/// 给 docx 加一个部件（写入 word/ 下并在 document.xml.rels 里登记）。
fn with_part(data: &[u8], typ: &str, target: &str, xml: &str) -> Vec<u8> {
    let rel = format!("<Relationship Id=\"rIdP{typ}\" Type=\"{REL}/{typ}\" Target=\"{target}\"/>");
    let out = rezip(data, |name, s| {
        if name == "word/_rels/document.xml.rels" { s.replace("</Relationships>", &format!("{rel}</Relationships>")) } else { s }
    });
    let mut w = ZipWriter::new_append(Cursor::new(out)).unwrap();
    w.start_file(format!("word/{target}"), SimpleFileOptions::default()).unwrap();
    w.write_all(xml.as_bytes()).unwrap();
    w.finish().unwrap().into_inner()
}

// ------------------------------------------------------------------ 内容类检查
#[test]
fn abstract_content_rules() {
    let o = Ov::default;
    let mut t = T::build(false);
    t.paras[3] = add("body", "摘要太短。", o());
    t.paras[6] = add("body", "This thesis，studies format checking。", o());
    t.paras[7] = add("plain", "Key words: format; check", o());
    t.paras.insert(4, fig_para());
    let rep = chk(&t);
    let len = rules(&rep, "abstract.length");
    assert!(len.len() == 1 && len[0].actual["chars"] == 5, "{:?}", msgs(&len));
    assert_eq!(rules(&rep, "abstract.figure")[0].paragraph_index, Some(4));
    let punct = rules(&rep, "abstract.en_punct");
    assert!(punct.len() == 1 && punct[0].actual["punct"] == json!(["。", "，"]), "{:?}", msgs(&punct));
    let kw = rules(&rep, "keywords.match");
    assert!(kw.len() == 1 && kw[0].expected["count"] == 3 && kw[0].actual["count"] == 2);
}

#[test]
fn chapter_and_appendix_numbering() {
    let o = Ov::default;
    let mut t = T::build(false);
    t.paras[17] = add("chapter", "第二章 方法", o());
    t.paras.extend([
        add("chapter", "附录 B 数据", o()),
        cap("图B.1 数据分布"),
        cap("图1-1 错误编号"),
        add("chapter", "声  明", o()),
    ]);
    let rep = chk(&t);
    let ch = rules(&rep, "chapter.number");
    assert!(ch.len() == 1 && ch[0].paragraph_index == Some(17) && ch[0].expected["number"] == "第 2 章");
    // 第二章里的图 2-1 仍按第 2 章编号
    assert!(rules(&rep, "caption.number").iter().all(|f| f.paragraph_index != Some(18)));
    let ap = rules(&rep, "appendix.number");
    assert!(ap.len() == 1 && ap[0].paragraph_index == Some(23) && ap[0].message.contains("附录 A"), "{:?}", msgs(&ap));
    // 附录内的图冠以附录序号（按标题里的字母 B）
    let cn: Vec<_> = rules(&rep, "caption.number").into_iter().filter(|f| f.paragraph_index >= Some(23)).collect();
    assert!(cn.len() == 1 && cn[0].paragraph_index == Some(25) && cn[0].message.contains("附录序号应为 B，实际 1"), "{:?}", msgs(&cn));
    // 声明按章标题格式检查，fixture 的章标题格式合规
    assert_eq!(rep.roles.iter().find(|r| r.text.starts_with("声")).unwrap().role, Some("end_title"));
    assert!(rep.findings.iter().all(|f| !f.rule_id.starts_with("role.end_title")));
}

fn tc(f: &Finding) -> &edit::TextChange {
    f.confirm.as_ref().and_then(|c| c.text.as_ref()).unwrap_or_else(|| panic!("没有文字修改：{}", f.message))
}

fn text_fix<'a>(rep: &'a Report, id: &str) -> &'a edit::TextChange {
    let f = rules(rep, id);
    assert!(f.len() == 1 && !f[0].fixable, "{:?}", msgs(&f));
    tc(f[0])
}

#[test]
fn text_fixes_are_offered_but_never_applied_automatically() {
    let o = Ov::default;
    let mut t = T::build(true);
    t.paras[6] = add("body", "This thesis，studies（format）checking。", o());
    t.paras[17] = add("chapter", "第二章 方法", o());
    // 半角逗号与其后的空格分在两个 run 里
    t.paras[14] = t.paras[14].replace("他说,", "他说,</w:t></w:r><w:r><w:t xml:space=\"preserve\">  ");
    t.paras.extend([add("chapter", "附录 外文资料", o()), add("chapter", "附录 C 数据", o())]);
    let src = t.bytes();
    let rep = check(&src, PACK).unwrap();
    let after = |id: &str| text_fix(&rep, id).after.clone();
    assert_eq!(after("chapter.number"), "第2章 方法");
    assert_eq!(after("abstract.en_punct"), "This thesis, studies (format) checking.");
    assert_eq!(after("punct.halfwidth"), "他说，这样不行（见表1）。样例ＡＢＣ。");
    assert_eq!(after("punct.fullwidth_alnum"), "他说,  这样不行(见表1)。样例ABC。");
    // 中文关键词分隔符换成“；”，英文换成“; ”
    let kw: Vec<_> = rules(&rep, "keywords.separator").iter().map(|f| tc(f).after.clone()).collect();
    assert_eq!(kw, ["关键词：格式；检查；论文；规则；批注；修复", "Key words: format; check"]);
    let ap: Vec<_> = rules(&rep, "appendix.number").iter().map(|f| tc(f).after.clone()).collect();
    assert_eq!(ap, ["附录 A 外文资料", "附录 B 数据"]);
    assert!(rules(&rep, "appendix.number").iter().all(|f| f.message.contains("逐条确认")));
    let change = text_fix(&rep, "punct.halfwidth");
    assert_eq!(change.segs.iter().map(|s| (s.del.as_str(), s.ins.as_str())).collect::<Vec<_>>()[..3], [(",  ", "，"), ("(", "（"), (")", "）")]);

    // 全部修复、按 id 选中修复都不碰文字
    assert_eq!(texts(&src), texts(&fix(&src, PACK).unwrap()));
    let ids: Vec<String> = rep.findings.iter().filter(|f| f.confirm.is_some()).map(|f| f.id.clone()).collect();
    assert_eq!(texts(&src), texts(&fix_selected(&src, PACK, &ids, None).unwrap()));

    // 逐条确认：改后文字须与确认时一致；只改这一段
    let f = rules(&rep, "punct.halfwidth")[0];
    let want = &tc(f).after;
    assert!(apply_confirmed(&src, PACK, &f.id, None, "别的内容").is_err());
    let out = apply_confirmed(&src, PACK, &f.id, None, want).unwrap();
    let (a, b) = (edit::para_texts(&Doc::parse(entry(&src, "word/document.xml").as_bytes()).unwrap()), edit::para_texts(&Doc::parse(entry(&out, "word/document.xml").as_bytes()).unwrap()));
    let changed: Vec<_> = a.iter().zip(&b).filter(|(x, y)| x.1 != y.1).map(|(_, y)| y.1.as_str()).collect();
    assert_eq!(changed, [want.as_str()]);
    let doc = entry(&out, "word/document.xml");
    assert!(doc.contains(">他说，</w:t>") && doc.contains("<w:t xml:space=\"preserve\">这样不行（见表1）。样例ＡＢＣ。</w:t>"), "{}", para_xml(&doc, "他说"));
    let rep2 = check(&out, PACK).unwrap();
    assert!(rules(&rep2, "punct.halfwidth").is_empty() && rules(&rep2, "punct.fullwidth_alnum").len() == 1);
    // 用的是旧检查结果（文档已改）：同一 id 指向别的问题，或改后文字对不上，都拒绝
    assert!(apply_confirmed(&out, PACK, &f.id, None, want).is_err());
}

#[test]
fn common_template_variants_are_recognised() {
    // 模板库里常见的写法：中文摘要 / 目次、不带“第 章”的章标题、全文连续编号的图题、中英双语题注
    let mut t = T::build(false);
    let o = Ov::default;
    let outline0 = |s: String| s.replacen("</w:pPr>", "<w:outlineLvl w:val=\"0\"/></w:pPr>", 1);
    t.paras[2] = add("abs_title", "中文摘要", o());
    t.paras[8] = add("abs_title", "目  次", o());
    t.paras[10] = outline0(add("chapter", "1 绪论", o()));
    t.paras[16] = add("fig", "图2 处理流程", o());
    t.paras[17] = outline0(add("chapter", "2方法", o()));
    t.paras.insert(16, add("fig", "Figure 1-1 System framework", o()));
    let rep = chk(&t);
    let role = |text: &str| rep.roles.iter().find(|r| r.text == text).map(|r| (r.role, r.ambiguous)).unwrap();
    assert_eq!(role("中文摘要"), (Some("abstract_title_zh"), false));
    assert_eq!(role("目  次"), (Some("toc_title"), false));
    assert_eq!(role("1 绪论"), (Some("chapter"), false));
    assert_eq!(role("2方法"), (Some("chapter"), false));
    // 英文题注不检查，也不当正文
    assert_eq!(role("Figure 1-1 System framework"), (None, false));
    assert_eq!(role("图2 处理流程"), (Some("fig_caption"), false));
    let n = rules(&rep, "caption.number");
    assert!(n.len() == 1 && n[0].paragraph_index == Some(17) && n[0].expected["number"] == "1-2", "{:?}", msgs(&n));
    // 图 2-1 仍按第 2 章（章号取自“2方法”）编号，不误报
    assert!(rules(&rep, "figure.citation").iter().all(|f| f.paragraph_index != Some(19)), "{:?}", msgs(&rules(&rep, "figure.citation")));
}

#[test]
fn more_template_variants_are_recognised() {
    // 标题带冒号、关键词不用冒号、“一、”式章标题、分图说明的长题注、全文编号的续表、正文里的手打目录行
    let mut t = T::build(false);
    let o = Ov::default;
    t.paras[2] = add("abs_title", "摘  要：", o());
    t.paras[7] = add("plain", "Key words  format; check; thesis", o());
    t.paras[10] = add("chapter", "一、绪论", o()).replacen("</w:pPr>", "<w:outlineLvl w:val=\"0\"/></w:pPr>", 1);
    t.paras[18] = add("fig", "图2-1 示例：(a) 原图；(b) 处理结果", o());
    t.paras.insert(19, add("tab", "续表2 数据", o()));
    t.paras.insert(14, add("plain", "1.2 研究现状……………………3", o()));
    let rep = chk(&t);
    let role = |text: &str| rep.roles.iter().find(|r| r.text == text).map(|r| (r.role, r.ambiguous)).unwrap();
    assert_eq!(role("摘  要："), (Some("abstract_title_zh"), false));
    assert_eq!(role("Key words  format; check; thesis"), (Some("keywords_en"), false));
    assert_eq!(role("一、绪论"), (Some("chapter"), false));
    assert_eq!(role("图2-1 示例：(a) 原图；(b) 处理结果"), (Some("fig_caption"), false));
    assert_eq!(role("续表2 数据"), (Some("table_caption"), false));
    assert_eq!(role("1.2 研究现状……………………3"), (Some("toc_item"), false));
    // 不带冒号的关键词照样数个数、查分隔符；“一、绪论”算第 1 章，图 1-x 不报章号不符
    for id in ["keywords.count", "keywords.separator", "keywords.match"] {
        assert!(rules(&rep, id).is_empty(), "{id}: {:?}", msgs(&rules(&rep, id)));
    }
    assert!(rules(&rep, "caption.number").iter().all(|f| !f.message.contains("章号")), "{:?}", msgs(&rules(&rep, "caption.number")));
}

#[test]
fn toc_leaders_appendix_intros_and_unstyled_chapters() {
    // 武汉纺织大学样本：目录里的“附录……67”、正文里的“附录的内容包括：”、无大纲级别的后续章“2 方法”
    let mut t = T::build(false);
    let o = Ov::default;
    t.paras[10] = add("chapter", "1 绪论", o()).replacen("</w:pPr>", "<w:outlineLvl w:val=\"0\"/></w:pPr>", 1);
    t.paras[17] = add("plain", "2 方法", o());
    t.paras.insert(19, add("body", "附录的内容包括：", o()));
    // 只有序号的附录标题带冒号仍是标题（首都师大样例“附录一：（宋体三号加粗）”）
    t.paras.push(add("chapter", "附录一：（宋体三号加粗）", o()).replacen("</w:pPr>", "<w:outlineLvl w:val=\"2\"/></w:pPr>", 1));
    // 双语题注的英文行、图表下的注与数据来源
    t.paras.insert(19, add("plain", "Tab. 2-1 Results of the test", o()));
    t.paras.insert(19, add("plain", "注：数据为三次测量的均值。", o()));
    t.paras.insert(19, add("plain", "数据来源：国家统计局", o()));
    t.paras.insert(9, add("plain", "附录……………………………………67", o()));
    let rep = chk(&t);
    let role = |text: &str| rep.roles.iter().find(|r| r.text == text).map(|r| (r.role, r.ambiguous)).unwrap();
    assert_eq!(role("附录……………………………………67").0.map(|r| r.starts_with("toc_")), Some(true));
    assert_eq!(role("2 方法"), (Some("chapter"), false));
    assert_ne!(role("附录的内容包括：").0, Some("appendix_title"));
    assert_eq!(role("附录一：（宋体三号加粗）"), (Some("appendix_title"), false));
    assert_eq!(role("Tab. 2-1 Results of the test"), (None, false));
    assert_eq!(role("注：数据为三次测量的均值。").0, Some("table_source"));
    assert_eq!(role("数据来源：国家统计局").0, Some("table_source"));
}

#[test]
fn adjacent_separators_merge_into_one() {
    // 南开大学 2026 版模板里的写法：两个分隔符之间只隔空白（空关键词），曾导致修改区间重叠而崩溃
    let mut t = T::build(false);
    t.paras[4] = add("plain", "关键词：格式， 、检查 ，论文；；规范", Ov::default());
    assert_eq!(text_fix(&chk(&t), "keywords.separator").after, "关键词：格式；检查；论文；规范");
}

#[test]
fn text_in_revisions_or_fields_is_not_offered() {
    let mut t = T::build(true);
    // 用错的分隔符在修订里；章号在域结果里
    t.paras[4] = t.paras[4].replacen("、", "</w:t></w:r><w:ins w:id=\"9\" w:author=\"a\"><w:r><w:t>、</w:t></w:r></w:ins><w:r><w:t>", 1);
    t.paras[17] = add("chapter", "第二章 方法", Ov::default()).replace(
        "<w:t xml:space=\"preserve\">第二章 方法</w:t></w:r>",
        "<w:fldChar w:fldCharType=\"begin\"/></w:r><w:r><w:instrText> QUOTE </w:instrText></w:r><w:r><w:fldChar w:fldCharType=\"separate\"/></w:r><w:r><w:t>第二章</w:t></w:r><w:r><w:fldChar w:fldCharType=\"end\"/></w:r><w:r><w:t xml:space=\"preserve\"> 方法</w:t></w:r>",
    );
    let rep = chk(&t);
    for id in ["keywords.separator", "chapter.number"] {
        let f = rules(&rep, id);
        assert!(!f.is_empty() && f[0].confirm.is_none() && f[0].message.contains("需手动修改"), "{id}: {:?}", msgs(&f));
    }
}

#[test]
fn figures_follow_first_citation() {
    let t = with_blocks(vec![
        cap("图1-3 甲"),
        add("body", "结果见图1-3～1-4。", Ov::default()),
        cap("图1-4 乙"),
        cap("图1-5 丙"),
    ]);
    let rep = chk(&t);
    let f = rules(&rep, "figure.citation");
    let at: Vec<_> = f.iter().map(|f| (f.paragraph_index, f.message.contains("之前"))).collect();
    // 1-3 在引用之前；1-4 由“～”范围引用；1-5 未被引用
    assert_eq!(at, [(Some(17), true), (Some(20), false)], "{:?}", msgs(&f));
}

#[test]
fn references_match_citations() {
    let o = Ov::default;
    let mut t = T::build(false);
    t.paras[12] = add("body", "已有研究[2]，另见[1]与[5]，区间[0, 1]不算。", o());
    t.paras.insert(21, add("ref", "[2] 李四. 方法[M]. 北京: 出版社, 2021.", o()));
    let rep = chk(&t);
    let order = rules(&rep, "citation.order");
    assert!(order.len() == 1 && order[0].actual["number"] == 2 && order[0].expected["number"] == 1);
    assert_eq!(rules(&rep, "citation.missing")[0].actual["numbers"], json!([5]));
    assert!(rules(&rep, "citation.uncited").is_empty() && rules(&rep, "citation.list").is_empty());
    // 表中跳号、且有一条未被引用
    t.paras[21] = add("ref", "[3] 李四. 方法[M]. 北京: 出版社, 2021.", o());
    t.paras[12] = add("body", "已有研究[1]。", o());
    let rep = chk(&t);
    assert_eq!(rules(&rep, "citation.list")[0].paragraph_index, Some(21));
    assert_eq!(rules(&rep, "citation.uncited")[0].actual["numbers"], json!([3]));
}

#[test]
fn equation_layout_must_be_consistent() {
    let eq = |jc: &str, n: &str| {
        format!("<w:p><m:oMathPara xmlns:m=\"http://schemas.openxmlformats.org/officeDocument/2006/math\">{jc}<m:oMath><m:r><m:t>E=mc</m:t></m:r></m:oMath></m:oMathPara><w:r><w:tab/><w:t>{n}</w:t></w:r></w:p>")
    };
    let left = "<m:oMathParaPr><m:jc m:val=\"left\"/></m:oMathParaPr>";
    let rep = chk(&with_blocks(vec![eq("", "（1-1）"), eq(left, "（1-2）"), eq("", "（1-3）")]));
    let f = rules(&rep, "equation.layout");
    assert!(f.len() == 1 && f[0].paragraph_index == Some(18) && f[0].actual["indented"] == 1, "{:?}", msgs(&f));
    assert!(rules(&chk(&with_blocks(vec![eq("", "（1-1）"), eq("", "（1-2）")])), "equation.layout").is_empty());
}

#[test]
fn reference_entry_split_is_fixed_with_keep_lines() {
    let t = T::build(false);
    let mut l = lay(&PAGES);
    l.paras[20].e = Some(7);
    let rep = lrep(&t, &l);
    let f = rules(&rep, "layout.ref_split");
    assert!(f.len() == 1 && f[0].paragraph_index == Some(20) && f[0].fixable, "{:?}", msgs(&f));
    let out = fix_selected(&t.bytes(), PACK, &[f[0].id.clone()], Some(&l)).unwrap();
    let xml = entry(&out, "word/document.xml");
    assert!(xml.matches("<w:keepLines/>").count() == 1 && xml.find("<w:keepLines/>") < xml.find("张三"));
}

#[test]
fn footnotes_format_and_numbering_are_fixed() {
    let mut t = T::build(false);
    t.paras[14] = t.paras[14].replacen("</w:p>", "<w:r><w:footnoteReference w:id=\"1\"/></w:r></w:p>", 1);
    // 只写在 settings 里的编号方式 Word 不认（Word 16 实测），仍应报告
    t.settings = Some("<w:footnotePr><w:numFmt w:val=\"decimalEnclosedCircle\"/><w:numRestart w:val=\"eachPage\"/></w:footnotePr>".into());
    let notes = format!(
        "<w:footnotes xmlns:w=\"{W}\"><w:footnote w:type=\"separator\" w:id=\"-1\"><w:p><w:r><w:separator/></w:r></w:p></w:footnote>\
         <w:footnote w:id=\"1\"><w:p><w:r><w:rPr><w:rFonts w:ascii=\"Times New Roman\" w:hAnsi=\"Times New Roman\" w:eastAsia=\"宋体\"/><w:sz w:val=\"21\"/></w:rPr><w:t>注释 Note</w:t></w:r></w:p></w:footnote></w:footnotes>"
    );
    let src = with_part(&t.bytes(), "footnotes", "footnotes.xml", &notes);
    let rep = check(&src, PACK).unwrap();
    let fmt = rules(&rep, "footnote.format");
    assert!(fmt.len() == 1 && fmt[0].paragraph_index == Some(14) && fmt[0].message.contains("小五"), "{:?}", msgs(&fmt));
    assert!(fmt[0].message.contains("悬挂缩进应为 1.5 字符"));
    let num = rules(&rep, "footnote.numbering");
    assert!(num.len() == 1 && num[0].message.contains("带圈") && num[0].message.contains("每页"), "{:?}", msgs(&num));
    let ids = [fmt[0].id.clone(), num[0].id.clone()];
    let out = fix_selected(&src, PACK, &ids, None).unwrap();
    let fx = entry(&out, "word/footnotes.xml");
    assert!(fx.contains("<w:sz w:val=\"18\"/>") && fx.contains("w:hangingChars=\"150\"") && fx.contains("注释 Note"), "{fx}");
    let pr = "<w:footnotePr><w:numFmt w:val=\"decimalEnclosedCircle\"/><w:numRestart w:val=\"eachPage\"/></w:footnotePr>";
    assert_eq!(entry(&out, "word/document.xml").matches(pr).count(), 3); // 每一节
    let after = check(&out, PACK).unwrap();
    assert!(after.findings.iter().all(|f| !f.rule_id.starts_with("footnote.")));
}

#[test]
fn reference_hanging_indent_is_fixed() {
    let mut t = T::build(false);
    t.paras[20] = add("ref", "[1] 张三. 论文写作[M]. 北京: 出版社, 2020.", Ov { first: Some(0.0), ..Ov::default() });
    let src = t.bytes();
    let rep = chk(&t);
    let f = rules(&rep, "role.ref_item.indent");
    assert!(f.len() == 1 && f[0].fixable, "{:?}", msgs(&f));
    let out = fix_selected(&src, PACK, &[f[0].id.clone()], None).unwrap();
    // 五号 2 字符 = 420 twip；首行缩进被清掉
    let xml = entry(&out, "word/document.xml");
    assert!(xml.contains("<w:ind w:hangingChars=\"200\" w:hanging=\"420\"/>"), "{xml}");
    assert!(rules(&check(&out, PACK).unwrap(), "role.ref_item.indent").is_empty());
}

#[test]
fn fields_in_a_single_run_are_recognised() {
    // begin / instrText / separate / end 全放在同一个 run 里也是合法写法
    let t = T::build(false).chapter_styles();
    let t = T { headers: vec![hfield("\"标题 1\"", "摘要"), hfield("\"标题 1\"", "绪论")], ..t }.use_headers(Some(1), Some(2));
    let one_run = |xml: String| {
        let rpr = xml[xml.find("<w:rPr>").unwrap()..xml.find("</w:rPr>").unwrap() + "</w:rPr>".len()].to_string();
        xml.replace(&format!("</w:r><w:r>{rpr}"), "")
    };
    let src = rezip(&t.bytes(), |name, xml| if name.contains("/footer") || name.contains("/header") { one_run(xml) } else { xml });
    assert!(!entry(&src, "word/footer1.xml").contains("</w:r><w:r>") && !entry(&src, "word/header1.xml").contains("</w:r><w:r>"));
    let rep = check(&src, PACK).unwrap();
    let bad: Vec<_> = rep.findings.iter().filter(|f| matches!(f.severity, Severity::Error | Severity::Warning)).map(|f| &f.message).collect();
    assert!(bad.is_empty(), "{bad:?}");
}

#[test]
fn headers_static_matches_chapter_title() {
    // 空白差异、省略「第N章」前缀都算相同
    let t = three([Some(1), Some(2), Some(3)], vec![hstatic("摘 要", "center", 21), hstatic("绪论", "center", 21), hstatic("第2章　方法", "center", 21)]);
    let rep = chk(&t);
    assert!(hdr_findings(&rep).is_empty(), "{:?}", hdr_findings(&rep).iter().map(|f| &f.message).collect::<Vec<_>>());
}

#[test]
fn headers_static_wrong_text() {
    let t = three([Some(1), Some(2), Some(3)], vec![hstatic("摘要", "center", 21), hstatic("绪论", "center", 21), hstatic("绪论", "center", 21)]);
    let rep = chk(&t);
    let f = rules(&rep, "headers.content");
    assert_eq!(f.len(), 1);
    assert!(f[0].severity == Severity::Error && f[0].message.contains("第3节"));
    assert_eq!(f[0].expected["text"], "第2章 方法");
    assert_eq!(f[0].actual["text"], "绪论");
    assert_eq!(hdr_findings(&rep).len(), 1);
}

#[test]
fn headers_inherited_from_previous_chapter() {
    // 第 3 节新起一章，却沿用（链接到前一节）第 2 节的页眉
    let t = three([Some(1), Some(2), None], vec![hstatic("摘要", "center", 21), hstatic("绪论", "center", 21)]);
    let rep = chk(&t);
    let f = rules(&rep, "headers.content");
    assert_eq!(f.len(), 1);
    assert!(f[0].message.contains("第3节") && f[0].message.contains("链接到前一节") && f[0].message.contains("第2节"), "{}", f[0].message);
    assert_eq!(f[0].expected["text"], "第2章 方法");
}

#[test]
fn headers_styleref_to_chapter_style_ok() {
    for arg in ["\"标题 1\"", "\"heading 1\"", "Heading1", "1 \\s"] {
        let t = three([Some(1), None, None], vec![hfield(arg, "绪论")]);
        let rep = chk(&t);
        assert!(hdr_findings(&rep).is_empty(), "{arg}: {:?}", hdr_findings(&rep).iter().map(|f| &f.message).collect::<Vec<_>>());
    }
}

#[test]
fn headers_styleref_to_unused_style_errors() {
    for arg in ["\"标题 2\"", "NoSuchStyle", "2"] {
        let t = three([Some(1), None, None], vec![hfield(arg, "绪论")]);
        let rep = chk(&t);
        let f = rules(&rep, "headers.content");
        assert_eq!(f.len(), 1, "{arg}"); // 三节共用同一页眉，只报一次
        assert!(f[0].severity == Severity::Error && f[0].message.contains("STYLEREF"), "{}", f[0].message);
    }
}

#[test]
fn headers_format_reports_all_mismatches_once() {
    // 左对齐 + 小四；三节共用同一页眉部件，只报一次
    let t = three([Some(1), None, None], vec![hstatic("摘要", "left", 24)]);
    let rep = chk(&t);
    let f = rules(&rep, "headers.format");
    assert_eq!(f.len(), 1);
    assert!(f[0].severity == Severity::Error && f[0].fixable);
    assert!(f[0].message.contains("居中") && f[0].message.contains("居左") && f[0].message.contains("五号") && f[0].message.contains("小四"), "{}", f[0].message);
    assert_eq!(f[0].expected["align"], "center");
    assert_eq!(f[0].actual["size"], 12.0);
    // 修复写回页眉部件：对齐、字号改对，文字不变；修复后不再报
    let out = fix_selected(&t.bytes(), PACK, &[f[0].id.clone()], None).unwrap();
    let hx = entry(&out, "word/header1.xml");
    assert!(hx.contains("<w:jc w:val=\"center\"/>") && hx.contains("<w:sz w:val=\"21\"/>") && hx.contains("摘要"), "{hx}");
    assert!(rules(&check(&out, PACK).unwrap(), "headers.format").is_empty());
}

#[test]
fn headers_format_checks_fonts() {
    let bad = hstatic("摘要", "center", 21).replace("w:eastAsia=\"宋体\"", "w:eastAsia=\"黑体\"");
    let rep = chk(&three([Some(1), None, None], vec![bad]));
    let f = rules(&rep, "headers.format");
    assert_eq!(f.len(), 1);
    assert!(f[0].message.contains("中文字体应为 宋体") && f[0].message.contains("黑体"), "{}", f[0].message);
}

#[test]
fn headers_static_with_two_chapters_warns() {
    let o = Ov::default;
    let paras = vec![
        add("abs_title", "摘  要", o()),
        end_sect(hs(Some(1), "upperRoman")),
        add("chapter", "第1章 绪论", o()),
        add("body", "正文。", o()),
        add("chapter", "第2章 方法", o()),
        add("body", "正文。", o()),
    ];
    let t = T { paras, last_sect: hs(Some(2), "decimal"), styles_extra: String::new(), comments: None, headers: vec![hstatic("摘要", "center", 21), hstatic("绪论", "center", 21)], settings: None };
    let rep = chk(&t);
    let f = rules(&rep, "headers.content");
    assert_eq!(f.len(), 1);
    assert!(f[0].severity == Severity::Warning && f[0].message.contains("多个章") && f[0].message.contains("第2节"), "{}", f[0].message);
}

// ------------------------------------------------------------------ 页码分节与重新编号
fn pn_findings(rep: &Report) -> Vec<(&str, &str)> {
    rep.findings.iter().filter(|f| f.rule_id.starts_with("page_numbers.")).map(|f| (f.rule_id.as_str(), f.message.as_str())).collect()
}

/// 修复全部问题，返回修复后的字节与重新检查的报告。
fn fixed(t: &T) -> (Vec<u8>, Report) {
    let src = t.bytes();
    let (out, _, skipped) = fix_with_stats(&src, PACK).unwrap();
    assert_eq!(skipped, 0);
    assert_eq!(texts(&src), texts(&out));
    let rep = check(&out, PACK).unwrap();
    (out, rep)
}

/// document.xml 里各 sectPr 的原文，按出现顺序。
fn sect_prs(out: &[u8]) -> Vec<String> {
    let doc = entry(out, "word/document.xml");
    doc.split("<w:sectPr").skip(1).map(|s| s.split("</w:sectPr>").next().unwrap().to_string()).collect()
}

#[test]
fn page_numbers_must_restart_per_part() {
    let mut t = T::build(false);
    t.paras.iter_mut().for_each(|p| *p = p.replace("w:start=\"1\" ", ""));
    t.last_sect = t.last_sect.replace("w:start=\"1\" ", "");
    let rep = chk(&t);
    let f = rules(&rep, "page_numbers.restart");
    assert_eq!(f.len(), 2, "{:?}", pn_findings(&rep));
    assert!(f.iter().all(|f| f.fixable && f.message.contains("应重新从 1 开始")));
    assert!(f[0].message.contains("前置部分") && f[1].message.contains("正文部分"));
    let (out, rep) = fixed(&t);
    assert!(pn_findings(&rep).is_empty(), "{:?}", pn_findings(&rep));
    let s = sect_prs(&out);
    assert!(s[1].contains("w:start=\"1\"") && s[2].contains("w:start=\"1\""));
}

#[test]
fn later_section_must_not_restart() {
    // 第1章自成一节并从 1 起；第2章起的最后一节又写了 start，页码会再次从头开始
    let mut t = T::build(false);
    let at = t.paras.iter().position(|p| p.contains(">第2章")).unwrap();
    let mid = sect(3.0, None, Some("decimal")).replacen("<w:pgNumType ", "<w:pgNumType w:start=\"1\" ", 1);
    t.paras.insert(at, format!("<w:p><w:pPr>{mid}</w:pPr></w:p>"));
    let rep = chk(&t);
    let f = rules(&rep, "page_numbers.continue");
    assert!(f.len() == 1 && f[0].fixable && f[0].message.contains("第4节"), "{:?}", pn_findings(&rep));
    assert!(rules(&rep, "page_numbers.restart").is_empty());
    let (out, rep) = fixed(&t);
    assert!(pn_findings(&rep).is_empty(), "{:?}", pn_findings(&rep));
    let s = sect_prs(&out);
    assert!(s[2].contains("w:start=\"1\"") && !s[3].contains("w:start"));
}

#[test]
fn cover_footer_with_page_number_moves_to_abstract() {
    let mut t = T::build(false);
    t.paras[1] = format!("<w:p><w:pPr>{}</w:pPr></w:p>", sect(3.0, Some("rId3"), None));
    let front = t.paras.iter_mut().find(|p| p.contains("r:id=\"rId3\"") && p.contains("upperRoman")).unwrap();
    *front = front.replace("<w:footerReference w:type=\"default\" r:id=\"rId3\"/>", ""); // 摘要节继承封面的页脚
    let rep = chk(&t);
    let f = rules(&rep, "page_numbers.cover");
    assert!(f.len() == 1 && f[0].fixable && f[0].message.contains("第1节"), "{:?}", pn_findings(&rep));
    let (out, rep) = fixed(&t);
    assert!(pn_findings(&rep).is_empty(), "{:?}", pn_findings(&rep));
    let s = sect_prs(&out);
    assert!(!s[0].contains("footerReference") && s[1].contains("r:id=\"rId3\""));
    // 封面开了「首页不同」且没有首页页脚：首页不显示页码，不报
    t.paras[1] = t.paras[1].replace("<w:cols ", "<w:titlePg/><w:cols ");
    assert!(rules(&chk(&t), "page_numbers.cover").is_empty());
}

/// 封面、摘要、正文全在一节，封面末尾还有手动分页符。
fn one_section() -> T {
    let mut t = T::build(false);
    t.paras.retain(|p| !p.contains("<w:sectPr>"));
    t.paras.insert(1, "<w:p><w:r><w:br w:type=\"page\"/></w:r></w:p>".into());
    t.last_sect = sect(3.0, Some("rId4"), None);
    t
}

#[test]
fn single_section_thesis_is_split_into_cover_front_body() {
    let t = one_section();
    let rep = chk(&t);
    let ids: Vec<_> = pn_findings(&rep).into_iter().map(|(id, _)| id).collect();
    assert!(ids.contains(&"page_numbers.cover_shared") && ids.contains(&"page_numbers.mixed_section"), "{ids:?}");
    assert!(rules(&rep, "page_numbers.cover_shared")[0].paragraph_index == Some(by_text(&rep, "摘")));
    assert!(rep.findings.iter().filter(|f| f.rule_id.starts_with("page_numbers.")).all(|f| f.fixable));
    let (out, rep) = fixed(&t);
    assert!(pn_findings(&rep).is_empty(), "{:?}", pn_findings(&rep));
    let s = sect_prs(&out);
    assert_eq!(s.len(), 3);
    // 封面：无页脚、不编页码；前置：罗马从 1 起；正文：阿拉伯从 1 起，两者沿用原页脚
    assert!(!s[0].contains("footerReference") && !s[0].contains("pgNumType"));
    assert!(s[1].contains("rId4") && s[1].contains("w:fmt=\"upperRoman\"") && s[1].contains("w:start=\"1\""));
    assert!(s[2].contains("rId4") && s[2].contains("w:fmt=\"decimal\"") && s[2].contains("w:start=\"1\""));
    // 分节符本身换页，封面末尾的手动分页符被去掉，否则多出一张空白页
    assert!(!entry(&out, "word/document.xml").contains("w:type=\"page\""));
    // 修复结果可再次修复而不变
    assert_eq!(fix_with_stats(&out, PACK).unwrap().1, 0);
}

#[test]
fn split_only_cover_then_body_in_separate_steps() {
    // 只修封面分节，再修正文分节：结果与一次全修相同
    let t = one_section();
    let rep = chk(&t);
    let cover = rules(&rep, "page_numbers.cover_shared")[0].id.clone();
    let step1 = fix_selected(&t.bytes(), PACK, &[cover], None).unwrap();
    let rep1 = check(&step1, PACK).unwrap();
    assert!(rules(&rep1, "page_numbers.cover_shared").is_empty());
    let mixed = rules(&rep1, "page_numbers.mixed_section")[0].id.clone();
    let step2 = fix_selected(&step1, PACK, &[mixed], None).unwrap();
    let rep2 = check(&step2, PACK).unwrap();
    assert!(pn_findings(&rep2).is_empty(), "{:?}", pn_findings(&rep2));
}

// ------------------------------------------------------------------ 规则包扩展字段
/// 以 thu-master 为底改出的导入规则包；检查与修复都可用，测试结束（drop）时删掉。
struct TestPack(String, std::path::PathBuf);

impl std::ops::Deref for TestPack {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl Drop for TestPack {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.1);
    }
}

fn custom_pack(name: &str, edit: impl FnOnce(&mut serde_yaml_ng::Value)) -> TestPack {
    let mut raw: serde_yaml_ng::Value = serde_yaml_ng::from_str(include_str!("../../packs/thu-master.yaml")).unwrap();
    edit(&mut raw);
    let id = format!("user-{name}");
    raw["meta"]["id"] = id.clone().into();
    let yaml = serde_yaml_ng::to_string(&raw).unwrap();
    Pack::parse(&yaml).unwrap();
    let path = test_user_dir().join(format!("{id}.yaml"));
    std::fs::write(&path, yaml).unwrap();
    TestPack(id, path)
}

fn map(v: &mut serde_yaml_ng::Value) -> &mut serde_yaml_ng::Mapping {
    v.as_mapping_mut().unwrap()
}

#[test]
fn line_spacing_and_cm_indent_are_checked_and_fixed() {
    let id = custom_pack("lines", |p| {
        let b = map(&mut p["roles"]["body"]);
        b.remove("before");
        b.remove("first_line_chars");
        b.insert("before_lines".into(), 0.5.into());
        b.insert("first_line_cm".into(), 0.85.into());
    });
    let t = T::build(false);
    let rep = check(&t.bytes(), &id).unwrap();
    let sp = rules(&rep, "role.body.spacing");
    assert!(!sp.is_empty() && sp[0].message.contains("段前应为 0.5 行，实际 0 磅"), "{:?}", msgs(&sp));
    // 小四首行缩进 2 字符 = 0.85 厘米
    assert!(rules(&rep, "role.body.indent").is_empty(), "{:?}", msgs(&rules(&rep, "role.body.indent")));
    let out = fix(&t.bytes(), &id).unwrap();
    assert!(rules(&check(&out, &id).unwrap(), "role.body.spacing").is_empty());
    assert!(entry(&out, "word/document.xml").contains("w:beforeLines=\"50\""));
    // 无缩进的段：按厘米报，修复写 twip
    let v = T::build(true);
    let ind = rules(&check(&v.bytes(), &id).unwrap(), "role.body.indent").iter().map(|f| f.message.clone()).collect::<Vec<_>>();
    assert!(ind.iter().any(|m| m.contains("应为 首行缩进 0.85 厘米，实际 无缩进")), "{ind:?}");
    let out = fix(&v.bytes(), &id).unwrap();
    assert!(rules(&check(&out, &id).unwrap(), "role.body.indent").is_empty());
    assert!(entry(&out, "word/document.xml").contains("w:firstLine=\"482\""));
    // 两种写法不能同时出现
    let mut raw: serde_yaml_ng::Value = serde_yaml_ng::from_str(include_str!("../../packs/thu-master.yaml")).unwrap();
    map(&mut raw["roles"]["body"]).insert("before_lines".into(), 1.into());
    assert!(Pack::parse(&serde_yaml_ng::to_string(&raw).unwrap()).err().unwrap().contains("before 与 before_lines"));
}

#[test]
fn minimum_margins_accept_larger_values() {
    let set = |name: &str, v: f64| {
        custom_pack(name, |p| {
            p["page"]["margins_cm"] = serde_yaml_ng::from_str(&format!("{{top: {v}, bottom: {v}, left: {v}, right: {v}, at_least: true}}")).unwrap();
        })
    };
    let bytes = T::build(false).bytes();
    assert!(rules(&check(&bytes, &set("min25", 2.5)).unwrap(), "page.margins").is_empty());
    let rep = check(&bytes, &set("min35", 3.5)).unwrap();
    let m = rules(&rep, "page.margins");
    assert!(!m.is_empty() && m[0].message.contains("上边距应不小于 3.5 cm，实际 3 cm"), "{:?}", msgs(&m));
}

#[test]
fn front_matter_without_page_numbers_or_headers() {
    let id = custom_pack("front", |p| {
        p["page_numbers"]["front_no_number"] = true.into();
        p["headers"]["front_none"] = true.into();
    });
    let headers = vec![hstatic("摘  要", "center", 21), hstatic("绪论", "center", 21)];
    let rep = check(&three([Some(1), Some(2), None], headers).bytes(), &id).unwrap();
    let pn = rules(&rep, "page_numbers.front_none");
    assert!(pn.len() == 1 && pn[0].paragraph_index == Some(by_text(&rep, "摘")), "{:?}", msgs(&pn));
    // 不编页码的前置部分不再要求罗马数字格式
    assert!(rules(&rep, "page_numbers.format").is_empty(), "{:?}", msgs(&rules(&rep, "page_numbers.format")));
    let h = rules(&rep, "headers.front_none");
    assert!(h.len() == 1 && h[0].message.contains("「摘  要」"), "{:?}", msgs(&h));
    // 前置部分没有页眉：不报缺页眉
    let rep = check(&three([None, Some(1), None], vec![hstatic("绪论", "center", 21)]).bytes(), &id).unwrap();
    assert!(rules(&rep, "headers.front_none").is_empty() && rules(&rep, "headers.missing").is_empty(), "{:?}", msgs(&hdr_findings(&rep)));
}

#[test]
fn outside_page_numbers_and_even_page_headers() {
    let id = custom_pack("outside", |p| {
        map(&mut p["page_numbers"]).remove("footer_align");
        p["page_numbers"]["outside"] = true.into();
        map(&mut p["headers"]).remove("same_odd_even");
        p["headers"]["even"] = serde_yaml_ng::from_str("{text: 清华大学硕士学位论文}").unwrap();
    });
    // 没开「奇偶页不同」
    let rep = check(&T::build(false).bytes(), &id).unwrap();
    assert_eq!(rules(&rep, "page_numbers.outside").len(), 1);
    assert!(rules(&rep, "headers.odd_even").iter().any(|f| f.message.contains("奇数页、偶数页页眉不同")));
    // 开了：奇数页（默认）页脚居中应改居右；偶数页页脚缺页码
    let mut t = T::build(false);
    t.settings = Some("<w:evenAndOddHeaders/>".into());
    let rep = check(&t.bytes(), &id).unwrap();
    assert!(rules(&rep, "page_numbers.outside").is_empty());
    let ff = rules(&rep, "page_numbers.footer_format");
    assert!(!ff.is_empty() && ff[0].message.contains("奇数页页脚页码应居右，实际居中"), "{:?}", msgs(&ff));
    assert!(rules(&rep, "page_numbers.footer_missing").iter().any(|f| f.message.contains("偶数页页脚")));
    let out = fix(&t.bytes(), &id).unwrap();
    assert!(rules(&check(&out, &id).unwrap(), "page_numbers.footer_format").is_empty());
    assert!(entry(&out, "word/footer1.xml").contains("<w:jc w:val=\"right\"/>"));
    // 页码在 xAlign="outside" 的图文框里（Word「页码 → 外侧」）：不需要奇偶页不同
    fn frame(x: &'static str) -> impl Fn(&str, String) -> String {
        move |name, xml| {
            if name.contains("footer") { xml.replacen("<w:pPr>", &format!("<w:pPr><w:framePr w:wrap=\"around\" w:hAnchor=\"margin\" w:xAlign=\"{x}\"/>"), 1) } else { xml }
        }
    }
    let framed = rezip(&T::build(false).bytes(), frame("outside"));
    let rep = check(&framed, &id).unwrap();
    let pn: Vec<_> = rep.findings.iter().filter(|f| f.rule_id.starts_with("page_numbers.")).map(|f| f.message.clone()).collect();
    assert!(pn.is_empty(), "{pn:?}");
    // 只有一部分节用了外侧图文框：没用的节单独报告
    let half = rezip(&T::build(false).bytes(), |name, xml| if name.ends_with("footer2.xml") { frame("outside")(name, xml) } else { xml });
    let o = rules(&check(&half, &id).unwrap(), "page_numbers.outside").iter().map(|f| f.message.clone()).collect::<Vec<_>>();
    assert!(!o.is_empty() && o.iter().all(|m| m.contains("其他节的页码用了外侧图文框")), "{o:?}");
    // 图文框在左侧、规范要求居中：修复移动图文框本身
    let left = rezip(&T::build(false).bytes(), frame("left"));
    let ff = rules(&check(&left, PACK).unwrap(), "page_numbers.footer_format").iter().map(|f| f.message.clone()).collect::<Vec<_>>();
    assert!(!ff.is_empty() && ff[0].contains("页码应居中，实际居左"), "{ff:?}");
    let out = fix(&left, PACK).unwrap();
    assert!(entry(&out, "word/footer1.xml").contains("w:xAlign=\"center\""));
    assert!(rules(&check(&out, PACK).unwrap(), "page_numbers.footer_format").is_empty());
    // 偶数页页眉是固定文字
    let even = |s: String| s.replacen("<w:sectPr>", "<w:sectPr><w:headerReference w:type=\"even\" r:id=\"rIdH3\"/>", 1);
    let headers = vec![hstatic("摘  要", "center", 21), hstatic("绪论", "center", 21), hstatic("北京大学硕士学位论文", "center", 21)];
    let mut t = three([Some(1), Some(2), Some(2)], headers);
    t.paras[2] = even(t.paras[2].clone());
    t.settings = Some("<w:evenAndOddHeaders/>".into());
    let rep = check(&t.bytes(), &id).unwrap();
    let c: Vec<_> = rules(&rep, "headers.content").iter().filter(|f| f.message.contains("偶数页")).map(|f| f.message.clone()).collect();
    assert!(c.len() == 1 && c[0].contains("应为「清华大学硕士学位论文」，实际为「北京大学硕士学位论文」"), "{c:?}");
}

#[test]
fn appendix_and_continued_caption_styles() {
    let id = custom_pack("styles", |p| {
        p["checks"]["appendix_number"]["style"] = "roman".into();
        p["checks"]["continued_caption"]["style"] = "suffix".into();
    });
    let mut t = T::build(false);
    t.paras.push(add("chapter", "附录Ⅰ 推导", Ov::default()));
    t.paras.push(add("chapter", "附录 3 数据", Ov::default()));
    let rep = check(&t.bytes(), &id).unwrap();
    let a = rules(&rep, "appendix.number");
    assert!(a.len() == 1 && a[0].message.contains("用罗马数字编序号，此处应为“附录 II”，实际“附录 3”"), "{:?}", msgs(&a));
    assert!(a[0].confirm.as_ref().is_some_and(|c| c.text.as_ref().unwrap().after.contains("附录 II 数据")));
    // 「（续）」写法：不算编号重复，与原表一致即可；写成「续表」要改
    let ok = with_blocks(vec![cap("表1-1 示例数据"), table(), cap("表1-1 示例数据（续）"), table()]);
    let rep = check(&ok.bytes(), &id).unwrap();
    assert!(rules(&rep, "caption.continued").is_empty() && rules(&rep, "caption.number").is_empty(), "{:?}", msgs(&rules(&rep, "caption.number")));
    let rep = check(&with_blocks(vec![cap("表1-1 示例数据"), table(), cap("续表1-1 示例数据"), table()]).bytes(), &id).unwrap();
    let c = rules(&rep, "caption.continued");
    assert!(c.len() == 1 && c[0].message.contains("应在题注后加“（续）”") && c[0].message.contains("“表 1-1 示例数据（续）”"), "{:?}", msgs(&c));
    // 默认（表序前加「续」）规则包反过来
    let rep = chk(&ok);
    assert!(rules(&rep, "caption.continued").iter().any(|f| f.message.contains("应在表序前加“续”字")));
}

#[test]
fn font_aliases_theme_hans_and_approx_abstract_length() {
    // 「楷体_GB2312」即「楷体」
    let id = custom_pack("kai", |p| {
        p["roles"]["body"]["cn_font"] = "楷体".into();
        p["checks"]["abstract_text"]["chars_approx"] = true.into();
    });
    let mut t = T::build(false);
    t.paras[12] = add("body", "论文格式检查是一项繁琐的工作。", Ov { cn: Some("楷体_GB2312"), ..Ov::default() });
    t.paras[3] = add("body", "很短的摘要。", Ov::default());
    let rep = check(&t.bytes(), &id).unwrap();
    assert!(!rules(&rep, "role.body.font").iter().any(|f| f.paragraph_index == Some(12)), "{:?}", msgs(&rules(&rep, "role.body.font")));
    let a = rules(&rep, "abstract.length");
    assert!(a.len() == 1 && a[0].severity == Severity::Info && a[0].message.contains("仅供参考"));
    // 主题的 <a:ea> 为空、中文字体写在 script="Hans" 里：正文没直接设中文字体时取它
    let mut t = T::build(false);
    t.paras[13] = t.paras[13].replace(" w:eastAsia=\"宋体\"", "");
    let theme = |hans: &str| {
        format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><a:theme xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"><a:themeElements><a:fontScheme name=\"Office\"><a:majorFont><a:latin typeface=\"Calibri Light\"/><a:ea typeface=\"\"/></a:majorFont><a:minorFont><a:latin typeface=\"Calibri\"/><a:ea typeface=\"\"/>{hans}</a:minorFont></a:fontScheme></a:themeElements></a:theme>")
    };
    let font13 = |b: &[u8]| rules(&check(b, PACK).unwrap(), "role.body.font").iter().filter(|f| f.paragraph_index == Some(13)).map(|f| f.message.clone()).collect::<Vec<_>>();
    assert!(font13(&t.bytes()).iter().any(|m| m.contains("未确定")));
    let hans = rezip(&t.bytes(), |name, xml| if name == "word/theme/theme1.xml" { theme("<a:font script=\"Hans\" typeface=\"宋体\"/>") } else { xml });
    assert!(font13(&hans).is_empty(), "{:?}", font13(&hans));
}

#[test]
fn front_titles_numbered_chapters_and_template_notes() {
    let o = Ov::default;
    let mut t = T::build(false);
    // 无大纲级别的「1 绪论」「2 方法」：编号连续、格式一致，第 1 章后跟着 1.1 节
    t.paras[10] = add("chapter", "1 绪论", o());
    t.paras[17] = add("chapter", "2 方法", o());
    // 正文里加粗的编号小标题：后面没有对应的节，不算章
    t.paras.insert(14, add("plain", "2 方案二", Ov { bold: true, ..o() }));
    t.paras.insert(14, add("plain", "1 方案一", Ov { bold: true, ..o() }));
    t.paras.insert(9, add("abs_title", "符号说明", o()));
    t.paras.insert(9, add("abs_title", "插图清单", o()));
    t.paras[2] = add("abs_title", "摘  要（三号黑体，居中）", o());
    t.paras.insert(4, add("plain", "（正文宋体小四号，1.5倍行距）", o()));
    let rep = chk(&t);
    let role = |text: &str| rep.roles.iter().find(|r| r.text == text).map(|r| (r.role, r.ambiguous)).unwrap();
    assert_eq!(role("1 绪论"), (Some("chapter"), false));
    assert_eq!(role("2 方法"), (Some("chapter"), false));
    assert_ne!(role("1 方案一").0, Some("chapter"));
    assert_eq!(role("插图清单"), (Some("front_title"), false));
    assert_eq!(role("符号说明"), (Some("front_title"), false));
    assert_eq!(role("摘  要（三号黑体，居中）"), (Some("abstract_title_zh"), false));
    assert_eq!(role("（正文宋体小四号，1.5倍行距）").0, None);
    let n = rules(&rep, "template.note");
    assert_eq!(n.len(), 2, "{:?}", msgs(&n));
    let title = n.iter().find(|f| f.message.contains("（三号黑体，居中）")).unwrap();
    assert!(title.confirm.as_ref().is_some_and(|c| c.text.as_ref().unwrap().after == "摘  要"));
    assert!(n.iter().any(|f| f.message.contains("整段都是说明") && f.confirm.is_none()));
}

#[test]
fn caption_with_non_breaking_hyphen() {
    // 题注“包含章节号”、分隔符选不间断连字符：Word 写 <w:noBreakHyphen/>，应读作“表1-1”
    let nb = |s: String| s.replace(">表1-1 ", ">表1</w:t><w:noBreakHyphen/><w:t xml:space=\"preserve\">1 ");
    let t = with_blocks(vec![nb(cap("表1-1 示例数据")), table(), nb(cap("续表1-1 示例数据")), table()]);
    assert!(t.paras.iter().any(|p| p.contains("noBreakHyphen")));
    let rep = chk(&t);
    assert_eq!(rep.roles.iter().find(|r| r.text == "表1-1 示例数据").map(|r| r.role), Some(Some("table_caption")));
    for id in ["caption.number", "caption.continued", "table.caption"] {
        assert!(rules(&rep, id).is_empty(), "{id}: {:?}", msgs(&rules(&rep, id)));
    }
    // 修复前后正文文字比对把连字符算在内，不会误判为改了文字
    let (out, _) = fixed(&t);
    assert!(entry(&out, "word/document.xml").contains("<w:noBreakHyphen/>"));
}

