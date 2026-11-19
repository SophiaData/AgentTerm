use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
}

#[derive(Default)]
pub struct ProjectStore {
    data: Mutex<ProjectData>,
}

#[derive(Default)]
struct ProjectData {
    projects: Vec<Project>,
    path: Option<PathBuf>,
}

impl ProjectStore {
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, ProjectData>, String> {
        self.data
            .lock()
            .map_err(|_| "项目列表暂时不可用，请重试".to_string())
    }

    pub fn list(&self) -> Result<Vec<Project>, String> {
        let data = self.lock()?;
        match &data.path {
            Some(path) => persisted(path, false, |projects| Ok(projects.clone())),
            None => Ok(data.projects.clone()),
        }
    }

    /// Backend-only path, fixed before serving commands; never creates a directory.
    pub fn configure(&self, path: PathBuf) -> Result<(), String> {
        let mut data = self.lock()?;
        if data.path.is_some() || !data.projects.is_empty() {
            return Err("项目存储已经使用，不能切换".into());
        }
        let parent = path
            .parent()
            .filter(|p| p.is_dir())
            .ok_or_else(|| "项目存储父目录必须已经存在".to_string())?;
        let parent = parent.canonicalize().map_err(storage_error)?;
        let name = path
            .file_name()
            .ok_or_else(|| "项目存储路径无效".to_string())?;
        data.path = Some(parent.join(name));
        Ok(())
    }

    pub fn add(&self, path: &str) -> Result<Project, String> {
        let mut data = self.lock()?;
        match &data.path {
            Some(storage) => persisted(storage, true, |projects| {
                add_project_to_list(projects, path)
            }),
            None => add_project_to_list(&mut data.projects, path),
        }
    }

    pub fn remove(&self, id: &str) -> Result<Project, String> {
        let mut data = self.lock()?;
        match &data.path {
            Some(path) => persisted(path, true, |projects| {
                remove_project_from_list(projects, id)
            }),
            None => remove_project_from_list(&mut data.projects, id),
        }
    }
}

const PROJECT_BYTES: u64 = 1024 * 1024;
const PROJECT_LIMIT: usize = 200;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectFile {
    schema: u32,
    projects: Vec<Project>,
}

struct ProjectLock(File);
impl Drop for ProjectLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

fn storage_error(_: std::io::Error) -> String {
    "项目存储读写失败；请重新读取并核实，原文件不会自动重置".into()
}

