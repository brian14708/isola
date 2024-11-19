<script lang="ts">
	import JSON5 from "json5";
	import { superForm } from "sveltekit-superforms";
	import { get } from "svelte/store";
	import { superstructClient } from "sveltekit-superforms/adapters";
	import { toJsonString } from "@bufbuild/protobuf";
	import * as Form from "$lib/components/ui/form";
	import { Input } from "$lib/components/ui/input";
	import { Checkbox } from "$lib/components/ui/checkbox";
	import { Textarea } from "$lib/components/ui/textarea";
	import client from "$lib/rpc/client";
	import {
		ContentType,
		MethodInfoSchema,
		ScriptService,
		type ExecutionMetadata,
		type ExecutionStreamMetadata,
		type Result,
	} from "$lib/rpc/promptkit/script/v1/service_pb";
	import { TraceLevel, TraceSchema } from "$lib/rpc/promptkit/script/v1/trace_pb";
	import { Button } from "$lib/components/ui/button";
	import CodeMirror from "$lib/components/editor/CodeMirror.svelte";
	import Menubar from "./Menubar.svelte";
	import { dataSchema, type Data, DEFAULT_DATA } from "./schema";

	let editor: CodeMirror;
	const form = superForm(DEFAULT_DATA, {
		SPA: true,
		validators: superstructClient(dataSchema),
		validationMethod: "onblur",
		onUpdate: ({ form, cancel }) => {
			if (form.valid) {
				run();
			} else {
				validateForm({ update: true });
			}

			cancel();
		},
	});
	const { form: formData, validateForm, enhance } = form;
	validateForm({ update: true });

	let result = $state<
		| undefined
		| {
				loading: boolean;
				data: string[];
				traces: string[];
		  }
		| { error: string }
	>();

	let prevData: Data = DEFAULT_DATA;
	function save(): Data {
		const code = editor.content();
		if (code && code !== prevData.code) {
			formData.update((current) => ({ ...current, code }));
		}

		const d = get(formData);
		let changed = false;
		if (d !== prevData) {
			for (const key in d) {
				if (d[key as keyof Data] !== prevData[key as keyof Data]) {
					changed = true;
					break;
				}
			}
		}
		if (changed) {
			prevData = d;
			window.localStorage.setItem("script-src", JSON.stringify(d));
		}
		return d;
	}

	let cli: ReturnType<typeof client<typeof ScriptService>>;
	$effect(() => {
		cli = client(ScriptService);
	});

	let runId = 0;
	async function run() {
		const d = save();
		const currentId = ++runId;
		result = {
			loading: true,
			data: [],
			traces: [],
		};

		const secs = Math.floor(d.timeout);
		// eslint-disable-next-line @typescript-eslint/no-explicit-any
		const args: Record<string, any> | Array<any> = JSON5.parse(d.arguments);
		const req = {
			source: {
				sourceType: {
					value: {
						script: d.code + `\n\n# ${new Date().getTime()}`,
						runtime: "python3",
						prelude: d.prelude,
					},
					case: "scriptInline" as const,
				},
			},
			spec: {
				method: d.method,
				timeout: {
					seconds: BigInt(secs),
					nanos: Math.floor((d.timeout - secs) * 1e9),
				},
				arguments: Array.isArray(args)
					? args.map((arg) => ({
							argumentType: {
								case: "json" as const,
								value: JSON.stringify(arg),
							},
						}))
					: Object.entries(args).map(([key, value]) => ({
							argumentType: {
								case: "json" as const,
								value: JSON.stringify(value),
							},
							name: key,
						})),
				traceLevel: TraceLevel.ALL,
			},
			resultContentType: [ContentType.JSON],
		};
		try {
			result = {
				loading: false,
				data: [],
				traces: [],
			};
			const { data, traces } = result;
			const append = (ret: {
				result?: Result;
				metadata?: ExecutionStreamMetadata | ExecutionMetadata;
			}) => {
				if (currentId !== runId) {
					return;
				}
				ret.metadata?.traces?.forEach((trace) => {
					traces.push(
						`[${trace.timestamp?.seconds}.${trace.timestamp?.nanos}] ${toJsonString(TraceSchema, trace)}`
					);
				});
				switch (ret.result?.resultType.case) {
					case undefined:
						return true;
					case "json":
						data.push(ret.result?.resultType.value?.toString() || "{}");
						return true;
					case "error":
						result = {
							error: `${ret.result?.resultType.value?.message}`,
						};
						return false;
					default:
						result = {
							error: `UNKNOWN RESULT`,
						};
						return false;
				}
			};

			if (d.stream) {
				for await (const ret of cli.executeServerStream(req)) {
					if (!append(ret)) {
						return;
					}
				}
			} else {
				const ret = await cli.execute(req);
				if (!append(ret)) {
					return;
				}
			}
		} catch (e) {
			if (currentId !== runId) {
				return;
			}
			result = {
				error: `${e}`,
			};
		}
	}

	$effect(() => {
		// load data from local storage
		try {
			const s = window.localStorage.getItem("script-src");
			if (s) {
				const newData = JSON.parse(s);
				formData.set(newData);
			}
		} catch (e) {
			console.error(e);
		}

		const current = get(formData);
		editor.setContent(current.code);

		const interval = setInterval(save, 5000);
		return () => clearInterval(interval);
	});

	function onCodeEvent(e: "run" | "save" | "reset") {
		if (e === "save") {
			save();
		} else if (e === "run") {
			run();
		} else if (e === "reset") {
			formData.set(DEFAULT_DATA);
			editor.setContent(DEFAULT_DATA.code);
			save();
		}
	}

	async function analyze() {
		const d = save();
		const currentId = ++runId;
		result = {
			loading: true,
			data: [],
			traces: [],
		};

		const secs = Math.floor(d.timeout);
		try {
			const r = await cli.analyze({
				source: {
					sourceType: {
						value: {
							script: d.code + `\n\n# ${new Date().getTime()}`,
							runtime: "python3",
							prelude: d.prelude,
						},
						case: "scriptInline" as const,
					},
				},
				spec: {
					timeout: {
						seconds: BigInt(secs),
						nanos: Math.floor((d.timeout - secs) * 1e9),
					},
				},
				methods: [d.method],
			});
			if (currentId !== runId) {
				return;
			}
			switch (r.resultType.case) {
				case "error": {
					result = {
						error: `${r.resultType.value?.message}`,
					};
					break;
				}
				case "analyzeResult": {
					result = {
						loading: false,
						data:
							r.resultType.value?.methodInfos?.map((r) => toJsonString(MethodInfoSchema, r)) || [],
						traces: [],
					};
					break;
				}
				default: {
					result = {
						error: "unknown message",
					};
					break;
				}
			}
		} catch (e) {
			if (currentId !== runId) {
				return;
			}
			result = {
				error: `${e}`,
			};
		}
	}
