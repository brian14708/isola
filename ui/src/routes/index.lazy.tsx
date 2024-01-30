import { ModeToggle } from "@/components/mode-toggle";
import { createLazyFileRoute } from "@tanstack/react-router";

export const Route = createLazyFileRoute("/")({
  component: () => (
    <div>
      <ModeToggle />
    </div>
  ),
});
