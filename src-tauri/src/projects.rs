use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
}

#[derive(Default)]
pub struct ProjectStore {
    projects: Mutex<Vec<Project>>,
}

impl ProjectStore {
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Vec<Project>>, String> {
        self.projects
            .lock()
            .map_err(|_| "项目列表暂时不可用，请重试".to_string())
    }

    pub fn list(&self) -> Result<Vec<Project>, String> {
        Ok(self.lock()?.clone())
    }

    pub fn add(&self, path: &str) -> Result<Project, String> {
        let mut projects = self.lock()?;
        add_project_to_list(&mut projects, path)
    }

    pub fn remove(&self, id: &str) -> Result<Project, String> {
        let mut projects = self.lock()?;
        remove_project_from_list(&mut projects, id)
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
        Project, add_project_to_list, normalize_project_dir, project_name, remove_project_from_list,
    };
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
}
