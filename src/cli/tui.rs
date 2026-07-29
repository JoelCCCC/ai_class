use std::io::{self, stdout};
use std::path::PathBuf;

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{
        Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
    },
    Frame, Terminal,
};
use tui_textarea::TextArea;

use crate::agent::AgentEvent;

#[derive(Clone, Copy, PartialEq)]
pub enum Theme {
    Dark,
    Light,
}

struct ThemeColors {
    user: Color,
    ai: Color,
    tool: Color,
    tool_result: Color,
    system: Color,
    separator: Color,
    error: Color,
}

impl ThemeColors {
    fn new(theme: Theme) -> Self {
        match theme {
            Theme::Dark => Self {
                user: Color::Rgb(122, 162, 247),
                ai: Color::Rgb(158, 206, 106),
                tool: Color::Rgb(187, 154, 247),
                tool_result: Color::Rgb(86, 95, 137),
                system: Color::Rgb(115, 125, 170),
                separator: Color::Rgb(54, 59, 80),
                error: Color::Rgb(247, 118, 142),
            },
            Theme::Light => Self {
                user: Color::Rgb(0, 102, 204),
                ai: Color::Rgb(30, 130, 30),
                tool: Color::Rgb(130, 80, 200),
                tool_result: Color::Rgb(140, 140, 140),
                system: Color::Rgb(120, 120, 120),
                separator: Color::Rgb(200, 200, 200),
                error: Color::Rgb(200, 30, 30),
            },
        }
    }
}

#[derive(Clone)]
pub enum ChatLine {
    User(String),
    Assistant(String),
    ToolStart { name: String },
    ToolResult { content: String, is_error: bool },
    System(String),
    Separator,
}

#[derive(Clone)]
pub enum Command {
    SwitchSession(String),
    SwitchModel(String),
    SwitchExpert(String),
    SwitchTeam(String),
    ConfigureApi { key: String },
    ClearChat,
    TogglePlanning,
    ToggleTheme,
    Quit,
}

enum SubMenu {
    None,
    Main {
        selected: usize,
    },
    SessionPicker {
        sessions: Vec<String>,
        selected: usize,
    },
    ModelPicker {
        models: Vec<String>,
        selected: usize,
    },
    ExpertPicker {
        experts: Vec<(String, String)>,
        selected: usize,
    },
    TeamPicker {
        teams: Vec<(String, String)>,
        selected: usize,
    },
    ApiConfig {
        input: String,
    },
    Help,
}

fn theme_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".config")
        .join("ai-code")
        .join("theme")
}

fn load_theme() -> Theme {
    let path = theme_path();
    if let Ok(content) = std::fs::read_to_string(&path) {
        match content.trim() {
            "light" => Theme::Light,
            _ => Theme::Dark,
        }
    } else {
        Theme::Dark
    }
}

fn save_theme(theme: Theme) {
    let path = theme_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(
        &path,
        if theme == Theme::Dark {
            "dark"
        } else {
            "light"
        },
    );
}

pub struct Tui {
    terminal: Terminal<ratatui::backend::CrosstermBackend<io::Stdout>>,
    app: App,
}

struct App {
    messages: Vec<ChatLine>,
    input: TextArea<'static>,
    scroll_offset: usize,
    is_thinking: bool,
    status: String,
    quit: bool,
    error: Option<String>,
    pending_input: Option<String>,
    confirm_prompt: Option<String>,
    sub_menu: SubMenu,
    pending_command: Option<Command>,
    model_info: String,
    current_api_url: String,
    is_planning: bool,
    theme: Theme,
    colors: ThemeColors,
}

const MENU_ITEMS: &[&str] = &[
    "Switch Session",
    "Switch Model",
    "Switch Expert",
    "Select Team",
    "Configure API",
    "Toggle Planning Mode",
    "Toggle Theme",
    "Clear Chat",
    "Help",
    "Quit",
];

