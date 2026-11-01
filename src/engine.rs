//! One worker thread owns every `ApkSession` and the JADX JVM. rust_ASC's DEX model is `Rc`
//! based (not `Send`), so callers on any thread send jobs over a channel and wait for the answer.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::Instant;

use crate::apk::ApkSession;
use crate::class_name::ClassName;
use crate::error::{Error, Result};
use crate::jadx::JadxWorker;

const MAX_SESSIONS: usize = 8;

/// Per-stage milliseconds of one `getclass`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Timing {
    pub locate: f64,
    pub context: f64,
    pub extract: f64,
    pub jadx: f64,
}

impl fmt::Display for Timing {
    /// The line `serve --timing` prints on stderr; external benchmark harnesses parse it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "timing ms: locate={:.1}  context={:.1}  extract={:.1}  jadx={:.1}",
            self.locate, self.context, self.extract, self.jadx
        )
    }
}

struct Stopwatch(Instant);

impl Stopwatch {
    fn lap(&mut self) -> f64 {
        let now = Instant::now();
        let ms = now.duration_since(self.0).as_secs_f64() * 1000.0;
        self.0 = now;
        ms
    }
}

pub struct Decompiled {
    pub code: String,
    pub timing: Timing,
}

enum Job {
    Open {
        apk: PathBuf,
        reply: Sender<Result<()>>,
    },
    GetClass {
        apk: PathBuf,
        class: ClassName,
        context: bool,
        reply: Sender<Result<Decompiled>>,
    },
}

/// Handle to the worker thread. Clones share the one thread, so every APK and every request
/// in the process goes through the same sessions and the same JVM.
#[derive(Clone)]
pub struct Engine {
    tx: Sender<Job>,
}

impl Engine {
    /// Starts the worker thread. Drop every `Engine` clone, then join the handle: that closes
    /// the JVM cleanly.
    pub fn start() -> (Engine, JoinHandle<()>) {
        let (tx, rx) = channel();
        let handle = std::thread::spawn(move || Worker::default().run(rx));
        (Engine { tx }, handle)
    }

    /// Opens (or finds) the session for `apk`, so a bad path fails here instead of on first use.
    pub fn open(&self, apk: &Path) -> Result<()> {
        let (reply, rx) = channel();
        self.send(
            Job::Open {
                apk: apk.to_path_buf(),
                reply,
            },
            rx,
        )
    }

    pub fn getclass(&self, apk: &Path, class: &ClassName, context: bool) -> Result<Decompiled> {
        let (reply, rx) = channel();
        let job = Job::GetClass {
            apk: apk.to_path_buf(),
            class: class.clone(),
            context,
            reply,
        };
        self.send(job, rx)
    }

    fn send<T>(&self, job: Job, rx: Receiver<Result<T>>) -> Result<T> {
        let died = || Error::Internal("engine thread is gone".into());
        self.tx.send(job).map_err(|_| died())?;
        rx.recv().map_err(|_| died())?
    }
}

#[derive(Default)]
struct Worker {
    jadx: JadxWorker,
    /// Least recently used first.
    sessions: Vec<(PathBuf, ApkSession)>,
}

impl Worker {
    fn run(mut self, rx: Receiver<Job>) {
        for job in rx {
            match job {
                Job::Open { apk, reply } => {
                    let _ = reply.send(self.session(&apk).map(|_| ()));
                }
                Job::GetClass {
                    apk,
                    class,
                    context,
                    reply,
                } => {
                    let _ = reply.send(self.getclass(&apk, &class, context));
                }
            }
        }
    }

    fn session(&mut self, apk: &Path) -> Result<&mut ApkSession> {
        let key = std::fs::canonicalize(apk)
            .map_err(|e| Error::Apk(format!("cannot open APK {}: {e}", apk.display())))?;
        let entry = match self.sessions.iter().position(|(k, _)| *k == key) {
            Some(i) => self.sessions.remove(i),
            None => (key.clone(), ApkSession::open(&key)?),
        };
        self.sessions.push(entry);
        if self.sessions.len() > MAX_SESSIONS {
            self.sessions.remove(0);
        }
        Ok(&mut self.sessions.last_mut().unwrap().1)
    }

    fn getclass(&mut self, apk: &Path, class: &ClassName, context: bool) -> Result<Decompiled> {
        let mut watch = Stopwatch(Instant::now());
        let mut timing = Timing::default();
        let session = self.session(apk)?;
        if session.find(class)?.is_none() {
            return Err(Error::ClassNotFound(class.dotted()));
        }
        timing.locate = watch.lap();
        let classes = if context {
            session.context(class)?
        } else {
            vec![class.clone()]
        };
        timing.context = watch.lap();
        let inputs = if context {
            session.extract_with_owners(class, &classes)?
        } else {
            vec![session.extract(class)?]
        };
        timing.extract = watch.lap();
        let code = self.jadx.decompile(class, &inputs)?;
        timing.jadx = watch.lap();
        Ok(Decompiled { code, timing })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timing_line_has_the_format_the_benchmark_harness_parses() {
        let t = Timing {
            locate: 0.04,
            context: 1.26,
            extract: 3.0,
            jadx: 33.333,
        };
        assert_eq!(
            t.to_string(),
            "timing ms: locate=0.0  context=1.3  extract=3.0  jadx=33.3"
        );
    }
}
