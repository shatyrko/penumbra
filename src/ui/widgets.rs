//! Small widgets used everywhere: sliders that do not fight the hub, drop-downs that can be
//! refilled without firing their own signal.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::prelude::*;

type Cb<T> = Rc<RefCell<Option<Box<dyn Fn(T)>>>>;

/// A slider with an icon in front and a value on the right.
#[derive(Clone)]
pub struct SliderRow {
    pub root: gtk::Box,
    pub scale: gtk::Scale,
    value: gtk::Label,
    format: Rc<dyn Fn(f64) -> String>,
    guard: Rc<Cell<bool>>,
    last_user: Rc<Cell<Option<Instant>>>,
    cb: Cb<f64>,
}

impl SliderRow {
    pub fn new(icon: &str, min: f64, max: f64, step: f64, format: impl Fn(f64) -> String + 'static) -> SliderRow {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        root.add_css_class("pen-row");
        let img = gtk::Image::from_icon_name(icon);
        img.add_css_class("pen-row-icon");
        let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, min, max, step);
        scale.set_hexpand(true);
        scale.set_draw_value(false);
        scale.add_css_class("pen-slider");
        let value = gtk::Label::new(None);
        value.add_css_class("pen-value");
        value.set_width_chars(6);
        value.set_xalign(1.0);
        // The mouse wheel never moves a slider (too easy to change a value while scrolling the
        // panel): it scrolls whatever the slider sits in instead.
        let wheel = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
        wheel.set_propagation_phase(gtk::PropagationPhase::Capture);
        wheel.connect_scroll(|c, _dx, dy| {
            let sw = c.widget().and_then(|w| w.ancestor(gtk::ScrolledWindow::static_type())).and_downcast::<gtk::ScrolledWindow>();
            if let Some(sw) = sw {
                let adj = sw.vadjustment();
                adj.set_value(adj.value() + dy * adj.step_increment().max(40.0));
            }
            glib::Propagation::Stop
        });
        scale.add_controller(wheel);
        root.append(&img);
        root.append(&scale);
        root.append(&value);
        let format: Rc<dyn Fn(f64) -> String> = Rc::new(format);
        let row = SliderRow { root, scale: scale.clone(), value, format, guard: Rc::new(Cell::new(false)), last_user: Rc::new(Cell::new(None)), cb: Rc::new(RefCell::new(None)) };
        let r2 = row.clone();
        scale.connect_value_changed(move |s| {
            let v = s.value();
            r2.value.set_text(&(r2.format)(v));
            if r2.guard.get() {
                return;
            }
            r2.last_user.set(Some(Instant::now()));
            if let Some(cb) = r2.cb.borrow().as_ref() {
                cb(v);
            }
        });
        row.value.set_text(&(row.format)(scale.value()));
        row
    }

    pub fn connect_changed(&self, f: impl Fn(f64) + 'static) {
        *self.cb.borrow_mut() = Some(Box::new(f));
    }

    /// Set from the outside (hub updates). Ignored for a moment after the user touched it,
    /// so that a slow monitor's old answers do not pull the slider back.
    pub fn set_quiet(&self, v: f64) {
        if let Some(t) = self.last_user.get() {
            if t.elapsed() < Duration::from_millis(900) {
                return;
            }
        }
        self.guard.set(true);
        self.scale.set_value(v);
        self.guard.set(false);
        self.value.set_text(&(self.format)(v));
    }

    pub fn set_marks(&self, marks: &[(f64, &str)]) {
        self.scale.clear_marks();
        for (v, t) in marks {
            self.scale.add_mark(*v, gtk::PositionType::Bottom, if t.is_empty() { None } else { Some(t) });
        }
    }
}

/// A label with a drop-down on the right.
#[derive(Clone)]
pub struct DropRow {
    pub root: gtk::Box,
    pub dd: gtk::DropDown,
    items: Rc<RefCell<Vec<String>>>,
    guard: Rc<Cell<bool>>,
    cb: Cb<usize>,
}

impl DropRow {
    pub fn new(label: &str) -> DropRow {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        root.add_css_class("pen-row");
        let l = gtk::Label::new(Some(label));
        l.set_xalign(0.0);
        l.set_hexpand(true);
        l.add_css_class("pen-row-label");
        let dd = gtk::DropDown::from_strings(&[]);
        dd.add_css_class("pen-dropdown");
        dd.set_valign(gtk::Align::Center);
        root.append(&l);
        root.append(&dd);
        let row = DropRow { root, dd: dd.clone(), items: Rc::new(RefCell::new(Vec::new())), guard: Rc::new(Cell::new(false)), cb: Rc::new(RefCell::new(None)) };
        let r2 = row.clone();
        dd.connect_selected_notify(move |d| {
            if r2.guard.get() {
                return;
            }
            if let Some(cb) = r2.cb.borrow().as_ref() {
                cb(d.selected() as usize);
            }
        });
        row
    }

    pub fn connect_selected(&self, f: impl Fn(usize) + 'static) {
        *self.cb.borrow_mut() = Some(Box::new(f));
    }

    /// Refill only when the list changed; then select.
    pub fn set(&self, items: &[String], selected: Option<usize>) {
        self.guard.set(true);
        if *self.items.borrow() != items {
            let refs: Vec<&str> = items.iter().map(|s| s.as_str()).collect();
            self.dd.set_model(Some(&gtk::StringList::new(&refs)));
            *self.items.borrow_mut() = items.to_vec();
        }
        match selected {
            Some(i) if (i as u32) < items.len() as u32 => self.dd.set_selected(i as u32),
            _ => self.dd.set_selected(gtk::INVALID_LIST_POSITION),
        }
        self.guard.set(false);
    }
}

pub fn section_label(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(&text.to_uppercase()));
    l.set_xalign(0.0);
    l.add_css_class("pen-section");
    l
}

pub fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    let b = gtk::Button::from_icon_name(icon);
    b.add_css_class("flat");
    b.add_css_class("circular");
    b.set_tooltip_text(Some(tooltip));
    b.set_valign(gtk::Align::Center);
    b
}
