//! The arrangement canvas: screens as tiles, dragged into place with magnetic edges.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gtk::prelude::*;

use crate::hub::{DisplayView, Hub};
use crate::layout::{self, Rect};

pub struct Arrange {
    pub root: gtk::Box,
    fixed: gtk::Fixed,
    hub: Hub,
    tiles: RefCell<HashMap<String, gtk::Box>>,
    rects: Rc<RefCell<Vec<(String, Rect)>>>,
    scale: Rc<Cell<f64>>,
    origin: Rc<Cell<(i32, i32)>>,
    dragging: Rc<Cell<bool>>,
    hint: gtk::Label,
    on_toast: Rc<RefCell<Option<Box<dyn Fn(&str)>>>>,
}

const HEIGHT: i32 = 300;
const PAD: f64 = 28.0;

impl Arrange {
    pub fn new(hub: &Hub) -> Rc<Arrange> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let fixed = gtk::Fixed::new();
        fixed.set_size_request(-1, HEIGHT);
        fixed.add_css_class("pen-canvas");
        fixed.set_hexpand(true);
        let hint = gtk::Label::new(Some("Drag a screen next to another. Edges snap together. Screens must touch."));
        hint.add_css_class("pen-hint");
        hint.set_xalign(0.0);
        root.append(&fixed);
        root.append(&hint);
        let a = Rc::new(Arrange {
            root,
            fixed: fixed.clone(),
            hub: hub.clone(),
            tiles: RefCell::new(HashMap::new()),
            rects: Rc::new(RefCell::new(Vec::new())),
            scale: Rc::new(Cell::new(0.1)),
            origin: Rc::new(Cell::new((0, 0))),
            dragging: Rc::new(Cell::new(false)),
            hint,
            on_toast: Rc::new(RefCell::new(None)),
        });
        // Re-fit when the width changes.
        let a2 = a.clone();
        let last_w = Rc::new(Cell::new(0));
        fixed.add_tick_callback(move |f, _| {
            let w = f.width();
            if w != last_w.get() && w > 0 {
                last_w.set(w);
                a2.fit();
            }
            glib::ControlFlow::Continue
        });
        a
    }

    pub fn connect_toast(&self, f: impl Fn(&str) + 'static) {
        *self.on_toast.borrow_mut() = Some(Box::new(f));
    }

    /// Compute the scale and origin so that every screen fits, then place the tiles.
    fn fit(&self) {
        let rects = self.rects.borrow();
        if rects.is_empty() {
            return;
        }
        let min_x = rects.iter().map(|(_, r)| r.x).min().unwrap();
        let min_y = rects.iter().map(|(_, r)| r.y).min().unwrap();
        let max_x = rects.iter().map(|(_, r)| r.right()).max().unwrap();
        let max_y = rects.iter().map(|(_, r)| r.bottom()).max().unwrap();
        let (tw, th) = ((max_x - min_x).max(1) as f64, (max_y - min_y).max(1) as f64);
        // Room around the screens, so that one can be dragged beside the group.
        let (aw, ah) = ((self.fixed.width() as f64 - 2.0 * PAD).max(100.0), HEIGHT as f64 - 2.0 * PAD);
        let s = (aw / (tw * 1.6)).min(ah / (th * 1.5)).min(0.25);
        self.scale.set(s);
        let ox = (self.fixed.width() as f64 - tw * s) / 2.0;
        let oy = (HEIGHT as f64 - th * s) / 2.0;
        self.origin.set((min_x - (ox / s).round() as i32, min_y - (oy / s).round() as i32));
        drop(rects);
        self.place_all();
    }

    fn to_px(&self, x: i32, y: i32) -> (f64, f64) {
        let (ox, oy) = self.origin.get();
        let s = self.scale.get();
        ((x - ox) as f64 * s, (y - oy) as f64 * s)
    }

    fn to_logical(&self, px: f64, py: f64) -> (i32, i32) {
        let (ox, oy) = self.origin.get();
        let s = self.scale.get();
        ((px / s).round() as i32 + ox, (py / s).round() as i32 + oy)
    }

    fn place_all(&self) {
        let tiles = self.tiles.borrow();
        let s = self.scale.get();
        for (name, r) in self.rects.borrow().iter() {
            if let Some(t) = tiles.get(name) {
                let (px, py) = self.to_px(r.x, r.y);
                t.set_size_request((r.w as f64 * s).round() as i32, (r.h as f64 * s).round() as i32);
                self.fixed.move_(t, px, py);
            }
        }
    }

    pub fn sync(self: &Rc<Self>, views: &[DisplayView]) {
        if self.dragging.get() {
            return;
        }
        let enabled: Vec<&DisplayView> = views.iter().filter(|v| v.output.enabled).collect();
        {
            let mut tiles = self.tiles.borrow_mut();
            let gone: Vec<String> = tiles.keys().filter(|k| !enabled.iter().any(|v| &v.name == *k)).cloned().collect();
            for g in gone {
                if let Some(t) = tiles.remove(&g) {
                    self.fixed.remove(&t);
                }
            }
        }
        *self.rects.borrow_mut() = enabled.iter().map(|v| (v.name.clone(), layout::rect_of(&v.output))).collect();
        for v in &enabled {
            let exists = self.tiles.borrow().contains_key(&v.name);
            if !exists {
                let t = self.make_tile(&v.name);
                self.fixed.put(&t, 0.0, 0.0);
                self.tiles.borrow_mut().insert(v.name.clone(), t);
            }
            let tiles = self.tiles.borrow();
            let t = &tiles[&v.name];
            let title = t.first_child().and_downcast::<gtk::Label>().unwrap();
            title.set_text(&v.display_name());
            let sub = title.next_sibling().and_downcast::<gtk::Label>().unwrap();
            let (lw, lh) = v.output.logical_size();
            sub.set_text(&format!("{} · {}×{}", v.name, lw, lh));
        }
        self.fit();
    }

    fn make_tile(self: &Rc<Self>, name: &str) -> gtk::Box {
        let t = gtk::Box::new(gtk::Orientation::Vertical, 2);
        t.add_css_class("pen-screen");
        t.set_valign(gtk::Align::Center);
        t.set_halign(gtk::Align::Center);
        let title = gtk::Label::new(None);
        title.add_css_class("pen-screen-title");
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let sub = gtk::Label::new(None);
        sub.add_css_class("pen-screen-sub");
        sub.set_ellipsize(gtk::pango::EllipsizeMode::End);
        t.append(&title);
        t.append(&sub);
        t.set_vexpand(false);
        t.set_hexpand(false);
        t.set_overflow(gtk::Overflow::Hidden);
        let drag = gtk::GestureDrag::new();
        let start = Rc::new(Cell::new((0.0f64, 0.0f64)));
        {
            let (a, name, start) = (self.clone(), name.to_string(), start.clone());
            drag.connect_drag_begin(move |_, _, _| {
                a.dragging.set(true);
                let r = a.rects.borrow().iter().find(|(n, _)| *n == name).map(|(_, r)| *r);
                if let Some(r) = r {
                    start.set(a.to_px(r.x, r.y));
                }
            });
        }
        {
            let (a, name, start) = (self.clone(), name.to_string(), start.clone());
            drag.connect_drag_update(move |_, dx, dy| {
                let (sx, sy) = start.get();
                let (lx, ly) = a.to_logical(sx + dx, sy + dy);
                let (snapped, _) = a.snapped(&name, lx, ly);
                let (px, py) = a.to_px(snapped.x, snapped.y);
                if let Some(t) = a.tiles.borrow().get(&name) {
                    a.fixed.move_(t, px, py);
                    t.add_css_class("dragging");
                }
            });
        }
        {
            let (a, name, start) = (self.clone(), name.to_string(), start.clone());
            drag.connect_drag_end(move |_, dx, dy| {
                a.dragging.set(false);
                if let Some(t) = a.tiles.borrow().get(&name) {
                    t.remove_css_class("dragging");
                }
                let (sx, sy) = start.get();
                let (lx, ly) = a.to_logical(sx + dx, sy + dy);
                let (snapped, others) = a.snapped(&name, lx, ly);
                let mut all = others;
                all.push(snapped);
                if dx.abs() < 3.0 && dy.abs() < 3.0 {
                    a.place_all();
                    return;
                }
                if layout::connected(&all) {
                    a.hub.set_position(&name, snapped.x, snapped.y);
                } else {
                    a.place_all();
                    if let Some(f) = a.on_toast.borrow().as_ref() {
                        f("Screens must touch each other.");
                    }
                }
            });
        }
        t.add_controller(drag);
        t
    }

    fn snapped(&self, name: &str, lx: i32, ly: i32) -> (Rect, Vec<Rect>) {
        let rects = self.rects.borrow();
        let me = rects.iter().find(|(n, _)| n == name).map(|(_, r)| *r).unwrap_or(Rect { x: 0, y: 0, w: 100, h: 100 });
        let others: Vec<Rect> = rects.iter().filter(|(n, _)| n != name).map(|(_, r)| *r).collect();
        let tol = (28.0 / self.scale.get()).round() as i32;
        let r = layout::snap(Rect { x: lx, y: ly, ..me }, &others, tol);
        (r, others)
    }

    #[allow(dead_code)]
    fn hint(&self) -> &gtk::Label {
        &self.hint
    }
}
