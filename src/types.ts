export type Severity = "error" | "warning" | "info" | "manual";
export type Category =
  | "page" | "heading" | "body" | "abstract" | "caption" | "reference" | "numbering" | "punctuation" | "table" | "figure" | "layout" | "manual";

export interface PackMeta {
  id: string;
  title: string;
  version: string;
  source: string;
  valid_from?: string;
  /** 学校 / 期刊名。 */
  org?: string;
  kind?: "university" | "journal";
  /** 研究生 / 本科 / 投稿 等。 */
  category?: string;
  /** 内置包：已对照该单位规范核对。 */
  verified: boolean;
  /** 用户从自己的模板导入（未核对规范）；内置包没有此字段。 */
  user?: true;
}

export interface Finding {
  id: string;
  rule_id: string;
  severity: Severity;
  category: Category;
  role: string | null;
  paragraph_index: number | null;
  location: { chapter: string | null; snippet: string };
  message: string;
  expected: Record<string, unknown>;
  actual: Record<string, unknown>;
  fixable: boolean;
  source: string;
  /** 要改正文文字或结构的修复（改文字、拆续表）：只能逐条确认后执行，不计入 fixable、不进「全部修复」「修复本组」。 */
  confirm?: Confirm;
}

export interface Confirm {
  kind: "text" | "split_table";
  /** 改动说明。 */
  summary: string;
  /** 改段落文字时的改前 / 改后对照。 */
  text?: TextChange;
  /** 原样传回 `apply_confirmed`；文档变了后端会拒绝。 */
  token: string;
}

/** 段落文字改前 / 改后，`segs` 依次为「保留 → 删去 → 换成」。 */
export interface TextChange {
  before: string;
  after: string;
  segs: { keep: string; del: string; ins: string }[];
}

export interface Report {
  pack: PackMeta;
  summary: Record<Severity | "paragraphs" | "fixable", number>;
  findings: Finding[];
  /** 已对副本做的修改步数（可撤销步数）。 */
  steps: number;
  roles: { index: number; role: string | null; ambiguous: boolean; text: string }[];
  /** 合并了精确检查时的排版依据；没跑或已作废为 null。 */
  precise: PreciseInfo | null;
  /** 自动写入的工作副本路径；设置里未开启时为 null。 */
  work_path: string | null;
}

/** 用户设置，字段与后端 `settings.rs` 一致。 */
export interface Settings {
  /** 打开文件时在原文件旁创建工作副本，修复 / 撤销后自动写入。 */
  work_copy: boolean;
  work_copy_suffix: string;
  fixed_suffix: string;
  annotate_suffix: string;
  /** 打开文件后自动精确检查。 */
  auto_precise: boolean;
  /** 启动时选用的规则包 / 排版程序；null 用内置默认。 */
  pack: string | null;
  engine: string | null;
}

export interface PreciseInfo {
  engine: string;
  pages: number;
  seconds: number;
}

/** 本机可用的排版引擎（Microsoft Word / WPS 文字）。 */
export interface LayoutEngine {
  id: string;
  name: string;
}
