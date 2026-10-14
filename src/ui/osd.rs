//! On-screen indicators: the brightness bar after a hotkey, and "Identify" (a big name on a screen).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

fn layer_window(ns: &str) -> gtk::Window {
    let w = gtk::Window::builder().decorated(false).resizable(false).build();
    w.add_css_class("pen-window");
    w.init_layer_shell();
    w.set_layer(Layer::Overlay);
    w.set_namespace(Some(ns));
    w.set_keyboard_mode(KeyboardMode::None);
    w.set_exclusive_zone(-1);
    w
}

/// Show the name of a screen on that screen for two seconds.
pub fn identify(output: &str) {
    let w = layer_window("penumbra-identify");
    if let Some(m) = super::monitor_for(output) {
        w.set_monitor(Some(&m));
    }
    w.set_anchor(Edge::Top, true);
    w.set_anchor(Edge::Left, true);
    w.set_margin(Edge::Top, 40);
    w.set_margin(Edge::Left, 40);
    let b = gtk::Box::new(gtk::Orientation::Vertical, 4);
    b.add_css_class("pen-identify");
    let l = gtk::Label::new(Some(output));
    l.add_css_class("pen-identify-name");
    b.append(&l);
    w.set_child(Some(&b));
    w.set_can_target(false);
    w.present();
    glib::timeout_add_local_once(Duration::from_secs(2), move || w.close());
}

/// The bar that appears after a brightness hotkey.
pub struct Osd {
    window: gtk::Window,
    bar: gtk::LevelBar,
    label: gtk::Label,
    title: gtk::Label,
    generation: Rc<Cell<u32>>,
    current_monitor: RefCell<String>,
}

impl Osd {
    pub fn new() -> Osd {
        let window = layer_window("penumbra-osd");
        window.set_anchor(Edge::Bottom, true);
        window.set_margin(Edge::Bottom, 90);
        window.set_can_target(false);
        let b = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        b.add_css_class("pen-osd");
        let icon = gtk::Image::from_icon_name("display-brightness-symbolic");
        icon.set_pixel_size(22);
        let col = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let title = gtk::Label::new(None);
        title.set_xalign(0.0);
        title.add_css_class("pen-osd-title");
        let bar = gtk::LevelBar::for_interval(0.0, 1.0);
        bar.set_size_request(220, 8);
        bar.add_css_class("pen-osd-bar");
        col.append(&title);
        col.append(&bar);
        let label = gtk::Label::new(None);
        label.add_css_class("pen-value");
        label.set_width_chars(5);
        b.append(&icon);
        b.append(&col);
        b.append(&label);
        window.set_child(Some(&b));
        Osd { window, bar, label, title, generation: Rc::new(Cell::new(0)), current_monitor: RefCell::new(String::new()) }
    }

    pub fn show(&self, output: &str, name: &str, fraction: f64) {
        if *self.current_monitor.borrow() != output {
            if let Some(m) = super::monitor_for(output) {
                self.window.set_monitor(Some(&m));
            }
            *self.current_monitor.borrow_mut() = output.to_string();
        }
        self.title.set_text(name);
        self.bar.set_value(fraction.clamp(0.0, 1.0));
        self.label.set_text(&format!("{}%", (fraction * 100.0).round()));
        self.window.set_visible(true);
        self.window.present();
        let g = self.generation.get() + 1;
        self.generation.set(g);
        let (gen, w) = (self.generation.clone(), self.window.clone());
        glib::timeout_add_local_once(Duration::from_millis(1400), move || {
            if gen.get() == g {
                w.set_visible(false);
            }
        });
    }
}
