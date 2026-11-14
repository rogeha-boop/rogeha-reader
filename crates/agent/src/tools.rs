//! The tools the assistant can use. The reader runs them; this file only describes them to
//! the model and checks the model's input before it is passed on.

use serde_json::{Value, json};

struct Tool {
    name: &'static str,
    description: &'static str,
    /// (name, JSON type, description, required)
    params: &'static [(&'static str, &'static str, &'static str, bool)],
}

const TOOLS: &[Tool] = &[
    Tool {
        name: "get_document_info",
        description: "Overview of the open document: file name, number of pages, the page the user is looking at, which pages are landscape, and how many comments, form fields and digital signatures it has. Call this first when you need the document's shape.",
        params: &[],
    },
    Tool {
        name: "get_pages",
        description: "Read the text of a range of pages (1-based, inclusive). Each page starts with a line '=== Page N ==='. Long ranges are returned in parts: the result says where it stopped, so call again from there. Read pages before quoting or citing them.",
        params: &[("from", "integer", "First page to read", true), ("to", "integer", "Last page to read", true)],
    },
    Tool {
        name: "search",
        description: "Find a word or phrase in the whole document (not case sensitive). Returns the pages it appears on with the number of matches, and highlights the matches for the user.",
        params: &[("query", "string", "The exact word or phrase to look for", true)],
    },
    Tool {
        name: "go_to_page",
        description: "Scroll the reader to a page so the user sees it.",
        params: &[("page", "integer", "Page number, 1-based", true)],
    },
    Tool {
        name: "rotate_pages",
        description: "Rotate pages. The rotation becomes part of the document when the user saves. Use get_document_info to find landscape pages.",
        params: &[
            ("pages", "array", "Page numbers to rotate, 1-based", true),
            ("degrees", "integer", "Clockwise rotation: 90, 180 or 270", true),
        ],
    },
    Tool {
        name: "delete_pages",
        description: "Delete pages from the document. The reader asks the user to confirm first; the result says whether they agreed.",
        params: &[("pages", "array", "Page numbers to delete, 1-based", true)],
    },
    Tool {
        name: "add_note",
        description: "Add a sticky-note comment to a page (a standard PDF comment other readers can see).",
        params: &[("page", "integer", "Page number, 1-based", true), ("text", "string", "The comment text", true)],
    },
    Tool {
        name: "highlight_text",
        description: "Highlight every occurrence of a word or phrase on one page, optionally with a comment attached.",
        params: &[
            ("page", "integer", "Page number, 1-based", true),
            ("text", "string", "The exact text to highlight, as it appears on the page", true),
            ("comment", "string", "Optional comment to attach to the highlight", false),
        ],
    },
    Tool {
        name: "place_signature",
        description: "Stamp the user's saved visual signature onto a page. Fails if the user has not saved a signature in the Sign window yet. This is a picture of a signature, not a certificate signature.",
        params: &[
            ("page", "integer", "Page number, 1-based", true),
            ("position", "string", "Where on the page: bottom-left, bottom-center or bottom-right", true),
        ],
    },
    Tool {
        name: "get_form_fields",
        description: "List the fillable form fields: name, kind, current value and choices.",
        params: &[],
    },
    Tool {
        name: "fill_form_field",
        description: "Fill in one form field by its name (from get_form_fields). For checkboxes and radio buttons use the value 'on' or 'off'; for dropdowns use one of the listed choices.",
        params: &[("name", "string", "The field's name", true), ("value", "string", "The value to enter", true)],
    },
];

fn schema(tool: &Tool) -> Value {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();
    for (name, kind, description, is_required) in tool.params {
        let mut property = json!({ "type": kind, "description": description });
        if *kind == "array" {
            property["items"] = json!({ "type": "integer" });
        }
        properties.insert((*name).to_owned(), property);
        if *is_required {
            required.push(*name);
        }
    }
    json!({ "type": "object", "properties": properties, "required": required, "additionalProperties": false })
}

/// Tool definitions for the Claude Messages API.
pub fn anthropic() -> Value {
    TOOLS
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "input_schema": schema(t),
                // Stream tool input as it is generated; `validate` below checks it, since
                // the API then no longer does.
                "eager_input_streaming": true,
            })
        })
        .collect()
}

/// The same tools as OpenAI-style function definitions.
pub fn openai() -> Value {
    TOOLS
        .iter()
        .map(|t| json!({ "type": "function", "function": { "name": t.name, "description": t.description, "parameters": schema(t) } }))
        .collect()
}

/// Check a tool call against its definition. The error text goes back to the model.
pub fn validate(name: &str, input: &Value) -> Result<(), String> {
    let Some(tool) = TOOLS.iter().find(|t| t.name == name) else {
        return Err(format!("There is no tool called {name}."));
    };
    for (param, kind, _, required) in tool.params {
        let value = &input[*param];
        if value.is_null() {
            if *required {
                return Err(format!("Missing required input \"{param}\"."));
            }
            continue;
        }
        let ok = match *kind {
            "integer" => value.is_i64() || value.is_u64(),
            "string" => value.is_string(),
            "array" => value.as_array().is_some_and(|a| !a.is_empty() && a.iter().all(|v| v.is_i64() || v.is_u64())),
            _ => true,
        };
        if !ok {
            return Err(format!("Input \"{param}\" must be {}.", if *kind == "array" { "a list of page numbers" } else { kind }));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inputs_are_checked() {
        assert!(validate("get_pages", &json!({ "from": 1, "to": 3 })).is_ok());
        assert!(validate("get_pages", &json!({ "from": 1 })).is_err());
        assert!(validate("get_pages", &json!({ "from": "1", "to": 3 })).is_err());
        assert!(validate("rotate_pages", &json!({ "pages": [2, 5], "degrees": 90 })).is_ok());
        assert!(validate("rotate_pages", &json!({ "pages": [], "degrees": 90 })).is_err());
        assert!(validate("highlight_text", &json!({ "page": 1, "text": "x" })).is_ok(), "comment is optional");
        assert!(validate("no_such_tool", &json!({})).is_err());
    }

    #[test]
    fn definitions_are_well_formed() {
        let tools = anthropic();
        assert_eq!(tools.as_array().unwrap().len(), TOOLS.len());
        for (a, o) in tools.as_array().unwrap().iter().zip(openai().as_array().unwrap()) {
            assert_eq!(a["name"], o["function"]["name"]);
            assert_eq!(a["input_schema"]["type"], "object");
            assert_eq!(a["input_schema"]["additionalProperties"], false);
        }
    }
}
