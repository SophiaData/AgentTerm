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
    pub load_error: Option<String>,
    /// Where this store persists. `None` means "use the real user config path".
    /// An explicit path lets tests isolate themselves completely, so running the
    /// suite can never touch the developer's real project list.
    path: Option<PathBuf>,
}

fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))?;
    Some(base.join("agentterm").join("projects.json"))
}

impl ProjectStore {
    /// Load from the real user config path.
    pub fn load() -> Self {
        let mut store = Self {
            projects: Vec::new(),
            load_error: None,
            path: config_path(),
        };
        store.read_into_memory();
        store
    }

    /// A store bound to an explicit file. Used by tests (and anywhere a private
    /// location is wanted) so it never reads or writes the real user config.
    #[cfg(test)]
    pub fn with_path(path: PathBuf) -> Self {
        let mut store = Self {
            projects: Vec::new(),
            load_error: None,
            path: Some(path),
        };
        store.read_into_memory();
        store
    }

    fn read_into_memory(&mut self) {
        let Some(path) = &self.path else {
            return;
        };
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(_) => {
                self.load_error = Some("无法读取项目配置，已保留原文件".to_string());
                return;
            }
        };
        let Ok(data) = serde_json::from_str::<serde_json::Value>(&text) else {
            self.load_error = Some("项目配置损坏，已保留原文件；请修复或移除后重试".to_string());
            return;
        };
        let Some(items) = data.get("projects").and_then(|value| value.as_array()) else {
            self.load_error =
                Some("项目配置格式无效，已保留原文件；请修复或移除后重试".to_string());
            return;
        };

        let mut projects = Vec::with_capacity(items.len());
        for item in items {
            let Some(path) = item.get("path").and_then(|value| value.as_str()) else {
                self.load_error =
                    Some("项目配置包含无效记录，已保留原文件；请修复或移除后重试".to_string());
                self.projects.clear();
                return;
            };
            let last_tool = item
                .get("last_tool")
                .and_then(|value| value.as_str())
                .map(str::to_string);
            projects.push(Project {
                path: PathBuf::from(path),
                last_tool,
            });
        }
        self.projects = projects;
    }

    #[cfg(test)]
    pub fn bound_path(&self) -> Option<&std::path::Path> {
        self.path.as_deref()
    }

    pub fn save(&self) -> Result<(), String> {
        if let Some(error) = &self.load_error {
            return Err(error.clone());
        }
        let Some(path) = &self.path else {
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
        std::fs::rename(&tmp, path).map_err(|e| format!("无法保存配置：{e}"))?;
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
    use std::path::PathBuf;

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

    /// A store bound to a private temp file, never the real user config.
    fn isolated_store(label: &str) -> ProjectStore {
        let path = std::env::temp_dir().join(format!(
            "agentterm-store-cfg-{label}-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        ProjectStore::with_path(path)
    }

    #[test]
    fn add_then_reject_duplicate() {
        let mut store = isolated_store("dup");
        let path = temp_project("dup");
        store.add(&path).unwrap();
        assert_eq!(store.projects.len(), 1);
        assert!(store.add(&path).is_err());
    }

    #[test]
    fn remove_keeps_folder_on_disk() {
        let mut store = isolated_store("keep");
        let path = temp_project("keep");
        store.add(&path).unwrap();
        let removed = store.remove(0).unwrap();
        assert!(removed.path.is_dir());
        assert!(store.projects.is_empty());
    }

    #[test]
    fn save_and_reload_round_trips_through_bound_path() {
        let label = "roundtrip";
        let path = temp_project("roundtrip");
        let bound = std::env::temp_dir().join(format!(
            "agentterm-store-cfg-{label}-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        {
            let mut store = ProjectStore::with_path(bound.clone());
            store.add(&path).unwrap();
            store.save().unwrap();
        }
        let reloaded = ProjectStore::with_path(bound.clone());
        assert_eq!(reloaded.projects.len(), 1);
        assert!(reloaded.bound_path() == Some(bound.as_path()));
        let _ = std::fs::remove_file(&bound);
    }

    #[test]
    fn damaged_config_is_reported_without_becoming_an_empty_store() {
        let path = std::env::temp_dir().join(format!(
            "agentterm-store-damaged-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, b"{not-json").unwrap();
        let store = ProjectStore::with_path(path.clone());
        assert!(store.projects.is_empty());
        assert!(store.load_error.is_some());
        assert!(store.save().is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"{not-json");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn invalid_project_record_blocks_overwriting_a_partly_valid_config() {
        let path = std::env::temp_dir().join(format!(
            "agentterm-store-bad-record-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let body = br#"{"version":1,"projects":[{"path":"/valid"},{"last_tool":null}]}"#;
        std::fs::write(&path, body).unwrap();
        let store = ProjectStore::with_path(path.clone());
        assert!(store.projects.is_empty());
        assert!(store.load_error.is_some());
        assert!(store.save().is_err());
        assert_eq!(std::fs::read(&path).unwrap(), body);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn missing_path_is_rejected() {
        let mut store = ProjectStore::default();
        assert!(store.add("/definitely/not/here/xyz").is_err());
    }

    #[test]
    fn isolated_store_never_uses_real_config() {
        // A bound store must point at the temp dir, never the user's real path.
        let store = isolated_store("guard");
        let bound = store.bound_path().expect("bound path");
        assert!(bound.starts_with(std::env::temp_dir()));
        let real = super::config_path();
        if let Some(real) = real {
            assert_ne!(bound, real.as_path());
        }
        let _ = PathBuf::from(bound);
    }
}
