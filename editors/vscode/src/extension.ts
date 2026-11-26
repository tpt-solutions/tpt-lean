// SPDX-License-Identifier: MIT OR Apache-2.0
// The tpt-lean VS Code extension: starts `tpt-lean lsp` for Lean files and adds three commands
// and a proof-state panel on top of the server's workspace/executeCommand requests.

import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";
import { LanguageClient, LanguageClientOptions, ServerOptions, TransportKind } from "vscode-languageclient/node";
import {
    ExplainView,
    GoalView,
    LemmaView,
    escapeHtml,
    explainText,
    lemmaItems,
    renderGoalsHtml,
} from "./render";

const SEARCH_LEMMAS = "tpt-lean.searchLemmas";
const EXPLAIN_ERROR = "tpt-lean.explainError";
const PROOF_STATE = "tpt-lean.proofState";
const LEAN_EXTENSION = "leanprover.lean4";
const DEBOUNCE_MS = 150;

interface Position {
    uri: string;
    line: number;
    character: number;
}

interface ProofStateView {
    uri: string;
    line: number;
    character: number;
    goals: GoalView[];
}

let client: LanguageClient | undefined;
let output: vscode.OutputChannel;
let panel: vscode.WebviewPanel | undefined;
let panelTimer: NodeJS.Timeout | undefined;

export async function activate(context: vscode.ExtensionContext): Promise<void> {
    output = vscode.window.createOutputChannel("tpt-lean");
    context.subscriptions.push(output);

    warnAboutLeanExtension(context);

    context.subscriptions.push(
        vscode.commands.registerCommand("tpt-lean.searchLemmas", searchLemmas),
        vscode.commands.registerCommand("tpt-lean.explainError", explainError),
        vscode.commands.registerCommand("tpt-lean.showProofState", showProofState),
        vscode.commands.registerCommand("tpt-lean.restartServer", restartServer),
        vscode.window.onDidChangeTextEditorSelection(() => scheduleProofStateRefresh()),
        vscode.window.onDidChangeActiveTextEditor(() => scheduleProofStateRefresh()),
    );

    if (vscode.workspace.getConfiguration("tpt-lean").get<boolean>("enable", true)) {
        await startClient();
    }
}

export async function deactivate(): Promise<void> {
    if (panelTimer) {
        clearTimeout(panelTimer);
    }
    await client?.stop();
    client = undefined;
}

async function startClient(): Promise<void> {
    const root = projectRoot();
    if (!root) {
        vscode.window.showWarningMessage(
            "tpt-lean: open a folder that contains a Lean project (lakefile.lean or lakefile.toml).",
        );
        return;
    }
    const exe = vscode.workspace.getConfiguration("tpt-lean").get<string>("path", "tpt-lean");
    const serverOptions: ServerOptions = {
        command: exe,
        args: ["lsp", "--project", root],
        transport: TransportKind.stdio,
    };
    const clientOptions: LanguageClientOptions = {
        documentSelector: [{ scheme: "file", language: "lean4" }],
        outputChannel: output,
    };
    const started = new LanguageClient("tpt-lean", "tpt-lean", serverOptions, clientOptions);
    try {
        await started.start();
        client = started;
        output.appendLine(`tpt-lean server started for ${root} using ${exe}`);
    } catch (e) {
        vscode.window.showErrorMessage(
            `tpt-lean could not start (${errorText(e)}). Check the tpt-lean.path setting or install tpt-lean.`,
        );
    }
}

async function restartServer(): Promise<void> {
    await client?.stop();
    client = undefined;
    await startClient();
}

/** The nearest folder with a lakefile, walking up from the active file, else the first workspace folder. */
function projectRoot(): string | undefined {
    const folders = vscode.workspace.workspaceFolders ?? [];
    const active = vscode.window.activeTextEditor?.document.uri;
    if (active?.scheme === "file") {
        let dir = path.dirname(active.fsPath);
        for (;;) {
            if (hasLakefile(dir)) {
                return dir;
            }
            const parent = path.dirname(dir);
            if (parent === dir) {
                break;
            }
            dir = parent;
        }
    }
    const withLakefile = folders.find((f) => hasLakefile(f.uri.fsPath));
    return (withLakefile ?? folders[0])?.uri.fsPath;
}

function hasLakefile(dir: string): boolean {
    return fs.existsSync(path.join(dir, "lakefile.lean")) || fs.existsSync(path.join(dir, "lakefile.toml"));
}

/** The cursor in the active Lean editor, or undefined when the active editor is not Lean. */
function leanCursor(): Position | undefined {
    const editor = vscode.window.activeTextEditor;
    if (!editor || editor.document.languageId !== "lean4") {
        return undefined;
    }
    const p = editor.selection.active;
    return { uri: editor.document.uri.toString(), line: p.line, character: p.character };
}

