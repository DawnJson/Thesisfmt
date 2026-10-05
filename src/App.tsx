import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import {
  ArrowRight, BookOpenCheck, CircleAlert, CircleCheck, FileText, FileUp, FolderOpen, Info, ListChecks, LoaderCircle,
  MessageSquareText, Save, ScanSearch, Settings as SettingsIcon, ShieldCheck, TriangleAlert, Undo2, WandSparkles,
} from "lucide-react";
import { baseName, confirmDiscard, pickDocx, pickSavePath } from "./platform";
import Preview, { type Focus } from "./Preview";
import ReportView from "./ReportView";
import SettingsPage from "./SettingsPage";
import PackPicker from "./PackPicker";
import type { LayoutEngine, PackMeta, Report, Settings } from "./types";

type Status = { kind: "idle" } | { kind: "busy"; text: string } | { kind: "error"; text: string };
/** 精确检查（用 Word / WPS 排版分页）的状态；结果本身在 report.precise。 */
type Precise = { kind: "idle" } | { kind: "running"; name: string } | { kind: "stale" } | { kind: "error"; text: string };

const errText = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** 已保存的默认包优先，其次清华硕士（thu-master），最后列表第一个。 */
const defaultPack = (list: PackMeta[], saved: string | null | undefined) =>
  (list.find((p) => p.id === saved) ?? list.find((p) => p.id === "thu-master") ?? list[0])?.id ?? "";

