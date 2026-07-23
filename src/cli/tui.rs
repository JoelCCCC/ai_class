use std::io::{self, stdout};

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap},
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

pub struct Tui {
    terminal: Terminal<ratatui::backend::CrosstermBackend<io::Stdout>>,
    app: App,
}

struct App {
    messages: Vec<ChatLine>,
    input: TextArea<'static>,
    scroll: usize,
    is_thinking: bool,
    status: String,
    quit: bool,
    error: Option<String>,
    pending_input: Option<String>,
}

impl App {
    fn new() -> Self {
        let mut input = TextArea::default();
        input.set_block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Input (Enter to send, Esc to quit) "),
        );
        input.set_placeholder_text("Type your message...");
        input.set_cursor_line_style(Style::default());

        Self {
            messages: vec![
                ChatLine::System("AI Code Agent v0.1 — type /help for commands".into()),
                ChatLine::Separator,
            ],
            input,
            scroll: 0,
            is_thinking: false,
            status: String::new(),
            quit: false,
            error: None,
            pending_input: None,
        }
    }

    fn handle_input(&mut self, event: Event) {
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Enter => {
                    let text: String = self.input.lines().join("\n").trim().to_string();
                    if !text.is_empty() {
                        self.messages.push(ChatLine::User(text.clone()));
                        self.pending_input = Some(text);
                        self.input = TextArea::default();
                        self.input.set_block(
                            Block::default()
                                .borders(Borders::ALL)
                                .title(" Input (Enter to send, Esc to quit) "),
                        );
                        self.input.set_placeholder_text("Type your message...");
                        self.input.set_cursor_line_style(Style::default());
                    }
                }
                KeyCode::Esc => {
                    self.quit = true;
                }
                KeyCode::PageUp => {
                    self.scroll = self.scroll.saturating_add(10);
                }
                KeyCode::PageDown => {
                    self.scroll = self.scroll.saturating_sub(10);
                }
                _ => {
                    self.input.input(key);
                }
            },
            Event::Mouse(mouse) => match mouse.kind {
                event::MouseEventKind::ScrollUp => {
                    self.scroll = self.scroll.saturating_add(3);
                }
                event::MouseEventKind::ScrollDown => {
                    self.scroll = self.scroll.saturating_sub(3);
                }
                _ => {}
            },
            Event::Resize(_, _) => {}
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
                self.scroll = 0;
            }
            AgentEvent::ToolCallStart { name, .. } => {
                self.is_thinking = false;
                self.status = format!("Running: {}...", name);
                self.messages.push(ChatLine::ToolStart { name });
                self.scroll = 0;
            }
            AgentEvent::ToolCallEnd(result) => {
                self.status = String::new();
                self.messages.push(ChatLine::ToolResult {
                    content: result.content,
                    is_error: result.is_error,
                });
                self.scroll = 0;
            }
            AgentEvent::Done => {
                self.is_thinking = false;
                self.status = String::new();
                self.scroll = 0;
            }
            AgentEvent::Error(msg) => {
                self.is_thinking = false;
                self.status = String::new();
                self.error = Some(msg);
            }
        }
    }

    fn take_input(&mut self) -> Option<String> {
        self.pending_input.take()
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

    pub fn quit(&self) -> bool {
        self.app.quit
    }

    pub fn shutdown(mut self) -> io::Result<()> {
        disable_raw_mode()?;
        execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )?;
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
}

fn render_status(f: &mut Frame, area: Rect, app: &App) {
    let (text, color) = if let Some(ref err) = app.error {
        (format!(" Error: {}", err), Color::Red)
    } else if app.is_thinking {
        (
            format!(" {} {}", spinner_char(), app.status),
            Color::Yellow,
        )
    } else if !app.status.is_empty() {
        (format!(" {}", app.status), Color::Cyan)
    } else {
        (" Ready".into(), Color::Green)
    };

    let para = Paragraph::new(Line::from(Span::styled(text, Style::default().fg(color))))
        .block(Block::default().style(Style::default().bg(Color::DarkGray)));
    f.render_widget(para, area);
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
                let wrapped: Vec<String> =
                    textwrap::wrap(text, width).into_iter().map(|c| c.into_owned()).collect();
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
                let wrapped: Vec<String> =
                    textwrap::wrap(text, width).into_iter().map(|c| c.into_owned()).collect();
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
                let color = if *is_error { Color::Red } else { Color::DarkGray };
                let short = if content.len() > 300 {
                    format!("{}...", &content[..300].replace('\n', " "))
                } else {
                    content.replace('\n', " ")
                };
                let wrapped: Vec<String> =
                    textwrap::wrap(&short, width.saturating_sub(2)).into_iter().map(|c| c.into_owned()).collect();
                for w in &wrapped {
                    lines.push(Line::from(vec![Span::styled(
                        format!("  {}", w),
                        Style::default().fg(color),
                    )]));
                }
            }
            ChatLine::System(text) => {
                let wrapped: Vec<String> =
                    textwrap::wrap(text, width).into_iter().map(|c| c.into_owned()).collect();
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
    let visible = (inner.height as usize).saturating_sub(1);

    if total_lines > visible {
        let scroll_offset = if app.scroll == 0 {
            total_lines.saturating_sub(visible)
        } else {
            app.scroll.min(total_lines.saturating_sub(visible))
        };
        let text = Text::from(lines);
        let para = Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .scroll((scroll_offset as u16, 0));
        f.render_widget(para, inner);

        let mut scrollbar_state =
            ScrollbarState::new(total_lines.saturating_sub(visible)).position(scroll_offset);
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
    let frames = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    let idx = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as usize
        / 80
        % frames.len();
    frames[idx]
}
