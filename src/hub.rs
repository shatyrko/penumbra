//! The hub: one model of every display, merged from Wayland outputs, DDC/CI, the built-in
//! backlight, gamma ramps and what was remembered. The interface and the command line only
//! talk to this.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::backlight::Backlight;
use crate::brightness;
use crate::compositor;
use crate::ddc::*;
use crate::model::*;
use crate::queue::JobQueue;
use crate::store::{Profile, Settings, Store, VirtualDisplay};
use crate::wl::{self, WlCmd, WlEvent, WlHandle};

#[derive(Debug, Clone)]
pub enum HubEvent {
    Changed,
    Notice(String),
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DdcStatus {
    Detecting,
    Reading,
    Ready,
    Failed(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Control {
    Ddc,
    Backlight,
    Software,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DdcView {
    pub bus: u32,
    pub status: DdcStatus,
    pub caps: Option<DdcCaps>,
    pub values: BTreeMap<u8, VcpValue>,
}

impl DdcView {
    pub fn has(&self, code: u8) -> bool {
        self.values.contains_key(&code)
    }

    /// The choices of a list feature: what the monitor reported, else a sensible default.
    pub fn choices(&self, code: u8) -> Vec<(u32, String)> {
        let mut v = self.caps.as_ref().and_then(|c| c.values.get(&code).cloned()).unwrap_or_default();
        if v.is_empty() {
            v = default_choices(code);
        }
        if let Some(cur) = self.values.get(&code) {
            if !v.iter().any(|(x, _)| *x == cur.current) {
                v.push((cur.current, format!("Value 0x{:02X}", cur.current)));
            }
        }
        v
    }
}

pub fn default_choices(code: u8) -> Vec<(u32, String)> {
    let l = |a: &[(u32, &str)]| a.iter().map(|(v, n)| (*v, n.to_string())).collect();
    match code {
        VCP_INPUT => l(&[(0x01, "VGA-1"), (0x03, "DVI-1"), (0x0f, "DisplayPort-1"), (0x10, "DisplayPort-2"), (0x11, "HDMI-1"), (0x12, "HDMI-2"), (0x1b, "USB-C")]),
        VCP_PRESET => l(&[(0x01, "sRGB"), (0x02, "Native"), (0x04, "5000 K"), (0x05, "6500 K"), (0x06, "7500 K"), (0x08, "9300 K"), (0x0b, "User 1")]),
        VCP_POWER => l(&[(0x01, "On"), (0x04, "Off (standby)"), (0x05, "Off (hard)")]),
        _ => Vec::new(),
    }
}

pub fn feature_name(code: u8) -> &'static str {
    match code {
        VCP_BRIGHTNESS => "Brightness",
        VCP_CONTRAST => "Contrast",
        VCP_PRESET => "Colour preset",
        VCP_GAIN_R => "Red gain",
        VCP_GAIN_G => "Green gain",
        VCP_GAIN_B => "Blue gain",
        VCP_INPUT => "Input source",
        VCP_VOLUME => "Volume",
        VCP_SHARPNESS => "Sharpness",
        VCP_MUTE => "Mute",
        VCP_POWER => "Power",
        _ => "Feature",
    }
}

/// "brightness", "contrast", "volume", "input", "0x62" → VCP code.
pub fn parse_feature(s: &str) -> Option<u8> {
    let l = s.to_ascii_lowercase();
    Some(match l.as_str() {
        "brightness" => VCP_BRIGHTNESS,
        "contrast" => VCP_CONTRAST,
        "preset" | "colour" | "color" | "colour-preset" | "color-preset" => VCP_PRESET,
        "red" | "red-gain" => VCP_GAIN_R,
        "green" | "green-gain" => VCP_GAIN_G,
        "blue" | "blue-gain" => VCP_GAIN_B,
        "input" | "source" => VCP_INPUT,
        "volume" => VCP_VOLUME,
        "sharpness" => VCP_SHARPNESS,
        "mute" => VCP_MUTE,
        "power" => VCP_POWER,
        _ => {
            let h = l.trim_start_matches("0x");
            return u8::from_str_radix(h, 16).ok();
        }
    })
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DisplayView {
    pub name: String,
    pub id: String,
    pub title: String,
    pub nickname: Option<String>,
    pub output: OutputInfo,
    pub control: Control,
    /// Combined slider position 0..1.
    pub brightness: f64,
    pub gamma: Gamma,
    pub ddc: Option<DdcView>,
    pub hidden: bool,
}

impl DisplayView {
    pub fn display_name(&self) -> String {
        self.nickname.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| self.title.clone())
    }
}

struct Live {
    display: DdcDisplay,
    status: DdcStatus,
    caps: Option<DdcCaps>,
    values: HashMap<u8, VcpValue>,
    restored: bool,
}

#[derive(Default)]
struct State {
    outputs: Vec<OutputInfo>,
    got_outputs: bool,
    ddc: HashMap<u32, Live>,
    ddc_detected: bool,
    gamma: HashMap<String, Gamma>,
    slider: HashMap<String, f64>,
    internal_raw: Option<u32>,
    missing: Vec<String>,
    touched: HashSet<String>,
    seen: HashSet<String>,
}

enum BusJob {
    Set(u8, u32),
    ReadAll,
    Refresh,
}

type Done = Box<dyn FnOnce(Result<(), String>) + Send>;

struct Inner {
    state: Mutex<State>,
    cv: Condvar,
    wl: Option<WlHandle>,
    backend: Arc<dyn DdcBackend>,
    store: Mutex<Store>,
    dirty: AtomicBool,
    queues: Mutex<HashMap<u32, Arc<JobQueue<BusJob>>>>,
    backlight: Option<Backlight>,
    backlight_q: Mutex<Option<Arc<JobQueue<f64>>>>,
    last_written_raw: Mutex<Option<u32>>,
    events: async_channel::Sender<HubEvent>,
    token: AtomicU64,
    callbacks: Mutex<HashMap<u64, Done>>,
    stopping: AtomicBool,
    last_refresh: Mutex<Option<Instant>>,
}

#[derive(Clone)]
pub struct Hub(Arc<Inner>);

pub struct HubOptions {
    pub backend: Option<Arc<dyn DdcBackend>>,
    pub store_path: Option<std::path::PathBuf>,
    pub backlight: Option<Backlight>,
}

impl Default for HubOptions {
    fn default() -> Self {
        HubOptions { backend: None, store_path: None, backlight: Backlight::find() }
    }
}

impl Hub {
    pub fn start(opts: HubOptions) -> Result<(Hub, async_channel::Receiver<HubEvent>), String> {
        let (etx, erx) = async_channel::unbounded();
        let store = Store::load(opts.store_path.unwrap_or_else(Store::default_path));
        let backend: Arc<dyn DdcBackend> = opts.backend.unwrap_or_else(|| {
            let d = Ddcutil::default();
            *d.sleep_multiplier.lock().unwrap() = store.data.settings.ddc_speed;
            Arc::new(d)
        });
        let (wl_handle, wl_rx) = wl::spawn()?;
        let inner = Arc::new(Inner {
            state: Mutex::new(State::default()),
            cv: Condvar::new(),
            wl: Some(wl_handle),
            backend,
            store: Mutex::new(store),
            dirty: AtomicBool::new(false),
            queues: Mutex::new(HashMap::new()),
            backlight: opts.backlight,
            backlight_q: Mutex::new(None),
            last_written_raw: Mutex::new(None),
            events: etx,
            token: AtomicU64::new(1),
            callbacks: Mutex::new(HashMap::new()),
            stopping: AtomicBool::new(false),
            last_refresh: Mutex::new(None),
        });
        let hub = Hub(inner);
        hub.spawn_threads(wl_rx);
        Ok((hub, erx))
    }

    fn spawn_threads(&self, wl_rx: async_channel::Receiver<WlEvent>) {
        // Wayland events
        let w = Arc::downgrade(&self.0);
        std::thread::Builder::new()
            .name("penumbra-wl-events".into())
            .spawn(move || {
                while let Ok(ev) = wl_rx.recv_blocking() {
                    let Some(inner) = w.upgrade() else { return };
                    Hub(inner).on_wl(ev);
                }
            })
            .unwrap();

        // Saver
        let w = Arc::downgrade(&self.0);
        std::thread::Builder::new()
            .name("penumbra-saver".into())
            .spawn(move || loop {
                std::thread::sleep(Duration::from_millis(400));
                let Some(inner) = w.upgrade() else { return };
                if inner.stopping.load(Ordering::Relaxed) {
                    return;
                }
                if inner.dirty.swap(false, Ordering::Relaxed) {
                    inner.store.lock().unwrap().save();
                }
            })
            .unwrap();

        // Backlight: writer and watcher of outside changes (keyboard keys, other tools)
        if let Some(bl) = self.0.backlight.clone() {
            let w: Weak<Inner> = Arc::downgrade(&self.0);
            let bl2 = bl.clone();
            let q = JobQueue::spawn("penumbra-backlight", move |f: f64| {
                if let Some(inner) = w.upgrade() {
                    if let Err(e) = bl2.set_fraction(f) {
                        let _ = inner.events.try_send(HubEvent::Error(e));
                    }
                    *inner.last_written_raw.lock().unwrap() = bl2.read();
                }
            });
            *self.0.backlight_q.lock().unwrap() = Some(q);
            let w = Arc::downgrade(&self.0);
            std::thread::Builder::new()
                .name("penumbra-backlight-watch".into())
                .spawn(move || loop {
                    std::thread::sleep(Duration::from_millis(400));
                    let Some(inner) = w.upgrade() else { return };
                    if inner.stopping.load(Ordering::Relaxed) {
                        return;
                    }
                    Hub(inner).poll_backlight(&bl);
                })
                .unwrap();
        }
    }

    // ------------------------------------------------------------ plumbing

    fn changed(&self) {
        self.0.cv.notify_all();
        let _ = self.0.events.try_send(HubEvent::Changed);
    }

    pub fn notice(&self, m: impl Into<String>) {
        let _ = self.0.events.try_send(HubEvent::Notice(m.into()));
    }

    pub fn error(&self, m: impl Into<String>) {
        let _ = self.0.events.try_send(HubEvent::Error(m.into()));
    }

    fn remember(&self, f: impl FnOnce(&mut Store)) {
        f(&mut self.0.store.lock().unwrap());
        self.0.dirty.store(true, Ordering::Relaxed);
    }

    pub fn flush(&self) {
        self.0.dirty.store(false, Ordering::Relaxed);
        self.0.store.lock().unwrap().save();
    }

    pub fn shutdown(&self) {
        self.flush();
        self.0.stopping.store(true, Ordering::Relaxed);
        if let Some(w) = &self.0.wl {
            w.send(WlCmd::Stop);
        }
        for q in self.0.queues.lock().unwrap().values() {
            q.stop();
        }
    }

    pub fn settings(&self) -> Settings {
        self.0.store.lock().unwrap().data.settings.clone()
    }

    pub fn update_settings(&self, f: impl FnOnce(&mut Settings)) {
        self.remember(|s| f(&mut s.data.settings));
        let speed = self.settings().ddc_speed;
        let _ = speed; // applied the next time ddcutil starts through a new Hub
        self.changed();
    }

    /// Outputs are known and every monitor has been probed (or probing is off).
    pub fn settled(&self) -> bool {
        let ddc_on = self.settings().ddc_enabled;
        let st = self.0.state.lock().unwrap();
        st.got_outputs && (!ddc_on || st.ddc_detected) && st.ddc.values().all(|l| matches!(l.status, DdcStatus::Ready | DdcStatus::Failed(_)))
    }

    pub fn wait_settled(hub: &Hub, timeout: Duration) -> bool {
        let end = Instant::now() + timeout;
        while Instant::now() < end {
            if hub.settled() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    /// Wait until every queued write has been sent to the monitors.
    pub fn drain(&self, timeout: Duration) {
        let end = Instant::now() + timeout;
        loop {
            let busy = self.0.queues.lock().unwrap().values().any(|q| !q.idle())
                || self.0.backlight_q.lock().unwrap().as_ref().map(|q| !q.idle()).unwrap_or(false);
            if !busy || Instant::now() > end {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    pub fn missing(&self) -> Vec<String> {
        self.0.state.lock().unwrap().missing.clone()
    }

    // ------------------------------------------------------------ views

    fn control_for(&self, st: &State, o: &OutputInfo) -> Control {
        if o.is_internal() && self.0.backlight.is_some() {
            return Control::Backlight;
        }
        if let Some(l) = ddc_match(st, o) {
            if !matches!(l.status, DdcStatus::Failed(_)) {
                return Control::Ddc;
            }
        }
        Control::Software
    }

    fn live_for<'a>(&self, st: &'a State, o: &OutputInfo) -> Option<&'a Live> {
        ddc_match(st, o)
    }

    pub fn views(&self) -> Vec<DisplayView> {
        let st = self.0.state.lock().unwrap();
        let store = self.0.store.lock().unwrap();
        st.outputs
            .iter()
            .map(|o| {
                let id = o.identity();
                let mem = store.data.displays.get(&id);
                let control = self.control_for(&st, o);
                let gamma = st.gamma.get(&o.name).copied().unwrap_or_default();
                let ddc = self.live_for(&st, o).map(|l| DdcView {
                    bus: l.display.bus,
                    status: l.status.clone(),
                    caps: l.caps.clone(),
                    values: l.values.iter().map(|(k, v)| (*k, *v)).collect(),
                });
                DisplayView {
                    name: o.name.clone(),
                    id: id.clone(),
                    title: o.title(),
                    nickname: mem.and_then(|m| m.nickname.clone()),
                    output: o.clone(),
                    control,
                    brightness: st.slider.get(&o.name).copied().unwrap_or(1.0),
                    gamma,
                    ddc,
                    hidden: mem.map(|m| m.hidden).unwrap_or(false),
                }
            })
            .collect()
    }

    pub fn view(&self, name: &str) -> Option<DisplayView> {
        self.views().into_iter().find(|v| v.name == name)
    }

    /// Block until `cond` holds for the current views, or the timeout passes.
    pub fn wait_until(&self, timeout: Duration, cond: impl Fn(&[DisplayView]) -> bool) -> bool {
        let end = Instant::now() + timeout;
        loop {
            if cond(&self.views()) {
                return true;
            }
            let now = Instant::now();
            if now >= end {
                return false;
            }
            let g = self.0.state.lock().unwrap();
            let _ = self.0.cv.wait_timeout(g, (end - now).min(Duration::from_millis(250))).unwrap();
        }
    }

    /// Resolve "all", "focused", a connector, a 1-based index or part of a name.
    pub fn resolve(&self, target: &str) -> Vec<String> {
        let views = self.views();
        let t = target.trim();
        match t.to_ascii_lowercase().as_str() {
            "" | "all" => return views.iter().map(|v| v.name.clone()).collect(),
            "focused" | "current" => {
                if let Some(f) = compositor::focused_output() {
                    return vec![f];
                }
                return views.iter().map(|v| v.name.clone()).collect();
            }
            "internal" | "builtin" => return views.iter().filter(|v| v.output.is_internal()).map(|v| v.name.clone()).collect(),
            "external" => return views.iter().filter(|v| !v.output.is_internal()).map(|v| v.name.clone()).collect(),
            _ => {}
        }
        if let Some(v) = views.iter().find(|v| v.name.eq_ignore_ascii_case(t)) {
            return vec![v.name.clone()];
        }
        if let Ok(i) = t.parse::<usize>() {
            if i >= 1 && i <= views.len() {
                return vec![views[i - 1].name.clone()];
            }
        }
        let l = t.to_ascii_lowercase();
        views.iter().filter(|v| v.display_name().to_ascii_lowercase().contains(&l)).map(|v| v.name.clone()).collect()
    }

    // ------------------------------------------------------------ Wayland events

    fn on_wl(&self, ev: WlEvent) {
        match ev {
            WlEvent::Outputs(outs) => self.on_outputs(outs),
            WlEvent::Applied { token, ok, message } => {
                let cb = self.0.callbacks.lock().unwrap().remove(&token);
                if let Some(cb) = cb {
                    cb(if ok { Ok(()) } else { Err(message.clone()) });
                }
                if !ok {
                    self.error(format!("Display change failed: {message}"));
                }
            }
            WlEvent::GammaFailed { output } => {
                self.error(format!("{output}: the compositor refused software dimming (another program may hold the gamma control)"));
            }
            WlEvent::Missing(p) => {
                self.0.state.lock().unwrap().missing.push(p);
                self.changed();
            }
        }
    }

    fn on_outputs(&self, outs: Vec<OutputInfo>) {
        let (first, new_names, changed_set) = {
            let mut st = self.0.state.lock().unwrap();
            let before: HashSet<String> = st.outputs.iter().map(|o| o.name.clone()).collect();
            let now: HashSet<String> = outs.iter().map(|o| o.name.clone()).collect();
            let first = !st.got_outputs;
            st.got_outputs = true;
            let new_names: Vec<String> = outs.iter().filter(|o| !st.seen.contains(&o.name)).map(|o| o.name.clone()).collect();
            for n in &now {
                st.seen.insert(n.clone());
            }
            st.outputs = outs.clone();
            // Gamma and slider entries for outputs that left would be stale.
            st.gamma.retain(|k, _| now.contains(k));
            (first, new_names, before != now)
        };

        // Remember the layout of outputs we just changed.
        let touched: Vec<String> = {
            let mut st = self.0.state.lock().unwrap();
            let t: Vec<String> = st.touched.drain().collect();
            t
        };
        for name in touched {
            if let Some(o) = outs.iter().find(|o| o.name == name) {
                let req = full_request(o);
                self.remember(|s| s.memory(&o.identity()).layout = Some(req));
            }
        }

        // New outputs: bring back what was remembered.
        for name in &new_names {
            if let Some(o) = outs.iter().find(|o| &o.name == name) {
                self.restore_output(o);
            }
        }
        if let Some(bl) = &self.0.backlight {
            if let (Some(raw), Some(n)) = (bl.read(), outs.iter().find(|o| o.is_internal())) {
                if new_names.contains(&n.name) {
                    self.0.state.lock().unwrap().internal_raw = Some(raw);
                    self.sync_slider_from_hw(&n.name, raw as f64 / bl.max as f64);
                }
            }
        }
        if first || changed_set {
            self.schedule_ddc_detect(first);
        }
        if first {
            self.recreate_virtuals(&outs);
        }
        self.changed();
    }

    fn restore_output(&self, o: &OutputInfo) {
        let (mem, settings) = {
            let s = self.0.store.lock().unwrap();
            (s.data.displays.get(&o.identity()).cloned(), s.data.settings.clone())
        };
        let Some(mem) = mem else { return };
        if !mem.gamma.is_identity() {
            self.push_gamma(&o.name, mem.gamma);
        }
        if let Some(b) = mem.brightness {
            self.0.state.lock().unwrap().slider.insert(o.name.clone(), b);
        }
        if settings.restore_layout_on_connect {
            if let Some(mut req) = mem.layout.clone() {
                req.name = o.name.clone();
                self.apply_layout(vec![req], None);
            }
        }
    }

    fn recreate_virtuals(&self, outs: &[OutputInfo]) {
        let vds = self.settings().virtual_displays;
        for vd in vds {
            if !outs.iter().any(|o| o.name == vd.name) {
                if let Err(e) = compositor::create_virtual(&vd.name, vd.width, vd.height, vd.hz) {
                    self.error(format!("Virtual display {}: {e}", vd.name));
                }
            }
        }
    }

    // ------------------------------------------------------------ DDC

    fn schedule_ddc_detect(&self, immediately: bool) {
        if !self.settings().ddc_enabled {
            return;
        }
        let w = Arc::downgrade(&self.0);
        std::thread::Builder::new()
            .name("penumbra-ddc-detect".into())
            .spawn(move || {
                if !immediately {
                    // The monitor answers DDC a moment after the connector appears.
                    std::thread::sleep(Duration::from_millis(1500));
                }
                let Some(inner) = w.upgrade() else { return };
                let hub = Hub(inner);
                match hub.0.backend.detect() {
                    Ok(list) => hub.on_ddc_detected(list),
                    Err(e) => {
                        {
                            let mut st = hub.0.state.lock().unwrap();
                            st.ddc_detected = true;
                        }
                        if !matches!(e, DdcError::Missing) {
                            hub.error(format!("Monitor detection failed: {e}"));
                        } else {
                            hub.error("ddcutil is not installed: monitor controls are unavailable (pacman -S ddcutil)".to_string());
                        }
                        hub.changed();
                    }
                }
            })
            .unwrap();
    }

    fn on_ddc_detected(&self, list: Vec<DdcDisplay>) {
        {
            let mut st = self.0.state.lock().unwrap();
            st.ddc_detected = true;
            let buses: HashSet<u32> = list.iter().map(|d| d.bus).collect();
            st.ddc.retain(|b, _| buses.contains(b));
            for d in &list {
                st.ddc.entry(d.bus).or_insert_with(|| Live { display: d.clone(), status: DdcStatus::Detecting, caps: None, values: HashMap::new(), restored: false });
            }
        }
        for d in &list {
            let q = self.queue_for(d.bus);
            q.push(BusJob::ReadAll, |a, b| matches!((a, b), (BusJob::ReadAll, BusJob::ReadAll)));
        }
        self.changed();
    }

    fn queue_for(&self, bus: u32) -> Arc<JobQueue<BusJob>> {
        let mut qs = self.0.queues.lock().unwrap();
        qs.entry(bus)
            .or_insert_with(|| {
                let w = Arc::downgrade(&self.0);
                JobQueue::spawn(&format!("penumbra-ddc-{bus}"), move |job| {
                    if let Some(inner) = w.upgrade() {
                        Hub(inner).run_bus_job(bus, job);
                    }
                })
            })
            .clone()
    }

    fn run_bus_job(&self, bus: u32, job: BusJob) {
        match job {
            BusJob::Set(code, value) => {
                if let Err(e) = self.0.backend.set(bus, code, value) {
                    self.error(format!("{}: {e}", feature_name(code)));
                }
            }
            BusJob::ReadAll => {
                self.set_status(bus, DdcStatus::Reading);
                match self.0.backend.get(bus, &KNOWN_FEATURES) {
                    Ok(values) if !values.is_empty() => {
                        {
                            let mut st = self.0.state.lock().unwrap();
                            if let Some(l) = st.ddc.get_mut(&bus) {
                                l.values = values;
                                l.status = DdcStatus::Ready;
                            }
                        }
                        self.after_first_read(bus);
                        self.changed();
                        // Capabilities are slow; the sliders are usable before they arrive.
                        if let Ok(caps) = self.0.backend.capabilities(bus) {
                            if let Some(l) = self.0.state.lock().unwrap().ddc.get_mut(&bus) {
                                l.caps = Some(caps);
                            }
                            self.changed();
                        }
                    }
                    Ok(_) => self.set_status(bus, DdcStatus::Failed("the monitor does not answer DDC/CI (is DDC/CI switched on in its menu?)".into())),
                    Err(e) => self.set_status(bus, DdcStatus::Failed(e.to_string())),
                }
            }
            BusJob::Refresh => {
                if let Ok(values) = self.0.backend.get(bus, &KNOWN_FEATURES) {
                    if !values.is_empty() {
                        let mut name = None;
                        {
                            let mut st = self.0.state.lock().unwrap();
                            let connector = st.outputs.iter().find(|o| ddc_match(&st, o).map(|m| m.display.bus) == Some(bus)).map(|o| o.name.clone());
                            if let Some(l) = st.ddc.get_mut(&bus) {
                                l.values = values.clone();
                            }
                            if let (Some(c), Some(b)) = (connector, values.get(&VCP_BRIGHTNESS)) {
                                name = Some((c, *b));
                            }
                        }
                        if let Some((c, b)) = name {
                            self.sync_slider_from_hw(&c, b.current as f64 / b.max.max(1) as f64);
                        }
                        self.changed();
                    }
                }
            }
        }
    }

    fn set_status(&self, bus: u32, s: DdcStatus) {
        if let Some(l) = self.0.state.lock().unwrap().ddc.get_mut(&bus) {
            l.status = s;
        }
        self.changed();
    }

    fn sync_slider_from_hw(&self, output: &str, hw_fraction: f64) {
        let combined = self.settings().combined_brightness;
        let mut st = self.0.state.lock().unwrap();
        let dim = st.gamma.get(output).map(|g| g.brightness).unwrap_or(1.0);
        st.slider.insert(output.to_string(), brightness::join(hw_fraction, dim, combined));
    }

    fn after_first_read(&self, bus: u32) {
        let (output, values, restored) = {
            let st = self.0.state.lock().unwrap();
            let Some(l) = st.ddc.get(&bus) else { return };
            let Some(out) = st.outputs.iter().find(|o| ddc_match(&st, o).map(|m| m.display.bus) == Some(bus)) else { return };
            (out.name.clone(), l.values.clone(), l.restored)
        };
        let outinfo = self.0.state.lock().unwrap().outputs.iter().find(|o| o.name == output).cloned();
        let settings = self.settings();
        let mem = outinfo.as_ref().and_then(|o| self.0.store.lock().unwrap().data.displays.get(&o.identity()).cloned());
        if let (Some(b), false) = (values.get(&VCP_BRIGHTNESS), restored) {
            // What the monitor says now is the truth for the slider; a remembered value wins
            // only when restoring is on.
            self.sync_slider_from_hw(&output, b.current as f64 / b.max.max(1) as f64);
        }
        if let Some(l) = self.0.state.lock().unwrap().ddc.get_mut(&bus) {
            l.restored = true;
        }
        if restored || !settings.restore_on_connect {
            return;
        }
        let Some(mem) = mem else { return };
        if let Some(b) = mem.brightness {
            if self.view(&output).is_some() {
                self.set_brightness(&output, b);
            }
        }
        for (k, v) in &mem.ddc {
            let Ok(code) = u8::from_str_radix(k, 16) else { continue };
            if code == VCP_BRIGHTNESS || code == VCP_POWER {
                continue;
            }
            if values.get(&code).map(|c| c.current) != Some(*v) {
                self.set_ddc(&output, code, *v);
            }
        }
    }

    /// Re-read the monitors' values (cheap; rate limited unless `force`).
    pub fn refresh(&self, force: bool) {
        {
            let mut last = self.0.last_refresh.lock().unwrap();
            if !force {
                if let Some(t) = *last {
                    if t.elapsed() < Duration::from_secs(8) {
                        return;
                    }
                }
            }
            *last = Some(Instant::now());
        }
        let buses: Vec<u32> = self
            .0
            .state
            .lock()
            .unwrap()
            .ddc
            .iter()
            .filter(|(_, l)| l.status == DdcStatus::Ready)
            .map(|(b, _)| *b)
            .collect();
        for b in buses {
            self.queue_for(b).push(BusJob::Refresh, |a, b| matches!((a, b), (BusJob::Refresh, BusJob::Refresh)));
        }
    }

    /// Detect monitors again (after plugging one in, or switching DDC/CI on).
    pub fn redetect(&self) {
        self.schedule_ddc_detect(true);
    }

    // ------------------------------------------------------------ brightness and DDC

    pub fn set_brightness(&self, output: &str, v: f64) {
        let v = v.clamp(0.0, 1.0);
        let Some(view) = self.view(output) else { return };
        let combined = self.settings().combined_brightness;
        match view.control {
            Control::Ddc => {
                let bus = view.ddc.as_ref().map(|d| d.bus);
                let has_hw = view.ddc.as_ref().map(|d| d.has(VCP_BRIGHTNESS) || d.status != DdcStatus::Ready).unwrap_or(false);
                if let (Some(bus), true) = (bus, has_hw) {
                    let (hw, dim) = brightness::split(v, combined);
                    let max = view.ddc.as_ref().and_then(|d| d.values.get(&VCP_BRIGHTNESS)).map(|x| x.max).filter(|m| *m > 0).unwrap_or(100);
                    let raw = (hw * max as f64).round() as u32;
                    self.queue_for(bus).push(BusJob::Set(VCP_BRIGHTNESS, raw), |a, b| matches!((a, b), (BusJob::Set(x, _), BusJob::Set(y, _)) if x == y));
                    if let Some(l) = self.0.state.lock().unwrap().ddc.get_mut(&bus) {
                        l.values.insert(VCP_BRIGHTNESS, VcpValue { current: raw, max });
                    }
                    self.set_dim(output, dim);
                    self.remember(|s| {
                        let m = s.memory(&view.id);
                        m.brightness = Some(v);
                        m.ddc.insert(format!("{VCP_BRIGHTNESS:02x}"), raw);
                    });
                } else {
                    self.set_dim(output, brightness::soft_only(v));
                }
            }
            Control::Backlight => {
                let (hw, dim) = brightness::split(v, combined);
                if let Some(q) = self.0.backlight_q.lock().unwrap().as_ref() {
                    q.push(hw, |_, _| true);
                }
                self.set_dim(output, dim);
                self.remember(|s| s.memory(&view.id).brightness = Some(v));
            }
            Control::Software => {
                self.set_dim(output, brightness::soft_only(v));
                self.remember(|s| s.memory(&view.id).brightness = Some(v));
            }
        }
        self.0.state.lock().unwrap().slider.insert(output.to_string(), v);
        self.changed();
    }

    /// Software dimming factor (the part of brightness done in gamma ramps).
    fn set_dim(&self, output: &str, dim: f64) {
        let mut g = self.0.state.lock().unwrap().gamma.get(output).copied().unwrap_or_default();
        g.brightness = dim;
        self.push_gamma(output, g);
    }

    fn push_gamma(&self, output: &str, g: Gamma) {
        self.0.state.lock().unwrap().gamma.insert(output.to_string(), g);
        if let Some(w) = &self.0.wl {
            w.send(WlCmd::SetGamma { output: output.to_string(), gamma: if g.is_identity() { None } else { Some(g) } });
        }
        if let Some(v) = self.view_light(output) {
            self.remember(|s| s.memory(&v).gamma = g);
        }
    }

    fn view_light(&self, output: &str) -> Option<String> {
        self.0.state.lock().unwrap().outputs.iter().find(|o| o.name == output).map(|o| o.identity())
    }

    pub fn set_temperature(&self, output: &str, kelvin: f64) {
        let mut g = self.view(output).map(|v| v.gamma).unwrap_or_default();
        g.temperature = kelvin.clamp(1500.0, 10000.0);
        self.push_gamma(output, g);
        self.changed();
    }

    pub fn set_gamma_curve(&self, output: &str, gamma: f64) {
        let mut g = self.view(output).map(|v| v.gamma).unwrap_or_default();
        g.gamma = gamma.clamp(0.5, 2.5);
        self.push_gamma(output, g);
        self.changed();
    }

    pub fn reset_colour(&self, output: &str) {
        let mut g = self.view(output).map(|v| v.gamma).unwrap_or_default();
        g.temperature = 6500.0;
        g.gamma = 1.0;
        self.push_gamma(output, g);
        self.changed();
    }

    pub fn set_ddc(&self, output: &str, code: u8, value: u32) {
        let Some(view) = self.view(output) else { return };
        let Some(d) = &view.ddc else { return };
        let bus = d.bus;
        if code == VCP_BRIGHTNESS {
            let max = d.values.get(&code).map(|x| x.max).filter(|m| *m > 0).unwrap_or(100);
            self.set_brightness(output, brightness::join(value as f64 / max as f64, view.gamma.brightness, self.settings().combined_brightness));
            return;
        }
        self.queue_for(bus).push(BusJob::Set(code, value), |a, b| matches!((a, b), (BusJob::Set(x, _), BusJob::Set(y, _)) if x == y));
        if let Some(l) = self.0.state.lock().unwrap().ddc.get_mut(&bus) {
            if let Some(v) = l.values.get_mut(&code) {
                v.current = value;
            }
        }
        self.remember(|s| {
            s.memory(&view.id).ddc.insert(format!("{code:02x}"), value);
        });
        self.changed();
    }

    fn poll_backlight(&self, bl: &Backlight) {
        let Some(raw) = bl.read() else { return };
        let mine = *self.0.last_written_raw.lock().unwrap();
        let known = self.0.state.lock().unwrap().internal_raw;
        if known == Some(raw) || mine == Some(raw) {
            self.0.state.lock().unwrap().internal_raw = Some(raw);
            return;
        }
        self.0.state.lock().unwrap().internal_raw = Some(raw);
        if known.is_none() {
            // First look: seed the slider.
            let name = self.internal_name();
            if let Some(n) = name {
                self.sync_slider_from_hw(&n, raw as f64 / bl.max as f64);
                self.changed();
            }
            return;
        }
        // Changed by someone else (a brightness key).
        let Some(n) = self.internal_name() else { return };
        self.sync_slider_from_hw(&n, raw as f64 / bl.max as f64);
        let settings = self.settings();
        {
            let v = self.0.state.lock().unwrap().slider.get(&n).copied().unwrap_or(1.0);
            for o in self.views().iter().filter(|o| !o.output.is_internal()) {
                if settings.follow_internal || self.follow_of(&o.id) {
                    self.set_brightness(&o.name, v);
                }
            }
        }
        self.changed();
    }

    fn internal_name(&self) -> Option<String> {
        self.0.state.lock().unwrap().outputs.iter().find(|o| o.is_internal()).map(|o| o.name.clone())
    }

    // ------------------------------------------------------------ layout

    pub fn apply_layout(&self, requests: Vec<OutputRequest>, done: Option<Done>) {
        let Some(w) = &self.0.wl else { return };
        let token = self.0.token.fetch_add(1, Ordering::Relaxed);
        {
            let mut st = self.0.state.lock().unwrap();
            for r in &requests {
                st.touched.insert(r.name.clone());
            }
        }
        if let Some(d) = done {
            self.0.callbacks.lock().unwrap().insert(token, d);
        }
        w.send(WlCmd::Apply { token, requests });
    }

    /// Apply and wait for the compositor's answer.
    pub fn apply_layout_blocking(&self, requests: Vec<OutputRequest>) -> Result<(), String> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.apply_layout(requests, Some(Box::new(move |r| {
            let _ = tx.send(r);
        })));
        rx.recv_timeout(Duration::from_secs(10)).map_err(|_| "the compositor did not answer".to_string())?
    }

    pub fn set_mode(&self, output: &str, m: &Mode) {
        self.apply_layout(vec![OutputRequest { name: output.into(), mode: Some((m.width, m.height, m.refresh_mhz)), ..Default::default() }], None);
    }

    pub fn set_scale(&self, output: &str, scale: f64) {
        self.apply_layout(vec![OutputRequest { name: output.into(), scale: Some(scale), ..Default::default() }], None);
    }

    pub fn set_transform(&self, output: &str, t: Transform) {
        self.apply_layout(vec![OutputRequest { name: output.into(), transform: Some(t), ..Default::default() }], None);
    }

    pub fn set_position(&self, output: &str, x: i32, y: i32) {
        self.apply_layout(vec![OutputRequest { name: output.into(), position: Some((x, y)), ..Default::default() }], None);
    }

    pub fn set_enabled(&self, output: &str, on: bool) {
        self.apply_layout(vec![OutputRequest { name: output.into(), enabled: Some(on), ..Default::default() }], None);
    }

    pub fn set_vrr(&self, output: &str, on: bool) {
        self.apply_layout(vec![OutputRequest { name: output.into(), vrr: Some(on), ..Default::default() }], None);
    }

    pub fn set_nickname(&self, id: &str, nick: Option<String>) {
        self.remember(|s| s.memory(id).nickname = nick.filter(|n| !n.trim().is_empty()));
        self.changed();
    }

    pub fn set_hidden(&self, id: &str, hidden: bool) {
        self.remember(|s| s.memory(id).hidden = hidden);
        self.changed();
    }

    pub fn follow_of(&self, id: &str) -> bool {
        self.0.store.lock().unwrap().data.displays.get(id).map(|m| m.follow_internal).unwrap_or(false)
    }

    pub fn set_follow(&self, id: &str, follow: bool) {
        self.remember(|s| s.memory(id).follow_internal = follow);
        self.changed();
    }

    // ------------------------------------------------------------ virtual displays

    pub fn create_virtual(&self, vd: VirtualDisplay) -> Result<(), String> {
        compositor::create_virtual(&vd.name, vd.width, vd.height, vd.hz)?;
        self.remember(|s| {
            s.data.settings.virtual_displays.retain(|v| v.name != vd.name);
            s.data.settings.virtual_displays.push(vd);
        });
        self.changed();
        Ok(())
    }

    pub fn destroy_virtual(&self, name: &str) -> Result<(), String> {
        compositor::destroy_virtual(name)?;
        self.remember(|s| s.data.settings.virtual_displays.retain(|v| v.name != name));
        self.changed();
        Ok(())
    }

    // ------------------------------------------------------------ profiles

    pub fn profile_names(&self) -> Vec<String> {
        self.0.store.lock().unwrap().data.profiles.keys().cloned().collect()
    }

    pub fn save_profile(&self, name: &str) {
        let mut p = Profile::default();
        let store_mem = self.0.store.lock().unwrap().data.displays.clone();
        for v in self.views() {
            let mut m = store_mem.get(&v.id).cloned().unwrap_or_default();
            m.layout = Some(full_request(&v.output));
            m.gamma = v.gamma;
            m.brightness = Some(v.brightness);
            if let Some(d) = &v.ddc {
                for code in [VCP_CONTRAST, VCP_PRESET, VCP_GAIN_R, VCP_GAIN_G, VCP_GAIN_B, VCP_INPUT, VCP_VOLUME, VCP_SHARPNESS] {
                    if let Some(x) = d.values.get(&code) {
                        m.ddc.insert(format!("{code:02x}"), x.current);
                    }
                }
            }
            p.displays.insert(v.id.clone(), m);
        }
        self.remember(|s| {
            s.data.profiles.insert(name.to_string(), p);
        });
        self.changed();
    }

    pub fn delete_profile(&self, name: &str) {
        self.remember(|s| {
            s.data.profiles.remove(name);
        });
        self.changed();
    }

    pub fn apply_profile(&self, name: &str) -> Result<usize, String> {
        let p = self.0.store.lock().unwrap().data.profiles.get(name).cloned().ok_or_else(|| format!("no profile called {name}"))?;
        let views = self.views();
        let mut requests = Vec::new();
        let mut matched = 0;
        for v in &views {
            let Some(m) = p.displays.get(&v.id) else { continue };
            matched += 1;
            if let Some(mut r) = m.layout.clone() {
                r.name = v.name.clone();
                requests.push(r);
            }
        }
        if !requests.is_empty() {
            self.apply_layout(requests, None);
        }
        for v in &views {
            let Some(m) = p.displays.get(&v.id) else { continue };
            for (k, val) in &m.ddc {
                if let Ok(code) = u8::from_str_radix(k, 16) {
                    self.set_ddc(&v.name, code, *val);
                }
            }
            if let Some(b) = m.brightness {
                self.set_brightness(&v.name, b);
            }
            let mut g = self.view(&v.name).map(|x| x.gamma).unwrap_or_default();
            g.temperature = m.gamma.temperature;
            g.gamma = m.gamma.gamma;
            self.push_gamma(&v.name, g);
        }
        self.changed();
        Ok(matched)
    }
}

/// Which DDC monitor belongs to an output: by connector name, or, when ddcutil could not say
/// which connector a bus is on (some NVIDIA setups), by the model and serial from the EDID.
fn ddc_match<'a>(st: &'a State, o: &OutputInfo) -> Option<&'a Live> {
    if let Some(l) = st.ddc.values().find(|l| !l.display.connector.is_empty() && l.display.connector.eq_ignore_ascii_case(&o.name)) {
        return Some(l);
    }
    let norm = |s: &str| s.to_ascii_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>();
    let model = norm(&o.model);
    if model.is_empty() {
        return None;
    }
    let outputs_named: Vec<&str> = st.outputs.iter().map(|x| x.name.as_str()).collect();
    let cands: Vec<&Live> = st
        .ddc
        .values()
        .filter(|l| l.display.connector.is_empty() || !outputs_named.iter().any(|n| n.eq_ignore_ascii_case(&l.display.connector)))
        .filter(|l| norm(&l.display.model) == model)
        .filter(|l| l.display.serial.is_empty() || o.serial.is_empty() || norm(&l.display.serial) == norm(&o.serial))
        .collect();
    // Two identical monitors we cannot tell apart: better no control than the wrong monitor.
    let same_model_outputs = st.outputs.iter().filter(|x| norm(&x.model) == model).count();
    if cands.len() == 1 && same_model_outputs == 1 {
        return cands.into_iter().next();
    }
    if cands.len() >= 1 && !o.serial.is_empty() {
        return cands.into_iter().find(|l| norm(&l.display.serial) == norm(&o.serial));
    }
    None
}

fn full_request(o: &OutputInfo) -> OutputRequest {
    OutputRequest {
        name: o.name.clone(),
        enabled: Some(o.enabled),
        mode: o.mode.as_ref().map(|m| (m.width, m.height, m.refresh_mhz)),
        position: Some(o.position),
        transform: Some(o.transform),
        scale: Some(o.scale),
        vrr: o.vrr,
    }
}
