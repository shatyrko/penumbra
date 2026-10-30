use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};

use penumbra::commands::{execute, Command, Response};
use penumbra::hub::{Hub, HubOptions};
use penumbra::{ipc, ui};

#[derive(Parser)]
#[command(name = "penumbra", version, about = "Display control for Wayland: brightness, DDC/CI, resolution, scaling, virtual screens. Run without a command to start the tray app.")]
struct Cli {
    /// Which display: focused, all, internal, external, a connector (HDMI-A-1), a number or part of a name
    #[arg(short, long, global = true, default_value = "focused")]
    display: String,
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Start the tray app (the default)
    Run,
    /// Open or close the panel of the running app
    Toggle,
    Show,
    Hide,
    /// Open the settings window
    Settings {
        /// displays, arrangement, virtual, profiles, general, about
        page: Option<String>,
    },
    /// Quit the running app
    Quit,
    /// List the displays
    List {
        #[arg(long)]
        json: bool,
    },
    /// Set brightness in percent: 50, +5, -5 (alone: +/- one step)
    Brightness { value: String },
    /// Set a monitor feature: contrast, volume, input, mute, preset, power, sharpness, red, green, blue, or a hex code
    Set { feature: String, value: String },
    /// Colour temperature in kelvin (6500 = neutral), through the compositor
    Temperature { kelvin: f64 },
    /// Gamma curve (1.0 = neutral)
    Gamma { gamma: f64 },
    /// Back to neutral colour (6500 K, gamma 1.0)
    ResetColour,
    /// Resolution and refresh rate: 2560x1440@144
    Mode { mode: String },
    /// Scale factor: 1, 1.25, 1.5, 2 ...
    Scale { scale: f64 },
    /// Rotate: 0, 90, 180, 270
    Rotate { degrees: i32 },
    /// Position in the global layout
    Position { x: i32, y: i32 },
    /// Turn the output on
    Enable,
    /// Turn the output off
    Disable,
    /// Adaptive sync on or off
    Vrr { state: OnOff },
    /// Give a display a nickname
    Name { nickname: String },
    /// Saved setups of every display
    Profile {
        #[command(subcommand)]
        action: ProfileCmd,
    },
    /// Virtual screens (headless outputs for streaming, VNC, extra space)
    Virtual {
        #[command(subcommand)]
        action: VirtualCmd,
    },
    /// Read the monitors' values again
    Refresh,
    /// Probe for monitors again
    Detect,
    /// Start with the session (on) or not (off)
    Autostart { state: OnOff },
    /// Put a launcher entry and the icon into your applications menu
    InstallLauncher,
    /// The monitor's light (LampArray over USB): status, own, off, on, colour <#rrggbb|white|warm|purple|blue|red|sunset|ocean|aurora>, brightness <50|+10|-10>
    Light {
        action: Option<String>,
        value: Option<String>,
    },
    /// Find a monitor's own settings in DDC/CI (read-only, never writes to the monitor)
    Probe {
        #[command(subcommand)]
        action: ProbeCmd,
    },
}

#[derive(Subcommand)]
enum ProbeCmd {
    /// Read every code the monitor answers, with its value
    Scan {
        #[arg(long)]
        json: bool,
    },
    /// Save the current values under a name
    Snap { name: String },
    /// Saved snapshots
    List,
    /// What changed between two snapshots (or one snapshot and now)
    Diff { a: String, b: Option<String> },
    /// Live: change a setting in the monitor's menu and see which code moves (Ctrl+C stops)
    Watch,
}

#[derive(clap::ValueEnum, Clone, Copy)]
enum OnOff {
    On,
    Off,
}

#[derive(Subcommand)]
enum ProfileCmd {
    Save { name: String },
    Apply { name: String },
    Delete { name: String },
    List,
}

