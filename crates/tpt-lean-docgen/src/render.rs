// SPDX-License-Identifier: MIT OR Apache-2.0
//! Static site generation: HTML pages, stylesheet, search index and JSON export.
//!
//! Every page is self-contained: no network access, no external scripts or fonts.
//! Source text is always escaped before it reaches the output.

use crate::model::{self, Collected, DeclInfo, Model, PROJECT_PACKAGE};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use tpt_lean_core::{Error, ErrorCode, Result};

const STYLE: &str = r#"
:root {
  color-scheme: light dark;
  --bg: #fdfdfb; --fg: #1d1f24; --muted: #5d6470; --accent: #1f5fbf;
  --panel: #f1f3f6; --border: #d8dde6; --code-bg: #f5f6f9;
  --kind: #0f7b6c; --external: #8a5a00;
}
@media (prefers-color-scheme: dark) {
  :root {
    --bg: #14161b; --fg: #e4e7ee; --muted: #9aa3b2; --accent: #7fb2ff;
    --panel: #1c1f26; --border: #2e333d; --code-bg: #1a1d24;
    --kind: #5fd3b8; --external: #e0b35c;
  }
}
* { box-sizing: border-box; }
body { margin: 0; background: var(--bg); color: var(--fg); font: 16px/1.5 system-ui, sans-serif; }
a { color: var(--accent); overflow-wrap: anywhere; }
header {
  display: flex; flex-wrap: wrap; align-items: center; gap: 12px;
  padding: 10px 16px; border-bottom: 1px solid var(--border); background: var(--panel);
}
header nav a { margin-right: 10px; }
main { max-width: 960px; margin: 0 auto; padding: 16px; }
footer { max-width: 960px; margin: 0 auto; padding: 0 16px 24px; font-size: 14px; }
pre, code { font-family: ui-monospace, Consolas, monospace; font-size: 14px; }
pre {
  background: var(--code-bg); border: 1px solid var(--border); border-radius: 6px;
  padding: 12px; overflow-x: auto; white-space: pre-wrap;
}
.kind { color: var(--kind); font-family: ui-monospace, Consolas, monospace; }
.muted { color: var(--muted); }
.external { color: var(--external); }
.crumbs { color: var(--muted); font-size: 14px; }
.doc { white-space: pre-wrap; }
.source { white-space: pre; }
.line { display: block; }
.line:target { background: var(--panel); }
.ln {
  display: inline-block; width: 4em; padding-right: 1em; text-align: right;
  color: var(--muted); text-decoration: none; user-select: none;
}
#search { position: relative; margin-left: auto; max-width: 100%; }
#search-input {
  width: 260px; max-width: 100%; padding: 6px 8px; font: inherit;
  color: var(--fg); background: var(--bg); border: 1px solid var(--border); border-radius: 6px;
}
#search-results {
  position: absolute; right: 0; z-index: 10; width: min(420px, 90vw); max-height: 60vh;
  overflow-y: auto; background: var(--panel); border: 1px solid var(--border); border-radius: 6px;
}
#search-results[hidden] { display: none; }
#search-results a { display: block; padding: 6px 10px; text-decoration: none; }
#search-results a:hover { background: var(--bg); }
#search-results .sub { color: var(--muted); font-size: 13px; margin-left: 8px; }
@media (max-width: 600px) {
  main { padding: 12px; }
  #search, #search-input { width: 100%; }
  #search-results { left: 0; right: auto; width: 100%; }
}
"#;

