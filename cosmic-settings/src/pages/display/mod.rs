// Copyright 2023 System76 <info@system76.com>
// SPDX-License-Identifier: GPL-3.0-only

pub mod arrangement;
// pub mod night_light;

use crate::{app, pages};
use arrangement::Arrangement;
use cosmic::iced::wayland::window::start_drag_window;
use cosmic::iced::{ Alignment, Length, time };
use cosmic::iced_widget::scrollable::{Direction, Properties, RelativeOffset};
use cosmic::prelude::CollectionWidget;
use cosmic::widget::{
    self, column, container, dropdown, list_column, segmented_button, tab_bar, toggler
};
use cosmic::{command, Apply, Command, Element};
use cosmic_randr_shell::{List, Output, OutputKey, Transform};
use cosmic_settings_page::{self as page, section, Section};
use slab::Slab;
use slotmap::{Key, SlotMap};
use std::{
    collections::BTreeMap,
    process::ExitStatus, 
    sync::Arc,
};

/// Display color depth options
#[derive(Clone, Copy, Debug)]
pub struct ColorDepth(usize);

/// Identifies the content to display in the context drawer
// pub enum ContextDrawer {
//     NightLight,
// }

/// Display mirroring options
#[derive(Clone, Copy, Debug)]
pub enum Mirroring {
    Disable,
    ProjectToAll,
    Project(OutputKey),
    Mirror(OutputKey),
}

/// Night light preferences
// #[derive(Clone, Copy, Debug)]
// pub enum NightLight {
/// Toggles night light's automatic scheduling.
//     AutoSchedule(bool),
/// Sets the night light schedule.
//     ManualSchedule,
/// Changes the preferred night light temperature.
//     Temperature(f32),
/// Toggles night light mode
//     Toggle(bool),
// }

#[derive(Clone, Debug)]
pub enum Message {
    /// Change placement of display
    Position(OutputKey, i32, i32),
    /// Changes the active display being configured.
    Display(segmented_button::Entity),
    /// Set the color depth of a display.
    ColorDepth(ColorDepth),
    /// Set the color profile of a display.
    ColorProfile(usize),
    /// The dialog was cancelled, and will revert setting to the given request.
    DialogCancel(Randr),
    /// The dialog was completed.
    DialogComplete,
    /// Toggles display on or off.
    DisplayToggle(bool),
    /// Configures mirroring status of a display.
    Mirroring(Mirroring),
    /// Handle night light preferences.
    //  NightLight(NightLight),
    /// Show the night light mode context drawer.
    //  NightLightContext,
    /// Set the orientation of a display.
    Orientation(Transform),
    /// Status of an applied display change.
    RandrResult(Arc<std::io::Result<ExitStatus>>),
    /// Request to reload the page.
    Refresh,
    /// Set the refresh rate of a display.
    RefreshRate(usize),
    /// Set the resolution of a display.
    Resolution(usize),
    /// Set the preferred scale for a display.
    Scale(usize),
    /// Refreshes display outputs.
    Update {
        /// Available outputs from cosmic-randr.
        randr: Arc<Result<List, cosmic_randr_shell::Error>>,
    },
}

impl From<Message> for app::Message {
    fn from(message: Message) -> Self {
        let page_message = crate::pages::Message::Displays(message);
        app::Message::PageMessage(page_message)
    }
}

#[derive(Clone, Copy, Debug)]
enum Randr {
    Position(i32, i32),
    RefreshRate(u32),
    Resolution(u32, u32),
    Scale(u32),
    Transform(Transform),
    Toggle(bool),
}

/// The page struct for the display settings page.
pub struct Page {
    list: List,
    display_tabs: segmented_button::SingleSelectModel,
    active_display: OutputKey,
    background_service: Option<tokio::task::JoinHandle<()>>,
    config: Config,
    cache: ViewCache,
    //  context: Option<ContextDrawer>,
    display_arrangement_scrollable: cosmic::widget::Id,
    /// The setting to revert to if the next dialog page is cancelled, and the 
    /// instant the setting was changed.
    dialog: Option<(Randr, time::Instant)>,
}


