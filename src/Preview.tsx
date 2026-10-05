import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { renderAsync } from "docx-preview";
import type { Finding, Severity } from "./types";

/** 联动焦点：para 为正文段落序号；src 表示由谁触发（决定哪一侧需要滚动）。 */
export type Focus = { para: number; src: "list" | "preview"; n: number };

const RANK: Severity[] = ["manual", "info", "warning", "error"];
const BOOKMARK = "_tfmt_";

/** 后端在第 N 个正文段落开头插入了书签 `_tfmt_N`，docx-preview 渲染为 `<span id>`。
 * 段内分页符会把一段拆成多个 `<p>`（可能落在下一页的 section 里）：
 * 正文层所有段落都带书签，所以没有书签的顶层 `<p>` 必是上一个带书签 `<p>` 的续段。 */
function annotate(host: HTMLElement) {
  for (const span of host.querySelectorAll<HTMLElement>(`span[id^="${BOOKMARK}"]`)) {
    const p = span.closest("p");
    if (p) p.dataset.para = span.id.slice(BOOKMARK.length);
  }
  let cur: string | undefined;
  for (const el of host.querySelectorAll<HTMLElement>("section.docx > article > *")) {
    if (!(el instanceof HTMLParagraphElement)) cur = undefined;
    else if (el.dataset.para) cur = el.dataset.para;
    else if (cur) el.dataset.para = cur;
  }
}

interface Props {
  /** 工作副本版本号；变化即重新加载预览（后端返回当前副本）。 */
  rev: number;
  findings: Finding[];
  focus: Focus | null;
  onPick: (para: number) => void;
}

export default function Preview({ rev, findings, focus, onPick }: Props) {
  const paneRef = useRef<HTMLDivElement>(null);
  const hostRef = useRef<HTMLDivElement>(null);
  const naturalWidth = useRef(0);
  const [state, setState] = useState<{ kind: "loading" } | { kind: "ready" } | { kind: "error"; text: string }>({ kind: "loading" });

  useEffect(() => {
    let dead = false;
    // 刷新时保留旧预览直到新的渲染完成，避免闪烁和滚动位置丢失
    setState((s) => (s.kind === "ready" ? s : { kind: "loading" }));
    void (async () => {
      try {
        const bytes = await invoke<ArrayBuffer>("preview_doc");
        const doc = document.createElement("div"); // 先渲染到游离节点，过期的渲染不会污染页面
        await renderAsync(bytes, doc, undefined, { inWrapper: true, breakPages: true });
        if (dead) return;
        const pane = paneRef.current!;
        const top = pane.scrollTop;
        hostRef.current!.replaceChildren(doc);
        pane.scrollTop = top;
        annotate(doc);
        naturalWidth.current = Math.max(0, ...[...doc.querySelectorAll<HTMLElement>("section.docx")].map((s) => s.offsetWidth)) + 32;
        fit();
        setState({ kind: "ready" });
      } catch (e) {
        if (!dead) setState({ kind: "error", text: `预览失败：${e instanceof Error ? e.message : String(e)}` });
      }
    })();
    return () => {
      dead = true;
    };
  }, [rev]);

  // 页面宽度超过窗格时整体缩小以适应宽度
  const fit = () => {
    const pane = paneRef.current;
    if (pane && hostRef.current && naturalWidth.current)
      hostRef.current.style.zoom = String(Math.max(0.3, Math.min(1, pane.clientWidth / naturalWidth.current)));
  };
  useEffect(() => {
    const pane = paneRef.current!;
    const ro = new ResizeObserver(fit);
    ro.observe(pane);
    return () => ro.disconnect();
  }, []);

  // 按最高严重度着色
  useEffect(() => {
    if (state.kind !== "ready") return;
    const host = hostRef.current!;
    for (const p of host.querySelectorAll<HTMLElement>("[data-para]")) p.classList.remove("sev-error", "sev-warning", "sev-info", "sev-manual");
    const top = new Map<number, Severity>();
    for (const f of findings) {
      if (f.paragraph_index == null) continue;
      const cur = top.get(f.paragraph_index);
      if (!cur || RANK.indexOf(f.severity) > RANK.indexOf(cur)) top.set(f.paragraph_index, f.severity);
    }
    for (const [para, sev] of top)
      for (const p of host.querySelectorAll<HTMLElement>(`[data-para="${para}"]`)) p.classList.add(`sev-${sev}`);
  }, [state, findings]);

  useEffect(() => {
    if (state.kind !== "ready") return;
    const host = hostRef.current!;
    for (const p of host.querySelectorAll(".tf-active")) p.classList.remove("tf-active", "tf-flash");
    if (!focus) return;
    const ps = host.querySelectorAll<HTMLElement>(`[data-para="${focus.para}"]`);
    ps.forEach((p) => p.classList.add("tf-active"));
    if (focus.src === "list" && ps[0]) {
      ps[0].scrollIntoView({ block: "center" });
      ps.forEach((p) => {
        p.classList.add("tf-flash");
        p.addEventListener("animationend", () => p.classList.remove("tf-flash"), { once: true });
      });
    }
  }, [state, focus]);

  const click = (e: React.MouseEvent) => {
    const p = (e.target as HTMLElement).closest<HTMLElement>("[data-para]");
    if (p && /\bsev-/.test(p.className)) onPick(Number(p.dataset.para));
  };

  return (
    <div className="preview" ref={paneRef}>
      {state.kind === "loading" && <p className="preview-note">正在渲染预览…</p>}
      {state.kind === "error" && <p className="preview-note error">{state.text}</p>}
      <div className="preview-host" ref={hostRef} onClick={click} />
    </div>
  );
}
