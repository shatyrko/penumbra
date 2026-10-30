//! The command language shared by the command line and the D-Bus interface of the running
//! application: `penumbra brightness +5`, `penumbra input HDMI-A-1 hdmi2`, ...

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::ddc::*;
use crate::hub::*;
use crate::model::*;
use crate::store::VirtualDisplay;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    List,
    /// `value`: "50" sets, "+5" / "-5" steps (percent of the slider).
    Brightness { display: String, value: String },
    Ddc { display: String, feature: String, value: String },
    Temperature { display: String, kelvin: f64 },
    Gamma { display: String, gamma: f64 },
    ResetColour { display: String },
    Mode { display: String, mode: String },
    Scale { display: String, scale: f64 },
    Rotate { display: String, degrees: i32 },
    Position { display: String, x: i32, y: i32 },
    Enable { display: String, on: bool },
    Vrr { display: String, on: bool },
    Name { display: String, nickname: String },
    ProfileSave { name: String },
    ProfileApply { name: String },
    ProfileDelete { name: String },
    Profiles,
    VirtualCreate { name: String, width: i32, height: i32, hz: u32 },
    VirtualDestroy { name: String },
    Refresh,
    Redetect,
    /// The display's light: status, own, off, on, colour <#rrggbb|look>, look <name>, brightness <50|+10|-10>
    Light { display: String, action: String, value: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    pub message: String,
    pub data: Value,
}

impl Response {
    fn ok(message: impl Into<String>) -> Response {
        Response { ok: true, message: message.into(), data: Value::Null }
    }
    fn err(message: impl Into<String>) -> Response {
        Response { ok: false, message: message.into(), data: Value::Null }
    }
    fn with(mut self, data: Value) -> Response {
        self.data = data;
        self
    }
}

/// "2560x1440@144", "2560x1440" (best refresh), "1440p" is not understood.
pub fn parse_mode(text: &str, modes: &[Mode]) -> Option<Mode> {
    let (size, hz) = match text.split_once('@') {
        Some((s, h)) => (s, h.trim_end_matches("Hz").trim_end_matches("hz").parse::<f64>().ok()),
        None => (text, None),
    };
    let (w, h) = size.split_once(['x', '×', 'X'])?;
    let (w, h): (i32, i32) = (w.trim().parse().ok()?, h.trim().parse().ok()?);
    let mut cands: Vec<&Mode> = modes.iter().filter(|m| m.width == w && m.height == h).collect();
    if cands.is_empty() {
        // Not advertised: the compositor may still take it as a custom mode.
        return hz.map(|hz| Mode { width: w, height: h, refresh_mhz: (hz * 1000.0).round() as i32, preferred: false });
    }
    match hz {
        Some(hz) => {
            cands.sort_by_key(|m| (m.refresh_mhz as f64 - hz * 1000.0).abs() as i64);
            let best = cands[0];
            if (best.hz() - hz).abs() <= 0.5 {
                Some(best.clone())
            } else {
                Some(Mode { width: w, height: h, refresh_mhz: (hz * 1000.0).round() as i32, preferred: false })
            }
        }
        None => {
            cands.sort_by_key(|m| -m.refresh_mhz);
            Some(cands[0].clone())
        }
    }
}

/// "hdmi1", "dp2", "usb-c", "0x11", "17" → input value, against the choices the monitor has.
pub fn parse_choice(text: &str, choices: &[(u32, String)]) -> Option<u32> {
    let norm = |s: &str| s.to_ascii_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>();
    let t = norm(text);
    if let Some((v, _)) = choices.iter().find(|(_, n)| norm(n) == t) {
        return Some(*v);
    }
    // Short forms: "hdmi1" for HDMI-1, "dp2" for DisplayPort-2.
    let short = t.replace("displayport", "dp");
    if let Some((v, _)) = choices.iter().find(|(_, n)| norm(n).replace("displayport", "dp") == short) {
        return Some(*v);
    }
    if let Some(h) = text.strip_prefix("0x") {
        return u32::from_str_radix(h, 16).ok();
    }
    text.parse().ok()
}

fn percent(value: &str, current: f64, step_default: f64) -> Option<f64> {
    let v = value.trim();
    let n = |s: &str| s.trim().trim_end_matches('%').parse::<f64>().ok();
    if let Some(d) = v.strip_prefix('+') {
        let d = if d.is_empty() { step_default * 100.0 } else { n(d)? };
        Some(current + d / 100.0)
    } else if let Some(d) = v.strip_prefix('-') {
        let d = if d.is_empty() { step_default * 100.0 } else { n(d)? };
        Some(current - d / 100.0)
    } else {
        Some(n(v)? / 100.0)
    }
}

