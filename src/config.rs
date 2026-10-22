use std::{collections::BTreeMap, str::FromStr, sync::Arc};

use itertools::Itertools;
use regex::Regex;
use zellij_tile::prelude::*;

use crate::{
    border::{BorderConfig, BorderPosition, parse_border_config},
    render::FormattedPart,
    widgets::{command::CommandResult, notification, widget::Widget},
};
use chrono::{DateTime, Local};

#[derive(Default, Debug, Clone)]
pub struct ZellijState {
    pub cols: usize,
    pub command_results: BTreeMap<String, CommandResult>,
    pub pipe_results: BTreeMap<String, String>,
    pub mode: ModeInfo,
    pub panes: PaneManifest,
    pub plugin_uuid: String,
    pub tabs: Vec<TabInfo>,
    pub sessions: Vec<SessionInfo>,
    pub start_time: DateTime<Local>,
    pub incoming_notification: Option<notification::Message>,
    pub cache_mask: u8,
    pub focused_pane_id: Option<PaneId>,
    pub focused_pane_cwd: Option<std::path::PathBuf>,
    /// Columns the tabs may fill with tab_fit_width: beside the rest of the
    /// bar, and once the overlength rule hides the parts ranked below them.
    pub tab_space: Option<(usize, usize)>,
}

#[derive(Clone, Debug, Ord, Eq, PartialEq, PartialOrd, Copy)]
pub enum Part {
    Left,
    Center,
    Right,
}

impl FromStr for Part {
    fn from_str(part: &str) -> Result<Self> {
        match part {
            "l" => Ok(Part::Left),
            "c" => Ok(Part::Center),
            "r" => Ok(Part::Right),
            _ => anyhow::bail!("Invalid part: {}", part),
        }
    }

    type Err = anyhow::Error;
}

pub enum UpdateEventMask {
    Always = 0b10000000,
    Mode = 0b00000001,
    Tab = 0b00000011,
    Command = 0b00000100,
    Session = 0b00001000,
    None = 0b00000000,
}

pub fn event_mask_from_widget_name(name: &str) -> u8 {
    match name {
        "command" => UpdateEventMask::Always as u8,
        "datetime" => UpdateEventMask::Always as u8,
        "mode" => UpdateEventMask::Mode as u8,
        "notifications" => UpdateEventMask::Always as u8,
        "session" => UpdateEventMask::Mode as u8,
        "swap_layout" => UpdateEventMask::Tab as u8,
        "tabs" => UpdateEventMask::Tab as u8,
        "pipe" => UpdateEventMask::Always as u8,
        _ => UpdateEventMask::None as u8,
    }
}

#[derive(Default, Debug)]
pub struct ModuleConfig {
    pub left_parts_config: String,
    pub left_parts: Vec<FormattedPart>,
    pub center_parts_config: String,
    pub center_parts: Vec<FormattedPart>,
    pub right_parts_config: String,
    pub right_parts: Vec<FormattedPart>,
    pub format_space: FormattedPart,
    pub hide_frame_for_single_pane: bool,
    pub hide_frame_except_for_search: bool,
    pub hide_frame_except_for_fullscreen: bool,
    pub hide_frame_except_for_scroll: bool,
    pub pane_frame_style: String,
    pub border: BorderConfig,
    pub format_precedence: Vec<Part>,
    pub hide_on_overlength: bool,
    /// The part holding {tabs} when tab_fit_width is on, and that part
    /// without {tabs}, rendered to measure what the tabs must leave room for.
    pub tabs_part: Option<Part>,
    pub tabs_rest_parts: Vec<FormattedPart>,
    pub last_tab_space: Option<(usize, usize)>,
}

impl ModuleConfig {
    /// Whether any frame-hiding option is enabled. These are the only features
    /// that read Zellij's periodic session updates, so when they are all off
    /// the plugin does not need to subscribe to them at all.
    pub fn needs_session_updates(&self) -> bool {
        self.hide_frame_for_single_pane
            || self.hide_frame_except_for_search
            || self.hide_frame_except_for_fullscreen
            || self.hide_frame_except_for_scroll
    }

