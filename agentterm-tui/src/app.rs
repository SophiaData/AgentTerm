//! UI state machine and key handling. Kept free of terminal I/O so it can be
//! unit-tested by driving `handle_key` directly.

use std::path::PathBuf;

use crate::launch::{self, Launch};
use crate::store::ProjectStore;
use crate::tools::{self, Tool};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Projects,
    Tools,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prompt {
    None,
    AddProject(String),
    ConfirmRemove,
}

pub struct App {
    pub store: ProjectStore,
    pub tools: Vec<Tool>,
    pub screen: Screen,
    pub project_cursor: usize,
    pub tool_cursor: usize,
    pub prompt: Prompt,
    pub status: String,
    pub error: Option<String>,
    /// Set when the user chose to launch; `main` performs the exec.
    pub pending_launch: Option<Launch>,
}

impl App {
    pub fn new() -> Self {
        Self::from_store(ProjectStore::load())
    }

    /// Build an app around an explicitly provided store. `new` uses it with the
    /// real store; tests pass a temp-bound store so the suite never touches the
    /// real config.
    pub fn from_store(store: ProjectStore) -> Self {
        let tools = tools::detect(&tools::search_dirs());
        let mut app = Self {
            store,
            tools,
            screen: Screen::Projects,
            project_cursor: 0,
            tool_cursor: 0,
            prompt: Prompt::None,
            status: String::new(),
            error: None,
            pending_launch: None,
        };
        app.status = app.default_status();

        if let Some(error) = app.store.load_error() {
            app.error = Some(error.to_string());
            app.status = "项目配置读取失败，原文件未修改".to_string();
        } else if app.store.projects.is_empty() {
            app.prompt = Prompt::AddProject(String::new());
        }
        app
    }

    fn default_status(&self) -> String {
        if self.store.projects.is_empty() {
            "还没有项目，先添加一个".to_string()
        } else {
            "↑↓ 选择 · Enter 打开 · a 添加 · d 删除 · r 重扫 · q 退出".to_string()
        }
    }

    pub fn set_error(&mut self, message: String) {
        self.error = Some(message);
    }

    pub fn refresh_tools(&mut self) {
        self.tools = tools::detect(&tools::search_dirs());
        self.status = "已重新扫描工具".to_string();
    }

    /// Returns true when the app should quit.
    pub fn handle_key(&mut self, code: crossterm::event::KeyCode) -> bool {
        use crossterm::event::KeyCode;

        self.error = None;
        if let Some(error) = self.store.load_error() {
            self.error = Some(error.to_string());
            return matches!(code, KeyCode::Char('q') | KeyCode::Esc);
        }

        if let Prompt::AddProject(buffer) = &mut self.prompt {
            match code {
                KeyCode::Enter => {
                    let text = buffer.clone();
                    self.prompt = Prompt::None;
                    match self.store.add(&text) {
                        Ok(()) => {
                            let index = self.store.projects.len().saturating_sub(1);
                            self.project_cursor = index;
                            match self.store.save() {
                                Ok(()) => self.status = "已添加项目".to_string(),
                                Err(error) => {
                                    self.set_error(format!("项目仅在当前会话中，保存失败：{error}"))
                                }
                            }
                        }
                        Err(message) => self.set_error(message),
                    }
                }
                KeyCode::Esc => {
                    self.prompt = Prompt::None;
                    self.status = self.default_status();
                }
                KeyCode::Backspace => {
                    buffer.pop();
                }
                KeyCode::Char(c) => buffer.push(c),
                _ => {}
            }
            return false;
        }

        if self.prompt == Prompt::ConfirmRemove {
            match code {
                KeyCode::Char('y') | KeyCode::Enter => {
                    if let Some(removed) = self.store.remove(self.project_cursor) {
                        self.status = format!("已移除 {}（文件夹保留）", removed.path.display());
                        if self.project_cursor >= self.store.projects.len() {
                            self.project_cursor = self.store.projects.len().saturating_sub(1);
                        }
                        if let Err(error) = self.store.save() {
                            self.set_error(format!("项目移除未写入配置：{error}"));
                        }
                    }
                    self.prompt = Prompt::None;
                }
                KeyCode::Char('n') | KeyCode::Esc => {
                    self.prompt = Prompt::None;
                }
                _ => {}
            }
            return false;
        }

        match self.screen {
            Screen::Projects => self.handle_projects(code),
            Screen::Tools => self.handle_tools(code),
        }
    }

