//! The panel that drops down from the tray icon: every display with its brightness, and the
//! rest of its controls folded underneath.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use super::controls::DisplayControls;
use super::widgets::*;
use crate::hub::{Control, DisplayView, Hub};
use crate::store::Anchor;

struct Card {
    root: gtk::Box,
    title: gtk::Label,
    chip: gtk::Label,
    subtitle: gtk::Label,
    icon: gtk::Image,
    brightness: SliderRow,
    controls: DisplayControls,
    menu_items: gtk::Box,
}

pub struct Panel {
    pub window: gtk::Window,
    scrim: gtk::Window,
    hub: Hub,
    cards: RefCell<HashMap<String, Card>>,
    list: gtk::Box,
    master: SliderRow,
    master_box: gtk::Box,
    empty: gtk::Label,
    off_box: gtk::Box,
    off_sig: RefCell<String>,
    toast: gtk::Label,
    toast_gen: Rc<Cell<u32>>,
    profiles_box: gtk::Box,
    expanded: RefCell<std::collections::HashSet<String>>,
    on_settings: RefCell<Option<Box<dyn Fn(&str)>>>,
    on_quit: RefCell<Option<Box<dyn Fn()>>>,
    scroller: gtk::ScrolledWindow,
}

impl Panel {
    pub fn new(app: &gtk::Application, hub: &Hub) -> Rc<Panel> {
        let window = gtk::Window::builder().application(app).title("Penumbra").decorated(false).resizable(false).build();
        window.add_css_class("pen-window");
        window.init_layer_shell();
        window.set_layer(Layer::Overlay);
        window.set_namespace(Some("penumbra-panel"));
        window.set_keyboard_mode(KeyboardMode::OnDemand);

        let scrim = gtk::Window::builder().application(app).title("Penumbra scrim").decorated(false).build();
        scrim.add_css_class("pen-scrim");
        scrim.init_layer_shell();
        scrim.set_layer(Layer::Overlay);
        scrim.set_namespace(Some("penumbra-scrim"));
        scrim.set_keyboard_mode(KeyboardMode::None);
        for e in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
            scrim.set_anchor(e, true);
        }
        scrim.set_exclusive_zone(0);

        let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        outer.add_css_class("pen-panel");
        outer.set_margin_top(10);
        outer.set_margin_bottom(18);
        outer.set_margin_start(18);
        outer.set_margin_end(18);

        // header
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.add_css_class("pen-header");
        let title = gtk::Label::new(Some("Displays"));
        title.add_css_class("pen-heading");
        title.set_hexpand(true);
        title.set_xalign(0.0);
        let profiles_btn = gtk::MenuButton::new();
        profiles_btn.set_icon_name("view-list-symbolic");
        profiles_btn.add_css_class("flat");
        profiles_btn.add_css_class("circular");
        profiles_btn.set_tooltip_text(Some("Profiles"));
        let profiles_pop = gtk::Popover::new();
        profiles_pop.add_css_class("pen-menu");
        let profiles_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        profiles_pop.set_child(Some(&profiles_box));
        profiles_btn.set_popover(Some(&profiles_pop));
        let settings_btn = icon_button("emblem-system-symbolic", "Settings");
        let more_btn = gtk::MenuButton::new();
        more_btn.set_icon_name("view-more-symbolic");
        more_btn.add_css_class("flat");
        more_btn.add_css_class("circular");
        let more_pop = gtk::Popover::new();
        more_pop.add_css_class("pen-menu");
        let more_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        more_pop.set_child(Some(&more_box));
        more_btn.set_popover(Some(&more_pop));
        header.append(&title);
        header.append(&profiles_btn);
        header.append(&settings_btn);
        header.append(&more_btn);

        // master slider
        let master = SliderRow::new("display-brightness-symbolic", 0.0, 100.0, 1.0, |v| format!("{}%", v.round()));
        master.root.add_css_class("pen-big");
        let master_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
        master_box.add_css_class("pen-card");
        let ml = gtk::Label::new(Some("All displays"));
        ml.set_xalign(0.0);
        ml.add_css_class("pen-title");
        master_box.append(&ml);
        master_box.append(&master.root);

