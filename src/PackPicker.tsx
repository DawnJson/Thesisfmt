import { useEffect, useMemo, useRef, useState } from "react";
import { ask, open } from "@tauri-apps/plugin-dialog";
import { ChevronDown, FileUp, Trash2 } from "lucide-react";
import type { PackMeta } from "./types";

const GROUPS = ["我导入的", "高校", "期刊", "其他"] as const;
export type PackGroup = (typeof GROUPS)[number];

export const packGroup = (p: PackMeta): PackGroup => (p.user ? "我导入的" : p.kind === "university" ? "高校" : p.kind === "journal" ? "期刊" : "其他");

/** 分组（我导入的 / 高校 / 期刊 / 其他），组内按单位、类别排序；空组不返回。 */
export function groupPacks(packs: PackMeta[]): { group: PackGroup; items: PackMeta[] }[] {
  const cmp = (a: string | undefined, b: string | undefined) => (a ?? "").localeCompare(b ?? "", "zh-CN");
  return GROUPS.map((group) => ({
    group,
    items: packs.filter((p) => packGroup(p) === group).sort((a, b) => cmp(a.org ?? a.title, b.org ?? b.title) || cmp(a.category, b.category)),
  })).filter((g) => g.items.length > 0);
}

/** 「单位 类别（版本）」，没有单位时用标题。 */
export const packLabel = (p: PackMeta) => `${p.org ? (p.category ? `${p.org} ${p.category}` : p.org) : p.title}（${p.version}）`;

const badgeOf = (p: PackMeta) =>
  p.user ? { text: "我导入的 · 未核对规范", cls: "user" } : p.verified ? { text: "已核对规范", cls: "ok" } : { text: "未核对规范", cls: "" };

function PackBadge({ pack }: { pack: PackMeta }) {
  const b = badgeOf(pack);
  return <span className={`pack-badge ${b.cls}`}>{b.text}</span>;
}

interface Props {
  packs: PackMeta[];
  packId: string;
  disabled: boolean;
  onSelect: (id: string) => void | Promise<void>;
  /** 选取模板文件后导入，成功返回新包 id，失败（已显示错误）返回 null。 */
  onImport: (path: string) => Promise<string | null>;
  /** 删除导入的模板，成功返回 true。 */
  onDelete: (id: string) => Promise<boolean>;
}

export default function PackPicker({ packs, packId, disabled, onSelect, onImport, onDelete }: Props) {
  const [opened, setOpened] = useState(false);
  const [query, setQuery] = useState("");
  const root = useRef<HTMLDivElement>(null);
  const current = packs.find((p) => p.id === packId) ?? null;

  useEffect(() => {
    if (!opened) return;
    const onDown = (e: MouseEvent) => {
      if (root.current && !root.current.contains(e.target as Node)) setOpened(false);
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setOpened(false);
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [opened]);

  useEffect(() => {
    if (disabled) setOpened(false);
  }, [disabled]);

  const groups = useMemo(() => {
    const q = query.trim().toLowerCase();
    const hit = (p: PackMeta) => !q || [p.org, p.title, p.category].some((s) => s?.toLowerCase().includes(q));
    return groupPacks(packs.filter(hit));
  }, [packs, query]);

  const toggle = () => {
    setQuery("");
    setOpened((o) => !o);
  };

  const choose = (id: string) => {
    setOpened(false);
    if (id !== packId) void onSelect(id);
  };

  const importTemplate = async () => {
    const path = await open({ filters: [{ name: "Word 模板", extensions: ["docx", "dotx"] }] });
    if (typeof path !== "string") return;
    setOpened(false);
    await onImport(path);
  };

  const remove = async (p: PackMeta) => {
    const yes = await ask(`删除导入的模板「${p.title}」？此操作不可撤销。`, { title: "删除模板", kind: "warning", okLabel: "删除", cancelLabel: "取消" });
    if (yes) await onDelete(p.id);
  };

  return (
    <div className="pack-picker" ref={root}>
      <button className="pack-trigger" onClick={toggle} disabled={disabled} aria-haspopup="listbox" aria-expanded={opened} title="检查依据的学校 / 期刊规范">
        <span className="muted">模板</span>
        {current ? (
          <>
            <span className="pack-name">{current.org ?? current.title}</span>
            {current.category && <span className="muted">{current.category}</span>}
            <span className="muted">{current.version}</span>
            <PackBadge pack={current} />
          </>
        ) : (
          <span className="muted">未选择</span>
        )}
        <ChevronDown size={14} />
      </button>
      {opened && (
        <div className="pack-panel">
          <div className="pack-panel-head">
            <input autoFocus value={query} onChange={(e) => setQuery(e.target.value)} placeholder="搜索学校、期刊或类别，如「北京」" aria-label="搜索模板" />
            <button onClick={() => void importTemplate()}>
              <FileUp size={14} />
              导入模板…
            </button>
          </div>
          <div className="pack-list" role="listbox">
            {groups.length === 0 && <div className="pack-empty muted">没有匹配的模板</div>}
            {groups.map((g) => (
              <div key={g.group}>
                <div className="pack-group">{g.group}</div>
                {g.items.map((p) => (
                  <div key={p.id} className={`pack-row${p.id === packId ? " active" : ""}`}>
                    <button className="pack-pick" role="option" aria-selected={p.id === packId} onClick={() => choose(p.id)}>
                      <span className="pack-line">
                        <span className="pack-name">{p.org ?? p.title}</span>
                        {p.category && <span className="muted">{p.category}</span>}
                        <span className="muted">{p.version}</span>
                        <PackBadge pack={p} />
                      </span>
                      {p.org && <span className="pack-title muted">{p.title}</span>}
                    </button>
                    {p.user && (
                      <button className="icon-btn" onClick={() => void remove(p)} title="删除这个导入的模板" aria-label={`删除 ${p.title}`}>
                        <Trash2 size={14} />
                      </button>
                    )}
                  </div>
                ))}
              </div>
            ))}
          </div>
          <p className="pack-note">标「已核对规范」的内置模板已对照该单位的规范逐条核对；导入的模板只照搬你模板里的格式，未对照任何规范核对。</p>
        </div>
      )}
    </div>
  );
}
