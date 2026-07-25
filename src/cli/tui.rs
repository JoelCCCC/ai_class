use std::cell::Cell;
use std::io::{self, stdout};

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
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
    ConfigureApi { key: String },
    ClearChat,
    TogglePlanning,
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
    ApiConfig {
        input: String,
    },
    Help,
}

pub struct Tui {
    terminal: Terminal<ratatui::backend::CrosstermBackend<io::Stdout>>,
    app: App,
}

struct App {
    messages: Vec<ChatLine>,
    input: TextArea<'static>,
    scroll_offset: usize,
    last_total_lines: Cell<usize>,
    last_visible: Cell<usize>,
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
}

const MENU_ITEMS: &[&str] = &[
    "Switch Session",
    "Switch Model",
    "Configure API",
    "Toggle Planning Mode",
    "Clear Chat",
    "Help",
    "Quit",
];

impl App {
    fn new() -> Self {
        let mut input = TextArea::default();
        input.set_block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Input (Enter to send, Esc to quit, Ctrl+X for menu) "),
        );
        input.set_placeholder_text("Type your message...");
        input.set_cursor_line_style(Style::default());

        Self {
            messages: vec![
                ChatLine::System("AI Code Agent v0.1 — type /help for commands".into()),
                ChatLine::Separator,
            ],
            input,
            scroll_offset: 0,
            last_total_lines: Cell::new(usize::MAX / 2),
            last_visible: Cell::new(1),
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
                        self.input = TextArea::default();
                        self.input.set_block(
                            Block::default()
                                .borders(Borders::ALL)
                                .title(" Input (Enter to send, Esc to quit, Ctrl+X for menu) "),
                        );
                        self.input.set_placeholder_text("Type your message...");
                        self.input.set_cursor_line_style(Style::default());
                    }
                }
                KeyCode::Esc => {
                    self.quit = true;
                }
                KeyCode::PageUp => {
                    let max = self
                        .last_total_lines
                        .get()
                        .saturating_sub(self.last_visible.get())
                        .max(1);
                    self.scroll_offset = self.scroll_offset.saturating_add(5).min(max);
                }
                KeyCode::PageDown => {
                    self.scroll_offset = self.scroll_offset.saturating_sub(5);
                }
                _ => {
                    self.input.input(key);
                }
            },
            Event::Mouse(mouse) => match mouse.kind {
                event::MouseEventKind::ScrollUp => {
                    let max = self
                        .last_total_lines
                        .get()
                        .saturating_sub(self.last_visible.get())
                        .max(1);
                    self.scroll_offset = self.scroll_offset.saturating_add(3).min(max);
                }
                event::MouseEventKind::ScrollDown => {
                    if self.scroll_offset <= 3 {
                        self.scroll_offset = 0;
                    } else {
                        self.scroll_offset -= 3;
                    }
                }
                _ => {}
            },
            Event::Resize(_, _) => {}
            _ => {}
        }
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
                KeyCode::Up => {
                    *selected = selected.saturating_sub(1);
                }
                KeyCode::Down => {
                    *selected = (*selected + 1).min(MENU_ITEMS.len() - 1);
                }
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
                        self.sub_menu = SubMenu::ApiConfig {
                            input: String::new(),
                        };
                    }
                    3 => {
                        self.pending_command = Some(Command::TogglePlanning);
                        self.sub_menu = SubMenu::None;
                    }
                    4 => {
                        self.pending_command = Some(Command::ClearChat);
                        self.sub_menu = SubMenu::None;
                    }
                    5 => {
                        self.sub_menu = SubMenu::Help;
                    }
                    6 => {
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
                KeyCode::Up => {
                    *selected = selected.saturating_sub(1);
                }
                KeyCode::Down => {
                    *selected = (*selected + 1).min(sessions.len().saturating_sub(1));
                }
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
                KeyCode::Up => {
                    *selected = selected.saturating_sub(1);
                }
                KeyCode::Down => {
                    *selected = (*selected + 1).min(models.len().saturating_sub(1));
                }
                KeyCode::Enter => {
                    let name = models[*selected].clone();
                    self.pending_command = Some(Command::SwitchModel(name));
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
                    Some(ChatLine::Assistant(existing)) => {
                        existing.push_str(&delta);
                    }
                    _ => {
                        self.messages.push(ChatLine::Assistant(delta));
                    }
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
    let cwd_slug = cwd.to_string_lossy().replace('/', "__");
    let dir = dirs::home_dir()
        .unwrap_or_default()
        .join(".config")
        .join("ai-code")
        .join("sessions")
        .join(&cwd_slug);
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

    let cloud_models: &[&str] = if api_url.contains("openai.com") {
        &["gpt-4o", "gpt-4-turbo", "gpt-4o-mini"]
    } else if api_url.contains("anthropic.com") {
        &["claude-sonnet-4-20250514", "claude-haiku-3-5-20241022"]
    } else if api_url.contains("deepseek.com") {
        &["deepseek-v4-pro", "deepseek-v4-flash"]
    } else {
        &["gpt-4o", "deepseek-v4-pro"]
    };

    for m in cloud_models {
        all.push(m.to_string());
    }
    all
}

impl Tui {
    pub fn new() -> io::Result<Self> {
        enable_raw_mode()?;
        execute!(stdout(), EnterAlternateScreen)?;
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
        execute!(self.terminal.backend_mut(), LeaveAlternateScreen)?;
        self.terminal.show_cursor()?;
        Ok(())
    }
}

fn render(f: &mut Frame, app: &App) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(6)])
        .split(f.area());

    let chat_area = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(outer[0]);

    render_status(f, chat_area[0], app);
    render_chat(f, chat_area[1], app);
    render_input(f, outer[1], app);

    if app.menu_active() {
        render_menu(f, f.area(), app);
    }
}