const SEARCH_JS: &str = r#"
(function () {
  var box = document.getElementById('search-input');
  var out = document.getElementById('search-results');
  var holder = document.getElementById('search');
  if (!box || !out || !holder) { return; }
  var root = holder.getAttribute('data-root') || '';
  var loading = null;
  var failMessage = 'Search needs this folder served over HTTP. Serve it and reload.';

  function load() {
    if (!loading) {
      loading = fetch(root + 'search-index.json').then(function (r) {
        if (!r.ok) { throw new Error('HTTP ' + r.status); }
        return r.json();
      });
    }
    return loading;
  }

  function clear() {
    while (out.firstChild) { out.removeChild(out.firstChild); }
  }

  function render(query) {
    var q = query.trim().toLowerCase();
    clear();
    if (q === '') { out.hidden = true; return; }
    out.hidden = false;
    load().then(function (items) {
      clear();
      var scored = [];
      for (var i = 0; i < items.length; i++) {
        var it = items[i];
        var name = String(it.n).toLowerCase();
        var hay = (name + ' ' + String(it.s) + ' ' + String(it.d)).toLowerCase();
        if (hay.indexOf(q) === -1) { continue; }
        var score = name.indexOf(q) === 0 ? 0 : (name.indexOf(q) !== -1 ? 1 : 2);
        scored.push({ score: score, item: it });
      }
      scored.sort(function (a, b) {
        if (a.score !== b.score) { return a.score - b.score; }
        return a.item.n < b.item.n ? -1 : (a.item.n > b.item.n ? 1 : 0);
      });
      if (scored.length === 0) {
        var none = document.createElement('div');
        none.className = 'sub';
        none.textContent = 'No matches';
        out.appendChild(none);
      }
      for (var j = 0; j < scored.length && j < 40; j++) {
        var hit = scored[j].item;
        var a = document.createElement('a');
        a.href = root + hit.u;
        a.textContent = hit.n;
        var sub = document.createElement('span');
        sub.className = 'sub';
        sub.textContent = hit.k + (hit.s ? ' in ' + hit.s : '');
        a.appendChild(sub);
        out.appendChild(a);
      }
    }).catch(function () {
      clear();
      var msg = document.createElement('div');
      msg.className = 'sub';
      msg.textContent = failMessage;
      out.appendChild(msg);
    });
  }

  box.addEventListener('input', function () { render(box.value); });
  box.addEventListener('keydown', function (e) {
    if (e.key === 'Escape') { box.value = ''; render(''); }
  });
})();
"#;

/// Escape text for HTML element content and quoted attribute values.
pub(crate) fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(c),
        }
    }
    out
}

/// ASCII-only, filesystem- and URL-safe fragment derived from a name.
fn slug(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(60)
        .collect();
    if out.is_empty() {
        out.push('x');
    }
    out
}

// Paths are relative to the output root. The index prefix keeps every file name
// unique, so two different entities never write the same file.
fn decl_href(i: usize, d: &DeclInfo) -> String {
    format!("decls/{i}-{}.html", slug(&d.name))
}

fn src_href(i: usize, module_name: &str) -> String {
    format!("src/{i}-{}.html", slug(module_name))
}

fn ns_href(i: usize, ns: &str) -> String {
    format!("ns/{i}-{}.html", slug(ns))
}

fn ns_ids(model: &Model) -> HashMap<&str, usize> {
    model
        .namespaces
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i))
        .collect()
}

/// Write the whole site into `out`. Returns the number of files written.
pub fn write_site(out: &Path, collected: &Collected) -> Result<usize> {
    let model = &collected.model;
    let ids = ns_ids(model);
    let mut count = 0;
    emit(out, "style.css", STYLE, &mut count)?;
    emit(out, "search-index.json", &search_index(model)?, &mut count)?;
    emit(out, "api.json", &model::to_json(model)?, &mut count)?;
    emit(
        out,
        "index.html",
        &page(model, "Overview", "", &index_body(model)),
        &mut count,
    )?;
    emit(
        out,
        "namespaces.html",
        &page(model, "Namespaces", "", &namespaces_body(model, &ids)),
        &mut count,
    )?;
    emit(
        out,
        "deps.html",
        &page(model, "Dependencies", "", &deps_body(model)),
        &mut count,
    )?;
    for (i, ns) in model.namespaces.iter().enumerate() {
        let body = namespace_body(model, ns);
        let title = format!("namespace {ns}");
        emit(
            out,
            &ns_href(i, ns),
            &page(model, &title, "../", &body),
            &mut count,
        )?;
    }
    for (i, d) in model.declarations.iter().enumerate() {
        let body = decl_body(model, d, &ids);
        emit(
            out,
            &decl_href(i, d),
            &page(model, &d.name, "../", &body),
            &mut count,
        )?;
    }
    for (i, m) in model.modules.iter().enumerate() {
        let body = source_body(model, i, &collected.sources[i]);
        let href = src_href(i, &m.name);
        emit(out, &href, &page(model, &m.file, "../", &body), &mut count)?;
    }
    Ok(count)
}