pub fn execute(hub: &Hub, cmd: Command) -> Response {
    let step = hub.settings().step as f64 / 100.0;
    match cmd {
        Command::List => {
            let views = hub.views();
            let rows: Vec<Value> = views.iter().map(view_json).collect();
            Response::ok(format!("{} display(s)", views.len())).with(Value::Array(rows))
        }
        Command::Brightness { display, value } => each(hub, &display, |hub, n, v| {
            let target = percent(&value, v.brightness, step).ok_or_else(|| format!("not a brightness: {value}"))?;
            hub.set_brightness(n, target.clamp(0.0, 1.0));
            Ok(format!("{n}: brightness {}%", (target.clamp(0.0, 1.0) * 100.0).round()))
        }),
        Command::Ddc { display, feature, value } => {
            let code = match parse_feature(&feature) {
                Some(c) => c,
                None => return Response::err(format!("unknown feature: {feature}")),
            };
            each(hub, &display, |hub, n, v| {
                let d = v.ddc.as_ref().ok_or_else(|| format!("{n}: no DDC/CI"))?;
                if !d.has(code) && d.status == DdcStatus::Ready {
                    return Err(format!("{n}: the monitor has no {}", feature_name(code)));
                }
                let raw = match code {
                    VCP_INPUT | VCP_PRESET | VCP_POWER => parse_choice(&value, &d.choices(code)).ok_or_else(|| format!("not a choice: {value}"))?,
                    VCP_MUTE => match value.to_ascii_lowercase().as_str() {
                        "on" | "1" | "mute" | "true" => 1,
                        _ => 2,
                    },
                    _ => {
                        let max = d.values.get(&code).map(|x| x.max).filter(|m| *m > 0).unwrap_or(100);
                        let cur = d.values.get(&code).map(|x| x.current as f64 / max as f64).unwrap_or(0.0);
                        (percent(&value, cur, step).ok_or_else(|| format!("not a value: {value}"))?.clamp(0.0, 1.0) * max as f64).round() as u32
                    }
                };
                hub.set_ddc(n, code, raw);
                Ok(format!("{n}: {} = {raw}", feature_name(code)))
            })
        }
        Command::Temperature { display, kelvin } => each(hub, &display, |hub, n, _| {
            hub.set_temperature(n, kelvin);
            Ok(format!("{n}: {kelvin} K"))
        }),
        Command::Gamma { display, gamma } => each(hub, &display, |hub, n, _| {
            hub.set_gamma_curve(n, gamma);
            Ok(format!("{n}: gamma {gamma}"))
        }),
        Command::ResetColour { display } => each(hub, &display, |hub, n, _| {
            hub.reset_colour(n);
            Ok(format!("{n}: colour reset"))
        }),
        Command::Mode { display, mode } => layout(hub, &display, |n, v| {
            let m = parse_mode(&mode, &v.output.modes).ok_or_else(|| format!("not a mode: {mode}"))?;
            Ok(OutputRequest { name: n.into(), mode: Some((m.width, m.height, m.refresh_mhz)), ..Default::default() })
        }),
        Command::Scale { display, scale } => layout(hub, &display, |n, _| {
            if !(0.25..=4.0).contains(&scale) {
                return Err("the scale must be between 0.25 and 4".into());
            }
            Ok(OutputRequest { name: n.into(), scale: Some(scale), ..Default::default() })
        }),
        Command::Rotate { display, degrees } => layout(hub, &display, |n, v| {
            Ok(OutputRequest { name: n.into(), transform: Some(Transform::from_parts(degrees, v.output.transform.flipped())), ..Default::default() })
        }),
        Command::Position { display, x, y } => layout(hub, &display, |n, _| Ok(OutputRequest { name: n.into(), position: Some((x, y)), ..Default::default() })),
        Command::Enable { display, on } => layout(hub, &display, |n, _| Ok(OutputRequest { name: n.into(), enabled: Some(on), ..Default::default() })),
        Command::Vrr { display, on } => layout(hub, &display, |n, _| Ok(OutputRequest { name: n.into(), vrr: Some(on), ..Default::default() })),
        Command::Name { display, nickname } => each(hub, &display, |hub, _, v| {
            hub.set_nickname(&v.id, Some(nickname.clone()));
            Ok(format!("named {nickname}"))
        }),
        Command::ProfileSave { name } => {
            hub.save_profile(&name);
            Response::ok(format!("saved profile {name}"))
        }
        Command::ProfileApply { name } => match hub.apply_profile(&name) {
            Ok(n) => Response::ok(format!("applied {name} to {n} display(s)")),
            Err(e) => Response::err(e),
        },
        Command::ProfileDelete { name } => {
            hub.delete_profile(&name);
            Response::ok(format!("deleted profile {name}"))
        }
        Command::Profiles => Response::ok("profiles").with(json!(hub.profile_names())),
        Command::VirtualCreate { name, width, height, hz } => match hub.create_virtual(VirtualDisplay { name: name.clone(), width, height, hz }) {
            Ok(()) => Response::ok(format!("created virtual display {name}")),
            Err(e) => Response::err(e),
        },
        Command::VirtualDestroy { name } => match hub.destroy_virtual(&name) {
            Ok(()) => Response::ok(format!("removed virtual display {name}")),
            Err(e) => Response::err(e),
        },
        Command::Refresh => {
            hub.refresh(true);
            Response::ok("refreshing")
        }
        Command::Redetect => {
            hub.redetect();
            Response::ok("detecting monitors")
        }
        Command::Light { display, action, value } => light(hub, &display, &action, &value, step),
    }
}

