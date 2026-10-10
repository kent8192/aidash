import * as React from "react";

import { cn } from "../../lib/utils";
import { controlClass } from "./control";

const Textarea = React.forwardRef<
  HTMLTextAreaElement,
  React.ComponentProps<"textarea">
>(({ className, ...props }, ref) => (
  <textarea
    data-slot="textarea"
    className={cn(controlClass, "min-h-20 py-2 leading-relaxed", className)}
    ref={ref}
    {...props}
  />
));
Textarea.displayName = "Textarea";

export { Textarea };