fn emit(out: &Path, rel: &str, content: &str, count: &mut usize) -> Result<()> {
    let path = out.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| Error::from(e).with_path(parent))?;
    }
    std::fs::write(&path, content).map_err(|e| Error::from(e).with_path(&path))?;
    *count += 1;
    Ok(())
}

/// Wrap a page body in the shared chrome. `prefix` is the relative path back to the site root.
fn page(model: &Model, title: &str, prefix: &str, body: &str) -> String {
    let project = esc(&model.project);
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title} - {project}</title>
<link rel="stylesheet" href="{prefix}style.css">
</head>
<body>
<header>
<strong><a href="{prefix}index.html">{project}</a></strong>
<nav><a href="{prefix}index.html">Overview</a><a href="{prefix}namespaces.html">Namespaces</a><a href="{prefix}deps.html">Dependencies</a></nav>
<div id="search" data-root="{prefix}">
<input id="search-input" type="search" placeholder="Search declarations" aria-label="Search declarations" autocomplete="off">
<div id="search-results" role="list" hidden></div>
</div>
</header>
<main>
{body}
</main>
<footer class="muted">Generated by tpt-lean docgen.</footer>
<script>{SEARCH_JS}</script>
</body>
</html>
"#,
        title = esc(title),
        project = project,
        prefix = prefix,
        body = body,
        SEARCH_JS = SEARCH_JS,
    )
}

/// `file:line` for lists. Packages other than the project are prefixed with their name.
fn location(model: &Model, d: &DeclInfo) -> String {
    let m = &model.modules[d.module];
    if m.package == PROJECT_PACKAGE {
        format!("{}:{}", m.file, d.line)
    } else {
        format!("{}: {}:{}", m.package, m.file, d.line)
    }
}

fn decl_row(model: &Model, i: usize, d: &DeclInfo, prefix: &str) -> String {
    let private = if d.is_private {
        " <span class=\"muted\">private</span>"
    } else {
        ""
    };
    format!(
        "<li><a href=\"{prefix}{href}\">{name}</a> <span class=\"kind\">{kind}</span>{private} <span class=\"muted\">{loc}</span></li>\n",
        href = decl_href(i, d),
        name = esc(&d.name),
        kind = esc(&d.kind),
        loc = esc(&location(model, d)),
    )
}

fn index_body(model: &Model) -> String {
    let mut public: Vec<(usize, &DeclInfo)> = model
        .declarations
        .iter()
        .enumerate()
        .filter(|(_, d)| !d.is_private)
        .collect();
    public.sort_by(|a, b| a.1.name.cmp(&b.1.name));
    let mut b = String::new();
    b.push_str(&format!("<h1>{}</h1>\n", esc(&model.project)));
    b.push_str(&format!(
        "<p class=\"muted\">{} modules, {} public declarations, {} namespaces, {} import edges.</p>\n",
        model.modules.len(),
        public.len(),
        model.namespaces.len(),
        model.imports.len(),
    ));
    let packages: Vec<String> = model.packages.iter().map(|p| esc(p)).collect();
    b.push_str(&format!(
        "<p class=\"muted\">Packages: {}</p>\n",
        packages.join(", ")
    ));
    b.push_str("<h2>Declarations</h2>\n<ul>\n");
    for (i, d) in public {
        b.push_str(&decl_row(model, i, d, ""));
    }
    b.push_str("</ul>\n");
    b
}

#[derive(Default)]
struct NsNode {
    full: String,
    children: BTreeMap<String, NsNode>,
}

fn namespaces_body(model: &Model, ids: &HashMap<&str, usize>) -> String {
    let mut root = NsNode::default();
    for n in &model.namespaces {
        let mut node = &mut root;
        let mut acc = String::new();
        for seg in n.split('.') {
            if !acc.is_empty() {
                acc.push('.');
            }
            acc.push_str(seg);
            let full = acc.clone();
            node = node.children.entry(seg.to_string()).or_insert_with(|| NsNode {
                full,
                children: BTreeMap::new(),
            });
        }
    }
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for d in &model.declarations {
        *counts.entry(d.namespace.as_str()).or_insert(0) += 1;
    }
    let mut b = String::from("<h1>Namespaces</h1>\n");
    if root.children.is_empty() {
        b.push_str("<p class=\"muted\">No namespaces declared.</p>\n");
    } else {
        render_tree(&root, &counts, ids, &mut b);
    }
    b
}