#[derive(Subcommand)]
enum VirtualCmd {
    /// penumbra virtual create Stream 2560x1440@60
    Create { name: String, mode: Option<String> },
    Destroy { name: String },
}

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let cli = Cli::parse();
    let d = cli.display.clone();
    let cmd = match cli.command.unwrap_or(Cmd::Run) {
        Cmd::Run => return ui::run(),
        Cmd::Toggle => return simple("Toggle"),
        Cmd::Show => return simple("Show"),
        Cmd::Hide => return simple("Hide"),
        Cmd::Quit => return simple("Quit"),
        Cmd::Settings { page } => {
            return match ipc::send_settings(&page.unwrap_or_default()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => fail(&format!("the app is not running ({e}); start it with `penumbra`")),
            }
        }
        Cmd::Autostart { state } => return autostart(matches!(state, OnOff::On)),
        Cmd::Probe { action } => {
            let r = match action {
                ProbeCmd::Scan { json } => penumbra::probe::scan(&d, json),
                ProbeCmd::Snap { name } => penumbra::probe::snap(&d, &name),
                ProbeCmd::List => penumbra::probe::list(),
                ProbeCmd::Diff { a, b } => penumbra::probe::diff_cmd(&d, &a, b.as_deref()),
                ProbeCmd::Watch => penumbra::probe::watch(&d),
            };
            return match r {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => fail(&e),
            };
        }
        Cmd::InstallLauncher => {
            return match penumbra::autostart::install_launcher() {
                Ok(p) => {
                    println!("installed {}", p.display());
                    ExitCode::SUCCESS
                }
                Err(e) => fail(&e),
            }
        }
        Cmd::List { json } => {
            let resp = run(Command::List);
            if json {
                println!("{}", serde_json::to_string_pretty(&resp.data).unwrap_or_default());
            } else {
                print_list(&resp);
            }
            return code(&resp);
        }
        Cmd::Brightness { value } => Command::Brightness { display: d, value },
        Cmd::Set { feature, value } => Command::Ddc { display: d, feature, value },
        Cmd::Temperature { kelvin } => Command::Temperature { display: d, kelvin },
        Cmd::Gamma { gamma } => Command::Gamma { display: d, gamma },
        Cmd::ResetColour => Command::ResetColour { display: d },
        Cmd::Mode { mode } => Command::Mode { display: d, mode },
        Cmd::Scale { scale } => Command::Scale { display: d, scale },
        Cmd::Rotate { degrees } => Command::Rotate { display: d, degrees },
        Cmd::Position { x, y } => Command::Position { display: d, x, y },
        Cmd::Enable => Command::Enable { display: d, on: true },
        Cmd::Disable => Command::Enable { display: d, on: false },
        Cmd::Vrr { state } => Command::Vrr { display: d, on: matches!(state, OnOff::On) },
        Cmd::Name { nickname } => Command::Name { display: d, nickname },
        Cmd::Light { action, value } => Command::Light { display: d, action: action.unwrap_or_default(), value: value.unwrap_or_default() },
        Cmd::Profile { action } => match action {
            ProfileCmd::Save { name } => Command::ProfileSave { name },
            ProfileCmd::Apply { name } => Command::ProfileApply { name },
            ProfileCmd::Delete { name } => Command::ProfileDelete { name },
            ProfileCmd::List => {
                let resp = run(Command::Profiles);
                if let Some(a) = resp.data.as_array() {
                    for n in a {
                        println!("{}", n.as_str().unwrap_or(""));
                    }
                }
                return code(&resp);
            }
        },
        Cmd::Virtual { action } => match action {
            VirtualCmd::Create { name, mode } => {
                let m = mode.unwrap_or_else(|| "1920x1080@60".into());
                let parsed = m.split_once('@').map(|(s, h)| (s.to_string(), h.parse::<u32>().ok())).unwrap_or((m.clone(), Some(60)));
                let Some((w, h)) = parsed.0.split_once('x').and_then(|(w, h)| Some((w.parse::<i32>().ok()?, h.parse::<i32>().ok()?))) else {
                    return fail(&format!("not a mode: {m}"));
                };
                Command::VirtualCreate { name, width: w, height: h, hz: parsed.1.unwrap_or(60) }
            }
            VirtualCmd::Destroy { name } => Command::VirtualDestroy { name },
        },
        Cmd::Refresh => Command::Refresh,
        Cmd::Detect => Command::Redetect,
    };
    let resp = run(cmd);
    if !resp.message.is_empty() {
        if resp.ok {
            println!("{}", resp.message);
        } else {
            eprintln!("penumbra: {}", resp.message);
        }
    }
    code(&resp)
}