export default function App() {
  const [packs, setPacks] = useState<PackMeta[]>([]);
  const [packId, setPackId] = useState("");
  const [path, setPath] = useState("");
  const [report, setReport] = useState<Report | null>(null);
  const [status, setStatus] = useState<Status>({ kind: "idle" });
  const [notice, setNotice] = useState("");
  const [dragging, setDragging] = useState(false);
  const [leftW, setLeftW] = useState(420);
  const [focus, setFocus] = useState<Focus | null>(null);
  const [rev, setRev] = useState(0); // 副本内容每变一次 +1，预览据此重新加载
  const [savedSteps, setSavedSteps] = useState(0);
  const [engines, setEngines] = useState<LayoutEngine[] | null>(null); // null：还没检测完
  const [engineId, setEngineId] = useState("");
  const [precise, setPrecise] = useState<Precise>({ kind: "idle" });
  const [settings, setSettings] = useState<Settings | null>(null); // null：还没读到
  const [showSettings, setShowSettings] = useState(false);
  const preciseRun = useRef(0); // 每次开始或作废 +1，过期的返回直接丢弃
  const ready = packs.length > 0 && settings !== null;
  const engine = engines?.find((e) => e.id === engineId) ?? null;
  const engineRef = useRef(engine);
  engineRef.current = engine;
  const settingsRef = useRef(settings);
  settingsRef.current = settings;
  const running = precise.kind === "running";

  /** 成功返回 `{ value }`（命令无返回值时 value 为 null），失败返回 null 并显示错误。 */
  const call = useCallback(async <T = Report,>(cmd: string, args: Record<string, unknown>, busy: string, fail: string) => {
    setNotice("");
    setStatus({ kind: "busy", text: busy });
    try {
      const value = await invoke<T>(cmd, args);
      setStatus({ kind: "idle" });
      return { value };
    } catch (e) {
      setStatus({ kind: "error", text: `${fail}：${errText(e)}` });
      return null;
    }
  }, []);

  const runPrecise = useCallback(async (eng: LayoutEngine) => {
    const run = ++preciseRun.current;
    setNotice("");
    setPrecise({ kind: "running", name: eng.name });
    try {
      const snap = await invoke<Report | null>("precise_check", { engine: eng.id });
      if (run !== preciseRun.current) return;
      if (snap) {
        setReport(snap); // 副本没变，预览不用重载
        setPrecise({ kind: "idle" });
        setNotice(`排版依据：${snap.precise?.engine}，共 ${snap.precise?.pages} 页`);
      } else {
        setPrecise({ kind: "stale" });
      }
    } catch (e) {
      if (run === preciseRun.current) setPrecise({ kind: "error", text: errText(e) });
    }
  }, []);

  /** 副本或规则包变了：运行中的精确检查丢弃，已有的结果后端已作废，提示可重新检查。 */
  const invalidatePrecise = () => {
    preciseRun.current++;
    setPrecise(report?.precise || precise.kind !== "idle" ? { kind: "stale" } : { kind: "idle" });
  };

  const runOpen = useCallback(
    async (file: string, pack: string) => {
      preciseRun.current++;
      setPrecise({ kind: "idle" });
      setReport(null);
      setPath(file);
      setFocus(null);
      const r = await call("open_doc", { path: file, packId: pack }, "正在检查…", "检查失败");
      if (r) {
        setReport(r.value);
        setRev((n) => n + 1);
        setSavedSteps(0);
        if (engineRef.current && settingsRef.current?.auto_precise !== false) void runPrecise(engineRef.current);
      }
    },
    [call, runPrecise],
  );

  const accept = useCallback(
    (file: string, pack: string) => {
      const lower = file.toLowerCase();
      if (lower.endsWith(".doc")) {
        setStatus({ kind: "error", text: "不支持 .doc 旧格式。请用 Word 打开该文件，选择「文件 → 另存为 → Word 文档 (*.docx)」后再拖入。" });
        return;
      }
      if (!lower.endsWith(".docx")) {
        setStatus({ kind: "error", text: "只支持 .docx 文件。" });
        return;
      }
      void runOpen(file, pack);
    },
    [runOpen],
  );

  // 开了工作副本时每步修改都已写盘，不存在未保存的修改
  const dirty = !report?.work_path && (report?.steps ?? 0) !== savedSteps;

  // 事件回调里要读最新的 accept / packId / busy，用 ref 避免反复订阅
  const latest = useRef({ accept, packId, busy: false, dirty: false, settingsOpen: false });
  latest.current = { accept, packId, busy: status.kind === "busy", dirty, settingsOpen: showSettings };

  useEffect(() => {
    void (async () => {
      try {
        const [list, saved] = await Promise.all([invoke<PackMeta[]>("list_packs"), invoke<Settings>("get_settings")]);
        setPacks(list);
        setSettings(saved);
        const id = defaultPack(list, saved.pack);
        setPackId(id);
        const found = await invoke<LayoutEngine[]>("layout_engines");
        setEngines(found);
        setEngineId((found.find((e) => e.id === saved.engine) ?? found.find((e) => e.id === "word") ?? found[0])?.id ?? "");
        const launched = await invoke<string | null>("launch_file");
        if (launched && id) accept(launched, id);
      } catch (e) {
        setStatus({ kind: "error", text: `启动失败：${errText(e)}` });
      }
    })();
    // 只在启动时执行一次
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const unlisten = getCurrentWebview().onDragDropEvent((e) => {
      const p = e.payload;
      if (p.type === "enter" || p.type === "over") setDragging(true);
      else if (p.type === "leave") setDragging(false);
      else {
        setDragging(false);
        const { accept, packId, busy, dirty, settingsOpen } = latest.current;
        if (p.paths[0] && packId && !busy && !settingsOpen) void (async () => (!dirty || (await confirmDiscard("打开新文件"))) && accept(p.paths[0], packId))();
      }
    });
    return () => void unlisten.then((f) => f());
  }, []);

  const pick = async () => {
    const file = await pickDocx();
    if (file && (!dirty || (await confirmDiscard("打开新文件")))) accept(file, packId);
  };

  // 切换规则包只重新检查，不动工作副本，不会丢失已做的修改，所以无需确认；还没打开文件时只记下选择
  const changePack = async (id: string) => {
    if (!report) {
      setPackId(id);
      return;
    }
    const r = await call("set_pack", { packId: id }, "正在检查…", "检查失败");
    if (r) {
      setPackId(id);
      setReport(r.value);
      invalidatePrecise();
    }
  };

  /** 刷新模板列表并返回新列表。 */
  const refreshPacks = async () => {
    const list = await invoke<PackMeta[]>("list_packs");
    setPacks(list);
    return list;
  };

  const importTemplate = async (file: string) => {
    const added = await call<PackMeta>("import_template", { path: file }, "正在导入模板…", "导入模板失败");
    if (!added) return null;
    try {
      await refreshPacks();
    } catch (e) {
      setStatus({ kind: "error", text: `刷新模板列表失败：${errText(e)}` });
      return null;
    }
    await changePack(added.value.id);
    return added.value.id;
  };

  const deletePack = async (id: string) => {
    const done = await call<null>("delete_pack", { id }, "正在删除模板…", "删除模板失败");
    if (!done) return false;
    try {
      const list = await refreshPacks();
      if (id === packId) {
        const fallback = defaultPack(list, settingsRef.current?.pack);
        if (fallback) await changePack(fallback);
        else setPackId("");
      }
    } catch (e) {
      setStatus({ kind: "error", text: `刷新模板列表失败：${errText(e)}` });
    }
    return true;
  };

  const update = async (cmd: string, args: Record<string, unknown>, busy: string, fail: string) => {
    const r = await call(cmd, args, busy, fail);
    if (r) {
      setReport(r.value);
      setRev((n) => n + 1);
      invalidatePrecise();
    }
  };
  const fix = (ids: string[]) => update("apply_fix", { ids }, "正在修复…", "修复失败");
  const confirmFix = (id: string, token: string) => update("apply_confirmed", { id, token }, "正在修改…", "修改失败");
  const undo = () => update("undo", {}, "正在撤销…", "撤销失败");

  const save = async (kind: "fixed" | "annotate") => {
    const label = kind === "annotate" ? "带批注副本" : "修改后的副本";
    const out = await pickSavePath(path, kind === "annotate" ? settings!.annotate_suffix : settings!.fixed_suffix);
    if (!out) return;
    const ok = await call<null>("save_copy", { kind, out }, `正在生成${label}…`, `保存${label}失败`);
    if (ok) {
      setSavedSteps(report?.steps ?? 0);
      setNotice(`已保存到 ${out}。原文件未改动。`);
    }
  };

  /** 成功返回 null；默认排版程序立即切换，默认规则包仅在没有打开文件时立即切换（避免重新检查）。 */
  const saveSettings = async (next: Settings) => {
    try {
      await invoke("set_settings", { settings: next });
    } catch (e) {
      return errText(e);
    }
    setSettings(next);
    if (next.engine && !running && engines?.some((e) => e.id === next.engine)) setEngineId(next.engine);
    if (!report && next.pack && packs.some((p) => p.id === next.pack)) setPackId(next.pack);
    return null;
  };

  const busy = status.kind === "busy";

  const locate = (para: number, src: Focus["src"]) => setFocus((f) => ({ para, src, n: (f?.n ?? 0) + 1 }));

  // 拖动分隔条调整左栏宽度
  const drag = useRef<{ x: number; w: number } | null>(null);
  const onDividerMove = (e: React.PointerEvent) => {
    if (drag.current) setLeftW(Math.max(320, Math.min(window.innerWidth - 360, drag.current.w + e.clientX - drag.current.x)));
  };

  return (
    <div className="app">
      <header className="appbar">
        <div className="brand">
          <span className="brand-mark">
            <BookOpenCheck size={16} />
          </span>
          论文格式体检
        </div>
        {path && (
          <div className="doc-chip" title={report?.work_path ? `原文件：${path}\n工作副本：${report.work_path}` : path}>
            <FileText size={14} />
            <span className="doc-name">{baseName(path)}</span>
            {report?.work_path && (
              <>
                <ArrowRight size={12} className="muted" />
                <span className="work-tag">工作副本 {baseName(report.work_path)}</span>
              </>
            )}
            {dirty && <span className="dirty-dot" title="有尚未保存的修改" />}
          </div>
        )}
        <span className="spacer" />
        <PackPicker packs={packs} packId={packId} disabled={!ready || busy} onSelect={changePack} onImport={importTemplate} onDelete={deletePack} />
        <div className="privacy" tabIndex={0}>
          <ShieldCheck size={14} />
          本机处理
          <div className="privacy-full">
            <strong>你的论文不会离开你的电脑</strong>
            文件只在本机处理：程序不联网、没有统计、不上传任何内容。想自己验证？断开网络再检查一份文档即可。
          </div>
        </div>
        <button className="icon-btn" disabled={!settings} onClick={() => setShowSettings(true)} title="设置" aria-label="设置">
          <SettingsIcon size={18} />
        </button>
      </header>

      <nav className="commandbar">
        <button disabled={!ready || busy} onClick={() => void pick()}>
          <FolderOpen size={16} />
          打开
        </button>
        <span className="sep" />
        <button disabled={!report || busy || running || report.summary.fixable === 0} onClick={() => void fix(report!.findings.filter((f) => f.fixable).map((f) => f.id))}>
          <WandSparkles size={16} />
          全部修复
          {!!report?.summary.fixable && <span className="pill">{report.summary.fixable}</span>}
        </button>
        <button disabled={!report || busy || running || report.steps === 0} onClick={() => void undo()} title="撤销上一步修复">
          <Undo2 size={16} />
          撤销
        </button>
        <span className="sep" />
        <div className="engine">
          <button
            disabled={!report || busy || running || !engine}
            title={engine ? `用 ${engine.name} 真正排版，检查分页、目录和页码` : "需要本机安装 Microsoft Word 或 WPS"}
            onClick={() => engine && void runPrecise(engine)}
          >
            <ScanSearch size={16} />
            精确检查
          </button>
          {engines && engines.length > 1 && (
            <select value={engineId} onChange={(e) => setEngineId(e.target.value)} disabled={running} title="用于排版分页的程序">
              {engines.map((e) => (
                <option key={e.id} value={e.id}>
                  {e.name}
                </option>
              ))}
            </select>
          )}
          {engines?.length === 0 && <small className="muted">需要本机安装 Word 或 WPS</small>}
        </div>
        <span className="spacer" />
        <button disabled={!report || busy} onClick={() => void save("fixed")} title="把当前修改另存为新文件">
          <Save size={16} />
          保存修改后的副本
        </button>
        <button className="primary" disabled={!report || busy} onClick={() => void save("annotate")} title="在副本中把问题写成 Word 批注">
          <MessageSquareText size={16} />
          保存带批注副本
        </button>
      </nav>

      <div className="infobars">
        {status.kind !== "idle" && (
          <p className={`infobar ${status.kind === "error" ? "error" : "info"}`}>
            {status.kind === "busy" ? <LoaderCircle size={16} className="spin" /> : <CircleAlert size={16} />}
            {status.text}
          </p>
        )}
        {precise.kind === "running" && (
          <p className="infobar info">
            <LoaderCircle size={16} className="spin" />
            正在用 {precise.name} 排版分页…
          </p>
        )}
        {precise.kind === "stale" && (
          <p className="infobar warning">
            <TriangleAlert size={16} />
            文档已修改，分页结果已失效。
            {engine && (
              <button disabled={busy} onClick={() => void runPrecise(engine)}>
                重新精确检查
              </button>
            )}
          </p>
        )}
        {precise.kind === "error" && (
          <p className="infobar error">
            <CircleAlert size={16} />
            精确检查失败：{precise.text}
            {engine && (
              <button disabled={busy} onClick={() => void runPrecise(engine)}>
                重试
              </button>
            )}
          </p>
        )}
        {notice && (
          <p className="infobar ok">
            <CircleCheck size={16} />
            {notice}
          </p>
        )}
        {report && report.steps > 0 && status.kind === "idle" && !notice && (
          <p className={`infobar ${dirty ? "warning" : "info"}`}>
            <Info size={16} />
            {report.work_path
              ? `已修改 ${report.steps} 步，已自动写入工作副本；原文件未被修改。`
              : dirty
                ? `已修改 ${report.steps} 步，尚未保存；原文件不会被修改。`
                : `已修改 ${report.steps} 步，已保存副本；原文件未被修改。`}
          </p>
        )}
      </div>

      {path ? (
        <main className="split" style={{ gridTemplateColumns: `${leftW}px 5px 1fr` }}>
          <aside className="pane left">
            {report ? (
              <ReportView
                report={report}
                focus={focus}
                onPick={(p) => locate(p, "list")}
                onFix={(ids) => void fix(ids)}
                onConfirm={(id, token) => void confirmFix(id, token)}
                disabled={busy || running}
              />
            ) : (
              <p className="placeholder">检查结果会显示在这里</p>
            )}
          </aside>
          <div
            className="divider"
            onPointerDown={(e) => {
              e.currentTarget.setPointerCapture(e.pointerId);
              drag.current = { x: e.clientX, w: leftW };
            }}
            onPointerMove={onDividerMove}
            onPointerUp={() => (drag.current = null)}
          />
          <section className="pane right">
            {report && <Preview rev={rev} findings={report.findings} focus={focus} onPick={(p) => locate(p, "preview")} />}
          </section>
        </main>
      ) : (
        <main className="welcome">
          <div className="dropzone">
            <span className="dropzone-icon">
              <FileUp size={28} />
            </span>
            <h2>把论文 .docx 拖到这里</h2>
            <p className="muted">或者</p>
            <button className="primary large" disabled={!ready || busy} onClick={() => void pick()}>
              <FolderOpen size={16} />
              选择文件
            </button>
            <ul className="welcome-points">
              <li>
                <ListChecks size={14} />
                按学校规范逐项检查页面、标题、正文、图表、参考文献
              </li>
              <li>
                <WandSparkles size={14} />
                大部分格式问题可一键修复，随时撤销
              </li>
              <li>
                <ShieldCheck size={14} />
                原文件不会被修改，结果另存为副本
              </li>
            </ul>
          </div>
        </main>
      )}

      {dragging && !showSettings && (
        <div className="drag-overlay">
          <FileUp size={32} />
          松开鼠标以打开论文
        </div>
      )}
      {showSettings && settings && (
        <SettingsPage settings={settings} packs={packs} engines={engines} onSave={saveSettings} onClose={() => setShowSettings(false)} />
      )}
    </div>
  );
}
