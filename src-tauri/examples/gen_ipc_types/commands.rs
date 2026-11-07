//! The IPC command manifest (#576 C3): every command `generate_handler!` registers,
//! with the arguments Tauri deserializes, the value it resolves with and the value
//! it rejects with.
//!
//! `generate_handler!` is the only registry of commands, and a `#[tauri::command]`
//! signature the only statement of a command's argument names, so both are read
//! from the source with `syn` - one real parser, run by `pnpm gen:ipc-types`, in
//! place of the text scanners each consumer used to carry. Types resolve by their
//! last path segment against the types this generator exports, plus the fixed
//! tables below; anything else stops the generator, so the manifest is never
//! silently partial.
//!
//! Output: `src/types/ipc/IpcCommands.ts` (the `IpcCommands` map `invokeIpc` and
//! the Playwright mock are typed against, and `IPC_COMMAND_NAMES`) and
//! `src/types/ipc/ipcCommands.json` (the same data for the Rust tests).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use serde::Serialize;
use syn::punctuated::Punctuated;
use syn::visit::Visit;
use syn::{FnArg, GenericArgument, Item, Pat, PathArguments, ReturnType, Token, Type};

/// Command parameters Tauri injects itself; they never cross the wire.
const INJECTED: &[&str] = &["State", "AppHandle", "Window", "WebviewWindow", "Webview"];

/// Rust types whose JSON form is a string.
const STRING_TYPES: &[&str] = &["String", "str", "char", "PathBuf", "Path", "NaiveDate"];

/// Rust types whose JSON form is a number. The 64-bit ones are `number` too: no IPC
/// path delivers a `bigint` (#576 C6).
const NUMBER_TYPES: &[&str] = &[
    "u8",
    "u16",
    "u32",
    "u64",
    "usize",
    "i8",
    "i16",
    "i32",
    "i64",
    "isize",
    "f32",
    "f64",
    // `pub type SectionId = u32` / `ShotId = u32` (models); `main` pins both at compile time.
    "SectionId",
    "ShotId",
];

/// Types that cross the wire with no generated declaration, and why.
const OPAQUE_TYPES: &[(&str, &str)] = &[
    // Deliberately opaque to the frontend (models::dive_plan): store it, hand it back.
    ("StoredAnchor", "unknown"),
];

#[derive(Serialize)]
struct TypeRef {
    rust: String,
    ts: String,
}

#[derive(Serialize)]
struct Arg {
    /// The Rust parameter name.
    name: String,
    /// The key the frontend sends: Tauri's default camelCase of `name`.
    key: String,
    rust: String,
    ts: String,
    /// An `Option` parameter: Tauri reads a missing key as `None`.
    optional: bool,
}

#[derive(Serialize)]
struct Command {
    name: String,
    /// `commands::<module>` in `generate_handler!`.
    module: String,
    #[serde(rename = "async")]
    is_async: bool,
    args: Vec<Arg>,
    result: TypeRef,
    /// `never` for a command that does not return a `Result`.
    error: TypeRef,
}

#[derive(Serialize)]
struct Manifest {
    #[serde(rename = "generatedBy")]
    generated_by: &'static str,
    commands: Vec<Command>,
}

/// Rust type ident -> generated TS name, for every type the generator exported.
pub(crate) struct TypeTable {
    named: BTreeMap<String, String>,
}

impl TypeTable {
    /// `exported` is `(Rust ident, TS name)` per exported type. Two Rust types that
    /// share an ident would make a signature ambiguous, so that stops the generator.
    pub(crate) fn new(exported: &[(String, String)]) -> Self {
        let mut named = BTreeMap::new();
        for (rust, ts) in exported {
            if let Some(previous) = named.insert(rust.clone(), ts.clone()) {
                assert_eq!(
                    &previous, ts,
                    "two exported Rust types are both called `{rust}`; a command signature \
                     naming it would be ambiguous"
                );
            }
        }
        Self { named }
    }
}

