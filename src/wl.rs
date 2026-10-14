//! Wayland side: output management (mode, scale, position, rotation, VRR, enable) and
//! gamma control (software dimming and colour temperature). Runs in its own thread.

use std::collections::HashMap;
use std::io::{Seek, Write};
use std::os::fd::AsFd;

use calloop::channel::{channel, Channel, Event as ChanEvent, Sender};
use calloop::EventLoop;
use calloop_wayland_source::WaylandSource;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_output, wl_registry};
use wayland_client::{event_created_child, Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols_wlr::gamma_control::v1::client::{zwlr_gamma_control_manager_v1 as gmgr, zwlr_gamma_control_v1 as gctl};
use wayland_protocols_wlr::output_management::v1::client::{
    zwlr_output_configuration_head_v1 as cfg_head, zwlr_output_configuration_v1 as cfg, zwlr_output_head_v1 as head_p,
    zwlr_output_manager_v1 as mgr, zwlr_output_mode_v1 as mode_p,
};

use crate::colour;
use crate::model::{Gamma, Mode, OutputInfo, OutputRequest, Transform};

#[derive(Debug)]
pub enum WlCmd {
    /// Change outputs; `token` comes back in `WlEvent::Applied`.
    Apply { token: u64, requests: Vec<OutputRequest> },
    /// `None` gives the original colours back.
    SetGamma { output: String, gamma: Option<Gamma> },
    Stop,
}

#[derive(Debug, Clone)]
pub enum WlEvent {
    Outputs(Vec<OutputInfo>),
    Applied { token: u64, ok: bool, message: String },
    GammaFailed { output: String },
    /// The compositor lacks a protocol we need.
    Missing(String),
}

#[derive(Clone)]
pub struct WlHandle {
    tx: Sender<WlCmd>,
}

impl WlHandle {
    pub fn send(&self, cmd: WlCmd) {
        let _ = self.tx.send(cmd);
    }
}

/// Start the Wayland thread. Events arrive on the returned receiver.
pub fn spawn() -> Result<(WlHandle, async_channel::Receiver<WlEvent>), String> {
    let conn = Connection::connect_to_env().map_err(|e| format!("cannot connect to the Wayland compositor: {e}"))?;
    let (etx, erx) = async_channel::unbounded();
    let (tx, rx) = channel::<WlCmd>();
    std::thread::Builder::new()
        .name("penumbra-wayland".into())
        .spawn(move || {
            if let Err(e) = run(conn, rx, etx.clone()) {
                log::error!("wayland thread ended: {e}");
                let _ = etx.send_blocking(WlEvent::Missing(e));
            }
        })
        .map_err(|e| e.to_string())?;
    Ok((WlHandle { tx }, erx))
}

struct ModeState {
    proxy: mode_p::ZwlrOutputModeV1,
    width: i32,
    height: i32,
    refresh: i32,
    preferred: bool,
}

struct HeadState {
    proxy: head_p::ZwlrOutputHeadV1,
    name: String,
    description: String,
    make: String,
    model: String,
    serial: String,
    phys: (i32, i32),
    enabled: bool,
    current_mode: Option<mode_p::ZwlrOutputModeV1>,
    modes: Vec<ModeState>,
    position: (i32, i32),
    transform: Transform,
    scale: f64,
    vrr: Option<bool>,
    finished: bool,
}

struct OutState {
    proxy: wl_output::WlOutput,
    name: Option<String>,
}

struct GammaState {
    ctl: gctl::ZwlrGammaControlV1,
    size: u32,
    want: Gamma,
}

struct State {
    events: async_channel::Sender<WlEvent>,
    manager: Option<mgr::ZwlrOutputManagerV1>,
    gamma_mgr: Option<gmgr::ZwlrGammaControlManagerV1>,
    heads: Vec<HeadState>,
    serial: u32,
    outs: Vec<OutState>,
    gammas: HashMap<String, GammaState>,
    wanted_gamma: HashMap<String, Gamma>,
    running: bool,
}

fn convert_transform(t: WEnum<wl_output::Transform>) -> Transform {
    match t {
        WEnum::Value(wl_output::Transform::_90) => Transform::R90,
        WEnum::Value(wl_output::Transform::_180) => Transform::R180,
        WEnum::Value(wl_output::Transform::_270) => Transform::R270,
        WEnum::Value(wl_output::Transform::Flipped) => Transform::Flipped,
        WEnum::Value(wl_output::Transform::Flipped90) => Transform::Flipped90,
        WEnum::Value(wl_output::Transform::Flipped180) => Transform::Flipped180,
        WEnum::Value(wl_output::Transform::Flipped270) => Transform::Flipped270,
        _ => Transform::Normal,
    }
}

fn to_wl_transform(t: Transform) -> wl_output::Transform {
    match t {
        Transform::Normal => wl_output::Transform::Normal,
        Transform::R90 => wl_output::Transform::_90,
        Transform::R180 => wl_output::Transform::_180,
        Transform::R270 => wl_output::Transform::_270,
        Transform::Flipped => wl_output::Transform::Flipped,
        Transform::Flipped90 => wl_output::Transform::Flipped90,
        Transform::Flipped180 => wl_output::Transform::Flipped180,
        Transform::Flipped270 => wl_output::Transform::Flipped270,
    }
}

impl State {
    fn snapshot(&self) -> Vec<OutputInfo> {
        self.heads
            .iter()
            .filter(|h| !h.finished)
            .map(|h| {
                let to_mode = |m: &ModeState| Mode { width: m.width, height: m.height, refresh_mhz: m.refresh, preferred: m.preferred };
                OutputInfo {
                    name: h.name.clone(),
                    description: h.description.clone(),
                    make: h.make.clone(),
                    model: h.model.clone(),
                    serial: h.serial.clone(),
                    phys_mm: h.phys,
                    enabled: h.enabled,
                    mode: h.current_mode.as_ref().and_then(|c| h.modes.iter().find(|m| m.proxy == *c)).map(to_mode),
                    modes: h.modes.iter().map(to_mode).collect(),
                    position: h.position,
                    transform: h.transform,
                    scale: h.scale,
                    vrr: h.vrr,
                }
            })
            .collect()
    }

    fn head_mut(&mut self, p: &head_p::ZwlrOutputHeadV1) -> Option<&mut HeadState> {
        self.heads.iter_mut().find(|h| h.proxy == *p)
    }

    fn apply(&mut self, qh: &QueueHandle<State>, token: u64, requests: Vec<OutputRequest>) {
        let Some(manager) = self.manager.clone() else {
            let _ = self.events.send_blocking(WlEvent::Applied { token, ok: false, message: "the compositor has no output management".into() });
            return;
        };
        let config = manager.create_configuration(self.serial, qh, token);
        for h in self.heads.iter().filter(|h| !h.finished) {
            let req = requests.iter().find(|r| r.name == h.name);
            let enabled = req.and_then(|r| r.enabled).unwrap_or(h.enabled);
            if !enabled {
                config.disable_head(&h.proxy);
                continue;
            }
            let ch = config.enable_head(&h.proxy, qh, ());
            // Mode
            let wanted = req.and_then(|r| r.mode);
            match wanted {
                Some((w, hh, r)) => {
                    // Prefer an advertised mode with the closest refresh rate; else a custom mode.
                    let best = h
                        .modes
                        .iter()
                        .filter(|m| m.width == w && m.height == hh)
                        .min_by_key(|m| (m.refresh - r).abs())
                        .filter(|m| (m.refresh - r).abs() <= 500 || r == 0);
                    match best {
                        Some(m) => ch.set_mode(&m.proxy),
                        None => ch.set_custom_mode(w, hh, r),
                    }
                }
                None => {
                    if let Some(c) = &h.current_mode {
                        ch.set_mode(c);
                    } else if let Some(m) = h.modes.iter().find(|m| m.preferred).or(h.modes.first()) {
                        ch.set_mode(&m.proxy);
                    }
                }
            }
            let pos = req.and_then(|r| r.position).unwrap_or(h.position);
            ch.set_position(pos.0, pos.1);
            ch.set_transform(to_wl_transform(req.and_then(|r| r.transform).unwrap_or(h.transform)));
            ch.set_scale(req.and_then(|r| r.scale).unwrap_or(h.scale));
            if h.vrr.is_some() || req.and_then(|r| r.vrr).is_some() {
                let v = req.and_then(|r| r.vrr).or(h.vrr).unwrap_or(false);
                if config.version() >= 4 {
                    ch.set_adaptive_sync(if v { head_p::AdaptiveSyncState::Enabled } else { head_p::AdaptiveSyncState::Disabled });
                }
            }
        }
        config.apply();
    }

    fn ensure_gamma(&mut self, qh: &QueueHandle<State>, output: &str) {
        if self.gammas.contains_key(output) {
            return;
        }
        let Some(gm) = self.gamma_mgr.clone() else { return };
        let Some(out) = self.outs.iter().find(|o| o.name.as_deref() == Some(output)) else { return };
        let ctl = gm.get_gamma_control(&out.proxy, qh, output.to_string());
        self.gammas.insert(output.to_string(), GammaState { ctl, size: 0, want: Gamma::default() });
    }

    fn set_gamma(&mut self, qh: &QueueHandle<State>, output: &str, gamma: Option<Gamma>) {
        match gamma {
            None => {
                self.wanted_gamma.remove(output);
                if let Some(g) = self.gammas.remove(output) {
                    g.ctl.destroy();
                }
            }
            Some(g) => {
                self.wanted_gamma.insert(output.to_string(), g);
                self.ensure_gamma(qh, output);
                if let Some(st) = self.gammas.get_mut(output) {
                    st.want = g;
                    if st.size > 0 {
                        write_ramp(&st.ctl, st.size, &g);
                    }
                }
            }
        }
    }
}

fn write_ramp(ctl: &gctl::ZwlrGammaControlV1, size: u32, g: &Gamma) {
    let ramp = colour::ramp(size as usize, g);
    let fd = match rustix::fs::memfd_create("penumbra-gamma", rustix::fs::MemfdFlags::CLOEXEC) {
        Ok(fd) => fd,
        Err(e) => {
            log::error!("memfd_create: {e}");
            return;
        }
    };
    let mut file = std::fs::File::from(fd);
    let bytes: Vec<u8> = ramp.iter().flat_map(|v| v.to_ne_bytes()).collect();
    if file.write_all(&bytes).is_err() || file.rewind().is_err() {
        return;
    }
    ctl.set_gamma(file.as_fd());
}

fn run(conn: Connection, rx: Channel<WlCmd>, events: async_channel::Sender<WlEvent>) -> Result<(), String> {
    let (globals, queue) = registry_queue_init::<State>(&conn).map_err(|e| e.to_string())?;
    let qh = queue.handle();
    let manager = globals.bind::<mgr::ZwlrOutputManagerV1, _, _>(&qh, 1..=4, ()).ok();
    let gamma_mgr = globals.bind::<gmgr::ZwlrGammaControlManagerV1, _, _>(&qh, 1..=1, ()).ok();
    if manager.is_none() {
        let _ = events.send_blocking(WlEvent::Missing("zwlr_output_manager_v1".into()));
    }
    if gamma_mgr.is_none() {
        let _ = events.send_blocking(WlEvent::Missing("zwlr_gamma_control_manager_v1".into()));
    }
    let mut state = State {
        events,
        manager,
        gamma_mgr,
        heads: Vec::new(),
        serial: 0,
        outs: Vec::new(),
        gammas: HashMap::new(),
        wanted_gamma: HashMap::new(),
        running: true,
    };
    for g in globals.contents().clone_list() {
        if g.interface == "wl_output" {
            let v = g.version.min(4);
            let proxy = globals.registry().bind::<wl_output::WlOutput, _, _>(g.name, v, &qh, g.name);
            state.outs.push(OutState { proxy, name: None });
        }
    }

    let mut event_loop: EventLoop<State> = EventLoop::try_new().map_err(|e| e.to_string())?;
    let handle = event_loop.handle();
    WaylandSource::new(conn, queue).insert(handle.clone()).map_err(|e| e.to_string())?;
    let qh2 = qh.clone();
    handle
        .insert_source(rx, move |ev, _, st| match ev {
            ChanEvent::Msg(WlCmd::Apply { token, requests }) => st.apply(&qh2, token, requests),
            ChanEvent::Msg(WlCmd::SetGamma { output, gamma }) => st.set_gamma(&qh2, &output, gamma),
            ChanEvent::Msg(WlCmd::Stop) | ChanEvent::Closed => st.running = false,
        })
        .map_err(|e| e.to_string())?;

    while state.running {
        event_loop.dispatch(None, &mut state).map_err(|e| e.to_string())?;
    }
    for (_, g) in state.gammas.drain() {
        g.ctl.destroy();
    }
    Ok(())
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(state: &mut Self, registry: &wl_registry::WlRegistry, event: wl_registry::Event, _: &GlobalListContents, _: &Connection, qh: &QueueHandle<Self>) {
        match event {
            wl_registry::Event::Global { name, interface, version } if interface == "wl_output" => {
                let proxy = registry.bind::<wl_output::WlOutput, _, _>(name, version.min(4), qh, name);
                state.outs.push(OutState { proxy, name: None });
            }
            wl_registry::Event::GlobalRemove { name } => {
                // The wl_output user data holds the registry name.
                let gone: Vec<String> = state
                    .outs
                    .iter()
                    .filter(|o| o.proxy.data::<u32>().copied() == Some(name))
                    .filter_map(|o| o.name.clone())
                    .collect();
                state.outs.retain(|o| o.proxy.data::<u32>().copied() != Some(name));
                for n in gone {
                    if let Some(g) = state.gammas.remove(&n) {
                        g.ctl.destroy();
                    }
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_output::WlOutput, u32> for State {
    fn event(state: &mut Self, proxy: &wl_output::WlOutput, event: wl_output::Event, _: &u32, _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_output::Event::Name { name } = event {
            let mut pending = None;
            if let Some(o) = state.outs.iter_mut().find(|o| o.proxy == *proxy) {
                o.name = Some(name.clone());
                pending = state.wanted_gamma.get(&name).copied();
            }
            // An output that came back (hotplug) gets its dimming again.
            if let Some(g) = pending {
                state.set_gamma(qh, &name, Some(g));
            }
        }
    }
}

impl Dispatch<mgr::ZwlrOutputManagerV1, ()> for State {
    fn event(state: &mut Self, _: &mgr::ZwlrOutputManagerV1, event: mgr::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            mgr::Event::Head { head } => {
                state.heads.push(HeadState {
                    proxy: head,
                    name: String::new(),
                    description: String::new(),
                    make: String::new(),
                    model: String::new(),
                    serial: String::new(),
                    phys: (0, 0),
                    enabled: false,
                    current_mode: None,
                    modes: Vec::new(),
                    position: (0, 0),
                    transform: Transform::Normal,
                    scale: 1.0,
                    vrr: None,
                    finished: false,
                });
            }
            mgr::Event::Done { serial } => {
                state.serial = serial;
                state.heads.retain(|h| !h.finished);
                let _ = state.events.send_blocking(WlEvent::Outputs(state.snapshot()));
            }
            mgr::Event::Finished => {}
            _ => {}
        }
    }
    event_created_child!(State, mgr::ZwlrOutputManagerV1, [
        mgr::EVT_HEAD_OPCODE => (head_p::ZwlrOutputHeadV1, ()),
    ]);
}

impl Dispatch<head_p::ZwlrOutputHeadV1, ()> for State {
    fn event(state: &mut Self, proxy: &head_p::ZwlrOutputHeadV1, event: head_p::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        let Some(h) = state.head_mut(proxy) else { return };
        match event {
            head_p::Event::Name { name } => h.name = name,
            head_p::Event::Description { description } => h.description = description,
            head_p::Event::PhysicalSize { width, height } => h.phys = (width, height),
            head_p::Event::Mode { mode } => h.modes.push(ModeState { proxy: mode, width: 0, height: 0, refresh: 0, preferred: false }),
            head_p::Event::Enabled { enabled } => h.enabled = enabled != 0,
            head_p::Event::CurrentMode { mode } => h.current_mode = Some(mode),
            head_p::Event::Position { x, y } => h.position = (x, y),
            head_p::Event::Transform { transform } => h.transform = convert_transform(transform),
            head_p::Event::Scale { scale } => h.scale = scale,
            head_p::Event::Finished => h.finished = true,
            head_p::Event::Make { make } => h.make = make,
            head_p::Event::Model { model } => h.model = model,
            head_p::Event::SerialNumber { serial_number } => h.serial = serial_number,
            head_p::Event::AdaptiveSync { state: s } => h.vrr = Some(matches!(s, WEnum::Value(head_p::AdaptiveSyncState::Enabled))),
            _ => {}
        }
    }
    event_created_child!(State, head_p::ZwlrOutputHeadV1, [
        head_p::EVT_MODE_OPCODE => (mode_p::ZwlrOutputModeV1, ()),
    ]);
}

impl Dispatch<mode_p::ZwlrOutputModeV1, ()> for State {
    fn event(state: &mut Self, proxy: &mode_p::ZwlrOutputModeV1, event: mode_p::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        let Some(head_idx) = state.heads.iter().position(|h| h.modes.iter().any(|m| m.proxy == *proxy)) else { return };
        let Some(m) = state.heads[head_idx].modes.iter_mut().find(|m| m.proxy == *proxy) else { return };
        match event {
            mode_p::Event::Size { width, height } => {
                m.width = width;
                m.height = height;
            }
            mode_p::Event::Refresh { refresh } => m.refresh = refresh,
            mode_p::Event::Preferred => m.preferred = true,
            mode_p::Event::Finished => {
                state.heads[head_idx].modes.retain(|x| x.proxy != *proxy);
            }
            _ => {}
        }
    }
}

impl Dispatch<cfg::ZwlrOutputConfigurationV1, u64> for State {
    fn event(state: &mut Self, proxy: &cfg::ZwlrOutputConfigurationV1, event: cfg::Event, token: &u64, _: &Connection, _: &QueueHandle<Self>) {
        let (ok, message) = match event {
            cfg::Event::Succeeded => (true, String::new()),
            cfg::Event::Failed => (false, "the compositor refused this configuration".into()),
            cfg::Event::Cancelled => (false, "the displays changed meanwhile; try again".into()),
            _ => return,
        };
        proxy.destroy();
        let _ = state.events.send_blocking(WlEvent::Applied { token: *token, ok, message });
    }
}

impl Dispatch<cfg_head::ZwlrOutputConfigurationHeadV1, ()> for State {
    fn event(_: &mut Self, _: &cfg_head::ZwlrOutputConfigurationHeadV1, _: cfg_head::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<gmgr::ZwlrGammaControlManagerV1, ()> for State {
    fn event(_: &mut Self, _: &gmgr::ZwlrGammaControlManagerV1, _: gmgr::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<gctl::ZwlrGammaControlV1, String> for State {
    fn event(state: &mut Self, _: &gctl::ZwlrGammaControlV1, event: gctl::Event, output: &String, _: &Connection, _: &QueueHandle<Self>) {
        match event {
            gctl::Event::GammaSize { size } => {
                if let Some(st) = state.gammas.get_mut(output) {
                    st.size = size;
                    write_ramp(&st.ctl, size, &st.want);
                }
            }
            gctl::Event::Failed => {
                if let Some(g) = state.gammas.remove(output) {
                    g.ctl.destroy();
                }
                let _ = state.events.send_blocking(WlEvent::GammaFailed { output: output.clone() });
            }
            _ => {}
        }
    }
}