/// Causes a dialog to be shown. If the dialog fails, the revert_action will
/// be performed. 
/// The revert_action should be which Randr call will revert the setting change.
//macro_rules! set_dialog {
//    ($self: ident, $Randr: ident { $($revert_action: $kind) }) => {
        //$self.dialog = Some(($revert_action, time::Instant::now()))
    //};
//}

impl Default for Page {
    fn default() -> Self {
        Self {
            list: List::default(),
            display_tabs: segmented_button::SingleSelectModel::default(),
            active_display: OutputKey::default(),
            background_service: None,
            config: Config::default(),
            cache: ViewCache::default(),
            //          context: None,
            display_arrangement_scrollable: cosmic::widget::Id::unique(),
            dialog: None,
        }
    }
}

#[derive(Default)]
struct Config {
    /// Whether night light is enabled.
    //  night_light_enabled: bool,
    refresh_rate: Option<u32>,
    resolution: Option<(u32, u32)>,
    scale: u32,
}

/// Cached view content for widgets.
#[derive(Default)]
struct ViewCache {
    modes: BTreeMap<(u32, u32), Vec<u32>>,
    orientations: [String; 4],
    refresh_rates: Vec<String>,
    resolutions: Vec<String>,
    orientation_selected: Option<usize>,
    refresh_rate_selected: Option<usize>,
    resolution_selected: Option<usize>,
    scale_selected: Option<usize>,
}

impl page::AutoBind<crate::pages::Message> for Page {}

impl page::Page<crate::pages::Message> for Page {
    fn content(
        &self,
        sections: &mut SlotMap<section::Entity, Section<crate::pages::Message>>,
    ) -> Option<page::Content> {
        Some(vec![
            // Night light
            //            sections.insert(
            //                Section::default()
            //                    .descriptions(vec![
            //                        text::NIGHT_LIGHT.as_str().into(),
            //                        text::NIGHT_LIGHT_AUTO.as_str().into(),
            //                        text::NIGHT_LIGHT_DESCRIPTION.as_str().into(),
            //                    ])
            //                    .view::<Page>(move |_binder, page, _section| page.night_light_view()),
            //            ),
            // Display arrangement
            sections.insert(display_arrangement()),
            // Display configuration
            sections.insert(display_configuration()),
        ])
    }

    fn info(&self) -> page::Info {
        page::Info::new("display", "preferences-desktop-display-symbolic")
            .title(fl!("display"))
            .description(fl!("display", "desc"))
    }

    #[cfg(not(feature = "test"))]
    fn on_enter(
        &mut self,
        _page: page::Entity,
        sender: tokio::sync::mpsc::Sender<crate::pages::Message>,
    ) -> Command<crate::pages::Message> {
        if let Some(task) = self.background_service.take() {
            task.abort();
        }

        // Spawns a background service to monitor for display state changes.
        // This must be spawned onto its own thread because `*mut wayland_sys::client::wl_display` is not Send-able.
        let runtime = tokio::runtime::Handle::current();
        self.background_service = Some(tokio::task::spawn_blocking(move || {
            runtime.block_on(async move {
                let (tx, mut rx) = tachyonix::channel(5);
                let Ok((mut context, mut event_queue)) = cosmic_randr::connect(tx) else {
                    return;
                };

                while context.dispatch(&mut event_queue).await.is_ok() {
                    while let Ok(message) = rx.try_recv() {
                        if let cosmic_randr::Message::ManagerDone = message {
                            let _ = sender
                                .send(pages::Message::Displays(Message::Refresh))
                                .await;
                        }
                    }
                }
            });
        }));

        command::future(on_enter())
    }

    fn on_leave(&mut self) -> Command<crate::pages::Message> {
        if let Some(task) = self.background_service.take() {
            task.abort();
        }

        Command::none()
    }

