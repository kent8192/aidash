import * as React from "react";

import { cn } from "../../lib/utils";

function Kbd({ className, ...props }: React.ComponentProps<"kbd">) {
  return (
    <kbd
      className={cn(
        "inline-flex h-[18px] min-w-[18px] items-center justify-center rounded-sm border border-border-strong px-1 font-mono text-[10.5px] font-medium leading-none text-muted-foreground",
        className,
      )}
      {...props}
    />
  );
}

export { Kbd };
