// SPDX-License-Identifier: MIT OR Apache-2.0
//! Shell completion scripts for `tpt-lean` and `tlean`, generated from one command table.
//!
//! The table is the single list of commands the scripts complete. A test checks that every
//! command in it appears in the help text, so the scripts cannot name a command that does not
//! exist.

/// Top-level commands, each with its subcommands (empty when it takes none).
pub const COMMANDS: &[(&str, &[&str])] = &[
    ("init", &[]),
    ("doctor", &[]),
    ("sorry-report", &[]),
    ("fix", &[]),
    ("upgrade", &[]),
    ("watch", &[]),
    ("setup", &[]),
    ("config", &["show"]),
    ("clean", &[]),
    ("cache", &["status", "prune", "export", "import"]),
    ("fmt", &[]),
    ("imports", &[]),
    ("search", &[]),
    ("index", &["scan", "refresh", "status", "clear", "sidecar"]),
    ("build", &[]),
    ("check", &[]),
    ("goals", &[]),
    ("explain", &[]),
    ("context-pack", &[]),
    ("profile", &[]),
    ("docgen", &[]),
    ("ci", &[]),
    ("bisect", &[]),
    ("server", &["start", "status", "stop"]),
    ("mcp", &["stdio", "http"]),
    ("lsp", &[]),
    ("completions", &["bash", "zsh", "fish", "powershell"]),
    ("help", &[]),
];

/// Shells that have a completion script.
pub const SHELLS: &[&str] = &["bash", "zsh", "fish", "powershell"];

/// The completion script for `shell`, or `None` when the shell is not supported.
pub fn script(shell: &str) -> Option<String> {
    match shell {
        "bash" => Some(bash()),
        "zsh" => Some(zsh()),
        "fish" => Some(fish()),
        "powershell" => Some(powershell()),
        _ => None,
    }
}

fn names(list: &[&str]) -> String {
    list.join(" ")
}

fn top_names() -> String {
    COMMANDS.iter().map(|(c, _)| *c).collect::<Vec<_>>().join(" ")
}

fn bash() -> String {
    let mut s = String::from(
        "# bash completion for tpt-lean and tlean\n\
         _tpt_lean() {\n\
         \x20   local cur cmd\n\
         \x20   COMPREPLY=()\n\
         \x20   cur=\"${COMP_WORDS[COMP_CWORD]}\"\n\
         \x20   cmd=\"${COMP_WORDS[1]}\"\n\
         \x20   if [ \"$COMP_CWORD\" -eq 1 ]; then\n",
    );
    s.push_str(&format!(
        "\x20       COMPREPLY=( $(compgen -W \"{}\" -- \"$cur\") )\n",
        top_names()
    ));
    s.push_str("\x20       return 0\n\x20   fi\n\x20   case \"$cmd\" in\n");
    for (name, subs) in COMMANDS {
        if subs.is_empty() {
            continue;
        }
        s.push_str(&format!(
            "\x20       {name})\n\x20           if [ \"$COMP_CWORD\" -eq 2 ]; then\n\x20               COMPREPLY=( $(compgen -W \"{}\" -- \"$cur\") )\n\x20           fi\n\x20           ;;\n",
            names(subs)
        ));
    }
    s.push_str("\x20       *) ;;\n\x20   esac\n\x20   return 0\n}\n");
    s.push_str("complete -F _tpt_lean tpt-lean tlean\n");
    s
}

fn zsh() -> String {
    let mut s =
        String::from("#compdef tpt-lean tlean\n\n_tpt_lean() {\n    local -a commands\n    commands=(\n");
    for (name, _) in COMMANDS {
        s.push_str(&format!("        '{name}'\n"));
    }
    s.push_str(
        "    )\n    if (( CURRENT == 2 )); then\n        _describe 'command' commands\n        return\n    fi\n    case $words[2] in\n",
    );
    for (name, subs) in COMMANDS {
        if subs.is_empty() {
            continue;
        }
        s.push_str(&format!("        {name}) compadd {} ;;\n", names(subs)));
    }
    s.push_str("        *) _files ;;\n    esac\n}\n\n_tpt_lean \"$@\"\n");
    s
}

fn fish() -> String {
    let mut s = String::from("# fish completion for tpt-lean and tlean\n");
    for bin in ["tpt-lean", "tlean"] {
        s.push_str(&format!(
            "complete -c {bin} -f -n '__fish_use_subcommand' -a '{}'\n",
            top_names()
        ));
        for (name, subs) in COMMANDS {
            if subs.is_empty() {
                continue;
            }
            s.push_str(&format!(
                "complete -c {bin} -f -n '__fish_seen_subcommand_from {name}' -a '{}'\n",
                names(subs)
            ));
        }
    }
    s
}

fn powershell() -> String {
    let mut s = String::from(
        "# PowerShell completion for tpt-lean and tlean\n\
         Register-ArgumentCompleter -Native -CommandName tpt-lean,tlean -ScriptBlock {\n\
         \x20   param($wordToComplete, $commandAst, $cursorPosition)\n\
         \x20   $elements = @($commandAst.CommandElements | ForEach-Object { $_.ToString() })\n\
         \x20   # The word being completed is the last element unless the line ends in a space.\n\
         \x20   $position = $elements.Count\n\
         \x20   if ($wordToComplete -eq '') { $position += 1 }\n\
         \x20   if ($position -le 2) {\n",
    );
    s.push_str(&format!(
        "\x20       $candidates = '{}'.Split(' ')\n",
        top_names()
    ));
    s.push_str("\x20   } else {\n\x20       $candidates = switch ($elements[1]) {\n");
    for (name, subs) in COMMANDS {
        if subs.is_empty() {
            continue;
        }
        s.push_str(&format!(
            "\x20           '{name}' {{ '{}'.Split(' ') }}\n",
            names(subs)
        ));
    }
    s.push_str(
        "\x20           default { @() }\n\x20       }\n\x20   }\n\
         \x20   $candidates | Where-Object { $_ -like \"$wordToComplete*\" } | ForEach-Object {\n\
         \x20       [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterValue', $_)\n\
         \x20   }\n}\n",
    );
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_in_the_table_is_in_the_help_text() {
        let help = include_str!("commands.rs");
        for (name, subs) in COMMANDS {
            assert!(help.contains(name), "`{name}` is not mentioned in commands.rs");
            for sub in *subs {
                assert!(
                    help.contains(sub),
                    "`{name} {sub}` is not mentioned in commands.rs"
                );
            }
        }
    }

    #[test]
    fn every_shell_has_a_script_that_names_every_command() {
        for shell in SHELLS {
            let text = script(shell).expect("script");
            for (name, _) in COMMANDS {
                assert!(text.contains(name), "{shell} script lacks `{name}`");
            }
        }
    }

    #[test]
    fn unknown_shells_have_no_script() {
        assert!(script("csh").is_none());
    }
}
