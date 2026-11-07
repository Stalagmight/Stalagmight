//! slice-rs - Extract code from Rust files using syn AST parsing
//!
//! Usage:
//!   slice-rs <file> --toc              # Show table of contents
//!   slice-rs <file> --fn <name>        # Extract function
//!   slice-rs <file> --impl <name>      # Extract impl block
//!   slice-rs <file> --struct <name>    # Extract struct
//!   slice-rs <file> --enum <name>      # Extract enum
//!   slice-rs <file> --trait <name>     # Extract trait
//!   slice-rs <file> --mod <name>       # Extract module
//!   slice-rs <file> --lines <range>    # Extract line range

use std::env;
use std::fs;
use std::ops::Range;
use syn::spanned::Spanned;
use syn::{File, ImplItem, Item};

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 3 {
        eprintln!(
            "Usage: slice-rs <file> --toc | --fn <name> | --impl <name> | --struct <name> | --enum <name> | --trait <name> | --mod <name> | --lines <range>"
        );
        std::process::exit(1);
    }

    let file_path = &args[1];
    let mode = &args[2];
    let target = args.get(3).map(|s| s.as_str());

    let source = match fs::read_to_string(file_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error reading file: {}", e);
            std::process::exit(1);
        }
    };

    let syntax: File = match syn::parse_file(&source) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Parse error: {}", e);
            std::process::exit(1);
        }
    };

    let lines: Vec<&str> = source.lines().collect();
    let total_lines = lines.len();

    match mode.as_str() {
        "--toc" => print_toc(file_path, &syntax, total_lines),
        "--fn" | "--function" => {
            let name = target.unwrap_or_else(|| {
                eprintln!("Error: Function name required");
                std::process::exit(1);
            });
            extract_function(&syntax, name, &lines);
        }
        "--impl" => {
            let name = target.unwrap_or_else(|| {
                eprintln!("Error: Impl type name required");
                std::process::exit(1);
            });
            extract_impl(&syntax, name, &lines);
        }
        "--struct" => {
            let name = target.unwrap_or_else(|| {
                eprintln!("Error: Struct name required");
                std::process::exit(1);
            });
            extract_struct(&syntax, name, &lines);
        }
        "--enum" => {
            let name = target.unwrap_or_else(|| {
                eprintln!("Error: Enum name required");
                std::process::exit(1);
            });
            extract_enum(&syntax, name, &lines);
        }
        "--trait" => {
            let name = target.unwrap_or_else(|| {
                eprintln!("Error: Trait name required");
                std::process::exit(1);
            });
            extract_trait(&syntax, name, &lines);
        }
        "--mod" | "--module" => {
            let name = target.unwrap_or_else(|| {
                eprintln!("Error: Module name required");
                std::process::exit(1);
            });
            extract_mod(&syntax, name, &lines);
        }
        "--lines" => {
            let range_str = target.unwrap_or_else(|| {
                eprintln!("Error: Line range required (e.g., 100-200)");
                std::process::exit(1);
            });
            extract_lines(range_str, &lines, total_lines);
        }
        _ => {
            eprintln!("Unknown mode: {}", mode);
            eprintln!(
                "Valid modes: --toc, --fn, --impl, --struct, --enum, --trait, --mod, --lines"
            );
            std::process::exit(1);
        }
    }
}

fn is_pub(vis: &syn::Visibility) -> bool {
    !matches!(vis, syn::Visibility::Inherited)
}

fn get_type_name(ty: &syn::Type) -> String {
    match ty {
        syn::Type::Path(type_path) => {
            type_path.path.segments.last()
                .map(|s| s.ident.to_string())
                .unwrap_or_else(|| "?".to_string())
        }
        syn::Type::Reference(r) => {
            format!("&{}", get_type_name(&r.elem))
        }
        _ => "?".to_string()
    }
}