impl App {
    fn new() -> Self {
        let theme = load_theme();
        let colors = ThemeColors::new(theme);
        let mut input = TextArea::default();
        input.set_block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(colors.separator)),
        );
        input.set_placeholder_text("Type a message...");
        input.set_placeholder_style(Style::default().fg(colors.tool_result));
        input.set_cursor_line_style(Style::default());

        Self {
            messages: vec![
                ChatLine::System("AI Code Agent — type /help for commands, Ctrl+X for menu".into()),
                ChatLine::Separator,
            ],
            input,
            scroll_offset: 0,
            is_thinking: false,
            status: String::new(),
            quit: false,
            error: None,
            pending_input: None,
            confirm_prompt: None,
            sub_menu: SubMenu::None,
            pending_command: None,
            model_info: String::new(),
            current_api_url: String::new(),
            is_planning: false,
            theme,
            colors,
        }
    }

    fn handle_input(&mut self, event: Event) {
        if let Event::Key(key) = &event {
            if key.kind != KeyEventKind::Press {
                return;
            }
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('x') {
                if matches!(self.sub_menu, SubMenu::None) {
                    self.open_menu();
                } else {
                    self.sub_menu = SubMenu::None;
                }
                return;
            }
        }

        match &self.sub_menu {
            SubMenu::None => self.handle_chat_input(event),
            SubMenu::Main { .. } => self.handle_menu_input(event),
            SubMenu::SessionPicker { .. } => self.handle_session_picker_input(event),
            SubMenu::ModelPicker { .. } => self.handle_model_picker_input(event),
            SubMenu::ExpertPicker { .. } => self.handle_expert_picker_input(event),
            SubMenu::TeamPicker { .. } => self.handle_team_picker_input(event),
            SubMenu::ApiConfig { .. } => self.handle_api_config_input(event),
            SubMenu::Help => self.handle_help_input(event),
        }
    }

    fn handle_chat_input(&mut self, event: Event) {
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Enter => {
                    let text: String = self.input.lines().join("\n").trim().to_string();
                    if !text.is_empty() {
                        self.scroll_offset = 0;
                        self.messages.push(ChatLine::User(text.clone()));
                        self.pending_input = Some(text);
                        self.reset_input();
                    }
                }
                KeyCode::Esc => self.quit = true,
                KeyCode::PageUp => self.scroll_offset = self.scroll_offset.saturating_add(5),
                KeyCode::PageDown => self.scroll_offset = self.scroll_offset.saturating_sub(5),
                _ => {
                    self.input.input(key);
                }
            },
            Event::Mouse(mouse) => match mouse.kind {
                event::MouseEventKind::ScrollUp => {
                    self.scroll_offset = self.scroll_offset.saturating_add(1);
                }
                event::MouseEventKind::ScrollDown => {
                    self.scroll_offset = self.scroll_offset.saturating_sub(1);
                }
                _ => {}
            },
            Event::Resize(_, _) => {}
            _ => {}
        }
    }

    fn reset_input(&mut self) {
        self.input = TextArea::default();
        self.input.set_block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(self.colors.separator)),
        );
        self.input.set_placeholder_text("Type a message...");
        self.input.set_placeholder_style(Style::default().fg(self.colors.tool_result));
        self.input.set_cursor_line_style(Style::default());
    }

    fn open_menu(&mut self) {
        self.sub_menu = SubMenu::Main { selected: 0 };
    }

    fn handle_menu_input(&mut self, event: Event) {
        let Event::Key(key) = event else { return };
        if key.kind != KeyEventKind::Press {
            return;
        }
        if let SubMenu::Main { ref mut selected } = self.sub_menu {
            match key.code {
                KeyCode::Esc => self.sub_menu = SubMenu::None,
                KeyCode::Up => *selected = selected.saturating_sub(1),
                KeyCode::Down => *selected = (*selected + 1).min(MENU_ITEMS.len() - 1),
                KeyCode::Enter => match *selected {
                    0 => {
                        let sessions = list_sessions();
                        if sessions.is_empty() {
                            self.messages
                                .push(ChatLine::System("No saved sessions found.".into()));
                            self.sub_menu = SubMenu::None;
                        } else {
                            self.sub_menu = SubMenu::SessionPicker {
                                sessions,
                                selected: 0,
                            };
                        }
                    }
                    1 => {
                        let models = list_available_models(&self.current_api_url);
                        if models.is_empty() {
                            self.messages.push(ChatLine::System(
                                "No models found. Run `ollama pull llama3.2` first.".into(),
                            ));
                            self.sub_menu = SubMenu::None;
                        } else {
                            self.sub_menu = SubMenu::ModelPicker {
                                models,
                                selected: 0,
                            };
                        }
                    }
                    2 => {
                        let experts = list_experts();
                        if experts.is_empty() {
                            self.messages
                                .push(ChatLine::System("No expert profiles found.".into()));
                            self.sub_menu = SubMenu::None;
                        } else {
                            self.sub_menu = SubMenu::ExpertPicker {
                                experts,
                                selected: 0,
                            };
                        }
                    }
                    3 => {
                        let teams = list_teams_simple();
                        if teams.is_empty() {
                            self.messages
                                .push(ChatLine::System("No team configurations found.".into()));
                            self.sub_menu = SubMenu::None;
                        } else {
                            self.sub_menu = SubMenu::TeamPicker { teams, selected: 0 };
                        }
                    }
                    4 => {
                        self.sub_menu = SubMenu::ApiConfig {
                            input: String::new(),
                        }
                    }
                    5 => {
                        self.pending_command = Some(Command::TogglePlanning);
                        self.sub_menu = SubMenu::None;
                    }
                    6 => {
                        self.pending_command = Some(Command::ToggleTheme);
                        self.sub_menu = SubMenu::None;
                    }
                    7 => {
                        self.pending_command = Some(Command::ClearChat);
                        self.sub_menu = SubMenu::None;
                    }
                    8 => self.sub_menu = SubMenu::Help,
                    9 => {
                        self.pending_command = Some(Command::Quit);
                        self.sub_menu = SubMenu::None;
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }

    fn handle_session_picker_input(&mut self, event: Event) {
        let Event::Key(key) = event else { return };
        if key.kind != KeyEventKind::Press {
            return;
        }
        if let SubMenu::SessionPicker {
            ref sessions,
            ref mut selected,
        } = self.sub_menu
        {
            match key.code {
                KeyCode::Esc => self.open_menu(),
                KeyCode::Up => *selected = selected.saturating_sub(1),
                KeyCode::Down => *selected = (*selected + 1).min(sessions.len().saturating_sub(1)),
                KeyCode::Enter => {
                    let name = sessions[*selected].clone();
                    self.pending_command = Some(Command::SwitchSession(name));
                    self.sub_menu = SubMenu::None;
                }
                _ => {}
            }
        }
    }

    fn handle_model_picker_input(&mut self, event: Event) {
        let Event::Key(key) = event else { return };
        if key.kind != KeyEventKind::Press {
            return;
        }
        if let SubMenu::ModelPicker {
            ref models,
            ref mut selected,
        } = self.sub_menu
        {
            match key.code {
                KeyCode::Esc => self.open_menu(),
                KeyCode::Up => *selected = selected.saturating_sub(1),
                KeyCode::Down => *selected = (*selected + 1).min(models.len().saturating_sub(1)),
                KeyCode::Enter => {
                    let name = models[*selected].clone();
                    self.pending_command = Some(Command::SwitchModel(name));
                    self.sub_menu = SubMenu::None;
                }
                _ => {}
            }
        }
    }

    fn handle_expert_picker_input(&mut self, event: Event) {
        let Event::Key(key) = event else { return };
        if key.kind != KeyEventKind::Press {
            return;
        }
        if let SubMenu::ExpertPicker {
            ref experts,
            ref mut selected,
        } = self.sub_menu
        {
            match key.code {
                KeyCode::Esc => self.open_menu(),
                KeyCode::Up => *selected = selected.saturating_sub(1),
                KeyCode::Down => *selected = (*selected + 1).min(experts.len().saturating_sub(1)),
                KeyCode::Enter => {
                    let slug = experts[*selected].0.clone();
                    self.pending_command = Some(Command::SwitchExpert(slug));
                    self.sub_menu = SubMenu::None;
                }
                _ => {}
            }
        }
    }

    fn handle_team_picker_input(&mut self, event: Event) {
        let Event::Key(key) = event else { return };
        if key.kind != KeyEventKind::Press {
            return;
        }
        if let SubMenu::TeamPicker {
            ref teams,
            ref mut selected,
        } = self.sub_menu
        {
            match key.code {
                KeyCode::Esc => self.open_menu(),
                KeyCode::Up => *selected = selected.saturating_sub(1),
                KeyCode::Down => *selected = (*selected + 1).min(teams.len().saturating_sub(1)),
                KeyCode::Enter => {
                    let slug = teams[*selected].0.clone();
                    self.pending_command = Some(Command::SwitchTeam(slug));
                    self.sub_menu = SubMenu::None;
                }
                _ => {}
            }
        }
    }

    fn handle_api_config_input(&mut self, event: Event) {
        let Event::Key(key) = event else { return };
        if key.kind != KeyEventKind::Press {
            return;
        }
        if let SubMenu::ApiConfig { ref mut input } = self.sub_menu {
            match key.code {
                KeyCode::Esc => self.open_menu(),
                KeyCode::Enter => {
                    let key_val = input.trim().to_string();
                    if !key_val.is_empty() {
                        self.pending_command = Some(Command::ConfigureApi { key: key_val });
                        self.sub_menu = SubMenu::None;
                    }
                }
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char(c) => {
                    input.push(c);
                }
                _ => {}
            }
        }
    }

    fn handle_help_input(&mut self, event: Event) {
        let Event::Key(key) = event else { return };
        if key.kind != KeyEventKind::Press {
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Enter => self.open_menu(),
            _ => {}
        }
    }

    fn handle_agent_event(&mut self, event: AgentEvent) {
        self.error = None;
        match event {
            AgentEvent::Thinking => {
                self.is_thinking = true;
                self.status = "Thinking...".into();
            }
            AgentEvent::AssistantDelta(delta) => {
                self.is_thinking = false;
                self.status = String::new();
                match self.messages.last_mut() {
                    Some(ChatLine::Assistant(existing)) => existing.push_str(&delta),
                    _ => self.messages.push(ChatLine::Assistant(delta)),
                }
            }
            AgentEvent::ToolCallStart { name, .. } => {
                self.is_thinking = false;
                self.status = format!("Running: {}...", name);
                self.messages.push(ChatLine::ToolStart { name });
            }
            AgentEvent::ToolCallEnd(result) => {
                self.status = String::new();
                self.messages.push(ChatLine::ToolResult {
                    content: result.content,
                    is_error: result.is_error,
                });
            }
            AgentEvent::Done => {
                self.is_thinking = false;
                self.status = String::new();
            }
            AgentEvent::Error(msg) => {
                self.is_thinking = false;
                self.status = String::new();
                self.error = Some(msg);
            }
            AgentEvent::WriteRequest { .. } => {}
        }
    }

    fn take_input(&mut self) -> Option<String> {
        self.pending_input.take()
    }
    fn take_command(&mut self) -> Option<Command> {
        self.pending_command.take()
    }
    fn menu_active(&self) -> bool {
        !matches!(self.sub_menu, SubMenu::None)
    }
}

fn list_sessions() -> Vec<String> {
    let cwd = std::env::current_dir().unwrap_or_default();
    let slug = cwd.to_string_lossy().replace('/', "__");
    let dir = dirs::home_dir()
        .unwrap_or_default()
        .join(".config")
        .join("ai-code")
        .join("sessions")
        .join(&slug);
    let mut names = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    names.push(stem.to_string());
                }
            }
        }
    }
    names.sort();
    names
}

