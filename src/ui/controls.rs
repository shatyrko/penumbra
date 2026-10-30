//! Everything you can change about one display, as one widget. The panel shows it folded
//! under each display; the settings window shows it open.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;

use super::widgets::*;
use crate::ddc::*;
use crate::hub::{DdcStatus, DisplayView, Hub};
use crate::layout;
use crate::light::{LightMode, LOOKS};
use crate::model::Transform;

pub struct DisplayControls {
    pub root: gtk::Box,
    hub: Hub,
    name: Rc<RefCell<String>>,
    scale_opts: Rc<RefCell<Vec<f64>>>,
    mode_sizes: Rc<RefCell<Vec<(i32, i32)>>>,
    mode_rates: Rc<RefCell<Vec<crate::model::Mode>>>,
    guard: Rc<Cell<bool>>,

    monitor: gtk::Box,
    status: gtk::Label,
    retry: gtk::Button,
    contrast: SliderRow,
    volume: SliderRow,
    mute: gtk::ToggleButton,
    sharpness: SliderRow,
    input: DropRow,
    preset: DropRow,
    rgb: gtk::Box,
    red: SliderRow,
    green: SliderRow,
    blue: SliderRow,
    input_codes: Rc<RefCell<Vec<u32>>>,
    preset_codes: Rc<RefCell<Vec<u32>>>,

    temp: SliderRow,
    gamma: SliderRow,

    res: DropRow,
    refresh: DropRow,
    scale: DropRow,
    rot: Vec<gtk::ToggleButton>,
    vrr: gtk::Box,
    vrr_switch: gtk::Switch,

    light: gtk::Box,
    light_id: Rc<RefCell<String>>,
    light_mode: DropRow,
    light_look: DropRow,
    light_colour_row: gtk::Box,
    #[allow(deprecated)]
    light_chooser: gtk::ColorChooserWidget,
    light_swatch: Rc<Cell<[u8; 3]>>,
    light_swatch_area: gtk::DrawingArea,
    light_bright: SliderRow,
}

const LIGHT_MODES: [(LightMode, &str); 3] = [(LightMode::Own, "Monitor's own"), (LightMode::Off, "Off"), (LightMode::On, "Colour")];

