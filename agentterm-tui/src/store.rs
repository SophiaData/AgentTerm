//! Persistent project list, stored as JSON in the user's config directory.
//!
//! Only project paths and the last-used tool are stored. Nothing about a tool's
//! conversations, credentials or environment is ever written here.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Project {
    pub path: PathBuf,
    pub last_tool: Option<String>,
}

#[derive(Debug, Default)]
pub struct ProjectStore {
    pub projects: Vec<Project>,
    /// Where this store persists. None means "use the real user config path".
    /// Tests always bind an explicit isolated path.
    path: Option<PathBuf>,
    /// The snapshot from the last successful load/save. TUI edits happen in
    /// memory; this baseline lets save() merge only locally changed fields.
    baseline: Vec<Project>,
    load_error: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredProject {
    path: String,
    #[serde(default)]
    last_tool: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredFile {
    version: u32,
    projects: Vec<StoredProject>,
}

fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))?;
    Some(base.join("agentterm").join("projects.json"))
}

/// Read strict version-1 data, never treating malformed or future-version
/// configuration as an empty list that could later overwrite the original.
fn read_projects(path: &Path) -> Result<Vec<Project>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("无法读取项目配置（原文件未修改）：{error}")),
    };
    let saved: StoredFile = serde_json::from_str(&text)
        .map_err(|error| format!("项目配置损坏（原文件未修改）：{error}"))?;
    if saved.version != 1 {
        return Err(format!(
            "不支持的项目配置版本 {}（原文件未修改）",
            saved.version
        ));
    }

    let mut seen = HashSet::new();
    let mut projects = Vec::with_capacity(saved.projects.len());
    for item in saved.projects {
        if item.path.trim().is_empty() {
            return Err("项目配置包含空路径（原文件未修改）".to_string());
        }
        let original = PathBuf::from(item.path);
        // Normalize aliases the same way as the GUI while keeping missing
        // folders listed so they can be removed without deleting their files.
        let path = fs::canonicalize(&original).unwrap_or(original);
        if !seen.insert(path.clone()) {
            return Err("项目配置包含重复路径（原文件未修改）".to_string());
        }
        projects.push(Project {
            path,
            last_tool: item.last_tool,
        });
    }
    Ok(projects)
}

/// Both GUI and TUI lock the same stable sidecar path. Locking the JSON file
/// itself is unsafe because atomic rename replaces its inode on every save.
fn lock_config(path: &Path) -> Result<File, String> {
    let parent = path.parent().ok_or("无法定位配置目录")?;
    fs::create_dir_all(parent).map_err(|error| format!("无法创建配置目录：{error}"))?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let lock = options
        .open(path.with_extension("json.lock"))
        .map_err(|error| format!("无法打开项目配置锁：{error}"))?;
    lock.lock()
        .map_err(|error| format!("无法锁定项目配置：{error}"))?;
    Ok(lock)
}

/// Merge a stale TUI snapshot against the newest GUI/TUI JSON under the lock.
/// Unchanged fields use the latest disk value. Different simultaneous changes
/// to the same last_tool (or remove versus edit) fail explicitly, rather than
/// overwriting the other process's work.
fn merge_projects(
    baseline: &[Project],
    edited: &[Project],
    mut latest: Vec<Project>,
) -> Result<Vec<Project>, String> {
    let mut seen = HashSet::new();
    for project in edited {
        if !seen.insert(&project.path) {
            return Err("项目列表包含重复路径，无法保存".to_string());
        }
    }

    for original in baseline {
        let local = edited.iter().find(|project| project.path == original.path);
        let disk = latest
            .iter()
            .position(|project| project.path == original.path);

        match (local, disk) {
            (None, Some(index)) => {
                if latest[index].last_tool != original.last_tool {
                    return Err(format!(
                        "项目 {} 已被其他进程修改，请重新加载后重试",
                        original.path.display()
                    ));
                }
                latest.remove(index);
            }
            (Some(local), Some(index)) if local.last_tool != original.last_tool => {
                if latest[index].last_tool != original.last_tool
                    && latest[index].last_tool != local.last_tool
                {
                    return Err(format!(
                        "项目 {} 的最近工具有并发冲突，请重新加载后重试",
                        original.path.display()
                    ));
                }
                latest[index].last_tool = local.last_tool.clone();
            }
            (Some(local), None) if local.last_tool != original.last_tool => {
                return Err(format!(
                    "项目 {} 已被其他进程移除，请重新加载后重试",
                    original.path.display()
                ));
            }
            // No local changes: keep the latest disk record (including removals).
            _ => {}
        }
    }

    for project in edited {
        if baseline
            .iter()
            .any(|original| original.path == project.path)
        {
            continue;
        }
        if let Some(remote) = latest.iter_mut().find(|item| item.path == project.path) {
            if let Some(local_tool) = &project.last_tool {
                if remote
                    .last_tool
                    .as_ref()
                    .is_some_and(|saved| saved != local_tool)
                {
                    return Err(format!(
                        "项目 {} 的最近工具有并发冲突，请重新加载后重试",
                        project.path.display()
                    ));
                }
                remote.last_tool = Some(local_tool.clone());
            }
        } else {
            latest.push(project.clone());
        }
    }

    Ok(latest)
}

