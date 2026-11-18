//! Persistent project list, stored as JSON in the user's config directory.
//!
//! Only project paths and the last-used tool are stored. Nothing about a tool's
//! conversations, credentials or environment is ever written here.

use std::path::PathBuf;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Project {
    pub path: PathBuf,
    pub last_tool: Option<String>,
}

#[derive(Debug, Default)]
pub struct ProjectStore {
    pub projects: Vec<Project>,
}

fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))?;
    Some(base.join("agentterm").join("projects.json"))
}

impl ProjectStore {
    pub fn load() -> Self {
        let Some(path) = config_path() else {
            return Self::default();
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        let Ok(data) = serde_json::from_str::<serde_json::Value>(&text) else {
            return Self::default();
        };

        let projects = data
            .get("projects")
            .and_then(|value| value.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let path = item.get("path")?.as_str()?;
                        let last_tool = item
                            .get("last_tool")
                            .and_then(|value| value.as_str())
                            .map(str::to_string);
                        Some(Project {
                            path: PathBuf::from(path),
                            last_tool,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        Self { projects }
    }

    pub fn save(&self) -> Result<(), String> {
        let Some(path) = config_path() else {
            return Err("无法定位配置目录".to_string());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("无法创建配置目录：{e}"))?;
        }

        let projects: Vec<serde_json::Value> = self
            .projects
            .iter()
            .map(|project| {
                serde_json::json!({
                    "path": project.path.to_string_lossy(),
                    "last_tool": project.last_tool,
                })
            })
            .collect();
        let body = serde_json::json!({ "version": 1, "projects": projects });
        let text = serde_json::to_string_pretty(&body).map_err(|e| format!("无法编码配置：{e}"))?;

        // Write to a temp file then rename, so a crash never corrupts the list.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| format!("无法写入配置：{e}"))?;
        std::fs::rename(&tmp, &path).map_err(|e| format!("无法保存配置：{e}"))?;
        Ok(())
    }

    pub fn add(&mut self, raw: &str) -> Result<(), String> {
        let path = canonical_project(raw)?;
        if self.projects.iter().any(|p| p.path == path) {
            return Err("这个项目已经在列表里了".to_string());
        }
        self.projects.push(Project {
            path,
            last_tool: None,
        });
        Ok(())
    }

    pub fn remove(&mut self, index: usize) -> Option<Project> {
        if index < self.projects.len() {
            // Only the record is dropped; the folder on disk is never touched.
            Some(self.projects.remove(index))
        } else {
            None
        }
    }

    pub fn set_last_tool(&mut self, index: usize, tool: &str) {
        if let Some(project) = self.projects.get_mut(index) {
            project.last_tool = Some(tool.to_string());
        }
    }
}

pub fn canonical_project(raw: &str) -> Result<PathBuf, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("路径不能为空".to_string());
    }
    let expanded = if let Some(rest) = trimmed.strip_prefix("~/") {
        let home = std::env::var_os("HOME").ok_or("无法读取 HOME")?;
        PathBuf::from(home).join(rest)
    } else {
        PathBuf::from(trimmed)
    };
    let canonical =
        std::fs::canonicalize(&expanded).map_err(|_| format!("找不到这个文件夹：{trimmed}"))?;
    if !canonical.is_dir() {
        return Err(format!("这不是一个文件夹：{trimmed}"));
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::ProjectStore;

    fn temp_project(label: &str) -> String {
        let dir = std::env::temp_dir().join(format!(
            "agentterm-store-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.to_string_lossy().into_owned()
    }

    #[test]
    fn add_then_reject_duplicate() {
        let mut store = ProjectStore::default();
        let path = temp_project("dup");
        store.add(&path).unwrap();
        assert_eq!(store.projects.len(), 1);
        assert!(store.add(&path).is_err());
    }

    #[test]
    fn remove_keeps_folder_on_disk() {
        let mut store = ProjectStore::default();
        let path = temp_project("keep");
        store.add(&path).unwrap();
        let removed = store.remove(0).unwrap();
        assert!(removed.path.is_dir());
        assert!(store.projects.is_empty());
    }

    #[test]
    fn missing_path_is_rejected() {
        let mut store = ProjectStore::default();
        assert!(store.add("/definitely/not/here/xyz").is_err());
    }
}
