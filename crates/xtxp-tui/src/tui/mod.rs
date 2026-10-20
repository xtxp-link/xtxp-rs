//! The TUI: ratatui on crossterm, for editing y-sweet documents interactively.

pub mod keys;
pub mod model;
pub mod render;

use xtxp_muppet::adapters::ws;
use xtxp_muppet::core::Mode;
use xtxp_muppet::ports::{Outcome, Screen, View};
use model::Model;
use ratatui::crossterm::event::{self, Event};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct Ui {
    model: Model,
    screen: Screen,
    terminal: ratatui::DefaultTerminal,
    start: Instant,
}

impl Ui {
    fn now(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    fn redraw(&mut self) {
        let now = self.now();
        let Ui {
            model,
            screen,
            terminal,
            ..
        } = self;
        let _ = terminal.draw(|f| render::draw(f, model, screen, now));
    }
}

/// Updates the shared UI from the muppet core and redraws.
struct TuiView(Arc<Mutex<Ui>>);

impl View for TuiView {
    fn show(&mut self, screen: &Screen) {
        let mut ui = self.0.lock().expect("ui lock");
        let now = ui.now();
        let old = std::mem::replace(&mut ui.screen, screen.clone());
        keys::after_show(&mut ui.model, &old, screen, now);
        ui.redraw();
    }
    fn status(&mut self, line: &str) {
        let mut ui = self.0.lock().expect("ui lock");
        ui.model.status = line.to_string();
        ui.redraw();
    }
    fn finished(&mut self, _: &Outcome) {}
}

/// Runs the TUI until the person quits.
pub fn run(server: &str, doc: &str, read_only: bool) -> Result<(), String> {
    let terminal = ratatui::init();
    let ui = Arc::new(Mutex::new(Ui {
        model: Model::new(doc, read_only),
        screen: Screen::default(),
        terminal,
        start: Instant::now(),
    }));
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let keys_ui = ui.clone();
    let keys = std::thread::spawn(move || loop {
        let ready = event::poll(Duration::from_millis(200)).unwrap_or(false);
        let mut ui = keys_ui.lock().expect("ui lock");
        if ready {
            if let Ok(Event::Key(key)) = event::read() {
                let Ui { model, screen, .. } = &mut *ui;
                for command in keys::handle(model, screen, key) {
                    let _ = tx.send(command);
                }
                if ui.model.quit {
                    return;
                }
            }
        }
        ui.redraw();
    });
    ws::run(Mode::Interactive, server, doc, TuiView(ui), Some(rx));
    let _ = keys.join();
    ratatui::restore();
    Ok(())
}
