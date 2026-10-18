//! The `[ui.sidebar.panel]` section: a command's colored output between spaces and agents.

use std::time::{Duration, Instant};

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

use super::state::{SidebarPanelPress, SidebarPanelState};
use super::tab_bar_status::{
    is_unicode_format_control, spawn_output_command, StatusCommandTask, StatusOutput,
};
use super::App;
use crate::config::PanelSidebarConfig;

const MAX_PANEL_OUTPUT_BYTES: usize = 64 * 1024;

/// Focused pane, body size, zoom and drag a run renders for.
type PanelInputs = (Option<String>, (u16, u16), u8, (i16, i16));

pub(super) struct SidebarPanelRuntime {
    command: String,
    interval: Duration,
    timeout: Duration,
    next_run_at: Instant,
    /// Inputs of the last run; a change reruns at once.
    last_inputs: Option<PanelInputs>,
    task: Option<StatusCommandTask>,
}

impl Drop for SidebarPanelRuntime {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.cancel();
        }
    }
}

impl App {
    pub(super) fn configure_sidebar_panel(&mut self, config: &PanelSidebarConfig) {
        self.sidebar_panel_generation = self.sidebar_panel_generation.wrapping_add(1);
        self.state.sidebar_panel.config = config.clone();
        self.state.sidebar_panel.lines.clear();
        self.state.sidebar_panel.zoom = config.zoom.min(config.zoom_levels.saturating_sub(1));
        self.state.sidebar_panel.drag = (0, 0);
        self.state.sidebar_panel.press = None;
        self.state.sidebar_panel.stale = false;
        let valid = self.state.sidebar_panel.enabled()
            && crate::platform::status_commands_supported()
            && (1..=crate::config::MAX_TAB_BAR_COMMAND_INTERVAL_SECONDS)
                .contains(&config.interval_seconds)
            && (1..=crate::config::MAX_TAB_BAR_COMMAND_TIMEOUT_SECONDS)
                .contains(&config.timeout_seconds);
        self.sidebar_panel = valid.then(|| SidebarPanelRuntime {
            command: config.command.clone(),
            interval: Duration::from_secs(config.interval_seconds),
            timeout: Duration::from_secs(config.timeout_seconds),
            next_run_at: Instant::now(),
            last_inputs: None,
            task: None,
        });
    }

    /// Starts the panel command when its interval elapsed or the focused pane, body size or
    /// view changed. A hidden panel (zero body size) does not run.
    pub(super) fn handle_sidebar_panel_tasks(&mut self, now: Instant) {
        let size = self.state.sidebar_panel.body_size;
        if self
            .sidebar_panel
            .as_ref()
            .is_none_or(|runtime| runtime.task.is_some())
            || size.0 == 0
            || size.1 == 0
        {
            return;
        }
        let (mut environment, cwd) = self.custom_command_env();
        let pane = environment
            .iter()
            .find(|(key, _)| key == "HERDR_ACTIVE_PANE_ID")
            .map(|(_, value)| value.clone());
        let generation = self.sidebar_panel_generation;
        let event_tx = self.event_tx.clone();
        let Some(runtime) = self.sidebar_panel.as_mut() else {
            return;
        };
        // Another focused pane starts from its own view.
        if runtime
            .last_inputs
            .as_ref()
            .is_some_and(|(last, ..)| *last != pane)
        {
            self.state.sidebar_panel.drag = (0, 0);
        }
        let panel = &self.state.sidebar_panel;
        let inputs = (pane, size, panel.zoom, panel.drag);
        if now < runtime.next_run_at && runtime.last_inputs.as_ref() == Some(&inputs) {
            return;
        }
        environment.extend(panel_view_env(panel));
        environment.push(("COLUMNS".into(), size.0.to_string()));
        environment.push(("LINES".into(), size.1.to_string()));
        runtime.last_inputs = Some(inputs);
        runtime.next_run_at = now.checked_add(runtime.interval).unwrap_or(now);
        runtime.task = Some(spawn_output_command(
            event_tx,
            runtime.command.clone(),
            runtime.timeout,
            environment,
            cwd,
            StatusOutput::Whole {
                limit: MAX_PANEL_OUTPUT_BYTES,
            },
            move |result| crate::events::AppEvent::SidebarPanelCommandFinished {
                generation,
                result,
            },
        ));
    }

