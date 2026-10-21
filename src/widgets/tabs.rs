use std::{cmp, collections::BTreeMap};

use zellij_tile::{
    prelude::{InputMode, ModeInfo, PaneInfo, PaneManifest, TabInfo},
    shim::switch_tab_to,
};

use crate::{config::ZellijState, render::FormattedPart};

use super::widget::Widget;

pub struct TabsWidget {
    active_tab_format: Vec<FormattedPart>,
    active_tab_fullscreen_format: Vec<FormattedPart>,
    active_tab_sync_format: Vec<FormattedPart>,
    normal_tab_format: Vec<FormattedPart>,
    normal_tab_fullscreen_format: Vec<FormattedPart>,
    normal_tab_sync_format: Vec<FormattedPart>,
    normal_tab_bell_format: Option<Vec<FormattedPart>>,
    normal_tab_flashing_bell_format: Option<Vec<FormattedPart>>,
    rename_tab_format: Vec<FormattedPart>,
    separator: Option<FormattedPart>,
    fullscreen_indicator: Option<String>,
    floating_indicator: Option<String>,
    sync_indicator: Option<String>,
    bell_indicator: Option<String>,
    flashing_bell_indicator: Option<String>,
    tab_display_count: Option<usize>,
    tab_truncate_start_format: Vec<FormattedPart>,
    tab_truncate_end_format: Vec<FormattedPart>,
    tab_zero_based_index: bool,
    spinner: Option<TabSpinner>,
}

/// Animates a marker that something else puts in front of a tab name, e.g. a
/// status hook renaming a tab to "» build" while work runs. The bar swaps the
/// marker for a spinner frame at render time, so the tab is renamed once
/// instead of on every frame.
#[derive(Clone, Debug, PartialEq)]
pub struct TabSpinner {
    marker: String,
    frames: Vec<String>,
    interval_ms: u64,
}

impl TabSpinner {
    /// `tab_spinner_marker` and `tab_spinner_frames` (space separated) are
    /// required, `tab_spinner_fps` defaults to 4 and is capped at 20.
    pub fn from_config(config: &BTreeMap<String, String>) -> Option<Self> {
        let marker = config.get("tab_spinner_marker")?.trim().to_owned();
        let frames: Vec<String> = config
            .get("tab_spinner_frames")?
            .split_whitespace()
            .map(str::to_owned)
            .collect();

        if marker.is_empty() || frames.is_empty() {
            return None;
        }

        let fps = config
            .get("tab_spinner_fps")
            .and_then(|fps| fps.parse::<u64>().ok())
            .filter(|fps| *fps > 0)
            .unwrap_or(4)
            .min(20);

        Some(Self {
            marker,
            frames,
            interval_ms: 1000 / fps,
        })
    }

    /// Seconds between two frames.
    pub fn interval(&self) -> f64 {
        self.interval_ms as f64 / 1000.0
    }

    pub fn is_spinning(&self, name: &str) -> bool {
        name.strip_prefix(self.marker.as_str())
            .is_some_and(|rest| rest.starts_with(' '))
    }

    /// The name with its marker replaced by frame `tick`, or unchanged.
    pub fn frame(&self, name: &str, tick: u64) -> String {
        match name.strip_prefix(self.marker.as_str()) {
            Some(rest) if rest.starts_with(' ') => {
                let frame = &self.frames[(tick % self.frames.len() as u64) as usize];
                format!("{frame}{rest}")
            }
            _ => name.to_owned(),
        }
    }

    /// Frame number from the wall clock, so every spinner on screen agrees.
    fn tick_now(&self) -> u64 {
        chrono::Utc::now().timestamp_millis().max(0) as u64 / self.interval_ms
    }
}