        let list = gtk::Box::new(gtk::Orientation::Vertical, 10);
        let empty = gtk::Label::new(Some("No displays found"));
        empty.add_css_class("pen-hint");
        empty.set_margin_top(20);
        empty.set_margin_bottom(20);
        let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
        body.set_margin_start(12);
        body.set_margin_end(12);
        body.set_margin_bottom(12);
        body.append(&master_box);
        body.append(&list);
        let off_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        body.append(&empty);
        body.append(&off_box);
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_child(Some(&body));
        // As tall as the content, up to the screen's height (see `fit_height`); beyond that the
        // content scrolls, with a scrollbar that only shows while scrolling.
        scroller.set_propagate_natural_height(true);
        scroller.set_max_content_height(680);
        scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroller.set_vscrollbar_policy(gtk::PolicyType::Automatic);
        scroller.set_overlay_scrolling(true);
        scroller.set_min_content_width(440);

        let toast = gtk::Label::new(None);
        toast.add_css_class("pen-toast");
        toast.set_wrap(true);
        toast.set_visible(false);
        toast.set_margin_start(12);
        toast.set_margin_end(12);
        toast.set_margin_bottom(10);

        outer.append(&header);
        outer.append(&scroller);
        outer.append(&toast);
        window.set_child(Some(&outer));

        let panel = Rc::new(Panel {
            window,
            scrim,
            hub: hub.clone(),
            cards: RefCell::new(HashMap::new()),
            list,
            master,
            master_box,
            empty,
            off_box,
            off_sig: RefCell::new(String::new()),
            toast,
            toast_gen: Rc::new(Cell::new(0)),
            profiles_box,
            expanded: RefCell::new(Default::default()),
            on_settings: RefCell::new(None),
            on_quit: RefCell::new(None),
            scroller,
        });