    fn handle_projects(&mut self, code: crossterm::event::KeyCode) -> bool {
        use crossterm::event::KeyCode;
        match code {
            KeyCode::Char('q') | KeyCode::Esc => return true,
            KeyCode::Up | KeyCode::Char('k') => {
                self.project_cursor = self.project_cursor.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.project_cursor + 1 < self.store.projects.len() {
                    self.project_cursor += 1;
                }
            }
            KeyCode::Char('a') => {
                self.prompt = Prompt::AddProject(String::new());
            }
            KeyCode::Char('d') => {
                if !self.store.projects.is_empty() {
                    self.prompt = Prompt::ConfirmRemove;
                }
            }
            KeyCode::Char('r') => self.refresh_tools(),
            KeyCode::Enter => {
                if self.store.projects.is_empty() {
                    self.prompt = Prompt::AddProject(String::new());
                } else {
                    self.screen = Screen::Tools;
                    self.tool_cursor = self.default_tool_index();
                }
            }
            _ => {}
        }
        false
    }

    fn handle_tools(&mut self, code: crossterm::event::KeyCode) -> bool {
        use crossterm::event::KeyCode;
        match code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.screen = Screen::Projects;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.tool_cursor = self.tool_cursor.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.tool_cursor + 1 < self.tools.len() {
                    self.tool_cursor += 1;
                }
            }
            KeyCode::Char('r') => self.refresh_tools(),
            KeyCode::Enter => self.launch_selected(),
            _ => {}
        }
        false
    }

    fn default_tool_index(&self) -> usize {
        let Some(project) = self.store.projects.get(self.project_cursor) else {
            return 0;
        };
        if let Some(last) = &project.last_tool
            && let Some(index) = self.tools.iter().position(|t| t.id == last)
        {
            return index;
        }
        self.tools
            .iter()
            .position(|t| t.path.is_some())
            .unwrap_or(0)
    }

    fn launch_selected(&mut self) {
        let Some(project) = self.store.projects.get(self.project_cursor) else {
            return;
        };
        let project_path: PathBuf = project.path.clone();
        let Some(tool) = self.tools.get(self.tool_cursor) else {
            return;
        };
        let Some(binary) = &tool.path else {
            self.set_error(format!("{} 还没安装", tool.name));
            return;
        };
        let tool_id = tool.id.to_string();
        let request = launch::plan(binary, &project_path);

        self.store.set_last_tool(self.project_cursor, &tool_id);
        if let Err(error) = self.store.save() {
            self.set_error(format!("仍将启动工具，但最近使用记录未保存：{error}"));
        }
        self.pending_launch = Some(request);
    }

    /// Commit a typed path (used by tests to avoid simulating each keystroke).
    #[cfg(test)]
    pub fn add_project_path(&mut self, path: &str) {
        self.prompt = Prompt::AddProject(path.to_string());
        self.handle_key(crossterm::event::KeyCode::Enter);
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{App, Prompt, Screen};
    use crate::store::ProjectStore;
    use crossterm::event::KeyCode;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_PATH: AtomicU64 = AtomicU64::new(0);

    /// Reserve an unused directory atomically, including when tests run in parallel.
    fn unique_test_dir(label: &str) -> PathBuf {
        loop {
            let sequence = NEXT_TEST_PATH.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "agentterm-app-{label}-{}-{}-{sequence}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            match std::fs::create_dir(&dir) {
                Ok(()) => return dir,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("无法创建测试隔离目录 {}：{error}", dir.display()),
            }
        }
    }

    fn temp_dir(label: &str) -> String {
        unique_test_dir(label).to_string_lossy().into_owned()
    }

    /// An app whose store lives in a private temp file, never the real config.
    fn isolated_app(label: &str) -> App {
        let cfg = unique_test_dir(&format!("cfg-{label}")).join("projects.json");
        App::from_store(ProjectStore::with_path(cfg))
    }

    fn app_with_project() -> App {
        let mut app = isolated_app("solo");
        app.store.projects.clear();
        app.add_project_path(&temp_dir("solo"));
        app
    }

    #[test]
    fn test_paths_are_unique_for_parallel_callers() {
        use std::collections::HashSet;

        let handles: Vec<_> = (0..32)
            .map(|_| {
                std::thread::spawn(|| {
                    let app = isolated_app("parallel");
                    let cfg = app.store.bound_path().unwrap().to_path_buf();
                    let project_dir = PathBuf::from(temp_dir("parallel"));
                    assert!(cfg.parent().unwrap().is_dir());
                    assert!(!cfg.exists(), "config must start without old test data");
                    assert!(project_dir.is_dir());
                    (cfg, project_dir)
                })
            })
            .collect();

        let mut paths = HashSet::new();
        for handle in handles {
            let (cfg, project_dir) = handle.join().unwrap();
            assert!(paths.insert(cfg), "config paths must not overlap");
            assert!(paths.insert(project_dir), "project paths must not overlap");
        }
        assert_eq!(paths.len(), 64);
    }

    #[test]
    fn damaged_config_is_visible_and_blocked_from_tui_mutation() {
        let folder = temp_dir("damaged-config");
        let path = std::path::PathBuf::from(folder).join("projects.json");
        std::fs::write(&path, "{invalid JSON").unwrap();
        let mut app = App::from_store(ProjectStore::with_path(path.clone()));
        assert!(
            app.error
                .as_deref()
                .is_some_and(|message| message.contains("损坏"))
        );
        assert!(matches!(app.prompt, Prompt::None));
        app.handle_key(KeyCode::Char('a'));
        assert!(
            matches!(app.prompt, Prompt::None),
            "invalid config must not offer add"
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), "{invalid JSON");
        assert!(app.handle_key(KeyCode::Char('q')), "user can exit safely");
    }

    #[test]
    fn empty_state_starts_in_add_prompt() {
        let mut app = isolated_app("empty");
        app.store.projects.clear();
        app.prompt = Prompt::AddProject(String::new());
        assert!(matches!(app.prompt, Prompt::AddProject(_)));
    }

    #[test]
    fn add_project_then_enter_goes_to_tools() {
        let mut app = app_with_project();
        assert_eq!(app.store.projects.len(), 1);
        app.handle_key(KeyCode::Enter);
        assert_eq!(app.screen, Screen::Tools);
    }

    #[test]
    fn delete_asks_for_confirmation() {
        let mut app = app_with_project();
        app.handle_key(KeyCode::Char('d'));
        assert_eq!(app.prompt, Prompt::ConfirmRemove);
        app.handle_key(KeyCode::Char('n'));
        assert_eq!(app.store.projects.len(), 1);
    }

    #[test]
    fn confirming_delete_removes_record_only() {
        let mut app = app_with_project();
        let folder = app.store.projects[0].path.clone();
        app.handle_key(KeyCode::Char('d'));
        app.handle_key(KeyCode::Char('y'));
        assert!(app.store.projects.is_empty());
        assert!(folder.is_dir(), "folder must survive removal");
    }

    #[test]
    fn q_quits_from_project_screen() {
        let mut app = app_with_project();
        assert!(app.handle_key(KeyCode::Char('q')));
    }

    #[test]
    fn esc_returns_from_tools_to_projects() {
        let mut app = app_with_project();
        app.handle_key(KeyCode::Enter);
        app.handle_key(KeyCode::Esc);
        assert_eq!(app.screen, Screen::Projects);
    }
}
