use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

use crate::tools::TOOL_DEFINITIONS;

static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_tool: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct StoreData {
    projects: Vec<Project>,
    last_tools: BTreeMap<String, Option<String>>,
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

/// Uses the same config priority and filename as agentterm-tui/src/store.rs.
/// An explicit file path is used in every test, never the real home directory.
fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))?;
    Some(base.join("agentterm").join("projects.json"))
}

fn load_data(path: Option<&Path>) -> Result<StoreData, String> {
    let Some(path) = path else {
        return Ok(StoreData::default());
    };
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(StoreData::default()),
        Err(error) => return Err(format!("无法读取项目配置（原文件未修改）：{error}")),
    };
    let disk: StoredFile = serde_json::from_str(&text)
        .map_err(|error| format!("项目配置已损坏（原文件未修改）：{error}"))?;
    if disk.version != 1 {
        return Err(format!(
            "不支持的项目配置版本 {}（原文件未修改）",
            disk.version
        ));
    }

    let mut data = StoreData::default();
    for saved in disk.projects {
        if saved.path.trim().is_empty() {
            return Err("项目配置包含空路径（原文件未修改）".to_string());
        }
        let original = PathBuf::from(&saved.path);
        // macOS /var -> /private/var aliases and TUI-imported symlinks must
        // compare against the same canonical identity used by GUI additions.
        // Keep missing folders in the list so they can still be removed.
        let canonical = fs::canonicalize(&original).unwrap_or(original);
        let path = canonical.to_string_lossy().into_owned();
        if data.last_tools.contains_key(&path) {
            return Err("项目配置包含重复路径（原文件未修改）".to_string());
        }
        data.projects.push(Project {
            id: path.clone(),
            name: project_name(&canonical),
            path: path.clone(),
            last_tool: saved.last_tool.clone(),
        });
        data.last_tools.insert(path, saved.last_tool);
    }
    Ok(data)
}

/// A stable sidecar path survives atomic replacement of projects.json.
/// Both GUI and TUI must acquire this same OS-level exclusive file lock
/// before read-modify-write; locking projects.json itself would lock an
/// obsolete inode after rename and allow lost updates.
fn lock_config(path: Option<&Path>) -> Result<File, String> {
    let path = path.ok_or("无法定位配置目录，项目变更未保存")?;
    let parent = path.parent().ok_or("无法定位配置目录，项目变更未保存")?;
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

fn save_data(path: Option<&Path>, data: &StoreData) -> Result<(), String> {
    let path = path.ok_or("无法定位配置目录，项目变更未保存")?;
    let parent = path.parent().ok_or("无法定位配置目录，项目变更未保存")?;
    fs::create_dir_all(parent).map_err(|error| format!("无法创建配置目录：{error}"))?;

    let projects = data
        .projects
        .iter()
        .map(|project| StoredProject {
            path: project.path.clone(),
            last_tool: data.last_tools.get(&project.id).cloned().flatten(),
        })
        .collect();
    let body = serde_json::to_vec_pretty(&StoredFile {
        version: 1,
        projects,
    })
    .map_err(|error| format!("无法编码项目配置：{error}"))?;

    // Unique, same-directory, create_new temporary file: never truncate the source.
    // Renaming a fully flushed file is the atomic commit point.
    let suffix = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
    let tmp_name = format!(
        ".{}.{}.{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id(),
        suffix
    );
    let tmp = parent.join(tmp_name);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&tmp)
        .map_err(|error| format!("无法准备项目配置：{error}"))?;
    file.write_all(&body)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("无法写入项目配置：{error}"))?;
    drop(file);
    fs::rename(&tmp, path).map_err(|error| format!("无法原子保存项目配置：{error}"))
}

pub struct ProjectStore {
    data: Mutex<StoreData>,
    path: Option<PathBuf>,
    #[cfg(test)]
    fail_next_save: std::sync::atomic::AtomicBool,
}

