use penumbra::model::*;
use penumbra::wl::*;
use std::time::Duration;

fn wait_outputs(rx: &async_channel::Receiver<WlEvent>, pred: impl Fn(&[OutputInfo]) -> bool) -> Vec<OutputInfo> {
    loop {
        match rx.recv_blocking_timeout(Duration::from_secs(5)) {
            Some(WlEvent::Outputs(o)) if pred(&o) => return o,
            Some(other) => println!("  event: {:?}", match &other { WlEvent::Outputs(o) => format!("outputs({})", o.len()), x => format!("{x:?}") }),
            None => panic!("timeout"),
        }
    }
}
trait RecvT { fn recv_blocking_timeout(&self, d: Duration) -> Option<WlEvent>; }
impl RecvT for async_channel::Receiver<WlEvent> {
    fn recv_blocking_timeout(&self, d: Duration) -> Option<WlEvent> {
        let end = std::time::Instant::now() + d;
        loop {
            if let Ok(v) = self.try_recv() { return Some(v); }
            if std::time::Instant::now() > end { return None; }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
fn main() {
    let (h, rx) = spawn().unwrap();
    let outs = wait_outputs(&rx, |_| true);
    for o in &outs { println!("{} {} {:?} scale {} pos {:?} vrr {:?} modes {}", o.name, o.title(), o.mode.as_ref().map(|m| m.label()), o.scale, o.position, o.vrr, o.modes.len()); }
    let name = "PTest-1";
    std::process::Command::new("umbriel").args(["output-create", name, "1920x1080@60"]).status().unwrap();
    let outs = wait_outputs(&rx, |o| o.iter().any(|x| x.name == name));
    let v = outs.iter().find(|x| x.name == name).unwrap();
    println!("virtual: {:?}", v);
    h.send(WlCmd::Apply { token: 1, requests: vec![OutputRequest { name: name.into(), mode: Some((2560, 1440, 120000)), scale: Some(1.5), transform: Some(Transform::R90), ..Default::default() }] });
    loop { match rx.recv_blocking_timeout(Duration::from_secs(5)) { Some(WlEvent::Applied { ok, message, .. }) => { println!("applied ok={ok} {message}"); break } Some(_) => {}, None => panic!("no apply result") } }
    let outs = wait_outputs(&rx, |o| o.iter().any(|x| x.name == name && x.scale == 1.5));
    println!("after: {:?}", outs.iter().find(|x| x.name == name));
    h.send(WlCmd::SetGamma { output: name.into(), gamma: Some(Gamma { brightness: 0.3, temperature: 4000.0, gamma: 1.0 }) });
    std::thread::sleep(Duration::from_millis(800));
    while let Ok(e) = rx.try_recv() { println!("gamma-phase event: {:?}", match &e { WlEvent::Outputs(_) => "outputs".to_string(), x => format!("{x:?}") }); }
    h.send(WlCmd::SetGamma { output: name.into(), gamma: None });
    std::thread::sleep(Duration::from_millis(200));
    std::process::Command::new("umbriel").args(["output-destroy", name]).status().unwrap();
    wait_outputs(&rx, |o| !o.iter().any(|x| x.name == name));
    println!("cleaned up");
    h.send(WlCmd::Stop);
}
