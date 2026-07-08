use std::{
    error::Error,
    fs,
    io,
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::Duration,
};

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame, Terminal,
};
use serde::{Deserialize, Serialize};

// UI active panels
#[derive(Clone, Copy, PartialEq, Eq)]
enum ActivePanel {
    Feeds,
    Articles,
    Reading,
    Adding,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct FeedSource {
    name: String,
    url: String,
}

#[derive(Clone, Debug)]
struct FeedItem {
    title: String,
    #[allow(dead_code)]
    description: String,
    content: String,
    link: String,
    published: String,
}

// Background thread communication
enum Msg {
    FeedLoaded {
        feed_idx: usize,
        items: Vec<FeedItem>,
    },
    FeedFetchFailed {
        feed_idx: usize,
        error: String,
    },
    FeedAdded {
        source: FeedSource,
        items: Vec<FeedItem>,
    },
    FeedAddFailed {
        error: String,
    },
}

const CONFIG_DIR: &str = "ai/storage";
const CONFIG_FILE: &str = "rss-tui-feeds.json";

fn get_config_path() -> Result<PathBuf, Box<dyn Error>> {
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    Ok(home.join(CONFIG_DIR).join(CONFIG_FILE))
}

fn get_default_feeds() -> Vec<FeedSource> {
    vec![
        FeedSource {
            name: "Muhammad's Blog (75-Day)".to_string(),
            url: "https://blog.usmanity.com/tag/75-day-challenge/rss".to_string(),
        },
        FeedSource {
            name: "The Go Programming Language Blog".to_string(),
            url: "https://go.dev/blog/feed.atom".to_string(),
        },
        FeedSource {
            name: "Hacker News".to_string(),
            url: "https://news.ycombinator.com/rss".to_string(),
        },
    ]
}

fn load_feeds() -> Result<Vec<FeedSource>, Box<dyn Error>> {
    let path = get_config_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    if !path.exists() {
        let defaults = get_default_feeds();
        save_feeds(&defaults)?;
        return Ok(defaults);
    }

    let data = fs::read_to_string(path)?;
    let feeds: Vec<FeedSource> = serde_json::from_str(&data)?;
    Ok(feeds)
}

fn save_feeds(feeds: &[FeedSource]) -> Result<(), Box<dyn Error>> {
    let path = get_config_path()?;
    let data = serde_json::to_string_pretty(feeds)?;
    fs::write(path, data)?;
    Ok(())
}

fn fetch_feed(url: &str) -> Result<(String, Vec<FeedItem>), Box<dyn Error>> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;

    let response = client.get(url).send()?;
    let bytes = response.bytes()?;

    let feed = feed_rs::parser::parse(&bytes[..])?;
    let title = feed
        .title
        .map(|t| t.content)
        .unwrap_or_else(|| "New Feed".to_string());

    let mut items = Vec::new();
    for entry in feed.entries {
        let title = entry.title.map(|t| t.content).unwrap_or_default();
        let link = entry
            .links
            .first()
            .map(|l| l.href.clone())
            .unwrap_or_default();

        let published = entry
            .published
            .or(entry.updated)
            .map(|dt| dt.format("%b %d, %Y • %H:%M").to_string())
            .unwrap_or_default();

        let description = entry.summary.map(|s| s.content).unwrap_or_default();

        let content = entry
            .content
            .and_then(|c| c.body)
            .unwrap_or_else(|| description.clone());

        items.push(FeedItem {
            title,
            description,
            content,
            link,
            published,
        });
    }

    Ok((title, items))
}

fn clean_html(html: &str) -> String {
    let mut inside = false;
    let mut result = String::new();

    for c in html.chars() {
        if c == '<' {
            inside = true;
            continue;
        }
        if c == '>' {
            inside = false;
            continue;
        }
        if !inside {
            result.push(c);
        }
    }

    let res = result
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");

    let lines: Vec<&str> = res.lines().map(|l| l.trim()).collect();
    let mut clean_lines = Vec::new();
    for line in lines {
        if !line.is_empty() {
            clean_lines.push(line);
        } else if !clean_lines.is_empty() && *clean_lines.last().unwrap() != "" {
            clean_lines.push("");
        }
    }
    clean_lines.join("\n")
}

struct App {
    feeds: Vec<FeedSource>,
    articles: Vec<FeedItem>,
    active_feed_idx: usize,
    active_article_idx: usize,
    active_panel: ActivePanel,

    feed_list_state: ListState,
    article_list_state: ListState,
    reading_scroll_y: u16,

    input_url: String,
    is_loading: bool,
    error_msg: Option<String>,

    msg_rx: Receiver<Msg>,
    msg_tx: Sender<Msg>,
}