impl Default for ProjectStore {
    fn default() -> Self {
        Self::at_path(config_path())
    }
}

impl ProjectStore {
    fn at_path(path: Option<PathBuf>) -> Self {
        Self {
            data: Mutex::new(StoreData::default()),
            path,
            #[cfg(test)]
            fail_next_save: std::sync::atomic::AtomicBool::new(false),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_path(path: PathBuf) -> Self {
        Self::at_path(Some(path))
    }

    #[cfg(test)]
    pub(crate) fn fail_next_save_for_test(&self) {
        self.fail_next_save.store(true, Ordering::SeqCst);
    }

    /// Refresh before every operation so TUI and GUI observe each other's edits.
    /// Malformed/unsupported configs are never silently replaced with an empty list.
    fn lock_latest(&self) -> Result<MutexGuard<'_, StoreData>, String> {
        let mut data = self
            .data
            .lock()
            .map_err(|_| "项目列表暂时不可用，请重试".to_string())?;
        *data = load_data(self.path.as_deref())?;
        Ok(data)
    }

    fn persist(&self, data: &StoreData) -> Result<(), String> {
        #[cfg(test)]
        if self.fail_next_save.swap(false, Ordering::SeqCst) {
            return Err("模拟项目配置保存失败".to_string());
        }
        save_data(self.path.as_deref(), data)
    }

    fn change<T>(
        &self,
        change: impl FnOnce(&mut StoreData) -> Result<T, String>,
    ) -> Result<T, String> {
        // Hold the cross-process lock through reload, mutation and atomic rename.
        // The in-process mutex alone cannot coordinate GUI with a TUI process.
        let _lock = lock_config(self.path.as_deref())?;
        let mut data = self.lock_latest()?;
        let previous = data.clone();
        let result = match change(&mut data) {
            Ok(value) => value,
            Err(error) => {
                *data = previous;
                return Err(error);
            }
        };
        if let Err(error) = self.persist(&data) {
            *data = previous;
            return Err(error);
        }
        Ok(result)
    }

    pub fn list(&self) -> Result<Vec<Project>, String> {
        Ok(self.lock_latest()?.projects.clone())
    }

    pub fn add(&self, path: &str) -> Result<Project, String> {
        self.change(|data| {
            let added = add_project_to_list(&mut data.projects, path)?;
            data.last_tools.insert(added.id.clone(), None);
            Ok(added)
        })
    }

    pub fn remove(&self, id: &str) -> Result<Project, String> {
        self.change(|data| {
            let removed = remove_project_from_list(&mut data.projects, id)?;
            data.last_tools.remove(&removed.id);
            Ok(removed)
        })
    }

    /// The JSON schema has no enabled flag: membership in the saved list is
    /// the enabled state. Removed projects may not be launched.
    pub fn require_enabled_project(&self, path: &str) -> Result<PathBuf, String> {
        let canonical = normalize_project_dir(path)?;
        let data = self.lock_latest()?;
        if data
            .projects
            .iter()
            .any(|item| Path::new(&item.path) == canonical)
        {
            Ok(canonical)
        } else {
            Err("项目未注册或已禁用，请先将目录添加到项目列表".to_string())
        }
    }

    /// Record only allowlisted tools for existing projects; never accept
    /// arbitrary commands or alter a project folder on disk.
    pub fn remember_last_tool(&self, path: &str, tool_id: &str) -> Result<(), String> {
        if !TOOL_DEFINITIONS.iter().any(|tool| tool.id == tool_id) {
            return Err(format!("不支持的工具：{tool_id}"));
        }
        let canonical = normalize_project_dir(path)?;
        let canonical = canonical.to_string_lossy().into_owned();
        self.change(|data| {
            if !data.projects.iter().any(|item| item.id == canonical) {
                return Err("项目未注册或已禁用，请先将目录添加到项目列表".to_string());
            }
            data.last_tools
                .insert(canonical.clone(), Some(tool_id.to_string()));
            if let Some(project) = data.projects.iter_mut().find(|item| item.id == canonical) {
                project.last_tool = Some(tool_id.to_string());
            }
            Ok(())
        })
    }
}

pub fn normalize_project_dir(raw_path: &str) -> Result<PathBuf, String> {
    let trimmed = raw_path.trim();

    if trimmed.is_empty() {
        return Err("请选择一个项目文件夹".to_string());
    }

    let path = PathBuf::from(trimmed);
    if !path.is_dir() {
        return Err(format!("找不到文件夹：{trimmed}"));
    }

    std::fs::canonicalize(&path).map_err(|error| format!("无法读取项目目录：{error}"))
}

pub fn project_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| path.display().to_string())
}