    #[cfg(feature = "test")]
    fn on_enter(
        &mut self,
        _page: page::Entity,
        sender: tokio::sync::mpsc::Sender<crate::pages::Message>,
    ) -> Command<crate::pages::Message> {
        command::future(async move {
            let mut randr = List::default();

            let test_mode = randr.modes.insert(cosmic_randr_shell::Mode {
                size: (1920, 1080),
                refresh_rate: 144_000,
                preferred: true,
            });

            randr.outputs.insert(cosmic_randr_shell::Output {
                name: "Dummy-1".into(),
                enabled: true,
                make: None,
                model: "Test 1".into(),
                mirroring: None,
                physical: (1, 1),
                position: (0, 0),
                scale: 1.0,
                transform: Some(Transform::Normal),
                modes: vec![test_mode],
                current: Some(test_mode),
            });

            randr.outputs.insert(cosmic_randr_shell::Output {
                name: "Dummy-2".into(),
                enabled: true,
                make: None,
                model: "Test 1".into(),
                mirroring: None,
                physical: (1, 1),
                position: (1920, 0),
                scale: 1.0,
                transform: Some(Transform::Normal),
                modes: vec![test_mode],
                current: Some(test_mode),
            });

            crate::pages::Message::Displays(Message::Update {
                randr: Arc::new(Ok(randr)),
            })
        })
    }

    //    fn context_drawer(&self) -> Option<Element<pages::Message>> {
    //        Some(match self.context {

    //            Some(ContextDrawer::NightLight) => self.night_light_context_view(),

    //            None => return None,
    //        })
    //    }

    /// Opens a dialog to confirm the display settings.
    ///
    /// This dialog has a 10 (arbitrary) second counter which will 
    /// automatically revert to the original display settings when depleted.
    /// 
    /// To make a setting activate this dialog. Call the set_dialog  macto with 
    /// the Randr enum value which undos the current change.
    fn dialog(&self) -> Option<Element<pages::Message>> {
        // An arbitrarily chosen amound of time (in seconds) in which the user
        // has to confirm the new display settings before reverting.
        const DIALOG_CANCEL_TIME: time::Duration = time::Duration::from_secs(11);
        let Some((revert_request, start)) = self.dialog else {
            return None;
        };
        let dialog = widget::dialog(fl!("dialog", "title"))
            .body(fl!(
                "dialog", 
                "change-prompt", 
                // Countdown
                time = (DIALOG_CANCEL_TIME - start.elapsed()).as_secs()
            ))
            .primary_action(
                widget::button::suggested(fl!("dialog", "keep-changes"))
                    .on_press(pages::Message::Displays(Message::DialogComplete))
            )
            .secondary_action(
                widget::button::standard(fl!("dialog", "revert-settings"))
                    .on_press(pages::Message::Displays(Message::DialogCancel(revert_request)))
            );
        Some(dialog.into())
    }
}

impl Page {
    pub fn update(&mut self, message: Message) -> Command<app::Message> {
        match message {
            Message::RandrResult(result) => {
                if let Some(Err(why)) = Arc::into_inner(result) {
                    tracing::error!(?why, "cosmic-randr error");
                } else {
                    // Reload display info
                    return cosmic::command::future(async move {
                        crate::Message::PageMessage(on_enter().await)
                    });
                }
            },

            Message::DialogCancel(request) => {
                let Some(output) = self.list.outputs.get(self.active_display) else {
                    return Command::none();
                };
                self.dialog = None;
                return self.exec_randr(output, request);
            },

            Message::DialogComplete => {
                self.dialog = None;
            },

            Message::Display(display) => self.set_display(display),

            Message::ColorDepth(color_depth) => return self.set_color_depth(color_depth),

            Message::ColorProfile(profile) => return self.set_color_profile(profile),

            Message::DisplayToggle(enable) => return self.toggle_display(enable),

            Message::Mirroring(mirroring) => match mirroring {
                Mirroring::Disable => (),
                Mirroring::Mirror(target_display) => (),
                Mirroring::Project(target_display) => (),
                Mirroring::ProjectToAll => (),
            },

            //            Message::NightLight(night_light) => {}
            //
            //            Message::NightLightContext => {
            //                self.context = Some(ContextDrawer::NightLight);
            //                return cosmic::command::message(app::Message::OpenContextDrawer(
            //                    text::NIGHT_LIGHT.clone().into(),
            //                ));
            //            }
            Message::Orientation(orientation) => return self.set_orientation(orientation),

            Message::Position(display, x, y) => return self.set_position(display, x, y),

            Message::Refresh => {
                return cosmic::command::future(async move {
                    crate::Message::PageMessage(on_enter().await)
                });
            }

            Message::RefreshRate(rate) => return self.set_refresh_rate(rate),

            Message::Resolution(option) => return self.set_resolution(option),

            Message::Scale(scale) => return self.set_scale(scale),

            Message::Update { randr } => {
                match Arc::into_inner(randr) {
                    Some(Ok(outputs)) => self.update_displays(outputs),

                    Some(Err(why)) => {
                        tracing::error!(?why, "error fetching displays");
                    }

                    None => (),
                }

                self.cache.orientations = [
                    fl!("orientation", "standard"),
                    fl!("orientation", "rotate-90"),
                    fl!("orientation", "rotate-180"),
                    fl!("orientation", "rotate-270"),
                ];
            }
        }

        cosmic::iced::widget::scrollable::snap_to(
            self.display_arrangement_scrollable.clone(),
            RelativeOffset { x: 0.5, y: 0.5 },
        )
    }

