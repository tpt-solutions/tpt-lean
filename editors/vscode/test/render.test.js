// SPDX-License-Identifier: MIT OR Apache-2.0
const test = require("node:test");
const assert = require("node:assert/strict");
const { escapeHtml, renderGoalsHtml, lemmaItems, explainText } = require("../out/render");

test("escapes markup from Lean output", () => {
    assert.equal(escapeHtml(`<script>a & "b" 'c'</script>`), "&lt;script&gt;a &amp; &quot;b&quot; &#39;c&#39;&lt;/script&gt;");
});

test("goal panel escapes hypotheses and target", () => {
    const html = renderGoalsHtml(
        [{ hypotheses: ["h : <img onerror=x>"], target: "a < b", text: "" }],
        0,
        4,
    );
    assert.ok(!html.includes("<img"), html);
    assert.ok(html.includes("&lt;img onerror=x&gt;"), html);
    assert.ok(html.includes("a &lt; b"), html);
    assert.ok(html.includes("Line 1, column 5"), html);
});

test("goal panel says when there are no goals", () => {
    assert.ok(renderGoalsHtml([], 2, 0).includes("No goals here."));
});

test("goal panel falls back to the raw text when the target cannot be split", () => {
    const html = renderGoalsHtml([{ hypotheses: [], target: null, text: "weird <goal>" }], 0, 0);
    assert.ok(html.includes("weird &lt;goal&gt;"));
});

test("lemma rows carry the bare name for insertion", () => {
    const [row] = lemmaItems([{ name: "Nat.add_comm", kind: "theorem", source: "Mathlib/Nat.lean", line: 12, score: 0.5 }]);
    assert.equal(row.name, "Nat.add_comm");
    assert.equal(row.label, "Nat.add_comm");
    assert.equal(row.detail, "Mathlib/Nat.lean:12  score 0.50");
});

test("explanation lists the error and every goal", () => {
    const text = explainText({ error: { message: "unknown identifier 'x'", line: 3, column: 1 }, goals: ["g1", "g2"] });
    assert.match(text, /error at 4:2/);
    assert.match(text, /goals \(2\)/);
    assert.match(text, /--- goal 2 ---\ng2/);
});

test("explanation without an error says so", () => {
    assert.equal(explainText({ error: null, goals: [] }), "no error at this position");
});