fn list_available_models(api_url: &str) -> Vec<String> {
    let mut all: Vec<String> = Vec::new();
    if let Ok(output) = std::process::Command::new("ollama").args(["list"]).output() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut locals: Vec<String> = stdout
            .lines()
            .skip(1)
            .filter_map(|line| line.split_whitespace().next().map(|s| s.to_string()))
            .collect();
        locals.sort();
        for m in locals {
            all.push(format!("[local] {}", m));
        }
    }
    if all.is_empty() && api_url.contains("localhost") {
        all.push("[local] llama3.2".into());
    }
    if api_url.contains("localhost") || api_url.contains("127.0.0.1") {
        return all;
    }
    let cloud: &[&str] = if api_url.contains("openai.com") {
        &["gpt-4o", "gpt-4-turbo", "gpt-4o-mini"]
    } else if api_url.contains("anthropic.com") {
        &["claude-sonnet-4-20250514", "claude-haiku-3-5-20241022"]
    } else if api_url.contains("deepseek.com") {
        &["deepseek-v4-pro", "deepseek-v4-flash"]
    } else {
        &["gpt-4o", "deepseek-v4-pro"]
    };
    for m in cloud {
        all.push(m.to_string());
    }
    all
}

fn list_experts() -> Vec<(String, String)> {
    let raw = crate::experts::list_profiles();
    let mut items: Vec<(String, String)> = raw.into_iter().map(|(s, p)| (s, p.name)).collect();
    items.insert(0, ("_general".into(), "General (no specialization)".into()));
    items
}

