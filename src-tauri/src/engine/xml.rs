//! 最小可变 XML DOM：保留命名空间前缀与声明（mc:Ignorable 依赖前缀），按需解析/回写 OOXML 部件。

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

const W_URI: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const M_URI: &str = "http://schemas.openxmlformats.org/officeDocument/2006/math";
const A_URI: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const R_URI: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const MC_URI: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
const DECL: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>";

#[derive(Clone, Debug, PartialEq)]
pub enum Ns {
    None,
    W,
    M,
    A,
    R,
    Mc,
    Other(String),
}

impl Ns {
    fn from_uri(uri: &str) -> Ns {
        match uri {
            "" => Ns::None,
            W_URI => Ns::W,
            M_URI => Ns::M,
            A_URI => Ns::A,
            R_URI => Ns::R,
            MC_URI => Ns::Mc,
            u => Ns::Other(u.to_string()),
        }
    }
}

pub type Id = usize;

#[derive(Clone)]
struct Attr {
    qname: String,
    ns: Ns,
    value: String,
}

#[derive(Clone)]
struct Elem {
    qname: String,
    ns: Ns,
    attrs: Vec<Attr>,
}

impl Elem {
    fn local(&self) -> &str {
        self.qname.rsplit(':').next().unwrap_or(&self.qname)
    }

    fn prefix(&self) -> &str {
        self.qname.split_once(':').map_or("", |(p, _)| p)
    }
}

#[derive(Clone)]
enum Data {
    Doc,
    Elem(Elem),
    Text(String),
    Raw(String),
}

struct Node {
    parent: Option<Id>,
    children: Vec<Id>,
    data: Data,
}

pub struct Doc {
    nodes: Vec<Node>,
}

fn lookup(scope: &[(String, String)], prefix: &str) -> Ns {
    match scope.iter().rev().find(|(p, _)| p == prefix) {
        Some((_, uri)) => Ns::from_uri(uri),
        None if prefix == "xml" => Ns::Other("http://www.w3.org/XML/1998/namespace".into()),
        None => Ns::None,
    }
}

fn escape(s: &str, attr: bool, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attr => out.push_str("&quot;"),
            '\n' if attr => out.push_str("&#10;"),
            '\t' if attr => out.push_str("&#9;"),
            '\r' => out.push_str("&#13;"),
            c => out.push(c),
        }
    }
}

impl Doc {
    pub fn parse(bytes: &[u8]) -> Result<Doc, String> {
        let s = std::str::from_utf8(bytes).map_err(|e| format!("XML 不是 UTF-8: {e}"))?;
        let s = s.strip_prefix('\u{feff}').unwrap_or(s);
        let mut reader = Reader::from_str(s);
        let mut doc = Doc { nodes: vec![Node { parent: None, children: vec![], data: Data::Doc }] };
        let mut stack = vec![0usize];
        let mut scope: Vec<(String, String)> = vec![];
        let mut marks: Vec<usize> = vec![];
        loop {
            let parent = *stack.last().unwrap();
            match reader.read_event().map_err(|e| format!("XML 解析失败: {e}"))? {
                Event::Start(e) => {
                    marks.push(scope.len());
                    let id = doc.open(&e, parent, &mut scope)?;
                    stack.push(id);
                }
                Event::Empty(e) => {
                    let mark = scope.len();
                    doc.open(&e, parent, &mut scope)?;
                    scope.truncate(mark);
                }
                Event::End(_) => {
                    stack.pop();
                    scope.truncate(marks.pop().ok_or("XML 标签不匹配")?);
                }
                Event::Text(t) => {
                    let t = t.unescape().map_err(|e| format!("XML 解析失败: {e}"))?;
                    // 根元素之外只可能是空白；转义 \r 会得到 Word 打不开的 `&#13;`
                    doc.push(parent, if parent == 0 { Data::Raw(t.into_owned()) } else { Data::Text(t.into_owned()) });
                }
                Event::CData(c) => {
                    doc.push(parent, Data::Text(String::from_utf8_lossy(&c).into_owned()));
                }
                Event::Comment(c) => {
                    doc.push(parent, Data::Raw(format!("<!--{}-->", String::from_utf8_lossy(&c))));
                }
                Event::PI(p) => {
                    doc.push(parent, Data::Raw(format!("<?{}?>", String::from_utf8_lossy(&p))));
                }
                Event::DocType(d) => {
                    doc.push(parent, Data::Raw(format!("<!DOCTYPE {}>", String::from_utf8_lossy(&d).trim())));
                }
                Event::Decl(_) => {
                    doc.push(parent, Data::Raw(DECL.into()));
                }
                Event::Eof => break,
            }
        }
        Ok(doc)
    }

    fn push(&mut self, parent: Id, data: Data) -> Id {
        let id = self.nodes.len();
        self.nodes.push(Node { parent: Some(parent), children: vec![], data });
        self.nodes[parent].children.push(id);
        id
    }

