import * as React from "react";

import { cn } from "../../lib/utils";
import { controlClass } from "./control";

const Input = React.forwardRef<HTMLInputElement, React.ComponentProps<"input">>(
  ({ className, type, ...props }, ref) => (
    <input
      data-slot="input"
      type={type}
      className={cn(
        controlClass,
        "h-8 py-1 file:border-0 file:bg-transparent file:text-[13px] file:font-medium file:text-foreground",
        className,
      )}
      ref={ref}
      {...props}
    />
  ),
);
Input.displayName = "Input";

export { Input };