fn code(r: &Response) -> ExitCode {
    if r.ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn fail(m: &str) -> ExitCode {
    eprintln!("penumbra: {m}");
    ExitCode::FAILURE
}

fn simple(method: &str) -> ExitCode {
    match ipc::send_simple(method) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(&format!("the app is not running ({e}); start it with `penumbra`")),
    }
}

/// Through the running app when there is one, else with a short-lived hub of our own.
fn run(cmd: Command) -> Response {
    if ipc::running() {
        return ipc::send_command(&cmd).unwrap_or_else(|e| Response { ok: false, message: e, data: serde_json::Value::Null });
    }
    let (hub, _rx) = match Hub::start(HubOptions::default()) {
        Ok(h) => h,
        Err(e) => return Response { ok: false, message: e, data: serde_json::Value::Null },
    };
    let needs_dimming = matches!(cmd, Command::Temperature { .. } | Command::Gamma { .. } | Command::Brightness { .. });
    if let Command::Light { action, .. } = &cmd {
        // A light needs no monitor probing: find it, set it, done.
        let changes = !matches!(action.trim().to_ascii_lowercase().as_str(), "" | "status" | "list");
        hub.wait_lights(Duration::from_secs(5));
        let mut resp = execute(&hub, cmd);
        if resp.ok && changes && !resp.message.contains("own light") {
            resp.message.push_str("\nnote: without the penumbra app running, a monitor that sleeps goes back to its own light");
        }
        hub.shutdown();
        return resp;
    }
    let waited = Hub::wait_settled(&hub, Duration::from_secs(25));
    let mut resp = execute(&hub, cmd);
    hub.drain(Duration::from_secs(10));
    std::thread::sleep(Duration::from_millis(250));
    if !waited {
        resp.message.push_str("\n(monitors were still being probed when the command ran)");
    }
    if needs_dimming && resp.ok {
        resp.message.push_str("\nnote: software dimming and colour only last while the penumbra app runs; start it with `penumbra`");
    }
    hub.shutdown();
    resp
}

fn print_list(resp: &Response) {
    let Some(rows) = resp.data.as_array() else {
        eprintln!("penumbra: {}", resp.message);
        return;
    };
    for (i, r) in rows.iter().enumerate() {
        let s = |k: &str| r[k].as_str().unwrap_or("").to_string();
        println!(
            "{}  {:<12} {:<28} {:<24} scale {:<5} brightness {:>3}% ({})",
            i + 1,
            s("name"),
            s("title"),
            r["mode"].as_str().unwrap_or("off"),
            r["scale"].as_f64().map(|x| format!("{x:.2}")).unwrap_or_default(),
            r["brightness_percent"].as_f64().unwrap_or(0.0),
            s("control")
        );
        if let Some(f) = r["ddc"]["features"].as_object() {
            let parts: Vec<String> = f.iter().map(|(k, v)| format!("{k}={}", v["value"])).collect();
            println!("   monitor: {}", parts.join("  "));
        }
    }
}

fn autostart(on: bool) -> ExitCode {
    match penumbra::autostart::set(on) {
        Ok(p) if on => {
            println!("Penumbra starts with the session ({})", p.display());
            ExitCode::SUCCESS
        }
        Ok(_) => {
            println!("Penumbra no longer starts with the session");
            ExitCode::SUCCESS
        }
        Err(e) => fail(&e),
    }
}
