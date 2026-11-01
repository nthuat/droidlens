//! Black-box tests against the built `droidlens` binary. They need the Telegram X APK (and for
//! some, NewPipe) plus a JADX jar; each test skips itself when its fixtures are absent.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Output, Stdio};

use serde_json::{Value, json};

const SCRATCH: &str = "/private/tmp/claude-501/-Users-thuat-nguyen-Projects-AI-android/3ff31b41-bbf9-43f9-b6de-a3058030a005/scratchpad";
const SYNC_TASK: &str = "org.thunderdog.challegram.sync.SyncTask";
const MAIN_ACTIVITY: &str = "org.thunderdog.challegram.MainActivity";

fn fixture(env: &str, default: &str) -> PathBuf {
    std::env::var_os(env).map(PathBuf::from).unwrap_or_else(|| Path::new(SCRATCH).join(default))
}

fn apk() -> PathBuf {
    fixture("DROIDLENS_TEST_APK", "dl/apk/Telegram-X-0.29.1.1817.apk")
}

fn newpipe() -> PathBuf {
    fixture("DROIDLENS_TEST_APK2", "dl/apk2/newpipe/NewPipe_v0.29.1.apk")
}

fn jar() -> PathBuf {
    fixture("DROIDLENS_JADX_JAR", "dl/jadx/lib/jadx-1.5.6-all.jar")
}

/// False (and a note on stderr) when a fixture is missing, so the test returns early.
fn have(paths: &[PathBuf]) -> bool {
    for p in paths {
        if !p.exists() {
            eprintln!("skipped: fixture absent: {}", p.display());
            return false;
        }
    }
    true
}

fn droidlens() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_droidlens"));
    c.env("DROIDLENS_JADX_JAR", jar());
    c.env("DROIDLENS_HOME", fixture("DROIDLENS_HOME", "dlhome"));
    c
}

fn run(args: &[&str]) -> Output {
    droidlens().args(args).output().expect("droidlens runs")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn java_errors(code: &str) -> usize {
    fn count(node: tree_sitter::Node) -> usize {
        let own = usize::from(node.is_error() || node.is_missing());
        let mut cursor = node.walk();
        own + node.children(&mut cursor).map(count).sum::<usize>()
    }
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_java::LANGUAGE.into()).unwrap();
    count(parser.parse(code, None).unwrap().root_node())
}

fn child_pids(pid: u32) -> Vec<String> {
    let out = Command::new("pgrep").arg("-P").arg(pid.to_string()).output().unwrap();
    String::from_utf8_lossy(&out.stdout).split_whitespace().map(str::to_owned).collect()
}

/// A `serve` or `mcp` process with a line-oriented stdio conversation.
struct Stdio2 {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Stdio2 {
    fn spawn(mut cmd: Command) -> Stdio2 {
        let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Stdio2 { child, stdin, stdout }
    }

    fn send(&mut self, v: Value) {
        self.stdin.write_all(v.to_string().as_bytes()).unwrap();
        self.stdin.write_all(b"\n").unwrap();
        self.stdin.flush().unwrap();
    }

    fn recv(&mut self) -> Value {
        let mut line = String::new();
        assert!(self.stdout.read_line(&mut line).unwrap() > 0, "process closed stdout");
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("not JSON ({e}): {line}"))
    }