    /// Displays the night light context drawer.
    //    pub fn night_light_context_view(&self) -> Element<pages::Message> {
    //        column().into()
    //    }

    /// Reloads the display list, and all information relevant to the active display.
    pub fn update_displays(&mut self, list: List) {
        let active_display_name = self
            .display_tabs
            .text_remove(self.display_tabs.active())
            .unwrap_or_default();
        let mut active_tab_pos: u16 = 0;

        self.active_display = OutputKey::null();
        self.display_tabs.clear();
        self.list = list;

        let sorted_outputs = self
            .list
            .outputs
            .iter()
            .map(|(key, output)| (&*output.name, key))
            .collect::<BTreeMap<_, _>>();

        for (pos, (_name, id)) in sorted_outputs.into_iter().enumerate() {
            let Some(output) = self.list.outputs.get(id) else {
                continue;
            };

            let text = crate::utils::display_name(&output.name, output.physical);

            if text == active_display_name {
                active_tab_pos = pos as u16;
            }

            self.display_tabs.insert().text(text).data::<OutputKey>(id);
        }

        self.display_tabs.activate_position(active_tab_pos);

        // Retrieve data for the first, activated display.
        self.set_display(self.display_tabs.active());
    }

    /// Changes the color depth of the active display.
    pub fn set_color_depth(&mut self, depth: ColorDepth) -> Command<app::Message> {
        unimplemented!()
    }

    /// Changes the color profile of the active display.
    pub fn set_color_profile(&mut self, profile: usize) -> Command<app::Message> {
        unimplemented!()
    }

    /// Changes the active display, and regenerates available options for it.
    pub fn set_display(&mut self, display: segmented_button::Entity) {
        let Some(&output_id) = self.display_tabs.data::<OutputKey>(display) else {
            return;
        };

        let Some(output) = self.list.outputs.get_mut(output_id) else {
            return;
        };

        self.display_tabs.activate(display);
        self.active_display = output_id;
        self.config.refresh_rate = None;
        self.config.resolution = None;
        self.config.scale = (output.scale * 100.0) as u32;

        self.cache.modes.clear();
        self.cache.refresh_rates.clear();
        self.cache.resolutions.clear();
        self.cache.orientation_selected = match output.transform {
            Some(Transform::Normal) => Some(0),
            Some(Transform::Rotate90) => Some(1),
            Some(Transform::Rotate180) => Some(2),
            Some(Transform::Rotate270) => Some(3),
            _ => None,
        };
        self.cache.resolution_selected = None;
        self.cache.refresh_rate_selected = None;

        self.cache.scale_selected = Some(if self.config.scale < 75 {
            0
        } else if self.config.scale < 100 {
            1
        } else if self.config.scale < 125 {
            2
        } else if self.config.scale < 150 {
            3
        } else if self.config.scale < 175 {
            4
        } else if self.config.scale < 200 {
            5
        } else {
            6
        });

        if let Some(current_mode_id) = output.current {
            for (mode_id, mode) in output
                .modes
                .iter()
                .filter_map(|&id| self.list.modes.get(id).map(|m| (id, m)))
            {
                let refresh_rates = self.cache.modes.entry(mode.size).or_default();

                refresh_rates.push(mode.refresh_rate);

                if current_mode_id == mode_id {
                    self.cache.refresh_rate_selected = Some(refresh_rates.len() - 1);
                    self.cache.resolution_selected = Some(self.cache.modes.len() - 1);
                    self.config.resolution = Some(mode.size);
                    self.config.refresh_rate = Some(mode.refresh_rate);
                }
            }
        }

        for (&resolution, rates) in self.cache.modes.iter().rev() {
            self.cache
                .resolutions
                .push(format!("{}x{}", resolution.0, resolution.1));
            if Some(resolution) == self.config.resolution {
                cache_rates(&mut self.cache.refresh_rates, rates);
            }
        }
    }

