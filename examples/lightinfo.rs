//! List the LampArray lights (reads their layout, changes nothing): cargo run --example lightinfo
fn main() {
    for f in penumbra::light::scan() {
        println!("{}  {}  id={}  reports={:?}", f.dev.display(), f.name, f.id, f.reports);
        match penumbra::light::LampArray::open(f) {
            Ok(l) => {
                let (xs, ys): (Vec<f32>, Vec<f32>) = l.lamps.iter().copied().unzip();
                let mm = |v: &[f32]| (v.iter().cloned().fold(f32::MAX, f32::min), v.iter().cloned().fold(f32::MIN, f32::max));
                println!("  {} lamps, x {:?} mm, y {:?} mm", l.lamps.len(), mm(&xs), mm(&ys));
            }
            Err(e) => println!("  can't open: {e}"),
        }
    }
}
