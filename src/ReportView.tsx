import { useEffect, useMemo, useRef, useState } from "react";
import { CircleCheck, PenLine, TableProperties, Wrench } from "lucide-react";
import type { Focus } from "./Preview";
import type { Category, Finding, Report, Severity, TextChange } from "./types";

const SEVERITY: { key: Severity; label: string }[] = [
  { key: "error", label: "错误" },
  { key: "warning", label: "警告" },
  { key: "info", label: "提示" },
  { key: "manual", label: "需人工确认" },
];
const SEVERITY_LABEL: Record<Severity, string> = { error: "错误", warning: "警告", info: "提示", manual: "人工确认" };

const CATEGORY_LABEL: Record<Category, string> = {
  page: "页面与页眉页脚",
  heading: "标题",
  body: "正文",
  abstract: "摘要与关键词",
  caption: "图表题注",
  reference: "参考文献",
  numbering: "编号",
  punctuation: "标点",
  table: "表格",
  figure: "图片",
  layout: "分页与目录（精确检查）",
  manual: "人工确认",
};

type GroupBy = "category" | "chapter";

function group(findings: Finding[], by: GroupBy): [string, Finding[]][] {
  const groups = new Map<string, Finding[]>();
  for (const f of findings) {
    const key = by === "category" ? CATEGORY_LABEL[f.category] : (f.location.chapter ?? "（章节之前 / 未识别）");
    groups.set(key, [...(groups.get(key) ?? []), f]);
  }
  return [...groups];
}

const SIDE: Record<string, string> = {
  top: "上边距",
  bottom: "下边距",
  left: "左边距",
  right: "右边距",
  header: "页眉距",
  footer: "页脚距",
  header_distance_cm: "页眉距",
  footer_distance_cm: "页脚距",
  width_cm: "纸张宽度",
  height_cm: "纸张高度",
};
const CM_KEYS = new Set(Object.keys(SIDE));
const LABEL: Record<string, string> = {
  ...SIDE,
  paper: "纸张",
  format: "页码格式",
  align: "对齐",
  font: "字体",
  size: "字号",
  line: "行距",
  before: "段前",
  after: "段后",
  indent: "缩进",
  max: "最多",
  min: "最少",
  count: "数量",
  number: "编号",
  separator: "分隔符",
  separators: "分隔符",
  docGrid: "文档网格",
  new_page: "另起页",
  page: "所在页",
  pages: "页数",
  max_pages: "最多",
  span: "跨页",
  repeat_header: "重复表头",
  continued_caption: "续表题注",
  toc: "目录",
  first: "例",
  cn_font: "中文字体",
  latin_font: "西文字体",
  odd_even_different: "奇偶页不同",
  styleref: "STYLEREF 样式",
  titles: "本节章标题",
  text: "内容",
  position: "位置",
  style: "样式",
  problems: "问题",
  start: "起始页码",
  page_number: "页码",
  top_bottom_pt: "顶线 / 底线（磅）",
  header_pt: "表头线（磅）",
  cells: "不符的单元格",
  layout: "排法",
  centred: "居中",
  indented: "缩进书写",
  chars: "字数",
  figure: "含图片",
  table: "含表格",
  punct: "标点",
  numbers: "序号",
  notes: "脚注条数",
  restart: "编号方式",
  same_page: "同一页",
  note_format: "脚注序号",
};
const FORMAT_CN: Record<string, string> = { upperRoman: "大写罗马", lowerRoman: "小写罗马", decimal: "阿拉伯数字" };
const ALIGN_CN: Record<string, string> = { left: "居左", center: "居中", right: "居右", justify: "两端对齐", distribute: "分散对齐" };

function value(k: string, v: unknown): string {
  if (Array.isArray(v)) return v.map((x) => value(k, x)).join("、");
  if (v == null) return "未确定";
  if (v === true) return "是";
  if (v === false) return "否";
  if (typeof v === "object" && v !== null) return JSON.stringify(v);
  if (CM_KEYS.has(k)) return `${v} cm`;
  if (k === "format") return FORMAT_CN[String(v)] ?? String(v);
  if (k === "align") return ALIGN_CN[String(v)] ?? String(v);
  if (k === "size" && typeof v === "number") return `${v} 磅`;
  if (k === "max" || k === "count") return `${v} 个`;
  if (k === "page") return `第 ${v} 页`;
  if (k === "pages" || k === "max_pages") return `${v} 页`;
  return String(v);
}