impl App {
    fn new(msg_tx: Sender<Msg>, msg_rx: Receiver<Msg>) -> Self {
        let feeds = load_feeds().unwrap_or_else(|_| get_default_feeds());
        let mut feed_list_state = ListState::default();
        if !feeds.is_empty() {
            feed_list_state.select(Some(0));
        }

        Self {
            feeds,
            articles: Vec::new(),
            active_feed_idx: 0,
            active_article_idx: 0,
            active_panel: ActivePanel::Feeds,
            feed_list_state,
            article_list_state: ListState::default(),
            reading_scroll_y: 0,
            input_url: String::new(),
            is_loading: false,
            error_msg: None,
            msg_tx,
            msg_rx,
        }
    }

    fn trigger_fetch(&mut self, idx: usize) {
        if self.feeds.is_empty() {
            return;
        }
        self.is_loading = true;
        self.articles.clear();
        self.article_list_state.select(None);
        self.error_msg = None;

        let tx = self.msg_tx.clone();
        let url = self.feeds[idx].url.clone();
        thread::spawn(move || {
            match fetch_feed(&url) {
                Ok((_, items)) => {
                    let _ = tx.send(Msg::FeedLoaded { feed_idx: idx, items });
                }
                Err(e) => {
                    let _ = tx.send(Msg::FeedFetchFailed {
                        feed_idx: idx,
                        error: e.to_string(),
                    });
                }
            }
        });
    }

    fn trigger_add_feed(&mut self, url: String) {
        self.is_loading = true;
        self.error_msg = None;

        let tx = self.msg_tx.clone();
        thread::spawn(move || {
            match fetch_feed(&url) {
                Ok((title, items)) => {
                    let source = FeedSource { name: title, url };
                    let _ = tx.send(Msg::FeedAdded { source, items });
                }
                Err(e) => {
                    let _ = tx.send(Msg::FeedAddFailed {
                        error: e.to_string(),
                    });
                }
            }
        });
    }

