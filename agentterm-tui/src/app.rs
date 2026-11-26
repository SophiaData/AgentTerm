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
        let error = store.load_error.clone();
        let mut app = Self {
            store,
            tools,
            screen: Screen::Projects,
            project_cursor: 0,
            tool_cursor: 0,
            prompt: Prompt::None,
            status: String::new(),
            error,
            pending_launch: None,
        };
        app.status = app.default_status();

        if app.store.projects.is_empty() && app.error.is_none() {
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

        self.error = self.store.load_error.clone();
        if self.error.is_some() {
            return matches!(code, KeyCode::Char('q') | KeyCode::Esc);
        }

        if let Prompt::AddProject(buffer) = &mut self.prompt {
            match code {
                KeyCode::Enter => {
                    let text = buffer.clone();
                    self.prompt = Prompt::None;
                    match self.store.add(&text) {
                        Ok(()) => match self.store.save() {
                            Ok(()) => {
                                self.project_cursor = self.store.projects.len().saturating_sub(1);
                                self.status = "已添加项目".to_string();
                            }
                            Err(message) => {
                                self.store.projects.pop();
                                self.set_error(message);
                            }
                        },
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
                        match self.store.save() {
                            Ok(()) => {
                                self.status =
                                    format!("已移除 {}（文件夹保留）", removed.path.display());
                                self.project_cursor = self
                                    .project_cursor
                                    .min(self.store.projects.len().saturating_sub(1));
                            }
                            Err(message) => {
                                self.store.projects.insert(self.project_cursor, removed);
                                self.set_error(message);
                            }
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

        let previous_tool = self.store.projects[self.project_cursor].last_tool.clone();
        self.store.set_last_tool(self.project_cursor, &tool_id);
        // Preferred tool is only a hint: a failed save rolls back the in-memory
        // change and reports it, but never blocks launching the CLI.
        if let Err(error) = self.store.save() {
            if let Some(project) = self.store.projects.get_mut(self.project_cursor) {
                project.last_tool = previous_tool;
            }
            self.set_error(error);
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

    fn temp_dir(label: &str) -> String {
        let dir = std::env::temp_dir().join(format!(
            "agentterm-app-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.to_string_lossy().into_owned()
    }

    /// An app whose store lives in a private temp file, never the real config.
    fn isolated_app(label: &str) -> App {
        let cfg = std::env::temp_dir().join(format!(
            "agentterm-app-cfg-{label}-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        App::from_store(ProjectStore::with_path(cfg))
    }

    fn app_with_project() -> App {
        let mut app = isolated_app("solo");
        app.store.projects.clear();
        app.add_project_path(&temp_dir("solo"));
        app
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
    fn project_mutations_roll_back_when_the_config_cannot_be_saved() {
        let bound = std::path::PathBuf::from(temp_dir("save-fails")).join("projects.json");
        let mut app = App::from_store(ProjectStore::with_path(bound.clone()));
        // Simulate a target that becomes unwritable after the initial load.
        std::fs::create_dir(&bound).unwrap();
        let project = temp_dir("save-fails-project");

        app.add_project_path(&project);
        assert!(app.store.projects.is_empty());
        assert!(app.error.as_deref().is_some_and(|e| e.contains("保存配置")));

        app.store.add(&project).unwrap();
        app.handle_key(KeyCode::Char('d'));
        app.handle_key(KeyCode::Char('y'));
        assert_eq!(app.store.projects.len(), 1);
        assert_eq!(app.project_cursor, 0);
        assert!(app.error.as_deref().is_some_and(|e| e.contains("保存配置")));
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