    /// Change display orientation.
    pub fn set_orientation(&mut self, transform: Transform) -> Command<app::Message> {
        let Some(output) = self.list.outputs.get(self.active_display) else {
            return Command::none();
        };

        if let Some(orientation) = self.cache.orientation_selected {
            let current_orientation = match orientation {
                1 => Transform::Rotate90,
                2 => Transform::Flipped180,
                3 => Transform::Flipped270,
                _ => Transform::Normal,
            };
            self.dialog = Some((Randr::Transform(current_orientation), time::Instant::now()));
        }

        self.cache.orientation_selected = match transform {
            Transform::Normal => Some(0),
            Transform::Rotate90 => Some(1),
            Transform::Rotate180 => Some(2),
            _ => Some(3),
        };

        self.exec_randr(output, Randr::Transform(transform))
    }

    /// Changes the position of the display.
    pub fn set_position(&mut self, display: OutputKey, x: i32, y: i32) -> Command<app::Message> {
        let Some(output) = self.list.outputs.get_mut(display) else {
            return Command::none();
        };

        // TODO: Find current display position.
        // Attempt?
        let (current_x, current_y) = output.position;
        self.dialog = Some((Randr::Position(current_x, current_y), time::Instant::now())); 

        output.position = (x, y);

        if cfg!(feature = "test") {
            tracing::debug!("set position {x},{y}");
            return Command::none();
        }
        

        let output = &self.list.outputs[display];
        self.exec_randr(output, Randr::Position(x, y))
    }

    /// Changes the refresh rate of the active display.
    pub fn set_refresh_rate(&mut self, option: usize) -> Command<app::Message> {
        let Some(output) = self.list.outputs.get(self.active_display) else {
            return Command::none();
        };

        if let Some(ref resolution) = self.config.resolution {
            if let Some(rates) = self.cache.modes.get(resolution) {
                if let Some(&rate) = rates.get(option) {
                    if let Some(current_rate) = self.config.refresh_rate {
                        self.dialog = Some((Randr::RefreshRate(current_rate), time::Instant::now()));
                    }
                    self.cache.refresh_rate_selected = Some(option);
                    self.config.refresh_rate = Some(rate);
                    return self.exec_randr(output, Randr::RefreshRate(rate));
                }
            }
        }

        Command::none()
    }

    /// Change the resolution of the active display.
    pub fn set_resolution(&mut self, option: usize) -> Command<app::Message> {
        let Some(output) = self.list.outputs.get(self.active_display) else {
            return Command::none();
        };

        let Some((&resolution, rates)) = self.cache.modes.iter().rev().nth(option) else {
            return Command::none();
        };

        self.cache.refresh_rates.clear();
        cache_rates(&mut self.cache.refresh_rates, rates);

        let Some(&rate) = rates.first() else {
            return Command::none();
        };

        if let Some((current_width, current_height)) = self.config.resolution {
            self.dialog = Some((Randr::Resolution(current_width, current_height), time::Instant::now()));
        };

        self.config.refresh_rate = Some(rate);
        self.config.resolution = Some(resolution);
        self.cache.refresh_rate_selected = Some(0);
        self.cache.resolution_selected = Some(option);
        self.exec_randr(output, Randr::Resolution(resolution.0, resolution.1))
    }

    /// Set the scale of the active display.
    pub fn set_scale(&mut self, option: usize) -> Command<app::Message> {
        let Some(output) = self.list.outputs.get(self.active_display) else {
            return Command::none();
        };

        let scale = (option * 25 + 50) as u32;

        self.dialog = Some((Randr::Scale(self.config.scale as u32), time::Instant::now()));

        self.cache.scale_selected = Some(option);
        self.config.scale = scale;
        self.exec_randr(output, Randr::Scale(scale))
    }