fn list_teams_simple() -> Vec<(String, String)> {
    let raw = crate::teams::list_teams();
    let items: Vec<(String, String)> = raw.into_iter().map(|(s, t)| (s, t.name)).collect();
    if items.is_empty() {
        vec![("_none".into(), "No teams configured".into())]
    } else {
        items
    }
}

impl Tui {
    pub fn new() -> io::Result<Self> {
        enable_raw_mode()?;
        execute!(stdout(), EnterAlternateScreen, EnableMouseCapture)?;
        let backend = ratatui::backend::CrosstermBackend::new(stdout());
        let terminal = Terminal::new(backend)?;
        Ok(Self {
            terminal,
            app: App::new(),
        })
    }

    pub fn draw(&mut self) -> io::Result<()> {
        self.terminal.draw(|f| render(f, &self.app))?;
        Ok(())
    }

    pub fn handle_input(&mut self, event: Event) {
        self.app.handle_input(event);
    }
    pub fn handle_agent_event(&mut self, event: AgentEvent) {
        self.app.handle_agent_event(event);
    }
    pub fn take_input(&mut self) -> Option<String> {
        self.app.take_input()
    }
    pub fn take_command(&mut self) -> Option<Command> {
        self.app.take_command()
    }
    pub fn menu_active(&self) -> bool {
        self.app.menu_active()
    }