/// Read the registry and the signatures under `src_dir` (the app crate's `src/`) and
/// write the manifest into `out_dir` (`src/types/ipc`).
pub(crate) fn write_manifest(src_dir: &Path, out_dir: &Path, table: &TypeTable) {
    let (commands, errors) = read_commands(src_dir, table);
    // A dev tool: stop with every problem listed at once.
    assert!(
        errors.is_empty(),
        "cannot build the IPC command manifest:\n  - {}",
        errors.join("\n  - ")
    );
    let mut imports = BTreeSet::new();
    for command in &commands {
        for ts in command
            .args
            .iter()
            .map(|a| &a.ts)
            .chain([&command.result.ts, &command.error.ts])
        {
            collect_imports(ts, table, &mut imports);
        }
    }
    fs::write(
        out_dir.join("IpcCommands.ts"),
        render_ts(&commands, &imports),
    )
    .expect("Failed to write IpcCommands.ts");
    let manifest = Manifest {
        generated_by: "pnpm gen:ipc-types (src-tauri/examples/gen_ipc_types/commands.rs)",
        commands,
    };
    let json = serde_json::to_string_pretty(&manifest).expect("manifest serializes");
    fs::write(out_dir.join("ipcCommands.json"), json + "\n")
        .expect("Failed to write ipcCommands.json");
}

/// The commands, sorted by name, and every problem met on the way.
fn read_commands(src_dir: &Path, table: &TypeTable) -> (Vec<Command>, Vec<String>) {
    let lib = match parse(&src_dir.join("lib.rs")) {
        Ok(lib) => lib,
        Err(e) => return (Vec::new(), vec![e]),
    };
    let mut finder = HandlerFinder::default();
    finder.visit_file(&lib);
    let paths = match <[_; 1]>::try_from(finder.found) {
        Ok([Ok(paths)]) => paths,
        Ok([Err(e)]) => return (Vec::new(), vec![e]),
        Err(found) => {
            let e = format!(
                "expected exactly one `generate_handler!` in lib.rs, found {}",
                found.len()
            );
            return (Vec::new(), vec![e]);
        }
    };

    let mut files: BTreeMap<String, syn::File> = BTreeMap::new();
    let mut commands = Vec::new();
    let mut errors = Vec::new();
    let mut seen = BTreeSet::new();
    for path in paths {
        match read_command(src_dir, &path, table, &mut files) {
            Ok(command) => {
                if !seen.insert(command.name.clone()) {
                    errors.push(format!("`{}` is registered twice", command.name));
                }
                commands.push(command);
            }
            Err(mut problems) => errors.append(&mut problems),
        }
    }
    commands.sort_by(|a, b| a.name.cmp(&b.name));
    (commands, errors)
}

fn parse(path: &Path) -> Result<syn::File, String> {
    let source =
        fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    syn::parse_file(&source).map_err(|e| format!("cannot parse {}: {e}", path.display()))
}

/// Collects the entries of every `generate_handler![...]` it meets.
#[derive(Default)]
struct HandlerFinder {
    found: Vec<Result<Vec<syn::Path>, String>>,
}

impl<'ast> Visit<'ast> for HandlerFinder {
    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        if mac
            .path
            .segments
            .last()
            .is_some_and(|s| s.ident == "generate_handler")
        {
            self.found.push(
                mac.parse_body_with(Punctuated::<syn::Path, Token![,]>::parse_terminated)
                    .map(|entries| entries.into_iter().collect())
                    .map_err(|e| {
                        format!(
                            "a `generate_handler!` entry is not a plain `commands::<module>::<fn>` \
                             path ({e}); the manifest cannot follow attributes such as #[cfg]"
                        )
                    }),
            );
        }
        syn::visit::visit_macro(self, mac);
    }
}