    /// None while a run is in flight or the panel is hidden, which skips runs; once a render
    /// shows it again, the overdue deadline wakes the loop to run it.
    pub(super) fn next_sidebar_panel_deadline(&self) -> Option<Instant> {
        let size = self.state.sidebar_panel.body_size;
        if size.0 == 0 || size.1 == 0 {
            return None;
        }
        self.sidebar_panel
            .as_ref()
            .filter(|runtime| runtime.task.is_none())
            .map(|runtime| runtime.next_run_at)
    }

    /// Stores a finished run's output, or marks the shown one stale when the run failed;
    /// returns whether the panel changed.
    pub(super) fn handle_sidebar_panel_command_finished(
        &mut self,
        generation: u64,
        result: Result<Vec<u8>, String>,
    ) -> bool {
        if generation != self.sidebar_panel_generation {
            return false;
        }
        let Some(runtime) = self.sidebar_panel.as_mut() else {
            return false;
        };
        runtime.task = None;
        let panel = &mut self.state.sidebar_panel;
        let was_stale = std::mem::replace(&mut panel.stale, result.is_err());
        match result {
            Ok(output) => {
                let lines = parse_sgr_lines(&output, usize::from(panel.config.rows));
                let changed = lines != panel.lines || was_stale;
                panel.lines = lines;
                changed
            }
            Err(error) => {
                tracing::warn!(command = %runtime.command, error, "sidebar panel command failed");
                !was_stale
            }
        }
    }

    /// Handles a mouse event on the panel: a left press that moves drags the body, one
    /// released in place clicks, and the wheel zooms. Returns whether the panel took it.
    pub(crate) fn handle_sidebar_panel_mouse(&mut self, mouse: MouseEvent) -> bool {
        let at = (mouse.column, mouse.row);
        let on_panel = self
            .state
            .sidebar_panel_rect()
            .contains(ratatui::layout::Position::new(at.0, at.1))
            && !self.state.on_sidebar_divider(at.0, at.1);
        let panel = &mut self.state.sidebar_panel;
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) if on_panel => {
                panel.press = Some(SidebarPanelPress {
                    start: at,
                    last: at,
                    moved: false,
                });
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some(press) = panel.press.as_mut() else {
                    return false;
                };
                let step = (
                    i32::from(at.0) - i32::from(press.last.0),
                    i32::from(at.1) - i32::from(press.last.1),
                );
                if step != (0, 0) {
                    press.last = at;
                    press.moved = true;
                    panel.drag = (
                        clamp_i16(i32::from(panel.drag.0) + step.0),
                        clamp_i16(i32::from(panel.drag.1) + step.1),
                    );
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let Some(press) = panel.press.take() else {
                    return false;
                };
                if !press.moved {
                    self.open_sidebar_panel_click(press.start);
                }
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                if on_panel && panel.config.zoom_levels > 1 =>
            {
                panel.zoom = if mouse.kind == MouseEventKind::ScrollUp {
                    (panel.zoom + 1).min(panel.config.zoom_levels - 1)
                } else {
                    panel.zoom.saturating_sub(1)
                };
                panel.drag = (0, 0);
            }
            _ => return false,
        }
        true
    }

    /// Runs `click_command` in a popup for a click at `at` on the panel.
    fn open_sidebar_panel_click(&mut self, at: (u16, u16)) {
        let panel = &self.state.sidebar_panel;
        let config = &panel.config;
        if config.click_command.trim().is_empty() {
            return;
        }
        let command = config.click_command.clone();
        let geometry = crate::app::popup::PopupGeometry {
            width: config.click_width,
            height: config.click_height,
        };
        let view = panel_view_env(panel);
        let (mut environment, _) = self.custom_command_env();
        environment.extend(view);
        let body = crate::ui::sidebar_panel_body_rect(self.state.sidebar_panel_rect());
        if body.contains(ratatui::layout::Position::new(at.0, at.1)) {
            environment.push(("HERDR_PANEL_COLUMN".into(), (at.0 - body.x).to_string()));
            environment.push(("HERDR_PANEL_LINE".into(), (at.1 - body.y).to_string()));
            environment.push(("HERDR_PANEL_WIDTH".into(), body.width.to_string()));
            environment.push(("HERDR_PANEL_HEIGHT".into(), body.height.to_string()));
        }
        if let Err(error) = self.spawn_popup_shell_command(&command, None, environment, geometry) {
            tracing::warn!(command = %command, %error, "sidebar panel click command failed");
        }
    }
}