fn render_tree(node: &NsNode, counts: &HashMap<&str, usize>, ids: &HashMap<&str, usize>, out: &mut String) {
    out.push_str("<ul>\n");
    for (seg, child) in &node.children {
        let count = counts.get(child.full.as_str()).copied().unwrap_or(0);
        let label = match ids.get(child.full.as_str()) {
            Some(i) => format!("<a href=\"{}\">{}</a>", ns_href(*i, &child.full), esc(seg)),
            None => esc(seg),
        };
        out.push_str(&format!(
            "<li>{label} <span class=\"muted\">({count} declarations)</span>"
        ));
        if !child.children.is_empty() {
            render_tree(child, counts, ids, out);
        }
        out.push_str("</li>\n");
    }
    out.push_str("</ul>\n");
}

fn namespace_body(model: &Model, ns: &str) -> String {
    let mut b = format!(
        "<p class=\"crumbs\"><a href=\"../index.html\">Overview</a> / <a href=\"../namespaces.html\">Namespaces</a></p>\n<h1>Namespace <code>{}</code></h1>\n",
        esc(ns)
    );
    let prefix = format!("{ns}.");
    let children: Vec<(usize, &String)> = model
        .namespaces
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            c.strip_prefix(prefix.as_str())
                .is_some_and(|rest| !rest.contains('.'))
        })
        .collect();
    if !children.is_empty() {
        b.push_str("<h2>Sub-namespaces</h2>\n<ul>\n");
        for (i, c) in children {
            b.push_str(&format!(
                "<li><a href=\"../{}\"><code>{}</code></a></li>\n",
                ns_href(i, c),
                esc(c)
            ));
        }
        b.push_str("</ul>\n");
    }
    b.push_str("<h2>Declarations</h2>\n<ul>\n");
    for (i, d) in model.declarations.iter().enumerate() {
        if d.namespace == ns {
            b.push_str(&decl_row(model, i, d, "../"));
        }
    }
    b.push_str("</ul>\n");
    b
}

fn decl_body(model: &Model, d: &DeclInfo, ids: &HashMap<&str, usize>) -> String {
    let m = &model.modules[d.module];
    let src = src_href(d.module, &m.name);
    let mut crumbs = String::from("<a href=\"../index.html\">Overview</a>");
    if !d.namespace.is_empty() {
        if let Some(i) = ids.get(d.namespace.as_str()) {
            crumbs.push_str(&format!(
                " / <a href=\"../{}\">{}</a>",
                ns_href(*i, &d.namespace),
                esc(&d.namespace)
            ));
        }
    }
    let private = if d.is_private {
        " <span class=\"muted\">private</span>"
    } else {
        ""
    };
    let docs = match &d.docstring {
        Some(doc) if !doc.is_empty() => format!("<div class=\"doc\">{}</div>\n", esc(doc)),
        _ => "<p class=\"muted\">No docstring.</p>\n".to_string(),
    };
    format!(
        "<p class=\"crumbs\">{crumbs}</p>\n\
         <h1>{name}</h1>\n\
         <p><span class=\"kind\">{kind}</span>{private}</p>\n\
         <p>Source: <a href=\"../{src}#L{line}\">{loc}</a> (package {package}, module <code>{module}</code>)</p>\n\
         <h2>Signature</h2>\n<pre><code>{sig}</code></pre>\n\
         <h2>Documentation</h2>\n{docs}",
        name = esc(&d.name),
        kind = esc(&d.kind),
        line = d.line,
        loc = esc(&format!("{}:{}", m.file, d.line)),
        package = esc(&m.package),
        module = esc(&m.name),
        sig = esc(&d.signature),
    )
}