    pub fn set_model_info(&mut self, info: &str, api_url: &str) {
        self.app.model_info = info.to_string();
        self.app.current_api_url = api_url.to_string();
    }

    pub fn set_planning(&mut self, planning: bool) {
        self.app.is_planning = planning;
    }
    pub fn is_planning(&self) -> bool {
        self.app.is_planning
    }

    pub fn toggle_theme(&mut self) -> Theme {
        let new = if self.app.theme == Theme::Dark {
            Theme::Light
        } else {
            Theme::Dark
        };
        self.app.theme = new;
        self.app.colors = ThemeColors::new(new);
        save_theme(new);
        new
    }

    pub fn push_system_message(&mut self, msg: String) {
        self.app.messages.push(ChatLine::System(msg));
    }
    pub fn push_user_message(&mut self, msg: String) {
        self.app.messages.push(ChatLine::User(msg));
    }
    pub fn push_assistant_message(&mut self, msg: String) {
        self.app.messages.push(ChatLine::Assistant(msg));
    }
    pub fn push_tool_start(&mut self, name: String) {
        self.app.messages.push(ChatLine::ToolStart { name });
    }
    pub fn push_tool_result(&mut self, content: String, is_error: bool) {
        self.app
            .messages
            .push(ChatLine::ToolResult { content, is_error });
    }

    pub fn clear_chat(&mut self) {
        self.app.messages.clear();
        self.app
            .messages
            .push(ChatLine::System("Chat cleared — fresh session".into()));
        self.app.messages.push(ChatLine::Separator);
        self.app.scroll_offset = 0;
    }

    pub fn show_confirm(&mut self, path: &str) {
        self.app.confirm_prompt = Some(path.to_string());
        self.app.scroll_offset = 0;
    }
    pub fn clear_confirm(&mut self) {
        self.app.confirm_prompt = None;
    }
    pub fn quit(&self) -> bool {
        self.app.quit
    }
    pub fn set_quit(&mut self) {
        self.app.quit = true;
    }

    pub fn shutdown(mut self) -> io::Result<()> {
        disable_raw_mode()?;
        execute!(self.terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
        self.terminal.show_cursor()?;
        Ok(())
    }
}

fn render(f: &mut Frame, app: &App) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(3),
        ])
        .split(f.area());

    render_status(f, outer[0], app);
    render_chat(f, outer[1], app);
    render_input(f, outer[2], app);
    if app.menu_active() {
        render_menu(f, f.area(), app);
    }
}