impl TabsWidget {
    pub fn new(config: &BTreeMap<String, String>) -> Self {
        let mut normal_tab_format: Vec<FormattedPart> = Vec::new();
        if let Some(form) = config.get("tab_normal") {
            normal_tab_format = FormattedPart::multiple_from_format_string(form, config);
        }

        let normal_tab_fullscreen_format = match config.get("tab_normal_fullscreen") {
            Some(form) => FormattedPart::multiple_from_format_string(form, config),
            None => normal_tab_format.clone(),
        };

        let normal_tab_sync_format = match config.get("tab_normal_sync") {
            Some(form) => FormattedPart::multiple_from_format_string(form, config),
            None => normal_tab_format.clone(),
        };

        let normal_tab_bell_format = config
            .get("tab_normal_bell")
            .map(|form| FormattedPart::multiple_from_format_string(form, config));

        let normal_tab_flashing_bell_format = config
            .get("tab_normal_flashing_bell")
            .map(|form| FormattedPart::multiple_from_format_string(form, config));

        let mut active_tab_format = normal_tab_format.clone();
        if let Some(form) = config.get("tab_active") {
            active_tab_format = FormattedPart::multiple_from_format_string(form, config);
        }

        let active_tab_fullscreen_format = match config.get("tab_active_fullscreen") {
            Some(form) => FormattedPart::multiple_from_format_string(form, config),
            None => active_tab_format.clone(),
        };

        let active_tab_sync_format = match config.get("tab_active_sync") {
            Some(form) => FormattedPart::multiple_from_format_string(form, config),
            None => active_tab_format.clone(),
        };

        let rename_tab_format = match config.get("tab_rename") {
            Some(form) => FormattedPart::multiple_from_format_string(form, config),
            None => active_tab_format.clone(),
        };

        let tab_display_count = match config.get("tab_display_count") {
            Some(count) => count.parse::<usize>().ok(),
            None => None,
        };

        let tab_truncate_start_format = config
            .get("tab_truncate_start_format")
            .map(|form| FormattedPart::multiple_from_format_string(form, config))
            .unwrap_or_default();

        let tab_truncate_end_format = config
            .get("tab_truncate_end_format")
            .map(|form| FormattedPart::multiple_from_format_string(form, config))
            .unwrap_or_default();

        let tab_zero_based_index = match config.get("tab_zero_based_index") {
            Some(e) => matches!(e.as_str(), "true"),
            None => false,
        };

        let separator = config
            .get("tab_separator")
            .map(|s| FormattedPart::from_format_string(s, config));

        let bell_indicator = config.get("tab_bell_indicator").cloned();
        let flashing_bell_indicator = config
            .get("tab_flashing_bell_indicator")
            .cloned()
            .or_else(|| bell_indicator.clone());

        Self {
            normal_tab_format,
            normal_tab_fullscreen_format,
            normal_tab_sync_format,
            normal_tab_bell_format,
            normal_tab_flashing_bell_format,
            active_tab_format,
            active_tab_fullscreen_format,
            active_tab_sync_format,
            rename_tab_format,
            separator,
            floating_indicator: config.get("tab_floating_indicator").cloned(),
            sync_indicator: config.get("tab_sync_indicator").cloned(),
            fullscreen_indicator: config.get("tab_fullscreen_indicator").cloned(),
            bell_indicator,
            flashing_bell_indicator,
            tab_display_count,
            tab_truncate_start_format,
            tab_truncate_end_format,
            tab_zero_based_index,
            spinner: TabSpinner::from_config(config),
        }
    }
}

impl Widget for TabsWidget {
    fn process(&self, _name: &str, state: &ZellijState) -> String {
        let mut output = "".to_owned();
        let mut counter = 0;

        let (truncated_start, truncated_end, tabs) =
            get_tab_window(&state.tabs, self.tab_display_count);

        if truncated_start > 0 {
            for f in &self.tab_truncate_start_format {
                let mut content = f.content.clone();

                if content.contains("{count}") {
                    content = content.replace("{count}", (truncated_start).to_string().as_str());
                }

                output = format!("{output}{}", f.format_string(&content));
            }
        }

        for tab in &tabs {
            let content = self.render_tab(tab, &state.panes, &state.mode);
            counter += 1;

            output = format!("{}{}", output, content);

            if counter < tabs.len()
                && let Some(sep) = &self.separator
            {
                output = format!("{}{}", output, sep.format_string(&sep.content));
            }
        }

        if truncated_end > 0 {
            for f in &self.tab_truncate_end_format {
                let mut content = f.content.clone();

                if content.contains("{count}") {
                    content = content.replace("{count}", (truncated_end).to_string().as_str());
                }

                output = format!("{output}{}", f.format_string(&content));
            }
        }

        output
    }