fn read_command(
    src_dir: &Path,
    path: &syn::Path,
    table: &TypeTable,
    files: &mut BTreeMap<String, syn::File>,
) -> Result<Command, Vec<String>> {
    let segments: Vec<String> = path.segments.iter().map(|s| s.ident.to_string()).collect();
    let [root, module, name] = segments.as_slice() else {
        return Err(vec![format!(
            "`{}` is not a `commands::<module>::<fn>` path",
            segments.join("::")
        )]);
    };
    if root != "commands" {
        return Err(vec![format!(
            "`{}` is not under `commands::`",
            segments.join("::")
        )]);
    }
    if !files.contains_key(module) {
        let flat = src_dir.join("commands").join(format!("{module}.rs"));
        let nested = src_dir.join("commands").join(module).join("mod.rs");
        let file = parse(if flat.exists() { &flat } else { &nested }).map_err(|e| vec![e])?;
        files.insert(module.clone(), file);
    }
    let file = &files[module];
    let func = file
        .items
        .iter()
        .find_map(|item| match item {
            Item::Fn(f) if f.sig.ident == name.as_str() => Some(f),
            _ => None,
        })
        .ok_or_else(|| {
            vec![format!(
                "`{module}::{name}` is registered but no `fn {name}` is defined in commands/{module}"
            )]
        })?;
    check_command_attribute(func, module, name)?;

    let mut errors = Vec::new();
    let mut args = Vec::new();
    for input in &func.sig.inputs {
        let FnArg::Typed(pat_type) = input else {
            errors.push(format!("`{name}` takes `self`"));
            continue;
        };
        let Pat::Ident(ident) = pat_type.pat.as_ref() else {
            errors.push(format!("`{name}` has a destructuring parameter"));
            continue;
        };
        if last_ident(&pat_type.ty).is_some_and(|t| INJECTED.contains(&t.as_str())) {
            continue;
        }
        let arg_name = ident.ident.to_string();
        match ts_of(&pat_type.ty, table) {
            Ok(ts) => args.push(Arg {
                key: camel_case(&arg_name),
                optional: last_ident(&pat_type.ty).as_deref() == Some("Option"),
                rust: rust_of(&pat_type.ty),
                ts,
                name: arg_name,
            }),
            Err(e) => errors.push(format!("`{name}` argument `{arg_name}`: {e}")),
        }
    }

    let (result, error) = match &func.sig.output {
        ReturnType::Default => (
            Ok(TypeRef {
                rust: "()".into(),
                ts: "null".into(),
            }),
            Ok(never()),
        ),
        ReturnType::Type(_, ty) => match result_args(ty) {
            Some((ok, err)) => (result_ref(ok, table), type_ref(err, table)),
            None => (result_ref(ty, table), Ok(never())),
        },
    };
    match (result, error) {
        (Ok(result), Ok(error)) if errors.is_empty() => Ok(Command {
            name: name.clone(),
            module: module.clone(),
            is_async: func.sig.asyncness.is_some(),
            args,
            result,
            error,
        }),
        (result, error) => {
            if let Err(e) = result {
                errors.push(format!("`{name}` result: {e}"));
            }
            if let Err(e) = error {
                errors.push(format!("`{name}` error: {e}"));
            }
            Err(errors)
        }
    }
}

/// The function must carry a bare `#[tauri::command]`: arguments such as
/// `rename_all` change the wire keys, which this manifest does not model.
fn check_command_attribute(
    func: &syn::ItemFn,
    module: &str,
    name: &str,
) -> Result<(), Vec<String>> {
    let attr = func.attrs.iter().find(|a| {
        a.path()
            .segments
            .last()
            .is_some_and(|s| s.ident == "command")
    });
    match attr {
        None => Err(vec![format!(
            "`{module}::{name}` is registered but has no #[tauri::command]"
        )]),
        Some(a) if !matches!(a.meta, syn::Meta::Path(_)) => Err(vec![format!(
            "`{module}::{name}`: arguments to #[tauri::command] change the wire keys; \
             teach the manifest about them first"
        )]),
        Some(_) => Ok(()),
    }
}

