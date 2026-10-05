import { useEffect, useState, type ReactNode } from "react";
import { ArrowLeft, BookOpenCheck, Copy, FileOutput, MessageSquareText, MonitorCog, ScanSearch, ShieldCheck } from "lucide-react";
import { groupPacks, packLabel } from "./PackPicker";
import type { LayoutEngine, PackMeta, Settings } from "./types";

interface Props {
  settings: Settings;
  packs: PackMeta[];
  /** null：还没检测完。 */
  engines: LayoutEngine[] | null;
  /** 保存成功返回 null，失败返回错误信息。 */
  onSave: (s: Settings) => Promise<string | null>;
  onClose: () => void;
}

type SuffixKey = "work_copy_suffix" | "fixed_suffix" | "annotate_suffix";

/** 一项设置：左侧图标、标题、说明，右侧控件（Windows 设置页的 SettingsCard 布局）。 */
function Card({ icon, title, desc, error, sub, disabled, children }: {
  icon?: ReactNode; title: string; desc?: ReactNode; error?: string; sub?: boolean; disabled?: boolean; children: ReactNode;
}) {
  return (
    <div className={`setting-card${sub ? " sub" : ""}${disabled ? " disabled" : ""}`}>
      {icon && <span className="setting-icon">{icon}</span>}
      <div className="setting-text">
        <div className="setting-title">{title}</div>
        {desc && <div className="setting-desc">{desc}</div>}
        {error && <div className="setting-error">{error}</div>}
      </div>
      <div className="setting-control">{children}</div>
    </div>
  );
}

function Toggle({ checked, onChange, label }: { checked: boolean; onChange: (v: boolean) => void; label: string }) {
  return (
    <button type="button" role="switch" aria-checked={checked} aria-label={label} className="toggle" onClick={() => onChange(!checked)}>
      <span className="toggle-text">{checked ? "开" : "关"}</span>
      <span className="toggle-track">
        <span className="toggle-thumb" />
      </span>
    </button>
  );
}

/** 文本框失焦或回车时才提交；校验失败保留输入并显示错误。 */
function SuffixInput({ value, disabled, label, onCommit }: { value: string; disabled?: boolean; label: string; onCommit: (v: string) => void }) {
  const [draft, setDraft] = useState(value);
  useEffect(() => setDraft(value), [value]);
  return (
    <span className="suffix-input">
      <input
        value={draft}
        disabled={disabled}
        aria-label={label}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={() => onCommit(draft)}
        onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
      />
      <small className="muted">论文{draft}.docx</small>
    </span>
  );
}

export default function SettingsPage({ settings, packs, engines, onSave, onClose }: Props) {
  const [error, setError] = useState<{ key: keyof Settings; text: string } | null>(null);

  // 改动立即生效，不需要「保存」按钮；改回当前值时只清掉之前的错误
  const set = async <K extends keyof Settings>(key: K, value: Settings[K]) => {
    const err = settings[key] === value ? null : await onSave({ ...settings, [key]: value });
    setError((prev) => (err ? { key, text: err } : prev?.key === key ? null : prev));
  };
  const errorOf = (key: keyof Settings) => (error?.key === key ? error.text : undefined);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const suffix = (key: SuffixKey, label: string, disabled = false) => (
    <SuffixInput value={settings[key]} disabled={disabled} label={label} onCommit={(v) => void set(key, v)} />
  );

  return (
    <div className="settings-page">
      <header className="settings-head">
        <button className="icon-btn" onClick={onClose} title="返回（Esc）" aria-label="返回">
          <ArrowLeft size={18} />
        </button>
        <h1>设置</h1>
      </header>
      <div className="settings-body">
        <h2>文件</h2>
        <Card
          icon={<Copy size={20} />}
          title="工作副本"
          desc="打开文件时在原文件同目录新建一份副本，修复和撤销会自动写入它，不用再手动保存。同名文件已存在时自动编号，不会覆盖。下次打开文件时生效。"
        >
          <Toggle checked={settings.work_copy} onChange={(v) => void set("work_copy", v)} label="工作副本" />
        </Card>
        <Card sub title="工作副本文件名后缀" disabled={!settings.work_copy} error={errorOf("work_copy_suffix")}>
          {suffix("work_copy_suffix", "工作副本文件名后缀", !settings.work_copy)}
        </Card>

        <h2>导出</h2>
        <Card icon={<FileOutput size={20} />} title="修改后副本的文件名后缀" desc="「保存修改后的副本」默认文件名" error={errorOf("fixed_suffix")}>
          {suffix("fixed_suffix", "修改后副本的文件名后缀")}
        </Card>
        <Card icon={<MessageSquareText size={20} />} title="批注副本的文件名后缀" desc="「保存带批注副本」默认文件名" error={errorOf("annotate_suffix")}>
          {suffix("annotate_suffix", "批注副本的文件名后缀")}
        </Card>

        <h2>检查</h2>
        <Card icon={<BookOpenCheck size={20} />} title="默认规则包" desc="启动时选用的学校规范" error={errorOf("pack")}>
          <select value={settings.pack ?? ""} onChange={(e) => void set("pack", e.target.value || null)}>
            <option value="">默认（清华大学硕士，没有则列表第一个）</option>
            {groupPacks(packs).map((g) => (
              <optgroup key={g.group} label={g.group}>
                {g.items.map((p) => (
                  <option key={p.id} value={p.id}>
                    {packLabel(p)}
                  </option>
                ))}
              </optgroup>
            ))}
          </select>
        </Card>
        <Card
          icon={<MonitorCog size={20} />}
          title="精确检查使用"
          desc={engines?.length === 0 ? "需要本机安装 Microsoft Word 或 WPS 文字" : "用哪个程序排版分页，检查跨页表格、目录页码和章节起页"}
          disabled={!engines?.length}
          error={errorOf("engine")}
        >
          <select value={settings.engine ?? ""} onChange={(e) => void set("engine", e.target.value || null)} disabled={!engines?.length}>
            <option value="">自动（优先 Word）</option>
            {engines?.map((e) => (
              <option key={e.id} value={e.id}>
                {e.name}
              </option>
            ))}
          </select>
        </Card>
        <Card icon={<ScanSearch size={20} />} title="打开文件后自动精确检查" desc="会在后台启动 Word / WPS，较慢；关闭后可随时手动点「精确检查」" error={errorOf("auto_precise")}>
          <Toggle checked={settings.auto_precise} onChange={(v) => void set("auto_precise", v)} label="打开文件后自动精确检查" />
        </Card>

        <h2>关于</h2>
        <Card icon={<ShieldCheck size={20} />} title="隐私" desc="论文与设置都只保存在本机。程序不联网、没有统计、不上传任何内容。">
          {null}
        </Card>
      </div>
    </div>
  );
}
