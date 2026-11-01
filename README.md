# droidlens

**Let AI agents read the code of any Android app. Fast, light, and with the same output quality as JADX.**

[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
![Rust](https://img.shields.io/badge/built%20with-Rust-orange.svg)
![MCP](https://img.shields.io/badge/MCP-server-8A2BE2.svg)
[![CI](https://github.com/nthuat/droidlens/actions/workflows/ci.yml/badge.svg)](https://github.com/nthuat/droidlens/actions/workflows/ci.yml)
![JADX 1.5.6](https://img.shields.io/badge/decompiler-JADX%201.5.6-green.svg)

Ask your AI agent *"how does login work in this app?"* and it searches the APK, decompiles just the classes it needs, and reads them. Each class arrives in about **10 ms**, with **no wait for the app to load** and about **500 MB of memory**, even for a 400 MB APK.

**Claude Code answering questions about real apps**, with droidlens or with the JADX command line (12 questions on 4 apps, 11 to 420 MB):

| | droidlens | JADX command line |
| --- | --- | --- |
| Correct answers | 12 / 12 | 12 / 12 |
| Time per question (median) | **18 s** | 37 s |
| Tool memory (median) | **0.4 GB** | 6.6 GB |
| Files written | **none** | 67 to 165 MB per app |
| Token cost (total) | $1.35 | **$1.24** |

**Decompiler output on 200 random classes:** 5 files that are not valid Java, the same 5 as JADX. Full numbers and method in [Compared with other tools](#compared-with-other-tools).

## Built for

- 🔍 **Security reviews with an AI agent.** Find hard-coded keys, the servers an app talks to, weak crypto, or tracking code.
- 📦 **Many apps at once.** Scan hundreds of APKs, or compare two versions of the same app, from one process.
- 🐘 **Very large apps.** Memory stays around 500 MB whatever the APK size.
- ⚙️ **Servers and pipelines.** No up-front load, small memory, and the same output on every run.

## See it in action

You ask Claude Code:

> In ~/Downloads/NewPipe.apk, what does the `Image` class in the extractor hold?

Claude calls droidlens's `getclass` tool and gets readable Java back in milliseconds:

```java
public final class Image implements Serializable {
    private final ResolutionLevel estimatedResolutionLevel;
    private final int height;
    private final String url;
    private final int width;

    public Image(String str, int i, int i2, ResolutionLevel resolutionLevel) {
        this.url = str;
        this.height = i;
        this.width = i2;
        Objects.requireNonNull(resolutionLevel, "estimatedResolutionLevel is null");
        this.estimatedResolutionLevel = resolutionLevel;
    }
    ...
```

It can also search the app first: where a string is used, which classes extend a type, what the manifest declares. Then it decompiles only what matters.

## How it works

![How droidlens works: getclass goes through find class, pick context and cut tiny DEX to a JADX process that stays running, which returns Java source. Search reads the bytecode directly; findrefs also sends its top 5 matching classes through JADX and returns their matching Java lines.](docs/how-it-works.svg)

A normal decompiler loads and analyses the whole app before it can show you one class. That takes seconds and gigabytes. droidlens does the opposite:

1. **Find.** It finds the requested class inside the APK without unpacking or indexing the app.
2. **Pick context.** It picks only the classes JADX needs to get that class right: its inner classes, its parent classes and interfaces, and the classes that own R8 or Kotlin names it uses. Without them you would lose `@Override`, inner classes and readable names.
3. **Extract.** It cuts each of those classes out into a tiny DEX file. This uses [rust_ASC](https://github.com/MG1937/rust_ASC), patched to be about 10 times faster.
4. **Decompile.** It hands the tiny files to a JADX process that stays running. JADX is patched to load its model of the Android framework once, not on every request, which saves about 30 ms per class.

Search takes the lower path. `findrefs` and the list tools read the APK's bytecode directly. `findrefs` also sends its top 5 matching classes through steps 3 and 4 and returns their matching Java lines, like `grep`, so an agent often needs only one call.

## Quick start

You need a JDK 17 (`java` and `javac`) and Rust.

```bash
# 1. Install
git clone https://github.com/nthuat/droidlens && cd droidlens
cargo install --path .

# 2. Add to Claude Code
claude mcp add droidlens -- droidlens mcp
```

3. Ask your agent, giving the APK path:

> In ~/Downloads/app.apk, find where the string `api_key` is used and show me the decompiled class.

On first use, droidlens downloads JADX 1.5.6 (72 MB, checksum verified) into `~/.cache/droidlens`. That took about 10 s in our test, and it happens once.

## MCP tools

Every tool takes the path of an APK. Split-APK bundles work too: an XAPK, APKS or APKM file, or a folder of split APKs. droidlens merges the code of all splits into one cached APK under `~/.cache/droidlens/bundles` the first time (about 0.4 s for a 34 MB XAPK). One server can work on many APKs. It keeps the 8 most recently used open.

| Tool | What it does |
| --- | --- |
| `getclass` | Decompiles a class to Java. `class` can be `pkg.Cls`, `pkg.Outer$Inner` or `Lpkg/Cls;`. |
| `findrefs` | Finds where a `string`, `type`, `method` or `field` is used, and shows the matching decompiled Java lines with 3 lines of context, like `grep -n -C3` (first 5 classes; `code: false` turns this off). Also finds which classes `extends` / `impl` a type. |
| `listclass` | Lists classes, optionally under a package `prefix`. |
| `listmethod` | Lists methods, optionally of one `class` or matching a `filter`. |
| `liststring` | Lists strings in the app, optionally matching a `filter`. |
| `getmanifest` | Returns `AndroidManifest.xml` as XML. |

Lists return at most `limit` rows (default 500) and say when results were cut off.

For other MCP clients, add this to `.mcp.json`:

```json
{
  "mcpServers": {
    "droidlens": { "command": "droidlens", "args": ["mcp"] }
  }
}
```

## Command line

```bash
droidlens getclass app.apk com.example.MainActivity               # print Java
droidlens getclass app.apk 'com.example.Outer$Inner' -o Inner.java  # quote names with $
droidlens getclass app.apk com.example.MainActivity --timing       # show time per step
droidlens serve app.apk                                            # JSON lines on stdin/stdout
```

Each `getclass` call starts Java, so it takes about 0.5 s. To decompile many classes from a script, use `serve`. It keeps everything warm, and each request takes about 10 ms:

```
-> {"id":1,"op":"getclass","class":"com.example.MainActivity","context":true}
<- {"id":1,"code":"package com.example; ..."}
```

An unknown class or an invalid APK prints an error and exits with code 1.

## Compared with other tools

Measured on the same 200 random classes from 4 apps: NewPipe, Mihon, Bitwarden, and Element X (a 400 MB APK).

| | droidlens | JADX (whole app loaded) | ASC (Python) | rust_ASC |
| --- | --- | --- | --- | --- |
| Files that aren't valid Java | **5** | **5** | 97 | 97 |
| `@Override` emitted | 382 | 379 | 0 | 0 |
| Time per class (median) | 9 to 13 ms | 3 to 8 ms | 43 to 66 ms | 13 to 50 ms |
| Wait before the first class | ~0.5 s (Java start) | 1.5 to 3.1 s per app | none | none |
| Peak memory | 468 to 518 MB | 1.3 to 2.1 GB per app | 54 to 93 MB | 54 to 79 MB |
| Search tools | ✅ | API only | ✅ | ✅ |
| MCP server | ✅ | ❌ | ❌ | ✅ |

In short:

- **droidlens** gives JADX-quality code without loading the whole app, with a quarter to a third of JADX's memory.
- **JADX** kept open on a single app is faster per class once its load is done.
- **ASC and rust_ASC** are the lightest, but about half of their output isn't valid Java.

### With an AI agent

The same question about an app, asked to Claude Code (Sonnet) with droidlens's tools or with the JADX command line, in a fresh empty folder each time, 3 runs per app, medians. Tool memory is the peak memory of the processes Claude started (droidlens, or the JADX command line) during a session. Both setups answered all 12 questions correctly.

| App (APK size) | Time: droidlens / JADX | Cost: droidlens / JADX | Tool memory: droidlens / JADX |
| --- | --- | --- | --- |
| NewPipe (11 MB) | **15 s** / 24 s | $0.101 / **$0.089** | **0.3 GB** / 6.5 GB |
| Telegram X (48 MB) | **32 s** / 42 s | $0.147 / **$0.108** | **0.5 GB** / 7.4 GB |
| Bitwarden (191 MB) | **20 s** / 37 s | **$0.115** / $0.117 | **0.3 GB** / 7.3 GB |
| Element X (420 MB) | **13 s** / 36 s | **$0.084** / $0.092 | **0.6 GB** / 6.4 GB |
| All 12 questions | **18 s** / 37 s | $1.35 / **$1.24** (total) | **0.4 GB** / 6.6 GB |

droidlens answers in about half the time and uses about 16 times less memory (median 0.4 GB against 6.6 GB), and writes no files (JADX wrote 67 to 165 MB of sources per app). It costs about 9% more in tokens overall: on NewPipe and Telegram X Claude still takes a few more steps with droidlens than with one decompile plus `grep`, while on the two large apps droidlens costs the same or less. With 3 runs per app, cost differences of a few percent are within run-to-run noise. JADX takes up to 70% of RAM by default, and this Mac has 24 GB, so on a smaller machine its memory figure would be lower and its time likely higher.

**When to use something else:**

- To work interactively inside the JADX GUI, renaming variables, adding comments or using the debugger, use [jadx-mcp-server](https://github.com/zinja-coder/jadx-mcp-server).
- To decompile a whole app to disk, run JADX directly (`jadx -d out app.apk`).
- For native `.so` libraries or runtime hooking, pair droidlens with a Ghidra or Frida MCP server.

## Configuration

| Variable | Default | Purpose |
| --- | --- | --- |
| `DROIDLENS_HOME` | `~/.cache/droidlens` | Where JADX and the compiled Java worker are stored |
| `DROIDLENS_JADX_JAR` | (download) | Use an existing `jadx-1.5.6-all.jar` instead of downloading |
| `DROIDLENS_XMX` | `512m` | Java heap size. If a request runs out of memory, droidlens returns an error and restarts Java for the next request. |
| `DROIDLENS_TIMEOUT_SECS` | `60` | Longest one decompile may take. A slower request returns an error and Java is restarted, so one pathological class cannot stall the server. |

<details>
<summary><b>Measurement details</b></summary>

**Per-step time on Telegram X 0.29.1** (release build, warm `serve`, median of 12 rounds, Apple M5 Pro):

| Class | Before (Python) | Now: total (locate + context + extract + JADX) |
| --- | --- | --- |
| `sync.SyncTask` | 44 ms | 8.5 ms (0.0 + 0.5 + 0.3 + 7.6) |
| `MainActivity` | 114 ms | 49.9 ms (0.0 + 0.6 + 2.7 + 46.4) |
| `service.TGCallService` | 61 ms | 18.9 ms (0.0 + 0.5 + 0.8 + 17.2) |
| `th2` | 78 ms | 38.1 ms (0.0 + 0.2 + 0.8 + 36.8) |

JADX is 90 to 96 percent of each request.

**200 classes from 4 apps** (50 random top-level classes each, fixed seed, warm `serve`, compared with JADX with the whole app loaded and resources skipped):

- All 200 decompiled, with no crashes or timeouts.
- The median per class was 9 to 13 ms and p95 was 38 to 55 ms, against JADX's 58 to 358 ms p95.
- Two full runs produced byte-identical output.

ASC and rust_ASC were run as one CLI call per class, the way their own CLI and MCP use them. Syntax errors were counted with tree-sitter-java.

For scale, decompiling all of Telegram X (48 MB) to disk with the JADX CLI took 17 s and 8 GB of memory.

</details>

<details>
<summary><b>Project layout</b></summary>

- `src/main.rs`: the command-line interface.
- `src/mcp.rs` and `src/serve.rs`: the MCP server and the JSON-lines server.
- `src/engine.rs`: one thread that owns the open APKs and the Java process; everything else sends it jobs.
- `src/apk.rs`: one opened APK. It finds classes, picks the context classes and extracts tiny DEX files.
- `src/jadx.rs`: downloads JADX, compiles the Java worker and talks to it.
- `src/class_name.rs`: parses class names (`pkg.Cls` or `Lpkg/Cls;`).
- `java/DecompileWorker.java`: the Java side, which decompiles the tiny DEX files with JADX.
- `java/ClspGraph.java`: JADX 1.5.6's class, patched to load the Android framework model once.
- `third_party/rust_ASC`: the vendored extractor. `DROIDLENS_PATCHES.md` there lists what we changed and why.

</details>

<details>
<summary><b>Running the tests</b></summary>

```bash
scripts/fetch-fixtures.sh   # downloads the test APKs and JADX into .fixtures/ (about 140 MB, checksum verified)
cargo test --release
```

The integration tests in `tests/cli.rs` run the real binary on Telegram X 0.29.1 and NewPipe 0.29.1. A test skips itself if its files are missing, unless `DROIDLENS_REQUIRE_FIXTURES=1` is set, as it is in CI.

CI (`.github/workflows/ci.yml`) checks formatting and lints, then runs every test on Linux and macOS. Pushing a `v*` tag that matches the `Cargo.toml` version builds release binaries for Linux x86_64, Linux arm64 and macOS arm64 and attaches them to a GitHub release (`.github/workflows/release.yml`).

</details>

## Credits

- [rust_ASC](https://github.com/MG1937/rust_ASC) (Apache-2.0) for finding and extracting classes, and for search.
- [JADX](https://github.com/skylot/jadx) (Apache-2.0) for decompiling.

## License

Apache License 2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE). droidlens includes a patched copy of rust_ASC and one patched JADX source file, both also Apache-2.0.
