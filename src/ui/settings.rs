//! The settings window: one page per display, the arrangement of screens, virtual displays,
//! profiles and general options.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;

use super::arrange::Arrange;
use super::controls::DisplayControls;
use super::widgets::*;
use crate::hub::{Control, DdcStatus, DisplayView, Hub};
use crate::store::{Anchor, VirtualDisplay};

const PAGES: [(&str, &str, &str); 6] = [
    ("displays", "Displays", "video-display-symbolic"),
    ("arrangement", "Arrangement", "view-grid-symbolic"),
    ("virtual", "Virtual displays", "video-joined-displays-symbolic"),
    ("profiles", "Profiles", "view-list-symbolic"),
    ("general", "General", "emblem-system-symbolic"),
    ("about", "About", "help-about-symbolic"),
];

struct DisplayPage {
    root: gtk::Box,
    title: gtk::Label,
    nick: adw::EntryRow,
    brightness: SliderRow,
    controls: DisplayControls,
    shown: adw::SwitchRow,
    follow: adw::SwitchRow,
    off_row: adw::ActionRow,
    off_btn: gtk::Button,
    rows: HashMap<&'static str, adw::ActionRow>,
}

struct Ui {
    window: adw::ApplicationWindow,
    hub: Hub,
    stack: gtk::Stack,
    list: gtk::ListBox,
    content: adw::NavigationPage,
    toasts: adw::ToastOverlay,
    selector: gtk::Box,
    display_stack: gtk::Stack,
    pages: RefCell<HashMap<String, DisplayPage>>,
    selected: RefCell<Option<String>>,
    chips: RefCell<HashMap<String, gtk::ToggleButton>>,
    arrange: Rc<Arrange>,
    virtual_group: adw::PreferencesGroup,
    virtual_rows: RefCell<Vec<adw::ActionRow>>,
    virtual_sig: RefCell<String>,
    profile_group: adw::PreferencesGroup,
    profile_rows: RefCell<Vec<adw::ActionRow>>,
    profile_sig: RefCell<String>,
}

pub struct Settings {
    ui: Rc<Ui>,
}

impl Settings {
    pub fn new(app: &gtk::Application, hub: &Hub) -> Rc<Settings> {
        let window = adw::ApplicationWindow::builder().application(app).title("Penumbra").default_width(1000).default_height(720).build();
        window.set_hide_on_close(true);
        let toasts = adw::ToastOverlay::new();
        let split = adw::NavigationSplitView::new();
        split.set_min_sidebar_width(210.0);
        split.set_max_sidebar_width(250.0);

        // sidebar
        let list = gtk::ListBox::new();
        list.add_css_class("navigation-sidebar");
        list.set_selection_mode(gtk::SelectionMode::Single);
        for (id, title, icon) in PAGES {
            let row = gtk::ListBoxRow::new();
            row.set_widget_name(id);
            let b = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            b.set_margin_top(8);
            b.set_margin_bottom(8);
            b.set_margin_start(6);
            b.append(&gtk::Image::from_icon_name(icon));
            b.append(&gtk::Label::new(Some(title)));
            row.set_child(Some(&b));
            list.append(&row);
        }
        let side_view = adw::ToolbarView::new();
        let side_head = adw::HeaderBar::new();
        side_head.set_title_widget(Some(&gtk::Label::new(Some("Penumbra"))));
        side_view.add_top_bar(&side_head);
        let side_scroll = gtk::ScrolledWindow::new();
        side_scroll.set_child(Some(&list));
        side_view.set_content(Some(&side_scroll));
        let side_page = adw::NavigationPage::new(&side_view, "Penumbra");
        split.set_sidebar(Some(&side_page));

        // content
        let stack = gtk::Stack::new();
        stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        let content_view = adw::ToolbarView::new();
        content_view.add_top_bar(&adw::HeaderBar::new());
        content_view.set_content(Some(&stack));
        let content = adw::NavigationPage::new(&content_view, "Displays");
        split.set_content(Some(&content));
        toasts.set_child(Some(&split));
        window.set_content(Some(&toasts));

        let selector = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        selector.add_css_class("linked");
        let display_stack = gtk::Stack::new();
        let arrange = Arrange::new(hub);
        let virtual_group = adw::PreferencesGroup::new();
        let profile_group = adw::PreferencesGroup::new();

        let ui = Rc::new(Ui {
            window,
            hub: hub.clone(),
            stack,
            list,
            content,
            toasts,
            selector,
            display_stack,
            pages: RefCell::new(HashMap::new()),
            selected: RefCell::new(None),
            chips: RefCell::new(HashMap::new()),
            arrange,
            virtual_group,
            virtual_rows: RefCell::new(Vec::new()),
            virtual_sig: RefCell::new(String::new()),
            profile_group,
            profile_rows: RefCell::new(Vec::new()),
            profile_sig: RefCell::new(String::new()),
        });
        ui.build_pages();
        {
            let u = ui.clone();
            ui.list.connect_row_selected(move |_, row| {
                if let Some(r) = row {
                    u.go(&r.widget_name());
                }
            });
        }
        {
            let u = ui.clone();
            ui.arrange.connect_toast(move |m| u.toast(m));
        }
        Rc::new(Settings { ui })
    }