fn save_projects(path: &Path, projects: &[Project]) -> Result<(), String> {
    let parent = path.parent().ok_or("无法定位配置目录")?;
    let body = serde_json::to_vec_pretty(&StoredFile {
        version: 1,
        projects: projects
            .iter()
            .map(|project| StoredProject {
                path: project.path.to_string_lossy().into_owned(),
                last_tool: project.last_tool.clone(),
            })
            .collect(),
    })
    .map_err(|error| format!("无法编码项目配置：{error}"))?;

    // Same-directory, unique create_new temp file; flush before atomic commit.
    let unique = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
    let temp = parent.join(format!(
        ".{}.{}.{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id(),
        unique
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temp)
        .map_err(|error| format!("无法准备项目配置：{error}"))?;
    file.write_all(&body)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("无法写入项目配置：{error}"))?;
    drop(file);
    fs::rename(&temp, path).map_err(|error| format!("无法原子保存项目配置：{error}"))
}

impl ProjectStore {
    /// Load from the real user config path.
    pub fn load() -> Self {
        let mut store = Self {
            path: config_path(),
            ..Self::default()
        };
        store.read_into_memory();
        store
    }

    /// An explicitly bound test-only store never accesses the real user config.
    #[cfg(test)]
    pub fn with_path(path: PathBuf) -> Self {
        let mut store = Self {
            path: Some(path),
            ..Self::default()
        };
        store.read_into_memory();
        store
    }

    fn read_into_memory(&mut self) {
        let result = self
            .path
            .as_deref()
            .ok_or_else(|| "无法定位配置目录".to_string())
            .and_then(read_projects);
        match result {
            Ok(projects) => {
                self.baseline = projects.clone();
                self.projects = projects;
                self.load_error = None;
            }
            Err(error) => {
                self.projects.clear();
                self.baseline.clear();
                self.load_error = Some(error);
            }
        }
    }

    /// Allows the existing UI's App::error to present an accurate error
    /// instead of treating a damaged configuration as a fresh empty project list.
    pub fn load_error(&self) -> Option<&str> {
        self.load_error.as_deref()
    }

    #[cfg(test)]
    pub fn bound_path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn save(&mut self) -> Result<(), String> {
        if let Some(error) = &self.load_error {
            return Err(format!("加载项目配置失败，已禁止保存：{error}"));
        }
        let path = self.path.as_deref().ok_or("无法定位配置目录")?;
        let _lock = lock_config(path)?;
        let latest = read_projects(path)?;
        let merged = merge_projects(&self.baseline, &self.projects, latest)?;
        save_projects(path, &merged)?;
        // Refresh both after commit: a future save must not delete externally
        // added projects simply because the stale UI didn't know about them.
        self.projects = merged.clone();
        self.baseline = merged;
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

    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "agentterm-tui-merge-{label}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn project(&self, name: &str) -> String {
            let path = self.0.join(name);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::canonicalize(path).unwrap().display().to_string()
        }

