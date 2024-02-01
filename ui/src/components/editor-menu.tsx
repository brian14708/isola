import {
  Menubar,
  MenubarContent,
  MenubarItem,
  MenubarMenu,
  MenubarRadioGroup,
  MenubarRadioItem,
  MenubarSeparator,
  MenubarShortcut,
  MenubarTrigger,
} from "@/components/ui/menubar";

import { useTheme } from "./theme-provider";

export type EditorEvent =
  | {
      type: "run" | "save" | "share";
    }
  | {
      type: "load";
      url?: string;
    };

export interface EditorMenuProps {
  onEvent?: (_: EditorEvent) => void;
}

export function EditorMenu(props: EditorMenuProps) {
  const { theme, setTheme } = useTheme();

  return (
    <Menubar>
      <MenubarMenu>
        <MenubarTrigger disabled>PromptKit</MenubarTrigger>
      </MenubarMenu>
      <MenubarMenu>
        <MenubarTrigger>Code</MenubarTrigger>
        <MenubarContent>
          <MenubarItem
            onClick={() => {
              props.onEvent?.({ type: "run" });
            }}
          >
            Run
            <MenubarShortcut>⌘⏎</MenubarShortcut>
          </MenubarItem>
          <MenubarItem
            onClick={() => {
              props.onEvent?.({ type: "save" });
            }}
          >
            Save
            <MenubarShortcut>⌘S</MenubarShortcut>
          </MenubarItem>
          <MenubarSeparator />
          <MenubarItem
            onClick={() => {
              props.onEvent?.({ type: "share" });
            }}
          >
            Share
          </MenubarItem>
          <MenubarSeparator />
          <MenubarItem
            onClick={() => {
              props.onEvent?.({ type: "load" });
            }}
          >
            Reset
          </MenubarItem>
        </MenubarContent>
      </MenubarMenu>
      <MenubarMenu>
        <MenubarTrigger>Theme</MenubarTrigger>
        <MenubarContent>
          <MenubarRadioGroup
            value={theme}
            onValueChange={(v) => setTheme(v === "dark" ? "dark" : "light")}
          >
            <MenubarRadioItem value="dark">Dark</MenubarRadioItem>
            <MenubarRadioItem value="light">Light</MenubarRadioItem>
          </MenubarRadioGroup>
        </MenubarContent>
      </MenubarMenu>
    </Menubar>
  );
}