    fn ask(&mut self, v: Value) -> Value {
        self.send(v);
        self.recv()
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    fn finish(mut self) {
        drop(self.stdin);
        let status = self.child.wait().unwrap();
        assert!(status.success(), "process exited with {status}");
    }
}

fn serve(apk: &Path, env: &[(&str, &str)]) -> Stdio2 {
    let mut cmd = droidlens();
    cmd.arg("serve").arg(apk).envs(env.iter().copied());
    Stdio2::spawn(cmd)
}

fn serve_req(id: u64, class: &str) -> Value {
    json!({"id": id, "op": "getclass", "class": class, "context": true})
}

#[test]
fn getclass_with_context_restores_override_and_inner_class() {
    if !have(&[apk(), jar()]) {
        return;
    }
    let r = run(&["getclass", apk().to_str().unwrap(), SYNC_TASK]);
    assert!(r.status.success(), "{}", stderr(&r));
    let code = stdout(&r);
    assert!(code.contains("@Override"), "{code}");
    assert!(code.contains("class AnonymousClass1"), "{code}");
}

#[test]
fn getclass_without_context_has_no_override() {
    if !have(&[apk(), jar()]) {
        return;
    }
    let r = run(&["getclass", apk().to_str().unwrap(), SYNC_TASK, "--no-context"]);
    assert!(r.status.success(), "{}", stderr(&r));
    assert!(!stdout(&r).contains("@Override"));
}

#[test]
fn descriptor_form_is_accepted() {
    if !have(&[apk(), jar()]) {
        return;
    }
    let r = run(&["getclass", apk().to_str().unwrap(), "Lorg/thunderdog/challegram/sync/SyncTask;"]);
    assert!(r.status.success(), "{}", stderr(&r));
    assert!(stdout(&r).contains("class SyncTask"));
}

#[test]
fn main_activity_parses_as_java() {
    if !have(&[apk(), jar()]) {
        return;
    }
    let r = run(&["getclass", apk().to_str().unwrap(), MAIN_ACTIVITY]);
    assert!(r.status.success(), "{}", stderr(&r));
    assert_eq!(java_errors(&stdout(&r)), 0);
}

#[test]
fn unknown_class_exits_1_with_message() {
    if !have(&[apk(), jar()]) {
        return;
    }
    let r = run(&["getclass", apk().to_str().unwrap(), "no.such.Clazz"]);
    assert_eq!(r.status.code(), Some(1));
    let err = stderr(&r);
    assert!(err.contains("class not found") && err.contains("no.such.Clazz"), "{err}");
}

#[test]
fn timing_reports_all_stages() {
    if !have(&[apk(), jar()]) {
        return;
    }
    let r = run(&["getclass", apk().to_str().unwrap(), SYNC_TASK, "--timing"]);
    let err = stderr(&r);
    for stage in ["locate", "context", "extract", "jadx"] {
        assert!(err.contains(&format!("{stage}=")), "{err}");
    }
}

#[test]
fn serve_answers_sequential_requests_on_one_jvm() {
    if !have(&[apk(), jar()]) {
        return;
    }
    let mut p = serve(&apk(), &[]);
    let first = p.ask(serve_req(1, SYNC_TASK));
    let jvms = child_pids(p.pid());
    let second = p.ask(serve_req(2, MAIN_ACTIVITY));
    let bad = p.ask(serve_req(3, "no.such.Clazz"));
    assert_eq!(first["id"], 1);
    assert!(first["code"].as_str().unwrap().contains("@Override"));
    assert_eq!(second["id"], 2);
    assert!(second["code"].as_str().unwrap().contains("class MainActivity"));
    assert_eq!(bad["id"], 3);
    assert!(bad["error"].as_str().unwrap().contains("class not found"));
    assert_eq!(jvms.len(), 1, "one JVM child");
    assert_eq!(child_pids(p.pid()), jvms, "the same JVM served every request");
    p.finish();
}

#[test]
fn classes_naming_r8_and_kotlin_symbols_decompile_to_valid_java() {
    if !have(&[newpipe(), jar()]) {
        return;
    }
    for cls in [
        "androidx.compose.ui.BiasAlignment",
        "androidx.compose.foundation.layout.UnspecifiedConstraintsNode",
        "org.ocpsoft.prettytime.i18n.Resources_ja",
    ] {
        let r = run(&["getclass", newpipe().to_str().unwrap(), cls]);
        assert!(r.status.success(), "{cls}: {}", stderr(&r));
        let code = stdout(&r);
        assert_eq!(java_errors(&code), 0, "{cls} has syntax errors");
        assert!(!code.contains("-CC.") && !code.contains("$r8$lambda$"), "{cls} keeps an R8 name");
    }
}

#[test]
fn out_of_memory_returns_error_and_next_request_works() {
    if !have(&[apk(), jar()]) {
        return;
    }
    // 48m is too small for MainActivity and enough for SyncTask.
    let mut p = serve(&apk(), &[("DROIDLENS_XMX", "48m")]);
    let small = p.ask(serve_req(1, SYNC_TASK));
    let old_jvm = child_pids(p.pid());
    let oom = p.ask(serve_req(2, MAIN_ACTIVITY));
    let after = p.ask(serve_req(3, SYNC_TASK));
    assert!(small["code"].as_str().unwrap().contains("@Override"));
    assert_eq!(oom["id"], 2);
    assert!(oom["error"].as_str().unwrap().contains("OutOfMemoryError"), "{oom}");
    assert_eq!(after["id"], 3);
    assert!(after["code"].as_str().unwrap().contains("@Override"));
    assert_ne!(child_pids(p.pid()), old_jvm, "the JVM was replaced after the OOM");
    p.finish();
}

fn mcp() -> Stdio2 {
    let mut cmd = droidlens();
    cmd.arg("mcp");
    let mut p = Stdio2::spawn(cmd);
    let init = p.ask(json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
        "protocolVersion": "2024-11-05", "capabilities": {},
        "clientInfo": {"name": "droidlens-test", "version": "0"}}}));
    assert_eq!(init["result"]["serverInfo"]["name"], "droidlens", "{init}");
    p.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    p
}

