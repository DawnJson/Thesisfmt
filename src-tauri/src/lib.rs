mod engine;
mod precise;
mod settings;

/// 开发工具（`examples/survey.rs`、`examples/extract.rs`）：模板库识别统计、由模板生成规则包草稿。
#[doc(hidden)]
pub use engine::{check_yaml, extract::extract_pack, survey::survey, validate_pack, PackMeta};

use engine::{Layout, Report};
use serde::Serialize;
use settings::Settings;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use tauri::{Manager, State};

/// 工作副本会话：`history[0]` 是原始字节，末尾是当前副本；原文件永不写入。
struct Session {
    path: String,
    pack_id: String,
    history: Vec<Vec<u8>>,
    /// 开启「工作副本」设置时在原文件旁新建的文件；每次修复 / 撤销后写入当前副本。
    work_path: Option<String>,
    /// 当前副本的精确检查结果；副本或规则包一变就作废。
    precise: Option<Layout>,
    /// 副本 / 规则包 / 文件每变一次换一个新值，用来丢弃过期的精确检查结果。
    version: u64,
}

static VERSION: AtomicU64 = AtomicU64::new(0);

fn next_version() -> u64 {
    VERSION.fetch_add(1, Ordering::Relaxed) + 1
}

type Sessions = Mutex<Option<Session>>;

/// 用户设置及其所在的应用配置目录。
struct Config {
    dir: PathBuf,
    settings: Mutex<Settings>,
}

#[derive(Serialize)]
struct Snapshot {
    #[serde(flatten)]
    report: Report,
    /// 已对原文件副本做了几步修改（可撤销的步数）。
    steps: usize,
    /// 自动写入的工作副本路径；未开启时为 null。
    work_path: Option<String>,
}

impl Session {
    fn new(path: String, pack_id: String, docx: Vec<u8>) -> Session {
        Session { path, pack_id, history: vec![docx], work_path: None, precise: None, version: next_version() }
    }

    /// 有工作副本时把 `bytes` 写进去；失败则调用方不应改动历史，保证内存与磁盘一致。
    fn write_work(&self, bytes: &[u8]) -> Result<(), String> {
        match &self.work_path {
            Some(p) => std::fs::write(p, bytes).map_err(|e| format!("写入工作副本失败 {p}: {e}")),
            None => Ok(()),
        }
    }

    /// 工作副本或规则包变了：精确检查结果作废。
    fn changed(&mut self) {
        self.precise = None;
        self.version = next_version();
    }

    /// 保存 `version` 那一刻的副本排出来的结果；排版期间副本、规则包或文件已变则丢弃并返回 false。
    fn accept(&mut self, version: u64, layout: Layout) -> bool {
        let fresh = self.version == version;
        if fresh {
            self.precise = Some(layout);
        }
        fresh
    }

    fn snapshot(&self) -> Result<Snapshot, String> {
        let current = self.history.last().expect("history 非空");
        Ok(Snapshot {
            report: engine::check_with(current, &self.pack_id, self.precise.as_ref())?,
            steps: self.history.len() - 1,
            work_path: self.work_path.clone(),
        })
    }
}

fn with_session<T>(state: &Sessions, f: impl FnOnce(&mut Session) -> Result<T, String>) -> Result<T, String> {
    let mut guard = state.lock().map_err(|e| e.to_string())?;
    f(guard.as_mut().ok_or("尚未打开文件")?)
}

static LAUNCH_TAKEN: AtomicBool = AtomicBool::new(false);

/// 启动参数里的第一个 .docx 路径（双击文件 / 「打开方式」）；只返回一次，
/// 避免前端重复调用（如开发模式 StrictMode 下 effect 执行两次）时重复打开、重复创建工作副本。
#[tauri::command]
fn launch_file() -> Option<String> {
    if LAUNCH_TAKEN.swap(true, Ordering::Relaxed) {
        return None;
    }
    std::env::args()
        .skip(1)
        .find(|a| a.to_lowercase().ends_with(".docx"))
}

#[tauri::command]
async fn list_packs() -> Vec<PackMeta> {
    engine::list_packs()
}

/// 从用户选的 .docx / .dotx 模板提取格式规则（不核对规范）。
#[tauri::command]
async fn import_template(path: String) -> Result<PackMeta, String> {
    let docx = read_docx(&path)?;
    let name = Path::new(&path).file_stem().and_then(|s| s.to_str()).unwrap_or("模板");
    engine::import_template(&docx, name)
}

#[tauri::command]
async fn delete_pack(id: String) -> Result<(), String> {
    engine::delete_user_pack(&id)
}

fn read_docx(path: &str) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("{path}: {e}"))
}

