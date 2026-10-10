import * as React from "react";

import { cn } from "../../lib/utils";

function Skeleton({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      aria-hidden
      className={cn("animate-pulse rounded-md bg-raised motion-reduce:animate-none", className)}
      {...props}
    />
  );
}

export { Skeleton };