fn never() -> TypeRef {
    TypeRef {
        rust: "!".into(),
        ts: "never".into(),
    }
}

fn type_ref(ty: &Type, table: &TypeTable) -> Result<TypeRef, String> {
    Ok(TypeRef {
        ts: ts_of(ty, table)?,
        rust: rust_of(ty),
    })
}

/// A command's success value: `tauri::ipc::Response` is the raw binary body
/// (`ArrayBuffer` in the webview), everything else is JSON.
fn result_ref(ty: &Type, table: &TypeTable) -> Result<TypeRef, String> {
    if last_ident(ty).as_deref() == Some("Response") {
        return Ok(TypeRef {
            rust: rust_of(ty),
            ts: "ArrayBuffer".into(),
        });
    }
    type_ref(ty, table)
}

/// `Result<T, E>` -> `(T, E)`.
fn result_args(ty: &Type) -> Option<(&Type, &Type)> {
    let args = generic_args(ty, "Result")?;
    match args.as_slice() {
        [ok, err] => Some((ok, err)),
        _ => None,
    }
}

fn last_ident(ty: &Type) -> Option<String> {
    match ty {
        Type::Path(p) => p.path.segments.last().map(|s| s.ident.to_string()),
        Type::Reference(r) => last_ident(&r.elem),
        _ => None,
    }
}

/// The type arguments of `ty` when its last segment is `ident`.
fn generic_args<'a>(ty: &'a Type, ident: &str) -> Option<Vec<&'a Type>> {
    let Type::Path(p) = ty else { return None };
    let last = p.path.segments.last()?;
    if last.ident != ident {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &last.arguments else {
        return Some(Vec::new());
    };
    Some(
        args.args
            .iter()
            .filter_map(|a| match a {
                GenericArgument::Type(t) => Some(t),
                _ => None,
            })
            .collect(),
    )
}

/// The TS declaration of a JSON value of Rust type `ty`, in ts-rs's spelling.
fn ts_of(ty: &Type, table: &TypeTable) -> Result<String, String> {
    match ty {
        Type::Reference(r) => ts_of(&r.elem, table),
        Type::Paren(p) => ts_of(&p.elem, table),
        Type::Tuple(t) if t.elems.is_empty() => Ok("null".into()),
        Type::Tuple(t) => {
            let elems: Result<Vec<_>, _> = t.elems.iter().map(|e| ts_of(e, table)).collect();
            Ok(format!("[{}]", elems?.join(", ")))
        }
        Type::Path(p) => {
            let Some(last) = p.path.segments.last() else {
                return Err("empty type path".into());
            };
            let ident = last.ident.to_string();
            let args = generic_args(ty, &ident).unwrap_or_default();
            let one = |what: &str| -> Result<String, String> {
                match args.as_slice() {
                    [inner] => ts_of(inner, table),
                    _ => Err(format!("`{what}` with {} type arguments", args.len())),
                }
            };
            match ident.as_str() {
                "Option" => Ok(format!("{} | null", one("Option")?)),
                "Vec" | "VecDeque" | "HashSet" | "BTreeSet" => {
                    Ok(format!("Array<{}>", one(&ident)?))
                }
                "Box" | "Arc" | "Rc" => one(&ident),
                "HashMap" | "BTreeMap" => match args.as_slice() {
                    [k, v] => Ok(format!(
                        "Record<{}, {}>",
                        ts_of(k, table)?,
                        ts_of(v, table)?
                    )),
                    _ => Err(format!("`{ident}` with {} type arguments", args.len())),
                },
                "bool" => Ok("boolean".into()),
                s if STRING_TYPES.contains(&s) => Ok("string".into()),
                s if NUMBER_TYPES.contains(&s) => Ok("number".into()),
                s => {
                    if let Some((_, ts)) = OPAQUE_TYPES.iter().find(|(name, _)| *name == s) {
                        return Ok((*ts).to_string());
                    }
                    if !args.is_empty() {
                        return Err(format!("generic type `{}`", rust_of(ty)));
                    }
                    table.named.get(s).cloned().ok_or_else(|| {
                        format!(
                            "`{}` has no generated declaration - derive TS on it and add it to the \
                             export list in gen_ipc_types.rs (or to OPAQUE_TYPES, with the reason)",
                            rust_of(ty)
                        )
                    })
                }
            }
        }
        other => Err(format!("unsupported type syntax `{}`", rust_of(other))),
    }
}

