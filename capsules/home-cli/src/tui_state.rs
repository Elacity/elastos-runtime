impl Default for TuiState {
    fn default() -> Self {
        Self {
            tab: Tab::Home,
            home_index: 0,
            inbox_index: 0,
            people_index: 0,
            app_index: 0,
            system_index: 0,
            show_help: false,
            notice: None,
        }
    }
}

impl TuiState {
    fn next_tab(&mut self) {
        let current = DEFAULT_TABS
            .iter()
            .position(|tab| *tab == self.tab)
            .unwrap_or(0);
        self.tab = DEFAULT_TABS[(current + 1) % DEFAULT_TABS.len()];
    }

    fn prev_tab(&mut self) {
        let current = DEFAULT_TABS
            .iter()
            .position(|tab| *tab == self.tab)
            .unwrap_or(0);
        self.tab = DEFAULT_TABS[(current + DEFAULT_TABS.len() - 1) % DEFAULT_TABS.len()];
    }

    fn move_prev(&mut self, snapshot: &HomeSnapshot) {
        match self.tab {
            Tab::Home => {
                if !home_action_indices(snapshot).is_empty() {
                    self.home_index = self.home_index.saturating_sub(1);
                }
            }
            Tab::Inbox => {
                if !notification_indices(snapshot).is_empty() {
                    self.inbox_index = self.inbox_index.saturating_sub(1);
                }
            }
            Tab::People => {
                if !people_actions(snapshot).is_empty() {
                    self.people_index = self.people_index.saturating_sub(1);
                }
            }
            Tab::Apps => {
                if !app_entries(snapshot).is_empty() {
                    self.app_index = self.app_index.saturating_sub(1);
                }
            }
            Tab::System => {
                if !system_actions(snapshot).is_empty() {
                    self.system_index = self.system_index.saturating_sub(1);
                }
            }
        }
    }

    fn move_next(&mut self, snapshot: &HomeSnapshot) {
        match self.tab {
            Tab::Home => {
                let items = home_action_indices(snapshot);
                if !items.is_empty() {
                    self.home_index = (self.home_index + 1).min(items.len() - 1);
                }
            }
            Tab::Inbox => {
                let items = notification_indices(snapshot);
                if !items.is_empty() {
                    self.inbox_index = (self.inbox_index + 1).min(items.len() - 1);
                }
            }
            Tab::People => {
                let items = people_actions(snapshot);
                if !items.is_empty() {
                    self.people_index = (self.people_index + 1).min(items.len() - 1);
                }
            }
            Tab::Apps => {
                let items = app_entries(snapshot);
                if !items.is_empty() {
                    self.app_index = (self.app_index + 1).min(items.len() - 1);
                }
            }
            Tab::System => {
                let items = system_actions(snapshot);
                if !items.is_empty() {
                    self.system_index = (self.system_index + 1).min(items.len() - 1);
                }
            }
        }
    }

    fn activate(&self, snapshot: &HomeSnapshot) -> Option<String> {
        match self.tab {
            Tab::Home => selected_action(snapshot, &home_action_indices(snapshot), self.home_index)
                .filter(|action| action.ready)
                .map(|action| action.id.clone()),
            Tab::Inbox => selected_notification_action(snapshot, self.inbox_index)
                .filter(|action| action.ready)
                .map(|action| action.id.clone()),
            Tab::People => selected_people_action(snapshot, self.people_index)
                .filter(|action| action.ready)
                .map(|action| action.id),
            Tab::Apps => selected_app_action(snapshot, self.app_index)
                .filter(|action| action.ready)
                .map(|action| action.id.clone()),
            Tab::System => selected_system_action(snapshot, self.system_index)
                .filter(|action| action.ready)
                .map(|action| action.id),
        }
    }

    fn handle_mouse(&mut self, event: MouseEvent, cols: usize, snapshot: &HomeSnapshot) -> bool {
        if event.released {
            return false;
        }

        match event.button {
            64 => {
                self.move_prev(snapshot);
                true
            }
            65 => {
                self.move_next(snapshot);
                true
            }
            0 if event.y == TUI_TAB_ROW => {
                let tab_count = DEFAULT_TABS.len() as u16;
                let cols = cols.max(tab_count as usize) as u16;
                let slot = event.x.saturating_sub(1).saturating_mul(tab_count) / cols;
                self.tab = DEFAULT_TABS[slot.min(tab_count - 1) as usize];
                true
            }
            _ => false,
        }
    }
}