pub fn add_project_to_list(projects: &mut Vec<Project>, raw_path: &str) -> Result<Project, String> {
    let canonical = normalize_project_dir(raw_path)?;
    let id = canonical.display().to_string();

    if projects.iter().any(|project| project.id == id) {
        return Err(format!("{} 已在项目列表中", project_name(&canonical)));
    }

    let project = Project {
        id,
        name: project_name(&canonical),
        path: canonical.display().to_string(),
        last_tool: None,
    };
    projects.push(project.clone());

    Ok(project)
}

pub fn remove_project_from_list(projects: &mut Vec<Project>, id: &str) -> Result<Project, String> {
    let index = projects
        .iter()
        .position(|project| project.id == id)
        .ok_or_else(|| "项目不存在或已被移除".to_string())?;

    // Only the in-memory record is removed; project files are never touched.
    Ok(projects.remove(index))
}

#[tauri::command]
pub fn list_projects(state: tauri::State<'_, ProjectStore>) -> Result<Vec<Project>, String> {
    state.list()
}

#[tauri::command]
pub fn add_project(state: tauri::State<'_, ProjectStore>, path: String) -> Result<Project, String> {
    state.add(&path)
}

#[tauri::command]
pub fn remove_project(
    state: tauri::State<'_, ProjectStore>,
    id: String,
) -> Result<Project, String> {
    state.remove(&id)
}

