import { ask, open, save } from "@tauri-apps/plugin-dialog";

export const baseName = (path: string) => path.split(/[\\/]/).pop() ?? path;

export const pickDocx = async () => {
  const path = await open({ filters: [{ name: "Word 文档", extensions: ["docx"] }] });
  return typeof path === "string" ? path : null;
};

/** 默认同目录、原文件名加后缀。 */
export const pickSavePath = (src: string, suffix: string) =>
  save({
    defaultPath: src.replace(/\.docx$/i, "") + suffix + ".docx",
    filters: [{ name: "Word 文档", extensions: ["docx"] }],
  });

/** 有未保存的修改时，继续操作前确认丢弃。 */
export const confirmDiscard = (what: string) =>
  ask(`当前有尚未保存的修改，${what}会丢弃这些修改。继续吗？`, { title: "尚未保存", kind: "warning", okLabel: "继续", cancelLabel: "取消" });