fn render_status(f: &mut Frame, area: Rect, app: &App) {
    let (text, color) = if let Some(ref err) = app.error {
        (format!(" Error: {}", err), Color::Red)
    } else if let Some(ref path) = app.confirm_prompt {
        (format!(" Allow write to {}? (y/n) ", path), Color::Yellow)
    } else if app.menu_active() {
        (" Ctrl+X: Menu  |  Esc: Close menu".into(), Color::Yellow)
    } else if app.is_thinking {
        (format!(" {} {}", spinner_char(), app.status), Color::Yellow)
    } else if !app.status.is_empty() {
        (format!(" {}", app.status), Color::Cyan)
    } else if app.is_planning {
        let prefix = if app.model_info.is_empty() {
            " Planning (read-only)  |  Ctrl+X: Menu".into()
        } else {
            format!(" Planning  |  {}  |  Ctrl+X: Menu", app.model_info)
        };
        (prefix, Color::Blue)
    } else {
        let prefix = if app.model_info.is_empty() {
            " Ready  |  Ctrl+X: Menu".into()
        } else {
            format!(" Ready  |  {}  |  Ctrl+X: Menu", app.model_info)
        };
        (prefix, Color::Green)
    };

    let para = Paragraph::new(Line::from(Span::styled(text, Style::default().fg(color))))
        .block(Block::default().style(Style::default().bg(Color::DarkGray)));
    f.render_widget(para, area);
}