fn deps_body(model: &Model) -> String {
    let mut importers: Vec<Vec<usize>> = vec![Vec::new(); model.modules.len()];
    for e in &model.imports {
        if let Some(to) = e.resolved {
            importers[to].push(e.from);
        }
    }
    let mut b = String::from(
        "<h1>Dependencies</h1>\n<p class=\"muted\">Import edges between documented modules. Imports that do not match a documented module are shown as external.</p>\n",
    );
    for (i, m) in model.modules.iter().enumerate() {
        b.push_str(&format!(
            "<section id=\"m{i}\">\n<h2><a href=\"{}\">{}</a> <span class=\"muted\">{}</span></h2>\n",
            src_href(i, &m.name),
            esc(&m.name),
            esc(&m.package)
        ));
        let outgoing: Vec<&model::ImportEdge> = model.imports.iter().filter(|e| e.from == i).collect();
        if outgoing.is_empty() {
            b.push_str("<p class=\"muted\">No imports.</p>\n");
        } else {
            b.push_str("<p>Imports:</p>\n<ul>\n");
            for e in outgoing {
                match e.resolved {
                    Some(j) => b.push_str(&format!(
                        "<li><a href=\"{}\">{}</a></li>\n",
                        src_href(j, &model.modules[j].name),
                        esc(&e.to)
                    )),
                    None => b.push_str(&format!(
                        "<li><span class=\"external\">{}</span> <span class=\"muted\">(external)</span></li>\n",
                        esc(&e.to)
                    )),
                }
            }
            b.push_str("</ul>\n");
        }
        if !importers[i].is_empty() {
            b.push_str("<p>Imported by:</p>\n<ul>\n");
            for &j in &importers[i] {
                b.push_str(&format!(
                    "<li><a href=\"{}\">{}</a></li>\n",
                    src_href(j, &model.modules[j].name),
                    esc(&model.modules[j].name)
                ));
            }
            b.push_str("</ul>\n");
        }
        b.push_str("</section>\n");
    }
    b
}

fn source_body(model: &Model, i: usize, src: &str) -> String {
    let m = &model.modules[i];
    let mut b = format!(
        "<p class=\"crumbs\"><a href=\"../index.html\">Overview</a> / <a href=\"../deps.html#m{i}\">Dependencies</a> / package {}</p>\n<h1><code>{}</code></h1>\n",
        esc(&m.package),
        esc(&m.file)
    );
    let decls: Vec<(usize, &DeclInfo)> = model
        .declarations
        .iter()
        .enumerate()
        .filter(|(_, d)| d.module == i)
        .collect();
    if !decls.is_empty() {
        b.push_str("<h2>Declarations in this file</h2>\n<ul>\n");
        for (j, d) in decls {
            b.push_str(&format!(
                "<li><a href=\"../{}\">{}</a> <span class=\"kind\">{}</span> <span class=\"muted\">line {}</span></li>\n",
                decl_href(j, d),
                esc(&d.name),
                esc(&d.kind),
                d.line
            ));
        }
        b.push_str("</ul>\n");
    }
    b.push_str("<h2>Source</h2>\n<pre class=\"source\">");
    for (n, line) in src.lines().enumerate() {
        let n = n + 1;
        b.push_str(&format!(
            "<span class=\"line\" id=\"L{n}\"><a class=\"ln\" href=\"#L{n}\">{n}</a>{}</span>\n",
            esc(line)
        ));
    }
    b.push_str("</pre>\n");
    b
}

#[derive(Serialize)]
struct SearchItem<'a> {
    /// Qualified name.
    n: &'a str,
    /// Declaration kind.
    k: &'a str,
    /// Namespace.
    s: &'a str,
    /// URL relative to the site root.
    u: String,
    /// Short docstring preview.
    d: String,
}

const SEARCH_DOC_CHARS: usize = 160;

fn search_index(model: &Model) -> Result<String> {
    let items: Vec<SearchItem> = model
        .declarations
        .iter()
        .enumerate()
        .filter(|(_, d)| !d.is_private)
        .map(|(i, d)| SearchItem {
            n: &d.name,
            k: &d.kind,
            s: &d.namespace,
            u: decl_href(i, d),
            d: d.docstring
                .as_deref()
                .unwrap_or("")
                .chars()
                .take(SEARCH_DOC_CHARS)
                .collect(),
        })
        .collect();
    serde_json::to_string(&items).map_err(|e| Error::new(ErrorCode::Internal, format!("search index: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_markup_and_quotes() {
        assert_eq!(
            esc("<a href='x'>&\"</a>"),
            "&lt;a href=&#x27;x&#x27;&gt;&amp;&quot;&lt;/a&gt;"
        );
    }

    #[test]
    fn slugs_are_ascii_safe() {
        assert_eq!(slug("Foo.add_comm'"), "Foo_add_comm_");
        assert_eq!(slug("αβ"), "__");
        assert_eq!(slug(""), "x");
    }
}
