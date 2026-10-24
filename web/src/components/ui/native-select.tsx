import * as React from "react";
import { ChevronDown } from "lucide-react";

import { cn } from "../../lib/utils";
import { controlClass } from "./control";

/** Native <select> (kept native for platform keyboard behaviour and form semantics) with Night Ops styling. */
const NativeSelect = React.forwardRef<
  HTMLSelectElement,
  React.ComponentProps<"select"> & { wrapperClassName?: string }
>(({ className, wrapperClassName, ...props }, ref) => (
  <span className={cn("relative inline-flex w-full min-w-0", wrapperClassName)}>
    <select
      data-slot="native-select"
      className={cn(controlClass, "h-8 appearance-none pr-7", className)}
      ref={ref}
      {...props}
    />
    <ChevronDown
      aria-hidden
      className="pointer-events-none absolute right-2 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground"
    />
  </span>
));
NativeSelect.displayName = "NativeSelect";

export { NativeSelect };