    pub fn new(config: &BTreeMap<String, String>) -> anyhow::Result<Self> {
        let format_space_config = match config.get("format_space") {
            Some(space_config) => space_config,
            None => "",
        };

        let hide_frame_for_single_pane = match config.get("hide_frame_for_single_pane") {
            Some(toggle) => toggle == "true",
            None => false,
        };
        let hide_frame_except_for_search = match config.get("hide_frame_except_for_search") {
            Some(toggle) => toggle == "true",
            None => false,
        };
        let hide_frame_except_for_fullscreen = match config.get("hide_frame_except_for_fullscreen")
        {
            Some(toggle) => toggle == "true",
            None => false,
        };
        let hide_frame_except_for_scroll = match config.get("hide_frame_except_for_scroll") {
            Some(toggle) => toggle == "true",
            None => false,
        };
        let pane_frame_style = match config.get("pane_frame_style") {
            Some(style) => style.to_owned(),
            None => "titles".to_owned(),
        };

        let left_parts_config = match config.get("format_left") {
            Some(conf) => conf,
            None => "",
        };

        let right_parts_config = match config.get("format_right") {
            Some(conf) => conf,
            None => "",
        };

        let center_parts_config = match config.get("format_center") {
            Some(conf) => conf,
            None => "",
        };

        let format_precedence = match config.get("format_precedence") {
            Some(conf) => {
                let prec = conf
                    .chars()
                    .map(|c| Part::from_str(&c.to_string()))
                    .collect();

                match prec {
                    Ok(prec) => prec,
                    Err(e) => {
                        anyhow::bail!("Invalid format_precedence: {}", e);
                    }
                }
            }
            None => vec![Part::Left, Part::Center, Part::Right],
        };

        let hide_on_overlength = match config.get("format_hide_on_overlength") {
            Some(opt) => opt == "true",
            None => false,
        };

        let border_config = parse_border_config(config).unwrap_or_default();

        let tabs_part = config
            .get("tab_fit_width")
            .is_some_and(|v| v == "true")
            .then(|| {
                [
                    (Part::Left, left_parts_config),
                    (Part::Center, center_parts_config),
                    (Part::Right, right_parts_config),
                ]
                .into_iter()
                .find(|(_, conf)| conf.contains("{tabs}"))
            })
            .flatten();
        let tabs_rest_parts = match tabs_part {
            Some((_, conf)) => parts_from_config(Some(&conf.replace("{tabs}", "")), config),
            None => vec![],
        };

        Ok(Self {
            left_parts_config: left_parts_config.to_owned(),
            left_parts: parts_from_config(Some(&left_parts_config.to_owned()), config),
            center_parts_config: center_parts_config.to_owned(),
            center_parts: parts_from_config(Some(&center_parts_config.to_owned()), config),
            right_parts_config: right_parts_config.to_owned(),
            right_parts: parts_from_config(Some(&right_parts_config.to_owned()), config),
            format_space: FormattedPart::from_format_string(format_space_config, config),
            hide_frame_for_single_pane,
            hide_frame_except_for_search,
            hide_frame_except_for_fullscreen,
            hide_frame_except_for_scroll,
            pane_frame_style,
            border: border_config,
            format_precedence,
            hide_on_overlength,
            tabs_part: tabs_part.map(|(part, _)| part),
            tabs_rest_parts,
            last_tab_space: None,
        })
    }

    pub fn handle_mouse_action(
        &mut self,
        mut state: ZellijState,
        mouse: Mouse,
        widget_map: BTreeMap<String, Arc<dyn Widget>>,
    ) {
        let click_pos = match mouse {
            Mouse::ScrollUp(_) => return,
            Mouse::ScrollDown(_) => return,
            Mouse::ScrollLeft(_) => return,
            Mouse::ScrollRight(_) => return,
            Mouse::LeftClick(_, y) => y,
            Mouse::RightClick(_, y) => y,
            Mouse::Hold(_, y) => y,
            Mouse::Release(_, y) => y,
            Mouse::Hover(_, _) => return,
        };

        let (output_left, output_center, output_right) = self.render_parts(&mut state, &widget_map);

        let (output_left, output_center, output_right) = match self.hide_on_overlength {
            true => self.trim_output(&output_left, &output_center, &output_right, state.cols),
            false => (output_left, output_center, output_right),
        };

        let mut offset = console::measure_text_width(&output_left);

        self.process_widget_click(click_pos, &self.left_parts, &widget_map, &state, 0);

        if click_pos <= offset {
            return;
        }

        if !output_center.is_empty() {
            tracing::debug!("widgetclick center");
            offset += console::measure_text_width(&self.get_spacer_left(
                &output_left,
                &output_center,
                state.cols,
            ));

            offset += self.process_widget_click(
                click_pos,
                &self.center_parts,
                &widget_map,
                &state,
                offset,
            );

            if click_pos <= offset {
                return;
            }

            offset += console::measure_text_width(&self.get_spacer_right(
                &output_right,
                &output_center,
                state.cols,
            ));
        } else {
            offset += console::measure_text_width(&self.get_spacer(
                &output_left,
                &output_right,
                state.cols,
            ));
        }

        self.process_widget_click(click_pos, &self.right_parts, &widget_map, &state, offset);
    }