    pub fn show(&self, page: &str) {
        let id = PAGES.iter().find(|(id, _, _)| *id == page).map(|(id, _, _)| *id).unwrap_or("displays");
        self.ui.sync();
        self.ui.select(id);
        self.ui.window.present();
    }

    pub fn sync(&self) {
        if self.ui.window.is_visible() {
            self.ui.sync();
        }
    }

    pub fn toast(&self, msg: &str, _error: bool) {
        if self.ui.window.is_visible() {
            self.ui.toast(msg);
        }
    }
}

fn clamp_page(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    let clamp = adw::Clamp::new();
    clamp.set_maximum_size(760);
    clamp.set_margin_top(18);
    clamp.set_margin_bottom(24);
    clamp.set_margin_start(16);
    clamp.set_margin_end(16);
    clamp.set_child(Some(child));
    let sc = gtk::ScrolledWindow::new();
    sc.set_child(Some(&clamp));
    sc.set_hscrollbar_policy(gtk::PolicyType::Never);
    sc
}

impl Ui {
    fn toast(&self, msg: &str) {
        self.toasts.add_toast(adw::Toast::new(msg));
    }

    fn select(&self, id: &str) {
        let mut i = 0;
        while let Some(row) = self.list.row_at_index(i) {
            if row.widget_name() == id {
                self.list.select_row(Some(&row));
                break;
            }
            i += 1;
        }
        self.go(id);
    }

    fn go(&self, id: &str) {
        self.stack.set_visible_child_name(id);
        if let Some((_, title, _)) = PAGES.iter().find(|(p, _, _)| *p == id) {
            self.content.set_title(title);
        }
    }

    fn build_pages(self: &Rc<Self>) {
        // displays
        let dv = gtk::Box::new(gtk::Orientation::Vertical, 14);
        let sel_scroll = gtk::ScrolledWindow::new();
        sel_scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Never);
        sel_scroll.set_child(Some(&self.selector));
        dv.append(&sel_scroll);
        dv.append(&self.display_stack);
        self.stack.add_named(&clamp_page(&dv), Some("displays"));

        // arrangement
        let av = gtk::Box::new(gtk::Orientation::Vertical, 14);
        let g = adw::PreferencesGroup::new();
        g.set_title("Arrangement");
        g.set_description(Some("Place the screens the way they sit on your desk. The pointer crosses from one to the next along the touching edges."));
        av.append(&g);
        av.append(&self.arrange.root);
        let ident = gtk::Button::with_label("Identify screens");
        ident.add_css_class("pill");
        ident.set_halign(gtk::Align::Start);
        {
            let hub = self.hub.clone();
            ident.connect_clicked(move |_| {
                for v in hub.views().iter().filter(|v| v.output.enabled) {
                    super::osd::identify(&v.name);
                }
            });
        }
        av.append(&ident);
        self.stack.add_named(&clamp_page(&av), Some("arrangement"));

