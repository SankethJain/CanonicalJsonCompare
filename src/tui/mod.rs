//! The terminal user interface.

mod app;
mod browser;
mod draw;
mod input;
mod results;

use std::io;
use std::time::Duration;

use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
};
use ratatui::crossterm::execute;

pub use app::App;

/// Runs the interface until the user quits.
pub fn run(mut app: App, start_now: bool) -> io::Result<()> {
    let mut terminal = ratatui::init();
    // Mouse and bracketed paste (drag & drop of files) are best effort: some
    // terminals do not support them and that is fine.
    let _ = execute!(io::stdout(), EnableBracketedPaste, EnableMouseCapture);
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(io::stdout(), DisableMouseCapture, DisableBracketedPaste);
        previous_hook(info);
    }));

    if start_now {
        app.start();
    }
    let result = main_loop(&mut terminal, &mut app);

    let _ = execute!(io::stdout(), DisableMouseCapture, DisableBracketedPaste);
    ratatui::restore();
    result
}

fn main_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> io::Result<()> {
    while !app.should_quit {
        terminal.draw(|f| draw::draw(f, app))?;
        let timeout = if app.is_running() || app.toast.is_some() {
            Duration::from_millis(100)
        } else {
            Duration::from_millis(500)
        };
        if event::poll(timeout)? {
            // Handle everything that is queued before redrawing.
            loop {
                app.handle_event(event::read()?);
                if app.should_quit || !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }
        app.tick();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{Event, KeyCode, KeyEvent};

    use super::app::{App, Screen};
    use super::results::Results;
    use crate::config::Settings;

    fn press(app: &mut App, code: KeyCode) {
        app.handle_event(Event::Key(KeyEvent::from(code)));
    }

    fn render(app: &mut App, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| super::draw::draw(f, app)).unwrap();
        let buf = term.backend().buffer().clone();
        buf.content.iter().map(|c| c.symbol()).collect()
    }

    /// Draws every screen at several sizes, including tiny ones, and walks
    /// through the main navigation paths. Catches layout panics.
    #[test]
    fn every_screen_renders_at_any_size() {
        let dir = std::env::temp_dir().join(format!("mc-ui-{}", std::process::id()));
        let (src, dst) = crate::demo::generate(&dir, 300).unwrap();
        let mut app = App::new(Settings::default());
        app.set_paths(Some(&src), Some(&dst));
        let sizes = [(120, 40), (80, 24), (40, 12), (12, 5), (1, 1)];
        for (w, h) in sizes {
            render(&mut app, w, h);
        }
        assert!(render(&mut app, 120, 40).contains("Start comparison"));

        // File picker.
        press(&mut app, KeyCode::BackTab);
        press(&mut app, KeyCode::Up);
        app.setup.focus = super::app::Field::Source;
        press(&mut app, KeyCode::Enter);
        assert!(app.browser.is_some());
        for (w, h) in sizes {
            render(&mut app, w, h);
        }
        press(&mut app, KeyCode::Esc);
        assert!(app.browser.is_none());

        // Results, computed synchronously.
        let req = crate::engine::CompareRequest {
            source: src,
            dest: dst,
            created_field: String::new(),
            updated_field: String::new(),
            options: Default::default(),
        };
        let report = crate::engine::run(&req, &crate::engine::Progress::default()).unwrap();
        app.screen = Screen::Results(Box::new(Results::new(report)));
        let walks: &[&[KeyCode]] = &[
            &[
                KeyCode::Char('1'),
                KeyCode::Enter,
                KeyCode::Enter,
                KeyCode::Char('a'),
                KeyCode::Enter,
            ],
            &[
                KeyCode::Esc,
                KeyCode::Char('2'),
                KeyCode::Char('f'),
                KeyCode::Char('/'),
                KeyCode::Char('6'),
            ],
            &[
                KeyCode::Enter,
                KeyCode::End,
                KeyCode::Enter,
                KeyCode::Right,
                KeyCode::Left,
            ],
            &[
                KeyCode::Char('3'),
                KeyCode::Enter,
                KeyCode::Enter,
                KeyCode::Enter,
                KeyCode::Char('d'),
            ],
            &[
                KeyCode::Char('m'),
                KeyCode::Enter,
                KeyCode::Char('4'),
                KeyCode::Enter,
                KeyCode::Enter,
            ],
            &[KeyCode::Char('5'), KeyCode::PageDown, KeyCode::Char('?')],
            &[KeyCode::Esc, KeyCode::Char('q')],
        ];
        for walk in walks {
            for &k in *walk {
                press(&mut app, k);
                for (w, h) in sizes {
                    render(&mut app, w, h);
                }
            }
        }
        assert!(app.popup.is_some(), "q asks for confirmation");
        assert!(render(&mut app, 120, 40).contains("Quit?"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
