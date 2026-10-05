//! 角色识别统计：遍历一个目录里的高校、期刊模板（.docx / .dotx），统计各角色段数和疑似漏认的段落。
//! `cargo run --release --example survey -- <模板目录> <输出 JSON>`

use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if p.is_dir() {
            if name != "__MACOSX" {
                walk(&p, out);
            }
        } else {
            let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("").to_lowercase();
            if !name.starts_with("~$") && !name.starts_with("._") && matches!(ext.as_str(), "docx" | "dotx" | "docm" | "dotm") {
                out.push(p);
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [root, out] = args.as_slice() else {
        eprintln!("用法：cargo run --release --example survey -- <模板目录> <输出 JSON>");
        std::process::exit(2);
    };
    let (root, out) = (PathBuf::from(root), PathBuf::from(out));
    let mut files = vec![];
    walk(&root, &mut files);
    files.sort();
    let mut rows = vec![];
    let (mut failed, mut crashed) = (0, 0);
    let mut misses: BTreeMap<String, u64> = BTreeMap::new();
    let mut docs_missing: BTreeMap<String, u64> = BTreeMap::new();
    std::panic::set_hook(Box::new(|info| eprintln!("panic in {info}")));
    for f in &files {
        let rel = f.strip_prefix(&root).unwrap_or(f).to_string_lossy().replace('\\', "/");
        let bytes = match std::fs::read(f) {
            Ok(b) => b,
            Err(e) => {
                rows.push(json!({"file": rel, "error": e.to_string()}));
                failed += 1;
                continue;
            }
        };
        let row = match std::panic::catch_unwind(|| thesisfmt_lib::survey(&bytes)) {
            Ok(Ok(mut v)) => {
                if let Some(m) = v["misses"].as_object() {
                    for (k, n) in m {
                        *misses.entry(k.clone()).or_default() += n.as_u64().unwrap_or(0);
                        *docs_missing.entry(k.clone()).or_default() += 1;
                    }
                }
                v["file"] = json!(rel);
                v
            }
            Ok(Err(e)) => {
                failed += 1;
                json!({"file": rel, "error": e})
            }
            Err(p) => {
                crashed += 1;
                let msg = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()));
                json!({"file": rel, "panic": msg})
            }
        };
        rows.push(row);
    }
    let clean = rows.iter().filter(|r| r["misses"].as_object().is_some_and(|m| m.is_empty())).count();
    let summary = json!({
        "files": files.len(), "failed": failed, "panicked": crashed, "all_recognised": clean,
        "misses_by_role": misses, "docs_with_misses_by_role": docs_missing,
    });
    std::fs::write(&out, serde_json::to_string_pretty(&json!({"summary": summary, "docs": rows})).unwrap()).unwrap();
    println!("{}", serde_json::to_string_pretty(&summary).unwrap());
}
