//! The tray icon (StatusNotifierItem).

use ksni::blocking::TrayMethods;
use ksni::menu::*;

use crate::hub::Hub;
use crate::icon;
use crate::ipc::UiMsg;

pub struct Tray {
    pub ui: async_channel::Sender<UiMsg>,
    pub hub: Hub,
    pub profiles: Vec<String>,
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "penumbra".into()
    }

    fn title(&self) -> String {
        "Penumbra".into()
    }

    fn category(&self) -> ksni::Category {
        ksni::Category::Hardware
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        [16u32, 22, 24, 32, 48, 64].iter().map(|s| ksni::Icon { width: *s as i32, height: *s as i32, data: icon::argb(*s, (235, 238, 250)) }).collect()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip { title: "Penumbra".into(), description: "Click for display controls, scroll for brightness".into(), icon_name: String::new(), icon_pixmap: Vec::new() }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.ui.try_send(UiMsg::Toggle);
    }

    fn secondary_activate(&mut self, _x: i32, _y: i32) {
        let _ = self.ui.try_send(UiMsg::Settings("displays".into()));
    }

    fn scroll(&mut self, delta: i32, orientation: ksni::Orientation) {
        if !matches!(orientation, ksni::Orientation::Vertical) || delta == 0 {
            return;
        }
        let step = if delta > 0 { "+" } else { "-" };
        let r = crate::commands::execute(&self.hub, crate::commands::Command::Brightness { display: "focused".into(), value: step.into() });
        if r.ok {
            let _ = self.ui.try_send(UiMsg::Osd { display: "focused".into() });
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let mut items: Vec<MenuItem<Self>> = vec![
            StandardItem {
                label: "Show displays".into(),
                activate: Box::new(|t: &mut Self| {
                    let _ = t.ui.try_send(UiMsg::Show);
                }),
                ..Default::default()
            }
            .into(),
        ];
        if !self.profiles.is_empty() {
            let sub: Vec<MenuItem<Self>> = self
                .profiles
                .iter()
                .map(|p| {
                    let name = p.clone();
                    StandardItem {
                        label: p.clone(),
                        activate: Box::new(move |t: &mut Self| {
                            let _ = t.hub.apply_profile(&name);
                        }),
                        ..Default::default()
                    }
                    .into()
                })
                .collect();
            items.push(SubMenu { label: "Profiles".into(), submenu: sub, ..Default::default() }.into());
        }
        items.push(MenuItem::Separator);
        items.push(
            StandardItem {
                label: "Settings…".into(),
                activate: Box::new(|t: &mut Self| {
                    let _ = t.ui.try_send(UiMsg::Settings("general".into()));
                }),
                ..Default::default()
            }
            .into(),
        );
        items.push(
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(|t: &mut Self| {
                    let _ = t.ui.try_send(UiMsg::Quit);
                }),
                ..Default::default()
            }
            .into(),
        );
        items
    }
}

pub fn spawn(tray: Tray) -> Option<ksni::blocking::Handle<Tray>> {
    match tray.assume_sni_available(true).spawn() {
        Ok(h) => Some(h),
        Err(e) => {
            log::error!("tray icon: {e}");
            None
        }
    }
}
