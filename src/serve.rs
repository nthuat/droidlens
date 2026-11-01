//! `droidlens serve APK`: JSON lines on stdio, one request per line, one response per line.
//!
//! `{"id":1,"op":"getclass","class":"pkg.Cls","context":true}` answers
//! `{"id":1,"code":"..."}` or `{"id":1,"error":"..."}`.

use std::io::{self, BufRead, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::class_name::ClassName;
use crate::engine::{Engine, Timing};
use crate::error::{Error, Result};

#[derive(Deserialize)]
struct Request {
    op: String,
    class: String,
    #[serde(default = "yes")]
    context: bool,
}

fn yes() -> bool {
    true
}

#[derive(Serialize)]
struct Response {
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn handle(engine: &Engine, apk: &Path, line: &str) -> (Value, Result<(String, Timing)>) {
    let value: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return (Value::Null, Err(Error::BadRequest(format!("bad request: {e}")))),
    };
    let id = value.get("id").cloned().unwrap_or(Value::Null);
    let result = (|| {
        let req: Request = serde_json::from_value(value).map_err(|e| Error::BadRequest(format!("bad request: {e}")))?;
        if req.op != "getclass" {
            return Err(Error::BadRequest(format!("unknown op: {:?}", req.op)));
        }
        let class = ClassName::parse(&req.class)?;
        let d = engine.getclass(apk, &class, req.context)?;
        Ok((d.code, d.timing))
    })();
    (id, result)
}

pub fn run(engine: &Engine, apk: &Path, timing: bool) -> Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line.map_err(|e| Error::Internal(format!("cannot read stdin: {e}")))?;
        if line.trim().is_empty() {
            continue;
        }
        let (id, result) = handle(engine, apk, &line);
        let response = match result {
            Ok((code, t)) => {
                if timing {
                    eprintln!("{t}");
                }
                Response { id, code: Some(code), error: None }
            }
            Err(e) => Response { id, code: None, error: Some(e.to_string()) },
        };
        serde_json::to_writer(&mut stdout, &response).map_err(|e| Error::Internal(e.to_string()))?;
        stdout.write_all(b"\n").and_then(|()| stdout.flush()).map_err(|e| Error::Internal(e.to_string()))?;
    }
    Ok(())
}