fn light(hub: &Hub, display: &str, action: &str, value: &str, step: f64) -> Response {
    use crate::light::{look_label, parse_colour, LightMode, LOOKS};
    let lights = hub.lights_for(display);
    if lights.is_empty() {
        return Response::err("no display light found (a monitor light connected by USB, see the README)");
    }
    let describe = |s: &crate::light::LightSetting| match s.mode {
        LightMode::Own => "the monitor's own light".to_string(),
        LightMode::Off => "off".to_string(),
        LightMode::On => format!("{} at {}%", if s.look == "custom" { format!("#{:02x}{:02x}{:02x}", s.colour[0], s.colour[1], s.colour[2]) } else { look_label(&s.look).to_string() }, (s.brightness * 100.0).round()),
    };
    let action = action.trim().to_ascii_lowercase();
    if matches!(action.as_str(), "" | "status" | "list") {
        let text: Vec<String> = lights.iter().map(|l| format!("{} ({} lamps{}): {}", l.name, l.lamps, l.display.as_ref().map(|d| format!(", {d}")).unwrap_or_default(), describe(&l.setting))).collect();
        return Response::ok(text.join("\n")).with(json!(lights));
    }
    let change: Box<dyn Fn(&mut crate::light::LightSetting) -> Result<(), String>> = match action.as_str() {
        "own" | "auto" | "default" | "monitor" => Box::new(|s| { s.mode = LightMode::Own; Ok(()) }),
        "off" => Box::new(|s| { s.mode = LightMode::Off; Ok(()) }),
        "on" => Box::new(|s| { s.mode = LightMode::On; Ok(()) }),
        "colour" | "color" | "look" | "preset" => {
            let v = value.trim().to_ascii_lowercase();
            if let Some((key, _, _)) = LOOKS.iter().find(|l| l.0 == v || l.1.to_ascii_lowercase() == v) {
                let key = key.to_string();
                Box::new(move |s| { s.look = key.clone(); s.mode = LightMode::On; Ok(()) })
            } else if let Some(c) = parse_colour(&v) {
                Box::new(move |s| { s.look = "custom".into(); s.colour = c; s.mode = LightMode::On; Ok(()) })
            } else {
                let names: Vec<&str> = LOOKS.iter().filter(|l| l.0 != "custom").map(|l| l.0).collect();
                return Response::err(format!("not a colour: {value} (#rrggbb or one of {})", names.join(", ")));
            }
        }
        "brightness" => {
            let v = value.to_string();
            Box::new(move |s| {
                let t = percent(&v, s.brightness, step).ok_or_else(|| format!("not a brightness: {v}"))?;
                s.brightness = t.clamp(0.0, 1.0);
                if s.mode == LightMode::Own {
                    s.mode = LightMode::On;
                }
                Ok(())
            })
        }
        _ => return Response::err(format!("unknown light action: {action} (status, own, off, on, colour, brightness)")),
    };
    let mut msgs = Vec::new();
    for l in &lights {
        let mut err = None;
        let r = hub.set_light(&l.id, |s| {
            if let Err(e) = change(s) {
                err = Some(e);
            }
        });
        if let Some(e) = err {
            return Response::err(e);
        }
        match r {
            Ok(s) => msgs.push(format!("{}: {}", l.name, describe(&s))),
            Err(e) => msgs.push(format!("{}: {e}", l.name)),
        }
    }
    Response::ok(msgs.join("\n"))
}