    fn process_widget_click(
        &self,
        click_pos: usize,
        widgets: &[FormattedPart],
        widget_map: &BTreeMap<String, Arc<dyn Widget>>,
        state: &ZellijState,
        offset: usize,
    ) -> usize {
        let widget_string = widgets.iter().fold(String::new(), |a, b| a + &b.content);

        let mut rendered_output = widget_string.clone();

        let tokens: Vec<String> = widget_map.keys().map(|k| k.to_owned()).collect();

        let widgets_regex = Regex::new("(\\{[a-z_0-9]+\\})").unwrap();
        for widget in widgets_regex.captures_iter(widget_string.as_str()) {
            let match_name = widget.get(0).unwrap().as_str();
            let widget_key = match_name.trim_matches(|c| c == '{' || c == '}');
            let mut widget_key_name = widget_key;

            if widget_key.starts_with("command_") {
                widget_key_name = "command";
            }

            if widget_key.starts_with("pipe_") {
                widget_key_name = "pipe";
            }

            if !tokens.contains(&widget_key_name.to_owned()) {
                continue;
            }

            let wid = match widget_map.get(widget_key_name) {
                Some(wid) => wid,
                None => continue,
            };

            let pos = match rendered_output.find(match_name) {
                Some(_pos) => {
                    let pref = rendered_output.split(match_name).collect::<Vec<&str>>()[0];
                    console::measure_text_width(pref)
                }
                None => continue,
            };

            let wid_res = wid.process(widget_key, state);
            rendered_output = rendered_output.replace(match_name, &wid_res);

            if click_pos < pos + offset
                || click_pos > pos + offset + console::measure_text_width(&wid_res)
            {
                continue;
            }

            wid.process_click(widget_key, state, click_pos - (pos + offset));
        }

        console::measure_text_width(&rendered_output)
    }

    pub fn render_bar(
        &mut self,
        mut state: ZellijState,
        widget_map: BTreeMap<String, Arc<dyn Widget>>,
    ) -> String {
        if self.left_parts.is_empty() && self.center_parts.is_empty() && self.right_parts.is_empty()
        {
            return "No configuration found. See https://github.com/dj95/zjstatus/wiki/3-%E2%80%90-Configuration for more info".to_string();
        }

        let (output_left, output_center, output_right) = self.render_parts(&mut state, &widget_map);

        let (output_left, output_center, output_right) = match self.hide_on_overlength {
            true => self.trim_output(&output_left, &output_center, &output_right, state.cols),
            false => (output_left, output_center, output_right),
        };

        if self.border.enabled {
            let mut border_top = "".to_owned();
            if self.border.enabled && self.border.position == BorderPosition::Top {
                border_top = format!("{}\n", self.border.draw(state.cols));
            }

            let mut border_bottom = "".to_owned();
            if self.border.enabled && self.border.position == BorderPosition::Bottom {
                border_bottom = format!("\n{}", self.border.draw(state.cols));
            }

            if !output_center.is_empty() {
                return format!(
                    "{}{}{}{}{}{}{}",
                    border_top,
                    output_left,
                    self.get_spacer_left(&output_left, &output_center, state.cols),
                    output_center,
                    self.get_spacer_right(&output_right, &output_center, state.cols),
                    output_right,
                    border_bottom,
                );
            }

            return format!(
                "{}{}{}{}{}",
                border_top,
                output_left,
                self.get_spacer(&output_left, &output_right, state.cols),
                output_right,
                border_bottom,
            );
        }

        if !output_center.is_empty() {
            return format!(
                "{}{}{}{}{}",
                output_left,
                self.get_spacer_left(&output_left, &output_center, state.cols),
                output_center,
                self.get_spacer_right(&output_right, &output_center, state.cols),
                output_right,
            );
        }

        format!(
            "{}{}{}",
            output_left,
            self.get_spacer(&output_left, &output_right, state.cols),
            output_right,
        )
    }