impl DisplayControls {
    pub fn new(hub: &Hub, output: &str) -> DisplayControls {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
        root.add_css_class("pen-controls");
        let name = Rc::new(RefCell::new(output.to_string()));
        let guard = Rc::new(Cell::new(false));

        // ---- monitor (DDC/CI)
        let monitor = gtk::Box::new(gtk::Orientation::Vertical, 8);
        monitor.append(&section_label("Monitor"));
        let status = gtk::Label::new(None);
        status.set_xalign(0.0);
        status.set_wrap(true);
        status.add_css_class("pen-hint");
        let retry = gtk::Button::with_label("Detect again");
        retry.add_css_class("pill");
        retry.set_halign(gtk::Align::Start);
        {
            let hub = hub.clone();
            retry.connect_clicked(move |_| hub.redetect());
        }
        monitor.append(&status);
        monitor.append(&retry);

        let contrast = SliderRow::new("display-brightness-symbolic", 0.0, 100.0, 1.0, |v| format!("{}%", v.round()));
        let volume = SliderRow::new("audio-volume-high-symbolic", 0.0, 100.0, 1.0, |v| format!("{}%", v.round()));
        let mute = gtk::ToggleButton::new();
        mute.set_icon_name("audio-volume-muted-symbolic");
        mute.add_css_class("flat");
        mute.add_css_class("circular");
        mute.set_tooltip_text(Some("Mute the monitor's speakers"));
        mute.set_valign(gtk::Align::Center);
        volume.root.append(&mute);
        let sharpness = SliderRow::new("find-location-symbolic", 0.0, 100.0, 1.0, |v| format!("{}%", v.round()));
        let input = DropRow::new("Input");
        let preset = DropRow::new("Colour preset");
        let rgb = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let red = SliderRow::new("media-record-symbolic", 0.0, 100.0, 1.0, |v| format!("{}", v.round()));
        let green = SliderRow::new("media-record-symbolic", 0.0, 100.0, 1.0, |v| format!("{}", v.round()));
        let blue = SliderRow::new("media-record-symbolic", 0.0, 100.0, 1.0, |v| format!("{}", v.round()));
        red.root.add_css_class("pen-red");
        green.root.add_css_class("pen-green");
        blue.root.add_css_class("pen-blue");
        rgb.append(&red.root);
        rgb.append(&green.root);
        rgb.append(&blue.root);
        let rgb_reveal = gtk::Revealer::new();
        rgb_reveal.set_child(Some(&rgb));
        let rgb_toggle = gtk::ToggleButton::with_label("RGB gain");
        rgb_toggle.add_css_class("flat");
        rgb_toggle.set_halign(gtk::Align::Start);
        rgb_toggle.bind_property("active", &rgb_reveal, "reveal-child").sync_create().build();

        let contrast_l = labeled("Contrast", &contrast.root);
        let volume_l = labeled("Volume", &volume.root);
        let sharp_l = labeled("Sharpness", &sharpness.root);
        monitor.append(&contrast_l);
        monitor.append(&volume_l);
        monitor.append(&sharp_l);
        monitor.append(&input.root);
        monitor.append(&preset.root);
        let rgb_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
        rgb_box.append(&rgb_toggle);
        rgb_box.append(&rgb_reveal);
        monitor.append(&rgb_box);

        // ---- colour (software)
        let colour = gtk::Box::new(gtk::Orientation::Vertical, 8);
        colour.append(&section_label("Colour (software)"));
        let temp = SliderRow::new("night-light-symbolic", 2000.0, 10000.0, 100.0, |v| format!("{} K", v.round()));
        temp.set_marks(&[(6500.0, "")]);
        let gamma = SliderRow::new("applications-graphics-symbolic", 0.5, 2.0, 0.05, |v| format!("{v:.2}"));
        let reset = icon_button("edit-undo-symbolic", "Back to neutral colour");
        temp.root.append(&reset);
        colour.append(&labeled("Colour temperature", &temp.root));
        colour.append(&labeled("Gamma", &gamma.root));

        // ---- display
        let layout_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
        layout_box.append(&section_label("Display"));
        let res = DropRow::new("Resolution");
        let refresh = DropRow::new("Refresh rate");
        let scale = DropRow::new("Looks like");
        let rot_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        rot_row.add_css_class("pen-row");
        let rl = gtk::Label::new(Some("Rotation"));
        rl.set_xalign(0.0);
        rl.set_hexpand(true);
        rot_row.append(&rl);
        let chips = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        chips.add_css_class("linked");
        let mut rot = Vec::new();
        let mut first: Option<gtk::ToggleButton> = None;
        for t in Transform::ROTATIONS {
            let b = gtk::ToggleButton::with_label(&format!("{}°", t.degrees()));
            if let Some(f) = &first {
                b.set_group(Some(f));
            } else {
                first = Some(b.clone());
            }
            chips.append(&b);
            rot.push(b);
        }
        rot_row.append(&chips);
        let vrr = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        vrr.add_css_class("pen-row");
        let vl = gtk::Label::new(Some("Adaptive sync (VRR)"));
        vl.set_xalign(0.0);
        vl.set_hexpand(true);
        let vrr_switch = gtk::Switch::new();
        vrr_switch.set_valign(gtk::Align::Center);
        vrr.append(&vl);
        vrr.append(&vrr_switch);
        layout_box.append(&res.root);
        layout_box.append(&refresh.root);
        layout_box.append(&scale.root);
        layout_box.append(&rot_row);
        layout_box.append(&vrr);

        // ---- the monitor's light (LampArray over USB)
        let light = gtk::Box::new(gtk::Orientation::Vertical, 8);
        light.append(&section_label("Light"));
        let light_mode = DropRow::new("Light");
        light_mode.set(&LIGHT_MODES.iter().map(|m| m.1.to_string()).collect::<Vec<_>>(), Some(0));
        let light_look = DropRow::new("Colour");
        light_look.set(&LOOKS.iter().map(|l| l.1.to_string()).collect::<Vec<_>>(), Some(0));
        let light_colour_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        light_colour_row.add_css_class("pen-row");
        let cl = gtk::Label::new(Some("Custom colour"));
        cl.set_xalign(0.0);
        cl.set_hexpand(true);
        // The picker is a popup beside the panel (a dialog window would open full size on a
        // tiling desktop).
        let light_swatch = Rc::new(Cell::new([255u8, 255, 255]));
        let light_swatch_area = gtk::DrawingArea::new();
        light_swatch_area.set_content_width(28);
        light_swatch_area.set_content_height(18);
        {
            let sw = light_swatch.clone();
            light_swatch_area.set_draw_func(move |_, cr, w, h| {
                let [r, g, b] = sw.get().map(|x| x as f64 / 255.0);
                let (w, h, rad) = (w as f64, h as f64, 5.0);
                cr.new_sub_path();
                cr.arc(w - rad, rad, rad, -std::f64::consts::FRAC_PI_2, 0.0);
                cr.arc(w - rad, h - rad, rad, 0.0, std::f64::consts::FRAC_PI_2);
                cr.arc(rad, h - rad, rad, std::f64::consts::FRAC_PI_2, std::f64::consts::PI);
                cr.arc(rad, rad, rad, std::f64::consts::PI, 1.5 * std::f64::consts::PI);
                cr.close_path();
                cr.set_source_rgb(r, g, b);
                let _ = cr.fill_preserve();
                cr.set_source_rgba(0.5, 0.5, 0.5, 0.5);
                cr.set_line_width(1.0);
                let _ = cr.stroke();
            });
        }
        #[allow(deprecated)]
        let light_chooser = {
            let c = gtk::ColorChooserWidget::new();
            c.set_use_alpha(false);
            c
        };
        let pop = gtk::Popover::new();
        pop.set_child(Some(&light_chooser));
        pop.set_position(gtk::PositionType::Left);
        let light_colour = gtk::MenuButton::new();
        light_colour.set_child(Some(&light_swatch_area));
        light_colour.set_popover(Some(&pop));
        light_colour.set_tooltip_text(Some("Pick a colour"));
        light_colour.set_valign(gtk::Align::Center);
        light_colour_row.append(&cl);
        light_colour_row.append(&light_colour);
        let light_bright = SliderRow::new("display-brightness-symbolic", 0.0, 100.0, 1.0, |v| format!("{}%", v.round()));
        let lhint = gtk::Label::new(Some("Works while the light is switched on in the monitor's own menu."));
        lhint.set_xalign(0.0);
        lhint.set_wrap(true);
        lhint.add_css_class("pen-hint");
        light.append(&light_mode.root);
        light.append(&light_look.root);
        light.append(&light_colour_row);
        light.append(&labeled("Brightness", &light_bright.root));
        light.append(&lhint);
        light.set_visible(false);

        root.append(&monitor);
        root.append(&light);
        root.append(&colour);
        root.append(&layout_box);

        let c = DisplayControls {
            root,
            hub: hub.clone(),
            name,
            scale_opts: Rc::new(RefCell::new(Vec::new())),
            mode_sizes: Rc::new(RefCell::new(Vec::new())),
            mode_rates: Rc::new(RefCell::new(Vec::new())),
            guard,
            monitor,
            status,
            retry,
            contrast,
            volume,
            mute,
            sharpness,
            input,
            preset,
            rgb: rgb_box,
            red,
            green,
            blue,
            input_codes: Rc::new(RefCell::new(Vec::new())),
            preset_codes: Rc::new(RefCell::new(Vec::new())),
            temp,
            gamma,
            res,
            refresh,
            scale,
            rot,
            vrr,
            vrr_switch,
            light,
            light_id: Rc::new(RefCell::new(String::new())),
            light_mode,
            light_look,
            light_colour_row,
            light_chooser,
            light_swatch,
            light_swatch_area,
            light_bright,
        };
        c.wire(reset);
        c.wire_light();
        c
    }

