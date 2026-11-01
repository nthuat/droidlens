mod apk;
mod class_name;
mod engine;
mod error;
mod jadx;
mod mcp;
mod serve;

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use crate::class_name::ClassName;
use crate::engine::Engine;
use crate::error::{Error, Result};

#[derive(Parser)]
#[command(name = "droidlens", version, about = "Single-class Android decompiler for AI agents")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Decompile one class of an APK to Java on stdout
    Getclass {
        apk: PathBuf,
        /// Dotted (`pkg.Outer$Inner`) or descriptor (`Lpkg/Outer$Inner;`) form
        #[arg(value_name = "CLASS")]
        class: String,
        /// Write the Java to FILE instead of stdout
        #[arg(short, long, value_name = "FILE")]
        output: Option<PathBuf>,
        /// Skip inner classes and in-APK supertypes
        #[arg(long)]
        no_context: bool,
        /// Per-stage milliseconds on stderr
        #[arg(long)]
        timing: bool,
    },
    /// JSON-lines server on stdio for one APK, keeping it and the JVM warm
    Serve {
        apk: PathBuf,
        /// Per-stage milliseconds on stderr for each request
        #[arg(long)]
        timing: bool,
    },
    /// MCP server on stdio, serving any number of APKs through one JVM
    Mcp,
}

fn run(cli: Cli, engine: &Engine) -> Result<()> {
    match cli.cmd {
        Cmd::Getclass { apk, class, output, no_context, timing } => {
            let class = ClassName::parse(&class)?;
            engine.open(&apk)?;
            let d = engine.getclass(&apk, &class, !no_context)?;
            match output {
                Some(path) => std::fs::write(&path, &d.code)
                    .map_err(|e| Error::Internal(format!("cannot write {}: {e}", path.display())))?,
                None => std::io::stdout()
                    .write_all(d.code.as_bytes())
                    .map_err(|e| Error::Internal(format!("cannot write stdout: {e}")))?,
            }
            if timing {
                eprintln!("{}", d.timing);
            }
            Ok(())
        }
        Cmd::Serve { apk, timing } => {
            engine.open(&apk)?;
            serve::run(engine, &apk, timing)
        }
        Cmd::Mcp => mcp::run(engine.clone()),
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let (engine, worker) = Engine::start();
    let result = run(cli, &engine);
    drop(engine);
    let _ = worker.join();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("droidlens: {e}");
            ExitCode::FAILURE
        }
    }
}