    /// Renders the left, center and right parts. With tab_fit_width the
    /// other parts go first, so the tabs get the room they leave.
    fn render_parts(
        &mut self,
        state: &mut ZellijState,
        widget_map: &BTreeMap<String, Arc<dyn Widget>>,
    ) -> (String, String, String) {
        let mut output = BTreeMap::new();

        for part in [Part::Left, Part::Center, Part::Right] {
            if Some(part) != self.tabs_part {
                output.insert(part, render_all(self.parts_mut(part), widget_map, state));
            }
        }

        if let Some(tabs_part) = self.tabs_part {
            let rest = console::measure_text_width(&render_all(
                &mut self.tabs_rest_parts,
                widget_map,
                state,
            ));
            let rank = |part: Part| self.format_precedence.iter().position(|p| *p == part);
            let others = |keep: &dyn Fn(Part) -> bool| -> usize {
                output
                    .iter()
                    .filter(|(part, _)| keep(**part))
                    .map(|(_, text)| console::measure_text_width(text))
                    .sum()
            };

            let narrow = state.cols.saturating_sub(rest + others(&|_| true));
            let wide = match self.hide_on_overlength {
                true => state
                    .cols
                    .saturating_sub(rest + others(&|part| rank(part) < rank(tabs_part))),
                false => narrow,
            };

            state.tab_space = Some((narrow, wide));
            if state.tab_space != self.last_tab_space {
                state.cache_mask |= UpdateEventMask::Tab as u8;
                self.last_tab_space = state.tab_space;
            }

            output.insert(
                tabs_part,
                render_all(self.parts_mut(tabs_part), widget_map, state),
            );
        }

        (
            output.remove(&Part::Left).unwrap_or_default(),
            output.remove(&Part::Center).unwrap_or_default(),
            output.remove(&Part::Right).unwrap_or_default(),
        )
    }

    fn parts_mut(&mut self, part: Part) -> &mut Vec<FormattedPart> {
        match part {
            Part::Left => &mut self.left_parts,
            Part::Center => &mut self.center_parts,
            Part::Right => &mut self.right_parts,
        }
    }

    fn trim_output(
        &self,
        output_left: &str,
        output_center: &str,
        output_right: &str,
        cols: usize,
    ) -> (String, String, String) {
        let center_pos = (cols as f32 / 2.0).floor() as usize;

        let mut output = BTreeMap::from([
            (Part::Left, output_left.to_owned()),
            (Part::Center, output_center.to_owned()),
            (Part::Right, output_right.to_owned()),
        ]);

        let combinations = [
            (self.format_precedence[2], self.format_precedence[1]),
            (self.format_precedence[1], self.format_precedence[0]),
            (self.format_precedence[2], self.format_precedence[0]),
        ];

        for win in combinations.iter() {
            let (a, b) = win;

            let part_a = output.get(a).unwrap();
            let part_b = output.get(b).unwrap();

            let a_count = console::measure_text_width(part_a);
            let b_count = console::measure_text_width(part_b);

            let overlap = match (a, b) {
                (Part::Left, Part::Right) => a_count + b_count > cols,
                (Part::Right, Part::Left) => a_count + b_count > cols,
                (Part::Left, Part::Center) => a_count > center_pos - (b_count / 2),
                (Part::Center, Part::Left) => b_count > center_pos - (a_count / 2),
                (Part::Right, Part::Center) => a_count > center_pos - (b_count / 2),
                (Part::Center, Part::Right) => b_count > center_pos - (a_count / 2),
                _ => false,
            };

            if overlap {
                output.insert(*a, "".to_owned());
            }
        }

        output.values().cloned().collect_tuple().unwrap()
    }

    #[tracing::instrument(skip_all)]
    fn get_spacer_left(&self, output_left: &str, output_center: &str, cols: usize) -> String {
        let text_count = console::measure_text_width(output_left)
            + (console::measure_text_width(output_center) as f32 / 2.0).floor() as usize;

        let center_pos = (cols as f32 / 2.0).floor() as usize;

        // verify we are able to count the difference, since zellij sometimes drops a col
        // count of 0 on tab creation
        let space_count = center_pos.saturating_sub(text_count);

        tracing::debug!("space_count: {:?}", space_count);
        self.format_space.format_string(&" ".repeat(space_count))
    }

    #[tracing::instrument(skip_all)]
    fn get_spacer_right(&self, output_right: &str, output_center: &str, cols: usize) -> String {
        let text_count = console::measure_text_width(output_right)
            + (console::measure_text_width(output_center) as f32 / 2.0).ceil() as usize;

        let center_pos = (cols as f32 / 2.0).ceil() as usize;

        // verify we are able to count the difference, since zellij sometimes drops a col
        // count of 0 on tab creation
        let space_count = center_pos.saturating_sub(text_count);

        tracing::debug!("space_count: {:?}", space_count);
        self.format_space.format_string(&" ".repeat(space_count))
    }

