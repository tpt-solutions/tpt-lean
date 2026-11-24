// SPDX-License-Identifier: MIT OR Apache-2.0
//! Optional prompt templates (specification section 11.6): plain text with `{{argument}}`
//! placeholders. The tools do the work; these only tell a model which tools to use.

use crate::{RpcError, INVALID_PARAMS};
use serde_json::{json, Value};

/// One argument of a prompt.
struct Arg {
    name: &'static str,
    description: &'static str,
    required: bool,
}

/// One prompt template.
struct Prompt {
    name: &'static str,
    description: &'static str,
    args: &'static [Arg],
    template: &'static str,
}

const PROMPTS: &[Prompt] = &[
    Prompt {
        name: "explain_lean_error",
        description: "Explain a Lean error and suggest a fix.",
        args: &[
            Arg { name: "error", description: "The error message as Lean printed it.", required: true },
            Arg { name: "file", description: "Optional .lean file the error comes from.", required: false },
        ],
        template: "Explain this Lean error and suggest a fix.\n\nError:\n{{error}}\n\nFile: {{file}}\n\nIf a file and position are known, use tpt_lean_explain_diagnostic or tpt_lean_goals_at to see the goal there before proposing a change. Check the fix with tpt_lean_check_file.",
    },
    Prompt {
        name: "find_lemma_for_goal",
        description: "Find library lemmas that could close a goal.",
        args: &[Arg { name: "goal", description: "The goal, for example `⊢ a + b = b + a`.", required: true }],
        template: "Find Lean library lemmas that could close this goal:\n\n{{goal}}\n\nUse tpt_lean_search with the main constants and operators in the goal, then tpt_lean_context_pack if a position is available. Test candidates with tpt_lean_check_file before recommending one.",
    },
    Prompt {
        name: "reduce_proof_timeout",
        description: "Make a slow proof cheaper to elaborate.",
        args: &[
            Arg { name: "file", description: "The .lean file with the slow proof.", required: true },
            Arg { name: "line", description: "1-based line of the slow step or declaration.", required: true },
        ],
        template: "The proof at {{file}}:{{line}} times out or is slow to elaborate. Reduce its cost without changing the statement.\n\nStart with tpt_lean_context_pack at that position. Look for large simp or omega calls, unnecessary unfolding, and steps that can be split into lemmas. Re-run tpt_lean_check_file after each change.",
    },
    Prompt {
        name: "suggest_imports",
        description: "Find the import that brings a name into scope.",
        args: &[Arg { name: "symbol", description: "The name that is not in scope.", required: true }],
        template: "Which module should be imported so that `{{symbol}}` is in scope?\n\nUse tpt_lean_imports_suggest with the query `{{symbol}}`. Check the chosen module with tpt_lean_check_file.",
    },
    Prompt {
        name: "summarize_proof_state",
        description: "Summarize the proof state at a position.",
        args: &[
            Arg { name: "file", description: "The .lean file.", required: true },
            Arg { name: "line", description: "1-based line.", required: true },
            Arg { name: "column", description: "1-based column.", required: true },
        ],
        template: "Summarize the proof state at {{file}}:{{line}}:{{column}}: the hypotheses, the open goal, and any diagnostics there.\n\nUse tpt_lean_explain_diagnostic for that position.",
    },
];

/// `prompts/list`.
pub(crate) fn list() -> Value {
    let prompts: Vec<Value> = PROMPTS
        .iter()
        .map(|p| {
            let args: Vec<Value> = p
                .args
                .iter()
                .map(|a| json!({ "name": a.name, "description": a.description, "required": a.required }))
                .collect();
            json!({ "name": p.name, "description": p.description, "arguments": args })
        })
        .collect();
    json!({ "prompts": prompts })
}

/// `prompts/get`: fill the template with the given arguments.
pub(crate) fn get(params: &Value) -> Result<Value, RpcError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or((INVALID_PARAMS, "prompts/get needs a prompt name".to_string()))?;
    let prompt = PROMPTS
        .iter()
        .find(|p| p.name == name)
        .ok_or((INVALID_PARAMS, format!("unknown prompt {name}")))?;
    let given = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
    let mut values: Vec<(&str, String)> = Vec::new();
    for arg in prompt.args {
        match given.get(arg.name) {
            Some(v) => values.push((arg.name, value_text(v))),
            None if arg.required => {
                return Err((
                    INVALID_PARAMS,
                    format!("prompt {name} needs argument `{}`", arg.name),
                ));
            }
            None => values.push((arg.name, String::new())),
        }
    }
    let text = render(prompt.template, &values);
    Ok(json!({
        "description": prompt.description,
        "messages": [{ "role": "user", "content": { "type": "text", "text": text } }]
    }))
}

fn value_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Replace `{{name}}` with its value in one pass. Unknown placeholders are left as they are,
/// and substituted values are never rescanned.
pub(crate) fn render(template: &str, values: &[(&str, String)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find("}}") {
            Some(end) => {
                let key = &after[..end];
                match values.iter().find(|(k, _)| *k == key) {
                    Some((_, v)) => out.push_str(v),
                    None => out.push_str(&rest[start..start + 2 + end + 2]),
                }
                rest = &after[end + 2..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_the_five_prompts_with_arguments() {
        let l = list();
        let names: Vec<&str> = l["prompts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            vec![
                "explain_lean_error",
                "find_lemma_for_goal",
                "reduce_proof_timeout",
                "suggest_imports",
                "summarize_proof_state"
            ]
        );
        assert_eq!(l["prompts"][0]["arguments"][0]["required"], true);
    }

    #[test]
    fn get_fills_arguments() {
        let r = get(&json!({"name": "suggest_imports", "arguments": {"symbol": "Nat.add_comm"}})).unwrap();
        let text = r["messages"][0]["content"]["text"].as_str().unwrap();
        assert!(text.contains("`Nat.add_comm` is in scope"));
        assert!(!text.contains("{{"));
    }

    #[test]
    fn get_requires_required_arguments() {
        let err = get(&json!({"name": "find_lemma_for_goal"})).unwrap_err();
        assert_eq!(err.0, INVALID_PARAMS);
        assert!(err.1.contains("goal"));
    }

    #[test]
    fn get_rejects_unknown_prompt() {
        assert_eq!(get(&json!({"name": "nope"})).unwrap_err().0, INVALID_PARAMS);
    }

    #[test]
    fn optional_arguments_may_be_omitted() {
        let r = get(&json!({"name": "explain_lean_error", "arguments": {"error": "boom"}})).unwrap();
        let text = r["messages"][0]["content"]["text"].as_str().unwrap();
        assert!(text.contains("boom"));
        assert!(text.contains("File: \n"));
    }

    #[test]
    fn substituted_values_are_not_rescanned() {
        let out = render("a {{x}} b {{y}}", &[("x", "{{y}}".into()), ("y", "Y".into())]);
        assert_eq!(out, "a {{y}} b Y");
        assert_eq!(render("open {{ unclosed", &[]), "open {{ unclosed");
    }
}