    fn update_from_channel(&mut self) {
        while let Ok(msg) = self.msg_rx.try_recv() {
            match msg {
                Msg::FeedLoaded { feed_idx, items } => {
                    if self.active_feed_idx == feed_idx {
                        self.articles = items;
                        self.is_loading = false;
                        self.error_msg = None;
                        if !self.articles.is_empty() {
                            self.article_list_state.select(Some(0));
                            self.active_article_idx = 0;
                        }
                    }
                }
                Msg::FeedFetchFailed { feed_idx, error } => {
                    if self.active_feed_idx == feed_idx {
                        self.is_loading = false;
                        self.error_msg = Some(error);
                    }
                }
                Msg::FeedAdded { source, items } => {
                    self.is_loading = false;
                    self.error_msg = None;
                    self.feeds.push(source);
                    let _ = save_feeds(&self.feeds);

                    let new_idx = self.feeds.len() - 1;
                    self.active_feed_idx = new_idx;
                    self.feed_list_state.select(Some(new_idx));

                    self.articles = items;
                    if !self.articles.is_empty() {
                        self.article_list_state.select(Some(0));
                        self.active_article_idx = 0;
                    }
                    self.active_panel = ActivePanel::Articles;
                }
                Msg::FeedAddFailed { error } => {
                    self.is_loading = false;
                    self.error_msg = Some(error);
                    self.active_panel = ActivePanel::Adding;
                }
            }
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Channels for async work
    let (tx, rx) = mpsc::channel();

    // Create app state
    let mut app = App::new(tx, rx);

    // Initial fetch
    if !app.feeds.is_empty() {
        app.trigger_fetch(0);
    }

    let res = run_app(&mut terminal, &mut app);

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        println!("Error: {err:?}");
    }

    Ok(())
}

fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> io::Result<()> {
    loop {
        // Poll for background worker updates
        app.update_from_channel();

        // Render Frame
        terminal.draw(|f| ui(f, app))?;

        // Poll for user inputs (non-blocking with 20ms timeout)
        if event::poll(Duration::from_millis(20))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Release {
                    continue;
                }

                // Global Exit
                if key.code == KeyCode::Char('q')
                    || (key.code == KeyCode::Char('c')
                        && key.modifiers.contains(event::KeyModifiers::CONTROL))
                {
                    return Ok(());
                }

                match app.active_panel {
                    ActivePanel::Feeds => match key.code {
                        KeyCode::Tab => {
                            if !app.articles.is_empty() {
                                app.active_panel = ActivePanel::Articles;
                            }
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            if !app.feeds.is_empty() {
                                let i = match app.feed_list_state.selected() {
                                    Some(i) => {
                                        if i >= app.feeds.len() - 1 {
                                            0
                                        } else {
                                            i + 1
                                        }
                                    }
                                    None => 0,
                                };
                                app.active_feed_idx = i;
                                app.feed_list_state.select(Some(i));
                                app.trigger_fetch(i);
                            }
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            if !app.feeds.is_empty() {
                                let i = match app.feed_list_state.selected() {
                                    Some(i) => {
                                        if i == 0 {
                                            app.feeds.len() - 1
                                        } else {
                                            i - 1
                                        }
                                    }
                                    None => 0,
                                };
                                app.active_feed_idx = i;
                                app.feed_list_state.select(Some(i));
                                app.trigger_fetch(i);
                            }
                        }
                        KeyCode::Char('a') => {
                            app.active_panel = ActivePanel::Adding;
                            app.input_url.clear();
                            app.error_msg = None;
                        }
                        KeyCode::Char('d') => {
                            if !app.feeds.is_empty() {
                                app.feeds.remove(app.active_feed_idx);
                                let _ = save_feeds(&app.feeds);
                                if app.feeds.is_empty() {
                                    app.feed_list_state.select(None);
                                    app.articles.clear();
                                    app.article_list_state.select(None);
                                } else {
                                    if app.active_feed_idx >= app.feeds.len() {
                                        app.active_feed_idx = app.feeds.len() - 1;
                                    }
                                    let new_idx = app.active_feed_idx;
                                    app.feed_list_state.select(Some(new_idx));
                                    app.trigger_fetch(new_idx);
                                }
                            }
                        }
                        KeyCode::Char('r') => {
                            app.trigger_fetch(app.active_feed_idx);
                        }
                        _ => {}
                    },

                    ActivePanel::Articles => match key.code {
                        KeyCode::Tab | KeyCode::Esc | KeyCode::Backspace => {
                            app.active_panel = ActivePanel::Feeds;
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            if !app.articles.is_empty() {
                                let i = match app.article_list_state.selected() {
                                    Some(i) => {
                                        if i >= app.articles.len() - 1 {
                                            0
                                        } else {
                                            i + 1
                                        }
                                    }
                                    None => 0,
                                };
                                app.active_article_idx = i;
                                app.article_list_state.select(Some(i));
                            }
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            if !app.articles.is_empty() {
                                let i = match app.article_list_state.selected() {
                                    Some(i) => {
                                        if i == 0 {
                                            app.articles.len() - 1
                                        } else {
                                            i - 1
                                        }
                                    }
                                    None => 0,
                                };
                                app.active_article_idx = i;
                                app.article_list_state.select(Some(i));
                            }
                        }
                        KeyCode::Enter => {
                            if !app.articles.is_empty() {
                                app.active_panel = ActivePanel::Reading;
                                app.reading_scroll_y = 0;
                            }
                        }
                        KeyCode::Char('o') => {
                            if !app.articles.is_empty() {
                                let link = app.articles[app.active_article_idx].link.clone();
                                if !link.is_empty() {
                                    let _ = open::that(link);
                                }
                            }
                        }
                        KeyCode::Char('r') => {
                            app.trigger_fetch(app.active_feed_idx);
                        }
                        _ => {}
                    },

                    ActivePanel::Reading => match key.code {
                        KeyCode::Esc | KeyCode::Backspace => {
                            app.active_panel = ActivePanel::Articles;
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            app.reading_scroll_y = app.reading_scroll_y.saturating_add(1);
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            app.reading_scroll_y = app.reading_scroll_y.saturating_sub(1);
                        }
                        KeyCode::Char('o') => {
                            let link = app.articles[app.active_article_idx].link.clone();
                            if !link.is_empty() {
                                let _ = open::that(link);
                            }
                        }
                        _ => {}
                    },

                    ActivePanel::Adding => match key.code {
                        KeyCode::Esc => {
                            app.active_panel = ActivePanel::Feeds;
                            app.error_msg = None;
                        }
                        KeyCode::Char(c) => {
                            app.input_url.push(c);
                        }
                        KeyCode::Backspace => {
                            app.input_url.pop();
                        }
                        KeyCode::Enter => {
                            let url = app.input_url.trim().to_string();
                            if !url.is_empty() {
                                app.active_panel = ActivePanel::Feeds;
                                app.trigger_add_feed(url);
                            }
                        }
                        _ => {}
                    },
                }
            }
        }
    }
}