/// `HERDR_PANEL_ZOOM`, `HERDR_PANEL_DRAG_X` and `HERDR_PANEL_DRAG_Y` for the panel's commands.
fn panel_view_env(panel: &SidebarPanelState) -> [(String, String); 3] {
    [
        ("HERDR_PANEL_ZOOM".into(), panel.zoom.to_string()),
        ("HERDR_PANEL_DRAG_X".into(), panel.drag.0.to_string()),
        ("HERDR_PANEL_DRAG_Y".into(), panel.drag.1.to_string()),
    ]
}

fn clamp_i16(value: i32) -> i16 {
    value.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
}

/// Parses command output into at most `max_lines` styled lines.
///
/// SGR sequences (`ESC [ ... m`) set colors and attributes; other escape sequences and control
/// characters are dropped, and tabs become spaces.
pub(crate) fn parse_sgr_lines(output: &[u8], max_lines: usize) -> Vec<Line<'static>> {
    let text = String::from_utf8_lossy(output);
    let mut style = Style::default();
    let mut lines = Vec::new();
    for raw in text.split('\n').take(max_lines) {
        let mut spans = Vec::new();
        let mut current = String::new();
        let mut chars = raw.chars().peekable();
        while let Some(character) = chars.next() {
            if character == '\u{1b}' {
                let params = match chars.next() {
                    Some('[') => {
                        let mut params = String::new();
                        let mut last = None;
                        for next in chars.by_ref() {
                            if ('\u{40}'..='\u{7e}').contains(&next) {
                                last = Some(next);
                                break;
                            }
                            params.push(next);
                        }
                        (last == Some('m')).then_some(params)
                    }
                    Some(']') => {
                        // OSC: skip to BEL or ESC \.
                        while let Some(next) = chars.next() {
                            if next == '\u{7}'
                                || (next == '\u{1b}' && chars.next_if_eq(&'\\').is_some())
                            {
                                break;
                            }
                        }
                        None
                    }
                    _ => None,
                };
                if let Some(params) = params {
                    let next_style = apply_sgr(style, &params);
                    if next_style != style && !current.is_empty() {
                        spans.push(Span::styled(std::mem::take(&mut current), style));
                    }
                    style = next_style;
                }
            } else if character == '\t' {
                current.push(' ');
            } else if !character.is_control() && !is_unicode_format_control(character) {
                current.push(character);
            }
        }
        if !current.is_empty() {
            spans.push(Span::styled(current, style));
        }
        lines.push(Line::from(spans));
    }
    while lines.last().is_some_and(|line| line.spans.is_empty()) {
        lines.pop();
    }
    lines
}

