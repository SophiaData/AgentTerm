use crate::projects::{ProjectStore, normalize_project_dir};
use serde::Serialize;
use std::fs::{self, File, Metadata};
use std::io::Read;
use std::path::{Path, PathBuf};

const ENTRY_LIMIT: usize = 1000;
const PREVIEW_LIMIT: u64 = 1024 * 1024;

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EntryKind {
    Directory,
    File,
    Unavailable,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    name: String,
    path: String,
    kind: EntryKind,
    bytes: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectoryListing {
    entries: Vec<FileEntry>,
    truncated: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilePreview {
    path: String,
    content: String,
    bytes: u64,
}

fn registered_root(projects: &ProjectStore, project_id: &str) -> Result<PathBuf, String> {
    let project = projects
        .list()?
        .into_iter()
        .find(|project| project.id == project_id)
        .ok_or_else(|| "项目已移除，请重新选择项目".to_string())?;
    let root = normalize_project_dir(&project.path)?;
    if root != Path::new(&project.path) {
        return Err("项目位置已变化，请重新添加项目".into());
    }
    Ok(root)
}

fn ordinary_component(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', ':'])
        && !name.chars().any(char::is_control)
}

fn is_link(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn resolve(root: &Path, relative: &str) -> Result<PathBuf, String> {
    if relative.len() > 4096 {
        return Err("文件路径过长".into());
    }
    let mut requested = root.to_path_buf();
    if !relative.is_empty() {
        for component in relative.split('/') {
            if !ordinary_component(component) {
                return Err("只能查看当前项目内的相对路径".into());
            }
            requested.push(component);
            let metadata = fs::symlink_metadata(&requested)
                .map_err(|_| "文件不存在或无法访问，请刷新目录".to_string())?;
            if is_link(&metadata) {
                return Err("链接文件或目录请在系统资源管理器中查看".into());
            }
        }
    }
    let canonical = requested
        .canonicalize()
        .map_err(|_| "文件不存在或无法访问，请刷新目录".to_string())?;
    if !canonical.starts_with(root) {
        return Err("文件位置超出当前项目".into());
    }
    Ok(canonical)
}

fn list_directory(root: &Path, relative: &str) -> Result<DirectoryListing, String> {
    let directory = resolve(root, relative)?;
    let iterator =
        fs::read_dir(&directory).map_err(|_| "无法读取此目录，请检查访问权限后刷新".to_string())?;
    let mut entries = Vec::new();
    let mut truncated = false;
    for (index, item) in iterator.enumerate() {
        if index >= ENTRY_LIMIT {
            truncated = true;
            break;
        }
        let item = item.map_err(|_| "目录内容发生变化，请刷新".to_string())?;
        let Some(name) = item.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let metadata = fs::symlink_metadata(item.path()).ok();
        let kind = match &metadata {
            Some(metadata) if ordinary_component(&name) && !is_link(metadata) => {
                if metadata.is_dir() {
                    EntryKind::Directory
                } else if metadata.is_file() {
                    EntryKind::File
                } else {
                    EntryKind::Unavailable
                }
            }
            _ => EntryKind::Unavailable,
        };
        let bytes = if kind == EntryKind::File {
            metadata.map(|metadata| metadata.len())
        } else {
            None
        };
        let path = if relative.is_empty() {
            name.clone()
        } else {
            format!("{relative}/{name}")
        };
        entries.push(FileEntry {
            name,
            path,
            kind,
            bytes,
        });
    }
    if resolve(root, relative)? != directory {
        return Err("目录位置已变化，请刷新".into());
    }
    entries.sort_by(|left, right| {
        (left.kind != EntryKind::Directory)
            .cmp(&(right.kind != EntryKind::Directory))
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
            .then_with(|| left.name.cmp(&right.name))
    });
    Ok(DirectoryListing { entries, truncated })
}

fn preview_file(root: &Path, relative: &str) -> Result<FilePreview, String> {
    let path = resolve(root, relative)?;
    let metadata =
        fs::symlink_metadata(&path).map_err(|_| "文件不存在或无法访问，请刷新目录".to_string())?;
    if !metadata.is_file() || is_link(&metadata) {
        return Err("请选择普通文本文件".into());
    }
    if metadata.len() > PREVIEW_LIMIT {
        return Err("文件超过 1 MiB，请在系统资源管理器中查看".into());
    }
    let file = File::open(&path).map_err(|_| "无法读取文件，请检查访问权限".to_string())?;
    if !file.metadata().map_err(|_| "无法读取文件信息")?.is_file() {
        return Err("请选择普通文本文件".into());
    }
    let mut bytes = Vec::new();
    file.take(PREVIEW_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "读取文件失败，请刷新后重试".to_string())?;
    if bytes.len() as u64 > PREVIEW_LIMIT {
        return Err("文件超过 1 MiB，请在系统资源管理器中查看".into());
    }
    if resolve(root, relative)? != path {
        return Err("文件位置已变化，请重新选择".into());
    }
    let size = bytes.len() as u64;
    let content = String::from_utf8(bytes)
        .map_err(|_| "此文件不是 UTF-8 文本，请在系统资源管理器中查看".to_string())?;
    if content
        .chars()
        .any(|value| value.is_control() && !matches!(value, '\n' | '\r' | '\t'))
    {
        return Err("此文件不是纯文本，请在系统资源管理器中查看".into());
    }
    Ok(FilePreview {
        path: relative.to_owned(),
        content: content
            .strip_prefix('\u{feff}')
            .unwrap_or(&content)
            .to_owned(),
        bytes: size,
    })
}

fn containing_directory(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let target = resolve(root, relative)?;
    if target.is_dir() {
        Ok(target)
    } else if target.is_file() {
        target
            .parent()
            .filter(|parent| parent.starts_with(root))
            .map(Path::to_path_buf)
            .ok_or_else(|| "文件位置超出当前项目".into())
    } else {
        Err("请选择普通文件或目录".into())
    }
}

fn open_directory(directory: &Path) -> Result<(), String> {
    #[cfg(windows)]
    let mut command = {
        use std::os::windows::process::CommandExt;
        let explorer = PathBuf::from(
            std::env::var_os("SystemRoot").ok_or_else(|| "系统资源管理器位置不可用".to_string())?,
        )
        .join("explorer.exe");
        let raw = directory.to_string_lossy();
        let display = if let Some(rest) = raw.strip_prefix(r"\\?\UNC\") {
            format!(r"\\{rest}")
        } else {
            raw.strip_prefix(r"\\?\").unwrap_or(&raw).to_owned()
        };
        let mut command = std::process::Command::new(explorer);
        command.arg(display).creation_flags(0x08000000);
        command
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new("/usr/bin/open");
        command.arg(directory);
        command
    };
    #[cfg(any(windows, target_os = "macos"))]
    {
        let mut child = command
            .spawn()
            .map_err(|_| "无法打开系统资源管理器，请稍后重试".to_string())?;
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = directory;
        Err("当前平台不支持打开系统资源管理器".into())
    }
}

#[tauri::command]
pub async fn list_project_files(
    projects: tauri::State<'_, ProjectStore>,
    project_id: String,
    relative_path: String,
) -> Result<DirectoryListing, String> {
    let root = registered_root(&projects, &project_id)?;
    tauri::async_runtime::spawn_blocking(move || list_directory(&root, &relative_path))
        .await
        .map_err(|_| "目录读取中断，请重试".to_string())?
}

#[tauri::command]
pub async fn preview_project_file(
    projects: tauri::State<'_, ProjectStore>,
    project_id: String,
    relative_path: String,
) -> Result<FilePreview, String> {
    let root = registered_root(&projects, &project_id)?;
    tauri::async_runtime::spawn_blocking(move || preview_file(&root, &relative_path))
        .await
        .map_err(|_| "文件读取中断，请重试".to_string())?
}

#[tauri::command]
pub async fn reveal_project_file(
    projects: tauri::State<'_, ProjectStore>,
    project_id: String,
    relative_path: String,
) -> Result<(), String> {
    let root = registered_root(&projects, &project_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        open_directory(&containing_directory(&root, &relative_path)?)
    })
    .await
    .map_err(|_| "打开目录中断，请重试".to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "agentterm-files-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir(&path).unwrap();
            Self(path.canonicalize().unwrap())
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn only_registered_projects_can_be_browsed() {
        let fixture = Fixture::new();
        let projects = ProjectStore::default();
        assert!(registered_root(&projects, &fixture.0.to_string_lossy()).is_err());
        let project = projects.add(&fixture.0.to_string_lossy()).unwrap();
        assert_eq!(registered_root(&projects, &project.id).unwrap(), fixture.0);
        projects.remove(&project.id).unwrap();
        assert!(registered_root(&projects, &project.id).is_err());
    }

    #[test]
    fn lists_one_level_directories_first_and_previews_chinese_text() {
        let fixture = Fixture::new();
        fs::create_dir(fixture.0.join("SQL 文档")).unwrap();
        let body = "\u{feff}-- 中文\nselect '<script>alert(1)</script>';\n";
        fs::write(fixture.0.join("SQL 文档/测试.sql"), body).unwrap();
        fs::write(fixture.0.join("README.md"), "说明").unwrap();
        let listing = list_directory(&fixture.0, "").unwrap();
        assert_eq!(listing.entries.len(), 2);
        assert_eq!(listing.entries[0].kind, EntryKind::Directory);
        assert_eq!(listing.entries[0].path, "SQL 文档");
        assert!(!listing.truncated);
        let nested = list_directory(&fixture.0, "SQL 文档").unwrap();
        assert_eq!(nested.entries[0].path, "SQL 文档/测试.sql");
        let preview = preview_file(&fixture.0, &nested.entries[0].path).unwrap();
        assert_eq!(preview.content, body.trim_start_matches('\u{feff}'));
        assert_eq!(preview.bytes, body.len() as u64);
        assert_eq!(
            fs::read_to_string(fixture.0.join(&preview.path)).unwrap(),
            body
        );
        assert_eq!(
            containing_directory(&fixture.0, &preview.path).unwrap(),
            fixture.0.join("SQL 文档")
        );
    }

    #[test]
    fn rejects_traversal_absolute_and_alternate_stream_paths() {
        let fixture = Fixture::new();
        for path in [
            "../outside",
            "/etc/passwd",
            "C:/test",
            "a/../b",
            "a\\b",
            "a:b",
            "a//b",
        ] {
            assert!(resolve(&fixture.0, path).is_err(), "{path}");
        }
    }

    #[test]
    fn refuses_large_binary_and_non_regular_previews() {
        let fixture = Fixture::new();
        fs::write(fixture.0.join("binary"), [0, 1, 2]).unwrap();
        fs::write(fixture.0.join("invalid.txt"), [0xff, 0xfe]).unwrap();
        fs::write(
            fixture.0.join("large.txt"),
            vec![b'a'; PREVIEW_LIMIT as usize + 1],
        )
        .unwrap();
        fs::write(fixture.0.join("empty.txt"), "").unwrap();
        for file in ["binary", "invalid.txt", "large.txt", ""] {
            assert!(preview_file(&fixture.0, file).is_err(), "{file}");
        }
        assert_eq!(preview_file(&fixture.0, "empty.txt").unwrap().content, "");
        assert!(preview_file(&fixture.0, "missing.txt").is_err());
    }

    #[test]
    fn limits_large_directory_responses() {
        let fixture = Fixture::new();
        for index in 0..=ENTRY_LIMIT {
            File::create(fixture.0.join(format!("file-{index}"))).unwrap();
        }
        let listing = list_directory(&fixture.0, "").unwrap();
        assert_eq!(listing.entries.len(), ENTRY_LIMIT);
        assert!(listing.truncated);
    }

    #[cfg(unix)]
    #[test]
    fn does_not_follow_files_or_directories_linked_outside_the_project() {
        let fixture = Fixture::new();
        let outside = Fixture::new();
        fs::write(outside.0.join("outside.txt"), "outside").unwrap();
        std::os::unix::fs::symlink(outside.0.join("outside.txt"), fixture.0.join("linked.txt"))
            .unwrap();
        std::os::unix::fs::symlink(&outside.0, fixture.0.join("linked-dir")).unwrap();
        let listing = list_directory(&fixture.0, "").unwrap();
        assert!(
            listing
                .entries
                .iter()
                .all(|entry| entry.kind == EntryKind::Unavailable)
        );
        assert!(preview_file(&fixture.0, "linked.txt").is_err());
        assert!(preview_file(&fixture.0, "linked-dir/outside.txt").is_err());
        assert!(list_directory(&fixture.0, "linked-dir").is_err());
        assert!(containing_directory(&fixture.0, "linked-dir").is_err());
    }
}