const describe = (v: Record<string, unknown>) =>
  Object.entries(v)
    .map(([k, val]) => `${LABEL[k] ?? k}：${value(k, val)}`)
    .join("；");

type Handlers = {
  onPick: (para: number) => void;
  onFix: (ids: string[]) => void;
  onConfirm: (id: string, token: string) => void;
  disabled: boolean;
};

/** 保留的长段落只显示改动处前后各 CONTEXT 个字。 */
const CONTEXT = 16;
const clip = (s: string, first: boolean, last: boolean) => {
  const n = [...s];
  if (first && n.length > CONTEXT) return "…" + n.slice(-CONTEXT).join("");
  if (last && n.length > CONTEXT) return n.slice(0, CONTEXT).join("") + "…";
  if (!first && !last && n.length > CONTEXT * 2) return n.slice(0, CONTEXT).join("") + " … " + n.slice(-CONTEXT).join("");
  return s;
};

/** 段落文字的改动对照：删去的划线标红，换成的标绿；空白显示为 ␣ 以免看漏。 */
function TextDiff({ change }: { change: TextChange }) {
  const show = (s: string) => s.replace(/ /g, "␣").replace(/\t/g, "→");
  const n = change.segs.length;
  return (
    <p className="text-diff">
      {change.segs.map((s, i) => (
        <span key={i}>
          {clip(s.keep, i === 0, i === n - 1)}
          {s.del && <del>{show(s.del)}</del>}
          {s.ins && <ins>{show(s.ins)}</ins>}
        </span>
      ))}
    </p>
  );
}

function FindingItem({ f, selected, onPick, onFix, onConfirm, disabled }: { f: Finding; selected: boolean } & Handlers) {
  const hasExpected = Object.keys(f.expected).length > 0;
  const hasActual = Object.keys(f.actual).length > 0;
  const [confirming, setConfirming] = useState(false);
  const c = f.confirm;
  // 修改或撤销后同一 id 可能换成了别的问题：收起确认面板
  useEffect(() => setConfirming(false), [c?.token]);
  return (
    <li
      className={`finding sev-${f.severity}${f.paragraph_index != null ? " locatable" : ""}${selected ? " selected" : ""}`}
      data-para={f.paragraph_index ?? undefined}
      onClick={f.paragraph_index != null ? () => onPick(f.paragraph_index!) : undefined}
    >
      <div className="finding-head">
        <span className="badge">{SEVERITY_LABEL[f.severity]}</span>
        <span className="finding-msg">{f.message}</span>
        {f.fixable && (
          <button
            className="fix-btn"
            disabled={disabled}
            title="自动修复这一项"
            onClick={(e) => {
              e.stopPropagation();
              onFix([f.id]);
            }}
          >
            <Wrench size={12} />
            修复
          </button>
        )}
        {c && !confirming && (
          <button
            className="fix-btn"
            disabled={disabled}
            title={c.kind === "text" ? "要改正文文字，先预览改动，确认后才修改" : "要改动表格结构，先看改动说明，确认后才修改"}
            onClick={(e) => {
              e.stopPropagation();
              setConfirming(true);
            }}
          >
            {c.kind === "text" ? <PenLine size={12} /> : <TableProperties size={12} />}
            {c.kind === "text" ? "改文字…" : "拆为续表…"}
          </button>
        )}
      </div>
      {c && confirming && (
        <div className="text-confirm" onClick={(e) => e.stopPropagation()}>
          <div className="text-confirm-title">{c.summary}{c.text ? "：" : ""}</div>
          {c.text && <TextDiff change={c.text} />}
          <div className="text-confirm-actions">
            <button
              className="primary"
              disabled={disabled}
              onClick={() => {
                setConfirming(false);
                onConfirm(f.id, c.token);
              }}
            >
              确认修改
            </button>
            <button onClick={() => setConfirming(false)}>取消</button>
          </div>
        </div>
      )}
      <div className="finding-meta">
        {f.location.chapter && <span>{f.location.chapter}</span>}
        {f.paragraph_index != null && <span>第 {f.paragraph_index + 1} 段</span>}
        {f.source && <span>{f.source.startsWith("规范") ? f.source : `规范 ${f.source}`}</span>}
      </div>
      {f.location.snippet && <blockquote>{f.location.snippet}</blockquote>}
      {(hasExpected || hasActual) && (
        <dl>
          {hasExpected && (
            <>
              <dt>应为</dt>
              <dd>{describe(f.expected)}</dd>
            </>
          )}
          {hasActual && (
            <>
              <dt>实际</dt>
              <dd>{describe(f.actual)}</dd>
            </>
          )}
        </dl>
      )}
    </li>
  );
}