        // virtual
        self.stack.add_named(&clamp_page(&self.build_virtual()), Some("virtual"));
        // profiles
        self.stack.add_named(&clamp_page(&self.build_profiles()), Some("profiles"));
        // general
        self.stack.add_named(&clamp_page(&self.build_general()), Some("general"));
        // about
        self.stack.add_named(&clamp_page(&self.build_about()), Some("about"));
    }

    // -------------------------------------------------------------- virtual displays

    fn build_virtual(self: &Rc<Self>) -> gtk::Box {
        let b = gtk::Box::new(gtk::Orientation::Vertical, 18);
        self.virtual_group.set_title("Virtual displays");
        self.virtual_group.set_description(Some("Extra screens without a monitor: for screen sharing, a VNC or remote desktop session, or more room. They come back at every start of Penumbra."));
        b.append(&self.virtual_group);

        let new = adw::PreferencesGroup::new();
        new.set_title("Add a virtual display");
        let name = adw::EntryRow::new();
        name.set_title("Name");
        name.set_text("Virtual-1");
        let presets: [(&str, i32, i32); 7] = [
            ("1280 × 720", 1280, 720),
            ("1920 × 1080 (Full HD)", 1920, 1080),
            ("2560 × 1440 (QHD)", 2560, 1440),
            ("3840 × 2160 (4K)", 3840, 2160),
            ("2560 × 1600", 2560, 1600),
            ("1920 × 1200", 1920, 1200),
            ("1024 × 768", 1024, 768),
        ];
        let res = adw::ComboRow::new();
        res.set_title("Resolution");
        res.set_model(Some(&gtk::StringList::new(&presets.iter().map(|p| p.0).collect::<Vec<_>>())));
        res.set_selected(1);
        let hz = adw::ComboRow::new();
        hz.set_title("Refresh rate");
        let rates = [30u32, 60, 90, 120, 144];
        hz.set_model(Some(&gtk::StringList::new(&rates.iter().map(|r| format!("{r} Hz")).collect::<Vec<_>>().iter().map(|s| s.as_str()).collect::<Vec<_>>())));
        hz.set_selected(1);
        let create = adw::ButtonRow::new();
        create.set_title("Create");
        create.add_css_class("suggested-action");
        {
            let (u, name, res, hz) = (self.clone(), name.clone(), res.clone(), hz.clone());
            create.connect_activated(move |_| {
                let p = presets[res.selected() as usize];
                let vd = VirtualDisplay { name: name.text().trim().to_string(), width: p.1, height: p.2, hz: rates[hz.selected() as usize] };
                match u.hub.create_virtual(vd.clone()) {
                    Ok(()) => {
                        u.toast(&format!("Created {}", vd.name));
                        // the next name
                        let n = vd.name.trim_end_matches(|c: char| c.is_ascii_digit()).to_string();
                        let num: u32 = vd.name[n.len()..].parse().unwrap_or(0) + 1;
                        name.set_text(&format!("{n}{num}"));
                    }
                    Err(e) => u.toast(&e),
                }
            });
        }
        new.add(&name);
        new.add(&res);
        new.add(&hz);
        new.add(&create);
        b.append(&new);
        b
    }

    fn sync_virtual(self: &Rc<Self>, views: &[DisplayView]) {
        let settings = self.hub.settings();
        let mut entries: Vec<(String, String)> = Vec::new();
        for v in views.iter().filter(|v| v.output.is_virtual()) {
            let m = v.output.mode.as_ref().map(|m| m.label()).unwrap_or_default();
            entries.push((v.name.clone(), m));
        }
        for vd in &settings.virtual_displays {
            if !entries.iter().any(|(n, _)| *n == vd.name) {
                entries.push((vd.name.clone(), format!("{}×{} @ {} Hz (not created)", vd.width, vd.height, vd.hz)));
            }
        }
        let sig = format!("{entries:?}");
        if *self.virtual_sig.borrow() == sig {
            return;
        }
        *self.virtual_sig.borrow_mut() = sig;
        for r in self.virtual_rows.borrow_mut().drain(..) {
            self.virtual_group.remove(&r);
        }
        if entries.is_empty() {
            let r = adw::ActionRow::new();
            r.set_title("No virtual displays yet");
            self.virtual_group.add(&r);
            self.virtual_rows.borrow_mut().push(r);
        }
        for (name, sub) in entries {
            let r = adw::ActionRow::new();
            r.set_title(&name);
            r.set_subtitle(&sub);
            let del = icon_button("user-trash-symbolic", "Remove");
            let (u, n) = (self.clone(), name.clone());
            del.connect_clicked(move |_| {
                if let Err(e) = u.hub.destroy_virtual(&n) {
                    u.toast(&e);
                }
            });
            r.add_suffix(&del);
            self.virtual_group.add(&r);
            self.virtual_rows.borrow_mut().push(r);
        }
    }

    // -------------------------------------------------------------- profiles

    fn build_profiles(self: &Rc<Self>) -> gtk::Box {
        let b = gtk::Box::new(gtk::Orientation::Vertical, 18);
        self.profile_group.set_title("Profiles");
        self.profile_group.set_description(Some("A profile remembers, for every display, its resolution, scale, position, rotation, brightness, colour and monitor settings (input, contrast, volume...)."));
        b.append(&self.profile_group);
        let new = adw::PreferencesGroup::new();
        new.set_title("Save the current setup");
        let name = adw::EntryRow::new();
        name.set_title("Name (Desk, Presentation, Night...)");
        let save = adw::ButtonRow::new();
        save.set_title("Save profile");
        save.add_css_class("suggested-action");
        {
            let (u, name) = (self.clone(), name.clone());
            save.connect_activated(move |_| {
                let n = name.text().trim().to_string();
                if n.is_empty() {
                    u.toast("Give the profile a name first");
                    return;
                }
                u.hub.save_profile(&n);
                u.toast(&format!("Saved profile “{n}”"));
                name.set_text("");
            });
        }
        new.add(&name);
        new.add(&save);
        b.append(&new);
        b
    }

    fn sync_profiles(self: &Rc<Self>) {
        let names = self.hub.profile_names();
        let sig = names.join("\u{1}");
        if *self.profile_sig.borrow() == sig {
            return;
        }
        *self.profile_sig.borrow_mut() = sig;
        for r in self.profile_rows.borrow_mut().drain(..) {
            self.profile_group.remove(&r);
        }
        if names.is_empty() {
            let r = adw::ActionRow::new();
            r.set_title("No profiles yet");
            self.profile_group.add(&r);
            self.profile_rows.borrow_mut().push(r);
        }
        for n in names {
            let r = adw::ActionRow::new();
            r.set_title(&n);
            let apply = gtk::Button::with_label("Apply");
            apply.add_css_class("suggested-action");
            apply.set_valign(gtk::Align::Center);
            let del = icon_button("user-trash-symbolic", "Delete");
            let (u, nn) = (self.clone(), n.clone());
            apply.connect_clicked(move |_| match u.hub.apply_profile(&nn) {
                Ok(k) => u.toast(&format!("Applied “{nn}” to {k} display(s)")),
                Err(e) => u.toast(&e),
            });
            let (u, nn) = (self.clone(), n.clone());
            del.connect_clicked(move |_| u.hub.delete_profile(&nn));
            r.add_suffix(&apply);
            r.add_suffix(&del);
            self.profile_group.add(&r);
            self.profile_rows.borrow_mut().push(r);
        }
    }

    // -------------------------------------------------------------- general

    fn build_general(self: &Rc<Self>) -> gtk::Box {
        let hub = self.hub.clone();
        let s = hub.settings();
        let b = gtk::Box::new(gtk::Orientation::Vertical, 18);

        let g = adw::PreferencesGroup::new();
        g.set_title("Brightness");
        let combined = adw::SwitchRow::new();
        combined.set_title("Combined brightness");
        combined.set_subtitle("The lowest part of the slider dims the picture in software, below what the monitor's backlight can do");
        combined.set_active(s.combined_brightness);
        {
            let hub = hub.clone();
            combined.connect_active_notify(move |r| {
                let on = r.is_active();
                hub.update_settings(|s| s.combined_brightness = on);
            });
        }
        let follow = adw::SwitchRow::new();
        follow.set_title("External monitors follow the built-in screen");
        follow.set_subtitle("The brightness keys of the laptop then change every monitor");
        follow.set_active(s.follow_internal);
        {
            let hub = hub.clone();
            follow.connect_active_notify(move |r| {
                let on = r.is_active();
                hub.update_settings(|s| s.follow_internal = on);
            });
        }
        let step = adw::SpinRow::with_range(1.0, 25.0, 1.0);
        step.set_title("Step");
        step.set_subtitle("Percent per scroll notch or brightness key");
        step.set_value(s.step as f64);
        {
            let hub = hub.clone();
            step.connect_value_notify(move |r| {
                let v = r.value() as u32;
                hub.update_settings(|s| s.step = v);
            });
        }
        g.add(&combined);
        g.add(&follow);
        g.add(&step);
        b.append(&g);

        let g = adw::PreferencesGroup::new();
        g.set_title("Monitors (DDC/CI)");
        g.set_description(Some("DDC/CI talks to the monitor itself over the video cable. Switch it on in the monitor's own menu if a monitor shows no controls."));
        let ddc = adw::SwitchRow::new();
        ddc.set_title("Control monitors over DDC/CI");
        ddc.set_active(s.ddc_enabled);
        {
            let hub = hub.clone();
            ddc.connect_active_notify(move |r| {
                let on = r.is_active();
                hub.update_settings(|s| s.ddc_enabled = on);
                if on {
                    hub.redetect();
                }
            });
        }
        let restore = adw::SwitchRow::new();
        restore.set_title("Restore monitor settings when it appears");
        restore.set_subtitle("Brightness, contrast, volume, input... come back after a monitor was off or plugged in again");
        restore.set_active(s.restore_on_connect);
        {
            let hub = hub.clone();
            restore.connect_active_notify(move |r| {
                let on = r.is_active();
                hub.update_settings(|s| s.restore_on_connect = on);
            });
        }
        let restore_l = adw::SwitchRow::new();
        restore_l.set_title("Restore resolution, scale and position too");
        restore_l.set_subtitle("Leave off when the compositor's own configuration handles this");
        restore_l.set_active(s.restore_layout_on_connect);
        {
            let hub = hub.clone();
            restore_l.connect_active_notify(move |r| {
                let on = r.is_active();
                hub.update_settings(|s| s.restore_layout_on_connect = on);
            });
        }
        let speed = adw::ComboRow::new();
        speed.set_title("DDC speed");
        speed.set_subtitle("Faster is riskier; applies after a restart of Penumbra");
        let speeds = [(2.0f32, "Careful"), (1.0, "Normal"), (0.5, "Fast"), (0.25, "Fastest")];
        speed.set_model(Some(&gtk::StringList::new(&speeds.iter().map(|s| s.1).collect::<Vec<_>>())));
        speed.set_selected(speeds.iter().position(|x| (x.0 - s.ddc_speed).abs() < 0.01).unwrap_or(1) as u32);
        {
            let hub = hub.clone();
            speed.connect_selected_notify(move |r| {
                let v = speeds[r.selected() as usize].0;
                hub.update_settings(|s| s.ddc_speed = v);
            });
        }
        let redetect = adw::ButtonRow::new();
        redetect.set_title("Detect monitors again");
        {
            let hub = hub.clone();
            redetect.connect_activated(move |_| hub.redetect());
        }
        g.add(&ddc);
        g.add(&restore);
        g.add(&restore_l);
        g.add(&speed);
        g.add(&redetect);
        b.append(&g);

        let g = adw::PreferencesGroup::new();
        g.set_title("Panel and start");
        let pos = adw::ComboRow::new();
        pos.set_title("Panel position");
        let anchors = [(Anchor::TopRight, "Top right"), (Anchor::TopLeft, "Top left"), (Anchor::BottomRight, "Bottom right"), (Anchor::BottomLeft, "Bottom left"), (Anchor::Center, "Centre")];
        pos.set_model(Some(&gtk::StringList::new(&anchors.iter().map(|a| a.1).collect::<Vec<_>>())));
        pos.set_selected(anchors.iter().position(|a| a.0 == s.panel_anchor).unwrap_or(0) as u32);
        {
            let hub = hub.clone();
            pos.connect_selected_notify(move |r| {
                let a = anchors[r.selected() as usize].0;
                hub.update_settings(|s| s.panel_anchor = a);
            });
        }
        let auto = adw::SwitchRow::new();
        auto.set_title("Start with the session");
        auto.set_active(crate::autostart::is_enabled());
        {
            let u = self.clone();
            auto.connect_active_notify(move |r| {
                if let Err(e) = crate::autostart::set(r.is_active()) {
                    u.toast(&e);
                }
                let _ = crate::autostart::install_launcher();
            });
        }
        g.add(&pos);
        g.add(&auto);
        b.append(&g);

        let g = adw::PreferencesGroup::new();
        g.set_title("Keyboard shortcuts");
        g.set_description(Some("Wayland compositors own the keys, so bind them there. In the Umbriel config, for example:"));
        let snippet = "\"XF86MonBrightnessUp\" = \"spawn:penumbra brightness +5\"\n\"XF86MonBrightnessDown\" = \"spawn:penumbra brightness -5\"\n\"Mod+D\" = \"spawn:penumbra toggle\"";
        let code = gtk::Label::new(Some(snippet));
        code.set_selectable(true);
        code.set_xalign(0.0);
        code.add_css_class("monospace");
        code.add_css_class("card");
        code.set_margin_top(4);
        code.set_wrap(true);
        let inner = gtk::Box::new(gtk::Orientation::Vertical, 6);
        code.set_margin_start(2);
        inner.append(&code);
        let hint = gtk::Label::new(Some("Add --display all to change every screen at once, or a connector name (HDMI-A-1) for one screen. `penumbra --help` lists every command."));
        hint.add_css_class("pen-hint");
        hint.set_wrap(true);
        hint.set_xalign(0.0);
        inner.append(&hint);
        g.add(&inner);
        b.append(&g);
        b
    }

    fn build_about(&self) -> gtk::Box {
        let b = gtk::Box::new(gtk::Orientation::Vertical, 10);
        b.set_margin_top(30);
        let icon = gtk::Image::from_icon_name("penumbra");
        icon.set_pixel_size(96);
        let name = gtk::Label::new(Some("Penumbra"));
        name.add_css_class("title-1");
        let ver = gtk::Label::new(Some(&format!("Version {}", env!("CARGO_PKG_VERSION"))));
        ver.add_css_class("dim-label");
        let desc = gtk::Label::new(Some("Brightness, colour, resolution, scaling and virtual screens for every display, from the tray.\nA BetterDisplay-style utility for Linux and Wayland."));
        desc.set_justify(gtk::Justification::Center);
        desc.set_wrap(true);
        let credits = gtk::Label::new(Some("Monitors are controlled over DDC/CI with ddcutil. Screens are configured through the wlr-output-management protocol; software dimming uses wlr-gamma-control."));
        credits.add_css_class("pen-hint");
        credits.set_wrap(true);
        credits.set_justify(gtk::Justification::Center);
        for w in [icon.upcast::<gtk::Widget>(), name.upcast(), ver.upcast(), desc.upcast(), credits.upcast()] {
            b.append(&w);
        }
        b
    }

    // -------------------------------------------------------------- displays

    fn sync(self: &Rc<Self>) {
        let views = self.hub.views();
        self.sync_selector(&views);
        for v in &views {
            if let Some(p) = self.pages.borrow().get(&v.name) {
                self.update_display_page(p, v);
            }
        }
        self.arrange.sync(&views);
        self.sync_virtual(&views);
        self.sync_profiles();
    }

    fn sync_selector(self: &Rc<Self>, views: &[DisplayView]) {
        // pages
        {
            let mut pages = self.pages.borrow_mut();
            let gone: Vec<String> = pages.keys().filter(|k| !views.iter().any(|v| &v.name == *k)).cloned().collect();
            for g in gone {
                if let Some(p) = pages.remove(&g) {
                    self.display_stack.remove(&p.root);
                }
                if let Some(c) = self.chips.borrow_mut().remove(&g) {
                    self.selector.remove(&c);
                }
            }
        }
        for v in views {
            if !self.pages.borrow().contains_key(&v.name) {
                let page = self.build_display_page(v);
                self.display_stack.add_named(&page.root, Some(&v.name));
                self.pages.borrow_mut().insert(v.name.clone(), page);
                let chip = gtk::ToggleButton::new();
                self.selector.append(&chip);
                if let Some(first) = self.chips.borrow().values().next() {
                    chip.set_group(Some(first));
                }
                let (u, n) = (self.clone(), v.name.clone());
                chip.connect_toggled(move |b| {
                    if b.is_active() {
                        *u.selected.borrow_mut() = Some(n.clone());
                        u.display_stack.set_visible_child_name(&n);
                    }
                });
                self.chips.borrow_mut().insert(v.name.clone(), chip);
            }
            if let Some(chip) = self.chips.borrow().get(&v.name) {
                chip.set_label(&v.display_name());
            }
        }
        // keep a valid selection
        let sel = self.selected.borrow().clone();
        let valid = sel.as_ref().map(|s| views.iter().any(|v| &v.name == s)).unwrap_or(false);
        if !valid {
            let pick = views.iter().find(|v| !v.output.is_internal()).or(views.first()).map(|v| v.name.clone());
            if let Some(p) = pick {
                if let Some(c) = self.chips.borrow().get(&p) {
                    c.set_active(true);
                }
                *self.selected.borrow_mut() = Some(p.clone());
                self.display_stack.set_visible_child_name(&p);
            }
        }
    }

    fn build_display_page(self: &Rc<Self>, v: &DisplayView) -> DisplayPage {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 16);
        let title = gtk::Label::new(None);
        title.add_css_class("title-2");
        title.set_xalign(0.0);

        let top = adw::PreferencesGroup::new();
        let nick = adw::EntryRow::new();
        nick.set_title("Name");
        nick.set_show_apply_button(true);
        {
            let (hub, name) = (self.hub.clone(), v.name.clone());
            nick.connect_apply(move |r| {
                if let Some(v) = hub.view(&name) {
                    hub.set_nickname(&v.id, Some(r.text().to_string()));
                }
            });
        }
        top.add(&nick);

        let card = gtk::Box::new(gtk::Orientation::Vertical, 10);
        card.add_css_class("pen-card");
        let bl = gtk::Label::new(Some("Brightness"));
        bl.add_css_class("pen-title");
        bl.set_xalign(0.0);
        let brightness = SliderRow::new("display-brightness-symbolic", 0.0, 100.0, 1.0, |v| format!("{}%", v.round()));
        brightness.root.add_css_class("pen-big");
        {
            let (hub, name) = (self.hub.clone(), v.name.clone());
            brightness.connect_changed(move |x| hub.set_brightness(&name, x / 100.0));
        }
        card.append(&bl);
        card.append(&brightness.root);
        let controls = DisplayControls::new(&self.hub, &v.name);
        let cc = gtk::Box::new(gtk::Orientation::Vertical, 0);
        cc.add_css_class("pen-card");
        cc.append(&controls.root);

        let opts = adw::PreferencesGroup::new();
        opts.set_title("Options");
        let shown = adw::SwitchRow::new();
        shown.set_title("Show in the panel");
        {
            let (hub, name) = (self.hub.clone(), v.name.clone());
            shown.connect_active_notify(move |r| {
                if let Some(v) = hub.view(&name) {
                    if hub.view(&name).map(|x| x.hidden == r.is_active()).unwrap_or(false) {
                        hub.set_hidden(&v.id, !r.is_active());
                    }
                }
            });
        }
        let follow = adw::SwitchRow::new();
        follow.set_title("Follow the built-in screen's brightness");
        {
            let (hub, name) = (self.hub.clone(), v.name.clone());
            follow.connect_active_notify(move |r| {
                if let Some(v) = hub.view(&name) {
                    hub.set_follow(&v.id, r.is_active());
                }
            });
        }
        let ident = adw::ActionRow::new();
        ident.set_title("Identify");
        ident.set_subtitle("Show this screen's name on it");
        let ib = gtk::Button::with_label("Show");
        ib.set_valign(gtk::Align::Center);
        {
            let name = v.name.clone();
            ib.connect_clicked(move |_| super::osd::identify(&name));
        }
        ident.add_suffix(&ib);
        let off = adw::ActionRow::new();
        off.set_title("Turn this output off");
        off.set_subtitle("Removes it from the desktop until you turn it on again with `penumbra enable`");
        let ob = gtk::Button::with_label("Turn off");
        ob.add_css_class("destructive-action");
        ob.set_valign(gtk::Align::Center);
        {
            let (u, name) = (self.clone(), v.name.clone());
            ob.connect_clicked(move |_| {
                let on = u.hub.view(&name).map(|d| d.output.enabled).unwrap_or(false);
                if !on {
                    u.hub.set_enabled(&name, true);
                } else if u.hub.views().iter().filter(|d| d.output.enabled).count() <= 1 {
                    u.toast("That is your only screen; it stays on.");
                } else {
                    u.hub.set_enabled(&name, false);
                }
            });
        }
        off.add_suffix(&ob);
        opts.add(&shown);
        opts.add(&follow);
        opts.add(&ident);
        opts.add(&off);

        // custom mode
        let custom = adw::PreferencesGroup::new();
        custom.set_title("Custom resolution");
        custom.set_description(Some("A mode the monitor does not list, such as 2560x1440@75. The compositor may refuse it if the monitor cannot show it."));
        let ce = adw::EntryRow::new();
        ce.set_title("Width × Height @ Hz");
        ce.set_show_apply_button(true);
        {
            let (u, name) = (self.clone(), v.name.clone());
            ce.connect_apply(move |r| {
                let text = r.text().to_string();
                let modes = u.hub.view(&name).map(|v| v.output.modes).unwrap_or_default();
                match crate::commands::parse_mode(text.trim(), &modes) {
                    Some(m) => u.hub.set_mode(&name, &m),
                    None => u.toast("Write it like 2560x1440@75"),
                }
            });
        }
        custom.add(&ce);

        // information
        let info = adw::PreferencesGroup::new();
        info.set_title("Information");
        let mut rows = HashMap::new();
        for key in ["model", "connector", "size", "density", "serial", "ddc", "features"] {
            let r = adw::ActionRow::new();
            r.set_title(match key {
                "model" => "Model",
                "connector" => "Connection",
                "size" => "Size",
                "density" => "Pixel density",
                "serial" => "Serial number",
                "ddc" => "DDC/CI",
                _ => "Monitor features",
            });
            r.set_subtitle_selectable(true);
            info.add(&r);
            rows.insert(key, r);
        }

        root.append(&title);
        root.append(&top);
        root.append(&card);
        root.append(&cc);
        root.append(&opts);
        root.append(&custom);
        root.append(&info);
        DisplayPage { root, title, nick, brightness, controls, shown, follow, off_row: off, off_btn: ob, rows }
    }

    fn update_display_page(&self, p: &DisplayPage, v: &DisplayView) {
        p.title.set_text(&v.display_name());
        if !p.nick.has_focus() && !p.nick.is_focus() {
            let want = v.nickname.clone().unwrap_or_default();
            if p.nick.text() != want {
                p.nick.set_text(&want);
            }
        }
        p.nick.set_tooltip_text(Some(&v.title));
        p.brightness.set_quiet(v.brightness * 100.0);
        p.controls.update(v);
        p.off_row.set_title(if v.output.enabled { "Turn this output off" } else { "This output is off" });
        p.off_btn.set_label(if v.output.enabled { "Turn off" } else { "Turn on" });
        if v.output.enabled {
            p.off_btn.add_css_class("destructive-action");
            p.off_btn.remove_css_class("suggested-action");
        } else {
            p.off_btn.remove_css_class("destructive-action");
            p.off_btn.add_css_class("suggested-action");
        }
        if p.shown.is_active() == v.hidden {
            p.shown.set_active(!v.hidden);
        }
        let mem_follow = self.hub.follow_of(&v.id);
        p.follow.set_visible(!v.output.is_internal());
        if p.follow.is_active() != mem_follow {
            p.follow.set_active(mem_follow);
        }
        let o = &v.output;
        let set = |k: &str, text: String| {
            if let Some(r) = p.rows.get(k) {
                r.set_subtitle(&text);
                r.set_visible(!text.is_empty());
            }
        };
        set("model", format!("{} {}", o.make, o.model).trim().to_string());
        set(
            "connector",
            format!("{}{}", o.name, match v.control {
                Control::Backlight => " · built-in panel",
                _ if o.is_virtual() => " · virtual",
                _ => "",
            }),
        );
        set("size", match (o.diagonal_inches(), o.phys_mm) {
            (Some(d), (w, h)) => format!("{d:.1}″  ({w} × {h} mm)"),
            _ => String::new(),
        });
        set("density", o.ppi().map(|p| format!("{} pixels per inch", p.round())).unwrap_or_default());
        set("serial", o.serial.clone());
        set("ddc", match &v.ddc {
            Some(d) => format!("bus i2c-{} · {}", d.bus, match &d.status {
                DdcStatus::Ready => d.caps.as_ref().map(|c| if c.mccs.is_empty() { "ready".to_string() } else { format!("MCCS {}", c.mccs) }).unwrap_or("ready".into()),
                DdcStatus::Failed(_) => "no answer".into(),
                _ => "reading…".into(),
            }),
            None => {
                if o.is_internal() {
                    "built-in panels use the backlight instead".into()
                } else if o.is_virtual() {
                    String::new()
                } else {
                    "not available (software dimming only)".into()
                }
            }
        });
        set("features", v.ddc.as_ref().and_then(|d| d.caps.as_ref()).map(|c| {
            c.features.iter().map(|f| format!("{f:02X}")).collect::<Vec<_>>().join(" ")
        }).unwrap_or_default());
    }
}