    /// Enables or disables the active display.
    pub fn toggle_display(&mut self, enable: bool) -> Command<app::Message> {
        let Some(output) = self.list.outputs.get_mut(self.active_display) else {
            return Command::none();
        };

        output.enabled = enable;

        let output = &self.list.outputs[self.active_display];
        self.dialog = Some((Randr::Toggle(!output.enabled), time::Instant::now()));
        self.exec_randr(output, Randr::Toggle(output.enabled))
    }

    /// Applies a display configuration via `cosmic-randr`.
    fn exec_randr(&self, output: &Output, request: Randr) -> Command<app::Message> {
        let name = &*output.name;
        let mut command = tokio::process::Command::new("cosmic-randr");

        match request {
            Randr::Position(x, y) => {
                let Some(current) = output.current.and_then(|id| self.list.modes.get(id)) else {
                    return Command::none();
                };

                command
                    .arg("mode")
                    .arg("--pos-x")
                    .arg(itoa::Buffer::new().format(x))
                    .arg("--pos-y")
                    .arg(itoa::Buffer::new().format(y))
                    .arg(name)
                    .arg(itoa::Buffer::new().format(current.size.0))
                    .arg(itoa::Buffer::new().format(current.size.1));
            }

            Randr::RefreshRate(rate) => {
                let Some(current) = output.current.and_then(|id| self.list.modes.get(id)) else {
                    return Command::none();
                };

                command
                    .arg("mode")
                    .arg("--refresh")
                    .arg(
                        &[
                            itoa::Buffer::new().format(rate / 1000),
                            ".",
                            itoa::Buffer::new().format(rate % 1000),
                        ]
                        .concat(),
                    )
                    .arg(name)
                    .arg(itoa::Buffer::new().format(current.size.0))
                    .arg(itoa::Buffer::new().format(current.size.1));
            }

            Randr::Resolution(width, height) => {
                command
                    .arg("mode")
                    .arg(name)
                    .arg(itoa::Buffer::new().format(width))
                    .arg(itoa::Buffer::new().format(height));
            }

            Randr::Scale(scale) => {
                let Some(current) = output.current.and_then(|id| self.list.modes.get(id)) else {
                    return Command::none();
                };

                command
                    .arg("mode")
                    .arg("--scale")
                    .arg(
                        &[
                            itoa::Buffer::new().format(scale / 100),
                            ".",
                            itoa::Buffer::new().format(scale % 100),
                        ]
                        .concat(),
                    )
                    .arg(name)
                    .arg(itoa::Buffer::new().format(current.size.0))
                    .arg(itoa::Buffer::new().format(current.size.1));
            }

            Randr::Toggle(enable) => {
                command
                    .arg(if enable { "enable" } else { "disable" })
                    .arg(name);
            }

            Randr::Transform(transform) => {
                let Some(current) = output.current.and_then(|id| self.list.modes.get(id)) else {
                    return Command::none();
                };

                command
                    .arg("mode")
                    .arg("--transform")
                    .arg(&*format!("{transform}"))
                    .arg(name)
                    .arg(itoa::Buffer::new().format(current.size.0))
                    .arg(itoa::Buffer::new().format(current.size.1));
            }
        }

        cosmic::command::future(async move {
            tracing::debug!(?command, "executing");
            app::Message::from(Message::RandrResult(Arc::new(command.status().await)))
        })
    }

}

/// View for the display arrangement section.
pub fn display_arrangement() -> Section<crate::pages::Message> {
    let mut descriptions = Slab::new();

    _ = descriptions.insert(fl!("display", "arrangement"));
    let display_arrangement_desc = descriptions.insert(fl!("display", "arrangement-desc"));

    Section::default()
        .title(fl!("display", "arrangement"))
        .descriptions(descriptions)
        // Show section when there is more than 1 display
        .show_while::<Page>(|page| page.list.outputs.len() > 1)
        .view::<Page>(move |_binder, page, section| {
            let descriptions = &section.descriptions;
            let theme = cosmic::theme::active();

            column()
                .padding(cosmic::iced::Padding::from([
                    theme.cosmic().space_s(),
                    theme.cosmic().space_m(),
                ]))
                .spacing(theme.cosmic().space_xs())
                .push(cosmic::widget::text::body(
                    &descriptions[display_arrangement_desc],
                ))
                .push({
                    Arrangement::new(&page.list, &page.display_tabs)
                        .on_select(|id| pages::Message::Displays(Message::Display(id)))
                        .on_placement(|id, x, y| {
                            pages::Message::Displays(Message::Position(id, x, y))
                        })
                        .apply(cosmic::widget::scrollable)
                        .id(page.display_arrangement_scrollable.clone())
                        .width(Length::Shrink)
                        .direction(Direction::Horizontal(Properties::new()))
                        .apply(container)
                        .center_x()
                        .width(Length::Fill)
                })
                .apply(cosmic::widget::list::container)
                .into()
        })
}