fn ui(f: &mut Frame, app: &mut App) {
    let active_color = Color::Rgb(255, 105, 180); // Neon Pink
    let secondary_color = Color::Rgb(0, 255, 200); // Neon Mint/Teal
    let muted_color = Color::DarkGray;

    // Full screen constraints
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(5),    // Body Content
            Constraint::Length(2), // Status Bar & Help menu
        ])
        .split(f.area());

    // 2. Body View (varies by panel)
    if app.active_panel == ActivePanel::Reading {
        // Detailed reader mode
        let article = &app.articles[app.active_article_idx];
        let cleaned_body = clean_html(&article.content);

        let content_text = format!(
            "Title: {}\nPublished: {}\nLink: {}\n\n{}",
            article.title, article.published, article.link, cleaned_body
        );

        let border_color = active_color;
        let reader_block = Block::default()
            .title(format!(" Reading: {} ", article.title))
            .title_style(Style::default().fg(secondary_color).bold())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color))
            .border_type(BorderType::Rounded);

        let reader = Paragraph::new(content_text)
            .block(reader_block)
            .wrap(Wrap { trim: true })
            .scroll((app.reading_scroll_y, 0));

        f.render_widget(reader, chunks[0]);
    } else {
        // Split Column layout
        let split_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(25), Constraint::Percentage(75)])
            .split(chunks[0]);

        // Sidebar Feeds list
        let feed_border = if app.active_panel == ActivePanel::Feeds {
            active_color
        } else {
            muted_color
        };
        let feed_block = Block::default()
            .title(" Feeds ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(feed_border))
            .border_type(BorderType::Rounded);

        let feed_items: Vec<ListItem> = app
            .feeds
            .iter()
            .map(|f| ListItem::new(format!("• {}", f.name)))
            .collect();

        let feed_list = List::new(feed_items)
            .block(feed_block)
            .highlight_style(
                Style::default()
                    .fg(secondary_color)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▶ ");
        f.render_stateful_widget(feed_list, split_chunks[0], &mut app.feed_list_state);

        // Main Articles list
        let articles_border = if app.active_panel == ActivePanel::Articles {
            active_color
        } else {
            muted_color
        };
        let active_feed_name = if !app.feeds.is_empty() {
            &app.feeds[app.active_feed_idx].name
        } else {
            "None"
        };
        let articles_block = Block::default()
            .title(format!(" Articles in {} ", active_feed_name))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(articles_border))
            .border_type(BorderType::Rounded);

        let article_items: Vec<ListItem> = app
            .articles
            .iter()
            .map(|item| {
                ListItem::new(vec![
                    Line::from(Span::styled(&item.title, Style::default().bold())),
                    Line::from(Span::styled(
                        format!("  Published: {}  ", item.published),
                        Style::default().fg(Color::DarkGray).italic(),
                    )),
                ])
            })
            .collect();

        let articles_list = List::new(article_items)
            .block(articles_block)
            .highlight_style(
                Style::default()
                    .fg(secondary_color)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("⚡ ");
        f.render_stateful_widget(articles_list, split_chunks[1], &mut app.article_list_state);
    }

    // 3. Status Bar & Help menu
    let status_text = if app.is_loading {
        "⠋ Loading feed data...".to_string()
    } else if let Some(ref err) = app.error_msg {
        format!("❌ Error: {err}")
    } else {
        "✓ Ready".to_string()
    };

    let help_text = match app.active_panel {
        ActivePanel::Feeds => "Tab: Switch focus • a: Add feed • d: Delete feed • r: Refresh • q: Quit",
        ActivePanel::Articles => "Tab: Feeds • Enter: Read article • o: Open link • r: Refresh • q: Quit",
        ActivePanel::Reading => "Esc: Back • j/k: Scroll • o: Open link • q: Quit",
        ActivePanel::Adding => "Esc: Cancel • Enter: Save feed • q: Quit",
    };

    let status_line = Line::from(vec![
        Span::styled(format!(" {status_text}  "), Style::default().fg(active_color).bold()),
        Span::styled(help_text, Style::default().fg(Color::Gray)),
    ]);
    let footer_para = Paragraph::new(status_line);
    f.render_widget(footer_para, chunks[1]);

    // 4. Render modal dialog if in Adding state
    if app.active_panel == ActivePanel::Adding {
        let area = centered_rect(60, 20, f.area());
        f.render_widget(Clear, area); // clear background

        let modal_block = Block::default()
            .title(" Add RSS Feed ")
            .title_style(Style::default().fg(secondary_color).bold())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(active_color))
            .border_type(BorderType::Rounded);

        let modal_text = format!(
            "\n Input Feed URL:\n {}\n\n Press Enter to add, Esc to cancel",
            app.input_url
        );
        let modal_para = Paragraph::new(modal_text).block(modal_block);
        f.render_widget(modal_para, area);
    }
}

// Helper to center a rectangular block in viewport
fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
