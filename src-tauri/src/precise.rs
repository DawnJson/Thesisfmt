//! 精确检查的采集：调用本机 Microsoft Word / WPS 文字（COM 自动化）真正排版分页。
//! 只用标准库启动 PowerShell 执行内嵌脚本，脚本把结果写成 JSON 文件；检查本身在 `engine::layout`。

use crate::engine::{layout_copy, Layout};
use serde::Serialize;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SCRIPT: &str = include_str!("collect_layout.ps1");
const TIMEOUT: Duration = Duration::from_secs(300);

struct Engine {
    id: &'static str,
    name: &'static str,
    /// 按顺序尝试的 COM ProgID。
    prog_ids: &'static [&'static str],
    /// 进程名（不含 .exe），用来区分新启动的实例与用户已打开的实例。
    process: &'static str,
}

const ENGINES: [Engine; 2] = [
    Engine { id: "word", name: "Microsoft Word", prog_ids: &["Word.Application"], process: "WINWORD" },
    Engine { id: "wps", name: "WPS 文字", prog_ids: &["KWps.Application", "_Wps_.Application"], process: "wps" },
];

#[derive(Serialize)]
pub struct EngineInfo {
    id: &'static str,
    name: &'static str,
}

fn command(program: &str) -> Command {
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut cmd, 0x0800_0000); // CREATE_NO_WINDOW
    cmd
}

/// 注册表 HKCR 下已注册的第一个 ProgID。
fn prog_id(e: &Engine) -> Option<&'static str> {
    e.prog_ids.iter().copied().find(|p| {
        command("reg")
            .args(["query", &format!("HKCR\\{p}")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    })
}

pub fn engines() -> Vec<EngineInfo> {
    ENGINES.iter().filter(|e| prog_id(e).is_some()).map(|e| EngineInfo { id: e.id, name: e.name }).collect()
}

/// 临时目录，离开作用域即删除。
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Scratch, String> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!("thesisfmt-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(|e| format!("创建临时目录失败：{e}"))?;
        Ok(Scratch(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 用指定引擎排版 `docx`（工作副本字节），返回分页结果。会阻塞，调用方应放到后台线程。
pub fn collect(engine_id: &str, docx: &[u8]) -> Result<Layout, String> {
    let e = ENGINES.iter().find(|e| e.id == engine_id).ok_or_else(|| format!("未知的排版引擎：{engine_id}"))?;
    let fail = |msg: &str| format!("{}排版失败：{msg}", e.name);
    let prog = prog_id(e).ok_or_else(|| fail("未检测到该程序"))?;
    let (bytes, paras, tables) = layout_copy(docx)?;
    let dir = Scratch::new()?;
    let path = |name: &str| dir.0.join(name);
    std::fs::write(path("layout.docx"), bytes).map_err(|err| fail(&err.to_string()))?;

    let log = File::create(path("stderr.txt")).map_err(|err| fail(&err.to_string()))?;
    let mut child = command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command"])
        .arg("& ([scriptblock]::Create([Console]::In.ReadToEnd()))")
        .env("TFMT_PROGID", prog)
        .env("TFMT_PROC", e.process)
        .env("TFMT_NAME", e.name)
        .env("TFMT_DOC", path("layout.docx"))
        .env("TFMT_OUT", path("out.json"))
        .env("TFMT_PIDFILE", path("pid.txt"))
        .env("TFMT_PARAS", paras.to_string())
        .env("TFMT_TABLES", tables.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(log)
        .spawn()
        .map_err(|err| fail(&format!("无法启动 PowerShell：{err}")))?;
    child
        .stdin
        .take()
        .expect("stdin 已重定向")
        .write_all(SCRIPT.as_bytes())
        .map_err(|err| fail(&format!("无法向 PowerShell 传入脚本：{err}")))?;

    let start = Instant::now();
    while child.try_wait().map_err(|err| fail(&err.to_string()))?.is_none() {
        if start.elapsed() > TIMEOUT {
            let _ = child.kill();
            // 只结束脚本记录下的、由它自己启动的实例
            if let Some(pid) = std::fs::read_to_string(path("pid.txt")).ok().filter(|p| p.chars().all(|c| c.is_ascii_digit() || c == ',')) {
                for p in pid.split(',').filter(|p| !p.is_empty()) {
                    let _ = command("taskkill").args(["/PID", p, "/F"]).stdout(Stdio::null()).stderr(Stdio::null()).status();
                }
            }
            return Err(fail("超时（5 分钟）"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    match std::fs::read_to_string(path("out.json")) {
        Ok(json) => Layout::parse(json.trim_start_matches('\u{feff}')).map_err(|m| fail(&m)),
        Err(_) => {
            let stderr = std::fs::read_to_string(path("stderr.txt")).unwrap_or_default();
            Err(fail(&format!("没有得到排版结果（{}）", stderr.trim().chars().take(200).collect::<String>())))
        }
    }
}
