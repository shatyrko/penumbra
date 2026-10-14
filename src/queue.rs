//! A worker thread fed by a queue in which a newer job of the same kind replaces the older
//! one: dragging a slider sends hundreds of values, the monitor needs only the last.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

pub struct JobQueue<J> {
    inner: Mutex<(VecDeque<J>, bool)>,
    cv: Condvar,
    active: AtomicBool,
}

impl<J: Send + 'static> JobQueue<J> {
    /// `replaces(old, new)` says whether `new` makes `old` pointless.
    pub fn spawn(name: &str, mut run: impl FnMut(J) + Send + 'static) -> Arc<JobQueue<J>> {
        let q = Arc::new(JobQueue { inner: Mutex::new((VecDeque::new(), false)), cv: Condvar::new(), active: AtomicBool::new(false) });
        let q2 = q.clone();
        std::thread::Builder::new()
            .name(name.to_string())
            .spawn(move || loop {
                let job = {
                    let mut g = q2.inner.lock().unwrap();
                    loop {
                        if let Some(j) = g.0.pop_front() {
                            break Some(j);
                        }
                        if g.1 {
                            break None;
                        }
                        g = q2.cv.wait(g).unwrap();
                    }
                };
                match job {
                    Some(j) => {
                        q2.active.store(true, Ordering::SeqCst);
                        run(j);
                        q2.active.store(false, Ordering::SeqCst);
                    }
                    None => return,
                }
            })
            .expect("spawn worker");
        q
    }

    pub fn push(&self, job: J, replaces: impl Fn(&J, &J) -> bool) {
        let mut g = self.inner.lock().unwrap();
        g.0.retain(|old| !replaces(old, &job));
        g.0.push_back(job);
        self.cv.notify_one();
    }

    /// Nothing queued and nothing running.
    pub fn idle(&self) -> bool {
        self.pending() == 0 && !self.active.load(Ordering::SeqCst)
    }

    pub fn pending(&self) -> usize {
        self.inner.lock().unwrap().0.len()
    }

    pub fn stop(&self) {
        self.inner.lock().unwrap().1 = true;
        self.cv.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn newer_replaces_older() {
        let (tx, rx) = mpsc::channel();
        let gate = Arc::new(Mutex::new(()));
        let g2 = gate.clone();
        let hold = gate.lock().unwrap();
        let q = JobQueue::spawn("t", move |j: (u8, u32)| {
            let _l = g2.lock().unwrap();
            tx.send(j).unwrap();
        });
        q.push((1, 1), |a, b| a.0 == b.0); // taken by the worker, which then blocks on the gate
        std::thread::sleep(Duration::from_millis(50));
        q.push((1, 2), |a, b| a.0 == b.0);
        q.push((1, 3), |a, b| a.0 == b.0);
        q.push((2, 9), |a, b| a.0 == b.0);
        drop(hold);
        let got: Vec<_> = (0..3).map(|_| rx.recv_timeout(Duration::from_secs(2)).unwrap()).collect();
        assert_eq!(got, vec![(1, 1), (1, 3), (2, 9)]);
        q.stop();
    }
}