function Group({ title, items, focus, ...h }: { title: string; items: Finding[]; focus: Focus | null } & Handlers) {
  // 改文字的修复要逐条确认，不进「修复本组」
  const fixable = items.filter((f) => f.fixable).map((f) => f.id);
  return (
    <details open={items.length <= 30}>
      <summary>
        <span className="group-title">{title}</span>
        <span className="count">{items.length}</span>
        {fixable.length > 0 && (
          <button
            className="fix-btn group-fix"
            disabled={h.disabled}
            onClick={(e) => {
              e.preventDefault();
              h.onFix(fixable);
            }}
          >
            <Wrench size={12} />
            修复本组 {fixable.length}
          </button>
        )}
      </summary>
      <ul>
        {items.map((f) => (
          <FindingItem key={f.id} f={f} selected={focus != null && focus.para === f.paragraph_index} {...h} />
        ))}
      </ul>
    </details>
  );
}

export default function ReportView({ report, focus, ...h }: { report: Report; focus: Focus | null } & Handlers) {
  const [by, setBy] = useState<GroupBy>("category");
  const [filter, setFilter] = useState<Severity | null>(null);
  const rootRef = useRef<HTMLElement>(null);
  const handled = useRef(0);
  const { manual, rest } = useMemo(
    () => ({
      manual: report.findings.filter((f) => f.severity === "manual"),
      rest: report.findings.filter((f) => f.severity !== "manual" && (!filter || f.severity === filter)),
    }),
    [report, filter],
  );

  // 预览里点段落：必要时取消筛选，展开所在分组并滚动到该段的第一条问题
  useEffect(() => {
    if (focus?.src !== "preview" || handled.current === focus.n) return;
    const hit = rootRef.current?.querySelector<HTMLElement>(`[data-para="${focus.para}"]`);
    if (!hit) {
      if (filter && report.findings.some((f) => f.paragraph_index === focus.para && f.severity !== filter)) setFilter(null);
      return;
    }
    handled.current = focus.n;
    hit.closest("details")!.open = true;
    hit.scrollIntoView({ block: "center" });
  }, [focus, filter, report]);
  const { summary } = report;
  const confirmable = report.findings.filter((f) => f.confirm).length;

  return (
    <section className="report" ref={rootRef}>
      <div className="summary">
        {SEVERITY.map(({ key, label }) => (
          <button
            key={key}
            className={`stat sev-${key}${filter === key ? " active" : ""}`}
            onClick={() => setFilter(filter === key ? null : key)}
          >
            <strong>{summary[key]}</strong>
            <span>{label}</span>
          </button>
        ))}
      </div>
      <p className="summary-note">
        共 {summary.paragraphs} 段，{summary.fixable} 项可自动修复
        {confirmable > 0 && `，${confirmable} 项可确认后修改`} · {report.pack.title}（{report.pack.version}）
        {report.precise && ` · 排版依据 ${report.precise.engine}，共 ${report.precise.pages} 页，用时 ${report.precise.seconds.toFixed(1)} 秒`}
      </p>

      {report.findings.length === 0 && (
        <p className="all-ok">
          <CircleCheck size={16} />
          没有发现格式问题。
        </p>
      )}

      {rest.length > 0 && (
        <div className="groups">
          <div className="groupby">
            <span className="muted">分组</span>
            <div className="segmented" role="radiogroup">
              <button role="radio" aria-checked={by === "category"} className={by === "category" ? "on" : ""} onClick={() => setBy("category")}>
                按类别
              </button>
              <button role="radio" aria-checked={by === "chapter"} className={by === "chapter" ? "on" : ""} onClick={() => setBy("chapter")}>
                按章节
              </button>
            </div>
            {filter && (
              <button className="link" onClick={() => setFilter(null)}>
                清除筛选
              </button>
            )}
          </div>
          {group(rest, by).map(([title, items]) => (
            <Group key={title} title={title} items={items} focus={focus} {...h} />
          ))}
        </div>
      )}

      {manual.length > 0 && (
        <div className="groups manual">
          <h3>需人工确认</h3>
          <Group title="以下事项无法自动判断，请自行核对" items={manual} focus={focus} {...h} />
        </div>
      )}
    </section>
  );
}
