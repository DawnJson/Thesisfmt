//! 规则包开发工具。
//! - `cargo run --example extract -- draft <模板.docx> <输出.yaml> '<meta JSON>'`：由模板生成规则包草稿；
//!   meta 是 `PackMeta` 的 JSON（id、title、version、source、org、kind、category）。
//! - `cargo run --example extract -- check <规则包.yaml>...`：按程序的规则包格式校验，逐个打印结果。
//! - `cargo run --example extract -- test <规则包.yaml> <文档.docx>`：用该规则包检查文档，打印全部问题（不含精确检查）。

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("draft") if args.len() == 4 => {
            let run = || -> Result<(), String> {
                let docx = std::fs::read(&args[1]).map_err(|e| format!("{}: {e}", args[1]))?;
                let meta: thesisfmt_lib::PackMeta = serde_json::from_str(&args[3]).map_err(|e| format!("meta: {e}"))?;
                let name = std::path::Path::new(&args[1]).file_name().and_then(|s| s.to_str()).unwrap_or("模板");
                let yaml = thesisfmt_lib::extract_pack(&docx, &meta, name)?;
                std::fs::write(&args[2], yaml).map_err(|e| format!("{}: {e}", args[2]))
            };
            match run() {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("{e}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("check") if args.len() > 1 => {
            let mut ok = true;
            for f in &args[1..] {
                let res = std::fs::read_to_string(f).map_err(|e| e.to_string()).and_then(|y| thesisfmt_lib::validate_pack(&y));
                match res {
                    Ok(m) if std::path::Path::new(f).file_stem().and_then(|s| s.to_str()) == Some(m.id.as_str()) => println!("OK  {f}"),
                    Ok(m) => {
                        ok = false;
                        println!("ERR {f}: meta.id「{}」与文件名不一致", m.id);
                    }
                    Err(e) => {
                        ok = false;
                        println!("ERR {f}: {e}");
                    }
                }
            }
            if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE }
        }
        Some("test") if args.len() == 3 => {
            let run = || -> Result<Vec<(String, String)>, String> {
                let yaml = std::fs::read_to_string(&args[1]).map_err(|e| format!("{}: {e}", args[1]))?;
                let docx = std::fs::read(&args[2]).map_err(|e| format!("{}: {e}", args[2]))?;
                thesisfmt_lib::check_yaml(&docx, &yaml)
            };
            match run() {
                Ok(items) => {
                    for (id, msg) in &items {
                        println!("{id}\t{msg}");
                    }
                    println!("共 {} 项", items.len());
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("{e}");
                    ExitCode::FAILURE
                }
            }
        }
        _ => {
            eprintln!("用法: extract draft <模板.docx> <输出.yaml> '<meta JSON>' | check <规则包.yaml>... | test <规则包.yaml> <文档.docx>");
            ExitCode::from(2)
        }
    }
}
