//! The JADX side: provisioning the jar and the compiled worker, and one long-lived JVM that
//! answers JSON-lines decompile requests.

use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::apk::MiniDex;
use crate::class_name::ClassName;
use crate::error::{Error, Result};

pub const JADX_VERSION: &str = "1.5.6";
const JADX_URL: &str = "https://github.com/skylot/jadx/releases/download/v1.5.6/jadx-1.5.6.zip";
const JADX_SHA256: &str = "545ea2be9c242511bc145755cf4bda2485ade42966e096f8b4d3da2a230e8974";

const WORKER_SRC: &str = include_str!("../java/DecompileWorker.java");
const CLSP_SRC: &str = include_str!("../java/ClspGraph.java");

pub(crate) fn home() -> Result<PathBuf> {
    if let Some(h) = std::env::var_os("DROIDLENS_HOME") {
        return Ok(PathBuf::from(h));
    }
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".cache").join("droidlens"))
        .ok_or_else(|| Error::Internal("neither DROIDLENS_HOME nor HOME is set".into()))
}

/// Runs `build(tmp)` then renames `tmp` onto `dest`; a second run with `dest` present does nothing.
/// Processes racing on a cold cache each build their own `tmp`; a loser whose directory rename
/// fails because the winner's `dest` now exists uses the winner's identical copy.
pub(crate) fn atomic(dest: &Path, build: impl FnOnce(&Path) -> Result<()>) -> Result<()> {
    if dest.exists() {
        return Ok(());
    }
    let parent = dest.parent().expect("dest has a parent");
    fs::create_dir_all(parent)
        .map_err(|e| Error::Internal(format!("cannot create {}: {e}", parent.display())))?;
    let name = dest.file_name().unwrap().to_string_lossy();
    let tmp = parent.join(format!(".{name}.{}.tmp", std::process::id()));
    let result = build(&tmp).and_then(|()| match fs::rename(&tmp, dest) {
        Err(_) if dest.exists() => Ok(()),
        r => r.map_err(|e| Error::Internal(format!("cannot rename onto {}: {e}", dest.display()))),
    });
    if tmp.is_dir() {
        let _ = fs::remove_dir_all(&tmp);
    } else {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn fetch_jar(dest: &Path) -> Result<()> {
    let zip_path = dest.with_extension("zip.part");
    let result = (|| {
        let mut resp = ureq::get(JADX_URL)
            .call()
            .map_err(|e| Error::Internal(format!("cannot download {JADX_URL}: {e}")))?;
        let mut file = fs::File::create(&zip_path)
            .map_err(|e| Error::Internal(format!("cannot create {}: {e}", zip_path.display())))?;
        io::copy(&mut resp.body_mut().as_reader(), &mut file)
            .map_err(|e| Error::Internal(format!("download of {JADX_URL} failed: {e}")))?;
        drop(file);
        let bytes = fs::read(&zip_path).map_err(|e| Error::Internal(e.to_string()))?;
        let digest = format!("{:x}", Sha256::digest(&bytes));
        if digest != JADX_SHA256 {
            return Err(Error::Internal(format!(
                "JADX download sha256 mismatch: {digest}"
            )));
        }
        let mut archive = zip::ZipArchive::new(io::Cursor::new(bytes))
            .map_err(|e| Error::Internal(format!("JADX zip is unreadable: {e}")))?;
        let mut jar = archive
            .by_name(&format!("lib/jadx-{JADX_VERSION}-all.jar"))
            .map_err(|e| Error::Internal(format!("JADX zip has no jar: {e}")))?;
        let mut out = fs::File::create(dest).map_err(|e| Error::Internal(e.to_string()))?;
        io::copy(&mut jar, &mut out)
            .map_err(|e| Error::Internal(format!("cannot write jar: {e}")))?;
        Ok(())
    })();
    let _ = fs::remove_file(&zip_path);
    result
}

fn compile_worker(jar: &Path, dest: &Path) -> Result<()> {
    let src = dest.with_extension("src");
    let _ = fs::remove_dir_all(&src);
    fs::create_dir_all(&src)
        .map_err(|e| Error::Internal(format!("cannot create {}: {e}", src.display())))?;
    fs::create_dir_all(dest)
        .map_err(|e| Error::Internal(format!("cannot create {}: {e}", dest.display())))?;
    let worker = src.join("DecompileWorker.java");
    let clsp = src.join("ClspGraph.java");
    fs::write(&worker, WORKER_SRC)
        .and_then(|()| fs::write(&clsp, CLSP_SRC))
        .map_err(|e| Error::Internal(format!("cannot write worker sources: {e}")))?;
    let out = Command::new("javac")
        .arg("-cp")
        .arg(jar)
        .arg("-d")
        .arg(dest)
        .arg(&worker)
        .arg(&clsp)
        .output()
        .map_err(|e| Error::Internal(format!("cannot run javac (is a JDK on PATH?): {e}")))?;
    let _ = fs::remove_dir_all(&src);
    if !out.status.success() {
        return Err(Error::Internal(format!(
            "javac failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(())
}

/// The JADX jar and the directory holding the compiled worker classes, fetched and compiled on
/// first use. `DROIDLENS_JADX_JAR` points at an existing jar instead of downloading one.
pub fn provision() -> Result<(PathBuf, PathBuf)> {
    let jar = match std::env::var_os("DROIDLENS_JADX_JAR") {
        Some(p) => {
            let jar = PathBuf::from(p);
            if !jar.is_file() {
                return Err(Error::Internal(format!(
                    "DROIDLENS_JADX_JAR is not a file: {}",
                    jar.display()
                )));
            }
            jar
        }
        None => {
            let jar = home()?.join(format!("jadx-{JADX_VERSION}-all.jar"));
            atomic(&jar, fetch_jar)?;
            jar
        }
    };
    let jar = fs::canonicalize(&jar).unwrap_or(jar);
    let key = {
        let mut h = Sha256::new();
        h.update(WORKER_SRC.as_bytes());
        h.update(CLSP_SRC.as_bytes());
        h.update(jar.to_string_lossy().as_bytes());
        format!("{:x}", h.finalize())[..10].to_string()
    };
    let classes = home()?.join(format!("worker-{key}"));
    atomic(&classes, |tmp| compile_worker(&jar, tmp))?;
    Ok((jar, classes))
}

struct Jvm {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Jvm {
    fn start() -> Result<Jvm> {
        let (jar, classes) = provision()?;
        let xmx = std::env::var("DROIDLENS_XMX").unwrap_or_else(|_| "512m".into());
        let cp = std::env::join_paths([classes.as_os_str(), jar.as_os_str()])
            .map_err(|e| Error::Internal(format!("bad classpath: {e}")))?;
        let mut child = Command::new("java")
            .arg("-Xss16m")
            .arg(format!("-Xmx{xmx}"))
            .arg("-cp")
            .arg(cp)
            .arg("DecompileWorker")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| Error::Internal(format!("cannot start java (is a JDK on PATH?): {e}")))?;
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Ok(Jvm {
            child,
            stdin,
            stdout,
        })
    }

    fn roundtrip(&mut self, request: &str) -> io::Result<String> {
        self.stdin.write_all(request.as_bytes())?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;
        let mut line = String::new();
        if self.stdout.read_line(&mut line)? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "JADX worker exited",
            ));
        }
        Ok(line)
    }

    /// Closes stdin so the worker's read loop ends, waits up to three seconds, then kills.
    fn close(mut self) {
        drop(self.stdin);
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_)) | Err(_)) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Serialize)]
struct WorkerRequest<'a> {
    id: u64,
    target: String,
    inputs: Vec<&'a Path>,
}