#[cfg(test)]
mod tests {
    use super::{
        Project, ProjectStore, add_project_to_list, normalize_project_dir, project_name,
        remove_project_from_list,
    };
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            let unique = format!(
                "agentterm-projects-{label}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|value| value.as_nanos())
                    .unwrap_or_default()
            );
            let path = std::env::temp_dir().join(unique);
            std::fs::create_dir_all(&path).expect("temp directory should be created");
            Self(path)
        }

        fn path(&self) -> PathBuf {
            self.0.clone()
        }

        fn path_string(&self) -> String {
            self.0.display().to_string()
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn empty_and_missing_paths_are_rejected() {
        assert_eq!(
            normalize_project_dir("  ").unwrap_err(),
            "请选择一个项目文件夹"
        );

        let missing = std::env::temp_dir().join("agentterm-project-does-not-exist");
        assert!(
            normalize_project_dir(&missing.display().to_string())
                .unwrap_err()
                .contains("找不到文件夹")
        );
    }

    #[test]
    fn adding_a_directory_uses_its_canonical_path_and_name() {
        let dir = TempDir::new("valid");
        let mut projects: Vec<Project> = Vec::new();

        let project = add_project_to_list(&mut projects, &dir.path_string()).unwrap();

        assert_eq!(
            project.name,
            dir.path().file_name().unwrap().to_string_lossy()
        );
        assert_eq!(
            project.id,
            normalize_project_dir(&dir.path_string())
                .unwrap()
                .display()
                .to_string()
        );
        assert_eq!(projects.len(), 1);
    }

    #[test]
    fn adding_the_same_directory_twice_is_rejected() {
        let dir = TempDir::new("duplicate");
        let mut projects: Vec<Project> = Vec::new();

        add_project_to_list(&mut projects, &dir.path_string()).unwrap();
        let error = add_project_to_list(&mut projects, &dir.path_string()).unwrap_err();

        assert!(error.contains("已在项目列表中"));
        assert_eq!(projects.len(), 1);
    }

    #[test]
    fn removing_a_project_keeps_its_files_on_disk() {
        let dir = TempDir::new("remove");
        let marker = dir.path().join("keep-me.txt");
        std::fs::write(&marker, "must survive").expect("marker file should be written");

        let mut projects: Vec<Project> = Vec::new();
        let project = add_project_to_list(&mut projects, &dir.path_string()).unwrap();
        let removed = remove_project_from_list(&mut projects, &project.id).unwrap();

        assert_eq!(removed.id, project.id);
        assert!(projects.is_empty());
        assert!(dir.path().is_dir());
        assert_eq!(
            std::fs::read_to_string(&marker).expect("project file should still exist"),
            "must survive"
        );
    }

    #[test]
    fn removing_an_unknown_project_is_reported() {
        let mut projects: Vec<Project> = Vec::new();

        assert_eq!(
            remove_project_from_list(&mut projects, "/not/registered").unwrap_err(),
            "项目不存在或已被移除"
        );
    }

    #[test]
    fn root_like_paths_still_have_a_display_name() {
        let root = if cfg!(windows) { "C:\\" } else { "/" };

        assert_eq!(project_name(&normalize_project_dir(root).unwrap()), root);
    }

    #[test]
    fn missing_config_starts_empty_and_writes_tui_compatible_json() {
        let tmp = TempDir::new("persist");
        let config = tmp.path().join("agentterm/projects.json");
        let directory = tmp.path().join("project");
        std::fs::create_dir(&directory).unwrap();
        let store = ProjectStore::with_path(config.clone());

        assert!(store.list().unwrap().is_empty());
        assert!(!config.exists());
        let added = store.add(&directory.display().to_string()).unwrap();
        store.remember_last_tool(&added.path, "codex").unwrap();

        let document: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
        assert_eq!(document["version"], 1);
        assert_eq!(document["projects"].as_array().unwrap().len(), 1);
        assert_eq!(document["projects"][0]["path"], added.path);
        assert_eq!(document["projects"][0]["last_tool"], "codex");
        assert!(document["projects"][0].get("id").is_none());

        let reloaded = ProjectStore::with_path(config);
        let restored = reloaded.list().unwrap().pop().unwrap();
        assert_eq!(restored.path, added.path);
        assert_eq!(restored.last_tool.as_deref(), Some("codex"));
        assert_eq!(
            serde_json::to_value(&restored).unwrap()["lastTool"],
            "codex"
        );
        assert!(
            serde_json::to_value(&added)
                .unwrap()
                .get("lastTool")
                .is_none()
        );
    }

    #[test]
    fn tui_format_is_read_and_external_changes_are_refreshed() {
        let tmp = TempDir::new("tui");
        let project = tmp.path().join("project");
        std::fs::create_dir(&project).unwrap();
        let config = tmp.path().join("projects.json");
        let store = ProjectStore::with_path(config.clone());

        let body = serde_json::json!({
            "version": 1,
            "projects": [{"path": project.display().to_string(), "last_tool": "claude"}]
        });
        std::fs::write(&config, serde_json::to_vec(&body).unwrap()).unwrap();

        let projects = store.list().unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].last_tool.as_deref(), Some("claude"));
        assert_eq!(
            projects[0].path,
            std::fs::canonicalize(&project)
                .unwrap()
                .display()
                .to_string()
        );
        assert_eq!(
            store.require_enabled_project(&projects[0].path).unwrap(),
            std::fs::canonicalize(&project).unwrap()
        );
        store.remember_last_tool(&projects[0].path, "agy").unwrap();
        let updated: serde_json::Value =
            serde_json::from_slice(&std::fs::read(config).unwrap()).unwrap();
        assert_eq!(updated["projects"][0]["last_tool"], "agy");
    }

    #[test]
    fn corrupted_and_unsupported_configs_are_preserved() {
        let tmp = TempDir::new("corrupt");
        let directory = tmp.path().join("project");
        std::fs::create_dir(&directory).unwrap();
        let path = tmp.path().join("projects.json");

        for corrupt in [
            "{ this is not JSON }".to_string(),
            r#"{"version":2,"projects":[]}"#.to_string(),
            r#"{"version":1,"projects":[{"path":3}]}"#.to_string(),
        ] {
            std::fs::write(&path, &corrupt).unwrap();
            let store = ProjectStore::with_path(path.clone());
            assert!(store.list().is_err());
            assert!(store.add(&directory.display().to_string()).is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), corrupt);
        }
    }

    #[test]
    fn failed_save_rolls_back_memory_and_keeps_original_json() {
        let tmp = TempDir::new("rollback");
        let first = tmp.path().join("first");
        let second = tmp.path().join("second");
        std::fs::create_dir(&first).unwrap();
        std::fs::create_dir(&second).unwrap();
        let path = tmp.path().join("projects.json");
        let store = ProjectStore::with_path(path.clone());

        let original = store.add(&first.display().to_string()).unwrap();
        let original_bytes = std::fs::read(&path).unwrap();

        store.fail_next_save.store(true, Ordering::SeqCst);
        assert!(store.add(&second.display().to_string()).is_err());
        assert_eq!(store.data.lock().unwrap().projects, vec![original.clone()]);
        assert_eq!(std::fs::read(&path).unwrap(), original_bytes);

        store.fail_next_save.store(true, Ordering::SeqCst);
        assert!(store.remember_last_tool(&original.path, "claude").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original_bytes);
        assert_eq!(
            store.data.lock().unwrap().last_tools.get(&original.id),
            Some(&None)
        );

        store.fail_next_save.store(true, Ordering::SeqCst);
        assert!(store.remove(&original.id).is_err());
        assert_eq!(store.list().unwrap(), vec![original]);
        assert_eq!(std::fs::read(&path).unwrap(), original_bytes);
    }

    #[test]
    fn two_independent_gui_stores_merge_concurrent_adds_under_sidecar_lock() {
        use std::sync::{Arc, Barrier};

        let tmp = TempDir::new("concurrent-gui");
        let config = tmp.path().join("config/agentterm/projects.json");
        let dirs: Vec<_> = (0..8)
            .map(|index| {
                let directory = tmp.path().join(format!("project-{index}"));
                std::fs::create_dir_all(&directory).unwrap();
                directory
            })
            .collect();
        let barrier = Arc::new(Barrier::new(dirs.len() + 1));
        let handles: Vec<_> = dirs
            .into_iter()
            .map(|directory| {
                let config = config.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let store = ProjectStore::with_path(config);
                    barrier.wait();
                    store.add(&directory.display().to_string()).unwrap();
                })
            })
            .collect();

        barrier.wait();
        for handle in handles {
            handle.join().unwrap();
        }
        let store = ProjectStore::with_path(config.clone());
        assert_eq!(store.list().unwrap().len(), 8);
        assert!(config.with_extension("json.lock").exists());
    }

    #[test]
    fn unregistered_project_and_unknown_tool_cannot_record_history() {
        let tmp = TempDir::new("safety");
        let directory = tmp.path().join("project");
        std::fs::create_dir(&directory).unwrap();
        let path = directory.display().to_string();
        let store = ProjectStore::with_path(tmp.path().join("projects.json"));

        assert!(store.require_enabled_project(&path).is_err());
        assert!(store.remember_last_tool(&path, "codex").is_err());
        store.add(&path).unwrap();
        assert!(
            store
                .remember_last_tool(&path, "sh -c echo injected")
                .is_err()
        );
        let project = store.list().unwrap().pop().unwrap();
        store.remove(&project.id).unwrap();
        assert!(store.require_enabled_project(&path).is_err());
    }
}