    fn wire(&self, reset: gtk::Button) {
        let ddc_slider = |row: &SliderRow, code: u8| {
            let hub = self.hub.clone();
            let name = self.name.clone();
            row.connect_changed(move |v| hub.set_ddc(&name.borrow(), code, v.round() as u32));
        };
        ddc_slider(&self.contrast, VCP_CONTRAST);
        ddc_slider(&self.volume, VCP_VOLUME);
        ddc_slider(&self.sharpness, VCP_SHARPNESS);
        ddc_slider(&self.red, VCP_GAIN_R);
        ddc_slider(&self.green, VCP_GAIN_G);
        ddc_slider(&self.blue, VCP_GAIN_B);
        {
            let (hub, name, guard) = (self.hub.clone(), self.name.clone(), self.guard.clone());
            self.mute.connect_toggled(move |b| {
                if !guard.get() {
                    hub.set_ddc(&name.borrow(), VCP_MUTE, if b.is_active() { 1 } else { 2 });
                }
            });
        }
        {
            let (hub, name, codes) = (self.hub.clone(), self.name.clone(), self.input_codes.clone());
            self.input.connect_selected(move |i| {
                if let Some(c) = codes.borrow().get(i) {
                    hub.set_ddc(&name.borrow(), VCP_INPUT, *c);
                }
            });
        }
        {
            let (hub, name, codes) = (self.hub.clone(), self.name.clone(), self.preset_codes.clone());
            self.preset.connect_selected(move |i| {
                if let Some(c) = codes.borrow().get(i) {
                    hub.set_ddc(&name.borrow(), VCP_PRESET, *c);
                }
            });
        }
        {
            let (hub, name) = (self.hub.clone(), self.name.clone());
            self.temp.connect_changed(move |v| hub.set_temperature(&name.borrow(), v));
        }
        {
            let (hub, name) = (self.hub.clone(), self.name.clone());
            self.gamma.connect_changed(move |v| hub.set_gamma_curve(&name.borrow(), v));
        }
        {
            let (hub, name) = (self.hub.clone(), self.name.clone());
            reset.connect_clicked(move |_| hub.reset_colour(&name.borrow()));
        }
        // resolution: keep the refresh rate when the new size has it
        {
            let (hub, name, sizes) = (self.hub.clone(), self.name.clone(), self.mode_sizes.clone());
            self.res.connect_selected(move |i| {
                let Some(size) = sizes.borrow().get(i).copied() else { return };
                let Some(v) = hub.view(&name.borrow()) else { return };
                let cur_hz = v.output.mode.as_ref().map(|m| m.refresh_mhz).unwrap_or(60000);
                let rates = layout::rates(&v.output.modes, size);
                let best = rates.iter().min_by_key(|m| (m.refresh_mhz - cur_hz).abs()).cloned();
                if let Some(m) = best {
                    hub.set_mode(&name.borrow(), &m);
                }
            });
        }
        {
            let (hub, name, rates) = (self.hub.clone(), self.name.clone(), self.mode_rates.clone());
            self.refresh.connect_selected(move |i| {
                if let Some(m) = rates.borrow().get(i) {
                    hub.set_mode(&name.borrow(), m);
                }
            });
        }
        {
            let (hub, name, opts) = (self.hub.clone(), self.name.clone(), self.scale_opts.clone());
            self.scale.connect_selected(move |i| {
                if let Some(s) = opts.borrow().get(i) {
                    hub.set_scale(&name.borrow(), *s);
                }
            });
        }
        for (i, b) in self.rot.iter().enumerate() {
            let (hub, name, guard) = (self.hub.clone(), self.name.clone(), self.guard.clone());
            b.connect_toggled(move |b| {
                if guard.get() || !b.is_active() {
                    return;
                }
                let Some(v) = hub.view(&name.borrow()) else { return };
                hub.set_transform(&name.borrow(), Transform::from_parts(Transform::ROTATIONS[i].degrees(), v.output.transform.flipped()));
            });
        }
        {
            let (hub, name, guard) = (self.hub.clone(), self.name.clone(), self.guard.clone());
            self.vrr_switch.connect_state_set(move |_, on| {
                if !guard.get() {
                    hub.set_vrr(&name.borrow(), on);
                }
                glib::Propagation::Proceed
            });
        }
    }

