<script lang="ts">
	import * as Menubar from "$lib/components/ui/menubar";
	import { mode, setMode } from "mode-watcher";

	let { onCodeEvent } = $props<{
		onCodeEvent: (event: "run" | "save" | "reset") => void;
	}>();
</script>

<Menubar.Root>
	<Menubar.Menu>
		<Menubar.Trigger disabled>PromptKit</Menubar.Trigger>
	</Menubar.Menu>

	<Menubar.MenubarMenu>
		<Menubar.MenubarTrigger>Code</Menubar.MenubarTrigger>
		<Menubar.MenubarContent>
			<Menubar.MenubarItem onclick={onCodeEvent.bind(null, "run")}>
				Run
				<Menubar.MenubarShortcut>⌘⏎</Menubar.MenubarShortcut>
			</Menubar.MenubarItem>
			<Menubar.MenubarItem onclick={onCodeEvent.bind(null, "save")}>
				Save
				<Menubar.MenubarShortcut>⌘S</Menubar.MenubarShortcut>
			</Menubar.MenubarItem>
			<Menubar.MenubarSeparator />
			<Menubar.MenubarItem onclick={onCodeEvent.bind(null, "reset")}>Reset</Menubar.MenubarItem>
		</Menubar.MenubarContent>
	</Menubar.MenubarMenu>
	<Menubar.Menu>
		<Menubar.Trigger>Theme</Menubar.Trigger>

		<Menubar.MenubarContent>
			<Menubar.MenubarRadioGroup
				value={mode.current}
				onValueChange={(m) => {
					setMode(m !== "dark" ? "light" : "dark");
				}}
			>
				<Menubar.MenubarRadioItem value="dark">Dark</Menubar.MenubarRadioItem>
				<Menubar.MenubarRadioItem value="light">Light</Menubar.MenubarRadioItem>
			</Menubar.MenubarRadioGroup>
		</Menubar.MenubarContent>
	</Menubar.Menu>
</Menubar.Root>
