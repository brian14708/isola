import { Editor, type EditorProps } from "@monaco-editor/react";
import { cva, type VariantProps } from "class-variance-authority";

import { cn } from "@/lib/utils";
import { useTheme } from "@/components/theme-provider";

const codeEditorVariants = cva("rounded-md border bg-background text-foreground overflow-hidden", {
  variants: {
    size: {
      sm: "h-32",
      md: "h-64",
      lg: "h-96",
      xl: "h-[32rem]",
      full: "h-full",
    },
  },
  defaultVariants: {
    size: "md",
  },
});

interface CodeEditorProps
  extends Omit<EditorProps, "className">,
    VariantProps<typeof codeEditorVariants> {
  className?: string;
}

function CodeEditor({
  className,
  size,
  theme,
  language = "python",
  options = {},
  ...props
}: CodeEditorProps) {
  const { resolvedTheme } = useTheme();

  const monacoTheme = theme || (resolvedTheme === "dark" ? "vs-dark" : "vs");

  const defaultOptions = {
    minimap: { enabled: false },
    scrollBeyondLastLine: false,
    fontSize: 14,
    lineNumbers: "on" as const,
    roundedSelection: false,
    scrollbar: {
      vertical: "auto" as const,
      horizontal: "auto" as const,
    },
    automaticLayout: true,
    ...options,
  };

  return (
    <div className={cn(codeEditorVariants({ size, className }))}>
      <Editor theme={monacoTheme} language={language} options={defaultOptions} {...props} />
    </div>
  );
}

export { CodeEditor, codeEditorVariants };
export type { CodeEditorProps };
