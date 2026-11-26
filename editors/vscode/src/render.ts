// SPDX-License-Identifier: MIT OR Apache-2.0
// Pure rendering helpers: no VS Code API, so they can be tested with `node --test`.

/** One goal as the server returns it from `tpt-lean.proofState`. */
export interface GoalView {
    hypotheses: string[];
    target: string | null;
    text: string;
}

/** One lemma candidate as the server returns it from `tpt-lean.searchLemmas`. */
export interface LemmaView {
    name: string;
    kind: string;
    source: string;
    line: number;
    score: number;
}

/** The result of `tpt-lean.explainError`. */
export interface ExplainView {
    error: { message: string; line: number; column: number } | null;
    goals: string[];
}

/** Escape text for HTML element content and double-quoted attributes. */
export function escapeHtml(text: string): string {
    return text
        .replace(/&/g, "&amp;")
        .replace(/</g, "&lt;")
        .replace(/>/g, "&gt;")
        .replace(/"/g, "&quot;")
        .replace(/'/g, "&#39;");
}

/** The proof-state panel body. Every string from Lean is escaped before it is placed. */
export function renderGoalsHtml(goals: GoalView[], line: number, character: number): string {
    const header = `<p class="where">Line ${line + 1}, column ${character + 1}</p>`;
    if (goals.length === 0) {
        return `${header}<p class="none">No goals here.</p>`;
    }
    const blocks = goals.map((g, i) => {
        const hyps = g.hypotheses.length
            ? `<ul class="hyps">${g.hypotheses.map((h) => `<li><code>${escapeHtml(h)}</code></li>`).join("")}</ul>`
            : `<p class="none">No hypotheses.</p>`;
        const target = g.target !== null
            ? `<div class="target"><code>⊢ ${escapeHtml(g.target)}</code></div>`
            : `<div class="target"><code>${escapeHtml(g.text)}</code></div>`;
        return `<section class="goal"><h2>Goal ${i + 1} of ${goals.length}</h2>${hyps}${target}</section>`;
    });
    return header + blocks.join("");
}

/** Quick-pick rows for lemma candidates. The label is the name, so it can be inserted as text. */
export function lemmaItems(lemmas: LemmaView[]): { label: string; description: string; detail: string; name: string }[] {
    return lemmas.map((l) => ({
        label: l.name,
        description: l.kind,
        detail: `${l.source}:${l.line}  score ${l.score.toFixed(2)}`,
        name: l.name,
    }));
}

/** Plain-text explanation of an error and the goals at its position, for an output channel. */
export function explainText(result: ExplainView): string {
    const lines: string[] = [];
    if (result.error) {
        const e = result.error;
        lines.push(`error at ${e.line + 1}:${e.column + 1}`, e.message);
    } else {
        lines.push("no error at this position");
    }
    if (result.goals.length > 0) {
        lines.push("", `goals (${result.goals.length}):`);
        result.goals.forEach((g, i) => lines.push(`--- goal ${i + 1} ---`, g));
    }
    return lines.join("\n");
}