#[derive(Deserialize)]
struct WorkerResponse {
    code: Option<String>,
    error: Option<String>,
}

#[derive(Default)]
pub struct JadxWorker {
    jvm: Option<Jvm>,
    next_id: u64,
}

impl JadxWorker {
    fn roundtrip(
        &mut self,
        request: &str,
    ) -> Result<std::result::Result<WorkerResponse, io::Error>> {
        if self.jvm.is_none() {
            self.jvm = Some(Jvm::start()?);
        }
        let line = match self.jvm.as_mut().unwrap().roundtrip(request) {
            Ok(line) => line,
            Err(e) => return Ok(Err(e)),
        };
        Ok(serde_json::from_str(&line).map_err(io::Error::other))
    }

    /// Decompiles `target` from `inputs` (target first). A JVM that died is restarted once;
    /// one that ran out of heap answers this request with an error and is replaced before the next.
    pub fn decompile(&mut self, target: &ClassName, inputs: &[MiniDex]) -> Result<String> {
        let tmp = tempfile::Builder::new()
            .prefix("droidlens-")
            .tempdir()
            .map_err(|e| Error::Internal(format!("cannot create temp dir: {e}")))?;
        let mut paths = Vec::with_capacity(inputs.len());
        for (n, input) in inputs.iter().enumerate() {
            let p = tmp.path().join(format!("{n:03}.dex"));
            fs::write(&p, &input.bytes)
                .map_err(|e| Error::Internal(format!("cannot write {}: {e}", p.display())))?;
            paths.push(p);
        }
        self.next_id += 1;
        let request = serde_json::to_string(&WorkerRequest {
            id: self.next_id,
            target: target.dotted(),
            inputs: paths.iter().map(PathBuf::as_path).collect(),
        })
        .expect("request serializes");

        let response = match self.roundtrip(&request)? {
            Ok(r) => r,
            Err(_) => {
                self.close();
                match self.roundtrip(&request)? {
                    Ok(r) => r,
                    Err(e) => {
                        self.close();
                        return Err(Error::Jadx(format!("worker failed to start or died: {e}")));
                    }
                }
            }
        };
        if response
            .error
            .as_deref()
            .is_some_and(|e| e.contains("OutOfMemoryError"))
        {
            self.close();
        }
        match (response.code, response.error) {
            (Some(code), _) => Ok(code),
            (None, Some(error)) => Err(Error::Jadx(error)),
            (None, None) => Err(Error::Jadx("worker answered without code or error".into())),
        }
    }

    pub fn close(&mut self) {
        if let Some(jvm) = self.jvm.take() {
            jvm.close();
        }
    }
}

impl Drop for JadxWorker {
    fn drop(&mut self) {
        self.close();
    }
}
