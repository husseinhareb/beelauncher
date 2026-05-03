//! Core application state.  All mutation goes through this type.

use crate::{
    app_index::AppIndex,
    config::Config,
    exec,
    input::Action,
    search,
    ui::Renderer,
};

pub struct AppState {
    pub index:    AppIndex,
    pub renderer: Renderer,
    /// Current search query (typed by the user).
    pub query:    String,
    /// Filtered + scored results for the current query.
    pub results:  Vec<crate::search::Match<'static>>,
    /// Index into `results` of the highlighted row.
    pub selected: usize,
    /// First visible row (for scrolling).
    pub scroll:   usize,
    /// Set to true when the app should quit.
    pub should_exit: bool,
}

impl AppState {
    pub fn new(cfg: &Config) -> Self {
        let index = AppIndex::load();
        let renderer = Renderer::new(cfg);

        // SAFETY: We extend the lifetime of Match borrows to 'static here.
        // The `index` field owns the AppEntry data and lives as long as AppState.
        // This avoids a self-referential struct by holding an explicit Vec<Match<'static>>.
        let results = unsafe {
            let entries: &'static [crate::app_index::AppEntry] =
                std::mem::transmute(index.entries.as_slice());
            search::filter("", entries)
        };

        Self {
            index,
            renderer,
            query: String::with_capacity(64),
            results,
            selected: 0,
            scroll:   0,
            should_exit: false,
        }
    }

    /// Apply a keyboard action and return true if a redraw is needed.
    pub fn handle_action(&mut self, action: Action, cfg: &Config) -> bool {
        match action {
            Action::Char(ch) => {
                self.query.push(ch);
                self.refilter();
                true
            }
            Action::Backspace => {
                if self.query.pop().is_some() {
                    self.refilter();
                    true
                } else {
                    false
                }
            }
            Action::DeleteWord => {
                let trimmed = self.query.trim_end();
                let keep = trimmed.rfind(' ').map(|i| i + 1).unwrap_or(0);
                if keep < self.query.len() {
                    self.query.truncate(keep);
                    self.refilter();
                    true
                } else {
                    false
                }
            }
            Action::ClearLine => {
                if !self.query.is_empty() {
                    self.query.clear();
                    self.refilter();
                    true
                } else {
                    false
                }
            }
            Action::SelectNext => {
                if !self.results.is_empty() {
                    self.selected = (self.selected + 1).min(self.results.len() - 1);
                    self.adjust_scroll(cfg);
                }
                true
            }
            Action::SelectPrev => {
                self.selected = self.selected.saturating_sub(1);
                self.adjust_scroll(cfg);
                true
            }
            Action::Confirm => {
                if !self.results.is_empty() || (cfg.behavior.launch_on_empty && !self.query.is_empty()) {
                    self.launch(cfg);
                }
                false
            }
            Action::Cancel => {
                self.should_exit = true;
                false
            }
        }
    }

    pub fn render_frame(&mut self, pixels: &mut [u32]) {
        self.renderer.render(
            pixels,
            &self.query,
            &self.results,
            self.selected,
            self.scroll,
        );
    }

    fn refilter(&mut self) {
        // SAFETY: same transmute pattern as in new(); index lives for 'self.
        self.results = unsafe {
            let entries: &'static [crate::app_index::AppEntry] =
                std::mem::transmute(self.index.entries.as_slice());
            search::filter(&self.query, entries)
        };
        self.selected = 0;
        self.scroll = 0;
    }

    fn adjust_scroll(&mut self, cfg: &Config) {
        let visible = cfg.window.max_entries;
        if self.selected >= self.scroll + visible {
            self.scroll = self.selected + 1 - visible;
        } else if self.selected < self.scroll {
            self.scroll = self.selected;
        }
    }

    fn launch(&self, _cfg: &Config) {
        let exec = if self.results.is_empty() {
            self.query.as_str()
        } else {
            self.results[self.selected].entry.exec.as_str()
        };

        if let Err(e) = exec::launch(exec) {
            log::error!("launch failed: {e}");
        }
    }
}