        // master slider drives every display
        {
            let hub = hub.clone();
            panel.master.connect_changed(move |v| {
                for d in hub.views().iter().filter(|d| !d.hidden && d.output.enabled) {
                    hub.set_brightness(&d.name, v / 100.0);
                }
            });
        }
        // buttons
        {
            let p = panel.clone();
            settings_btn.connect_clicked(move |_| {
                p.hide();
                if let Some(f) = p.on_settings.borrow().as_ref() {
                    f("displays");
                }
            });
        }
        panel.fill_more(&more_box, &more_pop);
        {
            let p = panel.clone();
            profiles_pop.connect_show(move |_| p.fill_profiles());
        }
        // closing: Esc, click on the scrim, focus lost
        {
            let p = panel.clone();
            let keys = gtk::EventControllerKey::new();
            keys.connect_key_pressed(move |_, key, _, _| {
                if key == gtk::gdk::Key::Escape {
                    p.hide();
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
            panel.window.add_controller(keys);
        }
        {
            let p = panel.clone();
            let click = gtk::GestureClick::new();
            click.connect_pressed(move |_, _, _, _| p.hide());
            panel.scrim.add_controller(click);
        }
        {
            let p = panel.clone();
            panel.window.connect_is_active_notify(move |w| {
                if !w.is_active() {
                    let p = p.clone();
                    glib::timeout_add_local_once(Duration::from_millis(350), move || {
                        if p.window.is_visible() && !p.window.is_active() && !p.popover_open() {
                            p.hide();
                        }
                    });
                }
            });
        }
        panel
    }

    fn popover_open(&self) -> bool {
        // A drop-down's list is a popover; while one is open the window may report "inactive".
        let mut open = false;
        let mut w = self.window.first_child();
        fn walk(w: &gtk::Widget, open: &mut bool) {
            if let Some(p) = w.downcast_ref::<gtk::Popover>() {
                if p.is_visible() {
                    *open = true;
                }
            }
            let mut c = w.first_child();
            while let Some(x) = c {
                walk(&x, open);
                c = x.next_sibling();
            }
        }
        while let Some(x) = w {
            walk(&x, &mut open);
            w = x.next_sibling();
        }
        open
    }

    fn fill_more(self: &Rc<Self>, more_box: &gtk::Box, pop: &gtk::Popover) {
        let add = |label: &str, f: Box<dyn Fn()>| {
            let b = gtk::Button::with_label(label);
            b.add_css_class("flat");
            b.set_halign(gtk::Align::Fill);
            if let Some(l) = b.child().and_then(|c| c.downcast::<gtk::Label>().ok()) {
                l.set_xalign(0.0);
            }
            let pop = pop.clone();
            b.connect_clicked(move |_| {
                pop.popdown();
                f();
            });
            more_box.append(&b);
        };
        let p = self.clone();
        add("Detect monitors again", Box::new(move || p.hub.redetect()));
        let p = self.clone();
        add("Virtual displays…", Box::new(move || {
            p.hide();
            if let Some(f) = p.on_settings.borrow().as_ref() {
                f("virtual");
            }
        }));
        let p = self.clone();
        add("Settings…", Box::new(move || {
            p.hide();
            if let Some(f) = p.on_settings.borrow().as_ref() {
                f("general");
            }
        }));
        let p = self.clone();
        add("Quit Penumbra", Box::new(move || {
            if let Some(f) = p.on_quit.borrow().as_ref() {
                f();
            }
        }));
    }

    fn fill_profiles(self: &Rc<Self>) {
        while let Some(c) = self.profiles_box.first_child() {
            self.profiles_box.remove(&c);
        }
        let names = self.hub.profile_names();
        if names.is_empty() {
            let l = gtk::Label::new(Some("No profiles yet"));
            l.add_css_class("pen-hint");
            l.set_margin_top(6);
            l.set_margin_bottom(6);
            l.set_margin_start(8);
            l.set_margin_end(8);
            self.profiles_box.append(&l);
        }
        for n in names {
            let b = gtk::Button::with_label(&n);
            b.add_css_class("flat");
            if let Some(l) = b.child().and_then(|c| c.downcast::<gtk::Label>().ok()) {
                l.set_xalign(0.0);
            }
            let p = self.clone();
            b.connect_clicked(move |_| {
                match p.hub.apply_profile(&n) {
                    Ok(k) => p.toast(&format!("Applied “{n}” to {k} display(s)"), false),
                    Err(e) => p.toast(&e, true),
                }
            });
            self.profiles_box.append(&b);
        }
        self.profiles_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        let save = gtk::Button::with_label("Save current setup…");
        save.add_css_class("flat");
        if let Some(l) = save.child().and_then(|c| c.downcast::<gtk::Label>().ok()) {
            l.set_xalign(0.0);
        }
        let p = self.clone();
        save.connect_clicked(move |_| {
            p.hide();
            if let Some(f) = p.on_settings.borrow().as_ref() {
                f("profiles");
            }
        });
        self.profiles_box.append(&save);
    }

    pub fn connect_settings(&self, f: impl Fn(&str) + 'static) {
        *self.on_settings.borrow_mut() = Some(Box::new(f));
    }

    pub fn connect_quit(&self, f: impl Fn() + 'static) {
        *self.on_quit.borrow_mut() = Some(Box::new(f));
    }

    pub fn toast(&self, msg: &str, error: bool) {
        self.toast.set_text(msg);
        if error {
            self.toast.add_css_class("error");
        } else {
            self.toast.remove_css_class("error");
        }
        self.toast.set_visible(true);
        let g = self.toast_gen.get() + 1;
        self.toast_gen.set(g);
        let t = self.toast.clone();
        let gen = self.toast_gen.clone();
        glib::timeout_add_local_once(Duration::from_secs(if error { 7 } else { 3 }), move || {
            // only the newest toast may hide it
            if gen.get() == g {
                t.set_visible(false);
            }
        });
    }

    pub fn is_visible(&self) -> bool {
        self.window.is_visible()
    }

    pub fn toggle(self: &Rc<Self>) {
        if self.is_visible() {
            self.hide();
        } else {
            self.show();
        }
    }

    pub fn show(self: &Rc<Self>) {
        let s = self.hub.settings();
        let w = &self.window;
        for e in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
            w.set_anchor(e, false);
        }
        let m = s.panel_margin;
        match s.panel_anchor {
            Anchor::TopRight => {
                w.set_anchor(Edge::Top, true);
                w.set_anchor(Edge::Right, true);
            }
            Anchor::TopLeft => {
                w.set_anchor(Edge::Top, true);
                w.set_anchor(Edge::Left, true);
            }
            Anchor::BottomRight => {
                w.set_anchor(Edge::Bottom, true);
                w.set_anchor(Edge::Right, true);
            }
            Anchor::BottomLeft => {
                w.set_anchor(Edge::Bottom, true);
                w.set_anchor(Edge::Left, true);
            }
            Anchor::Center => {}
        }
        for e in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
            w.set_margin(e, m);
        }
        // The panel opens on the screen that has focus.
        if let Some(name) = crate::compositor::focused_output() {
            if let Some(mon) = super::monitor_for(&name) {
                w.set_monitor(Some(&mon));
                self.scrim.set_monitor(Some(&mon));
                let h = crate::compositor::usable_height(&name).unwrap_or_else(|| mon.geometry().height());
                self.fit_height(h, s.panel_margin);
            }
        }
        self.hub.refresh(false);
        self.sync(&self.hub.views());
        self.scrim.present();
        w.present();
    }