    fn process_click(&self, _name: &str, state: &ZellijState, pos: usize) {
        let mut offset = 0;
        let mut counter = 0;

        let (truncated_start, truncated_end, tabs) =
            get_tab_window(&state.tabs, self.tab_display_count);

        let active_pos = &state
            .tabs
            .iter()
            .find(|t| t.active)
            .expect("no active tab")
            .position
            + 1;

        if truncated_start > 0 {
            for f in &self.tab_truncate_start_format {
                let mut content = f.content.clone();

                if content.contains("{count}") {
                    content = content.replace("{count}", (truncated_end).to_string().as_str());
                }

                offset += console::measure_text_width(&f.format_string(&content));

                if pos <= offset {
                    switch_tab_to(active_pos.saturating_sub(1) as u32);
                }
            }
        }

        for tab in &tabs {
            counter += 1;

            let mut rendered_content = self.render_tab(tab, &state.panes, &state.mode);

            if counter < tabs.len()
                && let Some(sep) = &self.separator
            {
                rendered_content =
                    format!("{}{}", rendered_content, sep.format_string(&sep.content));
            }

            let content_len = console::measure_text_width(&rendered_content);

            if pos > offset && pos < offset + content_len {
                switch_tab_to(tab.position as u32 + 1);

                break;
            }

            offset += content_len;
        }

        if truncated_end > 0 {
            for f in &self.tab_truncate_end_format {
                let mut content = f.content.clone();

                if content.contains("{count}") {
                    content = content.replace("{count}", (truncated_end).to_string().as_str());
                }

                offset += console::measure_text_width(&f.format_string(&content));

                if pos <= offset {
                    switch_tab_to(cmp::min(active_pos + 1, state.tabs.len()) as u32);
                }
            }
        }
    }
}

impl TabsWidget {
    fn select_format(&self, info: &TabInfo, mode: &ModeInfo) -> &Vec<FormattedPart> {
        if info.active && mode.mode == InputMode::RenameTab {
            return &self.rename_tab_format;
        }

        if !info.active && info.is_flashing_bell {
            let fmt = self
                .normal_tab_flashing_bell_format
                .as_ref()
                .or(self.normal_tab_bell_format.as_ref());
            if let Some(fmt) = fmt {
                return fmt;
            }
        }

        if !info.active
            && info.has_bell_notification
            && let Some(fmt) = self.normal_tab_bell_format.as_ref()
        {
            return fmt;
        }

        if info.active && info.is_fullscreen_active {
            return &self.active_tab_fullscreen_format;
        }

        if info.active && info.is_sync_panes_active {
            return &self.active_tab_sync_format;
        }

        if info.active {
            return &self.active_tab_format;
        }

        if info.is_fullscreen_active {
            return &self.normal_tab_fullscreen_format;
        }

        if info.is_sync_panes_active {
            return &self.normal_tab_sync_format;
        }

        &self.normal_tab_format
    }

