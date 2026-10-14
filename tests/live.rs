//! Runs against the real compositor (Umbriel), using a virtual output and a mock monitor.
//! Skipped when there is no Wayland session or no umbriel.

use std::sync::Arc;
use std::time::Duration;

use penumbra::commands::{execute, Command};
use penumbra::compositor;
use penumbra::ddc::*;
use penumbra::hub::*;

const OUT: &str = "PTEST-1";

fn setup() -> Option<(Hub, Arc<Mock>, std::path::PathBuf)> {
    if std::env::var("WAYLAND_DISPLAY").is_err() || !compositor::available() {
        eprintln!("skipped: no wayland session / umbriel");
        return None;
    }
    let _ = compositor::destroy_virtual(OUT);
    compositor::create_virtual(OUT, 1920, 1080, 60).expect("create virtual output");
    let mock = Arc::new(Mock::new(&[OUT.to_string()]));
    let path = std::env::temp_dir().join(format!("penumbra-live-{}.json", std::process::id()));
    let (hub, _rx) = Hub::start(HubOptions { backend: Some(mock.clone()), store_path: Some(path.clone()), backlight: None }).unwrap();
    Some((hub, mock, path))
}

fn ok(r: penumbra::commands::Response) -> penumbra::commands::Response {
    assert!(r.ok, "command failed: {}", r.message);
    r
}

#[test]
fn full_flow() {
    let Some((hub, mock, path)) = setup() else { return };
    assert!(hub.wait_until(Duration::from_secs(10), |v| v.iter().any(|d| d.name == OUT && d.ddc.as_ref().map(|x| x.status == DdcStatus::Ready).unwrap_or(false))), "mock monitor never became ready");
    let bus = hub.view(OUT).unwrap().ddc.unwrap().bus;

    // Brightness: combined slider 60% → hardware (60-15)/85
    let initial = hub.view(OUT).unwrap();
    assert_eq!(initial.control, Control::Ddc);
    ok(execute(&hub, Command::Brightness { display: OUT.into(), value: "100".into() }));
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(mock.get(bus, &[VCP_BRIGHTNESS]).unwrap()[&VCP_BRIGHTNESS].current, 100);
    ok(execute(&hub, Command::Brightness { display: OUT.into(), value: "-10".into() }));
    std::thread::sleep(Duration::from_millis(500));
    let hw = mock.get(bus, &[VCP_BRIGHTNESS]).unwrap()[&VCP_BRIGHTNESS].current;
    assert_eq!(hw, ((0.9f64 - 0.15) / 0.85 * 100.0).round() as u32);

    // Below the hardware range: hardware 0 + software dim
    ok(execute(&hub, Command::Brightness { display: OUT.into(), value: "5".into() }));
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(mock.get(bus, &[VCP_BRIGHTNESS]).unwrap()[&VCP_BRIGHTNESS].current, 0);
    let g = hub.view(OUT).unwrap().gamma;
    assert!(g.brightness < 1.0 && g.brightness > 0.05, "dim {}", g.brightness);

    // Choices and plain features
    ok(execute(&hub, Command::Ddc { display: OUT.into(), feature: "input".into(), value: "hdmi2".into() }));
    ok(execute(&hub, Command::Ddc { display: OUT.into(), feature: "volume".into(), value: "25".into() }));
    ok(execute(&hub, Command::Ddc { display: OUT.into(), feature: "contrast".into(), value: "+10".into() }));
    std::thread::sleep(Duration::from_millis(900));
    let m = mock.get(bus, &[VCP_INPUT, VCP_VOLUME, VCP_CONTRAST]).unwrap();
    assert_eq!(m[&VCP_INPUT].current, 0x12);
    assert_eq!(m[&VCP_VOLUME].current, 25);
    assert_eq!(m[&VCP_CONTRAST].current, 60);
    assert!(!execute(&hub, Command::Ddc { display: OUT.into(), feature: "input".into(), value: "nonsense".into() }).ok);

    // Colour temperature
    ok(execute(&hub, Command::Temperature { display: OUT.into(), kelvin: 4500.0 }));
    assert_eq!(hub.view(OUT).unwrap().gamma.temperature, 4500.0);
    ok(execute(&hub, Command::ResetColour { display: OUT.into() }));

    // Layout: mode, scale, rotation
    ok(execute(&hub, Command::Mode { display: OUT.into(), mode: "2560x1440@120".into() }));
    ok(execute(&hub, Command::Scale { display: OUT.into(), scale: 1.25 }));
    ok(execute(&hub, Command::Rotate { display: OUT.into(), degrees: 90 }));
    assert!(hub.wait_until(Duration::from_secs(5), |v| v.iter().any(|d| d.name == OUT && d.output.scale == 1.25 && d.output.transform.degrees() == 90)));
    let v = hub.view(OUT).unwrap();
    assert_eq!(v.output.mode.as_ref().unwrap().width, 2560);
    assert_eq!(v.output.logical_size(), (1152, 2048)); // 1440/1.25 wide after rotation, 2560/1.25 tall

    // Profiles
    ok(execute(&hub, Command::ProfileSave { name: "t".into() }));
    ok(execute(&hub, Command::Rotate { display: OUT.into(), degrees: 0 }));
    ok(execute(&hub, Command::Scale { display: OUT.into(), scale: 1.0 }));
    ok(execute(&hub, Command::ProfileApply { name: "t".into() }));
    assert!(hub.wait_until(Duration::from_secs(5), |v| v.iter().any(|d| d.name == OUT && d.output.scale == 1.25 && d.output.transform.degrees() == 90)));

    // List renders
    let list = ok(execute(&hub, Command::List));
    assert!(list.data.as_array().unwrap().iter().any(|d| d["name"] == OUT && d["ddc"]["status"] == "Ready"));

    // Persistence
    hub.flush();
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(saved.contains("\"t\"") && saved.contains("PTEST-1"));

    hub.shutdown();
    let _ = compositor::destroy_virtual(OUT);
    let _ = std::fs::remove_file(path);
}