    fn open(&mut self, e: &BytesStart, parent: Id, scope: &mut Vec<(String, String)>) -> Result<Id, String> {
        let mut raw = vec![];
        for a in e.attributes() {
            let a = a.map_err(|e| format!("XML 解析失败: {e}"))?;
            let key = String::from_utf8_lossy(a.key.as_ref()).into_owned();
            let value = a.unescape_value().map_err(|e| format!("XML 解析失败: {e}"))?.into_owned();
            if key == "xmlns" {
                scope.push((String::new(), value.clone()));
            } else if let Some(p) = key.strip_prefix("xmlns:") {
                scope.push((p.to_string(), value.clone()));
            }
            raw.push((key, value));
        }
        let attrs = raw
            .into_iter()
            .map(|(qname, value)| {
                let ns = match qname.split_once(':') {
                    Some((p, _)) if p != "xmlns" => lookup(scope, p),
                    _ => Ns::None,
                };
                Attr { qname, ns, value }
            })
            .collect();
        let qname = String::from_utf8_lossy(e.name().as_ref()).into_owned();
        let ns = lookup(scope, qname.split_once(':').map_or("", |(p, _)| p));
        Ok(self.push(parent, Data::Elem(Elem { qname, ns, attrs })))
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = String::new();
        self.write(0, &mut out);
        out.into_bytes()
    }

    fn write(&self, id: Id, out: &mut String) {
        let n = &self.nodes[id];
        match &n.data {
            Data::Doc => n.children.iter().for_each(|&c| self.write(c, out)),
            Data::Text(t) => escape(t, false, out),
            Data::Raw(r) => out.push_str(r),
            Data::Elem(e) => {
                out.push('<');
                out.push_str(&e.qname);
                for a in &e.attrs {
                    out.push(' ');
                    out.push_str(&a.qname);
                    out.push_str("=\"");
                    escape(&a.value, true, out);
                    out.push('"');
                }
                if n.children.is_empty() {
                    out.push_str("/>");
                } else {
                    out.push('>');
                    n.children.iter().for_each(|&c| self.write(c, out));
                    out.push_str("</");
                    out.push_str(&e.qname);
                    out.push('>');
                }
            }
        }
    }

    fn elem(&self, id: Id) -> Option<&Elem> {
        match &self.nodes[id].data {
            Data::Elem(e) => Some(e),
            _ => None,
        }
    }

    fn elem_mut(&mut self, id: Id) -> &mut Elem {
        match &mut self.nodes[id].data {
            Data::Elem(e) => e,
            _ => unreachable!("不是元素节点"),
        }
    }

    // ------------------------------------------------------------ 查询
    pub fn root(&self) -> Id {
        self.nodes[0].children.iter().copied().find(|&c| self.elem(c).is_some()).expect("XML 没有根元素")
    }

    pub fn parent(&self, id: Id) -> Option<Id> {
        self.nodes[id].parent
    }

    pub fn children(&self, id: Id) -> &[Id] {
        &self.nodes[id].children
    }

    pub fn is(&self, id: Id, ns: &Ns, local: &str) -> bool {
        self.elem(id).is_some_and(|e| e.ns == *ns && e.local() == local)
    }

    pub fn is_w(&self, id: Id, local: &str) -> bool {
        self.is(id, &Ns::W, local)
    }

    /// 任意命名空间下的本地名（rels / Content_Types 用）。
    pub fn local(&self, id: Id) -> &str {
        self.elem(id).map_or("", |e| e.local())
    }

    pub fn prefix(&self, id: Id) -> &str {
        self.elem(id).map_or("", |e| e.prefix())
    }

    pub fn child(&self, id: Id, ns: &Ns, local: &str) -> Option<Id> {
        self.children(id).iter().copied().find(|&c| self.is(c, ns, local))
    }

    pub fn wchild(&self, id: Id, local: &str) -> Option<Id> {
        self.child(id, &Ns::W, local)
    }