fn companion(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn regular_or_missing(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => Ok(true),
        Ok(_) => Err("项目存储必须是普通文件，不能是链接".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(storage_error(error)),
    }
}

fn validate_projects(projects: &[Project]) -> Result<(), String> {
    let mut seen = std::collections::HashSet::new();
    if projects.len() > PROJECT_LIMIT {
        return Err("项目列表超过200项上限".into());
    }
    for project in projects {
        if project.id != project.path
            || !Path::new(&project.path).is_absolute()
            || project.path.len() > 4096
            || project.name.is_empty()
            || project.name.len() > 1024
            || project.path.chars().any(char::is_control)
            || project.name.chars().any(char::is_control)
            || !seen.insert(&project.id)
        {
            return Err("项目列表记录无效或重复；保留原文件，需核实".into());
        }
    }
    Ok(())
}

fn persisted<T>(
    path: &Path,
    write: bool,
    operation: impl FnOnce(&mut Vec<Project>) -> Result<T, String>,
) -> Result<T, String> {
    let guard_path = companion(path, ".lock");
    let pending = companion(path, ".pending");
    let data_exists = regular_or_missing(path)?;
    let guard_exists = regular_or_missing(&guard_path)?;
    if !write && !guard_exists {
        if data_exists || regular_or_missing(&pending)? {
            return Err("项目存储缺少锁文件或有未完成写入；保留原件，需核实".into());
        }
        return operation(&mut Vec::new());
    }
    if data_exists && !guard_exists {
        return Err("项目存储缺少锁文件；保留原件，需核实".into());
    }
    let guard = OpenOptions::new()
        .read(true)
        .write(true)
        .create(write)
        .truncate(false)
        .open(&guard_path)
        .map_err(storage_error)?;
    guard
        .try_lock()
        .map_err(|_| "项目列表正被另一操作使用，请重试".to_string())?;
    let _guard = ProjectLock(guard);
    if regular_or_missing(&pending)? {
        return Err("项目列表有未完成写入；保留原件与pending文件，需核实".into());
    }
    let mut projects = if regular_or_missing(path)? {
        let file = File::open(path).map_err(storage_error)?;
        if file.metadata().map_err(storage_error)?.len() > PROJECT_BYTES {
            return Err("项目存储超过1 MiB上限".into());
        }
        let mut bytes = Vec::new();
        file.take(PROJECT_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(storage_error)?;
        if bytes.len() as u64 > PROJECT_BYTES {
            return Err("项目存储超过1 MiB上限".into());
        }
        let saved: ProjectFile = serde_json::from_slice(&bytes)
            .map_err(|_| "项目列表版本未知或内容损坏；保留原文件，需核实".to_string())?;
        if saved.schema != 1 {
            return Err("项目列表版本未知；保留原文件，需核实".into());
        }
        validate_projects(&saved.projects)?;
        saved.projects
    } else {
        Vec::new()
    };
    let result = operation(&mut projects)?;
    if write {
        validate_projects(&projects)?;
        let bytes = serde_json::to_vec(&ProjectFile {
            schema: 1,
            projects,
        })
        .map_err(|_| "项目列表无法序列化".to_string())?;
        if bytes.len() as u64 > PROJECT_BYTES {
            return Err("项目存储超过1 MiB上限".into());
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&pending)
            .map_err(storage_error)?;
        file.write_all(&bytes).map_err(storage_error)?;
        file.sync_all().map_err(storage_error)?;
        drop(file);
        // Same-directory atomic replacement. No project directory/file is moved.
        fs::rename(&pending, path).map_err(storage_error)?;
        #[cfg(unix)]
        File::open(
            path.parent()
                .ok_or_else(|| "项目存储位置无效".to_string())?,
        )
        .and_then(|file| file.sync_all())
        .map_err(storage_error)?;
    }
    Ok(result)
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
    };
    projects.push(project.clone());

    Ok(project)
}

pub fn remove_project_from_list(projects: &mut Vec<Project>, id: &str) -> Result<Project, String> {
    let index = projects
        .iter()
        .position(|project| project.id == id)
        .ok_or_else(|| "项目不存在或已被移除".to_string())?;

    // Only the project record is removed; project files are never touched.
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
        Project, add_project_to_list, normalize_project_dir, project_name, remove_project_from_list,
    };
    use super::{ProjectStore, companion};
    use std::path::PathBuf;

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

    fn store_path() -> PathBuf {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "agentterm-project-store-{}-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ))
    }

    fn persistent(path: &std::path::Path) -> ProjectStore {
        let store = ProjectStore::default();
        store.configure(path.to_owned()).unwrap();
        store
    }

    #[test]
    fn persistent_projects_reopen_and_multiple_instances_read_fresh_data() {
        let path = store_path();
        let one = persistent(&path);
        let two = persistent(&path);
        assert!(one.list().unwrap().is_empty());
        assert!(!path.exists()); // read/configure never creates storage
        let dir = std::env::temp_dir();
        let project = one.add(&dir.to_string_lossy()).unwrap();
        assert_eq!(two.list().unwrap(), vec![project.clone()]);
        assert!(two.add(&dir.to_string_lossy()).is_err());
        assert!(!companion(&path, ".pending").exists());
        two.remove(&project.id).unwrap(); // Windows replace-existing path tested
        assert!(one.list().unwrap().is_empty());
        assert!(persistent(&path).list().unwrap().is_empty());
        assert!(dir.is_dir());
    }

    #[test]
    fn damaged_unknown_duplicate_and_oversized_projects_are_preserved() {
        let path = store_path();
        let store = persistent(&path);
        let project = store.add(&std::env::temp_dir().to_string_lossy()).unwrap();
        for bytes in [
            b"{broken".to_vec(),
            br#"{"schema":99,"projects":[]}"#.to_vec(),
            serde_json::to_vec(&super::ProjectFile {
                schema: 1,
                projects: vec![project.clone(), project.clone()],
            })
            .unwrap(),
            vec![b' '; super::PROJECT_BYTES as usize + 1],
        ] {
            std::fs::write(&path, &bytes).unwrap();
            assert!(store.list().is_err());
            assert!(store.remove(&project.id).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
    }

    #[test]
    fn interrupted_atomic_write_preserves_original_and_blocks_further_writes() {
        let path = store_path();
        let store = persistent(&path);
        let project = store.add(&std::env::temp_dir().to_string_lossy()).unwrap();
        let before = std::fs::read(&path).unwrap();
        let pending = companion(&path, ".pending");
        std::fs::write(&pending, b"partial replacement").unwrap();
        assert!(store.list().unwrap_err().contains("未完成写入"));
        assert!(store.remove(&project.id).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(std::fs::read(&pending).unwrap(), b"partial replacement");
    }

    #[test]
    fn persistent_lock_prevents_lost_updates_and_configuration_cannot_switch() {
        let path = store_path();
        let one = persistent(&path);
        let project = one.add(&std::env::temp_dir().to_string_lossy()).unwrap();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(companion(&path, ".lock"))
            .unwrap();
        file.try_lock().unwrap();
        let two = persistent(&path);
        assert!(two.remove(&project.id).unwrap_err().contains("另一操作"));
        file.unlock().unwrap();
        assert_eq!(two.list().unwrap(), vec![project]);
        assert!(one.configure(store_path()).is_err());
        let missing = store_path();
        assert!(
            ProjectStore::default()
                .configure(missing.join("projects.json"))
                .is_err()
        );
        assert!(!missing.exists());
    }

    #[test]
    fn failed_new_project_validation_leaves_saved_bytes_unchanged() {
        let path = store_path();
        let store = persistent(&path);
        store.add(&std::env::temp_dir().to_string_lossy()).unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(store.add(&store_path().to_string_lossy()).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(!companion(&path, ".pending").exists());
    }

    #[cfg(unix)]
    #[test]
    fn storage_symlinks_are_refused_without_changing_the_target() {
        let target = store_path();
        std::fs::write(&target, b"keep").unwrap();
        let path = store_path();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        let store = persistent(&path);
        assert!(store.list().is_err());
        assert!(store.add(&std::env::temp_dir().to_string_lossy()).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"keep");
    }
}