/// View for the display configuration section.
pub fn display_configuration() -> Section<crate::pages::Message> {
    let mut descriptions = Slab::new();

    let _display = descriptions.insert(fl!("display"));
    let refresh_rate = descriptions.insert(fl!("display", "refresh-rate"));
    let resolution = descriptions.insert(fl!("display", "resolution"));
    let scale = descriptions.insert(fl!("display", "scale"));
    let orientation = descriptions.insert(fl!("orientation"));
    let enable_label = descriptions.insert(fl!("display", "enable"));
    let options_label = descriptions.insert(fl!("display", "options"));

    Section::default()
        .descriptions(descriptions)
        .view::<Page>(move |_binder, page, section| {
            let descriptions = &section.descriptions;
            let theme = cosmic::theme::active();

            let Some(&active_id) = page.display_tabs.active_data::<OutputKey>() else {
                return column().into();
            };

            let active_output = &page.list.outputs[active_id];

            let display_options = active_output.enabled.then(|| {
                list_column()
                    .add(cosmic::widget::settings::flex_item(
                        &descriptions[resolution],
                        dropdown(
                            &page.cache.resolutions,
                            page.cache.resolution_selected,
                            Message::Resolution,
                        ),
                    ))
                    .add(cosmic::widget::settings::flex_item(
                        &descriptions[refresh_rate],
                        dropdown(
                            &page.cache.refresh_rates,
                            page.cache.refresh_rate_selected,
                            Message::RefreshRate,
                        ),
                    ))
                    .add(cosmic::widget::settings::flex_item(
                        &descriptions[scale],
                        dropdown(
                            &["50%", "75%", "100%", "125%", "150%", "175%", "200%"],
                            page.cache.scale_selected,
                            Message::Scale,
                        ),
                    ))
                    .add(cosmic::widget::settings::flex_item(
                        &descriptions[orientation],
                        dropdown(
                            &page.cache.orientations,
                            page.cache.orientation_selected,
                            |id| {
                                Message::Orientation(match id {
                                    0 => Transform::Normal,
                                    1 => Transform::Rotate90,
                                    2 => Transform::Rotate180,
                                    _ => Transform::Rotate270,
                                })
                            },
                        ),
                    ))
            });

            let mut content = column().spacing(theme.cosmic().space_m());

            if page.list.outputs.len() > 1 {
                let display_switcher = tab_bar::horizontal(&page.display_tabs)
                    .button_alignment(Alignment::Center)
                    .on_activate(Message::Display);

                let display_enable = (page
                    // Don't allow disabling display if it's the only active
                    .list
                    .outputs
                    .values()
                    .filter(|display| display.enabled)
                    .count()
                    > 1
                    || !active_output.enabled)
                    .then(|| {
                        list_column().add(cosmic::widget::settings::flex_item(
                            &descriptions[enable_label],
                            toggler(None, active_output.enabled, Message::DisplayToggle),
                        ))
                    });

                content = content.push(display_switcher).push_maybe(display_enable);
            }

            content
                .push(cosmic::widget::text::heading(&descriptions[options_label]))
                .push_maybe(display_options)
                .apply(Element::from)
                .map(pages::Message::Displays)
        })
}

fn cache_rates(cached_rates: &mut Vec<String>, rates: &[u32]) {
    *cached_rates = rates
        .iter()
        .map(|&rate| format!("{:>3}.{:02} Hz", rate / 1000, rate % 1000))
        .collect();
}

pub async fn on_enter() -> crate::pages::Message {
    let randr_fut = cosmic_randr_shell::list();
    let randr = futures::future::ready(randr_fut).await;

    crate::pages::Message::Displays(Message::Update {
        randr: Arc::new(randr.await),
    })
}