fn print_toc(file_path: &str, syntax: &File, total_lines: usize) {
    let file_name = std::path::Path::new(file_path)
        .file_name()
        .map(|s| s.to_string_lossy())
        .unwrap_or_default();

    println!("# {} ({} lines)", file_name, total_lines);
    println!();

    for item in &syntax.items {
        match item {
            Item::Fn(f) => {
                let vis = if is_pub(&f.vis) { " [pub]" } else { "" };
                let async_tag = if f.sig.asyncness.is_some() {
                    " [async]"
                } else {
                    ""
                };
                let line = f.sig.ident.span().start().line;
                println!("  fn {}{}{}  L{}", f.sig.ident, vis, async_tag, line);
            }
            Item::Struct(s) => {
                let vis = if is_pub(&s.vis) { " [pub]" } else { "" };
                let line = s.ident.span().start().line;
                println!("  struct {}{}  L{}", s.ident, vis, line);
            }
            Item::Enum(e) => {
                let vis = if is_pub(&e.vis) { " [pub]" } else { "" };
                let line = e.ident.span().start().line;
                println!("  enum {}{}  L{}", e.ident, vis, line);
            }
            Item::Trait(t) => {
                let vis = if is_pub(&t.vis) { " [pub]" } else { "" };
                let line = t.ident.span().start().line;
                println!("  trait {}{}  L{}", t.ident, vis, line);
            }
            Item::Impl(i) => {
                let type_name = get_type_name(&i.self_ty);
                let trait_info = i.trait_.as_ref().map(|(_, path, _)| {
                    let trait_name = path.segments.last().map(|s| s.ident.to_string()).unwrap_or_default();
                    format!(" impl {} for", trait_name)
                });
                let line = i.impl_token.span.start().line;

                if let Some(ti) = trait_info {
                    println!(" {}  {}  L{}", ti, type_name, line);
                } else {
                    println!("  impl {}  L{}", type_name, line);
                }

                // List methods
                for item in &i.items {
                    if let ImplItem::Fn(m) = item {
                        let vis = if is_pub(&m.vis) { " [pub]" } else { "" };
                        let async_tag = if m.sig.asyncness.is_some() {
                            " [async]"
                        } else {
                            ""
                        };
                        let mline = m.sig.ident.span().start().line;
                        println!("    .{}{}{}  L{}", m.sig.ident, vis, async_tag, mline);
                    }
                }
            }
            Item::Mod(m) => {
                let vis = if is_pub(&m.vis) { " [pub]" } else { "" };
                let line = m.ident.span().start().line;
                println!("  mod {}{}  L{}", m.ident, vis, line);
            }
            Item::Use(_) | Item::Const(_) | Item::Static(_) | Item::Type(_) => {
                // Skip use statements, consts, statics, type aliases for brevity
            }
            _ => {}
        }
    }
}

/// The 0-based, end-exclusive line range to print for an item whose keyword
/// token sits on 1-based line `start_hint` and whose last token sits on 1-based
/// line `end_line`. The end comes from the item's syn span (#576 E18(g)): the
/// old end was found by counting `{`/`}` over raw characters, so a brace in a
/// string, char literal or comment cut the item short or ran it on, and a
/// braceless item (`mod x;`) ran on until the next stray brace.
fn find_item_range(lines: &[&str], start_hint: usize, end_line: usize) -> Range<usize> {
    // Start from the hint line and work backwards to find doc comments/attributes
    let mut actual_start = start_hint.saturating_sub(1);

    // Look backwards for doc comments and attributes
    while actual_start > 0 {
        let prev_line = lines[actual_start - 1].trim();
        if prev_line.starts_with("///")
            || prev_line.starts_with("#[")
            || prev_line.starts_with("//!")
            || prev_line.is_empty()
        {
            if prev_line.is_empty() && actual_start > 1 {
                // Check if there's more above
                let above = lines[actual_start - 2].trim();
                if !above.starts_with("///") && !above.starts_with("#[") {
                    break;
                }
            }
            actual_start -= 1;
        } else {
            break;
        }
    }

    actual_start..end_line.max(start_hint).min(lines.len())
}

/// The last 1-based line of any syntax node, from its span (needs
/// proc-macro2's `span-locations`, enabled in Cargo.toml).
fn end_line_of<T: Spanned>(node: &T) -> usize {
    node.span().end().line
}

fn print_lines(lines: &[&str], range: Range<usize>) {
    let width = format!("{}", range.end).len();
    for i in range {
        if i < lines.len() {
            println!("{:>width$}| {}", i + 1, lines[i], width = width);
        }
    }
}

fn extract_function(syntax: &File, name: &str, lines: &[&str]) {
    // Check top-level functions
    for item in &syntax.items {
        if let Item::Fn(f) = item {
            if f.sig.ident == name {
                let start_line = f.sig.fn_token.span.start().line;
                let range = find_item_range(lines, start_line, end_line_of(f));
                print_lines(lines, range);
                return;
            }
        }

        // Check impl blocks for methods
        if let Item::Impl(i) = item {
            for impl_item in &i.items {
                if let ImplItem::Fn(m) = impl_item {
                    if m.sig.ident == name {
                        let start_line = m.sig.fn_token.span.start().line;
                        let range = find_item_range(lines, start_line, end_line_of(m));
                        print_lines(lines, range);
                        return;
                    }
                }
            }
        }
    }

    eprintln!("Function not found: {}", name);
    std::process::exit(1);
}

fn extract_impl(syntax: &File, name: &str, lines: &[&str]) {
    for item in &syntax.items {
        if let Item::Impl(i) = item {
            let type_name = get_type_name(&i.self_ty);
            if type_name == name || type_name.contains(name) {
                let start_line = i.impl_token.span.start().line;
                let range = find_item_range(lines, start_line, end_line_of(i));
                print_lines(lines, range);
                return;
            }
        }
    }

    eprintln!("Impl block not found: {}", name);
    std::process::exit(1);
}