    fn render_tab(&self, tab: &TabInfo, panes: &PaneManifest, mode: &ModeInfo) -> String {
        let formatters = self.select_format(tab, mode);
        let mut output = "".to_owned();

        let tab_name = match mode.mode {
            InputMode::RenameTab => match tab.name.is_empty() {
                true => "Enter name...".to_owned(),
                false => tab.name.clone(),
            },
            _name => match &self.spinner {
                Some(spinner) => spinner.frame(&tab.name, spinner.tick_now()),
                None => tab.name.clone(),
            },
        };

        for f in formatters.iter() {
            let mut content = f.content.clone();

            if content.contains("{name}") {
                content = content.replace("{name}", &tab_name);
            }

            if content.contains("{index}") {
                let index = match self.tab_zero_based_index {
                    true => tab.position,
                    false => tab.position + 1,
                };
                content = content.replace("{index}", index.to_string().as_str());
            }

            if content.contains("{floating_total_count}") {
                let panes_for_tab: Vec<PaneInfo> =
                    panes.panes.get(&tab.position).cloned().unwrap_or_default();

                content = content.replace(
                    "{floating_total_count}",
                    &format!("{}", panes_for_tab.iter().filter(|p| p.is_floating).count()),
                );
            }

            if content.contains("{focused_pane_title}") {
                let panes_for_tab: Vec<PaneInfo> =
                    panes.panes.get(&tab.position).cloned().unwrap_or_default();

                let focused_pane_title = panes_for_tab
                    .iter()
                    .find(|pane| pane.is_focused)
                    .map(|pane| pane.title.clone())
                    .unwrap_or_default();

                content = content.replace("{focused_pane_title}", &focused_pane_title);
            }

            content = self.replace_indicators(content, tab, panes);

            output = format!("{}{}", output, f.format_string(&content));
        }

        output.to_owned()
    }

    fn replace_indicators(&self, content: String, tab: &TabInfo, panes: &PaneManifest) -> String {
        let mut content = content;
        if content.contains("{fullscreen_indicator}")
            && let Some(fullscreen_indicator) = self.fullscreen_indicator.clone()
        {
            content = content.replace(
                "{fullscreen_indicator}",
                if tab.is_fullscreen_active {
                    fullscreen_indicator.as_ref()
                } else {
                    ""
                },
            );
        }

        if content.contains("{sync_indicator}")
            && let Some(sync_indicator) = self.sync_indicator.clone()
        {
            content = content.replace(
                "{sync_indicator}",
                if tab.is_sync_panes_active {
                    sync_indicator.as_ref()
                } else {
                    ""
                },
            );
        }

        if content.contains("{floating_indicator}")
            && let Some(floating_indicator) = self.floating_indicator.clone()
        {
            let panes_for_tab: Vec<PaneInfo> =
                panes.panes.get(&tab.position).cloned().unwrap_or_default();

            let is_floating = panes_for_tab.iter().any(|p| p.is_floating);

            content = content.replace(
                "{floating_indicator}",
                if is_floating {
                    floating_indicator.as_ref()
                } else {
                    ""
                },
            );
        }

        if content.contains("{bell_indicator}")
            && (self.bell_indicator.is_some() || self.flashing_bell_indicator.is_some())
        {
            let indicator = if tab.is_flashing_bell {
                self.flashing_bell_indicator.as_deref().unwrap_or("")
            } else if tab.has_bell_notification {
                self.bell_indicator.as_deref().unwrap_or("")
            } else {
                ""
            };

            content = content.replace("{bell_indicator}", indicator);
        }

        content
    }
}

pub fn get_tab_window(
    tabs: &Vec<TabInfo>,
    max_count: Option<usize>,
) -> (usize, usize, Vec<TabInfo>) {
    let max_count = match max_count {
        Some(count) => count,
        None => return (0, 0, tabs.to_vec()),
    };

    if tabs.len() <= max_count {
        return (0, 0, tabs.to_vec());
    }

    let active_index = tabs.iter().position(|t| t.active).expect("no active tab");

    // active tab is in the last #max_count tabs, so return the last #max_count
    if active_index > tabs.len().saturating_sub(max_count) {
        return (
            tabs.len().saturating_sub(max_count),
            0,
            tabs.iter()
                .cloned()
                .rev()
                .take(max_count)
                .rev()
                .collect::<Vec<TabInfo>>(),
        );
    }

    // tabs must be truncated
    let first_index = active_index.saturating_sub(1);
    let last_index = cmp::min(first_index + max_count, tabs.len());

    (
        first_index,
        tabs.len().saturating_sub(last_index),
        tabs.as_slice()[first_index..last_index].to_vec(),
    )
}

