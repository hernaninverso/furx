import type { ReactNode } from "react";
import { TOOL_LABELS, TOOL_URLS, type ToolName } from "@/lib/tools";

type Props = {
  name: ToolName;
  children?: ReactNode;
  className?: string;
};

export function ToolLink({ name, children, className = "text-accent hover:underline" }: Props) {
  return (
    <a href={TOOL_URLS[name]} target="_blank" rel="noopener noreferrer" className={className}>
      {children ?? TOOL_LABELS[name]}
    </a>
  );
}