async function request<T>(command: string, position: Position): Promise<T> {
    if (!client) {
        throw new Error("the tpt-lean server is not running (see the tpt-lean output channel)");
    }
    return client.sendRequest<T>("workspace/executeCommand", { command, arguments: [position] });
}

async function searchLemmas(): Promise<void> {
    const pos = leanCursor();
    if (!pos) {
        vscode.window.showInformationMessage("tpt-lean: place the cursor in a Lean file first.");
        return;
    }
    try {
        const lemmas = await request<LemmaView[]>(SEARCH_LEMMAS, pos);
        if (lemmas.length === 0) {
            vscode.window.showInformationMessage("tpt-lean: no lemma candidates for this goal.");
            return;
        }
        const pick = await vscode.window.showQuickPick(lemmaItems(lemmas), {
            placeHolder: "Lemma candidates for the goal at the cursor. The chosen name is copied.",
            matchOnDetail: true,
        });
        if (pick) {
            await vscode.env.clipboard.writeText(pick.name);
            vscode.window.setStatusBarMessage(`tpt-lean: copied ${pick.name}`, 3000);
        }
    } catch (e) {
        vscode.window.showErrorMessage(`tpt-lean: lemma search failed: ${errorText(e)}`);
    }
}

async function explainError(): Promise<void> {
    const pos = leanCursor();
    if (!pos) {
        vscode.window.showInformationMessage("tpt-lean: place the cursor in a Lean file first.");
        return;
    }
    try {
        const result = await request<ExplainView>(EXPLAIN_ERROR, pos);
        output.clear();
        output.appendLine(explainText(result));
        output.show(true);
    } catch (e) {
        vscode.window.showErrorMessage(`tpt-lean: explain failed: ${errorText(e)}`);
    }
}

function showProofState(): void {
    if (!panel) {
        panel = vscode.window.createWebviewPanel(
            "tptLeanProofState",
            "tpt-lean: Proof State",
            { viewColumn: vscode.ViewColumn.Beside, preserveFocus: true },
            { enableScripts: false, retainContextWhenHidden: false },
        );
        panel.onDidDispose(() => {
            panel = undefined;
        });
    }
    panel.reveal(panel.viewColumn, true);
    void refreshProofState();
}

function scheduleProofStateRefresh(): void {
    const followCursor = vscode.workspace
        .getConfiguration("tpt-lean")
        .get<boolean>("proofStatePanel.followCursor", true);
    if (!panel || !followCursor) {
        return;
    }
    if (panelTimer) {
        clearTimeout(panelTimer);
    }
    panelTimer = setTimeout(() => void refreshProofState(), DEBOUNCE_MS);
}

async function refreshProofState(): Promise<void> {
    if (!panel) {
        return;
    }
    const pos = leanCursor();
    if (!pos) {
        // Keep the last state visible when the focus moves to a non-Lean editor.
        return;
    }
    try {
        const state = await request<ProofStateView>(PROOF_STATE, pos);
        panel.webview.html = page(renderGoalsHtml(state.goals, state.line, state.character));
    } catch (e) {
        panel.webview.html = page(`<p class="none">${escapeHtml(errorText(e))}</p>`);
    }
}

/** A complete page with a strict Content Security Policy: no scripts, inline styles only. */
function page(body: string): string {
    return `<!doctype html>
<html><head><meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline';">
<style>
body { font-family: var(--vscode-font-family); color: var(--vscode-foreground); padding: 0 12px; }
h2 { font-size: 1em; margin: 1em 0 0.3em; }
code { font-family: var(--vscode-editor-font-family); white-space: pre-wrap; }
.target { border-left: 3px solid var(--vscode-textLink-foreground); padding: 4px 8px; margin: 6px 0; }
.where, .none { opacity: 0.75; }
ul.hyps { padding-left: 1.2em; margin: 0; }
</style></head><body>${body}</body></html>`;
}

/** The Lean 4 extension runs its own server; two servers on one project duplicate diagnostics. */
function warnAboutLeanExtension(context: vscode.ExtensionContext): void {
    if (!vscode.extensions.getExtension(LEAN_EXTENSION)) {
        return;
    }
    const key = "tpt-lean.warnedAboutLeanExtension";
    if (context.globalState.get<boolean>(key)) {
        return;
    }
    void context.globalState.update(key, true);
    vscode.window.showWarningMessage(
        "tpt-lean and the Lean 4 extension both run a Lean server. Disable one of them for this workspace to avoid duplicate diagnostics.",
    );
}

function errorText(e: unknown): string {
    return e instanceof Error ? e.message : String(e);
}