#[cfg(test)]
mod test {
    use std::collections::BTreeMap;

    use zellij_tile::prelude::TabInfo;

    use super::{TabSpinner, TabsWidget, get_tab_window};
    use crate::{config::ZellijState, widgets::widget::Widget};
    use rstest::rstest;

    fn spinner_config(fps: Option<&str>) -> BTreeMap<String, String> {
        let mut config = BTreeMap::from([
            ("tab_spinner_marker".to_owned(), "»".to_owned()),
            ("tab_spinner_frames".to_owned(), "⠋ ⠙ ⠹".to_owned()),
        ]);
        if let Some(fps) = fps {
            config.insert("tab_spinner_fps".to_owned(), fps.to_owned());
        }
        config
    }

    #[test]
    fn spinner_needs_marker_and_frames() {
        assert!(TabSpinner::from_config(&spinner_config(None)).is_some());

        let mut no_marker = spinner_config(None);
        no_marker.remove("tab_spinner_marker");
        assert!(TabSpinner::from_config(&no_marker).is_none());

        let mut no_frames = spinner_config(None);
        no_frames.insert("tab_spinner_frames".to_owned(), "  ".to_owned());
        assert!(TabSpinner::from_config(&no_frames).is_none());
    }

    #[rstest]
    #[case(None, 250)]
    #[case(Some("2"), 500)]
    #[case(Some("0"), 250)]
    #[case(Some("fast"), 250)]
    #[case(Some("100"), 50)]
    fn spinner_interval(#[case] fps: Option<&str>, #[case] expected_ms: u64) {
        let spinner = TabSpinner::from_config(&spinner_config(fps)).unwrap();
        assert_eq!((spinner.interval() * 1000.0).round() as u64, expected_ms);
    }

    #[rstest]
    #[case("» build", 0, "⠋ build")]
    #[case("» build", 1, "⠙ build")]
    #[case("» build", 4, "⠙ build")]
    #[case("»build", 0, "»build")]
    #[case("✓ build", 0, "✓ build")]
    #[case("build", 0, "build")]
    fn spinner_frame(#[case] name: &str, #[case] tick: u64, #[case] expected: &str) {
        let spinner = TabSpinner::from_config(&spinner_config(None)).unwrap();
        assert_eq!(spinner.frame(name, tick), expected);
        assert_eq!(spinner.is_spinning(name), name.starts_with("» "));
    }

    #[test]
    fn spinner_replaces_marker_in_rendered_tab() {
        let mut config = spinner_config(None);
        config.insert("tab_normal".to_owned(), "{name}".to_owned());
        let state = ZellijState {
            tabs: vec![TabInfo {
                name: "» build".to_owned(),
                active: true,
                ..TabInfo::default()
            }],
            ..ZellijState::default()
        };

        let output = TabsWidget::new(&config).process("tabs", &state);

        assert!(output.contains(" build"));
        assert!(!output.contains('»'));
    }