fn render_status(f: &mut Frame, area: Rect, app: &App) {
    let (text, color) = if let Some(ref err) = app.error {
        (format!("  {}", err), app.colors.error)
    } else if let Some(ref _path) = app.confirm_prompt {
        ("  Allow write? (y/n)".to_string(), Color::Rgb(224, 175, 104))
    } else if app.menu_active() {
        ("  Ctrl+X Menu  ·  Esc: Close".into(), Color::Rgb(224, 175, 104))
    } else if app.is_thinking {
        (format!("  ⟳ {}", app.status), Color::Rgb(224, 175, 104))
    } else if !app.status.is_empty() {
        (format!("  {}", app.status), app.colors.user)
    } else {
        let mode = if app.is_planning { "plan" } else { "ready" };
        let mode_color = if app.is_planning {
            Color::Rgb(224, 175, 104)
        } else {
            Color::Rgb(158, 206, 106)
        };
        let indicator = match app.theme {
            Theme::Dark => "☾",
            Theme::Light => "☀",
        };
        let info = if app.model_info.is_empty() {
            format!(" {}  ·  {}  ·  Ctrl+X", mode, indicator)
        } else {
            format!(" {}  ·  {}  ·  {}  ·  Ctrl+X", mode, app.model_info, indicator)
        };
        (info, mode_color)
    };

    f.render_widget(
        Paragraph::new(Line::from(Span::styled(text, Style::default().fg(color)))),
        area,
    );
}