    /// 直接子元素（跳过文本等）。
    pub fn child_els(&self, id: Id) -> impl Iterator<Item = Id> + '_ {
        self.children(id).iter().copied().filter(|&c| self.elem(c).is_some())
    }

    /// 前序遍历的后代元素（不含自身）。
    pub fn descendants(&self, id: Id) -> Vec<Id> {
        let mut out = vec![];
        let mut stack: Vec<Id> = self.children(id).iter().rev().copied().collect();
        while let Some(n) = stack.pop() {
            if self.elem(n).is_some() {
                out.push(n);
                stack.extend(self.children(n).iter().rev());
            }
        }
        out
    }

    pub fn attr(&self, id: Id, ns: &Ns, local: &str) -> Option<&str> {
        self.elem(id)?
            .attrs
            .iter()
            .find(|a| a.ns == *ns && a.qname.rsplit(':').next() == Some(local))
            .map(|a| a.value.as_str())
    }

    pub fn wattr(&self, id: Id, local: &str) -> Option<&str> {
        self.attr(id, &Ns::W, local)
    }

    /// 无命名空间属性（Relationship 的 Id / Type / Target 等）。
    pub fn plain_attr(&self, id: Id, name: &str) -> Option<&str> {
        self.attr(id, &Ns::None, name)
    }

    /// 直接文本子节点拼接（w:t / w:instrText）。
    pub fn text(&self, id: Id) -> String {
        self.children(id)
            .iter()
            .filter_map(|&c| match &self.nodes[c].data {
                Data::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect()
    }

    /// 元素 `id` 在其父节点中的下标。
    fn index_in_parent(&self, id: Id) -> Option<usize> {
        let p = self.parent(id)?;
        self.children(p).iter().position(|&c| c == id)
    }

    // ------------------------------------------------------------ 修改
    /// 新建与 `like` 同前缀、W 命名空间的游离元素。
    pub fn create(&mut self, like: Id, local: &str) -> Id {
        let p = self.prefix(like);
        let qname = if p.is_empty() { local.to_string() } else { format!("{p}:{local}") };
        let ns = self.elem(like).map_or(Ns::W, |e| e.ns.clone());
        let id = self.nodes.len();
        self.nodes.push(Node {
            parent: None,
            children: vec![],
            data: Data::Elem(Elem { qname, ns, attrs: vec![] }),
        });
        id
    }

    pub fn create_text(&mut self, text: &str) -> Id {
        let id = self.nodes.len();
        self.nodes.push(Node { parent: None, children: vec![], data: Data::Text(text.to_string()) });
        id
    }

    /// 设置与元素同命名空间前缀的属性（w:val 等）。
    pub fn set_wattr(&mut self, id: Id, local: &str, value: &str) {
        let prefix = self.prefix(id).to_string();
        let e = self.elem_mut(id);
        if let Some(a) = e.attrs.iter_mut().find(|a| a.ns == Ns::W && a.qname.rsplit(':').next() == Some(local)) {
            a.value = value.to_string();
        } else {
            let qname = if prefix.is_empty() { local.to_string() } else { format!("{prefix}:{local}") };
            e.attrs.push(Attr { qname, ns: Ns::W, value: value.to_string() });
        }
    }

    /// 无命名空间属性。
    pub fn set_plain_attr(&mut self, id: Id, name: &str, value: &str) {
        let e = self.elem_mut(id);
        if let Some(a) = e.attrs.iter_mut().find(|a| a.ns == Ns::None && a.qname == name) {
            a.value = value.to_string();
        } else {
            e.attrs.push(Attr { qname: name.to_string(), ns: Ns::None, value: value.to_string() });
        }
    }

    pub fn remove_wattr(&mut self, id: Id, local: &str) {
        self.elem_mut(id).attrs.retain(|a| !(a.ns == Ns::W && a.qname.rsplit(':').next() == Some(local)));
    }

    /// 把 `id` 的文本子节点换成 `text`（w:t 用）；首尾有空白时补 `xml:space="preserve"`。
    pub fn set_text(&mut self, id: Id, text: &str) {
        let kids = std::mem::take(&mut self.nodes[id].children);
        let mut placed = false;
        for c in kids {
            if matches!(self.nodes[c].data, Data::Text(_)) {
                if placed {
                    self.nodes[c].parent = None;
                    continue;
                }
                self.nodes[c].data = Data::Text(text.to_string());
                placed = true;
            }
            self.nodes[id].children.push(c);
        }
        if !placed && !text.is_empty() {
            let t = self.create_text(text);
            self.append(id, t);
        }
        let xml = Ns::Other("http://www.w3.org/XML/1998/namespace".into());
        if text.trim() != text && self.attr(id, &xml, "space").is_none() {
            self.elem_mut(id).attrs.push(Attr { qname: "xml:space".into(), ns: xml, value: "preserve".into() });
        }
    }

    pub fn append(&mut self, parent: Id, child: Id) {
        self.nodes[child].parent = Some(parent);
        self.nodes[parent].children.push(child);
    }

    pub fn insert_at(&mut self, parent: Id, index: usize, child: Id) {
        self.nodes[child].parent = Some(parent);
        self.nodes[parent].children.insert(index, child);
    }

    pub fn insert_before(&mut self, sibling: Id, child: Id) {
        let (p, i) = (self.parent(sibling).expect("无父节点"), self.index_in_parent(sibling).unwrap());
        self.insert_at(p, i, child);
    }

    pub fn insert_after(&mut self, sibling: Id, child: Id) {
        let (p, i) = (self.parent(sibling).expect("无父节点"), self.index_in_parent(sibling).unwrap());
        self.insert_at(p, i + 1, child);
    }

    /// 从父节点上摘下（节点仍留在 arena 中，可再插入）。
    pub fn detach(&mut self, id: Id) {
        if let Some(p) = self.nodes[id].parent.take() {
            self.nodes[p].children.retain(|&c| c != id);
        }
    }

    /// 深拷贝 `id` 及其后代，返回游离的新节点。
    pub fn deep_clone(&mut self, id: Id) -> Id {
        let new = self.nodes.len();
        self.nodes.push(Node { parent: None, children: vec![], data: self.nodes[id].data.clone() });
        for c in self.nodes[id].children.clone() {
            let cc = self.deep_clone(c);
            self.append(new, cc);
        }
        new
    }
}
