<script lang="ts">
	import { EditorView, basicSetup } from "codemirror";
	import { keymap } from "@codemirror/view";
	import { python } from "@codemirror/lang-python";
	import { indentUnit } from "@codemirror/language";
	import { indentWithTab } from "@codemirror/commands";
	import { githubLight, githubDark } from "@uiw/codemirror-theme-github";
	import { Compartment } from "@codemirror/state";
	import { mode } from "mode-watcher";

	let dom: HTMLDivElement;
	let editor: EditorView;

	// eslint-disable-next-line @typescript-eslint/no-unused-expressions
	void EditorView,
		basicSetup,
		keymap,
		python,
		indentUnit,
		indentWithTab,
		githubLight,
		githubDark,
		Compartment,
		mode;

	export function content() {
		return editor.state.doc.toString();
	}

	export function setContent(e: string) {
		editor.dispatch(
			editor.state.update({
				changes: { from: 0, to: editor.state.doc.length, insert: e },
			})
		);
	}

	const { onCodeEvent } = $props<{
		onCodeEvent: (event: "run" | "save") => void;
	}>();

	$effect(() => {
		let editorTheme = new Compartment();
		editor = new EditorView({
			extensions: [
				keymap.of([
					{
						key: "Mod-s",
						run: () => {
							onCodeEvent("save");
							return true;
						},
					},
					{
						key: "Mod-Enter",
						run: () => {
							onCodeEvent("run");
							return true;
						},
					},
					indentWithTab,
				]),
				EditorView.theme({
					"&": { height: "100%" },
				}),
				editorTheme.of(githubLight),
				basicSetup,
				python(),
				indentUnit.of("    "),
			],
			parent: dom,
		});
		dom.children[0].remove();

		$effect(() => {
			editor.dispatch({
				effects: editorTheme.reconfigure($mode === "light" ? githubLight : githubDark),
			});
		});
	});
</script>

<div class="contents" bind:this={dom}>
	<div class="flex h-full items-center justify-center italic">Loading...</div>
</div>