    /// The tallest the content may be: the screen's usable height less the panel's own margins,
    /// its header and a gap above and below.
    fn fit_height(&self, screen: i32, margin: i32) {
        const GAP: i32 = 12;
        let outer = self.scroller.parent().and_then(|p| p.downcast::<gtk::Box>().ok());
        let mut chrome = 2 * margin + 2 * GAP;
        if let Some(o) = &outer {
            chrome += o.margin_top() + o.margin_bottom();
            let mut c = o.first_child();
            while let Some(w) = c {
                if w != *self.scroller.upcast_ref::<gtk::Widget>() && w.is_visible() {
                    chrome += w.measure(gtk::Orientation::Vertical, -1).1;
                }
                c = w.next_sibling();
            }
        }
        self.scroller.set_max_content_height((screen - chrome).max(200));
    }

    pub fn hide(&self) {
        self.window.set_visible(false);
        self.scrim.set_visible(false);
    }

    /// Bring the cards in line with the hub's view of the displays.
    pub fn sync(self: &Rc<Self>, views: &[DisplayView]) {
        let visible: Vec<&DisplayView> = views.iter().filter(|v| !v.hidden && v.output.enabled).collect();
        let mut cards = self.cards.borrow_mut();
        // remove gone
        let names: Vec<String> = visible.iter().map(|v| v.name.clone()).collect();
        let gone: Vec<String> = cards.keys().filter(|k| !names.contains(k)).cloned().collect();
        for g in gone {
            if let Some(c) = cards.remove(&g) {
                self.list.remove(&c.root);
            }
        }
        // add new, update all; keep order
        let mut prev: Option<gtk::Widget> = None;
        for v in &visible {
            if !cards.contains_key(&v.name) {
                let c = self.make_card(v);
                self.list.append(&c.root);
                cards.insert(v.name.clone(), c);
            }
            let c = cards.get(&v.name).unwrap();
            self.list.reorder_child_after(&c.root, prev.as_ref());
            prev = Some(c.root.clone().upcast());
            self.update_card(c, v);
        }
        self.empty.set_visible(visible.is_empty() && !views.iter().any(|v| !v.output.enabled));
        self.sync_disabled(views);
        self.master_box.set_visible(visible.len() > 1);
        if !visible.is_empty() {
            let avg = visible.iter().map(|v| v.brightness).sum::<f64>() / visible.len() as f64;
            self.master.set_quiet(avg * 100.0);
        }
    }

