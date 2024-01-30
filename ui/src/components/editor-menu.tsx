import {
  Menubar,
  MenubarContent,
  MenubarItem,
  MenubarMenu,
  MenubarRadioGroup,
  MenubarRadioItem,
  MenubarTrigger,
} from "@/components/ui/menubar";
import { useTheme } from "./theme-provider";

export interface EditorMenuProps {
  onLoad?: (_: { url?: string }) => void;
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
              props.onLoad?.({});
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
