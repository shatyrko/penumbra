use penumbra::model::*;
use penumbra::wl::*;
use std::time::Duration;
fn main() {
    let (h, rx) = spawn().unwrap();
    std::thread::sleep(Duration::from_millis(500));
    h.send(WlCmd::SetGamma { output: "eDP-1".into(), gamma: Some(Gamma { brightness: 0.85, temperature: 6000.0, gamma: 1.0 }) });
    std::thread::sleep(Duration::from_millis(1500));
    while let Ok(e) = rx.try_recv() { if let WlEvent::Outputs(_) = e {} else { println!("{e:?}"); } }
    println!("gamma set (no failure above = accepted)");
    h.send(WlCmd::SetGamma { output: "eDP-1".into(), gamma: None });
    std::thread::sleep(Duration::from_millis(300));
    h.send(WlCmd::Stop);
}