        fn config(&self) -> PathBuf {
            self.0.join("config/agentterm/projects.json")
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn stale_tui_add_merges_gui_json_without_losing_last_tool() {
        let tmp = TempRoot::new("gui-compat");
        let gui = tmp.project("gui");
        let tui = tmp.project("tui");
        let config = tmp.config();
        let mut store = ProjectStore::with_path(config.clone());

        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        let gui_document = serde_json::json!({
            "version": 1,
            "projects": [{"path": gui, "last_tool": "codex"}]
        });
        std::fs::write(&config, serde_json::to_vec(&gui_document).unwrap()).unwrap();

        store.add(&tui).unwrap();
        store.save().unwrap();
        assert_eq!(store.projects.len(), 2);
        let gui_project = store
            .projects
            .iter()
            .find(|item| item.path.to_string_lossy() == gui)
            .unwrap();
        assert_eq!(gui_project.last_tool.as_deref(), Some("codex"));
        store.save().unwrap(); // Repeated save must also preserve external records.

        let disk: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
        assert_eq!(disk["version"], 1);
        assert_eq!(disk["projects"].as_array().unwrap().len(), 2);
        assert!(config.with_extension("json.lock").exists());
    }

    #[test]
    fn two_stale_tui_snapshots_merge_disjoint_project_additions() {
        let tmp = TempRoot::new("two-writers");
        let a = tmp.project("alpha");
        let b = tmp.project("beta");
        let config = tmp.config();
        let mut first = ProjectStore::with_path(config.clone());
        let mut second = ProjectStore::with_path(config.clone());

        first.add(&a).unwrap();
        second.add(&b).unwrap();
        first.save().unwrap();
        second.save().unwrap();

        assert_eq!(second.projects.len(), 2);
        let final_store = ProjectStore::with_path(config);
        assert_eq!(final_store.projects.len(), 2);
        assert!(
            final_store
                .projects
                .iter()
                .any(|project| project.path.to_string_lossy() == a)
        );
        assert!(
            final_store
                .projects
                .iter()
                .any(|project| project.path.to_string_lossy() == b)
        );
    }

    #[test]
    fn concurrent_last_tool_conflict_does_not_overwrite_first_writer() {
        let tmp = TempRoot::new("conflict");
        let config = tmp.config();
        let project = tmp.project("project");
        let mut initial = ProjectStore::with_path(config.clone());
        initial.add(&project).unwrap();
        initial.save().unwrap();
        let mut first = ProjectStore::with_path(config.clone());
        let mut second = ProjectStore::with_path(config.clone());

        first.set_last_tool(0, "claude");
        second.set_last_tool(0, "codex");
        first.save().unwrap();
        let original = std::fs::read(&config).unwrap();
        assert!(second.save().unwrap_err().contains("并发冲突"));
        assert_eq!(std::fs::read(config).unwrap(), original);
        assert_eq!(second.projects[0].last_tool.as_deref(), Some("codex"));
    }

    #[test]
    fn remove_conflicting_with_external_history_change_is_rejected() {
        let tmp = TempRoot::new("remove-conflict");
        let config = tmp.config();
        let project = tmp.project("project");
        let mut initial = ProjectStore::with_path(config.clone());
        initial.add(&project).unwrap();
        initial.save().unwrap();
        let mut remover = ProjectStore::with_path(config.clone());
        let mut editor = ProjectStore::with_path(config.clone());

        remover.remove(0).unwrap();
        editor.set_last_tool(0, "agy");
        editor.save().unwrap();
        let before = std::fs::read(&config).unwrap();
        assert!(remover.save().unwrap_err().contains("其他进程修改"));
        assert_eq!(std::fs::read(&config).unwrap(), before);
    }

    #[test]
    fn malformed_unknown_and_partially_invalid_json_preserve_original_config() {
        let tmp = TempRoot::new("corrupt");
        let config = tmp.config();
        let project = tmp.project("new");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        for broken in [
            "{ malformed".to_string(),
            r#"{"version":2,"projects":[]}"#.to_string(),
            r#"{"version":1,"projects":[{"path":8,"last_tool":null}]}"#.to_string(),
        ] {
            std::fs::write(&config, &broken).unwrap();
            let mut store = ProjectStore::with_path(config.clone());
            assert!(store.load_error().is_some());
            assert!(store.projects.is_empty());
            store.add(&project).unwrap();
            assert!(store.save().unwrap_err().contains("禁止保存"));
            assert_eq!(std::fs::read_to_string(&config).unwrap(), broken);
        }
    }

    #[test]
    fn independent_threads_merge_simultaneous_tui_writes() {
        use std::sync::{Arc, Barrier};
        let tmp = TempRoot::new("thread-race");
        let config = tmp.config();
        let names = [tmp.project("alpha"), tmp.project("beta")];
        let barrier = Arc::new(Barrier::new(3));
        let handles: Vec<_> = names
            .into_iter()
            .map(|name| {
                let config = config.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let mut store = ProjectStore::with_path(config);
                    store.add(&name).unwrap();
                    barrier.wait();
                    store.save().unwrap();
                })
            })
            .collect();
        barrier.wait();
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(ProjectStore::with_path(config).projects.len(), 2);
    }

    #[test]
    fn separate_process_save_merges_stale_snapshot() {
        const CHILD_PATH: &str = "AGENTTERM_TUI_ISOLATED_CHILD_PATH";
        if let Some(root) = std::env::var_os(CHILD_PATH) {
            let root = PathBuf::from(root);
            let config = root.join("config/agentterm/projects.json");
            let child_project = root.join("child");
            std::fs::create_dir_all(&child_project).unwrap();
            let mut store = ProjectStore::with_path(config);
            store.add(&child_project.display().to_string()).unwrap();
            store.save().unwrap();
            return;
        }

        let tmp = TempRoot::new("process");
        let parent_project = tmp.project("parent");
        let mut parent = ProjectStore::with_path(tmp.config());
        parent.add(&parent_project).unwrap();

        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "store::tests::separate_process_save_merges_stale_snapshot",
            ])
            .env(CHILD_PATH, &tmp.0)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "isolated TUI child failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );

        parent.save().unwrap();
        let loaded = ProjectStore::with_path(tmp.config());
        assert_eq!(loaded.projects.len(), 2);
        assert!(
            loaded
                .projects
                .iter()
                .any(|project| project.path.to_string_lossy() == parent_project)
        );
    }
}