/// `ty` as written, without spans or whitespace quirks.
fn rust_of(ty: &Type) -> String {
    match ty {
        Type::Reference(r) => format!("&{}", rust_of(&r.elem)),
        Type::Paren(p) => rust_of(&p.elem),
        Type::Tuple(t) => format!(
            "({})",
            t.elems.iter().map(rust_of).collect::<Vec<_>>().join(", ")
        ),
        Type::Path(p) => p
            .path
            .segments
            .iter()
            .map(|s| match &s.arguments {
                PathArguments::AngleBracketed(a) => {
                    let inner: Vec<String> = a
                        .args
                        .iter()
                        .map(|g| match g {
                            GenericArgument::Type(t) => rust_of(t),
                            GenericArgument::Lifetime(l) => format!("'{}", l.ident),
                            _ => "?".into(),
                        })
                        .collect();
                    format!("{}<{}>", s.ident, inner.join(", "))
                }
                _ => s.ident.to_string(),
            })
            .collect::<Vec<_>>()
            .join("::"),
        _ => "<unsupported>".into(),
    }
}

/// Tauri's default argument renaming: snake_case -> lowerCamelCase.
fn camel_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut upper = false;
    for c in name.trim_start_matches('_').chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// Every generated type name `ts` mentions, as a `./Name` import.
fn collect_imports(ts: &str, table: &TypeTable, imports: &mut BTreeSet<String>) {
    for word in ts.split(|c: char| !c.is_ascii_alphanumeric() && c != '_') {
        if table.named.values().any(|name| name == word) {
            imports.insert(word.to_string());
        }
    }
}

fn render_ts(commands: &[Command], imports: &BTreeSet<String>) -> String {
    let mut out = String::from(
        "// This file is generated by `pnpm gen:ipc-types` from `generate_handler!` in \
         src-tauri/src/lib.rs. Do not edit by hand.\n",
    );
    for name in imports {
        let _ = writeln!(out, "import type {{ {name} }} from \"./{name}\";");
    }
    out.push_str(
        "\n/**\n * Every IPC command the backend registers, by the name `invoke()` takes (#576 C3).\n \
         * `args` is the object Tauri deserializes - camelCase keys; an `Option` argument may be\n \
         * omitted. `result` is what the command resolves with, `error` what it rejects with\n \
         * (`never`: it does not return a `Result`; `unknown`: its type is not generated yet).\n \
         */\nexport interface IpcCommands {\n",
    );
    for c in commands {
        let args = if c.args.is_empty() {
            "Record<string, never>".to_string()
        } else {
            let fields: Vec<String> = c
                .args
                .iter()
                .map(|a| format!("{}{}: {}", a.key, if a.optional { "?" } else { "" }, a.ts))
                .collect();
            format!("{{ {} }}", fields.join("; "))
        };
        let _ = writeln!(
            out,
            "  {}: {{ args: {args}; result: {}; error: {} }};",
            c.name, c.result.ts, c.error.ts
        );
    }
    out.push_str(
        "}\n\n/** Every registered command name. */\nexport const IPC_COMMAND_NAMES = [\n",
    );
    for c in commands {
        let _ = writeln!(out, "  \"{}\",", c.name);
    }
    out.push_str("] as const;\n");
    out
}
