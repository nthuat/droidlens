# droidlens

Decompile one class of an APK to JADX-quality Java in tens of milliseconds, for AI agents.

droidlens is one Rust binary. It finds the class in the APK without indexing it and rebuilds a tiny single-class DEX with a vendored, patched [rust_ASC](https://github.com/MG1937/rust_ASC). It feeds that DEX to a long-lived JADX 1.5.6 JVM that runs a patched `ClspGraph`, so JADX parses its Android framework class set once per process instead of once per request. The class's inner classes, its in-APK supertypes, and the owners of any R8 or Kotlin-mangled names it uses go in as extra tiny DEX inputs, so `@Override`, inner classes and `this` casts come out as in a full-APK JADX run.

## Compared with other tools

Same 200 classes: 50 random top-level classes (fixed seed) from each of NewPipe 0.29.1, Mihon 0.21.3 arm64, Bitwarden 2026.10.1 and Element X 26.10.1 (a 400 MB APK). All measured on one Apple Silicon Mac. ASC and rust_ASC were run as one CLI call per class, the way their MCP/CLI use them. JADX 1.5.6 had the whole APK loaded once with resources skipped, then was asked for each class.

| | droidlens | JADX, whole APK loaded | ASC 0.1.2 (Python) | rust_ASC 0.1.2-post1 |
| --- | --- | --- | --- | --- |
| Classes decompiled | 200 / 200 | 200 / 200 | 200 / 200 | 200 / 200 |
| Files that are not valid Java (tree-sitter) | **5** | **5** | 97 (126 counting the `Lpkg/Cls;` class header) | 97 (126) |
| Dalvik register names left in code (`v12_3`, `p1`) | 40 | 44 | 13,448 | 13,397 |
| `@Override` emitted | 342 | 379 | 0 | 0 |
| Methods marked as failed | 13 | 13 | 0 (failures are silent) | 0 (failures are silent) |
| Per class, median | 9 to 13 ms | 3 to 8 ms | 43 to 66 ms | 13 to 50 ms |
| Up-front load | none (one JVM start, about 0.4 s, on first use) | 1.5 to 3.1 s per APK | none | none |
| Peak memory | 468 to 518 MB | 1.3 to 2.1 GB per APK | 54 to 93 MB | 54 to 79 MB |
| Search (strings, refs, subclasses) | yes (rust_ASC, in-process) | through its API | yes | yes |
| MCP server | yes | no | no | yes |

In short: droidlens gives JADX's code quality with no per-APK load and about a quarter to a third of JADX's memory, and serves any number of APKs from one JVM. A JADX process kept open on one APK is faster per class after its load. ASC and rust_ASC are the lightest, but their decompiler (a DAD design) leaves about half of the classes as invalid Java.

For scale, decompiling a whole APK to disk with the JADX CLI took 17 s and 8 GB peak on Telegram X 0.29.1 (48 MB). REA, Androguard, apktool and JEB were not measured.

## Install

```
cargo install --path .
```

droidlens needs a JDK 17 (`java` and `javac`) on `PATH`. On first use it downloads JADX 1.5.6 (72 MB, sha256-checked) into `$DROIDLENS_HOME` (default `~/.cache/droidlens`) and compiles its two Java sources there. Set `DROIDLENS_JADX_JAR` to use an existing `jadx-1.5.6-all.jar` instead of downloading.

## Use

```
droidlens getclass APP.apk org.example.Outer$Inner [-o FILE] [--no-context] [--timing]
droidlens serve APP.apk [--timing]
droidlens mcp
```

`CLASS` is dotted (`pkg.Cls`, `pkg.Outer$Inner`) or a descriptor (`Lpkg/Cls;`). `getclass` exits 1 with a message when the class is not in the APK. `--no-context` skips the inner classes and in-APK supertypes. `--timing` prints the per-stage milliseconds on stderr.

`serve` keeps one APK and the JVM warm and speaks JSON lines on stdio:

```
-> {"id":1,"op":"getclass","class":"pkg.Cls","context":true}
<- {"id":1,"code":"..."}   or   {"id":1,"error":"..."}
```

With `--timing`, each answered request also prints `timing ms: locate=0.0  context=0.5  extract=0.3  jadx=7.6` on stderr.

`mcp` is a stdio MCP server. Every tool takes an `apk` path. One JVM serves every APK, and the server keeps the eight most recently used APKs open.

| tool | arguments | returns |
| --- | --- | --- |
| `getclass` | `apk`, `class`, `context` (default true) | the Java source |
| `listclass` | `apk`, `prefix`, `limit` | class descriptors, one per line |
| `listmethod` | `apk`, `class`, `filter`, `limit` | method signatures |
| `liststring` | `apk`, `filter`, `limit` | string pool values |
| `findrefs` | `apk`, `kind` (`string`, `type`, `method`, `field`, `extends`, `impl`), `query`, `class`, `fuzzy_class`, `limit` | referencing methods, or subclasses and implementations |
| `getmanifest` | `apk` | `AndroidManifest.xml` as XML |

`limit` defaults to 500 rows. When a list is longer, the output ends with a `[truncated: showing 500 of N results ...]` line.

To add it to Claude Code:

```
claude mcp add droidlens -- droidlens mcp
```

or in `.mcp.json`:

```json
{
	"mcpServers": {
		"droidlens": { "command": "droidlens", "args": ["mcp"] }
	}
}
```

The JVM runs with `-Xmx512m` and `-Xss16m`. `DROIDLENS_XMX` overrides the heap. A request that runs out of heap returns an error, and the JVM restarts for the next request.

## Measured

Telegram X 0.29.1.1817 (3 DEX files), Apple M5 Pro, release build, warm `serve --timing` with context. The harness sends the 4 classes in turn for 15 rounds and reports the median over rounds 4 to 15 (12 samples per class). A second run agreed within 3 percent on every class. The same harness drove the previous Python package (same machine, same session, unpatched rust_ASC and JADX) for the "before" column.

| class | before, ms | after, ms (locate + context + extract + JADX) | @Override (same as full-APK JADX) |
| --- | --- | --- | --- |
| `sync.SyncTask` | 44 | 8.5 (0.0 + 0.5 + 0.3 + 7.6) | 1 |
| `MainActivity` | 114 | 49.9 (0.0 + 0.6 + 2.7 + 46.4) | 11 |
| `service.TGCallService` | 61 | 18.9 (0.0 + 0.5 + 0.8 + 17.2) | 12 |
| `th2` | 78 | 38.1 (0.0 + 0.2 + 0.8 + 36.8) | 1 |

The JADX stage is 90 to 96 percent of each request, so JADX's own decompile of the input classes bounds the number. The patched `ClspGraph` removed 26 to 33 ms of per-request framework parsing on every class, and the patched rust_ASC with one reused `DexManager` per DEX cut the extract stage by about 10x. The largest process in the `serve` tree (the JVM) peaked at 463 MB, against 694 MB before.

A one-shot `getclass` on `th2` takes 0.53 s wall clock with a warm disk cache (0.89 s the first time after the binary was built). Of that, JVM start plus the first JADX request is about 0.46 s. `SyncTask` takes 0.39 s.

For the 7 classes the tests cover (4 from Telegram X, 3 from NewPipe 0.29.1), the Java output is byte-identical to the Python package's, apart from the temp path in JADX's `loaded from` comments.

### Robustness: 200 classes from 4 more apps

NewPipe 0.29.1, Mihon 0.21.3 arm64, Bitwarden 2026.10.1 and Element X 26.10.1 (a 400 MB APK), 50 random top-level classes each (fixed seed), warm `serve` with context, compared with full-APK JADX 1.5.6 (whole APK loaded once, resources skipped):

| | droidlens | full-APK JADX |
| --- | --- | --- |
| classes decompiled | 200 / 200, no crashes or timeouts | 200 / 200 |
| files with Java syntax errors (tree-sitter) | 5 | 5 |
| `@Override` emitted | 342 | 379 |
| per class, median / p95 | 9 to 13 ms / 38 to 55 ms | 3 to 8 ms / 58 to 358 ms, after a 1.5 to 3.1 s load |
| peak memory | 468 to 518 MB (whole `serve` tree) | 1.3 to 2.1 GB |

Two full runs produce byte-identical output for all 200 classes. 199 of 200 are byte-identical to the Python package; the other differs only in the order of two annotations.

## Layout

- `src/class_name.rs` parses class names once at the boundaries into a `ClassName` descriptor type.
- `src/apk.rs` holds one opened APK: lazily inflated DEX entries, the context and extract rules, the DEX header fix.
- `src/jadx.rs` provisions the jar and compiled worker, and runs the JVM over JSON lines.
- `src/engine.rs` is the one thread that owns every `ApkSession` and the JVM; the CLI, `serve` and `mcp` send it jobs.
- `src/serve.rs` and `src/mcp.rs` are the two protocol adapters. `src/main.rs` is the CLI.
- `java/DecompileWorker.java` is the JVM side. `java/ClspGraph.java` is jadx 1.5.6's class with the shared framework cache; it precedes the jadx jar on the classpath. Both are embedded in the binary and compiled with `javac` on first use.
- `third_party/rust_ASC` is the vendored extractor. `third_party/rust_ASC/DROIDLENS_PATCHES.md` lists what changed and why.

## Test

```
cargo test
```

The integration tests in `tests/cli.rs` drive the built binary. They need the Telegram X APK (`DROIDLENS_TEST_APK`), the NewPipe APK (`DROIDLENS_TEST_APK2`) and a JADX jar (`DROIDLENS_JADX_JAR`), and skip themselves when a fixture is missing. The Java outputs are checked with tree-sitter for zero syntax errors.