/// 在 `src` 同目录新建 `原名{suffix}.docx` 并写入 `bytes`；同名文件已存在则依次尝试 `(2)`、`(3)`…，绝不覆盖已有文件。
fn create_work_copy(src: &str, suffix: &str, bytes: &[u8]) -> Result<String, String> {
    let src = Path::new(src);
    let dir = src.parent().unwrap_or(Path::new(""));
    let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("论文");
    let mut n = 1u32;
    loop {
        let name = if n == 1 { format!("{stem}{suffix}.docx") } else { format!("{stem}{suffix}({n}).docx") };
        let path = dir.join(name);
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                return match f.write_all(bytes) {
                    Ok(()) => Ok(path.to_string_lossy().into_owned()),
                    Err(e) => {
                        drop(f);
                        let _ = std::fs::remove_file(&path);
                        Err(format!("创建工作副本失败 {}: {e}", path.display()))
                    }
                };
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
            Err(e) => return Err(format!("创建工作副本失败 {}: {e}", path.display())),
        }
    }
}

#[tauri::command]
async fn open_doc(state: State<'_, Sessions>, config: State<'_, Config>, path: String, pack_id: String) -> Result<Snapshot, String> {
    let docx = read_docx(&path)?;
    let mut session = Session::new(path, pack_id, docx);
    // 先检查：打不开的文件不留下工作副本
    let mut snap = session.snapshot()?;
    let settings = config.settings.lock().map_err(|e| e.to_string())?.clone();
    if settings.work_copy {
        let work = create_work_copy(&session.path, &settings.work_copy_suffix, &session.history[0])?;
        snap.work_path = Some(work.clone());
        session.work_path = Some(work);
    }
    *state.lock().map_err(|e| e.to_string())? = Some(session);
    Ok(snap)
}

#[tauri::command]
fn get_settings(config: State<'_, Config>) -> Result<Settings, String> {
    Ok(config.settings.lock().map_err(|e| e.to_string())?.clone())
}

/// 校验并写入配置文件；工作副本相关设置从下次打开文件起生效。
#[tauri::command]
fn set_settings(config: State<'_, Config>, settings: Settings) -> Result<(), String> {
    let mut current = config.settings.lock().map_err(|e| e.to_string())?;
    settings.save(&config.dir)?;
    *current = settings;
    Ok(())
}

#[tauri::command]
async fn set_pack(state: State<'_, Sessions>, pack_id: String) -> Result<Snapshot, String> {
    with_session(&state, |s| {
        let old = std::mem::replace(&mut s.pack_id, pack_id);
        let precise = s.precise.take();
        let snap = s.snapshot();
        match &snap {
            Ok(_) => s.changed(),
            Err(_) => {
                s.pack_id = old;
                s.precise = precise;
            }
        }
        snap
    })
}

/// 只对当前副本执行 `ids` 指定的修复，压入历史；有工作副本时同步写入。
#[tauri::command]
async fn apply_fix(state: State<'_, Sessions>, ids: Vec<String>) -> Result<Snapshot, String> {
    with_session(&state, |s| {
        let next = engine::fix_selected(s.history.last().expect("history 非空"), &s.pack_id, &ids, s.precise.as_ref())?;
        s.write_work(&next)?;
        s.history.push(next);
        s.changed();
        s.snapshot()
    })
}

/// 执行一条用户确认过的修改（改文字、拆续表）；`token` 是确认时看到的 `Confirm.token`，对不上就拒绝。
#[tauri::command]
async fn apply_confirmed(state: State<'_, Sessions>, id: String, token: String) -> Result<Snapshot, String> {
    with_session(&state, |s| {
        let next = engine::apply_confirmed(s.history.last().expect("history 非空"), &s.pack_id, &id, s.precise.as_ref(), &token)?;
        s.write_work(&next)?;
        s.history.push(next);
        s.changed();
        s.snapshot()
    })
}

#[tauri::command]
async fn undo(state: State<'_, Sessions>) -> Result<Snapshot, String> {
    with_session(&state, |s| {
        if s.history.len() > 1 {
            s.write_work(&s.history[s.history.len() - 2])?;
            s.history.pop();
            s.changed();
        }
        s.snapshot()
    })
}

/// 本机可用的排版引擎（Word / WPS 文字）。
#[tauri::command]
async fn layout_engines() -> Vec<precise::EngineInfo> {
    precise::engines()
}

/// 用 `engine` 指定的 Word / WPS 对当前副本排版分页，把分页相关的检查合并进报告。
/// 排版期间副本、规则包或文件变了则丢弃结果，返回 None。
#[tauri::command]
async fn precise_check(state: State<'_, Sessions>, engine: String) -> Result<Option<Snapshot>, String> {
    let (docx, version) = with_session(&state, |s| Ok((s.history.last().expect("history 非空").clone(), s.version)))?;
    let started = std::time::Instant::now();
    let mut layout = tauri::async_runtime::spawn_blocking(move || precise::collect(&engine, &docx))
        .await
        .map_err(|e| e.to_string())??;
    layout.seconds = started.elapsed().as_secs_f64();
    with_session(&state, |s| {
        if !s.accept(version, layout) {
            return Ok(None);
        }
        s.snapshot().map(Some)
    })
}