fn each(hub: &Hub, target: &str, mut f: impl FnMut(&Hub, &str, &DisplayView) -> Result<String, String>) -> Response {
    let names = hub.resolve(target);
    if names.is_empty() {
        return Response::err(format!("no display matches \"{target}\""));
    }
    let mut msgs = Vec::new();
    let mut failed = Vec::new();
    for n in &names {
        let Some(v) = hub.view(n) else { continue };
        match f(hub, n, &v) {
            Ok(m) => msgs.push(m),
            Err(e) => failed.push(e),
        }
    }
    if !failed.is_empty() && msgs.is_empty() {
        Response::err(failed.join("; "))
    } else {
        let mut all = msgs;
        all.extend(failed);
        Response::ok(all.join("\n"))
    }
}

fn layout(hub: &Hub, target: &str, mut f: impl FnMut(&str, &DisplayView) -> Result<OutputRequest, String>) -> Response {
    let names = hub.resolve(target);
    if names.is_empty() {
        return Response::err(format!("no display matches \"{target}\""));
    }
    let mut reqs = Vec::new();
    for n in &names {
        let Some(v) = hub.view(n) else { continue };
        match f(n, &v) {
            Ok(r) => reqs.push(r),
            Err(e) => return Response::err(e),
        }
    }
    match hub.apply_layout_blocking(reqs) {
        Ok(()) => {
            // Wait for the compositor's new state to arrive.
            std::thread::sleep(Duration::from_millis(150));
            Response::ok("done")
        }
        Err(e) => Response::err(e),
    }
}

pub fn view_json(v: &DisplayView) -> Value {
    let o = &v.output;
    let mut ddc = Value::Null;
    if let Some(d) = &v.ddc {
        let mut vals = serde_json::Map::new();
        for (c, x) in &d.values {
            vals.insert(feature_name(*c).to_lowercase().replace(' ', "_"), json!({ "code": format!("0x{c:02X}"), "value": x.current, "max": x.max }));
        }
        ddc = json!({ "bus": d.bus, "status": format!("{:?}", d.status), "features": vals });
    }
    json!({
        "name": v.name,
        "title": v.display_name(),
        "control": format!("{:?}", v.control).to_lowercase(),
        "enabled": o.enabled,
        "mode": o.mode.as_ref().map(|m| m.label()),
        "scale": o.scale,
        "position": [o.position.0, o.position.1],
        "transform": o.transform.degrees(),
        "vrr": o.vrr,
        "brightness_percent": (v.brightness * 100.0).round(),
        "temperature": v.gamma.temperature,
        "ddc": ddc,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modes() -> Vec<Mode> {
        vec![
            Mode { width: 3840, height: 2160, refresh_mhz: 164991, preferred: true },
            Mode { width: 3840, height: 2160, refresh_mhz: 60000, preferred: false },
            Mode { width: 2560, height: 1440, refresh_mhz: 144000, preferred: false },
        ]
    }

    #[test]
    fn mode_parsing() {
        assert_eq!(parse_mode("3840x2160@165", &modes()).unwrap().refresh_mhz, 164991);
        assert_eq!(parse_mode("3840x2160@60", &modes()).unwrap().refresh_mhz, 60000);
        assert_eq!(parse_mode("3840x2160", &modes()).unwrap().refresh_mhz, 164991);
        assert_eq!(parse_mode("1920x1080@75", &modes()).unwrap().refresh_mhz, 75000);
        assert!(parse_mode("1920x1080", &modes()).is_none());
        assert!(parse_mode("nonsense", &modes()).is_none());
    }

    #[test]
    fn choices() {
        let c = default_choices(VCP_INPUT);
        assert_eq!(parse_choice("hdmi1", &c), Some(0x11));
        assert_eq!(parse_choice("HDMI-2", &c), Some(0x12));
        assert_eq!(parse_choice("dp1", &c), Some(0x0f));
        assert_eq!(parse_choice("0x1b", &c), Some(0x1b));
        assert_eq!(parse_choice("usbc", &c), Some(0x1b));
        assert_eq!(parse_choice("zzz", &c), None);
    }

    #[test]
    fn percent_steps() {
        assert_eq!(percent("50", 0.2, 0.05), Some(0.5));
        assert!((percent("+10", 0.2, 0.05).unwrap() - 0.3).abs() < 1e-9);
        assert!((percent("-", 0.2, 0.05).unwrap() - 0.15).abs() < 1e-9);
        assert_eq!(percent("abc", 0.2, 0.05), None);
    }
}