fn render_menu(f: &mut Frame, screen: Rect, app: &App) {
    let (title, lines) = match &app.sub_menu {
        SubMenu::None => return,
        SubMenu::Main { selected } => {
            let mut ml: Vec<Line> = vec![Line::from("")];
            for (i, item) in MENU_ITEMS.iter().enumerate() {
                let style = if i == *selected {
                    let fg = match app.theme {
                        Theme::Dark => Color::Rgb(192, 202, 245),
                        Theme::Light => Color::Rgb(20, 20, 20),
                    };
                    let bg = match app.theme {
                        Theme::Dark => Color::Rgb(65, 72, 104),
                        Theme::Light => Color::Rgb(220, 220, 220),
                    };
                    Style::default()
                        .fg(fg)
                        .bg(bg)
                        .add_modifier(Modifier::BOLD)
                } else {
                    let fg = match app.theme {
                        Theme::Dark => Color::Rgb(169, 177, 214),
                        Theme::Light => Color::Rgb(80, 80, 80),
                    };
                    Style::default().fg(fg)
                };
                ml.push(Line::from(Span::styled(format!("  {}  ", item), style)));
            }
            ml.push(Line::from(""));
            ml.push(Line::from(Span::styled(
                "  arrows  Enter  Esc  ",
                Style::default().fg(app.colors.tool_result),
            )));
            (" Menu ", ml)
        }
        SubMenu::SessionPicker { sessions, selected } => {
            let mut ml: Vec<Line> = vec![Line::from("")];
            for (i, name) in sessions.iter().enumerate() {
                let style = if i == *selected {
                    let fg = match app.theme {
                        Theme::Dark => Color::Rgb(192, 202, 245),
                        Theme::Light => Color::Rgb(20, 20, 20),
                    };
                    let bg = match app.theme {
                        Theme::Dark => Color::Rgb(65, 72, 104),
                        Theme::Light => Color::Rgb(220, 220, 220),
                    };
                    Style::default()
                        .fg(fg)
                        .bg(bg)
                        .add_modifier(Modifier::BOLD)
                } else {
                    let fg = match app.theme {
                        Theme::Dark => Color::Rgb(169, 177, 214),
                        Theme::Light => Color::Rgb(80, 80, 80),
                    };
                    Style::default().fg(fg)
                };
                ml.push(Line::from(Span::styled(format!("  {}  ", name), style)));
            }
            ml.push(Line::from(""));
            ml.push(Line::from(Span::styled(
                "  Enter=select  Esc=back  ",
                Style::default().fg(app.colors.tool_result),
            )));
            (" Sessions ", ml)
        }
        SubMenu::ModelPicker { models, selected } => {
            let mut ml: Vec<Line> = vec![Line::from("")];
            for (i, name) in models.iter().enumerate() {
                let style = if i == *selected {
                    let fg = match app.theme {
                        Theme::Dark => Color::Rgb(192, 202, 245),
                        Theme::Light => Color::Rgb(20, 20, 20),
                    };
                    let bg = match app.theme {
                        Theme::Dark => Color::Rgb(65, 72, 104),
                        Theme::Light => Color::Rgb(220, 220, 220),
                    };
                    Style::default()
                        .fg(fg)
                        .bg(bg)
                        .add_modifier(Modifier::BOLD)
                } else {
                    let fg = match app.theme {
                        Theme::Dark => Color::Rgb(169, 177, 214),
                        Theme::Light => Color::Rgb(80, 80, 80),
                    };
                    Style::default().fg(fg)
                };
                ml.push(Line::from(Span::styled(format!("  {}  ", name), style)));
            }
            ml.push(Line::from(""));
            ml.push(Line::from(Span::styled(
                "  Enter=select  Esc=back  ",
                Style::default().fg(app.colors.tool_result),
            )));
            (" Models ", ml)
        }
        SubMenu::ExpertPicker { experts, selected } => {
            let mut ml: Vec<Line> = vec![Line::from("")];
            for (i, (_, name)) in experts.iter().enumerate() {
                let style = if i == *selected {
                    let fg = match app.theme {
                        Theme::Dark => Color::Rgb(192, 202, 245),
                        Theme::Light => Color::Rgb(20, 20, 20),
                    };
                    let bg = match app.theme {
                        Theme::Dark => Color::Rgb(65, 72, 104),
                        Theme::Light => Color::Rgb(220, 220, 220),
                    };
                    Style::default()
                        .fg(fg)
                        .bg(bg)
                        .add_modifier(Modifier::BOLD)
                } else {
                    let fg = match app.theme {
                        Theme::Dark => Color::Rgb(169, 177, 214),
                        Theme::Light => Color::Rgb(80, 80, 80),
                    };
                    Style::default().fg(fg)
                };
                ml.push(Line::from(Span::styled(format!("  {}  ", name), style)));
            }
            ml.push(Line::from(""));
            ml.push(Line::from(Span::styled(
                "  Enter=select  Esc=back  ",
                Style::default().fg(app.colors.tool_result),
            )));
            (" Experts ", ml)
        }
        SubMenu::TeamPicker { teams, selected } => {
            let mut ml: Vec<Line> = vec![Line::from("")];
            for (i, (_, name)) in teams.iter().enumerate() {
                let style = if i == *selected {
                    let fg = match app.theme {
                        Theme::Dark => Color::Rgb(192, 202, 245),
                        Theme::Light => Color::Rgb(20, 20, 20),
                    };
                    let bg = match app.theme {
                        Theme::Dark => Color::Rgb(65, 72, 104),
                        Theme::Light => Color::Rgb(220, 220, 220),
                    };
                    Style::default()
                        .fg(fg)
                        .bg(bg)
                        .add_modifier(Modifier::BOLD)
                } else {
                    let fg = match app.theme {
                        Theme::Dark => Color::Rgb(169, 177, 214),
                        Theme::Light => Color::Rgb(80, 80, 80),
                    };
                    Style::default().fg(fg)
                };
                ml.push(Line::from(Span::styled(format!("  {}  ", name), style)));
            }
            ml.push(Line::from(""));
            ml.push(Line::from(Span::styled(
                "  Enter=select  Esc=back  ",
                Style::default().fg(app.colors.tool_result),
            )));
            (" Team ", ml)
        }
        SubMenu::ApiConfig { input } => {
            let display = if input.is_empty() {
                String::new()
            } else {
                "*".repeat(input.len())
            };
            let fg_label = match app.theme {
                Theme::Dark => Color::Rgb(192, 202, 245),
                Theme::Light => Color::Rgb(30, 30, 30),
            };
            (
                " Configure API ",
                vec![
                    Line::from(""),
                    Line::from(Span::styled(
                        "  Enter your API key:  ",
                        Style::default().fg(fg_label),
                    )),
                    Line::from(Span::styled(
                        format!("  > {}  ", display),
                        Style::default()
                            .fg(Color::Rgb(224, 175, 104))
                            .add_modifier(Modifier::BOLD),
                    )),
                    Line::from(""),
                    Line::from(Span::styled(
                        "  Enter=confirm  Esc=back  ",
                        Style::default().fg(app.colors.tool_result),
                    )),
                ],
            )
        }
        SubMenu::Help => {
            let accent = match app.theme {
                Theme::Dark => Color::Rgb(122, 162, 247),
                Theme::Light => Color::Rgb(0, 102, 204),
            };
            (
                " Help ",
                vec![
                    Line::from(""),
                    Line::from(Span::styled(
                        "  AI Code Agent  ",
                        Style::default()
                            .fg(accent)
                            .add_modifier(Modifier::BOLD),
                    )),
                    Line::from(""),
                    Line::from("  Enter      — send message"),
                    Line::from("  Ctrl+X     — open menu"),
                    Line::from("  Esc        — quit / close menu"),
                    Line::from("  PgUp/PgDn  — scroll"),
                    Line::from("  Mouse      — scroll"),
                    Line::from(""),
                    Line::from("  /help      — commands"),
                    Line::from("  /plan      — planning mode"),
                    Line::from("  /execute   — execution mode"),
                    Line::from("  /clear     — fresh chat"),
                    Line::from("  /quit      — exit"),
                    Line::from(""),
                    Line::from(Span::styled(
                        "  Esc/Enter=back  ",
                        Style::default().fg(app.colors.tool_result),
                    )),
                ],
            )
        }
    };

    let height = lines.len() as u16 + 2;
    let width = 46;
    let x = (screen.width.saturating_sub(width)) / 2;
    let y = (screen.height.saturating_sub(height)) / 2;
    let menu_area = Rect::new(x, y, width.min(screen.width), height.min(screen.height));
    f.render_widget(Clear, menu_area);

    let bg_color = match app.theme {
        Theme::Dark => Color::Rgb(36, 40, 59),
        Theme::Light => Color::Rgb(245, 245, 245),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.colors.separator))
        .title(title)
        .style(Style::default().bg(bg_color));
    let inner = block.inner(menu_area);
    f.render_widget(block, menu_area);
    f.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn render_chat(f: &mut Frame, area: Rect, app: &App) {
    let width = area.width.saturating_sub(4) as usize;
    let c = &app.colors;
    let mut lines: Vec<Line> = Vec::new();

    for msg in &app.messages {
        match msg {
            ChatLine::User(text) => {
                for w in textwrap::wrap(text, width) {
                    lines.push(Line::from(vec![
                        Span::styled(
                            " You ",
                            Style::default().fg(c.user).add_modifier(Modifier::BOLD),
                        ),
                        Span::raw(w.into_owned()),
                    ]));
                }
            }
            ChatLine::Assistant(text) => {
                for (i, w) in textwrap::wrap(text, width).into_iter().enumerate() {
                    let label = if i == 0 { " AI  " } else { "     " };
                    lines.push(Line::from(vec![
                        Span::styled(
                            label,
                            Style::default().fg(c.ai).add_modifier(Modifier::BOLD),
                        ),
                        Span::raw(w.into_owned()),
                    ]));
                }
            }
            ChatLine::ToolStart { name } => {
                lines.push(Line::from(vec![Span::styled(
                    format!("  · {} ", name),
                    Style::default().fg(c.tool).add_modifier(Modifier::ITALIC),
                )]));
            }
            ChatLine::ToolResult { content, is_error } => {
                let color = if *is_error { c.error } else { c.tool_result };
                let short = if content.len() > 300 {
                    format!("{}...", content[..300].replace('\n', " "))
                } else {
                    content.replace('\n', " ")
                };
                for w in textwrap::wrap(&short, width.saturating_sub(2)) {
                    lines.push(Line::from(vec![Span::styled(
                        format!("  {}", w),
                        Style::default().fg(color),
                    )]));
                }
            }
            ChatLine::System(text) => {
                for w in textwrap::wrap(text, width) {
                    lines.push(Line::from(vec![Span::styled(
                        format!("  {}", w),
                        Style::default().fg(c.system),
                    )]));
                }
            }
            ChatLine::Separator => {
                lines.push(Line::from(vec![Span::styled(
                    "─".repeat(width.min(60)),
                    Style::default().fg(c.separator),
                )]));
            }
        }
    }

    let total_lines = lines.len();
    let visible = area.height as usize;

    if total_lines > visible {
        let max_scroll = total_lines.saturating_sub(visible);
        let from_bottom = app.scroll_offset.min(max_scroll);
        let scroll = max_scroll - from_bottom;
        let para = Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: false })
            .scroll((scroll as u16, 0));
        f.render_widget(para, area);
        let mut sb = ScrollbarState::new(max_scroll).position(scroll);
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None),
            area,
            &mut sb,
        );
    } else {
        f.render_widget(
            Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false }),
            area,
        );
    }
}

fn render_input(f: &mut Frame, area: Rect, app: &App) {
    f.render_widget(&app.input, area);
}


