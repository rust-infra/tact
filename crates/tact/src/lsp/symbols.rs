use crate::lsp::uri::uri_to_path;

pub(crate) fn extract_locations(result: &serde_json::Value) -> Vec<String> {
    let items: Vec<&serde_json::Value> = if let Some(arr) = result.as_array() {
        arr.iter().collect()
    } else if result.is_object() {
        vec![result]
    } else {
        return Vec::new();
    };

    items
        .into_iter()
        .filter_map(|loc| {
            let uri = loc.get("uri")?.as_str()?;
            let line = loc.pointer("/range/start/line").and_then(|v| v.as_u64()).unwrap_or(0) + 1; // convert to 1-based
            let col = loc.pointer("/range/start/character").and_then(|v| v.as_u64()).unwrap_or(0) + 1;
            let path = uri_to_path(uri);
            Some(format!("{}:{}:{}", path, line, col))
        })
        .collect()
}

/// Recursively collect symbol names from a DocumentSymbol or SymbolInformation node.
pub(crate) fn collect_symbol(sym: &serde_json::Value, depth: usize, out: &mut Vec<String>) {
    let indent = "  ".repeat(depth);
    let name = sym.get("name").and_then(|n| n.as_str()).unwrap_or("<unnamed>");
    let kind = sym.get("kind").and_then(|k| k.as_u64()).unwrap_or(0);
    let kind_str = symbol_kind_name(kind);
    out.push(format!("{}{} ({})", indent, name, kind_str));

    // DocumentSymbol may have nested children
    if let Some(children) = sym.get("children").and_then(|c| c.as_array()) {
        for child in children {
            collect_symbol(child, depth + 1, out);
        }
    }
}

pub(crate) fn symbol_kind_name(kind: u64) -> &'static str {
    match kind {
        1 => "file",
        2 => "module",
        3 => "namespace",
        4 => "package",
        5 => "class",
        6 => "method",
        7 => "property",
        8 => "field",
        9 => "constructor",
        10 => "enum",
        11 => "interface",
        12 => "function",
        13 => "variable",
        14 => "constant",
        15 => "string",
        16 => "number",
        17 => "boolean",
        18 => "array",
        19 => "object",
        20 => "key",
        21 => "null",
        22 => "enum-member",
        23 => "struct",
        24 => "event",
        25 => "operator",
        26 => "type-parameter",
        _ => "symbol",
    }
}