fn extract_struct(syntax: &File, name: &str, lines: &[&str]) {
    for item in &syntax.items {
        if let Item::Struct(s) = item {
            if s.ident == name {
                let start_line = s.struct_token.span.start().line;
                let range = find_item_range(lines, start_line, end_line_of(s));
                print_lines(lines, range);
                return;
            }
        }
    }

    eprintln!("Struct not found: {}", name);
    std::process::exit(1);
}

fn extract_enum(syntax: &File, name: &str, lines: &[&str]) {
    for item in &syntax.items {
        if let Item::Enum(e) = item {
            if e.ident == name {
                let start_line = e.enum_token.span.start().line;
                let range = find_item_range(lines, start_line, end_line_of(e));
                print_lines(lines, range);
                return;
            }
        }
    }

    eprintln!("Enum not found: {}", name);
    std::process::exit(1);
}

fn extract_trait(syntax: &File, name: &str, lines: &[&str]) {
    for item in &syntax.items {
        if let Item::Trait(t) = item {
            if t.ident == name {
                let start_line = t.trait_token.span.start().line;
                let range = find_item_range(lines, start_line, end_line_of(t));
                print_lines(lines, range);
                return;
            }
        }
    }

    eprintln!("Trait not found: {}", name);
    std::process::exit(1);
}

fn extract_mod(syntax: &File, name: &str, lines: &[&str]) {
    for item in &syntax.items {
        if let Item::Mod(m) = item {
            if m.ident == name {
                let start_line = m.mod_token.span.start().line;
                let range = find_item_range(lines, start_line, end_line_of(m));
                print_lines(lines, range);
                return;
            }
        }
    }

    eprintln!("Module not found: {}", name);
    std::process::exit(1);
}

fn extract_lines(range_str: &str, lines: &[&str], total_lines: usize) {
    let parts: Vec<&str> = range_str.split('-').collect();
    if parts.len() != 2 {
        eprintln!("Error: Invalid line range format. Use: start-end (e.g., 100-200)");
        std::process::exit(1);
    }

    let start: usize = parts[0].parse().unwrap_or_else(|_| {
        eprintln!("Error: Invalid start line");
        std::process::exit(1);
    });

    let end: usize = parts[1].parse().unwrap_or_else(|_| {
        eprintln!("Error: Invalid end line");
        std::process::exit(1);
    });

    if start < 1 || end > total_lines || start > end {
        eprintln!("Error: Invalid line range. File has {} lines.", total_lines);
        std::process::exit(1);
    }

    print_lines(lines, (start - 1)..end);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Locate a top-level fn / mod / struct the way the extractors do and
    /// return the 1-based inclusive line range they would print.
    fn printed(source: &str, kind: &str, name: &str) -> (usize, usize) {
        let syntax = syn::parse_file(source).expect("sample parses");
        let lines: Vec<&str> = source.lines().collect();
        for item in &syntax.items {
            let found = match (kind, item) {
                ("fn", Item::Fn(f)) if f.sig.ident == name => {
                    Some((f.sig.fn_token.span.start().line, end_line_of(f)))
                }
                ("mod", Item::Mod(m)) if m.ident == name => {
                    Some((m.mod_token.span.start().line, end_line_of(m)))
                }
                ("struct", Item::Struct(s)) if s.ident == name => {
                    Some((s.struct_token.span.start().line, end_line_of(s)))
                }
                _ => None,
            };
            if let Some((start, end)) = found {
                let range = find_item_range(&lines, start, end);
                return (range.start + 1, range.end);
            }
        }
        panic!("{kind} {name} not found");
    }

    const SAMPLE: &str = "/// Formats a closing brace.\nfn closer() -> String {\n    let s = \"}\"; // a brace in a string\n    let open = '{';\n    format!(\"{open}{s}\")\n}\n\nfn after() -> u32 {\n    42\n}\n\n#[cfg(test)]\nmod tests;\n\n/// Mentions } and {.\nstruct Holder {\n    text: &'static str,\n}\n";

    #[test]
    fn a_brace_in_a_string_or_char_literal_does_not_cut_the_fn_short() {
        // The brace counter stopped at line 3 (the "}" string).
        assert_eq!(printed(SAMPLE, "fn", "closer"), (1, 6));
        assert_eq!(printed(SAMPLE, "fn", "after"), (8, 10));
    }

    #[test]
    fn a_braceless_mod_prints_its_attribute_and_one_line() {
        // The brace counter ran on to the next stray brace (the doc comment below).
        assert_eq!(printed(SAMPLE, "mod", "tests"), (12, 13));
    }

    #[test]
    fn a_brace_in_a_doc_comment_does_not_shift_the_struct() {
        assert_eq!(printed(SAMPLE, "struct", "Holder"), (15, 18));
    }
}