fn render_menu(f: &mut Frame, screen: Rect, app: &App) {
    let (title, lines) = match &app.sub_menu {
        SubMenu::None => return,
        SubMenu::Main { selected } => {
            let mut menu_lines: Vec<Line> = vec![Line::from("")];
            for (i, item) in MENU_ITEMS.iter().enumerate() {
                let style = if i == *selected {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::White)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Gray)
                };
                menu_lines.push(Line::from(Span::styled(format!("  {}  ", item), style)));
            }
            menu_lines.push(Line::from(""));
            menu_lines.push(Line::from(Span::styled(
                "  arrows=navigate  Enter=select  Esc=close  ",
                Style::default().fg(Color::DarkGray),
            )));
            (" Menu ", menu_lines)
        }
        SubMenu::SessionPicker { sessions, selected } => {
            let mut picker_lines: Vec<Line> = vec![Line::from("")];
            if sessions.is_empty() {
                picker_lines.push(Line::from(Span::styled(
                    "  No sessions found  ",
                    Style::default().fg(Color::DarkGray),
                )));
            } else {
                for (i, name) in sessions.iter().enumerate() {
                    let style = if i == *selected {
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::White)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::Gray)
                    };
                    picker_lines.push(Line::from(Span::styled(format!("  {}  ", name), style)));
                }
            }
            picker_lines.push(Line::from(""));
            picker_lines.push(Line::from(Span::styled(
                "  Enter=select  Esc=back  ",
                Style::default().fg(Color::DarkGray),
            )));
            (" Sessions ", picker_lines)
        }
        SubMenu::ModelPicker { models, selected } => {
            let mut picker_lines: Vec<Line> = vec![Line::from("")];
            if models.is_empty() {
                picker_lines.push(Line::from(Span::styled(
                    "  No models found  ",
                    Style::default().fg(Color::DarkGray),
                )));
            } else {
                for (i, name) in models.iter().enumerate() {
                    let style = if i == *selected {
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::White)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::Gray)
                    };
                    picker_lines.push(Line::from(Span::styled(format!("  {}  ", name), style)));
                }
            }
            picker_lines.push(Line::from(""));
            picker_lines.push(Line::from(Span::styled(
                "  Enter=select  Esc=back  ",
                Style::default().fg(Color::DarkGray),
            )));
            (" Models ", picker_lines)
        }
        SubMenu::ApiConfig { input } => {
            let display = if input.is_empty() {
                "".to_string()
            } else {
                "*".repeat(input.len())
            };
            (
                " Configure API ",
                vec![
                    Line::from(""),
                    Line::from(Span::styled(
                        "  Enter your API key:  ",
                        Style::default().fg(Color::White),
                    )),
                    Line::from(Span::styled(
                        format!("  > {}  ", display),
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    )),
                    Line::from(""),
                    Line::from(Span::styled(
                        "  Enter=confirm  Esc=back  ",
                        Style::default().fg(Color::DarkGray),
                    )),
                ],
            )
        }
        SubMenu::Help => (
            " Help ",
            vec![
                Line::from(""),
                Line::from(Span::styled(
                    "  AI Code Agent  ",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from("  Enter      — send message"),
                Line::from("  Ctrl+X     — open/close menu"),
                Line::from("  Esc        — quit / close menu"),
                Line::from("  PgUp/PgDn  — scroll chat"),
                Line::from("  Mouse      — scroll chat"),
                Line::from(""),
                Line::from("  /help      — show commands"),
                Line::from("  /clear     — clear session"),
                Line::from("  /quit      — exit"),
                Line::from(""),
                Line::from(Span::styled(
                    "  Esc/Enter=back  ",
                    Style::default().fg(Color::DarkGray),
                )),
            ],
        ),
    };

    let height = lines.len() as u16 + 2;
    let width = 42;
    let x = (screen.width.saturating_sub(width)) / 2;
    let y = (screen.height.saturating_sub(height)) / 2;
    let menu_area = Rect::new(x, y, width.min(screen.width), height.min(screen.height));

    f.render_widget(Clear, menu_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .style(Style::default().bg(Color::Rgb(30, 30, 40)));
    let inner = block.inner(menu_area);
    f.render_widget(block, menu_area);

    let text = Text::from(lines);
    f.render_widget(Paragraph::new(text), inner);
}

fn render_chat(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default().borders(Borders::ALL).title(" Chat ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let width = inner.width.saturating_sub(4) as usize;

    let mut lines: Vec<Line> = Vec::new();

    for msg in &app.messages {
        match msg {
            ChatLine::User(text) => {
                let wrapped: Vec<String> = textwrap::wrap(text, width)
                    .into_iter()
                    .map(|c| c.into_owned())
                    .collect();
                for w in &wrapped {
                    lines.push(Line::from(vec![
                        Span::styled(
                            " You ",
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::raw(w.clone()),
                    ]));
                }
            }
            ChatLine::Assistant(text) => {
                let wrapped: Vec<String> = textwrap::wrap(text, width)
                    .into_iter()
                    .map(|c| c.into_owned())
                    .collect();
                for (i, w) in wrapped.iter().enumerate() {
                    let label = if i == 0 { " AI  " } else { "     " };
                    lines.push(Line::from(vec![
                        Span::styled(
                            label,
                            Style::default()
                                .fg(Color::Green)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::raw(w.clone()),
                    ]));
                }
            }
            ChatLine::ToolStart { name } => {
                lines.push(Line::from(vec![Span::styled(
                    format!("  [tool: {}]", name),
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::ITALIC),
                )]));
            }
            ChatLine::ToolResult { content, is_error } => {
                let color = if *is_error {
                    Color::Red
                } else {
                    Color::DarkGray
                };
                let short = if content.len() > 300 {
                    format!("{}...", content[..300].replace('\n', " "))
                } else {
                    content.replace('\n', " ")
                };
                let wrapped: Vec<String> = textwrap::wrap(&short, width.saturating_sub(2))
                    .into_iter()
                    .map(|c| c.into_owned())
                    .collect();
                for w in &wrapped {
                    lines.push(Line::from(vec![Span::styled(
                        format!("  {}", w),
                        Style::default().fg(color),
                    )]));
                }
            }
            ChatLine::System(text) => {
                let wrapped: Vec<String> = textwrap::wrap(text, width)
                    .into_iter()
                    .map(|c| c.into_owned())
                    .collect();
                for w in &wrapped {
                    lines.push(Line::from(vec![Span::styled(
                        format!("  {}", w),
                        Style::default().fg(Color::DarkGray),
                    )]));
                }
            }
            ChatLine::Separator => {
                lines.push(Line::from(vec![Span::styled(
                    "─".repeat(width.min(60)),
                    Style::default().fg(Color::DarkGray),
                )]));
            }
        }
    }

    let total_lines = lines.len();
    let visible = inner.height as usize;

    app.last_total_lines.set(total_lines);
    app.last_visible.set(visible);

    if total_lines > visible {
        let scroll = if app.scroll_offset == 0 {
            total_lines.saturating_sub(visible)
        } else {
            app.scroll_offset.min(total_lines.saturating_sub(visible))
        };
        let text = Text::from(lines);
        let para = Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .scroll((scroll as u16, 0));
        f.render_widget(para, inner);

        let mut scrollbar_state =
            ScrollbarState::new(total_lines.saturating_sub(visible)).position(scroll);
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None);
        f.render_stateful_widget(scrollbar, inner, &mut scrollbar_state);
    } else {
        let text = Text::from(lines);
        let para = Paragraph::new(text).wrap(Wrap { trim: false });
        f.render_widget(para, inner);
    }
}

fn render_input(f: &mut Frame, area: Rect, app: &App) {
    f.render_widget(&app.input, area);
}

fn spinner_char() -> char {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static FRAME: AtomicUsize = AtomicUsize::new(0);
    let frames = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    let idx = FRAME.fetch_add(1, Ordering::Relaxed) % frames.len();
    frames[idx]
}