fn tool_call(id: u64, name: &str, args: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": args}})
}

fn tool_text(resp: &Value) -> (bool, String) {
    let result = &resp["result"];
    let is_error = result["isError"].as_bool().unwrap_or(false);
    let text = result["content"][0]["text"].as_str().unwrap_or_else(|| panic!("{resp}")).to_string();
    (is_error, text)
}

#[test]
fn mcp_lists_six_tools_and_answers_getclass_and_truncated_listclass() {
    if !have(&[apk(), jar()]) {
        return;
    }
    let mut p = mcp();
    let list = p.ask(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}));
    let mut names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    names.sort_unstable();
    assert_eq!(names, ["findrefs", "getclass", "getmanifest", "listclass", "listmethod", "liststring"]);

    let apk = apk().to_str().unwrap().to_string();
    let (err, code) = tool_text(&p.ask(tool_call(2, "getclass", json!({"apk": apk, "class": SYNC_TASK}))));
    assert!(!err && code.contains("class SyncTask") && code.contains("@Override"), "{code}");

    let (err, listed) = tool_text(&p.ask(tool_call(3, "listclass", json!({"apk": apk, "prefix": "org.thunderdog", "limit": 5}))));
    assert!(!err, "{listed}");
    assert_eq!(listed.lines().filter(|l| l.starts_with("Lorg/thunderdog/")).count(), 5);
    assert!(listed.contains("truncated"), "{listed}");

    let (err, msg) = tool_text(&p.ask(tool_call(4, "getclass", json!({"apk": apk, "class": "no.such.Clazz"}))));
    assert!(err && msg.contains("class not found"), "{msg}");

    let (err, manifest) = tool_text(&p.ask(tool_call(5, "getmanifest", json!({"apk": apk}))));
    assert!(!err && manifest.contains("<manifest"), "{manifest}");

    let (err, refs) = tool_text(&p.ask(tool_call(6, "findrefs", json!({"apk": apk, "kind": "extends", "query": "androidx.work.Worker", "limit": 10}))));
    assert!(!err && refs.contains("SyncTask"), "{refs}");
    p.finish();
}

#[test]
fn mcp_serves_two_apks_through_one_jvm() {
    if !have(&[apk(), newpipe(), jar()]) {
        return;
    }
    let mut p = mcp();
    let (err, a) = tool_text(&p.ask(tool_call(1, "getclass", json!({"apk": apk().to_str().unwrap(), "class": SYNC_TASK}))));
    assert!(!err && a.contains("class SyncTask"), "{a}");
    let jvms = child_pids(p.pid());
    let (err, b) = tool_text(&p.ask(tool_call(2, "getclass", json!({"apk": newpipe().to_str().unwrap(), "class": "androidx.compose.ui.BiasAlignment"}))));
    assert!(!err && b.contains("class BiasAlignment"), "{b}");
    let (err, a2) = tool_text(&p.ask(tool_call(3, "getclass", json!({"apk": apk().to_str().unwrap(), "class": SYNC_TASK, "context": false}))));
    assert!(!err && a2.contains("class SyncTask") && !a2.contains("@Override"), "{a2}");
    assert_eq!(jvms.len(), 1);
    assert_eq!(child_pids(p.pid()), jvms, "both APKs went through the same JVM");
    p.finish();
}