    fn get_spacer(&self, output_left: &str, output_right: &str, cols: usize) -> String {
        let text_count =
            console::measure_text_width(output_left) + console::measure_text_width(output_right);

        // verify we are able to count the difference, since zellij sometimes drops a col
        // count of 0 on tab creation
        let space_count = cols.saturating_sub(text_count);

        self.format_space.format_string(&" ".repeat(space_count))
    }
}

fn render_all(
    parts: &mut [FormattedPart],
    widget_map: &BTreeMap<String, Arc<dyn Widget>>,
    state: &ZellijState,
) -> String {
    parts.iter_mut().fold(String::new(), |acc, part| {
        acc + &part.format_string_with_widgets(widget_map, state)
    })
}

fn parts_from_config(
    format: Option<&String>,
    config: &BTreeMap<String, String>,
) -> Vec<FormattedPart> {
    match format {
        Some(format) => match format.is_empty() {
            true => vec![],
            false => format
                .split("#[")
                .map(|s| FormattedPart::from_format_string(s, config))
                .collect(),
        },
        None => vec![],
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_needs_session_updates() {
        let mut mc = ModuleConfig::default();
        assert!(!mc.needs_session_updates());

        mc.hide_frame_for_single_pane = true;
        assert!(mc.needs_session_updates());

        mc = ModuleConfig::default();
        mc.hide_frame_except_for_scroll = true;
        assert!(mc.needs_session_updates());
    }

    use anstyle::{Effects, RgbColor};

    #[test]
    fn test_formatted_part_from_string() {
        let input = "#[fg=#ff0000,bg=#00ff00,bold,italic]foo";

        let part = FormattedPart::from_format_string(input, &BTreeMap::new());

        assert_eq!(
            part,
            FormattedPart {
                fg: Some(RgbColor(255, 0, 0).into()),
                bg: Some(RgbColor(0, 255, 0).into()),
                effects: Effects::BOLD | Effects::ITALIC,
                content: "foo".to_owned(),
                ..Default::default()
            },
        )
    }

    // Click handlers call into zellij, which wasmtime cannot provide: a
    // widget behind `dyn Widget` would link them into the test binary.
    struct NoClick<W>(W);

    impl<W: Widget> Widget for NoClick<W> {
        fn process(&self, name: &str, state: &ZellijState) -> String {
            self.0.process(name, state)
        }

        fn process_click(&self, _name: &str, _state: &ZellijState, _pos: usize) {}
    }

    // The bar from dotfiles' zellij main.kdl, with the clock as fixed text
    // (the command widgets need a zellij host).
    fn fitted_bar() -> (ModuleConfig, BTreeMap<String, Arc<dyn Widget>>) {
        let config: BTreeMap<String, String> = [
            ("format_left", "#[fg=#928374,bg=#282828] {session} {tabs}"),
            (
                "format_right",
                "#[fg=#1d2021,bg=#d3869b,bold] W43 #[fg=#d4be98,bg=#45403d] Thu/22/10 #[fg=#1d2021,bg=#89b482,bold] 16:42:08 ",
            ),
            ("format_space", "#[bg=#282828]"),
            ("format_hide_on_overlength", "true"),
            (
                "tab_normal",
                "#[fg=#a89984,bg=#504945] {index} #[fg=#ddc7a1,bg=#504945]{name} ",
            ),
            (
                "tab_active",
                "#[fg=#7c6f64,bg=#d8a657] {index} #[fg=#1d2021,bg=#d8a657,bold]{name} ",
            ),
            ("tab_separator", " "),
            ("tab_fit_width", "true"),
            ("tab_truncate_start_format", "#[fg=#a89984,bg=#282828]‹{count} "),
            ("tab_truncate_end_format", "#[fg=#a89984,bg=#282828] {count}›"),
            ("tab_spinner_marker", "»"),
            ("tab_spinner_frames", "· ✢ ✳ ✶ ✻ ✽"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();

        let mut widgets: BTreeMap<String, Arc<dyn Widget>> = BTreeMap::new();
        widgets.insert(
            "session".to_owned(),
            Arc::new(NoClick(crate::widgets::session::SessionWidget::new(
                &config,
            ))),
        );
        widgets.insert(
            "tabs".to_owned(),
            Arc::new(NoClick(crate::widgets::tabs::TabsWidget::new(&config))),
        );

        (ModuleConfig::new(&config).unwrap(), widgets)
    }

    fn render_fitted(
        module: &mut ModuleConfig,
        widgets: &BTreeMap<String, Arc<dyn Widget>>,
        names: &[String],
        active: usize,
        cols: usize,
    ) -> String {
        let state = ZellijState {
            cols,
            mode: ModeInfo {
                session_name: Some("coding-mutations-2".to_owned()),
                ..ModeInfo::default()
            },
            tabs: names
                .iter()
                .enumerate()
                .map(|(i, name)| TabInfo {
                    position: i,
                    active: i == active,
                    name: name.clone(),
                    ..TabInfo::default()
                })
                .collect(),
            ..ZellijState::default()
        };

        console::strip_ansi_codes(&module.render_bar(state, widgets.clone())).to_string()
    }

    fn tab_sets() -> Vec<Vec<String>> {
        let real = [
            "✓ sloprik",
            "» coordination",
            "✓ 39067-system-made-values-attribution",
            "✓ 38947-38948-template-line-user-changes",
        ];
        let many = (0..17).map(|i| format!("story-{}{}", i, "-word".repeat(i % 6)));

        vec![
            real.iter().map(|s| s.to_string()).collect(),
            many.collect(),
            vec!["only-tab".to_owned()],
        ]
    }

    const SESSION_WIDTH: usize = " coding-mutations-2 ".len();
    const CLOCK_WIDTH: usize = " W43  Thu/22/10  16:42:08 ".len();

    fn marker_width(hidden: usize) -> usize {
        match hidden {
            0 => 0,
            n => 2 + n.to_string().len(),
        }
    }

    #[test]
    fn fitted_tabs_hold_up_at_every_width() {
        let marker = Regex::new(r"‹(\d+) |\s(\d+)›").unwrap();

        for names in tab_sets() {
            let count = names.len();

            for active in 0..count {
                let (mut module, widgets) = fitted_bar();

                for cols in 10..=400 {
                    let bar = render_fitted(&mut module, &widgets, &names, active, cols);
                    let width = console::measure_text_width(&bar);
                    let at = format!("{count} tabs, active {active}, {cols} cols: {bar:?}");

                    // the active tab, with at least the start of its name
                    let index = format!(" {} ", active + 1);
                    let head: String = names[active].chars().skip(1).take(4).collect();
                    assert!(bar.contains(&format!("{index}")), "no active tab: {at}");
                    assert!(bar.contains(&head), "active name cut away: {at}");

                    // hidden counts and shown tabs add up, around the active one
                    let mut hidden = (0, 0);
                    for cap in marker.captures_iter(&bar) {
                        if let Some(n) = cap.get(1) {
                            hidden.0 = n.as_str().parse().unwrap();
                        }
                        if let Some(n) = cap.get(2) {
                            hidden.1 = n.as_str().parse().unwrap();
                        }
                    }
                    let shown: Vec<usize> = (1..=count)
                        .filter(|i| bar.contains(&format!(" {i} ")))
                        .collect();
                    let expected: Vec<usize> = (hidden.0 + 1..=count - hidden.1).collect();
                    assert_eq!(shown, expected, "{at}");

                    let active_alone = SESSION_WIDTH
                        + 3
                        + (active + 1).to_string().len()
                        + console::measure_text_width(&names[active])
                        + marker_width(active)
                        + marker_width(count - 1 - active);

                    // fills the bar exactly once the squeezed active tab fits
                    if cols >= active_alone - names[active].chars().count() + 8 {
                        assert_eq!(width, cols, "bar not filled: {at}");
                    }

                    // the clock goes only when it could not have stayed
                    let clock = bar.contains("16:42:08");
                    if !clock {
                        assert!(
                            active_alone + CLOCK_WIDTH > cols || CLOCK_WIDTH > cols / 2,
                            "clock dropped with room for it: {at}"
                        );
                    }
                    if cols == 400 && count < 17 {
                        assert!(clock, "{at}");
                    }
                }
            }
        }
    }

    #[test]
    fn fitted_tabs_samples() {
        let names = &tab_sets()[0];
        let (mut module, widgets) = fitted_bar();

        for cols in [40, 47, 60, 80, 100, 120, 160, 200, 358] {
            let bar = render_fitted(&mut module, &widgets, names, 2, cols);
            println!("{cols:>4} |{bar}|");
        }
    }
}