    #[rstest]
    #[case(
        vec![
            TabInfo {
                active: false,
                name: "1".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "2".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: true,
                name: "3".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "4".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "5".to_owned(),
                ..TabInfo::default()
            },
        ],
        Some(3),
        (1, 1, vec![
                TabInfo {
                    active: false,
                    name: "2".to_owned(),
                    ..TabInfo::default()
                },
                TabInfo {
                    active: true,
                    name: "3".to_owned(),
                    ..TabInfo::default()
                },
                TabInfo {
                    active: false,
                    name: "4".to_owned(),
                    ..TabInfo::default()
                },
            ]
        )
    )]
    #[case(
        vec![
            TabInfo {
                active: true,
                name: "1".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "2".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "3".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "4".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "5".to_owned(),
                ..TabInfo::default()
            },
        ],
        Some(3),
        (0, 2, vec![
                TabInfo {
                    active: true,
                    name: "1".to_owned(),
                    ..TabInfo::default()
                },
                TabInfo {
                    active: false,
                    name: "2".to_owned(),
                    ..TabInfo::default()
                },
                TabInfo {
                    active: false,
                    name: "3".to_owned(),
                    ..TabInfo::default()
                },
            ]
        )
    )]
    #[case(
        vec![
            TabInfo {
                active: false,
                name: "1".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: true,
                name: "2".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "3".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "4".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "5".to_owned(),
                ..TabInfo::default()
            },
        ],
        Some(3),
        (0, 2, vec![
                TabInfo {
                    active: false,
                    name: "1".to_owned(),
                    ..TabInfo::default()
                },
                TabInfo {
                    active: true,
                    name: "2".to_owned(),
                    ..TabInfo::default()
                },
                TabInfo {
                    active: false,
                    name: "3".to_owned(),
                    ..TabInfo::default()
                },
            ]
        )
    )]
    #[case(
        vec![
            TabInfo {
                active: false,
                name: "1".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "2".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "3".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "4".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: true,
                name: "5".to_owned(),
                ..TabInfo::default()
            },
        ],
        Some(3),
        (2, 0, vec![
                TabInfo {
                    active: false,
                    name: "3".to_owned(),
                    ..TabInfo::default()
                },
                TabInfo {
                    active: false,
                    name: "4".to_owned(),
                    ..TabInfo::default()
                },
                TabInfo {
                    active: true,
                    name: "5".to_owned(),
                    ..TabInfo::default()
                },
            ]
        )
    )]
    #[case(
        vec![
            TabInfo {
                active: false,
                name: "1".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "2".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "3".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: true,
                name: "4".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "5".to_owned(),
                ..TabInfo::default()
            },
        ],
        Some(3),
        (2, 0, vec![
                TabInfo {
                    active: false,
                    name: "3".to_owned(),
                    ..TabInfo::default()
                },
                TabInfo {
                    active: true,
                    name: "4".to_owned(),
                    ..TabInfo::default()
                },
                TabInfo {
                    active: false,
                    name: "5".to_owned(),
                    ..TabInfo::default()
                },
            ]
        )
    )]
    #[case(
        vec![
            TabInfo {
                active: false,
                name: "1".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "2".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: true,
                name: "3".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "4".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "5".to_owned(),
                ..TabInfo::default()
            },
        ],
        None,
        (0, 0, vec![
            TabInfo {
                active: false,
                name: "1".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "2".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: true,
                name: "3".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "4".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "5".to_owned(),
                ..TabInfo::default()
            },
            ]
        )
    )]
    #[case(
        vec![
            TabInfo {
                active: false,
                name: "1".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: true,
                name: "2".to_owned(),
                ..TabInfo::default()
            },
        ],
        Some(3),
        (0, 0, vec![
            TabInfo {
                active: false,
                name: "1".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: true,
                name: "2".to_owned(),
                ..TabInfo::default()
            },
            ]
        )
    )]
    #[case(
        vec![
            TabInfo {
                active: false,
                name: "1".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: true,
                name: "2".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "3".to_owned(),
                ..TabInfo::default()
            },
        ],
        Some(3),
        (0, 0, vec![
            TabInfo {
                active: false,
                name: "1".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: true,
                name: "2".to_owned(),
                ..TabInfo::default()
            },
            TabInfo {
                active: false,
                name: "3".to_owned(),
                ..TabInfo::default()
            },
            ]
        )
    )]
    pub fn test_get_tab_window(
        #[case] tabs: Vec<TabInfo>,
        #[case] max_count: Option<usize>,
        #[case] expected: (usize, usize, Vec<TabInfo>),
    ) {
        let res = get_tab_window(&tabs, max_count);

        assert_eq!(res, expected);
    }
}