fn apply_sgr(mut style: Style, params: &str) -> Style {
    let codes: Vec<u16> = if params.is_empty() {
        vec![0]
    } else {
        params
            .split([';', ':'])
            .map(|code| code.parse().unwrap_or(0))
            .collect()
    };
    let mut codes = codes.into_iter();
    while let Some(code) = codes.next() {
        style = match code {
            0 => Style::default(),
            1 => style.add_modifier(Modifier::BOLD),
            2 => style.add_modifier(Modifier::DIM),
            3 => style.add_modifier(Modifier::ITALIC),
            4 => style.add_modifier(Modifier::UNDERLINED),
            7 => style.add_modifier(Modifier::REVERSED),
            9 => style.add_modifier(Modifier::CROSSED_OUT),
            22 => style.remove_modifier(Modifier::BOLD | Modifier::DIM),
            23 => style.remove_modifier(Modifier::ITALIC),
            24 => style.remove_modifier(Modifier::UNDERLINED),
            27 => style.remove_modifier(Modifier::REVERSED),
            29 => style.remove_modifier(Modifier::CROSSED_OUT),
            30..=37 => style.fg(Color::Indexed((code - 30) as u8)),
            90..=97 => style.fg(Color::Indexed((code - 90 + 8) as u8)),
            40..=47 => style.bg(Color::Indexed((code - 40) as u8)),
            100..=107 => style.bg(Color::Indexed((code - 100 + 8) as u8)),
            39 => Style { fg: None, ..style },
            49 => Style { bg: None, ..style },
            38 | 48 => {
                let color = match codes.next() {
                    Some(5) => codes.next().map(|index| Color::Indexed(index as u8)),
                    Some(2) => match (codes.next(), codes.next(), codes.next()) {
                        (Some(r), Some(g), Some(b)) => Some(Color::Rgb(r as u8, g as u8, b as u8)),
                        _ => None,
                    },
                    _ => None,
                };
                match (code, color) {
                    (38, Some(color)) => style.fg(color),
                    (48, Some(color)) => style.bg(color),
                    _ => style,
                }
            }
            _ => style,
        };
    }
    style
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hidden_panel_has_no_deadline_and_runs_once_shown() {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &crate::config::Config::default(),
            true,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        app.configure_sidebar_panel(&PanelSidebarConfig {
            command: "true".into(),
            ..Default::default()
        });
        let now = Instant::now();

        // A collapsed sidebar: an overdue deadline here would wake the loop without end.
        app.state.sidebar_panel.body_size = (0, 0);
        app.handle_sidebar_panel_tasks(now);
        assert_eq!(app.next_sidebar_panel_deadline(), None);

        app.state.sidebar_panel.body_size = (25, 12);
        assert!(app
            .next_sidebar_panel_deadline()
            .is_some_and(|deadline| deadline <= now));
        app.handle_sidebar_panel_tasks(now);
        assert!(app
            .sidebar_panel
            .as_ref()
            .is_some_and(|runtime| runtime.task.is_some()));
        assert_eq!(app.next_sidebar_panel_deadline(), None);
    }

    #[tokio::test]
    async fn failed_run_keeps_the_output_and_marks_it_stale() {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &crate::config::Config::default(),
            true,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        app.configure_sidebar_panel(&PanelSidebarConfig {
            command: "true".into(),
            ..Default::default()
        });
        let generation = app.sidebar_panel_generation;

        assert!(app.handle_sidebar_panel_command_finished(generation, Ok(b"map".to_vec())));
        assert!(app.handle_sidebar_panel_command_finished(generation, Err("exit 1".into())));
        assert!(app.state.sidebar_panel.stale);
        assert_eq!(app.state.sidebar_panel.lines[0].spans[0].content, "map");
        assert!(!app.handle_sidebar_panel_command_finished(generation, Err("exit 1".into())));
        assert!(app.handle_sidebar_panel_command_finished(generation, Ok(b"map".to_vec())));
        assert!(!app.state.sidebar_panel.stale);
    }

    #[test]
    fn sgr_colors_split_spans_and_reset() {
        let lines = parse_sgr_lines(b"\x1b[1;38;2;255;0;124m#112\x1b[0m Eval\n\x1b[32mok", 8);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].spans[0].content, "#112");
        assert_eq!(
            lines[0].spans[0].style,
            Style::default()
                .fg(Color::Rgb(255, 0, 124))
                .add_modifier(Modifier::BOLD)
        );
        assert_eq!(lines[0].spans[1].content, " Eval");
        assert_eq!(lines[0].spans[1].style, Style::default());
        assert_eq!(
            lines[1].spans[0].style,
            Style::default().fg(Color::Indexed(2))
        );
    }

    #[test]
    fn non_sgr_escapes_and_controls_are_dropped() {
        let lines = parse_sgr_lines(b"a\x1b[2Kb\x1b]8;;http://x\x07c\td\r\n\n\n", 8);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].spans[0].content, "abc d");
    }

    #[test]
    fn output_is_cut_to_the_body_rows() {
        assert_eq!(parse_sgr_lines(b"1\n2\n3\n4", 2).len(), 2);
    }
}