    /// Outputs that are turned off: one line each, with a button to turn them on again.
    fn sync_disabled(self: &Rc<Self>, views: &[DisplayView]) {
        let off: Vec<&DisplayView> = views.iter().filter(|v| !v.output.enabled).collect();
        let sig = off.iter().map(|v| v.name.clone()).collect::<Vec<_>>().join(",");
        if *self.off_sig.borrow() == sig {
            return;
        }
        *self.off_sig.borrow_mut() = sig;
        while let Some(c) = self.off_box.first_child() {
            self.off_box.remove(&c);
        }
        for v in off {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            row.add_css_class("pen-card");
            let l = gtk::Label::new(Some(&format!("{} is off", v.display_name())));
            l.set_hexpand(true);
            l.set_xalign(0.0);
            let b = gtk::Button::with_label("Turn on");
            b.add_css_class("suggested-action");
            let (hub, n) = (self.hub.clone(), v.name.clone());
            b.connect_clicked(move |_| hub.set_enabled(&n, true));
            row.append(&l);
            row.append(&b);
            self.off_box.append(&row);
        }
    }

    fn make_card(self: &Rc<Self>, v: &DisplayView) -> Card {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.add_css_class("pen-card");

        let head = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let icon = gtk::Image::from_icon_name("video-display-symbolic");
        icon.set_pixel_size(22);
        icon.add_css_class("pen-display-icon");
        let texts = gtk::Box::new(gtk::Orientation::Vertical, 0);
        texts.set_hexpand(true);
        let title = gtk::Label::new(None);
        title.set_xalign(0.0);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        title.add_css_class("pen-title");
        let subtitle = gtk::Label::new(None);
        subtitle.set_xalign(0.0);
        subtitle.set_ellipsize(gtk::pango::EllipsizeMode::End);
        subtitle.add_css_class("pen-sub");
        let chip = gtk::Label::new(None);
        chip.add_css_class("pen-chip");
        let title_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        title_row.append(&title);
        title_row.append(&chip);
        texts.append(&title_row);
        texts.append(&subtitle);

        let menu_btn = gtk::MenuButton::new();
        menu_btn.set_icon_name("view-more-symbolic");
        menu_btn.add_css_class("flat");
        menu_btn.add_css_class("circular");
        menu_btn.set_valign(gtk::Align::Center);
        let pop = gtk::Popover::new();
        pop.add_css_class("pen-menu");
        let menu_items = gtk::Box::new(gtk::Orientation::Vertical, 2);
        pop.set_child(Some(&menu_items));
        menu_btn.set_popover(Some(&pop));
        let chevron = gtk::ToggleButton::new();
        chevron.set_icon_name("pan-down-symbolic");
        chevron.add_css_class("flat");
        chevron.add_css_class("circular");
        chevron.set_valign(gtk::Align::Center);
        chevron.set_tooltip_text(Some("More controls"));
        head.append(&icon);
        head.append(&texts);
        head.append(&chevron);
        head.append(&menu_btn);

        let brightness = SliderRow::new("display-brightness-symbolic", 0.0, 100.0, 1.0, |v| format!("{}%", v.round()));
        brightness.root.add_css_class("pen-big");
        {
            let hub = self.hub.clone();
            let name = v.name.clone();
            brightness.connect_changed(move |x| hub.set_brightness(&name, x / 100.0));
        }
        // scroll over the slider's row = steps of the configured size
        let controls = DisplayControls::new(&self.hub, &v.name);
        let reveal = gtk::Revealer::new();
        reveal.set_transition_type(gtk::RevealerTransitionType::SlideDown);
        reveal.set_transition_duration(180);
        reveal.set_child(Some(&controls.root));
        chevron.bind_property("active", &reveal, "reveal-child").sync_create().build();
        {
            let p = self.clone();
            let name = v.name.clone();
            chevron.connect_toggled(move |b| {
                if b.is_active() {
                    p.expanded.borrow_mut().insert(name.clone());
                } else {
                    p.expanded.borrow_mut().remove(&name);
                }
                b.set_icon_name(if b.is_active() { "pan-up-symbolic" } else { "pan-down-symbolic" });
            });
        }
        if self.expanded.borrow().contains(&v.name) || std::env::var("PENUMBRA_EXPAND").is_ok() {
            chevron.set_active(true);
        }
        root.append(&head);
        root.append(&brightness.root);
        root.append(&reveal);
        Card { root, title, chip, subtitle, icon, brightness, controls, menu_items }
    }

