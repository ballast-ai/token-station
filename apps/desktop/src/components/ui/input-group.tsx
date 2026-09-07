import * as React from "react";
import { cn } from "@/lib/utils";
import { Input } from "./input";

// The group owns the border and focus indicator. The control has no second ring.
function InputGroup({ className, ...props }: React.ComponentProps<"div">) {
  return <div data-slot="input-group" role="group"
    className={cn("relative flex w-full min-w-0 items-center rounded-lg border border-input", className)}
    {...props} />;
}

function InputGroupAddon({ className, align = "inline-start", ...props }:
  React.ComponentProps<"div"> & { align?: "inline-start" | "inline-end" }) {
  return <div data-slot="input-group-addon" data-align={align}
    className={cn("flex shrink-0 items-center justify-center text-muted-foreground [&>svg]:size-4",
      align === "inline-start" ? "order-first" : "order-last", className)}
    onClick={(event) => {
      if ((event.target as HTMLElement).closest("button")) return;
      event.currentTarget.parentElement?.querySelector("input")?.focus();
    }} {...props} />;
}

function InputGroupInput({ className, ...props }: React.ComponentProps<"input">) {
  return <Input data-slot="input-group-control"
    className={cn("flex-1 rounded-none border-0 bg-transparent shadow-none ring-0 focus-visible:ring-0", className)}
    {...props} />;
}

export { InputGroup, InputGroupAddon, InputGroupInput };
