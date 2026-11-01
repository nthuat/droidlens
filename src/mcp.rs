//! `droidlens mcp`: a stdio MCP server. `getclass` goes through the shared `Engine` (one JVM for
//! every APK); the list and search tools use rust_ASC's stateless `ApkHandler` in-process.

use std::path::Path;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
    ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};
use rust_ASC::asc_client::apk_handler::ApkHandler;
use rust_ASC::asc_client::manifest_handler::get_manifest_xml;
use rust_ASC::asc_core::findrefs::findrefs_manager::FindRefInput;
use rust_ASC::asc_core::findrefs::locator::field_locator::FieldFind;
use rust_ASC::asc_core::findrefs::locator::method_locator::MethodFind;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use crate::bundle;
use crate::class_name::ClassName;
use crate::engine::Engine;
use crate::error::{Error, Result};
use crate::java_text::snippets;

const DEFAULT_LIMIT: usize = 500;
/// `findrefs` decompiles at most this many referencing classes to show their matching lines.
const CODE_CLASSES: usize = 5;
const CODE_CONTEXT_LINES: usize = 3;
const CODE_MAX_LINES_PER_CLASS: usize = 40;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GetClassArgs {
    /// Path to the APK, or to an XAPK / APKS / APKM bundle or a folder of split APKs.
    apk: String,
    /// Dotted class name (`pkg.Outer$Inner`) or Dalvik descriptor (`Lpkg/Outer$Inner;`).
    class: String,
    /// Also feed JADX the inner classes and in-APK supertypes, so `@Override`, inner classes and
    /// casts come out as in a full-APK run. Default true.
    context: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListClassArgs {
    /// Path to the APK, or to an XAPK / APKS / APKM bundle or a folder of split APKs.
    apk: String,
    /// Case sensitive class or package prefix, dotted or descriptor form.
    prefix: Option<String>,
    /// Maximum number of rows returned. Default 500.
    limit: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListMethodArgs {
    /// Path to the APK, or to an XAPK / APKS / APKM bundle or a folder of split APKs.
    apk: String,
    /// Only methods defined by this class, dotted or descriptor form.
    class: Option<String>,
    /// Case insensitive substring filter over method names.
    filter: Option<String>,
    /// Maximum number of rows returned. Default 500.
    limit: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListStringArgs {
    /// Path to the APK, or to an XAPK / APKS / APKM bundle or a folder of split APKs.
    apk: String,
    /// Case insensitive substring filter over string values.
    filter: Option<String>,
    /// Maximum number of rows returned. Default 500.
    limit: Option<usize>,
}

#[derive(Deserialize, JsonSchema, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum RefKind {
    /// Methods whose code references a string constant containing `query`.
    String,
    /// Methods whose code references a type whose descriptor contains `query`.
    Type,
    /// Methods whose code calls a method named like `query` (and/or declared by `class`).
    Method,
    /// Methods whose code accesses a field named like `query` (and/or declared by `class`).
    Field,
    /// Classes that extend the class named by `query`.
    Extends,
    /// Classes that implement the interface named by `query`.
    Impl,
}

impl RefKind {
    fn name(self) -> &'static str {
        match self {
            RefKind::String => "string",
            RefKind::Type => "type",
            RefKind::Method => "method",
            RefKind::Field => "field",
            RefKind::Extends => "extends",
            RefKind::Impl => "impl",
        }
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FindRefsArgs {
    /// Path to the APK, or to an XAPK / APKS / APKM bundle or a folder of split APKs.
    apk: String,
    /// What to search for.
    kind: RefKind,
    /// The string, type substring, method or field name, or class name to look for. For `method`
    /// and `field` it may be empty when `class` is given.
    query: String,
    /// For `method` and `field`: the declaring class, dotted or descriptor form.
    class: Option<String>,
    /// For `method` and `field`: match `class` as a case sensitive descriptor substring.
    fuzzy_class: Option<bool>,
    /// Maximum number of rows returned. Default 500.
    limit: Option<usize>,
    /// Also decompile the first few referencing classes and show the matching Java lines with
    /// surrounding context, like `grep -n -C3`. Default true. Not used for `extends` and `impl`.
    code: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GetManifestArgs {
    /// Path to the APK, or to an XAPK / APKS / APKM bundle or a folder of split APKs.
    apk: String,
}

fn tool<I: JsonSchema + 'static>(name: &'static str, description: &'static str) -> Tool {
    Tool::new(name, description, serde_json::Map::new())
        .with_input_schema::<I>()
        .with_annotations(
            ToolAnnotations::new()
                .read_only(true)
                .destructive(false)
                .idempotent(true)
                .open_world(false),
        )
}

fn tools() -> Vec<Tool> {
    vec![
        tool::<GetClassArgs>(
            "getclass",
            "Decompile one class of an APK to JADX-quality Java in tens of milliseconds. Use it when the \
             matching lines from `findrefs` are not enough.",
        ),
        tool::<ListClassArgs>("listclass", "List class descriptors defined in an APK."),
        tool::<ListMethodArgs>("listmethod", "List method signatures defined in an APK."),
        tool::<ListStringArgs>("liststring", "List string pool values of an APK."),
        tool::<FindRefsArgs>(
            "findrefs",
            "Search an APK's code, like grep over the decompiled app. Finds the methods that reference a \
             string, type, method or field, or the classes that extend or implement a class. For references \
             it also shows the matching Java lines with 3 lines of context from the first 5 classes, so one \
             call usually answers where and how a value is used. Start here.",
        ),
        tool::<GetManifestArgs>(
            "getmanifest",
            "Decode AndroidManifest.xml of an APK to XML.",
        ),
    ]
}

fn handler(apk: &str) -> ApkHandler {
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    ApkHandler::new(apk, false, workers)
}

/// Joins at most `limit` rows and says so when rows were dropped.
fn render(rows: Vec<String>, limit: Option<usize>) -> String {
    let limit = limit.unwrap_or(DEFAULT_LIMIT).max(1);
    let total = rows.len();
    if total == 0 {
        return "(no results)".into();
    }
    let mut out = rows
        .iter()
        .take(limit)
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    if total > limit {
        out.push_str(&format!(
            "\n[truncated: showing {limit} of {total} results; raise `limit` or narrow the query]"
        ));
    }
    out
}

fn find_input(args: &FindRefsArgs) -> Result<FindRefInput> {
    let mut input = FindRefInput {
        string: None,
        type_: None,
        method: None,
        field: None,
        extends: None,
        impl_: None,
    };
    let query = args.query.trim();
    match args.kind {
        RefKind::String => input.string = Some(query.to_string()),
        RefKind::Type => input.type_ = Some(query.to_string()),
        RefKind::Extends => input.extends = Some(ClassName::parse(query)?.descriptor().to_string()),
        RefKind::Impl => input.impl_ = Some(ClassName::parse(query)?.descriptor().to_string()),
        RefKind::Method | RefKind::Field => {
            let fuzzy = args.fuzzy_class.unwrap_or(false);
            let class = match args
                .class
                .as_deref()
                .map(str::trim)
                .filter(|c| !c.is_empty())
            {
                None => None,
                Some(c) if fuzzy => Some((c.replace('.', "/"), false)),
                Some(c) => Some((ClassName::parse(c)?.descriptor().to_string(), true)),
            };
            let name = (!query.is_empty()).then(|| query.to_string());
            if class.is_none() && name.is_none() {
                return Err(Error::BadRequest(format!(
                    "{} query needs a name or a class",
                    args.kind.name()
                )));
            }
            if args.kind == RefKind::Method {
                input.method = Some(MethodFind::new(class, name));
            } else {
                input.field = Some(FieldFind::new(class, name));
            }
        }
    }
    Ok(input)
}

/// The text a referencing class's Java contains where it uses the `findrefs` target.
fn needle(kind: RefKind, query: &str) -> Option<String> {
    let q = query.trim();
    let n = match kind {
        RefKind::String | RefKind::Method | RefKind::Field => q.to_string(),
        RefKind::Type => q
            .trim_start_matches('L')
            .trim_end_matches(';')
            .rsplit(['/', '.', '$'])
            .next()
            .unwrap_or_default()
            .to_string(),
        RefKind::Extends | RefKind::Impl => return None,
    };
    (!n.is_empty()).then_some(n)
}

/// The referencing class of a `findrefs` row such as `classes.dex | Lpkg/Cls;->m | matched=(..)`.
fn row_class(row: &str) -> Option<&str> {
    let rest = row.split_once(" | ")?.1;
    let end = rest.find(";->")? + 1;
    Some(&rest[..end])
}

/// Matching Java lines from the first `CODE_CLASSES` distinct classes in `rows`.
fn code_blocks(engine: &Engine, apk: &Path, rows: &[String], needle: &str) -> String {
    let mut classes: Vec<&str> = Vec::new();
    for c in rows.iter().filter_map(|r| row_class(r)) {
        if !classes.contains(&c) {
            classes.push(c);
        }
        if classes.len() == CODE_CLASSES {
            break;
        }
    }
    let mut out = String::new();
    for desc in classes {
        let Ok(class) = ClassName::parse(desc) else {
            continue;
        };
        let body = match engine.getclass(apk, &class, true) {
            Ok(d) => snippets(
                &d.code,
                needle,
                CODE_CONTEXT_LINES,
                CODE_MAX_LINES_PER_CLASS,
            )
            .unwrap_or_else(|| {
                format!("(\"{needle}\" not found verbatim in the decompiled Java; call getclass)")
            }),
            Err(e) => format!("(could not decompile: {e})"),
        };
        out.push_str(&format!("\n\n=== {} ===\n{body}", class.dotted()));
    }
    out
}

fn call(engine: &Engine, name: &str, mut args: Value) -> Result<String> {
    if let Some(apk) = args.get_mut("apk")
        && let Some(path) = apk.as_str()
    {
        *apk = Value::String(
            bundle::resolve(Path::new(path))?
                .to_string_lossy()
                .into_owned(),
        );
    }
    let bad = |e: serde_json::Error| Error::BadRequest(format!("invalid arguments: {e}"));
    match name {
        "getclass" => {
            let a: GetClassArgs = serde_json::from_value(args).map_err(bad)?;
            let class = ClassName::parse(&a.class)?;
            Ok(engine
                .getclass(Path::new(&a.apk), &class, a.context.unwrap_or(true))?
                .code)
        }
        "listclass" => {
            let a: ListClassArgs = serde_json::from_value(args).map_err(bad)?;
            let rows = handler(&a.apk)
                .list_classes(a.prefix.as_deref())
                .map_err(Error::Apk)?;
            Ok(render(rows, a.limit))
        }
        "listmethod" => {
            let a: ListMethodArgs = serde_json::from_value(args).map_err(bad)?;
            let class = a.class.as_deref().map(ClassName::parse).transpose()?;
            let rows = handler(&a.apk)
                .list_methods(
                    class.as_ref().map(ClassName::descriptor),
                    a.filter.as_deref(),
                )
                .map_err(Error::Apk)?;
            Ok(render(rows, a.limit))
        }
        "liststring" => {
            let a: ListStringArgs = serde_json::from_value(args).map_err(bad)?;
            let rows = handler(&a.apk)
                .list_strings(a.filter.as_deref())
                .map_err(Error::Apk)?;
            Ok(render(rows, a.limit))
        }
        "findrefs" => {
            let a: FindRefsArgs = serde_json::from_value(args).map_err(bad)?;
            let input = find_input(&a)?;
            let rows: Vec<String> = handler(&a.apk)
                .for_each_findrefs(a.kind.name(), &input)
                .into_iter()
                .flat_map(|(_dex, lines)| lines)
                .collect();
            let limit = a.limit.unwrap_or(DEFAULT_LIMIT).max(1);
            let code = match (a.code.unwrap_or(true), needle(a.kind, &a.query)) {
                (true, Some(needle)) => code_blocks(
                    engine,
                    Path::new(&a.apk),
                    &rows[..rows.len().min(limit)],
                    &needle,
                ),
                _ => String::new(),
            };
            Ok(render(rows, a.limit) + &code)
        }
        "getmanifest" => {
            let a: GetManifestArgs = serde_json::from_value(args).map_err(bad)?;
            get_manifest_xml(&a.apk, true).map_err(Error::Apk)
        }
        _ => Err(Error::BadRequest(format!("unknown tool: {name}"))),
    }
}

struct Server {
    engine: Engine,
    tools: Vec<Tool>,
}

impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("droidlens", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Decompile and search local Android APKs. Every tool takes an `apk` path on the \
                 server filesystem. Start with `findrefs`: it finds where a string, type, method or \
                 field is used and shows the matching decompiled Java lines, like grep. Use \
                 `getclass` for a whole class when those lines are not enough. List and search \
                 tools cap their output at `limit` rows (default 500) and say when they truncated.",
            )
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<ListToolsResult, ErrorData> {
        if request.is_some_and(|r| r.cursor.is_some()) {
            return Err(ErrorData::invalid_params(
                "tool list has no continuation cursor",
                None,
            ));
        }
        Ok(ListToolsResult::with_all_items(self.tools.clone()))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tools.iter().find(|t| t.name == name).cloned()
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResponse, ErrorData> {
        if self.get_tool(&request.name).is_none() {
            return Err(ErrorData::invalid_params(
                format!("unknown tool: {}", request.name),
                None,
            ));
        }
        let engine = self.engine.clone();
        let name = request.name.to_string();
        let args = Value::Object(request.arguments.unwrap_or_default());
        let result = tokio::task::spawn_blocking(move || call(&engine, &name, args)).await;
        let result = match result {
            Ok(Ok(text)) => CallToolResult::success(vec![ContentBlock::text(text)]),
            Ok(Err(e)) => CallToolResult::error(vec![ContentBlock::text(e.to_string())]),
            Err(e) => {
                CallToolResult::error(vec![ContentBlock::text(format!("tool task failed: {e}"))])
            }
        };
        Ok(result.into())
    }
}

pub fn run(engine: Engine) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .map_err(|e| Error::Internal(e.to_string()))?;
    runtime.block_on(async {
        let server = Server {
            engine,
            tools: tools(),
        };
        let service = server
            .serve((tokio::io::stdin(), tokio::io::stdout()))
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        service
            .waiting()
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_reports_truncation_only_when_rows_were_dropped() {
        let rows = |n: usize| (0..n).map(|i| i.to_string()).collect::<Vec<_>>();
        assert_eq!(render(rows(3), Some(3)), "0\n1\n2");
        let out = render(rows(4), Some(3));
        assert!(
            out.starts_with("0\n1\n2\n[truncated: showing 3 of 4"),
            "{out}"
        );
        assert_eq!(render(Vec::new(), None), "(no results)");
    }
}