    fn update_card(self: &Rc<Self>, c: &Card, v: &DisplayView) {
        c.title.set_text(&v.display_name());
        let o = &v.output;
        let mut sub = Vec::new();
        if let Some(m) = &o.mode {
            sub.push(format!("{}×{}", m.width, m.height));
            sub.push(m.hz_label());
        }
        if (o.scale - 1.0).abs() > 0.01 {
            sub.push(format!("{}%", (o.scale * 100.0).round()));
        }
        c.chip.set_text(match v.control {
            Control::Ddc => "DDC/CI",
            Control::Backlight => "Built-in",
            Control::Software => "Software",
        });
        c.subtitle.set_text(&sub.join("  ·  "));
        c.icon.set_icon_name(Some(if o.is_virtual() { "video-joined-displays-symbolic" } else { "video-display-symbolic" }));
        c.brightness.set_quiet(v.brightness * 100.0);
        c.controls.update(v);
        self.fill_card_menu(c, v);
    }

    fn fill_card_menu(self: &Rc<Self>, c: &Card, v: &DisplayView) {
        // Rebuilt only when what it offers changes.
        let sig = format!("{}|{}|{}", v.name, v.ddc.is_some(), v.output.is_virtual());
        if c.menu_items.widget_name() == sig {
            return;
        }
        c.menu_items.set_widget_name(&sig);
        while let Some(ch) = c.menu_items.first_child() {
            c.menu_items.remove(&ch);
        }
        let name = v.name.clone();
        let id = v.id.clone();
        let add = |label: &str, f: Box<dyn Fn(&Rc<Panel>)>| {
            let b = gtk::Button::with_label(label);
            b.add_css_class("flat");
            if let Some(l) = b.child().and_then(|c| c.downcast::<gtk::Label>().ok()) {
                l.set_xalign(0.0);
            }
            let p = self.clone();
            b.connect_clicked(move |btn| {
                if let Some(pop) = btn.ancestor(gtk::Popover::static_type()).and_then(|w| w.downcast::<gtk::Popover>().ok()) {
                    pop.popdown();
                }
                f(&p);
            });
            c.menu_items.append(&b);
        };
        let n = name.clone();
        add("Identify", Box::new(move |_| super::osd::identify(&n)));
        let (hub, n) = (self.hub.clone(), name.clone());
        add("Reset colour", Box::new(move |_| hub.reset_colour(&n)));
        if v.ddc.is_some() {
            let (hub, n) = (self.hub.clone(), name.clone());
            add("Turn monitor off (DDC)", Box::new(move |_| hub.set_ddc(&n, crate::ddc::VCP_POWER, 4)));
        }
        let (hub, n) = (self.hub.clone(), name.clone());
        add("Disable this output", Box::new(move |p| {
            if hub.views().iter().filter(|d| d.output.enabled).count() <= 1 {
                p.toast("That is your only screen; it stays on.", true);
            } else {
                hub.set_enabled(&n, false);
            }
        }));
        let (hub, i) = (self.hub.clone(), id.clone());
        add("Hide from this panel", Box::new(move |_| hub.set_hidden(&i, true)));
        if v.output.is_virtual() {
            let (hub, n) = (self.hub.clone(), name.clone());
            add("Remove virtual display", Box::new(move |p| {
                if let Err(e) = hub.destroy_virtual(&n) {
                    p.toast(&e, true);
                }
            }));
        }
    }
}