/// 预览用 docx 字节（当前副本 + 段落书签）。
#[tauri::command]
async fn preview_doc(state: State<'_, Sessions>) -> Result<tauri::ipc::Response, String> {
    with_session(&state, |s| engine::preview(s.history.last().expect("history 非空")))
        .map(tauri::ipc::Response::new)
}

fn same_file(a: &str, b: &str) -> bool {
    a == b
        || matches!((Path::new(a).canonicalize(), Path::new(b).canonicalize()), (Ok(x), Ok(y)) if x == y)
}

/// `kind` 为 "fixed"（当前副本）或 "annotate"（当前副本的带批注版）；`out` 不能是原文件，带批注版也不能写到工作副本上。
#[tauri::command]
async fn save_copy(state: State<'_, Sessions>, kind: String, out: String) -> Result<(), String> {
    let bytes = with_session(&state, |s| {
        if same_file(&s.path, &out) {
            return Err("导出路径不能与原文件相同".to_string());
        }
        if kind == "annotate" && s.work_path.as_deref().is_some_and(|w| same_file(w, &out)) {
            return Err("带批注副本不能覆盖工作副本，请换一个文件名".to_string());
        }
        let current = s.history.last().expect("history 非空");
        match kind.as_str() {
            "fixed" => Ok(current.clone()),
            "annotate" => engine::annotate_with(current, &s.pack_id, s.precise.as_ref()),
            other => Err(format!("未知导出类型: {other}")),
        }
    })?;
    std::fs::write(&out, bytes).map_err(|e| format!("{out}: {e}"))
}

/// 配置里的默认尺寸放不进屏幕工作区（如 1080p 加 125% 缩放）时缩到工作区的 90%，居中后再显示，避免先大后小的闪烁。
fn fit_window(win: &tauri::WebviewWindow) -> tauri::Result<()> {
    let Some(monitor) = win.current_monitor()? else { return Ok(()) };
    let scale = monitor.scale_factor();
    let area = monitor.work_area().size.to_logical::<f64>(scale);
    let size = win.inner_size()?.to_logical::<f64>(scale);
    let fitted = tauri::LogicalSize::new(size.width.min(area.width * 0.9), size.height.min(area.height * 0.9));
    if fitted != size {
        win.set_size(fitted)?;
        win.center()?;
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(Sessions::default())
        .setup(|app| {
            let dir = app.path().app_config_dir()?;
            engine::set_user_dir(dir.join("packs"));
            app.manage(Config { settings: Mutex::new(Settings::load(&dir)), dir });
            if let Some(win) = app.get_webview_window("main") {
                // 调整失败也要显示窗口，否则程序看起来没启动
                let fitted = fit_window(&win);
                win.show()?;
                fitted?;
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            launch_file, list_packs, import_template, delete_pack, open_doc, set_pack, apply_fix, apply_confirmed, undo, preview_doc,
            save_copy, layout_engines, precise_check, get_settings, set_settings
        ])
        .run(tauri::generate_context!())
        .expect("启动 Tauri 失败");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> Layout {
        Layout::parse(r#"{"name":"Microsoft Word","version":"16.0","pages":1,"paras":[],"tables":[],"tocs":[]}"#).unwrap()
    }

    #[test]
    fn precise_result_is_dropped_when_copy_changes_during_layout() {
        let mut s = Session::new("a.docx".into(), "thu-master".into(), vec![]);
        let started = s.version;
        assert!(s.accept(started, layout()) && s.precise.is_some());
        // 修复 / 撤销 / 换规则包：作废并换版本，排版期间发起的结果不再被接受
        s.changed();
        assert!(s.precise.is_none() && s.version != started);
        assert!(!s.accept(started, layout()) && s.precise.is_none());
        // 换文件：新会话的版本号不会与旧会话重复
        assert_ne!(Session::new("b.docx".into(), "thu-master".into(), vec![]).version, started);
    }

    #[test]
    fn work_copy_never_overwrites_existing_files() {
        let dir = std::env::temp_dir().join(format!("thesisfmt-work-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("论文.docx");
        std::fs::write(&src, b"orig").unwrap();
        std::fs::write(dir.join("论文_工作副本.docx"), b"earlier edits").unwrap();
        let work = create_work_copy(src.to_str().unwrap(), "_工作副本", b"orig").unwrap();
        assert_eq!(Path::new(&work), dir.join("论文_工作副本(2).docx"));
        assert_eq!(std::fs::read(dir.join("论文_工作副本.docx")).unwrap(), b"earlier edits");

        // 修复 / 撤销写入的是工作副本，原文件不动
        let mut s = Session::new(src.to_str().unwrap().into(), "thu-master".into(), b"orig".to_vec());
        s.work_path = Some(work.clone());
        s.write_work(b"fixed").unwrap();
        assert_eq!(std::fs::read(&work).unwrap(), b"fixed");
        assert_eq!(std::fs::read(&src).unwrap(), b"orig");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
