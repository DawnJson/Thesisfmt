//! 用户设置：保存在应用配置目录的 `settings.json`；文件缺失或损坏时用默认值。

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// 打开文件时在原文件旁创建工作副本（`原名 + work_copy_suffix`），修复 / 撤销后自动写入该副本。
    pub work_copy: bool,
    pub work_copy_suffix: String,
    /// 「保存修改后的副本」「保存带批注副本」默认文件名的后缀。
    pub fixed_suffix: String,
    pub annotate_suffix: String,
    /// 打开文件后自动用 Word / WPS 做一次精确检查。
    pub auto_precise: bool,
    /// 启动时选用的规则包 / 排版程序；None 或已不存在时用内置默认。
    pub pack: Option<String>,
    pub engine: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            work_copy: false,
            work_copy_suffix: "_工作副本".into(),
            fixed_suffix: "_修复".into(),
            annotate_suffix: "_批注".into(),
            auto_precise: true,
            pack: None,
            engine: None,
        }
    }
}

const FILE: &str = "settings.json";

/// 后缀会拼进文件名：不能为空（否则与原文件同名），不能含路径分隔符或 Windows 文件名非法字符。
fn check_suffix(what: &str, s: &str) -> Result<(), String> {
    if s.trim().is_empty() {
        return Err(format!("{what}不能为空"));
    }
    if let Some(c) = s.chars().find(|c| matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control()) {
        return Err(format!("{what}不能包含字符 {c:?}"));
    }
    Ok(())
}

impl Settings {
    pub fn validate(&self) -> Result<(), String> {
        check_suffix("工作副本后缀", &self.work_copy_suffix)?;
        check_suffix("修改后副本后缀", &self.fixed_suffix)?;
        check_suffix("批注副本后缀", &self.annotate_suffix)
    }

    /// 读不到、解析失败或校验不过都退回默认值：设置坏了不应让程序起不来。
    pub fn load(dir: &Path) -> Settings {
        std::fs::read(dir.join(FILE))
            .ok()
            .and_then(|b| serde_json::from_slice::<Settings>(&b).ok())
            .filter(|s| s.validate().is_ok())
            .unwrap_or_default()
    }

    pub fn save(&self, dir: &Path) -> Result<(), String> {
        self.validate()?;
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let path = dir.join(FILE);
        let json = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, json).map_err(|e| format!("{}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffix_that_would_hit_original_or_escape_directory_is_rejected() {
        let with = |s: &str| Settings { work_copy_suffix: s.into(), ..Settings::default() };
        assert!(with("  ").validate().is_err());
        assert!(with("/../x").validate().is_err());
        assert!(with("a:b").validate().is_err());
        assert!(with("_备份 1").validate().is_ok());
    }

    #[test]
    fn missing_fields_take_defaults_and_bad_file_falls_back() {
        let dir = std::env::temp_dir().join(format!("thesisfmt-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(FILE), r#"{"work_copy":true}"#).unwrap();
        let s = Settings::load(&dir);
        assert!(s.work_copy && s.auto_precise && s.fixed_suffix == "_修复");
        std::fs::write(dir.join(FILE), r#"{"work_copy_suffix":""}"#).unwrap();
        assert_eq!(Settings::load(&dir), Settings::default());
        std::fs::write(dir.join(FILE), "not json").unwrap();
        assert_eq!(Settings::load(&dir), Settings::default());
        let custom = Settings { engine: Some("wps".into()), ..Settings::default() };
        custom.save(&dir).unwrap();
        assert_eq!(Settings::load(&dir), custom);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
