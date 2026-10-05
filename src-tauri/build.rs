use std::fmt::Write;

/// 把 packs/*.yaml 编进程序：生成 `PACKS: [(id, yaml)]`，id 为文件名（不含扩展名），按 id 排序。
fn embed_packs() {
    println!("cargo:rerun-if-changed=packs");
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("packs");
    let mut ids: Vec<String> = std::fs::read_dir(&dir)
        .expect("缺少 packs 目录")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "yaml"))
        .map(|p| p.file_stem().unwrap().to_str().expect("规则包文件名须为 UTF-8").to_string())
        .collect();
    ids.sort();
    // user- 开头的 id 留给用户导入的模板
    if let Some(bad) = ids.iter().find(|id| id.starts_with("user-")) {
        panic!("内置规则包不能以 user- 开头: {bad}.yaml");
    }
    let mut src = format!("const PACKS: [(&str, &str); {}] = [\n", ids.len());
    for id in &ids {
        let path = dir.join(format!("{id}.yaml"));
        writeln!(src, "    ({id:?}, include_str!({:?})),", path.to_str().unwrap()).unwrap();
    }
    src.push_str("];\n");
    let out = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("packs.rs");
    std::fs::write(out, src).unwrap();
}

fn main() {
    embed_packs();
    tauri_build::build()
}