</script>

<div class="flex h-screen w-screen flex-col">
	<div class="p-2">
		<Menubar {onCodeEvent} />
	</div>

	<div class="m-2 grid flex-1 grid-cols-5 grid-rows-4 gap-5 overflow-hidden">
		<div class="col-span-3 row-span-3 flex flex-col overflow-clip rounded-md border">
			<CodeMirror {onCodeEvent} bind:this={editor} />
		</div>
		<div class="col-span-2 row-span-3 overflow-auto rounded-md border p-4">
			<form method="POST" use:enhance class="space-y-6">
				<Form.Field {form} name="arguments">
					<Form.Control let:attrs>
						<Form.Label>Arguments</Form.Label>
						<Textarea
							{...attrs}
							class="resize-none font-mono"
							rows={6}
							bind:value={$formData.arguments}
						/>
					</Form.Control>
					<Form.FieldErrors />
				</Form.Field>
				<Form.Field {form} name="method">
					<Form.Control let:attrs>
						<Form.Label>Method</Form.Label>
						<Input {...attrs} bind:value={$formData.method} />
					</Form.Control>
					<Form.FieldErrors />
				</Form.Field>
				<Form.Field {form} name="timeout">
					<Form.Control let:attrs>
						<Form.Label>Timeout</Form.Label>
						<Input {...attrs} type="number" min={0.1} step=".01" bind:value={$formData.timeout} />
					</Form.Control>
					<Form.FieldErrors />
				</Form.Field>
				<Form.Field {form} name="stream" class="flex items-center space-x-2 space-y-0">
					<Form.Control let:attrs>
						<Checkbox {...attrs} bind:checked={$formData.stream} />
						<Form.Label>Stream</Form.Label>
					</Form.Control>
				</Form.Field>
				<Form.Field {form} name="prelude">
					<Form.Control let:attrs>
						<Form.Label>Prelude</Form.Label>
						<Textarea
							{...attrs}
							class="resize-none font-mono"
							rows={6}
							bind:value={$formData.prelude}
						/>
					</Form.Control>
					<Form.FieldErrors />
				</Form.Field>

				<Form.Button>Run</Form.Button>
				<Button onclick={analyze}>Analyze</Button>
			</form>
		</div>
		<div class="col-span-5 row-span-4 overflow-auto rounded-md border p-2">
			{#if result}
				{#if "error" in result}
					<pre class="text-red-500">{result.error}</pre>
				{:else if result.loading}
					<i>Loading...</i>
				{:else}
					<div class="space-y-2 p-2">
						<div class="font-bold">Result</div>
						{#each result.data as d}
							<pre>{d}</pre>
						{/each}
						<div class="font-bold">Traces</div>
						{#each result.traces as trace}
							<pre class="text-sm">{trace}</pre>
						{/each}
					</div>
				{/if}
			{/if}
		</div>
	</div>
</div>