    fn wire_light(&self) {
        let set = {
            let (hub, id) = (self.hub.clone(), self.light_id.clone());
            Rc::new(move |f: &dyn Fn(&mut crate::light::LightSetting)| {
                let id = id.borrow().clone();
                if !id.is_empty() {
                    if let Err(e) = hub.set_light(&id, |s| f(s)) {
                        hub.error(format!("Light: {e}"));
                    }
                }
            })
        };
        {
            let (set, guard) = (set.clone(), self.guard.clone());
            self.light_mode.connect_selected(move |i| {
                if !guard.get() {
                    if let Some((m, _)) = LIGHT_MODES.get(i) {
                        set(&|s| s.mode = *m);
                    }
                }
            });
        }
        {
            let (set, guard) = (set.clone(), self.guard.clone());
            self.light_look.connect_selected(move |i| {
                if !guard.get() {
                    if let Some(l) = LOOKS.get(i) {
                        set(&|s| {
                            s.look = l.0.to_string();
                            s.mode = LightMode::On;
                        });
                    }
                }
            });
        }
        {
            let (set, guard) = (set.clone(), self.guard.clone());
            #[allow(deprecated)]
            self.light_chooser.connect_rgba_notify(move |b| {
                if guard.get() {
                    return;
                }
                let c = gtk::prelude::ColorChooserExt::rgba(b);
                let rgb = [c.red(), c.green(), c.blue()].map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u8);
                set(&|s| {
                    s.look = "custom".into();
                    s.colour = rgb;
                    s.mode = LightMode::On;
                });
            });
        }
        {
            let set = set.clone();
            self.light_bright.connect_changed(move |v| {
                set(&|s| {
                    s.brightness = v / 100.0;
                    if s.mode == LightMode::Own {
                        s.mode = LightMode::On;
                    }
                })
            });
        }
    }

    fn update_light(&self, v: &DisplayView) {
        let Some(l) = &v.light else {
            self.light.set_visible(false);
            return;
        };
        self.light.set_visible(true);
        *self.light_id.borrow_mut() = l.id.clone();
        let s = &l.setting;
        let mode = LIGHT_MODES.iter().position(|m| m.0 == s.mode).unwrap_or(0);
        self.light_mode.set(&LIGHT_MODES.iter().map(|m| m.1.to_string()).collect::<Vec<_>>(), Some(mode));
        let look = LOOKS.iter().position(|x| x.0 == s.look).unwrap_or(LOOKS.len() - 1);
        self.light_look.set(&LOOKS.iter().map(|x| x.1.to_string()).collect::<Vec<_>>(), Some(look));
        let on = s.mode == LightMode::On;
        self.light_look.root.set_visible(on);
        self.light_colour_row.set_visible(on && s.look == "custom");
        self.light_bright.root.parent().unwrap().set_visible(on);
        let [r, g, b] = s.colour.map(|x| x as f32 / 255.0);
        #[allow(deprecated)]
        gtk::prelude::ColorChooserExt::set_rgba(&self.light_chooser, &gtk::gdk::RGBA::new(r, g, b, 1.0));
        self.light_swatch.set(s.colour);
        self.light_swatch_area.queue_draw();
        self.light_bright.set_quiet(s.brightness * 100.0);
    }

    pub fn set_output(&self, name: &str) {
        *self.name.borrow_mut() = name.to_string();
    }

    pub fn update(&self, v: &DisplayView) {
        self.guard.set(true);
        // -- monitor
        match &v.ddc {
            Some(d) => {
                self.monitor.set_visible(true);
                let (msg, retry) = match &d.status {
                    DdcStatus::Detecting | DdcStatus::Reading => ("Reading the monitor…".to_string(), false),
                    DdcStatus::Failed(e) => (e.clone(), true),
                    DdcStatus::Ready => (String::new(), false),
                };
                self.status.set_text(&msg);
                self.status.set_visible(!msg.is_empty());
                self.retry.set_visible(retry);
                let show = |w: &gtk::Widget, on: bool| w.set_visible(on);
                let ready = d.status == DdcStatus::Ready;
                show(self.contrast.root.parent().unwrap().upcast_ref(), ready && d.has(VCP_CONTRAST));
                show(self.volume.root.parent().unwrap().upcast_ref(), ready && d.has(VCP_VOLUME));
                show(self.sharpness.root.parent().unwrap().upcast_ref(), ready && d.has(VCP_SHARPNESS));
                self.input.root.set_visible(ready && d.has(VCP_INPUT));
                self.preset.root.set_visible(ready && d.has(VCP_PRESET));
                self.rgb.set_visible(ready && d.has(VCP_GAIN_R) && d.has(VCP_GAIN_G) && d.has(VCP_GAIN_B));
                let set = |row: &SliderRow, code: u8| {
                    if let Some(x) = d.values.get(&code) {
                        let max = x.max.max(1) as f64;
                        row.scale.adjustment().set_upper(max);
                        row.set_quiet(x.current as f64);
                    }
                };
                set(&self.contrast, VCP_CONTRAST);
                set(&self.volume, VCP_VOLUME);
                set(&self.sharpness, VCP_SHARPNESS);
                set(&self.red, VCP_GAIN_R);
                set(&self.green, VCP_GAIN_G);
                set(&self.blue, VCP_GAIN_B);
                if let Some(m) = d.values.get(&VCP_MUTE) {
                    self.mute.set_active(m.current == 1);
                }
                self.mute.set_visible(d.has(VCP_MUTE));
                for (code, row, codes) in [(VCP_INPUT, &self.input, &self.input_codes), (VCP_PRESET, &self.preset, &self.preset_codes)] {
                    if !d.has(code) {
                        continue;
                    }
                    let choices = d.choices(code);
                    let cur = d.values.get(&code).map(|x| x.current);
                    *codes.borrow_mut() = choices.iter().map(|(c, _)| *c).collect();
                    let labels: Vec<String> = choices.iter().map(|(_, n)| n.clone()).collect();
                    row.set(&labels, choices.iter().position(|(c, _)| Some(*c) == cur));
                }
            }
            None => self.monitor.set_visible(false),
        }

        self.update_light(v);

        // -- colour
        self.temp.set_quiet(v.gamma.temperature);
        self.gamma.set_quiet(v.gamma.gamma);

        // -- display
        let o = &v.output;
        let sizes = layout::sizes(&o.modes);
        let cur_size = o.mode.as_ref().map(|m| (m.width, m.height));
        *self.mode_sizes.borrow_mut() = sizes.clone();
        self.res.set(&sizes.iter().map(|(w, h)| format!("{w} × {h}")).collect::<Vec<_>>(), cur_size.and_then(|s| sizes.iter().position(|x| *x == s)));
        let rates = cur_size.map(|s| layout::rates(&o.modes, s)).unwrap_or_default();
        *self.mode_rates.borrow_mut() = rates.clone();
        self.refresh.set(&rates.iter().map(|m| m.hz_label()).collect::<Vec<_>>(), o.mode.as_ref().and_then(|c| rates.iter().position(|m| m.refresh_mhz == c.refresh_mhz)));
        if let Some(m) = &o.mode {
            let (opts, idx) = layout::scale_options((m.width, m.height), o.scale);
            *self.scale_opts.borrow_mut() = opts.iter().map(|(s, _)| *s).collect();
            self.scale.set(&opts.into_iter().map(|(_, l)| l).collect::<Vec<_>>(), Some(idx));
        }
        for (i, b) in self.rot.iter().enumerate() {
            b.set_active(Transform::ROTATIONS[i].degrees() == o.transform.degrees());
        }
        self.vrr.set_visible(o.vrr.is_some());
        self.vrr_switch.set_active(o.vrr.unwrap_or(false));
        self.guard.set(false);
    }
}

fn labeled(label: &str, row: &gtk::Box) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let l = gtk::Label::new(Some(label));
    l.set_xalign(0.0);
    l.add_css_class("pen-row-label");
    b.append(&l);
    b.append(row);
    b
}
